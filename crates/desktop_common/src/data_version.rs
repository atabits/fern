use gpui::{App, EntityId, Global};

/// Counts the changes made in any window, so that the other windows show them; `by` is the view
/// that made the last one, which has it already.
#[derive(Default)]
pub struct DataVersion {
    version: u64,
    by: Option<EntityId>,
}

impl Global for DataVersion {}

/// Tells the other windows that `by` changed the data.
pub fn data_changed(by: EntityId, cx: &mut App) {
    let data = cx.default_global::<DataVersion>();
    data.version += 1;
    data.by = Some(by);
}

/// Whether the last change was made by a view other than `observer`.
pub fn changed_elsewhere(observer: EntityId, cx: &App) -> bool {
    cx.try_global::<DataVersion>().is_some_and(|data| data.by != Some(observer))
}
