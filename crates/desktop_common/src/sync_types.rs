//! HTTP-free sync status and error classification helpers shared across desktop crates
//! without pulling the HTTP client (`ureq`) into every UI crate.

use chrono::NaiveDateTime;
use fern_core::attachment::{AttachmentTooLarge, MAX_ATTACHMENT_BYTES};
use fern_core::cloud::CloudMeta;
use fern_core::e2ee::E2eeError;
use fern_core::sync::IncompatibleProtocol;

/// How the last sync went, for the sidebar's sync button and diagnostics report.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SyncStatus {
    /// No server set.
    #[default]
    Off,
    Running,
    Done(NaiveDateTime),
    Failed(String),
}

/// Live connection state of the Fern Cloud real-time SSE push channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RealtimeStatus {
    #[default]
    Disconnected,
    Connecting,
    Live,
    /// Access/refresh credentials were revoked or expired; client stopped reconnecting until re-login.
    SessionExpired,
}

/// Structured Fern Cloud API error (mirroring Zed's `ClientApiError`) so Desktop and Web can
/// cleanly distinguish permanent revocation (`401`), transient refresh race (`409`), rate limits
/// (`429`), protocol upgrades (`426`), and network failures without parsing error strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloudApiError {
    /// Permanent authentication failure (revoked device, expired refresh token, or token reuse).
    Unauthorized(String),
    /// No local session is unlocked on this device.
    NotSignedIn,
    /// Local session belongs to a different user than expected.
    AccountMismatch,
    /// Benign refresh rotation race inside the server's 15s grace window (`409 refresh_race`).
    GraceConflict,
    /// Server rate limit (`429 Too Many Requests`).
    RateLimited {
        retry_after_secs: u64,
        message: String,
    },
    /// Transport/connectivity error (DNS, TCP, TLS, timeout).
    Network(String),
    /// Non-success HTTP response other than 401/409-grace/426/429.
    Server { status: u16, message: String },
    /// Malformed JSON or unexpected payload from server.
    InvalidResponse(String),
}

impl std::fmt::Display for CloudApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized(msg) => write!(f, "{msg}"),
            Self::NotSignedIn => write!(
                f,
                "Fern Cloud session not unlocked on this device — please sign in again in Settings → Data & Sync"
            ),
            Self::AccountMismatch => {
                write!(f, "Fern Cloud session account mismatch — please sign in again")
            }
            Self::GraceConflict => {
                write!(f, "Session was just refreshed by another request; retrying")
            }
            Self::RateLimited {
                retry_after_secs,
                message,
            } => write!(f, "{message} (retry in {retry_after_secs}s)"),
            Self::Network(msg) => write!(f, "Network error: {msg}"),
            Self::Server { message, .. } => write!(f, "{message}"),
            Self::InvalidResponse(msg) => write!(f, "Invalid server response: {msg}"),
        }
    }
}

impl std::error::Error for CloudApiError {}

/// Returns the underlying [`CloudApiError`] if `err` (or its source chain) carries one.
pub fn cloud_api_error(err: &anyhow::Error) -> Option<&CloudApiError> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<CloudApiError>())
}

/// Returns `true` when `err` represents a permanently revoked/expired cloud session (`401` after refresh).
pub fn is_session_expired(err: &anyhow::Error) -> bool {
    matches!(cloud_api_error(err), Some(CloudApiError::Unauthorized(_)))
}

/// Returns `true` when `err` is a benign server-side refresh grace conflict (`409 refresh_race`).
pub fn is_grace_conflict(err: &anyhow::Error) -> bool {
    matches!(cloud_api_error(err), Some(CloudApiError::GraceConflict))
}

/// Returns the `Retry-After` delay in seconds if `err` is a `429 RateLimited` response.
pub fn rate_limit_retry_after(err: &anyhow::Error) -> Option<u64> {
    match cloud_api_error(err) {
        Some(CloudApiError::RateLimited {
            retry_after_secs, ..
        }) => Some(*retry_after_secs),
        _ => None,
    }
}

/// Reactive UI snapshot of the connected Fern Cloud account, stored as a GPUI [`gpui::Global`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CloudSyncSnapshot {
    pub signed_in: bool,
    pub server_url: String,
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    pub initials: String,
    pub realtime: RealtimeStatus,
    pub storage_used_bytes: u64,
    pub storage_quota_bytes: u64,
    pub synced_rows_count: u64,
    pub max_rows_quota: u64,
    pub device_count: u64,
}

impl CloudSyncSnapshot {
    pub fn from_meta(meta: &CloudMeta) -> Self {
        Self {
            signed_in: meta.is_configured(),
            server_url: meta.server_url.clone(),
            user_id: meta.user_id.clone(),
            email: meta.email.clone(),
            display_name: meta.display_name.clone(),
            initials: meta.initials(),
            ..Self::default()
        }
    }
}

/// Global wrapper for [`CloudSyncSnapshot`] observed by sidebar, account menu, and settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveCloudSnapshot(pub CloudSyncSnapshot);

impl gpui::Global for LiveCloudSnapshot {}

pub type CloudAttachmentOpenFn = std::rc::Rc<dyn Fn(String, String, &mut gpui::App)>;
pub type CloudAttachmentUploadFn = std::rc::Rc<dyn Fn(fern_core::Id, &mut gpui::App)>;

/// Platform callback bridge registered by environments without a local attachments directory
/// (such as Fern Web) so task attachments can be downloaded/uploaded E2EE directly against
/// Fern Cloud `/api/v1/files/{key}`.
#[derive(Clone)]
pub struct CloudAttachmentBridge {
    /// `(storage_name, display_name, cx)` — downloads, decrypts, and opens/saves a cloud blob.
    pub open: CloudAttachmentOpenFn,
    /// `(task_id, cx)` — picks a file, encrypts + uploads to Fern Cloud, and inserts the row.
    pub upload: CloudAttachmentUploadFn,
}

impl gpui::Global for CloudAttachmentBridge {}

/// Returns the underlying [`IncompatibleProtocol`] error if `err` (or its source chain) represents
/// a sync protocol version mismatch.
pub fn is_incompatible_protocol(err: &anyhow::Error) -> Option<&IncompatibleProtocol> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<IncompatibleProtocol>())
}

/// Returns the underlying [`E2eeError`] if `err` (or its source chain) represents a client-side
/// end-to-end encryption error.
pub fn is_e2ee_error(err: &anyhow::Error) -> Option<&E2eeError> {
    err.chain().find_map(|cause| cause.downcast_ref::<E2eeError>())
}

pub fn attachment_error(error: &anyhow::Error, strings: &fern_i18n::Strings) -> String {
    if error.is::<AttachmentTooLarge>() {
        strings
            .attachment_size_limit
            .replace("{limit}", &(MAX_ATTACHMENT_BYTES / (1024 * 1024)).to_string())
    } else {
        format!("{error:#}")
    }
}
