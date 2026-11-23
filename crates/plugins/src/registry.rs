//! Multi-registry client with ETag caching, SHA-256 verification, zip-slip-safe
//! archive extraction, atomic swap, local `.zip` install, and permission diff detection.

use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use fern_core::plugin_store::PluginState;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::transport::DefaultConnector;
use uuid::Uuid;
use zip::ZipArchive;

use crate::http::{
    MAX_REDIRECTS, SsrfSafeResolver, is_ip_blocked, is_loopback_ip, resolve_redirect_uri,
};
use crate::manifest::{
    CURRENT_API_VERSION, LoadedManifest, PermissionApprovalStatus, PluginManifest,
    validate_plugin_id,
};
use crate::permissions::{Permission, PermissionSet, is_loopback_host_literal, normalize_host};

pub const DEFAULT_REGISTRY_URL: &str = "https://atabits.github.io/fern-plugins/registry.json";
pub const MAX_REGISTRY_INDEX_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_PLUGIN_ZIP_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_PLUGIN_UNPACKED_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_PLUGIN_ZIP_ENTRIES: usize = 1024;
pub const MAX_PLUGIN_ZIP_DEPTH: usize = 16;
const REGISTRY_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REGISTRY_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySource {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for RegistrySource {
    fn default() -> Self {
        Self {
            url: DEFAULT_REGISTRY_URL.to_owned(),
            name: Some("Official Fern Registry".to_owned()),
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryIndex {
    pub schema_version: u32,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub plugins: Vec<RegistryPluginEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPluginEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    #[serde(default)]
    pub min_fern_version: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    pub download_url: String,
    pub sha256: String,
    #[serde(default)]
    pub size_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct CachedRegistryIndex {
    pub source_url: String,
    pub index: RegistryIndex,
    pub etag: Option<String>,
    pub from_cache: bool,
}

#[derive(Clone, Debug)]
pub struct InstallOutcome {
    pub manifest: PluginManifest,
    pub manifest_hash: String,
    pub install_dir: PathBuf,
    pub approval_status: PermissionApprovalStatus,
    pub is_update: bool,
}

/// Client for fetching plugin registries (with `ETag` offline cache) and safely installing plugin `.zip`s.
#[derive(Clone, Debug)]
pub struct RegistryClient {
    plugins_dir: PathBuf,
    cache_dir: PathBuf,
    #[cfg(test)]
    allow_insecure_local_http: bool,
}

impl RegistryClient {
    pub fn new(plugins_dir: impl Into<PathBuf>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            plugins_dir: plugins_dir.into(),
            cache_dir: cache_dir.into(),
            #[cfg(test)]
            allow_insecure_local_http: false,
        }
    }

    #[cfg(test)]
    fn with_insecure_local_http_for_tests(mut self) -> Self {
        self.allow_insecure_local_http = true;
        self
    }

    fn allow_local_http(&self) -> bool {
        #[cfg(test)]
        {
            self.allow_insecure_local_http
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Reads any previously cached `registry.json` for `url` without hitting the network.
    pub fn load_cached_index(&self, url: &str) -> Result<Option<CachedRegistryIndex>> {
        let (json_path, etag_path) = self.cache_paths(url);
        if !json_path.is_file() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&json_path)
            .with_context(|| format!("cannot read cached registry {}", json_path.display()))?;
        let index: RegistryIndex = serde_json::from_str(&raw)?;
        ensure!(
            index.schema_version == 1,
            "unsupported cached registry schema_version {}",
            index.schema_version
        );
        let etag = fs::read_to_string(&etag_path)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        Ok(Some(CachedRegistryIndex {
            source_url: url.to_owned(),
            index,
            etag,
            from_cache: true,
        }))
    }

    /// Fetches a `registry.json` index using conditional `If-None-Match` (`ETag`) caching.
    pub fn fetch_index(&self, url: &str) -> Result<CachedRegistryIndex> {
        let allow_local = self.allow_local_http();
        validate_registry_url(url, allow_local)?;
        let cached = self.load_cached_index(url).ok().flatten();

        let mut extra_headers: Vec<(&str, &str)> = vec![("Accept", "application/json")];
        if let Some(etag) = cached.as_ref().and_then(|c| c.etag.as_deref()) {
            extra_headers.push(("If-None-Match", etag));
        }

        let mut response = registry_get_with_redirects(url, &extra_headers, allow_local)
            .with_context(|| format!("failed to fetch plugin registry {url}"))?;
        let status = response.status().as_u16();
        if status == 304 {
            if let Some(hit) = cached {
                return Ok(hit);
            }
            bail!("registry returned 304 Not Modified without a valid local cache");
        }
        ensure!(
            (200..300).contains(&status),
            "registry returned HTTP status {status}"
        );

        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());

        let mut raw_bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_REGISTRY_INDEX_BYTES as u64 + 1)
            .read_to_end(&mut raw_bytes)?;
        ensure!(
            raw_bytes.len() <= MAX_REGISTRY_INDEX_BYTES,
            "registry index exceeds {MAX_REGISTRY_INDEX_BYTES} bytes limit"
        );
        let raw_str = String::from_utf8(raw_bytes).context("registry index is not UTF-8")?;
        let index: RegistryIndex =
            serde_json::from_str(&raw_str).context("invalid registry.json format")?;
        ensure!(
            index.schema_version == 1,
            "unsupported registry schema_version {} (expected 1)",
            index.schema_version
        );

        self.write_cache(url, &raw_str, etag.as_deref())?;

        Ok(CachedRegistryIndex {
            source_url: url.to_owned(),
            index,
            etag,
            from_cache: false,
        })
    }

    /// Fetches all enabled registries, falling back to local cache when offline.
    pub fn fetch_all_resilient(
        &self,
        sources: &[RegistrySource],
    ) -> Vec<(String, Result<CachedRegistryIndex>)> {
        sources
            .iter()
            .filter(|s| s.enabled)
            .map(|source| {
                let res = match self.fetch_index(&source.url) {
                    Ok(idx) => Ok(idx),
                    Err(err) => match self.load_cached_index(&source.url) {
                        Ok(Some(cached)) => Ok(cached),
                        _ => Err(err),
                    },
                };
                (source.url.clone(), res)
            })
            .collect()
    }

    /// Downloads a plugin `.zip` from a registry entry, verifies SHA-256 and catalog permissions,
    /// extracts safely, and swaps atomically.
    pub fn install_from_registry(
        &self,
        entry: &RegistryPluginEntry,
        existing_state: Option<&PluginState>,
    ) -> Result<InstallOutcome> {
        ensure!(
            validate_plugin_id(&entry.id),
            "invalid plugin id {:?} in registry",
            entry.id
        );
        ensure!(
            entry.api_version == CURRENT_API_VERSION,
            "plugin {:?} targets unsupported api_version {}",
            entry.id,
            entry.api_version
        );
        let allow_local = self.allow_local_http();
        validate_registry_url(&entry.download_url, allow_local)?;
        let expected_sha = entry.sha256.trim().to_ascii_lowercase();
        ensure!(
            expected_sha.len() == 64 && expected_sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid sha256 checksum in registry entry for {:?}",
            entry.id
        );

        let mut response = registry_get_with_redirects(&entry.download_url, &[], allow_local)
            .with_context(|| format!("failed to download plugin archive {}", entry.download_url))?;
        let status = response.status().as_u16();
        ensure!(
            (200..300).contains(&status),
            "plugin download returned HTTP status {status}"
        );

        let mut zip_bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_PLUGIN_ZIP_BYTES as u64 + 1)
            .read_to_end(&mut zip_bytes)?;
        ensure!(
            zip_bytes.len() <= MAX_PLUGIN_ZIP_BYTES,
            "plugin archive exceeds {MAX_PLUGIN_ZIP_BYTES} bytes limit"
        );

        let actual_sha = hex::encode(Sha256::digest(&zip_bytes));
        ensure!(
            actual_sha == expected_sha,
            "SHA-256 mismatch for plugin {:?}: expected {expected_sha}, got {actual_sha}",
            entry.id
        );

        let catalog_permissions = PermissionSet::new(&entry.permissions);
        self.install_zip_bytes_inner(
            &zip_bytes,
            Some(&entry.id),
            Some(&entry.version),
            Some(&catalog_permissions),
            existing_state,
        )
    }

    /// Installs a plugin from a local `.zip` file on disk with bounded atomic read (no TOCTOU),
    /// zip-slip protection, and permission diff check.
    pub fn install_from_zip_file(
        &self,
        zip_path: &Path,
        existing_state: Option<&PluginState>,
    ) -> Result<InstallOutcome> {
        let file = File::open(zip_path)
            .with_context(|| format!("cannot open zip file {}", zip_path.display()))?;
        let meta = file
            .metadata()
            .with_context(|| format!("cannot stat zip file {}", zip_path.display()))?;
        ensure!(meta.is_file(), "zip path is not a regular file");
        let mut zip_bytes = Vec::new();
        file.take(MAX_PLUGIN_ZIP_BYTES as u64 + 1)
            .read_to_end(&mut zip_bytes)
            .with_context(|| format!("cannot read zip file {}", zip_path.display()))?;
        ensure!(
            zip_bytes.len() <= MAX_PLUGIN_ZIP_BYTES,
            "plugin archive exceeds {MAX_PLUGIN_ZIP_BYTES} bytes limit"
        );
        self.install_zip_bytes(&zip_bytes, None, None, existing_state)
    }

    /// Extracts validated plugin archive bytes, validates `manifest.json`, and atomically swaps into `plugins_dir`.
    pub fn install_zip_bytes(
        &self,
        zip_bytes: &[u8],
        expected_id: Option<&str>,
        expected_version: Option<&str>,
        existing_state: Option<&PluginState>,
    ) -> Result<InstallOutcome> {
        self.install_zip_bytes_inner(zip_bytes, expected_id, expected_version, None, existing_state)
    }

    fn install_zip_bytes_inner(
        &self,
        zip_bytes: &[u8],
        expected_id: Option<&str>,
        expected_version: Option<&str>,
        max_permissions: Option<&PermissionSet>,
        existing_state: Option<&PluginState>,
    ) -> Result<InstallOutcome> {
        ensure!(
            zip_bytes.len() <= MAX_PLUGIN_ZIP_BYTES,
            "plugin archive exceeds {MAX_PLUGIN_ZIP_BYTES} bytes limit"
        );
        fs::create_dir_all(&self.plugins_dir)?;

        let staging_dir = self
            .plugins_dir
            .join(format!(".staging-{}", Uuid::now_v7()));
        fs::create_dir_all(&staging_dir)?;

        struct StagingCleanup<'a>(&'a Path);
        impl Drop for StagingCleanup<'_> {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(self.0);
            }
        }
        let _cleanup = StagingCleanup(&staging_dir);

        extract_zip_safely(zip_bytes, &staging_dir)?;

        let extracted_root = locate_manifest_root(&staging_dir)?;
        let loaded = LoadedManifest::load_from_dir(&extracted_root)?;

        if let Some(exp_id) = expected_id {
            ensure!(
                loaded.manifest.id == exp_id,
                "archive manifest id {:?} does not match expected {:?}",
                loaded.manifest.id,
                exp_id
            );
        }
        if let Some(exp_ver) = expected_version {
            ensure!(
                loaded.manifest.version == exp_ver,
                "archive manifest version {:?} does not match expected {:?}",
                loaded.manifest.version,
                exp_ver
            );
        }
        if let Some(allowed) = max_permissions {
            let diff = allowed.diff(&loaded.permissions);
            ensure!(
                !diff.is_escalation(),
                "archive manifest requests permissions not declared in registry catalog: {:?}",
                diff.added
            );
        }

        let approval_status = loaded
            .manifest
            .check_permission_diff(&loaded.manifest_hash, existing_state);
        let target_dir = self.plugins_dir.join(&loaded.manifest.id);
        let is_update = target_dir.exists();

        atomic_swap_dir(&extracted_root, &target_dir, &self.plugins_dir)?;

        Ok(InstallOutcome {
            manifest: loaded.manifest,
            manifest_hash: loaded.manifest_hash,
            install_dir: target_dir,
            approval_status,
            is_update,
        })
    }

    pub fn uninstall_plugin(&self, plugin_id: &str) -> Result<bool> {
        ensure!(
            validate_plugin_id(plugin_id),
            "invalid plugin id {plugin_id:?}"
        );
        let dir = self.plugins_dir.join(plugin_id);
        if !dir.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(&dir)
            .with_context(|| format!("cannot remove plugin directory {}", dir.display()))?;
        Ok(true)
    }

    fn cache_paths(&self, url: &str) -> (PathBuf, PathBuf) {
        let key = hex::encode(Sha256::digest(url.trim().as_bytes()));
        (
            self.cache_dir.join(format!("{key}.json")),
            self.cache_dir.join(format!("{key}.etag")),
        )
    }

    fn write_cache(&self, url: &str, json_str: &str, etag: Option<&str>) -> Result<()> {
        fs::create_dir_all(&self.cache_dir)?;
        let (json_path, etag_path) = self.cache_paths(url);
        write_atomic_text(&json_path, json_str)?;
        if let Some(tag) = etag {
            write_atomic_text(&etag_path, tag)?;
        } else {
            let _ = fs::remove_file(&etag_path);
        }
        Ok(())
    }
}

fn validate_registry_url(url: &str, allow_local_http: bool) -> Result<Uri> {
    let trimmed = url.trim();
    ensure!(
        !trimmed.contains('#'),
        "registry URL must not contain a fragment (#)"
    );
    let uri: Uri = trimmed
        .parse()
        .with_context(|| format!("invalid registry URL {url:?}"))?;
    let scheme = uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let authority = uri
        .authority()
        .context("registry URL must contain a valid host")?;
    ensure!(
        !authority.as_str().contains('@'),
        "registry URL must not contain embedded userinfo (@)"
    );
    let host = authority.host();
    ensure!(!host.is_empty(), "registry URL host must not be empty");

    if scheme == "https" {
        // Public HTTPS only (no private/loopback/link-local literal IPs or localhost unless local test mode).
        let host_clean = normalize_host(host);
        if !allow_local_http {
            ensure!(
                !is_loopback_host_literal(&host_clean) && !host_clean.ends_with(".localhost"),
                "SSRF protection blocked loopback registry host {host:?}"
            );
        }
        if let Ok(ip) = host_clean.parse::<IpAddr>() {
            ensure!(
                !is_ip_blocked(ip, allow_local_http),
                "SSRF protection blocked registry IP {ip}"
            );
        }
        Ok(uri)
    } else if scheme == "http" && allow_local_http {
        ensure!(
            is_loopback_host_literal(host),
            "registry URL over http:// is only allowed for loopback test servers"
        );
        let host_clean = normalize_host(host);
        if let Ok(ip) = host_clean.parse::<IpAddr>() {
            ensure!(
                is_loopback_ip(ip),
                "SSRF protection blocked non-loopback test registry IP {ip}"
            );
        }
        Ok(uri)
    } else {
        bail!("registry URL must use https:// (got scheme {scheme:?})")
    }
}

fn registry_origin_key(uri: &Uri) -> (String, String, u16) {
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

fn registry_get_with_redirects(
    url: &str,
    extra_headers: &[(&str, &str)],
    allow_local_http: bool,
) -> Result<ureq::http::Response<ureq::Body>> {
    let deadline = Instant::now() + REGISTRY_TOTAL_TIMEOUT;
    let mut current_uri = validate_registry_url(url, allow_local_http)?;
    let initial_origin = registry_origin_key(&current_uri);
    let mut same_origin = true;

    for hop in 0..=MAX_REDIRECTS {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "registry request timed out");

        let cur_scheme = current_uri
            .scheme_str()
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let loopback_only = allow_local_http && cur_scheme == "http";

        let config = Config::builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .proxy(None)
            .timeout_connect(Some(REGISTRY_CONNECT_TIMEOUT.min(remaining)))
            .timeout_global(Some(remaining))
            .build();
        let resolver = SsrfSafeResolver::new(allow_local_http, loopback_only);
        let agent = ureq::Agent::with_parts(config, DefaultConnector::default(), resolver);

        let mut builder = ureq::http::Request::builder()
            .method("GET")
            .uri(&current_uri)
            .header("User-Agent", format!("fern/{}", env!("CARGO_PKG_VERSION")));
        if same_origin {
            for (k, v) in extra_headers {
                builder = builder.header(*k, *v);
            }
        }
        let req = builder
            .body(())
            .context("failed to build registry HTTP request")?;
        let response = agent.run(req)?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if hop >= MAX_REDIRECTS {
                bail!("registry request exceeded maximum of {MAX_REDIRECTS} redirects");
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .context("registry redirect missing valid Location header")?;
            let next_uri = resolve_redirect_uri(&current_uri, location)?;
            let next_str = next_uri.to_string();
            let validated_next = validate_registry_url(&next_str, allow_local_http)?;
            let next_scheme = validated_next
                .scheme_str()
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            ensure!(
                !(cur_scheme == "https" && next_scheme == "http"),
                "registry redirect blocked: HTTPS to HTTP downgrade is forbidden"
            );
            same_origin &= registry_origin_key(&validated_next) == initial_origin;
            current_uri = validated_next;
            continue;
        }
        return Ok(response);
    }

    bail!("registry request exceeded maximum of {MAX_REDIRECTS} redirects")
}

fn extract_zip_safely(zip_bytes: &[u8], dest_dir: &Path) -> Result<()> {
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = ZipArchive::new(cursor).context("invalid ZIP archive")?;
    ensure!(
        archive.len() <= MAX_PLUGIN_ZIP_ENTRIES,
        "plugin archive exceeds maximum of {MAX_PLUGIN_ZIP_ENTRIES} entries (got {})",
        archive.len()
    );
    let mut total_unpacked: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_symlink()
            || entry
                .unix_mode()
                .is_some_and(|mode| (mode & 0o170000) == 0o120000)
        {
            bail!("symlinks are forbidden in plugin archives ({:?})", entry.name());
        }
        let rel_path = entry
            .enclosed_name()
            .ok_or_else(|| anyhow!("zip entry contains unsafe path traversal: {:?}", entry.name()))?;
        if rel_path.as_os_str().is_empty() {
            continue;
        }
        ensure!(
            rel_path.components().count() <= MAX_PLUGIN_ZIP_DEPTH,
            "zip entry exceeds maximum path depth of {MAX_PLUGIN_ZIP_DEPTH}: {:?}",
            entry.name()
        );
        let out_path = dest_dir.join(&rel_path);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&out_path)
            .with_context(|| format!("cannot extract {}", out_path.display()))?;
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total_unpacked = total_unpacked.saturating_add(n as u64);
            ensure!(
                total_unpacked <= MAX_PLUGIN_UNPACKED_BYTES,
                "unpacked plugin archive exceeds {MAX_PLUGIN_UNPACKED_BYTES} bytes limit"
            );
            file.write_all(&buf[..n])?;
        }
    }
    Ok(())
}

fn locate_manifest_root(staging_dir: &Path) -> Result<PathBuf> {
    if staging_dir.join("manifest.json").is_file() {
        return Ok(staging_dir.to_path_buf());
    }
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in fs::read_dir(staging_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path);
        } else {
            files.push(path);
        }
    }
    if files.is_empty() && dirs.len() == 1 && dirs[0].join("manifest.json").is_file() {
        return Ok(dirs.remove(0));
    }
    bail!("plugin archive missing manifest.json at archive root")
}

fn atomic_swap_dir(src_dir: &Path, target_dir: &Path, plugins_dir: &Path) -> Result<()> {
    let backup_dir = plugins_dir.join(format!(".backup-{}", Uuid::now_v7()));
    let had_existing = target_dir.exists();
    if had_existing {
        fs::rename(target_dir, &backup_dir).with_context(|| {
            format!(
                "cannot move existing plugin directory {} to backup",
                target_dir.display()
            )
        })?;
    }
    match fs::rename(src_dir, target_dir) {
        Ok(()) => {
            if had_existing {
                let _ = fs::remove_dir_all(&backup_dir);
            }
            Ok(())
        }
        Err(err) => {
            if had_existing && backup_dir.exists() {
                let _ = fs::rename(&backup_dir, target_dir);
            }
            Err(err).with_context(|| {
                format!(
                    "cannot move extracted plugin into {}",
                    target_dir.display()
                )
            })
        }
    }
}

fn write_atomic_text(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("cache"),
        Uuid::now_v7()
    ));
    {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        if let Err(err) = f.write_all(content.as_bytes()).and_then(|()| f.sync_all()) {
            drop(f);
            let _ = fs::remove_file(&tmp);
            return Err(err.into());
        }
    }
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use zip::write::SimpleFileOptions;

    use super::*;

    fn build_plugin_zip(files: &[(&str, &[u8])]) -> Result<Vec<u8>> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let opts = SimpleFileOptions::default();
            for (name, bytes) in files {
                writer.start_file(*name, opts)?;
                writer.write_all(bytes)?;
            }
            writer.finish()?;
        }
        Ok(cursor.into_inner())
    }

    #[test]
    fn registry_sha256_etag_zip_slip_and_permission_escalation() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let plugins_dir = tmp.path().join("plugins");
        let cache_dir = tmp.path().join("cache");
        let client = RegistryClient::new(&plugins_dir, &cache_dir).with_insecure_local_http_for_tests();

        // 1. Zip-slip rejection.
        let evil_zip_path = tmp.path().join("evil.zip");
        let evil_bytes = build_plugin_zip(&[
            (
                "manifest.json",
                br#"{"api_version":1,"id":"com.test.evil","name":"Evil","version":"1.0.0","entry":"index.js"}"#,
            ),
            ("../escape.js", b"export function activate() {}"),
        ])?;
        fs::write(&evil_zip_path, &evil_bytes)?;
        let evil_err = client
            .install_from_zip_file(&evil_zip_path, None)
            .unwrap_err();
        assert!(
            format!("{evil_err:#}").contains("unsafe path traversal"),
            "unexpected error: {evil_err:#}"
        );

        // 2. Clean local zip install v1.0.0.
        let v1_zip_path = tmp.path().join("sync-v1.zip");
        let v1_manifest = br#"{
            "api_version": 1,
            "id": "com.example.sync",
            "name": "Example Sync",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["tasks:read"]
        }"#;
        let v1_bytes = build_plugin_zip(&[
            ("manifest.json", v1_manifest),
            ("index.js", b"export function activate() {}\n"),
        ])?;
        fs::write(&v1_zip_path, &v1_bytes)?;
        let out_v1 = client.install_from_zip_file(&v1_zip_path, None)?;
        assert!(!out_v1.is_update);
        assert!(matches!(
            out_v1.approval_status,
            PermissionApprovalStatus::NeedsApproval {
                is_first_install: true,
                ..
            }
        ));
        assert!(plugins_dir.join("com.example.sync/index.js").is_file());

        // 3. Build v1.1.0 zip with extra network permission + start local registry HTTP server.
        let v2_manifest = br#"{
            "api_version": 1,
            "id": "com.example.sync",
            "name": "Example Sync",
            "version": "1.1.0",
            "entry": "index.js",
            "permissions": ["tasks:read", { "network": ["api.github.com"] }]
        }"#;
        let v2_bytes = build_plugin_zip(&[
            ("manifest.json", v2_manifest),
            ("index.js", b"export function activate() { /* v1.1 */ }\n"),
        ])?;
        let v2_sha = hex::encode(Sha256::digest(&v2_bytes));

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let download_url = format!("http://{addr}/sync-1.1.0.zip");
        let index_url = format!("http://{addr}/registry.json");

        let index_json = serde_json::json!({
            "schema_version": 1,
            "generated_at": "2026-11-22T00:00:00Z",
            "plugins": [{
                "id": "com.example.sync",
                "name": "Example Sync",
                "version": "1.1.0",
                "api_version": 1,
                "description": "Sync plugin",
                "author": "Fern",
                "permissions": ["tasks:read", { "network": ["api.github.com"] }],
                "download_url": download_url,
                "sha256": v2_sha,
                "size_bytes": v2_bytes.len()
            }]
        })
        .to_string();

        let v2_bytes_srv = v2_bytes;
        let index_srv = index_json;
        let srv = std::thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let first_line = req.lines().next().unwrap_or("");
                if first_line.contains("/registry.json") {
                    if req.contains("\"v1\"") {
                        let _ = stream.write_all(
                            b"HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    } else {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            index_srv.len()
                        );
                        let _ = stream.write_all(hdr.as_bytes());
                        let _ = stream.write_all(index_srv.as_bytes());
                    }
                } else {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        v2_bytes_srv.len()
                    );
                    let _ = stream.write_all(hdr.as_bytes());
                    let _ = stream.write_all(&v2_bytes_srv);
                }
            }
        });

        let fetched_first = client.fetch_index(&index_url)?;
        assert!(!fetched_first.from_cache);
        assert_eq!(fetched_first.etag.as_deref(), Some("\"v1\""));

        let fetched_cached = client.fetch_index(&index_url)?;
        assert!(fetched_cached.from_cache);
        assert_eq!(fetched_cached.index.plugins.len(), 1);

        let mut bad_entry = fetched_first.index.plugins[0].clone();
        bad_entry.sha256 = "0".repeat(64);
        let sha_err = client.install_from_registry(&bad_entry, None).unwrap_err();
        assert!(format!("{sha_err:#}").contains("SHA-256 mismatch"));

        let approved_v1 = PluginState {
            plugin_id: "com.example.sync".to_owned(),
            enabled: true,
            granted_permissions: r#"["tasks:read"]"#.to_owned(),
            manifest_hash: out_v1.manifest_hash,
            settings_json: "{}".to_owned(),
            consecutive_errors: 0,
            updated_at: 0,
        };
        let out_v2 = client.install_from_registry(&fetched_first.index.plugins[0], Some(&approved_v1))?;
        assert!(out_v2.is_update);
        match out_v2.approval_status {
            PermissionApprovalStatus::NeedsApproval {
                diff,
                is_first_install,
            } => {
                assert!(!is_first_install);
                assert_eq!(
                    diff.added,
                    vec![Permission::Network(vec!["api.github.com".to_owned()])]
                );
            }
            other => bail!("expected NeedsApproval, got {other:?}"),
        }

        let _ = srv.join();
        Ok(())
    }

    #[test]
    fn registry_requires_https_blocks_ssrf_checks_catalog_permissions_and_caps_zip() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let plugins_dir = tmp.path().join("plugins");
        let cache_dir = tmp.path().join("cache");

        // 1. Default production RegistryClient rejects plaintext http:// and private/loopback HTTPS IPs.
        let strict_client = RegistryClient::new(&plugins_dir, &cache_dir);
        assert!(strict_client.fetch_index("http://registry.example.com/registry.json").is_err());
        assert!(strict_client.fetch_index("http://127.0.0.1:8080/registry.json").is_err());
        assert!(strict_client.fetch_index("https://127.0.0.1/registry.json").is_err());
        assert!(strict_client.fetch_index("https://169.254.169.254/latest/meta-data/").is_err());
        assert!(strict_client.fetch_index("https://10.0.0.1/registry.json").is_err());

        // 2. Archive requesting more permissions than declared in catalog entry must be rejected.
        let escalated_zip = build_plugin_zip(&[
            (
                "manifest.json",
                br#"{
                    "api_version": 1,
                    "id": "com.test.sneaky",
                    "name": "Sneaky",
                    "version": "1.0.0",
                    "entry": "index.js",
                    "permissions": ["tasks:read", "tasks:write"]
                }"#,
            ),
            ("index.js", b"export function activate() {}\n"),
        ])?;
        let escalated_sha = hex::encode(Sha256::digest(&escalated_zip));

        // Local mock server serving (a) escalated zip and (b) SSRF redirect to metadata IP.
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let zip_srv = escalated_zip;
        let srv = std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                if req.starts_with("GET /sneaky.zip") {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        zip_srv.len()
                    );
                    let _ = stream.write_all(hdr.as_bytes());
                    let _ = stream.write_all(&zip_srv);
                } else {
                    let _ = stream.write_all(
                        b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        });

        let test_client = RegistryClient::new(&plugins_dir, &cache_dir).with_insecure_local_http_for_tests();
        let entry = RegistryPluginEntry {
            id: "com.test.sneaky".to_owned(),
            name: "Sneaky".to_owned(),
            version: "1.0.0".to_owned(),
            api_version: CURRENT_API_VERSION,
            min_fern_version: None,
            description: String::new(),
            author: String::new(),
            homepage: None,
            tags: Vec::new(),
            permissions: vec![Permission::TasksRead], // Catalog claims tasks:read ONLY!
            download_url: format!("http://{addr}/sneaky.zip"),
            sha256: escalated_sha,
            size_bytes: 0,
        };
        let perm_err = test_client.install_from_registry(&entry, None).unwrap_err();
        assert!(
            format!("{perm_err:#}").contains("not declared in registry catalog"),
            "{perm_err:#}"
        );
        assert!(
            !plugins_dir.join("com.test.sneaky").exists(),
            "rejected archive must not be installed on disk"
        );

        // Redirect to 169.254.169.254 must be blocked.
        let ssrf_err = test_client
            .fetch_index(&format!("http://{addr}/ssrf-redir"))
            .unwrap_err();
        assert!(
            format!("{ssrf_err:#}").contains("loopback")
                || format!("{ssrf_err:#}").contains("SSRF")
                || format!("{ssrf_err:#}").contains("https://"),
            "{ssrf_err:#}"
        );
        let _ = srv.join();

        // 3. ZIP entry-count bomb (> MAX_PLUGIN_ZIP_ENTRIES) and excessive depth (> MAX_PLUGIN_ZIP_DEPTH) must fail.
        {
            let mut cursor = std::io::Cursor::new(Vec::new());
            {
                let mut writer = zip::ZipWriter::new(&mut cursor);
                let opts = SimpleFileOptions::default();
                for i in 0..=MAX_PLUGIN_ZIP_ENTRIES {
                    writer.start_file(format!("f{i}.txt"), opts)?;
                }
                writer.finish()?;
            }
            let bomb_err = strict_client
                .install_zip_bytes(cursor.get_ref(), None, None, None)
                .unwrap_err();
            assert!(format!("{bomb_err:#}").contains("exceeds maximum"), "{bomb_err:#}");
        }
        {
            let deep_path = format!("{}leaf.js", "d/".repeat(MAX_PLUGIN_ZIP_DEPTH + 1));
            let deep_zip = build_plugin_zip(&[(deep_path.as_str(), b"export {};")])?;
            let depth_err = strict_client
                .install_zip_bytes(&deep_zip, None, None, None)
                .unwrap_err();
            assert!(format!("{depth_err:#}").contains("maximum path depth"), "{depth_err:#}");
        }

        // 4. Oversized local zip file is rejected cleanly without unbounded read.
        let big_path = tmp.path().join("big.zip");
        {
            let f = File::create(&big_path)?;
            f.set_len(MAX_PLUGIN_ZIP_BYTES as u64 + 64)?;
        }
        let big_err = strict_client
            .install_from_zip_file(&big_path, None)
            .unwrap_err();
        assert!(format!("{big_err:#}").contains("exceeds"), "{big_err:#}");

        Ok(())
    }
}
