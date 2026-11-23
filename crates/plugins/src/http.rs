//! SSRF-safe HTTP client (`ureq`) with manual redirect validation, IP filtering,
//! response size caps, `${secret:key}` header interpolation, and a bounded 4-thread pool.

use std::collections::VecDeque;
use std::io::Read as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use futures::channel::oneshot;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use zeroize::Zeroizing;

use crate::permissions::{PermissionSet, normalize_host};
use crate::vault::SecretsVault;

pub const MAX_REDIRECTS: usize = 5;
pub const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(20);
pub const HTTP_POOL_THREADS: usize = 4;

#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Checks whether an IP address is disallowed for public plugin network access.
///
/// When `allow_local` is `true` (`network:local`), loopback and RFC1918/ULA private LAN ranges
/// are permitted, while cloud link-local metadata (`169.254.0.0/16`, `fe80::/10`), unspecified
/// (`0.0.0.0/8`, `::`), and multicast/reserved ranges remain blocked.
pub fn is_ip_blocked(ip: IpAddr, allow_local: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => is_v4_blocked(v4, allow_local),
        IpAddr::V6(v6) => is_v6_blocked(v6, allow_local),
    }
}

pub(crate) fn is_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

fn is_v4_blocked(ip: Ipv4Addr, allow_local: bool) -> bool {
    let o = ip.octets();
    // Always block unspecified (0.0.0.0/8), link-local/metadata (169.254.0.0/16),
    // multicast (224.0.0.0/4), broadcast, reserved (>=240), IETF protocol assignments
    // (192.0.0.0/24), and documentation ranges.
    if o[0] == 0
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || o[0] >= 240
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
    {
        return true;
    }
    // CGNAT 100.64.0.0/10 and benchmarking 198.18.0.0/15
    if (o[0] == 100 && (o[1] & 0b1100_0000) == 64) || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) {
        return !allow_local;
    }
    if ip.is_loopback() || ip.is_private() {
        return !allow_local;
    }
    false
}

fn is_v6_blocked(ip: Ipv6Addr, allow_local: bool) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_v4_blocked(mapped, allow_local);
    }
    let seg = ip.segments();
    // IPv4-compatible ::/96
    if seg[0..6] == [0, 0, 0, 0, 0, 0] && (seg[6] != 0 || seg[7] > 1) {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            (seg[6] & 0xff) as u8,
            (seg[7] >> 8) as u8,
            (seg[7] & 0xff) as u8,
        );
        return is_v4_blocked(v4, allow_local);
    }
    // Always block IPv6 transition/tunnel/special ranges that can embed arbitrary IPv4 targets:
    // - Well-known NAT64 (64:ff9b::/96) and local-use NAT64 (64:ff9b:1::/48)
    // - IPv4-translated SIIT (::ffff:0:0:0/96)
    // - 6to4 (2002::/16) and Teredo (2001::/32)
    // - Deprecated site-local (fec0::/10) and discard-only (100::/64)
    let is_nat64_well_known = seg[0..6] == [0x0064, 0xff9b, 0, 0, 0, 0];
    let is_nat64_local = seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2] == 0x0001;
    let is_ipv4_translated = seg[0..6] == [0, 0, 0, 0, 0xffff, 0];
    let is_6to4 = seg[0] == 0x2002;
    let is_teredo = seg[0] == 0x2001 && seg[1] == 0x0000;
    let is_site_local = (seg[0] & 0xffc0) == 0xfec0;
    let is_discard = seg[0] == 0x0100 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0;
    // Always block unspecified (::), multicast (ff00::/8), link-local (fe80::/10), doc (2001:db8::/32).
    let is_link_local = (seg[0] & 0xffc0) == 0xfe80;
    let is_doc = seg[0] == 0x2001 && seg[1] == 0x0db8;
    if ip.is_unspecified()
        || ip.is_multicast()
        || is_link_local
        || is_doc
        || is_nat64_well_known
        || is_nat64_local
        || is_ipv4_translated
        || is_6to4
        || is_teredo
        || is_site_local
        || is_discard
    {
        return true;
    }
    let is_ula = (seg[0] & 0xfe00) == 0xfc00;
    if ip.is_loopback() || is_ula {
        return !allow_local;
    }
    false
}

fn is_localhost_hostname(host: &str) -> bool {
    let norm = normalize_host(host);
    norm == "localhost" || norm.ends_with(".localhost")
}

/// Fast, non-blocking syntactic validation of URI scheme, authority, host permission, and literal
/// IP SSRF rules. Does NOT perform blocking DNS resolution (DNS + resolved-IP SSRF checks run on
/// the `HttpPool` worker thread inside [`SsrfSafeResolver`] under `CONNECT_TIMEOUT`).
pub fn validate_target_uri_syntax(
    uri: &Uri,
    permissions: &PermissionSet,
) -> Result<(String, Option<u16>)> {
    let scheme = uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    ensure!(
        scheme == "http" || scheme == "https",
        "unsupported URL scheme {scheme:?}: only http and https are allowed"
    );
    let authority = uri
        .authority()
        .context("URL must contain a valid authority/host")?;
    ensure!(
        !authority.as_str().contains('@'),
        "URLs containing embedded userinfo (@) are forbidden"
    );
    let host = authority.host();
    ensure!(!host.is_empty(), "URL host must not be empty");
    let port = authority.port_u16();
    let effective_port = port.or(match scheme.as_str() {
        "https" => Some(443),
        "http" => Some(80),
        _ => None,
    });

    ensure!(
        permissions.allows_host(host, port) || permissions.allows_host(host, effective_port),
        "network permission denied for host {host:?}"
    );

    let allow_local = permissions.allows_local();
    let explicitly_allowed = permissions.allows_explicit_host(host, port)
        || permissions.allows_explicit_host(host, effective_port);
    let host_clean = host.trim_start_matches('[').trim_end_matches(']');

    if let Ok(ip) = host_clean.parse::<IpAddr>() {
        ensure!(
            !is_ip_blocked(ip, allow_local),
            "SSRF protection blocked connection to {ip}"
        );
        if !explicitly_allowed {
            ensure!(
                is_loopback_ip(ip),
                "SSRF protection requires loopback IP for network:local host {host:?}, blocked {ip}"
            );
        }
    }

    Ok((host.to_owned(), effective_port))
}

/// Validates URI scheme, host permission, and literal-IP SSRF rules without blocking on DNS.
///
/// Resolved DNS addresses are strictly checked at connect time inside [`SsrfSafeResolver`]
/// on the bounded [`HttpPool`] thread so `PluginWorker` never stalls on slow DNS lookups.
pub fn validate_target_uri(uri: &Uri, permissions: &PermissionSet) -> Result<(String, Option<u16>)> {
    validate_target_uri_syntax(uri, permissions)
}

pub(crate) fn target_requires_loopback_only(uri: &Uri, permissions: &PermissionSet) -> bool {
    let Some(authority) = uri.authority() else {
        return true;
    };
    let host = authority.host();
    if is_localhost_hostname(host) {
        return true;
    }
    let scheme = uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let port = authority.port_u16();
    let effective_port = port.or(match scheme.as_str() {
        "https" => Some(443),
        "http" => Some(80),
        _ => None,
    });
    !(permissions.allows_explicit_host(host, port)
        || permissions.allows_explicit_host(host, effective_port))
}

/// `ureq` resolver that re-checks resolved socket addresses at connect time to prevent DNS rebinding
/// and `*.localhost` non-loopback bypasses.
#[derive(Debug)]
pub(crate) struct SsrfSafeResolver {
    allow_local: bool,
    loopback_only: bool,
    inner: DefaultResolver,
}

impl SsrfSafeResolver {
    pub(crate) fn new(allow_local: bool, loopback_only: bool) -> Self {
        Self {
            allow_local,
            loopback_only,
            inner: DefaultResolver::default(),
        }
    }
}

impl Resolver for SsrfSafeResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> std::result::Result<ResolvedSocketAddrs, ureq::Error> {
        let host_forces_loopback = uri.host().is_some_and(is_localhost_hostname);
        let require_loopback = self.loopback_only || host_forces_loopback;
        let resolved = self.inner.resolve(uri, config, timeout)?;
        let mut filtered: ResolvedSocketAddrs = self.empty();
        for &addr in &resolved {
            if is_ip_blocked(addr.ip(), self.allow_local) {
                return Err(ureq::Error::Other(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("SSRF protection blocked IP {}", addr.ip()),
                ))));
            }
            if require_loopback && !is_loopback_ip(addr.ip()) {
                return Err(ureq::Error::Other(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "SSRF protection blocked non-loopback IP {} for loopback-only host",
                        addr.ip()
                    ),
                ))));
            }
            filtered.push(addr);
        }
        if filtered.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(filtered)
    }
}

/// Executes an HTTP request with SSRF validation, manual redirect loop (up to 5 hops),
/// 10 MiB body cap, 5s connect / 20s total timeout, and `${secret:key}` header interpolation.
pub fn execute_http_request(
    plugin_id: &str,
    permissions: &PermissionSet,
    vault: Option<&SecretsVault>,
    req: &HttpRequest,
) -> Result<HttpResponse> {
    execute_http_request_with_redirects(plugin_id, permissions, vault, req, MAX_REDIRECTS)
}

pub(crate) fn execute_http_request_with_redirects(
    plugin_id: &str,
    permissions: &PermissionSet,
    vault: Option<&SecretsVault>,
    req: &HttpRequest,
    max_redirects: usize,
) -> Result<HttpResponse> {
    let total_budget = req
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(TOTAL_TIMEOUT)
        .min(TOTAL_TIMEOUT)
        .max(Duration::from_millis(100));
    let deadline = Instant::now() + total_budget;

    let mut current_method = req.method.trim().to_ascii_uppercase();
    if current_method.is_empty() {
        current_method = "GET".to_owned();
    }
    ensure!(
        matches!(
            current_method.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
        ),
        "unsupported HTTP method {current_method:?}"
    );

    let mut current_uri: Uri = req
        .url
        .trim()
        .parse()
        .with_context(|| format!("invalid URL {:?}", req.url))?;
    let mut current_body = req.body.clone();

    // Interpolate `${secret:key}` in headers up front and track which headers carried secrets.
    let mut interpolated_any_secret = false;
    let mut prepared_headers: Vec<(String, Zeroizing<String>, bool)> =
        Vec::with_capacity(req.headers.len());
    for (name, raw_val) in &req.headers {
        let had_secret = raw_val.contains("${secret:");
        let interpolated = if had_secret {
            interpolated_any_secret = true;
            let v = vault.context("secrets vault unavailable for header interpolation")?;
            Zeroizing::new(v.interpolate_secrets(plugin_id, raw_val)?)
        } else {
            Zeroizing::new(raw_val.clone())
        };
        let is_sensitive = had_secret
            || name.eq_ignore_ascii_case("authorization")
            || name.eq_ignore_ascii_case("cookie")
            || name.eq_ignore_ascii_case("proxy-authorization");
        prepared_headers.push((name.clone(), interpolated, is_sensitive));
    }

    let initial_origin = origin_key(&current_uri);
    let mut same_origin = true;
    let mut body_stripped_by_redirect = false;
    let allow_local = permissions.allows_local();

    for hop in 0..=max_redirects {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "HTTP request timed out");

        validate_target_uri_syntax(&current_uri, permissions)?;
        let loopback_only = target_requires_loopback_only(&current_uri, permissions);

        let config = Config::builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .proxy(None)
            .timeout_connect(Some(CONNECT_TIMEOUT.min(remaining)))
            .timeout_global(Some(remaining))
            .build();
        let resolver = SsrfSafeResolver::new(allow_local, loopback_only);
        let agent = ureq::Agent::with_parts(config, DefaultConnector::default(), resolver);

        let mut builder = ureq::http::Request::builder()
            .method(current_method.as_str())
            .uri(&current_uri)
            .header("User-Agent", format!("fern-plugin/{}", env!("CARGO_PKG_VERSION")));
        for (name, val, sensitive) in &prepared_headers {
            if *sensitive && !same_origin {
                continue;
            }
            if body_stripped_by_redirect && is_body_header(name) {
                continue;
            }
            builder = builder.header(name.as_str(), val.as_str());
        }

        let response_res = if let Some(body_bytes) = current_body.as_deref() {
            let http_req = builder
                .body(body_bytes)
                .context("failed to build HTTP request")?;
            agent.run(http_req)
        } else {
            let http_req = builder
                .body(())
                .context("failed to build HTTP request")?;
            agent.run(http_req)
        };

        let mut response = response_res.map_err(|err| {
            let msg = format!("{err}");
            let scrubbed = vault
                .map(|v| v.scrub_secrets(plugin_id, &msg))
                .unwrap_or(msg);
            anyhow!("HTTP request failed: {scrubbed}")
        })?;

        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if max_redirects == 0 {
                bail!("HTTP redirects are forbidden for this request (received HTTP {status})");
            }
            if hop >= max_redirects {
                bail!("HTTP request exceeded maximum of {max_redirects} redirects");
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .context("HTTP redirect response missing valid Location header")?;
            let next_uri = resolve_redirect_uri(&current_uri, location)?;
            ensure_no_scheme_downgrade(&current_uri, &next_uri)?;

            // Sticky cross-origin latch: once a redirect chain leaves `initial_origin`,
            // credentials and request bodies are never re-attached even if redirected back.
            same_origin &= origin_key(&next_uri) == initial_origin;

            let method_changes =
                status == 303 || ((status == 301 || status == 302) && current_method == "POST");
            if method_changes {
                current_method = "GET".to_owned();
                current_body = None;
                body_stripped_by_redirect = true;
            } else if !same_origin {
                current_body = None;
                body_stripped_by_redirect = true;
            }
            current_uri = next_uri;
            continue;
        }

        let mut headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (k.as_str().to_owned(), s.to_owned()))
            })
            .collect();

        let mut buf = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_RESPONSE_BYTES as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|err| {
                let msg = format!("failed reading HTTP response body: {err}");
                anyhow!(
                    "{}",
                    vault
                        .map(|v| v.scrub_secrets(plugin_id, &msg))
                        .unwrap_or(msg)
                )
            })?;
        ensure!(
            buf.len() <= MAX_RESPONSE_BYTES,
            "HTTP response body exceeds {MAX_RESPONSE_BYTES} bytes limit"
        );

        // If any `${secret:...}` placeholder was sent in request headers, scrub reflected secrets
        // from response headers and body so a reflecting endpoint cannot exfiltrate them to JS
        // (while leaving non-UTF-8 binary responses byte-for-byte intact when no secret is reflected).
        if interpolated_any_secret
            && let Some(v) = vault
        {
            for (_, val) in &mut headers {
                *val = v.scrub_secrets(plugin_id, val);
            }
            let text = String::from_utf8_lossy(&buf);
            let scrubbed = v.scrub_secrets(plugin_id, &text);
            if scrubbed.as_str() != text.as_ref() {
                buf = scrubbed.into_bytes();
            }
        }

        return Ok(HttpResponse {
            status,
            url: current_uri.to_string(),
            headers,
            body: buf,
        });
    }

    bail!("HTTP request exceeded maximum of {max_redirects} redirects")
}

fn ensure_no_scheme_downgrade(current_uri: &Uri, next_uri: &Uri) -> Result<()> {
    let cur_scheme = current_uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let next_scheme = next_uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    ensure!(
        !(cur_scheme == "https" && next_scheme == "http"),
        "HTTP redirect blocked: HTTPS to HTTP downgrade is forbidden"
    );
    Ok(())
}

fn is_body_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("content-type")
        || name.eq_ignore_ascii_case("content-encoding")
        || name.eq_ignore_ascii_case("transfer-encoding")
}

fn origin_key(uri: &Uri) -> (String, String, u16) {
    let scheme = uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let host = uri
        .host()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let port = uri.port_u16().unwrap_or(if scheme == "https" { 443 } else { 80 });
    (scheme, host, port)
}

pub(crate) fn resolve_redirect_uri(base: &Uri, location: &str) -> Result<Uri> {
    let loc = location.trim();
    ensure!(!loc.is_empty(), "empty redirect Location header");
    if loc.contains("://") {
        return loc
            .parse::<Uri>()
            .with_context(|| format!("invalid redirect URL {loc:?}"));
    }
    let scheme = base.scheme_str().context("base URI missing scheme")?;
    let authority = base
        .authority()
        .map(|a| a.as_str())
        .context("base URI missing authority")?;
    let resolved = if let Some(rest) = loc.strip_prefix("//") {
        format!("{scheme}://{rest}")
    } else if loc.starts_with('/') {
        format!("{scheme}://{authority}{loc}")
    } else if loc.starts_with('?') {
        format!("{scheme}://{authority}{}{loc}", base.path())
    } else {
        let base_path = base.path();
        let parent = match base_path.rfind('/') {
            Some(idx) => &base_path[..=idx],
            None => "/",
        };
        format!("{scheme}://{authority}{parent}{loc}")
    };
    resolved
        .parse::<Uri>()
        .with_context(|| format!("invalid resolved redirect URL {resolved:?}"))
}

struct PoolState {
    queue: VecDeque<Box<dyn FnOnce() + Send>>,
    closed: bool,
}

/// Bounded HTTP I/O worker pool (max 4 OS threads) so `PluginWorker` never blocks on network calls.
pub struct HttpPool {
    state: Arc<(Mutex<PoolState>, Condvar)>,
    workers: Vec<JoinHandle<()>>,
}

impl HttpPool {
    pub fn new(threads: usize) -> Self {
        let count = threads.clamp(1, HTTP_POOL_THREADS);
        let state = Arc::new((
            Mutex::new(PoolState {
                queue: VecDeque::new(),
                closed: false,
            }),
            Condvar::new(),
        ));
        let mut workers = Vec::with_capacity(count);
        for idx in 0..count {
            let shared = Arc::clone(&state);
            if let Ok(handle) = thread::Builder::new()
                .name(format!("fern-plugin-http-{idx}"))
                .spawn(move || {
                    loop {
                        let task = {
                            let (lock, cv) = &*shared;
                            let Ok(mut guard) = lock.lock() else {
                                break;
                            };
                            while guard.queue.is_empty() && !guard.closed {
                                match cv.wait(guard) {
                                    Ok(next) => guard = next,
                                    Err(_) => return,
                                }
                            }
                            if guard.closed && guard.queue.is_empty() {
                                break;
                            }
                            guard.queue.pop_front()
                        };
                        if let Some(job) = task {
                            job();
                        }
                    }
                })
            {
                workers.push(handle);
            }
        }
        Self { state, workers }
    }

    pub fn fetch(
        &self,
        plugin_id: String,
        permissions: PermissionSet,
        vault: SecretsVault,
        req: HttpRequest,
    ) -> oneshot::Receiver<Result<HttpResponse>> {
        let (tx, rx) = oneshot::channel();
        let (lock, cv) = &*self.state;
        if let Ok(mut guard) = lock.lock() {
            if guard.closed {
                let _ = tx.send(Err(anyhow!("HTTP pool is shutting down")));
                return rx;
            }
            guard.queue.push_back(Box::new(move || {
                let res = execute_http_request(&plugin_id, &permissions, Some(&vault), &req);
                let _ = tx.send(res);
            }));
            cv.notify_one();
        } else {
            let _ = tx.send(Err(anyhow!("HTTP pool lock poisoned")));
        }
        rx
    }

    /// Signals queued work to stop without waiting on slow sockets during app exit.
    pub fn close_nonblocking(&self) {
        let (lock, cv) = &*self.state;
        if let Ok(mut guard) = lock.lock() {
            guard.closed = true;
            guard.queue.clear();
            cv.notify_all();
        }
    }
}

impl Drop for HttpPool {
    fn drop(&mut self) {
        self.close_nonblocking();
        // Workers exit immediately once idle; do not block shutdown if a socket is mid-timeout.
        let _ = &self.workers;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;

    use super::*;
    use crate::permissions::Permission;

    #[test]
    fn ssrf_blocks_private_cidrs_and_redirect_to_loopback() -> Result<()> {
        assert!(is_ip_blocked("127.0.0.1".parse()?, false));
        assert!(is_ip_blocked("10.0.0.5".parse()?, false));
        assert!(is_ip_blocked("172.16.4.2".parse()?, false));
        assert!(is_ip_blocked("192.168.1.1".parse()?, false));
        assert!(is_ip_blocked("169.254.169.254".parse()?, false));
        assert!(is_ip_blocked("::1".parse()?, false));
        assert!(is_ip_blocked("fc00::1".parse()?, false));
        assert!(is_ip_blocked("fe80::1".parse()?, false));

        // Even with NetworkLocal, metadata 169.254.169.254 stays blocked while 127.0.0.1 is allowed.
        assert!(!is_ip_blocked("127.0.0.1".parse()?, true));
        assert!(is_ip_blocked("169.254.169.254".parse()?, true));

        // Spin up local HTTP server that redirects to 169.254.169.254 or disallowed host.
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let server = thread::spawn(move || {
            for stream in listener.incoming().take(3) {
                let Ok(mut sock) = stream else { break };
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let req_str = String::from_utf8_lossy(&buf);
                let resp = if req_str.starts_with("GET /redir-meta") {
                    "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\n\r\n".to_owned()
                } else if req_str.starts_with("GET /redir-evil") {
                    "HTTP/1.1 302 Found\r\nLocation: http://10.0.0.42/admin\r\nContent-Length: 0\r\n\r\n".to_owned()
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_owned()
                };
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let dir = tempfile::tempdir()?;
        let vault = SecretsVault::open(dir.path().join("v.vault"), dir.path().join("v.key"))?;
        vault.set("p1", "api_key", "secret-val")?;

        // 1. Without NetworkLocal, direct request to 127.0.0.1 fails even if listed in Network.
        let no_local = PermissionSet::new(&[Permission::Network(vec!["127.0.0.1".into()])]);
        let err = execute_http_request(
            "p1",
            &no_local,
            Some(&vault),
            &HttpRequest {
                method: "GET".into(),
                url: format!("http://{addr}/ok"),
                headers: vec![],
                body: None,
                timeout_ms: Some(2000),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("SSRF"), "{err}");

        // 2. With NetworkLocal + only 127.0.0.1 allowed, 302 redirect to 10.0.0.42 and 169.254.169.254 fail.
        let local_perms = PermissionSet::new(&[
            Permission::Network(vec![format!("127.0.0.1:{}", addr.port())]),
            Permission::NetworkLocal,
        ]);
        let err_redir = execute_http_request(
            "p1",
            &local_perms,
            Some(&vault),
            &HttpRequest {
                method: "GET".into(),
                url: format!("http://{addr}/redir-evil"),
                headers: vec![],
                body: None,
                timeout_ms: Some(2000),
            },
        )
        .unwrap_err();
        assert!(err_redir.to_string().contains("denied"), "{err_redir}");

        let all_local_perms = PermissionSet::new(&[
            Permission::Network(vec!["*".into()]),
            Permission::NetworkLocal,
        ]);
        let err_meta = execute_http_request(
            "p1",
            &all_local_perms,
            Some(&vault),
            &HttpRequest {
                method: "GET".into(),
                url: format!("http://{addr}/redir-meta"),
                headers: vec![],
                body: None,
                timeout_ms: Some(2000),
            },
        )
        .unwrap_err();
        assert!(err_meta.to_string().contains("SSRF"), "{err_meta}");

        // 3. Valid local request with secret interpolation succeeds.
        let ok = execute_http_request(
            "p1",
            &local_perms,
            Some(&vault),
            &HttpRequest {
                method: "GET".into(),
                url: format!("http://{addr}/ok"),
                headers: vec![("Authorization".into(), "Bearer ${secret:api_key}".into())],
                body: None,
                timeout_ms: Some(2000),
            },
        )?;
        assert_eq!(ok.status, 200);
        assert_eq!(ok.body_text(), "ok");

        drop(server);
        Ok(())
    }

    #[test]
    fn ssrf_blocks_ipv6_transition_prefixes_and_192_0_0_24() -> Result<()> {
        for addr in [
            "64:ff9b::169.254.169.254",
            "64:ff9b::127.0.0.1",
            "64:ff9b::8.8.8.8",
            "64:ff9b:1::1",
            "::ffff:0:127.0.0.1",
            "::ffff:0:169.254.169.254",
            "2002:7f00:0001::1",
            "2002:0a00:0001::1",
            "2001:0000:4136:e378:8000:63bf:3fff:fdd2",
            "fec0::1",
            "100::1",
            "192.0.0.1",
            "192.0.0.170",
        ] {
            let ip: IpAddr = addr.parse()?;
            assert!(is_ip_blocked(ip, false), "should block {addr} (public)");
            assert!(is_ip_blocked(ip, true), "should block {addr} (even with allow_local)");
        }
        // Public IPv6 and public IPv4 remain permitted.
        assert!(!is_ip_blocked("2606:4700:4700::1111".parse()?, false));
        assert!(!is_ip_blocked("1.1.1.1".parse()?, false));

        // validate_target_uri must be purely syntactic + literal-IP (no blocking DNS on worker).
        let perms = PermissionSet::new(&[Permission::Network(vec!["nonexistent.invalid".into()])]);
        let uri: Uri = "https://nonexistent.invalid/api".parse()?;
        assert!(validate_target_uri(&uri, &perms).is_ok());

        // Embedded userinfo `@` must be rejected.
        let userinfo_uri: Uri = "https://user:pass@nonexistent.invalid/api".parse()?;
        assert!(validate_target_uri(&userinfo_uri, &perms).is_err());

        // HTTPS -> HTTP downgrade redirects must be rejected.
        let https_uri: Uri = "https://api.example.com/start".parse()?;
        let http_uri: Uri = "http://api.example.com/downgraded".parse()?;
        let https_next: Uri = "https://api.example.com/next".parse()?;
        assert!(ensure_no_scheme_downgrade(&https_uri, &http_uri).is_err());
        assert!(ensure_no_scheme_downgrade(&https_uri, &https_next).is_ok());
        assert!(ensure_no_scheme_downgrade(&http_uri, &https_next).is_ok());
        Ok(())
    }

    #[test]
    fn sticky_cross_origin_redirect_strips_credentials_and_body_and_reflect_scrubs() -> Result<()> {
        // Two distinct local origins (A and B on different ports).
        let listener_a = TcpListener::bind("127.0.0.1:0")?;
        let addr_a = listener_a.local_addr()?;
        let listener_b = TcpListener::bind("127.0.0.1:0")?;
        let addr_b = listener_b.local_addr()?;

        let seen_b = Arc::new(Mutex::new(String::new()));
        let seen_a_final = Arc::new(Mutex::new(String::new()));

        let seen_b_c = Arc::clone(&seen_b);
        let srv_b = thread::spawn(move || {
            if let Ok((mut sock, _)) = listener_b.accept() {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap_or(0);
                *seen_b_c.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).into_owned();
                let resp = format!(
                    "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{addr_a}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let seen_a_final_c = Arc::clone(&seen_a_final);
        let srv_a = thread::spawn(move || {
            for _ in 0..5 {
                let Ok((mut sock, _)) = listener_a.accept() else {
                    break;
                };
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                if req.starts_with("POST /start") {
                    let resp = format!(
                        "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{addr_b}/bounce\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(resp.as_bytes());
                } else if req.starts_with("POST /final") || req.starts_with("GET /final") {
                    *seen_a_final_c.lock().unwrap() = req;
                    let _ = sock.write_all(
                        b"HTTP/1.1 200 OK\r\nX-Echo: secret-token-xyz\r\nContent-Length: 23\r\nConnection: close\r\n\r\nechoed=secret-token-xyz",
                    );
                } else if req.starts_with("GET /binary-clean") {
                    let bin_body: &[u8] = b"\x89PNG\r\n\x1a\n\xff\xfe\x00\x01";
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bin_body.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes());
                    let _ = sock.write_all(bin_body);
                } else if req.starts_with("GET /binary-leak") {
                    let leak_body: &[u8] = b"\xff\xfesecret-token-xyz";
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        leak_body.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes());
                    let _ = sock.write_all(leak_body);
                } else if req.starts_with("GET /no-redir") {
                    let resp = format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{addr_a}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(resp.as_bytes());
                }
            }
        });

        let dir = tempfile::tempdir()?;
        let vault = SecretsVault::open(dir.path().join("v.vault"), dir.path().join("v.key"))?;
        vault.set("p1", "tok", "secret-token-xyz")?;

        let perms = PermissionSet::new(&[
            Permission::Network(vec![
                format!("127.0.0.1:{}", addr_a.port()),
                format!("127.0.0.1:{}", addr_b.port()),
            ]),
            Permission::NetworkLocal,
        ]);

        // A -> B -> A redirect with 307: credentials and body must be stripped at B AND stay stripped back at A!
        let resp = execute_http_request(
            "p1",
            &perms,
            Some(&vault),
            &HttpRequest {
                method: "POST".into(),
                url: format!("http://{addr_a}/start"),
                headers: vec![
                    ("Authorization".into(), "Bearer ${secret:tok}".into()),
                    ("Cookie".into(), "sid=abc".into()),
                    ("Content-Type".into(), "application/x-www-form-urlencoded".into()),
                ],
                body: Some(b"refresh_token=super-sensitive-body".to_vec()),
                timeout_ms: Some(3000),
            },
        )?;
        assert_eq!(resp.status, 200);

        // Reflected secret in response body & header must be scrubbed because `${secret:...}` was sent.
        assert!(!resp.body_text().contains("secret-token-xyz"), "{}", resp.body_text());
        assert!(resp.body_text().contains("***"));
        assert!(
            resp.headers
                .iter()
                .all(|(_, v)| !v.contains("secret-token-xyz")),
            "{:?}",
            resp.headers
        );

        let b_req = seen_b.lock().unwrap().to_ascii_lowercase();
        assert!(!b_req.contains("authorization:"), "B saw auth: {b_req}");
        assert!(!b_req.contains("cookie:"), "B saw cookie: {b_req}");
        assert!(!b_req.contains("super-sensitive-body"), "B saw body: {b_req}");

        let a_final = seen_a_final.lock().unwrap().to_ascii_lowercase();
        assert!(
            !a_final.contains("authorization:"),
            "A->B->A re-attached auth: {a_final}"
        );
        assert!(
            !a_final.contains("cookie:"),
            "A->B->A re-attached cookie: {a_final}"
        );
        assert!(
            !a_final.contains("super-sensitive-body"),
            "A->B->A re-attached body: {a_final}"
        );

        // Binary non-UTF-8 response with NO reflected secret stays byte-for-byte intact.
        let clean_bin = execute_http_request(
            "p1",
            &perms,
            Some(&vault),
            &HttpRequest {
                method: "GET".into(),
                url: format!("http://{addr_a}/binary-clean"),
                headers: vec![("Authorization".into(), "Bearer ${secret:tok}".into())],
                body: None,
                timeout_ms: Some(2000),
            },
        )?;
        assert_eq!(clean_bin.body, b"\x89PNG\r\n\x1a\n\xff\xfe\x00\x01");

        // Invalid-UTF-8 response that DOES reflect a secret is still scrubbed.
        let leaked_bin = execute_http_request(
            "p1",
            &perms,
            Some(&vault),
            &HttpRequest {
                method: "GET".into(),
                url: format!("http://{addr_a}/binary-leak"),
                headers: vec![("Authorization".into(), "Bearer ${secret:tok}".into())],
                body: None,
                timeout_ms: Some(2000),
            },
        )?;
        assert!(!leaked_bin.body_text().contains("secret-token-xyz"));
        assert!(leaked_bin.body_text().contains("***"));

        // Zero redirects allowed rejects 302 immediately.
        let no_redir_err = execute_http_request_with_redirects(
            "p1",
            &perms,
            Some(&vault),
            &HttpRequest {
                method: "GET".into(),
                url: format!("http://{addr_a}/no-redir"),
                headers: vec![],
                body: None,
                timeout_ms: Some(2000),
            },
            0,
        )
        .unwrap_err();
        assert!(no_redir_err.to_string().contains("redirects are forbidden"), "{no_redir_err}");

        let _ = srv_b.join();
        let _ = srv_a.join();
        Ok(())
    }
}
