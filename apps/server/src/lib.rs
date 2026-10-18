//! Fern's sync server: it keeps the latest change of every synced row and hands each device the
//! changes it has not seen. It does not look into the rows; devices decide what they mean (see
//! `fern_core::sync`).
//!
//! One request does a whole sync, `POST /sync` with a bearer token:
//!
//! ```json
//! { "device": "<id>", "since": 42, "changes": [{ "entity": "tasks", "key": "…", "at": 1760000000000, "row": {…} }] }
//! ```
//!
//! The answer has the changes other devices made after `since`, and the position to send next
//! time: `{ "changes": […], "position": 57 }`.

use anyhow::{Context as _, Result, bail};
use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::{Value, json};

pub struct Server {
    conn: Connection,
}

impl Server {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS changes (
                 seq    INTEGER PRIMARY KEY AUTOINCREMENT,
                 entity TEXT NOT NULL,
                 key    TEXT NOT NULL,
                 at     INTEGER NOT NULL,
                 row    TEXT,
                 device TEXT NOT NULL,
                 UNIQUE (entity, key)
             );",
        )?;
        Ok(Self { conn })
    }

    /// One sync: stores the device's changes (the later change of a row wins) and returns the
    /// other devices' changes after `since`.
    pub fn sync(&mut self, request: &Value) -> Result<Value> {
        let device = request["device"].as_str().filter(|d| !d.is_empty()).context("a sync needs a device id")?;
        let since = request["since"].as_i64().unwrap_or(0);
        let incoming = match &request["changes"] {
            Value::Null => &Vec::new(),
            Value::Array(changes) => changes,
            _ => bail!("changes must be a list"),
        };
        let tx = self.conn.transaction()?;
        for change in incoming {
            let entity = change["entity"].as_str().context("a change needs an entity")?;
            let key = change["key"].as_str().context("a change needs a key")?;
            let at = change["at"].as_i64().context("a change needs a time")?;
            let row = match &change["row"] {
                Value::Null => None,
                row @ Value::Object(_) => Some(row.to_string()),
                _ => bail!("a change's row must be an object or null"),
            };
            let stored: Option<i64> = tx
                .query_row("SELECT at FROM changes WHERE entity = ?1 AND key = ?2", params![entity, key], |r| r.get(0))
                .optional()?;
            if stored.is_some_and(|stored| stored > at) {
                continue;
            }
            // A new sequence number, so that devices that saw the old change get this one.
            tx.execute("DELETE FROM changes WHERE entity = ?1 AND key = ?2", params![entity, key])?;
            tx.execute(
                "INSERT INTO changes (entity, key, at, row, device) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![entity, key, at, row, device],
            )?;
        }
        tx.commit()?;

        let position: i64 = self.conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM changes", [], |r| r.get(0))?;
        let mut stmt = self
            .conn
            .prepare("SELECT entity, key, at, row FROM changes WHERE seq > ?1 AND device != ?2 ORDER BY seq")?;
        let outgoing = stmt
            .query_map(params![since, device], |r| {
                let row: Option<String> = r.get(3)?;
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, row))
            })?
            .map(|item| {
                let (entity, key, at, row) = item?;
                let row = match row {
                    Some(text) => serde_json::from_str(&text)?,
                    None => Value::Null,
                };
                Ok(json!({ "entity": entity, "key": key, "at": at, "row": row }))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({ "changes": outgoing, "position": position }))
    }
}

/// Compares a request's `Authorization` header with the token, in constant time.
pub fn authorized(header: Option<&str>, token: &str) -> bool {
    let Some(given) = header.and_then(|h| h.strip_prefix("Bearer ")) else { return false };
    given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[cfg(test)]
mod tests {
    use fern_core::Store;
    use fern_core::sync::Change;

    use super::*;

    /// A device's sync through the server, as the app will do it.
    fn sync(server: &mut Server, store: &Store, device: &str, sent: &mut i64, seen: &mut i64) {
        let (changes, position) = store.changes_since(*sent).unwrap();
        let request = json!({
            "device": device,
            "since": *seen,
            "changes": changes.iter().map(Change::to_json).collect::<Vec<_>>(),
        });
        let answer = server.sync(&request).unwrap();
        let received: Vec<Change> =
            answer["changes"].as_array().unwrap().iter().map(|c| Change::from_json(c).unwrap()).collect();
        store.apply_changes(&received).unwrap();
        *sent = position;
        *seen = answer["position"].as_i64().unwrap();
    }

    #[test]
    fn devices_sync_through_the_server() {
        let mut server = Server::open_in_memory().unwrap();
        let laptop = Store::open_in_memory().unwrap();
        let phone = Store::open_in_memory().unwrap();
        let (mut laptop_sent, mut laptop_seen, mut phone_sent, mut phone_seen) = (0, 0, 0, 0);

        let task = laptop.add_task("Call the bank").unwrap();
        sync(&mut server, &laptop, "laptop", &mut laptop_sent, &mut laptop_seen);
        sync(&mut server, &phone, "phone", &mut phone_sent, &mut phone_seen);
        assert_eq!(phone.task(task.id).unwrap().unwrap().title, "Call the bank");

        phone.set_completed(task.id, true).unwrap();
        sync(&mut server, &phone, "phone", &mut phone_sent, &mut phone_seen);
        sync(&mut server, &laptop, "laptop", &mut laptop_sent, &mut laptop_seen);
        assert!(laptop.task(task.id).unwrap().unwrap().is_completed());

        // Nothing new: nothing comes back, and a device never gets its own changes.
        let answer = server.sync(&json!({ "device": "laptop", "since": laptop_seen })).unwrap();
        assert!(answer["changes"].as_array().unwrap().is_empty());
    }

    #[test]
    fn an_older_change_does_not_replace_a_newer_one() {
        let mut server = Server::open_in_memory().unwrap();
        let change = |at: i64, title: &str| json!({ "entity": "tasks", "key": "t1", "at": at, "row": { "id": "t1", "title": title } });
        server.sync(&json!({ "device": "a", "changes": [change(20, "new")] })).unwrap();
        server.sync(&json!({ "device": "b", "changes": [change(10, "old")] })).unwrap();
        let answer = server.sync(&json!({ "device": "c", "since": 0 })).unwrap();
        assert_eq!(answer["changes"][0]["row"]["title"], "new");
        assert!(server.sync(&json!({ "since": 0 })).is_err(), "a device id is required");
    }

    #[test]
    fn tokens() {
        assert!(authorized(Some("Bearer s3cret"), "s3cret"));
        assert!(!authorized(Some("Bearer s3cre"), "s3cret"));
        assert!(!authorized(Some("s3cret"), "s3cret"));
        assert!(!authorized(None, "s3cret"));
    }
}
