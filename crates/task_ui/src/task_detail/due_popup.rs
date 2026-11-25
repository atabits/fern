use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Frequency, Recurrence};
use ui::prelude::*;
use ui::popover;

use crate::calendar;
use crate::due_picker::{self, CustomReminder, DueDraft, DueMenu, DueTab, RangeField, ReminderUnit};
use crate::i18n::{self, Strings};
use crate::task_rows;

use super::{TaskDetail, check_mark};

impl TaskDetail {
    fn render_duration(
        &self,
        draft: &DueDraft,
        calendar_block: AnyElement,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let all_day = draft.is_all_day();
        let chip = |id: &'static str, label: SharedString, field: RangeField, cx: &mut Context<Self>| {
            let active = draft.range_field == Some(field);
            div()
                .id(id)
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .border_1()
                .border_color(if active { theme.colors().accent } else { theme.colors().border })
                .cursor_pointer()
                .child(Label::new(label).size(LabelSize::Small))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_draft(cx, |d| {
                        d.range_field = if d.range_field == Some(field) { None } else { Some(field) };
                        let shown = if field == RangeField::EndDate { d.end_date } else { d.date };
                        if let Some(date) = shown {
                            d.month = calendar::month_start(date);
                        }
                    })
                }))
        };
        let day = |date: Option<NaiveDate>| -> SharedString {
            date.map_or(SharedString::default(), |d| task_rows::short_date(d, strings).into())
        };
        let start_time_str: SharedString =
            draft.time.map_or(SharedString::default(), |t| crate::calendar::format_time(t, strings, cx).into());
        let end_time_str: SharedString =
            draft.end_time.map_or(SharedString::default(), |t| crate::calendar::format_time(t, strings, cx).into());
        let line = |label: &'static str, date_chip: AnyElement, time_chip: Option<AnyElement>| {
            h_flex()
                .gap_2()
                .child(div().w(px(40.)).child(Label::new(label).size(LabelSize::Small)))
                .child(date_chip)
                .children(time_chip)
        };
        let start_date = chip("range-start-date", day(draft.date), RangeField::StartDate, cx).into_any_element();
        let start_time = (!all_day).then(|| chip("range-start-time", start_time_str, RangeField::StartTime, cx).into_any_element());
        let end_date = chip("range-end-date", day(draft.end_date), RangeField::EndDate, cx).into_any_element();
        let end_time = (!all_day).then(|| chip("range-end-time", end_time_str, RangeField::EndTime, cx).into_any_element());
        let picker: Option<AnyElement> = match draft.range_field {
            Some(RangeField::StartDate | RangeField::EndDate) => Some(calendar_block),
            Some(field @ (RangeField::StartTime | RangeField::EndTime)) => {
                let current = if field == RangeField::EndTime { draft.end_time } else { draft.time };
                Some(
                    v_scroll(
                        "range-times-scroll",
                        v_flex()
                            .id("range-times")
                            .children(due_picker::time_options().into_iter().map(|t| {
                                ListItem::new(SharedString::from(format!("range-time-{t}")))
                                    .height(px(28.))
                                    .child(Label::new(crate::calendar::format_time(t, strings, cx)).size(LabelSize::Small))
                                    .when(current == Some(t), |item| item.end_slot(check_mark()))
                                    .on_click(cx.listener(move |this, _, _, cx| this.update_draft(cx, |d| d.pick_range_time(t))))
                            })),
                    )
                    .max_h(px(180.))
                    .compact()
                    .into_any_element(),
                )
            }
            None => None,
        };
        v_flex()
            .gap_2()
            .child(line(strings.range_start, start_date, start_time))
            .child(line(strings.range_end, end_date, end_time))
            .children(picker)
            .child(
                h_flex()
                    .id("range-all-day")
                    .cursor_pointer()
                    .child(div().flex_1().child(Label::new(strings.range_all_day).size(LabelSize::Small)))
                    .child(Switch::new().toggle_state(all_day))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_draft(cx, |d| d.set_all_day(!all_day)))),
            )
            .child(Divider::horizontal())
            .into_any_element()
    }

    pub(super) fn render_due_popup(&self, strings: &'static Strings, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        self.render_due_popup_in(window.viewport_size() - gpui::size(px(16.), px(16.)), strings, cx)
    }

    pub(super) fn render_due_popup_in(&self, available: gpui::Size<Pixels>, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = self.session.due_draft.clone() else { return div().into_any_element() };
        let theme = cx.theme().clone();
        let locale = i18n::locale(cx);
        let today = Local::now().date_naive();
        let month = draft.month;
        let [d_today, d_tomorrow, d_week, d_month] = due_picker::quick_dates(today, cx);

        let quick = |id: &'static str, icon: IconName, tip: &'static str, date: NaiveDate, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex_1()
                .h(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .tooltip(Tooltip::text(tip))
                .child(Icon::new(icon).size(IconSize::XLarge))
                .on_click(cx.listener(move |this, _, _, cx| this.update_draft(cx, |d| d.pick_date(date))))
        };

        let duration = draft.tab == DueTab::Duration;
        let picked = if draft.range_field == Some(RangeField::EndDate) {
            draft.end_date
        } else {
            draft.date
        };
        let other_endpoint = if duration {
            [draft.date, draft.end_date]
                .into_iter()
                .flatten()
                .find(|d| Some(*d) != picked)
        } else {
            None
        };
        let mut grid_cfg = desktop_common::month_calendar::MonthGridConfig::single("", month, today);
        grid_cfg.selected = picked;
        grid_cfg.other_endpoint = other_endpoint;
        grid_cfg.range = if duration {
            match (draft.date, draft.end_date) {
                (Some(s), Some(e)) => Some((s.min(e), s.max(e))),
                _ => None,
            }
        } else {
            None
        };
        grid_cfg.debug_prefix = Some("due");
        grid_cfg.cell_size = px(28.);
        let entity_pick = cx.entity();
        let weeks = desktop_common::month_calendar::render_month_grid(&grid_cfg, cx, move |date, _, cx| {
            entity_pick.update(cx, |this, cx| {
                this.update_draft(cx, |d| {
                    if duration {
                        d.pick_range_date(date)
                    } else {
                        d.pick_date(date)
                    }
                })
            });
        });

        let row = |id: &'static str, icon: IconName, label: SharedString, active: bool, menu: DueMenu, cx: &mut Context<Self>| {
            let color = if active { Color::Accent } else { Color::Default };
            h_flex()
                .id(id)
                .h(px(32.))
                .px_1()
                .gap_2()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(Icon::new(icon).color(if active { Color::Accent } else { Color::Muted }))
                .child(div().flex_1().child(Label::new(label).size(LabelSize::Small).color(color)))
                .child(
                    Icon::new(if draft.menu.as_ref() == Some(&menu) || (menu == DueMenu::Repeat && matches!(draft.menu, Some(DueMenu::Custom(_))))
                        || (menu == DueMenu::Reminder && matches!(draft.menu, Some(DueMenu::CustomReminder(_)))) {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                        .size(IconSize::XSmall),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let opened_tz = menu == DueMenu::TimeZone
                        && this.session.due_draft.as_ref().is_some_and(|d| d.menu.as_ref() != Some(&menu));
                    if opened_tz {
                        this.ensure_tz_input(cx);
                    }
                    this.update_draft(cx, |d| d.menu = if d.menu.as_ref() == Some(&menu) { None } else { Some(menu.clone()) })
                }))
        };
        let time_label: SharedString =
            draft.time.map_or(strings.time.into(), |t| crate::calendar::format_time(t, strings, cx).into());
        let repeat_label: SharedString =
            draft.repeat.as_ref().map_or(strings.repeat.into(), |r| due_picker::repeat_label(r, strings).into());

        let menu: Option<AnyElement> = match &draft.menu {
            Some(DueMenu::Time) => Some(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .h(px(28.))
                            .px_2()
                            .rounded(px(6.))
                            .bg(theme.colors().background)
                            .text_size(px(12.))
                            .child(self.time_input.clone()),
                    )
                    .child(
                        v_scroll(
                            "time-options-scroll",
                            v_flex()
                                .id("time-options")
                                .children(
                                    std::iter::once(None)
                                        .chain(due_picker::time_options().into_iter().map(Some))
                                        .map(|time| {
                                            let label: SharedString = time.map_or(strings.no_time.into(), |t| crate::calendar::format_time(t, strings, cx).into());
                                            ListItem::new(SharedString::from(format!("time-{label}")))
                                                .height(px(28.))
                                                .child(Label::new(label).size(LabelSize::Small))
                                                .when(draft.time == time, |item| item.end_slot(check_mark()))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.update_draft(cx, |d| d.pick_time(time, Local::now().date_naive()))
                                                }))
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                        )
                        .max_h(px(180.))
                        .compact(),
                    )
                    .into_any_element(),
            ),
            Some(DueMenu::TimeZone) => {
                let query = self
                    .tz_input
                    .as_ref()
                    .map(|input| input.read(cx).text().to_owned())
                    .unwrap_or_default();
                let options = due_picker::timezone_options(&query);
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            ListItem::new("tz-local")
                                .height(px(28.))
                                .child(Label::new(strings.timezone_local).size(LabelSize::Small))
                                .when(draft.timezone.is_none() && !draft.is_floating, |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_draft(cx, |d| {
                                        d.timezone = None;
                                        d.is_floating = false;
                                        d.menu = None;
                                    })
                                })),
                        )
                        .child(
                            div()
                                .id("tz-floating-tip")
                                .tooltip(Tooltip::text(strings.timezone_floating_hint))
                                .child(
                                    ListItem::new("tz-floating")
                                        .height(px(28.))
                                        .child(Label::new(strings.timezone_floating).size(LabelSize::Small))
                                        .when(draft.is_floating, |item| item.end_slot(check_mark()))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.update_draft(cx, |d| {
                                                d.is_floating = true;
                                                d.menu = None;
                                            })
                                        })),
                                ),
                        )
                        .child(div().py_0p5().child(Divider::horizontal()))
                        .when_some(self.tz_input.clone(), |el, input| {
                            el.child(
                                h_flex()
                                    .h(px(28.))
                                    .px_2()
                                    .rounded(px(6.))
                                    .bg(theme.colors().background)
                                    .text_size(px(12.))
                                    .child(input),
                            )
                        })
                        .child(
                            v_scroll(
                                "tz-options-scroll",
                                v_flex().id("tz-options").children(options.into_iter().map(|(name, offset)| {
                                    let selected = !draft.is_floating && draft.timezone.as_deref() == Some(name.as_str());
                                    let pick_name = name.clone();
                                    ListItem::new(SharedString::from(format!("tz-{name}")))
                                        .height(px(28.))
                                        .child(
                                            h_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .gap_1()
                                                .child(div().flex_1().min_w_0().child(Label::new(name).size(LabelSize::Small).truncate()))
                                                .child(Label::new(offset).size(LabelSize::XSmall).color(Color::Muted)),
                                        )
                                        .when(selected, |item| item.end_slot(check_mark()))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let pick_name = pick_name.clone();
                                            this.update_draft(cx, move |d| {
                                                d.timezone = Some(pick_name);
                                                d.is_floating = false;
                                                d.menu = None;
                                            })
                                        }))
                                })),
                            )
                            .max_h(px(160.))
                            .compact(),
                        )
                        .into_any_element(),
                )
            }
            Some(DueMenu::Custom(rule)) => Some(self.render_custom_repeat(rule.clone(), strings, cx)),
            Some(DueMenu::Reminder) => Some(
                v_flex()
                    .children(due_picker::reminder_options(draft.time.is_some(), strings, cx).into_iter().map(
                        |(minutes, label, hint)| {
                            ListItem::new(SharedString::from(format!("reminder-{minutes}")))
                                .height(px(28.))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(Label::new(label).size(LabelSize::Small))
                                        .when(!hint.is_empty(), |el| {
                                            el.child(Label::new(format!("({hint})")).size(LabelSize::Small).color(Color::Muted))
                                        }),
                                )
                                .when(draft.reminders.contains(&minutes), |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_draft(cx, |d| d.toggle_reminder(minutes, Local::now().date_naive()))
                                }))
                        },
                    ))
                    // Custom reminders already chosen, so they can be turned off again.
                    .children(
                        draft
                            .reminders
                            .iter()
                            .filter(|m| !due_picker::reminder_options(draft.time.is_some(), strings, cx).iter().any(|(o, _, _)| o == *m))
                            .map(|&minutes| {
                                ListItem::new(SharedString::from(format!("reminder-custom-{minutes}")))
                                    .height(px(28.))
                                    .child(
                                        Label::new(due_picker::describe_reminder(minutes, draft.time.is_some(), strings, cx))
                                            .size(LabelSize::Small),
                                    )
                                    .end_slot(check_mark())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_draft(cx, |d| d.toggle_reminder(minutes, Local::now().date_naive()))
                                    }))
                            })
                            .collect::<Vec<_>>(),
                    )
                    .child(div().py_1().child(Divider::horizontal()))
                    .child(
                        h_flex()
                            .id("reminder-constant")
                            .h(px(28.))
                            .px_2()
                            .gap_2()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.colors().element_hover))
                            .tooltip(Tooltip::text(strings.annoying_alert_hint))
                            .child(Checkbox::new("reminder-constant-box").size(px(14.)).toggle_state(draft.constant_reminder))
                            .child(Label::new(strings.annoying_alert).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let today = Local::now().date_naive();
                                this.update_draft(cx, |d| {
                                    d.constant_reminder = !d.constant_reminder;
                                    if d.constant_reminder && d.reminders.is_empty() {
                                        d.toggle_reminder(0, today);
                                    }
                                })
                            })),
                    )
                    .child(div().py_1().child(Divider::horizontal()))
                    .child(
                        ListItem::new("reminder-custom")
                            .height(px(28.))
                            .child(Label::new(strings.repeat_custom).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_draft(cx, |d| {
                                    d.menu = Some(DueMenu::CustomReminder(CustomReminder::new(d.time.is_some())))
                                })
                            })),
                    )
                    .into_any_element(),
            ),
            Some(DueMenu::CustomReminder(custom)) => Some(self.render_custom_reminder(*custom, &draft, strings, cx)),
            Some(DueMenu::Repeat) => {
                let base = draft.date.unwrap_or(today);
                let options = due_picker::repeat_options(base, strings, locale);
                let extended = due_picker::extended_repeat_options(base, strings, locale);
                Some(
                    v_flex()
                        .child(
                            ListItem::new("repeat-none")
                                .height(px(28.))
                                .child(Label::new(strings.repeat_none).size(LabelSize::Small))
                                .when(draft.repeat.is_none(), |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_draft(cx, |d| d.pick_repeat(None, Local::now().date_naive()))
                                })),
                        )
                        .children(options.into_iter().map(|(rule, label, hint)| {
                            ListItem::new(SharedString::from(format!("repeat-{rule}")))
                                .height(px(28.))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(Label::new(label).size(LabelSize::Small))
                                        .when(!hint.is_empty(), |el| {
                                            el.child(Label::new(format!("({hint})")).size(LabelSize::Small).color(Color::Muted))
                                        }),
                                )
                                .when(draft.repeat.as_ref() == Some(&rule), |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_draft(cx, |d| d.pick_repeat(Some(rule.clone()), Local::now().date_naive()))
                                }))
                        }))
                        .child(div().py_1().child(Divider::horizontal()))
                        .children(extended.into_iter().map(|(rule, label, hint)| {
                            ListItem::new(SharedString::from(format!("repeat-ext-{rule}")))
                                .height(px(28.))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(Label::new(label).size(LabelSize::Small))
                                        .when(!hint.is_empty(), |el| {
                                            el.child(Label::new(format!("({hint})")).size(LabelSize::Small).color(Color::Muted))
                                        }),
                                )
                                .when(draft.repeat.as_ref() == Some(&rule), |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_draft(cx, |d| d.pick_repeat(Some(rule.clone()), Local::now().date_naive()))
                                }))
                        }))
                        .child(div().py_1().child(Divider::horizontal()))
                        .child(
                            ListItem::new("repeat-custom")
                                .height(px(28.))
                                .child(Label::new(strings.repeat_custom).size(LabelSize::Small))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let today = Local::now().date_naive();
                                    this.update_draft(cx, |d| {
                                        d.menu = Some(DueMenu::Custom(due_picker::custom_start(d.repeat.clone(), d.date.unwrap_or(today))))
                                    })
                                })),
                        )
                        .into_any_element(),
                )
            }
            None => None,
        };
        let (time_menu, tz_menu, reminder_menu, repeat_menu) = match &draft.menu {
            Some(DueMenu::Time) => (menu, None, None, None),
            Some(DueMenu::TimeZone) => (None, menu, None, None),
            Some(DueMenu::Reminder | DueMenu::CustomReminder(_)) => (None, None, menu, None),
            Some(DueMenu::Repeat | DueMenu::Custom(_)) => (None, None, None, menu),
            None => (None, None, None, None),
        };
        let tz_label: SharedString = due_picker::short_timezone_label(draft.timezone.as_deref(), draft.is_floating, strings)
            .unwrap_or_else(|| strings.timezone_local.to_owned())
            .into();
        let reminder_label: SharedString = due_picker::reminder_label(&draft.reminders, draft.time.is_some(), strings, cx)
            .map_or(strings.reminder.into(), Into::into);

        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };

        let quick_row = h_flex()
                    .child(quick("due-today", IconName::Sun, strings.today, d_today, cx))
                    .child(quick("due-tomorrow", IconName::Sunrise, strings.tomorrow, d_tomorrow, cx))
                    .child(quick("due-week", IconName::CalendarPlus, strings.next_week, d_week, cx))
                    .child(quick("due-month", IconName::Moon, strings.next_month, d_month, cx));
        let horizon_row: Option<AnyElement> = calendar::horizons_enabled(cx).then(|| {
            use fern_core::Horizon;
            let week = Horizon::current_week(today);
            let month = Horizon::current_month(today);
            let quarter = Horizon::current_quarter(today);
            let year = Horizon::current_year(today);
            let q_short = format!("{}{}", strings.horizons_quarter_prefix, (today.month() - 1) / 3 + 1);
            let chips = [
                ("week", calendar::format_horizon_badge(week, strings), strings.horizon_this_week, week),
                ("month", calendar::format_horizon_badge(month, strings), strings.horizon_this_month, month),
                ("quarter", q_short, strings.horizon_this_quarter, quarter),
                ("year", today.year().to_string(), strings.horizon_this_year, year),
                ("life", strings.horizons_life.to_owned(), strings.horizons_life, Horizon::Life),
            ];
            h_flex()
                .gap(px(4.))
                .children(chips.into_iter().map(|(key, label, tip, h)| {
                    let active = draft.horizon == Some(h);
                    div()
                        .id(SharedString::from(format!("due-horizon-{key}")))
                        .debug_selector(move || format!("due-horizon-{key}"))
                        .flex_1()
                        .h(px(24.))
                        .px(px(3.))
                        .rounded(px(5.))
                        .border_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .border_color(if active {
                            theme.colors().accent.opacity(0.65)
                        } else {
                            theme.colors().border_variant
                        })
                        .bg(if active {
                            theme.colors().accent.opacity(0.14)
                        } else {
                            theme.colors().background.opacity(0.5)
                        })
                        .when(!active, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                        .tooltip(Tooltip::text(tip))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_draft(cx, |d| {
                                let next = if d.horizon == Some(h) { None } else { Some(h) };
                                d.set_horizon(next);
                            });
                        }))
                        .child(
                            Label::new(label)
                                .size(LabelSize::XSmall)
                                .color(if active { Color::Accent } else { Color::Muted })
                                .truncate(),
                        )
                }))
                .into_any_element()
        });
        let entity_shift = cx.entity();
        let entity_today = cx.entity();
        let mut header_cfg =
            desktop_common::month_calendar::MonthHeaderConfig::surface(month, strings, "prev-month", "next-month");
        header_cfg.today_id = Some("this-month");
        header_cfg.today_tooltip = Some(strings.this_month);
        let calendar_block = v_flex()
            .gap_2()
            .child(div().px_1().child(desktop_common::month_calendar::render_month_header(
                header_cfg,
                cx,
                move |delta, _, cx| {
                    entity_shift.update(cx, |this, cx| this.shift_month(delta, cx));
                },
                Some(Box::new(move |_, cx| {
                    entity_today.update(cx, |this, cx| {
                        let today = Local::now().date_naive();
                        this.update_draft(cx, |d| d.month = calendar::month_start(today));
                    });
                })),
            )))
            .child(desktop_common::month_calendar::render_weekday_row(
                strings,
                desktop_common::month_calendar::WeekdayStyle::Narrow,
                false,
                cx,
            ))
            .child(v_flex().gap_0p5().children(weeks))
            .into_any_element();
        let content = v_flex()
            .flex_none()
            .gap_2()
            .child(h_flex().p_0p5().rounded(px(6.)).bg(theme.colors().background).children(
                [(DueTab::Date, "due-tab-date", strings.date_tab), (DueTab::Duration, "due-tab-duration", strings.duration_tab)].map(
                    |(tab, id, label)| {
                        div()
                            .id(id)
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .when(draft.tab == tab, |el| el.bg(theme.colors().element_selected))
                            .child(Label::new(label).size(LabelSize::Small).color(if draft.tab == tab { Color::Default } else { Color::Muted }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let duration = this.default_duration();
                                this.update_draft(cx, |d| match tab {
                                    DueTab::Duration => d.start_duration(Local::now().naive_local(), duration),
                                    DueTab::Date => d.tab = DueTab::Date,
                                })
                            }))
                    },
                ),
            ))
            .map(|el| match draft.tab {
                DueTab::Date => el
                    .child(quick_row)
                    .when_some(horizon_row, |el, row| el.child(row))
                    .child(calendar_block)
                    .child(Divider::horizontal())
                    .child(row("due-time-row", IconName::Clock, time_label, draft.time.is_some(), DueMenu::Time, cx))
                    .children(time_menu)
                    .when(draft.time.is_some(), |el| {
                        el.child(row(
                            "due-timezone-row",
                            IconName::Map,
                            tz_label,
                            draft.timezone.is_some() || draft.is_floating,
                            DueMenu::TimeZone,
                            cx,
                        ))
                        .children(tz_menu)
                    }),
                DueTab::Duration => el.child(self.render_duration(&draft, calendar_block, strings, cx)),
            })
            .child(row(
                "due-reminder-row",
                IconName::AlarmClock,
                reminder_label,
                !draft.reminders.is_empty(),
                DueMenu::Reminder,
                cx,
            ))
            .children(reminder_menu)
            .child(row("due-repeat-row", IconName::Repeat, repeat_label, draft.repeat.is_some(), DueMenu::Repeat, cx))
            .children(repeat_menu);
        popover(cx)
            .w(px(260.).min(available.width.max(px(0.))))
            .max_h(available.height.max(px(0.)))
            .p_2()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.dismiss_popup(window, cx);
            }))
            .child(
                v_scroll(
                    "due-popup-content-scroll",
                    v_flex().id("due-popup-content").child(content),
                )
                .min_h_0()
                .compact(),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .pt_1()
                    .child(button("due-clear", strings.clear, false).on_click(cx.listener(|this, _, window, cx| this.clear_due(window, cx))))
                    .child(button("due-ok", strings.ok, true).debug_selector(|| "due-popup-ok".into())
                        .on_click(cx.listener(|this, _, window, cx| this.apply_due(window, cx)))),
            )
            .into_any_element()
    }

    fn update_custom_reminder(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut CustomReminder)) {
        self.update_draft(cx, |d| {
            if let Some(DueMenu::CustomReminder(custom)) = d.menu.as_mut() {
                change(custom);
            }
        })
    }

    /// The custom reminder editor, as in the original: unit, amount in advance, the time of day
    /// for all-day tasks, and when it will fire.
    fn render_custom_reminder(
        &self,
        custom: CustomReminder,
        draft: &DueDraft,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let has_time = draft.time.is_some();
        let units: &[ReminderUnit] = if has_time { &ReminderUnit::TIMED } else { &ReminderUnit::ALL_DAY };
        let offset = custom.offset(has_time);
        let fires = draft.date.map(|date| {
            let base = draft.time.unwrap_or_else(|| {
                chrono::NaiveTime::from_hms_opt(fern_core::reminder::ALL_DAY_REMINDER_HOUR, 0, 0).unwrap_or_default()
            });
            date.and_time(base) - chrono::Duration::minutes(offset)
        });
        let summary = fires.map(|at| {
            strings
                .reminder_summary
                .replace("{time}", &crate::calendar::format_time(at, strings, cx))
                .replace("{date}", &task_rows::format_day(strings.short_date[1], at.date(), strings))
        });
        let stepper_row = |label: &'static str, value: String, minus: AnyElement, plus: AnyElement| {
            h_flex()
                .gap_2()
                .child(div().flex_1().child(Label::new(label).size(LabelSize::Small)))
                .child(minus)
                .child(div().min_w(px(44.)).flex().justify_center().child(Label::new(value).size(LabelSize::Small)))
                .child(plus)
        };
        let small = |id: &'static str, icon: IconName| IconButton::new(id, icon).size(px(22.)).icon_size(IconSize::XSmall);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        v_flex()
            .gap_2()
            .p_2()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().border)
            .child(h_flex().gap_1().children(units.iter().map(|&unit| {
                let selected = unit == custom.unit;
                div()
                    .id(SharedString::from(format!("reminder-unit-{unit:?}")))
                    .flex_1()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(theme.colors().element_selected))
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(Label::new(unit.label(strings)).size(LabelSize::XSmall))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_custom_reminder(cx, |c| c.unit = unit)))
            })))
            .child(stepper_row(
                strings.reminder_in_advance,
                custom.amount.to_string(),
                small("reminder-fewer", IconName::Minus)
                    .on_click(cx.listener(|this, _, _, cx| this.update_custom_reminder(cx, |c| c.amount = c.amount.saturating_sub(1))))
                    .into_any_element(),
                small("reminder-more", IconName::Plus)
                    .on_click(cx.listener(|this, _, _, cx| this.update_custom_reminder(cx, |c| c.amount = (c.amount + 1).min(999))))
                    .into_any_element(),
            ))
            .when(!has_time, |el| {
                el.child(stepper_row(
                    strings.reminder_at,
                    crate::calendar::format_time(custom.at, strings, cx),
                    small("reminder-earlier", IconName::Minus)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_custom_reminder(cx, |c| c.at -= chrono::Duration::minutes(30))
                        }))
                        .into_any_element(),
                    small("reminder-later", IconName::Plus)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_custom_reminder(cx, |c| c.at += chrono::Duration::minutes(30))
                        }))
                        .into_any_element(),
                ))
            })
            .child(Label::new(due_picker::describe_reminder(offset, has_time, strings, cx)).size(LabelSize::XSmall).color(Color::Muted))
            .when_some(summary, |el, summary| el.child(Label::new(summary).size(LabelSize::XSmall).color(Color::Accent)))
            .child(
                h_flex()
                    .gap_2()
                    .child(button("reminder-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| {
                        this.update_draft(cx, |d| d.menu = Some(DueMenu::Reminder))
                    })))
                    .child(button("reminder-ok", strings.ok, true).on_click(cx.listener(move |this, _, _, cx| {
                        this.update_draft(cx, |d| d.add_reminder(offset, Local::now().date_naive()))
                    }))),
            )
            .into_any_element()
    }

    /// "Custom" repeat: every N days, weeks, months or years, and the weekdays for weekly rules.
    fn render_custom_repeat(&self, rule: Recurrence, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let unit = due_picker::unit_index(rule.frequency);
        let units = h_flex().gap_1().children(due_picker::UNITS.into_iter().enumerate().map(|(i, frequency)| {
            let selected = i == unit;
            div()
                .id(SharedString::from(format!("repeat-unit-{i}")))
                .flex_1()
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.colors().element_selected))
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(Label::new(strings.repeat_units[i]).size(LabelSize::XSmall))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_draft(cx, |d| {
                        if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                            rule.frequency = frequency;
                            if frequency != Frequency::Weekly {
                                rule.weekdays = 0;
                            }
                            if frequency != Frequency::Monthly {
                                rule.month_days = 0;
                                rule.nth_weekday = None;
                                rule.nth_workday = None;
                            }
                            if !matches!(frequency, Frequency::Monthly | Frequency::Yearly) {
                                rule.lunar = false;
                            }
                            if frequency != Frequency::Daily {
                                rule.skip_weekends = false;
                            }
                        }
                    })
                }))
        }));
        let weekdays = h_flex().justify_between().children((0..7u8).map(|i| {
            let on = rule.weekdays & (1 << i) != 0;
            div()
                .id(SharedString::from(format!("repeat-day-{i}")))
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(on, |el| el.bg(theme.colors().accent))
                .when(!on, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(strings.weekdays_narrow[i as usize]).size(LabelSize::XSmall).when(on, |l| l.color(Color::OnAccent)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_draft(cx, |d| {
                        if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                            rule.weekdays ^= 1 << i;
                        }
                    })
                }))
        }));
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        // The original's repeat modes: Due Dates, Completion Date, Specific Dates, Forgetting Curve.
        let mode = h_flex()
            .id("repeat-mode")
            .h(px(26.))
            .px_2()
            .justify_between()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.colors().border)
            .cursor_pointer()
            .child(
                Label::new(if rule.forgetting_curve {
                    strings.repeat_forgetting_curve
                } else if rule.is_specific_dates() {
                    strings.repeat_by_specific
                } else if rule.from_completion {
                    strings.repeat_by_completion
                } else {
                    strings.repeat_by_due
                })
                .size(LabelSize::Small),
            )
            .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
            .on_click(cx.listener(|this, _, _, cx| {
                let today = Local::now().date_naive();
                this.update_draft(cx, |d| {
                    let start = d.date.unwrap_or(today);
                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                        if rule.forgetting_curve {
                            rule.forgetting_curve = false;
                            rule.cycle = 0;
                        } else if rule.is_specific_dates() {
                            rule.dates.clear();
                            rule.forgetting_curve = true;
                            rule.cycle = 0;
                            rule.lunar = false;
                        } else if rule.from_completion {
                            rule.from_completion = false;
                            rule.dates = vec![start];
                            rule.lunar = false;
                        } else {
                            rule.from_completion = true;
                            rule.weekdays = 0;
                            rule.month_days = 0;
                            rule.nth_weekday = None;
                            rule.nth_workday = None;
                            rule.skip_weekends = false;
                            rule.lunar = false;
                        }
                    }
                })
            }));
        let skip_weekends = h_flex()
            .id("repeat-skip-weekends")
            .gap_2()
            .cursor_pointer()
            .child(Checkbox::new("repeat-skip-weekends-box").size(px(14.)).toggle_state(rule.skip_weekends))
            .child(Label::new(strings.repeat_skip_weekends).size(LabelSize::Small))
            .on_click(cx.listener(|this, _, _, cx| {
                this.update_draft(cx, |d| {
                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                        rule.skip_weekends = !rule.skip_weekends;
                    }
                })
            }));
        let skip_holidays = h_flex()
            .id("repeat-skip-holidays")
            .gap_2()
            .cursor_pointer()
            .child(Checkbox::new("repeat-skip-holidays-box").size(px(14.)).toggle_state(rule.skip_holidays))
            .child(Label::new(strings.repeat_skip_holidays).size(LabelSize::Small))
            .on_click(cx.listener(|this, _, _, cx| {
                this.update_draft(cx, |d| {
                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                        rule.skip_holidays = !rule.skip_holidays;
                    }
                })
            }));
        let lunar_toggle = h_flex()
            .id("repeat-lunar")
            .gap_2()
            .cursor_pointer()
            .child(Checkbox::new("repeat-lunar-box").size(px(14.)).toggle_state(rule.lunar))
            .child(Label::new(strings.repeat_lunar).size(LabelSize::Small))
            .on_click(cx.listener(|this, _, _, cx| {
                this.update_draft(cx, |d| {
                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                        rule.lunar = !rule.lunar;
                        if rule.lunar {
                            rule.month_days = 0;
                            rule.nth_weekday = None;
                            rule.nth_workday = None;
                        }
                    }
                })
            }));
        let by_due = !rule.from_completion;
        let actions = h_flex()
            .gap_2()
            .child(button("repeat-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| {
                this.update_draft(cx, |d| d.menu = Some(DueMenu::Repeat))
            })))
            .child(button("repeat-ok", strings.ok, true).on_click(cx.listener({
                let rule = rule.clone();
                move |this, _, _, cx| this.update_draft(cx, |d| d.pick_repeat(Some(rule.clone()), Local::now().date_naive()))
            })));
        if rule.forgetting_curve {
            return v_flex()
                .gap_2()
                .p_2()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .child(mode)
                .child(Label::new(strings.repeat_forgetting_curve_hint).size(LabelSize::XSmall).color(Color::Muted))
                .child(
                    Label::new(strings.repeat_forgetting_step.replace("{n}", &(rule.cycle + 1).to_string()))
                        .size(LabelSize::XSmall)
                        .color(Color::Accent),
                )
                .child(skip_weekends)
                .child(skip_holidays)
                .child(actions)
                .into_any_element();
        }
        if rule.is_specific_dates() {
            return v_flex()
                .gap_2()
                .p_2()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .child(mode)
                .child(self.render_specific_dates(&rule, cx))
                .child(actions)
                .into_any_element();
        }
        v_flex()
            .gap_2()
            .p_2()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().border)
            .child(mode)
            .child(
                h_flex()
                    .gap_2()
                    .child(Label::new(strings.repeat_every).size(LabelSize::Small))
                    .child(div().flex_1())
                    .child(
                        IconButton::new("repeat-fewer", IconName::Minus)
                            .size(px(22.))
                            .icon_size(IconSize::XSmall)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_draft(cx, |d| {
                                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                                        rule.interval = rule.interval.saturating_sub(1).max(1);
                                    }
                                })
                            })),
                    )
                    .child(div().min_w(px(20.)).flex().justify_center().child(Label::new(rule.interval.to_string())))
                    .child(
                        IconButton::new("repeat-more", IconName::Plus)
                            .size(px(22.))
                            .icon_size(IconSize::XSmall)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_draft(cx, |d| {
                                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                                        rule.interval = (rule.interval + 1).min(99);
                                    }
                                })
                            })),
                    ),
            )
            .child(units)
            .when(by_due && rule.frequency == Frequency::Daily, |el| el.child(skip_weekends))
            .when(by_due, |el| el.child(skip_holidays))
            .when(by_due && matches!(rule.frequency, Frequency::Monthly | Frequency::Yearly), |el| el.child(lunar_toggle))
            .when(by_due && rule.frequency == Frequency::Weekly, |el| el.child(weekdays))
            .when(by_due && rule.frequency == Frequency::Monthly && !rule.lunar, |el| el.child(self.render_monthly_rule(rule.clone(), strings, cx)))
            .child(actions)
            .into_any_element()
    }

    /// "By Specific Dates", as in the original: a month calendar where each click picks or drops
    /// a date; past days cannot be picked.
    fn render_specific_dates(&self, rule: &Recurrence, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let month = self.session.specific_month.unwrap_or_else(|| calendar::month_start(rule.dates.first().copied().unwrap_or(today)));
        let nav = |id: &'static str, icon: IconName, delta: i32| {
            IconButton::new(id, icon).size(px(22.)).icon_size(IconSize::XSmall).on_click(cx.listener(move |this, _, _, cx| {
                this.session.specific_month = Some(calendar::add_months(month, delta));
                cx.notify();
            }))
        };
        let days = calendar::month_grid(month, cx);
        let weeks: Vec<_> = days.chunks(7).map(|week| {
            h_flex().children(week.iter().map(|&date| {
                let picked = rule.dates.contains(&date);
                let past = date < today;
                let in_month = date.month() == month.month();
                div().flex_1().flex().justify_center().child(
                    div()
                        .id(SharedString::from(format!("specific-{date}")))
                        .size(px(26.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .when(picked, |el| el.bg(theme.colors().accent))
                        .when(!picked && !past, |el| el.cursor_pointer().hover(|s| s.bg(theme.colors().element_hover)))
                        .child(Label::new(date.day().to_string()).centered().size(LabelSize::Small).color(
                            if picked { Color::OnAccent } else if in_month && !past { Color::Default } else { Color::Muted },
                        ))
                        .when(!past, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                this.update_draft(cx, |d| {
                                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                                        if let Some(i) = rule.dates.iter().position(|d| *d == date) {
                                            // The last date stays: no dates would end "By Specific Dates".
                                            if rule.dates.len() > 1 {
                                                rule.dates.remove(i);
                                            }
                                        } else {
                                            rule.dates.push(date);
                                            rule.dates.sort();
                                        }
                                    }
                                })
                            }))
                        }),
                )
            }))
        }).collect();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(calendar::month_title(month, strings)).size(LabelSize::Small)))
                    .child(nav("specific-prev", IconName::ChevronLeft, -1))
                    .child(nav("specific-next", IconName::ChevronRight, 1)),
            )
            .child(h_flex().children(calendar::weekday_labels(&strings.weekdays_narrow, cx).map(|day| {
                div().flex_1().flex().justify_center().child(Label::new(day).size(LabelSize::XSmall).color(Color::Muted))
            })))
            .children(weeks)
            .into_any_element()
    }

    /// Monthly rules, as in the original: "Each" picks days of the month (and the last day),
    /// "On the" picks the nth weekday, "Workday" the first or last workday.
    fn render_monthly_rule(&self, rule: Recurrence, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let on_the = rule.nth_weekday.is_some();
        let workday = rule.nth_workday.is_some();
        let update = move |this: &mut Self, cx: &mut Context<Self>, change: &dyn Fn(&mut Recurrence)| {
            this.update_draft(cx, |d| {
                if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                    change(rule);
                }
            })
        };
        // Tabs: 0 "Each", 1 "On the", 2 "Workday".
        let tab = |id: &'static str, label: &'static str, selected: bool, to: u8| {
            div()
                .id(id)
                .flex_1()
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.colors().element_selected))
                .child(Label::new(label).size(LabelSize::XSmall))
                .on_click(cx.listener(move |this, _, _, cx| {
                    update(this, cx, &|rule| {
                        if to != 1 {
                            rule.nth_weekday = None;
                        }
                        if to != 2 {
                            rule.nth_workday = None;
                        }
                        if to != 0 {
                            rule.month_days = 0;
                        }
                        match to {
                            1 => _ = rule.nth_weekday.get_or_insert((1, chrono::Weekday::Mon)),
                            2 => _ = rule.nth_workday.get_or_insert(1),
                            _ => {}
                        }
                    })
                }))
        };
        let chip = |id: SharedString, label: String, on: bool, bit: u32| {
            div()
                .id(id)
                .h(px(22.))
                .min_w(px(22.))
                .px_1()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(on, |el| el.bg(theme.colors().accent))
                .when(!on, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).centered().size(LabelSize::XSmall).when(on, |l| l.color(Color::OnAccent)))
                .on_click(cx.listener(move |this, _, _, cx| update(this, cx, &|rule| rule.month_days ^= bit)))
        };
        let cycler = |id: &'static str, label: String, next: fn(&mut Recurrence)| {
            div()
                .id(id)
                .flex_1()
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .border_1()
                .border_color(theme.colors().border)
                .cursor_pointer()
                .child(Label::new(label).size(LabelSize::XSmall))
                .on_click(cx.listener(move |this, _, _, cx| update(this, cx, &|rule| next(rule))))
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .child(tab("monthly-each", strings.repeat_each, !on_the && !workday, 0))
                    .child(tab("monthly-on-the", strings.repeat_on_the, on_the, 1))
                    .child(tab("monthly-workday", strings.repeat_workday, workday, 2)),
            )
            .map(|el| match rule.nth_workday {
                Some(nth) => el.child(h_flex().child(cycler(
                    "monthly-workday-nth",
                    strings.repeat_workday_nth[usize::from(nth < 0)].to_owned(),
                    |rule| {
                        if let Some(nth) = rule.nth_workday.as_mut() {
                            *nth = if *nth < 0 { 1 } else { -1 };
                        }
                    },
                ))),
                None => el,
            })
            .when(!workday, |el| el.map(|el| match rule.nth_weekday {
                Some((nth, weekday)) => {
                    let nth_label = if nth < 0 { strings.repeat_nth[4] } else { strings.repeat_nth[(nth as usize).clamp(1, 4) - 1] };
                    el.child(
                        h_flex()
                            .gap_2()
                            .child(cycler("monthly-nth", nth_label.to_owned(), |rule| {
                                if let Some((nth, _)) = rule.nth_weekday.as_mut() {
                                    *nth = match *nth { 1 => 2, 2 => 3, 3 => 4, 4 => -1, _ => 1 };
                                }
                            }))
                            .child(cycler(
                                "monthly-weekday",
                                strings.weekdays_full[weekday.num_days_from_monday() as usize].to_owned(),
                                |rule| {
                                    if let Some((_, day)) = rule.nth_weekday.as_mut() {
                                        *day = day.succ();
                                    }
                                },
                            )),
                    )
                }
                None => el.child(
                    div().flex().flex_wrap().gap_1().children((0..31u32).map(|i| {
                        chip(SharedString::from(format!("month-day-{i}")), (i + 1).to_string(), rule.month_days & (1 << i) != 0, 1 << i)
                    }))
                    .child(chip(
                        "month-day-last".into(),
                        strings.repeat_last_day_title.to_owned(),
                        rule.month_days & fern_core::recurrence::LAST_DAY != 0,
                        fern_core::recurrence::LAST_DAY,
                    )),
                ),
            }))
    }
}
