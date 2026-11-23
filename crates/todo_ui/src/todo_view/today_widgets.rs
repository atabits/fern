use super::*;

impl TodoView {
    pub(super) fn reload_today_countdowns(&mut self) {
        if self.view != View::Today {
            self.today_countdowns.clear();
            return;
        }
        let today = Self::today();
        self.today_countdowns = self
            .store
            .countdowns()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| !c.archived && c.show_in_list.shows(c.next_date(today), today))
            .collect();
    }

    /// Today and Next 7 Days list the habits due today, unless turned off in the habit settings.
    pub(super) fn reload_today_habits(&mut self) {
        self.today_habits = Rc::new(Vec::new());
        if !matches!(self.view, View::Today | View::Next7Days)
            || self.store.setting(crate::habit_view::IN_TODAY_SETTING).ok().flatten().as_deref() == Some("0")
        {
            return;
        }
        let today = Self::today();
        let week_start = crate::calendar::week_start();
        let sections = self.store.habit_sections().unwrap_or_default();
        let section_order = fern_core::HabitSection::ordered(&sections);
        let all_records = self.store.all_habit_checkins().unwrap_or_default();
        let mut items: Vec<(usize, i64, fern_core::Habit, bool, f64)> = Vec::new();
        for habit in self.store.habits().unwrap_or_default().into_iter().filter(|h| !h.archived) {
            let recs = all_records.get(&habit.id).map(Vec::as_slice).unwrap_or(&[]);
            if habit.is_active_on(today, recs, week_start) {
                let today_rec = recs.iter().find(|c| c.date == today);
                let amount = today_rec.map_or(0.0, |c| c.value);
                let checked = today_rec.is_some_and(|c| c.is_done());
                let sec_idx = section_order.iter().position(|s| *s == habit.section).unwrap_or(section_order.len());
                items.push((sec_idx, habit.sort_order, habit, checked, amount));
            }
        }
        items.sort_by_key(|(sec_idx, sort_order, _, _, _)| (*sec_idx, *sort_order));
        self.today_habits = Rc::new(items.into_iter().map(|(_, _, h, c, a)| (h, c, a)).collect());
    }

    /// As in the original, Today and Next 7 Days list the habits due today, unless turned off in
    /// the habit settings. Checking one checks the habit in (one unit for amount goals).
    pub(super) fn render_habits(&self, strings: &Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let today = Self::today();
        let habits = Rc::clone(&self.today_habits);
        if habits.is_empty() {
            return None;
        }
        let count = habits.len();
        Some(
            v_flex()
                .child(
                    h_flex()
                        .h(px(36.))
                        .gap_1()
                        .pl(px(GUTTER))
                        // Next 7 Days names today's habits by the day, as the original: "Sunday, Today's Habit".
                        .child(Label::new(if self.view == View::Next7Days {
                            task_rows::relative_day(today, strings.todays_habit, strings)
                        } else {
                            strings.section_habits.to_owned()
                        }).weight(FontWeight::BOLD))
                        .child(div().pl_1().child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted))),
                )
                // As the original's rows: the icon and name, then "0 / 2 unit" and "Today" on
                // the right, no box; a click checks the habit in or adds a step.
                .children(habits.iter().map(|(habit, checked, amount)| {
                    let checked = *checked;
                    let amount = *amount;
                    let id = habit.id;
                    let has_goal = habit.goal.is_some();
                    let (target, unit) = habit.goal.as_ref().map_or((1.0, String::new()), |g| (g.amount, g.unit.clone()));
                    let progress_text = format!(
                        "{} / {} {}",
                        fern_core::habit::format_amount(amount),
                        fern_core::habit::format_amount(target),
                        unit
                    )
                    .trim()
                    .to_owned();
                    h_flex()
                        .id(SharedString::from(format!("habit-row-{id}")))
                        .h(px(ROW_HEIGHT))
                        .pl(px(GUTTER))
                        .pr_3()
                        .gap(px(10.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.colors().ghost_element_hover))
                        .child(
                            div()
                                .id(SharedString::from(format!("habit-check-{id}")))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    let result = if has_goal {
                                        this.store.step_checkin(id, today).map(|_| ())
                                    } else {
                                        this.store.toggle_checkin(id, today).map(|_| ())
                                    };
                                    this.check(result);
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload_today_habits();
                                    cx.notify();
                                }))
                                .child(crate::habit_view::avatar(habit, px(20.), cx)),
                        )
                        .child(
                            div().flex_1().min_w_0().child(
                                Label::new(habit.name.clone())
                                    .color(if checked { Color::Muted } else { Color::Default })
                                    .truncate(),
                            ),
                        )
                        .child(Label::new(progress_text).size(LabelSize::Small).color(Color::Muted))
                        .child(Label::new(strings.today).size(LabelSize::Small).color(Color::Accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let result = if has_goal {
                                this.store.step_checkin(id, today).map(|_| ())
                            } else {
                                this.store.toggle_checkin(id, today).map(|_| ())
                            };
                            this.check(result);
                            desktop_common::data_version::data_changed(cx.entity_id(), cx);
                            this.reload_today_habits();
                            cx.notify();
                        }))
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.habit_menu = Some((id, ev.position));
                                cx.notify();
                            }),
                        )
                        .border_b_1()
                        .border_color(theme.colors().border_variant)
                }))
                .into_any_element(),
        )
    }

    pub(super) fn render_habit_context_menu(&self, strings: &Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, pos) = self.habit_menu?;
        let habit = self.today_habits.iter().find(|(h, _, _)| h.id == id)?.0.clone();
        let today = Self::today();
        let name = habit.name;
        let name_sw = name.clone();
        Some(
            div()
                .id("todo-habit-ctx")
                .absolute()
                .left(pos.x)
                .top(pos.y)
                .occlude()
                .child(
                    popover(cx)
                        .min_w(px(176.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.habit_menu = None;
                            cx.notify();
                        }))
                        .child(
                            ListItem::new("todo-habit-open")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::FernHabit).color(Color::Muted))
                                .child(Label::new(strings.section_habits))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    cx.emit(OpenHabit(id));
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("todo-habit-pomo")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Timer).color(Color::Muted))
                                .child(Label::new(strings.habit_start_pomo))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    cx.emit(StartHabitFocus { habit_id: id, title: name.clone(), stopwatch: false });
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("todo-habit-sw")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Clock).color(Color::Muted))
                                .child(Label::new(strings.habit_start_stopwatch))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    cx.emit(StartHabitFocus { habit_id: id, title: name_sw.clone(), stopwatch: true });
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("todo-habit-skip")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Minus).color(Color::Muted))
                                .child(Label::new(strings.habit_skip))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    let res = this.store.skip_habit_date(id, today);
                                    this.check(res);
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload_today_habits();
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("todo-habit-undone")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Ban).color(Color::Muted))
                                .child(Label::new(strings.habit_unachieved))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    let res = this.store.mark_checkin_undone(id, today);
                                    this.check(res);
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload_today_habits();
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("todo-habit-reset")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::RotateCcw).color(Color::Muted))
                                .child(Label::new(strings.habit_reset))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    let res = this.store.reset_checkin(id, today, true);
                                    this.check(res);
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload_today_habits();
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The countdowns whose Show puts them in Today (on the day by default, as in the original),
    /// each with its icon, name and the days left.
    pub(super) fn render_countdowns(&self, strings: &Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.view != View::Today || self.today_countdowns.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        let today = Self::today();
        let count = self.today_countdowns.len();
        Some(
            v_flex()
                .child(
                    h_flex()
                        .h(px(36.))
                        .gap_1()
                        .pl(px(GUTTER))
                        .child(Label::new(strings.section_countdown).weight(FontWeight::BOLD))
                        .child(div().pl_1().child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted))),
                )
                .children(self.today_countdowns.iter().map(|countdown| {
                    let date = countdown.next_date(today);
                    let days = countdown.span(today).total_days;
                    let when = if date == today { strings.today.to_owned() } else { format!("{days} {}", strings.habit_days) };
                    h_flex()
                        .h(px(ROW_HEIGHT))
                        .pl(px(GUTTER))
                        .pr_3()
                        .gap(px(10.))
                        .child(match countdown.icon.clone() {
                            Some(icon) => div().w(px(16.)).child(icon).into_any_element(),
                            None => Icon::new(crate::countdown_view::kind_icon(countdown.kind)).color(Color::Accent).into_any_element(),
                        })
                        .child(div().flex_1().min_w_0().child(Label::new(countdown.name.clone()).truncate()))
                        .child(Label::new(when).size(LabelSize::Small).color(Color::Accent))
                        .border_b_1()
                        .border_color(theme.colors().border_variant)
                }))
                .into_any_element(),
        )
    }
}
