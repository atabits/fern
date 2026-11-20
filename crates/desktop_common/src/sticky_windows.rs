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

/// Keeps the window over the other windows, or puts it back among them: the original's pin.
#[cfg(windows)]
pub fn set_topmost(window: &gpui::Window, topmost: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };

    let Some(hwnd) = win32_window_handle(window) else { return };
    let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    if let Err(err) =
        unsafe { SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }
    {
        eprintln!("failed to pin the sticky note: {err}");
    }
}

#[cfg(windows)]
pub fn win32_window_handle(window: &gpui::Window) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return None };
    Some(windows::Win32::Foundation::HWND(handle.hwnd.get() as _))
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
