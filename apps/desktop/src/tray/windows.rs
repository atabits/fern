//! Windows notification area (system tray) backend using `Shell_NotifyIconW`.

use std::cell::{Cell, RefCell};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use gpui::Window;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    DefSubclassProc, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
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

use super::{TrayAction, TrayLabels, TrayPrefs};
use crate::i18n;

const SUBCLASS_ID: usize = 0xFE7B;
const TRAY_INPUT: u32 = WM_APP + 1;
const HIDE_MINIMIZED: u32 = WM_APP + 2;
const OPEN_CONTEXT_MENU: u32 = WM_APP + 3;
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
    menu_open: bool,
    taskbar_created: u32,
    activate_instance: u32,
}

thread_local! {
    // Only the main window installs this subclass. Its messages run on the UI thread.
    static STATE: Cell<Option<State>> = const { Cell::new(None) };
    static LABELS: RefCell<TrayLabels> = RefCell::new(TrayLabels::from_strings(i18n::Locale::En.strings()));
    static SENDER: RefCell<Option<UnboundedSender<TrayAction>>> = const { RefCell::new(None) };
    static LAST_TIP: RefCell<String> = const { RefCell::new(String::new()) };
}

fn clear_tip_cache() {
    LAST_TIP.with(|last| last.borrow_mut().clear());
}

pub(super) fn set_tooltip(tip: &str) {
    // Check icon state before mutating LAST_TIP so early calls cannot poison the cache.
    let Some(state) = STATE.get() else {
        return;
    };
    if !state.icon_added {
        return;
    }
    let unchanged = LAST_TIP.with(|last| *last.borrow() == tip);
    if unchanged {
        return;
    }
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: state.hwnd,
        uID: 1,
        uFlags: NIF_TIP | NIF_SHOWTIP,
        ..Default::default()
    };
    let max = data.szTip.len() - 1;
    for (slot, unit) in data.szTip.iter_mut().take(max).zip(tip.encode_utf16()) {
        *slot = unit;
    }
    if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
        LAST_TIP.with(|last| {
            let mut last = last.borrow_mut();
            last.clear();
            last.push_str(tip);
        });
    }
}

pub(super) fn set_enabled(is_enabled: bool) {
    if let Some(mut state) = STATE.get() {
        state.enabled = is_enabled;
        STATE.set(Some(state));
    }
}

pub(super) fn set_always_on_top(topmost: bool) {
    if let Some(mut state) = STATE.get() {
        state.always_on_top = topmost;
        STATE.set(Some(state));
        apply_topmost(state.hwnd, topmost);
    }
}

pub(super) fn hide_to_tray() -> bool {
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

pub(super) fn hide_to_tray_if_enabled() -> bool {
    let Some(state) = STATE.get() else {
        return false;
    };
    if !state.enabled || state.quitting {
        return false;
    }
    hide_to_tray()
}

pub(super) fn prepare_quit() {
    if let Some(mut state) = STATE.get() {
        state.quitting = true;
        STATE.set(Some(state));
    }
    remove_icon();
}

pub(super) fn restore_main_window() -> bool {
    let Some(mut state) = STATE.get() else {
        return false;
    };
    state.hidden_in_tray = false;
    STATE.set(Some(state));
    restore(state.hwnd);
    true
}

pub(super) fn update_labels(labels: TrayLabels) {
    LABELS.with(|slot| *slot.borrow_mut() = labels);
}

pub(super) fn prepare_channel() -> UnboundedReceiver<TrayAction> {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    SENDER.with(|slot| *slot.borrow_mut() = Some(tx));
    rx
}

pub(super) fn init(window: &Window, prefs: TrayPrefs, labels: TrayLabels) {
    let Some(hwnd) = desktop_common::win32::hwnd_from_window(window) else {
        return;
    };
    if !unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) }.as_bool() {
        desktop_common::logging::log_error("tray", "failed to install the notification area handler");
        return;
    }
    update_labels(labels);
    if prefs.always_on_top {
        apply_topmost(hwnd, true);
    }
    let (icon_added, v4) = add_icon(hwnd);
    if let Ok(dir) = crate::data_dir() {
        crate::single_instance::register_window(&dir, hwnd);
    }
    STATE.set(Some(State {
        hwnd,
        enabled: prefs.minimize_to_tray,
        hidden_in_tray: false,
        icon_added,
        v4,
        always_on_top: prefs.always_on_top,
        quitting: false,
        menu_open: false,
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        activate_instance: crate::single_instance::activate_message(),
    }));
}

fn apply_topmost(hwnd: HWND, topmost: bool) {
    let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    if let Err(err) =
        unsafe { SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }
    {
        desktop_common::logging::log_error("tray", format!("failed to update always-on-top: {err}"));
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

fn add_icon(hwnd: HWND) -> (bool, bool) {
    clear_tip_cache();
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
    clear_tip_cache();
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

fn show_context_menu(hwnd: HWND, is_always_on_top: bool) {
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
        let topmost_flag = if is_always_on_top { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | topmost_flag, CMD_ALWAYS_ON_TOP, PCWSTR(always_on_top.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, PCWSTR(quit.as_ptr()));
    }

    let mut pt = POINT::default();
    let _ = unsafe { GetCursorPos(&mut pt) };
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
        send_action(action);
    }
}

fn send_action(action: TrayAction) {
    SENDER.with(|slot| {
        if let Some(tx) = slot.borrow().as_ref() {
            let _ = tx.unbounded_send(action);
        }
    });
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
            OPEN_CONTEXT_MENU => {
                if !state.menu_open && !state.quitting {
                    state.menu_open = true;
                    let topmost = state.always_on_top;
                    STATE.set(Some(state));
                    show_context_menu(hwnd, topmost);
                    if let Some(mut latest) = STATE.get() {
                        latest.menu_open = false;
                        STATE.set(Some(latest));
                    }
                }
                return LRESULT(0);
            }
            TRAY_INPUT if state.icon_added => {
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
                } else if is_context && !state.menu_open {
                    let _ = unsafe { PostMessageW(Some(hwnd), OPEN_CONTEXT_MENU, WPARAM(0), LPARAM(0)) };
                }
                return LRESULT(0);
            }
            WM_SIZE if state.hidden_in_tray && !unsafe { IsIconic(hwnd).as_bool() } => {
                state.hidden_in_tray = false;
                STATE.set(Some(state));
            }
            WM_NCDESTROY => {
                STATE.set(None);
                clear_tip_cache();
                SENDER.with(|slot| *slot.borrow_mut() = None);
                if state.icon_added {
                    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd)) };
                }
                let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            }
            msg if state.taskbar_created != 0 && msg == state.taskbar_created => {
                clear_tip_cache();
                let (added, v4) = add_icon(hwnd);
                state.icon_added = added;
                state.v4 = v4;
                if !added && state.hidden_in_tray {
                    state.hidden_in_tray = false;
                    STATE.set(Some(state));
                    restore(hwnd);
                } else {
                    STATE.set(Some(state));
                }
            }
            msg if state.activate_instance != 0 && msg == state.activate_instance => {
                state.hidden_in_tray = false;
                STATE.set(Some(state));
                restore(hwnd);
                send_action(TrayAction::Activate);
                return LRESULT(0);
            }
            _ => {}
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
