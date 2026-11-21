use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, StatefulInteractiveElement, Styled, Window, div, px,
};
use icons::IconName;
use theme::ActiveTheme;

use crate::traits::ClickHandler;
use crate::{Clickable, Color, Disableable, Icon, IconSize, Toggleable};

/// A square button with an icon and a hover background.
type TooltipBuilder = Box<dyn Fn(&mut Window, &mut App) -> AnyView + 'static>;

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    icon_size: IconSize,
    icon_color: Color,
    hover_icon_color: Option<Color>,
    size: Pixels,
    selected: bool,
    disabled: bool,
    hover_color: Option<gpui::Hsla>,
    on_click: Option<ClickHandler>,
    tooltip: Option<TooltipBuilder>,
    tip: Option<(gpui::SharedString, crate::TipSide)>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        Self {
            id: id.into(),
            icon,
            icon_size: IconSize::Large,
            icon_color: Color::Icon,
            hover_icon_color: None,
            size: px(26.),
            selected: false,
            disabled: false,
            hover_color: None,
            on_click: None,
            tooltip: None,
            tip: None,
        }
    }

    pub fn icon_size(mut self, size: IconSize) -> Self {
        self.icon_size = size;
        self
    }

    pub fn icon_color(mut self, color: impl Into<Color>) -> Self {
        self.icon_color = color.into();
        self
    }

    pub fn hover_icon_color(mut self, color: impl Into<Color>) -> Self {
        self.hover_icon_color = Some(color.into());
        self
    }

    /// A hover tint for buttons drawn on a surface with its own palette.
    pub fn hover_color(mut self, color: gpui::Hsla) -> Self {
        self.hover_color = Some(color);
        self
    }

    /// Shows a tooltip on hover, e.g. `Tooltip::text("…")`.
    pub fn tooltip(mut self, tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        self.tooltip = Some(Box::new(tooltip));
        self
    }

    /// A tooltip centered over or under the button, as the original's (see `with_tip`).
    pub fn tip(mut self, text: impl Into<gpui::SharedString>, side: crate::TipSide) -> Self {
        self.tip = Some((text.into(), side));
        self
    }

    /// The side of the square button.
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}

impl Clickable for IconButton {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Toggleable for IconButton {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl Disableable for IconButton {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let icon_color = if self.selected { Color::Accent } else { self.icon_color };
        let tip = self.tip;
        let hover_color = self.hover_color.unwrap_or(colors.ghost_element_hover);
        let tip_key = gpui::ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "tip".into());
        let hover_group = self
            .hover_icon_color
            .filter(|_| !self.selected && !self.disabled)
            .map(|color| {
                (gpui::SharedString::from(format!("icon-button-{:?}", self.id)), color)
            });
        let icon = Icon::new(self.icon)
            .size(self.icon_size)
            .color(icon_color)
            .when_some(hover_group.clone(), |icon, (group, color)| icon.group_hover_color(group, color));
        let button = div()
            .id(self.id)
            .when_some(hover_group, |el, (group, _)| el.group(group))
            .flex_none()
            .size(self.size)
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .when(self.selected, |el| el.bg(colors.element_selected))
            .when(self.disabled, |el| el.opacity(0.4).cursor_default())
            .when(!self.disabled, |el| {
                el.cursor_pointer()
                    .when(!self.selected, |el| el.hover(|style| style.bg(hover_color)))
                    .when_some(self.on_click, |el, handler| el.on_click(handler))
            })
            .when_some(self.tooltip, |el, tooltip| el.tooltip(tooltip))
            .child(icon);
        match tip {
            Some((text, side)) => crate::with_tip(button, tip_key, text, side, window, cx),
            None => button,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Context, Modifiers, Render, TestAppContext, size};

    use super::*;

    struct IconButtonTestView {
        enabled_clicks: Rc<Cell<usize>>,
        disabled_clicks: Rc<Cell<usize>>,
    }

    impl Render for IconButtonTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let enabled = self.enabled_clicks.clone();
            let disabled = self.disabled_clicks.clone();
            crate::h_flex()
                .p_4()
                .gap_2()
                .child(
                    div()
                        .debug_selector(|| "ib-enabled".to_owned())
                        .child(IconButton::new("ib-enabled", IconName::Plus).on_click(
                            move |_, _, _| enabled.set(enabled.get() + 1),
                        )),
                )
                .child(
                    div()
                        .debug_selector(|| "ib-disabled".to_owned())
                        .child(
                            IconButton::new("ib-disabled", IconName::Trash)
                                .disabled(true)
                                .on_click(move |_, _, _| disabled.set(disabled.get() + 1)),
                        ),
                )
        }
    }

    #[gpui::test]
    fn disabled_icon_button_suppresses_clicks(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let enabled = Rc::new(Cell::new(0));
        let disabled = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view(|_, _| IconButtonTestView {
            enabled_clicks: enabled.clone(),
            disabled_clicks: disabled.clone(),
        });
        cx.simulate_resize(size(px(200.), px(100.)));
        cx.run_until_parked();

        let enabled_bounds = cx.debug_bounds("ib-enabled").unwrap();
        let disabled_bounds = cx.debug_bounds("ib-disabled").unwrap();

        cx.simulate_click(enabled_bounds.center(), Modifiers::default());
        assert_eq!(enabled.get(), 1);

        cx.simulate_click(disabled_bounds.center(), Modifiers::default());
        assert_eq!(disabled.get(), 0);
    }
}
