use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use web_time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::horizon::Horizon;
use crate::model::{Id, Kind, Priority, ScheduleBucket, Task, View, ViewCounts};

/// Maximum nesting depth for subtasks (`0..=4`, i.e. 5 levels total).
pub const MAX_SUBTASK_DEPTH: usize = 4;

macro_rules! task_columns {
    () => {
        "id, parent_id, kind, title, notes, priority, due_date, due_time, \
         completed_at, created_at, list_id, deleted_at, repeat, wont_do_at, reminders, end_date, end_time, modified_at, \
         pinned_at, section_id, estimate, in_progress_at, location, timezone, is_floating, snoozed_until, constant_reminder, \
         start_date, schedule_bucket, horizon"
    };
}

mod backups;
mod batch;
mod calendar_sync;
mod focus;
mod habits;
mod lists_tags;
mod migrations;
mod task_mutations;
mod templates_filters;
mod tree_sections;

pub use backups::{BackupEntry, TableCounts, list_backups};
pub use migrations::{SCHEMA_VERSION, apply_migrations_up_to};
#[cfg(test)]
pub(crate) use migrations::init_legacy_db_before_filter_rules;

use migrations::retry_sqlite_busy;

/// Drop target when dragging a list folder relative to another list or folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderDropTarget {
    List(Id),
    Folder(Id),
}

const SELECT_ALL_TASKS_SQL: &str = concat!(
    "SELECT ",
    task_columns!(),
    " FROM tasks ORDER BY completed_at IS NOT NULL, completed_at DESC, sort_order, rowid DESC"
);

const SELECT_TASK_BY_ID_SQL: &str =
    concat!("SELECT ", task_columns!(), " FROM tasks WHERE id = ?1");

const SELECT_TASKS_DUE_BETWEEN_SQL: &str = concat!(
    "SELECT ",
    task_columns!(),
    " FROM tasks WHERE deleted_at IS NULL AND due_date IS NOT NULL AND due_date <= ?2 AND (due_date >= ?1 OR end_date >= ?1) ",
    "ORDER BY completed_at IS NOT NULL, completed_at DESC, sort_order, rowid DESC"
);

#[derive(Clone, Copy)]
pub(super) enum SortTable {
    Lists,
    ListSections,
    ListFolders,
}

impl SortTable {
    const fn update_sql(self) -> &'static str {
        match self {
            Self::Lists => "UPDATE lists SET sort_order = ?2 WHERE id = ?1 AND sort_order != ?2",
            Self::ListSections => {
                "UPDATE list_sections SET sort_order = ?2 WHERE id = ?1 AND sort_order != ?2"
            }
            Self::ListFolders => {
                "UPDATE list_folders SET sort_order = ?2 WHERE id = ?1 AND sort_order != ?2"
            }
        }
    }
}

pub(crate) const DATE_FORMAT: &str = "%Y-%m-%d";
pub(super) const TIME_FORMAT: &str = "%H:%M";
pub(super) const SNOOZE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

pub struct Store {
    pub(crate) conn: Connection,
    attachments_dir: Option<PathBuf>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let attachments_dir = path.as_ref().parent().map(|parent| parent.join("attachments"));
        Self::init(Connection::open(path)?, attachments_dir)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    /// Opens a writable in-memory database hydrated from a previously serialized SQLite image.
    pub fn open_from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.deserialize_read_exact(
            rusqlite::MAIN_DB,
            std::io::Cursor::new(bytes),
            bytes.len(),
            false,
        )?;
        Self::init(conn, None)
    }

    /// Exports the current database into a standalone SQLite byte image.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut copy = Connection::open_in_memory()?;
        {
            let backup = rusqlite::backup::Backup::new(&self.conn, &mut copy)?;
            backup.step(-1)?;
        }
        let data = copy.serialize(rusqlite::MAIN_DB)?;
        Ok(data.to_vec())
    }

    /// Overrides the attachment folder used when purging tasks/lists unlinks orphaned files.
    pub fn with_attachments_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.attachments_dir = Some(dir.into());
        self
    }

    /// The attachment folder associated with this store, if any.
    pub fn attachments_dir(&self) -> Option<&Path> {
        self.attachments_dir.as_deref()
    }

    /// Runs `f` inside a transaction, or a nested `SAVEPOINT` when already inside an outer
    /// transaction. Automatically rolls back on error or panic.
    pub(crate) fn in_tx<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        if self.conn.is_autocommit() {
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            let result = f()?;
            tx.commit()?;
            Ok(result)
        } else {
            static NEXT_SP: AtomicU64 = AtomicU64::new(1);
            let name = format!("sp_{}", NEXT_SP.fetch_add(1, Ordering::Relaxed));
            self.conn.execute_batch(&format!("SAVEPOINT {name}"))?;
            struct Guard<'a> {
                conn: &'a Connection,
                name: String,
                committed: bool,
            }
            impl Drop for Guard<'_> {
                fn drop(&mut self) {
                    if !self.committed {
                        let _ = self.conn.execute_batch(&format!(
                            "ROLLBACK TO SAVEPOINT {0}; RELEASE SAVEPOINT {0}",
                            self.name
                        ));
                    }
                }
            }
            let mut guard = Guard { conn: &self.conn, name, committed: false };
            let result = f()?;
            self.conn.execute_batch(&format!("RELEASE SAVEPOINT {}", guard.name))?;
            guard.committed = true;
            Ok(result)
        }
    }

    pub(super) fn write_sort_order(&self, table: SortTable, ids: &[Id]) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(table.update_sql())?;
        for (i, id) in ids.iter().enumerate() {
            stmt.execute(params![id.to_string(), i as i64])?;
        }
        Ok(())
    }

    fn init(conn: Connection, attachments_dir: Option<PathBuf>) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // Switching journal_mode to WAL on a freshly created or rollback-journal DB requires an
        // exclusive lock; concurrent Store::open calls can receive immediate SQLITE_BUSY while
        // promoting from SHARED to EXCLUSIVE. Skip when already WAL/memory and retry briefly on BUSY.
        retry_sqlite_busy(|| {
            let mode: std::result::Result<String, rusqlite::Error> =
                conn.pragma_query_value(None, "journal_mode", |row| row.get(0));
            if matches!(
                mode.as_deref(),
                Ok(m) if m.eq_ignore_ascii_case("wal") || m.eq_ignore_ascii_case("memory")
            ) {
                return Ok(());
            }
            conn.pragma_update(None, "journal_mode", "WAL")
        })?;
        // With WAL, a commit waits for the disk only at checkpoints; a power cut can lose the
        // last changes but never corrupts the file.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // Keep temp B-trees in memory and raise page cache from 2 MiB default to 8 MiB (negative = KiB).
        // Intentionally do NOT enable mmap_size (causes file-locking hazards on Windows backup/restore/rename).
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.pragma_update(None, "cache_size", -8000_i64)?;
        // The views run the same queries on every change; parse each once.
        conn.set_prepared_statement_cache_capacity(128);
        Self::migrate(&conn)?;
        Ok(Self { conn, attachments_dir })
    }

    fn migrate(conn: &Connection) -> Result<()> {
        apply_migrations_up_to(conn, SCHEMA_VERSION)
    }

    /// All tasks: open ones first (newest on top), then completed ones (most recent on top).
    pub fn tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare_cached(SELECT_ALL_TASKS_SQL)?;
        let mut tasks: Vec<Task> = stmt.query_map([], task_from_row)?.collect::<Result<_, _>>()?;
        self.hydrate_tasks_with(&mut tasks, None)?;
        Ok(tasks)
    }

    /// Hydrates tags and checklist progress for a slice of tasks, querying only matched IDs.
    pub(super) fn hydrate_tasks(&self, tasks: &mut [Task]) -> Result<()> {
        match tasks {
            [] => Ok(()),
            [task] => self.hydrate_single_task(task),
            many => {
                let ids: Vec<String> = many.iter().map(|t| t.id.to_string()).collect();
                let ids_json = serde_json::to_string(&ids)?;
                self.hydrate_tasks_with(many, Some(&ids_json))
            }
        }
    }

    fn hydrate_single_task(&self, task: &mut Task) -> Result<()> {
        let id_str = task.id.to_string();
        let mut stmt =
            self.conn.prepare_cached("SELECT tag FROM task_tags WHERE task_id = ?1 ORDER BY tag")?;
        task.tags = stmt.query_map([&id_str], |row| row.get(0))?.collect::<Result<_, _>>()?;
        let (done, total): (i64, i64) = self
            .conn
            .prepare_cached("SELECT COUNT(completed_at), COUNT(*) FROM checklist_items WHERE task_id = ?1")?
            .query_row([&id_str], |row| Ok((row.get(0)?, row.get(1)?)))?;
        task.checklist = (total > 0).then_some((done as usize, total as usize));
        Ok(())
    }

    fn hydrate_tasks_with(&self, tasks: &mut [Task], ids_json: Option<&str>) -> Result<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        let mut tags: HashMap<Id, Vec<String>> = HashMap::new();
        let mut stmt = self.conn.prepare_cached(if ids_json.is_some() {
            "SELECT task_id, tag FROM task_tags WHERE task_id IN (SELECT value FROM json_each(?1)) ORDER BY tag"
        } else {
            "SELECT task_id, tag FROM task_tags ORDER BY tag"
        })?;
        for row in stmt.query_map(rusqlite::params_from_iter(ids_json), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            let (task_id, tag) = row?;
            if let Ok(id) = Uuid::parse_str(&task_id) {
                tags.entry(id).or_default().push(tag);
            }
        }
        let progress = self.checklist_progress(ids_json)?;
        for task in tasks {
            if let Some(task_tags) = tags.remove(&task.id) {
                task.tags = task_tags;
            }
            task.checklist = progress.get(&task.id).copied();
        }
        Ok(())
    }

    /// Tasks for a view. `today` is passed in so tests do not depend on the clock or time zone.
    pub fn tasks_in(&self, view: &View, today: NaiveDate) -> Result<Vec<Task>> {
        self.tasks_in_from(self.tasks()?, view, today)
    }

    /// [`Self::tasks_in`] out of `tasks`, all of them as [`Self::tasks`] loaded them: views that
    /// need several selections load the tasks once.
    pub fn tasks_in_from(&self, tasks: Vec<Task>, view: &View, today: NaiveDate) -> Result<Vec<Task>> {
        self.tasks_in_from_with_today_completed(tasks, view, today, false)
    }

    /// Includes tasks due today or completed today in the Today view when requested.
    pub fn tasks_in_from_with_today_completed(
        &self, tasks: Vec<Task>, view: &View, today: NaiveDate, show_completed: bool,
    ) -> Result<Vec<Task>> {
        if let View::Filter(id) = view {
            let filter = self.filters()?.into_iter().find(|f| f.id == *id).context("filter not found")?;
            let week_start = self.preferences()?.week_start;
            return Ok(tasks.into_iter().filter(|task| filter.matches(task, today, week_start)).collect());
        }
        if let View::Tag(tag) = view {
            let family = self.tag_family(tag)?;
            return Ok(tasks
                .into_iter()
                .filter(|task| matches_tag_family(task, &family))
                .collect());
        }
        let hidden = if is_smart(view) { self.hidden_lists()? } else { HashSet::new() };
        Ok(tasks
            .into_iter()
            .filter(|task| {
                if task.list_id.is_some_and(|l| hidden.contains(&l)) {
                    return false;
                }
                if show_completed && task.is_completed() && !task.is_deleted() && !task.is_wont_do() {
                    let completed_today = task
                        .completed_at
                        .and_then(chrono::DateTime::from_timestamp_millis)
                        .is_some_and(|at| at.with_timezone(&chrono::Local).date_naive() == today);
                    match view {
                        View::Today => {
                            return task.due_date == Some(today) || completed_today;
                        }
                        View::Tomorrow => {
                            return task.due_date == Some(today + Duration::days(1));
                        }
                        View::Next7Days => {
                            return task.due_date.is_some_and(|d| d >= today && d < today + Duration::days(7))
                                || task.horizon.is_some_and(|h| {
                                    matches!(h, Horizon::Week { .. }) && h == Horizon::current_week(today)
                                })
                                || completed_today;
                        }
                        _ => {}
                    }
                }
                matches_view(task, view, today)
            })
            .collect())
    }

    pub fn counts(&self, today: NaiveDate) -> Result<ViewCounts> {
        self.counts_from(&self.tasks()?, today)
    }

    /// [`Self::counts`] out of `tasks`, all of them as [`Self::tasks`] loaded them.
    pub fn counts_from(&self, tasks: &[Task], today: NaiveDate) -> Result<ViewCounts> {
        let hidden = self.hidden_lists()?;
        let tomorrow = today + Duration::days(1);
        let next_7_end = today + Duration::days(7);
        let current_week = Horizon::current_week(today);
        let mut counts = ViewCounts::default();

        for task in tasks {
            if task.is_completed() || task.is_deleted() || task.is_wont_do() {
                continue;
            }
            if task.list_id.is_none() {
                counts.inbox += 1;
            }
            if task.list_id.is_some_and(|l| hidden.contains(&l)) {
                continue;
            }
            counts.all += 1;
            if let Some(due) = task.due_date {
                if due < tomorrow {
                    counts.today += 1;
                }
                if due == tomorrow {
                    counts.tomorrow += 1;
                }
                if due < next_7_end {
                    counts.next_7_days += 1;
                    continue;
                }
            }
            if task
                .horizon
                .is_some_and(|h| matches!(h, Horizon::Week { .. }) && h <= current_week)
            {
                counts.next_7_days += 1;
            }
        }
        Ok(counts)
    }

    pub fn task(&self, id: Id) -> Result<Option<Task>> {
        let id_str = id.to_string();
        let task = self
            .conn
            .prepare_cached(SELECT_TASK_BY_ID_SQL)?
            .query_row([&id_str], task_from_row)
            .optional()?;
        let Some(mut task) = task else { return Ok(None) };
        self.hydrate_single_task(&mut task)?;
        Ok(Some(task))
    }

    /// Tasks (open and completed, not in the trash) due between `start` and `end`, inclusive,
    /// ordered by due date and time.
    pub fn tasks_due_between(&self, start: NaiveDate, end: NaiveDate) -> Result<Vec<Task>> {
        let start_s = start.format(DATE_FORMAT).to_string();
        let end_s = end.format(DATE_FORMAT).to_string();
        let mut stmt = self.conn.prepare_cached(SELECT_TASKS_DUE_BETWEEN_SQL)?;
        let mut tasks: Vec<Task> = stmt
            .query_map(params![start_s, end_s], task_from_row)?
            .collect::<Result<_, _>>()?;
        // Durations count on every day they cover; re-check parsed dates against bounds.
        tasks.retain(|t| t.due_date.is_some_and(|d| d <= end && t.end_date.unwrap_or(d).max(d) >= start));
        self.hydrate_tasks(&mut tasks)?;
        // Stable sort keeps the manual order within a day; all-day tasks come first.
        tasks.sort_by_key(|t| (t.due_date, t.due_time.is_some(), t.due_time));
        Ok(tasks)
    }

    /// "List Activities": what happened to the tasks of a list (`None` is the inbox), newest
    /// first, with each task's id and title.
    pub fn list_activity(&self, list: Option<Id>, limit: usize) -> Result<Vec<(crate::model::Activity, Id, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.at, a.kind, t.id, t.title FROM task_activity a JOIN tasks t ON t.id = a.task_id
             WHERE t.list_id IS ?1 ORDER BY a.at DESC, a.rowid DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![list.map(|l| l.to_string()), limit as i64], |row| {
                Ok((
                    crate::model::Activity { at: row.get(0)?, kind: row.get(1)? },
                    parse_uuid(&row.get::<_, String>(2)?, 2)?,
                    row.get(3)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// A task's activity, oldest first.
    pub fn task_activity(&self, task: Id) -> Result<Vec<crate::model::Activity>> {
        let mut stmt =
            self.conn.prepare_cached("SELECT at, kind FROM task_activity WHERE task_id = ?1 ORDER BY at, rowid")?;
        let activity = stmt
            .query_map([task.to_string()], |row| {
                Ok(crate::model::Activity { at: row.get(0)?, kind: row.get(1)? })
            })?
            .collect::<Result<_, _>>()?;
        Ok(activity)
    }

    pub(super) fn update(&self, id: Id, column: &'static str, value: impl rusqlite::ToSql) -> Result<()> {
        self.conn.execute(
            &format!("UPDATE tasks SET {column} = ?2 WHERE id = ?1"),
            params![id.to_string(), value],
        )?;
        Ok(())
    }
}

/// The smart lists a list can be kept out of with "Show in Smart List".
fn is_smart(view: &View) -> bool {
    matches!(view, View::All | View::Today | View::Tomorrow | View::Next7Days | View::Day(_))
}

/// Checks whether `task` belongs to a `View::Tag` whose lowercased tag family is `family`.
/// Preserves exact [`matches_view`] semantics for `View::Tag` (excludes deleted and won't-do tasks,
/// keeps completed tasks, and matches tag names case-insensitively without cloning family strings).
fn matches_tag_family(task: &Task, family: &HashSet<String>) -> bool {
    if task.is_deleted() || task.is_wont_do() {
        return false;
    }
    task.tags.iter().any(|t| {
        if family.contains(t.as_str()) {
            return true;
        }
        if t.is_ascii() && !t.bytes().any(|b| b.is_ascii_uppercase()) {
            return false;
        }
        family.contains(&t.to_lowercase())
    })
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
        View::Next7Days => {
            !task.is_completed()
                && (due_before(7)
                    || task.horizon.is_some_and(|h| {
                        matches!(h, Horizon::Week { .. }) && h <= Horizon::current_week(today)
                    }))
        }
        View::Completed => task.is_completed(),
        View::WontDo => task.is_wont_do(),
        View::Trash => true,
        View::Tag(tag) => task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()),
        // Filters are matched in `Store::tasks_in`, which knows their conditions.
        View::Filter(_) | View::Horizons | View::Summary => false,
        View::Day(day) => task.due_date == Some(*day),
        View::Search(query) => {
            let query = query.trim().to_lowercase();
            !query.is_empty()
                && (task.title.to_lowercase().contains(&query)
                    || task.notes.to_lowercase().contains(&query)
                    || task.location.as_deref().is_some_and(|loc| loc.to_lowercase().contains(&query))
                    || task.tags.iter().any(|t| t.to_lowercase().contains(&query)))
        }
    }
}

pub(crate) fn parse_uuid(value: &str, column: usize) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(e))
    })
}

pub(super) fn task_from_row(row: &Row) -> rusqlite::Result<Task> {
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
        modified_at: row.get(17)?,
        pinned_at: row.get(18)?,
        section_id: row.get::<_, Option<String>>(19)?.map(|s| parse_uuid(&s, 19)).transpose()?,
        estimate: row.get::<_, Option<String>>(20)?.and_then(|e| crate::model::Estimate::from_storage(&e)),
        in_progress_at: row.get(21)?,
        location: row.get(22)?,
        timezone: row.get(23)?,
        is_floating: row.get::<_, i64>(24)? != 0,
        snoozed_until: row.get::<_, Option<String>>(25)?.and_then(|s| NaiveDateTime::parse_from_str(&s, SNOOZE_FORMAT).ok()),
        constant_reminder: row.get::<_, i64>(26)? != 0,
        start_date: row.get::<_, Option<String>>(27)?.and_then(|d| NaiveDate::parse_from_str(&d, DATE_FORMAT).ok()),
        schedule_bucket: ScheduleBucket::parse(&row.get::<_, String>(28)?),
        horizon: row.get::<_, Option<String>>(29)?.and_then(|h| Horizon::parse(&h)),
    })
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests;
