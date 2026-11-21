//! Device-local window state. Kept outside synced preferences because monitor layouts differ.
use fern_core::{Id, Store};
use gpui::{App, Bounds, Pixels, point, px, size};
use serde::{Deserialize, Serialize};

pub const MIN_WIDTH: f32 = 240.;
pub const MIN_EXPANDED_HEIGHT: f32 = 200.;
pub const COMPACT_HEIGHT: f32 = 24.;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteWindow {
    pub task: Id,
    pub open: bool,
    pub pinned: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub color: Option<usize>,
    #[serde(default)]
    pub compact: bool,
}

impl NoteWindow {
    pub fn capture(task: Id, pinned: bool, bounds: Bounds<Pixels>) -> Self {
        Self { task, open: true, pinned, x: bounds.origin.x.into(), y: bounds.origin.y.into(),
            width: bounds.size.width.into(), height: bounds.size.height.into(), color: None, compact: false }
    }

    pub fn expanded_height(&self) -> Pixels {
        px(if self.height.is_finite() { self.height.clamp(MIN_EXPANDED_HEIGHT, 4096.) } else { 360. })
    }

    pub fn bounds(&self, cx: &App) -> Bounds<Pixels> {
        let width = px(if self.width.is_finite() { self.width.clamp(MIN_WIDTH, 4096.) } else { 320. });
        let height = if self.compact { px(COMPACT_HEIGHT) } else { self.expanded_height() };
        let size = size(width, height);
        let candidate = Bounds::new(point(px(self.x), px(self.y)), size);
        // Keep the entire drag strip reachable after unplugging a monitor or changing scaling.
        if self.x.is_finite() && self.y.is_finite() && cx.displays().iter().any(|display| {
            let screen = display.bounds();
            candidate.origin.x >= screen.origin.x && candidate.origin.y >= screen.origin.y
                && candidate.origin.x + px(120.) <= screen.right()
                && candidate.origin.y + px(COMPACT_HEIGHT) <= screen.bottom()
        }) { candidate } else { Bounds::centered(None, size, cx) }
    }
}

pub fn load(store: &Store) -> Vec<NoteWindow> {
    store.setting("local_sticky_windows").ok().flatten()
        .and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(store: &Store, note: NoteWindow, previous: Option<Id>) -> anyhow::Result<()> {
    let mut notes = load(store);
    if previous.is_none_or(|id| id == note.task) && notes.iter().find(|n| n.task == note.task) == Some(&note) {
        return Ok(());
    }
    notes.retain(|n| n.task != note.task && Some(n.task) != previous);
    notes.push(note);
    store.set_setting("local_sticky_windows", &serde_json::to_string(&notes)?)
}

pub fn close(store: &Store, task: Id) -> anyhow::Result<()> {
    let mut notes = load(store);
    if let Some(note) = notes.iter_mut().find(|n| n.task == task) { note.open = false; }
    store.set_setting("local_sticky_windows", &serde_json::to_string(&notes)?)
}

/// Screen edge target for snapping floating sticky notes without resizing them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenEdge {
    Top,
    Left,
    Right,
}

/// Keeps the window over other normal windows, or returns it to the normal window level.
pub fn set_topmost(window: &gpui::Window, topmost: bool) {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
        };

        let Some(hwnd) = win32_window_handle(window) else { return };
        let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
        if let Err(err) =
            unsafe { SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }
        {
            eprintln!("failed to pin the window: {err}");
        }
    }
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSFloatingWindowLevel, NSNormalWindowLevel};
        if let Some(ns_window) = macos_ns_window(window) {
            ns_window.setLevel(if topmost { NSFloatingWindowLevel } else { NSNormalWindowLevel });
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(window_id) = x11_window_id(window) {
            x11_set_topmost(window_id, topmost);
        }
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (window, topmost);
    }
}

/// Sets whole-window opacity (`10..=100` percent) including the native surface.
pub fn set_opacity(window: &gpui::Window, opacity: u8) {
    let clamped = opacity.clamp(10, 100);
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SetLayeredWindowAttributes, SetWindowLongPtrW,
            WS_EX_LAYERED,
        };
        let Some(hwnd) = win32_window_handle(window) else { return };
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
            let alpha = (u16::from(clamped) * 255 / 100) as u8;
            if let Err(err) =
                SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), alpha, LWA_ALPHA)
            {
                eprintln!("failed to set window opacity: {err}");
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(ns_window) = macos_ns_window(window) {
            ns_window.setAlphaValue(f64::from(clamped) / 100.0);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(window_id) = x11_window_id(window) {
            x11_set_opacity(window_id, clamped);
        }
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (window, clamped);
    }
}

/// Snaps the window to an edge of its display's work area without changing its size.
pub fn align_to_edge(window: &gpui::Window, edge: ScreenEdge) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowRect, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
        };
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
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSScreen;
        use objc2_foundation::NSPoint;
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
    #[cfg(target_os = "linux")]
    {
        if let Some(window_id) = x11_window_id(window) {
            x11_align_to_edge(window_id, edge);
        }
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (window, edge);
    }
}

/// Hints to the window manager to hide or show the window in the taskbar / pager (Linux X11).
/// Windows sticky notes use a dedicated hidden-owner taskbar helper in `sticky_ui`.
pub fn set_skip_taskbar(window: &gpui::Window, hidden: bool) {
    #[cfg(target_os = "linux")]
    {
        if let Some(window_id) = x11_window_id(window) {
            x11_set_wm_state(window_id, b"_NET_WM_STATE_SKIP_TASKBAR", hidden);
            x11_set_wm_state(window_id, b"_NET_WM_STATE_SKIP_PAGER", hidden);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (window, hidden);
    }
}

#[cfg(windows)]
pub fn win32_window_handle(window: &gpui::Window) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return None };
    Some(windows::Win32::Foundation::HWND(handle.hwnd.get() as _))
}

#[cfg(target_os = "macos")]
pub fn macos_ns_window(window: &gpui::Window) -> Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { return None };
    let view_ptr = handle.ns_view.as_ptr().cast::<objc2_app_kit::NSView>();
    // SAFETY: GPUI guarantees the AppKit handle points to a live NSView on the main thread.
    let view = unsafe { &*view_ptr };
    view.window()
}

#[cfg(target_os = "linux")]
pub fn x11_window_id(window: &gpui::Window) -> Option<u32> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok().filter(|&id| id != 0),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn x11_set_wm_state(window_id: u32, state_atom_name: &[u8], enabled: bool) {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask};
    use x11rb::rust_connection::RustConnection;

    let Ok((conn, screen_num)) = RustConnection::connect(None) else { return };
    let root = conn.setup().roots[screen_num].root;
    let Some(wm_state) = conn.intern_atom(false, b"_NET_WM_STATE").ok().and_then(|c| c.reply().ok()) else {
        return;
    };
    let Some(state_atom) = conn.intern_atom(false, state_atom_name).ok().and_then(|c| c.reply().ok()) else {
        return;
    };
    let action: u32 = if enabled { 1 } else { 0 }; // _NET_WM_STATE_ADD / _NET_WM_STATE_REMOVE
    let event = ClientMessageEvent::new(32, window_id, wm_state.atom, [action, state_atom.atom, 0, 1, 0]);
    let _ = conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    );
    let _ = conn.flush();
}

#[cfg(target_os = "linux")]
pub fn x11_set_topmost(window_id: u32, topmost: bool) {
    x11_set_wm_state(window_id, b"_NET_WM_STATE_ABOVE", topmost);
}

#[cfg(target_os = "linux")]
fn x11_set_opacity(window_id: u32, opacity: u8) {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    let Ok((conn, _)) = RustConnection::connect(None) else { return };
    let Some(atom) = conn.intern_atom(false, b"_NET_WM_WINDOW_OPACITY").ok().and_then(|c| c.reply().ok()) else {
        return;
    };
    if opacity >= 100 {
        let _ = conn.delete_property(window_id, atom.atom);
    } else {
        let value = ((u64::from(opacity) * u64::from(u32::MAX)) / 100) as u32;
        let _ = conn.change_property32(PropMode::REPLACE, window_id, atom.atom, AtomEnum::CARDINAL, &[value]);
    }
    let _ = conn.flush();
}

#[cfg(target_os = "linux")]
fn x11_align_to_edge(window_id: u32, edge: ScreenEdge) {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ConfigureWindowAux, ConnectionExt as _};
    use x11rb::rust_connection::RustConnection;

    let Ok((conn, screen_num)) = RustConnection::connect(None) else { return };
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let Some(geom) = conn.get_geometry(window_id).ok().and_then(|c| c.reply().ok()) else { return };
    let Some(coords) = conn.translate_coordinates(window_id, root, 0, 0).ok().and_then(|c| c.reply().ok()) else {
        return;
    };
    let (work_x, work_y, work_w) = conn
        .intern_atom(false, b"_NET_WORKAREA")
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|atom| {
            conn.get_property(false, root, atom.atom, AtomEnum::CARDINAL, 0, 4)
                .ok()
                .and_then(|c| c.reply().ok())
        })
        .and_then(|reply| {
            let mut vals = reply.value32()?;
            let x = vals.next()? as i32;
            let y = vals.next()? as i32;
            let w = vals.next()? as i32;
            Some((x, y, w))
        })
        .unwrap_or((0, 0, i32::from(screen.width_in_pixels)));

    let cur_x = i32::from(coords.dst_x);
    let cur_y = i32::from(coords.dst_y);
    let win_w = i32::from(geom.width);
    let aux = match edge {
        ScreenEdge::Top => ConfigureWindowAux::new().x(cur_x).y(work_y),
        ScreenEdge::Left => ConfigureWindowAux::new().x(work_x).y(cur_y),
        ScreenEdge::Right => ConfigureWindowAux::new().x(work_x + work_w - win_w).y(cur_y),
    };
    let _ = conn.configure_window(window_id, &aux);
    let _ = conn.flush();
}

#[cfg(target_os = "linux")]
pub fn x11_hide_window(window_id: u32) -> bool {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    let Ok((conn, _)) = RustConnection::connect(None) else { return false };
    if conn.unmap_window(window_id).is_err() {
        return false;
    }
    conn.flush().is_ok()
}

#[cfg(target_os = "linux")]
pub fn x11_show_window(window_id: u32) -> bool {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask};
    use x11rb::rust_connection::RustConnection;

    let Ok((conn, screen_num)) = RustConnection::connect(None) else { return false };
    let root = conn.setup().roots[screen_num].root;
    let _ = conn.map_window(window_id);
    if let Some(active_atom) = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW").ok().and_then(|c| c.reply().ok()) {
        let event = ClientMessageEvent::new(32, window_id, active_atom.atom, [1u32, 0, 0, 0, 0]);
        let _ = conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        );
    }
    conn.flush().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn note_lifecycle_keeps_geometry_and_replaces_navigated_task() {
        let store = Store::open_in_memory().unwrap();
        let first = store.add_task("First").unwrap().id;
        let next = store.add_task("Next").unwrap().id;
        let mut note = NoteWindow::capture(first, true, Bounds::new(point(px(-500.), px(20.)), size(px(480.), px(600.))));
        save(&store, note.clone(), None).unwrap();
        close(&store, first).unwrap();
        assert!(!load(&store)[0].open);
        assert_eq!(load(&store)[0].width, 480.);
        save(&store, note.clone(), None).unwrap();
        note.task = next;
        save(&store, note.clone(), Some(first)).unwrap();
        assert_eq!(load(&store), [note]);
        store.set_setting("local_sticky_windows", "broken").unwrap();
        assert!(load(&store).is_empty());
    }
}
