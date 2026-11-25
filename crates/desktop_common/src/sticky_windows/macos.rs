use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSFloatingWindowLevel, NSNormalWindowLevel, NSScreen, NSView, NSWindow};
use objc2_foundation::NSPoint;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::ScreenEdge;

pub fn macos_ns_window(window: &gpui::Window) -> Option<Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { return None };
    let _mtm = MainThreadMarker::new()?;
    let view_ptr = handle.ns_view.as_ptr().cast::<NSView>();
    // SAFETY: MainThreadMarker proves we are on the main thread, and GPUI guarantees the AppKit
    // handle points to a live NSView.
    let view = unsafe { &*view_ptr };
    view.window()
}

pub(super) fn set_topmost(window: &gpui::Window, topmost: bool) {
    if let Some(ns_window) = macos_ns_window(window) {
        ns_window.setLevel(if topmost { NSFloatingWindowLevel } else { NSNormalWindowLevel });
    }
}

pub(super) fn set_opacity(window: &gpui::Window, clamped: u8) {
    if let Some(ns_window) = macos_ns_window(window) {
        ns_window.setAlphaValue(f64::from(clamped) / 100.0);
    }
}

pub(super) fn align_to_edge(window: &gpui::Window, edge: ScreenEdge) {
    let Some(ns_window) = macos_ns_window(window) else { return };
    let screen = ns_window
        .screen()
        .or_else(|| MainThreadMarker::new().and_then(NSScreen::mainScreen));
    let Some(screen) = screen else { return };
    let work = screen.visibleFrame();
    let frame = ns_window.frame();
    // macOS screen coordinates have their origin at the bottom-left.
    let (x, y) = match edge {
        ScreenEdge::Top => (frame.origin.x, work.origin.y + work.size.height - frame.size.height),
        ScreenEdge::Left => (work.origin.x, frame.origin.y),
        ScreenEdge::Right => (work.origin.x + work.size.width - frame.size.width, frame.origin.y),
    };
    ns_window.setFrameOrigin(NSPoint::new(x, y));
}
