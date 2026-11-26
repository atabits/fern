//! OS-protected encrypted persistence for the active Fern Cloud session (`cloud-session.vault`).
//!
//! Secrets (access tokens, refresh tokens, and the unlocked 256-bit vault DEK) are strictly
//! stored outside `tasks.db` so database backups, exports, and diagnostics never expose credentials.
//! On Windows the vault master key is additionally protected by CurrentUser DPAPI (`CryptProtectData`)
//! and an owner-only DACL; on Unix both files are created atomically with mode `0600`.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use fern_core::cloud::CloudSessionState;
use zeroize::Zeroizing;

const VAULT_MAGIC: &[u8; 8] = b"FERNCLD\x01";
const DPAPI_KEY_MAGIC: &[u8; 8] = b"FERNCDP\x01";
const VAULT_AAD: &[u8] = b"fern-cloud-session-v1";
const KEYCHAIN_URL: &str = "https://fern.atabits.com/cloud-vault-v1";
const KEYCHAIN_USER: &str = "fern-cloud-vault";
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Process-wide OS keychain master key populated at startup via GPUI's native platform credentials
/// (Windows Credential Manager, macOS Keychain Services, Linux `oo7` Secret Service).
static OS_KEYCHAIN_KEY: RwLock<Option<Zeroizing<[u8; KEY_LEN]>>> = RwLock::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BootstrapPhase {
    NotStarted,
    Pending,
    Ready,
}

static KEYCHAIN_GATE: LazyLock<(Mutex<BootstrapPhase>, Condvar)> =
    LazyLock::new(|| (Mutex::new(BootstrapPhase::NotStarted), Condvar::new()));

const KEYCHAIN_BOOTSTRAP_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

struct BootstrapReadyGuard;
impl Drop for BootstrapReadyGuard {
    fn drop(&mut self) {
        mark_keychain_ready();
    }
}

fn mark_keychain_pending() {
    let (lock, _) = &*KEYCHAIN_GATE;
    if let Ok(mut state) = lock.lock() {
        *state = BootstrapPhase::Pending;
    }
}

fn mark_keychain_ready() {
    let (lock, cv) = &*KEYCHAIN_GATE;
    if let Ok(mut state) = lock.lock() {
        *state = BootstrapPhase::Ready;
    }
    cv.notify_all();
}

fn wait_gate_ready(gate: &(Mutex<BootstrapPhase>, Condvar), timeout: Duration) -> bool {
    let (lock, cv) = gate;
    let Ok(mut state) = lock.lock() else {
        return true;
    };
    if *state != BootstrapPhase::Pending {
        return true;
    }
    let deadline = Instant::now() + timeout;
    while *state == BootstrapPhase::Pending {
        let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        let Ok((next, wait_res)) = cv.wait_timeout(state, rem) else {
            return false;
        };
        state = next;
        if wait_res.timed_out() {
            return *state != BootstrapPhase::Pending;
        }
    }
    true
}

fn wait_for_keychain_ready(timeout: Duration) {
    if !use_os_keychain() {
        return;
    }
    let _ = wait_gate_ready(&KEYCHAIN_GATE, timeout);
}

fn use_os_keychain() -> bool {
    if crate::is_dev_state()
        || crate::portable::data_dir().is_some()
        || std::env::var_os("FERN_DATA_DIR")
            .filter(|v| !v.is_empty())
            .is_some()
    {
        return false;
    }
    // Mirror Zed's `ZED_DEVELOPMENT_USE_KEYCHAIN`: skip OS keychain in unsigned debug builds
    // unless explicitly opted in so macOS doesn't spam keychain prompts on every recompile.
    if cfg!(debug_assertions)
        && !std::env::var("FERN_DEV_USE_KEYCHAIN").is_ok_and(|v| !v.is_empty() && v != "0")
    {
        return false;
    }
    true
}

fn cached_keychain_key() -> Option<Zeroizing<[u8; KEY_LEN]>> {
    if !use_os_keychain() {
        return None;
    }
    OS_KEYCHAIN_KEY
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(|k| Zeroizing::new(**k)))
}

fn cache_keychain_key(key: [u8; KEY_LEN]) {
    if let Ok(mut guard) = OS_KEYCHAIN_KEY.write() {
        *guard = Some(Zeroizing::new(key));
    }
}

/// Ensures any existing vault file on disk can be decrypted with `keychain_key` (re-encrypting it
/// from `key_path` if needed) before securely wiping the legacy plaintext `.key` file on Unix.
#[cfg(not(windows))]
fn migrate_vault_and_wipe_legacy_key_file(
    keychain_key: &[u8; KEY_LEN],
    vault_path: Option<&Path>,
    key_path: Option<&Path>,
) {
    let Some(kp) = key_path.filter(|p| p.exists()) else {
        return;
    };
    if let Some(vp) = vault_path.filter(|p| p.exists()) {
        let Ok(blob) = fs::read(vp) else {
            return;
        };
        if decrypt_session(keychain_key, &blob).is_err() {
            // Vault was encrypted with the legacy file key; re-encrypt under the keychain key first!
            let Ok(file_key) = read_key_file(kp) else {
                return;
            };
            let Ok(session) = decrypt_session(&file_key, &blob) else {
                return;
            };
            let Ok(reencrypted) = encrypt_session(keychain_key, &session) else {
                return;
            };
            if write_atomic_restricted(vp, &reencrypted).is_err() {
                return;
            }
        }
    }
    let _ = clear_at(kp);
}

/// Initializes cross-platform OS keychain storage for the 32-byte Fern Cloud vault master key
/// using GPUI's native `Platform::read_credentials` / `write_credentials` (matching Zed).
///
/// Marks the keychain readiness gate `Pending` synchronously before spawning so background sync
/// and SSE threads wait cleanly for bootstrap completion instead of racing on cold start.
pub fn bootstrap_os_keychain(cx: &mut gpui::App) {
    if !use_os_keychain() {
        mark_keychain_ready();
        return;
    }
    mark_keychain_pending();
    let read_task = cx.read_credentials(KEYCHAIN_URL);
    let vault_path = crate::paths::cloud_session_path().ok();
    let key_path = crate::paths::cloud_session_key_path().ok();
    cx.spawn(async move |cx| {
        let _ready = BootstrapReadyGuard;
        match read_task.await {
            Ok(Some((_, bytes))) if bytes.len() == KEY_LEN => {
                let mut key = [0u8; KEY_LEN];
                key.copy_from_slice(&bytes);
                cache_keychain_key(key);
                #[cfg(not(windows))]
                migrate_vault_and_wipe_legacy_key_file(
                    &key,
                    vault_path.as_deref(),
                    key_path.as_deref(),
                );
                #[cfg(windows)]
                let _ = &vault_path;
                return;
            }
            Ok(_) => {}
            Err(err) => {
                crate::diagnostics::log_event(
                    "cloud",
                    format!("OS keychain unavailable, using restricted local key file: {err:#}"),
                );
                return;
            }
        }

        // Either migrate existing file key or mint a fresh 32-byte CSPRNG key into OS keychain.
        let migrated_from_file = key_path
            .as_deref()
            .filter(|p| p.exists())
            .and_then(|p| read_key_file(p).ok());
        let key: Zeroizing<[u8; KEY_LEN]> = match migrated_from_file {
            Some(existing) => existing,
            None => {
                let mut fresh = Zeroizing::new([0u8; KEY_LEN]);
                if getrandom::getrandom(&mut *fresh).is_err() {
                    return;
                }
                fresh
            }
        };

        let write_task = cx.update(|cx| cx.write_credentials(KEYCHAIN_URL, KEYCHAIN_USER, &*key));
        match write_task.await {
            Ok(()) => {
                cache_keychain_key(*key);
                #[cfg(not(windows))]
                migrate_vault_and_wipe_legacy_key_file(
                    &key,
                    vault_path.as_deref(),
                    key_path.as_deref(),
                );
                #[cfg(windows)]
                let _ = &vault_path;
            }
            Err(err) => {
                crate::diagnostics::log_event(
                    "cloud",
                    format!("OS keychain write failed, keeping local key file: {err:#}"),
                );
            }
        }
    })
    .detach();
}

/// Loads the active Fern Cloud session from the default app data directory, if present.
pub fn load_default() -> Result<Option<CloudSessionState>> {
    wait_for_keychain_ready(KEYCHAIN_BOOTSTRAP_WAIT_TIMEOUT);
    let vault = crate::paths::cloud_session_path()?;
    if !vault.exists() {
        return Ok(None);
    }
    let key_path = crate::paths::cloud_session_key_path()?;
    if let Some(key) = cached_keychain_key() {
        let blob = fs::read(&vault)
            .with_context(|| format!("cannot read cloud session vault {}", vault.display()))?;
        if let Ok(session) = decrypt_session(&key, &blob) {
            return Ok(Some(session));
        }
        // Fallback if the vault was written before OS keychain bootstrap completed.
        if key_path.exists() {
            let file_key = read_key_file(&key_path)?;
            let session = decrypt_session(&file_key, &blob)?;
            // Re-encrypt transparently under the OS keychain key on read.
            if let Ok(reencrypted) = encrypt_session(&key, &session) {
                let _ = write_atomic_restricted(&vault, &reencrypted);
            }
            return Ok(Some(session));
        }
        bail!("failed to decrypt cloud session vault with OS keychain key");
    }
    load_from(&vault, &key_path)
}

/// Persists `session` to the encrypted OS-protected vault in the default app data directory.
pub fn save_default(session: &CloudSessionState) -> Result<()> {
    wait_for_keychain_ready(KEYCHAIN_BOOTSTRAP_WAIT_TIMEOUT);
    let vault = crate::paths::cloud_session_path()?;
    if let Some(key) = cached_keychain_key() {
        let blob = encrypt_session(&key, session)?;
        return write_atomic_restricted(&vault, &blob);
    }
    let key_path = crate::paths::cloud_session_key_path()?;
    save_to(&vault, &key_path, session)
}

/// Securely overwrites and deletes the Fern Cloud session vault in the default app data directory.
pub fn clear_default() -> Result<()> {
    let vault = crate::paths::cloud_session_path()?;
    clear_at(&vault)
}

/// Loads and decrypts `CloudSessionState` from `vault_path` using `key_path`.
pub fn load_from(vault_path: &Path, key_path: &Path) -> Result<Option<CloudSessionState>> {
    if !vault_path.exists() {
        return Ok(None);
    }
    let key = read_key_file(key_path)?;
    let blob = fs::read(vault_path)
        .with_context(|| format!("cannot read cloud session vault {}", vault_path.display()))?;
    let session = decrypt_session(&key, &blob)?;
    Ok(Some(session))
}

/// Encrypts and atomically writes `session` to `vault_path` using `key_path`.
pub fn save_to(vault_path: &Path, key_path: &Path, session: &CloudSessionState) -> Result<()> {
    let key = ensure_key_file(key_path)?;
    let blob = encrypt_session(&key, session)?;
    write_atomic_restricted(vault_path, &blob)
}

/// Overwrites `vault_path` with zeroes before removing it (best-effort wipe on sign-out).
pub fn clear_at(vault_path: &Path) -> Result<()> {
    if !vault_path.exists() {
        return Ok(());
    }
    if let Ok(meta) = fs::metadata(vault_path)
        && let Ok(mut file) = OpenOptions::new().write(true).open(vault_path)
    {
        let zeros = vec![0u8; (meta.len() as usize).min(1024 * 1024)];
        let _ = file.write_all(&zeros);
        let _ = file.sync_all();
    }
    match fs::remove_file(vault_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn ensure_key_file(key_path: &Path) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    if key_path.exists() {
        return read_key_file(key_path);
    }
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    getrandom::getrandom(&mut *key)
        .map_err(|err| anyhow!("failed to generate cloud vault key: {err}"))?;
    let disk_bytes = protect_key_for_disk(&key)?;
    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = unique_temp_path(key_path, "cloud-key");
    create_exclusive_file_with_bytes(&tmp, &disk_bytes)?;
    match fs::hard_link(&tmp, key_path) {
        Ok(()) => {
            let _ = fs::remove_file(&tmp);
            Ok(key)
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&tmp);
            read_key_file(key_path)
        }
        Err(_) => {
            let _ = fs::remove_file(&tmp);
            match create_exclusive_file_with_bytes(key_path, &disk_bytes) {
                Ok(()) => Ok(key),
                Err(_) if key_path.exists() => read_key_file(key_path),
                Err(err) => Err(err),
            }
        }
    }
}

fn protect_key_for_disk(key: &[u8; KEY_LEN]) -> Result<Zeroizing<Vec<u8>>> {
    #[cfg(windows)]
    {
        let protected = windows_sec::dpapi_protect(key)?;
        let mut out = Vec::with_capacity(DPAPI_KEY_MAGIC.len() + protected.len());
        out.extend_from_slice(DPAPI_KEY_MAGIC);
        out.extend_from_slice(&protected);
        Ok(Zeroizing::new(out))
    }
    #[cfg(not(windows))]
    {
        Ok(Zeroizing::new(key.to_vec()))
    }
}

fn read_key_file(path: &Path) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let bytes = Zeroizing::new(
        fs::read(path).with_context(|| format!("cannot read cloud vault key {}", path.display()))?,
    );
    if let Some(dpapi_payload) = bytes.strip_prefix(DPAPI_KEY_MAGIC) {
        #[cfg(windows)]
        {
            let plain = windows_sec::dpapi_unprotect(dpapi_payload)?;
            ensure!(plain.len() == KEY_LEN, "invalid DPAPI cloud vault key length");
            let mut key = Zeroizing::new([0u8; KEY_LEN]);
            key.copy_from_slice(&plain);
            return Ok(key);
        }
        #[cfg(not(windows))]
        {
            let _ = dpapi_payload;
            bail!("cloud vault key is protected with Windows DPAPI and cannot be read on this OS");
        }
    }
    ensure!(bytes.len() == KEY_LEN, "invalid cloud vault key length");
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    key.copy_from_slice(&bytes);
    Ok(key)
}

fn encrypt_session(key: &[u8; KEY_LEN], session: &CloudSessionState) -> Result<Vec<u8>> {
    let plaintext = Zeroizing::new(serde_json::to_vec(session)?);
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce)
        .map_err(|err| anyhow!("failed to generate cloud vault nonce: {err}"))?;
    let cipher = ChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext.as_slice(),
                aad: VAULT_AAD,
            },
        )
        .map_err(|_| anyhow!("cloud session vault encryption failed"))?;
    let mut out = Vec::with_capacity(VAULT_MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(VAULT_MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt_session(key: &[u8; KEY_LEN], data: &[u8]) -> Result<CloudSessionState> {
    ensure!(data.starts_with(VAULT_MAGIC), "invalid cloud session vault header");
    let header_len = VAULT_MAGIC.len() + NONCE_LEN;
    ensure!(data.len() >= header_len + 16, "truncated cloud session vault");
    let nonce = &data[VAULT_MAGIC.len()..header_len];
    let ciphertext = &data[header_len..];
    let cipher = ChaCha20Poly1305::new(key.into());
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: VAULT_AAD,
                },
            )
            .map_err(|_| anyhow!("failed to decrypt cloud session vault"))?,
    );
    let session: CloudSessionState = serde_json::from_slice(&plaintext)
        .context("decrypted cloud session vault is not valid JSON")?;
    Ok(session)
}

fn write_atomic_restricted(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = unique_temp_path(path, "cloud-vault");
    create_exclusive_file_with_bytes(&tmp, bytes)?;
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(())
}

fn unique_temp_path(path: &Path, fallback: &str) -> PathBuf {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(fallback);
    path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::now_v7()))
}

fn create_exclusive_file_with_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    if let Err(err) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(err.into());
    }
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(err) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            let _ = fs::remove_file(path);
            return Err(err.into());
        }
    }
    #[cfg(windows)]
    {
        if let Err(err) = windows_sec::restrict_file_dacl_to_current_user(path) {
            let _ = fs::remove_file(path);
            return Err(err);
        }
    }
    Ok(())
}

#[cfg(windows)]
mod windows_sec {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;

    use anyhow::{Result, anyhow, bail};
    use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::{PCWSTR, PWSTR};
    use zeroize::{Zeroize as _, Zeroizing};

    use super::VAULT_AAD;

    struct HandleGuard(HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: `self.0` is an owned Win32 handle obtained from `OpenProcessToken`.
                let _ = unsafe { CloseHandle(self.0) };
            }
        }
    }

    struct LocalMemGuard(*mut core::ffi::c_void);
    impl Drop for LocalMemGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` was allocated by a Win32 API requiring `LocalFree`.
                let _ = unsafe { LocalFree(Some(HLOCAL(self.0))) };
            }
        }
    }

    fn to_wide_null(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn current_user_sid_string() -> Result<String> {
        let mut token = HANDLE::default();
        // SAFETY: `GetCurrentProcess` returns a pseudo-handle; `token` receives an owned handle.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
            .map_err(|e| anyhow!("OpenProcessToken failed: {e}"))?;
        let _token_guard = HandleGuard(token);

        let mut needed: u32 = 0;
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
        if needed == 0 {
            bail!("GetTokenInformation returned zero buffer size");
        }
        const _: () = assert!(std::mem::align_of::<usize>() >= std::mem::align_of::<TOKEN_USER>());
        let word_count = (needed as usize).div_ceil(std::mem::size_of::<usize>());
        let mut buf = vec![0usize; word_count];
        let buf_bytes = u32::try_from(buf.len() * std::mem::size_of::<usize>())
            .map_err(|_| anyhow!("TokenUser buffer size overflow"))?;
        // SAFETY: `buf` is aligned for `TOKEN_USER` and at least `needed` bytes long.
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                buf_bytes,
                &mut needed,
            )
        }
        .map_err(|e| anyhow!("GetTokenInformation failed: {e}"))?;

        // SAFETY: `buf` was initialized by `GetTokenInformation`.
        let token_user = unsafe { &*(buf.as_ptr().cast::<TOKEN_USER>()) };
        let mut sid_pwstr = PWSTR::null();
        // SAFETY: `token_user.User.Sid` is valid for the lifetime of `buf`.
        unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_pwstr) }
            .map_err(|e| anyhow!("ConvertSidToStringSidW failed: {e}"))?;
        let _sid_guard = LocalMemGuard(sid_pwstr.0.cast());
        // SAFETY: `sid_pwstr` is NUL-terminated UTF-16.
        let sid_str = unsafe { sid_pwstr.to_string() }
            .map_err(|e| anyhow!("invalid UTF-16 SID string: {e}"))?;
        Ok(sid_str)
    }

    pub(super) fn restrict_file_dacl_to_current_user(path: &Path) -> Result<()> {
        let sid_str = current_user_sid_string()?;
        let sddl = format!("D:P(A;;FA;;;{sid_str})");
        let sddl_wide = to_wide_null(OsStr::new(&sddl));
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl_wide` is NUL-terminated UTF-16; `sd` is freed by `LocalMemGuard`.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl_wide.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        }
        .map_err(|e| anyhow!("ConvertStringSecurityDescriptorToSecurityDescriptorW failed: {e}"))?;
        let _sd_guard = LocalMemGuard(sd.0);

        let mut dacl_present = false.into();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut dacl_defaulted = false.into();
        // SAFETY: `sd` is valid until `_sd_guard` drops.
        unsafe {
            GetSecurityDescriptorDacl(sd, &mut dacl_present, &mut dacl, &mut dacl_defaulted)
        }
        .map_err(|e| anyhow!("GetSecurityDescriptorDacl failed: {e}"))?;
        if !dacl_present.as_bool() || dacl.is_null() {
            bail!("generated SDDL security descriptor missing DACL");
        }
        let path_wide = to_wide_null(path.as_os_str());
        // SAFETY: `path_wide` is NUL-terminated UTF-16 and `dacl` points into `sd`.
        let status = unsafe {
            SetNamedSecurityInfoW(
                PCWSTR(path_wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl),
                None,
            )
        };
        if status != ERROR_SUCCESS {
            bail!("SetNamedSecurityInfoW failed with Win32 error {}", status.0);
        }
        Ok(())
    }

    pub(super) fn dpapi_protect(plaintext: &[u8]) -> Result<Vec<u8>> {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(plaintext.len()).map_err(|_| anyhow!("key too large"))?,
            pbData: plaintext.as_ptr().cast_mut(),
        };
        let entropy_blob = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(VAULT_AAD.len()).map_err(|_| anyhow!("AAD too large"))?,
            pbData: VAULT_AAD.as_ptr().cast_mut(),
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        // SAFETY: input slices stay alive across call; `out_blob.pbData` freed by `LocalMemGuard`.
        unsafe {
            CryptProtectData(
                &in_blob,
                None,
                Some(&entropy_blob),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        }
        .map_err(|e| anyhow!("CryptProtectData failed: {e}"))?;
        let _out_guard = LocalMemGuard(out_blob.pbData.cast());
        if out_blob.pbData.is_null() || out_blob.cbData == 0 {
            bail!("CryptProtectData returned empty ciphertext");
        }
        // SAFETY: Windows wrote `cbData` bytes at `pbData`.
        let slice = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
        Ok(slice.to_vec())
    }

    pub(super) fn dpapi_unprotect(ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(ciphertext.len()).map_err(|_| anyhow!("key blob too large"))?,
            pbData: ciphertext.as_ptr().cast_mut(),
        };
        let entropy_blob = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(VAULT_AAD.len()).map_err(|_| anyhow!("AAD too large"))?,
            pbData: VAULT_AAD.as_ptr().cast_mut(),
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        // SAFETY: input slices stay alive across call; `out_blob.pbData` zeroized and freed below.
        unsafe {
            CryptUnprotectData(
                &in_blob,
                None,
                Some(&entropy_blob),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        }
        .map_err(|e| anyhow!("CryptUnprotectData failed: {e}"))?;
        let _out_guard = LocalMemGuard(out_blob.pbData.cast());
        if out_blob.pbData.is_null() || out_blob.cbData == 0 {
            bail!("CryptUnprotectData returned empty plaintext");
        }
        // SAFETY: Windows allocated `cbData` writable bytes at `pbData`.
        let slice =
            unsafe { std::slice::from_raw_parts_mut(out_blob.pbData, out_blob.cbData as usize) };
        let plain = Zeroizing::new(slice.to_vec());
        slice.zeroize();
        Ok(plain)
    }
}

#[cfg(test)]
mod tests {
    use fern_core::cloud::{KdfParams, VaultKey};

    use super::*;

    #[test]
    fn cloud_session_vault_roundtrip_and_wipe() -> Result<()> {
        let tmp = std::env::temp_dir().join(format!("fern-cloud-vault-{}", uuid::Uuid::now_v7()));
        let vault_path = tmp.join("cloud-session.vault");
        let key_path = tmp.join("cloud-session.key");

        assert!(load_from(&vault_path, &key_path)?.is_none());

        let vault_key = VaultKey::generate()?;
        let session = CloudSessionState {
            server_url: "https://fern.atabits.com".into(),
            user_id: "usr-1".into(),
            email: "alice@example.com".into(),
            display_name: "Alice".into(),
            session_id: "ses-1".into(),
            access_token: "fc_at_secret_access".into(),
            access_expires_at: 1_900_000_000,
            refresh_token: Some("fc_rt_secret_refresh".into()),
            vault_key_b64: vault_key.to_base64().to_string(),
            kdf: KdfParams::generate_default()?,
        };
        save_to(&vault_path, &key_path, &session)?;

        // Verify disk bytes are encrypted and do not contain plaintext tokens.
        let disk = fs::read(&vault_path)?;
        assert!(disk.starts_with(VAULT_MAGIC));
        assert!(!disk.windows(b"fc_rt_secret_refresh".len()).any(|w| w == b"fc_rt_secret_refresh"));
        assert!(!disk.windows(b"alice@example.com".len()).any(|w| w == b"alice@example.com"));

        let loaded = load_from(&vault_path, &key_path)?.expect("session present");
        assert_eq!(loaded.user_id, session.user_id);
        assert_eq!(loaded.access_token, session.access_token);
        assert_eq!(loaded.refresh_token, session.refresh_token);
        assert_eq!(loaded.vault_key()?, vault_key);

        clear_at(&vault_path)?;
        assert!(load_from(&vault_path, &key_path)?.is_none());
        let _ = fs::remove_dir_all(&tmp);
        Ok(())
    }

    #[test]
    fn keychain_gate_blocks_until_bootstrap_completes() {
        use std::sync::Arc;
        let gate = Arc::new((Mutex::new(BootstrapPhase::Pending), Condvar::new()));
        let gate_bg = Arc::clone(&gate);
        let started = Instant::now();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            *gate_bg.0.lock().unwrap() = BootstrapPhase::Ready;
            gate_bg.1.notify_all();
        });
        assert!(wait_gate_ready(&gate, Duration::from_secs(2)));
        assert!(started.elapsed() >= Duration::from_millis(25));
        releaser.join().unwrap();
    }
}
