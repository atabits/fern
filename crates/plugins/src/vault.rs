//! Encrypted `SecretsVault` backed by a standalone ChaCha20-Poly1305 file outside `tasks.db`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};

const VAULT_MAGIC: &[u8; 8] = b"FERNVLT\x01";
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
    cached_key: Option<[u8; KEY_LEN]>,
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
        guard
            .secrets
            .entry(plugin_id.to_owned())
            .or_default()
            .insert(key.to_owned(), value.to_owned());
        guard.persist()
    }

    pub fn delete(&self, plugin_id: &str, key: &str) -> Result<bool> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("secrets vault lock poisoned"))?;
        let removed = guard
            .secrets
            .get_mut(plugin_id)
            .and_then(|map| map.remove(key))
            .is_some();
        if removed {
            if guard
                .secrets
                .get(plugin_id)
                .is_some_and(BTreeMap::is_empty)
            {
                guard.secrets.remove(plugin_id);
            }
            guard.persist()?;
        }
        Ok(removed)
    }

    pub fn delete_plugin(&self, plugin_id: &str) -> Result<bool> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("secrets vault lock poisoned"))?;
        let removed = guard.secrets.remove(plugin_id).is_some();
        if removed {
            guard.persist()?;
        }
        Ok(removed)
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

    /// Redacts all stored secret values belonging to `plugin_id` from `text` before logging.
    pub fn scrub_secrets(&self, plugin_id: &str, text: &str) -> String {
        let Ok(guard) = self.inner.lock() else {
            return text.to_owned();
        };
        let Some(plugin_map) = guard.secrets.get(plugin_id) else {
            return text.to_owned();
        };
        let mut values: Vec<&str> = plugin_map
            .values()
            .map(String::as_str)
            .filter(|v| v.len() >= MIN_SCRUB_LEN)
            .collect();
        if values.is_empty() {
            return text.to_owned();
        }
        // Scrub longest secrets first so substrings do not leave partial suffix leaks.
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        values.dedup();
        let mut redacted = text.to_owned();
        for secret in values {
            if redacted.contains(secret) {
                redacted = redacted.replace(secret, "***");
            }
        }
        redacted
    }
}

impl VaultInner {
    fn ensure_key(&mut self) -> Result<[u8; KEY_LEN]> {
        if let Some(key) = self.cached_key {
            return Ok(key);
        }
        if self.key_path.exists() {
            let key = read_key_file(&self.key_path)?;
            self.cached_key = Some(key);
            return Ok(key);
        }
        let mut key = [0u8; KEY_LEN];
        getrandom::getrandom(&mut key)
            .map_err(|err| anyhow!("failed to generate vault key: {err}"))?;
        write_atomic_restricted(&self.key_path, &key)?;
        self.cached_key = Some(key);
        Ok(key)
    }

    fn persist(&mut self) -> Result<()> {
        let key = self.ensure_key()?;
        let blob = encrypt_vault(&key, &self.secrets)?;
        write_atomic_restricted(&self.vault_path, &blob)
    }
}

fn read_key_file(path: &Path) -> Result<[u8; KEY_LEN]> {
    let bytes = fs::read(path)
        .with_context(|| format!("cannot read vault key {}", path.display()))?;
    ensure!(
        bytes.len() == KEY_LEN,
        "invalid vault key length {} (expected {KEY_LEN})",
        bytes.len()
    );
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&bytes);
    Ok(key)
}

fn encrypt_vault(key: &[u8; KEY_LEN], secrets: &VaultMap) -> Result<Vec<u8>> {
    let plaintext = serde_json::to_vec(secrets)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce_bytes)
        .map_err(|err| anyhow!("failed to generate vault nonce: {err}"))?;
    let cipher = ChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: &plaintext,
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
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: VAULT_AAD,
            },
        )
        .map_err(|_| anyhow!("failed to decrypt secrets vault (wrong key or tampered data)"))?;
    let map: VaultMap = serde_json::from_slice(&plaintext)
        .context("decrypted secrets vault is not valid JSON")?;
    Ok(map)
}

fn write_atomic_restricted(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_name = format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("vault"),
        uuid::Uuid::now_v7()
    );
    let tmp_path = path.with_file_name(tmp_name);
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut file = opts
            .open(&tmp_path)
            .with_context(|| format!("cannot write {}", tmp_path.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600))?;
    }
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err.into());
    }
    Ok(())
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
        vault.set("com.example.a", "token", "ghp_super_secret_123")?;
        vault.set("com.example.b", "token", "other_secret")?;

        // Reopen from disk to verify persistence and ChaCha20-Poly1305 decryption.
        let reopened = SecretsVault::open(&vault_path, &key_path)?;
        assert_eq!(
            reopened.get("com.example.a", "token")?.as_deref(),
            Some("ghp_super_secret_123")
        );
        assert_eq!(
            reopened.get("com.example.b", "token")?.as_deref(),
            Some("other_secret")
        );
        // Plugin A cannot resolve Plugin B's key.
        assert!(reopened.get("com.example.a", "missing")?.is_none());
        assert_eq!(
            reopened.interpolate_secrets("com.example.a", "Bearer ${secret:token}")?,
            "Bearer ghp_super_secret_123"
        );

        let scrubbed = reopened.scrub_secrets(
            "com.example.a",
            "Request failed with Authorization ghp_super_secret_123 on api",
        );
        assert!(!scrubbed.contains("ghp_super_secret_123"));
        assert!(scrubbed.contains("***"));
        // Plugin B's secret is unrelated to plugin A scrubbing, and vice versa.
        assert_eq!(
            reopened.scrub_secrets("com.example.b", "leaked ghp_super_secret_123"),
            "leaked ghp_super_secret_123"
        );
        Ok(())
    }
}
