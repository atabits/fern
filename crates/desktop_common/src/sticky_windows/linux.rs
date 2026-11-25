use super::ScreenEdge;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct X11Rect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) w: i32,
    pub(super) h: i32,
}

impl X11Rect {
    fn contains(self, px: i32, py: i32) -> bool {
        self.w > 0
            && self.h > 0
            && px >= self.x
            && px < self.x.saturating_add(self.w)
            && py >= self.y
            && py < self.y.saturating_add(self.h)
    }

    pub(super) fn intersect(self, other: Self) -> Option<Self> {
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

pub(super) fn x11_select_monitor(
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

pub(super) fn x11_desktop_workarea(workareas: &[u32], desktop: usize) -> Option<X11Rect> {
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

pub(super) fn x11_snap_position(
    work: X11Rect,
    cur_x: i32,
    cur_y: i32,
    win_w: i32,
    edge: ScreenEdge,
) -> (i32, i32) {
    match edge {
        ScreenEdge::Top => (cur_x, work.y),
        ScreenEdge::Left => (work.x, cur_y),
        ScreenEdge::Right => (work.x + work.w - win_w, cur_y),
    }
}

pub(super) fn x11_merge_wm_states(states: &mut Vec<u32>, atom1: u32, atom2: u32, enabled: bool) {
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
mod x11 {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::connection::Connection as _;
    use x11rb::errors::{ConnectionError, ReplyError};
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, MapState,
        PropMode, UNMAP_NOTIFY_EVENT, UnmapNotifyEvent,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    use super::{
        ScreenEdge, X11Rect, x11_desktop_workarea, x11_merge_wm_states, x11_select_monitor,
        x11_snap_position,
    };

    pub fn x11_window_id(window: &gpui::Window) -> Option<u32> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::Xcb(h) => Some(h.window.get()),
            RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok().filter(|&id| id != 0),
            _ => None,
        }
    }

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

    struct X11Client {
        conn: RustConnection,
        screen_num: usize,
        root: u32,
        atoms: X11Atoms,
    }

    impl X11Client {
        fn connect() -> Option<Self> {
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

    static X11_CLIENT: std::sync::Mutex<Option<X11Client>> = std::sync::Mutex::new(None);

    fn with_x11_client<T>(
        mut op: impl FnMut(&X11Client) -> Result<T, ConnectionError>,
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

    fn x11_reply<R>(
        cookie_res: Result<x11rb::cookie::Cookie<'_, RustConnection, R>, ConnectionError>,
    ) -> Result<Option<R>, ConnectionError>
    where
        R: x11rb::x11_utils::TryParse,
    {
        match cookie_res?.reply() {
            Ok(reply) => Ok(Some(reply)),
            Err(ReplyError::X11Error(_)) => Ok(None),
            Err(ReplyError::ConnectionError(err)) => Err(err),
        }
    }

    fn x11_set_wm_states(
        client: &X11Client,
        window_id: u32,
        atom1: u32,
        atom2: u32,
        enabled: bool,
    ) -> Result<(), ConnectionError> {
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

        let action: u32 = if enabled { 1 } else { 0 };
        let event =
            ClientMessageEvent::new(32, window_id, client.atoms.net_wm_state, [action, atom1, atom2, 1, 0]);
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

    pub fn x11_set_topmost(window_id: u32, topmost: bool) {
        with_x11_client(|client| {
            x11_set_wm_states(client, window_id, client.atoms.net_wm_state_above, 0, topmost)
        });
    }

    pub(in super::super) fn set_topmost(window: &gpui::Window, topmost: bool) {
        if let Some(window_id) = x11_window_id(window) {
            x11_set_topmost(window_id, topmost);
        }
    }

    pub(in super::super) fn set_opacity(window: &gpui::Window, clamped: u8) {
        let Some(window_id) = x11_window_id(window) else { return };
        with_x11_client(|client| {
            if clamped >= 100 {
                client
                    .conn
                    .delete_property(window_id, client.atoms.net_wm_window_opacity)?
                    .ignore_error();
            } else {
                let value = ((u64::from(clamped) * u64::from(u32::MAX)) / 100) as u32;
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

    pub(in super::super) fn align_to_edge(window: &gpui::Window, edge: ScreenEdge) {
        let Some(window_id) = x11_window_id(window) else { return };
        with_x11_client(|client| {
            let Some(geom) = x11_reply(client.conn.get_geometry(window_id))? else {
                return Ok(());
            };
            let Some(coords) = x11_reply(client.conn.translate_coordinates(window_id, client.root, 0, 0))?
            else {
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

    pub(in super::super) fn set_skip_taskbar(window: &gpui::Window, hidden: bool) {
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

    pub fn x11_hide_window(window_id: u32) -> bool {
        with_x11_client(|client| {
            client.conn.unmap_window(window_id)?.ignore_error();
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

    pub fn x11_show_window(window_id: u32) -> bool {
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
}

#[cfg(target_os = "linux")]
pub use x11::{x11_hide_window, x11_set_topmost, x11_show_window, x11_window_id};
#[cfg(target_os = "linux")]
pub(super) use x11::{align_to_edge, set_opacity, set_skip_taskbar, set_topmost};
