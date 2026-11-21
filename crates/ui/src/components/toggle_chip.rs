use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, px,
};
use icons::IconName;
use theme::ActiveTheme;

use crate::traits::ClickHandler;
use crate::{Clickable, Color, Disableable, Icon, IconSize, Label, LabelSize, Toggleable};

/// A selectable toggle chip with clearly distinct resting, hover, and selected states.
///
/// Always renders a 1px border in every state so toggling never causes layout shift.
#[derive(IntoElement)]
pub struct ToggleChip {
    id: ElementId,
    label: SharedString,
    icon: Option<IconName>,
    icon_color: Option<Color>,
    selected: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl ToggleChip {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            icon_color: None,
            selected: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn icon_color(mut self, color: impl Into<Color>) -> Self {
        self.icon_color = Some(color.into());
        self
    }
}

impl Toggleable for ToggleChip {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl Clickable for ToggleChip {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Disableable for ToggleChip {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

struct ChipSurface {
    rest_bg: gpui::Hsla,
    rest_border: gpui::Hsla,
    hover_bg: gpui::Hsla,
    hover_border: gpui::Hsla,
}

fn chip_surface(colors: &theme::ThemeColors, selected: bool) -> ChipSurface {
    if selected {
        ChipSurface {
            rest_bg: colors.accent.opacity(0.16),
            rest_border: colors.accent.opacity(0.65),
            hover_bg: colors.accent.opacity(0.24),
            hover_border: colors.accent,
        }
    } else {
        ChipSurface {
            rest_bg: gpui::transparent_black(),
            rest_border: colors.border,
            hover_bg: colors.element_hover,
            hover_border: colors.text_muted.opacity(0.45),
        }
    }
}

impl RenderOnce for ToggleChip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let text_color = if self.selected {
            Color::Accent
        } else {
            Color::Default
        };
        let icon_color = self.icon_color.unwrap_or(if self.selected {
            Color::Accent
        } else {
            Color::IconMuted
        });
        let surface = chip_surface(colors, self.selected);

        crate::h_flex()
            .id(self.id)
            .flex_none()
            .min_h(px(28.))
            .px_2p5()
            .py_1()
            .gap_1p5()
            .rounded(px(6.))
            .border_1()
            .border_color(surface.rest_border)
            .bg(surface.rest_bg)
            .whitespace_nowrap()
            .when(self.disabled, |el| el.opacity(0.4).cursor_default())
            .when(!self.disabled, |el| {
                el.cursor_pointer()
                    .hover(move |s| s.bg(surface.hover_bg).border_color(surface.hover_border))
                    .when_some(self.on_click, |el, handler| el.on_click(handler))
            })
            .when_some(self.icon, |el, icon| {
                el.child(Icon::new(icon).size(IconSize::XSmall).color(icon_color))
            })
            .child(Label::new(self.label).size(LabelSize::Small).color(text_color))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Context, Modifiers, Render, TestAppContext, div, size};

    use super::*;

    struct ChipTestView {
        selected: bool,
        disabled_clicks: Rc<Cell<usize>>,
    }

    impl Render for ChipTestView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let disabled_clicks = self.disabled_clicks.clone();
            crate::h_flex()
                .p_4()
                .gap_2()
                .child(
                    div()
                        .debug_selector(|| "chip-toggle".to_owned())
                        .child(
                            ToggleChip::new("chip-toggle", "High priority")
                                .icon(IconName::Flag)
                                .toggle_state(self.selected)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.selected = !this.selected;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "chip-disabled".to_owned())
                        .child(
                            ToggleChip::new("chip-disabled", "Locked")
                                .disabled(true)
                                .on_click(move |_, _, _| {
                                    disabled_clicks.set(disabled_clicks.get() + 1);
                                }),
                        ),
                )
        }
    }

    #[gpui::test]
    fn toggle_chip_toggles_without_size_shift_and_respects_disabled(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let disabled_clicks = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view(|_, _| ChipTestView {
            selected: false,
            disabled_clicks: disabled_clicks.clone(),
        });
        cx.simulate_resize(size(px(360.), px(120.)));
        cx.run_until_parked();

        let before = cx.debug_bounds("chip-toggle").unwrap();
        cx.simulate_click(before.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(view.read_with(cx, |this, _| this.selected));

        let after = cx.debug_bounds("chip-toggle").unwrap();
        assert_eq!(before.size, after.size, "toggling chip must not shift layout");

        let disabled_bounds = cx.debug_bounds("chip-disabled").unwrap();
        cx.simulate_click(disabled_bounds.center(), Modifiers::default());
        assert_eq!(disabled_clicks.get(), 0);
    }

    #[test]
    fn resting_hover_and_selected_states_are_distinct_across_themes() {
        for theme in [theme::Theme::light(), theme::Theme::dark()] {
            let colors = theme.colors();
            let unselected = chip_surface(colors, false);
            let selected = chip_surface(colors, true);
            assert_ne!(unselected.rest_bg, unselected.hover_bg);
            assert_ne!(unselected.rest_border, unselected.hover_border);
            assert_ne!(unselected.rest_bg, selected.rest_bg);
            assert_ne!(unselected.hover_bg, selected.rest_bg);
            assert_ne!(selected.rest_bg, selected.hover_bg);
            assert_ne!(selected.rest_border, selected.hover_border);
        }
    }
}
