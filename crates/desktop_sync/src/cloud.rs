//! Blocking Fern Cloud HTTP + real-time SSE client for native desktop builds.
//!
//! All network methods are blocking and must run off the GPUI UI thread (`cx.background_spawn`).
//! Every authenticated operation routes exclusively through [`CloudSessionCoordinator`] so
//! concurrent sync workers, attachment transfers, SSE reconnections, and settings views share a
//! single in-memory session and coalesce rotating refresh tokens (single-flight, Zed `Client` style).

use std::io::{BufRead as _, BufReader, Read as _};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use desktop_common::sync::{CloudApiError, is_grace_conflict};
use fern_core::attachment::{MAX_ATTACHMENT_BYTES, check_attachment_size, save_download};
use fern_core::cloud::{
    AccountProfile, AuthResponse, CLOUD_API_PREFIX, CloudSessionState, DeleteAccountRequest,
    DeviceMetadata, DeviceSessionInfo, EventsTicketResponse, LoginRequest, PreloginRequest,
    PreloginResponse, RecoverInitRequest, RecoverInitResponse, RecoveryKey, RefreshRequest,
    RefreshResponse, SyncEventNotification, UpdateRecoveryKeyRequest, derive_login_material,
    normalize_cloud_url, normalize_email, prepare_password_change, prepare_recovery_complete,
    prepare_registration, unwrap_vault_key, validate_email, validate_password,
    wrap_vault_key_for_recovery,
};
use fern_core::e2ee::{self, E2eeError};
use fern_core::sync::{
    IncompatibleProtocol, MIN_SYNC_PROTOCOL_VERSION, SYNC_PROTOCOL_VERSION, file_key,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::sync::FileTransfers;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SYNC_TIMEOUT: Duration = Duration::from_secs(45);
const SSE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const SSE_READ_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_JSON_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: u64 = 64 * 1024;
const E2EE_FILE_OVERHEAD: u64 = 64;
const TOKEN_REFRESH_LEEWAY_SECS: i64 = 60;

/// Process-wide pooled HTTP agents (reusing TLS 1.3 sessions & HTTP keep-alive connections across
/// requests, plus honoring `HTTP_PROXY` / `HTTPS_PROXY` environment settings).
fn api_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(12)))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build()
            .into()
    })
}

fn sync_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(12)))
            .timeout_global(Some(SYNC_TIMEOUT))
            .build()
            .into()
    })
}

fn sse_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(SSE_CONNECT_TIMEOUT))
            .timeout_recv_body(Some(SSE_READ_TIMEOUT))
            .build()
            .into()
    })
}

/// Process-wide single-flight Fern Cloud session & refresh coordinator (Zed `Client` pattern).
///
/// All callers (`sync_now`, `sync_files`, `fern-cloud-sse`, and `settings_ui`) read the live
/// session from this single in-memory source of truth and serialize token rotations through
/// `refresh_lock` with double-checked locking so concurrent workers never race on rotating
/// refresh tokens or trigger false-positive server-side reuse revocation.
pub struct CloudSessionCoordinator {
    cached: RwLock<Option<CloudSessionState>>,
    refresh_lock: Mutex<()>,
}

impl CloudSessionCoordinator {
    fn new() -> Self {
        Self {
            cached: RwLock::new(None),
            refresh_lock: Mutex::new(()),
        }
    }

    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<CloudSessionCoordinator> = OnceLock::new();
        INSTANCE.get_or_init(CloudSessionCoordinator::new)
    }

    pub fn load_session(&self) -> Result<Option<CloudSessionState>> {
        if let Ok(guard) = self.cached.read()
            && let Some(session) = guard.as_ref()
        {
            return Ok(Some(session.clone()));
        }
        let _lock = self.refresh_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(guard) = self.cached.read()
            && let Some(session) = guard.as_ref()
        {
            return Ok(Some(session.clone()));
        }
        let loaded = desktop_common::cloud_vault::load_default()?;
        if let Ok(mut guard) = self.cached.write() {
            *guard = loaded.clone();
        }
        Ok(loaded)
    }

    pub fn store_session(&self, session: CloudSessionState) -> Result<()> {
        let _lock = self.refresh_lock.lock().unwrap_or_else(|e| e.into_inner());
        desktop_common::cloud_vault::save_default(&session)?;
        if let Ok(mut guard) = self.cached.write() {
            *guard = Some(session);
        }
        Ok(())
    }

    pub fn clear_session(&self) -> Result<()> {
        let _lock = self.refresh_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(mut guard) = self.cached.write() {
            *guard = None;
        }
        desktop_common::cloud_vault::clear_default()
    }

    fn update_cached_profile(&self, session_id: &str, email: &str, display_name: &str) {
        if let Ok(mut guard) = self.cached.write()
            && let Some(s) = guard.as_mut()
            && s.session_id == session_id
        {
            s.email = email.to_owned();
            s.display_name = display_name.to_owned();
        }
    }

    fn refresh_singleflight_with(
        &self,
        failed_access: Option<&str>,
        now_secs: i64,
        mut do_refresh: impl FnMut(&CloudSessionState) -> Result<CloudSessionState>,
        persist: impl FnOnce(&CloudSessionState) -> Result<()>,
    ) -> Result<CloudSessionState> {
        let _lock = self.refresh_lock.lock().unwrap_or_else(|e| e.into_inner());
        let current = {
            let guard = self.cached.read().unwrap_or_else(|e| e.into_inner());
            guard.clone()
        };
        let current = match current {
            Some(s) => s,
            None => desktop_common::cloud_vault::load_default()?
                .ok_or_else(|| anyhow::Error::new(CloudApiError::NotSignedIn))?,
        };

        // Double-check: another concurrent worker may have already rotated the token while we
        // waited on `refresh_lock`. Reuse the fresh session without hitting `/auth/refresh` twice!
        if let Some(old_tok) = failed_access {
            if current.access_token != old_tok {
                return Ok(current);
            }
        } else if !current.is_access_expiring(now_secs, TOKEN_REFRESH_LEEWAY_SECS) {
            return Ok(current);
        }

        let updated = match do_refresh(&current) {
            Ok(next) => next,
            Err(err) if is_grace_conflict(&err) => {
                // Benign concurrent rotation (e.g. another local process); reload vault and retry once.
                std::thread::sleep(Duration::from_millis(150));
                let reloaded = desktop_common::cloud_vault::load_default()?
                    .unwrap_or_else(|| current.clone());
                if reloaded.access_token != current.access_token {
                    if let Ok(mut guard) = self.cached.write() {
                        *guard = Some(reloaded.clone());
                    }
                    return Ok(reloaded);
                }
                do_refresh(&reloaded)?
            }
            Err(err) => return Err(err),
        };
        persist(&updated)?;
        if let Ok(mut guard) = self.cached.write() {
            *guard = Some(updated.clone());
        }
        Ok(updated)
    }

    fn fresh_session_for(
        &self,
        expected_user_id: Option<&str>,
        agent: &ureq::Agent,
    ) -> Result<CloudSessionState> {
        let session = self
            .load_session()?
            .ok_or_else(|| anyhow::Error::new(CloudApiError::NotSignedIn))?;
        if let Some(uid) = expected_user_id
            && session.user_id != uid
        {
            return Err(CloudApiError::AccountMismatch.into());
        }
        let now = chrono::Utc::now().timestamp();
        if !session.is_access_expiring(now, TOKEN_REFRESH_LEEWAY_SECS) {
            return Ok(session);
        }
        let updated = self.refresh_singleflight_with(
            None,
            now,
            |cur| Ok(refresh_session_with_agent(agent, cur)?.0),
            desktop_common::cloud_vault::save_default,
        )?;
        if let Some(uid) = expected_user_id
            && updated.user_id != uid
        {
            return Err(CloudApiError::AccountMismatch.into());
        }
        Ok(updated)
    }

    fn execute_authed<T>(
        &self,
        expected_user_id: Option<&str>,
        agent: &ureq::Agent,
        mut op: impl FnMut(&ureq::Agent, &CloudSessionState) -> std::result::Result<T, (u16, anyhow::Error)>,
    ) -> Result<(T, CloudSessionState)> {
        let session = self.fresh_session_for(expected_user_id, agent)?;
        match op(agent, &session) {
            Ok(val) => Ok((val, session)),
            Err((401, _)) => {
                let failed_tok = session.access_token.clone();
                let now = chrono::Utc::now().timestamp();
                let updated = self.refresh_singleflight_with(
                    Some(&failed_tok),
                    now,
                    |cur| Ok(refresh_session_with_agent(agent, cur)?.0),
                    desktop_common::cloud_vault::save_default,
                )?;
                let val = op(agent, &updated).map_err(|(_, err)| err)?;
                Ok((val, updated))
            }
            Err((_, err)) => Err(err),
        }
    }
}

fn user_agent() -> String {
    format!("fern-desktop/{}", env!("CARGO_PKG_VERSION"))
}

pub fn device_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

pub fn default_device_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let os = match device_platform() {
        "windows" => "Windows",
        "macos" => "macOS",
        _ => "Linux",
    };
    match host {
        Some(h) => format!("{h} ({os})"),
        None => format!("Fern Desktop ({os})"),
    }
}

fn api_url(server_url: &str, path: &str) -> String {
    let base = normalize_cloud_url(server_url);
    format!("{base}{CLOUD_API_PREFIX}{path}")
}

fn read_body_limited(response: &mut ureq::http::Response<ureq::Body>, limit: u64) -> String {
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn parse_error_parts(status: u16, body: &str) -> (Option<String>, String) {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(body) {
        let code = val
            .get("error")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        if let Some(msg) = val
            .get("message")
            .or_else(|| val.get("error"))
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return (code, msg.trim().to_owned());
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        (None, format!("HTTP {status}"))
    } else {
        let snippet: String = trimmed.chars().take(200).collect();
        (None, format!("HTTP {status}: {snippet}"))
    }
}

fn classify_api_error(
    status: u16,
    retry_after_header: Option<&str>,
    body: &str,
) -> anyhow::Error {
    let (code, message) = parse_error_parts(status, body);
    match status {
        401 => CloudApiError::Unauthorized(message).into(),
        409 if code.as_deref() == Some("refresh_race") => CloudApiError::GraceConflict.into(),
        429 => {
            let from_hdr = retry_after_header.and_then(|s| s.trim().parse::<u64>().ok());
            let from_body = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v.get("retry_after_secs").and_then(serde_json::Value::as_u64));
            let retry_after_secs = from_hdr.or(from_body).unwrap_or(5).max(1);
            CloudApiError::RateLimited {
                retry_after_secs,
                message,
            }
            .into()
        }
        _ => CloudApiError::Server { status, message }.into(),
    }
}

fn send_json_request<B: Serialize, R: DeserializeOwned>(
    agent: &ureq::Agent,
    method: &str,
    url: &str,
    bearer: Option<&str>,
    body: Option<&B>,
) -> std::result::Result<R, (u16, anyhow::Error)> {
    let payload = match body {
        Some(b) => Some(serde_json::to_string(b).map_err(|e| (0, e.into()))?),
        None => None,
    };

    let response = match (method, payload) {
        ("GET", _) => {
            let mut req = agent
                .get(url)
                .config()
                .http_status_as_error(false)
                .build()
                .header("User-Agent", &user_agent())
                .header("X-Fern-Client", "desktop");
            if let Some(tok) = bearer {
                req = req.header("Authorization", &format!("Bearer {tok}"));
            }
            req.call()
        }
        ("DELETE", None) => {
            let mut req = agent
                .delete(url)
                .config()
                .http_status_as_error(false)
                .build()
                .header("User-Agent", &user_agent())
                .header("X-Fern-Client", "desktop");
            if let Some(tok) = bearer {
                req = req.header("Authorization", &format!("Bearer {tok}"));
            }
            req.call()
        }
        ("DELETE", Some(json)) => {
            let mut req = agent
                .delete(url)
                .config()
                .http_status_as_error(false)
                .build()
                .header("User-Agent", &user_agent())
                .header("X-Fern-Client", "desktop")
                .header("Content-Type", "application/json");
            if let Some(tok) = bearer {
                req = req.header("Authorization", &format!("Bearer {tok}"));
            }
            req.force_send_body().send(json)
        }
        ("PATCH", Some(json)) => {
            let mut req = agent
                .patch(url)
                .config()
                .http_status_as_error(false)
                .build()
                .header("User-Agent", &user_agent())
                .header("X-Fern-Client", "desktop")
                .header("Content-Type", "application/json");
            if let Some(tok) = bearer {
                req = req.header("Authorization", &format!("Bearer {tok}"));
            }
            req.send(json)
        }
        ("POST", maybe_json) => {
            let mut req = agent
                .post(url)
                .config()
                .http_status_as_error(false)
                .build()
                .header("User-Agent", &user_agent())
                .header("X-Fern-Client", "desktop");
            if let Some(tok) = bearer {
                req = req.header("Authorization", &format!("Bearer {tok}"));
            }
            if let Some(json) = maybe_json {
                req = req.header("Content-Type", "application/json");
                req.send(json)
            } else {
                req.send_empty()
            }
        }
        _ => return Err((0, anyhow::anyhow!("unsupported HTTP method {method}"))),
    };

    let mut response = response.map_err(|err| {
        desktop_common::diagnostics::log_event(
            "cloud",
            desktop_common::diagnostics::redact_sensitive(&format!("{method} {url} failed: {err}")),
        );
        (0, CloudApiError::Network(err.to_string()).into())
    })?;

    let status = response.status().as_u16();
    if status == 426 {
        let error_text = read_body_limited(&mut response, MAX_ERROR_BODY_BYTES);
        let parsed = serde_json::from_str::<serde_json::Value>(&error_text).ok();
        let server = parsed
            .as_ref()
            .and_then(|v| {
                v.get("server_protocol")
                    .or_else(|| v.get("protocol"))
                    .and_then(serde_json::Value::as_u64)
            })
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        let min_server = parsed
            .as_ref()
            .and_then(|v| v.get("min_protocol").and_then(serde_json::Value::as_u64))
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(MIN_SYNC_PROTOCOL_VERSION);
        return Err((
            426,
            anyhow::Error::new(IncompatibleProtocol {
                client: SYNC_PROTOCOL_VERSION,
                server,
                min_server,
            }),
        ));
    }
    if !(200..300).contains(&status) {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let error_text = read_body_limited(&mut response, MAX_ERROR_BODY_BYTES);
        return Err((
            status,
            classify_api_error(status, retry_after.as_deref(), &error_text),
        ));
    }
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_JSON_BYTES)
        .read_to_string()
        .map_err(|e| (status, CloudApiError::InvalidResponse(e.to_string()).into()))?;
    serde_json::from_str(&text)
        .map_err(|e| (status, CloudApiError::InvalidResponse(e.to_string()).into()))
}

fn register_raw(
    server_url: &str,
    email: &str,
    display_name: &str,
    password: &str,
    device_id: String,
    device_name: String,
    invite_code: Option<String>,
) -> Result<(CloudSessionState, AccountProfile, RecoveryKey)> {
    let server_url = normalize_cloud_url(server_url);
    anyhow::ensure!(!server_url.is_empty(), "Cloud server URL is required");
    let prepared = prepare_registration(
        email,
        display_name,
        password,
        DeviceMetadata {
            device_id,
            device_name,
            platform: device_platform().to_owned(),
        },
        invite_code,
    )?;
    let url = api_url(&server_url, "/auth/register");
    let auth: AuthResponse =
        send_json_request(api_agent(), "POST", &url, None, Some(&prepared.request))
            .map_err(|(_, err)| err)?;
    let session = CloudSessionState {
        server_url,
        user_id: auth.account.user_id.clone(),
        email: auth.account.email.clone(),
        display_name: auth.account.display_name.clone(),
        session_id: auth.session_id,
        access_token: auth.tokens.access_token,
        access_expires_at: auth.tokens.access_expires_at,
        refresh_token: auth.tokens.refresh_token,
        vault_key_b64: prepared.vault_key.to_base64().to_string(),
        kdf: auth.vault.kdf,
    };
    Ok((session, auth.account, prepared.recovery_key))
}

fn login_raw(
    server_url: &str,
    email: &str,
    password: &str,
    device_id: String,
    device_name: String,
) -> Result<(CloudSessionState, AccountProfile)> {
    let server_url = normalize_cloud_url(server_url);
    anyhow::ensure!(!server_url.is_empty(), "Cloud server URL is required");
    let email = validate_email(email)?;
    anyhow::ensure!(!password.is_empty(), "Password is required");

    let agent = api_agent();
    let pre: PreloginResponse = send_json_request(
        agent,
        "POST",
        &api_url(&server_url, "/auth/prelogin"),
        None,
        Some(&PreloginRequest {
            email: email.clone(),
        }),
    )
    .map_err(|(_, err)| err)?;

    let material = derive_login_material(password, &pre.kdf)?;
    let auth: AuthResponse = send_json_request(
        agent,
        "POST",
        &api_url(&server_url, "/auth/login"),
        None,
        Some(&LoginRequest {
            email: email.clone(),
            auth_key: material.auth_key.clone(),
            device_id,
            device_name,
            platform: device_platform().to_owned(),
        }),
    )
    .map_err(|(_, err)| err)?;

    let vault_key = unwrap_vault_key(&email, &material.kek, &auth.vault.wrapped_vault_key)
        .context("Account authenticated, but failed to decrypt vault key")?;

    let session = CloudSessionState {
        server_url,
        user_id: auth.account.user_id.clone(),
        email: auth.account.email.clone(),
        display_name: auth.account.display_name.clone(),
        session_id: auth.session_id,
        access_token: auth.tokens.access_token,
        access_expires_at: auth.tokens.access_expires_at,
        refresh_token: auth.tokens.refresh_token,
        vault_key_b64: vault_key.to_base64().to_string(),
        kdf: auth.vault.kdf,
    };
    Ok((session, auth.account))
}

fn refresh_session_with_agent(
    agent: &ureq::Agent,
    session: &CloudSessionState,
) -> Result<(CloudSessionState, AccountProfile)> {
    let refresh_token = session.refresh_token.clone().ok_or_else(|| {
        anyhow::Error::new(CloudApiError::Unauthorized(
            "Missing refresh token — please sign in again.".into(),
        ))
    })?;
    let resp: RefreshResponse = send_json_request(
        agent,
        "POST",
        &api_url(&session.server_url, "/auth/refresh"),
        None,
        Some(&RefreshRequest {
            refresh_token: Some(refresh_token),
        }),
    )
    .map_err(|(_, err)| err)?;

    let mut updated = session.clone();
    updated.session_id = resp.session_id;
    updated.email = resp.account.email.clone();
    updated.display_name = resp.account.display_name.clone();
    updated.access_token = resp.tokens.access_token;
    updated.access_expires_at = resp.tokens.access_expires_at;
    if let Some(rt) = resp.tokens.refresh_token {
        updated.refresh_token = Some(rt);
    }
    updated.kdf = resp.vault.kdf;
    Ok((updated, resp.account))
}

fn logout_raw(session: &CloudSessionState) -> Result<()> {
    let _: serde_json::Value = send_json_request(
        api_agent(),
        "POST",
        &api_url(&session.server_url, "/auth/logout"),
        Some(&session.access_token),
        Some(&RefreshRequest {
            refresh_token: session.refresh_token.clone(),
        }),
    )
    .map_err(|(_, err)| err)?;
    Ok(())
}

fn recover_account_raw(
    server_url: &str,
    email: &str,
    recovery_key_str: &str,
    new_password: &str,
    device_id: String,
    device_name: String,
) -> Result<(CloudSessionState, AccountProfile, RecoveryKey)> {
    let server_url = normalize_cloud_url(server_url);
    anyhow::ensure!(!server_url.is_empty(), "Cloud server URL is required");
    let email = validate_email(email)?;
    validate_password(new_password)?;
    let recovery_key = RecoveryKey::parse(recovery_key_str)?;

    let agent = api_agent();
    let pre: PreloginResponse = send_json_request(
        agent,
        "POST",
        &api_url(&server_url, "/auth/prelogin"),
        None,
        Some(&PreloginRequest {
            email: email.clone(),
        }),
    )
    .map_err(|(_, err)| err)?;

    let salt = pre.kdf.validate_and_decode_salt()?;
    let (recovery_auth_key, _) = recovery_key.derive_keys(&salt);
    let init: RecoverInitResponse = send_json_request(
        agent,
        "POST",
        &api_url(&server_url, "/auth/recover/init"),
        None,
        Some(&RecoverInitRequest {
            email: email.clone(),
            recovery_auth_key,
        }),
    )
    .map_err(|(_, err)| err)?;

    let prepared = prepare_recovery_complete(
        &email,
        &init.kdf,
        &recovery_key,
        &init.recovery_wrapped_vault_key,
        init.recovery_challenge_token,
        new_password,
        DeviceMetadata {
            device_id,
            device_name,
            platform: device_platform().to_owned(),
        },
    )?;

    let auth: AuthResponse = send_json_request(
        agent,
        "POST",
        &api_url(&server_url, "/auth/recover/complete"),
        None,
        Some(&prepared.request),
    )
    .map_err(|(_, err)| err)?;

    let session = CloudSessionState {
        server_url,
        user_id: auth.account.user_id.clone(),
        email: auth.account.email.clone(),
        display_name: auth.account.display_name.clone(),
        session_id: auth.session_id,
        access_token: auth.tokens.access_token,
        access_expires_at: auth.tokens.access_expires_at,
        refresh_token: auth.tokens.refresh_token,
        vault_key_b64: prepared.vault_key.to_base64().to_string(),
        kdf: auth.vault.kdf,
    };
    Ok((session, auth.account, prepared.new_recovery_key))
}

// ---------------------------------------------------------------------------
// Coordinated single-flight public API (Zed-style single source of truth)
// ---------------------------------------------------------------------------

/// Creates a brand-new Zero-Knowledge Fern Cloud account and persists the unlocked session.
pub fn register_and_store(
    server_url: &str,
    email: &str,
    display_name: &str,
    password: &str,
    device_id: String,
    device_name: String,
    invite_code: Option<String>,
) -> Result<(CloudSessionState, AccountProfile, RecoveryKey)> {
    let (session, account, recovery) = register_raw(
        server_url,
        email,
        display_name,
        password,
        device_id,
        device_name,
        invite_code,
    )?;
    CloudSessionCoordinator::global().store_session(session.clone())?;
    Ok((session, account, recovery))
}

/// Authenticates with Fern Cloud, unwraps the Vault Key locally, and stores the session.
pub fn login_and_store(
    server_url: &str,
    email: &str,
    password: &str,
    device_id: String,
    device_name: String,
) -> Result<(CloudSessionState, AccountProfile)> {
    let (session, account) = login_raw(server_url, email, password, device_id, device_name)?;
    CloudSessionCoordinator::global().store_session(session.clone())?;
    Ok((session, account))
}

/// Zero-knowledge password reset using a saved Recovery Key, storing the new session.
pub fn recover_and_store(
    server_url: &str,
    email: &str,
    recovery_key_str: &str,
    new_password: &str,
    device_id: String,
    device_name: String,
) -> Result<(CloudSessionState, AccountProfile, RecoveryKey)> {
    let (session, account, new_rec) = recover_account_raw(
        server_url,
        email,
        recovery_key_str,
        new_password,
        device_id,
        device_name,
    )?;
    CloudSessionCoordinator::global().store_session(session.clone())?;
    Ok((session, account, new_rec))
}

/// Best-effort remote logout followed by wiping the cached session & encrypted local vault.
pub fn logout_and_clear() -> Result<()> {
    let coord = CloudSessionCoordinator::global();
    if let Ok(Some(session)) = coord.load_session() {
        let _ = logout_raw(&session);
    }
    coord.clear_session()
}

/// Clears the local encrypted session vault without contacting the server (used on `401` revocation).
pub fn clear_local_session() -> Result<()> {
    CloudSessionCoordinator::global().clear_session()
}

pub fn fetch_account_and_devices_current()
-> Result<Option<(CloudSessionState, AccountProfile, Vec<DeviceSessionInfo>)>> {
    let coord = CloudSessionCoordinator::global();
    if coord.load_session()?.is_none() {
        return Ok(None);
    }
    let (acct_val, session) = coord.execute_authed(None, api_agent(), |agent, s| {
        send_json_request::<(), serde_json::Value>(
            agent,
            "GET",
            &api_url(&s.server_url, "/account"),
            Some(&s.access_token),
            None,
        )
    })?;
    let account: AccountProfile = serde_json::from_value(
        acct_val.get("account").cloned().unwrap_or(acct_val),
    )
    .context("malformed account profile response")?;
    coord.update_cached_profile(&session.session_id, &account.email, &account.display_name);
    let (devices, latest_session) = coord.execute_authed(None, api_agent(), |agent, s| {
        send_json_request::<(), Vec<DeviceSessionInfo>>(
            agent,
            "GET",
            &api_url(&s.server_url, "/devices"),
            Some(&s.access_token),
            None,
        )
    })?;
    Ok(Some((latest_session, account, devices)))
}

pub fn revoke_device_current(session_id: &str) -> Result<Vec<DeviceSessionInfo>> {
    let coord = CloudSessionCoordinator::global();
    let _ = coord.execute_authed(None, api_agent(), |agent, s| {
        send_json_request::<(), serde_json::Value>(
            agent,
            "DELETE",
            &api_url(&s.server_url, &format!("/devices/{session_id}")),
            Some(&s.access_token),
            None,
        )
    })?;
    let (devices, _) = coord.execute_authed(None, api_agent(), |agent, s| {
        send_json_request::<(), Vec<DeviceSessionInfo>>(
            agent,
            "GET",
            &api_url(&s.server_url, "/devices"),
            Some(&s.access_token),
            None,
        )
    })?;
    Ok(devices)
}

pub fn revoke_other_devices_current() -> Result<Vec<DeviceSessionInfo>> {
    let coord = CloudSessionCoordinator::global();
    let _ = coord.execute_authed(None, api_agent(), |agent, s| {
        send_json_request::<(), serde_json::Value>(
            agent,
            "DELETE",
            &api_url(&s.server_url, "/devices"),
            Some(&s.access_token),
            None,
        )
    })?;
    let (devices, _) = coord.execute_authed(None, api_agent(), |agent, s| {
        send_json_request::<(), Vec<DeviceSessionInfo>>(
            agent,
            "GET",
            &api_url(&s.server_url, "/devices"),
            Some(&s.access_token),
            None,
        )
    })?;
    Ok(devices)
}

pub fn change_password_current(
    current_password: &str,
    new_password: &str,
) -> Result<RecoveryKey> {
    let coord = CloudSessionCoordinator::global();
    let session = coord
        .load_session()?
        .ok_or_else(|| anyhow::Error::new(CloudApiError::NotSignedIn))?;
    let vault_key = session.vault_key()?;
    let prepared = prepare_password_change(
        &session.email,
        current_password,
        &session.kdf,
        new_password,
        &vault_key,
    )?;
    let (resp, mut latest): (AuthResponse, CloudSessionState) =
        coord.execute_authed(Some(&session.user_id), api_agent(), |agent, s| {
            send_json_request(
                agent,
                "POST",
                &api_url(&s.server_url, "/account/password"),
                Some(&s.access_token),
                Some(&prepared.request),
            )
        })?;
    latest.session_id = resp.session_id;
    latest.access_token = resp.tokens.access_token;
    latest.access_expires_at = resp.tokens.access_expires_at;
    if let Some(rt) = resp.tokens.refresh_token {
        latest.refresh_token = Some(rt);
    }
    latest.kdf = resp.vault.kdf;
    coord.store_session(latest)?;
    Ok(prepared.new_recovery_key)
}

pub fn regenerate_recovery_key_current(current_password: &str) -> Result<RecoveryKey> {
    let coord = CloudSessionCoordinator::global();
    let session = coord
        .load_session()?
        .ok_or_else(|| anyhow::Error::new(CloudApiError::NotSignedIn))?;
    let vault_key = session.vault_key()?;
    let current = derive_login_material(current_password, &session.kdf)?;
    let salt = session.kdf.validate_and_decode_salt()?;
    let new_recovery = RecoveryKey::generate()?;
    let (new_recovery_auth_key, new_recovery_wrapped_vault_key) =
        wrap_vault_key_for_recovery(&session.email, &salt, &new_recovery, &vault_key)?;
    let body = UpdateRecoveryKeyRequest {
        current_auth_key: current.auth_key.clone(),
        new_recovery_wrapped_vault_key,
        new_recovery_auth_key,
    };
    let _ = coord.execute_authed(Some(&session.user_id), api_agent(), |agent, s| {
        send_json_request::<_, serde_json::Value>(
            agent,
            "POST",
            &api_url(&s.server_url, "/account/recovery-key"),
            Some(&s.access_token),
            Some(&body),
        )
    })?;
    Ok(new_recovery)
}

pub fn delete_account_current(current_password: &str) -> Result<()> {
    let coord = CloudSessionCoordinator::global();
    let session = coord
        .load_session()?
        .ok_or_else(|| anyhow::Error::new(CloudApiError::NotSignedIn))?;
    let material = derive_login_material(current_password, &session.kdf)?;
    let _ = coord.execute_authed(Some(&session.user_id), api_agent(), |agent, s| {
        send_json_request::<_, serde_json::Value>(
            agent,
            "DELETE",
            &api_url(&s.server_url, "/account"),
            Some(&s.access_token),
            Some(&DeleteAccountRequest {
                current_auth_key: material.auth_key.clone(),
            }),
        )
    })?;
    coord.clear_session()
}

pub fn load_current_session(expected_user_id: &str) -> Result<CloudSessionState> {
    CloudSessionCoordinator::global().fresh_session_for(Some(expected_user_id), sync_agent())
}

pub fn post_sync_current(
    expected_user_id: &str,
    body: &serde_json::Value,
) -> Result<(serde_json::Value, CloudSessionState)> {
    CloudSessionCoordinator::global().execute_authed(
        Some(expected_user_id),
        sync_agent(),
        |agent, s| {
            send_json_request(
                agent,
                "POST",
                &api_url(&s.server_url, "/sync"),
                Some(&s.access_token),
                Some(body),
            )
        },
    )
}

pub fn transfer_files_current(
    expected_user_id: &str,
    dir: &Path,
    upload: &[String],
    download: &[String],
) -> FileTransfers {
    let mut outcome = FileTransfers::default();
    let coord = CloudSessionCoordinator::global();
    let session = match coord.fresh_session_for(Some(expected_user_id), sync_agent()) {
        Ok(s) => s,
        Err(err) => {
            for name in upload.iter().chain(download.iter()) {
                outcome.failed.push((name.clone(), anyhow::anyhow!("{err:#}")));
            }
            return outcome;
        }
    };
    let e2ee_key = match session.e2ee_key() {
        Ok(key) => key,
        Err(err) => {
            for name in upload.iter().chain(download.iter()) {
                outcome.failed.push((name.clone(), err.clone().into()));
            }
            return outcome;
        }
    };

    for name in upload {
        let sent = (|| -> Result<()> {
            anyhow::ensure!(fern_core::is_safe_attachment_name(name), "invalid attachment name");
            let file = std::fs::File::open(dir.join(name))?;
            check_attachment_size(file.metadata()?.len())?;
            let mut data = Vec::new();
            file.take(MAX_ATTACHMENT_BYTES + 1).read_to_end(&mut data)?;
            check_attachment_size(data.len() as u64)?;
            let encrypted = e2ee_key.encrypt_file(name, &data)?;
            let key = file_key(name);
            let _ = coord.execute_authed(Some(expected_user_id), sync_agent(), |agent, s| {
                let url = api_url(&s.server_url, &format!("/files/{key}"));
                let mut resp = agent
                    .put(&url)
                    .config()
                    .http_status_as_error(false)
                    .build()
                    .header("User-Agent", &user_agent())
                    .header("X-Fern-Client", "desktop")
                    .header("Authorization", &format!("Bearer {}", s.access_token))
                    .send(&encrypted[..])
                    .map_err(|e| (0, CloudApiError::Network(e.to_string()).into()))?;
                let status = resp.status().as_u16();
                if (200..300).contains(&status) {
                    Ok(())
                } else {
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    let body = read_body_limited(&mut resp, MAX_ERROR_BODY_BYTES);
                    Err((
                        status,
                        classify_api_error(status, retry_after.as_deref(), &body),
                    ))
                }
            })?;
            Ok(())
        })();
        match sent {
            Ok(()) => outcome.synced.push(name.clone()),
            Err(err) => outcome.failed.push((name.clone(), err)),
        }
    }

    let download_limit = MAX_ATTACHMENT_BYTES + E2EE_FILE_OVERHEAD;
    for name in download {
        let fetched = (|| -> Result<()> {
            anyhow::ensure!(fern_core::is_safe_attachment_name(name), "invalid attachment name");
            let key = file_key(name);
            let (raw, _) =
                coord.execute_authed(Some(expected_user_id), sync_agent(), |agent, s| {
                    let url = api_url(&s.server_url, &format!("/files/{key}"));
                    let mut resp = agent
                        .get(&url)
                        .config()
                        .http_status_as_error(false)
                        .build()
                        .header("User-Agent", &user_agent())
                        .header("X-Fern-Client", "desktop")
                        .header("Authorization", &format!("Bearer {}", s.access_token))
                        .call()
                        .map_err(|e| (0, CloudApiError::Network(e.to_string()).into()))?;
                    let status = resp.status().as_u16();
                    if !(200..300).contains(&status) {
                        let retry_after = resp
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned);
                        let body = read_body_limited(&mut resp, MAX_ERROR_BODY_BYTES);
                        return Err((
                            status,
                            classify_api_error(status, retry_after.as_deref(), &body),
                        ));
                    }
                    let bytes = resp
                        .body_mut()
                        .with_config()
                        .limit(download_limit)
                        .read_to_vec()
                        .map_err(|e| (status, CloudApiError::InvalidResponse(e.to_string()).into()))?;
                    Ok(bytes)
                })?;
            if !e2ee::is_encrypted_file(&raw) {
                return Err(anyhow::Error::new(E2eeError::Malformed(
                    "received unencrypted cloud attachment".into(),
                )));
            }
            let plain = e2ee_key.decrypt_file(name, &raw)?;
            check_attachment_size(plain.len() as u64)?;
            save_download(dir, name, &plain)
        })();
        match fetched {
            Ok(()) => {
                outcome.downloaded = true;
                outcome.synced.push(name.clone());
            }
            Err(err) => outcome.failed.push((name.clone(), err)),
        }
    }

    outcome
}

/// Best-effort coordinated deletion of remote attachment blobs in Fern Cloud.
pub fn delete_remote_files_current(expected_user_id: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let coord = CloudSessionCoordinator::global();
    for name in names {
        if !fern_core::is_safe_attachment_name(name) {
            continue;
        }
        let key = file_key(name);
        let _ = coord.execute_authed(Some(expected_user_id), api_agent(), |agent, s| {
            let url = api_url(&s.server_url, &format!("/files/{key}"));
            let resp = agent
                .delete(&url)
                .config()
                .http_status_as_error(false)
                .build()
                .header("User-Agent", &user_agent())
                .header("X-Fern-Client", "desktop")
                .header("Authorization", &format!("Bearer {}", s.access_token))
                .call()
                .map_err(|e| (0, CloudApiError::Network(e.to_string()).into()))?;
            let status = resp.status().as_u16();
            if (200..300).contains(&status) || status == 404 {
                Ok(())
            } else {
                Err((status, CloudApiError::Server { status, message: format!("HTTP {status}") }.into()))
            }
        });
    }
}

pub fn request_events_ticket_current(expected_user_id: &str) -> Result<(String, String)> {
    let (resp, session): (EventsTicketResponse, CloudSessionState) =
        CloudSessionCoordinator::global().execute_authed(
            Some(expected_user_id),
            api_agent(),
            |agent, s| {
                send_json_request::<(), EventsTicketResponse>(
                    agent,
                    "POST",
                    &api_url(&s.server_url, "/events/ticket"),
                    Some(&s.access_token),
                    None,
                )
            },
        )?;
    Ok((session.server_url.clone(), resp.ticket))
}

/// Connects to `GET /api/v1/events?ticket=...` and blocks reading SSE frames until EOF, error,
/// revocation (`event: revoked`), or `stop_flag` becomes true.
pub fn stream_events_once(
    server_url: &str,
    ticket: &str,
    stop_flag: &AtomicBool,
    on_connected: impl Fn(),
    on_sync_event: impl Fn(SyncEventNotification),
) -> Result<()> {
    let agent = sse_agent();
    let url = format!("{}?ticket={ticket}", api_url(server_url, "/events"));
    let mut response = agent
        .get(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .header("User-Agent", &user_agent())
        .header("Accept", "text/event-stream")
        .header("X-Fern-Client", "desktop")
        .call()
        .map_err(|e| CloudApiError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    if status != 200 {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = read_body_limited(&mut response, MAX_ERROR_BODY_BYTES);
        return Err(classify_api_error(status, retry_after.as_deref(), &body));
    }
    on_connected();

    let mut reader = BufReader::new(response.body_mut().as_reader());
    let mut event_name = String::new();
    let mut data_buf = String::new();
    let mut line = String::new();

    while !stop_flag.load(Ordering::Relaxed) {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // clean EOF
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\r', '\n']);
                if trimmed.is_empty() {
                    if event_name == "revoked" {
                        return Err(CloudApiError::Unauthorized(
                            "This device was signed out or revoked — please sign in again.".into(),
                        )
                        .into());
                    }
                    if event_name == "sync" && !data_buf.is_empty()
                        && let Ok(notif) = serde_json::from_str::<SyncEventNotification>(&data_buf)
                    {
                        on_sync_event(notif);
                    }
                    event_name.clear();
                    data_buf.clear();
                } else if trimmed.starts_with(':') {
                    // Heartbeat comment; keep alive.
                } else if let Some(ev) = trimmed.strip_prefix("event:") {
                    event_name = ev.trim().to_owned();
                } else if let Some(data) = trimmed.strip_prefix("data:") {
                    if !data_buf.is_empty() {
                        data_buf.push('\n');
                    }
                    data_buf.push_str(data.trim_start());
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                // Server sends heartbeat every 25s; a 60s silence means the connection died.
                bail!("SSE stream timed out");
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

/// Checks if `email` matches an already-connected account cleanly.
pub fn is_same_account(session: &CloudSessionState, email: &str) -> bool {
    normalize_email(&session.email) == normalize_email(email)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use desktop_common::sync::{is_session_expired, rate_limit_retry_after};
    use fern_core::cloud::{KdfParams, VaultKey};

    use super::*;

    #[test]
    fn single_flight_refresh_coalesces_concurrent_rotations() {
        let vk = VaultKey::generate().unwrap();
        let initial = CloudSessionState {
            server_url: "https://fern.atabits.com".into(),
            user_id: "usr-sf-1".into(),
            email: "sf@example.com".into(),
            display_name: "SF".into(),
            session_id: "ses-1".into(),
            access_token: "tok-old".into(),
            access_expires_at: 100,
            refresh_token: Some("rt-1".into()),
            vault_key_b64: vk.to_base64().to_string(),
            kdf: KdfParams::generate_default().unwrap(),
        };

        let coord = Arc::new(CloudSessionCoordinator::new());
        *coord.cached.write().unwrap() = Some(initial);

        let calls = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let coord = Arc::clone(&coord);
            let calls = Arc::clone(&calls);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                coord
                    .refresh_singleflight_with(
                        Some("tok-old"),
                        200,
                        |cur| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(25));
                            let mut next = cur.clone();
                            next.access_token = "tok-fresh-2".into();
                            next.access_expires_at = 2_000;
                            next.refresh_token = Some("rt-2".into());
                            Ok(next)
                        },
                        |_| Ok(()),
                    )
                    .unwrap()
            }));
        }

        for h in handles {
            let res = h.join().unwrap();
            assert_eq!(res.access_token, "tok-fresh-2");
            assert_eq!(res.refresh_token.as_deref(), Some("rt-2"));
        }
        // All 8 concurrent threads must coalesce into EXACTLY 1 refresh rotation!
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn classify_api_error_distinguishes_unauthorized_grace_and_rate_limit() {
        let e401 = classify_api_error(
            401,
            None,
            r#"{"error":"token_reuse_detected","message":"Session revoked"}"#,
        );
        assert!(is_session_expired(&e401));

        let e409 = classify_api_error(
            409,
            None,
            r#"{"error":"refresh_race","message":"retry"}"#,
        );
        assert!(is_grace_conflict(&e409));
        assert!(!is_session_expired(&e409));

        let e429 = classify_api_error(
            429,
            Some("17"),
            r#"{"error":"rate_limited","message":"Slow down"}"#,
        );
        assert_eq!(rate_limit_retry_after(&e429), Some(17));
    }

    #[test]
    fn stream_events_once_surfaces_revoked_event_as_session_expired() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let worker = std::thread::spawn(move || {
            if let Ok(req) = server.recv() {
                let body = "event: connected\ndata: {\"ok\":true}\n\nevent: revoked\ndata: {\"reason\":\"session_revoked\"}\n\n";
                let resp = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/event-stream"[..])
                        .unwrap(),
                );
                let _ = req.respond(resp);
            }
        });

        let stop = AtomicBool::new(false);
        let connected = AtomicBool::new(false);
        let res = stream_events_once(
            &format!("http://127.0.0.1:{port}"),
            "tkt-1",
            &stop,
            || connected.store(true, Ordering::SeqCst),
            |_| {},
        );
        worker.join().unwrap();
        assert!(connected.load(Ordering::SeqCst));
        let err = res.expect_err("revoked SSE frame must return Unauthorized error");
        assert!(is_session_expired(&err), "expected SessionExpired, got: {err:#}");
    }
}
