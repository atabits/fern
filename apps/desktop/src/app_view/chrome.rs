use desktop_common::plugin_bus::PluginBus;
use fern_core::RailItemId;
use ui::popover;
use ui::prelude::*;

use crate::i18n;
use crate::settings_window;
use crate::sidebar::{self, Section};
use crate::title_bar;

use super::AppView;

pub(super) fn reorder_visible_rail_items(
    original: &[RailItemId],
    visible: &[RailItemId],
    from: usize,
    to: usize,
) -> Vec<RailItemId> {
    if from >= visible.len() || to >= visible.len() || from == to {
        return original.to_vec();
    }
    let mut base = original.to_vec();
    for item in visible {
        if !base.contains(item) {
            base.push(item.clone());
        }
    }
    let mut reordered_visible = visible.to_vec();
    let moved = reordered_visible.remove(from);
    reordered_visible.insert(to, moved);
    let mut next_visible = reordered_visible.into_iter();
    base.into_iter()
        .map(|item| {
            if visible.contains(&item) {
                next_visible.next().unwrap_or(item)
            } else {
                item
            }
        })
        .collect()
}

impl AppView {
    pub(super) fn start_rail_drag(&mut self, section: Section, cx: &mut Context<Self>) {
        self.rail_drag_origin = Some(self.rail_items.clone());
        self.dragging_rail = Some(section);
        self.nav_hovered = None;
        cx.notify();
    }

    pub(super) fn cancel_rail_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.dragging_rail.take().is_some() {
            cx.stop_active_drag(window);
            if let Some(origin) = self.rail_drag_origin.take() {
                self.rail_items = origin;
            }
            self.nav_hovered = None;
            cx.notify();
            return true;
        }
        false
    }

    pub(super) fn finish_rail_drag_up(
        &mut self,
        event: &gpui::MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != gpui::MouseButton::Left || self.dragging_rail.take().is_none() {
            return;
        }
        cx.stop_active_drag(window);
        let origin = self.rail_drag_origin.take();
        if origin.as_ref() != Some(&self.rail_items) {
            let items = self.rail_items.clone();
            if let Ok(prefs) = self.store.update_preferences(|p| p.set_rail_items(items)) {
                self.rail_items = prefs.rail_items;
            }
        }
        self.nav_hovered = None;
        cx.notify();
    }

    pub(super) fn move_rail_drag(
        &mut self,
        event: &gpui::DragMoveEvent<sidebar::DraggedRailItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dragged = event.drag(cx).item.clone();
        if self.dragging_rail.is_none() {
            self.rail_drag_origin = Some(self.rail_items.clone());
        }
        self.dragging_rail = Some(Section::try_from_rail_item(&dragged));
        let plugin_views = cx
            .try_global::<PluginBus>()
            .map(|bus| bus.sidebar_views.clone())
            .unwrap_or_default();
        let visible = sidebar::visible_rail_items(&self.rail_items, self.features, &plugin_views);
        if visible.is_empty() {
            self.cancel_rail_drag(window, cx);
            return;
        }
        let top_y = title_bar::height(window, cx) + px(sidebar::SECTIONS_TOP_OFFSET);
        let content_y = event.event.position.y - top_y - self.rail_scroll.offset().y;
        let slot = ((f32::from(content_y) / sidebar::SLOT_HEIGHT).floor() as isize)
            .clamp(0, (visible.len() - 1) as isize) as usize;
        let Some(current) = visible.iter().position(|item| item == &dragged) else {
            self.cancel_rail_drag(window, cx);
            return;
        };
        if current != slot {
            self.rail_items = reorder_visible_rail_items(&self.rail_items, &visible, current, slot);
            self.nav_hovered = None;
            cx.notify();
        }
    }

    pub(super) fn set_account_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        self.account_menu_open = open;
        cx.notify();
    }

    pub(super) fn render_account_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let topmost = self.always_on_top;
        popover(cx)
            .min_w(px(196.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_account_menu(false, cx)))
            .child(
                ListItem::new("account-settings")
                    .start_slot(Icon::new(IconName::Settings).color(Color::Icon))
                    .child(Label::new(strings.settings))
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(None, window, cx))),
            )
            .child(
                ListItem::new("account-statistics")
                    .start_slot(Icon::new(IconName::ChartNoAxesColumn).color(Color::Icon))
                    .child(Label::new(strings.statistics))
                    .on_click(cx.listener(|this, _, window, cx| this.open_statistics(window, cx))),
            )
            .child(
                ListItem::new("account-shortcuts")
                    .start_slot(Icon::new(IconName::Keyboard).color(Color::Icon))
                    .child(Label::new(strings.shortcuts))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings_page(settings_window::Page::Shortcuts, window, cx);
                    })),
            )
            .when(!desktop_common::is_wayland_session(), |menu| {
                menu.child(div().my_1().child(Divider::horizontal())).child(
                    ListItem::new("account-always-on-top")
                        .start_slot(Icon::new(if topmost { IconName::PinOff } else { IconName::Pin }).color(if topmost { Color::Accent } else { Color::Icon }))
                        .child(Label::new(strings.tray_always_on_top).color(if topmost { Color::Accent } else { Color::Default }))
                        .when(topmost, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.account_menu_open = false;
                            if let Err(err) = crate::tray::set_always_on_top(&this.store, !topmost, cx) {
                                desktop_common::logging::log_error("tray", format!("failed to toggle always_on_top: {err:#}"));
                            }
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_update_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notes = self.update_notice.as_ref()?;
        let strings = i18n::t(cx);
        let message = strings.update_installed.replace("{version}", &notes.version);
        Some(
            popover(cx)
                .id("update-notice")
                .debug_selector(|| "update-notice".to_owned())
                .w(px(296.))
                .p_2()
                .gap_1()
                .child(
                    h_flex().items_start().gap_2()
                        .child(div().flex_none().pl_1().pt(px(5.)).child(
                            Icon::new(IconName::CircleCheck).size(IconSize::Small).color(Color::Accent),
                        ))
                        .child(div().flex_1().min_w_0().pt_1().line_height(px(20.)).child(
                            Label::new(message).weight(gpui::FontWeight::MEDIUM),
                        ))
                        .child(
                            div()
                                .debug_selector(|| "dismiss-update-notice".to_owned())
                                .child(
                                    IconButton::new("dismiss-update-notice", IconName::X)
                                        .icon_size(IconSize::Small)
                                        .icon_color(Color::Muted)
                                        .tooltip(Tooltip::text(strings.close))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.update_notice = None;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
                .child(
                    h_flex().pl(px(26.)).pb_1().child(
                        div()
                            .debug_selector(|| "updated-whats-new".to_owned())
                            .child(
                                Button::new("updated-whats-new", strings.whats_new)
                                    .variant(ButtonVariant::Secondary)
                                    .size(ButtonSize::Sm)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(notes) = this.update_notice.take() {
                                            this.open_settings(Some(notes), window, cx);
                                        }
                                    })),
                            ),
                    ),
                )
                .into_any_element(),
        )
    }
}
