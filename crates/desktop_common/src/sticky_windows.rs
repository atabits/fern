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
            with_x11_client(|client| {
                x11_set_wm_states(
                    client,
                    window_id,
                    client.atoms.net_wm_state_skip_taskbar,
                    client.atoms.net_wm_state_skip_pager,
                    hidden,
                )
            });
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
    let _mtm = objc2::MainThreadMarker::new()?;
    let view_ptr = handle.ns_view.as_ptr().cast::<objc2_app_kit::NSView>();
    // SAFETY: MainThreadMarker proves we are on the main thread, and GPUI guarantees the AppKit
    // handle points to a live NSView.
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

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct X11Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[cfg(any(target_os = "linux", test))]
impl X11Rect {
    fn contains(self, px: i32, py: i32) -> bool {
        self.w > 0
            && self.h > 0
            && px >= self.x
            && px < self.x.saturating_add(self.w)
            && py >= self.y
            && py < self.y.saturating_add(self.h)
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.x.saturating_add(self.w).min(other.x.saturating_add(other.w));
        let bottom = self.y.saturating_add(self.h).min(other.y.saturating_add(other.h));
        if right > left && bottom > top {
            Some(Self { x: left, y: top, w: right - left, h: bottom - top })
        } else {
            None
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn x11_select_monitor(
    monitors: &[(X11Rect, bool)],
    center_x: i32,
    center_y: i32,
    fallback: X11Rect,
) -> X11Rect {
    monitors
        .iter()
        .find(|(rect, _)| rect.contains(center_x, center_y))
        .or_else(|| monitors.iter().find(|(rect, primary)| *primary && rect.w > 0 && rect.h > 0))
        .or_else(|| monitors.iter().find(|(rect, _)| rect.w > 0 && rect.h > 0))
        .map(|(rect, _)| *rect)
        .unwrap_or(fallback)
}

#[cfg(any(target_os = "linux", test))]
fn x11_desktop_workarea(workareas: &[u32], desktop: usize) -> Option<X11Rect> {
    let slice = desktop
        .checked_mul(4)
        .and_then(|start| workareas.get(start..start.checked_add(4)?))
        .or_else(|| workareas.get(..4))?;
    let rect = X11Rect {
        x: slice[0] as i32,
        y: slice[1] as i32,
        w: slice[2] as i32,
        h: slice[3] as i32,
    };
    (rect.w > 0 && rect.h > 0).then_some(rect)
}

#[cfg(any(target_os = "linux", test))]
fn x11_snap_position(work: X11Rect, cur_x: i32, cur_y: i32, win_w: i32, edge: ScreenEdge) -> (i32, i32) {
    match edge {
        ScreenEdge::Top => (cur_x, work.y),
        ScreenEdge::Left => (work.x, cur_y),
        ScreenEdge::Right => (work.x + work.w - win_w, cur_y),
    }
}

#[cfg(any(target_os = "linux", test))]
fn x11_merge_wm_states(states: &mut Vec<u32>, atom1: u32, atom2: u32, enabled: bool) {
    for atom in [atom1, atom2].into_iter().filter(|&a| a != 0) {
        if enabled {
            if !states.contains(&atom) {
                states.push(atom);
            }
        } else {
            states.retain(|&existing| existing != atom);
        }
    }
}

#[cfg(target_os = "linux")]
struct X11Atoms {
    net_wm_state: u32,
    net_wm_state_above: u32,
    net_wm_state_skip_taskbar: u32,
    net_wm_state_skip_pager: u32,
    net_wm_window_opacity: u32,
    net_workarea: u32,
    net_current_desktop: u32,
    net_active_window: u32,
}

#[cfg(target_os = "linux")]
struct X11Client {
    conn: x11rb::rust_connection::RustConnection,
    screen_num: usize,
    root: u32,
    atoms: X11Atoms,
}

#[cfg(target_os = "linux")]
impl X11Client {
    fn connect() -> Option<Self> {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::ConnectionExt as _;
        use x11rb::rust_connection::RustConnection;

        let (conn, screen_num) = RustConnection::connect(None).ok()?;
        let root = conn.setup().roots.get(screen_num)?.root;

        let c_wm_state = conn.intern_atom(false, b"_NET_WM_STATE").ok()?;
        let c_above = conn.intern_atom(false, b"_NET_WM_STATE_ABOVE").ok()?;
        let c_skip_taskbar = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_TASKBAR").ok()?;
        let c_skip_pager = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_PAGER").ok()?;
        let c_opacity = conn.intern_atom(false, b"_NET_WM_WINDOW_OPACITY").ok()?;
        let c_workarea = conn.intern_atom(false, b"_NET_WORKAREA").ok()?;
        let c_current_desktop = conn.intern_atom(false, b"_NET_CURRENT_DESKTOP").ok()?;
        let c_active_window = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW").ok()?;

        let atoms = X11Atoms {
            net_wm_state: c_wm_state.reply().ok()?.atom,
            net_wm_state_above: c_above.reply().ok()?.atom,
            net_wm_state_skip_taskbar: c_skip_taskbar.reply().ok()?.atom,
            net_wm_state_skip_pager: c_skip_pager.reply().ok()?.atom,
            net_wm_window_opacity: c_opacity.reply().ok()?.atom,
            net_workarea: c_workarea.reply().ok()?.atom,
            net_current_desktop: c_current_desktop.reply().ok()?.atom,
            net_active_window: c_active_window.reply().ok()?.atom,
        };
        Some(Self { conn, screen_num, root, atoms })
    }
}

#[cfg(target_os = "linux")]
static X11_CLIENT: std::sync::Mutex<Option<X11Client>> = std::sync::Mutex::new(None);

#[cfg(target_os = "linux")]
fn with_x11_client<T>(
    mut op: impl FnMut(&X11Client) -> Result<T, x11rb::errors::ConnectionError>,
) -> Option<T> {
    let mut guard = X11_CLIENT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for _ in 0..2 {
        if guard.is_none() {
            *guard = X11Client::connect();
        }
        let client = guard.as_ref()?;
        match op(client) {
            Ok(value) => return Some(value),
            Err(_) => *guard = None,
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn x11_reply<R>(
    cookie_res: Result<
        x11rb::cookie::Cookie<'_, x11rb::rust_connection::RustConnection, R>,
        x11rb::errors::ConnectionError,
    >,
) -> Result<Option<R>, x11rb::errors::ConnectionError>
where
    R: x11rb::x11_utils::TryParse,
{
    match cookie_res?.reply() {
        Ok(reply) => Ok(Some(reply)),
        Err(x11rb::errors::ReplyError::X11Error(_)) => Ok(None),
        Err(x11rb::errors::ReplyError::ConnectionError(err)) => Err(err),
    }
}

#[cfg(target_os = "linux")]
fn x11_set_wm_states(
    client: &X11Client,
    window_id: u32,
    atom1: u32,
    atom2: u32,
    enabled: bool,
) -> Result<(), x11rb::errors::ConnectionError> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, MapState, PropMode,
    };
    use x11rb::wrapper::ConnectionExt as _;

    // If the window is still unmapped (e.g. during `open_window`), update `_NET_WM_STATE`
    // directly so the WM picks it up on initial map rather than racing with `MapWindow`.
    if let Some(attrs) = x11_reply(client.conn.get_window_attributes(window_id))?
        && attrs.map_state == MapState::UNMAPPED
    {
        let mut states: Vec<u32> = x11_reply(client.conn.get_property(
            false,
            window_id,
            client.atoms.net_wm_state,
            AtomEnum::ATOM,
            0,
            64,
        ))?
        .and_then(|reply| reply.value32().map(Iterator::collect))
        .unwrap_or_default();

        x11_merge_wm_states(&mut states, atom1, atom2, enabled);

        if states.is_empty() {
            client.conn.delete_property(window_id, client.atoms.net_wm_state)?.ignore_error();
        } else {
            client
                .conn
                .change_property32(
                    PropMode::REPLACE,
                    window_id,
                    client.atoms.net_wm_state,
                    AtomEnum::ATOM,
                    &states,
                )?
                .ignore_error();
        }
    }

    let action: u32 = if enabled { 1 } else { 0 }; // _NET_WM_STATE_ADD / _NET_WM_STATE_REMOVE
    let event = ClientMessageEvent::new(32, window_id, client.atoms.net_wm_state, [action, atom1, atom2, 1, 0]);
    client
        .conn
        .send_event(
            false,
            client.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )?
        .ignore_error();
    client.conn.flush()
}

#[cfg(target_os = "linux")]
pub fn x11_set_topmost(window_id: u32, topmost: bool) {
    with_x11_client(|client| {
        x11_set_wm_states(client, window_id, client.atoms.net_wm_state_above, 0, topmost)
    });
}

#[cfg(target_os = "linux")]
fn x11_set_opacity(window_id: u32, opacity: u8) {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
    use x11rb::wrapper::ConnectionExt as _;

    with_x11_client(|client| {
        if opacity >= 100 {
            client
                .conn
                .delete_property(window_id, client.atoms.net_wm_window_opacity)?
                .ignore_error();
        } else {
            let value = ((u64::from(opacity) * u64::from(u32::MAX)) / 100) as u32;
            client
                .conn
                .change_property32(
                    PropMode::REPLACE,
                    window_id,
                    client.atoms.net_wm_window_opacity,
                    AtomEnum::CARDINAL,
                    &[value],
                )?
                .ignore_error();
        }
        client.conn.flush()
    });
}

#[cfg(target_os = "linux")]
fn x11_align_to_edge(window_id: u32, edge: ScreenEdge) {
    use x11rb::connection::Connection as _;
    use x11rb::errors::{ConnectionError, ReplyError};
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{AtomEnum, ConfigureWindowAux, ConnectionExt as _};

    with_x11_client(|client| {
        let Some(geom) = x11_reply(client.conn.get_geometry(window_id))? else {
            return Ok(());
        };
        let Some(coords) = x11_reply(client.conn.translate_coordinates(window_id, client.root, 0, 0))? else {
            return Ok(());
        };

        let cur_x = i32::from(coords.dst_x);
        let cur_y = i32::from(coords.dst_y);
        let win_w = i32::from(geom.width);
        let win_h = i32::from(geom.height);
        let center_x = cur_x + win_w / 2;
        let center_y = cur_y + win_h / 2;

        let screen = &client.conn.setup().roots[client.screen_num];
        let fallback = X11Rect {
            x: 0,
            y: 0,
            w: i32::from(screen.width_in_pixels),
            h: i32::from(screen.height_in_pixels),
        };

        let monitors: Vec<(X11Rect, bool)> = match client.conn.randr_get_monitors(client.root, true) {
            Ok(cookie) => match cookie.reply() {
                Ok(reply) => reply
                    .monitors
                    .into_iter()
                    .map(|m| {
                        (
                            X11Rect {
                                x: i32::from(m.x),
                                y: i32::from(m.y),
                                w: i32::from(m.width),
                                h: i32::from(m.height),
                            },
                            m.primary,
                        )
                    })
                    .collect(),
                Err(ReplyError::X11Error(_)) => Vec::new(),
                Err(ReplyError::ConnectionError(err)) => return Err(err),
            },
            Err(ConnectionError::UnsupportedExtension) => Vec::new(),
            Err(err) => return Err(err),
        };
        let monitor = x11_select_monitor(&monitors, center_x, center_y, fallback);

        let desktop = x11_reply(client.conn.get_property(
            false,
            client.root,
            client.atoms.net_current_desktop,
            AtomEnum::CARDINAL,
            0,
            1,
        ))?
        .and_then(|reply| reply.value32()?.next())
        .unwrap_or(0) as usize;

        let workareas: Vec<u32> = x11_reply(client.conn.get_property(
            false,
            client.root,
            client.atoms.net_workarea,
            AtomEnum::CARDINAL,
            0,
            1024,
        ))?
        .and_then(|reply| reply.value32().map(Iterator::collect))
        .unwrap_or_default();

        let work = x11_desktop_workarea(&workareas, desktop)
            .and_then(|wa| monitor.intersect(wa))
            .unwrap_or(monitor);

        let (target_x, target_y) = x11_snap_position(work, cur_x, cur_y, win_w, edge);
        let aux = ConfigureWindowAux::new().x(target_x).y(target_y);
        client.conn.configure_window(window_id, &aux)?.ignore_error();
        client.conn.flush()
    });
}

#[cfg(target_os = "linux")]
pub fn x11_hide_window(window_id: u32) -> bool {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{
        ConnectionExt as _, EventMask, UNMAP_NOTIFY_EVENT, UnmapNotifyEvent,
    };

    with_x11_client(|client| {
        client.conn.unmap_window(window_id)?.ignore_error();
        // ICCCM §4.1.4 (matches XWithdrawWindow): notify reparenting WMs even when iconic.
        let event = UnmapNotifyEvent {
            response_type: UNMAP_NOTIFY_EVENT,
            sequence: 0,
            event: client.root,
            window: window_id,
            from_configure: false,
        };
        client
            .conn
            .send_event(
                false,
                client.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )?
            .ignore_error();
        client.conn.flush()?;
        Ok(true)
    })
    .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn x11_show_window(window_id: u32) -> bool {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask};

    with_x11_client(|client| {
        client.conn.map_window(window_id)?.ignore_error();
        let event = ClientMessageEvent::new(
            32,
            window_id,
            client.atoms.net_active_window,
            [1u32, 0, 0, 0, 0],
        );
        client
            .conn
            .send_event(
                false,
                client.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )?
            .ignore_error();
        client.conn.flush()?;
        Ok(true)
    })
    .unwrap_or(false)
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

    #[test]
    fn x11_multi_monitor_workarea_intersection_and_snapping() {
        let left_mon = X11Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let right_mon = X11Rect { x: 1920, y: 0, w: 2560, h: 1440 };
        let monitors = [(left_mon, true), (right_mon, false)];
        let fallback = X11Rect { x: 0, y: 0, w: 4480, h: 1440 };

        // Window centered on right monitor picks right monitor, not virtual desktop bounding box.
        let picked = x11_select_monitor(&monitors, 2300, 400, fallback);
        assert_eq!(picked, right_mon);

        // Off-screen center falls back to primary monitor; empty list falls back to root screen.
        assert_eq!(x11_select_monitor(&monitors, -500, -500, fallback), left_mon);
        assert_eq!(x11_select_monitor(&[], 100, 100, fallback), fallback);

        // Desktop 1 workarea indexed via `desktop * 4 .. desktop * 4 + 4`.
        let workareas = [
            0, 28, 4480, 1052, // desktop 0
            64, 32, 4416, 1368, // desktop 1 (left dock 64px, top panel 32px)
        ];
        let wa1 = x11_desktop_workarea(&workareas, 1).unwrap();
        assert_eq!(wa1, X11Rect { x: 64, y: 32, w: 4416, h: 1368 });
        // Out-of-range desktop falls back to desktop 0 entry.
        assert_eq!(x11_desktop_workarea(&workareas, 5), Some(X11Rect { x: 0, y: 28, w: 4480, h: 1052 }));

        // Intersect right monitor with desktop 1 workarea and snap edges inside right monitor.
        let right_work = right_mon.intersect(wa1).unwrap();
        assert_eq!(right_work, X11Rect { x: 1920, y: 32, w: 2560, h: 1368 });
        assert_eq!(x11_snap_position(right_work, 2200, 300, 320, ScreenEdge::Top), (2200, 32));
        assert_eq!(x11_snap_position(right_work, 2200, 300, 320, ScreenEdge::Left), (1920, 300));
        assert_eq!(x11_snap_position(right_work, 2200, 300, 320, ScreenEdge::Right), (4160, 300));

        // Left monitor respects left dock at x=64.
        let left_work = left_mon.intersect(wa1).unwrap();
        assert_eq!(left_work, X11Rect { x: 64, y: 32, w: 1856, h: 1048 });
        assert_eq!(x11_snap_position(left_work, 500, 200, 320, ScreenEdge::Left), (64, 200));
        assert_eq!(x11_snap_position(left_work, 500, 200, 320, ScreenEdge::Right), (1600, 200));
    }

    #[test]
    fn x11_merge_wm_states_updates_atoms_without_duplicates() {
        let mut states = vec![10];
        x11_merge_wm_states(&mut states, 20, 30, true);
        assert_eq!(states, vec![10, 20, 30]);
        // Adding again does not duplicate; zero atom is ignored.
        x11_merge_wm_states(&mut states, 20, 0, true);
        assert_eq!(states, vec![10, 20, 30]);
        // Removing clears both specified atoms while preserving unrelated states.
        x11_merge_wm_states(&mut states, 20, 30, false);
        assert_eq!(states, vec![10]);
        x11_merge_wm_states(&mut states, 10, 0, false);
        assert!(states.is_empty());
    }
}
