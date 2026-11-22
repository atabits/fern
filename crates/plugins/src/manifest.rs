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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarContribution {
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
            ensure!(!view.id.trim().is_empty(), "sidebar view id must not be empty");
            ensure!(
                !view.id.contains('/'),
                "sidebar view id {:?} must not contain '/'",
                view.id
            );
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

        let mut cmd_ids = HashSet::new();
        for cmd in &self.contributions.commands {
            ensure!(!cmd.id.trim().is_empty(), "command id must not be empty");
            ensure!(
                cmd_ids.insert(cmd.id.as_str()),
                "duplicate command id {:?}",
                cmd.id
            );
        }

        let mut setting_keys = HashSet::new();
        for setting in &self.settings {
            ensure!(!setting.key.trim().is_empty(), "setting key must not be empty");
            ensure!(
                setting_keys.insert(setting.key.as_str()),
                "duplicate setting key {:?}",
                setting.key
            );
            if setting.kind == SettingKind::Select {
                ensure!(
                    !setting.options.is_empty(),
                    "select setting {:?} must define options",
                    setting.key
                );
            }
        }

        for theme in &self.contributions.themes {
            ensure!(!theme.id.trim().is_empty(), "theme id must not be empty");
            if let Some(path) = &theme.path {
                validate_relative_path(path)?;
            }
        }
        for locale in &self.contributions.locales {
            ensure!(!locale.code.trim().is_empty(), "locale code must not be empty");
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
            pattern == "*"
                || pattern == event
                || pattern
                    .strip_suffix(":*")
                    .is_some_and(|prefix| event.starts_with(prefix))
                || (pattern == "onEvent:*" && event.starts_with("onEvent:"))
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

        let granted = PermissionSet::from_json_str(&state.granted_permissions).unwrap_or_default();
        let diff = granted.diff(&requested);
        if diff.is_escalation() {
            return PermissionApprovalStatus::NeedsApproval {
                diff,
                is_first_install: state.manifest_hash.is_empty(),
            };
        }
        // If manifest hash changed and no permissions were ever recorded yet while requesting permissions.
        if state.manifest_hash.is_empty() && !requested.is_empty() {
            return PermissionApprovalStatus::NeedsApproval {
                diff: PermissionSet::default().diff(&requested),
                is_first_install: true,
            };
        }
        let _ = manifest_hash;
        PermissionApprovalStatus::Approved
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
    }
}
