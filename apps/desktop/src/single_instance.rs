//! Windows single-instance guard keyed on the active data directory.
//!
//! Launching `fern.exe` or a `fern://task/<id>` deep link while an instance is already running
//! for the same data directory forwards the link (if any) and activates the existing window
//! instead of spawning duplicate reminder/sync loops and tray icons.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use fern_core::Id;
use sha2::{Digest as _, Sha256};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, WPARAM,
};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, GetWindowThreadProcessId, IsWindow, PostMessageW,
    RegisterWindowMessageW,
};
use windows::core::{PCWSTR, w};

const INSTANCE_HWND_FILE: &str = "instance.hwnd";
const PENDING_LINK_FILE: &str = "pending_link";

struct ActiveLock {
    /// Raw Windows mutex `HANDLE` stored as `usize` so it is `Send`.
    handle: usize,
    data_dir: PathBuf,
}

static LOCK: Mutex<Option<ActiveLock>> = Mutex::new(None);

/// Guard held by `main()` that releases the single-instance lock on process exit.
pub struct InstanceGuard;

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        release();
    }
}

/// Returns the custom Win32 window message ID used to wake the running instance.
pub fn activate_message() -> u32 {
    unsafe { RegisterWindowMessageW(w!("FernActivateInstance")) }
}

/// Computes a deterministic Windows named-mutex name scoped to `data_dir`.
fn mutex_name(data_dir: &Path) -> String {
    let canonical = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let normalized = canonical.to_string_lossy().to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    let mut hex = String::with_capacity(32);
    for byte in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    format!("Local\\FernInstance-{hex}")
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Acquires the single-instance mutex for `data_dir`.
///
/// Returns `Ok(Some(InstanceGuard))` when this process is the primary instance.
/// Returns `Ok(None)` when another instance is already running for `data_dir` after forwarding
/// `link` (if any) and asking the running instance to activate.
pub fn acquire(data_dir: &Path, link: Option<Id>) -> Result<Option<InstanceGuard>> {
    if std::env::var_os("FERN_DEV_STATE").is_some() {
        return Ok(Some(InstanceGuard));
    }
    std::fs::create_dir_all(data_dir).ok();
    let name = to_wide(&mutex_name(data_dir));
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }?;
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        let _ = unsafe { CloseHandle(handle) };
        forward_to_running(data_dir, link);
        return Ok(None);
    }
    let mut slot = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    *slot = Some(ActiveLock { handle: handle.0 as usize, data_dir: data_dir.to_path_buf() });
    Ok(Some(InstanceGuard))
}

/// Records the main window handle so a subsequent launch can activate it.
pub fn register_window(data_dir: &Path, hwnd: HWND) {
    let pid = unsafe { GetCurrentProcessId() };
    let content = format!("{pid} {}", hwnd.0 as usize);
    let target = data_dir.join(INSTANCE_HWND_FILE);
    let tmp = data_dir.join(format!("{INSTANCE_HWND_FILE}.{pid}.tmp"));
    if std::fs::write(&tmp, content.as_bytes()).is_ok()
        && std::fs::rename(&tmp, &target).is_err()
    {
        let _ = std::fs::write(&target, content.as_bytes());
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Reads and consumes any `fern://task/<id>` link forwarded by a secondary launch.
pub fn take_pending_link(data_dir: &Path) -> Option<Id> {
    let path = data_dir.join(PENDING_LINK_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    Id::parse_str(text.trim()).ok()
}

/// Releases the single-instance mutex and removes the recorded window handle.
/// Called on clean exit and right before `updater` spawns the updated executable.
pub fn release() {
    let active = LOCK.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(active) = active {
        let _ = std::fs::remove_file(active.data_dir.join(INSTANCE_HWND_FILE));
        let _ = unsafe { CloseHandle(HANDLE(active.handle as *mut std::ffi::c_void)) };
    }
}

fn write_pending_link(data_dir: &Path, id: Id) {
    let pid = unsafe { GetCurrentProcessId() };
    let target = data_dir.join(PENDING_LINK_FILE);
    let tmp = data_dir.join(format!("{PENDING_LINK_FILE}.{pid}.tmp"));
    let text = id.to_string();
    if std::fs::write(&tmp, text.as_bytes()).is_ok()
        && std::fs::rename(&tmp, &target).is_err()
    {
        let _ = std::fs::write(&target, text.as_bytes());
        let _ = std::fs::remove_file(&tmp);
    }
}

fn read_running_window(data_dir: &Path) -> Option<(u32, HWND)> {
    let text = std::fs::read_to_string(data_dir.join(INSTANCE_HWND_FILE)).ok()?;
    let mut parts = text.split_whitespace();
    let pid: u32 = parts.next()?.parse().ok()?;
    let raw: usize = parts.next()?.parse().ok()?;
    if raw == 0 {
        return None;
    }
    let hwnd = HWND(raw as *mut std::ffi::c_void);
    if !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return None;
    }
    let mut owner_pid = 0u32;
    let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner_pid)) };
    if owner_pid != pid {
        return None;
    }
    Some((pid, hwnd))
}

fn forward_to_running(data_dir: &Path, link: Option<Id>) {
    if let Some(id) = link {
        write_pending_link(data_dir, id);
    }
    let msg = activate_message();
    // Retry briefly in case the primary instance is still creating its main window.
    for _ in 0..20 {
        if let Some((pid, hwnd)) = read_running_window(data_dir) {
            let _ = unsafe { AllowSetForegroundWindow(pid) };
            if msg != 0 {
                let _ = unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0)) };
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutex_name_isolates_distinct_data_dirs_and_is_case_insensitive() {
        let a = mutex_name(Path::new(r"C:\Users\Test\FernDataA"));
        let a_lower = mutex_name(Path::new(r"c:\users\test\ferndataa"));
        let b = mutex_name(Path::new(r"C:\Users\Test\FernDataB"));
        assert!(a.starts_with("Local\\FernInstance-"));
        assert_eq!(a, a_lower);
        assert_ne!(a, b);
    }

    #[test]
    fn pending_link_roundtrip_consumes_file() {
        let dir = std::env::temp_dir().join(format!("fern-single-instance-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let id = uuid::Uuid::now_v7();
        write_pending_link(&dir, id);
        assert_eq!(take_pending_link(&dir), Some(id));
        assert_eq!(take_pending_link(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
