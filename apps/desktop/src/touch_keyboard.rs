//! The touch keyboard on Windows. Windows opens it on its own only for its own edit controls,
//! and GPUI's `request_virtual_keyboard` does nothing there, so a text input asks for it here
//! when it is tapped with a finger or a pen. Elsewhere these calls do nothing.

use gpui::{FocusHandle, Window};

/// Opens the touch keyboard for the input `focus` if the current mouse press came from a
/// touch screen or a pen. Returns whether the input now owns the keyboard.
pub fn show_for_tap(window: &Window, focus: &FocusHandle) -> bool {
    imp::show_for_tap(window, focus)
}

/// Hides the touch keyboard if the input `focus` opened it and no other input took it over.
pub fn release(focus: &FocusHandle) {
    imp::release(focus)
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;

    use gpui::{FocusHandle, WeakFocusHandle, Window};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::UI::ViewManagement::InputPane;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::IInputPaneInterop;
    use windows::Win32::UI::WindowsAndMessaging::GetMessageExtraInfo;

    /// Mouse messages Windows makes from touch and pen input carry this in their extra info.
    const MI_WP_SIGNATURE: isize = 0xFF51_5700;
    const SIGNATURE_MASK: isize = 0xFFFF_FF00;

    thread_local! {
        /// The input that opened the keyboard and the window it is in.
        static OWNER: RefCell<Option<(WeakFocusHandle, isize)>> = const { RefCell::new(None) };
    }

    pub fn show_for_tap(window: &Window, focus: &FocusHandle) -> bool {
        // GPUI handles a mouse message while it is being dispatched, so this is the press: a
        // finger's tap is made by `touch`, a pen's press comes marked by Windows.
        let extra = unsafe { GetMessageExtraInfo() }.0;
        if !crate::touch::tapping() && extra & SIGNATURE_MASK != MI_WP_SIGNATURE {
            return false;
        }
        // The trait's method: `Window` has an inherent `window_handle` too.
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return false;
        };
        let hwnd = handle.hwnd.get();
        if input_pane(hwnd).and_then(|pane| pane.TryShow()).is_err() {
            return false;
        }
        OWNER.set(Some((focus.downgrade(), hwnd)));
        true
    }

    pub fn release(focus: &FocusHandle) {
        let hwnd = OWNER.with_borrow_mut(|owner| {
            let (handle, hwnd) = owner.as_ref()?;
            if handle != focus {
                return None;
            }
            let hwnd = *hwnd;
            *owner = None;
            Some(hwnd)
        });
        if let Some(hwnd) = hwnd {
            input_pane(hwnd).and_then(|pane| pane.TryHide()).ok();
        }
    }

    fn input_pane(hwnd: isize) -> windows::core::Result<InputPane> {
        let interop = windows::core::factory::<InputPane, IInputPaneInterop>()?;
        unsafe { interop.GetForWindow(HWND(hwnd as _)) }
    }
}

#[cfg(not(windows))]
mod imp {
    use gpui::{FocusHandle, Window};

    pub fn show_for_tap(_: &Window, _: &FocusHandle) -> bool {
        false
    }

    pub fn release(_: &FocusHandle) {}
}
