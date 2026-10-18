//! Fires task reminders as desktop notifications while the app runs.

use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use fern_core::{Store, reminder};
use gpui::{App, AppContext as _};

use crate::i18n;
use crate::task_rows;

/// How often reminders are checked.
const CHECK_EVERY: Duration = Duration::from_secs(20);

/// Starts checking for reminders. Reminders that came due while the app was closed are not
/// shown, so only reminders after the start fire.
pub fn start(store: Rc<Store>, cx: &mut App) {
    // UI screenshots run the app with demo data; they must not notify on the real desktop.
    if std::env::var_os("FERN_DEV_STATE").is_some() {
        return;
    }
    cx.spawn(async move |cx| {
        let mut last: NaiveDateTime = Local::now().naive_local();
        loop {
            cx.background_executor().timer(CHECK_EVERY).await;
            let now = Local::now().naive_local();
            let tasks = store.tasks().unwrap_or_default();
            let notifications: Vec<(String, String)> = cx
                .update(|cx| {
                    let strings = i18n::t(cx);
                    reminder::due_between(&tasks, last, now)
                        .into_iter()
                        .map(|(task, _)| {
                            let due = task
                                .due_date
                                .map(|date| task_rows::due_label(date, task.due_time, now.date(), strings).0)
                                .unwrap_or_default();
                            (task.title.clone(), due)
                        })
                        .collect::<Vec<_>>()
                });
            last = now;
            for (title, due) in notifications {
                cx.background_spawn(async move {
                    let shown = notify_rust::Notification::new()
                        .appname("Fern")
                        .summary(&title)
                        .body(&due)
                        .icon("alarm-clock")
                        .show();
                    if let Err(err) = shown {
                        eprintln!("failed to show a reminder: {err}");
                    }
                })
                .detach();
            }
        }
    })
    .detach();
}
