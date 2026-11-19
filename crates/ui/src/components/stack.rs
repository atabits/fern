use std::panic::Location;

use gpui::{
    A11ySubtreeBuilder, AnyElement, App, Bounds, Div, Element, ElementId, GlobalElementId,
    InspectorElementId, InteractiveElement, Interactivity, IntoElement, LayoutId, ParentElement,
    Pixels, StyleRefinement, Styled, Window, accesskit, div,
};

use super::{Icon, inline_icons::RowCaptions};

/// A horizontal flex row. Inline icons follow the type in this row, including when
/// the label is inside a flexible or truncated content slot.
pub fn h_flex() -> HorizontalStack {
    HorizontalStack {
        element: div().flex().flex_row().items_center(),
        captions: None,
    }
}

/// A vertical flex container.
pub fn v_flex() -> Div {
    div().flex().flex_col()
}

pub struct HorizontalStack {
    element: Div,
    captions: Option<RowCaptions>,
}

impl Styled for HorizontalStack {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl InteractiveElement for HorizontalStack {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl ParentElement for HorizontalStack {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements.into_iter().map(|mut child| {
            if let Some(icon) = child.downcast_mut::<Icon>() {
                icon.row = Some(
                    self.captions
                        .get_or_insert_with(RowCaptions::default)
                        .clone(),
                );
            }
            child
        }));
    }
}

impl IntoElement for HorizontalStack {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for HorizontalStack {
    type RequestLayoutState = <Div as Element>::RequestLayoutState;
    type PrepaintState = <Div as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Element::id(&self.element)
    }
    fn source_location(&self) -> Option<&'static Location<'static>> {
        self.element.source_location()
    }
    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.element.a11y_role()
    }
    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.element.write_a11y_info(node);
    }
    fn a11y_synthetic_children(
        &mut self,
        state: &mut Self::PrepaintState,
        builder: &mut A11ySubtreeBuilder,
    ) {
        self.element.a11y_synthetic_children(state, builder);
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.element.request_layout(id, inspector, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let mut draw = |cx: &mut App| {
            self.element
                .prepaint(id, inspector, bounds, layout, window, cx)
        };
        if self.captions.is_some() || id.is_some() {
            RowCaptions::collect(self.captions.as_ref(), cx, draw)
        } else {
            draw(cx)
        }
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element
            .paint(id, inspector, bounds, layout, state, window, cx);
    }
}
