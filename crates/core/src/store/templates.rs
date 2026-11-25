use anyhow::{Context as _, Result};
use rusqlite::params;
use uuid::Uuid;

use crate::model::{Id, Priority, Task};

use super::{Store, now_ms, parse_uuid};

impl Store {
    /// "Save as Template": keeps a task's title, notes, priority, tags and checklist.
    pub fn save_template(&self, task: Id) -> Result<crate::model::Template> {
        let task = self.task(task)?.context("task not found")?;
        let checklist: Vec<String> = self.checklist(task.id)?.into_iter().map(|item| item.title).collect();
        let template = crate::model::Template {
            id: Uuid::now_v7(),
            title: task.title,
            notes: task.notes,
            priority: task.priority,
            tags: task.tags,
            checklist,
        };
        self.conn.execute(
            "INSERT INTO templates (id, title, notes, priority, tags, checklist, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                template.id.to_string(),
                template.title,
                template.notes,
                template.priority.to_i64(),
                template.tags.join("\n"),
                template.checklist.join("\n"),
                now_ms()
            ],
        )?;
        Ok(template)
    }

    /// Templates, newest first.
    pub fn templates(&self) -> Result<Vec<crate::model::Template>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, title, notes, priority, tags, checklist FROM templates ORDER BY created_at DESC, rowid DESC")?;
        let lines = |text: String| -> Vec<String> { text.lines().filter(|l| !l.is_empty()).map(str::to_owned).collect() };
        let templates = stmt
            .query_map([], |row| {
                Ok(crate::model::Template {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    title: row.get(1)?,
                    notes: row.get(2)?,
                    priority: Priority::from_i64(row.get(3)?),
                    tags: lines(row.get(4)?),
                    checklist: lines(row.get(5)?),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(templates)
    }

    pub fn delete_template(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM templates WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    /// A new task from a template, at the top of the inbox.
    pub fn add_from_template(&self, id: Id) -> Result<Task> {
        self.in_tx(|| {
            let template = self.templates()?.into_iter().find(|t| t.id == id).context("template not found")?;
            let task = self.add_task(&template.title)?;
            self.set_notes(task.id, &template.notes)?;
            self.set_priority(task.id, template.priority)?;
            self.set_tags(task.id, &template.tags)?;
            if !template.checklist.is_empty() {
                let mut item_stmt = self.conn.prepare_cached(
                    "INSERT INTO checklist_items (id, task_id, title, sort_order, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                let task_id_str = task.id.to_string();
                let now = now_ms();
                for (position, item) in template.checklist.iter().enumerate() {
                    item_stmt.execute(params![
                        Uuid::now_v7().to_string(),
                        &task_id_str,
                        item.trim(),
                        position as i64,
                        now,
                    ])?;
                }
            }
            self.task(task.id)?.context("task not found")
        })
    }
}
