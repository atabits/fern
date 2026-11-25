//! Platform-specific desktop notification backends (Windows WinRT toast, macOS
//! `UNUserNotificationCenter` / `notify_rust`, and freedesktop / `notify_rust` on Unix).

#[cfg(any(not(target_os = "macos"), test))]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::Reply;

#[cfg(windows)]
#[path = "platform/windows.rs"]
mod backend;

#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod backend;

#[cfg(all(unix, not(target_os = "macos")))]
#[path = "platform/linux.rs"]
mod backend;

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

#[cfg(any(windows, test))]
pub(super) fn encode_reg_sz(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(target_os = "macos")]
pub(super) fn init_macos_notifications() {
    backend::init_macos_notifications();
}

/// Shows a fire-and-forget desktop notification without blocking an OS waiter thread.
pub(super) fn show_notification_only(title: String, body: String, constant: bool) {
    backend::show_notification_only(title, body, constant);
}

/// Shows a desktop notification with its actions as buttons, and waits on a bounded,
/// cancellable waiter thread for the one clicked, if any.
pub(super) fn show(
    title: String,
    body: String,
    actions: Vec<(&'static str, &'static str)>,
    constant: bool,
) -> (futures::channel::oneshot::Receiver<Reply>, ToastCancel) {
    backend::show(title, body, actions, constant)
}
