use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement,
    Pixels, RenderOnce, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use theme::ActiveTheme;

use crate::traits::ClickHandler;
use crate::{Clickable, Toggleable};

/// A clickable row with an optional leading and trailing element: menu items, sidebar entries.
#[derive(IntoElement)]
pub struct ListItem {
    id: ElementId,
    height: Pixels,
    selected: bool,
    start_slot: Option<AnyElement>,
    end_slot: Option<AnyElement>,
    children: SmallVec<[AnyElement; 2]>,
    on_click: Option<ClickHandler>,
}

impl ListItem {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            height: px(32.),
            selected: false,
            start_slot: None,
            end_slot: None,
            children: SmallVec::new(),
            on_click: None,
        }
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }

    pub fn start_slot(mut self, element: impl IntoElement) -> Self {
        self.start_slot = Some(element.into_any_element());
        self
    }

    pub fn end_slot(mut self, element: impl IntoElement) -> Self {
        self.end_slot = Some(element.into_any_element());
        self
    }
}

impl ParentElement for ListItem {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Clickable for ListItem {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Toggleable for ListItem {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl RenderOnce for ListItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        div()
            .id(self.id)
            .h(self.height)
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .rounded(px(6.))
            .cursor_pointer()
            .when(self.selected, |el| el.bg(colors.ghost_element_selected))
            .when(!self.selected, |el| el.hover(|style| style.bg(colors.ghost_element_hover)))
            .when_some(self.on_click, |el, handler| el.on_click(handler))
            .children(self.start_slot)
            .child(div().flex_1().min_w_0().flex().items_center().children(self.children))
            .children(self.end_slot)
    }
}
