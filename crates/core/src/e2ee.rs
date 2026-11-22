//! Client-side end-to-end encryption (E2EE) for synced rows and attachment files.
//!
//! Keys are derived from a user passphrase with PBKDF2-HMAC-SHA256 (200,000 rounds, domain salt
//! `"fern-e2ee-v1"`) and cached in memory so repeated syncs avoid re-running the KDF. Payloads
//! are authenticated-encrypted with ChaCha20-Poly1305 using fresh random 12-byte nonces and
//! domain-separated AAD bound to the entity/key or file name.

use std::fmt;
use std::sync::{Mutex, OnceLock};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

const PBKDF2_ITERATIONS: u32 = 200_000;
const DOMAIN_SALT: &[u8] = b"fern-e2ee-v1";
const KEY_CACHE_CAPACITY: usize = 4;

const ROW_MARKER: &str = "__fern_e2ee";
const ROW_VERSION: u64 = 1;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const FILE_MAGIC: &[u8; 9] = b"FERNE2EE\x01";

/// Typed errors produced by client-side E2EE operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2eeError {
    /// Encrypted sync data was received, or encryption was requested, without a passphrase.
    PassphraseRequired,
    /// Decryption failed authentication (wrong passphrase, tampered ciphertext, or mismatched AAD).
    WrongPassphrase,
    /// The encrypted envelope or decrypted JSON payload is structurally invalid.
    Malformed(String),
}

impl fmt::Display for E2eeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PassphraseRequired => write!(f, "an encryption passphrase is required to decrypt synced data"),
            Self::WrongPassphrase => write!(f, "failed to decrypt synced data (wrong passphrase or corrupted payload)"),
            Self::Malformed(reason) => write!(f, "malformed encrypted payload: {reason}"),
        }
    }
}

impl std::error::Error for E2eeError {}

/// Symmetric key derived from a user passphrase for row and attachment encryption.
#[derive(Clone)]
pub struct E2eeKey([u8; 32]);

impl fmt::Debug for E2eeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("E2eeKey([REDACTED])")
    }
}

type KeyCache = Mutex<Vec<([u8; 32], [u8; 32])>>;

fn key_cache() -> &'static KeyCache {
    static CACHE: OnceLock<KeyCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::with_capacity(KEY_CACHE_CAPACITY)))
}

impl E2eeKey {
    /// Derives (or retrieves from the in-memory LRU cache) the 256-bit key for `passphrase`.
    pub fn from_passphrase(passphrase: &str) -> Result<Self, E2eeError> {
        if passphrase.is_empty() {
            return Err(E2eeError::PassphraseRequired);
        }
        let digest: [u8; 32] = Sha256::digest(passphrase.as_bytes()).into();
        if let Ok(mut cache) = key_cache().lock()
            && let Some(pos) = cache.iter().position(|(d, _)| *d == digest)
        {
            let entry = cache.remove(pos);
            let key = entry.1;
            cache.push(entry);
            return Ok(Self(key));
        }
        let mut key = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), DOMAIN_SALT, PBKDF2_ITERATIONS, &mut key);
        if let Ok(mut cache) = key_cache().lock() {
            if let Some(pos) = cache.iter().position(|(d, _)| *d == digest) {
                cache.remove(pos);
            } else if cache.len() >= KEY_CACHE_CAPACITY {
                cache.remove(0);
            }
            cache.push((digest, key));
        }
        Ok(Self(key))
    }

    /// Encrypts a synced database row into an authenticated JSON envelope.
    pub fn encrypt_row(&self, entity: &str, key: &str, row: &Map<String, Value>) -> Result<Map<String, Value>, E2eeError> {
        let plaintext = serde_json::to_vec(row)
            .map_err(|err| E2eeError::Malformed(format!("failed to serialize row: {err}")))?;
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::getrandom(&mut nonce_bytes)
            .map_err(|err| E2eeError::Malformed(format!("failed to generate random nonce: {err}")))?;
        let aad = format!("fern-row-v1:{entity}:{key}");
        let cipher = ChaCha20Poly1305::new((&self.0).into());
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload { msg: &plaintext, aad: aad.as_bytes() },
            )
            .map_err(|_| E2eeError::Malformed("row encryption failed".into()))?;

        let mut envelope = Map::with_capacity(3);
        envelope.insert(ROW_MARKER.into(), Value::from(ROW_VERSION));
        envelope.insert("nonce".into(), Value::from(BASE64.encode(nonce_bytes)));
        envelope.insert("ct".into(), Value::from(BASE64.encode(ciphertext)));
        Ok(envelope)
    }

    /// Decrypts an authenticated JSON row envelope back into its plaintext column map.
    pub fn decrypt_row(&self, entity: &str, key: &str, row: &Map<String, Value>) -> Result<Map<String, Value>, E2eeError> {
        let version = row
            .get(ROW_MARKER)
            .and_then(Value::as_u64)
            .ok_or_else(|| E2eeError::Malformed("missing or invalid E2EE version marker".into()))?;
        if version != ROW_VERSION {
            return Err(E2eeError::Malformed(format!("unsupported E2EE row version {version}")));
        }
        let nonce_b64 = row
            .get("nonce")
            .and_then(Value::as_str)
            .ok_or_else(|| E2eeError::Malformed("missing nonce field".into()))?;
        let ct_b64 = row
            .get("ct")
            .and_then(Value::as_str)
            .ok_or_else(|| E2eeError::Malformed("missing ct field".into()))?;
        let nonce_bytes = BASE64
            .decode(nonce_b64)
            .map_err(|err| E2eeError::Malformed(format!("invalid base64 nonce: {err}")))?;
        if nonce_bytes.len() != NONCE_LEN {
            return Err(E2eeError::Malformed(format!("invalid nonce length {}", nonce_bytes.len())));
        }
        let ciphertext = BASE64
            .decode(ct_b64)
            .map_err(|err| E2eeError::Malformed(format!("invalid base64 ciphertext: {err}")))?;

        let aad = format!("fern-row-v1:{entity}:{key}");
        let cipher = ChaCha20Poly1305::new((&self.0).into());
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload { msg: &ciphertext, aad: aad.as_bytes() },
            )
            .map_err(|_| E2eeError::WrongPassphrase)?;
        let value: Value = serde_json::from_slice(&plaintext)
            .map_err(|err| E2eeError::Malformed(format!("decrypted row is not valid JSON: {err}")))?;
        match value {
            Value::Object(map) => Ok(map),
            _ => Err(E2eeError::Malformed("decrypted row is not a JSON object".into())),
        }
    }

    /// Encrypts a file blob as `b"FERNE2EE\x01" || 12-byte nonce || ciphertext+tag`.
    pub fn encrypt_file(&self, name: &str, data: &[u8]) -> Result<Vec<u8>, E2eeError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::getrandom(&mut nonce_bytes)
            .map_err(|err| E2eeError::Malformed(format!("failed to generate random nonce: {err}")))?;
        let aad = format!("fern-file-v1:{name}");
        let cipher = ChaCha20Poly1305::new((&self.0).into());
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload { msg: data, aad: aad.as_bytes() },
            )
            .map_err(|_| E2eeError::Malformed("file encryption failed".into()))?;

        let mut out = Vec::with_capacity(FILE_MAGIC.len() + NONCE_LEN + ciphertext.len());
        out.extend_from_slice(FILE_MAGIC);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypts a file blob produced by [`Self::encrypt_file`].
    pub fn decrypt_file(&self, name: &str, data: &[u8]) -> Result<Vec<u8>, E2eeError> {
        if !data.starts_with(FILE_MAGIC) {
            return Err(E2eeError::Malformed("missing FERNE2EE header".into()));
        }
        let header_len = FILE_MAGIC.len() + NONCE_LEN;
        if data.len() < header_len + TAG_LEN {
            return Err(E2eeError::Malformed("truncated encrypted file".into()));
        }
        let nonce_bytes = &data[FILE_MAGIC.len()..header_len];
        let ciphertext = &data[header_len..];
        let aad = format!("fern-file-v1:{name}");
        let cipher = ChaCha20Poly1305::new((&self.0).into());
        cipher
            .decrypt(
                Nonce::from_slice(nonce_bytes),
                Payload { msg: ciphertext, aad: aad.as_bytes() },
            )
            .map_err(|_| E2eeError::WrongPassphrase)
    }
}

/// Returns `true` if `row` is an E2EE row envelope.
pub fn is_encrypted_row(row: &Map<String, Value>) -> bool {
    row.contains_key(ROW_MARKER)
}

/// Returns `true` if `data` starts with the Fern E2EE file magic header.
pub fn is_encrypted_file(data: &[u8]) -> bool {
    data.starts_with(FILE_MAGIC)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row() -> Map<String, Value> {
        let mut row = Map::new();
        row.insert("id".into(), Value::from("task-1"));
        row.insert("title".into(), Value::from("Secret project plan"));
        row.insert("priority".into(), Value::from(3));
        row.insert("notes".into(), Value::from("Top secret notes"));
        row
    }

    #[test]
    fn empty_passphrase_returns_passphrase_required() {
        assert_eq!(E2eeKey::from_passphrase("").unwrap_err(), E2eeError::PassphraseRequired);
    }

    #[test]
    fn row_roundtrip_and_fresh_nonces() {
        let key = E2eeKey::from_passphrase("correct horse battery staple").unwrap();
        let plain = sample_row();
        assert!(!is_encrypted_row(&plain));

        let enc1 = key.encrypt_row("tasks", "task-1", &plain).unwrap();
        let enc2 = key.encrypt_row("tasks", "task-1", &plain).unwrap();
        assert!(is_encrypted_row(&enc1));
        assert_eq!(enc1.get(ROW_MARKER).and_then(Value::as_u64), Some(1));
        assert_ne!(enc1.get("nonce"), enc2.get("nonce"), "each encryption must use a fresh nonce");

        let dec1 = key.decrypt_row("tasks", "task-1", &enc1).unwrap();
        let dec2 = key.decrypt_row("tasks", "task-1", &enc2).unwrap();
        assert_eq!(dec1, plain);
        assert_eq!(dec2, plain);
    }

    #[test]
    fn row_wrong_passphrase_and_tampered_aad_fail() {
        let key_a = E2eeKey::from_passphrase("passphrase-alpha").unwrap();
        let key_b = E2eeKey::from_passphrase("passphrase-beta").unwrap();
        let plain = sample_row();
        let enc = key_a.encrypt_row("tasks", "task-1", &plain).unwrap();

        assert_eq!(key_b.decrypt_row("tasks", "task-1", &enc).unwrap_err(), E2eeError::WrongPassphrase);
        assert_eq!(key_a.decrypt_row("lists", "task-1", &enc).unwrap_err(), E2eeError::WrongPassphrase);
        assert_eq!(key_a.decrypt_row("tasks", "task-2", &enc).unwrap_err(), E2eeError::WrongPassphrase);

        // Tamper with ciphertext
        let mut tampered = enc;
        let mut ct = BASE64.decode(tampered["ct"].as_str().unwrap()).unwrap();
        ct[0] ^= 0xFF;
        tampered.insert("ct".into(), Value::from(BASE64.encode(ct)));
        assert_eq!(key_a.decrypt_row("tasks", "task-1", &tampered).unwrap_err(), E2eeError::WrongPassphrase);
    }

    #[test]
    fn file_roundtrip_wrong_passphrase_and_tampered_aad() {
        let key_a = E2eeKey::from_passphrase("file-secret-1").unwrap();
        let key_b = E2eeKey::from_passphrase("file-secret-2").unwrap();
        let data = b"%PDF-1.7 confidential attachment bytes \x00\xff";
        assert!(!is_encrypted_file(data));

        let enc = key_a.encrypt_file("123-report.pdf", data).unwrap();
        assert!(is_encrypted_file(&enc));
        assert_eq!(key_a.decrypt_file("123-report.pdf", &enc).unwrap(), data);

        // Empty file roundtrip
        let enc_empty = key_a.encrypt_file("empty.txt", b"").unwrap();
        assert!(is_encrypted_file(&enc_empty));
        assert_eq!(key_a.decrypt_file("empty.txt", &enc_empty).unwrap(), b"");

        // Wrong passphrase & wrong file name (AAD)
        assert_eq!(key_b.decrypt_file("123-report.pdf", &enc).unwrap_err(), E2eeError::WrongPassphrase);
        assert_eq!(key_a.decrypt_file("999-report.pdf", &enc).unwrap_err(), E2eeError::WrongPassphrase);

        // Malformed file header / truncated
        assert!(matches!(key_a.decrypt_file("a.txt", b"plaintext"), Err(E2eeError::Malformed(_))));
        assert!(matches!(key_a.decrypt_file("a.txt", b"FERNE2EE\x01short"), Err(E2eeError::Malformed(_))));
    }

    #[test]
    fn key_cache_returns_identical_key() {
        let k1 = E2eeKey::from_passphrase("cached-passphrase").unwrap();
        let k2 = E2eeKey::from_passphrase("cached-passphrase").unwrap();
        assert_eq!(k1.0, k2.0);
        assert_eq!(format!("{k1:?}"), "E2eeKey([REDACTED])");
    }
}
