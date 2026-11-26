//! Fern Cloud zero-knowledge vault cryptography and shared API protocol contracts.
//!
//! # Security Architecture
//!
//! Fern Cloud uses a zero-knowledge split-key hierarchy inspired by Bitwarden and 1Password:
//!
//! 1. Each account has a unique random 16-byte `kdf_salt` and versioned `KdfParams`
//!    (`argon2id-v1`, OWASP 2024+ `m=19456 KiB, t=2, p=1`).
//! 2. On the client, `MK = Argon2id(password, kdf_salt)` produces a 256-bit master secret.
//! 3. RFC 5869 HKDF-SHA256 splits `MK` into two cryptographically independent 256-bit keys:
//!    - `auth_key = HKDF(MK, kdf_salt, "fern-cloud-auth-v1")` — transmitted over TLS for login,
//!      where the server hashes it a second time with its own Argon2id + random server salt.
//!    - `kek = HKDF(MK, kdf_salt, "fern-cloud-kek-v1")` — **never leaves the client**.
//! 4. Each user's synced rows and attachment blobs are encrypted using a random 256-bit
//!    **Vault Key (`DEK`)** via [`crate::e2ee::E2eeKey`] (ChaCha20-Poly1305 with domain-separated
//!    AAD). `DEK` is wrapped by `kek` (`wrapped_vault_key`) before upload.
//! 5. A 256-bit human-readable **Recovery Key** (Crockford Base32 with 16-bit SHA-256 checksum)
//!    independently splits via HKDF into `recovery_auth_key` (proves possession zero-knowledge)
//!    and `recovery_kek` (wraps `DEK` as `recovery_wrapped_vault_key`).
//!
//! Because data is encrypted with `DEK` rather than the password directly:
//! - Changing a password only re-wraps the 32-byte `DEK` (instant, zero row re-encryption).
//! - Forgetting a password can be recovered via the Recovery Key without any server backdoor.
//! - Even a full server + database compromise reveals zero plaintext tasks, notes, or files.

use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::e2ee::E2eeKey;

/// Default hosted Fern Cloud origin. Users can override this with any self-hosted Fern Cloud URL.
pub const DEFAULT_CLOUD_URL: &str = "https://fern.atabits.com";

/// API version prefix served by `fern-cloud`.
pub const CLOUD_API_PREFIX: &str = "/api/v1";

/// Current Fern Cloud API schema version.
pub const CLOUD_API_VERSION: u32 = 1;

/// Supported client-side KDF identifier.
pub const KDF_ALG_ARGON2ID_V1: &str = "argon2id-v1";

/// Default OWASP-recommended Argon2id parameters balanced for native desktop and browser WASM.
pub const DEFAULT_ARGON2_M_COST: u32 = 19_456; // 19 MiB
pub const DEFAULT_ARGON2_T_COST: u32 = 2;
pub const DEFAULT_ARGON2_P_COST: u32 = 1;

/// Client-enforced safety bounds on server-supplied KDF parameters to prevent DoS or downgrade.
pub const MIN_ARGON2_M_COST: u32 = 4_096;
pub const MAX_ARGON2_M_COST: u32 = 262_144; // 256 MiB
pub const MIN_ARGON2_T_COST: u32 = 1;
pub const MAX_ARGON2_T_COST: u32 = 16;
pub const MIN_ARGON2_P_COST: u32 = 1;
pub const MAX_ARGON2_P_COST: u32 = 4;

pub const MIN_PASSWORD_LEN: usize = 10;
pub const MAX_PASSWORD_LEN: usize = 256;
pub const MAX_EMAIL_LEN: usize = 254;
pub const MAX_DISPLAY_NAME_LEN: usize = 80;

const KDF_SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const WRAP_VERSION: u8 = 1;

const HKDF_INFO_AUTH: &[u8] = b"fern-cloud-auth-v1";
const HKDF_INFO_KEK: &[u8] = b"fern-cloud-kek-v1";
const HKDF_INFO_RECOVERY_AUTH: &[u8] = b"fern-cloud-recovery-auth-v1";
const HKDF_INFO_RECOVERY_KEK: &[u8] = b"fern-cloud-recovery-kek-v1";

const RECOVERY_PREFIX: &str = "FERN";
const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

type HmacSha256 = Hmac<Sha256>;

/// Typed errors from Fern Cloud cryptography and input validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudCryptoError {
    InvalidEmail(String),
    WeakPassword(String),
    InvalidKdfParams(String),
    InvalidRecoveryKey(String),
    DecryptionFailed,
    MalformedEnvelope(String),
    RandomFailed(String),
}

impl fmt::Display for CloudCryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEmail(msg) => write!(f, "invalid email: {msg}"),
            Self::WeakPassword(msg) => write!(f, "invalid password: {msg}"),
            Self::InvalidKdfParams(msg) => write!(f, "invalid KDF parameters: {msg}"),
            Self::InvalidRecoveryKey(msg) => write!(f, "invalid recovery key: {msg}"),
            Self::DecryptionFailed => write!(f, "wrong password or corrupted vault key"),
            Self::MalformedEnvelope(msg) => write!(f, "malformed vault envelope: {msg}"),
            Self::RandomFailed(msg) => write!(f, "secure random generation failed: {msg}"),
        }
    }
}

impl std::error::Error for CloudCryptoError {}

/// Non-secret metadata stored locally in `tasks.db` settings so UI knows which Fern Cloud account
/// is connected without ever storing access tokens, refresh tokens, or vault keys inside `tasks.db`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudMeta {
    pub server_url: String,
    pub user_id: String,
    pub email: String,
    pub display_name: String,
}

impl CloudMeta {
    pub fn is_configured(&self) -> bool {
        !self.server_url.trim().is_empty() && !self.user_id.trim().is_empty()
    }

    /// 1-2 uppercase initials for sidebar avatar badges.
    pub fn initials(&self) -> String {
        let source = if !self.display_name.trim().is_empty() {
            self.display_name.trim()
        } else {
            self.email.split('@').next().unwrap_or("").trim()
        };
        let mut chars = source
            .split(|c: char| c.is_whitespace() || matches!(c, '.' | '_' | '-'))
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.chars().next())
            .map(|c| c.to_uppercase().to_string());
        let first = chars.next().unwrap_or_default();
        let second = chars.next().unwrap_or_default();
        let out = format!("{first}{second}");
        if out.is_empty() { "F".to_owned() } else { out }
    }
}

/// Normalizes an email address for canonical lookup and AAD binding.
pub fn normalize_email(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

/// Validates and returns the normalized email address.
pub fn validate_email(raw: &str) -> Result<String, CloudCryptoError> {
    let email = normalize_email(raw);
    if email.is_empty() {
        return Err(CloudCryptoError::InvalidEmail("email is required".into()));
    }
    if email.len() > MAX_EMAIL_LEN {
        return Err(CloudCryptoError::InvalidEmail("email is too long".into()));
    }
    if email.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace()) {
        return Err(CloudCryptoError::InvalidEmail("email contains invalid whitespace".into()));
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err(CloudCryptoError::InvalidEmail("missing '@' symbol".into()));
    };
    if local.is_empty()
        || local.len() > 64
        || domain.is_empty()
        || domain.contains('@')
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.contains("..")
    {
        return Err(CloudCryptoError::InvalidEmail("enter a valid email address".into()));
    }
    Ok(email)
}

/// Validates password length requirements.
pub fn validate_password(password: &str) -> Result<(), CloudCryptoError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(CloudCryptoError::WeakPassword(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if password.len() > MAX_PASSWORD_LEN {
        return Err(CloudCryptoError::WeakPassword(format!(
            "password must be at most {MAX_PASSWORD_LEN} bytes"
        )));
    }
    Ok(())
}

/// Estimates password strength on a `0..=4` scale for registration UX feedback.
pub fn password_strength_score(password: &str) -> u8 {
    let len = password.chars().count();
    if len == 0 {
        return 0;
    }
    let mut score: u8 = 0;
    if len >= MIN_PASSWORD_LEN {
        score += 1;
    }
    if len >= 14 {
        score += 1;
    }
    let has_lower = password.chars().any(char::is_lowercase);
    let has_upper = password.chars().any(char::is_uppercase);
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_symbol = password.chars().any(|c| !c.is_alphanumeric());
    let classes = u8::from(has_lower) + u8::from(has_upper) + u8::from(has_digit) + u8::from(has_symbol);
    if classes >= 3 {
        score += 1;
    }
    if classes == 4 || len >= 20 {
        score += 1;
    }
    score.min(4)
}

/// Sanitizes an optional display name to safe length and single-line text.
pub fn sanitize_display_name(raw: &str) -> String {
    raw.trim()
        .replace(['\r', '\n'], " ")
        .chars()
        .take(MAX_DISPLAY_NAME_LEN)
        .collect()
}

/// Normalizes a Fern Cloud base URL (strips trailing slash and `/api/v1` suffix if user pasted it).
pub fn normalize_cloud_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    trimmed
        .strip_suffix(CLOUD_API_PREFIX)
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_owned()
}

/// Client-side Argon2id KDF parameters stored per account and returned by `/auth/prelogin`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub alg: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    /// Base64url-no-pad encoded random salt (>= 16 bytes).
    pub salt: String,
}

impl KdfParams {
    /// Generates fresh default Argon2id parameters with a 16-byte CSPRNG salt.
    pub fn generate_default() -> Result<Self, CloudCryptoError> {
        let mut salt = [0u8; KDF_SALT_LEN];
        getrandom::getrandom(&mut salt)
            .map_err(|err| CloudCryptoError::RandomFailed(err.to_string()))?;
        Ok(Self {
            alg: KDF_ALG_ARGON2ID_V1.to_owned(),
            m_cost: DEFAULT_ARGON2_M_COST,
            t_cost: DEFAULT_ARGON2_T_COST,
            p_cost: DEFAULT_ARGON2_P_COST,
            salt: B64URL.encode(salt),
        })
    }

    /// Validates parameters against anti-DoS and anti-downgrade bounds and returns decoded salt bytes.
    pub fn validate_and_decode_salt(&self) -> Result<Vec<u8>, CloudCryptoError> {
        if self.alg != KDF_ALG_ARGON2ID_V1 {
            return Err(CloudCryptoError::InvalidKdfParams(format!(
                "unsupported KDF algorithm {:?}",
                self.alg
            )));
        }
        if !(MIN_ARGON2_M_COST..=MAX_ARGON2_M_COST).contains(&self.m_cost) {
            return Err(CloudCryptoError::InvalidKdfParams(format!(
                "m_cost {} outside allowed range {}..={}",
                self.m_cost, MIN_ARGON2_M_COST, MAX_ARGON2_M_COST
            )));
        }
        if !(MIN_ARGON2_T_COST..=MAX_ARGON2_T_COST).contains(&self.t_cost) {
            return Err(CloudCryptoError::InvalidKdfParams(format!(
                "t_cost {} outside allowed range {}..={}",
                self.t_cost, MIN_ARGON2_T_COST, MAX_ARGON2_T_COST
            )));
        }
        if !(MIN_ARGON2_P_COST..=MAX_ARGON2_P_COST).contains(&self.p_cost) {
            return Err(CloudCryptoError::InvalidKdfParams(format!(
                "p_cost {} outside allowed range {}..={}",
                self.p_cost, MIN_ARGON2_P_COST, MAX_ARGON2_P_COST
            )));
        }
        let salt = B64URL
            .decode(&self.salt)
            .map_err(|err| CloudCryptoError::InvalidKdfParams(format!("invalid salt base64url: {err}")))?;
        if salt.len() < KDF_SALT_LEN || salt.len() > 64 {
            return Err(CloudCryptoError::InvalidKdfParams(format!(
                "salt length {} outside allowed range 16..=64",
                salt.len()
            )));
        }
        Ok(salt)
    }
}

/// Authenticated ChaCha20-Poly1305 envelope wrapping a 32-byte `VaultKey`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrappedKeyEnvelope {
    pub v: u8,
    pub nonce: String,
    pub ct: String,
}

/// Random 256-bit per-account Data Encryption Key (DEK) used for all synced rows and attachments.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct VaultKey([u8; KEY_LEN]);

impl fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VaultKey([REDACTED])")
    }
}

impl PartialEq for VaultKey {
    fn eq(&self, other: &Self) -> bool {
        bool::from(self.0.ct_eq(&other.0))
    }
}

impl Eq for VaultKey {}

impl VaultKey {
    pub fn generate() -> Result<Self, CloudCryptoError> {
        let mut bytes = [0u8; KEY_LEN];
        getrandom::getrandom(&mut bytes)
            .map_err(|err| CloudCryptoError::RandomFailed(err.to_string()))?;
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    pub fn to_e2ee_key(&self) -> E2eeKey {
        E2eeKey::from_raw_bytes(self.0)
    }

    pub fn to_base64(&self) -> Zeroizing<String> {
        Zeroizing::new(B64URL.encode(self.0))
    }

    pub fn from_base64(encoded: &str) -> Result<Self, CloudCryptoError> {
        let bytes = B64URL
            .decode(encoded.trim())
            .map_err(|err| CloudCryptoError::MalformedEnvelope(format!("invalid vault key b64: {err}")))?;
        let arr: [u8; KEY_LEN] = bytes
            .try_into()
            .map_err(|_| CloudCryptoError::MalformedEnvelope("vault key must be 32 bytes".into()))?;
        Ok(Self(arr))
    }
}

/// 256-bit key-encryption key derived from the user's password or recovery key (never sent to server).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct KeyEncryptionKey([u8; KEY_LEN]);

impl fmt::Debug for KeyEncryptionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeyEncryptionKey([REDACTED])")
    }
}

/// Client-derived login material produced by Argon2id + HKDF.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct LoginMaterial {
    /// Sent over TLS to `/api/v1/auth/login` (server hashes again with Argon2id).
    pub auth_key: String,
    /// Stays strictly on the client to unwrap `wrapped_vault_key`.
    pub kek: KeyEncryptionKey,
}

impl fmt::Debug for LoginMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LoginMaterial([REDACTED])")
    }
}

/// 256-bit zero-knowledge Recovery Key formatted in Crockford Base32 with a 16-bit SHA-256 checksum.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct RecoveryKey([u8; KEY_LEN]);

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryKey([REDACTED])")
    }
}

impl PartialEq for RecoveryKey {
    fn eq(&self, other: &Self) -> bool {
        bool::from(self.0.ct_eq(&other.0))
    }
}

impl Eq for RecoveryKey {}

impl RecoveryKey {
    pub fn generate() -> Result<Self, CloudCryptoError> {
        let mut bytes = [0u8; KEY_LEN];
        getrandom::getrandom(&mut bytes)
            .map_err(|err| CloudCryptoError::RandomFailed(err.to_string()))?;
        Ok(Self(bytes))
    }

    /// Formats as `FERN-XXXXX-XXXXX-...` (11 groups of 5 Crockford Base32 chars = 272 bits: 256-bit key + 16-bit checksum).
    pub fn to_formatted_string(&self) -> Zeroizing<String> {
        let checksum = recovery_checksum(&self.0);
        let mut payload = [0u8; KEY_LEN + 2];
        payload[..KEY_LEN].copy_from_slice(&self.0);
        payload[KEY_LEN..].copy_from_slice(&checksum);
        let encoded = encode_crockford_base32(&payload);
        payload.zeroize();

        let mut out = String::with_capacity(RECOVERY_PREFIX.len() + encoded.len() + 12);
        out.push_str(RECOVERY_PREFIX);
        for chunk in encoded.as_bytes().chunks(5) {
            out.push('-');
            out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        }
        Zeroizing::new(out)
    }

    /// Parses a user-entered recovery key (case-insensitive, ignores spaces/hyphens, normalizes
    /// Crockford confusables `O->0`, `I/L->1`, and verifies the 16-bit SHA-256 checksum).
    pub fn parse(input: &str) -> Result<Self, CloudCryptoError> {
        let cleaned: String = input
            .chars()
            .filter(|c| !c.is_ascii_whitespace() && *c != '-')
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let body = cleaned.strip_prefix(RECOVERY_PREFIX).unwrap_or(&cleaned);
        let mut payload = decode_crockford_base32(body)?;
        if payload.len() != KEY_LEN + 2 {
            payload.zeroize();
            return Err(CloudCryptoError::InvalidRecoveryKey(
                "recovery key length is invalid".into(),
            ));
        }
        let mut key_bytes = [0u8; KEY_LEN];
        key_bytes.copy_from_slice(&payload[..KEY_LEN]);
        let expected = recovery_checksum(&key_bytes);
        let valid = bool::from(payload[KEY_LEN..].ct_eq(&expected));
        payload.zeroize();
        if !valid {
            key_bytes.zeroize();
            return Err(CloudCryptoError::InvalidRecoveryKey(
                "recovery key checksum mismatch (check for typos)".into(),
            ));
        }
        Ok(Self(key_bytes))
    }

    /// Derives `(recovery_auth_key_b64, recovery_kek)` bound to `kdf_salt`.
    pub fn derive_keys(&self, kdf_salt: &[u8]) -> (String, KeyEncryptionKey) {
        let auth = hkdf_sha256_32(&self.0, kdf_salt, HKDF_INFO_RECOVERY_AUTH);
        let kek = hkdf_sha256_32(&self.0, kdf_salt, HKDF_INFO_RECOVERY_KEK);
        (B64URL.encode(auth), KeyEncryptionKey(kek))
    }
}

fn recovery_checksum(bytes: &[u8; KEY_LEN]) -> [u8; 2] {
    let mut hasher = Sha256::new();
    hasher.update(b"fern-recovery-cksum-v1:");
    hasher.update(bytes);
    let digest = hasher.finalize();
    [digest[0], digest[1]]
}

fn encode_crockford_base32(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() * 8).div_ceil(5));
    let mut buffer: u32 = 0;
    let mut bits: u8 = 0;
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let idx = ((buffer >> bits) & 0x1F) as usize;
            out.push(CROCKFORD_ALPHABET[idx] as char);
        }
    }
    if bits > 0 {
        let idx = ((buffer << (5 - bits)) & 0x1F) as usize;
        out.push(CROCKFORD_ALPHABET[idx] as char);
    }
    out
}

fn decode_crockford_base32(input: &str) -> Result<Vec<u8>, CloudCryptoError> {
    if input.is_empty() {
        return Err(CloudCryptoError::InvalidRecoveryKey("recovery key is empty".into()));
    }
    let mut out = Vec::with_capacity((input.len() * 5) / 8);
    let mut buffer: u32 = 0;
    let mut bits: u8 = 0;
    for ch in input.chars() {
        let val: u32 = match ch {
            '0' | 'O' => 0,
            '1' | 'I' | 'L' => 1,
            '2'..='9' => u32::from(ch as u8 - b'0'),
            'A'..='H' => u32::from(ch as u8 - b'A') + 10,
            'J' | 'K' => u32::from(ch as u8 - b'J') + 18,
            'M' | 'N' => u32::from(ch as u8 - b'M') + 20,
            'P'..='T' => u32::from(ch as u8 - b'P') + 22,
            'V'..='Z' => u32::from(ch as u8 - b'V') + 27,
            _ => {
                return Err(CloudCryptoError::InvalidRecoveryKey(format!(
                    "unexpected character {ch:?}"
                )));
            }
        };
        buffer = (buffer << 5) | val;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    // Reject non-zero trailing pad bits so malformed encodings fail cleanly.
    if bits > 0 && (buffer & ((1u32 << bits) - 1)) != 0 {
        return Err(CloudCryptoError::InvalidRecoveryKey(
            "invalid trailing padding in recovery key".into(),
        ));
    }
    Ok(out)
}

/// RFC 5869 HKDF-SHA256 producing 32 output bytes.
pub fn hkdf_sha256_32(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; KEY_LEN] {
    // Extract: PRK = HMAC-SHA256(salt, IKM)
    let mut extract =
        <HmacSha256 as Mac>::new_from_slice(salt).expect("HMAC accepts any key length");
    extract.update(ikm);
    let prk = extract.finalize().into_bytes();

    // Expand (L = 32 = HashLen => single block T(1) = HMAC-SHA256(PRK, info || 0x01))
    let mut expand =
        <HmacSha256 as Mac>::new_from_slice(&prk).expect("HMAC accepts any key length");
    expand.update(info);
    expand.update(&[0x01]);
    let okm = expand.finalize().into_bytes();
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(&okm);
    out
}

fn derive_master_key(password: &str, kdf: &KdfParams) -> Result<Zeroizing<[u8; KEY_LEN]>, CloudCryptoError> {
    let salt = kdf.validate_and_decode_salt()?;
    let params = Params::new(kdf.m_cost, kdf.t_cost, kdf.p_cost, Some(KEY_LEN))
        .map_err(|err| CloudCryptoError::InvalidKdfParams(err.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    argon2
        .hash_password_into(password.as_bytes(), &salt, &mut *out)
        .map_err(|err| CloudCryptoError::InvalidKdfParams(err.to_string()))?;
    Ok(out)
}

/// Derives `(auth_key, kek)` from a pre-computed 32-byte Argon2id master key (e.g. computed in a
/// browser Web Worker off the UI thread).
pub fn derive_login_material_from_master_key(
    master: &[u8; KEY_LEN],
    kdf: &KdfParams,
) -> Result<LoginMaterial, CloudCryptoError> {
    let salt = kdf.validate_and_decode_salt()?;
    let auth_bytes = hkdf_sha256_32(master, &salt, HKDF_INFO_AUTH);
    let kek_bytes = hkdf_sha256_32(master, &salt, HKDF_INFO_KEK);
    Ok(LoginMaterial {
        auth_key: B64URL.encode(auth_bytes),
        kek: KeyEncryptionKey(kek_bytes),
    })
}

/// Runs Argon2id + HKDF-SHA256 to derive `(auth_key, kek)` from a password and account `KdfParams`.
pub fn derive_login_material(
    password: &str,
    kdf: &KdfParams,
) -> Result<LoginMaterial, CloudCryptoError> {
    let master = derive_master_key(password, kdf)?;
    derive_login_material_from_master_key(&master, kdf)
}

fn wrap_vault_key_with_aad(
    kek: &KeyEncryptionKey,
    vault_key: &VaultKey,
    aad: &[u8],
) -> Result<WrappedKeyEnvelope, CloudCryptoError> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce)
        .map_err(|err| CloudCryptoError::RandomFailed(err.to_string()))?;
    let cipher = ChaCha20Poly1305::new((&kek.0).into());
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: vault_key.as_bytes(),
                aad,
            },
        )
        .map_err(|_| CloudCryptoError::MalformedEnvelope("vault key wrap failed".into()))?;
    Ok(WrappedKeyEnvelope {
        v: WRAP_VERSION,
        nonce: B64URL.encode(nonce),
        ct: B64URL.encode(ct),
    })
}

fn unwrap_vault_key_with_aad(
    kek: &KeyEncryptionKey,
    envelope: &WrappedKeyEnvelope,
    aad: &[u8],
) -> Result<VaultKey, CloudCryptoError> {
    if envelope.v != WRAP_VERSION {
        return Err(CloudCryptoError::MalformedEnvelope(format!(
            "unsupported wrapped key version {}",
            envelope.v
        )));
    }
    let nonce = B64URL
        .decode(&envelope.nonce)
        .map_err(|err| CloudCryptoError::MalformedEnvelope(format!("invalid nonce: {err}")))?;
    if nonce.len() != NONCE_LEN {
        return Err(CloudCryptoError::MalformedEnvelope("invalid nonce length".into()));
    }
    let ct = B64URL
        .decode(&envelope.ct)
        .map_err(|err| CloudCryptoError::MalformedEnvelope(format!("invalid ciphertext: {err}")))?;
    let cipher = ChaCha20Poly1305::new((&kek.0).into());
    let mut plain = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload { msg: &ct, aad },
        )
        .map_err(|_| CloudCryptoError::DecryptionFailed)?;
    if plain.len() != KEY_LEN {
        plain.zeroize();
        return Err(CloudCryptoError::MalformedEnvelope(
            "decrypted vault key has wrong length".into(),
        ));
    }
    let mut arr = [0u8; KEY_LEN];
    arr.copy_from_slice(&plain);
    plain.zeroize();
    Ok(VaultKey(arr))
}

fn password_wrap_aad(email_normalized: &str) -> String {
    format!("fern-vault-key-v1:{email_normalized}")
}

fn recovery_wrap_aad(email_normalized: &str) -> String {
    format!("fern-recovery-key-v1:{email_normalized}")
}

/// Wraps `vault_key` using the password-derived `kek`, bound to `email`.
pub fn wrap_vault_key(
    email: &str,
    kek: &KeyEncryptionKey,
    vault_key: &VaultKey,
) -> Result<WrappedKeyEnvelope, CloudCryptoError> {
    let norm = normalize_email(email);
    wrap_vault_key_with_aad(kek, vault_key, password_wrap_aad(&norm).as_bytes())
}

/// Unwraps `wrapped_vault_key` using the password-derived `kek`, verifying AAD binding to `email`.
pub fn unwrap_vault_key(
    email: &str,
    kek: &KeyEncryptionKey,
    envelope: &WrappedKeyEnvelope,
) -> Result<VaultKey, CloudCryptoError> {
    let norm = normalize_email(email);
    unwrap_vault_key_with_aad(kek, envelope, password_wrap_aad(&norm).as_bytes())
}

/// Wraps `vault_key` using a `RecoveryKey` and returns `(recovery_auth_key, recovery_wrapped_vault_key)`.
pub fn wrap_vault_key_for_recovery(
    email: &str,
    kdf_salt: &[u8],
    recovery_key: &RecoveryKey,
    vault_key: &VaultKey,
) -> Result<(String, WrappedKeyEnvelope), CloudCryptoError> {
    let norm = normalize_email(email);
    let (recovery_auth_key, recovery_kek) = recovery_key.derive_keys(kdf_salt);
    let wrapped = wrap_vault_key_with_aad(&recovery_kek, vault_key, recovery_wrap_aad(&norm).as_bytes())?;
    Ok((recovery_auth_key, wrapped))
}

/// Unwraps `recovery_wrapped_vault_key` using a user-provided `RecoveryKey`.
pub fn unwrap_vault_key_with_recovery(
    email: &str,
    kdf_salt: &[u8],
    recovery_key: &RecoveryKey,
    envelope: &WrappedKeyEnvelope,
) -> Result<VaultKey, CloudCryptoError> {
    let norm = normalize_email(email);
    let (_, recovery_kek) = recovery_key.derive_keys(kdf_salt);
    unwrap_vault_key_with_aad(&recovery_kek, envelope, recovery_wrap_aad(&norm).as_bytes())
}

/// Client device identity sent during registration, login, and account recovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceMetadata {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
}

/// Output of [`prepare_registration`]: the HTTP request payload plus the client-local secrets.
pub struct PreparedRegistration {
    pub request: RegisterRequest,
    pub vault_key: VaultKey,
    pub recovery_key: RecoveryKey,
}

/// Generates a fresh zero-knowledge vault, recovery key, and registration request using already
/// derived [`LoginMaterial`] (allowing Argon2id to run in a background Web Worker).
pub fn prepare_registration_with_material(
    email: &str,
    display_name: &str,
    kdf: KdfParams,
    login: &LoginMaterial,
    device: DeviceMetadata,
    invite_code: Option<String>,
) -> Result<PreparedRegistration, CloudCryptoError> {
    let email = validate_email(email)?;
    let display_name = sanitize_display_name(display_name);
    let salt = kdf.validate_and_decode_salt()?;
    let vault_key = VaultKey::generate()?;
    let wrapped_vault_key = wrap_vault_key(&email, &login.kek, &vault_key)?;
    let recovery_key = RecoveryKey::generate()?;
    let (recovery_auth_key, recovery_wrapped_vault_key) =
        wrap_vault_key_for_recovery(&email, &salt, &recovery_key, &vault_key)?;

    Ok(PreparedRegistration {
        request: RegisterRequest {
            email,
            display_name,
            auth_key: login.auth_key.clone(),
            kdf,
            wrapped_vault_key,
            recovery_wrapped_vault_key,
            recovery_auth_key,
            device_id: device.device_id,
            device_name: device.device_name,
            platform: device.platform,
            invite_code: invite_code.filter(|s| !s.trim().is_empty()),
        },
        vault_key,
        recovery_key,
    })
}

/// Generates a fresh zero-knowledge vault, recovery key, and registration request on the client.
pub fn prepare_registration(
    email: &str,
    display_name: &str,
    password: &str,
    device: DeviceMetadata,
    invite_code: Option<String>,
) -> Result<PreparedRegistration, CloudCryptoError> {
    let email = validate_email(email)?;
    validate_password(password)?;
    let kdf = KdfParams::generate_default()?;
    let login = derive_login_material(password, &kdf)?;
    prepare_registration_with_material(&email, display_name, kdf, &login, device, invite_code)
}

/// Output of [`prepare_password_change`]: request payload + newly rotated recovery key bound to new KDF salt.
pub struct PreparedPasswordChange {
    pub request: ChangePasswordRequest,
    pub new_recovery_key: RecoveryKey,
}

/// Re-wraps the existing `vault_key` using pre-derived [`LoginMaterial`] pairs (supports Web Worker KDF).
pub fn prepare_password_change_with_materials(
    email: &str,
    current: &LoginMaterial,
    new_kdf: KdfParams,
    next: &LoginMaterial,
    vault_key: &VaultKey,
) -> Result<PreparedPasswordChange, CloudCryptoError> {
    let email = validate_email(email)?;
    let new_salt = new_kdf.validate_and_decode_salt()?;
    let new_wrapped_vault_key = wrap_vault_key(&email, &next.kek, vault_key)?;
    let new_recovery_key = RecoveryKey::generate()?;
    let (new_recovery_auth_key, new_recovery_wrapped_vault_key) =
        wrap_vault_key_for_recovery(&email, &new_salt, &new_recovery_key, vault_key)?;

    Ok(PreparedPasswordChange {
        request: ChangePasswordRequest {
            current_auth_key: current.auth_key.clone(),
            new_auth_key: next.auth_key.clone(),
            new_kdf,
            new_wrapped_vault_key,
            new_recovery_wrapped_vault_key,
            new_recovery_auth_key,
        },
        new_recovery_key,
    })
}

/// Re-wraps the existing `vault_key` under a new password (with a fresh KDF salt) without touching
/// any synced rows. Also rotates the recovery key cleanly under the new salt.
pub fn prepare_password_change(
    email: &str,
    current_password: &str,
    current_kdf: &KdfParams,
    new_password: &str,
    vault_key: &VaultKey,
) -> Result<PreparedPasswordChange, CloudCryptoError> {
    let email = validate_email(email)?;
    validate_password(new_password)?;
    let current = derive_login_material(current_password, current_kdf)?;
    let new_kdf = KdfParams::generate_default()?;
    let next = derive_login_material(new_password, &new_kdf)?;
    prepare_password_change_with_materials(&email, &current, new_kdf, &next, vault_key)
}

/// Output of [`prepare_recovery_complete`]: request payload + unlocked `VaultKey` + fresh `RecoveryKey`.
pub struct PreparedRecoveryComplete {
    pub request: RecoverCompleteRequest,
    pub vault_key: VaultKey,
    pub new_recovery_key: RecoveryKey,
}

/// Completes zero-knowledge account recovery using pre-derived [`LoginMaterial`] for the new password.
#[allow(clippy::too_many_arguments)]
pub fn prepare_recovery_complete_with_material(
    email: &str,
    current_kdf: &KdfParams,
    recovery_key: &RecoveryKey,
    recovery_wrapped_vault_key: &WrappedKeyEnvelope,
    recovery_challenge_token: String,
    new_kdf: KdfParams,
    next: &LoginMaterial,
    device: DeviceMetadata,
) -> Result<PreparedRecoveryComplete, CloudCryptoError> {
    let email = validate_email(email)?;
    let current_salt = current_kdf.validate_and_decode_salt()?;
    let vault_key = unwrap_vault_key_with_recovery(
        &email,
        &current_salt,
        recovery_key,
        recovery_wrapped_vault_key,
    )?;

    let new_salt = new_kdf.validate_and_decode_salt()?;
    let new_wrapped_vault_key = wrap_vault_key(&email, &next.kek, &vault_key)?;
    let new_recovery_key = RecoveryKey::generate()?;
    let (new_recovery_auth_key, new_recovery_wrapped_vault_key) =
        wrap_vault_key_for_recovery(&email, &new_salt, &new_recovery_key, &vault_key)?;

    Ok(PreparedRecoveryComplete {
        request: RecoverCompleteRequest {
            email,
            recovery_challenge_token,
            new_auth_key: next.auth_key.clone(),
            new_kdf,
            new_wrapped_vault_key,
            new_recovery_wrapped_vault_key,
            new_recovery_auth_key,
            device_id: device.device_id,
            device_name: device.device_name,
            platform: device.platform,
        },
        vault_key,
        new_recovery_key,
    })
}

/// Completes zero-knowledge account recovery by unwrapping `vault_key` with `recovery_key`,
/// re-wrapping it under `new_password`, and rotating to a brand-new recovery key.
pub fn prepare_recovery_complete(
    email: &str,
    current_kdf: &KdfParams,
    recovery_key: &RecoveryKey,
    recovery_wrapped_vault_key: &WrappedKeyEnvelope,
    recovery_challenge_token: String,
    new_password: &str,
    device: DeviceMetadata,
) -> Result<PreparedRecoveryComplete, CloudCryptoError> {
    let email = validate_email(email)?;
    validate_password(new_password)?;
    let new_kdf = KdfParams::generate_default()?;
    let next = derive_login_material(new_password, &new_kdf)?;
    prepare_recovery_complete_with_material(
        &email,
        current_kdf,
        recovery_key,
        recovery_wrapped_vault_key,
        recovery_challenge_token,
        new_kdf,
        &next,
        device,
    )
}

// ---------------------------------------------------------------------------
// Shared API DTOs (used by fern-cloud server, desktop_sync, and fern-web)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreloginRequest {
    pub email: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreloginResponse {
    pub kdf: KdfParams,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub display_name: String,
    pub auth_key: String,
    pub kdf: KdfParams,
    pub wrapped_vault_key: WrappedKeyEnvelope,
    pub recovery_wrapped_vault_key: WrappedKeyEnvelope,
    pub recovery_auth_key: String,
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub auth_key: String,
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefreshRequest {
    /// Required for desktop clients; omitted on web where the `HttpOnly` cookie carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    /// Unix timestamp in seconds when `access_token` expires.
    pub access_expires_at: i64,
    /// Present for native desktop clients; `None` for browser clients using `HttpOnly` cookies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultBundle {
    pub kdf: KdfParams,
    pub wrapped_vault_key: WrappedKeyEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountProfile {
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    pub created_at: i64,
    pub storage_used_bytes: u64,
    pub storage_quota_bytes: u64,
    pub synced_rows_count: u64,
    pub max_rows_quota: u64,
    pub device_count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthResponse {
    pub session_id: String,
    pub account: AccountProfile,
    pub vault: VaultBundle,
    pub tokens: TokenPair,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefreshResponse {
    pub session_id: String,
    pub account: AccountProfile,
    pub vault: VaultBundle,
    pub tokens: TokenPair,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSessionInfo {
    pub session_id: String,
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub ip_hint: String,
    pub is_current: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: i64,
    pub event_type: String,
    pub device_name: String,
    pub platform: String,
    pub ip_hint: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateAccountRequest {
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_auth_key: String,
    pub new_auth_key: String,
    pub new_kdf: KdfParams,
    pub new_wrapped_vault_key: WrappedKeyEnvelope,
    pub new_recovery_wrapped_vault_key: WrappedKeyEnvelope,
    pub new_recovery_auth_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateRecoveryKeyRequest {
    pub current_auth_key: String,
    pub new_recovery_wrapped_vault_key: WrappedKeyEnvelope,
    pub new_recovery_auth_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoverInitRequest {
    pub email: String,
    pub recovery_auth_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoverInitResponse {
    pub kdf: KdfParams,
    pub recovery_wrapped_vault_key: WrappedKeyEnvelope,
    pub recovery_challenge_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoverCompleteRequest {
    pub email: String,
    pub recovery_challenge_token: String,
    pub new_auth_key: String,
    pub new_kdf: KdfParams,
    pub new_wrapped_vault_key: WrappedKeyEnvelope,
    pub new_recovery_wrapped_vault_key: WrappedKeyEnvelope,
    pub new_recovery_auth_key: String,
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteAccountRequest {
    pub current_auth_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventsTicketResponse {
    pub ticket: String,
    pub expires_in_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncEventNotification {
    pub position: i64,
    pub from_device: String,
    pub server_time: i64,
}

/// Persisted active Fern Cloud session (stored ONLY in OS-protected `cloud-session.vault` on
/// desktop or WebCrypto-wrapped IndexedDB on web — NEVER inside `tasks.db`).
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct CloudSessionState {
    pub server_url: String,
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    pub session_id: String,
    pub access_token: String,
    pub access_expires_at: i64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Base64url-encoded 32-byte unlocked vault DEK.
    pub vault_key_b64: String,
    #[zeroize(skip)]
    pub kdf: KdfParams,
}

impl fmt::Debug for CloudSessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloudSessionState")
            .field("server_url", &self.server_url)
            .field("user_id", &self.user_id)
            .field("email", &self.email)
            .field("session_id", &self.session_id)
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "[REDACTED]"))
            .field("vault_key_b64", &"[REDACTED]")
            .finish()
    }
}

impl CloudSessionState {
    pub fn vault_key(&self) -> Result<VaultKey, CloudCryptoError> {
        VaultKey::from_base64(&self.vault_key_b64)
    }

    pub fn e2ee_key(&self) -> Result<E2eeKey, CloudCryptoError> {
        Ok(self.vault_key()?.to_e2ee_key())
    }

    pub fn meta(&self) -> CloudMeta {
        CloudMeta {
            server_url: self.server_url.clone(),
            user_id: self.user_id.clone(),
            email: self.email.clone(),
            display_name: self.display_name.clone(),
        }
    }

    /// Returns `true` if the access token expires within `leeway_secs` seconds.
    pub fn is_access_expiring(&self, now_unix_secs: i64, leeway_secs: i64) -> bool {
        self.access_expires_at <= now_unix_secs + leeway_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_and_password_validation() {
        assert_eq!(validate_email("  Alice@Example.COM ").unwrap(), "alice@example.com");
        assert!(validate_email("").is_err());
        assert!(validate_email("not-an-email").is_err());
        assert!(validate_email("a@b").is_err());
        assert!(validate_email("a@.com").is_err());
        assert!(validate_email("a@b..com").is_err());
        assert!(validate_email("a b@example.com").is_err());

        assert!(validate_password("short").is_err());
        assert!(validate_password("123456789").is_err());
        assert!(validate_password("1234567890").is_ok());
        assert!(password_strength_score("a") < password_strength_score("Correct-Horse-99!"));
    }

    #[test]
    fn kdf_and_hkdf_produce_independent_keys_and_enforce_bounds() {
        let kdf = KdfParams {
            alg: KDF_ALG_ARGON2ID_V1.into(),
            m_cost: 4_096,
            t_cost: 1,
            p_cost: 1,
            salt: B64URL.encode([7u8; 16]),
        };
        let mat1 = derive_login_material("correct horse battery", &kdf).unwrap();
        let mat2 = derive_login_material("correct horse battery", &kdf).unwrap();
        assert_eq!(mat1.auth_key, mat2.auth_key);
        assert_eq!(mat1.kek.0, mat2.kek.0);
        assert_ne!(B64URL.decode(&mat1.auth_key).unwrap().as_slice(), &mat1.kek.0);

        let mut bad_kdf = kdf.clone();
        bad_kdf.m_cost = 999_999;
        assert!(bad_kdf.validate_and_decode_salt().is_err());
        let mut weak_kdf = kdf;
        weak_kdf.m_cost = 1024;
        assert!(weak_kdf.validate_and_decode_salt().is_err());
    }

    #[test]
    fn recovery_key_format_typo_tolerance_and_checksum() {
        let key = RecoveryKey::generate().unwrap();
        let formatted = key.to_formatted_string();
        assert!(formatted.starts_with("FERN-"));
        assert_eq!(RecoveryKey::parse(&formatted).unwrap(), key);

        // Lowercase, missing dashes, spaces, and Crockford confusables (o->0, i/l->1) parse identically.
        let sloppy = formatted
            .to_lowercase()
            .replace('-', " ")
            .replace('0', "o")
            .replace('1', "l");
        assert_eq!(RecoveryKey::parse(&sloppy).unwrap(), key);

        // Flipping any character in the body must fail checksum validation.
        let mut tampered = formatted.to_string();
        let bytes = unsafe { tampered.as_bytes_mut() };
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
        assert!(matches!(
            RecoveryKey::parse(&tampered),
            Err(CloudCryptoError::InvalidRecoveryKey(_))
        ));
    }

    #[test]
    fn registration_login_password_change_and_recovery_preserve_vault_key() {
        let prep = prepare_registration(
            "User@Fern.App",
            "Alice",
            "initial-strong-passphrase-1",
            DeviceMetadata {
                device_id: "dev-1".into(),
                device_name: "Workstation".into(),
                platform: "windows".into(),
            },
            None,
        )
        .unwrap();

        // Login unwraps the exact same VaultKey.
        let login = derive_login_material("initial-strong-passphrase-1", &prep.request.kdf).unwrap();
        assert_eq!(login.auth_key, prep.request.auth_key);
        let unwrapped = unwrap_vault_key("user@fern.app", &login.kek, &prep.request.wrapped_vault_key).unwrap();
        assert_eq!(unwrapped, prep.vault_key);

        // Wrong password or wrong email (AAD mismatch) must fail decryption.
        let wrong_login = derive_login_material("wrong-passphrase-999", &prep.request.kdf).unwrap();
        assert_eq!(
            unwrap_vault_key("user@fern.app", &wrong_login.kek, &prep.request.wrapped_vault_key).unwrap_err(),
            CloudCryptoError::DecryptionFailed
        );
        assert_eq!(
            unwrap_vault_key("attacker@fern.app", &login.kek, &prep.request.wrapped_vault_key).unwrap_err(),
            CloudCryptoError::DecryptionFailed
        );

        // Encrypt a task row with the original VaultKey.
        let e2ee = prep.vault_key.to_e2ee_key();
        let mut plain_row = serde_json::Map::new();
        plain_row.insert("title".into(), serde_json::Value::from("Ship Fern Cloud"));
        let encrypted_row = e2ee.encrypt_row("tasks", "t1", &plain_row).unwrap();

        // Change password -> re-wraps DEK without re-encrypting the task row!
        let changed = prepare_password_change(
            "user@fern.app",
            "initial-strong-passphrase-1",
            &prep.request.kdf,
            "brand-new-strong-password-2",
            &prep.vault_key,
        )
        .unwrap();
        assert_eq!(changed.request.current_auth_key, prep.request.auth_key);
        assert_ne!(changed.request.new_auth_key, prep.request.auth_key);
        assert_ne!(changed.request.new_kdf.salt, prep.request.kdf.salt);

        let login2 = derive_login_material("brand-new-strong-password-2", &changed.request.new_kdf).unwrap();
        let unwrapped2 = unwrap_vault_key("user@fern.app", &login2.kek, &changed.request.new_wrapped_vault_key).unwrap();
        assert_eq!(unwrapped2, prep.vault_key);
        assert_eq!(unwrapped2.to_e2ee_key().decrypt_row("tasks", "t1", &encrypted_row).unwrap(), plain_row);

        // Recover account using the rotated RecoveryKey -> still decrypts original task row!
        let recovered = prepare_recovery_complete(
            "user@fern.app",
            &changed.request.new_kdf,
            &changed.new_recovery_key,
            &changed.request.new_recovery_wrapped_vault_key,
            "challenge-tok".into(),
            "third-password-after-recovery",
            DeviceMetadata {
                device_id: "dev-2".into(),
                device_name: "Web".into(),
                platform: "web".into(),
            },
        )
        .unwrap();
        assert_eq!(recovered.vault_key, prep.vault_key);
        assert_eq!(
            recovered.vault_key.to_e2ee_key().decrypt_row("tasks", "t1", &encrypted_row).unwrap(),
            plain_row
        );
    }

    #[test]
    fn cloud_meta_initials_and_url_normalization() {
        let meta = CloudMeta {
            server_url: "https://fern.atabits.com".into(),
            user_id: "u1".into(),
            email: "amin.atabiev@example.com".into(),
            display_name: "Amin Atabiev".into(),
        };
        assert_eq!(meta.initials(), "AA");
        let email_only = CloudMeta { display_name: String::new(), ..meta };
        assert_eq!(email_only.initials(), "AA");
        assert_eq!(normalize_cloud_url("https://fern.atabits.com/api/v1/"), "https://fern.atabits.com");
        assert_eq!(normalize_cloud_url("  https://cloud.example.org/  "), "https://cloud.example.org");
    }
}
