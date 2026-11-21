use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, SharedString, Styled, Window, div, px,
};
use theme::ActiveTheme;

use crate::traits::ClickHandler;
use crate::{Button, ButtonVariant, Clickable, LabelSize, Modal};

type SharedClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A confirmation dialog built on [`Modal`] and [`Button`].
///
/// Displays a title, an optional wrapping muted description, a Cancel button, a Confirm button
/// (primary or destructive), and an optional third secondary action button.
#[derive(IntoElement)]
pub struct ConfirmDialog {
    id: ElementId,
    title: SharedString,
    description: Option<SharedString>,
    width: Pixels,
    cancel_label: SharedString,
    confirm_label: SharedString,
    destructive: bool,
    dismiss_on_backdrop: bool,
    on_cancel: Option<SharedClickHandler>,
    on_confirm: Option<ClickHandler>,
    secondary_action: Option<(SharedString, ClickHandler)>,
}

impl ConfirmDialog {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: None,
            width: px(380.),
            cancel_label: "Cancel".into(),
            confirm_label: "Confirm".into(),
            destructive: false,
            dismiss_on_backdrop: true,
            on_cancel: None,
            on_confirm: None,
            secondary_action: None,
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel_label = label.into();
        self
    }

    pub fn confirm_label(mut self, label: impl Into<SharedString>) -> Self {
        self.confirm_label = label.into();
        self
    }

    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Controls whether clicking outside the card invokes `on_cancel` (defaults to `true`).
    pub fn dismiss_on_backdrop(mut self, dismiss: bool) -> Self {
        self.dismiss_on_backdrop = dismiss;
        self
    }

    pub fn on_cancel(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }

    pub fn on_confirm(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_confirm = Some(Box::new(handler));
        self
    }

    /// Adds an optional third secondary button between Cancel and Confirm.
    pub fn secondary_action(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.secondary_action = Some((label.into(), Box::new(handler)));
        self
    }
}

impl RenderOnce for ConfirmDialog {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let cancel_id: ElementId = (self.id.clone(), "cancel").into();
        let secondary_id: ElementId = (self.id.clone(), "secondary").into();
        let confirm_id: ElementId = (self.id.clone(), "confirm").into();

        let mut modal = Modal::new(self.id).width(self.width);
        if self.dismiss_on_backdrop
            && let Some(on_cancel) = self.on_cancel.clone()
        {
            modal = modal.on_dismiss(move |ev, window, cx| on_cancel(ev, window, cx));
        }

        let mut cancel_btn = Button::new(cancel_id, self.cancel_label)
            .variant(ButtonVariant::Secondary);
        if let Some(on_cancel) = self.on_cancel {
            cancel_btn = cancel_btn.on_click(move |ev, window, cx| on_cancel(ev, window, cx));
        }

        let mut confirm_btn = Button::new(confirm_id, self.confirm_label).variant(
            if self.destructive {
                ButtonVariant::Destructive
            } else {
                ButtonVariant::Primary
            },
        );
        if let Some(on_confirm) = self.on_confirm {
            confirm_btn = confirm_btn.on_click(move |ev, window, cx| on_confirm(ev, window, cx));
        }

        let actions = crate::h_flex()
            .w_full()
            .pt_1()
            .justify_end()
            .gap_2()
            .child(
                div()
                    .when(cfg!(test), |el| el.debug_selector(|| "confirm-dialog-cancel".to_owned()))
                    .child(cancel_btn),
            )
            .when_some(self.secondary_action, |row, (label, handler)| {
                row.child(
                    div()
                        .when(cfg!(test), |el| {
                            el.debug_selector(|| "confirm-dialog-secondary".to_owned())
                        })
                        .child(
                            Button::new(secondary_id, label)
                                .variant(ButtonVariant::Secondary)
                                .on_click(move |ev, window, cx| handler(ev, window, cx)),
                        ),
                )
            })
            .child(
                div()
                    .when(cfg!(test), |el| {
                        el.debug_selector(|| "confirm-dialog-confirm".to_owned())
                    })
                    .child(confirm_btn),
            );

        modal
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text)
                    .child(self.title),
            )
            .when_some(self.description, |modal, description| {
                modal.child(
                    div()
                        .when(cfg!(test), |el| {
                            el.debug_selector(|| "confirm-dialog-description".to_owned())
                        })
                        .w_full()
                        .min_w_0()
                        .text_size(LabelSize::Small.pixels())
                        .line_height(px(18.))
                        .text_color(colors.text_muted)
                        .child(description),
                )
            })
            .child(actions)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Context, Modifiers, Render, TestAppContext, point, size};

    use super::*;

    struct ConfirmTestView {
        cancelled: Rc<Cell<usize>>,
        secondary: Rc<Cell<usize>>,
        confirmed: Rc<Cell<usize>>,
        dismiss_on_backdrop: bool,
    }

    impl Render for ConfirmTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let cancelled = self.cancelled.clone();
            let secondary = self.secondary.clone();
            let confirmed = self.confirmed.clone();
            div().relative().size_full().child(
                ConfirmDialog::new("test-confirm", "Delete recurring task?")
                    .width(px(360.))
                    .description(
                        "This will permanently remove the selected item and cannot be undone once confirmed across all synced devices.",
                    )
                    .cancel_label("Cancel")
                    .confirm_label("Delete all")
                    .destructive(true)
                    .dismiss_on_backdrop(self.dismiss_on_backdrop)
                    .on_cancel(move |_, _, _| cancelled.set(cancelled.get() + 1))
                    .secondary_action("Only this one", move |_, _, _| {
                        secondary.set(secondary.get() + 1);
                    })
                    .on_confirm(move |_, _, _| confirmed.set(confirmed.get() + 1)),
            )
        }
    }

    #[gpui::test]
    fn confirm_dialog_wraps_description_and_wires_all_three_actions(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let cancelled = Rc::new(Cell::new(0));
        let secondary = Rc::new(Cell::new(0));
        let confirmed = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view(|_, _| ConfirmTestView {
            cancelled: cancelled.clone(),
            secondary: secondary.clone(),
            confirmed: confirmed.clone(),
            dismiss_on_backdrop: true,
        });
        cx.simulate_resize(size(px(640.), px(420.)));
        cx.run_until_parked();

        let desc = cx.debug_bounds("confirm-dialog-description").unwrap();
        assert!(desc.size.width <= px(320.), "description must fit inside modal width");
        assert!(
            desc.size.height >= px(36.),
            "long description should wrap to multiple lines (got {:?})",
            desc.size.height
        );

        let cancel_bounds = cx.debug_bounds("confirm-dialog-cancel").unwrap();
        let secondary_bounds = cx.debug_bounds("confirm-dialog-secondary").unwrap();
        let confirm_bounds = cx.debug_bounds("confirm-dialog-confirm").unwrap();

        cx.simulate_click(cancel_bounds.center(), Modifiers::default());
        assert_eq!(cancelled.get(), 1);

        cx.simulate_click(secondary_bounds.center(), Modifiers::default());
        assert_eq!(secondary.get(), 1);

        cx.simulate_click(confirm_bounds.center(), Modifiers::default());
        assert_eq!(confirmed.get(), 1);

        // Backdrop click invokes on_cancel when dismiss_on_backdrop is true.
        cx.simulate_click(point(px(12.), px(12.)), Modifiers::default());
        assert_eq!(cancelled.get(), 2);

        // Disable backdrop dismiss and verify backdrop click no longer fires on_cancel.
        view.update(cx, |this, cx| {
            this.dismiss_on_backdrop = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_click(point(px(12.), px(12.)), Modifiers::default());
        assert_eq!(cancelled.get(), 2);
    }
}
