use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, RenderOnce, StatefulInteractiveElement, Styled, Window, div, px,
};
use smallvec::SmallVec;
use theme::ActiveTheme;

use crate::traits::ClickHandler;

const DEFAULT_WIDTH: Pixels = px(420.);
const DEFAULT_PADDING: Pixels = px(20.);
const DEFAULT_GAP: Pixels = px(12.);

/// A full-screen modal backdrop with a centered elevated card shell.
///
/// The backdrop occludes mouse events behind the modal and optionally invokes `on_dismiss`
/// when clicking outside the card. Clicks inside the card stop propagation so they never
/// trigger `on_dismiss`.
#[derive(IntoElement)]
pub struct Modal {
    id: ElementId,
    width: Option<Pixels>,
    max_width: Option<Pixels>,
    height: Option<Pixels>,
    max_height: Option<Pixels>,
    overflow_hidden: bool,
    padding: Pixels,
    gap: Pixels,
    on_dismiss: Option<ClickHandler>,
    children: SmallVec<[AnyElement; 4]>,
}

impl Modal {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            width: None,
            max_width: None,
            height: None,
            max_height: None,
            overflow_hidden: false,
            padding: DEFAULT_PADDING,
            gap: DEFAULT_GAP,
            on_dismiss: None,
            children: SmallVec::new(),
        }
    }

    /// Sets an explicit preferred width for the modal card (still clamped to the viewport).
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Sets the maximum width for a responsive modal card.
    pub fn max_width(mut self, max_width: Pixels) -> Self {
        self.max_width = Some(max_width);
        self
    }

    /// Sets an explicit height for the modal card.
    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    /// Sets the maximum height for the modal card.
    pub fn max_height(mut self, max_height: Pixels) -> Self {
        self.max_height = Some(max_height);
        self
    }

    /// Clips overflowing child content inside the modal card's rounded bounds.
    pub fn overflow_hidden(mut self) -> Self {
        self.overflow_hidden = true;
        self
    }

    /// Overrides the inner padding of the modal card (defaults to 20px).
    pub fn padding(mut self, padding: Pixels) -> Self {
        self.padding = padding;
        self
    }

    /// Overrides the vertical spacing between direct children (defaults to 12px).
    pub fn gap(mut self, gap: Pixels) -> Self {
        self.gap = gap;
        self
    }

    /// Registers a click-outside handler invoked when the user clicks the dimmed backdrop.
    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Box::new(handler));
        self
    }
}

impl ParentElement for Modal {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Modal {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors();
        let scrim_opacity = if theme.is_dark() { 0.50 } else { 0.35 };
        let card_id: ElementId = (self.id.clone(), "card").into();

        let card = crate::v_flex()
            .id(card_id)
            .p(self.padding)
            .gap(self.gap)
            .rounded(px(10.))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .map(|el| match (self.width, self.max_width) {
                (Some(w), Some(mw)) => el.w(w).max_w(mw),
                (Some(w), None) => el.w(w).max_w_full(),
                (None, Some(mw)) => el.w_full().max_w(mw),
                (None, None) => el.w(DEFAULT_WIDTH).max_w_full(),
            })
            .when_some(self.height, |el, h| el.h(h))
            .when_some(self.max_height, |el, mh| el.max_h(mh))
            .when(self.overflow_hidden, |el| el.overflow_hidden())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .children(self.children);

        div()
            .id(self.id)
            .absolute()
            .inset_0()
            .p_4()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(scrim_opacity))
            .occlude()
            .when_some(self.on_dismiss, |el, handler| el.on_click(handler))
            .child(card)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Context, Modifiers, Render, TestAppContext, point, size};

    use super::*;
    use crate::{Button, ButtonVariant, Clickable};

    struct ModalTestView {
        behind_clicks: Rc<Cell<usize>>,
        dismiss_clicks: Rc<Cell<usize>>,
        inner_clicks: Rc<Cell<usize>>,
    }

    impl Render for ModalTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let behind = self.behind_clicks.clone();
            let dismiss = self.dismiss_clicks.clone();
            let inner = self.inner_clicks.clone();
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .id("behind-target")
                        .size_full()
                        .on_click(move |_, _, _| behind.set(behind.get() + 1)),
                )
                .child(
                    Modal::new("test-modal")
                        .width(px(280.))
                        .on_dismiss(move |_, _, _| dismiss.set(dismiss.get() + 1))
                        .child(
                            div()
                                .debug_selector(|| "modal-body".to_owned())
                                .h(px(40.))
                                .w_full(),
                        )
                        .child(
                            div()
                                .debug_selector(|| "modal-action".to_owned())
                                .child(
                                    Button::new("modal-ok", "OK")
                                        .variant(ButtonVariant::Primary)
                                        .on_click(move |_, _, _| inner.set(inner.get() + 1)),
                                ),
                        ),
                )
        }
    }

    #[gpui::test]
    fn modal_stops_card_clicks_allows_inner_buttons_and_occludes_background(
        cx: &mut TestAppContext,
    ) {
        cx.update(theme::init);
        let behind = Rc::new(Cell::new(0));
        let dismiss = Rc::new(Cell::new(0));
        let inner = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view(|_, _| ModalTestView {
            behind_clicks: behind.clone(),
            dismiss_clicks: dismiss.clone(),
            inner_clicks: inner.clone(),
        });
        cx.simulate_resize(size(px(600.), px(400.)));
        cx.run_until_parked();

        let body = cx.debug_bounds("modal-body").unwrap();
        let action = cx.debug_bounds("modal-action").unwrap();
        // 280px card minus 20px left/right padding minus 1px border each side = 238px.
        assert!((body.size.width - px(238.)).abs() <= px(2.));

        // Clicking inside card body does not dismiss or leak to background.
        cx.simulate_click(body.center(), Modifiers::default());
        assert_eq!(dismiss.get(), 0);
        assert_eq!(behind.get(), 0);

        // Clicking inner button fires button click without dismissing modal.
        cx.simulate_click(action.center(), Modifiers::default());
        assert_eq!(inner.get(), 1);
        assert_eq!(dismiss.get(), 0);
        assert_eq!(behind.get(), 0);

        // Clicking on backdrop corner dismisses modal and occludes background.
        cx.simulate_click(point(px(10.), px(10.)), Modifiers::default());
        assert_eq!(dismiss.get(), 1);
        assert_eq!(behind.get(), 0);
    }
}
