//! Local SQLite storage for plugin state, KV, entity metadata, user scripts, and event outbox.

use anyhow::Result;
use rusqlite::{OptionalExtension as _, params};

use crate::store::{Store, now_ms};

/// Per-plugin KV storage quota (5 MiB across keys and values).
pub const PLUGIN_KV_QUOTA_BYTES: usize = 5 * 1024 * 1024;
/// Per-entry value cap for `plugin_entity_meta` (64 KiB).
pub const PLUGIN_META_ENTRY_CAP_BYTES: usize = 64 * 1024;
/// Default tail size retained when trimming `plugin_outbox`.
pub const DEFAULT_OUTBOX_RETENTION: usize = 500;
/// Maximum causal plugin event propagation depth before stopping loop delivery.
pub const MAX_EVENT_DEPTH: u32 = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginState {
    pub plugin_id: String,
    pub enabled: bool,
    pub granted_permissions: String,
    pub manifest_hash: String,
    pub settings_json: String,
    pub consecutive_errors: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserScript {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger_kind: String,
    pub trigger_arg: String,
    pub permissions: String,
    pub code: String,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginOutboxEvent {
    pub seq: i64,
    pub event_kind: String,
    /// For tasks and focus sessions this is the entity UUID; for `habit.checkin` it is
    /// `<habit_id>\x1f<YYYY-MM-DD>`.
    pub entity_id: String,
    /// `None` for user edits, `Some("__sync__")` for remote sync, or `Some(plugin_id)`.
    pub origin_plugin: Option<String>,
    pub depth: u32,
    pub at: i64,
}

fn read_plugin_state(row: &rusqlite::Row) -> rusqlite::Result<PluginState> {
    Ok(PluginState {
        plugin_id: row.get(0)?,
        enabled: row.get::<_, i64>(1)? != 0,
        granted_permissions: row.get(2)?,
        manifest_hash: row.get(3)?,
        settings_json: row.get(4)?,
        consecutive_errors: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn read_user_script(row: &rusqlite::Row) -> rusqlite::Result<UserScript> {
    Ok(UserScript {
        id: row.get(0)?,
        name: row.get(1)?,
        enabled: row.get::<_, i64>(2)? != 0,
        trigger_kind: row.get(3)?,
        trigger_arg: row.get(4)?,
        permissions: row.get(5)?,
        code: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

impl Store {
    pub fn plugin_state(&self, plugin_id: &str) -> Result<Option<PluginState>> {
        self.conn
            .prepare_cached(
                "SELECT plugin_id, enabled, granted_permissions, manifest_hash, settings_json,
                        consecutive_errors, updated_at
                 FROM plugin_state WHERE plugin_id = ?1",
            )?
            .query_row([plugin_id], read_plugin_state)
            .optional()
            .map_err(Into::into)
    }

    pub fn all_plugin_states(&self) -> Result<Vec<PluginState>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT plugin_id, enabled, granted_permissions, manifest_hash, settings_json,
                    consecutive_errors, updated_at
             FROM plugin_state ORDER BY plugin_id",
        )?;
        let rows = stmt.query_map([], read_plugin_state)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn save_plugin_state(&self, state: &PluginState) -> Result<()> {
        anyhow::ensure!(!state.plugin_id.is_empty(), "plugin_id must not be empty");
        let updated_at = if state.updated_at > 0 { state.updated_at } else { now_ms() };
        self.conn.execute(
            "INSERT INTO plugin_state (
                 plugin_id, enabled, granted_permissions, manifest_hash, settings_json,
                 consecutive_errors, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(plugin_id) DO UPDATE SET
                 enabled = excluded.enabled,
                 granted_permissions = excluded.granted_permissions,
                 manifest_hash = excluded.manifest_hash,
                 settings_json = excluded.settings_json,
                 consecutive_errors = excluded.consecutive_errors,
                 updated_at = excluded.updated_at",
            params![
                state.plugin_id,
                i64::from(state.enabled),
                state.granted_permissions,
                state.manifest_hash,
                state.settings_json,
                state.consecutive_errors,
                updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn set_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> Result<()> {
        anyhow::ensure!(!plugin_id.is_empty(), "plugin_id must not be empty");
        self.conn.execute(
            "INSERT INTO plugin_state (plugin_id, enabled, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(plugin_id) DO UPDATE SET enabled = excluded.enabled, updated_at = excluded.updated_at",
            params![plugin_id, i64::from(enabled), now_ms()],
        )?;
        Ok(())
    }

    pub fn set_plugin_settings(&self, plugin_id: &str, settings_json: &str) -> Result<()> {
        anyhow::ensure!(!plugin_id.is_empty(), "plugin_id must not be empty");
        self.conn.execute(
            "INSERT INTO plugin_state (plugin_id, settings_json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(plugin_id) DO UPDATE SET settings_json = excluded.settings_json, updated_at = excluded.updated_at",
            params![plugin_id, settings_json, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_plugin_permissions(
        &self,
        plugin_id: &str,
        granted_permissions: &str,
        manifest_hash: &str,
    ) -> Result<()> {
        anyhow::ensure!(!plugin_id.is_empty(), "plugin_id must not be empty");
        self.conn.execute(
            "INSERT INTO plugin_state (plugin_id, granted_permissions, manifest_hash, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(plugin_id) DO UPDATE SET
                 granted_permissions = excluded.granted_permissions,
                 manifest_hash = excluded.manifest_hash,
                 updated_at = excluded.updated_at",
            params![plugin_id, granted_permissions, manifest_hash, now_ms()],
        )?;
        Ok(())
    }

    pub fn increment_plugin_errors(&self, plugin_id: &str) -> Result<i64> {
        anyhow::ensure!(!plugin_id.is_empty(), "plugin_id must not be empty");
        let count = self.conn.query_row(
            "INSERT INTO plugin_state (plugin_id, consecutive_errors, updated_at) VALUES (?1, 1, ?2)
             ON CONFLICT(plugin_id) DO UPDATE SET
                 consecutive_errors = plugin_state.consecutive_errors + 1,
                 updated_at = excluded.updated_at
             RETURNING consecutive_errors",
            params![plugin_id, now_ms()],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn reset_plugin_errors(&self, plugin_id: &str) -> Result<()> {
        anyhow::ensure!(!plugin_id.is_empty(), "plugin_id must not be empty");
        self.conn.execute(
            "INSERT INTO plugin_state (plugin_id, consecutive_errors, updated_at) VALUES (?1, 0, ?2)
             ON CONFLICT(plugin_id) DO UPDATE SET consecutive_errors = 0, updated_at = excluded.updated_at",
            params![plugin_id, now_ms()],
        )?;
        Ok(())
    }

    pub fn delete_plugin_state(&self, plugin_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM plugin_state WHERE plugin_id = ?1", [plugin_id])?;
        Ok(())
    }

    pub fn plugin_kv_get(&self, plugin_id: &str, key: &str) -> Result<Option<String>> {
        self.conn
            .prepare_cached("SELECT value FROM plugin_kv WHERE plugin_id = ?1 AND key = ?2")?
            .query_row(params![plugin_id, key], |row| row.get(0))
            .optional()
            .map_err(Into::into)
    }

    pub fn plugin_kv_usage_bytes(&self, plugin_id: &str) -> Result<usize> {
        let used: i64 = self.conn.prepare_cached(
            "SELECT COALESCE(SUM(LENGTH(CAST(key AS BLOB)) + LENGTH(CAST(value AS BLOB))), 0)
             FROM plugin_kv WHERE plugin_id = ?1",
        )?
        .query_row([plugin_id], |row| row.get(0))?;
        Ok(usize::try_from(used).unwrap_or(0))
    }

    pub fn plugin_kv_set(&self, plugin_id: &str, key: &str, value: &str) -> Result<()> {
        anyhow::ensure!(!plugin_id.is_empty(), "plugin_id must not be empty");
        self.in_tx(|| {
            let other_bytes: i64 = self.conn.prepare_cached(
                "SELECT COALESCE(SUM(LENGTH(CAST(key AS BLOB)) + LENGTH(CAST(value AS BLOB))), 0)
                 FROM plugin_kv WHERE plugin_id = ?1 AND key != ?2",
            )?
            .query_row(params![plugin_id, key], |row| row.get(0))?;
            let next_total = usize::try_from(other_bytes)
                .unwrap_or(usize::MAX)
                .saturating_add(key.len())
                .saturating_add(value.len());
            anyhow::ensure!(
                next_total <= PLUGIN_KV_QUOTA_BYTES,
                "plugin_kv quota exceeded for {plugin_id} ({next_total} > {PLUGIN_KV_QUOTA_BYTES} bytes)"
            );
            self.conn.execute(
                "INSERT INTO plugin_kv (plugin_id, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(plugin_id, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![plugin_id, key, value, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn plugin_kv_delete(&self, plugin_id: &str, key: &str) -> Result<bool> {
        let deleted = self
            .conn
            .execute("DELETE FROM plugin_kv WHERE plugin_id = ?1 AND key = ?2", params![plugin_id, key])?;
        Ok(deleted > 0)
    }

    pub fn plugin_kv_keys(&self, plugin_id: &str, prefix: Option<&str>) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT key FROM plugin_kv WHERE plugin_id = ?1 ORDER BY key")?;
        let keys = stmt
            .query_map([plugin_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(match prefix {
            Some(prefix) if !prefix.is_empty() => {
                keys.into_iter().filter(|k| k.starts_with(prefix)).collect()
            }
            _ => keys,
        })
    }

    pub fn plugin_kv_clear(&self, plugin_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM plugin_kv WHERE plugin_id = ?1", [plugin_id])?;
        Ok(())
    }

    pub fn plugin_meta_get(
        &self,
        plugin_id: &str,
        entity_type: &str,
        entity_id: &str,
        key: &str,
    ) -> Result<Option<String>> {
        self.conn
            .prepare_cached(
                "SELECT value FROM plugin_entity_meta
                 WHERE plugin_id = ?1 AND entity_type = ?2 AND entity_id = ?3 AND key = ?4",
            )?
            .query_row(params![plugin_id, entity_type, entity_id, key], |row| row.get(0))
            .optional()
            .map_err(Into::into)
    }

    pub fn plugin_meta_set(
        &self,
        plugin_id: &str,
        entity_type: &str,
        entity_id: &str,
        key: &str,
        value: &str,
    ) -> Result<()> {
        anyhow::ensure!(!plugin_id.is_empty(), "plugin_id must not be empty");
        anyhow::ensure!(!entity_type.is_empty(), "entity_type must not be empty");
        anyhow::ensure!(!entity_id.is_empty(), "entity_id must not be empty");
        anyhow::ensure!(
            value.len() <= PLUGIN_META_ENTRY_CAP_BYTES,
            "plugin_entity_meta entry exceeds {PLUGIN_META_ENTRY_CAP_BYTES} bytes"
        );
        self.conn.execute(
            "INSERT INTO plugin_entity_meta (plugin_id, entity_type, entity_id, key, value, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(plugin_id, entity_type, entity_id, key) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at",
            params![plugin_id, entity_type, entity_id, key, value, now_ms()],
        )?;
        Ok(())
    }

    pub fn plugin_meta_list(
        &self,
        plugin_id: &str,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT key, value FROM plugin_entity_meta
             WHERE plugin_id = ?1 AND entity_type = ?2 AND entity_id = ?3
             ORDER BY key",
        )?;
        let rows = stmt
            .query_map(params![plugin_id, entity_type, entity_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn plugin_meta_for_entity(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT plugin_id, key, value FROM plugin_entity_meta
             WHERE entity_type = ?1 AND entity_id = ?2
             ORDER BY plugin_id, key",
        )?;
        let rows = stmt
            .query_map(params![entity_type, entity_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn plugin_meta_delete(
        &self,
        plugin_id: &str,
        entity_type: &str,
        entity_id: &str,
        key: &str,
    ) -> Result<bool> {
        let deleted = self.conn.execute(
            "DELETE FROM plugin_entity_meta
             WHERE plugin_id = ?1 AND entity_type = ?2 AND entity_id = ?3 AND key = ?4",
            params![plugin_id, entity_type, entity_id, key],
        )?;
        Ok(deleted > 0)
    }

    pub fn plugin_meta_clear(&self, plugin_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM plugin_entity_meta WHERE plugin_id = ?1", [plugin_id])?;
        Ok(())
    }

    pub fn user_scripts(&self) -> Result<Vec<UserScript>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, enabled, trigger_kind, trigger_arg, permissions, code, updated_at
             FROM user_scripts ORDER BY updated_at DESC, name",
        )?;
        let rows = stmt.query_map([], read_user_script)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn user_script(&self, id: &str) -> Result<Option<UserScript>> {
        self.conn
            .prepare_cached(
                "SELECT id, name, enabled, trigger_kind, trigger_arg, permissions, code, updated_at
                 FROM user_scripts WHERE id = ?1",
            )?
            .query_row([id], read_user_script)
            .optional()
            .map_err(Into::into)
    }

    pub fn save_user_script(&self, script: &UserScript) -> Result<()> {
        anyhow::ensure!(!script.id.trim().is_empty(), "user script id must not be empty");
        anyhow::ensure!(!script.name.trim().is_empty(), "user script name must not be empty");
        let updated_at = if script.updated_at > 0 { script.updated_at } else { now_ms() };
        self.conn.execute(
            "INSERT INTO user_scripts (
                 id, name, enabled, trigger_kind, trigger_arg, permissions, code, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 enabled = excluded.enabled,
                 trigger_kind = excluded.trigger_kind,
                 trigger_arg = excluded.trigger_arg,
                 permissions = excluded.permissions,
                 code = excluded.code,
                 updated_at = excluded.updated_at",
            params![
                script.id.trim(),
                script.name.trim(),
                i64::from(script.enabled),
                script.trigger_kind,
                script.trigger_arg,
                script.permissions,
                script.code,
                updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn set_user_script_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE user_scripts SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, i64::from(enabled), now_ms()],
        )?;
        Ok(())
    }

    pub fn delete_user_script(&self, id: &str) -> Result<bool> {
        let deleted = self.conn.execute("DELETE FROM user_scripts WHERE id = ?1", [id])?;
        Ok(deleted > 0)
    }

    /// Reads up to `limit` oldest pending events from `plugin_outbox` without removing them;
    /// call [`Self::ack_plugin_outbox`] once processed.
    pub fn drain_plugin_outbox(&self, limit: usize) -> Result<Vec<PluginOutboxEvent>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, event_kind, entity_id, origin_plugin, depth, at
             FROM plugin_outbox ORDER BY seq ASC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                let depth_i64: i64 = row.get(4)?;
                Ok(PluginOutboxEvent {
                    seq: row.get(0)?,
                    event_kind: row.get(1)?,
                    entity_id: row.get(2)?,
                    origin_plugin: row.get(3)?,
                    depth: u32::try_from(depth_i64.max(0)).unwrap_or(u32::MAX),
                    at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Deletes processed events up to `up_to_seq` inclusive, returning the deleted row count.
    pub fn ack_plugin_outbox(&self, up_to_seq: i64) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM plugin_outbox WHERE seq <= ?1", [up_to_seq])?)
    }

    /// Retains at most `keep_last` newest rows in `plugin_outbox` (`0` clears the entire outbox).
    pub fn trim_plugin_outbox(&self, keep_last: usize) -> Result<usize> {
        if keep_last == 0 {
            return Ok(self.conn.execute("DELETE FROM plugin_outbox", [])?);
        }
        Ok(self.conn.execute(
            "DELETE FROM plugin_outbox
             WHERE seq NOT IN (SELECT seq FROM plugin_outbox ORDER BY seq DESC LIMIT ?1)",
            [keep_last as i64],
        )?)
    }

    /// Runs `f` inside a transaction while `plugin_write_ctx` tags all triggered outbox events
    /// with `(plugin_id, depth)`. Always restores the previous context on completion, error, or panic.
    pub fn with_plugin_write_ctx<T>(
        &self,
        plugin_id: &str,
        depth: u32,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        self.in_tx(|| {
            let prev: Option<(String, i64)> = self
                .conn
                .query_row("SELECT plugin_id, depth FROM plugin_write_ctx LIMIT 1", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()?;
            struct CtxGuard<'a> {
                conn: &'a rusqlite::Connection,
                prev: Option<(String, i64)>,
            }
            impl Drop for CtxGuard<'_> {
                fn drop(&mut self) {
                    let _ = self.conn.execute("DELETE FROM plugin_write_ctx", []);
                    if let Some((pid, d)) = &self.prev {
                        let _ = self.conn.execute(
                            "INSERT INTO plugin_write_ctx (plugin_id, depth) VALUES (?1, ?2)",
                            params![pid, d],
                        );
                    }
                }
            }
            self.conn.execute("DELETE FROM plugin_write_ctx", [])?;
            self.conn.execute(
                "INSERT INTO plugin_write_ctx (plugin_id, depth) VALUES (?1, ?2)",
                params![plugin_id, i64::from(depth)],
            )?;
            let _guard = CtxGuard { conn: &self.conn, prev };
            f()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{Change, SYNC_PROTOCOL_VERSION};

    #[test]
    fn plugin_state_kv_meta_and_scripts_crud_and_quotas() -> Result<()> {
        let store = Store::open_in_memory()?;

        // State & error circuit counter.
        assert!(store.plugin_state("com.example.test")?.is_none());
        store.set_plugin_enabled("com.example.test", true)?;
        store.set_plugin_permissions("com.example.test", r#"["tasks:read"]"#, "hash1")?;
        store.set_plugin_settings("com.example.test", r#"{"k":1}"#)?;
        assert_eq!(store.increment_plugin_errors("com.example.test")?, 1);
        assert_eq!(store.increment_plugin_errors("com.example.test")?, 2);
        let st = store.plugin_state("com.example.test")?.unwrap();
        assert!(st.enabled);
        assert_eq!(st.consecutive_errors, 2);
        assert_eq!(st.manifest_hash, "hash1");
        store.reset_plugin_errors("com.example.test")?;
        assert_eq!(store.plugin_state("com.example.test")?.unwrap().consecutive_errors, 0);

        // KV quota (5 MB) and isolation between plugins.
        store.plugin_kv_set("p1", "alpha", "1")?;
        store.plugin_kv_set("p1", "alpha_two", "2")?;
        store.plugin_kv_set("p2", "alpha", "other")?;
        assert_eq!(store.plugin_kv_get("p1", "alpha")?.as_deref(), Some("1"));
        assert_eq!(store.plugin_kv_get("p2", "alpha")?.as_deref(), Some("other"));
        assert_eq!(store.plugin_kv_keys("p1", Some("alpha_"))?, vec!["alpha_two"]);
        let oversized = "x".repeat(PLUGIN_KV_QUOTA_BYTES + 1);
        assert!(store.plugin_kv_set("p1", "big", &oversized).is_err());
        // Updating existing key within quota succeeds even near cap.
        let near_cap = "y".repeat(PLUGIN_KV_QUOTA_BYTES - 100);
        store.plugin_kv_set("p1", "chunk", &near_cap)?;
        store.plugin_kv_set("p1", "chunk", "small")?;
        assert!(store.plugin_kv_delete("p1", "chunk")?);

        // Entity meta 64 KB cap.
        store.plugin_meta_set("p1", "task", "t1", "gh_id", "42")?;
        assert_eq!(store.plugin_meta_get("p1", "task", "t1", "gh_id")?.as_deref(), Some("42"));
        assert_eq!(store.plugin_meta_list("p1", "task", "t1")?.len(), 1);
        assert!(
            store
                .plugin_meta_set("p1", "task", "t1", "huge", &"z".repeat(PLUGIN_META_ENTRY_CAP_BYTES + 1))
                .is_err()
        );

        // User scripts.
        let script = UserScript {
            id: "s1".into(),
            name: "Auto tag".into(),
            enabled: true,
            trigger_kind: "event".into(),
            trigger_arg: "task.created".into(),
            permissions: r#"["tasks:read","tasks:write"]"#.into(),
            code: "export default () => {}".into(),
            updated_at: 100,
        };
        store.save_user_script(&script)?;
        assert_eq!(store.user_scripts()?, vec![script]);
        store.set_user_script_enabled("s1", false)?;
        assert!(!store.user_script("s1")?.unwrap().enabled);
        assert!(store.delete_user_script("s1")?);
        Ok(())
    }

    #[test]
    fn plugin_outbox_tracks_origin_depth_sync_and_trim() -> Result<()> {
        let store = Store::open_in_memory()?;

        // Direct user task lifecycle emits NULL origin at depth 0.
        let task = store.add_task("User task")?;
        store.set_title(task.id, "Renamed user task")?;
        store.set_completed(task.id, true)?;
        store.set_completed(task.id, false)?;
        store.delete_task(task.id)?;

        let events = store.drain_plugin_outbox(50)?;
        let kinds: Vec<&str> = events.iter().map(|e| e.event_kind.as_str()).collect();
        assert_eq!(kinds, ["task.created", "task.updated", "task.completed", "task.reopened", "task.deleted"]);
        assert!(events.iter().all(|e| e.origin_plugin.is_none() && e.depth == 0));
        store.ack_plugin_outbox(events.last().unwrap().seq)?;
        assert!(store.drain_plugin_outbox(50)?.is_empty());

        // Plugin write context tags origin_plugin and depth, and cleans up on error too.
        let plugin_task = store.with_plugin_write_ctx("com.example.sync", 2, || store.add_task("Plugin task"))?;
        let _: Result<()> = store.with_plugin_write_ctx("com.example.failing", 3, || {
            let _ = store.add_task("Rolled back")?;
            anyhow::bail!("boom")
        });
        // Subsequent user write must not inherit stale plugin_write_ctx.
        let after = store.add_task("After error")?;

        let events = store.drain_plugin_outbox(50)?;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].entity_id, plugin_task.id.to_string());
        assert_eq!(events[0].origin_plugin.as_deref(), Some("com.example.sync"));
        assert_eq!(events[0].depth, 2);
        assert_eq!(events[1].entity_id, after.id.to_string());
        assert_eq!(events[1].origin_plugin, None);
        assert_eq!(events[1].depth, 0);
        store.ack_plugin_outbox(events.last().unwrap().seq)?;

        // Remote sync writes are tagged '__sync__'.
        let remote_id = uuid::Uuid::now_v7().to_string();
        let req = store.sync_request()?;
        let answer = serde_json::json!({
            "protocol": SYNC_PROTOCOL_VERSION,
            "position": 1,
            "changes": [
                Change {
                    entity: "tasks".into(),
                    key: remote_id.clone(),
                    at: 1000,
                    row: Some(serde_json::Map::from_iter([
                        ("id".into(), serde_json::json!(remote_id)),
                        ("title".into(), serde_json::json!("From sync")),
                        ("created_at".into(), serde_json::json!(1000)),
                    ])),
                }.to_json()
            ]
        });
        store.finish_sync(&req, &answer)?;
        let sync_events = store.drain_plugin_outbox(50)?;
        assert_eq!(sync_events.len(), 1);
        assert_eq!(sync_events[0].origin_plugin.as_deref(), Some("__sync__"));
        store.ack_plugin_outbox(sync_events[0].seq)?;

        // Habits and focus sessions emit outbox events, and trim keeps newest rows.
        let habit = store.add_habit("Run")?;
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        store.add_checkin_amount(habit.id, day, 1.0)?;
        store.add_focus_session(Some(plugin_task.id), "Focus", 1000, 2000)?;
        let hf_events = store.drain_plugin_outbox(50)?;
        assert_eq!(hf_events.len(), 2);
        assert_eq!(hf_events[0].event_kind, "habit.checkin");
        assert_eq!(hf_events[0].entity_id, format!("{}\x1f2026-11-22", habit.id));
        assert_eq!(hf_events[1].event_kind, "focus.completed");

        store.trim_plugin_outbox(1)?;
        let trimmed = store.drain_plugin_outbox(50)?;
        assert_eq!(trimmed.len(), 1);
        assert_eq!(trimmed[0].event_kind, "focus.completed");
        Ok(())
    }
}
