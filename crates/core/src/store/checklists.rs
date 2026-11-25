use std::collections::HashMap;

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use crate::model::{ChecklistItem, Id};

use super::{Store, now_ms, parse_uuid};

impl Store {
    /// The task's checklist in display order.
    pub fn checklist(&self, task: Id) -> Result<Vec<ChecklistItem>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, task_id, title, completed_at IS NOT NULL FROM checklist_items
             WHERE task_id = ?1 ORDER BY sort_order, rowid",
        )?;
        let items = stmt
            .query_map([task.to_string()], |row| {
                Ok(ChecklistItem {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    task_id: parse_uuid(&row.get::<_, String>(1)?, 1)?,
                    title: row.get(2)?,
                    completed: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(items)
    }

    /// Adds an item to the end of the checklist, or right after `after` when given.
    pub fn add_checklist_item(&self, task: Id, title: &str, after: Option<Id>) -> Result<ChecklistItem> {
        self.in_tx(|| {
            let position: i64 = match after {
                Some(after) => {
                    let position: i64 = self.conn.query_row(
                        "SELECT sort_order FROM checklist_items WHERE id = ?1",
                        [after.to_string()],
                        |row| row.get(0),
                    )?;
                    self.conn.execute(
                        "UPDATE checklist_items SET sort_order = sort_order + 1
                         WHERE task_id = ?1 AND sort_order > ?2",
                        params![task.to_string(), position],
                    )?;
                    position + 1
                }
                None => self.conn.query_row(
                    "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM checklist_items WHERE task_id = ?1",
                    [task.to_string()],
                    |row| row.get(0),
                )?,
            };
            let item = ChecklistItem { id: Uuid::now_v7(), task_id: task, title: title.trim().to_owned(), completed: false };
            self.conn.execute(
                "INSERT INTO checklist_items (id, task_id, title, sort_order, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![item.id.to_string(), task.to_string(), item.title, position, now_ms()],
            )?;
            Ok(item)
        })
    }

    pub fn set_checklist_item_title(&self, id: Id, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE checklist_items SET title = ?2 WHERE id = ?1",
            params![id.to_string(), title],
        )?;
        Ok(())
    }

    pub fn set_checklist_item_completed(&self, id: Id, completed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE checklist_items SET completed_at = ?2 WHERE id = ?1",
            params![id.to_string(), completed.then(now_ms)],
        )?;
        Ok(())
    }

    pub fn delete_checklist_item(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM checklist_items WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    pub(super) fn checklist_progress(&self, ids_json: Option<&str>) -> Result<HashMap<Id, (usize, usize)>> {
        let mut stmt = self.conn.prepare_cached(if ids_json.is_some() {
            "SELECT task_id, COUNT(completed_at), COUNT(*) FROM checklist_items
             WHERE task_id IN (SELECT value FROM json_each(?1))
             GROUP BY task_id"
        } else {
            "SELECT task_id, COUNT(completed_at), COUNT(*) FROM checklist_items GROUP BY task_id"
        })?;
        let mut progress = HashMap::new();
        for row in stmt.query_map(rusqlite::params_from_iter(ids_json), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
        })? {
            let (task, done, total) = row?;
            if let Ok(id) = Uuid::parse_str(&task) {
                progress.insert(id, (done as usize, total as usize));
            }
        }
        Ok(progress)
    }
}
