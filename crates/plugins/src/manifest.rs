//! `manifest.json` schema, validation, path safety, SHA-256 hashing, and permission diff checks.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use fern_core::plugin_store::PluginState;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::permissions::{Permission, PermissionDiff, PermissionSet};
use crate::ui_ast::sanitize_svg;

/// Currently supported plugin host API version.
pub const CURRENT_API_VERSION: u32 = 1;

/// Validates `plugin_id` against `^[a-z0-9][a-z0-9._-]{1,63}$` and forbids `..`.
pub fn validate_plugin_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if !(2..=64).contains(&bytes.len()) {
        return false;
    }
    if !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
        return false;
    }
    if id.contains("..") {
        return false;
    }
    bytes[1..].iter().all(|&b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
    })
}

/// Computes lowercase hex SHA-256 digest of raw manifest bytes.
pub fn manifest_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
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
    pub entry: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub activation_events: Vec<String>,
    #[serde(default)]
    pub contributions: PluginContributions,
    #[serde(default)]
    pub settings: Vec<SettingSpec>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginContributions {
    #[serde(default)]
    pub sidebar: Vec<SidebarContribution>,
    #[serde(default)]
    pub pages: Vec<PageContribution>,
    #[serde(default)]
    pub commands: Vec<CommandContribution>,
    #[serde(default)]
    pub search_providers: Vec<SearchProviderContribution>,
    #[serde(default)]
    pub themes: Vec<ThemeContribution>,
    #[serde(default)]
    pub locales: Vec<LocaleContribution>,
    #[serde(default)]
    pub importers: Vec<ImporterContribution>,
    // Reserved Yellow-Zone contribution fields accepted without failing validation.
    #[serde(default)]
    pub task_badges: Vec<ReservedContribution>,
    #[serde(default)]
    pub task_actions: Vec<ReservedContribution>,
    #[serde(default)]
    pub virtual_lists: Vec<ReservedContribution>,
    #[serde(default)]
    pub task_detail_sections: Vec<ReservedContribution>,
}

impl PluginContributions {
    /// Returns names of any reserved Yellow-Zone contribution sections declared by the manifest.
    pub fn reserved_sections(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.task_badges.is_empty() {
            out.push("task_badges");
        }
        if !self.task_actions.is_empty() {
            out.push("task_actions");
        }
        if !self.virtual_lists.is_empty() {
            out.push("virtual_lists");
        }
        if !self.task_detail_sections.is_empty() {
            out.push("task_detail_sections");
        }
        out
    }
    /// Returns the primary interactive view (`(view_id, title)`) for opening the plugin's own screen.
    pub fn primary_view(&self) -> Option<(&str, &str)> {
        self.pages
            .first()
            .map(|p| (p.id.as_str(), p.title.as_str()))
            .or_else(|| self.sidebar.first().map(|s| (s.id.as_str(), s.title.as_str())))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarContribution {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
}

/// Dedicated plugin screen opened from `Settings → Plugins` without cluttering the main sidebar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageContribution {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandContribution {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchProviderContribution {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub prefix: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeContribution {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub appearance: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub colors: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocaleContribution {
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub strings: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImporterContribution {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub extensions: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReservedContribution {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKind {
    Text,
    Number,
    Boolean,
    Select,
    Secret,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SettingOption {
    pub label: String,
    pub value: String,
}

impl<'de> Deserialize<'de> for SettingOption {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawOption {
            Plain(String),
            Pair { label: String, value: String },
        }
        match RawOption::deserialize(deserializer)? {
            RawOption::Plain(s) => Ok(SettingOption { label: s.clone(), value: s }),
            RawOption::Pair { label, value } => Ok(SettingOption { label, value }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingSpec {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: SettingKind,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub options: Vec<SettingOption>,
}

/// A validated manifest loaded from disk together with its canonical directory and assets.
#[derive(Clone, Debug)]
pub struct LoadedManifest {
    pub manifest: PluginManifest,
    pub manifest_hash: String,
    pub dir: PathBuf,
    pub entry_path: PathBuf,
    pub sidebar_icons: BTreeMap<String, String>,
    pub permissions: PermissionSet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionApprovalStatus {
    Approved,
    NeedsApproval {
        diff: PermissionDiff,
        is_first_install: bool,
    },
}

impl PluginManifest {
    /// Parses and validates `manifest.json` text, returning `(manifest, sha256_hex)`.
    pub fn parse_and_validate(raw_json: &str) -> Result<(Self, String)> {
        let hash = manifest_sha256(raw_json.as_bytes());
        let manifest: PluginManifest =
            serde_json::from_str(raw_json).context("invalid manifest.json")?;
        manifest.validate()?;
        Ok((manifest, hash))
    }

    /// Validates all structural manifest invariants.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.api_version == CURRENT_API_VERSION,
            "unsupported plugin api_version {} (expected {CURRENT_API_VERSION})",
            self.api_version
        );
        ensure!(
            validate_plugin_id(&self.id),
            "invalid plugin id {:?}: must match ^[a-z0-9][a-z0-9._-]{{1,63}}$",
            self.id
        );
        ensure!(!self.name.trim().is_empty(), "plugin name must not be empty");
        ensure!(!self.version.trim().is_empty(), "plugin version must not be empty");
        ensure!(!self.entry.trim().is_empty(), "plugin entry must not be empty");
        validate_relative_path(&self.entry)?;

        let mut view_ids = HashSet::new();
        for view in &self.contributions.sidebar {
            validate_view_id(&view.id, "sidebar")?;
            ensure!(
                view_ids.insert(view.id.as_str()),
                "duplicate sidebar view id {:?}",
                view.id
            );
            ensure!(
                !view.title.trim().is_empty(),
                "sidebar view {:?} title must not be empty",
                view.id
            );
            if let Some(icon) = &view.icon {
                validate_relative_path(icon)?;
            }
        }

        for page in &self.contributions.pages {
            validate_view_id(&page.id, "page")?;
            ensure!(
                view_ids.insert(page.id.as_str()),
                "duplicate view id {:?}",
                page.id
            );
            ensure!(
                !page.title.trim().is_empty(),
                "page view {:?} title must not be empty",
                page.id
            );
            if let Some(icon) = &page.icon {
                validate_relative_path(icon)?;
            }
        }

        let mut cmd_ids = HashSet::new();
        for cmd in &self.contributions.commands {
            ensure!(!cmd.id.trim().is_empty(), "command id must not be empty");
            ensure!(
                cmd.id == cmd.id.trim(),
                "command id {:?} must not have leading or trailing whitespace",
                cmd.id
            );
            ensure!(
                cmd_ids.insert(cmd.id.as_str()),
                "duplicate command id {:?}",
                cmd.id
            );
            ensure!(
                !cmd.title.trim().is_empty(),
                "command {:?} title must not be empty",
                cmd.id
            );
        }

        let mut sp_ids = HashSet::new();
        for sp in &self.contributions.search_providers {
            ensure!(
                !sp.id.trim().is_empty(),
                "search provider id must not be empty"
            );
            ensure!(
                sp.id == sp.id.trim(),
                "search provider id {:?} must not have leading or trailing whitespace",
                sp.id
            );
            ensure!(
                sp_ids.insert(sp.id.as_str()),
                "duplicate search provider id {:?}",
                sp.id
            );
            ensure!(
                !sp.title.trim().is_empty(),
                "search provider {:?} title must not be empty",
                sp.id
            );
        }

        let mut imp_ids = HashSet::new();
        for imp in &self.contributions.importers {
            ensure!(!imp.id.trim().is_empty(), "importer id must not be empty");
            ensure!(
                imp.id == imp.id.trim(),
                "importer id {:?} must not have leading or trailing whitespace",
                imp.id
            );
            ensure!(
                imp_ids.insert(imp.id.as_str()),
                "duplicate importer id {:?}",
                imp.id
            );
            ensure!(
                !imp.name.trim().is_empty(),
                "importer {:?} name must not be empty",
                imp.id
            );
        }

        let mut setting_keys = HashSet::new();
        for setting in &self.settings {
            ensure!(!setting.key.trim().is_empty(), "setting key must not be empty");
            ensure!(
                setting.key == setting.key.trim(),
                "setting key {:?} must not have leading or trailing whitespace",
                setting.key
            );
            ensure!(
                setting_keys.insert(setting.key.as_str()),
                "duplicate setting key {:?}",
                setting.key
            );
            ensure!(
                !setting.label.trim().is_empty(),
                "setting {:?} label must not be empty",
                setting.key
            );
            if setting.kind == SettingKind::Select {
                ensure!(
                    !setting.options.is_empty(),
                    "select setting {:?} must define options",
                    setting.key
                );
                for opt in &setting.options {
                    ensure!(
                        !opt.value.trim().is_empty() && !opt.label.trim().is_empty(),
                        "select setting {:?} has option with empty value or label",
                        setting.key
                    );
                }
            }
            if setting.kind == SettingKind::Number {
                if let Some(min) = setting.min {
                    ensure!(min.is_finite(), "number setting {:?} min must be finite", setting.key);
                }
                if let Some(max) = setting.max {
                    ensure!(max.is_finite(), "number setting {:?} max must be finite", setting.key);
                }
                if let (Some(min), Some(max)) = (setting.min, setting.max) {
                    ensure!(
                        min <= max,
                        "number setting {:?} min ({min}) must be <= max ({max})",
                        setting.key
                    );
                }
                if let Some(step) = setting.step {
                    ensure!(
                        step.is_finite() && step > 0.0,
                        "number setting {:?} step must be finite and > 0",
                        setting.key
                    );
                }
            }
        }

        let mut theme_ids = HashSet::new();
        for theme in &self.contributions.themes {
            ensure!(!theme.id.trim().is_empty(), "theme id must not be empty");
            ensure!(
                theme.id == theme.id.trim(),
                "theme id {:?} must not have leading or trailing whitespace",
                theme.id
            );
            ensure!(
                theme_ids.insert(theme.id.as_str()),
                "duplicate theme id {:?}",
                theme.id
            );
            ensure!(
                !theme.name.trim().is_empty(),
                "theme {:?} name must not be empty",
                theme.id
            );
            if let Some(path) = &theme.path {
                validate_relative_path(path)?;
            }
        }
        let mut locale_codes = HashSet::new();
        for locale in &self.contributions.locales {
            ensure!(!locale.code.trim().is_empty(), "locale code must not be empty");
            ensure!(
                locale.code == locale.code.trim(),
                "locale code {:?} must not have leading or trailing whitespace",
                locale.code
            );
            ensure!(
                locale_codes.insert(locale.code.as_str()),
                "duplicate locale code {:?}",
                locale.code
            );
            ensure!(
                !locale.name.trim().is_empty(),
                "locale {:?} name must not be empty",
                locale.code
            );
            if let Some(path) = &locale.path {
                validate_relative_path(path)?;
            }
        }
        Ok(())
    }

    pub fn permission_set(&self) -> PermissionSet {
        PermissionSet::new(&self.permissions)
    }

    /// Merges non-secret default values from `manifest.settings` with stored `settings_json`.
    pub fn resolve_settings(&self, stored_json: &str) -> serde_json::Map<String, serde_json::Value> {
        let mut out = serde_json::Map::new();
        for spec in &self.settings {
            if spec.kind != SettingKind::Secret
                && let Some(default) = &spec.default
            {
                out.insert(spec.key.clone(), default.clone());
            }
        }
        if let Ok(serde_json::Value::Object(stored)) = serde_json::from_str::<serde_json::Value>(stored_json) {
            let secret_keys: HashSet<&str> = self
                .settings
                .iter()
                .filter(|s| s.kind == SettingKind::Secret)
                .map(|s| s.key.as_str())
                .collect();
            for (k, v) in stored {
                if !secret_keys.contains(k.as_str()) {
                    out.insert(k, v);
                }
            }
        }
        out
    }

    /// Returns whether the manifest matches an `activation_events` trigger token.
    pub fn matches_activation_event(&self, event: &str) -> bool {
        if self.activation_events.is_empty() {
            return true;
        }
        self.activation_events.iter().any(|pattern| {
            if pattern == "*" || pattern == event {
                return true;
            }
            if pattern.ends_with(":*") {
                // Keep the trailing ':' so `"onCommand:*"` only matches `"onCommand:<rest>"`,
                // never `"onCommand"` or `"onCommandLine:..."`.
                let prefix_with_colon = &pattern[..pattern.len() - 1];
                return event.starts_with(prefix_with_colon);
            }
            false
        })
    }

    /// Compares requested manifest permissions and hash against persisted `plugin_state`.
    pub fn check_permission_diff(
        &self,
        manifest_hash: &str,
        state: Option<&PluginState>,
    ) -> PermissionApprovalStatus {
        let requested = self.permission_set();
        let Some(state) = state else {
            if requested.is_empty() {
                return PermissionApprovalStatus::Approved;
            }
            return PermissionApprovalStatus::NeedsApproval {
                diff: PermissionSet::default().diff(&requested),
                is_first_install: true,
            };
        };

        let granted = match PermissionSet::from_json_str(&state.granted_permissions) {
            Ok(g) => g,
            Err(_) => {
                if !requested.is_empty() {
                    return PermissionApprovalStatus::NeedsApproval {
                        diff: PermissionSet::default().diff(&requested),
                        is_first_install: state.manifest_hash.is_empty(),
                    };
                }
                PermissionSet::default()
            }
        };
        let diff = granted.diff(&requested);
        if diff.is_escalation() {
            return PermissionApprovalStatus::NeedsApproval {
                diff,
                is_first_install: state.manifest_hash.is_empty(),
            };
        }
        // Require approval if no manifest hash was ever recorded or the supplied hash is empty
        // while permissions are requested.
        if (state.manifest_hash.is_empty() || manifest_hash.is_empty()) && !requested.is_empty() {
            return PermissionApprovalStatus::NeedsApproval {
                diff: PermissionSet::default().diff(&requested),
                is_first_install: state.manifest_hash.is_empty(),
            };
        }
        // When a disabled or overwritten plugin's manifest hash changed while still requesting
        // permissions, require explicit re-approval before re-enabling.
        if !state.enabled && state.manifest_hash != manifest_hash && !requested.is_empty() {
            return PermissionApprovalStatus::NeedsApproval {
                diff: PermissionSet::default().diff(&requested),
                is_first_install: false,
            };
        }
        PermissionApprovalStatus::Approved
    }

    /// Returns `Some(narrowed_permissions_json)` when an already-approved plugin's manifest
    /// dropped previously granted permissions or updated its `manifest_hash`, so persisted
    /// `granted_permissions` shrinks immediately and dropped capabilities cannot be silently
    /// reclaimed by a future update.
    pub fn narrowed_permissions_to_persist(
        &self,
        manifest_hash: &str,
        state: Option<&PluginState>,
    ) -> Option<String> {
        let state = state?;
        let requested = self.permission_set();
        let granted = PermissionSet::from_json_str(&state.granted_permissions).ok()?;
        let diff = granted.diff(&requested);
        if diff.is_escalation() {
            return None;
        }
        if !diff.removed.is_empty() || state.manifest_hash != manifest_hash {
            requested.to_json_string().ok()
        } else {
            None
        }
    }
}

impl LoadedManifest {
    /// Loads and validates `manifest.json`, entry point path, and sanitized SVG icons from `plugin_dir`.
    pub fn load_from_dir(plugin_dir: &Path) -> Result<Self> {
        let canonical_dir = plugin_dir
            .canonicalize()
            .with_context(|| format!("cannot canonicalize plugin directory {}", plugin_dir.display()))?;
        let manifest_path = canonical_dir.join("manifest.json");
        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("cannot read {}", manifest_path.display()))?;
        let (manifest, manifest_hash) = PluginManifest::parse_and_validate(&raw)?;

        let entry_path = resolve_inside_dir(&canonical_dir, &manifest.entry)
            .context("invalid plugin entry path")?;
        ensure!(
            entry_path.is_file(),
            "plugin entry file not found: {}",
            manifest.entry
        );

        let mut sidebar_icons = BTreeMap::new();
        for view in &manifest.contributions.sidebar {
            if let Some(icon_rel) = &view.icon {
                let icon_path = resolve_inside_dir(&canonical_dir, icon_rel)
                    .with_context(|| format!("invalid sidebar icon path {icon_rel:?}"))?;
                ensure!(icon_path.is_file(), "sidebar icon not found: {icon_rel}");
                let svg_raw = std::fs::read_to_string(&icon_path)
                    .with_context(|| format!("cannot read sidebar icon {icon_rel}"))?;
                let sanitized = sanitize_svg(&svg_raw)
                    .with_context(|| format!("invalid SVG in sidebar icon {icon_rel}"))?;
                sidebar_icons.insert(view.id.clone(), sanitized);
            }
        }

        for page in &manifest.contributions.pages {
            if let Some(icon_rel) = &page.icon {
                let icon_path = resolve_inside_dir(&canonical_dir, icon_rel)
                    .with_context(|| format!("invalid page icon path {icon_rel:?}"))?;
                ensure!(icon_path.is_file(), "page icon not found: {icon_rel}");
                let svg_raw = std::fs::read_to_string(&icon_path)
                    .with_context(|| format!("cannot read page icon {icon_rel}"))?;
                let sanitized = sanitize_svg(&svg_raw)
                    .with_context(|| format!("invalid SVG in page icon {icon_rel}"))?;
                sidebar_icons.insert(page.id.clone(), sanitized);
            }
        }

        let permissions = manifest.permission_set();
        Ok(Self {
            manifest,
            manifest_hash,
            dir: canonical_dir,
            entry_path,
            sidebar_icons,
            permissions,
        })
    }
}

fn validate_view_id(id: &str, kind: &str) -> Result<()> {
    ensure!(!id.trim().is_empty(), "{kind} view id must not be empty");
    ensure!(
        id == id.trim(),
        "{kind} view id {id:?} must not have leading or trailing whitespace"
    );
    ensure!(
        !id.contains('/') && !id.contains('\\'),
        "{kind} view id {id:?} must not contain path separators"
    );
    ensure!(
        !matches!(id, "__modal__" | "__default__"),
        "{kind} view id {id:?} is reserved"
    );
    Ok(())
}

/// Validates that `rel` is a clean relative path without parent traversal or root prefixes.
pub fn validate_relative_path(rel: &str) -> Result<()> {
    let trimmed = rel.trim();
    ensure!(!trimmed.is_empty(), "relative path must not be empty");
    let p = Path::new(trimmed);
    ensure!(!p.is_absolute(), "absolute paths are forbidden: {rel:?}");
    for comp in p.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("path traversal or root component forbidden: {rel:?}");
            }
        }
    }
    Ok(())
}

/// Resolves `rel` strictly inside `canonical_dir`, blocking `..` and symlink escapes.
pub fn resolve_inside_dir(canonical_dir: &Path, rel: &str) -> Result<PathBuf> {
    validate_relative_path(rel)?;
    let candidate = canonical_dir.join(rel);
    let canon = candidate
        .canonicalize()
        .with_context(|| format!("cannot resolve path {rel:?}"))?;
    ensure!(
        canon.starts_with(canonical_dir),
        "path {rel:?} escapes plugin directory"
    );
    Ok(canon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_id_reserved_yellow_fields_and_permission_escalation() {
        assert!(validate_plugin_id("com.example.github-issues"));
        assert!(!validate_plugin_id("../escape"));
        assert!(!validate_plugin_id("A"));
        assert!(!validate_plugin_id("bad..id"));

        let raw = r#"{
          "id": "com.example.test",
          "name": "Test Plugin",
          "version": "1.0.0",
          "api_version": 1,
          "entry": "index.js",
          "permissions": ["tasks:read", { "network": ["api.github.com"] }],
          "contributions": {
            "sidebar": [{ "id": "main", "title": "Main" }],
            "task_badges": [{ "id": "gh", "label": "GH" }]
          },
          "settings": [
            { "key": "token", "type": "secret", "label": "Token" },
            { "key": "mode", "type": "select", "label": "Mode", "options": ["a", "b"], "default": "a" }
          ]
        }"#;
        let (manifest, hash) = PluginManifest::parse_and_validate(raw).unwrap();
        assert_eq!(manifest.contributions.reserved_sections(), vec!["task_badges"]);

        let old_state = PluginState {
            plugin_id: manifest.id.clone(),
            enabled: true,
            granted_permissions: r#"["tasks:read"]"#.into(),
            manifest_hash: "old".into(),
            ..Default::default()
        };
        let status = manifest.check_permission_diff(&hash, Some(&old_state));
        assert!(matches!(
            status,
            PermissionApprovalStatus::NeedsApproval { is_first_install: false, .. }
        ));

        // Page-only plugin (no sidebar icon) validates cleanly and exposes primary_view().
        let page_raw = r#"{
          "id": "com.example.sync",
          "name": "Sync Plugin",
          "version": "1.0.0",
          "api_version": 1,
          "entry": "index.js",
          "contributions": {
            "pages": [{ "id": "sync", "title": "Sync" }]
          }
        }"#;
        let (page_manifest, _) = PluginManifest::parse_and_validate(page_raw).unwrap();
        assert!(page_manifest.contributions.sidebar.is_empty());
        assert_eq!(page_manifest.contributions.primary_view(), Some(("sync", "Sync")));

        // Duplicate view ID across sidebar and pages is rejected.
        let dup_raw = r#"{
          "id": "com.example.dup",
          "name": "Dup Plugin",
          "version": "1.0.0",
          "api_version": 1,
          "entry": "index.js",
          "contributions": {
            "sidebar": [{ "id": "main", "title": "Main" }],
            "pages": [{ "id": "main", "title": "Main Page" }]
          }
        }"#;
        assert!(PluginManifest::parse_and_validate(dup_raw).is_err());
    }

    #[test]
    fn rejects_reserved_or_untrimmed_view_ids_and_invalid_contributions() {
        for bad_id in ["__modal__", "__default__", " main", "main ", "a/b", "a\\b"] {
            let raw = serde_json::json!({
                "id": "com.example.views",
                "name": "Views",
                "version": "1.0.0",
                "api_version": 1,
                "entry": "index.js",
                "contributions": {
                    "sidebar": [{ "id": bad_id, "title": "Main" }]
                }
            })
            .to_string();
            assert!(
                PluginManifest::parse_and_validate(&raw).is_err(),
                "expected sidebar id {bad_id:?} to be rejected"
            );

            let page_raw = serde_json::json!({
                "id": "com.example.views",
                "name": "Views",
                "version": "1.0.0",
                "api_version": 1,
                "entry": "index.js",
                "contributions": {
                    "pages": [{ "id": bad_id, "title": "Page" }]
                }
            })
            .to_string();
            assert!(
                PluginManifest::parse_and_validate(&page_raw).is_err(),
                "expected page id {bad_id:?} to be rejected"
            );
        }

        // Invalid numeric setting bounds (min > max or step <= 0).
        let bad_num = r#"{
          "id": "com.example.num",
          "name": "Num",
          "version": "1.0.0",
          "api_version": 1,
          "entry": "index.js",
          "settings": [
            { "key": "limit", "type": "number", "label": "Limit", "min": 10, "max": 5 }
          ]
        }"#;
        assert!(PluginManifest::parse_and_validate(bad_num).is_err());

        let bad_step = r#"{
          "id": "com.example.step",
          "name": "Step",
          "version": "1.0.0",
          "api_version": 1,
          "entry": "index.js",
          "settings": [
            { "key": "limit", "type": "number", "label": "Limit", "step": 0 }
          ]
        }"#;
        assert!(PluginManifest::parse_and_validate(bad_step).is_err());

        // Duplicate search_providers / themes / locales and empty command titles are rejected.
        let bad_cmd = r#"{
          "id": "com.example.cmd",
          "name": "Cmd",
          "version": "1.0.0",
          "api_version": 1,
          "entry": "index.js",
          "contributions": { "commands": [{ "id": "run", "title": "   " }] }
        }"#;
        assert!(PluginManifest::parse_and_validate(bad_cmd).is_err());

        let dup_sp = r#"{
          "id": "com.example.sp",
          "name": "SP",
          "version": "1.0.0",
          "api_version": 1,
          "entry": "index.js",
          "contributions": {
            "search_providers": [
              { "id": "s1", "title": "One" },
              { "id": "s1", "title": "Two" }
            ]
          }
        }"#;
        assert!(PluginManifest::parse_and_validate(dup_sp).is_err());
    }

    #[test]
    fn activation_event_wildcard_requires_colon_boundary() {
        let raw = r#"{
          "id": "com.example.act",
          "name": "Act",
          "version": "1.0.0",
          "api_version": 1,
          "entry": "index.js",
          "activation_events": ["onCommand:*", "onEvent:task:*"]
        }"#;
        let (manifest, _) = PluginManifest::parse_and_validate(raw).unwrap();
        assert!(manifest.matches_activation_event("onCommand:run"));
        assert!(!manifest.matches_activation_event("onCommand"));
        assert!(!manifest.matches_activation_event("onCommandLine:run"));
        assert!(manifest.matches_activation_event("onEvent:task:created"));
        assert!(!manifest.matches_activation_event("onEvent:taskforce"));
        assert!(!manifest.matches_activation_event("onStartup"));
    }

    #[test]
    fn check_permission_diff_handles_hash_changes_and_shrinking_grants() {
        let raw_v1 = r#"{
          "id": "com.example.shrink",
          "name": "Shrink",
          "version": "1.0.0",
          "api_version": 1,
          "entry": "index.js",
          "permissions": ["tasks:read", { "network": ["*"] }]
        }"#;
        let (_m1, h1) = PluginManifest::parse_and_validate(raw_v1).unwrap();

        let raw_v2 = r#"{
          "id": "com.example.shrink",
          "name": "Shrink",
          "version": "1.1.0",
          "api_version": 1,
          "entry": "index.js",
          "permissions": ["tasks:read"]
        }"#;
        let (m2, h2) = PluginManifest::parse_and_validate(raw_v2).unwrap();

        let enabled_state = PluginState {
            plugin_id: "com.example.shrink".into(),
            enabled: true,
            granted_permissions: r#"["tasks:read",{"network":["*"]}]"#.into(),
            manifest_hash: h1.clone(),
            ..Default::default()
        };
        assert_eq!(
            m2.check_permission_diff(&h2, Some(&enabled_state)),
            PermissionApprovalStatus::Approved
        );
        let narrowed = m2
            .narrowed_permissions_to_persist(&h2, Some(&enabled_state))
            .expect("should shrink dropped network:* permission");
        assert_eq!(narrowed, r#"["tasks:read"]"#);

        // Once narrowed, a disabled overwritten plugin with a changed manifest_hash requires re-approval.
        let disabled_overwritten = PluginState {
            plugin_id: "com.example.shrink".into(),
            enabled: false,
            granted_permissions: r#"["tasks:read"]"#.into(),
            manifest_hash: h1,
            ..Default::default()
        };
        assert!(matches!(
            m2.check_permission_diff(&h2, Some(&disabled_overwritten)),
            PermissionApprovalStatus::NeedsApproval {
                is_first_install: false,
                ..
            }
        ));
    }
}
