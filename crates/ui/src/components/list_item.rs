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
/// Content is clipped between the slots; attach popovers outside the row.
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

    pub fn id(&self) -> ElementId {
        self.id.clone()
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
            .child(div().flex_1().min_w_0().overflow_hidden().flex().items_center().children(self.children))
            .children(self.end_slot)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Bounds, Context, Render, TestAppContext, canvas};

    use super::*;
    use crate::Label;

    type Painted = Rc<Cell<Option<(Bounds<Pixels>, Bounds<Pixels>)>>>;

    struct TestRow {
        start: Painted,
        content: Painted,
        end: Painted,
    }

    impl Render for TestRow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let probe = |painted: Painted| {
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, _| painted.set(Some((bounds, window.content_mask().bounds))),
                )
                .absolute()
                .inset_0()
            };
            div().w(px(180.)).child(
                ListItem::new("bounded-row")
                    .start_slot(div().relative().flex_none().size(px(16.)).child(probe(self.start.clone())))
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .whitespace_nowrap()
                            .child(Label::new("LongUnbrokenUserName".repeat(40)))
                            .child(probe(self.content.clone())),
                    )
                    .end_slot(div().relative().flex_none().size(px(16.)).child(probe(self.end.clone()))),
            )
        }
    }

    #[gpui::test]
    fn long_content_cannot_paint_over_slots(cx: &mut TestAppContext) {
        let start: Painted = Rc::default();
        let content: Painted = Rc::default();
        let end: Painted = Rc::default();
        cx.update(theme::init);
        let (_, cx) = cx.add_window_view(|_, _| TestRow {
            start: start.clone(),
            content: content.clone(),
            end: end.clone(),
        });
        cx.run_until_parked();

        let (start_bounds, start_mask) = start.get().expect("start slot painted");
        let (content_bounds, content_mask) = content.get().expect("content painted");
        let (end_bounds, end_mask) = end.get().expect("end slot painted");
        assert!(content_bounds.right() > end_bounds.left(), "the fixture must overflow the content area");
        assert!(content_mask.left() >= start_bounds.right());
        assert!(content_mask.right() <= end_bounds.left());
        assert_eq!(start_bounds.intersect(&start_mask), start_bounds);
        assert_eq!(end_bounds.intersect(&end_mask), end_bounds);
        assert!(end_bounds.right() <= px(180.));
    }
}
