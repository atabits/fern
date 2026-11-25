//! Cross-platform single-instance guard & CLI IPC keyed on the active data directory.
//!
//! - **Windows**: Named mutex (`Local\FernInstance-...`), window handle file (`instance.hwnd`),
//!   `pending_link`, and `PostMessageW` activation.
//! - **Unix (Linux / macOS)**: Unix domain socket (`0o600`) in `$XDG_RUNTIME_DIR` (or `/tmp` /
//!   `<data_dir>`) forwarding activation, `--quick-add`, `--toggle`, `--focus-mini`, and `fern://`
//!   deep links to the primary running instance.

use std::path::Path;

use anyhow::Result;

/// Command forwarded from a secondary CLI invocation to the running instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpcCommand {
    Activate,
    QuickAdd,
    Toggle,
    FocusMini,
    DeepLink(String),
}

impl IpcCommand {
    #[cfg_attr(not(any(unix, test)), allow(dead_code))]
    pub fn encode(&self) -> String {
        match self {
            Self::Activate => "activate\n".to_owned(),
            Self::QuickAdd => "quick-add\n".to_owned(),
            Self::Toggle => "toggle\n".to_owned(),
            Self::FocusMini => "focus-mini\n".to_owned(),
            Self::DeepLink(uri) => {
                let sanitized: String = uri.chars().filter(|&c| c != '\n' && c != '\r').collect();
                format!("link {sanitized}\n")
            }
        }
    }

    #[cfg_attr(not(any(unix, test)), allow(dead_code))]
    pub fn decode(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed == "quick-add" {
            Self::QuickAdd
        } else if trimmed == "toggle" {
            Self::Toggle
        } else if trimmed == "focus-mini" {
            Self::FocusMini
        } else if let Some(uri) = trimmed.strip_prefix("link ") {
            let uri = uri.trim();
            if uri.is_empty() {
                Self::Activate
            } else {
                Self::DeepLink(uri.to_owned())
            }
        } else {
            Self::Activate
        }
    }
}

fn data_dir_hex_hash(data_dir: &Path, byte_len: usize, lowercase: bool) -> String {
    use std::fmt::Write as _;

    use sha2::{Digest as _, Sha256};

    let canonical = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let raw = canonical.to_string_lossy();
    let normalized = if lowercase { raw.to_lowercase() } else { raw.into_owned() };
    let digest = Sha256::digest(normalized.as_bytes());
    let len = byte_len.min(digest.len());
    let mut hex = String::with_capacity(len * 2);
    for byte in &digest[..len] {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

/// Parses a serialized deep link (supporting `fern://` URIs and legacy bare UUID strings).
#[cfg(windows)]
pub fn parse_pending_link(raw: &str) -> Option<fern_core::link::DeepLink> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    fern_core::link::parse_deep_link(trimmed)
        .or_else(|| fern_core::Id::parse_str(trimmed).ok().map(fern_core::link::DeepLink::Task))
}

#[cfg(windows)]
mod platform {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    use anyhow::Result;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, WPARAM,
    };
    use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, GetWindowThreadProcessId, IsWindow, PostMessageW,
        RegisterWindowMessageW,
    };
    use windows::core::{PCWSTR, w};

    use super::{IpcCommand, data_dir_hex_hash, parse_pending_link};

    const INSTANCE_HWND_FILE: &str = "instance.hwnd";
    const PENDING_LINK_FILE: &str = "pending_link";
    const PENDING_COMMAND_FILE: &str = "pending_command";

    struct ActiveLock {
        /// Raw Windows mutex `HANDLE` stored as `usize` so it is `Send`.
        handle: usize,
        data_dir: PathBuf,
    }

    static LOCK: Mutex<Option<ActiveLock>> = Mutex::new(None);

    pub struct InstanceGuard;

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            release();
        }
    }

    pub fn activate_message() -> u32 {
        unsafe { RegisterWindowMessageW(w!("FernActivateInstance")) }
    }

    pub(crate) fn mutex_name(data_dir: &Path) -> String {
        format!("Local\\FernInstance-{}", data_dir_hex_hash(data_dir, 16, true))
    }

    fn write_atomic(data_dir: &Path, filename: &str, bytes: &[u8]) {
        let pid = unsafe { GetCurrentProcessId() };
        let target = data_dir.join(filename);
        let tmp = data_dir.join(format!("{filename}.{pid}.tmp"));
        if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, &target).is_err() {
            let _ = std::fs::write(&target, bytes);
            let _ = std::fs::remove_file(&tmp);
        }
    }

    fn to_wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn acquire(
        data_dir: &Path,
        command: IpcCommand,
    ) -> Result<Option<(InstanceGuard, Option<futures::channel::mpsc::UnboundedReceiver<IpcCommand>>)>>
    {
        if desktop_common::is_dev_state() {
            return Ok(Some((InstanceGuard, None)));
        }
        std::fs::create_dir_all(data_dir).ok();
        let name = to_wide(&mutex_name(data_dir));
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }?;
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_exists {
            let _ = unsafe { CloseHandle(handle) };
            forward_to_running(data_dir, &command);
            return Ok(None);
        }
        let mut slot = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *slot = Some(ActiveLock {
            handle: handle.0 as usize,
            data_dir: data_dir.to_path_buf(),
        });
        Ok(Some((InstanceGuard, None)))
    }

    pub fn register_window(data_dir: &Path, hwnd: HWND) {
        let pid = unsafe { GetCurrentProcessId() };
        let content = format!("{pid} {}", hwnd.0 as usize);
        write_atomic(data_dir, INSTANCE_HWND_FILE, content.as_bytes());
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn take_pending_link(data_dir: &Path) -> Option<fern_core::link::DeepLink> {
        let path = data_dir.join(PENDING_LINK_FILE);
        let text = std::fs::read_to_string(&path).ok()?;
        let _ = std::fs::remove_file(&path);
        parse_pending_link(&text)
    }

    pub fn take_pending_command(data_dir: &Path) -> Option<IpcCommand> {
        let cmd_path = data_dir.join(PENDING_COMMAND_FILE);
        if let Ok(text) = std::fs::read_to_string(&cmd_path) {
            let _ = std::fs::remove_file(&cmd_path);
            return Some(IpcCommand::decode(&text));
        }
        let link_path = data_dir.join(PENDING_LINK_FILE);
        if let Ok(text) = std::fs::read_to_string(&link_path) {
            let _ = std::fs::remove_file(&link_path);
            if let Some(link) = parse_pending_link(&text) {
                let raw = match link {
                    fern_core::link::DeepLink::Task(id) => fern_core::link::task_link(id),
                    _ => text.trim().to_owned(),
                };
                return Some(IpcCommand::DeepLink(raw));
            }
        }
        None
    }

    pub fn release() {
        let active = LOCK.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(active) = active {
            let _ = std::fs::remove_file(active.data_dir.join(INSTANCE_HWND_FILE));
            let _ = unsafe { CloseHandle(HANDLE(active.handle as *mut std::ffi::c_void)) };
        }
    }

    pub(crate) fn write_pending_command(data_dir: &Path, command: &IpcCommand) {
        write_atomic(data_dir, PENDING_COMMAND_FILE, command.encode().as_bytes());
    }

    #[cfg(test)]
    pub(crate) fn write_pending_link(data_dir: &Path, uri: &str) {
        write_atomic(data_dir, PENDING_LINK_FILE, uri.as_bytes());
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

    fn forward_to_running(data_dir: &Path, command: &IpcCommand) {
        if *command != IpcCommand::Activate {
            write_pending_command(data_dir, command);
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
}

#[cfg(unix)]
mod platform {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    use anyhow::{Context as _, Result};
    use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};

    use super::{IpcCommand, data_dir_hex_hash};

    const MAX_UNIX_SOCKET_PATH: usize = 96;
    const MAX_IPC_LINE_BYTES: u64 = 4096;

    static ACTIVE_SOCKET: Mutex<Option<PathBuf>> = Mutex::new(None);

    pub struct InstanceGuard {
        socket_path: Option<PathBuf>,
    }

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            if let Some(path) = self.socket_path.take() {
                let mut guard = ACTIVE_SOCKET.lock().unwrap_or_else(|e| e.into_inner());
                if guard.as_ref() == Some(&path) {
                    *guard = None;
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    fn current_uid(data_dir: &Path) -> u32 {
        std::fs::metadata("/proc/self")
            .or_else(|_| std::fs::metadata(data_dir))
            .map(|m| m.uid())
            .unwrap_or(0)
    }

    pub(crate) fn socket_path_for(
        data_dir: &Path,
        runtime_dir: Option<&Path>,
        temp_dir: &Path,
        uid: u32,
    ) -> PathBuf {
        let hash = data_dir_hex_hash(data_dir, 8, false);
        if let Some(runtime) = runtime_dir.filter(|p| !p.as_os_str().is_empty()) {
            let candidate = runtime.join(format!("fern-{hash}.sock"));
            if candidate.as_os_str().len() < MAX_UNIX_SOCKET_PATH {
                return candidate;
            }
        }
        let direct = data_dir.join("instance.sock");
        if direct.as_os_str().len() < MAX_UNIX_SOCKET_PATH {
            return direct;
        }
        temp_dir.join(format!("fern-u{uid}-{hash}.sock"))
    }

    pub(crate) fn socket_path(data_dir: &Path) -> PathBuf {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        socket_path_for(data_dir, runtime.as_deref(), &std::env::temp_dir(), current_uid(data_dir))
    }

    fn try_forward(socket: &Path, command: &IpcCommand) -> bool {
        let Ok(mut stream) = UnixStream::connect(socket) else {
            return false;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
        if stream.write_all(command.encode().as_bytes()).is_err() {
            return false;
        }
        let _ = stream.flush();
        let mut ack = [0u8; 3];
        let _ = stream.read(&mut ack);
        true
    }

    fn spawn_listener(listener: UnixListener, tx: UnboundedSender<IpcCommand>) {
        std::thread::Builder::new()
            .name("fern-ipc".into())
            .spawn(move || {
                for incoming in listener.incoming() {
                    let Ok(mut stream) = incoming else { break };
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                    let mut line = String::new();
                    let read_ok = {
                        let mut reader = BufReader::new((&stream).take(MAX_IPC_LINE_BYTES));
                        reader.read_line(&mut line).is_ok()
                    };
                    if read_ok {
                        let cmd = IpcCommand::decode(&line);
                        let _ = tx.unbounded_send(cmd);
                        let _ = stream.write_all(b"OK\n");
                    }
                }
            })
            .ok();
    }

    pub fn acquire(
        data_dir: &Path,
        command: IpcCommand,
    ) -> Result<Option<(InstanceGuard, Option<UnboundedReceiver<IpcCommand>>)>> {
        if desktop_common::is_dev_state() {
            return Ok(Some((InstanceGuard { socket_path: None }, None)));
        }
        std::fs::create_dir_all(data_dir).ok();
        let sock = socket_path(data_dir);
        if sock.exists() {
            if try_forward(&sock, &command) {
                return Ok(None);
            }
            // Stale socket from a killed or crashed process.
            let _ = std::fs::remove_file(&sock);
        }
        if let Some(parent) = sock.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let listener = UnixListener::bind(&sock)
            .with_context(|| format!("failed to bind IPC socket {}", sock.display()))?;
        let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));
        let (tx, rx) = mpsc::unbounded();
        *ACTIVE_SOCKET.lock().unwrap_or_else(|e| e.into_inner()) = Some(sock.clone());
        spawn_listener(listener, tx);
        Ok(Some((
            InstanceGuard {
                socket_path: Some(sock),
            },
            Some(rx),
        )))
    }

    pub fn release() {
        if let Some(path) = ACTIVE_SOCKET.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub use platform::{InstanceGuard, release};

#[cfg(windows)]
pub use platform::{activate_message, register_window, take_pending_command};
#[cfg(all(windows, test))]
pub use platform::take_pending_link;

/// Held primary-instance lock together with an optional IPC command stream (Unix).
pub struct AcquiredInstance {
    pub _guard: InstanceGuard,
    receiver: Option<futures::channel::mpsc::UnboundedReceiver<IpcCommand>>,
}

impl AcquiredInstance {
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn take_receiver(&mut self) -> Option<futures::channel::mpsc::UnboundedReceiver<IpcCommand>> {
        self.receiver.take()
    }
}

/// Acquires the single-instance lock for `data_dir`.
///
/// Returns `Ok(Some(AcquiredInstance))` when this process is the primary instance.
/// Returns `Ok(None)` when another instance is running after forwarding `command`.
pub fn acquire(data_dir: &Path, command: IpcCommand) -> Result<Option<AcquiredInstance>> {
    Ok(platform::acquire(data_dir, command)?.map(|(guard, receiver)| AcquiredInstance {
        _guard: guard,
        receiver,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use fern_core::link::DeepLink;

    #[test]
    fn ipc_command_encode_decode_roundtrip() {
        for cmd in [
            IpcCommand::Activate,
            IpcCommand::QuickAdd,
            IpcCommand::Toggle,
            IpcCommand::FocusMini,
            IpcCommand::DeepLink("fern://plugin/pomodoro/start?minutes=25".into()),
        ] {
            assert_eq!(IpcCommand::decode(&cmd.encode()), cmd);
        }
        assert_eq!(IpcCommand::decode("unknown\n"), IpcCommand::Activate);
    }

    #[cfg(windows)]
    #[test]
    fn mutex_name_isolates_distinct_data_dirs_and_is_case_insensitive() {
        let a = platform::mutex_name(Path::new(r"C:\Users\Test\FernDataA"));
        let a_lower = platform::mutex_name(Path::new(r"c:\users\test\ferndataa"));
        let b = platform::mutex_name(Path::new(r"C:\Users\Test\FernDataB"));
        assert!(a.starts_with("Local\\FernInstance-"));
        assert_eq!(a, a_lower);
        assert_ne!(a, b);
    }

    #[cfg(windows)]
    #[test]
    fn pending_link_roundtrip_consumes_file() {
        let dir = std::env::temp_dir().join(format!("fern-single-instance-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let id = uuid::Uuid::now_v7();

        // Backwards-compatible bare UUID format.
        platform::write_pending_link(&dir, &id.to_string());
        assert_eq!(take_pending_link(&dir), Some(DeepLink::Task(id)));
        assert_eq!(take_pending_link(&dir), None);

        // Task deep-link URI.
        platform::write_pending_link(&dir, &fern_core::link::task_link(id));
        assert_eq!(take_pending_link(&dir), Some(DeepLink::Task(id)));
        assert_eq!(take_pending_link(&dir), None);

        // Plugin deep-link URI with query parameters.
        platform::write_pending_link(&dir, "fern://plugin/pomodoro/start?minutes=25&label=Deep");
        assert_eq!(
            take_pending_link(&dir),
            Some(DeepLink::Plugin {
                plugin_id: "pomodoro".into(),
                command: "start".into(),
                params: vec![
                    ("minutes".into(), "25".into()),
                    ("label".into(), "Deep".into()),
                ],
            })
        );
        assert_eq!(take_pending_link(&dir), None);

        // Full IpcCommand forwarding round-trip on Windows (QuickAdd / Toggle / FocusMini / DeepLink).
        for cmd in [
            IpcCommand::QuickAdd,
            IpcCommand::Toggle,
            IpcCommand::FocusMini,
            IpcCommand::DeepLink("fern://plugin/pomodoro/start?minutes=25".into()),
        ] {
            platform::write_pending_command(&dir, &cmd);
            assert_eq!(take_pending_command(&dir), Some(cmd));
            assert_eq!(take_pending_command(&dir), None);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_path_fallback_is_deterministic_per_user() {
        let short_dir = Path::new("/var/lib/fern");
        let long_dir = std::path::PathBuf::from(
            "/var/lib/very/deep/nested/directory/hierarchy/for/fern/profile/data/storage/with/extra/length",
        );
        let tmp = Path::new("/tmp");

        // Short data_dir with no XDG_RUNTIME_DIR uses `<data_dir>/instance.sock`.
        assert_eq!(
            platform::socket_path_for(short_dir, None, tmp, 1000),
            short_dir.join("instance.sock")
        );

        // Long data_dir (>= 96 bytes with instance.sock) falls back to a deterministic per-UID path in temp_dir.
        let fallback_a = platform::socket_path_for(&long_dir, None, tmp, 1000);
        let fallback_b = platform::socket_path_for(&long_dir, None, tmp, 1000);
        let other_user = platform::socket_path_for(&long_dir, None, tmp, 1001);
        assert_eq!(fallback_a, fallback_b);
        assert_ne!(fallback_a, other_user);
        assert!(fallback_a.starts_with(tmp));
        assert!(fallback_a.to_string_lossy().contains("fern-u1000-"));

        // When XDG_RUNTIME_DIR is set, it always wins if short enough.
        let runtime = Path::new("/run/user/1000");
        let preferred = platform::socket_path_for(&long_dir, Some(runtime), tmp, 1000);
        assert!(preferred.starts_with(runtime));
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_ipc_forwards_commands_and_cleans_up_on_drop() {
        let dir = std::env::temp_dir().join(format!("fern-unix-ipc-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();

        let sock = platform::socket_path(&dir);
        assert!(sock.as_os_str().len() < 100);
        // Simulate stale socket leftover.
        std::fs::write(&sock, b"stale").unwrap();

        let mut acquired = acquire(&dir, IpcCommand::Activate)
            .expect("acquire primary")
            .expect("must become primary after cleaning stale socket");
        let mut rx = acquired.take_receiver().expect("ipc receiver present");

        assert!(acquire(&dir, IpcCommand::QuickAdd).unwrap().is_none());
        assert!(acquire(&dir, IpcCommand::Toggle).unwrap().is_none());
        assert!(acquire(&dir, IpcCommand::DeepLink("fern://task/123".into())).unwrap().is_none());

        let mut received = Vec::new();
        for _ in 0..3 {
            received.push(rx.try_recv().expect("queued command"));
        }
        assert_eq!(
            received,
            vec![
                IpcCommand::QuickAdd,
                IpcCommand::Toggle,
                IpcCommand::DeepLink("fern://task/123".into()),
            ]
        );

        drop(acquired);
        assert!(!sock.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
