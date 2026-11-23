//! Encrypted `SecretsVault` backed by a standalone ChaCha20-Poly1305 file outside `tasks.db`.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::{
    STANDARD as BASE64_STANDARD, STANDARD_NO_PAD as BASE64_STANDARD_NO_PAD,
    URL_SAFE as BASE64_URL_SAFE, URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD,
};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use zeroize::{Zeroize as _, Zeroizing};

const VAULT_MAGIC: &[u8; 8] = b"FERNVLT\x01";
const DPAPI_KEY_MAGIC: &[u8; 8] = b"FERNDP\x00\x01";
const VAULT_AAD: &[u8] = b"fern-plugin-secrets-v1";
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const MIN_SCRUB_LEN: usize = 3;

type VaultMap = BTreeMap<String, BTreeMap<String, String>>;

/// Encrypted per-plugin secrets store kept separate from `tasks.db`, backups, and sync.
#[derive(Clone)]
pub struct SecretsVault {
    inner: Arc<Mutex<VaultInner>>,
}

struct VaultInner {
    vault_path: PathBuf,
    key_path: PathBuf,
    secrets: VaultMap,
    cached_key: Option<Zeroizing<[u8; KEY_LEN]>>,
}

impl Drop for VaultInner {
    fn drop(&mut self) {
        zeroize_vault_map_in_place(&mut self.secrets);
        self.cached_key = None;
    }
}

fn zeroize_vault_map_in_place(map: &mut VaultMap) {
    for inner in map.values_mut() {
        for value in inner.values_mut() {
            value.zeroize();
        }
    }
    map.clear();
}

fn zeroize_vault_map(mut map: VaultMap) {
    zeroize_vault_map_in_place(&mut map);
}

impl SecretsVault {
    /// Opens an existing vault (if present) or prepares an empty in-memory vault backed by `vault_path`.
    pub fn open(vault_path: impl Into<PathBuf>, key_path: impl Into<PathBuf>) -> Result<Self> {
        let vault_path = vault_path.into();
        let key_path = key_path.into();

        let (secrets, cached_key) = if vault_path.exists() {
            let key = read_key_file(&key_path)?;
            let blob = fs::read(&vault_path)
                .with_context(|| format!("cannot read secrets vault {}", vault_path.display()))?;
            let secrets = decrypt_vault(&key, &blob)?;
            (secrets, Some(key))
        } else if key_path.exists() {
            let key = read_key_file(&key_path)?;
            (BTreeMap::new(), Some(key))
        } else {
            (BTreeMap::new(), None)
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(VaultInner {
                vault_path,
                key_path,
                secrets,
                cached_key,
            })),
        })
    }

    pub fn get(&self, plugin_id: &str, key: &str) -> Result<Option<String>> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("secrets vault lock poisoned"))?;
        Ok(guard
            .secrets
            .get(plugin_id)
            .and_then(|map| map.get(key))
            .cloned())
    }

    pub fn has(&self, plugin_id: &str, key: &str) -> Result<bool> {
        Ok(self
            .get(plugin_id, key)?
            .is_some_and(|value| !value.is_empty()))
    }

    pub fn keys(&self, plugin_id: &str) -> Result<Vec<String>> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("secrets vault lock poisoned"))?;
        Ok(guard
            .secrets
            .get(plugin_id)
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default())
    }

    pub fn set(&self, plugin_id: &str, key: &str, value: &str) -> Result<()> {
        ensure!(!plugin_id.trim().is_empty(), "plugin_id must not be empty");
        ensure!(!key.trim().is_empty(), "secret key must not be empty");
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("secrets vault lock poisoned"))?;
        let mut staged = guard.secrets.clone();
        staged
            .entry(plugin_id.to_owned())
            .or_default()
            .insert(key.to_owned(), value.to_owned());
        if let Err(err) = guard.persist_map(&staged) {
            zeroize_vault_map(staged);
            return Err(err);
        }
        let previous = std::mem::replace(&mut guard.secrets, staged);
        zeroize_vault_map(previous);
        Ok(())
    }

    pub fn delete(&self, plugin_id: &str, key: &str) -> Result<bool> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("secrets vault lock poisoned"))?;
        let exists = guard
            .secrets
            .get(plugin_id)
            .is_some_and(|map| map.contains_key(key));
        if !exists {
            return Ok(false);
        }
        let mut staged = guard.secrets.clone();
        if let Some(map) = staged.get_mut(plugin_id) {
            if let Some(mut old_val) = map.remove(key) {
                old_val.zeroize();
            }
            if map.is_empty() {
                staged.remove(plugin_id);
            }
        }
        if let Err(err) = guard.persist_map(&staged) {
            zeroize_vault_map(staged);
            return Err(err);
        }
        let previous = std::mem::replace(&mut guard.secrets, staged);
        zeroize_vault_map(previous);
        Ok(true)
    }

    pub fn delete_plugin(&self, plugin_id: &str) -> Result<bool> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("secrets vault lock poisoned"))?;
        if !guard.secrets.contains_key(plugin_id) {
            return Ok(false);
        }
        let mut staged = guard.secrets.clone();
        if let Some(mut removed_map) = staged.remove(plugin_id) {
            for val in removed_map.values_mut() {
                val.zeroize();
            }
        }
        if let Err(err) = guard.persist_map(&staged) {
            zeroize_vault_map(staged);
            return Err(err);
        }
        let previous = std::mem::replace(&mut guard.secrets, staged);
        zeroize_vault_map(previous);
        Ok(true)
    }

    /// Replaces `${secret:key}` placeholders in `input` using secrets scoped strictly to `plugin_id`.
    pub fn interpolate_secrets(&self, plugin_id: &str, input: &str) -> Result<String> {
        if !input.contains("${secret:") {
            return Ok(input.to_owned());
        }
        let guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("secrets vault lock poisoned"))?;
        let plugin_map = guard.secrets.get(plugin_id);

        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(start) = rest.find("${secret:") {
            out.push_str(&rest[..start]);
            let after_prefix = &rest[start + "${secret:".len()..];
            let Some(end) = after_prefix.find('}') else {
                bail!("unterminated ${{secret:...}} placeholder");
            };
            let secret_key = after_prefix[..end].trim();
            ensure!(!secret_key.is_empty(), "empty secret placeholder key");
            let secret_val = plugin_map
                .and_then(|m| m.get(secret_key))
                .ok_or_else(|| anyhow!("missing secret {secret_key:?} for plugin {plugin_id}"))?;
            out.push_str(secret_val);
            rest = &after_prefix[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    /// Redacts all stored secret values belonging to `plugin_id` (including Base64, percent-encoded,
    /// form-encoded, and JSON-escaped variants) from `text` before logging or returning to JS.
    ///
    /// Fails closed (`"[redacted]"`) if the vault lock is poisoned.
    pub fn scrub_secrets(&self, plugin_id: &str, text: &str) -> String {
        let Ok(guard) = self.inner.lock() else {
            return "[redacted]".to_owned();
        };
        let Some(plugin_map) = guard.secrets.get(plugin_id) else {
            return text.to_owned();
        };
        let values: Vec<&str> = plugin_map
            .values()
            .map(String::as_str)
            .filter(|v| v.len() >= MIN_SCRUB_LEN)
            .collect();
        if values.is_empty() {
            return text.to_owned();
        }
        scrub_secret_variants(text, &values)
    }
}

pub(crate) fn scrub_secret_variants(text: &str, secrets: &[&str]) -> String {
    let mut needles: Vec<Zeroizing<String>> = Vec::new();
    for &raw in secrets {
        if raw.len() < MIN_SCRUB_LEN {
            continue;
        }
        needles.push(Zeroizing::new(raw.to_owned()));

        // Percent-encoded variants (RFC 3986 upper/lower hex + form `+` space encoding).
        let pct_upper = percent_encode_with_case(raw, true);
        if pct_upper != raw {
            needles.push(Zeroizing::new(pct_upper.clone()));
            let pct_lower = percent_encode_with_case(raw, false);
            if pct_lower != pct_upper {
                needles.push(Zeroizing::new(pct_lower));
            }
        }
        if raw.contains(' ') {
            needles.push(Zeroizing::new(pct_upper.replace("%20", "+")));
            needles.push(Zeroizing::new(
                percent_encode_with_case(raw, false).replace("%20", "+"),
            ));
        }

        // Base64 variants (standard/url-safe, padded/unpadded).
        for b64 in [
            BASE64_STANDARD.encode(raw.as_bytes()),
            BASE64_STANDARD_NO_PAD.encode(raw.as_bytes()),
            BASE64_URL_SAFE.encode(raw.as_bytes()),
            BASE64_URL_SAFE_NO_PAD.encode(raw.as_bytes()),
        ] {
            if b64.len() >= 4 {
                needles.push(Zeroizing::new(b64));
            }
        }

        // JSON-escaped string content (without surrounding quotes).
        if let Ok(json_str) = serde_json::to_string(raw) {
            let inner = json_str
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(&json_str);
            if inner != raw && inner.len() >= MIN_SCRUB_LEN {
                needles.push(Zeroizing::new(inner.to_owned()));
            }
        }
    }

    if needles.is_empty() {
        return text.to_owned();
    }

    // Scrub longest variants first so substrings never leave partial suffix leaks.
    needles.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.as_str().cmp(b.as_str())));
    needles.dedup_by(|a, b| a.as_str() == b.as_str());

    let mut redacted = text.to_owned();
    for needle in &needles {
        let s = needle.as_str();
        if !s.is_empty() && redacted.contains(s) {
            redacted = redacted.replace(s, "***");
        }
    }
    redacted
}

fn percent_encode_with_case(input: &str, uppercase: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else if uppercase {
            out.push_str(&format!("%{byte:02X}"));
        } else {
            out.push_str(&format!("%{byte:02x}"));
        }
    }
    out
}

impl VaultInner {
    fn ensure_key(&mut self) -> Result<Zeroizing<[u8; KEY_LEN]>> {
        if let Some(ref key) = self.cached_key {
            return Ok(key.clone());
        }
        if self.key_path.exists() {
            let key = read_key_file(&self.key_path)?;
            self.cached_key = Some(key.clone());
            return Ok(key);
        }

        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        getrandom::getrandom(&mut *key)
            .map_err(|err| anyhow!("failed to generate vault key: {err}"))?;
        let disk_bytes = protect_key_for_disk(&key)?;

        match write_new_exclusive_restricted(&self.key_path, &disk_bytes)? {
            CreateExclusiveOutcome::Created => {
                self.cached_key = Some(key.clone());
                Ok(key)
            }
            CreateExclusiveOutcome::AlreadyExists => {
                // Another process raced us and created the key file first — adopt its key.
                let existing = read_key_file(&self.key_path)?;
                self.cached_key = Some(existing.clone());
                Ok(existing)
            }
        }
    }

    fn persist_map(&mut self, secrets: &VaultMap) -> Result<()> {
        let key = self.ensure_key()?;
        let blob = encrypt_vault(&key, secrets)?;
        write_atomic_restricted(&self.vault_path, &blob)
    }
}

enum CreateExclusiveOutcome {
    Created,
    AlreadyExists,
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
        fs::read(path).with_context(|| format!("cannot read vault key {}", path.display()))?,
    );
    if let Some(dpapi_payload) = bytes.strip_prefix(DPAPI_KEY_MAGIC) {
        #[cfg(windows)]
        {
            let plain = windows_sec::dpapi_unprotect(dpapi_payload)?;
            ensure!(
                plain.len() == KEY_LEN,
                "invalid DPAPI-unwrapped vault key length {} (expected {KEY_LEN})",
                plain.len()
            );
            let mut key = Zeroizing::new([0u8; KEY_LEN]);
            key.copy_from_slice(&plain);
            return Ok(key);
        }
        #[cfg(not(windows))]
        {
            let _ = dpapi_payload;
            bail!("vault key is protected with Windows DPAPI and cannot be unwrapped on this OS");
        }
    }
    ensure!(
        bytes.len() == KEY_LEN,
        "invalid vault key length {} (expected {KEY_LEN})",
        bytes.len()
    );
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    key.copy_from_slice(&bytes);
    Ok(key)
}

fn encrypt_vault(key: &[u8; KEY_LEN], secrets: &VaultMap) -> Result<Vec<u8>> {
    let plaintext = Zeroizing::new(serde_json::to_vec(secrets)?);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce_bytes)
        .map_err(|err| anyhow!("failed to generate vault nonce: {err}"))?;
    let cipher = ChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext.as_slice(),
                aad: VAULT_AAD,
            },
        )
        .map_err(|_| anyhow!("secrets vault encryption failed"))?;

    let mut out = Vec::with_capacity(VAULT_MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(VAULT_MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt_vault(key: &[u8; KEY_LEN], data: &[u8]) -> Result<VaultMap> {
    ensure!(
        data.starts_with(VAULT_MAGIC),
        "invalid secrets vault file header"
    );
    let header_len = VAULT_MAGIC.len() + NONCE_LEN;
    ensure!(data.len() >= header_len + 16, "truncated secrets vault file");
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
            .map_err(|_| anyhow!("failed to decrypt secrets vault (wrong key or tampered data)"))?,
    );
    let map: VaultMap = serde_json::from_slice(&plaintext)
        .context("decrypted secrets vault is not valid JSON")?;
    Ok(map)
}

/// Atomically creates a brand-new restricted file at `path` without overwriting an existing file.
///
/// Writes full bytes to an exclusively-created (`O_EXCL` / `create_new`) temp file, locks down
/// permissions, and publishes via `hard_link` (atomic non-overwriting publish), falling back to
/// direct `create_new(true)` when hard links are unsupported by the filesystem.
fn write_new_exclusive_restricted(path: &Path, bytes: &[u8]) -> Result<CreateExclusiveOutcome> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = unique_temp_path(path, "key");
    create_exclusive_file_with_bytes(&tmp_path, bytes)?;

    match fs::hard_link(&tmp_path, path) {
        Ok(()) => {
            let _ = fs::remove_file(&tmp_path);
            Ok(CreateExclusiveOutcome::Created)
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&tmp_path);
            Ok(CreateExclusiveOutcome::AlreadyExists)
        }
        Err(_) => {
            let _ = fs::remove_file(&tmp_path);
            match create_exclusive_file_with_bytes(path, bytes) {
                Ok(()) => Ok(CreateExclusiveOutcome::Created),
                Err(err)
                    if err
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|io_err| io_err.kind() == ErrorKind::AlreadyExists) =>
                {
                    Ok(CreateExclusiveOutcome::AlreadyExists)
                }
                Err(err) => Err(err),
            }
        }
    }
}

fn write_atomic_restricted(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = unique_temp_path(path, "vault");
    create_exclusive_file_with_bytes(&tmp_path, bytes)?;
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err.into());
    }
    Ok(())
}

fn unique_temp_path(path: &Path, fallback_stem: &str) -> PathBuf {
    let tmp_name = format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(fallback_stem),
        uuid::Uuid::now_v7()
    );
    path.with_file_name(tmp_name)
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
    if let Err(err) = apply_restricted_permissions(path) {
        let _ = fs::remove_file(path);
        return Err(err);
    }
    Ok(())
}

fn apply_restricted_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        windows_sec::restrict_file_dacl_to_current_user(path)?;
    }
    let _ = path;
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
                // SAFETY: `self.0` was allocated by a Win32 API documented to require `LocalFree`.
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
        // First call probes required buffer length (expected ERROR_INSUFFICIENT_BUFFER).
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
        if needed == 0 {
            bail!("GetTokenInformation returned zero buffer size for TokenUser");
        }
        // Back buffer with `Vec<usize>` so casting to `&TOKEN_USER` satisfies strict alignment.
        const _: () = assert!(std::mem::align_of::<usize>() >= std::mem::align_of::<TOKEN_USER>());
        let word_count = (needed as usize).div_ceil(std::mem::size_of::<usize>());
        let mut buf = vec![0usize; word_count];
        let buf_bytes = u32::try_from(buf.len() * std::mem::size_of::<usize>())
            .map_err(|_| anyhow!("TokenUser buffer size overflow"))?;
        // SAFETY: `buf` is `usize`-aligned and has at least `needed` writable bytes.
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                buf_bytes,
                &mut needed,
            )
        }
        .map_err(|e| anyhow!("GetTokenInformation(TokenUser) failed: {e}"))?;

        // SAFETY: `buf` is strictly aligned for `TOKEN_USER` and initialized by `GetTokenInformation`.
        let token_user = unsafe { &*(buf.as_ptr().cast::<TOKEN_USER>()) };
        let mut sid_pwstr = PWSTR::null();
        // SAFETY: `token_user.User.Sid` is valid for the lifetime of `buf`.
        unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_pwstr) }
            .map_err(|e| anyhow!("ConvertSidToStringSidW failed: {e}"))?;
        let _sid_guard = LocalMemGuard(sid_pwstr.0.cast());

        // SAFETY: `sid_pwstr` is a NUL-terminated UTF-16 string allocated by Windows.
        let sid_str = unsafe { sid_pwstr.to_string() }
            .map_err(|e| anyhow!("invalid UTF-16 SID string: {e}"))?;
        Ok(sid_str)
    }

    /// Replaces the file's DACL with a protected owner-only DACL (`D:P(A;;FA;;;<user-sid>)`),
    /// stripping all inherited ACEs so only the current Windows user can read or write the file.
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
        // SAFETY: `sd` is a valid security descriptor owned until `_sd_guard` drops.
        unsafe {
            GetSecurityDescriptorDacl(sd, &mut dacl_present, &mut dacl, &mut dacl_defaulted)
        }
        .map_err(|e| anyhow!("GetSecurityDescriptorDacl failed: {e}"))?;
        if !dacl_present.as_bool() || dacl.is_null() {
            bail!("generated SDDL security descriptor missing DACL");
        }

        let path_wide = to_wide_null(path.as_os_str());
        // SAFETY: `path_wide` is NUL-terminated UTF-16; `dacl` points inside live `sd`.
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
            bail!(
                "SetNamedSecurityInfoW failed for {} with Win32 error {}",
                path.display(),
                status.0
            );
        }
        Ok(())
    }

    /// Wraps key material with Windows DPAPI (`CryptProtectData`, CurrentUser scope, no UI).
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
        // SAFETY: Input blobs reference live slices; `out_blob.pbData` is freed via `LocalFree`.
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
        // SAFETY: Windows populated `cbData` readable bytes at `pbData`.
        let slice = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
        Ok(slice.to_vec())
    }

    /// Unwraps Windows DPAPI key material (`CryptUnprotectData`), zeroizing the Win32 buffer
    /// before freeing it.
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
        // SAFETY: Input blobs reference live slices; `out_blob.pbData` is zeroized & freed below.
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
    use super::*;

    #[test]
    fn vault_roundtrip_isolation_interpolation_and_scrubbing() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let vault_path = dir.path().join("plugin-secrets.vault");
        let key_path = dir.path().join("plugin-secrets.key");

        let vault = SecretsVault::open(&vault_path, &key_path)?;
        vault.set("com.example.a", "token", "ghp_super/secret+123=?")?;
        vault.set("com.example.b", "token", "other_secret")?;

        // Reopen from disk to verify persistence, ChaCha20-Poly1305 decryption, and DPAPI/ACL.
        let reopened = SecretsVault::open(&vault_path, &key_path)?;
        assert_eq!(
            reopened.get("com.example.a", "token")?.as_deref(),
            Some("ghp_super/secret+123=?")
        );
        assert_eq!(
            reopened.get("com.example.b", "token")?.as_deref(),
            Some("other_secret")
        );
        // Plugin A cannot resolve Plugin B's key.
        assert!(reopened.get("com.example.a", "missing")?.is_none());
        assert_eq!(
            reopened.interpolate_secrets("com.example.a", "Bearer ${secret:token}")?,
            "Bearer ghp_super/secret+123=?"
        );

        // Raw, Base64 (standard/url-safe/no-pad), and percent-encoded variants must all be scrubbed.
        let secret = "ghp_super/secret+123=?";
        let b64_std = BASE64_STANDARD.encode(secret.as_bytes());
        let b64_url = BASE64_URL_SAFE_NO_PAD.encode(secret.as_bytes());
        let pct_upper = percent_encode_with_case(secret, true);
        let pct_lower = percent_encode_with_case(secret, false);
        let log_line = format!(
            "raw={secret} b64={b64_std} b64url={b64_url} pctU={pct_upper} pctL={pct_lower}"
        );
        let scrubbed = reopened.scrub_secrets("com.example.a", &log_line);
        assert!(!scrubbed.contains(secret), "{scrubbed}");
        assert!(!scrubbed.contains(&b64_std), "{scrubbed}");
        assert!(!scrubbed.contains(&b64_url), "{scrubbed}");
        assert!(!scrubbed.contains(&pct_upper), "{scrubbed}");
        assert!(!scrubbed.contains(&pct_lower), "{scrubbed}");

        // Plugin B's secret is unrelated to plugin A scrubbing, and vice versa.
        assert_eq!(
            reopened.scrub_secrets("com.example.b", &format!("leaked {secret}")),
            format!("leaked {secret}")
        );

        // On Windows, verify newly generated key file is DPAPI-wrapped (not raw 32 bytes).
        #[cfg(windows)]
        {
            let key_bytes = fs::read(&key_path)?;
            assert!(
                key_bytes.starts_with(DPAPI_KEY_MAGIC),
                "expected DPAPI-wrapped key on Windows"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(fs::metadata(&key_path)?.permissions().mode() & 0o777, 0o600);
            assert_eq!(fs::metadata(&vault_path)?.permissions().mode() & 0o777, 0o600);
        }

        // Legacy 32-byte raw key files remain readable for backwards compatibility.
        let legacy_dir = tempfile::tempdir()?;
        let legacy_key = legacy_dir.path().join("legacy.key");
        let legacy_vault = legacy_dir.path().join("legacy.vault");
        let raw_key = [0x42u8; KEY_LEN];
        fs::write(&legacy_key, raw_key)?;
        let lv = SecretsVault::open(&legacy_vault, &legacy_key)?;
        lv.set("p1", "k", "legacy_val")?;
        let lv2 = SecretsVault::open(&legacy_vault, &legacy_key)?;
        assert_eq!(lv2.get("p1", "k")?.as_deref(), Some("legacy_val"));

        Ok(())
    }

    #[test]
    fn vault_does_not_mutate_memory_on_persist_failure_and_fails_closed_on_poison() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let vault_path = dir.path().join("blocked-dir");
        // Make `vault_path` an existing non-empty directory so atomic rename over it fails.
        fs::create_dir_all(&vault_path)?;
        fs::write(vault_path.join("sentinel"), b"x")?;
        let key_path = dir.path().join("vault.key");

        let vault = SecretsVault {
            inner: Arc::new(Mutex::new(VaultInner {
                vault_path,
                key_path,
                secrets: BTreeMap::new(),
                cached_key: None,
            })),
        };
        assert!(vault.set("com.example.a", "k", "v1").is_err());
        assert!(
            vault.get("com.example.a", "k")?.is_none(),
            "in-memory map must not mutate when disk persist fails"
        );

        // Poison the mutex and confirm `scrub_secrets` fails closed instead of leaking input text.
        let poisoned = vault.clone();
        let _ = std::thread::spawn(move || {
            let _g = poisoned.inner.lock().unwrap();
            panic!("intentional poison");
        })
        .join();
        let scrubbed = vault.scrub_secrets("com.example.a", "Authorization: Bearer super_secret");
        assert!(!scrubbed.contains("super_secret"));
        assert_eq!(scrubbed, "[redacted]");
        Ok(())
    }

    #[test]
    fn vault_concurrent_first_init_shares_single_key_without_corruption() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let key_path = dir.path().join("shared.key");
        let mut handles = Vec::new();
        for i in 0..8 {
            let kp = key_path.clone();
            let vp = dir.path().join(format!("v{i}.vault"));
            handles.push(std::thread::spawn(move || -> Result<()> {
                let v = SecretsVault::open(&vp, &kp)?;
                v.set("p", "k", &format!("val-{i}"))?;
                Ok(())
            }));
        }
        for h in handles {
            h.join().expect("worker panicked")?;
        }
        // Every vault file must decrypt cleanly with the single winning key file.
        for i in 0..8 {
            let vp = dir.path().join(format!("v{i}.vault"));
            let reopened = SecretsVault::open(&vp, &key_path)?;
            assert_eq!(
                reopened.get("p", "k")?.as_deref(),
                Some(format!("val-{i}").as_str())
            );
        }
        Ok(())
    }
}
