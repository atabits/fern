use std::rc::Rc;

use fern_core::{Id, Store};
use gpui::{App, Global};

/// Callback type for opening a sticky note window for a task.
pub type StickyOpenFn = Rc<dyn Fn(Rc<Store>, Id, &mut App)>;

/// Global callback registered at startup so feature crates can open a sticky note without depending
/// directly on `sticky_ui`.
pub struct StickyOpener(pub StickyOpenFn);

impl Global for StickyOpener {}

pub fn open_sticky_note(store: Rc<Store>, id: Id, cx: &mut App) {
    if let Some(opener) = cx.try_global::<StickyOpener>().map(|global| global.0.clone()) {
        opener(store, id, cx);
    }
}
