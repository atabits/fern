//! Sync between devices, the store's side: what changed here since a point, and applying what
//! changed elsewhere.
//!
//! Every write to synced data is logged by triggers (`sync_log`: table, row key, time). A
//! device sends the rows changed since its last send, as they are now — or as deleted — and
//! applies the changes it receives row by row: the later change of a row wins. Changes applied
//! that way are not logged again, so they do not bounce back. A server only has to keep the
//! latest change of every row and hand out the ones a device has not seen.

use std::collections::{HashMap, HashSet};
use std::fmt;

use anyhow::{Context as _, Result, bail};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{OptionalExtension as _, params, params_from_iter};
use serde_json::{Map, Value as Json};
use sha2::{Digest as _, Sha256};

use crate::cloud::CloudMeta;
use crate::e2ee::{self, E2eeError, E2eeKey};
use crate::store::Store;

/// Current sync protocol version spoken by this client.
pub const SYNC_PROTOCOL_VERSION: u32 = 2;
/// Oldest sync protocol version this client is compatible with.
pub const MIN_SYNC_PROTOCOL_VERSION: u32 = 1;

/// Returned when the sync server speaks a protocol version outside this client's supported range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompatibleProtocol {
    pub client: u32,
    pub server: u32,
    pub min_server: u32,
}

impl fmt::Display for IncompatibleProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "incompatible sync protocol: client supports {MIN_SYNC_PROTOCOL_VERSION}..={client}, server reported {server} (min {min_server})",
            client = self.client,
            server = self.server,
            min_server = self.min_server,
        )
    }
}

impl std::error::Error for IncompatibleProtocol {}

/// The sync server stores an attachment key as one file-name component.
pub const MAX_FILE_KEY_BYTES: usize = 255;

/// Canonical server storage key for an attachment or countdown background file name.
pub fn file_key(name: &str) -> String {
    let encoded = encode_file_key(name, true);
    if encoded.len() <= MAX_FILE_KEY_BYTES {
        encoded
    } else {
        // Escaping UTF-8 can exceed the server's component limit even for a valid local name.
        // The names already contain unique IDs. Only the storage key changes; sync metadata and
        // downloaded file names retain the original name. This works with older servers too.
        format!("~sha256-{:x}", Sha256::digest(name.as_bytes()))
    }
}

/// Percent-encodes a file name for use as a sync server `/files/<key>` path segment.
pub fn encode_file_key(name: &str, legacy_punctuation: bool) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(name.len());
    for byte in name.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        // Preserve the legacy keys for URL punctuation such as '+', '(' and ')'. ':' is
        // escaped as it has special meaning in native Windows paths.
        let punctuation = b"!$&'()*+,;=@[]{}".contains(&byte);
        if unreserved || (legacy_punctuation && punctuation) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// The synced tables, parents before children, with their key columns.
pub const SYNCED: &[(&str, &[&str])] = &[
    ("list_folders", &["id"]),
    ("lists", &["id"]),
    ("list_sections", &["id"]),
    ("tags", &["name"]),
    ("tasks", &["id"]),
    ("task_tags", &["task_id", "tag"]),
    ("checklist_items", &["id"]),
    ("countdowns", &["id"]),
    ("habit_sections", &["id"]),
    ("habits", &["id"]),
    ("habit_checkins", &["habit_id", "date"]),
    ("focus_sessions", &["id"]),
    ("templates", &["id"]),
    ("filters", &["id"]),
    ("summary_templates", &["id"]),
    ("task_attachments", &["id"]),
    ("task_comments", &["id"]),
    ("focus_timers", &["id"]),
    ("matrix_rules", &["quadrant"]),
    ("matrix_profiles", &["id"]),
];

/// Key parts are joined with this character, as the triggers do.
const KEY_SEPARATOR: char = '\u{1f}';
const FILTER_RULES_PREFIX: &str = "lists! fern_rules_v1:";

/// One row's change: the row as it is now, or `None` when it was deleted.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub entity: String,
    /// The key columns' values joined with U+001F.
    pub key: String,
    /// Unix milliseconds of the change; the later change of a row wins.
    pub at: i64,
    pub row: Option<Map<String, Json>>,
}

impl Change {
    /// The change as JSON, the form it travels in.
    pub fn to_json(&self) -> Json {
        serde_json::json!({ "entity": self.entity, "key": self.key, "at": self.at, "row": self.row })
    }

    pub fn from_json(value: &Json) -> Result<Self> {
        let entity = value["entity"].as_str().context("a change needs an entity")?.to_owned();
        let key = value["key"].as_str().context("a change needs a key")?.to_owned();
        let at = value["at"].as_i64().context("a change needs a time")?;
        let row = match &value["row"] {
            Json::Null => None,
            Json::Object(row) => Some(row.clone()),
            _ => bail!("a change's row must be an object or null"),
        };
        Ok(Self { entity, key, at, row })
    }
}

fn keys_of(entity: &str) -> Result<&'static [&'static str]> {
    SYNCED.iter().find(|(name, _)| *name == entity).map(|(_, keys)| *keys).with_context(|| format!("{entity} is not synced"))
}

fn to_json(value: ValueRef) -> Json {
    match value {
        ValueRef::Null => Json::Null,
        ValueRef::Integer(i) => Json::from(i),
        ValueRef::Real(f) => Json::from(f),
        ValueRef::Text(t) => Json::from(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Json::from(b.to_vec()),
    }
}

fn to_sql(value: &Json) -> Value {
    match value {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Integer(*b as i64),
        Json::Number(n) => n.as_i64().map(Value::Integer).unwrap_or_else(|| Value::Real(n.as_f64().unwrap_or(0.))),
        Json::String(s) => Value::Text(s.clone()),
        other => Value::Text(other.to_string()),
    }
}

/// Column names may only be plain identifiers: they come from other devices.
fn is_identifier(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Where this device syncs to: the server's address, the token, and optional E2EE passphrase.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncConfig {
    pub server: String,
    pub token: String,
    pub passphrase: String,
}

impl SyncConfig {
    /// Whether both a server URL and bearer token are configured.
    pub fn is_configured(&self) -> bool {
        !self.server.trim().is_empty() && !self.token.trim().is_empty()
    }
}

/// Summary returned by a detailed sync application.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub applied: usize,
    pub conflicts_preserved: usize,
}

const SYNC_SERVER: &str = "sync_server";
const SYNC_TOKEN: &str = "sync_token";
const SYNC_PASSPHRASE: &str = "sync_passphrase";
const SYNC_DEVICE: &str = "sync_device";
const SYNC_SENT: &str = "sync_sent";
const SYNC_SEEN: &str = "sync_seen";
const CLOUD_SERVER: &str = "cloud_server";
const CLOUD_USER_ID: &str = "cloud_user_id";
const CLOUD_EMAIL: &str = "cloud_email";
const CLOUD_DISPLAY_NAME: &str = "cloud_display_name";

/// Active synchronization mode for this local database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncMode {
    Off,
    Cloud(CloudMeta),
    SelfHosted(SyncConfig),
}

/// A sync on its way: the request to send, and what it covers of the local log.
pub struct SyncRequest {
    pub body: Json,
    sent: i64,
}

/// Extracts the raw local attachment or countdown background filename from a plaintext synced row.
pub fn extract_plaintext_file_name(entity: &str, row: &Map<String, Json>) -> Option<String> {
    match entity {
        "task_attachments" => {
            if let Some(stored) = row.get("stored_name").and_then(Json::as_str).filter(|s| !s.is_empty()) {
                return Some(stored.to_owned());
            }
            let id = row.get("id").and_then(Json::as_str).filter(|s| !s.is_empty())?;
            let name = row.get("name").and_then(Json::as_str).filter(|s| !s.is_empty())?;
            Some(format!("{id}-{name}"))
        }
        "countdowns" => {
            if let Some(bg) = row.get("background").and_then(Json::as_str).filter(|s| !s.is_empty()) {
                return Some(bg.to_owned());
            }
            row.get("cover_image")
                .and_then(Json::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        }
        _ => None,
    }
}

fn change_file_key(entity: &str, row: &Map<String, Json>) -> Option<String> {
    extract_plaintext_file_name(entity, row).map(|name| file_key(&name))
}

impl Store {
    /// The sync settings, if a server is set.
    pub fn sync_config(&self) -> Result<Option<SyncConfig>> {
        let server = self.setting(SYNC_SERVER)?.unwrap_or_default();
        let token = self.setting(SYNC_TOKEN)?.unwrap_or_default();
        let passphrase = self.setting(SYNC_PASSPHRASE)?.unwrap_or_default();
        Ok((!server.trim().is_empty()).then_some(SyncConfig { server, token, passphrase }))
    }

    /// Sets the server, token, and optional E2EE passphrase. A different server or passphrase
    /// starts over: everything here is sent again, and everything the server has is received.
    pub fn set_sync_config(&self, config: &SyncConfig) -> Result<()> {
        let server = config.server.trim();
        self.in_tx(|| {
            let server_changed = self.setting(SYNC_SERVER)?.unwrap_or_default() != server;
            let passphrase_changed = self.setting(SYNC_PASSPHRASE)?.unwrap_or_default() != config.passphrase;
            if server_changed || passphrase_changed {
                self.set_setting(SYNC_SENT, "0")?;
                self.set_setting(SYNC_SEEN, "0")?;
                self.conn.execute("DELETE FROM sync_uploaded", [])?;
            }
            self.set_setting(SYNC_SERVER, server)?;
            self.set_setting(SYNC_TOKEN, config.token.trim())?;
            self.set_setting(SYNC_PASSPHRASE, &config.passphrase)?;
            Ok(())
        })
    }

    /// This device's id for the server, made once.
    pub fn sync_device(&self) -> Result<String> {
        if let Some(id) = self.setting(SYNC_DEVICE)? {
            return Ok(id);
        }
        let id = uuid::Uuid::now_v7().to_string();
        self.set_setting(SYNC_DEVICE, &id)?;
        Ok(id)
    }

    fn sync_number(&self, key: &str) -> Result<i64> {
        Ok(self.setting(key)?.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    /// Whether there are local changes the server has not got yet.
    pub fn has_unsent_changes(&self) -> Result<bool> {
        Ok(self.sync_position()? > self.sync_number(SYNC_SENT)?)
    }

    /// Non-secret metadata for the currently connected Fern Cloud account, if any.
    pub fn cloud_meta(&self) -> Result<Option<CloudMeta>> {
        let server_url = self.setting(CLOUD_SERVER)?.unwrap_or_default();
        let user_id = self.setting(CLOUD_USER_ID)?.unwrap_or_default();
        let email = self.setting(CLOUD_EMAIL)?.unwrap_or_default();
        let display_name = self.setting(CLOUD_DISPLAY_NAME)?.unwrap_or_default();
        let meta = CloudMeta {
            server_url,
            user_id,
            email,
            display_name,
        };
        Ok(meta.is_configured().then_some(meta))
    }

    /// Updates or clears the non-secret Fern Cloud account metadata. Switching to a different
    /// cloud server or user ID resets sync cursors and watermarks so the new vault syncs cleanly.
    pub fn set_cloud_meta(&self, meta: Option<&CloudMeta>) -> Result<()> {
        self.in_tx(|| {
            match meta {
                Some(meta) => {
                    let prev_server = self.setting(CLOUD_SERVER)?.unwrap_or_default();
                    let prev_user = self.setting(CLOUD_USER_ID)?.unwrap_or_default();
                    let server = meta.server_url.trim();
                    let user_id = meta.user_id.trim();
                    let changed_account = prev_server != server || prev_user != user_id;
                    if changed_account {
                        self.set_setting(SYNC_SENT, "0")?;
                        self.set_setting(SYNC_SEEN, "0")?;
                        self.conn.execute("DELETE FROM sync_uploaded", [])?;
                        self.conn.execute("DELETE FROM sync_watermarks", [])?;
                    }
                    self.set_setting(CLOUD_SERVER, server)?;
                    self.set_setting(CLOUD_USER_ID, user_id)?;
                    self.set_setting(CLOUD_EMAIL, meta.email.trim())?;
                    self.set_setting(CLOUD_DISPLAY_NAME, meta.display_name.trim())?;
                }
                None => {
                    self.set_setting(CLOUD_SERVER, "")?;
                    self.set_setting(CLOUD_USER_ID, "")?;
                    self.set_setting(CLOUD_EMAIL, "")?;
                    self.set_setting(CLOUD_DISPLAY_NAME, "")?;
                    self.set_setting(SYNC_SENT, "0")?;
                    self.set_setting(SYNC_SEEN, "0")?;
                    self.conn.execute("DELETE FROM sync_uploaded", [])?;
                    self.conn.execute("DELETE FROM sync_watermarks", [])?;
                }
            }
            Ok(())
        })
    }

    /// Resets sync cursors so the next sync pushes all local rows and pulls full server state.
    pub fn reset_sync_cursors(&self, clear_watermarks: bool) -> Result<()> {
        self.in_tx(|| {
            self.set_setting(SYNC_SENT, "0")?;
            self.set_setting(SYNC_SEEN, "0")?;
            self.conn.execute("DELETE FROM sync_uploaded", [])?;
            if clear_watermarks {
                self.conn.execute("DELETE FROM sync_watermarks", [])?;
            }
            Ok(())
        })
    }

    /// Wipes all synced user entities and sync logs without generating deletion tombstones.
    /// Called when switching directly from one signed-in Fern Cloud user to a different `user_id`
    /// so the previous account's local rows are never uploaded into the new account's vault.
    pub fn clear_synced_user_data(&self) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute_batch(
                "PRAGMA defer_foreign_keys = ON; INSERT INTO sync_applying VALUES (1);",
            )?;
            let wipe_res = (|| -> Result<()> {
                for &(table, _) in SYNCED.iter().rev() {
                    self.conn.execute(&format!("DELETE FROM {table}"), [])?;
                }
                self.conn.execute("DELETE FROM sync_log", [])?;
                self.conn.execute("DELETE FROM sync_uploaded", [])?;
                self.conn.execute("DELETE FROM sync_watermarks", [])?;
                self.set_setting(SYNC_SENT, "0")?;
                self.set_setting(SYNC_SEEN, "0")?;
                Ok(())
            })();
            let cleanup_res = self.conn.execute("DELETE FROM sync_applying", []);
            wipe_res?;
            cleanup_res?;
            Ok(())
        })
    }

    /// Returns the active synchronization mode (Fern Cloud takes precedence over self-hosted when connected).
    pub fn sync_mode(&self) -> Result<SyncMode> {
        if let Some(meta) = self.cloud_meta()? {
            return Ok(SyncMode::Cloud(meta));
        }
        if let Some(cfg) = self.sync_config()?
            && cfg.is_configured()
        {
            return Ok(SyncMode::SelfHosted(cfg));
        }
        Ok(SyncMode::Off)
    }

    /// Last remote server sequence number seen by this device.
    pub fn sync_seen_position(&self) -> Result<i64> {
        self.sync_number(SYNC_SEEN)
    }

    /// The next sync's request: this device's unsent changes and what it has seen.
    pub fn sync_request(&self) -> Result<SyncRequest> {
        let passphrase = self.setting(SYNC_PASSPHRASE)?.unwrap_or_default();
        let e2ee_key = if passphrase.is_empty() {
            None
        } else {
            Some(E2eeKey::from_passphrase(&passphrase)?)
        };
        self.sync_request_with_key(e2ee_key.as_ref())
    }

    /// Builds a sync request encrypting outbound rows with an explicit `E2eeKey` (e.g., an
    /// unlocked Fern Cloud Vault DEK) instead of the self-hosted `sync_passphrase` setting.
    pub fn sync_request_with_key(&self, e2ee_key: Option<&E2eeKey>) -> Result<SyncRequest> {
        let (changes, sent) = self.changes_since(self.sync_number(SYNC_SENT)?)?;
        let mut wire_changes = Vec::with_capacity(changes.len());
        for change in changes {
            let fk = change.row.as_ref().and_then(|row| change_file_key(&change.entity, row));
            let row = match (e2ee_key, change.row) {
                (Some(key), Some(row)) => Some(key.encrypt_row(&change.entity, &change.key, &row)?),
                (_, row) => row,
            };
            let mut obj = Map::with_capacity(5);
            obj.insert("entity".into(), Json::from(change.entity));
            obj.insert("key".into(), Json::from(change.key));
            obj.insert("at".into(), Json::from(change.at));
            obj.insert("row".into(), match row {
                Some(map) => Json::Object(map),
                None => Json::Null,
            });
            if let Some(fk) = fk {
                obj.insert("file_key".into(), Json::from(fk));
            }
            wire_changes.push(Json::Object(obj));
        }
        let body = serde_json::json!({
            "protocol": SYNC_PROTOCOL_VERSION,
            "device": self.sync_device()?,
            "since": self.sync_number(SYNC_SEEN)?,
            "changes": wire_changes,
        });
        Ok(SyncRequest { body, sent })
    }

    /// Applies the server's answer to `request` and remembers how far both sides got. Returns
    /// how many changes came in.
    pub fn finish_sync(&self, request: &SyncRequest, answer: &Json) -> Result<usize> {
        Ok(self.finish_sync_detailed(request, answer)?.applied)
    }

    /// Applies the server's answer to `request` and returns both the applied change count and
    /// how many offline note conflicts were preserved.
    pub fn finish_sync_detailed(&self, request: &SyncRequest, answer: &Json) -> Result<SyncOutcome> {
        let passphrase = self.setting(SYNC_PASSPHRASE)?.unwrap_or_default();
        let e2ee_key = if passphrase.is_empty() {
            None
        } else {
            Some(E2eeKey::from_passphrase(&passphrase)?)
        };
        self.finish_sync_detailed_with_key(request, answer, e2ee_key.as_ref())
    }

    /// Applies the server's answer using an explicit `E2eeKey` (such as a Fern Cloud Vault DEK).
    pub fn finish_sync_detailed_with_key(
        &self,
        request: &SyncRequest,
        answer: &Json,
        e2ee_key: Option<&E2eeKey>,
    ) -> Result<SyncOutcome> {
        if let Some(server_proto) = answer.get("protocol").and_then(Json::as_u64) {
            let server_proto = u32::try_from(server_proto).unwrap_or(u32::MAX);
            let min_server = answer
                .get("min_protocol")
                .and_then(Json::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(MIN_SYNC_PROTOCOL_VERSION);
            if !(MIN_SYNC_PROTOCOL_VERSION..=SYNC_PROTOCOL_VERSION).contains(&server_proto) {
                return Err(IncompatibleProtocol {
                    client: SYNC_PROTOCOL_VERSION,
                    server: server_proto,
                    min_server,
                }
                .into());
            }
        }
        let received = answer["changes"]
            .as_array()
            .context("the server's answer has no changes")?
            .iter()
            .map(Change::from_json)
            .collect::<Result<Vec<_>>>()?;
        let position = answer["position"].as_i64().context("the server's answer has no position")?;
        self.in_tx(|| {
            let outcome = self.apply_changes_detailed_with_key(&received, e2ee_key)?;
            self.set_setting(SYNC_SENT, &request.sent.to_string())?;
            self.set_setting(SYNC_SEEN, &position.to_string())?;
            self.compact_sync_log()?;
            Ok(outcome)
        })
    }
}

impl Store {
    /// Attached files to move after a sync, by their file names in the attachment folder `dir`:
    /// the ones here the server does not have yet, and the ones listed here whose file is missing.
    pub fn files_to_sync(&self, dir: &std::path::Path) -> Result<(Vec<String>, Vec<String>)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.id, a.name, u.name IS NOT NULL FROM task_attachments a
             LEFT JOIN sync_uploaded u ON u.name = a.id || '-' || a.name",
        )?;
        let mut files: Vec<(String, bool)> = stmt
            .query_map([], |r| Ok((format!("{}-{}", r.get::<_, String>(0)?, r.get::<_, String>(1)?), r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        // Countdowns' background pictures go the same way.
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.background, u.name IS NOT NULL FROM countdowns c
             LEFT JOIN sync_uploaded u ON u.name = c.background WHERE c.background IS NOT NULL",
        )?;
        files.extend(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?);
        let (mut upload, mut download) = (Vec::new(), Vec::new());
        for (file, uploaded) in files {
            match (dir.join(&file).exists(), uploaded) {
                (true, false) => upload.push(file),
                (false, _) => download.push(file),
                (true, true) => {}
            }
        }
        Ok((upload, download))
    }

    /// Notes that the server has these files (sent from here, or fetched from it).
    pub fn mark_files_synced(&self, files: &[String]) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }
        self.in_tx(|| {
            let mut stmt = self
                .conn
                .prepare_cached("INSERT OR IGNORE INTO sync_uploaded (name) VALUES (?1)")?;
            for file in files {
                stmt.execute([file])?;
            }
            Ok(())
        })
    }

    /// Keeps one log entry per row, its latest: sending and conflicts only look at that one, so
    /// older entries are dropped whether or not they were sent. Returns how many went.
    pub fn compact_sync_log(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM sync_log WHERE EXISTS (
                 SELECT 1 FROM sync_log later
                 WHERE later.entity = sync_log.entity AND later.key = sync_log.key
                   AND later.seq > sync_log.seq AND later.at >= sync_log.at
             )",
            [],
        )?)
    }

    /// The last entry of the change log: what a device has sent up to.
    pub fn sync_position(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM sync_log", [], |row| row.get(0))?)
    }

    /// The rows changed here after log entry `since`, as they are now, each once with its
    /// latest change time; and the log position they reach.
    pub fn changes_since(&self, since: i64) -> Result<(Vec<Change>, i64)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT entity, key, MAX(at), MAX(seq) FROM sync_log WHERE seq > ?1 GROUP BY entity, key ORDER BY MAX(seq)",
        )?;
        let logged: Vec<(String, String, i64, i64)> =
            stmt.query_map([since], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?.collect::<Result<_, _>>()?;
        let mut position = since;
        let mut changes = Vec::new();
        for (entity, key, at, seq) in logged {
            position = position.max(seq);
            let row = self.sync_row(&entity, &key)?;
            changes.push(Change { entity, key, at, row });
        }
        Ok((changes, position))
    }

    fn sync_row(&self, entity: &str, key: &str) -> Result<Option<Map<String, Json>>> {
        let keys = keys_of(entity)?;
        let parts: Vec<&str> = key.splitn(keys.len(), KEY_SEPARATOR).collect();
        anyhow::ensure!(parts.len() == keys.len(), "malformed sync key for {entity}");
        let condition: Vec<String> = keys.iter().enumerate().map(|(i, k)| format!("{k} = ?{}", i + 1)).collect();
        let mut stmt = self.conn.prepare_cached(&format!("SELECT * FROM {entity} WHERE {}", condition.join(" AND ")))?;
        let names: Vec<String> = stmt.column_names().map(str::to_owned).collect();
        let mut row = stmt
            .query_row(params_from_iter(parts), |row| {
                let mut map = Map::new();
                for (i, name) in names.iter().enumerate() {
                    map.insert(name.clone(), to_json(row.get_ref(i)?));
                }
                Ok(map)
            })
            .optional()?;
        if entity == "filters"
            && let Some(row) = &mut row
            && let Some(Json::String(rules)) = row.remove("rules")
        {
            // Older clients insert every wire field into SQLite. Keep their column names and
            // give them a filter that matches nothing until they can read the new rules.
            row.insert("lists".into(), Json::from(""));
            row.insert("chain".into(), Json::from(format!(
                "{FILTER_RULES_PREFIX}{}", hex::encode(rules)
            )));
        }
        Ok(row)
    }

    /// Applies changes from another device. A row changed here later than the incoming change
    /// keeps its local state. Applied changes are not logged, so they are not sent back.
    pub fn apply_changes(&self, changes: &[Change]) -> Result<usize> {
        Ok(self.apply_changes_detailed(changes)?.applied)
    }

    /// Applies changes from another device and returns both the applied change count and the
    /// number of offline note conflicts preserved.
    pub fn apply_changes_detailed(&self, changes: &[Change]) -> Result<SyncOutcome> {
        let passphrase = self.setting(SYNC_PASSPHRASE)?.unwrap_or_default();
        let e2ee_key = if passphrase.is_empty() {
            None
        } else {
            Some(E2eeKey::from_passphrase(&passphrase)?)
        };
        self.apply_changes_detailed_with_key(changes, e2ee_key.as_ref())
    }

    /// Applies changes from another device using an explicit `E2eeKey` (e.g., an unlocked Fern
    /// Cloud Vault DEK) instead of the `sync_passphrase` setting.
    pub fn apply_changes_detailed_with_key(
        &self,
        changes: &[Change],
        explicit_key: Option<&E2eeKey>,
    ) -> Result<SyncOutcome> {
        self.in_tx(|| {
            // Rows may arrive before the rows they point to.
            self.conn.execute_batch("PRAGMA defer_foreign_keys = ON; INSERT INTO sync_applying VALUES (1);")?;
            let sync_sent = self.sync_number(SYNC_SENT)?;

            let mut ordered: Vec<&Change> = changes.iter().collect();
            let rank = |entity: &str| SYNCED.iter().position(|(name, _)| *name == entity).unwrap_or(usize::MAX);
            ordered.sort_by_key(|change| (change.row.is_none(), rank(&change.entity)));
            let mut table_columns: HashMap<&str, HashSet<String>> = HashMap::new();
            let mut applied = 0;
            let mut conflicts_preserved = 0;
            let mut conflict_relogs: Vec<(String, i64)> = Vec::new();

            for change in ordered {
                let Ok(keys) = keys_of(&change.entity) else {
                    continue;
                };
                let local: Option<i64> = self
                    .conn
                    .prepare_cached(
                        "SELECT MAX(at) FROM (
                             SELECT MAX(at) AS at FROM sync_log WHERE entity = ?1 AND key = ?2
                             UNION ALL
                             SELECT at FROM sync_watermarks WHERE entity = ?1 AND key = ?2
                         )",
                    )?
                    .query_row([&change.entity, &change.key], |row| row.get(0))
                    .optional()?
                    .flatten();
                if local.is_some_and(|at| at > change.at) {
                    continue;
                }
                let parts: Vec<&str> = change.key.splitn(keys.len(), KEY_SEPARATOR).collect();
                anyhow::ensure!(parts.len() == keys.len(), "malformed sync key for {}", change.entity);
                match &change.row {
                    Some(raw_row) => {
                        let decrypted_row;
                        let plain_row = if e2ee::is_encrypted_row(raw_row) {
                            let key = explicit_key.ok_or(E2eeError::PassphraseRequired)?;
                            decrypted_row = key.decrypt_row(&change.entity, &change.key, raw_row)?;
                            &decrypted_row
                        } else if explicit_key.is_some() {
                            return Err(E2eeError::Malformed(
                                "received unencrypted sync row while E2EE is enabled".into(),
                            )
                            .into());
                        } else {
                            raw_row
                        };

                        let filter_row;
                        let conflict_row;
                        let row = if change.entity == "filters" {
                            filter_row = decode_filter_row(plain_row)?;
                            &filter_row
                        } else if change.entity == "tasks" {
                            if let Some(merged) = self.preserve_task_note_conflict(&change.key, sync_sent, plain_row)? {
                                conflict_row = merged;
                                conflicts_preserved += 1;
                                conflict_relogs.push((change.key.clone(), change.at));
                                &conflict_row
                            } else {
                                plain_row
                            }
                        } else {
                            plain_row
                        };

                        let valid_cols = match table_columns.get(change.entity.as_str()) {
                            Some(cols) => cols,
                            None => {
                                let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({})", change.entity))?;
                                let cols: HashSet<String> =
                                    stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_, _>>()?;
                                table_columns.entry(change.entity.as_str()).or_insert(cols)
                            }
                        };
                        let columns: Vec<&String> =
                            row.keys().filter(|c| is_identifier(c) && valid_cols.contains(c.as_str())).collect();
                        if columns.is_empty() || !keys.iter().all(|k| columns.iter().any(|c| c.as_str() == *k)) {
                            continue;
                        }
                        let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
                        let updates: Vec<String> = columns
                            .iter()
                            .filter(|c| !keys.contains(&c.as_str()))
                            .map(|c| format!("{c} = excluded.{c}"))
                            .collect();
                        let on_conflict = if updates.is_empty() {
                            "DO NOTHING".to_owned()
                        } else {
                            format!("DO UPDATE SET {}", updates.join(", "))
                        };
                        let sql = format!(
                            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT({}) {on_conflict}",
                            change.entity,
                            columns.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", "),
                            placeholders.join(", "),
                            keys.join(", "),
                        );
                        self.conn
                            .prepare_cached(&sql)?
                            .execute(params_from_iter(columns.iter().map(|c| to_sql(&row[c.as_str()]))))?;
                    }
                    None => {
                        let condition: Vec<String> = keys.iter().enumerate().map(|(i, k)| format!("{k} = ?{}", i + 1)).collect();
                        let sql = format!("DELETE FROM {} WHERE {}", change.entity, condition.join(" AND "));
                        self.conn.prepare_cached(&sql)?.execute(params_from_iter(parts))?;
                    }
                }
                self.conn.prepare_cached(
                    "INSERT INTO sync_watermarks (entity, key, at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(entity, key) DO UPDATE SET at = MAX(sync_watermarks.at, excluded.at)",
                )?.execute(params![&change.entity, &change.key, change.at])?;
                applied += 1;
            }
            self.conn.execute_batch("DELETE FROM sync_applying;")?;
            if !conflict_relogs.is_empty() {
                let mut relog = self
                    .conn
                    .prepare_cached("INSERT INTO sync_log (entity, key, at) VALUES ('tasks', ?1, ?2)")?;
                let now_ms = chrono::Utc::now().timestamp_millis();
                for (task_key, remote_at) in conflict_relogs {
                    relog.execute(params![task_key, now_ms.max(remote_at + 1)])?;
                }
            }
            Ok(SyncOutcome { applied, conflicts_preserved })
        })
    }

    fn preserve_task_note_conflict(
        &self,
        task_key: &str,
        sync_sent: i64,
        incoming: &Map<String, Json>,
    ) -> Result<Option<Map<String, Json>>> {
        let has_unsent_local: bool = self
            .conn
            .prepare_cached("SELECT 1 FROM sync_log WHERE entity = 'tasks' AND key = ?1 AND seq > ?2 LIMIT 1")?
            .query_row(params![task_key, sync_sent], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        if !has_unsent_local {
            return Ok(None);
        }
        let Some(local_notes): Option<String> = self
            .conn
            .prepare_cached("SELECT notes FROM tasks WHERE id = ?1")?
            .query_row([task_key], |row| row.get(0))
            .optional()?
        else {
            return Ok(None);
        };
        let Some(remote_notes) = incoming.get("notes").and_then(Json::as_str) else {
            return Ok(None);
        };
        if local_notes.trim().is_empty()
            || remote_notes.trim().is_empty()
            || local_notes == remote_notes
            || remote_notes.contains(&local_notes)
            || local_notes.contains(remote_notes)
        {
            return Ok(None);
        }
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC");
        let merged = format!("{remote_notes}\n\n---\nSync conflict ({timestamp}):\n\n{local_notes}");
        let mut updated = incoming.clone();
        updated.insert("notes".into(), Json::from(merged));
        Ok(Some(updated))
    }
}

fn decode_filter_row(row: &Map<String, Json>) -> Result<Map<String, Json>> {
    let mut row = row.clone();
    let mut rules = row.remove("rules").unwrap_or(Json::Null);
    if rules.is_null()
        && let Some(encoded) = row.get("chain").and_then(Json::as_str)
            .and_then(|chain| chain.strip_prefix(FILTER_RULES_PREFIX))
    {
        rules = Json::from(String::from_utf8(hex::decode(encoded)?)?);
        row.insert("chain".into(), Json::from("lists!"));
    }
    if !rules.is_null() {
        let json = rules.as_str().context("filter rules must be a string")?;
        crate::filter::FilterRules::from_storage(json)?;
    }
    row.insert("rules".into(), rules);
    Ok(row)
}

#[cfg(test)]
mod tests;
