use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, HWND_NOTOPMOST, HWND_TOPMOST, LWA_ALPHA,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, WS_EX_LAYERED,
};

use super::ScreenEdge;
use crate::logging::log_error;

pub fn win32_window_handle(window: &gpui::Window) -> Option<HWND> {
    crate::win32::hwnd_from_window(window)
}

pub(super) fn set_topmost(window: &gpui::Window, topmost: bool) {
    let Some(hwnd) = win32_window_handle(window) else { return };
    let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    if let Err(err) =
        unsafe { SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }
    {
        log_error("sticky_windows", format!("failed to pin the window: {err}"));
    }
}

pub(super) fn set_opacity(window: &gpui::Window, clamped: u8) {
    let Some(hwnd) = win32_window_handle(window) else { return };
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
        let alpha = (u16::from(clamped) * 255 / 100) as u8;
        if let Err(err) = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA) {
            log_error("sticky_windows", format!("failed to set window opacity: {err}"));
        }
    }
}

pub(super) fn align_to_edge(window: &gpui::Window, edge: ScreenEdge) {
    let Some(hwnd) = win32_window_handle(window) else { return };
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return;
    }
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return;
    }
    let work = info.rcWork;
    let width = rect.right - rect.left;
    let (x, y) = match edge {
        ScreenEdge::Top => (rect.left, work.top),
        ScreenEdge::Left => (work.left, rect.top),
        ScreenEdge::Right => (work.right - width, rect.top),
    };
    let _ = unsafe { SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
}
