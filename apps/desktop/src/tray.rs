//! Windows notification icon for the main window while it is minimized to the tray.

use std::cell::Cell;

use fern_core::Store;
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    DefSubclassProc, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, RemoveWindowSubclass,
    SetWindowSubclass, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDI_APPLICATION, IsIconic, IsWindowVisible, LoadIconW, PostMessageW, RegisterWindowMessageW,
    SIZE_MINIMIZED, SW_HIDE, SW_RESTORE, SetForegroundWindow, ShowWindow, WM_APP, WM_LBUTTONUP,
    WM_NCDESTROY, WM_SIZE,
};
use windows::core::{PCWSTR, w};

const SETTING: &str = "minimize_to_tray";
const SUBCLASS_ID: usize = 0xFE7B;
const TRAY_INPUT: u32 = WM_APP + 1;
const HIDE_MINIMIZED: u32 = WM_APP + 2;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

#[derive(Clone, Copy)]
struct State {
    enabled: bool,
    in_tray: bool,
    taskbar_created: u32,
}

thread_local! {
    // Only the main window installs this subclass. Its messages run on the UI thread.
    static STATE: Cell<Option<State>> = const { Cell::new(None) };
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

pub fn init(window: &Window, store: &Store) {
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
    STATE.set(Some(State {
        enabled: enabled(store),
        in_tray: false,
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
    }));
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

fn add_icon(hwnd: HWND) -> bool {
    // build.rs embeds Fern's icon as resource 1. LoadIconW returns a shared handle.
    let icon = unsafe {
        GetModuleHandleW(PCWSTR::null())
            .and_then(|module| LoadIconW(Some(HINSTANCE(module.0)), PCWSTR(1 as *const u16)))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
    };
    let Ok(icon) = icon else { return false };
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
    data.uCallbackMessage = TRAY_INPUT;
    data.hIcon = icon;
    for (target, character) in data.szTip.iter_mut().zip("Fern".encode_utf16()) {
        *target = character;
    }
    if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        return false;
    }
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    let _ = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
    true
}

fn restore(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
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
            WM_SIZE if wparam.0 == SIZE_MINIMIZED as usize && state.enabled && !state.in_tray => {
                // The caption button and keyboard both trigger WM_SIZE. Leave GPUI's size
                // handling in place and hide the minimized window after it returns.
                let _ = unsafe { PostMessageW(Some(hwnd), HIDE_MINIMIZED, WPARAM(0), LPARAM(0)) };
            }
            HIDE_MINIMIZED => {
                if state.enabled
                    && !state.in_tray
                    && unsafe { IsIconic(hwnd).as_bool() && IsWindowVisible(hwnd).as_bool() }
                    && add_icon(hwnd)
                {
                    state.in_tray = true;
                    STATE.set(Some(state));
                    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
                }
                return LRESULT(0);
            }
            TRAY_INPUT if state.in_tray => {
                // Version 4 carries the event in the low word. Mouse-up also works with
                // older notification behavior if setting the version above failed.
                let event = lparam.0 as u32 & 0xffff;
                if matches!(event, WM_LBUTTONUP | NIN_SELECT | NIN_KEYSELECT) {
                    restore(hwnd);
                }
                return LRESULT(0);
            }
            WM_SIZE if state.in_tray && !unsafe { IsIconic(hwnd).as_bool() } => {
                state.in_tray = false;
                STATE.set(Some(state));
                let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd)) };
            }
            WM_NCDESTROY => {
                STATE.set(None);
                if state.in_tray {
                    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd)) };
                }
                let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            }
            msg if state.in_tray && state.taskbar_created != 0 && msg == state.taskbar_created => {
                if !add_icon(hwnd) {
                    // If Explorer restarts and the icon cannot be recreated, make Fern reachable.
                    restore(hwnd);
                }
            }
            _ => {}
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
