use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, SharedString, Styled, Window, div, px,
};
use icons::IconName;
use theme::ActiveTheme;

use crate::traits::ClickHandler;
use crate::{Button, ButtonVariant, Clickable, Color, Icon, IconSize, LabelSize};

/// A centered empty-state placeholder with a circular icon badge, wrapping muted message,
/// and an optional primary call-to-action button.
#[derive(IntoElement)]
pub struct EmptyState {
    icon: IconName,
    title: Option<SharedString>,
    message: SharedString,
    max_width: Pixels,
    action: Option<(ElementId, SharedString, ClickHandler)>,
}

impl EmptyState {
    pub fn new(icon: IconName, message: impl Into<SharedString>) -> Self {
        Self {
            icon,
            title: None,
            message: message.into(),
            max_width: px(320.),
            action: None,
        }
    }

    /// Optional heading rendered above the muted message.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Maximum text column width before wrapping (defaults to 320px).
    pub fn max_width(mut self, max_width: Pixels) -> Self {
        self.max_width = max_width;
        self
    }

    /// Optional primary CTA button displayed below the message.
    pub fn action(
        mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.action = Some((id.into(), label.into(), Box::new(on_click)));
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();

        let badge = div()
            .size(px(48.))
            .flex_none()
            .rounded_full()
            .bg(colors.surface_background)
            .border_1()
            .border_color(colors.border_variant)
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::new(self.icon).size(IconSize::XLarge).color(Color::IconMuted));

        crate::v_flex()
            .size_full()
            .flex_1()
            .items_center()
            .justify_center()
            .p_6()
            .child(
                crate::v_flex()
                    .w_full()
                    .max_w(self.max_width)
                    .items_center()
                    .gap_3()
                    .child(badge)
                    .when_some(self.title, |col, title| {
                        col.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_center()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text)
                                .child(title),
                        )
                    })
                    .child(
                        div()
                            .when(cfg!(test), |el| {
                                el.debug_selector(|| "empty-state-message".to_owned())
                            })
                            .w_full()
                            .min_w_0()
                            .text_center()
                            .text_size(LabelSize::Small.pixels())
                            .line_height(px(18.))
                            .text_color(colors.text_muted)
                            .child(self.message),
                    )
                    .when_some(self.action, |col, (id, label, handler)| {
                        col.child(
                            div()
                                .pt_1()
                                .when(cfg!(test), |el| {
                                    el.debug_selector(|| "empty-state-cta".to_owned())
                                })
                                .child(
                                    Button::new(id, label)
                                        .variant(ButtonVariant::Primary)
                                        .on_click(move |ev, window, cx| handler(ev, window, cx)),
                                ),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Context, Modifiers, Render, TestAppContext, size};

    use super::*;

    struct EmptyStateTestView {
        clicks: Rc<Cell<usize>>,
    }

    impl Render for EmptyStateTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    EmptyState::new(
                        IconName::Inbox,
                        "No items match this view yet. Add your first item to get started right away.",
                    )
                    .title("All clear")
                    .max_width(px(240.))
                    .action("empty-cta", "Add task", move |_, _, _| {
                        clicks.set(clicks.get() + 1);
                    }),
                )
        }
    }

    #[gpui::test]
    fn empty_state_wraps_message_and_triggers_cta(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view(|_, _| EmptyStateTestView {
            clicks: clicks.clone(),
        });
        cx.simulate_resize(size(px(400.), px(320.)));
        cx.run_until_parked();

        let msg = cx.debug_bounds("empty-state-message").unwrap();
        assert!(msg.size.width <= px(240.), "message must respect max_width");
        assert!(
            msg.size.height >= px(36.),
            "long message should wrap onto multiple lines (got {:?})",
            msg.size.height
        );

        let cta = cx.debug_bounds("empty-state-cta").unwrap();
        cx.simulate_click(cta.center(), Modifiers::default());
        assert_eq!(clicks.get(), 1);
    }
}
