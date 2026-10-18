//! Fires task reminders as desktop notifications while the app runs, and keeps them for the
//! sidebar's notification bell together with the ones missed while the app was closed.

use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use fern_core::{Id, Store, reminder};
use gpui::{AnyWindowHandle, Context};

use crate::app_view::AppView;
use crate::i18n;
use crate::task_rows;

/// How often reminders are checked.
const CHECK_EVERY: Duration = Duration::from_secs(20);
/// The setting with the last time reminders were checked, to find the ones missed in between.
const CHECKED_AT: &str = "reminders_checked_at";
const STAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// What a reminder is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Task(Id),
    Habit(Id),
}

/// What a habit's reminder offers besides opening it, as in the original: "Snooze", and "Check
/// In" for a habit without a goal, or "Record" (type the amount) and "+1" for one with a goal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    Snooze,
    CheckIn,
    Record,
    PlusOne,
}

/// How long "Snooze" puts a reminder off.
pub const SNOOZE_MINUTES: i64 = 15;

impl NoticeAction {
    pub fn for_habit(has_goal: bool) -> &'static [NoticeAction] {
        if has_goal {
            &[NoticeAction::Snooze, NoticeAction::Record, NoticeAction::PlusOne]
        } else {
            &[NoticeAction::Snooze, NoticeAction::CheckIn]
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            NoticeAction::Snooze => "snooze",
            NoticeAction::CheckIn => "check-in",
            NoticeAction::Record => "record",
            NoticeAction::PlusOne => "plus-one",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [NoticeAction::Snooze, NoticeAction::CheckIn, NoticeAction::Record, NoticeAction::PlusOne]
            .into_iter()
            .find(|action| action.key() == key)
    }

    pub fn label(self, strings: &'static i18n::Strings) -> &'static str {
        match self {
            NoticeAction::Snooze => strings.snooze,
            NoticeAction::CheckIn => strings.check_in,
            NoticeAction::Record => strings.habit_record,
            NoticeAction::PlusOne => "+1",
        }
    }
}

/// What a click on a desktop notification asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Open,
    Action(NoticeAction),
}

/// A reminder that fired.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub target: Target,
    pub title: String,
    pub at: NaiveDateTime,
}

/// The bell's notifications, newest first, and how many are unread.
#[derive(Default)]
pub struct Notices {
    pub items: Vec<Notice>,
    pub unread: usize,
}

impl Notices {
    pub fn push(&mut self, mut new: Vec<Notice>) {
        new.sort_by_key(|n| std::cmp::Reverse(n.at));
        self.unread += new.len();
        self.items.splice(0..0, new);
    }

    pub fn mark_read(&mut self) {
        self.unread = 0;
    }
}

/// Reminders of open tasks and of habits not yet done that day, firing in `(after, until]`.
pub fn fired(store: &Store, after: NaiveDateTime, until: NaiveDateTime) -> Vec<Notice> {
    let tasks = store.tasks().unwrap_or_default();
    let mut notices: Vec<Notice> = reminder::due_between(&tasks, after, until)
        .into_iter()
        .map(|(task, at)| Notice { target: Target::Task(task.id), title: task.title.clone(), at })
        .collect();
    let habits = store.habits().unwrap_or_default();
    let done = habits.iter().map(|h| (h.id, store.checkins(h.id).unwrap_or_default())).collect();
    notices.extend(
        fern_core::habit::reminders_between(&habits, &done, after, until)
            .into_iter()
            .map(|(habit, at)| Notice { target: Target::Habit(habit.id), title: habit.name.clone(), at }),
    );
    let snoozes = store.habit_snoozes().unwrap_or_default();
    notices.extend(
        fern_core::habit::snoozed_between(&habits, &done, &snoozes, after, until)
            .into_iter()
            .map(|(habit, at)| Notice { target: Target::Habit(habit.id), title: habit.name.clone(), at }),
    );
    notices
}

/// The actions a notice offers: none for tasks.
pub fn actions(store: &Store, target: Target) -> &'static [NoticeAction] {
    match target {
        Target::Task(_) => &[],
        Target::Habit(id) => {
            let has_goal = store.habits().unwrap_or_default().iter().any(|h| h.id == id && h.goal.is_some());
            NoticeAction::for_habit(has_goal)
        }
    }
}

/// Shows a desktop notification with its actions as buttons, and waits on a thread of its own for
/// the one clicked, if any.
fn show(title: String, body: String, actions: Vec<(&'static str, &'static str)>, constant: bool) -> futures::channel::oneshot::Receiver<Reply> {
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let mut notification = notify_rust::Notification::new();
        notification.appname("Fern").summary(&title).body(&body).icon("alarm-clock");
        // Settings → Notifications → "Constant Reminder": the notification stays until dismissed.
        if constant {
            notification.timeout(notify_rust::Timeout::Never);
        }
        for (key, label) in &actions {
            notification.action(key, label);
        }
        notification.action("default", &title);
        match notification.show() {
            #[cfg(all(unix, not(target_os = "macos")))]
            Ok(handle) => handle.wait_for_action(|key| {
                let reply = if key == "default" { Some(Reply::Open) } else { NoticeAction::from_key(key).map(Reply::Action) };
                if let Some(reply) = reply {
                    tx.send(reply).ok();
                }
            }),
            #[cfg(not(all(unix, not(target_os = "macos"))))]
            Ok(_) => drop(tx),
            Err(err) => eprintln!("failed to show a reminder: {err}"),
        }
    });
    rx
}

/// The day the daily notification was last shown.
const DAILY_SHOWN: &str = "daily_notification_shown";

/// The original's "Daily Notifications": once a day at its time, today's open tasks in one
/// notification.
fn show_daily(store: &Store, settings: fern_core::Notifications, now: NaiveDateTime, cx: &mut gpui::AsyncApp) {
    let last = store.setting(DAILY_SHOWN).ok().flatten().and_then(|d| chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok());
    if !settings.daily_due(now, last) {
        return;
    }
    store.set_setting(DAILY_SHOWN, &now.date().format("%Y-%m-%d").to_string()).ok();
    let tasks = store.tasks_in(&fern_core::View::Today, now.date()).unwrap_or_default();
    let open: Vec<String> = tasks.into_iter().filter(|t| !t.is_closed()).map(|t| t.title).collect();
    let (title, body) = cx.update(|cx| daily_text(&open, i18n::t(cx)));
    // Nothing to answer: a click on it only opens the app where the system does that.
    drop(show(title, body, Vec::new(), settings.constant));
}

/// The daily notification's title and body: how many tasks today, and the first few.
fn daily_text(titles: &[String], strings: &i18n::Strings) -> (String, String) {
    let title = strings.daily_title.replace("{n}", &titles.len().to_string());
    let mut body: Vec<String> = titles.iter().take(5).map(|t| format!("• {t}")).collect();
    if titles.len() > 5 {
        body.push("…".to_owned());
    }
    if titles.is_empty() {
        body.push(strings.daily_empty.to_owned());
    }
    (title, body.join("\n"))
}

/// Starts checking for reminders: collects the ones missed since the last run, then shows new
/// ones as desktop notifications.
pub fn start(store: Rc<Store>, window: AnyWindowHandle, cx: &mut Context<AppView>) {
    // UI screenshots run the app with demo data; they must not notify on the real desktop.
    if std::env::var_os("FERN_DEV_STATE").is_some() {
        return;
    }
    cx.spawn(async move |this, cx| {
        let now = Local::now().naive_local();
        let checked_at = store
            .setting(CHECKED_AT)
            .ok()
            .flatten()
            .and_then(|stamp| NaiveDateTime::parse_from_str(&stamp, STAMP_FORMAT).ok());
        let mut last = now;
        if let Some(checked_at) = checked_at {
            let missed = fired(&store, checked_at, now);
            if this.update(cx, |this, cx| this.add_notices(missed, cx)).is_err() {
                return;
            }
        }
        loop {
            store.set_setting(CHECKED_AT, &last.format(STAMP_FORMAT).to_string()).ok();
            cx.background_executor().timer(CHECK_EVERY).await;
            let now = Local::now().naive_local();
            let settings = store.preferences().unwrap_or_default().notifications;
            show_daily(&store, settings, now, cx);
            let notices = fired(&store, last, now);
            last = now;
            if notices.is_empty() {
                continue;
            }
            // With desktop reminders off, reminders only go to the bell.
            if !settings.desktop {
                if this.update(cx, |this, cx| this.add_notices(notices, cx)).is_err() {
                    break;
                }
                continue;
            }
            let shown: Vec<_> = cx.update(|cx| {
                let strings = i18n::t(cx);
                notices
                    .iter()
                    .map(|n| {
                        let due = task_rows::due_label(n.at.date(), Some(n.at.time()), now.date(), strings).0;
                        let buttons = actions(&store, n.target).iter().map(|a| (a.key(), a.label(strings))).collect();
                        (n.clone(), due, buttons)
                    })
                    .collect()
            });
            for (notice, due, buttons) in shown {
                let reply = show(notice.title.clone(), due, buttons, settings.constant);
                let this = this.clone();
                cx.spawn(async move |cx| {
                    let Ok(reply) = reply.await else { return };
                    cx.update(|cx| {
                        window
                            .update(cx, |_, window, cx| {
                                this.update(cx, |this, cx| this.answer_notice(&notice, reply, window, cx)).ok();
                            })
                            .ok();
                    });
                })
                .detach();
            }
            if this.update(cx, |this, cx| this.add_notices(notices, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn notice(title: &str, hour: u32) -> Notice {
        let at = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap().and_hms_opt(hour, 0, 0).unwrap();
        Notice { target: Target::Task(uuid::Uuid::now_v7()), title: title.into(), at }
    }

    #[test]
    fn daily_notification_text() {
        let titles: Vec<String> = (1..=7).map(|i| format!("Task {i}")).collect();
        let (title, body) = daily_text(&titles, &i18n::EN);
        assert_eq!(title, "Today: 7 tasks");
        assert_eq!(body.lines().count(), 6, "five tasks and an ellipsis");
        let (_, empty) = daily_text(&[], &i18n::EN);
        assert_eq!(empty, i18n::EN.daily_empty);
    }

    #[test]
    fn newest_first_and_unread_count() {
        let mut notices = Notices::default();
        notices.push(vec![notice("a", 9), notice("b", 10)]);
        notices.mark_read();
        notices.push(vec![notice("c", 11)]);
        let titles: Vec<&str> = notices.items.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["c", "b", "a"]);
        assert_eq!(notices.unread, 1);
    }

    #[test]
    fn missed_reminders_are_found_between_checks() {
        let store = Store::open_in_memory().unwrap();
        let task = store.add_task("water the plants").unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        store.set_due(task.id, Some(day), None).unwrap();
        store.set_reminders(task.id, &[0]).unwrap();
        let at = |h| day.and_hms_opt(h, 0, 0).unwrap();
        assert_eq!(fired(&store, at(8), at(10)).len(), 1);
        assert!(fired(&store, at(9), at(10)).is_empty());

        let habit = store.add_habit("Read").unwrap();
        store.set_habit_reminders(habit.id, &[chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap()]).unwrap();
        let notices = fired(&store, at(9), at(10));
        assert_eq!(notices.iter().map(|n| n.target).collect::<Vec<_>>(), [Target::Habit(habit.id)]);
        store.toggle_checkin(habit.id, day).unwrap();
        assert!(fired(&store, at(9), at(10)).is_empty(), "a habit done today is not reminded");
    }

    #[test]
    fn habit_reminder_actions_and_snooze() {
        let store = Store::open_in_memory().unwrap();
        let task = store.add_task("water the plants").unwrap();
        assert!(actions(&store, Target::Task(task.id)).is_empty());
        let read = store.add_habit("Read").unwrap();
        assert_eq!(actions(&store, Target::Habit(read.id)), [NoticeAction::Snooze, NoticeAction::CheckIn]);
        let water = store.add_habit("Water").unwrap();
        store.update_habit(water.id, "Water", Some(&fern_core::Goal { amount: 8, unit: "glasses".into() })).unwrap();
        assert_eq!(
            actions(&store, Target::Habit(water.id)),
            [NoticeAction::Snooze, NoticeAction::Record, NoticeAction::PlusOne]
        );
        for action in actions(&store, Target::Habit(water.id)) {
            assert_eq!(NoticeAction::from_key(action.key()), Some(*action));
        }

        // A snoozed reminder fires again when its time comes.
        let at = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap().and_hms_opt(9, 0, 0).unwrap();
        store.snooze_habit(read.id, at + chrono::Duration::minutes(SNOOZE_MINUTES)).unwrap();
        let notices = fired(&store, at, at + chrono::Duration::minutes(20));
        assert_eq!(notices.iter().map(|n| n.target).collect::<Vec<_>>(), [Target::Habit(read.id)]);
    }
}
