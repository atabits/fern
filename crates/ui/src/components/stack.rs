use std::panic::Location;

use gpui::{
    A11ySubtreeBuilder, AnyElement, App, Bounds, Div, Element, ElementId, GlobalElementId,
    InspectorElementId, InteractiveElement, Interactivity, IntoElement, LayoutId, ParentElement,
    Pixels, Stateful, StyleRefinement, Styled, Window, accesskit, div,
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
                child
            } else if child.downcast_mut::<Stateful<Div>>().is_some() {
                IsolatedControl(child).into_any_element()
            } else {
                child
            }
        }));
    }
}

struct IsolatedControl(AnyElement);

impl IntoElement for IsolatedControl {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for IsolatedControl {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.0.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        RowCaptions::collect(None, cx, |cx| self.0.prepaint(window, cx));
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.paint(window, cx);
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

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, canvas, point, px, size};
    use icons::IconName;

    use super::*;
    use crate::{Label, LabelSize};

    #[gpui::test]
    fn keyed_div_controls_isolate_captions_while_keeping_flex_layout(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let cx = cx.add_empty_window();
        cx.draw(point(px(0.), px(0.)), size(px(240.), px(40.)), |_, _| {
            let mut row = h_flex().w(px(240.)).h(px(40.));
            row.extend([
                Icon::new(IconName::Check).into_any_element(),
                div()
                    .id("isolated-chip")
                    .flex_1()
                    .h_full()
                    .debug_selector(|| "isolated-chip".into())
                    .child(
                        canvas(
                            |bounds, _, cx| {
                                RowCaptions::add(bounds, cx, || px(3.));
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                    .into_any_element(),
                div()
                    .w(px(60.))
                    .h_full()
                    .child(
                        canvas(
                            |bounds, _, cx| {
                                RowCaptions::add(bounds, cx, || px(19.));
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                    .child(Label::new("Row").size(LabelSize::Small))
                    .into_any_element(),
            ]);
            let captions = row.captions.clone().unwrap();
            v_flex()
                .child(row)
                .child(canvas(
                    move |_, window, _| {
                        let icon_slot = Bounds::new(point(px(0.), px(12.)), size(px(16.), px(16.)));
                        assert_eq!(captions.axis_for(icon_slot, window), Some(px(19.)));
                    },
                    |_, _, _, _| {},
                ))
        });
        let chip = cx.debug_bounds("isolated-chip").unwrap();
        assert_eq!(chip.size.width, px(240. - 16. - 60.));
    }
}
