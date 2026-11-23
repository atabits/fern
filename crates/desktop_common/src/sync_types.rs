//! HTTP-free sync status and error classification helpers shared across desktop crates
//! without pulling the HTTP client (`ureq`) into every UI crate.

use chrono::NaiveDateTime;
use fern_core::attachment::{AttachmentTooLarge, MAX_ATTACHMENT_BYTES};
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
