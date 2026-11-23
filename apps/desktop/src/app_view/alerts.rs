use ui::prelude::*;
use ui::popover;

use crate::i18n;
use crate::reminders::{self, Notice, NoticeAction, Reply, Target};
use crate::sidebar::Section;
use crate::title_bar;

use super::AppView;

impl AppView {
    pub fn add_notices(&mut self, notices: Vec<Notice>, cx: &mut Context<Self>) {
        if !notices.is_empty() {
            self.notices.push(notices);
            cx.notify();
        }
    }

    pub fn push_active_alerts(&mut self, mut notices: Vec<Notice>, cx: &mut Context<Self>) {
        if notices.is_empty() {
            return;
        }
        notices.sort_by_key(|n| std::cmp::Reverse(n.at));
        for notice in notices.into_iter().rev() {
            self.active_alerts.retain(|existing| existing.target != notice.target);
            self.active_alerts.insert(0, notice);
        }
        self.active_alerts.truncate(5);
        cx.notify();
    }

    pub fn has_active_alert(&self, target: Target) -> bool {
        self.active_alerts.iter().any(|n| n.target == target)
    }

    /// Opening the list marks the notifications read, which clears the badge.
    pub(crate) fn set_notices_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.notices_open = open;
        if open {
            self.notices.mark_read();
        }
        cx.notify();
    }

    fn open_notice(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.active_alerts.retain(|n| n.target != target);
        self.set_notices_open(false, cx);
        match target {
            Target::Task(task) => self.open_task(task, window, cx),
            Target::Habit(habit) => {
                if self.select(Section::Habits, window, cx) {
                    self.habits.update(cx, |habits, cx| habits.select(habit, cx));
                }
            }
            Target::Countdown(_) => {
                self.select(Section::Countdown, window, cx);
            }
        }
    }

    fn complete_task_from_notice(
        &mut self,
        id: fern_core::Id,
        notice: &Notice,
        cx: &mut Context<Self>,
    ) {
        match self.store.complete(id, true) {
            Ok(_) => desktop_common::sound::play_complete(&self.store),
            Err(err) => eprintln!("failed to answer the reminder: {err:#}"),
        }
        self.notices.items.retain(|n| n != notice);
        desktop_common::data_version::data_changed(cx.entity_id(), cx);
        self.todo.update(cx, |todo, cx| todo.sync_external(cx));
        self.things.update(cx, |things, cx| things.sync_external(cx));
        self.calendar.update(cx, |calendar, cx| calendar.sync_external(cx));
        self.matrix.update(cx, |matrix, cx| matrix.sync_external(cx));
        cx.notify();
    }

    /// A click on a reminder, in the bell, floating banner, or desktop notification.
    pub fn answer_notice(&mut self, notice: &Notice, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        self.active_alerts.retain(|n| n.target != notice.target);
        let Reply::Action(action) = reply else {
            window.activate_window();
            self.open_notice(notice.target, window, cx);
            return;
        };
        if let Target::Task(id) = notice.target
            && action == NoticeAction::Complete
            && let Some(mgr) = crate::quick_add_window::active_hook_manager(cx)
            && let Ok(Some(task)) = self.store.task(id)
        {
            let weak = cx.weak_entity();
            let notice_clone = notice.clone();
            crate::quick_add_window::run_before_task_complete(mgr, &task, cx, move |allowed, cx| {
                if allowed {
                    let _ = weak.update(cx, |this, cx| {
                        this.complete_task_from_notice(id, &notice_clone, cx);
                    });
                }
            });
            cx.notify();
            return;
        }
        let now = chrono::Local::now().naive_local();
        let snooze_minutes = match action {
            NoticeAction::Snooze5m => Some(5),
            NoticeAction::Snooze | NoticeAction::Snooze15m => Some(reminders::SNOOZE_MINUTES),
            NoticeAction::Snooze1h => Some(60),
            _ => None,
        };
        let mut touched_tasks = false;
        let mut touched_habits = false;
        let done = match notice.target {
            Target::Task(id) => {
                touched_tasks = true;
                if let Some(mins) = snooze_minutes {
                    self.store.snooze_task(id, now + chrono::Duration::minutes(mins))
                } else {
                    match action {
                        NoticeAction::Complete => {
                            let res = self.store.complete(id, true).map(|_| ());
                            if res.is_ok() {
                                desktop_common::sound::play_complete(&self.store);
                            }
                            res
                        }
                        NoticeAction::Dismiss => self.store.clear_task_snooze(id),
                        _ => Ok(()),
                    }
                }
            }
            Target::Habit(habit) => {
                touched_habits = true;
                if let Some(mins) = snooze_minutes {
                    self.store.snooze_habit(habit, now + chrono::Duration::minutes(mins))
                } else {
                    match action {
                        NoticeAction::CheckIn => {
                            let today = now.date();
                            if self.store.checkins(habit).unwrap_or_default().contains(&today) {
                                Ok(())
                            } else {
                                let res = self.store.toggle_checkin(habit, today).map(|_| ());
                                if res.is_ok() {
                                    desktop_common::sound::play_complete(&self.store);
                                }
                                res
                            }
                        }
                        NoticeAction::PlusOne => {
                            let step = self
                                .store
                                .habits()
                                .unwrap_or_default()
                                .into_iter()
                                .find(|h| h.id == habit)
                                .and_then(|h| h.goal)
                                .map_or(1.0, |g| g.step_amount());
                            self.store.add_checkin_amount(habit, now.date(), step).map(|_| ())
                        }
                        NoticeAction::Record => {
                            window.activate_window();
                            self.set_notices_open(false, cx);
                            if !self.select(Section::Habits, window, cx) {
                                return;
                            }
                            self.habits.update(cx, |habits, cx| habits.record(habit, window, cx));
                            Ok(())
                        }
                        _ => Ok(()),
                    }
                }
            }
            Target::Countdown(_) => Ok(()),
        };
        if let Err(err) = done {
            eprintln!("failed to answer the reminder: {err:#}");
        }
        self.notices.items.retain(|n| n != notice);
        desktop_common::data_version::data_changed(cx.entity_id(), cx);
        if touched_tasks {
            self.todo.update(cx, |todo, cx| todo.sync_external(cx));
            self.things.update(cx, |things, cx| things.sync_external(cx));
            self.calendar.update(cx, |calendar, cx| calendar.sync_external(cx));
            self.matrix.update(cx, |matrix, cx| matrix.sync_external(cx));
        }
        if touched_habits {
            self.habits.update(cx, |habits, cx| habits.sync_external(cx));
        }
        cx.notify();
    }

    pub(super) fn render_active_alerts(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.active_alerts.is_empty() {
            return None;
        }
        let strings = i18n::t(cx);
        let today = chrono::Local::now().date_naive();
        let theme = cx.theme().clone();
        Some(
            v_flex()
                .id("active-reminder-alerts")
                .debug_selector(|| "active-reminder-alerts".to_owned())
                .occlude()
                .absolute()
                .top(title_bar::height() + px(12.))
                .right(px(16.))
                .w(px(320.))
                .gap_2()
                .children(self.active_alerts.iter().enumerate().map(|(i, notice)| {
                    let target = notice.target;
                    let focus_title = notice.title.clone();
                    let (when, _) = crate::task_rows::due_label(notice.at.date(), Some(notice.at.time()), today, strings);
                    let actions = notice.actions();
                    let dismiss_notice = notice.clone();
                    let focus_notice = notice.clone();
                    let can_focus = matches!(target, Target::Task(_) | Target::Habit(_));
                    v_flex()
                        .id(SharedString::from(format!("alert-{i}")))
                        .p_3()
                        .gap_2()
                        .rounded(px(10.))
                        .bg(theme.colors().elevated_surface_background)
                        .border_1()
                        .border_color(if notice.constant { theme.colors().accent } else { theme.colors().border })
                        .shadow_md()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(Icon::new(IconName::AlarmClock).size(IconSize::Small).color(Color::Accent))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("alert-{i}-open")))
                                        .flex_1()
                                        .min_w_0()
                                        .cursor_pointer()
                                        .tooltip(Tooltip::text(notice.title.clone()))
                                        .child(Label::new(notice.title.clone()).truncate())
                                        .on_click(cx.listener(move |this, _, window, cx| this.open_notice(target, window, cx))),
                                )
                                .when(can_focus, |el| {
                                    el.child(
                                        IconButton::new(SharedString::from(format!("alert-{i}-focus")), IconName::CircleDot)
                                            .size(px(22.))
                                            .icon_size(IconSize::XSmall)
                                            .icon_color(Color::Muted)
                                            .hover_icon_color(Color::Accent)
                                            .tooltip(Tooltip::text(strings.start_pomo))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.active_alerts.retain(|n| n.target != focus_notice.target);
                                                this.notices.items.retain(|n| n != &focus_notice);
                                                match target {
                                                    Target::Task(id) => {
                                                        this.focus.update(cx, |focus, cx| {
                                                            focus.start_for(Some(id), focus_title.clone(), false, cx);
                                                        });
                                                    }
                                                    Target::Habit(id) => {
                                                        this.focus.update(cx, |focus, cx| {
                                                            focus.start_for_habit(Some(id), focus_title.clone(), false, cx);
                                                        });
                                                    }
                                                    Target::Countdown(_) => {}
                                                }
                                                let _ = this.select(Section::Pomodoro, window, cx);
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(
                                    div()
                                        .id(SharedString::from(format!("alert-{i}-dismiss")))
                                        .debug_selector(move || format!("alert-{i}-dismiss"))
                                        .child(
                                            IconButton::new(SharedString::from(format!("alert-{i}-dismiss-btn")), IconName::X)
                                                .size(px(22.))
                                                .icon_size(IconSize::XSmall)
                                                .icon_color(Color::Muted)
                                                .hover_icon_color(Color::Icon)
                                                .tooltip(Tooltip::text(strings.dismiss))
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    cx.stop_propagation();
                                                    this.answer_notice(&dismiss_notice, Reply::Action(NoticeAction::Dismiss), window, cx);
                                                })),
                                        ),
                                ),
                        )
                        .child(Label::new(when).size(LabelSize::XSmall).color(Color::Muted))
                        .when(!actions.is_empty(), |el| {
                            el.child(
                                h_flex().flex_wrap().gap_1().children(actions.iter().map(|&action| {
                                    let notice = notice.clone();
                                    let key = action.key();
                                    let primary = matches!(
                                        action,
                                        NoticeAction::Complete | NoticeAction::CheckIn | NoticeAction::PlusOne
                                    );
                                    div()
                                        .debug_selector(move || format!("alert-{i}-{key}"))
                                        .child(
                                            Button::new(
                                                SharedString::from(format!("alert-{i}-{key}")),
                                                action.label(strings),
                                            )
                                            .variant(if primary {
                                                ButtonVariant::Primary
                                            } else {
                                                ButtonVariant::Secondary
                                            })
                                            .size(ButtonSize::Sm)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.answer_notice(&notice, Reply::Action(action), window, cx);
                                            })),
                                        )
                                })),
                            )
                        })
                }))
                .into_any_element(),
        )
    }

    pub(super) fn render_notices(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let today = chrono::Local::now().date_naive();
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(300.))
            .max_h(px(420.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_notices_open(false, cx)))
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .items_center()
                    .child(div().flex_1().child(Label::new(strings.notifications).weight(gpui::FontWeight::SEMIBOLD)))
                    .when(!self.notices.items.is_empty() || !self.active_alerts.is_empty(), |el| {
                        el.child(
                            div()
                                .debug_selector(|| "notices-clear-all".to_owned())
                                .child(
                                    Button::new("notices-clear-all", strings.clear)
                                        .variant(ButtonVariant::Ghost)
                                        .size(ButtonSize::Sm)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.notices.items.clear();
                                            this.notices.unread = 0;
                                            this.active_alerts.clear();
                                            cx.notify();
                                        })),
                                ),
                        )
                    }),
            )
            .when(self.notices.items.is_empty(), |el| {
                el.child(
                    v_flex()
                        .py_6()
                        .px_3()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Bell).size(IconSize::Medium).color(Color::Muted))
                        .child(Label::new(strings.no_notifications).size(LabelSize::Small).color(Color::Muted)),
                )
            })
            .child(
                v_scroll(
                    "notices-scroll",
                    v_flex().id("notices").children(self.notices.items.iter().enumerate().map(|(i, notice)| {
                        let target = notice.target;
                        let (when, _) = crate::task_rows::due_label(notice.at.date(), Some(notice.at.time()), today, strings);
                        let actions = notice.actions();
                        let buttons = actions.iter().map(|&action| {
                            let notice = notice.clone();
                            let primary = matches!(
                                action,
                                NoticeAction::Complete | NoticeAction::CheckIn | NoticeAction::PlusOne
                            );
                            Button::new(
                                SharedString::from(format!("notice-{i}-{}", action.key())),
                                action.label(strings),
                            )
                            .variant(if primary {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            })
                            .size(ButtonSize::Sm)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.answer_notice(&notice, Reply::Action(action), window, cx);
                            }))
                        });
                        h_flex()
                            .id(SharedString::from(format!("notice-{i}")))
                            .debug_selector(move || format!("notice-{i}"))
                            .min_h(px(44.))
                            .w_full()
                            .items_start()
                            .gap_2()
                            .px_2()
                            .py_1p5()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.colors().ghost_element_hover))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_notice(target, window, cx)))
                            .child(
                                div()
                                    .flex_none()
                                    .pt(px(2.))
                                    .child(Icon::new(IconName::AlarmClock).color(Color::Accent)),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(Label::new(notice.title.clone()).truncate())
                                    .child(Label::new(when).size(LabelSize::XSmall).color(Color::Muted))
                                    .when(!actions.is_empty(), |el| {
                                        el.child(h_flex().flex_wrap().gap_1().children(buttons))
                                    }),
                            )
                    })),
                )
                .min_h_0()
                .compact(),
            )
            .into_any_element()
    }
}
