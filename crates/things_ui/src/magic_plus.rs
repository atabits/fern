//! Things-style draggable Magic Plus (`+`) button for click-create and drag-and-drop insertion.

use chrono::NaiveDate;
use fern_core::{Id, ScheduleBucket};
use fern_i18n::Strings;
use gpui::{AnyElement, ClickEvent, Context, Render, Window, div, px};
use ui::prelude::*;
use ui::{Color, Icon, IconName, IconSize, Tooltip};

/// Drag payload carried while dragging the Magic Plus (`+`) button over lists, headings, or rows.
#[derive(Clone, Debug, Default)]
pub struct DraggedMagicPlus;

impl Render for DraggedMagicPlus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        div()
            .size(px(34.))
            .rounded_full()
            .bg(colors.accent)
            .shadow_lg()
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::new(IconName::Plus).size(IconSize::Medium).color(Color::OnAccent))
    }
}

/// Resolved destination when Magic Plus is clicked or dropped onto a section, heading, or task slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MagicPlusDropTarget {
    pub list_id: Option<Id>,
    pub section_id: Option<Id>,
    pub start_date: Option<NaiveDate>,
    pub schedule_bucket: ScheduleBucket,
    /// Optional ordering anchor `(target_task_id, after)` inside the destination list.
    pub after_task: Option<(Id, bool)>,
}

/// Renders the signature circular Magic Plus (`+`) button.
pub fn render_magic_plus<V: 'static>(
    strings: &'static Strings,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &ClickEvent, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    let colors = cx.theme().colors();
    div()
        .id("things-magic-plus")
        .when(cfg!(test), |el| el.debug_selector(|| "things-magic-plus".to_owned()))
        .size(px(34.))
        .flex_none()
        .rounded_full()
        .bg(colors.accent)
        .shadow_md()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(colors.accent.opacity(0.88)))
        .tooltip(Tooltip::text(strings.things_new_todo))
        .child(Icon::new(IconName::Plus).size(IconSize::Medium).color(Color::OnAccent))
        .on_click(cx.listener(on_click))
        .on_drag(DraggedMagicPlus, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
        .into_any_element()
}
