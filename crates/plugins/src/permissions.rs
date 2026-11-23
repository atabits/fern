//! Capability-based plugin permission model and domain/escalation matching.

use std::collections::BTreeSet;
use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A discrete capability that a plugin or user script can request.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Permission {
    TasksRead,
    TasksWrite,
    ListsRead,
    ListsWrite,
    TagsRead,
    TagsWrite,
    HabitsRead,
    HabitsWrite,
    FocusRead,
    CountdownsRead,
    CountdownsWrite,
    Storage,
    Meta,
    Notifications,
    Clipboard,
    Network(Vec<String>),
    NetworkLocal,
    HooksIntercept,
    Services,
}

impl Permission {
    /// Canonical string identifier for flat (non-parameterized) permissions.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TasksRead => "tasks:read",
            Self::TasksWrite => "tasks:write",
            Self::ListsRead => "lists:read",
            Self::ListsWrite => "lists:write",
            Self::TagsRead => "tags:read",
            Self::TagsWrite => "tags:write",
            Self::HabitsRead => "habits:read",
            Self::HabitsWrite => "habits:write",
            Self::FocusRead => "focus:read",
            Self::CountdownsRead => "countdowns:read",
            Self::CountdownsWrite => "countdowns:write",
            Self::Storage => "storage",
            Self::Meta => "meta",
            Self::Notifications => "notifications",
            Self::Clipboard => "clipboard",
            Self::Network(_) => "network",
            Self::NetworkLocal => "network:local",
            Self::HooksIntercept => "hooks:intercept",
            Self::Services => "services",
        }
    }

    /// Parses a flat permission string (or `"network:local"`).
    pub fn parse_str(value: &str) -> Option<Self> {
        match value.trim() {
            "tasks:read" => Some(Self::TasksRead),
            "tasks:write" => Some(Self::TasksWrite),
            "lists:read" => Some(Self::ListsRead),
            "lists:write" => Some(Self::ListsWrite),
            "tags:read" => Some(Self::TagsRead),
            "tags:write" => Some(Self::TagsWrite),
            "habits:read" => Some(Self::HabitsRead),
            "habits:write" => Some(Self::HabitsWrite),
            "focus:read" => Some(Self::FocusRead),
            "countdowns:read" => Some(Self::CountdownsRead),
            "countdowns:write" => Some(Self::CountdownsWrite),
            "storage" => Some(Self::Storage),
            "meta" => Some(Self::Meta),
            "notifications" => Some(Self::Notifications),
            "clipboard" => Some(Self::Clipboard),
            "network:local" => Some(Self::NetworkLocal),
            "hooks:intercept" => Some(Self::HooksIntercept),
            "services" => Some(Self::Services),
            _ => None,
        }
    }

    /// Human-readable description for permission prompts.
    pub fn summary(&self) -> String {
        match self {
            Self::Network(hosts) => format!("network ({})", hosts.join(", ")),
            other => other.as_str().to_owned(),
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary())
    }
}

impl Serialize for Permission {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Network(hosts) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("network", hosts)?;
                map.end()
            }
            flat => serializer.serialize_str(flat.as_str()),
        }
    }
}

impl<'de> Deserialize<'de> for Permission {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PermissionVisitor;

        impl<'de> Visitor<'de> for PermissionVisitor {
            type Value = Permission;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a permission string or `{ \"network\": [\"host\"] }` object")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Permission::parse_str(v)
                    .ok_or_else(|| de::Error::unknown_variant(v, KNOWN_PERMISSION_STRS))
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut hosts: Option<Vec<String>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "network" {
                        if hosts.is_some() {
                            return Err(de::Error::duplicate_field("network"));
                        }
                        let raw: Vec<String> = map.next_value()?;
                        let mut cleaned = Vec::with_capacity(raw.len());
                        for item in raw {
                            let normalized = normalize_host_pattern(&item);
                            if normalized.is_empty() {
                                return Err(de::Error::custom(format!(
                                    "invalid network host pattern: {item:?}"
                                )));
                            }
                            cleaned.push(normalized);
                        }
                        hosts = Some(cleaned);
                    } else {
                        return Err(de::Error::unknown_field(&key, &["network"]));
                    }
                }
                let hosts = hosts.ok_or_else(|| de::Error::missing_field("network"))?;
                Ok(Permission::Network(hosts))
            }
        }

        deserializer.deserialize_any(PermissionVisitor)
    }
}

const KNOWN_PERMISSION_STRS: &[&str] = &[
    "tasks:read",
    "tasks:write",
    "lists:read",
    "lists:write",
    "tags:read",
    "tags:write",
    "habits:read",
    "habits:write",
    "focus:read",
    "countdowns:read",
    "countdowns:write",
    "storage",
    "meta",
    "notifications",
    "clipboard",
    "network:local",
    "hooks:intercept",
    "services",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum FlatPerm {
    TasksRead,
    TasksWrite,
    ListsRead,
    ListsWrite,
    TagsRead,
    TagsWrite,
    HabitsRead,
    HabitsWrite,
    FocusRead,
    CountdownsRead,
    CountdownsWrite,
    Storage,
    Meta,
    Notifications,
    Clipboard,
    NetworkLocal,
    HooksIntercept,
    Services,
}

impl FlatPerm {
    fn from_permission(p: &Permission) -> Option<Self> {
        match p {
            Permission::TasksRead => Some(Self::TasksRead),
            Permission::TasksWrite => Some(Self::TasksWrite),
            Permission::ListsRead => Some(Self::ListsRead),
            Permission::ListsWrite => Some(Self::ListsWrite),
            Permission::TagsRead => Some(Self::TagsRead),
            Permission::TagsWrite => Some(Self::TagsWrite),
            Permission::HabitsRead => Some(Self::HabitsRead),
            Permission::HabitsWrite => Some(Self::HabitsWrite),
            Permission::FocusRead => Some(Self::FocusRead),
            Permission::CountdownsRead => Some(Self::CountdownsRead),
            Permission::CountdownsWrite => Some(Self::CountdownsWrite),
            Permission::Storage => Some(Self::Storage),
            Permission::Meta => Some(Self::Meta),
            Permission::Notifications => Some(Self::Notifications),
            Permission::Clipboard => Some(Self::Clipboard),
            Permission::Network(_) => None,
            Permission::NetworkLocal => Some(Self::NetworkLocal),
            Permission::HooksIntercept => Some(Self::HooksIntercept),
            Permission::Services => Some(Self::Services),
        }
    }

    fn into_permission(self) -> Permission {
        match self {
            Self::TasksRead => Permission::TasksRead,
            Self::TasksWrite => Permission::TasksWrite,
            Self::ListsRead => Permission::ListsRead,
            Self::ListsWrite => Permission::ListsWrite,
            Self::TagsRead => Permission::TagsRead,
            Self::TagsWrite => Permission::TagsWrite,
            Self::HabitsRead => Permission::HabitsRead,
            Self::HabitsWrite => Permission::HabitsWrite,
            Self::FocusRead => Permission::FocusRead,
            Self::CountdownsRead => Permission::CountdownsRead,
            Self::CountdownsWrite => Permission::CountdownsWrite,
            Self::Storage => Permission::Storage,
            Self::Meta => Permission::Meta,
            Self::Notifications => Permission::Notifications,
            Self::Clipboard => Permission::Clipboard,
            Self::NetworkLocal => Permission::NetworkLocal,
            Self::HooksIntercept => Permission::HooksIntercept,
            Self::Services => Permission::Services,
        }
    }
}

/// Normalized capability set granted to a plugin or user script.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionSet {
    flat: BTreeSet<FlatPerm>,
    network_hosts: BTreeSet<String>,
}

/// Difference between currently granted permissions and newly requested permissions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionDiff {
    pub added: Vec<Permission>,
    pub removed: Vec<Permission>,
}

impl PermissionDiff {
    /// Returns `true` when the requested set introduces at least one capability not previously granted.
    pub fn is_escalation(&self) -> bool {
        !self.added.is_empty()
    }
}

impl PermissionSet {
    pub fn new(permissions: &[Permission]) -> Self {
        let mut set = Self::default();
        for perm in permissions {
            set.insert(perm.clone());
        }
        set
    }

    pub fn insert(&mut self, perm: Permission) {
        match perm {
            Permission::Network(hosts) => {
                for host in hosts {
                    let norm = normalize_host_pattern(&host);
                    if !norm.is_empty() {
                        self.network_hosts.insert(norm);
                    }
                }
            }
            other => {
                if let Some(fp) = FlatPerm::from_permission(&other) {
                    self.flat.insert(fp);
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.flat.is_empty() && self.network_hosts.is_empty()
    }

    pub fn to_vec(&self) -> Vec<Permission> {
        let mut out: Vec<Permission> = self.flat.iter().map(|f| f.into_permission()).collect();
        if !self.network_hosts.is_empty() {
            out.push(Permission::Network(self.network_hosts.iter().cloned().collect()));
        }
        out
    }

    pub fn to_json_string(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&self.to_vec())?)
    }

    pub fn from_json_str(json: &str) -> anyhow::Result<Self> {
        let trimmed = json.trim();
        if trimmed.is_empty() {
            return Ok(Self::default());
        }
        let perms: Vec<Permission> = serde_json::from_str(trimmed)?;
        Ok(Self::new(&perms))
    }

    pub fn has(&self, perm: &Permission) -> bool {
        match perm {
            Permission::Network(hosts) => hosts.iter().all(|h| {
                let (host, port) = split_host_port(h);
                self.allows_host(host, port)
            }),
            other => FlatPerm::from_permission(other).is_some_and(|fp| self.flat.contains(&fp)),
        }
    }

    pub fn allows_local(&self) -> bool {
        self.flat.contains(&FlatPerm::NetworkLocal)
    }

    pub fn network_hosts(&self) -> &BTreeSet<String> {
        &self.network_hosts
    }

    /// Checks whether `(host, port)` is explicitly permitted by `network: [...]` host patterns
    /// (excluding implicit loopback access granted solely by `network:local`).
    pub fn allows_explicit_host(&self, host: &str, port: Option<u16>) -> bool {
        let host_norm = normalize_host(host);
        if host_norm.is_empty() {
            return false;
        }
        self.network_hosts
            .iter()
            .any(|pattern| host_pattern_matches(pattern, &host_norm, port))
    }

    /// Checks whether `(host, port)` is permitted by `network` rules (or loopback `network:local`).
    pub fn allows_host(&self, host: &str, port: Option<u16>) -> bool {
        let host_norm = normalize_host(host);
        if host_norm.is_empty() {
            return false;
        }
        if self.allows_explicit_host(&host_norm, port) {
            return true;
        }
        self.allows_local() && is_loopback_host_literal(&host_norm)
    }

    /// Computes added and removed permissions relative to `requested`.
    pub fn diff(&self, requested: &PermissionSet) -> PermissionDiff {
        let mut added = Vec::new();
        let mut removed = Vec::new();

        for fp in &requested.flat {
            if !self.flat.contains(fp) {
                added.push(fp.into_permission());
            }
        }
        for fp in &self.flat {
            if !requested.flat.contains(fp) {
                removed.push(fp.into_permission());
            }
        }

        let added_hosts: Vec<String> = requested
            .network_hosts
            .iter()
            .filter(|pattern| !self.covers_host_pattern(pattern))
            .cloned()
            .collect();
        if !added_hosts.is_empty() {
            added.push(Permission::Network(added_hosts));
        }

        let removed_hosts: Vec<String> = self
            .network_hosts
            .iter()
            .filter(|pattern| !requested.covers_host_pattern(pattern))
            .cloned()
            .collect();
        if !removed_hosts.is_empty() {
            removed.push(Permission::Network(removed_hosts));
        }

        PermissionDiff { added, removed }
    }

    fn covers_host_pattern(&self, requested_pattern: &str) -> bool {
        let (req_host, req_port) = split_host_port(requested_pattern);
        if let Some(sub) = req_host.strip_prefix("*.") {
            return self.network_hosts.iter().any(|granted| {
                if granted == "*" {
                    return true;
                }
                let (g_host, g_port) = split_host_port(granted);
                if g_port.is_some() && g_port != req_port {
                    return false;
                }
                g_host == req_host
                    || g_host
                        .strip_prefix("*.")
                        .is_some_and(|g_sub| sub == g_sub || sub.ends_with(&format!(".{g_sub}")))
            });
        }
        self.network_hosts
            .iter()
            .any(|granted| host_pattern_matches(granted, req_host, req_port))
    }
}

pub fn normalize_host(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

pub fn normalize_host_pattern(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return String::new();
    }
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some((ip, port_part)) = rest.split_once("]:") {
            let ip_norm = normalize_host(ip);
            if !ip_norm.is_empty() && port_part.parse::<u16>().is_ok() {
                return format!("[{ip_norm}]:{port_part}");
            }
            return String::new();
        }
        return normalize_host(rest);
    }
    let (host, port) = split_host_port(trimmed);
    let host_norm = normalize_host(host);
    if host_norm.is_empty() || host_norm.contains('/') {
        return String::new();
    }
    match port {
        Some(p) => format!("{host_norm}:{p}"),
        None => host_norm,
    }
}

fn split_host_port(value: &str) -> (&str, Option<u16>) {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some((ip, port_str)) = rest.split_once("]:") {
            return (ip, port_str.parse::<u16>().ok());
        }
        return (rest.trim_end_matches(']'), None);
    }
    if let Some((host, port_str)) = trimmed.rsplit_once(':')
        && !host.contains(':')
        && let Ok(port) = port_str.parse::<u16>()
    {
        return (host, Some(port));
    }
    (trimmed, None)
}

fn host_pattern_matches(pattern: &str, host: &str, port: Option<u16>) -> bool {
    let (pat_host, pat_port) = split_host_port(pattern);
    if let Some(expected_port) = pat_port
        && port != Some(expected_port)
    {
        return false;
    }
    let pat_host = normalize_host(pat_host);
    let host = normalize_host(host);
    if pat_host == "*" {
        return !host.is_empty();
    }
    if let Some(suffix) = pat_host.strip_prefix("*.") {
        if suffix.is_empty() || host.len() <= suffix.len() + 1 {
            return false;
        }
        let dot_suffix = format!(".{suffix}");
        return host.ends_with(&dot_suffix) && !host[..host.len() - dot_suffix.len()].is_empty();
    }
    pat_host == host
}

/// Returns `true` strictly for exact `"localhost"` or literal loopback IP addresses (`127.0.0.0/8`, `::1`).
///
/// Subdomains such as `exfil.localhost` are intentionally NOT matched implicitly by `network:local`
/// to prevent non-RFC-6761 resolvers from bypassing the host allowlist.
pub fn is_loopback_host_literal(host: &str) -> bool {
    let h = normalize_host(host);
    h == "localhost"
        || h.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback(),
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_and_diff_matching() {
        let granted = PermissionSet::new(&[
            Permission::TasksRead,
            Permission::Network(vec!["*.github.com".into(), "api.openai.com:443".into()]),
        ]);
        assert!(granted.allows_host("api.github.com", Some(443)));
        assert!(granted.allows_host("sub.api.github.com", None));
        assert!(!granted.allows_host("github.com", None));
        assert!(!granted.allows_host("evilgithub.com", None));
        assert!(granted.allows_host("api.openai.com", Some(443)));
        assert!(!granted.allows_host("api.openai.com", Some(8443)));

        let narrower = PermissionSet::new(&[
            Permission::TasksRead,
            Permission::Network(vec!["api.github.com".into()]),
        ]);
        assert!(!granted.diff(&narrower).is_escalation());

        let escalated = PermissionSet::new(&[
            Permission::TasksRead,
            Permission::TasksWrite,
            Permission::Network(vec!["evil.example.com".into()]),
        ]);
        let diff = granted.diff(&escalated);
        assert!(diff.is_escalation());
        assert!(diff.added.contains(&Permission::TasksWrite));
    }

    #[test]
    fn network_local_blocks_subdomain_localhost_bypass() {
        let local_only = PermissionSet::new(&[Permission::NetworkLocal]);
        assert!(local_only.allows_host("localhost", Some(18481)));
        assert!(local_only.allows_host("127.0.0.1", Some(18481)));
        assert!(local_only.allows_host("::1", Some(18481)));
        assert!(!local_only.allows_explicit_host("localhost", Some(18481)));
        assert!(!local_only.allows_host("exfil.localhost", Some(80)));
        assert!(!local_only.allows_host("a.b.localhost", Some(443)));
        assert!(!local_only.allows_host("192.168.1.10", Some(80)));
    }
}
