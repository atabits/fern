use super::*;

impl TodoView {
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
        let days: Vec<NaiveDate> = (0..days_shown as i64).map(|i| from + chrono::Duration::days(i)).collect();
        let groups = task_rows::kanban_columns(self.rows_with(strings, &HashSet::new(), &HashSet::new()), strings);
        let scale_button = self.render_scale_button(strings, cx);
        let weekend = |date: &NaiveDate| matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        let shift = |delta: i64| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.timeline_from += chrono::Duration::days(delta);
                cx.notify();
            })
        };
        // The day columns behind each row, so that weekends stay shaded all the way down; a bar
        // dropped on one moves its task there, keeping its duration.
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
                    .child(scale_button),
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
                .filter_map(|task| fern_core::timeline::bar(&task, from, days_shown).map(|(first, span)| (task, first, span)))
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
                            let tint = task_rows::priority_color(task.priority, &theme).unwrap_or(theme.colors().accent);
                            let dragged = DraggedTask { id, title: task.title.clone().into() };
                            div()
                                .id(SharedString::from(format!("timeline-{i}-{id}")))
                                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                                .absolute()
                                .top(px(8. + 40. * n as f32))
                                .left(gpui::relative(first as f32 / days_shown as f32))
                                .w(gpui::relative(span as f32 / days_shown as f32))
                                .h(px(32.))
                                .px_0p5()
                                .child(
                                    div()
                                        .size_full()
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded(px(8.))
                                        .bg(tint.opacity(if task.is_completed() { 0.3 } else { 0.75 }))
                                        .cursor_pointer()
                                        .child(Label::new(task.title).truncate()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
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
            h_flex()
                .id(SharedString::from(format!("arrange-{id}")))
                .ml(px(depth as f32 * 14.))
                .h(px(22.))
                .px_1p5()
                .rounded(px(3.))
                .bg(color.opacity(0.7))
                .cursor_grab()
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
            items.push(
                h_flex()
                    .pt_2()
                    .pb_1()
                    .gap_1()
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    .child(Label::new(label).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(count.to_string()).size(LabelSize::XSmall).color(Color::Muted))
                    .into_any_element(),
            );
            for (task, subtasks) in undated {
                if task.due_date.is_none() {
                    items.push(bar(task, 0, cx));
                }
                for subtask in subtasks {
                    items.push(bar(subtask, 1, cx));
                }
            }
        }
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
                        .child(IconButton::new("arrange-close", IconName::X).size(px(22.)).icon_size(IconSize::Small).on_click(cx.listener(|this, _, _, cx| {
                            this.arrange_open = false;
                            cx.notify();
                        }))),
                )
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
