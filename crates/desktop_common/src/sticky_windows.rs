//! Device-local window state. Kept outside synced preferences because monitor layouts differ.

use fern_core::{Id, Store};
use gpui::{App, Bounds, Pixels, point, px, size};
use serde::{Deserialize, Serialize};

pub const MIN_WIDTH: f32 = 240.;
pub const MIN_EXPANDED_HEIGHT: f32 = 200.;
pub const COMPACT_HEIGHT: f32 = 24.;

#[cfg(windows)]
#[path = "sticky_windows/windows.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "sticky_windows/macos.rs"]
mod platform;

#[cfg(any(target_os = "linux", test))]
#[path = "sticky_windows/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod platform {
    use super::ScreenEdge;
    pub(super) fn set_topmost(_window: &gpui::Window, _topmost: bool) {}
    pub(super) fn set_opacity(_window: &gpui::Window, _clamped: u8) {}
    pub(super) fn align_to_edge(_window: &gpui::Window, _edge: ScreenEdge) {}
}

#[cfg(windows)]
pub use platform::win32_window_handle;
#[cfg(target_os = "macos")]
pub use platform::macos_ns_window;
#[cfg(target_os = "linux")]
pub use linux::{x11_hide_window, x11_set_topmost, x11_show_window, x11_window_id};

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
        Self {
            task,
            open: true,
            pinned,
            x: bounds.origin.x.into(),
            y: bounds.origin.y.into(),
            width: bounds.size.width.into(),
            height: bounds.size.height.into(),
            color: None,
            compact: false,
        }
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
        if self.x.is_finite()
            && self.y.is_finite()
            && cx.displays().iter().any(|display| {
                let screen = display.bounds();
                candidate.origin.x >= screen.origin.x
                    && candidate.origin.y >= screen.origin.y
                    && candidate.origin.x + px(120.) <= screen.right()
                    && candidate.origin.y + px(COMPACT_HEIGHT) <= screen.bottom()
            })
        {
            candidate
        } else {
            Bounds::centered(None, size, cx)
        }
    }
}

pub fn load(store: &Store) -> Vec<NoteWindow> {
    store
        .setting("local_sticky_windows")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
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
    if let Some(note) = notes.iter_mut().find(|n| n.task == task) {
        note.open = false;
    }
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
    platform::set_topmost(window, topmost);
}

/// Sets whole-window opacity (`10..=100` percent) including the native surface.
pub fn set_opacity(window: &gpui::Window, opacity: u8) {
    let clamped = opacity.clamp(10, 100);
    platform::set_opacity(window, clamped);
}

/// Snaps the window to an edge of its display's work area without changing its size.
pub fn align_to_edge(window: &gpui::Window, edge: ScreenEdge) {
    platform::align_to_edge(window, edge);
}

/// Hints to the window manager to hide or show the window in the taskbar / pager (Linux X11).
/// Windows sticky notes use a dedicated hidden-owner taskbar helper in `sticky_ui`.
pub fn set_skip_taskbar(window: &gpui::Window, hidden: bool) {
    #[cfg(target_os = "linux")]
    platform::set_skip_taskbar(window, hidden);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (window, hidden);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linux::{X11Rect, x11_desktop_workarea, x11_merge_wm_states, x11_select_monitor, x11_snap_position};

    #[test]
    fn note_lifecycle_keeps_geometry_and_replaces_navigated_task() {
        let store = Store::open_in_memory().unwrap();
        let first = store.add_task("First").unwrap().id;
        let next = store.add_task("Next").unwrap().id;
        let mut note = NoteWindow::capture(
            first,
            true,
            Bounds::new(point(px(-500.), px(20.)), size(px(480.), px(600.))),
        );
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

        let picked = x11_select_monitor(&monitors, 2300, 400, fallback);
        assert_eq!(picked, right_mon);

        assert_eq!(x11_select_monitor(&monitors, -500, -500, fallback), left_mon);
        assert_eq!(x11_select_monitor(&[], 100, 100, fallback), fallback);

        let workareas = [0, 28, 4480, 1052, 64, 32, 4416, 1368];
        let wa1 = x11_desktop_workarea(&workareas, 1).unwrap();
        assert_eq!(wa1, X11Rect { x: 64, y: 32, w: 4416, h: 1368 });
        assert_eq!(x11_desktop_workarea(&workareas, 5), Some(X11Rect { x: 0, y: 28, w: 4480, h: 1052 }));

        let right_work = right_mon.intersect(wa1).unwrap();
        assert_eq!(right_work, X11Rect { x: 1920, y: 32, w: 2560, h: 1368 });
        assert_eq!(x11_snap_position(right_work, 2200, 300, 320, ScreenEdge::Top), (2200, 32));
        assert_eq!(x11_snap_position(right_work, 2200, 300, 320, ScreenEdge::Left), (1920, 300));
        assert_eq!(x11_snap_position(right_work, 2200, 300, 320, ScreenEdge::Right), (4160, 300));

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
        x11_merge_wm_states(&mut states, 20, 0, true);
        assert_eq!(states, vec![10, 20, 30]);
        x11_merge_wm_states(&mut states, 20, 30, false);
        assert_eq!(states, vec![10]);
        x11_merge_wm_states(&mut states, 10, 0, false);
        assert!(states.is_empty());
    }
}
