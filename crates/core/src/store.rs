use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use chrono::{Duration, NaiveDate, NaiveTime};
use rusqlite::{Connection, OptionalExtension, Row, params};
use uuid::Uuid;

use crate::model::{Id, Kind, Priority, Tag, Task, View, ViewCounts};

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
];

const TASK_COLUMNS: &str =
    "id, parent_id, kind, title, notes, priority, due_date, due_time, completed_at, created_at";

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

    /// Все задачи: сначала невыполненные (новые сверху), затем выполненные (недавние сверху).
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
        for task in &mut tasks {
            if let Some(task_tags) = tags.remove(&task.id) {
                task.tags = task_tags;
            }
        }
        Ok(tasks)
    }

    /// Задачи для представления. `today` передаётся снаружи, чтобы не зависеть от часового пояса в тестах.
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
        Ok(Some(task))
    }

    pub fn add_task(&self, title: &str) -> Result<Task> {
        let task = Task {
            id: Uuid::now_v7(),
            parent_id: None,
            kind: Kind::Task,
            title: title.trim().to_owned(),
            notes: String::new(),
            priority: Priority::None,
            due_date: None,
            due_time: None,
            tags: Vec::new(),
            completed_at: None,
            created_at: now_ms(),
        };
        self.conn.execute(
            "INSERT INTO tasks (id, title, created_at) VALUES (?1, ?2, ?3)",
            params![task.id.to_string(), task.title, task.created_at],
        )?;
        Ok(task)
    }

    pub fn set_completed(&self, id: Id, completed: bool) -> Result<()> {
        self.update(id, "completed_at", completed.then(now_ms))
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

    /// Срок задачи. Время без даты не имеет смысла и игнорируется.
    pub fn set_due(&self, id: Id, date: Option<NaiveDate>, time: Option<NaiveTime>) -> Result<()> {
        let date_str = date.map(|d| d.format(DATE_FORMAT).to_string());
        let time_str = date.and(time).map(|t| t.format(TIME_FORMAT).to_string());
        self.conn.execute(
            "UPDATE tasks SET due_date = ?2, due_time = ?3 WHERE id = ?1",
            params![id.to_string(), date_str, time_str],
        )?;
        Ok(())
    }

    /// Делает задачу подзадачей `parent` (или поднимает на верхний уровень при `None`).
    pub fn set_parent(&self, id: Id, parent: Option<Id>) -> Result<()> {
        if let Some(parent) = parent {
            anyhow::ensure!(
                parent != id && !self.descendants(id)?.contains(&parent),
                "задачу нельзя вложить в саму себя"
            );
        }
        self.update(id, "parent_id", parent.map(|p| p.to_string()))
    }

    /// Заменяет набор тегов задачи; новые теги создаются автоматически.
    /// Теги сравниваются без учёта регистра (в том числе кириллица — `NOCASE` в SQLite её не понимает).
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
             GROUP BY t.name
             ORDER BY t.created_at, t.name",
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

    /// Удаляет задачу вместе с подзадачами.
    pub fn delete_task(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id.to_string()])?;
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
    let due_before = |days: i64| task.due_date.is_some_and(|due| due < today + Duration::days(days));
    match view {
        // Пока нет пользовательских списков, во «Входящих» лежит всё.
        View::Inbox | View::All => true,
        View::Today => !task.is_completed() && due_before(1),
        View::Tomorrow => {
            !task.is_completed() && task.due_date == Some(today + Duration::days(1))
        }
        View::Next7Days => !task.is_completed() && due_before(7),
        View::Completed => task.is_completed(),
        View::Tag(tag) => task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()),
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
        completed_at: row.get(8)?,
        created_at: row.get(9)?,
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
        let a = store.add_task("  купить молоко ")?;
        let b = store.add_task("позвонить маме")?;
        assert_eq!(a.title, "купить молоко");

        store.set_completed(a.id, true)?;
        let tasks = store.tasks()?;
        assert_eq!(tasks.iter().map(|t| t.id).collect::<Vec<_>>(), [b.id, a.id]);
        assert!(store.task(a.id)?.unwrap().is_completed());

        store.set_completed(a.id, false)?;
        assert!(!store.task(a.id)?.unwrap().is_completed());

        store.set_title(b.id, "позвонить папе")?;
        store.set_notes(b.id, "после 18:00\nне забыть про выходные")?;
        let b2 = store.task(b.id)?.unwrap();
        assert_eq!(b2.title, "позвонить папе");
        assert_eq!(b2.notes, "после 18:00\nне забыть про выходные");

        store.delete_task(b.id)?;
        assert_eq!(store.tasks()?.len(), 1);
        Ok(())
    }

    #[test]
    fn reopen_keeps_data() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("amin-test-{}", Uuid::now_v7()));
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
            "INSERT INTO tasks VALUES ('0192f000-0000-7000-8000-000000000001', 'старая', NULL, 1)",
            [],
        )?;
        let store = Store::init(conn)?;
        let task = &store.tasks()?[0];
        assert_eq!(
            (task.title.as_str(), task.kind, task.priority),
            ("старая", Kind::Task, Priority::None)
        );
        Ok(())
    }

    #[test]
    fn priority_kind_and_due() -> Result<()> {
        let store = Store::open_in_memory()?;
        let t = store.add_task("отчёт")?;
        store.set_priority(t.id, Priority::High)?;
        store.set_kind(t.id, Kind::Note)?;
        store.set_due(t.id, Some(d(2026, 10, 20)), NaiveTime::from_hms_opt(18, 30, 0))?;
        let t = store.task(t.id)?.unwrap();
        assert_eq!(t.priority, Priority::High);
        assert_eq!(t.kind, Kind::Note);
        assert_eq!(t.due_date, Some(d(2026, 10, 20)));
        assert_eq!(t.due_time, NaiveTime::from_hms_opt(18, 30, 0));

        // Время без даты не сохраняется.
        store.set_due(t.id, None, NaiveTime::from_hms_opt(9, 0, 0))?;
        let t = store.task(t.id)?.unwrap();
        assert_eq!((t.due_date, t.due_time), (None, None));
        Ok(())
    }

    #[test]
    fn smart_views_and_counts() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = d(2026, 10, 18);
        let overdue = store.add_task("просрочено")?;
        let now = store.add_task("сегодня")?;
        let tomorrow = store.add_task("завтра")?;
        let week = store.add_task("через 6 дней")?;
        let later = store.add_task("через неделю")?;
        let done = store.add_task("сделано сегодня")?;
        store.add_task("без срока")?;
        store.set_due(overdue.id, Some(d(2026, 10, 15)), None)?;
        store.set_due(now.id, Some(today), None)?;
        store.set_due(tomorrow.id, Some(d(2026, 10, 19)), None)?;
        store.set_due(week.id, Some(d(2026, 10, 24)), None)?;
        store.set_due(later.id, Some(d(2026, 10, 25)), None)?;
        store.set_due(done.id, Some(today), None)?;
        store.set_completed(done.id, true)?;

        assert_eq!(titles(&store.tasks_in(&View::Today, today)?), ["сегодня", "просрочено"]);
        assert_eq!(titles(&store.tasks_in(&View::Tomorrow, today)?), ["завтра"]);
        assert_eq!(
            titles(&store.tasks_in(&View::Next7Days, today)?),
            ["через 6 дней", "завтра", "сегодня", "просрочено"]
        );
        assert_eq!(titles(&store.tasks_in(&View::Completed, today)?), ["сделано сегодня"]);
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
        store.set_tags(a.id, &["Дом".into(), "проект".into()])?;
        store.set_tags(b.id, &["дом".into()])?;
        store.set_tag_color("Дом", Some("#4AA6EF"))?;

        assert_eq!(store.task(b.id)?.unwrap().tags, ["Дом"]);
        let tags = store.tags()?;
        assert_eq!(
            tags.iter().map(|t| (t.name.as_str(), t.open_count)).collect::<Vec<_>>(),
            [("Дом", 2), ("проект", 1)]
        );
        assert_eq!(tags[0].color.as_deref(), Some("#4AA6EF"));

        store.set_completed(b.id, true)?;
        assert_eq!(store.tags()?[0].open_count, 1);
        assert_eq!(titles(&store.tasks_in(&View::Tag("дом".into()), d(2026, 1, 1))?), ["a", "b"]);

        store.set_tags(a.id, &[])?;
        assert!(store.task(a.id)?.unwrap().tags.is_empty());
        Ok(())
    }

    #[test]
    fn subtasks_cascade_and_no_cycles() -> Result<()> {
        let store = Store::open_in_memory()?;
        let parent = store.add_task("распродать технику")?;
        let child = store.add_task("продать планшет")?;
        let grandchild = store.add_task("сбросить настройки")?;
        store.set_parent(child.id, Some(parent.id))?;
        store.set_parent(grandchild.id, Some(child.id))?;
        assert_eq!(store.task(child.id)?.unwrap().parent_id, Some(parent.id));

        assert!(store.set_parent(parent.id, Some(grandchild.id)).is_err());
        assert!(store.set_parent(parent.id, Some(parent.id)).is_err());

        store.delete_task(parent.id)?;
        assert!(store.tasks()?.is_empty());
        Ok(())
    }
}
