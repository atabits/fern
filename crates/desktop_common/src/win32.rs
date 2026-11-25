//! Shared Win32 window handle helpers for GPUI windows.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;

/// Extracts the Win32 `HWND` backing a GPUI window, if available.
pub fn hwnd_from_window(window: &gpui::Window) -> Option<HWND> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(HWND(handle.hwnd.get() as _))
}

/// Extracts the raw `HWND` value as `isize` for map keying or WinRT interop.
pub fn hwnd_isize(window: &gpui::Window) -> Option<isize> {
    Some(hwnd_from_window(window)?.0 as isize)
}
