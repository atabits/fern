use std::sync::{Arc, Mutex};

use desktop_common::logging::log_error;

use super::super::NoticeAction;
use super::{Reply, TOAST_WAIT_TIMEOUT, ToastCancel, WaiterSlot};

fn build_linux_notification(title: &str, body: &str, constant: bool) -> notify_rust::Notification {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname("Fern")
        .summary(title)
        .body(body)
        .icon(&desktop_common::desktop_icon_name())
        .hint(notify_rust::Hint::DesktopEntry(desktop_common::app_id()))
        .hint(notify_rust::Hint::Category("reminder".into()))
        .hint(notify_rust::Hint::Urgency(if constant {
            notify_rust::Urgency::Critical
        } else {
            notify_rust::Urgency::Normal
        }));
    if constant {
        notification
            .hint(notify_rust::Hint::Resident(true))
            .timeout(notify_rust::Timeout::Never);
    }
    notification
}

pub(in super::super) fn show_notification_only(title: String, body: String, constant: bool) {
    std::thread::spawn(move || {
        let notification = build_linux_notification(&title, &body, constant);
        if let Err(err) = notification.show() {
            log_error("reminders", format!("failed to show a reminder: {err}"));
        }
    });
}

pub(in super::super) fn show(
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
    let (cancel_tx, cancel_rx) = futures::channel::oneshot::channel::<()>();
    let cancel = ToastCancel::new(move || {
        let _ = cancel_tx.send(());
    });
    std::thread::spawn(move || {
        let _guard = slot;
        let mut notification = build_linux_notification(&title, &body, constant);
        for (key, label) in &actions {
            notification.action(key, label);
        }
        notification.action("default", &title);
        futures::executor::block_on(async move {
            let handle = match notification.show_async().await {
                Ok(handle) => handle,
                Err(err) => {
                    log_error("reminders", format!("failed to show a reminder: {err}"));
                    return;
                }
            };
            let reply_slot = Arc::new(Mutex::new(None::<Reply>));
            let reply_cb = Arc::clone(&reply_slot);
            enum Outcome {
                Responded,
                Canceled,
                TimedOut,
            }
            let outcome = {
                let wait_fut = handle.wait_for_action_async(move |response| {
                    let reply = match response {
                        notify_rust::NotificationResponse::Default => Some(Reply::Open),
                        notify_rust::NotificationResponse::Action(key) => {
                            NoticeAction::from_key(key).map(Reply::Action)
                        }
                        _ => None,
                    };
                    if let Ok(mut guard) = reply_cb.lock() {
                        *guard = reply;
                    }
                });
                let timeout = async_io::Timer::after(TOAST_WAIT_TIMEOUT);
                futures::pin_mut!(wait_fut);
                futures::pin_mut!(cancel_rx);
                futures::pin_mut!(timeout);
                let stop_fut = futures::future::select(cancel_rx, timeout);
                match futures::future::select(wait_fut, stop_fut).await {
                    futures::future::Either::Left(_) => Outcome::Responded,
                    futures::future::Either::Right((futures::future::Either::Left(_), _)) => Outcome::Canceled,
                    futures::future::Either::Right((futures::future::Either::Right(_), _)) => Outcome::TimedOut,
                }
            };
            match outcome {
                Outcome::Canceled => {
                    handle.close_async().await;
                }
                Outcome::TimedOut => {}
                Outcome::Responded => {
                    if let Ok(mut guard) = reply_slot.lock()
                        && let Some(reply) = guard.take()
                    {
                        tx.send(reply).ok();
                    }
                }
            }
        });
    });
    (rx, cancel)
}
