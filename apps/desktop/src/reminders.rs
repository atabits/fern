//! Fires task reminders as desktop notifications while the app runs, and keeps them for the
//! sidebar's notification bell together with the ones missed while the app was closed.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use fern_core::{Id, Store, reminder};
use gpui::{AnyWindowHandle, AppContext as _, Context};

use crate::app_view::AppView;
use crate::i18n;
use crate::task_rows;

mod platform;
#[cfg(test)]
mod tests;

/// How often reminders are checked.
const CHECK_EVERY: Duration = Duration::from_secs(20);
/// How often constant reminders ("annoying alerts") re-notify while unacknowledged.
pub const ANNOYING_REPEAT_SECS: u64 = 60;
const ANNOYING_MAX_REPEATS: usize = 15;
/// The setting with the last time reminders were checked, to find the ones missed in between.
const CHECKED_AT: &str = "reminders_checked_at";
const STAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";
/// Minimum interval between idle `CHECKED_AT` SQLite writes when no reminders fire.
const CHECKED_AT_PERSIST_INTERVAL: chrono::Duration = chrono::Duration::minutes(10);

/// What a reminder is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Task(Id),
    Habit(Id),
    Countdown(Id),
}

/// Actions offered on a reminder notification (both OS toast and in-app banner/bell).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    Snooze,
    Snooze5m,
    Snooze15m,
    Snooze1h,
    Complete,
    Dismiss,
    CheckIn,
    Record,
    PlusOne,
}

/// How long default "Snooze" puts a habit reminder off.
pub const SNOOZE_MINUTES: i64 = 15;

impl NoticeAction {
    pub fn for_task() -> &'static [NoticeAction] {
        &[
            NoticeAction::Snooze5m,
            NoticeAction::Snooze15m,
            NoticeAction::Snooze1h,
            NoticeAction::Complete,
        ]
    }

    pub fn for_countdown() -> &'static [NoticeAction] {
        &[NoticeAction::Dismiss]
    }

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
            NoticeAction::Snooze5m => "snooze-5m",
            NoticeAction::Snooze15m => "snooze-15m",
            NoticeAction::Snooze1h => "snooze-1h",
            NoticeAction::Complete => "complete",
            NoticeAction::Dismiss => "dismiss",
            NoticeAction::CheckIn => "check-in",
            NoticeAction::Record => "record",
            NoticeAction::PlusOne => "plus-one",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        [
            NoticeAction::Snooze,
            NoticeAction::Snooze5m,
            NoticeAction::Snooze15m,
            NoticeAction::Snooze1h,
            NoticeAction::Complete,
            NoticeAction::Dismiss,
            NoticeAction::CheckIn,
            NoticeAction::Record,
            NoticeAction::PlusOne,
        ]
        .into_iter()
        .find(|action| action.key() == key)
    }

    pub fn label(self, strings: &'static i18n::Strings) -> &'static str {
        match self {
            NoticeAction::Snooze => strings.snooze,
            NoticeAction::Snooze5m => strings.snooze_5m,
            NoticeAction::Snooze15m => strings.snooze_15m,
            NoticeAction::Snooze1h => strings.snooze_1h,
            NoticeAction::Complete => strings.mark_done,
            NoticeAction::Dismiss => strings.dismiss,
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
    pub constant: bool,
    /// Cached `habit.goal.is_some()` for `Target::Habit` so `render()` never queries SQLite.
    pub has_goal: bool,
}

impl Notice {
    /// The actions offered for this notice without hitting the database.
    pub fn actions(&self) -> &'static [NoticeAction] {
        match self.target {
            Target::Task(_) => NoticeAction::for_task(),
            Target::Countdown(_) => NoticeAction::for_countdown(),
            Target::Habit(_) => NoticeAction::for_habit(self.has_goal),
        }
    }
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

/// Reminders of open tasks, of habits not yet done that day and of countdowns, firing in
/// `(after, until]`.
pub fn fired(store: &Store, after: NaiveDateTime, until: NaiveDateTime) -> Vec<Notice> {
    let tasks = store.tasks().unwrap_or_default();
    let mut notices: Vec<Notice> = reminder::due_between(&tasks, after, until)
        .into_iter()
        .map(|(task, at)| Notice {
            target: Target::Task(task.id),
            title: task.title.clone(),
            at,
            constant: task.constant_reminder,
            has_goal: false,
        })
        .collect();
    let habits = store.habits().unwrap_or_default();
    let done = store.all_habit_done_dates().unwrap_or_default();
    notices.extend(
        fern_core::habit::reminders_between(&habits, &done, after, until)
            .into_iter()
            .map(|(habit, at)| Notice {
                target: Target::Habit(habit.id),
                title: habit.name.clone(),
                at,
                constant: habit.constant_reminder,
                has_goal: habit.goal.is_some(),
            }),
    );
    let countdowns = store.countdowns().unwrap_or_default();
    notices.extend(
        fern_core::countdown::reminders_between(&countdowns, after, until)
            .into_iter()
            .map(|(countdown, at)| Notice {
                target: Target::Countdown(countdown.id),
                title: countdown.name.clone(),
                at,
                constant: countdown.constant_reminder,
                has_goal: false,
            }),
    );
    let snoozes = store.habit_snoozes().unwrap_or_default();
    notices.extend(
        fern_core::habit::snoozed_between(&habits, &done, &snoozes, after, until)
            .into_iter()
            .map(|(habit, at)| Notice {
                target: Target::Habit(habit.id),
                title: habit.name.clone(),
                at,
                constant: habit.constant_reminder,
                has_goal: habit.goal.is_some(),
            }),
    );
    notices
}

#[cfg(test)]
pub fn actions(store: &Store, target: Target) -> &'static [NoticeAction] {
    match target {
        Target::Task(_) => NoticeAction::for_task(),
        Target::Countdown(_) => NoticeAction::for_countdown(),
        Target::Habit(id) => {
            let has_goal = store.habits().unwrap_or_default().iter().any(|h| h.id == id && h.goal.is_some());
            NoticeAction::for_habit(has_goal)
        }
    }
}

#[cfg_attr(not(any(test, windows)), allow(dead_code))]
pub(crate) fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg_attr(not(any(test, windows)), allow(dead_code))]
pub(crate) fn build_toast_xml(title: &str, body: &str, actions: &[(&str, &str)], constant: bool) -> String {
    let scenario = if constant { r#" scenario="reminder""# } else { "" };
    let title = escape_xml(title);
    let body = escape_xml(body);
    let mut xml = format!(
        r#"<toast launch="default"{scenario}><visual><binding template="ToastGeneric"><text>{title}</text><text>{body}</text></binding></visual>"#
    );
    if !actions.is_empty() {
        xml.push_str("<actions>");
        for &(key, label) in actions {
            let key = escape_xml(key);
            let label = escape_xml(label);
            xml.push_str(&format!(
                r#"<action content="{label}" arguments="{key}" activationType="foreground"/>"#
            ));
        }
        xml.push_str("</actions>");
    }
    xml.push_str("</toast>");
    xml
}

/// The day the daily notification was last shown.
const DAILY_SHOWN: &str = "daily_notification_shown";

/// Checks whether the daily summary notification is due, records today as shown, and returns
/// today's open task titles. Runs on a background thread when a WAL `DbPath` is available.
fn collect_daily_titles(
    store: &Store,
    settings: fern_core::Notifications,
    now: NaiveDateTime,
) -> Option<Vec<String>> {
    let last = store
        .setting(DAILY_SHOWN)
        .ok()
        .flatten()
        .and_then(|d| chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok());
    if !settings.daily_due(now, last) {
        return None;
    }
    set_setting_with_retry(store, DAILY_SHOWN, &now.date().format("%Y-%m-%d").to_string());
    let tasks = store.tasks_in(&fern_core::View::Today, now.date()).unwrap_or_default();
    let open: Vec<String> = tasks.into_iter().filter(|t| !t.is_closed()).map(|t| t.title).collect();
    Some(open)
}

/// Writes a setting, retrying briefly on transient SQLite busy errors from concurrent WAL writes.
fn set_setting_with_retry(store: &Store, key: &str, value: &str) {
    for attempt in 0..4 {
        if store.set_setting(key, value).is_ok() {
            return;
        }
        if attempt < 3 {
            std::thread::sleep(Duration::from_millis(25 * (attempt + 1)));
        }
    }
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

fn spawn_reply_listener(
    reply: futures::channel::oneshot::Receiver<Reply>,
    _cancel: Option<platform::ToastCancel>,
    notice: Notice,
    window: AnyWindowHandle,
    this: gpui::WeakEntity<AppView>,
    cx: &gpui::AsyncApp,
) {
    cx.spawn(async move |cx| {
        let _cancel = _cancel;
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

struct PollResult {
    settings: fern_core::Notifications,
    daily_titles: Option<Vec<String>>,
    notices: Vec<Notice>,
    persisted_checked_at: bool,
}

fn should_persist_checked_at(
    has_notices: bool,
    has_daily: bool,
    last_persisted: NaiveDateTime,
    now: NaiveDateTime,
) -> bool {
    has_notices
        || has_daily
        || now.signed_duration_since(last_persisted) >= CHECKED_AT_PERSIST_INTERVAL
        || now < last_persisted
}

fn is_target_still_open(store: &Store, target: Target, today: chrono::NaiveDate) -> bool {
    match target {
        Target::Task(id) => store
            .task(id)
            .ok()
            .flatten()
            .is_some_and(|t| !t.is_closed() && !t.is_deleted() && t.snoozed_until.is_none()),
        Target::Habit(id) => {
            let Some(habit) = store
                .habits()
                .ok()
                .and_then(|habits| habits.into_iter().find(|h| h.id == id && !h.archived))
            else {
                return false;
            };
            if habit.is_skipped(today) {
                return false;
            }
            !store.checkins(id).unwrap_or_default().contains(&today)
        }
        Target::Countdown(_) => true,
    }
}

fn poll_once(
    store: &Store,
    last: NaiveDateTime,
    last_persisted: NaiveDateTime,
    now: NaiveDateTime,
) -> PollResult {
    let settings = store.preferences().unwrap_or_default().notifications;
    let daily_titles = collect_daily_titles(store, settings, now);
    let notices = fired(store, last, now);
    let persisted_checked_at = should_persist_checked_at(
        !notices.is_empty(),
        daily_titles.is_some(),
        last_persisted,
        now,
    );
    if persisted_checked_at {
        set_setting_with_retry(store, CHECKED_AT, &now.format(STAMP_FORMAT).to_string());
    }
    PollResult {
        settings,
        daily_titles,
        notices,
        persisted_checked_at,
    }
}

fn open_or_reuse_bg_store(bg_store: Option<Store>, db_path: &Path) -> Option<Store> {
    bg_store.or_else(|| Store::open(db_path).ok())
}

fn apply_constant_override(notices: &mut [Notice], global_constant: bool) {
    if global_constant {
        for notice in notices {
            notice.constant = true;
        }
    }
}

fn notice_to_json(notice: &Notice) -> serde_json::Value {
    let (kind, target_id) = match notice.target {
        Target::Task(id) => ("task", id.to_string()),
        Target::Habit(id) => ("habit", id.to_string()),
        Target::Countdown(id) => ("countdown", id.to_string()),
    };
    serde_json::json!({
        "kind": kind,
        "targetId": target_id,
        "title": notice.title,
        "at": notice.at.format("%Y-%m-%dT%H:%M:%S").to_string(),
        "constant": notice.constant,
    })
}

/// Starts checking for reminders: collects the ones missed since the last run, then shows new
/// ones as desktop notifications. All periodic SQLite polling runs off the foreground UI thread.
pub fn start(store: Rc<Store>, window: AnyWindowHandle, cx: &mut Context<AppView>) {
    // UI screenshots run the app with demo data; they must not notify on the real desktop.
    if std::env::var_os("FERN_DEV_STATE").is_some() {
        return;
    }
    #[cfg(target_os = "macos")]
    platform::init_macos_notifications();
    let db_path = cx.try_global::<crate::DbPath>().map(|p| p.0.clone());
    cx.spawn(async move |this, cx| {
        let now = Local::now().naive_local();
        let mut last = now;
        let mut last_persisted = now;
        let mut bg_store: Option<Store> = None;

        // Collect missed reminders off the UI thread when backed by an on-disk WAL DB.
        let missed = if let Some(path) = db_path.clone() {
            let (returned_store, missed) = cx
                .background_spawn(async move {
                    let bg = open_or_reuse_bg_store(None, &path);
                    let missed = if let Some(ref s) = bg {
                        let checked_at = s
                            .setting(CHECKED_AT)
                            .ok()
                            .flatten()
                            .and_then(|stamp| NaiveDateTime::parse_from_str(&stamp, STAMP_FORMAT).ok());
                        let missed = checked_at.map(|after| fired(s, after, now)).unwrap_or_default();
                        set_setting_with_retry(s, CHECKED_AT, &now.format(STAMP_FORMAT).to_string());
                        missed
                    } else {
                        Vec::new()
                    };
                    (bg, missed)
                })
                .await;
            bg_store = returned_store;
            missed
        } else {
            let checked_at = store
                .setting(CHECKED_AT)
                .ok()
                .flatten()
                .and_then(|stamp| NaiveDateTime::parse_from_str(&stamp, STAMP_FORMAT).ok());
            let missed = checked_at.map(|after| fired(&store, after, now)).unwrap_or_default();
            store.set_setting(CHECKED_AT, &now.format(STAMP_FORMAT).to_string()).ok();
            missed
        };
        if !missed.is_empty() && this.update(cx, |this, cx| this.add_notices(missed, cx)).is_err() {
            return;
        }

        loop {
            cx.background_executor().timer(CHECK_EVERY).await;
            // Exit promptly if AppView has been dropped instead of leaking the loop forever.
            if this.upgrade().is_none() {
                break;
            }
            let now = Local::now().naive_local();
            let poll = if let Some(path) = db_path.clone() {
                let taken_store = bg_store.take();
                let prev = last;
                let prev_persisted = last_persisted;
                let (returned_store, poll) = cx
                    .background_spawn(async move {
                        let bg = open_or_reuse_bg_store(taken_store, &path);
                        let poll = bg.as_ref().map(|s| poll_once(s, prev, prev_persisted, now));
                        (bg, poll)
                    })
                    .await;
                bg_store = returned_store;
                match poll {
                    Some(poll) => poll,
                    None => continue,
                }
            } else {
                poll_once(&store, last, last_persisted, now)
            };
            last = now;
            if poll.persisted_checked_at {
                last_persisted = now;
            }

            if this.upgrade().is_none() {
                break;
            }
            if let Some(open_titles) = poll.daily_titles {
                let (title, body) = cx.update(|cx| daily_text(&open_titles, i18n::t(cx)));
                // Fire-and-forget: never blocks an OS thread waiting for a reply.
                platform::show_notification_only(title, body, poll.settings.constant);
            }
            if poll.notices.is_empty() {
                continue;
            }
            let mut notices = poll.notices;
            apply_constant_override(&mut notices, poll.settings.constant);
            if let Some(mgr) = cx.update(|cx| crate::quick_add_window::active_hook_manager(cx)) {
                let payloads: Vec<serde_json::Value> =
                    notices.iter().map(notice_to_json).collect();
                cx.background_spawn(async move {
                    for payload in payloads {
                        if let Err(err) = mgr.run_on_reminder(payload) {
                            eprintln!("plugin onReminder hook failed: {err:#}");
                        }
                    }
                })
                .detach();
            }
            // With desktop reminders off, reminders only go to the bell.
            if !poll.settings.desktop {
                if this.update(cx, |this, cx| this.add_notices(notices, cx)).is_err() {
                    break;
                }
                continue;
            }
            type ShownNotice = (Notice, String, Vec<(&'static str, &'static str)>);
            let shown: Vec<ShownNotice> = cx.update(|cx| {
                let strings = i18n::t(cx);
                notices
                    .into_iter()
                    .map(|n| {
                        let due = task_rows::due_label(n.at.date(), Some(n.at.time()), now.date(), strings).0;
                        let buttons = n.actions().iter().map(|a| (a.key(), a.label(strings))).collect();
                        (n, due, buttons)
                    })
                    .collect()
            });
            let live_notices: Vec<Notice> = shown.iter().map(|(n, _, _)| n.clone()).collect();
            if this
                .update(cx, |this, cx| {
                    this.add_notices(live_notices.clone(), cx);
                    this.push_active_alerts(live_notices, cx);
                })
                .is_err()
            {
                break;
            }
            for (notice, due, buttons) in shown {
                let constant = notice.constant;
                let (reply, initial_cancel) = platform::show(notice.title.clone(), due.clone(), buttons.clone(), constant);
                if constant {
                    spawn_reply_listener(reply, None, notice.clone(), window, this.clone(), cx);
                    let store = Rc::clone(&store);
                    let repeat_db_path = db_path.clone();
                    let this = this.clone();
                    cx.spawn(async move |cx| {
                        // Hold only the latest repeat's cancel guard: each repeat drops the
                        // previous toast waiter so at most 1 OS thread stays alive per alert.
                        let mut _active_cancel = initial_cancel;
                        for _ in 0..ANNOYING_MAX_REPEATS {
                            cx.background_executor().timer(Duration::from_secs(ANNOYING_REPEAT_SECS)).await;
                            let active = matches!(
                                this.read_with(cx, |this, _| this.has_active_alert(notice.target)),
                                Ok(true)
                            );
                            if !active {
                                break;
                            }
                            if matches!(notice.target, Target::Task(_) | Target::Habit(_)) {
                                let target = notice.target;
                                let today = Local::now().date_naive();
                                let still_open = if let Some(path) = repeat_db_path.clone() {
                                    cx.background_spawn(async move {
                                        Store::open(&path)
                                            .ok()
                                            .is_some_and(|s| is_target_still_open(&s, target, today))
                                    })
                                    .await
                                } else {
                                    is_target_still_open(&store, target, today)
                                };
                                if !still_open {
                                    break;
                                }
                            }
                            let (repeat_reply, repeat_cancel) = platform::show(notice.title.clone(), due.clone(), buttons.clone(), true);
                            _active_cancel = repeat_cancel;
                            spawn_reply_listener(repeat_reply, None, notice.clone(), window, this.clone(), cx);
                        }
                    })
                    .detach();
                } else {
                    spawn_reply_listener(reply, Some(initial_cancel), notice, window, this.clone(), cx);
                }
            }
        }
    })
    .detach();
}
