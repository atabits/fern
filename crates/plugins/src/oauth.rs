//! Zero-secret-exposure OAuth 2.0 Authorization Code (loopback callback) and token refresh helper.
//!
//! Credentials (`client_secret`, `access_token`, `refresh_token`) are read from and written directly
//! into the plugin's encrypted [`SecretsVault`] on the Rust host side — JS plugin code never sees
//! raw token values and instead uses `${secret:access_token}` in `fern.http.fetch` headers.

use std::io::{ErrorKind, Read as _, Write as _};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::{
    STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest as _, Sha256};
use ureq::http::Uri;
use zeroize::Zeroizing;

use crate::http::{
    HttpRequest, execute_http_request_with_redirects, is_loopback_ip, validate_target_uri,
};
use crate::permissions::{PermissionSet, is_loopback_host_literal, normalize_host};
use crate::vault::{SecretsVault, scrub_secret_variants};

pub const DEFAULT_REDIRECT_HOST: &str = "127.0.0.1";
pub const DEFAULT_REDIRECT_PORT: u16 = 18481;
pub const DEFAULT_REDIRECT_PATH: &str = "/callback";
pub const DEFAULT_OAUTH_TIMEOUT_SEC: u64 = 180;
const MAX_CALLBACK_REQ_BYTES: usize = 16 * 1024;
const LOOPBACK_POLL_INTERVAL: Duration = Duration::from_millis(25);
const RESERVED_OAUTH_AUTHORIZE_PARAMS: &[&str] = &[
    "response_type",
    "client_id",
    "redirect_uri",
    "state",
    "scope",
    "code_challenge",
    "code_challenge_method",
    "code_verifier",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthTokenAuth {
    #[default]
    Basic,
    Body,
}

impl OAuthTokenAuth {
    pub fn parse(raw: Option<&str>) -> Result<Self> {
        match raw.map(str::trim).unwrap_or("basic").to_ascii_lowercase().as_str() {
            "" | "basic" => Ok(Self::Basic),
            "body" | "post" => Ok(Self::Body),
            other => bail!("unsupported OAuth tokenAuth {other:?} (expected \"basic\" or \"body\")"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct OAuthAuthorizeRequest {
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret_key: Option<String>,
    pub scopes: Vec<String>,
    pub redirect_host: String,
    pub redirect_port: u16,
    pub redirect_path: String,
    pub token_auth: OAuthTokenAuth,
    pub save_access_token_as: String,
    pub save_refresh_token_as: Option<String>,
    pub extra_authorize_params: Vec<(String, String)>,
    pub timeout_sec: u64,
}

#[derive(Clone, Debug)]
pub struct OAuthRefreshRequest {
    pub token_url: String,
    pub client_id: String,
    pub client_secret_key: Option<String>,
    pub refresh_token_key: String,
    pub scopes: Vec<String>,
    pub token_auth: OAuthTokenAuth,
    pub save_access_token_as: String,
    pub save_refresh_token_as: Option<String>,
}

/// Optional extensions for OAuth 2.0 Authorization Code flows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthAuthorizeOptions {
    /// Enable RFC 7636 PKCE (`S256` challenge + verifier). Defaults to `false` for compatibility
    /// with providers such as TickTick that reject or do not support PKCE parameters.
    #[serde(default)]
    pub pkce: bool,
}

/// Non-secret metadata returned to JS after a successful OAuth exchange or refresh.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthTokenSummary {
    pub ok: bool,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    pub expires_in: Option<u64>,
    pub has_refresh_token: bool,
}

/// Validates redirect host/port/path and returns the canonical `redirect_uri`.
pub fn build_redirect_uri(host: &str, port: u16, path: &str) -> Result<String> {
    let host_clean = host.trim().to_ascii_lowercase();
    ensure!(
        host_clean == "127.0.0.1" || host_clean == "localhost",
        "OAuth redirectHost must be \"127.0.0.1\" or \"localhost\" (got {host:?})"
    );
    ensure!(
        port >= 1024,
        "OAuth redirectPort must be between 1024 and 65535 (got {port})"
    );
    let path_clean = path.trim();
    ensure!(
        path_clean.starts_with('/') && !path_clean.contains('?') && !path_clean.contains('#') && !path_clean.contains(' '),
        "OAuth redirectPath must start with '/' and contain no query/fragment/spaces (got {path:?})"
    );
    Ok(format!("http://{host_clean}:{port}{path_clean}"))
}

/// Executes the full loopback OAuth 2.0 Authorization Code flow synchronously on a worker thread.
///
/// Calls `on_open_url(full_authorize_url)` strictly AFTER the loopback listener is bound so the
/// browser cannot race ahead of the local callback server.
pub fn run_oauth_authorize<F>(
    plugin_id: &str,
    permissions: &PermissionSet,
    vault: &SecretsVault,
    req: &OAuthAuthorizeRequest,
    cancel: &AtomicBool,
    on_open_url: F,
) -> Result<OAuthTokenSummary>
where
    F: FnOnce(String),
{
    run_oauth_authorize_with_options(
        plugin_id,
        permissions,
        vault,
        req,
        &OAuthAuthorizeOptions::default(),
        cancel,
        on_open_url,
    )
}

/// Executes the full loopback OAuth 2.0 Authorization Code flow with optional PKCE (`S256`).
pub fn run_oauth_authorize_with_options<F>(
    plugin_id: &str,
    permissions: &PermissionSet,
    vault: &SecretsVault,
    req: &OAuthAuthorizeRequest,
    options: &OAuthAuthorizeOptions,
    cancel: &AtomicBool,
    on_open_url: F,
) -> Result<OAuthTokenSummary>
where
    F: FnOnce(String),
{
    let client_id = req.client_id.trim();
    ensure!(!client_id.is_empty(), "OAuth clientId must not be empty");
    let save_access = req.save_access_token_as.trim();
    ensure!(!save_access.is_empty(), "OAuth saveAccessTokenAs must not be empty");

    let authorize_url_trimmed = req.authorize_url.trim();
    ensure!(
        !authorize_url_trimmed.contains('#'),
        "OAuth authorizeUrl must not contain a URL fragment (#)"
    );
    let token_url_trimmed = req.token_url.trim();
    ensure!(
        !token_url_trimmed.contains('#'),
        "OAuth tokenUrl must not contain a URL fragment (#)"
    );

    let auth_uri: Uri = authorize_url_trimmed
        .parse()
        .with_context(|| format!("invalid authorizeUrl {:?}", req.authorize_url))?;
    let token_uri: Uri = token_url_trimmed
        .parse()
        .with_context(|| format!("invalid tokenUrl {:?}", req.token_url))?;

    validate_oauth_endpoint(&auth_uri, permissions, "authorizeUrl")?;
    validate_oauth_endpoint(&token_uri, permissions, "tokenUrl")?;

    if let Some(existing_query) = auth_uri.query() {
        for (raw_key, _) in parse_query_params(existing_query) {
            ensure!(
                !is_reserved_authorize_param(&raw_key),
                "OAuth authorizeUrl must not specify reserved query parameter {raw_key:?}"
            );
        }
    }
    for (k, _) in &req.extra_authorize_params {
        let key = k.trim();
        if !key.is_empty() {
            ensure!(
                !is_reserved_authorize_param(key),
                "OAuth extraAuthorizeParams cannot override reserved parameter {key:?}"
            );
        }
    }

    let client_secret: Option<Zeroizing<String>> = match req
        .client_secret_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(key) => {
            let val = Zeroizing::new(vault.get(plugin_id, key)?.ok_or_else(|| {
                anyhow!("missing secret {key:?} in SecretsVault; save it in plugin settings first")
            })?);
            ensure!(!val.trim().is_empty(), "secret {key:?} is empty");
            Some(val)
        }
        None => None,
    };

    let redirect_uri = build_redirect_uri(&req.redirect_host, req.redirect_port, &req.redirect_path)?;
    let state = random_hex_state()?;
    let scope_str = req
        .scopes
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    let pkce_pair = if options.pkce {
        Some(generate_pkce_s256()?)
    } else {
        None
    };

    let mut query_pairs: Vec<(&str, String)> = vec![
        ("response_type", "code".to_owned()),
        ("client_id", client_id.to_owned()),
        ("redirect_uri", redirect_uri.clone()),
        ("state", state.clone()),
    ];
    if !scope_str.is_empty() {
        query_pairs.push(("scope", scope_str.clone()));
    }
    if let Some((_, ref challenge)) = pkce_pair {
        query_pairs.push(("code_challenge", challenge.clone()));
        query_pairs.push(("code_challenge_method", "S256".to_owned()));
    }
    for (k, v) in &req.extra_authorize_params {
        let key = k.trim();
        if !key.is_empty() {
            query_pairs.push((key, v.clone()));
        }
    }

    let separator = if authorize_url_trimmed.contains('?') { "&" } else { "?" };
    let full_authorize_url = format!(
        "{}{}{}",
        authorize_url_trimmed,
        separator,
        encode_form_pairs(&query_pairs)
    );

    let listeners = bind_loopback_listeners(&req.redirect_host, req.redirect_port)?;
    on_open_url(full_authorize_url);

    let timeout = Duration::from_secs(req.timeout_sec.clamp(5, 300));
    let code = wait_for_oauth_code(
        &listeners,
        req.redirect_port,
        &req.redirect_path,
        &state,
        timeout,
        cancel,
    )?;

    let mut form_pairs: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("code", code),
        ("redirect_uri", redirect_uri),
    ];
    if !scope_str.is_empty() {
        form_pairs.push(("scope", scope_str));
    }
    let pkce_verifier = pkce_pair.as_ref().map(|(verifier, _)| verifier.as_str());
    if let Some(verifier) = pkce_verifier {
        form_pairs.push(("code_verifier", verifier.to_owned()));
    }

    exchange_token_endpoint(
        plugin_id,
        permissions,
        vault,
        token_url_trimmed,
        client_id,
        client_secret.as_deref().map(String::as_str),
        pkce_verifier,
        req.token_auth,
        &mut form_pairs,
        save_access,
        req.save_refresh_token_as.as_deref(),
    )
}

/// Refreshes an OAuth 2.0 access token using a stored refresh token in [`SecretsVault`].
pub fn run_oauth_refresh(
    plugin_id: &str,
    permissions: &PermissionSet,
    vault: &SecretsVault,
    req: &OAuthRefreshRequest,
) -> Result<OAuthTokenSummary> {
    let client_id = req.client_id.trim();
    ensure!(!client_id.is_empty(), "OAuth clientId must not be empty");
    let save_access = req.save_access_token_as.trim();
    ensure!(!save_access.is_empty(), "OAuth saveAccessTokenAs must not be empty");
    let refresh_key = req.refresh_token_key.trim();
    ensure!(!refresh_key.is_empty(), "OAuth refreshTokenKey must not be empty");

    let token_url_trimmed = req.token_url.trim();
    ensure!(
        !token_url_trimmed.contains('#'),
        "OAuth tokenUrl must not contain a URL fragment (#)"
    );
    let token_uri: Uri = token_url_trimmed
        .parse()
        .with_context(|| format!("invalid tokenUrl {:?}", req.token_url))?;
    validate_oauth_endpoint(&token_uri, permissions, "tokenUrl")?;

    let refresh_token = Zeroizing::new(
        vault
            .get(plugin_id, refresh_key)?
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow!("no refresh token stored under secret {refresh_key:?}"))?,
    );

    let client_secret: Option<Zeroizing<String>> = match req
        .client_secret_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(key) => vault
            .get(plugin_id, key)?
            .filter(|s| !s.trim().is_empty())
            .map(Zeroizing::new),
        None => None,
    };

    let scope_str = req
        .scopes
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    let mut form_pairs: Vec<(&str, String)> = vec![
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", refresh_token.as_str().to_owned()),
    ];
    if !scope_str.is_empty() {
        form_pairs.push(("scope", scope_str));
    }

    exchange_token_endpoint(
        plugin_id,
        permissions,
        vault,
        token_url_trimmed,
        client_id,
        client_secret.as_deref().map(String::as_str),
        None,
        req.token_auth,
        &mut form_pairs,
        save_access,
        req.save_refresh_token_as.as_deref(),
    )
}

#[allow(clippy::too_many_arguments)]
fn exchange_token_endpoint(
    plugin_id: &str,
    permissions: &PermissionSet,
    vault: &SecretsVault,
    token_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    pkce_verifier: Option<&str>,
    token_auth: OAuthTokenAuth,
    form_pairs: &mut Vec<(&str, String)>,
    save_access_token_as: &str,
    save_refresh_token_as: Option<&str>,
) -> Result<OAuthTokenSummary> {
    let mut headers = vec![
        (
            "Content-Type".to_owned(),
            "application/x-www-form-urlencoded".to_owned(),
        ),
        ("Accept".to_owned(), "application/json".to_owned()),
    ];

    let mut basic_encoded: Option<Zeroizing<String>> = None;
    match token_auth {
        OAuthTokenAuth::Basic => {
            let secret = client_secret.unwrap_or("");
            let raw_creds = Zeroizing::new(format!("{client_id}:{secret}"));
            let encoded = Zeroizing::new(BASE64_STANDARD.encode(raw_creds.as_bytes()));
            headers.push(("Authorization".to_owned(), format!("Basic {}", encoded.as_str())));
            basic_encoded = Some(encoded);
        }
        OAuthTokenAuth::Body => {
            form_pairs.push(("client_id", client_id.to_owned()));
            if let Some(secret) = client_secret {
                form_pairs.push(("client_secret", secret.to_owned()));
            }
        }
    }

    let body = Zeroizing::new(encode_form_pairs(form_pairs).into_bytes());
    let http_req = HttpRequest {
        method: "POST".to_owned(),
        url: token_url.to_owned(),
        headers,
        body: Some(body.to_vec()),
        timeout_ms: Some(20_000),
    };

    // Strictly disable HTTP redirects on OAuth token exchange (`max_redirects = 0`) so
    // authorization codes, refresh tokens, and client secrets can never be redirected.
    let resp = execute_http_request_with_redirects(plugin_id, permissions, Some(vault), &http_req, 0)?;
    if !resp.ok() {
        let mut preview = vault.scrub_secrets(plugin_id, &resp.body_text());
        let mut extra_secrets: Vec<&str> = Vec::new();
        if let Some(ref enc) = basic_encoded {
            extra_secrets.push(enc.as_str());
        }
        if let Some(secret) = client_secret {
            extra_secrets.push(secret);
        }
        if let Some(verifier) = pkce_verifier {
            extra_secrets.push(verifier);
        }
        if !extra_secrets.is_empty() {
            preview = scrub_secret_variants(&preview, &extra_secrets);
        }
        let short: String = preview.chars().take(240).collect();
        bail!("OAuth token endpoint returned HTTP {}: {short}", resp.status);
    }

    let parsed: JsonValue = serde_json::from_slice(&resp.body)
        .context("OAuth token endpoint returned invalid JSON")?;
    let access_token = parsed
        .get("access_token")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .context("OAuth token response missing access_token")?;

    vault.set(plugin_id, save_access_token_as, access_token)?;

    let refresh_token = parsed
        .get("refresh_token")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let has_refresh_token = if let (Some(rt), Some(rt_key)) = (
        refresh_token,
        save_refresh_token_as.map(str::trim).filter(|s| !s.is_empty()),
    ) {
        vault.set(plugin_id, rt_key, rt)?;
        true
    } else {
        save_refresh_token_as
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|k| vault.has(plugin_id, k).ok())
            .unwrap_or(false)
    };

    Ok(OAuthTokenSummary {
        ok: true,
        token_type: parsed
            .get("token_type")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        scope: parsed
            .get("scope")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        expires_in: parsed.get("expires_in").and_then(JsonValue::as_u64),
        has_refresh_token,
    })
}

fn is_reserved_authorize_param(key: &str) -> bool {
    let lower = key.trim().to_ascii_lowercase();
    RESERVED_OAUTH_AUTHORIZE_PARAMS.contains(&lower.as_str())
}

fn generate_pkce_s256() -> Result<(Zeroizing<String>, String)> {
    let mut raw = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(&mut *raw)
        .map_err(|e| anyhow!("failed to generate PKCE code_verifier: {e}"))?;
    let verifier = Zeroizing::new(BASE64_URL_SAFE_NO_PAD.encode(*raw));
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = BASE64_URL_SAFE_NO_PAD.encode(digest);
    Ok((verifier, challenge))
}

fn validate_oauth_endpoint(uri: &Uri, permissions: &PermissionSet, label: &str) -> Result<()> {
    let scheme = uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if scheme == "http" {
        ensure!(
            permissions.allows_local(),
            "OAuth {label} must use https://"
        );
        let authority = uri
            .authority()
            .with_context(|| format!("OAuth {label} missing host"))?;
        let host = authority.host();
        ensure!(
            is_loopback_host_literal(host),
            "OAuth {label} over http:// is only allowed for loopback hosts (127.0.0.1, ::1, localhost)"
        );
        let host_clean = normalize_host(host);
        if host_clean.parse::<IpAddr>().is_err() {
            let port = authority.port_u16().unwrap_or(80);
            let addrs: Vec<SocketAddr> = (host_clean.as_str(), port)
                .to_socket_addrs()
                .with_context(|| format!("OAuth {label} failed resolving loopback host {host:?}"))?
                .collect();
            ensure!(
                !addrs.is_empty() && addrs.iter().all(|a| is_loopback_ip(a.ip())),
                "OAuth {label} host {host:?} resolved to non-loopback IP"
            );
        }
    } else {
        ensure!(scheme == "https", "OAuth {label} must use https://");
    }
    validate_target_uri(uri, permissions).with_context(|| format!("OAuth {label} blocked"))?;
    Ok(())
}

fn bind_loopback_listeners(host: &str, port: u16) -> Result<Vec<TcpListener>> {
    let v4_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let v4 = TcpListener::bind(v4_addr).with_context(|| {
        format!("cannot bind OAuth callback port {port} on 127.0.0.1 (is another login in progress?)")
    })?;
    v4.set_nonblocking(true)?;
    let mut listeners = vec![v4];

    if host.trim().eq_ignore_ascii_case("localhost") {
        let v6_addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port);
        match TcpListener::bind(v6_addr) {
            Ok(v6) => {
                v6.set_nonblocking(true)?;
                listeners.push(v6);
            }
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::AddrInUse | ErrorKind::PermissionDenied
                ) =>
            {
                bail!(
                    "cannot bind OAuth callback port {port} on [::1] ({err}); refusing to proceed on IPv4-only for localhost"
                );
            }
            Err(_) => {
                // IPv6 loopback is disabled/unavailable on this machine.
            }
        }
    }
    Ok(listeners)
}

fn wait_for_oauth_code(
    listeners: &[TcpListener],
    expected_port: u16,
    expected_path: &str,
    expected_state: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Acquire) {
            bail!("OAuth authorization cancelled");
        }
        for listener in listeners {
            match listener.accept() {
                Ok((mut stream, _addr)) => {
                    if let Some(outcome) = handle_callback_connection(
                        &mut stream,
                        expected_port,
                        expected_path,
                        expected_state,
                    ) {
                        return outcome;
                    }
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {}
                Err(err) => return Err(anyhow!("OAuth callback accept error: {err}")),
            }
        }
        thread::sleep(LOOPBACK_POLL_INTERVAL);
    }
    bail!("OAuth authorization timed out waiting for browser callback")
}

fn read_callback_headers(stream: &mut TcpStream) -> Option<String> {
    let mut raw = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    while raw.len() < MAX_CALLBACK_REQ_BYTES {
        let to_read = chunk.len().min(MAX_CALLBACK_REQ_BYTES - raw.len());
        match stream.read(&mut chunk[..to_read]) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&chunk[..n]);
                if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                if !raw.is_empty() {
                    break;
                }
                return None;
            }
            Err(_) => return None,
        }
    }
    if raw.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&raw).into_owned())
}

fn is_valid_loopback_host_header(raw_request: &str, expected_port: u16) -> bool {
    let mut host_val: Option<&str> = None;
    for line in raw_request.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("host")
        {
            if host_val.is_some() {
                return false;
            }
            host_val = Some(v.trim());
        }
    }
    let Some(host_hdr) = host_val else {
        return false;
    };
    if host_hdr.is_empty() {
        return false;
    }

    let (host_part, port_part) = if let Some(rest) = host_hdr.strip_prefix('[') {
        if let Some((ip, port_str)) = rest.split_once("]:") {
            let Ok(p) = port_str.parse::<u16>() else {
                return false;
            };
            (ip, Some(p))
        } else if let Some(ip) = rest.strip_suffix(']') {
            (ip, None)
        } else {
            return false;
        }
    } else if let Some((h, port_str)) = host_hdr.rsplit_once(':') {
        if h.contains(':') {
            return false;
        }
        let Ok(p) = port_str.parse::<u16>() else {
            return false;
        };
        (h, Some(p))
    } else {
        (host_hdr, None)
    };

    if let Some(port) = port_part
        && port != expected_port
    {
        return false;
    }

    let norm = host_part.to_ascii_lowercase();
    norm == "127.0.0.1" || norm == "localhost" || norm == "::1"
}

fn handle_callback_connection(
    stream: &mut TcpStream,
    expected_port: u16,
    expected_path: &str,
    expected_state: &str,
) -> Option<Result<String>> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));

    let raw = read_callback_headers(stream)?;
    let first_line = raw.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");

    if method != "GET" {
        write_http_html(stream, 405, "Method Not Allowed", "Unsupported request method.", false);
        return None;
    }

    if !is_valid_loopback_host_header(&raw, expected_port) {
        write_http_html(stream, 400, "Invalid Host", "Invalid Host header.", false);
        return None;
    }

    let (req_path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };

    if req_path != expected_path {
        write_http_html(stream, 404, "Not Found", "Waiting for OAuth callback.", false);
        return None;
    }

    let params = parse_query_params(query);

    // Validate `state` STRICTLY BEFORE inspecting `error` or `code`.
    // Ignore requests with missing/invalid `state` (HTTP 400, keep listening) so cross-site
    // requests cannot abort an active OAuth session or spoof provider errors.
    let states: Vec<&str> = params
        .iter()
        .filter(|(k, _)| k == "state")
        .map(|(_, v)| v.as_str())
        .collect();
    if states.len() != 1 || !constant_time_eq(states[0].as_bytes(), expected_state.as_bytes()) {
        write_http_html(
            stream,
            400,
            "Invalid OAuth State",
            "State parameter mismatch. Ignoring unsolicited request.",
            false,
        );
        return None;
    }

    if let Some(err) = params.iter().find(|(k, _)| k == "error").map(|(_, v)| v.as_str()) {
        let desc = params
            .iter()
            .find(|(k, _)| k == "error_description")
            .map(|(_, v)| v.as_str())
            .unwrap_or(err);
        write_http_html(
            stream,
            400,
            "Authorization Failed",
            &format!("Provider returned error: {desc}"),
            false,
        );
        return Some(Err(anyhow!("OAuth authorization denied: {desc}")));
    }

    let code = params
        .iter()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.trim().to_owned())
        .filter(|s| !s.is_empty());
    let Some(code) = code else {
        write_http_html(
            stream,
            400,
            "Missing Code",
            "Authorization code missing in callback.",
            false,
        );
        return Some(Err(anyhow!("OAuth callback missing authorization code")));
    };

    write_http_html(
        stream,
        200,
        "Connected to Fern",
        "Authorization succeeded. You can close this browser tab and return to Fern.",
        true,
    );
    Some(Ok(code))
}

fn write_http_html(stream: &mut TcpStream, status: u16, title: &str, message: &str, ok: bool) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let accent = if ok { "#4ade80" } else { "#f87171" };
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{t}</title>\
         <style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#0f1117;\
         color:#e6eaf2;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;}}\
         .card{{max-width:420px;padding:28px 32px;border-radius:12px;background:#161b26;\
         border:1px solid #262e40;text-align:center;}}h1{{margin:0 0 10px;font-size:20px;\
         color:{accent};}}p{{margin:0;color:#9aa4b2;font-size:14px;line-height:1.5;}}</style></head>\
         <body><div class=\"card\"><h1>{t}</h1><p>{m}</p></div></body></html>",
        t = escape_html(title),
        m = escape_html(message),
    );
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn random_hex_state() -> Result<String> {
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|e| anyhow!("failed to generate OAuth state: {e}"))?;
    Ok(hex::encode(bytes))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn encode_form_pairs(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn parse_query_params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::Permission;

    #[test]
    fn percent_encode_and_query_decode_roundtrip() {
        let encoded = encode_form_pairs(&[
            ("scope", "tasks:read tasks:write".to_owned()),
            ("redirect_uri", "http://127.0.0.1:18481/callback".to_owned()),
        ]);
        let parsed = parse_query_params(&encoded);
        assert_eq!(
            parsed,
            vec![
                ("scope".to_owned(), "tasks:read tasks:write".to_owned()),
                (
                    "redirect_uri".to_owned(),
                    "http://127.0.0.1:18481/callback".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn loopback_oauth_authorize_and_refresh_store_tokens_in_vault() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let vault = SecretsVault::open(tmp.path().join("vault.bin"), tmp.path().join("vault.key"))?;
        vault.set("com.test.oauth", "client_secret", "super-secret-xyz")?;

        // Local mock OAuth token server.
        let token_srv = TcpListener::bind("127.0.0.1:0")?;
        let token_port = token_srv.local_addr()?.port();
        let token_thread = thread::spawn(move || {
            for idx in 0..2 {
                let (mut stream, _) = token_srv.accept().expect("accept token request");
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .expect("set read timeout");
                let mut raw = Vec::new();
                let mut tmp_buf = [0u8; 4096];
                let mut expected_total: Option<usize> = None;
                loop {
                    let n = stream.read(&mut tmp_buf).expect("read token request");
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&tmp_buf[..n]);
                    if expected_total.is_none()
                        && let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n")
                    {
                        let hdr_end = pos + 4;
                        let hdrs = String::from_utf8_lossy(&raw[..pos]);
                        let content_len = hdrs
                            .lines()
                            .find_map(|line| {
                                let (k, v) = line.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        expected_total = Some(hdr_end + content_len);
                    }
                    if let Some(total) = expected_total
                        && raw.len() >= total
                    {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&raw).into_owned();
                assert!(
                    text.to_ascii_lowercase().contains("authorization: basic "),
                    "expected Basic auth header, got: {text}"
                );
                let json_body = if idx == 0 {
                    assert!(text.contains("grant_type=authorization_code"));
                    assert!(text.contains("code=auth-code-999"));
                    r#"{"access_token":"tok-initial-123","refresh_token":"ref-456","token_type":"bearer","expires_in":3600,"scope":"tasks:read tasks:write"}"#
                } else {
                    assert!(text.contains("grant_type=refresh_token"));
                    assert!(text.contains("refresh_token=ref-456"));
                    r#"{"access_token":"tok-refreshed-789","token_type":"bearer","expires_in":7200}"#
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_body}",
                    json_body.len()
                );
                stream.write_all(resp.as_bytes()).expect("write token response");
            }
        });

        // Reserve a free callback port for the test.
        let cb_port = {
            let probe = TcpListener::bind("127.0.0.1:0")?;
            probe.local_addr()?.port()
        };

        let perms = PermissionSet::new(&[
            Permission::Network(vec![format!("127.0.0.1:{token_port}")]),
            Permission::NetworkLocal,
        ]);
        let cancel = AtomicBool::new(false);

        let summary = run_oauth_authorize(
            "com.test.oauth",
            &perms,
            &vault,
            &OAuthAuthorizeRequest {
                authorize_url: format!("http://127.0.0.1:{token_port}/oauth/authorize"),
                token_url: format!("http://127.0.0.1:{token_port}/oauth/token"),
                client_id: "my-client".to_owned(),
                client_secret_key: Some("client_secret".to_owned()),
                scopes: vec!["tasks:read".to_owned(), "tasks:write".to_owned()],
                redirect_host: "127.0.0.1".to_owned(),
                redirect_port: cb_port,
                redirect_path: "/callback".to_owned(),
                token_auth: OAuthTokenAuth::Basic,
                save_access_token_as: "access_token".to_owned(),
                save_refresh_token_as: Some("refresh_token".to_owned()),
                extra_authorize_params: Vec::new(),
                timeout_sec: 10,
            },
            &cancel,
            move |opened_url| {
                // Simulate browser hitting /favicon.ico first, then the real callback with matching state.
                thread::spawn(move || {
                    let query = opened_url.split_once('?').map(|(_, q)| q).unwrap_or("");
                    let state = parse_query_params(query)
                        .into_iter()
                        .find(|(k, _)| k == "state")
                        .map(|(_, v)| v)
                        .expect("state param present");

                    let mut s1 = TcpStream::connect(("127.0.0.1", cb_port)).expect("connect favicon");
                    let _ = s1.write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");

                    let mut s2 = TcpStream::connect(("127.0.0.1", cb_port)).expect("connect callback");
                    let req = format!(
                        "GET /callback?code=auth-code-999&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
                    );
                    s2.write_all(req.as_bytes()).expect("send callback");
                    let mut reply = String::new();
                    let _ = s2.read_to_string(&mut reply);
                    assert!(reply.contains("200 OK"));
                });
            },
        )?;

        assert!(summary.ok);
        assert!(summary.has_refresh_token);
        assert_eq!(summary.expires_in, Some(3600));
        assert_eq!(
            vault.get("com.test.oauth", "access_token")?.as_deref(),
            Some("tok-initial-123")
        );
        assert_eq!(
            vault.get("com.test.oauth", "refresh_token")?.as_deref(),
            Some("ref-456")
        );

        let refreshed = run_oauth_refresh(
            "com.test.oauth",
            &perms,
            &vault,
            &OAuthRefreshRequest {
                token_url: format!("http://127.0.0.1:{token_port}/oauth/token"),
                client_id: "my-client".to_owned(),
                client_secret_key: Some("client_secret".to_owned()),
                refresh_token_key: "refresh_token".to_owned(),
                scopes: vec!["tasks:read".to_owned(), "tasks:write".to_owned()],
                token_auth: OAuthTokenAuth::Basic,
                save_access_token_as: "access_token".to_owned(),
                save_refresh_token_as: Some("refresh_token".to_owned()),
            },
        )?;
        assert!(refreshed.ok);
        assert_eq!(refreshed.expires_in, Some(7200));
        assert_eq!(
            vault.get("com.test.oauth", "access_token")?.as_deref(),
            Some("tok-refreshed-789")
        );

        token_thread.join().expect("mock token server thread");
        Ok(())
    }

    #[test]
    fn oauth_rejects_remote_http_param_pollution_fragments_and_token_redirects() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let vault = SecretsVault::open(tmp.path().join("v.bin"), tmp.path().join("v.key"))?;
        vault.set("p", "sec", "my-secret-value")?;
        let perms = PermissionSet::new(&[
            Permission::Network(vec!["*".to_owned()]),
            Permission::NetworkLocal,
        ]);
        let cancel = AtomicBool::new(false);

        let base_req = OAuthAuthorizeRequest {
            authorize_url: "https://auth.example.com/authorize".to_owned(),
            token_url: "https://auth.example.com/token".to_owned(),
            client_id: "cid".to_owned(),
            client_secret_key: Some("sec".to_owned()),
            scopes: vec!["tasks:read".to_owned()],
            redirect_host: "127.0.0.1".to_owned(),
            redirect_port: 18482,
            redirect_path: "/callback".to_owned(),
            token_auth: OAuthTokenAuth::Basic,
            save_access_token_as: "access_token".to_owned(),
            save_refresh_token_as: None,
            extra_authorize_params: Vec::new(),
            timeout_sec: 5,
        };

        // 1. Plaintext http:// to remote host or LAN IP must fail even with network:local.
        for bad_url in [
            "http://auth.example.com/authorize",
            "http://192.168.1.20/authorize",
            "http://evil.localhost/authorize",
        ] {
            let mut r = base_req.clone();
            r.authorize_url = bad_url.to_owned();
            let err = run_oauth_authorize("p", &perms, &vault, &r, &cancel, |_| {}).unwrap_err();
            assert!(
                format!("{err:#}").contains("loopback") || format!("{err:#}").contains("https://"),
                "unexpected error for {bad_url}: {err:#}"
            );
        }

        // 2. Fragment `#` and reserved query keys in authorizeUrl or extraAuthorizeParams must fail.
        for reserved in ["redirect_uri", "state", "client_id", "response_type", "scope", "code_challenge"] {
            let mut r1 = base_req.clone();
            r1.authorize_url = format!("https://auth.example.com/authorize?{reserved}=evil");
            assert!(
                run_oauth_authorize("p", &perms, &vault, &r1, &cancel, |_| {}).is_err(),
                "authorizeUrl allowed reserved param {reserved}"
            );

            let mut r2 = base_req.clone();
            r2.extra_authorize_params = vec![(reserved.to_owned(), "evil".to_owned())];
            assert!(
                run_oauth_authorize("p", &perms, &vault, &r2, &cancel, |_| {}).is_err(),
                "extraAuthorizeParams allowed reserved param {reserved}"
            );
        }
        {
            let mut r = base_req;
            r.authorize_url = "https://auth.example.com/authorize#frag".to_owned();
            assert!(run_oauth_authorize("p", &perms, &vault, &r, &cancel, |_| {}).is_err());
        }

        // 3. Token exchange must refuse HTTP 307 redirects (max_redirects = 0).
        let redir_srv = TcpListener::bind("127.0.0.1:0")?;
        let redir_port = redir_srv.local_addr()?.port();
        let redir_thread = thread::spawn(move || {
            if let Ok((mut stream, _)) = redir_srv.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                let mut raw = Vec::new();
                let mut tmp_buf = [0u8; 4096];
                let mut expected_total: Option<usize> = None;
                while let Ok(n) = stream.read(&mut tmp_buf) {
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&tmp_buf[..n]);
                    if expected_total.is_none()
                        && let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n")
                    {
                        let hdrs = String::from_utf8_lossy(&raw[..pos]);
                        let cl = hdrs
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        expected_total = Some(pos + 4 + cl);
                    }
                    if let Some(total) = expected_total
                        && raw.len() >= total
                    {
                        break;
                    }
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                let _ = stream.flush();
            }
        });
        vault.set("p", "ref", "refresh-tok-secret")?;
        let redir_err = run_oauth_refresh(
            "p",
            &perms,
            &vault,
            &OAuthRefreshRequest {
                token_url: format!("http://127.0.0.1:{redir_port}/oauth/token"),
                client_id: "cid".to_owned(),
                client_secret_key: Some("sec".to_owned()),
                refresh_token_key: "ref".to_owned(),
                scopes: vec![],
                token_auth: OAuthTokenAuth::Body,
                save_access_token_as: "access_token".to_owned(),
                save_refresh_token_as: None,
            },
        )
        .unwrap_err();
        assert!(
            format!("{redir_err:#}").contains("redirects are forbidden"),
            "{redir_err:#}"
        );
        let _ = redir_thread.join();

        Ok(())
    }

    #[test]
    fn oauth_callback_ignores_bad_host_and_unauthenticated_error_and_supports_pkce_s256() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let vault = SecretsVault::open(tmp.path().join("v.bin"), tmp.path().join("v.key"))?;

        let token_srv = TcpListener::bind("127.0.0.1:0")?;
        let token_port = token_srv.local_addr()?.port();
        let captured_challenge = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let challenge_for_srv = std::sync::Arc::clone(&captured_challenge);

        let token_thread = thread::spawn(move || {
            let (mut stream, _) = token_srv.accept().expect("accept token");
            let mut raw = Vec::new();
            let mut tmp_buf = [0u8; 4096];
            let mut expected_total: Option<usize> = None;
            loop {
                let n = stream.read(&mut tmp_buf).expect("read token");
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&tmp_buf[..n]);
                if expected_total.is_none()
                    && let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    let hdrs = String::from_utf8_lossy(&raw[..pos]);
                    let cl = hdrs
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    expected_total = Some(pos + 4 + cl);
                }
                if let Some(total) = expected_total
                    && raw.len() >= total
                {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&raw);
            let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
            let form = parse_query_params(body);
            let verifier = form
                .iter()
                .find(|(k, _)| k == "code_verifier")
                .map(|(_, v)| v.as_str())
                .expect("code_verifier must be sent when pkce=true");
            let expected_challenge =
                BASE64_URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            assert_eq!(expected_challenge, *challenge_for_srv.lock().unwrap());
            let json_body = r#"{"access_token":"pkce-tok","token_type":"bearer"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_body}",
                json_body.len()
            );
            stream.write_all(resp.as_bytes()).expect("write resp");
        });

        let cb_port = {
            let probe = TcpListener::bind("127.0.0.1:0")?;
            probe.local_addr()?.port()
        };
        let perms = PermissionSet::new(&[
            Permission::Network(vec![format!("127.0.0.1:{token_port}")]),
            Permission::NetworkLocal,
        ]);
        let cancel = AtomicBool::new(false);
        let challenge_for_open = std::sync::Arc::clone(&captured_challenge);

        let summary = run_oauth_authorize_with_options(
            "com.test.pkce",
            &perms,
            &vault,
            &OAuthAuthorizeRequest {
                authorize_url: format!("http://127.0.0.1:{token_port}/oauth/authorize"),
                token_url: format!("http://127.0.0.1:{token_port}/oauth/token"),
                client_id: "pkce-client".to_owned(),
                client_secret_key: None,
                scopes: vec!["read".to_owned()],
                redirect_host: "127.0.0.1".to_owned(),
                redirect_port: cb_port,
                redirect_path: "/callback".to_owned(),
                token_auth: OAuthTokenAuth::Body,
                save_access_token_as: "access_token".to_owned(),
                save_refresh_token_as: None,
                extra_authorize_params: Vec::new(),
                timeout_sec: 10,
            },
            &OAuthAuthorizeOptions { pkce: true },
            &cancel,
            move |opened_url| {
                let query = opened_url.split_once('?').map(|(_, q)| q).unwrap_or("");
                let params = parse_query_params(query);
                let state = params
                    .iter()
                    .find(|(k, _)| k == "state")
                    .map(|(_, v)| v.clone())
                    .expect("state present");
                let challenge = params
                    .iter()
                    .find(|(k, _)| k == "code_challenge")
                    .map(|(_, v)| v.clone())
                    .expect("code_challenge present");
                assert_eq!(
                    params
                        .iter()
                        .find(|(k, _)| k == "code_challenge_method")
                        .map(|(_, v)| v.as_str()),
                    Some("S256")
                );
                *challenge_for_open.lock().unwrap() = challenge;

                thread::spawn(move || {
                    // 1. Attacker sends bad Host header -> must get 400 and NOT abort flow.
                    let mut s_host = TcpStream::connect(("127.0.0.1", cb_port)).unwrap();
                    let _ = s_host.write_all(
                        format!("GET /callback?code=evil&state={state} HTTP/1.1\r\nHost: attacker.com\r\n\r\n")
                            .as_bytes(),
                    );
                    let mut r1 = String::new();
                    let _ = s_host.read_to_string(&mut r1);
                    assert!(r1.contains("400 Bad Request"), "{r1}");

                    // 2. Attacker sends `?error=spoofed` WITHOUT state -> must get 400 and NOT abort flow.
                    let mut s_err = TcpStream::connect(("127.0.0.1", cb_port)).unwrap();
                    let _ = s_err.write_all(
                        format!("GET /callback?error=access_denied&error_description=phishing HTTP/1.1\r\nHost: 127.0.0.1:{cb_port}\r\n\r\n")
                            .as_bytes(),
                    );
                    let mut r2 = String::new();
                    let _ = s_err.read_to_string(&mut r2);
                    assert!(r2.contains("400 Bad Request"), "{r2}");

                    // 3. Attacker sends wrong state -> must get 400 and NOT abort flow.
                    let mut s_bad_state = TcpStream::connect(("127.0.0.1", cb_port)).unwrap();
                    let _ = s_bad_state.write_all(
                        format!("GET /callback?code=evil&state=wrong HTTP/1.1\r\nHost: 127.0.0.1:{cb_port}\r\n\r\n")
                            .as_bytes(),
                    );
                    let mut r3 = String::new();
                    let _ = s_bad_state.read_to_string(&mut r3);
                    assert!(r3.contains("400 Bad Request"), "{r3}");

                    // 4. Genuine callback succeeds!
                    let mut s_ok = TcpStream::connect(("127.0.0.1", cb_port)).unwrap();
                    let _ = s_ok.write_all(
                        format!("GET /callback?code=real-code&state={state} HTTP/1.1\r\nHost: 127.0.0.1:{cb_port}\r\n\r\n")
                            .as_bytes(),
                    );
                    let mut r4 = String::new();
                    let _ = s_ok.read_to_string(&mut r4);
                    assert!(r4.contains("200 OK"), "{r4}");
                });
            },
        )?;
        assert!(summary.ok);
        assert_eq!(
            vault.get("com.test.pkce", "access_token")?.as_deref(),
            Some("pkce-tok")
        );
        token_thread.join().expect("token thread");

        // Verify serde default for OAuthAuthorizeOptions is pkce = false.
        let opts: OAuthAuthorizeOptions = serde_json::from_str("{}")?;
        assert!(!opts.pkce);
        Ok(())
    }

    #[test]
    fn localhost_listener_fails_closed_when_ipv6_port_already_occupied() -> Result<()> {
        // Bind an ephemeral port on [::1]; if IPv6 loopback is supported on this host,
        // `bind_loopback_listeners("localhost", port)` must refuse to fall back to IPv4-only.
        let Ok(v6_occupier) = TcpListener::bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0))
        else {
            return Ok(());
        };
        let port = v6_occupier.local_addr()?.port();
        let res = bind_loopback_listeners("localhost", port);
        assert!(res.is_err(), "expected fail-closed error when [::1]:{port} is occupied");
        drop(v6_occupier);
        Ok(())
    }
}
