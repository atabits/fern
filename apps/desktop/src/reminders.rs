//! Fires task reminders as desktop notifications while the app runs, and keeps them for the
//! sidebar's notification bell together with the ones missed while the app was closed.

use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

use chrono::{Local, NaiveDateTime};
use fern_core::{Id, Store, reminder};
use gpui::{AnyWindowHandle, AppContext as _, Context};

use crate::app_view::AppView;
use crate::i18n;
use crate::task_rows;

/// How often reminders are checked.
const CHECK_EVERY: Duration = Duration::from_secs(20);
/// How often constant reminders ("annoying alerts") re-notify while unacknowledged.
pub const ANNOYING_REPEAT_SECS: u64 = 60;
const ANNOYING_MAX_REPEATS: usize = 15;
/// Maximum number of concurrent interactive toast waiter OS threads.
const MAX_TOAST_WAITERS: usize = 8;
/// Maximum time an interactive toast waiter stays alive waiting for a click.
#[cfg(windows)]
const TOAST_WAIT_TIMEOUT: Duration = Duration::from_secs(600);
/// Wake interval inside the waiter loop to detect dropped receivers without blocking 600s.
#[cfg(windows)]
const TOAST_POLL_SLICE: Duration = Duration::from_secs(5);
/// The setting with the last time reminders were checked, to find the ones missed in between.
const CHECKED_AT: &str = "reminders_checked_at";
const STAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

static ACTIVE_WAITERS: AtomicUsize = AtomicUsize::new(0);

struct WaiterSlot;

impl WaiterSlot {
    fn try_acquire() -> Option<Self> {
        let mut current = ACTIVE_WAITERS.load(Ordering::Acquire);
        loop {
            if current >= MAX_TOAST_WAITERS {
                return None;
            }
            match ACTIVE_WAITERS.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self),
                Err(next) => current = next,
            }
        }
    }
}

impl Drop for WaiterSlot {
    fn drop(&mut self) {
        ACTIVE_WAITERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Dropping this handle cancels the pending toast waiter thread immediately.
struct ToastCancel {
    slot: Arc<Mutex<Option<mpsc::Sender<Option<Reply>>>>>,
}

impl ToastCancel {
    fn inert() -> Self {
        Self { slot: Arc::new(Mutex::new(None)) }
    }

    fn cancel(&self) {
        if let Ok(mut guard) = self.slot.lock()
            && let Some(sender) = guard.take()
        {
            sender.send(None).ok();
        }
    }
}

impl Drop for ToastCancel {
    fn drop(&mut self) {
        self.cancel();
    }
}

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

    #[cfg_attr(target_os = "macos", allow(dead_code))]
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
    #[cfg_attr(target_os = "macos", allow(dead_code))]
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

#[cfg(windows)]
struct ToastWait {
    tx: futures::channel::oneshot::Sender<Reply>,
    done_tx: Arc<Mutex<Option<mpsc::Sender<Option<Reply>>>>>,
    done_rx: mpsc::Receiver<Option<Reply>>,
    _slot: WaiterSlot,
}

#[cfg(windows)]
fn show_windows_toast(
    title: &str,
    body: &str,
    actions: &[(&str, &str)],
    constant: bool,
    wait: Option<ToastWait>,
) -> windows::core::Result<()> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{
        ToastActivatedEventArgs, ToastDismissedEventArgs, ToastFailedEventArgs, ToastNotification,
        ToastNotificationManager,
    };
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
    use windows::core::{HSTRING, IInspectable, Interface as _};

    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { RoUninitialize() };
            }
        }
    }

    let _com = ComGuard(unsafe { RoInitialize(RO_INIT_MULTITHREADED).is_ok() });
    let doc = XmlDocument::new()?;
    let xml = build_toast_xml(title, body, actions, constant);
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;

    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
        r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe",
    ))?;

    let Some(ToastWait { tx, done_tx, done_rx, _slot }) = wait else {
        // Fire-and-forget (e.g. daily summary or waiter cap reached): show and exit immediately.
        notifier.Show(&toast)?;
        return Ok(());
    };

    let send_once = |slot: &Arc<Mutex<Option<mpsc::Sender<Option<Reply>>>>>, reply: Option<Reply>| {
        if let Ok(mut guard) = slot.lock()
            && let Some(sender) = guard.take()
        {
            sender.send(reply).ok();
        }
    };

    let act_slot = Arc::clone(&done_tx);
    let act_token = toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
        move |_, args| {
            let key = args
                .as_ref()
                .and_then(|insp| insp.cast::<ToastActivatedEventArgs>().ok())
                .and_then(|act| act.Arguments().ok())
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            let reply = if key.is_empty() || key == "default" {
                Some(Reply::Open)
            } else {
                NoticeAction::from_key(&key).map(Reply::Action)
            };
            send_once(&act_slot, reply);
            Ok(())
        },
    ))?;

    let dis_slot = Arc::clone(&done_tx);
    let dis_token = toast.Dismissed(&TypedEventHandler::<ToastNotification, ToastDismissedEventArgs>::new(
        move |_, _| {
            send_once(&dis_slot, None);
            Ok(())
        },
    ))?;

    let fail_slot = Arc::clone(&done_tx);
    let fail_token = toast.Failed(&TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(
        move |_, _| {
            send_once(&fail_slot, None);
            Ok(())
        },
    ))?;

    notifier.Show(&toast)?;

    let deadline = Instant::now() + TOAST_WAIT_TIMEOUT;
    loop {
        if tx.is_canceled() {
            break;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else { break };
        match done_rx.recv_timeout(remaining.min(TOAST_POLL_SLICE)) {
            Ok(Some(reply)) => {
                tx.send(reply).ok();
                break;
            }
            Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
        }
    }
    toast.RemoveActivated(act_token).ok();
    toast.RemoveDismissed(dis_token).ok();
    toast.RemoveFailed(fail_token).ok();
    Ok(())
}

/// Shows a fire-and-forget desktop notification without blocking an OS waiter thread.
fn show_notification_only(title: String, body: String, constant: bool) {
    #[cfg(windows)]
    {
        std::thread::spawn(move || {
            if let Err(err) = show_windows_toast(&title, &body, &[], constant, None) {
                eprintln!("failed to show a reminder: {err:#}");
            }
        });
    }
    #[cfg(not(windows))]
    {
        std::thread::spawn(move || {
            let mut notification = notify_rust::Notification::new();
            notification.appname("Fern").summary(&title).body(&body).icon("alarm-clock");
            if constant {
                notification.timeout(notify_rust::Timeout::Never);
            }
            if let Err(err) = notification.show() {
                eprintln!("failed to show a reminder: {err}");
            }
        });
    }
}

/// Shows a desktop notification with its actions as buttons, and waits on a bounded,
/// cancellable waiter thread for the one clicked, if any.
#[cfg(windows)]
fn show(
    title: String,
    body: String,
    actions: Vec<(&'static str, &'static str)>,
    constant: bool,
) -> (futures::channel::oneshot::Receiver<Reply>, ToastCancel) {
    let (tx, rx) = futures::channel::oneshot::channel();
    let Some(slot) = WaiterSlot::try_acquire() else {
        show_notification_only(title, body, constant);
        return (rx, ToastCancel::inert());
    };
    let (done_tx, done_rx) = mpsc::channel::<Option<Reply>>();
    let done_slot = Arc::new(Mutex::new(Some(done_tx)));
    let cancel = ToastCancel { slot: Arc::clone(&done_slot) };
    std::thread::spawn(move || {
        let wait = ToastWait { tx, done_tx: done_slot, done_rx, _slot: slot };
        if let Err(err) = show_windows_toast(&title, &body, &actions, constant, Some(wait)) {
            eprintln!("failed to show a reminder: {err:#}");
        }
    });
    (rx, cancel)
}

#[cfg(not(windows))]
fn show(
    title: String,
    body: String,
    actions: Vec<(&'static str, &'static str)>,
    constant: bool,
) -> (futures::channel::oneshot::Receiver<Reply>, ToastCancel) {
    let (tx, rx) = futures::channel::oneshot::channel();
    let Some(_slot) = WaiterSlot::try_acquire() else {
        show_notification_only(title, body, constant);
        return (rx, ToastCancel::inert());
    };
    std::thread::spawn(move || {
        let _guard = _slot;
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
                let reply = if key == "default" {
                    Some(Reply::Open)
                } else {
                    NoticeAction::from_key(key).map(Reply::Action)
                };
                if let Some(reply) = reply {
                    tx.send(reply).ok();
                }
            }),
            #[cfg(target_os = "macos")]
            Ok(_) => drop(tx),
            Err(err) => eprintln!("failed to show a reminder: {err}"),
        }
    });
    (rx, ToastCancel::inert())
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
    _cancel: Option<ToastCancel>,
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
}

fn poll_once(store: &Store, last: NaiveDateTime, now: NaiveDateTime) -> PollResult {
    let settings = store.preferences().unwrap_or_default().notifications;
    let daily_titles = collect_daily_titles(store, settings, now);
    let notices = fired(store, last, now);
    set_setting_with_retry(store, CHECKED_AT, &now.format(STAMP_FORMAT).to_string());
    PollResult { settings, daily_titles, notices }
}

fn open_or_reuse_bg_store(bg_store: Option<Store>, db_path: &Path) -> Option<Store> {
    bg_store.or_else(|| Store::open(db_path).ok())
}

/// Starts checking for reminders: collects the ones missed since the last run, then shows new
/// ones as desktop notifications. All periodic SQLite polling runs off the foreground UI thread.
pub fn start(store: Rc<Store>, window: AnyWindowHandle, cx: &mut Context<AppView>) {
    // UI screenshots run the app with demo data; they must not notify on the real desktop.
    if std::env::var_os("FERN_DEV_STATE").is_some() {
        return;
    }
    let db_path = cx.try_global::<crate::DbPath>().map(|p| p.0.clone());
    cx.spawn(async move |this, cx| {
        let now = Local::now().naive_local();
        let mut last = now;
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
                let (returned_store, poll) = cx
                    .background_spawn(async move {
                        let bg = open_or_reuse_bg_store(taken_store, &path);
                        let poll = bg.as_ref().map(|s| poll_once(s, prev, now));
                        (bg, poll)
                    })
                    .await;
                bg_store = returned_store;
                match poll {
                    Some(poll) => poll,
                    None => continue,
                }
            } else {
                poll_once(&store, last, now)
            };
            last = now;

            if this.upgrade().is_none() {
                break;
            }
            if let Some(open_titles) = poll.daily_titles {
                let (title, body) = cx.update(|cx| daily_text(&open_titles, i18n::t(cx)));
                // Fire-and-forget: never blocks an OS thread waiting for a reply.
                show_notification_only(title, body, poll.settings.constant);
            }
            if poll.notices.is_empty() {
                continue;
            }
            // With desktop reminders off, reminders only go to the bell.
            if !poll.settings.desktop {
                if this.update(cx, |this, cx| this.add_notices(poll.notices, cx)).is_err() {
                    break;
                }
                continue;
            }
            type ShownNotice = (Notice, String, Vec<(&'static str, &'static str)>);
            let shown: Vec<ShownNotice> = cx.update(|cx| {
                let strings = i18n::t(cx);
                poll.notices
                    .into_iter()
                    .map(|mut n| {
                        n.constant = poll.settings.constant || n.constant;
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
                let (reply, initial_cancel) = show(notice.title.clone(), due.clone(), buttons.clone(), constant);
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
                            if let Target::Task(id) = notice.target {
                                let still_open = if let Some(path) = repeat_db_path.clone() {
                                    cx.background_spawn(async move {
                                        Store::open(&path)
                                            .ok()
                                            .and_then(|s| s.task(id).ok().flatten())
                                            .is_some_and(|t| !t.is_closed() && !t.is_deleted() && t.snoozed_until.is_none())
                                    })
                                    .await
                                } else {
                                    store
                                        .task(id)
                                        .ok()
                                        .flatten()
                                        .is_some_and(|t| !t.is_closed() && !t.is_deleted() && t.snoozed_until.is_none())
                                };
                                if !still_open {
                                    break;
                                }
                            }
                            let (repeat_reply, repeat_cancel) = show(notice.title.clone(), due.clone(), buttons.clone(), true);
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

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn notice(title: &str, hour: u32) -> Notice {
        let at = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap().and_hms_opt(hour, 0, 0).unwrap();
        Notice { target: Target::Task(uuid::Uuid::now_v7()), title: title.into(), at, constant: false, has_goal: false }
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
        assert!(!notices[0].has_goal);
        assert_eq!(notices[0].actions(), [NoticeAction::Snooze, NoticeAction::CheckIn]);
        store.toggle_checkin(habit.id, day).unwrap();
        assert!(fired(&store, at(9), at(10)).is_empty(), "a habit done today is not reminded");
    }

    #[test]
    fn habit_and_task_reminder_actions_and_snooze() {
        let store = Store::open_in_memory().unwrap();
        let task = store.add_task("water the plants").unwrap();
        assert_eq!(actions(&store, Target::Task(task.id)), NoticeAction::for_task());
        let read = store.add_habit("Read").unwrap();
        assert_eq!(actions(&store, Target::Habit(read.id)), [NoticeAction::Snooze, NoticeAction::CheckIn]);
        let water = store.add_habit("Water").unwrap();
        store.update_habit(water.id, "Water", Some(&fern_core::Goal::new(8.0, "glasses"))).unwrap();
        assert_eq!(
            actions(&store, Target::Habit(water.id)),
            [NoticeAction::Snooze, NoticeAction::Record, NoticeAction::PlusOne]
        );
        for action in actions(&store, Target::Habit(water.id))
            .iter()
            .chain(actions(&store, Target::Task(task.id)))
            .chain(NoticeAction::for_countdown())
        {
            assert_eq!(NoticeAction::from_key(action.key()), Some(*action));
        }

        // A snoozed habit reminder and a snoozed task reminder both fire again when their time comes.
        let at = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap().and_hms_opt(9, 0, 0).unwrap();
        store.snooze_habit(read.id, at + chrono::Duration::minutes(SNOOZE_MINUTES)).unwrap();
        store.snooze_habit(water.id, at + chrono::Duration::minutes(SNOOZE_MINUTES)).unwrap();
        store.snooze_task(task.id, at + chrono::Duration::minutes(5)).unwrap();
        store.set_task_constant_reminder(task.id, true).unwrap();
        let notices = fired(&store, at, at + chrono::Duration::minutes(20));
        assert!(notices.iter().any(|n| n.target == Target::Task(task.id) && n.constant));
        assert!(notices.iter().any(|n| n.target == Target::Habit(read.id) && !n.has_goal));
        let water_notice = notices.iter().find(|n| n.target == Target::Habit(water.id)).unwrap();
        assert!(water_notice.has_goal);
        assert_eq!(water_notice.actions(), [NoticeAction::Snooze, NoticeAction::Record, NoticeAction::PlusOne]);
    }

    #[test]
    fn waiter_slots_are_bounded_and_released_on_drop() {
        let mut held = Vec::new();
        for _ in 0..MAX_TOAST_WAITERS {
            held.push(WaiterSlot::try_acquire().expect("slot should be available"));
        }
        assert!(WaiterSlot::try_acquire().is_none(), "exceeding MAX_TOAST_WAITERS must fail");
        held.pop();
        assert!(WaiterSlot::try_acquire().is_some(), "dropping a slot must free it");
    }

    #[test]
    fn windows_toast_xml_escapes_text_and_adds_reminder_scenario() {
        let normal = build_toast_xml("Call <Bob> & Co", "At \"5\"", &[("snooze-5m", "5m"), ("complete", "Done")], false);
        assert!(normal.contains("Call &lt;Bob&gt; &amp; Co"));
        assert!(normal.contains("At &quot;5&quot;"));
        assert!(!normal.contains("scenario=\"reminder\""));
        assert!(normal.contains(r#"<action content="5m" arguments="snooze-5m" activationType="foreground"/>"#));

        let urgent = build_toast_xml("Pay rent", "Now", &[("complete", "Done")], true);
        assert!(urgent.contains(r#"scenario="reminder""#));
    }
}
