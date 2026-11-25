use super::*;

impl TodoView {
    /// The date picker's Time, Reminder and Repeat rows, as in the task's date popover: a click
    /// opens the row's choices under it.
    pub(super) fn render_add_date_rows(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let draft = &self.quick_add.draft;
        let date = draft.date.unwrap_or_else(Self::today);
        let has_time = draft.time.is_some();
        let open = self.quick_add.date_row;
        let row = |i: u8, icon: IconName, label: String, set: bool| {
            h_flex()
                .id(SharedString::from(format!("add-date-row-{i}")))
                .h(px(36.))
                .px_1()
                .gap_2()
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(Icon::new(icon).size(IconSize::Small).color(if set { Color::Accent } else { Color::Muted }))
                .child(div().flex_1().min_w_0().child(Label::new(label).size(LabelSize::Small).color(if set { Color::Accent } else { Color::Default }).truncate()))
                .child(Icon::new(if open == Some(i) { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::XSmall).color(Color::Muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.quick_add.date_row = if this.quick_add.date_row == Some(i) { None } else { Some(i) };
                    cx.notify();
                }))
        };
        let choice = |id: SharedString, label: String, selected: bool| {
            h_flex()
                .id(id)
                .h(px(26.))
                .px_2()
                .gap_2()
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(div().flex_1().child(Label::new(label).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default })))
                .when(selected, |el| el.child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
        };
        let time_label = draft.time.map_or(strings.time.to_owned(), |time| crate::calendar::format_time(time, strings, cx));
        let reminder_label = draft
            .reminders
            .as_ref()
            .and_then(|r| crate::due_picker::reminder_label(r, has_time, strings, cx))
            .unwrap_or_else(|| strings.reminder.to_owned());
        let repeat_label =
            draft.repeat.as_ref().map_or(strings.repeat.to_owned(), |r| crate::due_picker::repeat_label(r, strings));
        let mut rows = v_flex().gap_0p5();
        // On the Duration tab the times are the Start and End fields, as in the original.
        let duration = self.quick_add.range.is_some();
        if !duration {
            rows = rows.child(row(0, IconName::Clock, time_label, has_time));
        }
        if open == Some(0) && !duration {
            let times = std::iter::once(None).chain(crate::due_picker::time_options().into_iter().map(Some));
            rows = rows.child(
                v_scroll(
                    "add-date-times-scroll",
                    v_flex().id("add-date-times").children(times.map(|time| {
                        let label = time.map_or(strings.no_time.to_owned(), |time| crate::calendar::format_time(time, strings, cx));
                        choice(SharedString::from(format!("add-time-{label}")), label, draft.time == time).on_click(cx.listener(move |this, _, _, cx| {
                            this.quick_add.draft.time = time;
                            this.quick_add.draft.date.get_or_insert_with(Self::today);
                            this.quick_add.draft.reminders = None;
                            this.quick_add.date_row = None;
                            cx.notify();
                        }))
                    })),
                )
                .max_h(px(150.))
                .compact(),
            );
        }
        rows = rows.child(row(1, IconName::AlarmClock, reminder_label, draft.reminders.as_ref().is_some_and(|r| !r.is_empty())));
        if open == Some(1) {
            let chosen = draft.reminders.clone().unwrap_or_default();
            rows = rows.child(v_flex().children(crate::due_picker::reminder_options(has_time, strings, cx).into_iter().map(|(offset, label, hint)| {
                let selected = chosen.contains(&offset);
                let label = if hint.is_empty() { label.to_owned() } else { format!("{label} ({hint})") };
                choice(SharedString::from(format!("add-reminder-{offset}")), label, selected).on_click(cx.listener(move |this, _, _, cx| {
                    let reminders = this.quick_add.draft.reminders.get_or_insert_with(Vec::new);
                    if let Some(i) = reminders.iter().position(|r| *r == offset) {
                        reminders.remove(i);
                    } else {
                        reminders.push(offset);
                    }
                    this.quick_add.draft.date.get_or_insert_with(Self::today);
                    cx.notify();
                }))
            })));
        }
        rows = rows.child(row(2, IconName::Repeat, repeat_label, draft.repeat.is_some()));
        if open == Some(2) {
            let options = crate::due_picker::repeat_options(date, strings, i18n::locale(cx));
            rows = rows.child(
                v_flex()
                    .child(choice("add-repeat-none".into(), strings.none_option.to_owned(), draft.repeat.is_none()).on_click(cx.listener(|this, _, _, cx| {
                        this.quick_add.draft.repeat = None;
                        this.quick_add.date_row = None;
                        cx.notify();
                    })))
                    .children(options.into_iter().enumerate().map(|(i, (rule, label, hint))| {
                        let selected = draft.repeat.as_ref() == Some(&rule);
                        let label = if hint.is_empty() { label } else { format!("{label} ({hint})") };
                        choice(SharedString::from(format!("add-repeat-{i}")), label, selected).on_click(cx.listener(move |this, _, _, cx| {
                            this.quick_add.draft.repeat = Some(rule.clone());
                            this.quick_add.draft.date.get_or_insert_with(Self::today);
                            this.quick_add.date_row = None;
                            cx.notify();
                        }))
                    })),
            );
        }
        rows.into_any_element()
    }

    /// The add field's date picker: the original's quick picks (today, tomorrow, next week, next
    /// month) and the month; a pick applies to the next task added. Clear drops it.
    pub(super) fn render_add_date(&self, month: NaiveDate, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let today = Self::today();
        let picked = self.quick_add.draft.date;
        // A pick stays in the picker, as in the original; OK closes it and Clear drops it all.
        let pick = |date: Option<NaiveDate>| {
            move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.quick_add.draft.date = date;
                this.quick_add.draft.horizon = None;
                if date.is_none() {
                    this.quick_add.draft.time = None;
                    this.quick_add.draft.reminders = None;
                    this.quick_add.draft.repeat = None;
                    this.quick_add.date_month = None;
                }
                cx.notify();
            }
        };
        // The original's Date and Duration tabs on top.
        let on_duration = self.quick_add.range.is_some();
        let tab = |id: &'static str, label: &'static str, duration: bool, cx: &mut Context<Self>| {
            let on = on_duration == duration;
            div()
                .id(id)
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(on, |el| el.bg(theme.colors().element_selected))
                .child(Label::new(label).size(LabelSize::Small).color(if on { Color::Default } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.quick_add.range = duration.then(|| {
                        let mut draft = crate::due_picker::DueDraft {
                            date: this.quick_add.draft.date,
                            time: this.quick_add.draft.time,
                            month: crate::calendar::month_start(this.quick_add.draft.date.unwrap_or_else(Self::today)),
                            ..Default::default()
                        };
                        if let Some((end, end_time)) = this.quick_add.draft.end {
                            (draft.end_date, draft.end_time) = (Some(end), end_time);
                        }
                        draft.start_duration(Local::now().naive_local(), this.preferences.task_defaults.duration);
                        draft
                    });
                    cx.notify();
                }))
        };
        let tabs = h_flex()
            .p(px(2.))
            .rounded(px(7.))
            .bg(theme.colors().element_hover)
            .child(tab("add-tab-date", strings.date_tab, false, cx))
            .child(tab("add-tab-duration", strings.duration_tab, true, cx));
        if let Some(range) = self.quick_add.range.clone() {
            return self.render_add_range(range, tabs, strings, cx);
        }
        let [d_today, d_tomorrow, d_week, d_month] = crate::due_picker::quick_dates(today, cx);
        let quick = |id: &'static str, icon: IconName, tip: &'static str, date: NaiveDate| {
            div()
                .id(id)
                .flex_1()
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .tooltip(Tooltip::text(tip))
                .child(Icon::new(icon).size(IconSize::Large))
                .on_click(cx.listener(pick(Some(date))))
        };
        let entity_shift = cx.entity();
        let entity_pick = cx.entity();
        let mut grid_cfg = desktop_common::month_calendar::MonthGridConfig::single("add", month, today);
        grid_cfg.selected = picked;
        let weeks = desktop_common::month_calendar::render_month_grid(&grid_cfg, cx, move |date, _, cx| {
            entity_pick.update(cx, |this, cx| {
                this.quick_add.draft.date = Some(date);
                this.quick_add.draft.horizon = None;
                cx.notify();
            });
        });
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        popover(cx)
            .w(px(260.))
            .p_2()
            .gap_1()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.quick_add.date_month = None;
                this.quick_add.range = None;
                cx.notify();
            }))
            .child(tabs)
            .child(
                h_flex()
                    .child(quick("add-date-today", IconName::Sun, strings.today, d_today))
                    .child(quick("add-date-tomorrow", IconName::Sunrise, strings.tomorrow, d_tomorrow))
                    .child(quick("add-date-week", IconName::CalendarPlus, strings.next_week, d_week))
                    .child(quick("add-date-month", IconName::Moon, strings.next_month, d_month)),
            )
            .when(crate::calendar::horizons_enabled(cx), |el| {
                use fern_core::Horizon;
                let week = Horizon::current_week(today);
                let month = Horizon::current_month(today);
                let quarter = Horizon::current_quarter(today);
                let year = Horizon::current_year(today);
                let q_short = format!("{}{}", strings.horizons_quarter_prefix, (today.month() - 1) / 3 + 1);
                let chips = [
                    ("week", crate::calendar::format_horizon_badge(week, strings), strings.horizon_this_week, week),
                    ("month", crate::calendar::format_horizon_badge(month, strings), strings.horizon_this_month, month),
                    ("quarter", q_short, strings.horizon_this_quarter, quarter),
                    ("year", today.year().to_string(), strings.horizon_this_year, year),
                    ("life", strings.horizons_life.to_owned(), strings.horizons_life, Horizon::Life),
                ];
                el.child(h_flex().gap(px(4.)).children(chips.into_iter().map(|(key, label, tip, h)| {
                    let active = self.quick_add.draft.horizon == Some(h);
                    div()
                        .id(SharedString::from(format!("add-horizon-{key}")))
                        .debug_selector(move || format!("add-horizon-{key}"))
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
                            let next = if this.quick_add.draft.horizon == Some(h) { None } else { Some(h) };
                            this.quick_add.draft.horizon = next;
                            if next.is_some() {
                                this.quick_add.draft.date = None;
                                this.quick_add.draft.time = None;
                                this.quick_add.draft.end = None;
                                this.quick_add.draft.reminders = None;
                            }
                            cx.notify();
                        }))
                        .child(
                            Label::new(label)
                                .size(LabelSize::XSmall)
                                .color(if active { Color::Accent } else { Color::Muted })
                                .truncate(),
                        )
                })))
            })
            .child(div().px_1().child({
                let mut h_cfg = desktop_common::month_calendar::MonthHeaderConfig::surface(
                    month,
                    strings,
                    "add-date-prev",
                    "add-date-next",
                );
                h_cfg.small_title = true;
                h_cfg.title_weight = gpui::FontWeight::NORMAL;
                desktop_common::month_calendar::render_month_header(
                    h_cfg,
                    cx,
                    move |delta, _, cx| {
                        entity_shift.update(cx, |this, cx| {
                            if let Some(month) = this.quick_add.date_month.as_mut() {
                                *month = crate::calendar::add_months(*month, delta);
                            }
                            cx.notify();
                        });
                    },
                    None,
                )
            }))
            .child(desktop_common::month_calendar::render_weekday_row(
                strings,
                desktop_common::month_calendar::WeekdayStyle::Narrow,
                false,
                cx,
            ))
            .children(weeks)
            // As the original's, no line between the month and the Time, Reminder and Repeat rows.
            .child(div().h(px(8.)))
            .child(self.render_add_date_rows(strings, cx))
            .child(
                h_flex()
                    .pt_1()
                    .gap_2()
                    .child(button("add-date-clear", strings.clear, false).on_click(cx.listener(pick(None))))
                    .child(button("add-date-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| {
                        this.quick_add.date_month = None;
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }

    /// The add field's date picker on the original's Duration tab: Start and End, each a date and
    /// a time picked from a month or a list, All Day, the reminder and repeat rows, Clear and OK.
    pub(super) fn render_add_range(
        &self,
        range: crate::due_picker::DueDraft,
        tabs: impl IntoElement,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::due_picker::RangeField;
        let theme = cx.theme().clone();
        let today = Self::today();
        let all_day = range.is_all_day();
        let chip = |id: &'static str, label: SharedString, field: RangeField, cx: &mut Context<Self>| {
            let active = range.range_field == Some(field);
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
                    if let Some(range) = this.quick_add.range.as_mut() {
                        range.range_field = if range.range_field == Some(field) { None } else { Some(field) };
                    }
                    cx.notify();
                }))
        };
        let day = |date: Option<NaiveDate>| -> SharedString { date.map_or(SharedString::default(), |d| task_rows::short_date(d, strings).into()) };
        let time = |t: Option<chrono::NaiveTime>| -> SharedString { t.map_or(SharedString::default(), |t| crate::calendar::format_time(t, strings, cx).into()) };
        let start_date_s = day(range.date);
        let end_date_s = day(range.end_date);
        let start_time_s = (!all_day).then(|| time(range.time));
        let end_time_s = (!all_day).then(|| time(range.end_time));
        let line = |label: &'static str, date_chip: AnyElement, time_chip: Option<AnyElement>| {
            h_flex().gap_2().child(div().w(px(40.)).child(Label::new(label).size(LabelSize::Small))).child(date_chip).children(time_chip)
        };
        let start = line(
            strings.range_start,
            chip("add-range-start-date", start_date_s, RangeField::StartDate, cx).into_any_element(),
            start_time_s.map(|s| chip("add-range-start-time", s, RangeField::StartTime, cx).into_any_element()),
        );
        let end = line(
            strings.range_end,
            chip("add-range-end-date", end_date_s, RangeField::EndDate, cx).into_any_element(),
            end_time_s.map(|s| chip("add-range-end-time", s, RangeField::EndTime, cx).into_any_element()),
        );
        // The field being picked: a month for a date, a list for a time.
        let picker: Option<AnyElement> = match range.range_field {
            Some(field @ (RangeField::StartDate | RangeField::EndDate)) => {
                let shown = if field == RangeField::EndDate { range.end_date } else { range.date };
                let month = crate::calendar::month_start(shown.unwrap_or(today));
                let (span_start, span_end) = match (range.date, range.end_date) {
                    (Some(s), Some(e)) => (Some(s.min(e)), Some(s.max(e))),
                    (s, e) => (s, e),
                };
                let entity_pick = cx.entity();
                let mut cfg = desktop_common::month_calendar::MonthGridConfig::single("add-range", month, today);
                cfg.selected = span_start;
                cfg.other_endpoint = span_end;
                cfg.range = span_start.zip(span_end);
                cfg.highlight_today_bg = false;
                cfg.accent_today_text = false;
                cfg.wrap_in_span_background = true;
                Some(
                    v_flex()
                        .children(desktop_common::month_calendar::render_month_grid(&cfg, cx, move |date, _, cx| {
                            entity_pick.update(cx, |this, cx| {
                                if let Some(range) = this.quick_add.range.as_mut() {
                                    range.pick_range_date(date);
                                }
                                cx.notify();
                            });
                        }))
                        .into_any_element(),
                )
            }
            Some(field @ (RangeField::StartTime | RangeField::EndTime)) => {
                let current = if field == RangeField::EndTime { range.end_time } else { range.time };
                Some(
                    v_scroll(
                        "add-range-times-scroll",
                        v_flex()
                            .id("add-range-times")
                            .children(crate::due_picker::time_options().into_iter().map(|t| {
                                ListItem::new(SharedString::from(format!("add-range-time-{t}")))
                                    .height(px(28.))
                                    .child(Label::new(crate::calendar::format_time(t, strings, cx)).size(LabelSize::Small))
                                    .when(current == Some(t), |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(range) = this.quick_add.range.as_mut() {
                                            range.pick_range_time(t);
                                        }
                                        cx.notify();
                                    }))
                            })),
                    )
                    .max_h(px(180.))
                    .compact()
                    .into_any_element(),
                )
            }
            None => None,
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        popover(cx)
            .w(px(260.))
            .p_2()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.quick_add.date_month = None;
                this.quick_add.range = None;
                cx.notify();
            }))
            .child(tabs)
            .child(start)
            .child(end)
            .children(picker)
            .child(
                h_flex()
                    .id("add-range-all-day")
                    .h(px(30.))
                    .px_1()
                    .cursor_pointer()
                    .child(div().flex_1().child(Label::new(strings.range_all_day).size(LabelSize::Small)))
                    .child(ui::Switch::new().toggle_state(all_day))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(range) = this.quick_add.range.as_mut() {
                            range.set_all_day(!all_day);
                        }
                        cx.notify();
                    })),
            )
            .child(self.render_add_date_rows(strings, cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(button("add-range-clear", strings.clear, false).on_click(cx.listener(|this, _, _, cx| {
                        this.quick_add.draft.date = None;
                        this.quick_add.draft.time = None;
                        this.quick_add.draft.end = None;
                        this.quick_add.draft.reminders = None;
                        this.quick_add.draft.repeat = None;
                        this.quick_add.range = None;
                        this.quick_add.date_month = None;
                        cx.notify();
                    })))
                    .child(button("add-range-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(range) = this.quick_add.range.take() {
                            this.quick_add.draft.date = range.date;
                            this.quick_add.draft.time = range.time;
                            this.quick_add.draft.end = range.end();
                        }
                        this.quick_add.date_month = None;
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }

    /// The original's "Input Box Setting": a preview of the Simple and the Detailed add field;
    /// a click picks one.
    pub(super) fn render_input_box_setting(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let detailed = self.preferences.detailed_input;
        let line = |w: f32| div().w(px(w)).h(px(6.)).rounded(px(3.)).bg(theme.colors().text_muted.opacity(0.4));
        let simple_preview = h_flex()
            .h(px(28.))
            .px_2()
            .gap_1()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.colors().accent)
            .child(Icon::new(IconName::Plus).size(IconSize::XSmall).color(Color::Muted))
            .child(line(70.))
            .child(div().flex_1())
            .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted));
        let detailed_preview = v_flex()
            .p_1p5()
            .gap_1p5()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.colors().accent)
            .child(line(90.))
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(IconName::Flag).size(IconSize::XSmall).color(Color::Muted))
                    .child(Icon::new(IconName::Inbox).size(IconSize::XSmall).color(Color::Muted))
                    .child(Icon::new(IconName::Ellipsis).size(IconSize::XSmall).color(Color::Muted))
                    .child(div().flex_1())
                    .child(div().w(px(22.)).h(px(12.)).rounded(px(3.)).bg(theme.colors().accent)),
            );
        let option = |id: &'static str, preview: AnyElement, label: &'static str, value: bool| {
            let selected = detailed == value;
            v_flex()
                .id(id)
                .flex_1()
                .gap_2()
                .items_center()
                .cursor_pointer()
                .child(
                    div()
                        .w_full()
                        .h(px(76.))
                        .px_2()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .rounded(px(6.))
                        .bg(theme.colors().background)
                        .child(preview),
                )
                .child(
                    h_flex()
                        .gap_1p5()
                        .child(
                            div()
                                .size(px(14.))
                                .rounded_full()
                                .border_1()
                                .border_color(if selected { theme.colors().accent } else { theme.colors().text_muted })
                                .when(selected, |el| el.bg(theme.colors().accent)),
                        )
                        .child(Label::new(label).size(LabelSize::Small)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.save_preferences(|p| p.detailed_input = value, cx);
                    this.sync_input_placeholder(cx);
                }))
        };
        popover(cx)
            .w(px(380.))
            .p_3()
            .gap_3()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.quick_add.input_box_open = false;
                cx.notify();
            }))
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.input_box_setting).weight(FontWeight::SEMIBOLD)))
                    .child(
                        IconButton::new("input-box-close", IconName::X)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(
                        cx.listener(|this, _, _, cx| {
                            this.quick_add.input_box_open = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(option("input-box-simple", simple_preview.into_any_element(), strings.input_box_simple, false))
                    .child(option("input-box-detailed", detailed_preview.into_any_element(), strings.input_box_detailed, true)),
            )
            .into_any_element()
    }

    /// The original's "Detailed" add field: the text on top, a toolbar under it — priority, list
    /// and "…" open the options menu — and an "Add" button.
    pub(super) fn render_detailed_add(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let input_focus = self.quick_add.input.read(cx).focus_handle(cx);
        let date_button = self.add_date_button(Placement::BelowLeft, strings, cx);
        let open_menu = |side: bool| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.quick_add.menu_open = !this.quick_add.menu_open;
                this.quick_add.list_side = side;
                cx.notify();
            })
        };
        let priority_color = self
            .quick_add.draft
            .priority
            .and_then(|p| task_rows::priority_color(p, &theme))
            .map_or(Color::Muted, Color::Custom);
        v_flex()
            .mx(px(20.))
            .mb_2()
            .flex_none()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().border)
            .bg(theme.colors().surface_background)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| window.focus(&input_focus, cx))
            .child(h_flex().gap_2().child(div().flex_1().min_w_0().debug_selector(|| "quick-add-input".to_owned()).child(self.quick_add.input.clone())).children(
                self.shown_add_due().map(|(date, time)| {
                    let (label, _) = task_rows::due_label(date, time, Self::today(), strings, cx);
                    let label = match self.quick_add.draft.end.filter(|_| self.quick_add.due.is_none()) {
                        Some((end, end_time)) => crate::due_picker::range_label(date, time, end, end_time, strings, cx),
                        None => label,
                    };
                    Label::new(label).size(LabelSize::Small).color(Color::Accent)
                }),
            ).children(
                self.shown_add_horizon(cx).map(|h| {
                    Label::new(crate::calendar::format_horizon_badge(h, strings))
                        .size(LabelSize::Small)
                        .color(Color::Accent)
                }),
            ))
            .child(
                h_flex()
                    .gap_1()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(date_button)
                    .child(
                        IconButton::new("add-priority", IconName::Flag)
                            .size(px(24.))
                            .icon_size(IconSize::Small)
                            .icon_color(priority_color)
                            .tooltip(Tooltip::text(strings.summary_priority))
                            .on_click(open_menu(false)),
                    )
                    .child(
                        IconButton::new("add-list", IconName::Inbox)
                            .size(px(24.))
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text(strings.move_to))
                            .on_click(open_menu(true)),
                    )
                    .child(self.add_field_popovers(
                        div().relative().child(
                            IconButton::new("add-more", IconName::Ellipsis)
                                .size(px(24.))
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text(strings.more))
                                .on_click(open_menu(false)),
                        ),
                        strings,
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("add-submit")
                            .h(px(24.))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .bg(theme.colors().accent)
                            .child(Label::new(strings.add).size(LabelSize::Small).color(Color::OnAccent))
                            .on_click(cx.listener(|this, _, window, cx| {
                                let text = this.quick_add.input.read(cx).text().to_owned();
                                this.submit_new_task(&text, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The add field's "▾" menu, as in the original.
    pub(super) fn render_add_menu(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let draft = self.quick_add.draft.clone();
        let flags = [Priority::High, Priority::Medium, Priority::Low, Priority::None].map(|priority| {
            let selected = draft.priority == Some(priority);
            let color = task_rows::priority_color(priority, &theme).unwrap_or(theme.colors().text_muted);
            div()
                .id(SharedString::from(format!("add-priority-{}", priority.to_i64())))
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.colors().element_selected))
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(Icon::new(if priority == Priority::None { IconName::Flag } else { IconName::FlagFilled }).color(Color::Custom(color)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.quick_add.draft.priority = if selected { None } else { Some(priority) };
                    cx.notify();
                }))
        });
        let list_name = match draft.list {
            Some(Some(id)) => self.lists.iter().find(|l| l.id == id).map_or(strings.inbox.to_owned(), |l| l.name.clone()),
            _ => strings.inbox.to_owned(),
        };
        let list_items = std::iter::once((None, strings.inbox.to_owned()))
            .chain(self.lists.iter().filter(|l| !l.archived).map(|l| (Some(l.id), l.name.clone())))
            .map(|(id, name)| {
                let selected = draft.list.unwrap_or(None) == id && (draft.list.is_some() || id.is_none());
                ListItem::new(SharedString::from(format!("add-list-{}", id.map_or("inbox".to_owned(), |i| i.to_string()))))
                    .child(Label::new(name).color(if selected { Color::Accent } else { Color::Default }).truncate())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.quick_add.draft.list = Some(id);
                        this.quick_add.menu_open = false;
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();
        // As the original's: 181 px wide (wider when a label needs it), a row every 34 px, no line under the flags.
        let list_bounds = std::rc::Rc::new(std::cell::Cell::new(None::<gpui::Bounds<Pixels>>));
        let list_bounds_out = list_bounds.clone();
        popover(cx)
            .min_w(px(181.))
            .on_mouse_down_out(cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                if list_bounds_out.get().is_some_and(|b| b.contains(&event.position)) {
                    return;
                }
                this.quick_add.menu_open = false;
                cx.notify();
            }))
            .child(div().px_2().pt_1().child(Label::new(strings.filter_priority).size(LabelSize::XSmall).color(Color::Muted)))
            .child(h_flex().px_1().py_1().justify_between().children(flags))
            .child(
                SubmenuItem::new("add-list", list_name)
                    .icon(IconName::Inbox)
                    .height(px(34.))
                    .open(self.quick_add.list_side)
                    .track_bounds(list_bounds)
                    .placement(Placement::RightSide)
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered && !this.quick_add.list_side {
                            this.quick_add.list_side = true;
                            cx.notify();
                        }
                    }))
                    .items(list_items),
            )
            .child(
                ListItem::new("add-tags").height(px(34.))
                    .start_slot(Icon::new(IconName::Tag).color(Color::Muted))
                    .child(Label::new(strings.tags))
                    .when(!draft.tags.is_empty(), |item| {
                        item.end_slot(Label::new(draft.tags.len().to_string()).size(LabelSize::XSmall).color(Color::Accent))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.quick_add.menu_open = false;
                        this.quick_add.tags = Some(this.quick_add.draft.tags.clone());
                        this.quick_add.tag_input.update(cx, |input, cx| input.set_text("", cx));
                        let focus = this.quick_add.tag_input.read(cx).focus_handle(cx);
                        window.focus(&focus, cx);
                        cx.notify();
                    })),
            )
            .child(
                ListItem::new("add-attachment").height(px(34.))
                    .start_slot(Icon::new(IconName::Paperclip).color(Color::Muted))
                    .child(Label::new(strings.attachment))
                    .when(!draft.files.is_empty(), |item| {
                        item.end_slot(Label::new(draft.files.len().to_string()).size(LabelSize::XSmall).color(Color::Accent))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.quick_add.menu_open = false;
                        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
                            files: true,
                            directories: false,
                            multiple: true,
                            prompt: None,
                        });
                        cx.spawn(async move |this, cx| {
                            let Some(paths) = chosen.await.ok().and_then(Result::ok).flatten() else { return };
                            this.update(cx, |this, cx| {
                                this.quick_add.draft.files.extend(paths);
                                cx.notify();
                            })
                            .ok();
                        })
                        .detach();
                        cx.notify();
                    })),
            )
            .child(
                ListItem::new("add-from-template").height(px(34.))
                    .start_slot(Icon::new(IconName::LayoutTemplate).color(Color::Muted))
                    .child(Label::new(strings.add_from_template))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.quick_add.menu_open = false;
                        this.cached_templates = this.store.templates().unwrap_or_default();
                        this.quick_add.templates_open = true;
                        cx.notify();
                    })),
            )
            .child(
                ListItem::new("add-as-note").height(px(34.))
                    .start_slot(Icon::new(IconName::FileText).color(Color::Muted))
                    .child(Label::new(if draft.note { strings.convert_to_task } else { strings.convert_to_note }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.quick_add.draft.note = !this.quick_add.draft.note;
                        this.quick_add.menu_open = false;
                        cx.notify();
                    })),
            )
            .child(div().py_1().child(Divider::horizontal()))
            .child(
                ListItem::new("add-input-box").height(px(34.))
                    .start_slot(Icon::new(IconName::Settings).color(Color::Muted))
                    .child(Label::new(strings.input_box_setting))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.quick_add.menu_open = false;
                        this.quick_add.input_box_open = true;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// The "▾" menu's Tags, as in the original: a search field over the tags, ticked by a click
    /// (Enter ticks a typed one, new tags too), then Cancel or OK; they go on the next task added.
    pub(super) fn render_add_tags(&self, picked: &[String], strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let query = self.quick_add.tag_input.read(cx).text().trim().to_lowercase();
        let mut names: Vec<String> = self.tags.iter().map(|t| t.name.clone()).collect();
        for tag in picked {
            if !names.iter().any(|n| n.to_lowercase() == tag.to_lowercase()) {
                names.push(tag.clone());
            }
        }
        let rows = names.into_iter().filter(|name| name.to_lowercase().contains(&query)).map(|name| {
            let ticked = picked.iter().any(|t| t.to_lowercase() == name.to_lowercase());
            ListItem::new(SharedString::from(format!("add-tag-{name}")))
                .start_slot(Icon::new(IconName::Tag).color(Color::Muted))
                .child(Label::new(name.clone()).truncate())
                .when(ticked, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(picked) = this.quick_add.tags.as_mut() {
                        if let Some(i) = picked.iter().position(|t| t.to_lowercase() == name.to_lowercase()) {
                            picked.remove(i);
                        } else {
                            picked.push(name.clone());
                        }
                    }
                    cx.notify();
                }))
        });
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        popover(cx)
            .w(px(240.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.quick_add.tags = None;
                cx.notify();
            }))
            .child(
                h_flex()
                    .px_2()
                    .pb_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(Icon::new(IconName::Search).size(IconSize::Small).color(Color::Muted))
                    .child(div().flex_1().child(self.quick_add.tag_input.clone())),
            )
            .child(
                v_scroll(
                    "add-tag-list-scroll",
                    v_flex().id("add-tag-list").py_1().children(rows),
                )
                .max_h(px(240.))
                .compact(),
            )
            .child(
                h_flex()
                    .p_2()
                    .gap_2()
                    .child(button("add-tags-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| {
                        this.quick_add.tags = None;
                        cx.notify();
                    })))
                    .child(button("add-tags-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(picked) = this.quick_add.tags.take() {
                            this.quick_add.draft.tags = picked;
                        }
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }
}
