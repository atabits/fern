//! Focus target picker popover and expanded full-page timer view.

use super::*;

/// Cached picker data populated when the picker opens, reloads, or switches task view, so
/// `render_picker()` performs zero SQLite queries per frame.
struct PickerCache {
    lists: Vec<fern_core::List>,
    tasks: Vec<Task>,
    timers: Vec<fern_core::focus::FocusTimer>,
    habits: Vec<(fern_core::Habit, HashSet<NaiveDate>)>,
}

/// The focus page's picker, opened from "Focus >" or the eye over the hours: the original's
/// Task, Timer and Habit tabs over a search field.
pub(super) struct Picker {
    search: Entity<TextInput>,
    /// Opened from the eye over the hours rather than from "Focus >".
    at_eye: bool,
    /// The Task tab's "Today >" list of views, while open.
    pub(super) views_open: bool,
    cache: PickerCache,
}

/// What the focus page's Start focuses on.
#[derive(Clone)]
pub(super) enum NextFocus {
    Task(Id, String),
    Timer(fern_core::focus::FocusTimer),
    Habit(Id, String),
}

impl NextFocus {
    fn title(&self) -> &str {
        match self {
            Self::Task(_, title) | Self::Habit(_, title) => title,
            Self::Timer(timer) => &timer.name,
        }
    }
}

impl FocusView {
    fn load_picker_cache(&self, today: NaiveDate) -> PickerCache {
        let tasks = if self.picker_view == View::Today {
            self.tasks.clone()
        } else {
            self.store.tasks_in(&self.picker_view, today).unwrap_or_default()
        };
        let timers = if !self.archived_tab {
            self.timers.clone()
        } else {
            self.store.focus_timers(false).unwrap_or_default()
        };
        PickerCache {
            lists: self.stats_data.lists.iter().filter(|l| !l.archived).cloned().collect(),
            tasks,
            timers,
            habits: self
                .stats_data
                .habits
                .iter()
                .filter(|(h, _)| !h.archived)
                .cloned()
                .collect(),
        }
    }

    pub(super) fn refresh_picker_cache(&mut self, today: NaiveDate) {
        if self.picker.is_some() {
            let cache = self.load_picker_cache(today);
            if let Some(picker) = self.picker.as_mut() {
                picker.cache = cache;
            }
        }
    }

    pub(super) fn set_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.expanded = expanded;
        self.record_menu = None;
        self.picker = None;
        cx.notify();
    }

    pub(super) fn open_picker(&mut self, at_eye: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            self.reload(cx);
        }
        if self.picker.as_ref().is_some_and(|p| p.at_eye == at_eye) {
            self.picker = None;
            cx.notify();
            return;
        }
        let strings = i18n::t(cx);
        let search = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.focus_picker_search, cx));
        cx.subscribe(&search, |_, _, _: &crate::text_input::Changed, cx| cx.notify()).detach();
        cx.subscribe(&search, |this, _, _: &crate::text_input::Cancelled, cx| {
            this.picker = None;
            cx.notify();
        })
        .detach();
        window.focus(&search.focus_handle(cx), cx);
        self.ensure_stats_data();
        let cache = self.load_picker_cache(Local::now().date_naive());
        self.picker = Some(Picker { search, at_eye, views_open: false, cache });
        cx.notify();
    }

    fn pick(&mut self, next: NextFocus, cx: &mut Context<Self>) {
        self.next = Some(next);
        self.picker = None;
        cx.notify();
    }

    /// The page's Start: the picked task, timer or habit, or a plain session.
    pub fn start_next(&mut self, cx: &mut Context<Self>) {
        match self.next.take() {
            Some(NextFocus::Task(id, title)) => self.start(Some((id, title)), cx),
            Some(NextFocus::Timer(timer)) => self.start_from_timer(&timer, cx),
            Some(NextFocus::Habit(id, name)) => {
                let stopwatch = self.stopwatch_mode();
                self.start_for_habit(Some(id), name, stopwatch, cx)
            }
            None => self.start(None, cx),
        }
    }

    /// The picker's list: today's tasks and due habits, the active timers, or the active habits,
    /// narrowed by the search. Reads exclusively from `picker.cache` (zero SQLite queries).
    fn render_picker(&self, picker: &Picker, cx: &mut Context<Self>) -> AnyElement {
        use fern_core::focus::matches_search;
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let query = picker.search.read(cx).text().to_owned();
        let today = Local::now().date_naive();
        let habits: Vec<(&fern_core::Habit, &HashSet<NaiveDate>)> = picker
            .cache
            .habits
            .iter()
            .filter(|(h, _)| matches_search(&h.name, &query))
            .map(|(h, done)| (h, done))
            .collect();
        // As in the original, the picked one has a filled circle and stays lit.
        let picked = self.next.as_ref().map(|n| n.title().to_owned());
        let item = |id: SharedString, title: String, next: NextFocus, cx: &mut Context<Self>| {
            let on = picked.as_deref() == Some(next.title());
            ListItem::new(id)
                .toggle_state(on)
                .start_slot(
                    Icon::new(if on { IconName::CircleDot } else { IconName::Circle })
                        .size(IconSize::Small)
                        .color(if on { Color::Accent } else { Color::Muted }),
                )
                .child(Label::new(title).size(LabelSize::Small).truncate())
                .on_click(cx.listener(move |this, _, _, cx| this.pick(next.clone(), cx)))
                .into_any_element()
        };
        let mut items: Vec<AnyElement> = Vec::new();
        // The views the Task tab can list: the smart lists, the Inbox and the lists.
        let views: Vec<(View, SharedString)> = [
            (View::Today, SharedString::new_static(strings.today)),
            (View::Tomorrow, SharedString::new_static(strings.tomorrow)),
            (View::Next7Days, SharedString::new_static(strings.next_7_days)),
            (View::Inbox, SharedString::new_static(strings.inbox)),
        ]
        .into_iter()
        .chain(picker.cache.lists.iter().map(|l| (View::List(l.id), l.name.clone().into())))
        .collect();
        let view_name = views.iter().find(|(v, _)| *v == self.picker_view).map_or(SharedString::new_static(strings.today), |(_, n)| n.clone());
        match self.picker_tab {
            0 if picker.views_open => {
                for (i, (view, name)) in views.into_iter().enumerate() {
                    let on = view == self.picker_view;
                    items.push(
                        ListItem::new(("focus-picker-view", i))
                            .toggle_state(on)
                            .child(Label::new(name).size(LabelSize::Small).color(if on { Color::Accent } else { Color::Default }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.picker_view = view.clone();
                                let tasks = if this.picker_view == View::Today {
                                    this.tasks.clone()
                                } else {
                                    this.store.tasks_in(&this.picker_view, Local::now().date_naive()).unwrap_or_default()
                                };
                                if let Some(picker) = this.picker.as_mut() {
                                    picker.views_open = false;
                                    picker.cache.tasks = tasks;
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            0 => {
                items.push(
                    h_flex()
                        .id("focus-picker-views")
                        .gap_1()
                        .px_2()
                        .py_1()
                        .cursor_pointer()
                        .child(Icon::new(IconName::CalendarDays).size(IconSize::Small).color(Color::Muted))
                        .child(Label::new(view_name).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                        .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(picker) = this.picker.as_mut() {
                                picker.views_open = true;
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                for task in picker.cache.tasks.iter().filter(|t| matches_search(&t.title, &query)) {
                    items.push(item(format!("pick-task-{}", task.id).into(), task.title.clone(), NextFocus::Task(task.id, task.title.clone()), cx));
                }
                // As in the original, the habits due today follow Today's tasks.
                let week_start = crate::calendar::week_start();
                let due: Vec<&fern_core::Habit> = habits
                    .iter()
                    .filter(|(h, done)| self.picker_view == View::Today && h.is_due_on(today, done, week_start))
                    .map(|(h, _)| *h)
                    .collect();
                if !due.is_empty() {
                    items.push(div().px_2().pt_1().child(Label::new(strings.focus_picker_tabs[2]).size(LabelSize::XSmall).color(Color::Muted)).into_any_element());
                }
                for habit in due {
                    items.push(item(format!("pick-habit-{}", habit.id).into(), habit.name.clone(), NextFocus::Habit(habit.id, habit.name.clone()), cx));
                }
            }
            1 => {
                for timer in picker.cache.timers.iter().filter(|t| matches_search(&t.name, &query)) {
                    items.push(item(format!("pick-timer-{}", timer.id).into(), timer.name.clone(), NextFocus::Timer(timer.clone()), cx));
                }
            }
            _ => {
                for (habit, _) in &habits {
                    items.push(item(format!("pick-habit-{}", habit.id).into(), habit.name.clone(), NextFocus::Habit(habit.id, habit.name.clone()), cx));
                }
            }
        }
        let tabs = h_flex().justify_center().gap_1().children(strings.focus_picker_tabs.iter().enumerate().map(|(i, label)| {
            let on = self.picker_tab == i;
            div()
                .id(("focus-picker-tab", i))
                .px_3()
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(12.))
                .cursor_pointer()
                .when(on, |el| el.bg(theme.colors().accent.opacity(0.2)))
                .child(Label::new(*label).size(LabelSize::Small).color(if on { Color::Accent } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.picker_tab = i;
                    if let Some(picker) = this.picker.as_mut() {
                        picker.views_open = false;
                    }
                    cx.notify();
                }))
        }));
        popover(cx)
            .w(px(300.))
            .h(px(420.))
            .p_2()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.picker = None;
                cx.notify();
            }))
            .child(tabs)
            .child(
                h_flex()
                    .h(px(30.))
                    .px_2()
                    .gap_2()
                    .rounded(px(6.))
                    .bg(theme.colors().element_hover)
                    .child(Icon::new(IconName::Search).size(IconSize::Small).color(Color::Muted))
                    .child(div().flex_1().child(picker.search.clone())),
            )
            .child(
                v_scroll(
                    ("focus-picker-items-scroll", self.picker_tab * 2 + usize::from(picker.views_open)),
                    v_flex()
                        .id(("focus-picker-items", self.picker_tab * 2 + usize::from(picker.views_open)))
                        .children(items),
                )
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }

    /// The original's full focus page: the arrow back, Pomo / Stopwatch and "…" on top, "Focus >"
    /// over a big ring with the time and Start (or Pause / Continue and End), and on the right
    /// the day's hours with its sessions and a red line at the time now.
    pub(super) fn render_page(&self, clock: String, label: SharedString, running: bool, paused: bool, cx: &mut Context<Self>) -> AnyElement {
        // The original's measures: an hour is 213 px, the ring 350 px, the panel 490 px wide.
        const HOUR: f32 = 213.;
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let now = Local::now();
        let (hour, minute) = (chrono::Timelike::hour(&now), chrono::Timelike::minute(&now));
        let pill = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .w(px(167.))
                .h(px(47.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(text).when(primary, |label| label.color(Color::OnAccent)))
        };
        // The picked task, timer or habit names the session until it starts.
        let label: SharedString = match (&self.next, running) {
            (Some(next), false) => next.title().to_owned().into(),
            _ => label,
        };
        let picker = self.picker.as_ref().map(|p| (p.at_eye, self.render_picker(p, cx)));
        let (picker_here, picker_eye) = match picker {
            Some((false, el)) => (Some(PopoverAttachment::new(Placement::BelowCenter, el)), None),
            Some((true, el)) => (None, Some(PopoverAttachment::new(Placement::BelowRight, el))),
            None => (None, None),
        };
        let buttons = if running {
            h_flex()
                .gap_3()
                .child(pill("focus-page-pause", if paused { strings.focus_continue } else { strings.focus_pause }, true).on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx))))
                .child(pill("focus-page-end", strings.focus_end, false).on_click(cx.listener(|this, _, window, cx| this.request_finish(window, cx))))
        } else {
            h_flex().child(pill("focus-page-start", strings.focus_start, true).on_click(cx.listener(|this, _, _, cx| this.start_next(cx))))
        };
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .border_r_1()
            .border_color(theme.colors().border)
            .child(
                h_flex()
                    .mt(crate::title_bar::header_inset())
                    .h(px(52.))
                    .flex_none()
                    .px_4()
                    .child(
                        div().flex_1().child(
                            IconButton::new("focus-page-collapse", IconName::ChevronDown)
                                .icon_color(Color::Muted)
                                .tooltip(ui::Tooltip::text(strings.focus_collapse))
                                .on_click(cx.listener(|this, _, _, cx| this.set_expanded(false, cx))),
                        ),
                    )
                    .when(!running, |el| el.child(self.render_mode_switch(LabelSize::Small, cx)))
                    .child(
                        h_flex().flex_1().justify_end().gap_1()
                        .child(self.render_white_noise_button("focus-page-white-noise", cx))
                        // The original's mini window: the clock in a small window over the others.
                        .child(
                            IconButton::new("focus-page-mini", IconName::PictureInPicture2)
                                .icon_color(Color::Muted)
                                .tooltip(ui::Tooltip::text(strings.focus_mini_window))
                                // Deferred: the window draws at once and reads this view.
                                .on_click(cx.listener(|_, _, _, cx| {
                                    let focus = cx.entity();
                                    cx.defer(move |cx| crate::focus_mini::open(focus, cx));
                                })),
                        )
                        .child(
                            div()
                                .relative()
                                .child(
                                    IconButton::new("focus-page-more", IconName::Ellipsis)
                                        .tooltip(Tooltip::text(strings.more))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set_record_menu(Some(RecordMenu::HeaderMore), cx)
                                        })),
                                )
                                .when_some(
                                    self.record_menu.filter(|m| matches!(m, RecordMenu::HeaderMore | RecordMenu::Settings)),
                                    |el, menu| el.child(PopoverAttachment::new(Placement::BelowRight, self.render_record_menu(menu, cx))),
                                ),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .relative()
                            .mb(px(110.))
                            .child(
                                h_flex()
                                    .id("focus-page-pick")
                                    .gap_0p5()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(6.))
                                    .when(!running, |el| {
                                        el.cursor_pointer()
                                            .hover(|s| s.bg(theme.colors().ghost_element_hover))
                                            .on_click(cx.listener(|this, _, window, cx| this.open_picker(false, window, cx)))
                                    })
                                    .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                                    .when(!running, |el| el.child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Muted))),
                            )
                            .children(picker_here),
                    )
                    .child(
                        div()
                            .size(px(350.))
                            .mb(px(120.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .border(px(5.))
                            .border_color(if running { theme.colors().accent } else { theme.colors().border })
                            .child(div().text_size(px(56.)).child(clock)),
                    )
                    .child(buttons),
            );

        // The hours from the one before now to midnight, as the original opens on now.
        let first = hour.saturating_sub(1);
        let today = now.date_naive();
        let top = |minute: u32| px((minute as f32 / 60. - first as f32) * HOUR);
        let hours = div()
            .relative()
            .h(px((24 - first) as f32 * HOUR))
            .children((first..24).map(|hour| {
                h_flex()
                    .absolute()
                    .top(top(hour * 60))
                    .left_0()
                    .right_0()
                    .gap_2()
                    .child(div().w(px(24.)).child(Label::new(hour.to_string()).size(LabelSize::XSmall).color(Color::Muted)))
                    .child(div().flex_1().h(px(1.)).bg(theme.colors().border_variant))
            }))
            .children(fern_core::focus::day_blocks(&self.sessions, today).into_iter().filter(|(_, to, _)| *to > first * 60).enumerate().map(|(i, (from, to, session))| {
                let dur = format_minutes(to.saturating_sub(from) as i64, strings);
                let tip = format!("{} · {:02}:{:02}–{:02}:{:02} ({dur})", session.title, from / 60, from % 60, to / 60, to % 60);
                div()
                    .id(SharedString::from(format!("focus-page-block-{i}")))
                    .absolute()
                    .top(top(from.max(first * 60)))
                    .left(px(32.))
                    .right_0()
                    .h(px(((to - from.max(first * 60)) as f32 / 60. * HOUR).max(4.)))
                    .px_2()
                    .rounded(px(4.))
                    .bg(theme.colors().accent.opacity(0.25))
                    .hover(|s| s.bg(theme.colors().accent.opacity(0.38)))
                    .border_l_2()
                    .border_color(theme.colors().accent)
                    .overflow_hidden()
                    .tooltip(Tooltip::text(tip))
                    .child(Label::new(session.title.clone()).size(LabelSize::XSmall))
            }))
            .child(
                h_flex()
                    .absolute()
                    .top(top(hour * 60 + minute) - px(3.))
                    .left(px(28.))
                    .right_0()
                    .child(div().size(px(6.)).rounded_full().bg(theme.status().error))
                    .child(div().flex_1().h(px(1.)).bg(theme.status().error)),
            );
        let stopwatch = self.stopwatch_mode();
        let right = v_flex()
            .w(px(490.))
            .flex_none()
            .h_full()
            .px_4()
            .child(
                h_flex()
                    .mt(crate::title_bar::header_inset())
                    .h(px(52.))
                    .gap_2()
                    .child(Icon::new(if stopwatch { IconName::Timer } else { IconName::FernFocus }).color(Color::Accent))
                    .child(div().flex_1().child(Label::new(strings.focus_modes[stopwatch as usize]).size(LabelSize::Large).color(Color::Muted)))
                    .child(
                        div()
                            .relative()
                            .child(
                                IconButton::new("focus-page-eye", IconName::Eye)
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text(strings.focus))
                                    .on_click(cx.listener(|this, _, window, cx| this.open_picker(true, window, cx))),
                            )
                            .children(picker_eye),
                    ),
            )
            .child(v_scroll("focus-page-hours-scroll", v_flex().id("focus-page-hours").pt_2().child(hours)).flex_1().min_h_0());
        self.bind_focus_root(h_flex().relative().size_full(), cx)
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(left)
            .child(right)
            .into_any_element()
    }
}
