use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use chrono::{Duration, NaiveDate, NaiveTime};
use rusqlite::{Connection, OptionalExtension, Row, params};
use uuid::Uuid;

use crate::countdown::{Countdown, CountdownKind};
use crate::focus::FocusSession;
use crate::habit::{Checkin, Goal, Habit};
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
    "
    ALTER TABLE tasks ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
    UPDATE tasks SET sort_order = -created_at;
    ",
    "
    CREATE TABLE countdowns (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        date       TEXT NOT NULL,
        kind       TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    ",
    "
    CREATE TABLE habits (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE habit_checkins (
        habit_id TEXT NOT NULL REFERENCES habits(id) ON DELETE CASCADE,
        date     TEXT NOT NULL,
        PRIMARY KEY (habit_id, date)
    );
    ",
    "
    CREATE TABLE focus_sessions (
        id         TEXT PRIMARY KEY,
        task_id    TEXT REFERENCES tasks(id) ON DELETE SET NULL,
        title      TEXT NOT NULL,
        started_at INTEGER NOT NULL,
        ended_at   INTEGER NOT NULL
    );
    ",
    "
    ALTER TABLE tasks ADD COLUMN wont_do_at INTEGER;
    ",
    "
    ALTER TABLE habits ADD COLUMN goal_amount INTEGER;
    ALTER TABLE habits ADD COLUMN goal_unit TEXT;
    ALTER TABLE habit_checkins ADD COLUMN value INTEGER NOT NULL DEFAULT 1;
    ALTER TABLE habit_checkins ADD COLUMN note TEXT;
    ",
    "
    ALTER TABLE tasks ADD COLUMN reminders TEXT;
    ",
    "
    ALTER TABLE habits ADD COLUMN reminders TEXT;
    ",
    "
    ALTER TABLE tasks ADD COLUMN end_date TEXT;
    ALTER TABLE tasks ADD COLUMN end_time TEXT;
    ",
];

const TASK_COLUMNS: &str = "id, parent_id, kind, title, notes, priority, due_date, due_time, \
                            completed_at, created_at, list_id, deleted_at, repeat, wont_do_at, reminders, end_date, end_time";

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
             ORDER BY completed_at IS NOT NULL, completed_at DESC, sort_order, rowid DESC"
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
            wont_do_at: None,
            end_date: None,
            end_time: None,
            reminders: Vec::new(),
            created_at: now_ms(),
        };
        // New tasks go to the top, as in TickTick.
        self.conn.execute(
            "INSERT INTO tasks (id, title, created_at, sort_order)
             VALUES (?1, ?2, ?3, (SELECT COALESCE(MIN(sort_order), 0) - 1024 FROM tasks))",
            params![task.id.to_string(), task.title, task.created_at],
        )?;
        Ok(task)
    }

    /// Completes or reopens a task. Completing a repeating task with a due date does not close
    /// it: a completed copy is kept for history, and the task moves to its next occurrence with
    /// its checklist reset — the way TickTick handles repeats.
    /// Moves a task right before (or after) `target` in the manual order, then renumbers the
    /// order with gaps. Renumbering everything keeps the code simple; personal task lists are
    /// small enough for that.
    pub fn move_task(&self, id: Id, target: Id, after: bool) -> Result<()> {
        if id == target {
            return Ok(());
        }
        let mut stmt = self.conn.prepare("SELECT id FROM tasks ORDER BY sort_order, rowid DESC")?;
        let mut order: Vec<String> = stmt.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
        let (id, target) = (id.to_string(), target.to_string());
        order.retain(|other| *other != id);
        let index = order.iter().position(|other| *other == target).context("target task not found")?;
        order.insert(if after { index + 1 } else { index }, id);

        let tx = self.conn.unchecked_transaction()?;
        for (position, task) in order.iter().enumerate() {
            tx.execute(
                "UPDATE tasks SET sort_order = ?2 WHERE id = ?1",
                params![task, position as i64 * 1024],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Tasks (open and completed, not in the trash) due between `start` and `end`, inclusive,
    /// ordered by due date and time.
    pub fn tasks_due_between(&self, start: NaiveDate, end: NaiveDate) -> Result<Vec<Task>> {
        let mut tasks: Vec<Task> = self
            .tasks()?
            .into_iter()
            .filter(|t| {
                // Durations count on every day they cover.
                !t.is_deleted()
                    && t.due_date.is_some_and(|d| d <= end && t.end_date.unwrap_or(d).max(d) >= start)
            })
            .collect();
        // Stable sort keeps the manual order within a day; all-day tasks come first.
        tasks.sort_by_key(|t| (t.due_date, t.due_time.is_some(), t.due_time));
        Ok(tasks)
    }

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
                            ?3, created_at, list_id, NULL, NULL, NULL, NULL, end_date, end_time
                     FROM tasks WHERE id = ?1"
                ),
                params![id.to_string(), copy.to_string(), now_ms()],
            )?;
            tx.execute(
                "INSERT INTO task_tags (task_id, tag) SELECT ?2, tag FROM task_tags WHERE task_id = ?1",
                params![id.to_string(), copy.to_string()],
            )?;
            let next = repeat.next_after(due);
            // A duration moves with its start.
            let end = task.end_date.map(|end| (end + (next - due)).format(DATE_FORMAT).to_string());
            tx.execute(
                "UPDATE tasks SET due_date = ?2, end_date = ?3 WHERE id = ?1",
                params![id.to_string(), next.format(DATE_FORMAT).to_string(), end],
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

    /// Marks a task "Won't Do" (it leaves the open views like a completed task), or reopens it.
    pub fn set_wont_do(&self, id: Id, wont_do: bool) -> Result<()> {
        self.update(id, "wont_do_at", wont_do.then(now_ms))
    }

    /// Copies a task with its tags and checklist (not its subtasks) right below the original.
    pub fn duplicate_task(&self, id: Id) -> Result<Task> {
        let original = self.task(id)?.context("task not found")?;
        let copy = self.add_task(&original.title)?;
        self.conn.execute(
            "UPDATE tasks SET parent_id = o.parent_id, kind = o.kind, notes = o.notes,
                 priority = o.priority, due_date = o.due_date, due_time = o.due_time,
                 list_id = o.list_id, repeat = o.repeat, reminders = o.reminders,
                 end_date = o.end_date, end_time = o.end_time
             FROM (SELECT * FROM tasks WHERE id = ?2) AS o WHERE tasks.id = ?1",
            params![copy.id.to_string(), id.to_string()],
        )?;
        self.set_tags(copy.id, &original.tags)?;
        for item in self.checklist(id)? {
            let new = self.add_checklist_item(copy.id, &item.title, None)?;
            self.set_checklist_item_completed(new.id, item.completed)?;
        }
        self.move_task(copy.id, id, true)?;
        Ok(self.task(copy.id)?.context("copy not found")?)
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
    /// Sets the end of a task's duration; `None` makes it a plain due date again.
    pub fn set_due_end(&self, id: Id, end: Option<(NaiveDate, Option<NaiveTime>)>) -> Result<()> {
        let date = end.map(|(d, _)| d.format(DATE_FORMAT).to_string());
        let time = end.and_then(|(_, t)| t).map(|t| t.format(TIME_FORMAT).to_string());
        self.conn.execute(
            "UPDATE tasks SET end_date = ?2, end_time = ?3 WHERE id = ?1",
            params![id.to_string(), date, time],
        )?;
        Ok(())
    }

    pub fn set_reminders(&self, id: Id, reminders: &[i64]) -> Result<()> {
        self.update(id, "reminders", crate::reminder::format(reminders))
    }

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

    /// Countdowns in creation order.
    pub fn countdowns(&self) -> Result<Vec<Countdown>> {
        let mut stmt =
            self.conn.prepare("SELECT id, name, date, kind FROM countdowns ORDER BY created_at, rowid")?;
        let rows = stmt
            .query_map([], |row| {
                let date: String = row.get(2)?;
                Ok(Countdown {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    date: NaiveDate::parse_from_str(&date, DATE_FORMAT).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
                    })?,
                    kind: CountdownKind::parse(&row.get::<_, String>(3)?),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn add_countdown(&self, name: &str, date: NaiveDate, kind: CountdownKind) -> Result<Countdown> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a countdown needs a name");
        let countdown = Countdown { id: Uuid::now_v7(), name: name.to_owned(), date, kind };
        self.conn.execute(
            "INSERT INTO countdowns (id, name, date, kind, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                countdown.id.to_string(),
                countdown.name,
                date.format(DATE_FORMAT).to_string(),
                kind.as_str(),
                now_ms()
            ],
        )?;
        Ok(countdown)
    }

    pub fn delete_countdown(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM countdowns WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    pub fn habits(&self) -> Result<Vec<Habit>> {
        let mut stmt =
            self.conn.prepare("SELECT id, name, goal_amount, goal_unit, reminders FROM habits ORDER BY created_at, rowid")?;
        let habits = stmt
            .query_map([], |row| {
                let goal = row.get::<_, Option<u32>>(2)?.map(|amount| Goal { amount, unit: row.get::<_, Option<String>>(3).ok().flatten().unwrap_or_default() });
                let reminders = row
                    .get::<_, Option<String>>(4)?
                    .map(|times| {
                        times.split(',').filter_map(|t| NaiveTime::parse_from_str(t.trim(), TIME_FORMAT).ok()).collect()
                    })
                    .unwrap_or_default();
                Ok(Habit { id: parse_uuid(&row.get::<_, String>(0)?, 0)?, name: row.get(1)?, goal, reminders })
            })?
            .collect::<Result<_, _>>()?;
        Ok(habits)
    }

    pub fn add_habit(&self, name: &str) -> Result<Habit> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a habit needs a name");
        let habit = Habit { id: Uuid::now_v7(), name: name.to_owned(), goal: None, reminders: Vec::new() };
        self.conn.execute(
            "INSERT INTO habits (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![habit.id.to_string(), habit.name, now_ms()],
        )?;
        Ok(habit)
    }

    pub fn delete_habit(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM habits WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    /// Renames a habit and sets its daily goal (`None`: "Achieve it all").
    /// Sets a habit's reminder times; they are kept sorted and without duplicates.
    pub fn set_habit_reminders(&self, id: Id, times: &[NaiveTime]) -> Result<()> {
        let mut times = times.to_vec();
        times.sort();
        times.dedup();
        let text: Vec<String> = times.iter().map(|t| t.format(TIME_FORMAT).to_string()).collect();
        self.conn.execute(
            "UPDATE habits SET reminders = ?2 WHERE id = ?1",
            params![id.to_string(), (!text.is_empty()).then(|| text.join(","))],
        )?;
        Ok(())
    }

    pub fn update_habit(&self, id: Id, name: &str, goal: Option<&Goal>) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a habit needs a name");
        if let Some(goal) = goal {
            anyhow::ensure!(goal.amount > 0, "a goal needs an amount");
        }
        self.conn.execute(
            "UPDATE habits SET name = ?2, goal_amount = ?3, goal_unit = ?4 WHERE id = ?1",
            params![id.to_string(), name, goal.map(|g| g.amount), goal.map(|g| g.unit.trim())],
        )?;
        Ok(())
    }

    /// Checks a habit in for `date` (reaching its goal at once), or removes the check-in when it
    /// is already there.
    pub fn toggle_checkin(&self, habit: Id, date: NaiveDate) -> Result<bool> {
        let date = date.format(DATE_FORMAT).to_string();
        let removed = self.conn.execute(
            "DELETE FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
            params![habit.to_string(), date],
        )?;
        if removed == 0 {
            self.conn.execute(
                "INSERT INTO habit_checkins (habit_id, date, value)
                 VALUES (?1, ?2, (SELECT COALESCE(goal_amount, 1) FROM habits WHERE id = ?1))",
                params![habit.to_string(), date],
            )?;
        }
        Ok(removed == 0)
    }

    /// One more unit towards a habit's daily goal (TickTick's check-in with an amount). A day that
    /// already reached the goal starts over from nothing. Returns the new amount.
    pub fn step_checkin(&self, habit: Id, date: NaiveDate) -> Result<u32> {
        let date = date.format(DATE_FORMAT).to_string();
        let goal: u32 = self.conn.query_row(
            "SELECT COALESCE(goal_amount, 1) FROM habits WHERE id = ?1",
            [habit.to_string()],
            |row| row.get(0),
        )?;
        let current: u32 = self
            .conn
            .query_row(
                "SELECT value FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                params![habit.to_string(), date],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let next = if current >= goal { 0 } else { current + 1 };
        if next == 0 {
            self.conn.execute(
                "DELETE FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                params![habit.to_string(), date],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO habit_checkins (habit_id, date, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(habit_id, date) DO UPDATE SET value = excluded.value",
                params![habit.to_string(), date, next],
            )?;
        }
        Ok(next)
    }

    /// Writes the note of an existing check-in; an empty note removes it.
    pub fn set_checkin_note(&self, habit: Id, date: NaiveDate, note: &str) -> Result<()> {
        let note = note.trim();
        let updated = self.conn.execute(
            "UPDATE habit_checkins SET note = ?3 WHERE habit_id = ?1 AND date = ?2",
            params![habit.to_string(), date.format(DATE_FORMAT).to_string(), (!note.is_empty()).then_some(note)],
        )?;
        anyhow::ensure!(updated == 1, "a note needs a check-in on that day");
        Ok(())
    }

    /// The days a habit was completed: checked in, and with a goal, the goal reached.
    pub fn checkins(&self, habit: Id) -> Result<std::collections::HashSet<NaiveDate>> {
        let goal = self.habits()?.into_iter().find(|h| h.id == habit).and_then(|h| h.goal).map_or(1, |g| g.amount);
        Ok(self.checkin_records(habit)?.into_iter().filter(|c| c.value >= goal).map(|c| c.date).collect())
    }

    /// A habit's check-ins with their amounts and notes, oldest first.
    pub fn checkin_records(&self, habit: Id) -> Result<Vec<Checkin>> {
        let mut stmt =
            self.conn.prepare("SELECT date, value, note FROM habit_checkins WHERE habit_id = ?1 ORDER BY date")?;
        let records = stmt
            .query_map([habit.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?, row.get::<_, Option<String>>(2)?))
            })?
            .filter_map(|r| {
                let (date, value, note) = r.ok()?;
                Some(Checkin { date: NaiveDate::parse_from_str(&date, DATE_FORMAT).ok()?, value, note })
            })
            .collect();
        Ok(records)
    }

    /// Records a finished focus session.
    pub fn add_focus_session(&self, task_id: Option<Id>, title: &str, started_at: i64, ended_at: i64) -> Result<FocusSession> {
        anyhow::ensure!(ended_at >= started_at, "a focus session cannot end before it starts");
        let session = FocusSession { id: Uuid::now_v7(), task_id, title: title.trim().to_owned(), started_at, ended_at };
        self.conn.execute(
            "INSERT INTO focus_sessions (id, task_id, title, started_at, ended_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session.id.to_string(), task_id.map(|t| t.to_string()), session.title, started_at, ended_at],
        )?;
        Ok(session)
    }

    /// Focus sessions, newest first.
    pub fn focus_sessions(&self) -> Result<Vec<FocusSession>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, title, started_at, ended_at FROM focus_sessions ORDER BY started_at DESC",
        )?;
        let sessions = stmt
            .query_map([], |row| {
                Ok(FocusSession {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    task_id: row.get::<_, Option<String>>(1)?.map(|t| parse_uuid(&t, 1)).transpose()?,
                    title: row.get(2)?,
                    started_at: row.get(3)?,
                    ended_at: row.get(4)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(sessions)
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
    // "Won't Do" tasks only show in their own view (and in search and the trash).
    if task.is_wont_do() && !matches!(view, View::WontDo | View::Search(_) | View::Trash) {
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
        View::WontDo => task.is_wont_do(),
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
        wont_do_at: row.get(13)?,
        reminders: row.get::<_, Option<String>>(14)?.map(|r| crate::reminder::parse(&r)).unwrap_or_default(),
        end_date: row.get::<_, Option<String>>(15)?.and_then(|d| NaiveDate::parse_from_str(&d, DATE_FORMAT).ok()),
        end_time: row.get::<_, Option<String>>(16)?.and_then(|t| NaiveTime::parse_from_str(&t, TIME_FORMAT).ok()),
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
    fn manual_order() -> Result<()> {
        let store = Store::open_in_memory()?;
        let a = store.add_task("a")?;
        let b = store.add_task("b")?;
        let c = store.add_task("c")?;
        assert_eq!(titles(&store.tasks()?), ["c", "b", "a"]);

        store.move_task(c.id, a.id, true)?;
        assert_eq!(titles(&store.tasks()?), ["b", "a", "c"]);
        store.move_task(a.id, b.id, false)?;
        assert_eq!(titles(&store.tasks()?), ["a", "b", "c"]);

        // New tasks still land on top after renumbering.
        store.add_task("d")?;
        assert_eq!(titles(&store.tasks()?), ["d", "a", "b", "c"]);
        assert!(store.move_task(a.id, Uuid::now_v7(), false).is_err());
        Ok(())
    }

    #[test]
    fn tasks_due_in_a_range() -> Result<()> {
        let store = Store::open_in_memory()?;
        let evening = store.add_task("evening")?;
        let morning = store.add_task("morning")?;
        let all_day = store.add_task("all day")?;
        let outside = store.add_task("outside")?;
        let trashed = store.add_task("trashed")?;
        store.set_due(evening.id, Some(d(2026, 10, 20)), NaiveTime::from_hms_opt(19, 0, 0))?;
        store.set_due(morning.id, Some(d(2026, 10, 20)), NaiveTime::from_hms_opt(8, 0, 0))?;
        store.set_due(all_day.id, Some(d(2026, 10, 20)), None)?;
        store.set_due(outside.id, Some(d(2026, 11, 2)), None)?;
        store.set_due(trashed.id, Some(d(2026, 10, 21)), None)?;
        store.delete_task(trashed.id)?;
        store.set_completed(morning.id, true)?;

        let tasks = store.tasks_due_between(d(2026, 10, 1), d(2026, 10, 31))?;
        assert_eq!(titles(&tasks), ["all day", "morning", "evening"]);
        Ok(())
    }

    #[test]
    fn countdowns_crud() -> Result<()> {
        let store = Store::open_in_memory()?;
        let trip = store.add_countdown(" Trip ", d(2027, 1, 1), CountdownKind::Countdown)?;
        store.add_countdown("Moved in", d(2025, 3, 1), CountdownKind::Anniversary)?;
        assert!(store.add_countdown(" ", d(2027, 1, 1), CountdownKind::Holiday).is_err());
        let all = store.countdowns()?;
        assert_eq!(all.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Trip", "Moved in"]);
        assert_eq!(all[1].kind, CountdownKind::Anniversary);
        store.delete_countdown(trip.id)?;
        assert_eq!(store.countdowns()?.len(), 1);
        Ok(())
    }

    #[test]
    fn habits_and_checkins() -> Result<()> {
        let store = Store::open_in_memory()?;
        let floss = store.add_habit(" Floss ")?;
        let read = store.add_habit("Read")?;
        assert!(store.add_habit("").is_err());
        assert!(store.toggle_checkin(floss.id, d(2026, 10, 17))?);
        assert!(store.toggle_checkin(floss.id, d(2026, 10, 18))?);
        assert!(!store.toggle_checkin(floss.id, d(2026, 10, 18))?);
        assert_eq!(store.checkins(floss.id)?, [d(2026, 10, 17)].into());
        assert!(store.checkins(read.id)?.is_empty());
        store.delete_habit(floss.id)?;
        assert_eq!(store.habits()?.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["Read"]);
        Ok(())
    }

    #[test]
    fn durations_count_on_every_day() -> Result<()> {
        let store = Store::open_in_memory()?;
        let trip = store.add_task("trip")?;
        store.set_due(trip.id, Some(d(2026, 10, 20)), None)?;
        store.set_due_end(trip.id, Some((d(2026, 10, 23), None)))?;
        let trip = store.task(trip.id)?.unwrap();
        assert!(!trip.occurs_on(d(2026, 10, 19)));
        assert!(trip.occurs_on(d(2026, 10, 20)) && trip.occurs_on(d(2026, 10, 22)) && trip.occurs_on(d(2026, 10, 23)));
        assert!(!trip.occurs_on(d(2026, 10, 24)));
        // A week view starting after the trip began still shows it.
        assert_eq!(store.tasks_due_between(d(2026, 10, 22), d(2026, 10, 28))?.len(), 1);
        assert!(store.tasks_due_between(d(2026, 10, 24), d(2026, 10, 30))?.is_empty());
        Ok(())
    }

    #[test]
    fn durations_move_with_repeats() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("conference")?;
        let t = |h| NaiveTime::from_hms_opt(h, 0, 0).unwrap();
        store.set_due(task.id, Some(d(2026, 10, 20)), Some(t(9)))?;
        store.set_due_end(task.id, Some((d(2026, 10, 22), Some(t(17)))))?;
        let stored = store.task(task.id)?.unwrap();
        assert_eq!((stored.end_date, stored.end_time), (Some(d(2026, 10, 22)), Some(t(17))));
        assert_eq!(store.duplicate_task(task.id)?.end_date, Some(d(2026, 10, 22)));
        store.set_repeat(task.id, Some(crate::Recurrence::new(crate::Frequency::Weekly, 1)), d(2026, 10, 18))?;
        store.set_completed(task.id, true)?;
        let next = store.task(task.id)?.unwrap();
        assert_eq!((next.due_date, next.end_date), (Some(d(2026, 10, 27)), Some(d(2026, 10, 29))));
        store.set_due_end(task.id, None)?;
        assert_eq!(store.task(task.id)?.unwrap().end_date, None);
        Ok(())
    }

    #[test]
    fn reminders_are_stored_and_kept_by_repeats() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("water the plants")?;
        store.set_due(task.id, Some(d(2026, 10, 20)), None)?;
        store.set_repeat(task.id, Some(crate::Recurrence::new(crate::Frequency::Daily, 1)), d(2026, 10, 18))?;
        store.set_reminders(task.id, &[0, 24 * 60])?;
        assert_eq!(store.task(task.id)?.unwrap().reminders, [0, 24 * 60]);
        assert_eq!(store.duplicate_task(task.id)?.reminders, [0, 24 * 60]);
        // Completing a repeating task keeps the reminders on the next occurrence only.
        store.set_completed(task.id, true)?;
        let tasks = store.tasks()?;
        let done = tasks.iter().find(|t| t.is_completed()).unwrap();
        assert!(done.reminders.is_empty());
        assert_eq!(store.task(task.id)?.unwrap().reminders, [0, 24 * 60]);
        store.set_reminders(task.id, &[])?;
        assert!(store.task(task.id)?.unwrap().reminders.is_empty());
        Ok(())
    }

    #[test]
    fn habit_goals_and_notes() -> Result<()> {
        let store = Store::open_in_memory()?;
        let water = store.add_habit("Water")?;
        let goal = Goal { amount: 8, unit: "glasses".into() };
        store.update_habit(water.id, "Drink water", Some(&goal))?;
        assert!(store.update_habit(water.id, "Drink water", Some(&Goal { amount: 0, unit: String::new() })).is_err());
        let habit = store.habits()?.remove(0);
        assert_eq!((habit.name.as_str(), habit.goal), ("Drink water", Some(goal)));

        store.toggle_checkin(water.id, d(2026, 10, 18))?;
        assert!(store.set_checkin_note(water.id, d(2026, 10, 17), "no check-in").is_err());
        store.set_checkin_note(water.id, d(2026, 10, 18), " Felt great ")?;
        let records = store.checkin_records(water.id)?;
        assert_eq!(records, [Checkin { date: d(2026, 10, 18), value: 8, note: Some("Felt great".into()) }]);

        // Partial amounts count towards the goal but do not complete the day.
        let day = d(2026, 10, 17);
        assert_eq!(store.step_checkin(water.id, day)?, 1);
        assert_eq!(store.step_checkin(water.id, day)?, 2);
        assert_eq!(store.checkins(water.id)?, [d(2026, 10, 18)].into());
        for _ in 2..8 {
            store.step_checkin(water.id, day)?;
        }
        assert_eq!(store.checkins(water.id)?, [d(2026, 10, 17), d(2026, 10, 18)].into());
        assert_eq!(store.step_checkin(water.id, day)?, 0, "a completed day starts over");

        store.update_habit(water.id, "Drink water", None)?;
        assert_eq!(store.habits()?[0].goal, None);

        let t = |h| NaiveTime::from_hms_opt(h, 0, 0).unwrap();
        store.set_habit_reminders(water.id, &[t(21), t(9), t(21)])?;
        assert_eq!(store.habits()?[0].reminders, [t(9), t(21)]);
        store.set_habit_reminders(water.id, &[])?;
        assert!(store.habits()?[0].reminders.is_empty());
        Ok(())
    }

    #[test]
    fn focus_sessions_are_stored_newest_first() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("write report")?;
        store.add_focus_session(Some(task.id), "write report", 1_000, 1_500_000)?;
        store.add_focus_session(None, "reading", 2_000_000, 3_500_000)?;
        assert!(store.add_focus_session(None, "x", 10, 5).is_err());
        let sessions = store.focus_sessions()?;
        assert_eq!(sessions.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["reading", "write report"]);
        assert_eq!(sessions[1].task_id, Some(task.id));
        // Deleting the task keeps the record.
        store.purge_task(task.id)?;
        assert_eq!(store.focus_sessions()?[1].task_id, None);
        Ok(())
    }

    #[test]
    fn wont_do_leaves_open_views() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = d(2026, 10, 18);
        let task = store.add_task("learn the banjo")?;
        store.set_due(task.id, Some(today), None)?;
        store.set_wont_do(task.id, true)?;
        assert!(store.tasks_in(&View::Inbox, today)?.is_empty());
        assert!(store.tasks_in(&View::Today, today)?.is_empty());
        assert!(store.tasks_in(&View::Completed, today)?.is_empty());
        assert_eq!(store.tasks_in(&View::WontDo, today)?.len(), 1);
        assert_eq!(store.counts(today)?.today, 0);
        store.set_wont_do(task.id, false)?;
        assert_eq!(store.tasks_in(&View::Today, today)?.len(), 1);
        Ok(())
    }

    #[test]
    fn duplicate_copies_fields_tags_and_checklist() -> Result<()> {
        let store = Store::open_in_memory()?;
        let list = store.create_list("Home")?;
        let task = store.add_task("pack")?;
        store.set_list(task.id, Some(list.id))?;
        store.set_priority(task.id, Priority::High)?;
        store.set_tags(task.id, &["trip".into()])?;
        let item = store.add_checklist_item(task.id, "socks", None)?;
        store.set_checklist_item_completed(item.id, true)?;
        let other = store.add_task("other")?;

        let copy = store.duplicate_task(task.id)?;
        assert_ne!(copy.id, task.id);
        assert_eq!((copy.title.as_str(), copy.priority, copy.list_id), ("pack", Priority::High, Some(list.id)));
        assert_eq!(copy.tags, ["trip"]);
        assert_eq!(copy.checklist, Some((1, 1)));
        // The copy sits right below the original.
        assert_eq!(titles(&store.tasks()?), ["other", "pack", "pack"]);
        let _ = other;
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
