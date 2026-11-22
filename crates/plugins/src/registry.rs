//! Multi-registry client with ETag caching, SHA-256 verification, zip-slip-safe
//! archive extraction, atomic swap, local `.zip` install, and permission diff detection.

use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use fern_core::plugin_store::PluginState;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;
use zip::ZipArchive;

use crate::manifest::{
    CURRENT_API_VERSION, LoadedManifest, PermissionApprovalStatus, PluginManifest,
    validate_plugin_id,
};
use crate::permissions::Permission;

pub const DEFAULT_REGISTRY_URL: &str = "https://atabits.github.io/fern-plugins/registry.json";
pub const MAX_REGISTRY_INDEX_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_PLUGIN_ZIP_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_PLUGIN_UNPACKED_BYTES: u64 = 32 * 1024 * 1024;
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
}

impl RegistryClient {
    pub fn new(plugins_dir: impl Into<PathBuf>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            plugins_dir: plugins_dir.into(),
            cache_dir: cache_dir.into(),
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
        validate_registry_url(url)?;
        let cached = self.load_cached_index(url).ok().flatten();
        let agent = registry_agent();
        let mut req = agent
            .get(url.trim())
            .header("Accept", "application/json")
            .header("User-Agent", format!("fern/{}", env!("CARGO_PKG_VERSION")));
        if let Some(etag) = cached.as_ref().and_then(|c| c.etag.as_deref()) {
            req = req.header("If-None-Match", etag);
        }

        let mut response = req
            .call()
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

    /// Downloads a plugin `.zip` from a registry entry, verifies SHA-256, extracts safely, and swaps atomically.
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
        validate_registry_url(&entry.download_url)?;
        let expected_sha = entry.sha256.trim().to_ascii_lowercase();
        ensure!(
            expected_sha.len() == 64 && expected_sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid sha256 checksum in registry entry for {:?}",
            entry.id
        );

        let agent = registry_agent();
        let mut response = agent
            .get(entry.download_url.trim())
            .header("User-Agent", format!("fern/{}", env!("CARGO_PKG_VERSION")))
            .call()
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

        self.install_zip_bytes(
            &zip_bytes,
            Some(&entry.id),
            Some(&entry.version),
            existing_state,
        )
    }

    /// Installs a plugin from a local `.zip` file on disk with zip-slip protection and permission diff check.
    pub fn install_from_zip_file(
        &self,
        zip_path: &Path,
        existing_state: Option<&PluginState>,
    ) -> Result<InstallOutcome> {
        let meta = fs::metadata(zip_path)
            .with_context(|| format!("cannot stat zip file {}", zip_path.display()))?;
        ensure!(meta.is_file(), "zip path is not a regular file");
        ensure!(
            meta.len() <= MAX_PLUGIN_ZIP_BYTES as u64,
            "plugin archive exceeds {MAX_PLUGIN_ZIP_BYTES} bytes limit"
        );
        let zip_bytes = fs::read(zip_path)
            .with_context(|| format!("cannot read zip file {}", zip_path.display()))?;
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

fn validate_registry_url(url: &str) -> Result<()> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    ensure!(
        lower.starts_with("https://") || lower.starts_with("http://"),
        "registry URL must use https:// or http://"
    );
    Ok(())
}

fn registry_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(REGISTRY_CONNECT_TIMEOUT))
        .timeout_global(Some(REGISTRY_TOTAL_TIMEOUT))
        .build()
        .into()
}

fn extract_zip_safely(zip_bytes: &[u8], dest_dir: &Path) -> Result<()> {
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = ZipArchive::new(cursor).context("invalid ZIP archive")?;
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
        file.sync_all()?;
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
        let mut f = File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
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
        let client = RegistryClient::new(&plugins_dir, &cache_dir);

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
}
