use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, px,
};
use icons::IconName;
use smallvec::SmallVec;
use theme::ActiveTheme;

use crate::traits::ClickHandler;
use crate::{Clickable, Color, Disableable, Icon, IconSize, LabelSize, LabelText, Toggleable};

/// A single tab segment inside [`SegmentedTabs`].
pub struct SegmentedTab {
    id: ElementId,
    label: SharedString,
    icon: Option<IconName>,
    selected: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl SegmentedTab {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            selected: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl Toggleable for SegmentedTab {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl Clickable for SegmentedTab {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Disableable for SegmentedTab {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A pill-style segmented tab bar with elevated active state and clear hover feedback for
/// unselected tabs.
#[derive(IntoElement, Default)]
pub struct SegmentedTabs {
    tabs: SmallVec<[SegmentedTab; 4]>,
    full_width: bool,
}

impl SegmentedTabs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tab(mut self, tab: SegmentedTab) -> Self {
        self.tabs.push(tab);
        self
    }

    pub fn tabs(mut self, tabs: impl IntoIterator<Item = SegmentedTab>) -> Self {
        self.tabs.extend(tabs);
        self
    }

    /// Stretches the tab bar to fill its parent width with equal-width segments.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }
}

impl RenderOnce for SegmentedTabs {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let full_width = self.full_width;

        crate::h_flex()
            .when(full_width, |el| el.w_full())
            .when(!full_width, |el| el.flex_none())
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(8.))
            .bg(colors.surface_background)
            .border_1()
            .border_color(colors.border_variant)
            .children(self.tabs.into_iter().map(|tab| {
                let icon_color = if tab.selected {
                    Color::Accent
                } else {
                    Color::IconMuted
                };
                let text_color = if tab.selected {
                    colors.text
                } else {
                    colors.text_muted
                };

                crate::h_flex()
                    .id(tab.id)
                    .when(full_width, |el| el.flex_1().min_w_0())
                    .when(!full_width, |el| el.flex_none())
                    .h(px(26.))
                    .px_2p5()
                    .gap_1p5()
                    .justify_center()
                    .rounded(px(6.))
                    .border_1()
                    .text_size(LabelSize::Small.pixels())
                    .text_color(text_color)
                    .whitespace_nowrap()
                    .when(tab.selected, |el| {
                        el.bg(colors.elevated_surface_background)
                            .border_color(colors.border_variant)
                            .shadow_sm()
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!tab.selected, |el| {
                        el.bg(gpui::transparent_black())
                            .border_color(gpui::transparent_black())
                    })
                    .when(tab.disabled, |el| el.opacity(0.4).cursor_default())
                    .when(!tab.disabled, |el| {
                        el.cursor_pointer()
                            .when(!tab.selected, |el| {
                                el.hover(|s| s.bg(colors.ghost_element_hover).text_color(colors.text))
                            })
                            .when_some(tab.on_click, |el, handler| el.on_click(handler))
                    })
                    .when_some(tab.icon, |el, icon| {
                        el.child(Icon::new(icon).size(IconSize::XSmall).color(icon_color))
                    })
                    .child(LabelText::new(tab.label))
            }))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Context, Modifiers, Render, TestAppContext, div, point, size};

    use super::*;

    struct TabsTestView {
        active: usize,
        disabled_clicks: Rc<Cell<usize>>,
    }

    impl Render for TabsTestView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let disabled_clicks = self.disabled_clicks.clone();
            div().w(px(300.)).p_4().child(
                div()
                    .debug_selector(|| "tabs-bar".to_owned())
                    .w_full()
                    .child(
                        SegmentedTabs::new()
                            .full_width()
                            .tab(
                                SegmentedTab::new("tab-0", "Day")
                                    .toggle_state(self.active == 0)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active = 0;
                                        cx.notify();
                                    })),
                            )
                            .tab(
                                SegmentedTab::new("tab-1", "Week")
                                    .toggle_state(self.active == 1)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active = 1;
                                        cx.notify();
                                    })),
                            )
                            .tab(
                                SegmentedTab::new("tab-2", "Month")
                                    .toggle_state(self.active == 2)
                                    .disabled(true)
                                    .on_click(move |_, _, _| {
                                        disabled_clicks.set(disabled_clicks.get() + 1);
                                    }),
                            ),
                    ),
            )
        }
    }

    #[gpui::test]
    fn segmented_tabs_switch_active_tab_and_block_disabled_tab(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let disabled_clicks = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view(|_, _| TabsTestView {
            active: 0,
            disabled_clicks: disabled_clicks.clone(),
        });
        cx.simulate_resize(size(px(340.), px(120.)));
        cx.run_until_parked();

        let bar = cx.debug_bounds("tabs-bar").unwrap();
        // Click middle third ("Week")
        let mid = point(bar.left() + bar.size.width * 0.5, bar.center().y);
        cx.simulate_click(mid, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |this, _| this.active), 1);

        // Click right third ("Month", disabled)
        let right = point(bar.left() + bar.size.width * 0.85, bar.center().y);
        cx.simulate_click(right, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |this, _| this.active), 1);
        assert_eq!(disabled_clicks.get(), 0);
    }
}
