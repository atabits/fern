use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use block2::{DynBlock, RcBlock};
use desktop_common::logging::log_error;
use dispatch2::{DispatchQueue, DispatchQueueGlobalPriority, DispatchTime, GlobalQueueIdentifier};
use fern_core::Id;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{AnyThread as _, MainThreadMarker, define_class, msg_send};
use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationDefaultActionIdentifier, UNNotificationDismissActionIdentifier,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use super::super::NoticeAction;
use super::{MAX_TOAST_WAITERS, Reply, TOAST_WAIT_TIMEOUT, ToastCancel};

type ActionButtons = Vec<(&'static str, &'static str)>;

struct PendingReply {
    tx: futures::channel::oneshot::Sender<Reply>,
    created_at: Instant,
}

struct UnState {
    // Keep the delegate strongly retained because `UNUserNotificationCenter.delegate` is weak.
    _delegate: Retained<FernNotificationDelegate>,
    pending: Mutex<HashMap<String, PendingReply>>,
    categories: Mutex<HashMap<String, ActionButtons>>,
}

enum Backend {
    Bundled(UnState),
    Unbundled,
}

static BACKEND: OnceLock<Backend> = OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing invariants; FernNotificationDelegate has no ivars or Drop.
    #[unsafe(super(NSObject))]
    struct FernNotificationDelegate;

    unsafe impl NSObjectProtocol for FernNotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for FernNotificationDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present_notification(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions) + 'static>,
        ) {
            let options = UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound;
            completion_handler.call((options,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive_notification_response(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &DynBlock<dyn Fn() + 'static>,
        ) {
            objc2::rc::autoreleasepool(|_| {
                let id = response.notification().request().identifier().to_string();
                if let Some(tx) = remove_pending(&id) {
                    let action_id = response.actionIdentifier();
                    // SAFETY: Framework-exported constant NSStrings from UserNotifications.
                    let (default_id, dismiss_id) = unsafe {
                        (UNNotificationDefaultActionIdentifier, UNNotificationDismissActionIdentifier)
                    };
                    if action_id.isEqualToString(default_id) {
                        let _ = tx.send(Reply::Open);
                    } else if !action_id.isEqualToString(dismiss_id) {
                        let key = action_id.to_string();
                        if let Some(action) = NoticeAction::from_key(&key) {
                            let _ = tx.send(Reply::Action(action));
                        }
                    }
                }
            });
            completion_handler.call(());
        }
    }
);

fn is_app_bundle(bundle: &NSBundle) -> bool {
    let has_id = bundle.bundleIdentifier().is_some();
    let is_app_ext = bundle
        .bundleURL()
        .pathExtension()
        .is_some_and(|ext| ext.to_string().eq_ignore_ascii_case("app"));
    has_id && is_app_ext
}

fn remove_pending(id: &str) -> Option<futures::channel::oneshot::Sender<Reply>> {
    let Some(Backend::Bundled(state)) = BACKEND.get() else {
        return None;
    };
    state
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id)
        .map(|entry| entry.tx)
}

fn ensure_category(
    state: &UnState,
    center: &UNUserNotificationCenter,
    actions: &[(&'static str, &'static str)],
) -> Option<String> {
    if actions.is_empty() {
        return None;
    }
    let mut category_id = String::from("fern.reminder");
    for &(key, _) in actions {
        category_id.push('.');
        category_id.push_str(key);
    }
    let snapshot = {
        let mut map = state.categories.lock().unwrap_or_else(|e| e.into_inner());
        if map.get(&category_id).is_some_and(|existing| existing.as_slice() == actions) {
            None
        } else {
            map.insert(category_id.clone(), actions.to_vec());
            Some(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>())
        }
    };
    if let Some(entries) = snapshot {
        let empty_intents = NSArray::<NSString>::new();
        let ns_categories: Vec<Retained<UNNotificationCategory>> = entries
            .iter()
            .map(|(cat_id, buttons)| {
                let ns_actions: Vec<Retained<UNNotificationAction>> = buttons
                    .iter()
                    .map(|&(key, label)| {
                        let opts = if key == NoticeAction::Record.key() {
                            UNNotificationActionOptions::Foreground
                        } else {
                            UNNotificationActionOptions::empty()
                        };
                        UNNotificationAction::actionWithIdentifier_title_options(
                            &NSString::from_str(key),
                            &NSString::from_str(label),
                            opts,
                        )
                    })
                    .collect();
                let actions_array = NSArray::from_retained_slice(&ns_actions);
                UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                    &NSString::from_str(cat_id),
                    &actions_array,
                    &empty_intents,
                    UNNotificationCategoryOptions::CustomDismissAction,
                )
            })
            .collect();
        let category_set = NSSet::from_retained_slice(&ns_categories);
        center.setNotificationCategories(&category_set);
    }
    Some(category_id)
}

fn schedule_un_notification(
    center: &UNUserNotificationCenter,
    id: &str,
    title: &str,
    body: &str,
    category_id: Option<&str>,
    clear_pending_on_err: bool,
) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    if let Some(cat_id) = category_id {
        content.setCategoryIdentifier(&NSString::from_str(cat_id));
    }
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(id),
        &content,
        None,
    );
    let id_on_err = clear_pending_on_err.then(|| id.to_owned());
    let completion = RcBlock::new(move |err: *mut NSError| {
        if !err.is_null() {
            objc2::rc::autoreleasepool(|_| {
                // SAFETY: UserNotifications passes a valid NSError pointer when non-null.
                let err = unsafe { &*err };
                log_error("reminders", format!("failed to show a reminder: {}", err.localizedDescription()));
            });
            if let Some(pending_id) = &id_on_err {
                remove_pending(pending_id);
            }
        }
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&completion));
}

pub(in super::super) fn init_macos_notifications() {
    let Some(_mtm) = MainThreadMarker::new() else {
        return;
    };
    let _ = BACKEND.get_or_init(|| {
        objc2::rc::autoreleasepool(|_| {
            let bundle = NSBundle::mainBundle();
            if is_app_bundle(&bundle) {
                let delegate: Retained<FernNotificationDelegate> = {
                    let this = FernNotificationDelegate::alloc().set_ivars(());
                    unsafe { msg_send![super(this), init] }
                };
                let center = UNUserNotificationCenter::currentNotificationCenter();
                center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                let auth_block = RcBlock::new(|_granted: Bool, err: *mut NSError| {
                    if !err.is_null() {
                        objc2::rc::autoreleasepool(|_| {
                            // SAFETY: UserNotifications passes a valid NSError pointer when non-null.
                            let err = unsafe { &*err };
                            log_error(
                                "reminders",
                                format!("notification authorization failed: {}", err.localizedDescription()),
                            );
                        });
                    }
                });
                center.requestAuthorizationWithOptions_completionHandler(
                    UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                    &auth_block,
                );
                Backend::Bundled(UnState {
                    _delegate: delegate,
                    pending: Mutex::new(HashMap::new()),
                    categories: Mutex::new(HashMap::new()),
                })
            } else {
                if bundle.bundleIdentifier().is_none() {
                    let _ = notify_rust::set_application("io.github.atabits.fern");
                }
                Backend::Unbundled
            }
        })
    });
}

pub(in super::super) fn show_notification_only(title: String, body: String, constant: bool) {
    if matches!(BACKEND.get(), Some(Backend::Bundled(_))) {
        objc2::rc::autoreleasepool(|_| {
            let center = UNUserNotificationCenter::currentNotificationCenter();
            let id = Id::now_v7().to_string();
            schedule_un_notification(&center, &id, &title, &body, None, false);
        });
        return;
    }
    std::thread::spawn(move || {
        let mut notification = notify_rust::Notification::new();
        notification.appname("Fern").summary(&title).body(&body);
        if constant {
            notification.timeout(notify_rust::Timeout::Never);
        }
        if let Err(err) = notification.show() {
            log_error("reminders", format!("failed to show a reminder: {err}"));
        }
    });
}

pub(in super::super) fn show(
    title: String,
    body: String,
    actions: ActionButtons,
    constant: bool,
) -> (futures::channel::oneshot::Receiver<Reply>, ToastCancel) {
    let (tx, rx) = futures::channel::oneshot::channel();
    let Some(Backend::Bundled(state)) = BACKEND.get() else {
        // Unbundled dev build (e.g. `cargo run`): deliver fire-and-forget without
        // acquiring a WaiterSlot or blocking on `wait_for_action`, so slots and
        // `NSUserNotificationCenter.delegate` can never leak or be clobbered.
        show_notification_only(title, body, constant);
        return (rx, ToastCancel::inert());
    };

    let id = Id::now_v7().to_string();
    let accepted = {
        let mut map = state.pending.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, entry| {
            !entry.tx.is_canceled() && entry.created_at.elapsed() < TOAST_WAIT_TIMEOUT
        });
        if map.len() >= MAX_TOAST_WAITERS {
            false
        } else {
            map.insert(id.clone(), PendingReply { tx, created_at: Instant::now() });
            true
        }
    };
    if !accepted {
        show_notification_only(title, body, constant);
        return (rx, ToastCancel::inert());
    }

    let cancel_id = id.clone();
    let cancel = ToastCancel::new(move || {
        remove_pending(&cancel_id);
        objc2::rc::autoreleasepool(|_| {
            let center = UNUserNotificationCenter::currentNotificationCenter();
            let ids = NSArray::from_retained_slice(&[NSString::from_str(&cancel_id)]);
            center.removePendingNotificationRequestsWithIdentifiers(&ids);
            center.removeDeliveredNotificationsWithIdentifiers(&ids);
        });
    });

    if let Ok(when) = DispatchTime::try_from(TOAST_WAIT_TIMEOUT) {
        let timeout_id = id.clone();
        let _ = DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(
            DispatchQueueGlobalPriority::Background,
        ))
        .after(when, move || {
            remove_pending(&timeout_id);
        });
    }

    objc2::rc::autoreleasepool(|_| {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let category_id = ensure_category(state, &center, &actions);
        schedule_un_notification(&center, &id, &title, &body, category_id.as_deref(), true);
    });

    (rx, cancel)
}
