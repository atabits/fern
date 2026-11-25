use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Instant;

use desktop_common::logging::log_error;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastDismissedEventArgs, ToastFailedEventArgs, ToastNotification,
    ToastNotificationManager,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegSetValueExW,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::{HSTRING, IInspectable, Interface as _};

use super::super::{NoticeAction, build_toast_xml};
use super::{
    Reply, TOAST_POLL_SLICE, TOAST_WAIT_TIMEOUT, ToastCancel, WaiterSlot, encode_reg_sz,
};

struct ToastWait {
    tx: futures::channel::oneshot::Sender<Reply>,
    done_tx: Arc<Mutex<Option<mpsc::Sender<Option<Reply>>>>>,
    done_rx: mpsc::Receiver<Option<Reply>>,
    _slot: WaiterSlot,
}

const FERN_TOAST_AUMID: &str = "Fern.Tasks";
const POWERSHELL_TOAST_AUMID: &str =
    r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

fn register_windows_toast_aumid() -> bool {
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

fn ensure_windows_toast_aumid() -> &'static str {
    static REGISTERED_AUMID: OnceLock<&'static str> = OnceLock::new();
    REGISTERED_AUMID.get_or_init(|| {
        if register_windows_toast_aumid() {
            FERN_TOAST_AUMID
        } else {
            POWERSHELL_TOAST_AUMID
        }
    })
}

fn show_windows_toast(
    title: &str,
    body: &str,
    actions: &[(&str, &str)],
    constant: bool,
    wait: Option<ToastWait>,
) -> windows::core::Result<()> {
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

pub(in super::super) fn show_notification_only(title: String, body: String, constant: bool) {
    std::thread::spawn(move || {
        if let Err(err) = show_windows_toast(&title, &body, &[], constant, None) {
            log_error("reminders", format!("failed to show a reminder: {err:#}"));
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
            log_error("reminders", format!("failed to show a reminder: {err:#}"));
        }
    });
    (rx, cancel)
}
