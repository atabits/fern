use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::Store;

impl Store {
    /// A user setting such as the UI language; `None` when never set.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([key], |row| row.get(0))
            .optional()?)
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )?
            .execute(params![key, value])?;
        Ok(())
    }

    /// Inserts a setting only if absent. Returns whether this connection inserted it.
    pub fn insert_setting(&self, key: &str, value: &str) -> Result<bool> {
        let inserted = self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO NOTHING",
            params![key, value],
        )?;
        Ok(inserted != 0)
    }
}
