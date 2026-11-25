//! Native sizing and caption gestures for sticky notes on Windows.

use gpui::Window;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    HTCAPTION, MINMAXINFO, WM_GETMINMAXINFO, WM_NCDESTROY, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN,
};

use super::{COMPACT_HEIGHT, MIN_EXPANDED_HEIGHT, win32_window_handle};

const SUBCLASS_ID: usize = 0xFE7D;

pub(super) fn set_compact(window: &Window, compact: bool) {
    let Some(hwnd) = win32_window_handle(window) else {
        return;
    };
    if !install(hwnd, compact) {
        desktop_common::logging::log_error("sticky_ui", "failed to update sticky note size constraints");
    }
}

fn install(hwnd: HWND, compact: bool) -> bool {
    // Updating the same subclass replaces its per-window mode; no GPUI entity or
    // App borrow is needed from the native sizing callback.
    unsafe {
        SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, usize::from(compact)).as_bool()
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    compact: usize,
) -> LRESULT {
    if message == WM_NCLBUTTONDBLCLK && wparam.0 == HTCAPTION as usize {
        // GPUI counts both messages as a press. Forward an ordinary caption press
        // so Windows can still drag the note but only Fern handles the double-click.
        return unsafe { DefSubclassProc(hwnd, WM_NCLBUTTONDOWN, wparam, lparam) };
    }
    if message == WM_NCDESTROY {
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
    }
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if message == WM_GETMINMAXINFO && compact == 0 {
        let Some(limits) = (unsafe { (lparam.0 as *mut MINMAXINFO).as_mut() }) else {
            return result;
        };
        // GPUI has already included the native frame in the compact minimum from
        // WindowOptions. Keep that calculation, width and maximum constraints;
        // only add the extra client height required by the expanded editor.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let scale = if dpi == 0 { 1. } else { dpi as f32 / 96. };
        let compact_height = (COMPACT_HEIGHT * scale).floor() as i32;
        let expanded_height = (MIN_EXPANDED_HEIGHT * scale).ceil() as i32;
        limits.ptMinTrackSize.y += expanded_height - compact_height;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowRect, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
        SendMessageW, SetWindowPos, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::w;

    struct TestWindow(HWND);

    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) }.unwrap();
        }
    }

    fn limits(hwnd: HWND) -> MINMAXINFO {
        let mut limits = MINMAXINFO::default();
        unsafe {
            SendMessageW(
                hwnd,
                WM_GETMINMAXINFO,
                None,
                Some(LPARAM(&mut limits as *mut _ as isize)),
            );
        }
        limits
    }

    unsafe extern "system" fn platform_minimum(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _: usize,
        _: usize,
    ) -> LRESULT {
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        if message == WM_GETMINMAXINFO
            && let Some(limits) = (unsafe { (lparam.0 as *mut MINMAXINFO).as_mut() })
        {
            // GPUI writes its WindowOptions minimum here. Supply a fixed base
            // to verify that our callback extends, rather than replaces it.
            limits.ptMinTrackSize.x = 360;
            limits.ptMinTrackSize.y = 48;
        }
        result
    }

    #[test]
    fn native_resize_stops_at_the_minimum_and_compact_mode_restores_its_limit() {
        // This private test window is never shown and does not use the user's data.
        let window = TestWindow(
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("Fern sizing test"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    640,
                    480,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .unwrap(),
        );
        let hwnd = window.0;
        assert!(unsafe { SetWindowSubclass(hwnd, Some(platform_minimum), 0, 0) }.as_bool());
        let base = limits(hwnd);
        assert!(install(hwnd, false));
        let expanded = limits(hwnd);
        assert!(expanded.ptMinTrackSize.y > base.ptMinTrackSize.y);
        assert_eq!(expanded.ptMinTrackSize.x, base.ptMinTrackSize.x);
        assert_eq!(expanded.ptMaxTrackSize, base.ptMaxTrackSize);

        // Windows clamps the proposed size in the original resize, without a
        // second corrective resize from a GPUI bounds observer.
        unsafe {
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                640,
                1,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
            )
            .unwrap();
        }
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }.unwrap();
        assert_eq!(rect.bottom - rect.top, expanded.ptMinTrackSize.y);

        assert!(install(hwnd, true));
        assert_eq!(limits(hwnd).ptMinTrackSize, base.ptMinTrackSize);
        assert!(install(hwnd, false));
        assert_eq!(limits(hwnd).ptMinTrackSize, expanded.ptMinTrackSize);
        assert!(install(hwnd, false));
        assert_eq!(
            limits(hwnd).ptMinTrackSize,
            expanded.ptMinTrackSize,
            "updating the mode must not stack callbacks"
        );

        // Sending WM_GETMINMAXINFO with a null LPARAM must not dereference null.
        unsafe {
            SendMessageW(hwnd, WM_GETMINMAXINFO, None, Some(LPARAM(0)));
        }
    }
}
