//! Windows notification area (system tray) icon, context menu, always-on-top, and minimize-to-tray.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use fern_core::Store;
use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedSender;
use gpui::{App, Window, WindowHandle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    DefSubclassProc, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, RemoveWindowSubclass,
    SetWindowSubclass, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HWND_NOTOPMOST, HWND_TOPMOST,
    IDI_APPLICATION, IsIconic, IsWindowVisible, LoadIconW, MF_CHECKED, MF_POPUP, MF_SEPARATOR,
    MF_STRING, MF_UNCHECKED, PostMessageW, RegisterWindowMessageW, SIZE_MINIMIZED, SW_HIDE,
    SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow,
    SetWindowPos, ShowWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NCDESTROY, WM_NULL, WM_RBUTTONUP, WM_SIZE,
};
use windows::core::{PCWSTR, w};

use crate::app_view::AppView;
use crate::i18n::{self, CurrentLocale, Strings};

const SETTING: &str = "minimize_to_tray";
const TOPMOST_SETTING: &str = "always_on_top";
const SUBCLASS_ID: usize = 0xFE7B;
const TRAY_INPUT: u32 = WM_APP + 1;
const HIDE_MINIMIZED: u32 = WM_APP + 2;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

const CMD_START_POMO: usize = 1001;
const CMD_START_STOPWATCH: usize = 1002;
const CMD_ALWAYS_ON_TOP: usize = 1003;
const CMD_QUIT: usize = 1004;

#[derive(Clone, Copy)]
struct State {
    hwnd: HWND,
    enabled: bool,
    hidden_in_tray: bool,
    icon_added: bool,
    v4: bool,
    always_on_top: bool,
    quitting: bool,
    taskbar_created: u32,
}

#[derive(Clone, Copy)]
struct TrayLabels {
    focus: &'static str,
    start_pomo: &'static str,
    stopwatch: &'static str,
    always_on_top: &'static str,
    quit: &'static str,
}

impl TrayLabels {
    fn from_strings(strings: &'static Strings) -> Self {
        Self {
            focus: strings.tray_focus,
            start_pomo: strings.tray_start_pomo,
            stopwatch: strings.tray_stopwatch,
            always_on_top: strings.tray_always_on_top,
            quit: strings.tray_quit,
        }
    }
}

enum TrayAction {
    StartPomo,
    StartStopwatch,
    ToggleAlwaysOnTop,
    Quit,
}

thread_local! {
    // Only the main window installs this subclass. Its messages run on the UI thread.
    static STATE: Cell<Option<State>> = const { Cell::new(None) };
    static LABELS: RefCell<TrayLabels> = RefCell::new(TrayLabels::from_strings(i18n::Locale::En.strings()));
    static SENDER: RefCell<Option<UnboundedSender<TrayAction>>> = const { RefCell::new(None) };
}

pub fn enabled(store: &Store) -> bool {
    store.setting(SETTING).ok().flatten().as_deref() == Some("1")
}

pub fn set_enabled(store: &Store, enabled: bool) -> anyhow::Result<()> {
    store.set_setting(SETTING, if enabled { "1" } else { "0" })?;
    if let Some(mut state) = STATE.get() {
        state.enabled = enabled;
        STATE.set(Some(state));
    }
    Ok(())
}

pub fn always_on_top(store: &Store) -> bool {
    store.setting(TOPMOST_SETTING).ok().flatten().as_deref() == Some("1")
}

pub fn set_always_on_top(store: &Store, topmost: bool) -> anyhow::Result<()> {
    store.set_setting(TOPMOST_SETTING, if topmost { "1" } else { "0" })?;
    if let Some(mut state) = STATE.get() {
        state.always_on_top = topmost;
        STATE.set(Some(state));
        apply_topmost(state.hwnd, topmost);
    }
    Ok(())
}

/// Hides the main window into the system tray instead of closing it.
/// Returns `false` when the tray is not initialized (e.g. in tests) or when quitting.
pub fn hide_to_tray() -> bool {
    let Some(mut state) = STATE.get() else {
        return false;
    };
    if state.quitting {
        return false;
    }
    if !state.icon_added {
        let (added, v4) = add_icon(state.hwnd);
        state.icon_added = added;
        state.v4 = v4;
    }
    if !state.icon_added {
        return false;
    }
    state.hidden_in_tray = true;
    STATE.set(Some(state));
    let _ = unsafe { ShowWindow(state.hwnd, SW_HIDE) };
    true
}

/// Marks the app as quitting and removes the tray icon so the window can close cleanly.
pub fn prepare_quit() {
    if let Some(mut state) = STATE.get() {
        state.quitting = true;
        STATE.set(Some(state));
    }
    remove_icon();
}

/// Restores the main window from the system tray (or minimized state) and brings it to front.
pub fn restore_main_window() -> bool {
    let Some(mut state) = STATE.get() else {
        return false;
    };
    state.hidden_in_tray = false;
    STATE.set(Some(state));
    restore(state.hwnd);
    true
}

pub fn update_labels(strings: &'static Strings) {
    LABELS.with(|labels| *labels.borrow_mut() = TrayLabels::from_strings(strings));
}

pub fn init(window: &Window, store: &Store, cx: &App) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as _);
    if !unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) }.as_bool() {
        eprintln!("failed to install the notification area handler");
        return;
    }
    update_labels(i18n::t(cx));
    let topmost = always_on_top(store);
    if topmost {
        apply_topmost(hwnd, true);
    }
    let (icon_added, v4) = add_icon(hwnd);
    STATE.set(Some(State {
        hwnd,
        enabled: enabled(store),
        hidden_in_tray: false,
        icon_added,
        v4,
        always_on_top: topmost,
        quitting: false,
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
    }));
}

pub fn connect(main_window: WindowHandle<AppView>, store: Rc<Store>, cx: &mut App) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    SENDER.with(|slot| *slot.borrow_mut() = Some(tx));
    cx.observe_global::<CurrentLocale>(|cx| update_labels(i18n::t(cx))).detach();
    cx.spawn(async move |cx| {
        while let Some(action) = rx.next().await {
            match action {
                TrayAction::StartPomo => {
                    let _ = main_window.update(cx, |view, _, cx| view.start_focus_from_tray(false, cx));
                }
                TrayAction::StartStopwatch => {
                    let _ = main_window.update(cx, |view, _, cx| view.start_focus_from_tray(true, cx));
                }
                TrayAction::ToggleAlwaysOnTop => {
                    let next = !always_on_top(&store);
                    if let Err(err) = set_always_on_top(&store, next) {
                        eprintln!("failed to save always_on_top setting: {err:#}");
                    }
                }
                TrayAction::Quit => {
                    cx.update(|cx| {
                        prepare_quit();
                        cx.quit();
                    });
                    break;
                }
            }
        }
    })
    .detach();
}

fn apply_topmost(hwnd: HWND, topmost: bool) {
    let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    if let Err(err) =
        unsafe { SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }
    {
        eprintln!("failed to update always-on-top: {err}");
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

/// Adds the tray icon and returns `(added, version_4_active)`.
fn add_icon(hwnd: HWND) -> (bool, bool) {
    // build.rs embeds Fern's icon as resource 1. LoadIconW returns a shared handle.
    let icon = unsafe {
        GetModuleHandleW(PCWSTR::null())
            .and_then(|module| LoadIconW(Some(HINSTANCE(module.0)), PCWSTR(std::ptr::without_provenance(1))))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
    };
    let Ok(icon) = icon else { return (false, false) };
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
    data.uCallbackMessage = TRAY_INPUT;
    data.hIcon = icon;
    for (target, character) in data.szTip.iter_mut().zip("Fern".encode_utf16()) {
        *target = character;
    }
    if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        return (false, false);
    }
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    let v4 = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) }.as_bool();
    (true, v4)
}

fn remove_icon() {
    if let Some(mut state) = STATE.get()
        && state.icon_added
    {
        state.icon_added = false;
        STATE.set(Some(state));
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &icon_data(state.hwnd)) };
    }
}

fn restore(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() || !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn show_context_menu(hwnd: HWND, state: State) {
    let labels = LABELS.with(|l| *l.borrow());
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else { return };
    let Ok(focus_menu) = (unsafe { CreatePopupMenu() }) else {
        let _ = unsafe { DestroyMenu(menu) };
        return;
    };

    let start_pomo = to_wide(labels.start_pomo);
    let stopwatch = to_wide(labels.stopwatch);
    let focus = to_wide(labels.focus);
    let always_on_top = to_wide(labels.always_on_top);
    let quit = to_wide(labels.quit);

    unsafe {
        let _ = AppendMenuW(focus_menu, MF_STRING, CMD_START_POMO, PCWSTR(start_pomo.as_ptr()));
        let _ = AppendMenuW(focus_menu, MF_STRING, CMD_START_STOPWATCH, PCWSTR(stopwatch.as_ptr()));
        if AppendMenuW(menu, MF_POPUP, focus_menu.0 as usize, PCWSTR(focus.as_ptr())).is_err() {
            let _ = DestroyMenu(focus_menu);
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let topmost_flag = if state.always_on_top { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | topmost_flag, CMD_ALWAYS_ON_TOP, PCWSTR(always_on_top.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, PCWSTR(quit.as_ptr()));
    }

    let mut pt = POINT::default();
    let _ = unsafe { GetCursorPos(&mut pt) };
    // Required by TrackPopupMenu so clicking outside the menu dismisses it immediately.
    let _ = unsafe { SetForegroundWindow(hwnd) };
    let selected = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        )
    };
    let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
    let _ = unsafe { DestroyMenu(menu) };

    let action = match selected.0 as usize {
        CMD_START_POMO => Some(TrayAction::StartPomo),
        CMD_START_STOPWATCH => Some(TrayAction::StartStopwatch),
        CMD_ALWAYS_ON_TOP => Some(TrayAction::ToggleAlwaysOnTop),
        CMD_QUIT => Some(TrayAction::Quit),
        _ => None,
    };
    if let Some(action) = action {
        SENDER.with(|slot| {
            if let Some(tx) = slot.borrow().as_ref() {
                let _ = tx.unbounded_send(action);
            }
        });
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if let Some(mut state) = STATE.get() {
        match message {
            WM_CLOSE if !state.quitting => {
                if !state.icon_added {
                    let (added, v4) = add_icon(hwnd);
                    state.icon_added = added;
                    state.v4 = v4;
                }
                if state.icon_added {
                    state.hidden_in_tray = true;
                    STATE.set(Some(state));
                    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
                    return LRESULT(0);
                }
            }
            WM_SIZE if wparam.0 == SIZE_MINIMIZED as usize && state.enabled && !state.hidden_in_tray => {
                // The caption button and keyboard both trigger WM_SIZE. Leave GPUI's size
                // handling in place and hide the minimized window after it returns.
                let _ = unsafe { PostMessageW(Some(hwnd), HIDE_MINIMIZED, WPARAM(0), LPARAM(0)) };
            }
            HIDE_MINIMIZED => {
                if state.enabled
                    && !state.hidden_in_tray
                    && unsafe { IsIconic(hwnd).as_bool() && IsWindowVisible(hwnd).as_bool() }
                {
                    if !state.icon_added {
                        let (added, v4) = add_icon(hwnd);
                        state.icon_added = added;
                        state.v4 = v4;
                    }
                    if state.icon_added {
                        state.hidden_in_tray = true;
                        STATE.set(Some(state));
                        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
                    }
                }
                return LRESULT(0);
            }
            TRAY_INPUT if state.icon_added => {
                // Version 4 carries the event in the low word.
                let event = lparam.0 as u32 & 0xffff;
                let is_select = if state.v4 {
                    matches!(event, NIN_SELECT | NIN_KEYSELECT)
                } else {
                    event == WM_LBUTTONUP
                };
                let is_context = if state.v4 {
                    event == WM_CONTEXTMENU
                } else {
                    event == WM_RBUTTONUP
                };
                if is_select {
                    state.hidden_in_tray = false;
                    STATE.set(Some(state));
                    restore(hwnd);
                } else if is_context {
                    show_context_menu(hwnd, state);
                }
                return LRESULT(0);
            }
            WM_SIZE if state.hidden_in_tray && !unsafe { IsIconic(hwnd).as_bool() } => {
                state.hidden_in_tray = false;
                STATE.set(Some(state));
            }
            WM_NCDESTROY => {
                STATE.set(None);
                SENDER.with(|slot| *slot.borrow_mut() = None);
                if state.icon_added {
                    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd)) };
                }
                let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            }
            msg if state.taskbar_created != 0 && msg == state.taskbar_created => {
                let (added, v4) = add_icon(hwnd);
                state.icon_added = added;
                state.v4 = v4;
                if !added && state.hidden_in_tray {
                    // If Explorer restarts and the icon cannot be recreated, make Fern reachable.
                    state.hidden_in_tray = false;
                    STATE.set(Some(state));
                    restore(hwnd);
                } else {
                    STATE.set(Some(state));
                }
            }
            _ => {}
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
