//! Values carried during drag-and-drop across the task list, kanban board, calendar, matrix,
//! and lists column; each also renders its drag preview.

use fern_core::{Id, RailSection, SidebarSection, SmartList};
use gpui::{AnyElement, FontWeight};
use ui::prelude::*;

fn drag_pill(label: SharedString, semibold: bool, radius: f32, cx: &App) -> Div {
    let colors = cx.theme().colors();
    div()
        .max_w(px(280.))
        .px_3()
        .py_1p5()
        .rounded(px(radius))
        .bg(colors.elevated_surface_background)
        .border_1()
        .border_color(colors.border)
        .shadow_lg()
        .child(if semibold {
            Label::new(label).weight(FontWeight::SEMIBOLD).truncate()
        } else {
            Label::new(label).truncate()
        })
}

/// Floating row preview matching the lists column's row dimensions so dragging from anywhere on
/// the row keeps the ghost anchored directly under the pointer.
fn sidebar_drag_row(
    icon: AnyElement,
    label: SharedString,
    dot: Option<Hsla>,
    semibold: bool,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    h_flex()
        .w(px(196.))
        .h(px(38.))
        .px_2p5()
        .gap_2()
        .rounded(px(6.))
        .bg(colors.elevated_surface_background.opacity(0.95))
        .border_1()
        .border_color(colors.border)
        .shadow_md()
        .child(icon)
        .child(div().flex_1().min_w_0().child(if semibold {
            Label::new(label).weight(FontWeight::SEMIBOLD).color(colors.list_text).truncate()
        } else {
            Label::new(label).color(colors.list_text).truncate()
        }))
        .when_some(dot, |el, color| el.child(div().size(px(6.)).rounded_full().bg(color)))
}

#[derive(Clone)]
pub struct DraggedTask {
    pub id: Id,
    pub title: SharedString,
}

impl Render for DraggedTask {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_pill(self.title.clone(), false, 6., cx)
    }
}

/// A smart list dragged in the lists column to reorder the top smart lists.
#[derive(Clone)]
pub struct DraggedSmartList {
    pub list: SmartList,
    pub label: SharedString,
    pub icon: IconName,
}

impl Render for DraggedSmartList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = Icon::new(self.icon).color(cx.theme().colors().list_text).into_any_element();
        sidebar_drag_row(icon, self.label.clone(), None, false, cx)
    }
}

/// A list dragged in the lists column, to reorder it, move it into a folder, or group with another list.
#[derive(Clone)]
pub struct DraggedList {
    pub id: Id,
    pub name: SharedString,
    pub icon: Option<String>,
    pub color: Option<Hsla>,
}

impl Render for DraggedList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = match &self.icon {
            Some(emoji) => div().w(px(16.)).flex().justify_center().text_size(px(14.)).child(emoji.clone()).into_any_element(),
            None => Icon::new(IconName::List).color(cx.theme().colors().list_text).into_any_element(),
        };
        sidebar_drag_row(icon, self.name.clone(), self.color, false, cx)
    }
}

/// A folder dragged in the lists column to reorder it (and its lists) relative to other items.
#[derive(Clone)]
pub struct DraggedFolder {
    pub id: Id,
    pub name: SharedString,
    pub has_lists: bool,
}

impl Render for DraggedFolder {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = Icon::new(IconName::Folder).color(cx.theme().colors().list_text).into_any_element();
        sidebar_drag_row(icon, self.name.clone(), None, true, cx)
    }
}

/// A standalone or sub-tag (without children) dragged in the lists column to reorder, nest, or promote.
#[derive(Clone)]
pub struct DraggedTag {
    pub name: SharedString,
    pub color: Option<Hsla>,
}

impl Render for DraggedTag {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = Icon::new(IconName::Tag).color(cx.theme().colors().list_text).into_any_element();
        sidebar_drag_row(icon, self.name.clone(), self.color, false, cx)
    }
}

/// A parent tag (one that already has sub-tags) dragged as a whole family at top-level.
/// Distinct from [`DraggedTag`] so parent tags use 2-zone top-level reordering without overlapping
/// the 3-zone nesting overlays of non-parent tags.
#[derive(Clone)]
pub struct DraggedParentTag {
    pub name: SharedString,
    pub color: Option<Hsla>,
}

impl Render for DraggedParentTag {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = Icon::new(IconName::Tag).color(cx.theme().colors().list_text).into_any_element();
        sidebar_drag_row(icon, self.name.clone(), self.color, true, cx)
    }
}

/// A saved filter dragged in the lists column to reorder filters.
#[derive(Clone)]
pub struct DraggedFilter {
    pub id: Id,
    pub name: SharedString,
}

impl Render for DraggedFilter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = Icon::new(IconName::Funnel).color(cx.theme().colors().list_text).into_any_element();
        sidebar_drag_row(icon, self.name.clone(), None, false, cx)
    }
}

/// A middle sidebar section heading (`Lists`, `Tags`, `Filters`) dragged to reorder sections.
#[derive(Clone)]
pub struct DraggedSidebarSection {
    pub section: SidebarSection,
    pub label: SharedString,
}

impl Render for DraggedSidebarSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        h_flex()
            .w(px(196.))
            .h(px(32.))
            .px_2p5()
            .rounded(px(6.))
            .bg(colors.elevated_surface_background.opacity(0.95))
            .border_1()
            .border_color(colors.border)
            .shadow_md()
            .child(Label::new(self.label.clone()).size(LabelSize::Small).weight(FontWeight::SEMIBOLD).color(colors.list_text_muted))
    }
}

/// An icon in the narrow left navigation rail dragged vertically to reorder rail sections.
#[derive(Clone)]
pub struct DraggedRailSection {
    pub section: RailSection,
    pub icon: IconName,
    pub active: bool,
}

impl Render for DraggedRailSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        div()
            .size(px(40.))
            .rounded(px(8.))
            .bg(colors.sidebar_background.opacity(0.95))
            .border_1()
            .border_color(colors.sidebar_border)
            .shadow_md()
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::new(self.icon).size(IconSize::Custom(px(24.))).color(
                if self.active { colors.sidebar_icon } else { colors.sidebar_icon_hover },
            ))
    }
}

/// A kanban section column dragged horizontally to reorder sections.
#[derive(Clone)]
pub struct DraggedColumn {
    pub id: Id,
    pub title: SharedString,
}

impl Render for DraggedColumn {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_pill(self.title.clone(), true, 8., cx)
    }
}

/// Which edge of a timed or multi-day span is being resized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeEdge {
    Start,
    End,
}

/// Drag payload when resizing a task's date span from its start or end edge.
#[derive(Clone)]
pub struct ResizedTask {
    pub id: Id,
    pub title: SharedString,
    pub start: chrono::NaiveDate,
    pub end: chrono::NaiveDate,
    pub edge: ResizeEdge,
}

impl Render for ResizedTask {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_pill(self.title.clone(), false, 6., cx)
    }
}

/// Drag payload when moving a writable CalDAV calendar event to another day.
#[derive(Clone)]
pub struct DraggedEvent {
    pub subscription_id: Id,
    pub uid: String,
    pub title: SharedString,
}

impl Render for DraggedEvent {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_pill(self.title.clone(), false, 6., cx)
    }
}
