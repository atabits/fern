use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, ClickEvent, ElementId, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
    px,
};
use icons::IconName;
use theme::ActiveTheme;

use crate::traits::ClickHandler;
use crate::{Clickable, Color, Disableable, Icon, IconSize, Label, LabelSize};

type TooltipBuilder = Box<dyn Fn(&mut Window, &mut App) -> AnyView + 'static>;

/// Visual hierarchy of a [`Button`].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub enum ButtonVariant {
    /// Solid accent fill for the primary action in a view or dialog.
    Primary,
    /// Bordered surface fill for secondary actions.
    #[default]
    Secondary,
    /// Solid error fill for irreversible or destructive actions.
    Destructive,
    /// Transparent at rest with a subtle hover fill.
    Ghost,
}

/// Height and typography scale of a [`Button`].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub enum ButtonSize {
    /// 26px tall, compact toolbar and inline controls.
    Sm,
    /// 32px tall, standard dialog and form actions.
    #[default]
    Md,
}

impl ButtonSize {
    fn height(self) -> Pixels {
        match self {
            Self::Sm => px(26.),
            Self::Md => px(32.),
        }
    }

    fn padding_x(self) -> Pixels {
        match self {
            Self::Sm => px(10.),
            Self::Md => px(14.),
        }
    }

    fn label_size(self) -> LabelSize {
        match self {
            Self::Sm => LabelSize::Small,
            Self::Md => LabelSize::Default,
        }
    }

    fn icon_size(self) -> IconSize {
        match self {
            Self::Sm => IconSize::XSmall,
            Self::Md => IconSize::Small,
        }
    }
}

/// A themed push button with optional leading icon, size/variant presets, and disabled state.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: ButtonVariant,
    size: ButtonSize,
    icon: Option<IconName>,
    disabled: bool,
    full_width: bool,
    on_click: Option<ClickHandler>,
    tooltip: Option<TooltipBuilder>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            icon: None,
            disabled: false,
            full_width: false,
            on_click: None,
            tooltip: None,
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn tooltip(mut self, tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        self.tooltip = Some(Box::new(tooltip));
        self
    }
}

impl Clickable for Button {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Disableable for Button {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors();
        let error = theme.status().error;

        let (text_color, icon_color) = match self.variant {
            ButtonVariant::Primary | ButtonVariant::Destructive => {
                (Color::OnAccent, Color::IconOnAccent)
            }
            ButtonVariant::Secondary | ButtonVariant::Ghost => (Color::Default, Color::Icon),
        };

        let weight = match self.variant {
            ButtonVariant::Primary | ButtonVariant::Destructive => Some(FontWeight::MEDIUM),
            ButtonVariant::Secondary | ButtonVariant::Ghost => None,
        };

        let mut label = Label::new(self.label)
            .size(self.size.label_size())
            .color(text_color);
        if let Some(weight) = weight {
            label = label.weight(weight);
        }

        crate::h_flex()
            .id(self.id)
            .when(self.full_width, |el| el.w_full())
            .when(!self.full_width, |el| el.flex_none())
            .h(self.size.height())
            .px(self.size.padding_x())
            .gap_1p5()
            .justify_center()
            .rounded(px(6.))
            .whitespace_nowrap()
            .map(|el| match self.variant {
                ButtonVariant::Primary => el.bg(colors.accent).when(!self.disabled, |el| {
                    el.hover(|s| s.bg(colors.accent.opacity(0.88)))
                }),
                ButtonVariant::Secondary => el
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.border)
                    .when(!self.disabled, |el| el.hover(|s| s.bg(colors.element_hover))),
                ButtonVariant::Destructive => el.bg(error).when(!self.disabled, |el| {
                    el.hover(|s| s.bg(error.opacity(0.88)))
                }),
                ButtonVariant::Ghost => el.when(!self.disabled, |el| {
                    el.hover(|s| s.bg(colors.ghost_element_hover))
                }),
            })
            .when(self.disabled, |el| el.opacity(0.4).cursor_default())
            .when(!self.disabled, |el| {
                el.cursor_pointer()
                    .when_some(self.on_click, |el, handler| el.on_click(handler))
            })
            .when_some(self.tooltip, |el, tooltip| el.tooltip(tooltip))
            .when_some(self.icon, |el, icon| {
                el.child(Icon::new(icon).size(self.size.icon_size()).color(icon_color))
            })
            .child(label)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Context, Modifiers, Render, TestAppContext, div, size};

    use super::*;

    struct ButtonTestView {
        enabled_clicks: Rc<Cell<usize>>,
        disabled_clicks: Rc<Cell<usize>>,
    }

    impl Render for ButtonTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let enabled_clicks = self.enabled_clicks.clone();
            let disabled_clicks = self.disabled_clicks.clone();
            crate::v_flex()
                .size_full()
                .p_4()
                .gap_2()
                .child(
                    div()
                        .debug_selector(|| "btn-enabled".to_owned())
                        .child(
                            Button::new("btn-enabled", "Save")
                                .variant(ButtonVariant::Primary)
                                .size(ButtonSize::Md)
                                .icon(IconName::Check)
                                .on_click(move |_, _, _| {
                                    enabled_clicks.set(enabled_clicks.get() + 1);
                                }),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "btn-disabled".to_owned())
                        .child(
                            Button::new("btn-disabled", "Delete")
                                .variant(ButtonVariant::Destructive)
                                .size(ButtonSize::Sm)
                                .disabled(true)
                                .on_click(move |_, _, _| {
                                    disabled_clicks.set(disabled_clicks.get() + 1);
                                }),
                        ),
                )
                .child(Button::new("btn-secondary", "Cancel").variant(ButtonVariant::Secondary))
                .child(Button::new("btn-ghost", "More").variant(ButtonVariant::Ghost))
        }
    }

    #[gpui::test]
    fn button_click_fires_when_enabled_and_is_suppressed_when_disabled(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let enabled_clicks = Rc::new(Cell::new(0));
        let disabled_clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view(|_, _| ButtonTestView {
            enabled_clicks: enabled_clicks.clone(),
            disabled_clicks: disabled_clicks.clone(),
        });
        cx.simulate_resize(size(px(300.), px(240.)));
        cx.run_until_parked();

        let enabled_bounds = cx.debug_bounds("btn-enabled").unwrap();
        let disabled_bounds = cx.debug_bounds("btn-disabled").unwrap();
        assert_eq!(enabled_bounds.size.height, px(32.));
        assert_eq!(disabled_bounds.size.height, px(26.));

        cx.simulate_click(enabled_bounds.center(), Modifiers::default());
        assert_eq!(enabled_clicks.get(), 1);

        cx.simulate_click(disabled_bounds.center(), Modifiers::default());
        assert_eq!(disabled_clicks.get(), 0);
    }
}
