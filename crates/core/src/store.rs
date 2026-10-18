use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use chrono::{Duration, NaiveDate, NaiveTime};
use rusqlite::{Connection, OptionalExtension, Row, params};
use uuid::Uuid;

use crate::model::{ChecklistItem, Id, Kind, List, Priority, Tag, Task, View, ViewCounts};
use crate::recurrence::Recurrence;

const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE tasks (
        id           TEXT PRIMARY KEY,
        title        TEXT NOT NULL,
        completed_at INTEGER,
        created_at   INTEGER NOT NULL
    );
    ",
    "
    ALTER TABLE tasks ADD COLUMN notes TEXT NOT NULL DEFAULT '';
    ",
    "
    ALTER TABLE tasks ADD COLUMN parent_id TEXT REFERENCES tasks(id) ON DELETE CASCADE;
    ALTER TABLE tasks ADD COLUMN kind      TEXT NOT NULL DEFAULT 'task';
    ALTER TABLE tasks ADD COLUMN priority  INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE tasks ADD COLUMN due_date  TEXT;
    ALTER TABLE tasks ADD COLUMN due_time  TEXT;
    CREATE INDEX tasks_parent ON tasks(parent_id);

    CREATE TABLE tags (
        name       TEXT PRIMARY KEY,
        color      TEXT,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE task_tags (
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        tag     TEXT NOT NULL REFERENCES tags(name) ON DELETE CASCADE ON UPDATE CASCADE,
        PRIMARY KEY (task_id, tag)
    );
    ",
    "
    CREATE TABLE lists (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        color      TEXT,
        created_at INTEGER NOT NULL
    );
    ALTER TABLE tasks ADD COLUMN list_id TEXT REFERENCES lists(id) ON DELETE CASCADE;
    CREATE INDEX tasks_list ON tasks(list_id);
    ",
    "
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    ",
    "
    CREATE TABLE checklist_items (
        id           TEXT PRIMARY KEY,
        task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        title        TEXT NOT NULL,
        completed_at INTEGER,
        sort_order   INTEGER NOT NULL,
        created_at   INTEGER NOT NULL
    );
    CREATE INDEX checklist_items_task ON checklist_items(task_id, sort_order);
    ",
    "
    ALTER TABLE tasks ADD COLUMN deleted_at INTEGER;
    ",
    "
    ALTER TABLE tasks ADD COLUMN repeat TEXT;
    ",
];

const TASK_COLUMNS: &str = "id, parent_id, kind, title, notes, priority, due_date, due_time, \
                            completed_at, created_at, list_id, deleted_at, repeat";

const DATE_FORMAT: &str = "%Y-%m-%d";
const TIME_FORMAT: &str = "%H:%M";

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let version: usize = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            conn.execute_batch(sql)?;
            conn.pragma_update(None, "user_version", i + 1)?;
        }
        Ok(Self { conn })
    }

    /// All tasks: open ones first (newest on top), then completed ones (most recent on top).
    pub fn tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks
             ORDER BY completed_at IS NOT NULL, completed_at DESC, created_at DESC, rowid DESC"
        ))?;
        let mut tasks: Vec<Task> = stmt.query_map([], task_from_row)?.collect::<Result<_, _>>()?;

        let mut tags: HashMap<Id, Vec<String>> = HashMap::new();
        let mut stmt = self.conn.prepare("SELECT task_id, tag FROM task_tags ORDER BY tag")?;
        for row in stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
            let (task_id, tag) = row?;
            if let Ok(id) = Uuid::parse_str(&task_id) {
                tags.entry(id).or_default().push(tag);
            }
        }
        let progress = self.checklist_progress()?;
        for task in &mut tasks {
            if let Some(task_tags) = tags.remove(&task.id) {
                task.tags = task_tags;
            }
            task.checklist = progress.get(&task.id).copied();
        }
        Ok(tasks)
    }

    /// Tasks for a view. `today` is passed in so tests do not depend on the clock or time zone.
    pub fn tasks_in(&self, view: &View, today: NaiveDate) -> Result<Vec<Task>> {
        Ok(self.tasks()?.into_iter().filter(|task| matches_view(task, view, today)).collect())
    }

    pub fn counts(&self, today: NaiveDate) -> Result<ViewCounts> {
        let tasks = self.tasks()?;
        let count = |view: View| {
            tasks.iter().filter(|t| !t.is_completed() && matches_view(t, &view, today)).count()
        };
        Ok(ViewCounts {
            inbox: count(View::Inbox),
            all: count(View::All),
            today: count(View::Today),
            tomorrow: count(View::Tomorrow),
            next_7_days: count(View::Next7Days),
        })
    }

    pub fn task(&self, id: Id) -> Result<Option<Task>> {
        let task = self
            .conn
            .query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
                [id.to_string()],
                task_from_row,
            )
            .optional()?;
        let Some(mut task) = task else { return Ok(None) };
        let mut stmt =
            self.conn.prepare("SELECT tag FROM task_tags WHERE task_id = ?1 ORDER BY tag")?;
        task.tags = stmt.query_map([id.to_string()], |row| row.get(0))?.collect::<Result<_, _>>()?;
        task.checklist = self.checklist_progress()?.get(&id).copied();
        Ok(Some(task))
    }

    pub fn add_task(&self, title: &str) -> Result<Task> {
        let task = Task {
            id: Uuid::now_v7(),
            list_id: None,
            parent_id: None,
            kind: Kind::Task,
            title: title.trim().to_owned(),
            notes: String::new(),
            priority: Priority::None,
            due_date: None,
            due_time: None,
            repeat: None,
            tags: Vec::new(),
            checklist: None,
            completed_at: None,
            deleted_at: None,
            created_at: now_ms(),
        };
        self.conn.execute(
            "INSERT INTO tasks (id, title, created_at) VALUES (?1, ?2, ?3)",
            params![task.id.to_string(), task.title, task.created_at],
        )?;
        Ok(task)
    }

    /// Completes or reopens a task. Completing a repeating task with a due date does not close
    /// it: a completed copy is kept for history, and the task moves to its next occurrence with
    /// its checklist reset — the way TickTick handles repeats.
    /// Direct subtasks of `parent` that are not in the trash, oldest first.
    pub fn subtasks(&self, parent: Id) -> Result<Vec<Task>> {
        let mut subtasks: Vec<Task> = self
            .tasks()?
            .into_iter()
            .filter(|t| t.parent_id == Some(parent) && !t.is_deleted())
            .collect();
        subtasks.reverse();
        Ok(subtasks)
    }

    /// Creates a subtask in the parent's list.
    pub fn add_subtask(&self, parent: Id, title: &str) -> Result<Task> {
        let list = self.task(parent)?.ok_or_else(|| anyhow::anyhow!("parent task not found"))?.list_id;
        let mut task = self.add_task(title)?;
        self.set_parent(task.id, Some(parent))?;
        if list.is_some() {
            self.set_list(task.id, list)?;
        }
        task.parent_id = Some(parent);
        task.list_id = list;
        Ok(task)
    }

    pub fn set_completed(&self, id: Id, completed: bool) -> Result<()> {
        if completed
            && let Some(task) = self.task(id)?
            && let (Some(repeat), Some(due)) = (task.repeat, task.due_date)
        {
            let tx = self.conn.unchecked_transaction()?;
            let copy = Uuid::now_v7();
            tx.execute(
                &format!(
                    "INSERT INTO tasks ({TASK_COLUMNS})
                     SELECT ?2, parent_id, kind, title, notes, priority, due_date, due_time,
                            ?3, created_at, list_id, NULL, NULL
                     FROM tasks WHERE id = ?1"
                ),
                params![id.to_string(), copy.to_string(), now_ms()],
            )?;
            tx.execute(
                "INSERT INTO task_tags (task_id, tag) SELECT ?2, tag FROM task_tags WHERE task_id = ?1",
                params![id.to_string(), copy.to_string()],
            )?;
            tx.execute(
                "UPDATE tasks SET due_date = ?2 WHERE id = ?1",
                params![id.to_string(), repeat.next_after(due).format(DATE_FORMAT).to_string()],
            )?;
            tx.execute(
                "UPDATE checklist_items SET completed_at = NULL WHERE task_id = ?1",
                [id.to_string()],
            )?;
            tx.commit()?;
            return Ok(());
        }
        self.update(id, "completed_at", completed.then(now_ms))
    }

    /// Sets or clears the repeat rule. A repeating task needs a due date; `today` is used when
    /// the task has none.
    pub fn set_repeat(&self, id: Id, repeat: Option<Recurrence>, today: NaiveDate) -> Result<()> {
        if repeat.is_some()
            && let Some(task) = self.task(id)?
            && task.due_date.is_none()
        {
            self.set_due(id, Some(today), task.due_time)?;
        }
        self.update(id, "repeat", repeat.map(|r| r.to_string()))
    }

    pub fn set_title(&self, id: Id, title: &str) -> Result<()> {
        self.update(id, "title", title)
    }

    pub fn set_notes(&self, id: Id, notes: &str) -> Result<()> {
        self.update(id, "notes", notes)
    }

    pub fn set_priority(&self, id: Id, priority: Priority) -> Result<()> {
        self.update(id, "priority", priority.to_i64())
    }

    pub fn set_kind(&self, id: Id, kind: Kind) -> Result<()> {
        self.update(id, "kind", kind.as_str())
    }

    /// Sets the due date. A time without a date is meaningless and is dropped.
    pub fn set_due(&self, id: Id, date: Option<NaiveDate>, time: Option<NaiveTime>) -> Result<()> {
        let date_str = date.map(|d| d.format(DATE_FORMAT).to_string());
        let time_str = date.and(time).map(|t| t.format(TIME_FORMAT).to_string());
        self.conn.execute(
            "UPDATE tasks SET due_date = ?2, due_time = ?3 WHERE id = ?1",
            params![id.to_string(), date_str, time_str],
        )?;
        Ok(())
    }

    /// Makes the task a subtask of `parent`, or a top-level task for `None`.
    pub fn set_parent(&self, id: Id, parent: Option<Id>) -> Result<()> {
        if let Some(parent) = parent {
            anyhow::ensure!(
                parent != id && !self.descendants(id)?.contains(&parent),
                "a task cannot be nested inside itself"
            );
        }
        self.update(id, "parent_id", parent.map(|p| p.to_string()))
    }

    /// Replaces the task's tags; unknown tags are created.
    /// Tags match case-insensitively in Rust, because SQLite's `NOCASE` only folds ASCII.
    pub fn set_tags(&self, id: Id, tags: &[String]) -> Result<()> {
        let existing: Vec<String> = self
            .conn
            .prepare("SELECT name FROM tags")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM task_tags WHERE task_id = ?1", [id.to_string()])?;
        for tag in tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
            let name = match existing.iter().find(|e| e.to_lowercase() == tag.to_lowercase()) {
                Some(name) => name.clone(),
                None => {
                    tx.execute(
                        "INSERT OR IGNORE INTO tags (name, created_at) VALUES (?1, ?2)",
                        params![tag, now_ms()],
                    )?;
                    tag.to_owned()
                }
            };
            tx.execute(
                "INSERT OR IGNORE INTO task_tags (task_id, tag) VALUES (?1, ?2)",
                params![id.to_string(), name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn tags(&self) -> Result<Vec<Tag>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name, t.color, COUNT(tk.id)
             FROM tags t
             LEFT JOIN task_tags tt ON tt.tag = t.name
             LEFT JOIN tasks tk ON tk.id = tt.task_id AND tk.completed_at IS NULL
                                AND tk.deleted_at IS NULL
             GROUP BY t.name
             ORDER BY t.created_at, t.rowid",
        )?;
        let tags = stmt
            .query_map([], |row| {
                Ok(Tag {
                    name: row.get(0)?,
                    color: row.get(1)?,
                    open_count: row.get::<_, i64>(2)? as usize,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(tags)
    }

    pub fn set_tag_color(&self, name: &str, color: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE tags SET color = ?2 WHERE name = ?1", params![name, color])?;
        Ok(())
    }

    pub fn lists(&self) -> Result<Vec<List>> {
        let mut stmt = self.conn.prepare(
            "SELECT l.id, l.name, l.color, COUNT(t.id)
             FROM lists l
             LEFT JOIN tasks t ON t.list_id = l.id AND t.completed_at IS NULL
                               AND t.deleted_at IS NULL
             GROUP BY l.id
             ORDER BY l.created_at, l.rowid",
        )?;
        let lists = stmt
            .query_map([], |row| {
                Ok(List {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    open_count: row.get::<_, i64>(3)? as usize,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(lists)
    }

    pub fn create_list(&self, name: &str) -> Result<List> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a list needs a name");
        let list = List { id: Uuid::now_v7(), name: name.to_owned(), color: None, open_count: 0 };
        self.conn.execute(
            "INSERT INTO lists (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![list.id.to_string(), list.name, now_ms()],
        )?;
        Ok(list)
    }

    pub fn rename_list(&self, id: Id, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a list needs a name");
        self.conn.execute("UPDATE lists SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(())
    }

    /// Sets the list color as `#RRGGBB`, or clears it with `None`.
    pub fn set_list_color(&self, id: Id, color: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE lists SET color = ?2 WHERE id = ?1", params![id.to_string(), color])?;
        Ok(())
    }

    /// Deletes the list together with its tasks.
    pub fn delete_list(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM lists WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    /// Moves the task and its subtasks to a list (`None` means the Inbox).
    pub fn set_list(&self, id: Id, list: Option<Id>) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let list = list.map(|l| l.to_string());
        for task in std::iter::once(id).chain(self.descendants(id)?) {
            tx.execute("UPDATE tasks SET list_id = ?2 WHERE id = ?1", params![task.to_string(), list])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The task's checklist in display order.
    pub fn checklist(&self, task: Id) -> Result<Vec<ChecklistItem>> {
        let mut stmt = self.conn.prepare(
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
        let tx = self.conn.unchecked_transaction()?;
        let position: i64 = match after {
            Some(after) => {
                let position: i64 = tx.query_row(
                    "SELECT sort_order FROM checklist_items WHERE id = ?1",
                    [after.to_string()],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "UPDATE checklist_items SET sort_order = sort_order + 1
                     WHERE task_id = ?1 AND sort_order > ?2",
                    params![task.to_string(), position],
                )?;
                position + 1
            }
            None => tx.query_row(
                "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM checklist_items WHERE task_id = ?1",
                [task.to_string()],
                |row| row.get(0),
            )?,
        };
        let item = ChecklistItem { id: Uuid::now_v7(), task_id: task, title: title.trim().to_owned(), completed: false };
        tx.execute(
            "INSERT INTO checklist_items (id, task_id, title, sort_order, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![item.id.to_string(), task.to_string(), item.title, position, now_ms()],
        )?;
        tx.commit()?;
        Ok(item)
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

    fn checklist_progress(&self) -> Result<HashMap<Id, (usize, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, COUNT(completed_at), COUNT(*) FROM checklist_items GROUP BY task_id",
        )?;
        let mut progress = HashMap::new();
        for row in stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
        })? {
            let (task, done, total) = row?;
            if let Ok(id) = Uuid::parse_str(&task) {
                progress.insert(id, (done as usize, total as usize));
            }
        }
        Ok(progress)
    }

    /// A user setting such as the UI language; `None` when never set.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Moves the task and its subtasks to the trash.
    pub fn delete_task(&self, id: Id) -> Result<()> {
        self.set_deleted(id, Some(now_ms()))
    }

    /// Brings the task and its subtasks back from the trash.
    pub fn restore_task(&self, id: Id) -> Result<()> {
        self.set_deleted(id, None)
    }

    /// Deletes the task and its subtasks permanently.
    pub fn purge_task(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    /// Permanently deletes everything in the trash.
    pub fn empty_trash(&self) -> Result<()> {
        self.conn.execute("DELETE FROM tasks WHERE deleted_at IS NOT NULL", [])?;
        Ok(())
    }

    fn set_deleted(&self, id: Id, deleted_at: Option<i64>) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for task in std::iter::once(id).chain(self.descendants(id)?) {
            tx.execute(
                "UPDATE tasks SET deleted_at = ?2 WHERE id = ?1",
                params![task.to_string(), deleted_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn update(&self, id: Id, column: &'static str, value: impl rusqlite::ToSql) -> Result<()> {
        self.conn.execute(
            &format!("UPDATE tasks SET {column} = ?2 WHERE id = ?1"),
            params![id.to_string(), value],
        )?;
        Ok(())
    }

    fn descendants(&self, id: Id) -> Result<Vec<Id>> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE sub(id) AS (
                 SELECT id FROM tasks WHERE parent_id = ?1
                 UNION ALL
                 SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
             )
             SELECT id FROM sub",
        )?;
        let ids = stmt
            .query_map([id.to_string()], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok().and_then(|s| Uuid::parse_str(&s).ok()))
            .collect();
        Ok(ids)
    }
}

fn matches_view(task: &Task, view: &View, today: NaiveDate) -> bool {
    if task.is_deleted() != (*view == View::Trash) {
        return false;
    }
    let due_before = |days: i64| task.due_date.is_some_and(|due| due < today + Duration::days(days));
    match view {
        View::Inbox => task.list_id.is_none(),
        View::All => true,
        View::List(id) => task.list_id == Some(*id),
        View::Today => !task.is_completed() && due_before(1),
        View::Tomorrow => {
            !task.is_completed() && task.due_date == Some(today + Duration::days(1))
        }
        View::Next7Days => !task.is_completed() && due_before(7),
        View::Completed => task.is_completed(),
        View::Trash => true,
        View::Tag(tag) => task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()),
        View::Search(query) => {
            let query = query.trim().to_lowercase();
            !query.is_empty()
                && (task.title.to_lowercase().contains(&query)
                    || task.notes.to_lowercase().contains(&query)
                    || task.tags.iter().any(|t| t.to_lowercase().contains(&query)))
        }
    }
}

fn parse_uuid(value: &str, column: usize) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn task_from_row(row: &Row) -> rusqlite::Result<Task> {
    let parent_id: Option<String> = row.get(1)?;
    let due_date: Option<String> = row.get(6)?;
    let due_time: Option<String> = row.get(7)?;
    Ok(Task {
        id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
        parent_id: parent_id.map(|p| parse_uuid(&p, 1)).transpose()?,
        kind: Kind::parse(&row.get::<_, String>(2)?),
        title: row.get(3)?,
        notes: row.get(4)?,
        priority: Priority::from_i64(row.get(5)?),
        due_date: due_date.and_then(|d| NaiveDate::parse_from_str(&d, DATE_FORMAT).ok()),
        due_time: due_time.and_then(|t| NaiveTime::parse_from_str(&t, TIME_FORMAT).ok()),
        tags: Vec::new(),
        checklist: None,
        completed_at: row.get(8)?,
        created_at: row.get(9)?,
        list_id: row.get::<_, Option<String>>(10)?.map(|l| parse_uuid(&l, 10)).transpose()?,
        deleted_at: row.get(11)?,
        repeat: row.get::<_, Option<String>>(12)?.and_then(|r| r.parse().ok()),
    })
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn titles(tasks: &[Task]) -> Vec<&str> {
        tasks.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn add_complete_delete() -> Result<()> {
        let store = Store::open_in_memory()?;
        let a = store.add_task("  buy milk ")?;
        let b = store.add_task("call the bank")?;
        assert_eq!(a.title, "buy milk");

        store.set_completed(a.id, true)?;
        let tasks = store.tasks()?;
        assert_eq!(tasks.iter().map(|t| t.id).collect::<Vec<_>>(), [b.id, a.id]);
        assert!(store.task(a.id)?.unwrap().is_completed());

        store.set_completed(a.id, false)?;
        assert!(!store.task(a.id)?.unwrap().is_completed());

        store.set_title(b.id, "call the dentist")?;
        store.set_notes(b.id, "after 6 pm\nask about weekends")?;
        let b2 = store.task(b.id)?.unwrap();
        assert_eq!(b2.title, "call the dentist");
        assert_eq!(b2.notes, "after 6 pm\nask about weekends");

        store.purge_task(b.id)?;
        assert_eq!(store.tasks()?.len(), 1);
        Ok(())
    }

    #[test]
    fn reopen_keeps_data() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-test-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("tasks.db");
        Store::open(&path)?.add_task("x")?;
        assert_eq!(Store::open(&path)?.tasks()?.len(), 1);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn migrates_v1_database() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(MIGRATIONS[0])?;
        conn.pragma_update(None, "user_version", 1)?;
        conn.execute(
            "INSERT INTO tasks VALUES ('0192f000-0000-7000-8000-000000000001', 'legacy', NULL, 1)",
            [],
        )?;
        let store = Store::init(conn)?;
        let task = &store.tasks()?[0];
        assert_eq!(
            (task.title.as_str(), task.kind, task.priority),
            ("legacy", Kind::Task, Priority::None)
        );
        Ok(())
    }

    #[test]
    fn priority_kind_and_due() -> Result<()> {
        let store = Store::open_in_memory()?;
        let t = store.add_task("report")?;
        store.set_priority(t.id, Priority::High)?;
        store.set_kind(t.id, Kind::Note)?;
        store.set_due(t.id, Some(d(2026, 10, 20)), NaiveTime::from_hms_opt(18, 30, 0))?;
        let t = store.task(t.id)?.unwrap();
        assert_eq!(t.priority, Priority::High);
        assert_eq!(t.kind, Kind::Note);
        assert_eq!(t.due_date, Some(d(2026, 10, 20)));
        assert_eq!(t.due_time, NaiveTime::from_hms_opt(18, 30, 0));

        // A time without a date is not stored.
        store.set_due(t.id, None, NaiveTime::from_hms_opt(9, 0, 0))?;
        let t = store.task(t.id)?.unwrap();
        assert_eq!((t.due_date, t.due_time), (None, None));
        Ok(())
    }

    #[test]
    fn smart_views_and_counts() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = d(2026, 10, 18);
        let overdue = store.add_task("overdue")?;
        let now = store.add_task("today")?;
        let tomorrow = store.add_task("tomorrow")?;
        let week = store.add_task("in 6 days")?;
        let later = store.add_task("in a week")?;
        let done = store.add_task("done today")?;
        store.add_task("no due date")?;
        store.set_due(overdue.id, Some(d(2026, 10, 15)), None)?;
        store.set_due(now.id, Some(today), None)?;
        store.set_due(tomorrow.id, Some(d(2026, 10, 19)), None)?;
        store.set_due(week.id, Some(d(2026, 10, 24)), None)?;
        store.set_due(later.id, Some(d(2026, 10, 25)), None)?;
        store.set_due(done.id, Some(today), None)?;
        store.set_completed(done.id, true)?;

        assert_eq!(titles(&store.tasks_in(&View::Today, today)?), ["today", "overdue"]);
        assert_eq!(titles(&store.tasks_in(&View::Tomorrow, today)?), ["tomorrow"]);
        assert_eq!(
            titles(&store.tasks_in(&View::Next7Days, today)?),
            ["in 6 days", "tomorrow", "today", "overdue"]
        );
        assert_eq!(titles(&store.tasks_in(&View::Completed, today)?), ["done today"]);
        assert_eq!(
            store.counts(today)?,
            ViewCounts { inbox: 6, all: 6, today: 2, tomorrow: 1, next_7_days: 4 }
        );
        Ok(())
    }

    #[test]
    fn tags_are_shared_case_insensitive_and_counted() -> Result<()> {
        let store = Store::open_in_memory()?;
        let a = store.add_task("a")?;
        let b = store.add_task("b")?;
        store.set_tags(a.id, &["Élan".into(), "focus".into()])?;
        store.set_tags(b.id, &["élan".into()])?;
        store.set_tag_color("Élan", Some("#3B82F6"))?;

        assert_eq!(store.task(b.id)?.unwrap().tags, ["Élan"]);
        let tags = store.tags()?;
        assert_eq!(
            tags.iter().map(|t| (t.name.as_str(), t.open_count)).collect::<Vec<_>>(),
            [("Élan", 2), ("focus", 1)]
        );
        assert_eq!(tags[0].color.as_deref(), Some("#3B82F6"));

        store.set_completed(b.id, true)?;
        assert_eq!(store.tags()?[0].open_count, 1);
        assert_eq!(titles(&store.tasks_in(&View::Tag("ÉLAN".into()), d(2026, 1, 1))?), ["a", "b"]);

        store.set_tags(a.id, &[])?;
        assert!(store.task(a.id)?.unwrap().tags.is_empty());
        Ok(())
    }

    #[test]
    fn lists_scope_inbox_and_move_subtasks() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = d(2026, 10, 18);
        let work = store.create_list(" Work ")?;
        assert_eq!(work.name, "Work");
        assert!(store.create_list("  ").is_err());

        let inbox_task = store.add_task("in the inbox")?;
        let parent = store.add_task("report")?;
        let child = store.add_task("collect numbers")?;
        store.set_parent(child.id, Some(parent.id))?;
        store.set_list(parent.id, Some(work.id))?;

        assert_eq!(store.task(child.id)?.unwrap().list_id, Some(work.id));
        assert_eq!(titles(&store.tasks_in(&View::Inbox, today)?), [inbox_task.title.as_str()]);
        assert_eq!(titles(&store.tasks_in(&View::List(work.id), today)?), ["collect numbers", "report"]);
        assert_eq!(store.tasks_in(&View::All, today)?.len(), 3);
        assert_eq!(store.counts(today)?.inbox, 1);
        assert_eq!(store.lists()?[0].open_count, 2);

        store.rename_list(work.id, "Office")?;
        assert_eq!(store.lists()?[0].name, "Office");
        assert!(store.rename_list(work.id, " ").is_err());
        store.set_list_color(work.id, Some("#22C55E"))?;
        assert_eq!(store.lists()?[0].color.as_deref(), Some("#22C55E"));
        store.set_list_color(work.id, None)?;
        assert_eq!(store.lists()?[0].color, None);

        store.delete_list(work.id)?;
        assert!(store.lists()?.is_empty());
        assert_eq!(titles(&store.tasks()?), ["in the inbox"]);
        Ok(())
    }

    #[test]
    fn checklist_items_order_progress_and_cascade() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("pack for the trip")?;
        let a = store.add_checklist_item(task.id, " passport ", None)?;
        let c = store.add_checklist_item(task.id, "charger", None)?;
        let b = store.add_checklist_item(task.id, "tickets", Some(a.id))?;
        let titles = |store: &Store| -> Result<Vec<String>> {
            Ok(store.checklist(task.id)?.into_iter().map(|i| i.title).collect())
        };
        assert_eq!(titles(&store)?, ["passport", "tickets", "charger"]);

        store.set_checklist_item_completed(b.id, true)?;
        store.set_checklist_item_title(c.id, "phone charger")?;
        assert_eq!(store.task(task.id)?.unwrap().checklist, Some((1, 3)));
        assert_eq!(store.tasks()?[0].checklist, Some((1, 3)));
        assert!(store.checklist(task.id)?[1].completed);

        store.delete_checklist_item(a.id)?;
        assert_eq!(titles(&store)?, ["tickets", "phone charger"]);
        store.purge_task(task.id)?;
        assert!(store.checklist(task.id)?.is_empty());
        Ok(())
    }

    #[test]
    fn trash_hides_restores_and_purges() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = d(2026, 10, 18);
        let parent = store.add_task("old project")?;
        let child = store.add_task("its step")?;
        let kept = store.add_task("keep me")?;
        store.set_parent(child.id, Some(parent.id))?;
        store.set_tags(parent.id, &["work".into()])?;

        store.delete_task(parent.id)?;
        assert_eq!(titles(&store.tasks_in(&View::Inbox, today)?), ["keep me"]);
        assert_eq!(titles(&store.tasks_in(&View::Trash, today)?), ["its step", "old project"]);
        assert_eq!(store.counts(today)?.inbox, 1);
        assert_eq!(store.tags()?[0].open_count, 0);

        store.restore_task(parent.id)?;
        assert_eq!(store.tasks_in(&View::Inbox, today)?.len(), 3);
        assert!(store.tasks_in(&View::Trash, today)?.is_empty());

        store.delete_task(parent.id)?;
        store.empty_trash()?;
        assert_eq!(titles(&store.tasks()?), ["keep me"]);
        assert!(store.task(kept.id)?.is_some());
        Ok(())
    }

    #[test]
    fn completing_a_repeating_task_moves_it_forward() -> Result<()> {
        use crate::recurrence::Frequency;

        let store = Store::open_in_memory()?;
        let today = d(2026, 10, 18);
        let task = store.add_task("water the plants")?;
        store.set_tags(task.id, &["home".into()])?;
        store.set_repeat(task.id, Some(Recurrence::new(Frequency::Weekly, 1)), today)?;
        assert_eq!(store.task(task.id)?.unwrap().due_date, Some(today));
        let item = store.add_checklist_item(task.id, "balcony", None)?;
        store.set_checklist_item_completed(item.id, true)?;

        store.set_completed(task.id, true)?;
        let task_now = store.task(task.id)?.unwrap();
        assert!(!task_now.is_completed());
        assert_eq!(task_now.due_date, Some(d(2026, 10, 25)));
        assert_eq!(task_now.checklist, Some((0, 1)));

        let history = store.tasks_in(&View::Completed, today)?;
        assert_eq!(titles(&history), ["water the plants"]);
        assert_eq!(history[0].due_date, Some(today));
        assert_eq!(history[0].repeat, None);
        assert_eq!(history[0].tags, ["home"]);

        store.set_repeat(task.id, None, today)?;
        store.set_completed(task.id, true)?;
        assert!(store.task(task.id)?.unwrap().is_completed());
        Ok(())
    }

    #[test]
    fn search_matches_title_notes_and_tags() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = d(2026, 10, 18);
        let a = store.add_task("Buy Milk")?;
        let b = store.add_task("Call the bank")?;
        let c = store.add_task("Купить хлеб")?;
        let gone = store.add_task("milk in the trash")?;
        store.set_notes(b.id, "ask about the milk subscription")?;
        store.set_tags(c.id, &["Groceries".into()])?;
        store.set_completed(a.id, true)?;
        store.delete_task(gone.id)?;

        let search = |q: &str| -> Result<Vec<String>> {
            Ok(store.tasks_in(&View::Search(q.into()), today)?.into_iter().map(|t| t.title).collect())
        };
        assert_eq!(search("MILK")?, ["Call the bank", "Buy Milk"]);
        assert_eq!(search("ХЛЕБ")?, ["Купить хлеб"]);
        assert_eq!(search("grocer")?, ["Купить хлеб"]);
        assert!(search("   ")?.is_empty());
        Ok(())
    }

    #[test]
    fn settings_roundtrip() -> Result<()> {
        let store = Store::open_in_memory()?;
        assert_eq!(store.setting("locale")?, None);
        store.set_setting("locale", "ru")?;
        store.set_setting("locale", "en")?;
        assert_eq!(store.setting("locale")?.as_deref(), Some("en"));
        Ok(())
    }

    #[test]
    fn subtasks_are_created_in_the_parent_list() -> Result<()> {
        let store = Store::open_in_memory()?;
        let list = store.create_list("Trip")?;
        let parent = store.add_task("pack")?;
        store.set_list(parent.id, Some(list.id))?;
        let first = store.add_subtask(parent.id, "passport")?;
        let second = store.add_subtask(parent.id, "charger")?;
        assert_eq!(store.task(first.id)?.unwrap().list_id, Some(list.id));
        assert_eq!(titles(&store.subtasks(parent.id)?), ["passport", "charger"]);

        store.delete_task(second.id)?;
        assert_eq!(titles(&store.subtasks(parent.id)?), ["passport"]);
        assert!(store.add_subtask(Uuid::now_v7(), "orphan").is_err());
        Ok(())
    }

    #[test]
    fn subtasks_cascade_and_no_cycles() -> Result<()> {
        let store = Store::open_in_memory()?;
        let parent = store.add_task("plan a trip")?;
        let child = store.add_task("book flights")?;
        let grandchild = store.add_task("compare prices")?;
        store.set_parent(child.id, Some(parent.id))?;
        store.set_parent(grandchild.id, Some(child.id))?;
        assert_eq!(store.task(child.id)?.unwrap().parent_id, Some(parent.id));

        assert!(store.set_parent(parent.id, Some(grandchild.id)).is_err());
        assert!(store.set_parent(parent.id, Some(parent.id)).is_err());

        store.purge_task(parent.id)?;
        assert!(store.tasks()?.is_empty());
        Ok(())
    }
}
