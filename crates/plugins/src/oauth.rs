//! Zero-secret-exposure OAuth 2.0 Authorization Code (loopback callback) and token refresh helper.
//!
//! Credentials (`client_secret`, `access_token`, `refresh_token`) are read from and written directly
//! into the plugin's encrypted [`SecretsVault`] on the Rust host side — JS plugin code never sees
//! raw token values and instead uses `${secret:access_token}` in `fern.http.fetch` headers.

use std::io::{Read as _, Write as _};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use ureq::http::Uri;

use crate::http::{HttpRequest, execute_http_request, validate_target_uri};
use crate::permissions::PermissionSet;
use crate::vault::SecretsVault;

pub const DEFAULT_REDIRECT_HOST: &str = "127.0.0.1";
pub const DEFAULT_REDIRECT_PORT: u16 = 18481;
pub const DEFAULT_REDIRECT_PATH: &str = "/callback";
pub const DEFAULT_OAUTH_TIMEOUT_SEC: u64 = 180;
const MAX_CALLBACK_REQ_BYTES: usize = 16 * 1024;
const LOOPBACK_POLL_INTERVAL: Duration = Duration::from_millis(25);

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
    let client_id = req.client_id.trim();
    ensure!(!client_id.is_empty(), "OAuth clientId must not be empty");
    let save_access = req.save_access_token_as.trim();
    ensure!(!save_access.is_empty(), "OAuth saveAccessTokenAs must not be empty");

    let auth_uri: Uri = req
        .authorize_url
        .trim()
        .parse()
        .with_context(|| format!("invalid authorizeUrl {:?}", req.authorize_url))?;
    let token_uri: Uri = req
        .token_url
        .trim()
        .parse()
        .with_context(|| format!("invalid tokenUrl {:?}", req.token_url))?;

    validate_oauth_endpoint(&auth_uri, permissions, "authorizeUrl")?;
    validate_oauth_endpoint(&token_uri, permissions, "tokenUrl")?;

    let client_secret = match req
        .client_secret_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(key) => {
            let val = vault.get(plugin_id, key)?.ok_or_else(|| {
                anyhow!("missing secret {key:?} in SecretsVault; save it in plugin settings first")
            })?;
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

    let mut query_pairs: Vec<(&str, String)> = vec![
        ("response_type", "code".to_owned()),
        ("client_id", client_id.to_owned()),
        ("redirect_uri", redirect_uri.clone()),
        ("state", state.clone()),
    ];
    if !scope_str.is_empty() {
        query_pairs.push(("scope", scope_str.clone()));
    }
    for (k, v) in &req.extra_authorize_params {
        if !k.trim().is_empty() {
            query_pairs.push((k.trim(), v.clone()));
        }
    }

    let separator = if req.authorize_url.contains('?') { "&" } else { "?" };
    let full_authorize_url = format!(
        "{}{}{}",
        req.authorize_url.trim(),
        separator,
        encode_form_pairs(&query_pairs)
    );

    let listeners = bind_loopback_listeners(&req.redirect_host, req.redirect_port)?;
    on_open_url(full_authorize_url);

    let timeout = Duration::from_secs(req.timeout_sec.clamp(5, 300));
    let code = wait_for_oauth_code(&listeners, &req.redirect_path, &state, timeout, cancel)?;

    let mut form_pairs: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("code", code),
        ("redirect_uri", redirect_uri),
    ];
    if !scope_str.is_empty() {
        form_pairs.push(("scope", scope_str));
    }

    exchange_token_endpoint(
        plugin_id,
        permissions,
        vault,
        req.token_url.trim(),
        client_id,
        client_secret.as_deref(),
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

    let token_uri: Uri = req
        .token_url
        .trim()
        .parse()
        .with_context(|| format!("invalid tokenUrl {:?}", req.token_url))?;
    validate_oauth_endpoint(&token_uri, permissions, "tokenUrl")?;

    let refresh_token = vault
        .get(plugin_id, refresh_key)?
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("no refresh token stored under secret {refresh_key:?}"))?;

    let client_secret = match req
        .client_secret_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(key) => vault.get(plugin_id, key)?.filter(|s| !s.trim().is_empty()),
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
        ("refresh_token", refresh_token),
    ];
    if !scope_str.is_empty() {
        form_pairs.push(("scope", scope_str));
    }

    exchange_token_endpoint(
        plugin_id,
        permissions,
        vault,
        req.token_url.trim(),
        client_id,
        client_secret.as_deref(),
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

    match token_auth {
        OAuthTokenAuth::Basic => {
            let secret = client_secret.unwrap_or("");
            let encoded = BASE64_STANDARD.encode(format!("{client_id}:{secret}"));
            headers.push(("Authorization".to_owned(), format!("Basic {encoded}")));
        }
        OAuthTokenAuth::Body => {
            form_pairs.push(("client_id", client_id.to_owned()));
            if let Some(secret) = client_secret {
                form_pairs.push(("client_secret", secret.to_owned()));
            }
        }
    }

    let body = encode_form_pairs(form_pairs).into_bytes();
    let http_req = HttpRequest {
        method: "POST".to_owned(),
        url: token_url.to_owned(),
        headers,
        body: Some(body),
        timeout_ms: Some(20_000),
    };

    let resp = execute_http_request(plugin_id, permissions, Some(vault), &http_req)?;
    if !resp.ok() {
        let preview = vault.scrub_secrets(plugin_id, &resp.body_text());
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

fn validate_oauth_endpoint(uri: &Uri, permissions: &PermissionSet, label: &str) -> Result<()> {
    let scheme = uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    ensure!(
        scheme == "https" || (scheme == "http" && permissions.allows_local()),
        "OAuth {label} must use https://"
    );
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
        if let Ok(v6) = TcpListener::bind(v6_addr)
            && v6.set_nonblocking(true).is_ok()
        {
            listeners.push(v6);
        }
    }
    Ok(listeners)
}

fn wait_for_oauth_code(
    listeners: &[TcpListener],
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
                    if let Some(outcome) =
                        handle_callback_connection(&mut stream, expected_path, expected_state)
                    {
                        return outcome;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(anyhow!("OAuth callback accept error: {err}")),
            }
        }
        thread::sleep(LOOPBACK_POLL_INTERVAL);
    }
    bail!("OAuth authorization timed out waiting for browser callback")
}

fn handle_callback_connection(
    stream: &mut TcpStream,
    expected_path: &str,
    expected_state: &str,
) -> Option<Result<String>> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));

    let mut buf = [0u8; MAX_CALLBACK_REQ_BYTES];
    let n = match stream.read(&mut buf) {
        Ok(0) | Err(_) => return None,
        Ok(n) => n,
    };
    let raw = String::from_utf8_lossy(&buf[..n]);
    let first_line = raw.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");

    if method != "GET" {
        write_http_html(stream, 405, "Method Not Allowed", "Unsupported request method.", false);
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

    let got_state = params
        .iter()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    if !constant_time_eq(got_state.as_bytes(), expected_state.as_bytes()) {
        write_http_html(
            stream,
            400,
            "Invalid OAuth State",
            "State parameter mismatch. Please retry login from Fern.",
            false,
        );
        return Some(Err(anyhow!("OAuth state mismatch")));
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
}
