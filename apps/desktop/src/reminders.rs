//! Fires task reminders as desktop notifications while the app runs, and keeps them for the
//! sidebar's notification bell together with the ones missed while the app was closed.

use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use fern_core::{Id, Store, reminder};
use gpui::{AppContext as _, Context};

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
    notices
}

/// Starts checking for reminders: collects the ones missed since the last run, then shows new
/// ones as desktop notifications.
pub fn start(store: Rc<Store>, cx: &mut Context<AppView>) {
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
            let notices = fired(&store, last, now);
            last = now;
            if notices.is_empty() {
                continue;
            }
            let shown: Vec<(String, String)> = cx.update(|cx| {
                let strings = i18n::t(cx);
                notices
                    .iter()
                    .map(|n| {
                        let due = task_rows::due_label(n.at.date(), Some(n.at.time()), now.date(), strings).0;
                        (n.title.clone(), due)
                    })
                    .collect::<Vec<_>>()
            });
            for (title, due) in shown {
                cx.background_spawn(async move {
                    let shown =
                        notify_rust::Notification::new().appname("Fern").summary(&title).body(&due).icon("alarm-clock").show();
                    if let Err(err) = shown {
                        eprintln!("failed to show a reminder: {err}");
                    }
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
}
