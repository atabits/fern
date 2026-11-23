//! Local SQLite storage for plugin state, KV, entity metadata, user scripts, and event outbox.

use anyhow::Result;
use rusqlite::{OptionalExtension as _, params};

use crate::store::{Store, now_ms};

/// Per-plugin KV storage quota (5 MiB across keys and values).
pub const PLUGIN_KV_QUOTA_BYTES: usize = 5 * 1024 * 1024;
/// Per-entry cap for `plugin_entity_meta` (64 KiB across entity_id, key, and value).
pub const PLUGIN_META_ENTRY_CAP_BYTES: usize = 64 * 1024;
/// Per-plugin total storage quota for `plugin_entity_meta` (5 MiB across entity_id, key, and value).
pub const PLUGIN_META_QUOTA_BYTES: usize = 5 * 1024 * 1024;
/// Default tail size retained when trimming `plugin_outbox`.
pub const DEFAULT_OUTBOX_RETENTION: usize = 500;
/// Hard ceiling enforced automatically on undrained `plugin_outbox` growth.
pub const PLUGIN_OUTBOX_MAX_ROWS: usize = 2000;
/// Settings key toggled when Safe Mode disables all plugins.
pub const PLUGIN_SAFE_MODE_SETTING_KEY: &str = "plugins_safe_mode";
/// Settings key toggled to pause recording new rows in `plugin_outbox`.
pub const PLUGIN_OUTBOX_PAUSED_SETTING_KEY: &str = "plugins_outbox_paused";
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

    /// Atomically removes all persisted local data for `plugin_id` (`plugin_state`,
    /// `plugin_kv`, and `plugin_entity_meta`).
    pub fn delete_plugin_all_data(&self, plugin_id: &str) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute("DELETE FROM plugin_state WHERE plugin_id = ?1", [plugin_id])?;
            self.conn.execute("DELETE FROM plugin_kv WHERE plugin_id = ?1", [plugin_id])?;
            self.conn.execute("DELETE FROM plugin_entity_meta WHERE plugin_id = ?1", [plugin_id])?;
            Ok(())
        })
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

    pub fn plugin_meta_usage_bytes(&self, plugin_id: &str) -> Result<usize> {
        let used: i64 = self
            .conn
            .prepare_cached(
                "SELECT COALESCE(SUM(
                     LENGTH(CAST(entity_id AS BLOB))
                     + LENGTH(CAST(key AS BLOB))
                     + LENGTH(CAST(value AS BLOB))
                 ), 0)
                 FROM plugin_entity_meta WHERE plugin_id = ?1",
            )?
            .query_row([plugin_id], |row| row.get(0))?;
        Ok(usize::try_from(used).unwrap_or(0))
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
        anyhow::ensure!(!key.is_empty(), "key must not be empty");
        let entry_bytes = entity_id.len().saturating_add(key.len()).saturating_add(value.len());
        anyhow::ensure!(
            entry_bytes <= PLUGIN_META_ENTRY_CAP_BYTES,
            "plugin_entity_meta entry exceeds {PLUGIN_META_ENTRY_CAP_BYTES} bytes"
        );
        self.in_tx(|| {
            let other_bytes: i64 = self
                .conn
                .prepare_cached(
                    "SELECT COALESCE(SUM(
                         LENGTH(CAST(entity_id AS BLOB))
                         + LENGTH(CAST(key AS BLOB))
                         + LENGTH(CAST(value AS BLOB))
                     ), 0)
                     FROM plugin_entity_meta
                     WHERE plugin_id = ?1
                       AND NOT (entity_type = ?2 AND entity_id = ?3 AND key = ?4)",
                )?
                .query_row(params![plugin_id, entity_type, entity_id, key], |row| row.get(0))?;
            let next_total = usize::try_from(other_bytes)
                .unwrap_or(usize::MAX)
                .saturating_add(entry_bytes);
            anyhow::ensure!(
                next_total <= PLUGIN_META_QUOTA_BYTES,
                "plugin_entity_meta quota exceeded for {plugin_id} ({next_total} > {PLUGIN_META_QUOTA_BYTES} bytes)"
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
        })
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

    /// Enables or pauses recording new rows into `plugin_outbox` (for example during Safe Mode or
    /// bulk operations). Triggers also automatically pause while `plugins_safe_mode = '1'`.
    pub fn set_plugin_outbox_enabled(&self, enabled: bool) -> Result<()> {
        self.set_setting(PLUGIN_OUTBOX_PAUSED_SETTING_KEY, if enabled { "0" } else { "1" })
    }

    /// Returns whether `plugin_outbox` triggers are currently recording events.
    pub fn is_plugin_outbox_enabled(&self) -> Result<bool> {
        let safe_mode = self.setting(PLUGIN_SAFE_MODE_SETTING_KEY)?.as_deref() == Some("1");
        let paused = self.setting(PLUGIN_OUTBOX_PAUSED_SETTING_KEY)?.as_deref() == Some("1");
        Ok(!safe_mode && !paused)
    }

    /// Retains at most `keep_last` newest rows in `plugin_outbox` (`0` clears the entire outbox).
    pub fn trim_plugin_outbox(&self, keep_last: usize) -> Result<usize> {
        if keep_last == 0 {
            return Ok(self.conn.execute("DELETE FROM plugin_outbox", [])?);
        }
        let offset = i64::try_from(keep_last).unwrap_or(i64::MAX);
        Ok(self.conn.execute(
            "DELETE FROM plugin_outbox
             WHERE seq <= (SELECT seq FROM plugin_outbox ORDER BY seq DESC LIMIT 1 OFFSET ?1)",
            [offset],
        )?)
    }

    /// Runs `f` inside a transaction while `plugin_write_ctx` tags all triggered outbox events
    /// with `(plugin_id, depth)`. Explicitly restores the previous context on the `Ok` path (failing
    /// and rolling back the transaction if restoration fails) and best-effort cleans up on `Err`/panic.
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
                armed: bool,
            }
            impl CtxGuard<'_> {
                fn restore_now(&mut self) -> Result<()> {
                    self.armed = false;
                    self.conn.execute("DELETE FROM plugin_write_ctx", [])?;
                    if let Some((pid, d)) = &self.prev {
                        self.conn.execute(
                            "INSERT INTO plugin_write_ctx (plugin_id, depth) VALUES (?1, ?2)",
                            params![pid, d],
                        )?;
                    }
                    Ok(())
                }
            }
            impl Drop for CtxGuard<'_> {
                fn drop(&mut self) {
                    if !self.armed {
                        return;
                    }
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
            let mut guard = CtxGuard { conn: &self.conn, prev, armed: true };
            let result = f()?;
            guard.restore_now()?;
            Ok(result)
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

    #[test]
    fn repeating_completion_emits_task_completed_restore_emits_updated_and_trashed_sync_skipped() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();

        let task = store.add_task("Recurring report")?;
        store.set_due(task.id, Some(today), None)?;
        store.set_repeat(task.id, Some(crate::Recurrence::new(crate::Frequency::Daily, 1)), today)?;
        store.trim_plugin_outbox(0)?;

        let copy = store.complete(task.id, true)?.expect("must create completed copy");
        let events = store.drain_plugin_outbox(50)?;
        assert!(
            events.iter().any(|e| e.event_kind == "task.completed" && e.entity_id == copy.to_string()),
            "completing a repeating task must emit task.completed for the completed snapshot: {events:?}"
        );
        assert!(
            events.iter().any(|e| e.event_kind == "task.updated" && e.entity_id == task.id.to_string()),
            "completing a repeating task must emit task.updated for the rescheduled row: {events:?}"
        );
        assert!(
            !events.iter().any(|e| e.event_kind == "task.created"),
            "completed repeat snapshot must not emit task.created: {events:?}"
        );
        store.trim_plugin_outbox(0)?;

        // Restoring a task from Trash emits task.updated.
        store.delete_task(task.id)?;
        store.trim_plugin_outbox(0)?;
        store.restore_task(task.id)?;
        let restored = store.drain_plugin_outbox(50)?;
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].event_kind, "task.updated");
        assert_eq!(restored[0].entity_id, task.id.to_string());
        store.trim_plugin_outbox(0)?;

        // Syncing an already-trashed task does not emit task.created or task.completed.
        let trashed_id = uuid::Uuid::now_v7().to_string();
        let req = store.sync_request()?;
        let answer = serde_json::json!({
            "protocol": SYNC_PROTOCOL_VERSION,
            "position": 1,
            "changes": [
                Change {
                    entity: "tasks".into(),
                    key: trashed_id.clone(),
                    at: 1000,
                    row: Some(serde_json::Map::from_iter([
                        ("id".into(), serde_json::json!(trashed_id)),
                        ("title".into(), serde_json::json!("Already trashed")),
                        ("created_at".into(), serde_json::json!(1000)),
                        ("deleted_at".into(), serde_json::json!(2000)),
                    ])),
                }.to_json()
            ]
        });
        store.finish_sync(&req, &answer)?;
        assert!(store.drain_plugin_outbox(50)?.is_empty());
        Ok(())
    }

    #[test]
    fn plugin_meta_orphans_cleaned_quotas_enforced_and_delete_all_data_wipes_plugin() -> Result<()> {
        let store = Store::open_in_memory()?;

        // 1. Orphan cleanup across task/subtask purge, empty_trash, list cascade, habit, and countdown.
        let parent = store.add_task("Parent")?;
        let child = store.add_subtask(parent.id, "Child")?;
        let trashed_parent = store.add_task("Trashed Parent")?;
        let trashed_child = store.add_subtask(trashed_parent.id, "Trashed Child")?;
        let list = store.add_list(&crate::ListDraft { name: "Work".into(), ..Default::default() })?;
        let list_task = store.create_task(
            crate::TaskDraft { title: "In list".into(), list_id: Some(list.id), ..Default::default() },
            true,
        )?;
        let list_sub = store.add_subtask(list_task.id, "List sub")?;
        let habit = store.add_habit("Stretch")?;
        let countdown = store.add_countdown(
            "Launch",
            chrono::NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
            crate::CountdownKind::Countdown,
        )?;

        store.plugin_meta_set("p1", "task", &parent.id.to_string(), "k", "1")?;
        store.plugin_meta_set("p1", "task", &child.id.to_string(), "k", "2")?;
        store.plugin_meta_set("p1", "task", &trashed_parent.id.to_string(), "k", "3")?;
        store.plugin_meta_set("p1", "task", &trashed_child.id.to_string(), "k", "4")?;
        store.plugin_meta_set("p1", "list", &list.id.to_string(), "k", "5")?;
        store.plugin_meta_set("p1", "task", &list_task.id.to_string(), "k", "6")?;
        store.plugin_meta_set("p1", "task", &list_sub.id.to_string(), "k", "7")?;
        store.plugin_meta_set("p1", "habit", &habit.id.to_string(), "k", "8")?;
        store.plugin_meta_set("p1", "countdown", &countdown.id.to_string(), "k", "9")?;
        store.plugin_meta_set("p1", "task", "synthetic-unlinked", "k", "keep")?;

        store.purge_task(parent.id)?;
        assert!(store.plugin_meta_get("p1", "task", &parent.id.to_string(), "k")?.is_none());
        assert!(store.plugin_meta_get("p1", "task", &child.id.to_string(), "k")?.is_none());

        store.delete_task(trashed_parent.id)?;
        store.empty_trash()?;
        assert!(store.plugin_meta_get("p1", "task", &trashed_parent.id.to_string(), "k")?.is_none());
        assert!(store.plugin_meta_get("p1", "task", &trashed_child.id.to_string(), "k")?.is_none());

        store.delete_list(list.id)?;
        assert!(store.plugin_meta_get("p1", "list", &list.id.to_string(), "k")?.is_none());
        assert!(store.plugin_meta_get("p1", "task", &list_task.id.to_string(), "k")?.is_none());
        assert!(store.plugin_meta_get("p1", "task", &list_sub.id.to_string(), "k")?.is_none());

        store.delete_habit(habit.id)?;
        assert!(store.plugin_meta_get("p1", "habit", &habit.id.to_string(), "k")?.is_none());

        store.delete_countdown(countdown.id)?;
        assert!(store.plugin_meta_get("p1", "countdown", &countdown.id.to_string(), "k")?.is_none());

        // Unrelated metadata is untouched.
        assert_eq!(store.plugin_meta_get("p1", "task", "synthetic-unlinked", "k")?.as_deref(), Some("keep"));

        // 2. Per-entry key+value cap and per-plugin total quota.
        assert!(store.plugin_meta_set("p1", "task", "e1", "", "v").is_err());
        let big_key = "k".repeat(PLUGIN_META_ENTRY_CAP_BYTES);
        assert!(store.plugin_meta_set("p1", "task", "e1", &big_key, "v").is_err());

        let chunk = "a".repeat(60 * 1024);
        let max_chunks = PLUGIN_META_QUOTA_BYTES / (chunk.len() + 10);
        for i in 0..max_chunks {
            store.plugin_meta_set("p_quota", "task", "e", &format!("k{i:04}"), &chunk)?;
        }
        assert!(store.plugin_meta_usage_bytes("p_quota")? > PLUGIN_META_QUOTA_BYTES - 128 * 1024);
        // Updating an existing entry in-place stays within quota.
        store.plugin_meta_set("p_quota", "task", "e", "k0000", &chunk)?;
        // Exceeding total quota fails for p_quota while p_other stays isolated.
        let mut exceeded = false;
        for extra in 0..5 {
            if store.plugin_meta_set("p_quota", "task", "e", &format!("over{extra}"), &chunk).is_err() {
                exceeded = true;
                break;
            }
        }
        assert!(exceeded, "per-plugin total meta quota must reject writes beyond 5 MiB");
        store.plugin_meta_set("p_other", "task", "e", "k0", &chunk)?;

        // 3. delete_plugin_all_data atomically wipes state, KV, and meta for one plugin only.
        store.set_plugin_enabled("p_quota", true)?;
        store.plugin_kv_set("p_quota", "token", "abc")?;
        store.set_plugin_enabled("p_other", true)?;
        store.plugin_kv_set("p_other", "token", "xyz")?;

        store.delete_plugin_all_data("p_quota")?;
        assert!(store.plugin_state("p_quota")?.is_none());
        assert!(store.plugin_kv_get("p_quota", "token")?.is_none());
        assert_eq!(store.plugin_meta_usage_bytes("p_quota")?, 0);
        assert!(store.plugin_state("p_other")?.is_some());
        assert_eq!(store.plugin_kv_get("p_other", "token")?.as_deref(), Some("xyz"));
        assert_eq!(store.plugin_meta_get("p_other", "task", "e", "k0")?.as_deref(), Some(chunk.as_str()));
        Ok(())
    }

    #[test]
    fn with_plugin_write_ctx_propagates_restore_failure_without_poisoning() -> Result<()> {
        let store = Store::open_in_memory()?;
        // Inject a trigger that aborts cleanup DELETE only after our marker task exists.
        store.conn.execute_batch(
            "CREATE TEMP TRIGGER fail_ctx_cleanup BEFORE DELETE ON plugin_write_ctx
             WHEN EXISTS (SELECT 1 FROM tasks WHERE title = 'trigger-cleanup-fail')
             BEGIN
                 SELECT RAISE(ABORT, 'simulated cleanup failure');
             END;",
        )?;

        let res: Result<()> = store.with_plugin_write_ctx("com.example.poison", 1, || {
            let _ = store.add_task("trigger-cleanup-fail")?;
            Ok(())
        });
        assert!(res.is_err(), "cleanup failure must be propagated instead of swallowed");
        // Transaction must have rolled back so plugin_write_ctx is clean and unpoisoned.
        let leaked: i64 =
            store.conn.query_row("SELECT COUNT(*) FROM plugin_write_ctx", [], |r| r.get(0))?;
        assert_eq!(leaked, 0, "plugin_write_ctx must not be poisoned after failed cleanup");
        let after = store.add_task("Clean user task")?;
        let events = store.drain_plugin_outbox(10)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].entity_id, after.id.to_string());
        assert_eq!(events[0].origin_plugin, None);
        assert_eq!(events[0].depth, 0);
        Ok(())
    }

    #[test]
    fn plugin_outbox_safe_mode_pause_trim_and_growth_cap() -> Result<()> {
        let store = Store::open_in_memory()?;
        assert!(store.is_plugin_outbox_enabled()?);

        // Safe Mode suppresses outbox writes.
        store.set_setting(PLUGIN_SAFE_MODE_SETTING_KEY, "1")?;
        assert!(!store.is_plugin_outbox_enabled()?);
        let _ = store.add_task("In safe mode")?;
        assert!(store.drain_plugin_outbox(50)?.is_empty());
        store.set_setting(PLUGIN_SAFE_MODE_SETTING_KEY, "0")?;
        assert!(store.is_plugin_outbox_enabled()?);

        // Explicit outbox pause also suppresses writes.
        store.set_plugin_outbox_enabled(false)?;
        assert!(!store.is_plugin_outbox_enabled()?);
        let _ = store.add_task("While paused")?;
        assert!(store.drain_plugin_outbox(50)?.is_empty());
        store.set_plugin_outbox_enabled(true)?;

        // Trim boundary checks (keep > count, usize::MAX, exact keep, 0).
        for i in 0..5 {
            let _ = store.add_task(&format!("T{i}"))?;
        }
        assert_eq!(store.trim_plugin_outbox(usize::MAX)?, 0);
        assert_eq!(store.trim_plugin_outbox(10)?, 0);
        assert_eq!(store.trim_plugin_outbox(2)?, 3);
        assert_eq!(store.drain_plugin_outbox(50)?.len(), 2);
        assert_eq!(store.trim_plugin_outbox(0)?, 2);
        assert!(store.drain_plugin_outbox(50)?.is_empty());

        // Undrained bulk writes automatically cap growth around PLUGIN_OUTBOX_MAX_ROWS.
        store.in_tx(|| {
            for i in 0..(PLUGIN_OUTBOX_MAX_ROWS + 300) {
                store.conn.execute(
                    "INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
                     VALUES ('task.updated', ?1, NULL, 0, 1)",
                    [format!("e{i}")],
                )?;
            }
            Ok(())
        })?;
        let count: usize = store
            .conn
            .query_row("SELECT COUNT(*) FROM plugin_outbox", [], |r| r.get::<_, i64>(0))?
            as usize;
        assert!(
            count <= PLUGIN_OUTBOX_MAX_ROWS + 128,
            "undrained outbox should stay capped near {PLUGIN_OUTBOX_MAX_ROWS}, got {count}"
        );
        Ok(())
    }

    #[test]
    fn wal_immediate_tx_handles_concurrent_kv_and_ctx_writes_without_busy_snapshot() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-wal-immediate-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join("tasks.db");
        let _init = Store::open(&db_path)?;

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let p1 = db_path.clone();
        let b1 = std::sync::Arc::clone(&barrier);
        let t1 = std::thread::spawn(move || -> Result<()> {
            let store = Store::open(&p1)?;
            b1.wait();
            for i in 0..40 {
                store.plugin_kv_set("com.example.a", &format!("k{i}"), "val")?;
            }
            Ok(())
        });

        let p2 = db_path;
        let b2 = std::sync::Arc::clone(&barrier);
        let t2 = std::thread::spawn(move || -> Result<()> {
            let store = Store::open(&p2)?;
            b2.wait();
            for i in 0..40 {
                store.with_plugin_write_ctx("com.example.b", 1, || {
                    let _ = store.add_task(&format!("Concurrent {i}"))?;
                    Ok(())
                })?;
            }
            Ok(())
        });

        t1.join().expect("kv writer thread must not panic")?;
        t2.join().expect("ctx writer thread must not panic")?;
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn plugin_outbox_emits_task_updated_for_tags_checklists_reminders_and_parent() -> Result<()> {
        let store = Store::open_in_memory()?;
        let parent = store.add_task("Parent")?;
        let task = store.add_task("Tagged task")?;
        store.trim_plugin_outbox(0)?;

        // 1. Adding and removing tags emit task.updated.
        store.set_tags(task.id, &["дом".into()])?;
        let evs = store.drain_plugin_outbox(20)?;
        assert!(
            evs.iter().any(|e| e.event_kind == "task.updated" && e.entity_id == task.id.to_string()),
            "set_tags add must emit task.updated: {evs:?}"
        );
        store.trim_plugin_outbox(0)?;

        store.batch_add_tag(&[task.id], "работа")?;
        let evs = store.drain_plugin_outbox(20)?;
        assert!(
            evs.iter().any(|e| e.event_kind == "task.updated" && e.entity_id == task.id.to_string()),
            "batch_add_tag must emit task.updated: {evs:?}"
        );
        store.trim_plugin_outbox(0)?;

        store.batch_remove_tag(&[task.id], "работа")?;
        let evs = store.drain_plugin_outbox(20)?;
        assert!(
            evs.iter().any(|e| e.event_kind == "task.updated" && e.entity_id == task.id.to_string()),
            "batch_remove_tag must emit task.updated: {evs:?}"
        );
        store.trim_plugin_outbox(0)?;

        // Renaming a tag emits task.updated for affected tasks.
        store.update_tag("дом", "квартира", None, None)?;
        let evs = store.drain_plugin_outbox(20)?;
        assert!(
            evs.iter().any(|e| e.event_kind == "task.updated" && e.entity_id == task.id.to_string()),
            "update_tag rename must emit task.updated: {evs:?}"
        );
        store.trim_plugin_outbox(0)?;

        // 2. Reminders and parent_id changes emit task.updated.
        store.set_reminders(task.id, &[15])?;
        let evs = store.drain_plugin_outbox(20)?;
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event_kind, "task.updated");
        store.trim_plugin_outbox(0)?;

        store.link_parent(task.id, parent.id)?;
        let evs = store.drain_plugin_outbox(20)?;
        assert!(
            evs.iter().any(|e| e.event_kind == "task.updated" && e.entity_id == task.id.to_string()),
            "link_parent must emit task.updated: {evs:?}"
        );
        store.trim_plugin_outbox(0)?;

        // 3. Checklist insert / update / delete emit task.updated.
        let item = store.add_checklist_item(task.id, "Step 1", None)?;
        let evs = store.drain_plugin_outbox(20)?;
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event_kind, "task.updated");
        store.trim_plugin_outbox(0)?;

        store.set_checklist_item_completed(item.id, true)?;
        let evs = store.drain_plugin_outbox(20)?;
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event_kind, "task.updated");
        store.trim_plugin_outbox(0)?;

        store.delete_checklist_item(item.id)?;
        let evs = store.drain_plugin_outbox(20)?;
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event_kind, "task.updated");
        store.trim_plugin_outbox(0)?;

        // 4. Purging a task with tags/checklist emits task.deleted, not spurious task.updated.
        store.add_checklist_item(task.id, "Cleanup check", None)?;
        store.trim_plugin_outbox(0)?;
        store.purge_task(task.id)?;
        let evs = store.drain_plugin_outbox(20)?;
        assert!(
            evs.iter().any(|e| e.event_kind == "task.deleted" && e.entity_id == task.id.to_string()),
            "purge_task must emit task.deleted: {evs:?}"
        );
        assert!(
            !evs.iter().any(|e| e.event_kind == "task.updated" && e.entity_id == task.id.to_string()),
            "cascade delete on purge_task must not emit spurious task.updated: {evs:?}"
        );

        // 5. Safe Mode suppresses tag outbox events too.
        let t2 = store.add_task("Safe mode tag task")?;
        store.trim_plugin_outbox(0)?;
        store.set_setting(PLUGIN_SAFE_MODE_SETTING_KEY, "1")?;
        store.set_tags(t2.id, &["muted".into()])?;
        assert!(store.drain_plugin_outbox(20)?.is_empty());
        Ok(())
    }
}
