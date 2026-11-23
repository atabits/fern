//! Platform-specific desktop notification backends (Windows WinRT toast, macOS
//! `UNUserNotificationCenter` / `notify_rust`, and freedesktop / `notify_rust` on Unix).

#[cfg(any(not(target_os = "macos"), test))]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(windows)]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

#[cfg(not(target_os = "macos"))]
use super::NoticeAction;
use super::Reply;

/// Maximum number of concurrent interactive toast waiters.
pub(super) const MAX_TOAST_WAITERS: usize = 8;
/// Maximum time an interactive toast waiter stays alive waiting for a click.
const TOAST_WAIT_TIMEOUT: Duration = Duration::from_secs(600);
/// Wake interval inside the waiter loop to detect dropped receivers without blocking 600s.
#[cfg(windows)]
const TOAST_POLL_SLICE: Duration = Duration::from_secs(5);

#[cfg(any(not(target_os = "macos"), test))]
static ACTIVE_WAITERS: AtomicUsize = AtomicUsize::new(0);

#[cfg(any(not(target_os = "macos"), test))]
pub(super) struct WaiterSlot;

#[cfg(any(not(target_os = "macos"), test))]
impl WaiterSlot {
    pub(super) fn try_acquire() -> Option<Self> {
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

#[cfg(any(not(target_os = "macos"), test))]
impl Drop for WaiterSlot {
    fn drop(&mut self) {
        ACTIVE_WAITERS.fetch_sub(1, Ordering::AcqRel);
    }
}

type CancelFn = Box<dyn FnOnce() + Send>;

/// Dropping this handle cancels the pending toast waiter immediately.
pub(super) struct ToastCancel {
    on_cancel: Arc<Mutex<Option<CancelFn>>>,
}

impl ToastCancel {
    pub(super) fn inert() -> Self {
        Self { on_cancel: Arc::new(Mutex::new(None)) }
    }

    pub(super) fn new(f: impl FnOnce() + Send + 'static) -> Self {
        Self { on_cancel: Arc::new(Mutex::new(Some(Box::new(f)))) }
    }

    fn cancel(&self) {
        if let Ok(mut guard) = self.on_cancel.lock()
            && let Some(callback) = guard.take()
        {
            callback();
        }
    }
}

impl Drop for ToastCancel {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(windows)]
struct ToastWait {
    tx: futures::channel::oneshot::Sender<Reply>,
    done_tx: Arc<Mutex<Option<mpsc::Sender<Option<Reply>>>>>,
    done_rx: mpsc::Receiver<Option<Reply>>,
    _slot: WaiterSlot,
}

#[cfg(windows)]
const FERN_TOAST_AUMID: &str = "Fern.Tasks";
#[cfg(windows)]
const POWERSHELL_TOAST_AUMID: &str =
    r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

#[cfg(any(windows, test))]
pub(super) fn encode_reg_sz(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
fn register_windows_toast_aumid() -> bool {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };
    use windows::core::HSTRING;

    struct RegKeyGuard(HKEY);
    impl Drop for RegKeyGuard {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe {
                    let _ = RegCloseKey(self.0);
                }
            }
        }
    }

    let subkey = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{FERN_TOAST_AUMID}"));
    let mut key = HKEY::default();
    let create_err = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &subkey,
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if create_err != ERROR_SUCCESS {
        return false;
    }
    let _guard = RegKeyGuard(key);
    let value_name = HSTRING::from("DisplayName");
    let data = encode_reg_sz("Fern");
    let set_err = unsafe { RegSetValueExW(key, &value_name, None, REG_SZ, Some(&data)) };
    set_err == ERROR_SUCCESS
}

#[cfg(windows)]
fn ensure_windows_toast_aumid() -> &'static str {
    static REGISTERED_AUMID: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    REGISTERED_AUMID.get_or_init(|| {
        if register_windows_toast_aumid() {
            FERN_TOAST_AUMID
        } else {
            POWERSHELL_TOAST_AUMID
        }
    })
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
    let xml = super::build_toast_xml(title, body, actions, constant);
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;

    let aumid = ensure_windows_toast_aumid();
    let notifier = match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(aumid)) {
        Ok(notifier) => notifier,
        Err(_) if aumid != POWERSHELL_TOAST_AUMID => {
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
                POWERSHELL_TOAST_AUMID,
            ))?
        }
        Err(err) => return Err(err),
    };

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

#[cfg(target_os = "macos")]
mod macos_notifications {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    use block2::{DynBlock, RcBlock};
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
                    eprintln!("failed to show a reminder: {}", err.localizedDescription());
                });
                if let Some(pending_id) = &id_on_err {
                    remove_pending(pending_id);
                }
            }
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&completion));
    }

    pub(super) fn init() {
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
                                eprintln!(
                                    "notification authorization failed: {}",
                                    err.localizedDescription()
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

    pub(super) fn show_only(title: String, body: String, constant: bool) {
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
                eprintln!("failed to show a reminder: {err}");
            }
        });
    }

    pub(super) fn show(
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
            show_only(title, body, constant);
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
            show_only(title, body, constant);
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
}

#[cfg(target_os = "macos")]
pub(super) fn init_macos_notifications() {
    macos_notifications::init();
}

/// Shows a fire-and-forget desktop notification without blocking an OS waiter thread.
pub(super) fn show_notification_only(title: String, body: String, constant: bool) {
    #[cfg(windows)]
    {
        std::thread::spawn(move || {
            if let Err(err) = show_windows_toast(&title, &body, &[], constant, None) {
                eprintln!("failed to show a reminder: {err:#}");
            }
        });
    }
    #[cfg(target_os = "macos")]
    {
        macos_notifications::show_only(title, body, constant);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
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
pub(super) fn show(
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
    let cancel_slot = Arc::clone(&done_slot);
    let cancel = ToastCancel::new(move || {
        if let Ok(mut guard) = cancel_slot.lock()
            && let Some(sender) = guard.take()
        {
            sender.send(None).ok();
        }
    });
    std::thread::spawn(move || {
        let wait = ToastWait { tx, done_tx: done_slot, done_rx, _slot: slot };
        if let Err(err) = show_windows_toast(&title, &body, &actions, constant, Some(wait)) {
            eprintln!("failed to show a reminder: {err:#}");
        }
    });
    (rx, cancel)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn show(
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
        futures::executor::block_on(async move {
            let handle = match notification.show_async().await {
                Ok(handle) => handle,
                Err(err) => {
                    eprintln!("failed to show a reminder: {err}");
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

#[cfg(target_os = "macos")]
pub(super) fn show(
    title: String,
    body: String,
    actions: Vec<(&'static str, &'static str)>,
    constant: bool,
) -> (futures::channel::oneshot::Receiver<Reply>, ToastCancel) {
    macos_notifications::show(title, body, actions, constant)
}
