//! Device-local window state. Kept outside synced preferences because monitor layouts differ.
use fern_core::{Id, Store};
use gpui::{App, Bounds, Pixels, point, px, size};
use serde::{Deserialize, Serialize};

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
}

impl NoteWindow {
    pub fn capture(task: Id, pinned: bool, bounds: Bounds<Pixels>) -> Self {
        Self { task, open: true, pinned, x: bounds.origin.x.into(), y: bounds.origin.y.into(),
            width: bounds.size.width.into(), height: bounds.size.height.into(), color: None }
    }

    pub fn bounds(&self, cx: &App) -> Bounds<Pixels> {
        let width = if self.width.is_finite() { self.width.clamp(240., 4096.) } else { 320. };
        let height = if self.height.is_finite() { self.height.clamp(200., 4096.) } else { 360. };
        let candidate = Bounds::new(point(px(self.x), px(self.y)), size(px(width), px(height)));
        // Keep the entire drag strip reachable after unplugging a monitor or changing scaling.
        if self.x.is_finite() && self.y.is_finite() && cx.displays().iter().any(|display| {
            let screen = display.bounds();
            candidate.origin.x >= screen.origin.x && candidate.origin.y >= screen.origin.y
                && candidate.origin.x + px(120.) <= screen.right()
                && candidate.origin.y + px(28.) <= screen.bottom()
        }) { candidate } else { Bounds::centered(None, size(px(width), px(height)), cx) }
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
