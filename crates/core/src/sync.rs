//! Sync between devices, the store's side: what changed here since a point, and applying what
//! changed elsewhere.
//!
//! Every write to synced data is logged by triggers (`sync_log`: table, row key, time). A
//! device sends the rows changed since its last send, as they are now — or as deleted — and
//! applies the changes it receives row by row: the later change of a row wins. Changes applied
//! that way are not logged again, so they do not bounce back. A server only has to keep the
//! latest change of every row and hand out the ones a device has not seen.

use anyhow::{Context as _, Result, bail};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{OptionalExtension as _, params_from_iter};
use serde_json::{Map, Value as Json};

use crate::store::Store;

/// The synced tables, parents before children, with their key columns.
pub const SYNCED: &[(&str, &[&str])] = &[
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
];

/// Key parts are joined with this character, as the triggers do.
const KEY_SEPARATOR: char = '\u{1f}';

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

/// Where this device syncs to: the server's address and the token.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncConfig {
    pub server: String,
    pub token: String,
}

const SYNC_SERVER: &str = "sync_server";
const SYNC_TOKEN: &str = "sync_token";
const SYNC_DEVICE: &str = "sync_device";
const SYNC_SENT: &str = "sync_sent";
const SYNC_SEEN: &str = "sync_seen";

/// A sync on its way: the request to send, and what it covers of the local log.
pub struct SyncRequest {
    pub body: Json,
    sent: i64,
}

impl Store {
    /// The sync settings, if a server is set.
    pub fn sync_config(&self) -> Result<Option<SyncConfig>> {
        let server = self.setting(SYNC_SERVER)?.unwrap_or_default();
        let token = self.setting(SYNC_TOKEN)?.unwrap_or_default();
        Ok((!server.trim().is_empty()).then_some(SyncConfig { server, token }))
    }

    /// Sets the server and token. A different server starts over: everything here is sent to it,
    /// and everything it has is received.
    pub fn set_sync_config(&self, config: &SyncConfig) -> Result<()> {
        let server = config.server.trim();
        if self.setting(SYNC_SERVER)?.unwrap_or_default() != server {
            self.set_setting(SYNC_SENT, "0")?;
            self.set_setting(SYNC_SEEN, "0")?;
        }
        self.set_setting(SYNC_SERVER, server)?;
        self.set_setting(SYNC_TOKEN, config.token.trim())
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

    /// The next sync's request: this device's unsent changes and what it has seen.
    pub fn sync_request(&self) -> Result<SyncRequest> {
        let (changes, sent) = self.changes_since(self.sync_number(SYNC_SENT)?)?;
        let body = serde_json::json!({
            "device": self.sync_device()?,
            "since": self.sync_number(SYNC_SEEN)?,
            "changes": changes.iter().map(Change::to_json).collect::<Vec<_>>(),
        });
        Ok(SyncRequest { body, sent })
    }

    /// Applies the server's answer to `request` and remembers how far both sides got. Returns
    /// how many changes came in.
    pub fn finish_sync(&self, request: &SyncRequest, answer: &Json) -> Result<usize> {
        let received = answer["changes"]
            .as_array()
            .context("the server's answer has no changes")?
            .iter()
            .map(Change::from_json)
            .collect::<Result<Vec<_>>>()?;
        let position = answer["position"].as_i64().context("the server's answer has no position")?;
        let applied = self.apply_changes(&received)?;
        self.set_setting(SYNC_SENT, &request.sent.to_string())?;
        self.set_setting(SYNC_SEEN, &position.to_string())?;
        self.compact_sync_log()?;
        Ok(applied)
    }
}

impl Store {
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
        let mut stmt = self.conn.prepare(
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
        let parts: Vec<&str> = key.split(KEY_SEPARATOR).collect();
        let condition: Vec<String> = keys.iter().enumerate().map(|(i, k)| format!("{k} = ?{}", i + 1)).collect();
        let mut stmt = self.conn.prepare(&format!("SELECT * FROM {entity} WHERE {}", condition.join(" AND ")))?;
        let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
        Ok(stmt
            .query_row(params_from_iter(parts), |row| {
                let mut map = Map::new();
                for (i, name) in names.iter().enumerate() {
                    map.insert(name.clone(), to_json(row.get_ref(i)?));
                }
                Ok(map)
            })
            .optional()?)
    }

    /// Applies changes from another device. A row changed here later than the incoming change
    /// keeps its local state. Applied changes are not logged, so they are not sent back.
    pub fn apply_changes(&self, changes: &[Change]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        // Rows may arrive before the rows they point to.
        tx.execute_batch("PRAGMA defer_foreign_keys = ON; INSERT INTO sync_applying VALUES (1);")?;
        let mut ordered: Vec<&Change> = changes.iter().collect();
        let rank = |entity: &str| SYNCED.iter().position(|(name, _)| *name == entity).unwrap_or(usize::MAX);
        ordered.sort_by_key(|change| (change.row.is_none(), rank(&change.entity)));
        let mut applied = 0;
        for change in ordered {
            let keys = keys_of(&change.entity)?;
            let local: Option<i64> = tx
                .query_row(
                    "SELECT MAX(at) FROM sync_log WHERE entity = ?1 AND key = ?2",
                    [&change.entity, &change.key],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if local.is_some_and(|at| at > change.at) {
                continue;
            }
            let parts: Vec<&str> = change.key.split(KEY_SEPARATOR).collect();
            match &change.row {
                Some(row) => {
                    let columns: Vec<&String> = row.keys().filter(|c| is_identifier(c)).collect();
                    let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
                    let updates: Vec<String> = columns
                        .iter()
                        .filter(|c| !keys.contains(&c.as_str()))
                        .map(|c| format!("{c} = excluded.{c}"))
                        .collect();
                    let on_conflict = if updates.is_empty() { "DO NOTHING".to_owned() } else { format!("DO UPDATE SET {}", updates.join(", ")) };
                    let sql = format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT({}) {on_conflict}",
                        change.entity,
                        columns.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", "),
                        placeholders.join(", "),
                        keys.join(", "),
                    );
                    tx.execute(&sql, params_from_iter(columns.iter().map(|c| to_sql(&row[c.as_str()]))))?;
                }
                None => {
                    let condition: Vec<String> = keys.iter().enumerate().map(|(i, k)| format!("{k} = ?{}", i + 1)).collect();
                    tx.execute(&format!("DELETE FROM {} WHERE {}", change.entity, condition.join(" AND ")), params_from_iter(parts))?;
                }
            }
            applied += 1;
        }
        tx.execute_batch("DELETE FROM sync_applying;")?;
        tx.commit()?;
        Ok(applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::View;

    /// Sends everything `from` changed since `since` to `to`, as a server would relay it.
    fn relay(from: &Store, since: i64, to: &Store) -> i64 {
        let (changes, position) = from.changes_since(since).unwrap();
        let sent: Vec<Change> = changes.iter().map(|c| Change::from_json(&c.to_json()).unwrap()).collect();
        to.apply_changes(&sent).unwrap();
        position
    }

    #[test]
    fn two_devices_meet() -> Result<()> {
        let laptop = Store::open_in_memory()?;
        let phone = Store::open_in_memory()?;

        let list = laptop.create_list("Home")?;
        let task = laptop.add_task("Fix the shelf")?;
        laptop.set_list(task.id, Some(list.id))?;
        laptop.set_tags(task.id, &["diy".to_owned()])?;
        laptop.add_checklist_item(task.id, "Buy screws", None)?;
        let sent = relay(&laptop, 0, &phone);

        let on_phone = phone.task(task.id)?.unwrap();
        assert_eq!((on_phone.title.as_str(), on_phone.list_id), ("Fix the shelf", Some(list.id)));
        assert_eq!(on_phone.tags, ["diy"]);
        assert_eq!(phone.checklist(task.id)?.len(), 1);
        // What was applied is not logged on the phone, so it is not sent back.
        assert!(phone.changes_since(0)?.0.is_empty());

        // An edit on the phone reaches the laptop; a deletion too.
        phone.set_title(task.id, "Fix the kitchen shelf")?;
        relay(&phone, 0, &laptop);
        assert_eq!(laptop.task(task.id)?.unwrap().title, "Fix the kitchen shelf");
        laptop.purge_task(task.id)?;
        relay(&laptop, sent, &phone);
        assert!(phone.task(task.id)?.is_none());
        assert!(phone.tasks_in(&View::Inbox, chrono::Local::now().date_naive())?.is_empty());
        Ok(())
    }

    #[test]
    fn the_later_change_wins() -> Result<()> {
        let a = Store::open_in_memory()?;
        let b = Store::open_in_memory()?;
        let task = a.add_task("Draft")?;
        relay(&a, 0, &b);
        let start = b.sync_position()?;

        // Both edit; b's edit is newer, so a's older change does not overwrite it.
        let (mut older, _) = a.changes_since(0)?;
        a.set_title(task.id, "From a")?;
        b.set_title(task.id, "From b")?;
        older = older.into_iter().map(|mut c| { c.at = 1; c }).collect();
        b.apply_changes(&older)?;
        assert_eq!(b.task(task.id)?.unwrap().title, "From b");

        relay(&b, start, &a);
        assert_eq!(a.task(task.id)?.unwrap().title, "From b");
        Ok(())
    }

    #[test]
    fn sync_settings_and_requests() -> Result<()> {
        let store = Store::open_in_memory()?;
        assert_eq!(store.sync_config()?, None);
        let config = SyncConfig { server: " https://fern.example.org ".into(), token: "t0ken".into() };
        store.set_sync_config(&config)?;
        assert_eq!(store.sync_config()?.unwrap().server, "https://fern.example.org");
        let device = store.sync_device()?;
        assert_eq!(store.sync_device()?, device, "the device id stays");

        store.add_task("Water the plants")?;
        assert!(store.has_unsent_changes()?);
        let request = store.sync_request()?;
        assert_eq!(request.body["device"], device.as_str());
        assert_eq!(request.body["changes"].as_array().unwrap().len(), 1);

        // The server answers with nothing new: the change counts as sent.
        store.finish_sync(&request, &serde_json::json!({ "changes": [], "position": 7 }))?;
        assert!(!store.has_unsent_changes()?);
        assert_eq!(store.sync_request()?.body["since"], 7);
        assert!(store.finish_sync(&request, &serde_json::json!({ "oops": true })).is_err());

        // Another server starts over.
        store.set_sync_config(&SyncConfig { server: "https://other.example.org".into(), token: "x".into() })?;
        assert!(store.has_unsent_changes()?);
        Ok(())
    }

    #[test]
    fn the_log_keeps_one_entry_per_row() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("Draft")?;
        for i in 0..20 {
            store.set_title(task.id, &format!("Draft {i}"))?;
        }
        let entries = |store: &Store| -> i64 { store.conn.query_row("SELECT COUNT(*) FROM sync_log", [], |r| r.get(0)).unwrap() };
        assert!(entries(&store) > 20);
        let (before, _) = store.changes_since(0)?;
        store.compact_sync_log()?;
        assert_eq!(entries(&store), 1);
        // What a sync sends is the same.
        let (after, position) = store.changes_since(0)?;
        assert_eq!(after, before);
        assert_eq!(position, store.sync_position()?);
        Ok(())
    }

    #[test]
    fn foreign_input_is_checked() {
        let store = Store::open_in_memory().unwrap();
        let unknown = Change { entity: "settings".into(), key: "theme".into(), at: 1, row: None };
        assert!(store.apply_changes(&[unknown]).is_err(), "only synced tables take changes");
        let mut row = Map::new();
        row.insert("id; DROP TABLE tasks".into(), Json::from("x"));
        row.insert("id".into(), Json::from("00000000-0000-0000-0000-000000000000"));
        row.insert("name".into(), Json::from("Odd"));
        row.insert("created_at".into(), Json::from(0));
        let odd = Change { entity: "lists".into(), key: "00000000-0000-0000-0000-000000000000".into(), at: 1, row: Some(row) };
        store.apply_changes(&[odd]).unwrap();
        assert_eq!(store.lists().unwrap().len(), 1, "odd column names are skipped");
        assert!(store.tasks().is_ok());
    }
}
