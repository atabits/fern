use desktop_common::i18n::{self, Locale};
use desktop_common::text_input::TextInput;
use desktop_sync::subscriptions::{self, ChinaHolidaySource, ChinaHolidayStatus};
use fern_core::ical::{CalendarKind, CalendarProvider};
use gpui::{Entity, FontWeight};
use task_ui::lists_panel;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use super::{ActiveConfirm, Menu, ROW_HEIGHT, SettingsWindow};

impl SettingsWindow {
    pub(super) fn confirm_delete_subscription(&mut self, cx: &mut Context<Self>) {
        let Some(ActiveConfirm::DeleteSubscription { id, .. }) = self.confirm.take() else {
            return;
        };
        if self
            .subs
            .form
            .as_ref()
            .is_some_and(|(form_id, _)| *form_id == Some(id))
        {
            self.subs.form = None;
        }
        if let Some(feeds) = subscriptions::feeds(cx) {
            feeds.update(cx, |feeds, cx| feeds.delete(id, cx));
        }
        cx.notify();
    }

    pub(super) fn render_china_holiday_source(
        &self,
        current: Option<(fern_core::Id, ChinaHolidaySource)>,
        enabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let hover = cx.theme().colors().ghost_element_hover;
        let subscription = current.map(|(id, _)| id);
        let selected = current.map(|(_, source)| source);
        let menu = Menu::ChinaHolidays(subscription);
        let open = enabled && self.menu == Some(menu);
        let dropdown = open.then(|| {
            PopoverAttachment::new(
                Placement::BelowLeft,
                popover(cx)
                    .min_w(px(156.))
                    .debug_selector(|| "china-holiday-source-menu".to_owned())
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_menu(None, cx)))
                    .children(ChinaHolidaySource::ALL.map(|source| {
                        h_flex()
                            .id(source.label())
                            .debug_selector(move || source.label().to_owned())
                            .tab_index(0)
                            .h(px(32.))
                            .gap_2()
                            .px_2()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|style| style.bg(hover))
                            .focus_visible(|style| style.bg(hover))
                            .child(div().flex_1().child(Label::new(source.label())))
                            .when(selected == Some(source), |el| {
                                el.child(
                                    Icon::new(IconName::Check)
                                        .color(Color::Accent)
                                        .size(IconSize::Small),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_menu(None, cx);
                                cx.stop_propagation();
                                if selected != Some(source)
                                    && let Some(feeds) = subscriptions::feeds(cx)
                                {
                                    feeds.update(cx, |feeds, cx| {
                                        feeds.select_china_holiday_source(
                                            subscription,
                                            source,
                                            strings.holidays_name,
                                            cx,
                                        );
                                    });
                                }
                            }))
                    })),
            )
        });
        let color = if enabled { Color::Accent } else { Color::Muted };
        h_flex()
            .id(match subscription {
                Some(id) => SharedString::from(format!("china-holiday-source-{id}")),
                None => SharedString::from("subscription-holidays"),
            })
            .debug_selector(|| "china-holiday-source".to_owned())
            .relative()
            .flex_none()
            .gap_1()
            .child(
                Label::new(selected.map_or(strings.add_holidays, ChinaHolidaySource::label))
                    .color(color)
                    .when(selected.is_some(), |label| label.size(LabelSize::Small)),
            )
            .when(enabled, |el| {
                el.cursor_pointer()
                    .tab_index(0)
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(color))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_menu((!open).then_some(menu), cx)
                    }))
                    .when(selected.is_some(), |el| {
                        el.tooltip(Tooltip::text(strings.subscription_change_source))
                    })
            })
            .children(dropdown)
            .into_any_element()
    }

    pub(super) fn render_integrations(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let Some(feeds) = subscriptions::feeds(cx) else {
            return Vec::new();
        };
        let feeds = feeds.read(cx);
        let chinese = i18n::locale(cx) == Locale::ZhCn;
        let china_status = feeds.china_holiday_status();
        let china_sources = ChinaHolidaySource::ALL.map(|source| {
            feeds.calendars().iter().any(|calendar| {
                ChinaHolidaySource::from_url(&calendar.subscription.url) == Some(source)
            })
        });
        let has_china_holidays = china_sources.into_iter().any(|installed| installed);
        let can_choose_source = china_status != ChinaHolidayStatus::Reading
            && !china_sources.into_iter().all(|installed| installed);
        let rows: Vec<AnyElement> = feeds
            .calendars()
            .iter()
            .map(|calendar| {
                let subscription = &calendar.subscription;
                let id = subscription.id;
                let hidden = subscription.hidden;
                let china_source = ChinaHolidaySource::from_url(&subscription.url);
                let status = if feeds.is_reading(id) {
                    (strings.subscription_reading.to_owned(), Color::Muted)
                } else if let Some(error) = &subscription.error {
                    (error.clone(), Color::Error)
                } else {
                    (
                        strings
                            .subscription_events
                            .replace("{n}", &calendar.events.len().to_string()),
                        Color::Muted,
                    )
                };
                let dot = lists_panel::parse_color(subscription.color.as_deref()).unwrap_or(colors.accent);
                h_flex()
                    .id(SharedString::from(format!("subscription-{id}")))
                    .when_some(subscription.error.as_ref(), |el, error| {
                        el.tooltip(Tooltip::text(error.clone()))
                    })
                    .min_h(px(ROW_HEIGHT))
                    .px_4()
                    .gap_3()
                    .child(div().size(px(10.)).flex_none().rounded_full().bg(dot))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .child(
                                        Label::new(subscription.title().to_owned())
                                            .truncate()
                                            .color(if hidden {
                                                Color::Muted
                                            } else {
                                                Color::Default
                                            }),
                                    )
                                    .when(subscription.is_caldav(), |el| {
                                        el.child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.))
                                                .rounded(px(4.))
                                                .bg(colors.accent.opacity(0.14))
                                                .child(
                                                    Label::new(strings.cal_badge_caldav)
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Accent),
                                                ),
                                        )
                                    }),
                            )
                            .child(Label::new(status.0).size(LabelSize::XSmall).color(status.1).truncate()),
                    )
                    .children(china_source.map(|source| {
                        self.render_china_holiday_source(Some((id, source)), can_choose_source, cx)
                    }))
                    .child(
                        div()
                            .debug_selector(move || format!("subscription-show-{id}"))
                            .child(
                                IconButton::new(
                                    SharedString::from(format!("subscription-show-{id}")),
                                    if hidden { IconName::EyeOff } else { IconName::Eye },
                                )
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text(if hidden {
                                    strings.subscription_show
                                } else {
                                    strings.subscription_hide
                                }))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    if let Some(feeds) = subscriptions::feeds(cx) {
                                        feeds.update(cx, |feeds, cx| {
                                            if let Some(s) = feeds
                                                .calendars()
                                                .iter()
                                                .find(|c| c.subscription.id == id)
                                                .map(|c| c.subscription.clone())
                                            {
                                                if s.is_caldav() {
                                                    feeds.update_caldav(
                                                        id,
                                                        &s.name,
                                                        &s.url,
                                                        s.color.as_deref(),
                                                        !hidden,
                                                        s.provider,
                                                        &s.username,
                                                        &s.password,
                                                        s.writable,
                                                        cx,
                                                    );
                                                } else {
                                                    feeds.update_subscription(
                                                        id,
                                                        &s.name,
                                                        &s.url,
                                                        s.color.as_deref(),
                                                        !hidden,
                                                        cx,
                                                    );
                                                }
                                            }
                                        });
                                    }
                                })),
                            ),
                    )
                    .child(
                        IconButton::new(
                            SharedString::from(format!("subscription-read-{id}")),
                            IconName::RefreshCw,
                        )
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text(strings.subscription_read_now))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(feeds) = subscriptions::feeds(cx) {
                                feeds.update(cx, |feeds, cx| feeds.read(id, cx));
                            }
                        })),
                    )
                    .child(
                        IconButton::new(
                            SharedString::from(format!("subscription-edit-{id}")),
                            IconName::Pencil,
                        )
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text(strings.subscription_edit))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_subscription_form(Some(id), window, cx)
                        })),
                    )
                    .child(
                        div()
                            .when(cfg!(test), |el| {
                                el.debug_selector(move || format!("subscription-delete-{id}"))
                            })
                            .child(
                                IconButton::new(
                                    SharedString::from(format!("subscription-delete-{id}")),
                                    IconName::Trash,
                                )
                                .icon_color(Color::Muted)
                                .hover_icon_color(Color::Error)
                                .tooltip(Tooltip::text(strings.subscription_delete))
                                .on_click({
                                    let title = subscription.title().to_owned();
                                    cx.listener(move |this, _, _, cx| {
                                        this.confirm = Some(ActiveConfirm::DeleteSubscription {
                                            id,
                                            title: title.clone(),
                                        });
                                        cx.notify();
                                    })
                                }),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        let group = || v_flex().py_1p5().rounded(px(8.)).bg(colors.surface_background);
        let field = |label: &'static str, input: Entity<TextInput>| {
            h_flex()
                .h(px(40.))
                .px_4()
                .gap_4()
                .child(div().w(px(80.)).child(Label::new(label)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .h(px(28.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .bg(colors.background)
                        .child(input),
                )
        };
        let form = self.subs.form.as_ref().map(|(_, color)| {
            let has_url = !self.subs.url.read(cx).text().trim().is_empty();
            let swatches = h_flex().gap_1p5().children(
                std::iter::once(None)
                    .chain(lists_panel::LIST_COLORS.iter().map(|c| Some(*c)))
                    .map(|swatch| {
                        let selected = color.as_deref() == swatch;
                        let fill = lists_panel::parse_color(swatch).unwrap_or(colors.accent);
                        div()
                            .id(SharedString::from(format!(
                                "subscription-color-{}",
                                swatch.unwrap_or("none")
                            )))
                            .size(px(20.))
                            .rounded_full()
                            .cursor_pointer()
                            .bg(fill.opacity(if swatch.is_none() { 0.35 } else { 1. }))
                            .when(selected, |el| el.border_2().border_color(colors.text))
                            .when(!selected, |el| el.hover(|s| s.border_2().border_color(colors.border)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some((_, color)) = &mut this.subs.form {
                                    *color = swatch.map(str::to_owned);
                                }
                                cx.notify();
                            }))
                    }),
            );
            let is_caldav = self.subs.kind == CalendarKind::CalDav;
            let set_kind = |kind: CalendarKind| {
                cx.listener(move |this, _, _, cx| {
                    this.subs.kind = kind;
                    if kind == CalendarKind::CalDav
                        && this.subs.provider == CalendarProvider::Url
                    {
                        this.subs.provider = CalendarProvider::CalDav;
                    }
                    cx.notify();
                })
            };
            let provider_label = |p: CalendarProvider| match p {
                CalendarProvider::Url => strings.cal_provider_url,
                CalendarProvider::CalDav => strings.cal_provider_caldav,
                CalendarProvider::ICloud => strings.cal_provider_icloud,
                CalendarProvider::Google => strings.cal_provider_google,
                CalendarProvider::Outlook => strings.cal_provider_outlook,
                CalendarProvider::Nextcloud => strings.cal_provider_nextcloud,
                CalendarProvider::Fastmail => strings.cal_provider_fastmail,
            };
            group()
                .child(
                    div().px_4().py_2().child(
                        div()
                            .debug_selector(|| "subscription-kind-tabs".to_owned())
                            .child(
                                SegmentedTabs::new()
                                    .full_width()
                                    .tab(
                                        SegmentedTab::new("subscription-kind-ics", strings.cal_source_ics)
                                            .toggle_state(!is_caldav)
                                            .on_click(set_kind(CalendarKind::Ics)),
                                    )
                                    .tab(
                                        SegmentedTab::new(
                                            "subscription-kind-caldav",
                                            strings.cal_source_caldav,
                                        )
                                        .toggle_state(is_caldav)
                                        .on_click(set_kind(CalendarKind::CalDav)),
                                    ),
                            ),
                    ),
                )
                .when(is_caldav, |el| {
                    el.child(
                        h_flex()
                            .min_h(px(36.))
                            .px_4()
                            .gap_4()
                            .child(div().w(px(80.)).child(Label::new(strings.cal_provider)))
                            .child(
                                h_flex()
                                    .flex_1()
                                    .flex_wrap()
                                    .gap_1()
                                    .children(CalendarProvider::CALDAV_PRESETS.into_iter().map(|preset| {
                                        let active = self.subs.provider == preset;
                                        let label = provider_label(preset);
                                        div()
                                            .id(SharedString::from(format!("caldav-provider-{}", preset.code())))
                                            .debug_selector(move || format!("caldav-provider-{}", preset.code()))
                                            .px_2()
                                            .h(px(24.))
                                            .flex()
                                            .items_center()
                                            .rounded(px(5.))
                                            .border_1()
                                            .border_color(if active { colors.accent } else { colors.border })
                                            .when(active, |el| el.bg(colors.accent.opacity(0.14)))
                                            .when(!active, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
                                            .cursor_pointer()
                                            .child(
                                                Label::new(label)
                                                    .size(LabelSize::XSmall)
                                                    .color(if active {
                                                        Color::Accent
                                                    } else {
                                                        Color::Default
                                                    }),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.subs.provider = preset;
                                                let suggested = preset.default_discovery_url();
                                                if !suggested.is_empty() {
                                                    let cur = this.subs.url.read(cx).text().trim().to_owned();
                                                    let is_preset_url = cur.is_empty()
                                                        || CalendarProvider::CALDAV_PRESETS
                                                            .iter()
                                                            .any(|p| p.default_discovery_url() == cur);
                                                    if is_preset_url {
                                                        this.subs.url.update(cx, |input, cx| {
                                                            input.set_text(suggested, cx)
                                                        });
                                                    }
                                                }
                                                cx.notify();
                                            }))
                                    })),
                            ),
                    )
                })
                .child(field(strings.subscription_name, self.subs.name.clone()))
                .child(field("URL", self.subs.url.clone()))
                .when(is_caldav, |el| {
                    el.child(field(strings.cal_username, self.subs.username.clone()))
                        .child(field(strings.cal_password, self.subs.password.clone()))
                        .child(
                            h_flex()
                                .id("caldav-writable-toggle")
                                .h(px(36.))
                                .px_4()
                                .gap_4()
                                .cursor_pointer()
                                .child(
                                    div()
                                        .flex_1()
                                        .child(Label::new(strings.cal_writable).size(LabelSize::Small)),
                                )
                                .child(ui::Switch::new().toggle_state(self.subs.writable))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.subs.writable = !this.subs.writable;
                                    cx.notify();
                                })),
                        )
                        .child(
                            h_flex()
                                .px_4()
                                .py_1()
                                .gap_2()
                                .child(
                                    Button::new(
                                        "caldav-discover",
                                        if self.subs.discovering {
                                            "…"
                                        } else {
                                            strings.cal_discover
                                        },
                                    )
                                    .variant(ButtonVariant::Secondary)
                                    .size(ButtonSize::Sm)
                                    .disabled(!has_url || self.subs.discovering)
                                    .on_click(cx.listener(|this, _, _, cx| this.discover_caldav(cx))),
                                )
                                .when_some(self.subs.discover_error.as_ref(), |el, err| {
                                    el.child(
                                        div().flex_1().min_w_0().child(
                                            Label::new(err.clone())
                                                .size(LabelSize::XSmall)
                                                .color(Color::Error)
                                                .truncate(),
                                        ),
                                    )
                                }),
                        )
                        .when(!self.subs.discovered.is_empty(), |el| {
                            el.child(
                                h_flex()
                                    .px_4()
                                    .py_1()
                                    .flex_wrap()
                                    .gap_1p5()
                                    .children(self.subs.discovered.iter().enumerate().map(|(i, cal)| {
                                        let cal = cal.clone();
                                        let selected = self.subs.url.read(cx).text().trim() == cal.url;
                                        div()
                                            .id(SharedString::from(format!("caldav-discovered-{i}")))
                                            .px_2p5()
                                            .h(px(26.))
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .rounded(px(6.))
                                            .border_1()
                                            .border_color(if selected {
                                                colors.accent
                                            } else {
                                                colors.border
                                            })
                                            .when(selected, |el| el.bg(colors.accent.opacity(0.12)))
                                            .when(!selected, |el| {
                                                el.hover(|s| s.bg(colors.ghost_element_hover))
                                            })
                                            .cursor_pointer()
                                            .child(
                                                Icon::new(IconName::CalendarPlus)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Muted),
                                            )
                                            .child(Label::new(cal.name.clone()).size(LabelSize::XSmall))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let name = cal.name.clone();
                                                let url = cal.url.clone();
                                                this.subs
                                                    .name
                                                    .update(cx, |input, cx| input.set_text(name, cx));
                                                this.subs
                                                    .url
                                                    .update(cx, |input, cx| input.set_text(url, cx));
                                                this.subs.writable = cal.writable;
                                                cx.notify();
                                            }))
                                    })),
                            )
                        })
                })
                .child(
                    h_flex()
                        .h(px(40.))
                        .px_4()
                        .gap_4()
                        .child(div().w(px(80.)).child(Label::new(strings.color)))
                        .child(swatches),
                )
                .child(
                    h_flex()
                        .px_4()
                        .py_2()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("subscription-cancel", strings.cancel)
                                .variant(ButtonVariant::Secondary)
                                .size(ButtonSize::Sm)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.subs.form = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("subscription-save", strings.ok)
                                .variant(ButtonVariant::Primary)
                                .size(ButtonSize::Sm)
                                .disabled(!has_url)
                                .on_click(cx.listener(|this, _, _, cx| this.save_subscription_form(cx))),
                        ),
                )
                .into_any_element()
        });
        let mut page = vec![
            div()
                .pb_3()
                .child(Label::new(strings.subscribed_calendars).weight(FontWeight::MEDIUM))
                .into_any_element(),
            div()
                .pb_2()
                .px_1()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(strings.subscriptions_hint)
                .into_any_element(),
        ];
        if !rows.is_empty() {
            page.push(group().children(rows).into_any_element());
        }
        if chinese || has_china_holidays {
            let message = match china_status {
                ChinaHolidayStatus::Idle => None,
                ChinaHolidayStatus::Reading => Some((strings.subscription_reading, Color::Muted)),
                ChinaHolidayStatus::Failed => Some((strings.holiday_source_failed, Color::Error)),
            };
            if let Some((text, color)) = message {
                page.push(
                    div()
                        .px_1()
                        .child(Label::new(text).size(LabelSize::Small).color(color))
                        .into_any_element(),
                );
            }
        }
        match form {
            Some(form) => page.push(form),
            None => {
                let has_holidays = if chinese {
                    has_china_holidays
                } else {
                    feeds
                        .calendars()
                        .iter()
                        .any(|c| c.subscription.url == strings.holidays_url)
                };
                page.push(
                    h_flex()
                        .px_1()
                        .gap_5()
                        .child(
                            div()
                                .id("subscription-add")
                                .cursor_pointer()
                                .child(Label::new(strings.add_calendar).color(Color::Accent))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_subscription_form(None, window, cx)
                                })),
                        )
                        .when(!has_holidays, |el| {
                            if chinese {
                                return el.child(self.render_china_holiday_source(
                                    None,
                                    can_choose_source,
                                    cx,
                                ));
                            }
                            el.child(
                                div()
                                    .id("subscription-holidays")
                                    .cursor_pointer()
                                    .child(Label::new(strings.add_holidays).color(Color::Accent))
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        let strings = i18n::t(cx);
                                        if let Some(feeds) = subscriptions::feeds(cx) {
                                            feeds.update(cx, |feeds, cx| {
                                                feeds.add(
                                                    strings.holidays_name,
                                                    strings.holidays_url,
                                                    Some("#22C55E"),
                                                    cx,
                                                )
                                            });
                                        }
                                    })),
                            )
                        })
                        .into_any_element(),
                )
            }
        }
        page
    }
}
