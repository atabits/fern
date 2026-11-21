use super::*;

fn visible_timeline_bar(task: &Task, days: &[NaiveDate]) -> Option<(usize, usize)> {
    let start = task.due_date?;
    let end = task.end_date.unwrap_or(start).max(start);
    let first_day = *days.first()?;
    let last_day = *days.last()?;
    if end < first_day || start > last_day {
        return None;
    }
    let first = days.iter().position(|d| *d >= start)?;
    let last = days.iter().rposition(|d| *d <= end)?;
    (last >= first).then_some((first, last - first + 1))
}

impl TodoView {
    pub(super) fn toggle_timeline_weekends(&mut self, cx: &mut Context<Self>) {
        let next = !self.timeline_show_weekends;
        let result = self.store.set_setting("timeline_show_weekends", if next { "1" } else { "0" });
        self.check(result);
        self.timeline_show_weekends = next;
        cx.notify();
    }

    pub(super) fn set_timeline_color_by(&mut self, choice: ColorBy, cx: &mut Context<Self>) {
        let result = self.store.set_setting("timeline_color_by", choice.code());
        self.check(result);
        self.timeline_color_by = choice;
        cx.notify();
    }

    pub(super) fn set_timeline_secondary_tz(&mut self, tz: Option<String>, cx: &mut Context<Self>) {
        let cleaned = tz
            .as_deref()
            .map(str::trim)
            .filter(|z| !z.is_empty() && z.parse::<chrono_tz::Tz>().is_ok())
            .map(str::to_owned);
        let result = self.store.set_setting("timeline_secondary_tz", cleaned.as_deref().unwrap_or(""));
        self.check(result);
        self.timeline_secondary_tz = cleaned;
        self.timeline_tz_menu = false;
        cx.notify();
    }

    pub(super) fn resize_timeline_task(&mut self, resized: &ResizedTask, day: NaiveDate, cx: &mut Context<Self>) {
        let (start, end) = match resized.edge {
            ResizeEdge::Start => (day.min(resized.end), resized.end),
            ResizeEdge::End => (resized.start, day.max(resized.start)),
        };
        let result = self.store.resize_dates(resized.id, start, end);
        self.check(result);
        crate::data_version::data_changed(cx.entity_id(), cx);
        self.reload(cx);
    }

    fn timeline_bar_color(&self, task: &Task, theme: &theme::Theme) -> Hsla {
        let accent = theme.colors().accent;
        match self.timeline_color_by {
            ColorBy::List => task
                .list_id
                .and_then(|id| self.lists.iter().find(|l| l.id == id))
                .and_then(|l| lists_panel::parse_color(l.color.as_deref()))
                .unwrap_or(accent),
            ColorBy::Tag => task
                .tags
                .first()
                .and_then(|name| self.tags.iter().find(|t| t.name.eq_ignore_ascii_case(name)))
                .and_then(|t| lists_panel::parse_color(t.color.as_deref()))
                .unwrap_or(accent),
            ColorBy::Priority => task_rows::priority_color(task.priority, theme).unwrap_or(accent),
        }
    }

    fn render_timeline_tz_button(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let label = self
            .timeline_secondary_tz
            .as_deref()
            .map(|z| z.rsplit('/').next().unwrap_or(z).replace('_', " "))
            .unwrap_or_else(|| strings.secondary_timezone_none.to_owned());
        let query = self.timeline_tz_input.read(cx).text().trim().to_lowercase();
        let tz_choices: Vec<&'static str> = if query.is_empty() {
            vec![
                "UTC",
                "America/New_York",
                "America/Los_Angeles",
                "America/Chicago",
                "Europe/London",
                "Europe/Berlin",
                "Europe/Moscow",
                "Asia/Dubai",
                "Asia/Kolkata",
                "Asia/Shanghai",
                "Asia/Tokyo",
                "Australia/Sydney",
            ]
        } else {
            chrono_tz::TZ_VARIANTS
                .iter()
                .map(|z| z.name())
                .filter(|name| name.to_lowercase().contains(&query))
                .take(40)
                .collect()
        };
        div()
            .relative()
            .ml_1p5()
            .child(
                h_flex()
                    .id("timeline-secondary-tz")
                    .px_2()
                    .h(px(22.))
                    .gap_1()
                    .rounded(px(11.))
                    .border_1()
                    .border_color(theme.colors().border)
                    .cursor_pointer()
                    .child(Icon::new(IconName::Clock).size(IconSize::XSmall).color(Color::Muted))
                    .child(Label::new(label).size(LabelSize::Small))
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.timeline_tz_menu = !this.timeline_tz_menu;
                        if this.timeline_tz_menu {
                            window.focus(&this.timeline_tz_input.focus_handle(cx), cx);
                        }
                        cx.notify();
                    })),
            )
            .when(self.timeline_tz_menu, |el| {
                el.child(PopoverAttachment::new(
                    Placement::BelowRight,
                    popover(cx)
                        .w(px(260.))
                        .p_2()
                        .gap_1p5()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.timeline_tz_menu = false;
                            cx.notify();
                        }))
                        .child(
                            h_flex()
                                .h(px(28.))
                                .px_2()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(theme.colors().border)
                                .child(self.timeline_tz_input.clone())
                                .child(
                                    Label::new(if self.timeline_tz_input.read(cx).text().is_empty() {
                                        strings.timezone_search
                                    } else {
                                        ""
                                    })
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                                ),
                        )
                        .child(
                            v_scroll(
                                "timeline-tz-scroll",
                                v_flex()
                                    .id("timeline-tz-list")
                                    .child(
                                        ListItem::new("timeline-tz-none")
                                            .height(px(28.))
                                            .toggle_state(self.timeline_secondary_tz.is_none())
                                            .child(Label::new(strings.secondary_timezone_none).size(LabelSize::Small))
                                            .on_click(cx.listener(|this, _, _, cx| this.set_timeline_secondary_tz(None, cx))),
                                    )
                                    .children(tz_choices.into_iter().map(|zone| {
                                        let active = self.timeline_secondary_tz.as_deref() == Some(zone);
                                        ListItem::new(SharedString::from(format!("timeline-tz-{zone}")))
                                            .height(px(28.))
                                            .toggle_state(active)
                                            .child(Label::new(zone).size(LabelSize::Small).truncate())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.set_timeline_secondary_tz(Some(zone.to_owned()), cx);
                                            }))
                                    })),
                            )
                            .max_h(px(200.))
                            .compact(),
                        )
                        .into_any_element(),
                ))
            })
            .into_any_element()
    }

    /// The Timeline's scale button, "Day ▾", and its menu: Day, Week, Month.
    pub(super) fn render_scale_button(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let labels = [strings.day, strings.week, strings.month];
        div()
            .relative()
            .ml_2()
            .child(
                h_flex()
                    .id("timeline-scale")
                    .px_2()
                    .h(px(22.))
                    .gap_1()
                    .rounded(px(11.))
                    .border_1()
                    .border_color(theme.colors().border)
                    .cursor_pointer()
                    .child(Label::new(labels[self.timeline_scale]).size(LabelSize::Small))
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.scale_menu_open = !this.scale_menu_open;
                        cx.notify();
                    })),
            )
            .when(self.scale_menu_open, |el| {
                el.child(PopoverAttachment::new(
                    Placement::BelowRight,
                    popover(cx)
                        .min_w(px(150.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.scale_menu_open = false;
                            cx.notify();
                        }))
                        .children(labels.into_iter().enumerate().map(|(i, label)| {
                            let selected = i == self.timeline_scale;
                            ListItem::new(SharedString::from(format!("timeline-scale-{i}")))
                                .child(Label::new(label).color(if selected { Color::Accent } else { Color::Default }))
                                .when(selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.timeline_scale = i;
                                    this.scale_menu_open = false;
                                    let result = this.store.set_setting(&format!("timeline_scale:{}", this.view_key()), &i.to_string());
                                    this.check(result);
                                    cx.notify();
                                }))
                        }))
                        .into_any_element(),
                ))
            })
            .into_any_element()
    }

    /// The original's Timeline: the groups as rows, the coming days as columns (weekends shaded),
    /// and each dated task as a bar from its date to the end of its duration; a click opens it.
    pub(super) fn render_timeline(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        // The original's scales: Day shows ten days, Week three weeks, Month six; the arrows move
        // by a week, two weeks or a month.
        let scale = self.timeline_scale;
        let days_shown = [10usize, 22, 45][scale];
        let step = [7i64, 14, 30][scale];
        const LABEL_WIDTH: f32 = 108.;
        let theme = cx.theme().clone();
        let today = Self::today();
        let from = self.timeline_from;
        let today_line = theme.status().error;
        let weekend = |date: &NaiveDate| matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        let days: Vec<NaiveDate> = (0..days_shown as i64)
            .map(|i| from + chrono::Duration::days(i))
            .filter(|d| self.timeline_show_weekends || !weekend(d))
            .collect();
        let visible_cols = days.len().max(1);
        let secondary_tz = self
            .timeline_secondary_tz
            .as_deref()
            .and_then(|z| z.parse::<chrono_tz::Tz>().ok());
        let groups = task_rows::kanban_columns(self.rows_with(strings, &HashSet::new(), &HashSet::new()), strings);
        let scale_button = self.render_scale_button(strings, cx);
        let tz_button = self.render_timeline_tz_button(strings, cx);
        let dual_clock = secondary_tz.map(|tz| {
            let now_local = Local::now();
            let now_sec = now_local.with_timezone(&tz);
            let city = tz.name().rsplit('/').next().unwrap_or(tz.name()).replace('_', " ");
            format!(
                "{} {} · {} {}",
                strings.timezone_local,
                crate::calendar::format_time(now_local.time(), strings),
                city,
                crate::calendar::format_time(now_sec.time(), strings)
            )
        });
        let shift = |delta: i64| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.timeline_from += chrono::Duration::days(delta);
                cx.notify();
            })
        };
        // The day columns behind each row, so that weekends stay shaded all the way down; a bar
        // dropped on one moves or resizes its task there.
        let accent = theme.colors().accent;
        let backdrop = |row: usize, height: Pixels, cx: &mut Context<Self>| {
            h_flex().absolute().top_0().left_0().w_full().h(height).children(days.iter().map(|&date| {
                div()
                    .id(SharedString::from(format!("timeline-day-{row}-{date}")))
                    .flex_1()
                    .h_full()
                    .when(weekend(&date), |el| el.bg(theme.colors().element_hover.opacity(0.5)))
                    // Week and Month mark today with a red line, as the original does.
                    .when(scale > 0 && date == today, |el| el.border_l_2().border_color(today_line))
                    .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.15)))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                        let result = this.store.move_dates(dragged.id, date);
                        this.check(result);
                        this.reload(cx);
                    }))
                    .drag_over::<ResizedTask>(move |style, _, _, _| style.bg(accent.opacity(0.18)))
                    .on_drop(cx.listener(move |this, resized: &ResizedTask, _, cx| {
                        this.resize_timeline_task(resized, date, cx);
                    }))
            }))
        };
        let header = v_flex()
            .flex_none()
            .child(
                h_flex()
                    .h(px(30.))
                    .px_3()
                    .child(div().w(px(LABEL_WIDTH - 12.)).flex_none().child(Label::new(strings.months_short[from.month0() as usize]).size(LabelSize::Small).color(Color::Muted)))
                    // A month's name above its first day, for the scales that cross months.
                    .child(h_flex().flex_1().mr(px(8.)).children(days.iter().map(|date| {
                        div().flex_1().when(scale > 0 && date.day() == 1 && *date != from, |el| {
                            el.child(Label::new(strings.months_short[date.month0() as usize]).size(LabelSize::Small).color(Color::Muted))
                        })
                    })))
                    .when_some(dual_clock, |el, text| {
                        el.child(div().mr_2().child(Label::new(text).size(LabelSize::XSmall).color(Color::Muted)))
                    })
                    .child(
                        div()
                            .id("timeline-today")
                            .px_2()
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .rounded(px(11.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .cursor_pointer()
                            .child(Label::new(strings.today).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.timeline_from = Self::today();
                                cx.notify();
                            })),
                    )
                    .child(scale_button)
                    .child(tz_button),
            )
            .child(
                h_flex()
                    .h(px(34.))
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(
                        h_flex()
                            .w(px(LABEL_WIDTH))
                            .flex_none()
                            .px_1()
                            .child(IconButton::new("timeline-prev", IconName::ChevronLeft).size(px(20.)).icon_size(IconSize::XSmall).on_click(shift(-step))),
                    )
                    .child(h_flex().flex_1().children(days.iter().map(|date| {
                        let is_today = *date == today;
                        // Month shows every seventh day's number, so that they fit.
                        let labelled = scale < 2 || (date.day() - 1) % 7 == 0;
                        // As in the original: the numbers centered over their days, semibold, and
                        // the weekend's muted.
                        let color = match (is_today, scale) {
                            (true, 0) => Color::Accent,
                            (true, _) => Color::Custom(today_line),
                            _ if weekend(date) => Color::Muted,
                            _ => Color::Default,
                        };
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(weekend(date), |el| el.bg(theme.colors().element_hover.opacity(0.5)))
                            .when(labelled || is_today, |el| el.child(Label::new(date.day().to_string()).centered().weight(FontWeight::SEMIBOLD).color(color)))
                    })))
                    .child(IconButton::new("timeline-next", IconName::ChevronRight).size(px(20.)).icon_size(IconSize::XSmall).on_click(shift(step))),
            );
        let rows = groups.into_iter().enumerate().map(|(i, (label, _, cards))| {
            let bars: Vec<(Task, usize, usize)> = cards
                .into_iter()
                .flat_map(|(task, subtasks)| std::iter::once(task).chain(subtasks))
                .filter_map(|task| visible_timeline_bar(&task, &days).map(|(first, span)| (task, first, span)))
                .collect();
            // A group folds up with the arrow before its name, as in the original.
            let folded = self.timeline_folded.contains(&label);
            let bars = if folded { Vec::new() } else { bars };
            let height = px(if folded { 40. } else { 20. + 40. * bars.len().max(1) as f32 });
            let fold_label = label.clone();
            h_flex()
                .h(height)
                .border_b_1()
                .border_color(theme.colors().border_variant)
                .child(
                    h_flex()
                        .id(SharedString::from(format!("timeline-group-{i}")))
                        .w(px(LABEL_WIDTH))
                        .flex_none()
                        .h_full()
                        .items_start()
                        .gap_1()
                        .pl_2()
                        .pt(px(14.))
                        .cursor_pointer()
                        .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall).color(Color::Muted))
                        .child(Label::new(label).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.timeline_folded.remove(&fold_label) {
                                this.timeline_folded.insert(fold_label.clone());
                            }
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .mr(px(20.))
                        .relative()
                        .child(backdrop(i, height, cx))
                        .children(bars.into_iter().enumerate().map(|(n, (task, first, span))| {
                            let id = task.id;
                            let completed = task.is_completed();
                            let tint = self.timeline_bar_color(&task, &theme);
                            let handle_title: SharedString = task.title.clone().into();
                            let dragged = DraggedTask { id, title: handle_title.clone() };
                            let span_start = task.due_date.unwrap_or(today);
                            let span_end = task.end_date.unwrap_or(span_start).max(span_start);
                            let sec_time_label = secondary_tz.and_then(|tz| {
                                let d = task.due_date?;
                                let t = task.due_time?;
                                let naive = d.and_time(t);
                                let local_dt = Local.from_local_datetime(&naive).earliest()?;
                                let sec_dt = local_dt.with_timezone(&tz);
                                Some(format!(
                                    "{} ({})",
                                    crate::calendar::format_time(t, strings),
                                    crate::calendar::format_time(sec_dt.time(), strings)
                                ))
                            });
                            let resize_handle = |edge: ResizeEdge| {
                                let tag = if edge == ResizeEdge::Start { "start" } else { "end" };
                                let payload = ResizedTask {
                                    id,
                                    title: handle_title.clone(),
                                    start: span_start,
                                    end: span_end,
                                    edge,
                                };
                                div()
                                    .id(SharedString::from(format!("timeline-resize-{tag}-{i}-{id}")))
                                    .flex_none()
                                    .w(px(6.))
                                    .h_full()
                                    .cursor_ew_resize()
                                    .on_drag(payload, |p, _, _, cx| cx.new(|_| p.clone()))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(|_, _, cx| cx.stop_propagation())
                            };
                            let bar_tooltip: SharedString = match (task.due_date, task.end_date) {
                                (Some(start), Some(end)) if end > start => format!(
                                    "{} ({} – {})",
                                    task.title,
                                    task_rows::short_date(start, strings),
                                    task_rows::short_date(end, strings),
                                )
                                .into(),
                                (Some(start), _) => format!("{} ({})", task.title, task_rows::short_date(start, strings)).into(),
                                _ => task.title.clone().into(),
                            };
                            let middle = h_flex()
                                .id(SharedString::from(format!("timeline-body-{i}-{id}")))
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .px_1()
                                .gap_1()
                                .items_center()
                                .cursor_grab()
                                .tooltip(Tooltip::text(bar_tooltip))
                                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                                .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                                .when(task.kind != Kind::Note, |el| {
                                    el.child(
                                        div()
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                            .child(
                                                Checkbox::new(SharedString::from(format!("timeline-check-{i}-{id}")))
                                                    .size(px(12.))
                                                    .toggle_state(completed)
                                                    .in_progress(task.is_in_progress())
                                                    .dismissed(task.is_wont_do())
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.toggle(id, !completed, cx);
                                                    })),
                                            ),
                                    )
                                })
                                .when_some(sec_time_label, |el, tz_txt| {
                                    el.child(Label::new(tz_txt).size(LabelSize::XSmall).color(Color::Muted))
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .when(completed, |el| el.line_through())
                                        .child(Label::new(task.title).truncate()),
                                );
                            h_flex()
                                .id(SharedString::from(format!("timeline-{i}-{id}")))
                                .absolute()
                                .top(px(8. + 40. * n as f32))
                                .left(gpui::relative(first as f32 / visible_cols as f32))
                                .w(gpui::relative(span as f32 / visible_cols as f32))
                                .h(px(32.))
                                .px_0p5()
                                .child(
                                    h_flex()
                                        .size_full()
                                        .rounded(px(8.))
                                        .bg(tint.opacity(if completed { 0.3 } else { 0.75 }))
                                        .child(resize_handle(ResizeEdge::Start))
                                        .child(middle)
                                        .child(resize_handle(ResizeEdge::End)),
                                )
                        })),
                )
        }).collect::<Vec<_>>();
        // Under the groups the weekends stay shaded down to the bottom, as in the original.
        let filler = h_flex()
            .flex_1()
            .min_h(px(200.))
            .child(div().w(px(LABEL_WIDTH)).flex_none())
            .child(div().flex_1().h_full().mr(px(20.)).relative().overflow_hidden().child(backdrop(usize::MAX, px(4000.), cx)));
        let timeline = v_scroll("timeline-scroll", v_flex().id("timeline").child(header).children(rows).child(filler))
            .flex_1()
            .min_w_0()
            .h_full();
        h_flex()
            .flex_1()
            .min_h_0()
            .items_start()
            .child(timeline)
            .when(self.arrange_open, |el| el.child(self.render_arrange(strings, cx)))
            .into_any_element()
    }

    /// The original's "Arrange Tasks": the list's undated open tasks by group, as bars in their
    /// first tag's color, to drag onto a day of the Timeline.
    pub(super) fn render_arrange(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let groups = task_rows::kanban_columns(self.rows_with(strings, &HashSet::new(), &HashSet::new()), strings);
        let tag_color = |task: &Task| {
            task.tags
                .first()
                .and_then(|name| self.tags.iter().find(|t| t.name.to_lowercase() == name.to_lowercase()))
                .and_then(|t| lists_panel::parse_color(t.color.as_deref()))
                .unwrap_or(theme.colors().accent)
        };
        let bar = |task: Task, depth: usize, cx: &mut Context<Self>| {
            let id = task.id;
            let color = tag_color(&task);
            let dragged = DraggedTask { id, title: task.title.clone().into() };
            let tip_title: SharedString = task.title.clone().into();
            h_flex()
                .id(SharedString::from(format!("arrange-{id}")))
                .ml(px(depth as f32 * 14.))
                .h(px(22.))
                .px_1p5()
                .rounded(px(3.))
                .bg(color.opacity(0.7))
                .cursor_grab()
                .tooltip(Tooltip::text(tip_title))
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                .gap_1()
                .when(task.kind == Kind::Note, |el| el.child(Icon::new(IconName::FileText).size(IconSize::XSmall)))
                .child(Label::new(task.title).size(LabelSize::XSmall).truncate())
                .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                .into_any_element()
        };
        let mut items: Vec<AnyElement> = Vec::new();
        for (label, _, cards) in groups {
            let undated: Vec<(Task, Vec<Task>)> = cards
                .into_iter()
                .filter(|(task, _)| !task.is_closed())
                .map(|(task, subtasks)| (task, subtasks.into_iter().filter(|t| t.due_date.is_none() && !t.is_closed()).collect::<Vec<_>>()))
                .filter(|(task, subtasks)| task.due_date.is_none() || !subtasks.is_empty())
                .collect();
            let count: usize = undated.iter().map(|(t, s)| usize::from(t.due_date.is_none()) + s.len()).sum();
            if count == 0 {
                continue;
            }
            let collapse_key = format!("arrange:{label}");
            let collapsed = self.collapsed_days.contains(&collapse_key);
            let toggle_key = collapse_key.clone();
            items.push(
                h_flex()
                    .id(SharedString::from(collapse_key))
                    .pt_2()
                    .pb_1()
                    .px_1()
                    .gap_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().ghost_element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_day(toggle_key.clone(), cx)))
                    .child(Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall).color(Color::Muted))
                    .child(Label::new(label).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(count.to_string()).size(LabelSize::XSmall).color(Color::Muted))
                    .into_any_element(),
            );
            if !collapsed {
                for (task, subtasks) in undated {
                    if task.due_date.is_none() {
                        items.push(bar(task, 0, cx));
                    }
                    for subtask in subtasks {
                        items.push(bar(subtask, 1, cx));
                    }
                }
            }
        }
        let no_undated = items.is_empty();
        v_scroll(
            "todo-arrange-tasks-scroll",
            v_flex()
                .id("arrange-tasks")
                .px_2()
                .pt_2()
                .gap_1()
                .child(
                    h_flex()
                        .child(div().flex_1().child(Label::new(strings.arrange_tasks).weight(FontWeight::SEMIBOLD)))
                        .child(
                            IconButton::new("arrange-close", IconName::X)
                                .size(px(22.))
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text(strings.close))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.arrange_open = false;
                                    cx.notify();
                                })),
                        ),
                )
                .when(no_undated, |el| {
                    el.child(
                        div()
                            .pt_6()
                            .px_2()
                            .flex()
                            .justify_center()
                            .child(Label::new(strings.no_tasks).size(LabelSize::Small).color(Color::Muted)),
                    )
                })
                .children(items),
        )
        .w(px(220.))
        .flex_none()
        .h_full()
        .border_l_1()
        .border_color(theme.colors().border_variant)
        .into_any_element()
    }

}
