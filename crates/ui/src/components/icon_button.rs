use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, StatefulInteractiveElement, Styled, Window, div, px,
};
use icons::IconName;
use theme::ActiveTheme;

use crate::traits::ClickHandler;
use crate::{Clickable, Color, Icon, IconSize, Toggleable};

/// A square button with an icon and a hover background.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    icon_size: IconSize,
    icon_color: Color,
    size: Pixels,
    selected: bool,
    on_click: Option<ClickHandler>,
    tooltip: Option<Box<dyn Fn(&mut Window, &mut App) -> AnyView + 'static>>,
    tip: Option<(gpui::SharedString, crate::TipSide)>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        Self {
            id: id.into(),
            icon,
            icon_size: IconSize::Large,
            icon_color: Color::Muted,
            size: px(26.),
            selected: false,
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

impl RenderOnce for IconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let icon_color = if self.selected { Color::Default } else { self.icon_color };
        let tip = self.tip;
        let tip_key = gpui::ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "tip".into());
        let button = div()
            .id(self.id)
            .flex_none()
            .size(self.size)
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .when(self.selected, |el| el.bg(colors.element_selected))
            .hover(|style| style.bg(colors.element_hover))
            .when_some(self.on_click, |el, handler| el.on_click(handler))
            .when_some(self.tooltip, |el, tooltip| el.tooltip(tooltip))
            .child(Icon::new(self.icon).size(self.icon_size).color(icon_color));
        match tip {
            Some((text, side)) => crate::with_tip(button, tip_key, text, side, window, cx),
            None => button,
        }
    }
}
