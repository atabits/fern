use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike as _};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::attachment::PreparedAttachment;
use crate::countdown::{Countdown, CountdownKind};
use crate::horizon::Horizon;
use crate::model::{Attachment, ChecklistItem, Id, Kind, List, ListDraft, ListFolder, Priority, ScheduleBucket, Tag, Task, TaskDraft, View, ViewCounts};
use crate::recurrence::Recurrence;

/// Maximum nesting depth for subtasks (`0..=4`, i.e. 5 levels total).
pub const MAX_SUBTASK_DEPTH: usize = 4;


mod backups;
mod batch;
mod calendar_sync;
mod focus;
mod habits;
mod migrations;

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

const TAG_ORDER_KEY: &str = "tag_order";
const FILTER_ORDER_KEY: &str = "filter_order";

macro_rules! task_columns {
    () => {
        "id, parent_id, kind, title, notes, priority, due_date, due_time, \
         completed_at, created_at, list_id, deleted_at, repeat, wont_do_at, reminders, end_date, end_time, modified_at, \
         pinned_at, section_id, estimate, in_progress_at, location, timezone, is_floating, snoozed_until, constant_reminder, \
         start_date, schedule_bucket, horizon"
    };
}

const SELECT_ALL_TASKS_SQL: &str = concat!(
    "SELECT ",
    task_columns!(),
    " FROM tasks ORDER BY completed_at IS NOT NULL, completed_at DESC, sort_order, rowid DESC"
);

const SELECT_TASK_BY_ID_SQL: &str =
    concat!("SELECT ", task_columns!(), " FROM tasks WHERE id = ?1");

const INSERT_REPEAT_SNAPSHOT_SQL: &str = concat!(
    "INSERT INTO tasks (",
    task_columns!(),
    ") SELECT ?2, parent_id, kind, title, notes, priority, due_date, due_time, \
       ?3, created_at, list_id, NULL, NULL, NULL, NULL, end_date, end_time, ?3, NULL, \
       section_id, estimate, NULL, location, timezone, is_floating, NULL, 0, \
       start_date, schedule_bucket, horizon \
     FROM tasks WHERE id = ?1"
);

macro_rules! insert_task_full_sql {
    ($order:literal) => {
        concat!(
            "INSERT INTO tasks (id, title, notes, list_id, kind, priority, due_date, due_time, \
             repeat, reminders, created_at, end_date, end_time, location, timezone, is_floating, \
             constant_reminder, start_date, schedule_bucket, horizon, sort_order) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ",
            $order,
            ")"
        )
    };
}

macro_rules! insert_task_basic_sql {
    ($order:literal) => {
        concat!(
            "INSERT INTO tasks (id, title, notes, list_id, kind, priority, due_date, due_time, \
             repeat, reminders, created_at, end_date, end_time, sort_order) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ",
            $order,
            ")"
        )
    };
}

const INSERT_TASK_FULL_TOP_SQL: &str =
    insert_task_full_sql!("(SELECT COALESCE(MIN(sort_order), 0) - 1024 FROM tasks)");
const INSERT_TASK_FULL_BOTTOM_SQL: &str =
    insert_task_full_sql!("(SELECT COALESCE(MAX(sort_order), 0) + 1024 FROM tasks)");
const INSERT_TASK_BASIC_TOP_SQL: &str =
    insert_task_basic_sql!("(SELECT COALESCE(MIN(sort_order), 0) - 1024 FROM tasks)");
const INSERT_TASK_BASIC_BOTTOM_SQL: &str =
    insert_task_basic_sql!("(SELECT COALESCE(MAX(sort_order), 0) + 1024 FROM tasks)");

#[derive(Clone, Copy)]
enum SortTable {
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
const TIME_FORMAT: &str = "%H:%M";
const SNOOZE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

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

    fn write_sort_order(&self, table: SortTable, ids: &[Id]) -> Result<()> {
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

        let mut tags: HashMap<Id, Vec<String>> = HashMap::new();
        let mut stmt = self.conn.prepare_cached("SELECT task_id, tag FROM task_tags ORDER BY tag")?;
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

    /// Saved filters, in custom sidebar order (falling back to creation order).
    pub fn filters(&self) -> Result<Vec<crate::filter::SmartFilter>> {
        use crate::filter::{ConditionValue as V, DateRule, FilterRules, SmartFilter};
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, lists, tags, date, priorities, keywords, kind, match_any, chain, rules FROM filters ORDER BY created_at, rowid",
        )?;
        let mut filters: Vec<SmartFilter> = stmt.query_map([], |row| {
            let read = || -> Result<SmartFilter> {
                let rules = match row.get::<_, Option<String>>(10)? {
                    Some(text) => FilterRules::from_storage(&text)?,
                    None => {
                        let lists: String = row.get(2)?;
                        let tags: String = row.get(3)?;
                        let priorities: String = row.get(5)?;
                        let date = DateRule::from_key(&row.get::<_, String>(4)?).unwrap_or(DateRule::All);
                        let values = vec![
                            V::Lists(lists.split(',').filter(|s| !s.is_empty()).map(|s| Uuid::parse_str(s).ok()).collect()),
                            V::Tags { names: tags.lines().filter(|s| !s.is_empty()).map(str::to_owned).collect(), untagged: false },
                            V::Date(if date == DateRule::All { vec![] } else { vec![date] }),
                            V::Priority(priorities.split(',').filter_map(|s| s.parse().ok()).map(Priority::from_i64).collect()),
                            V::Include(row.get(6)?),
                            V::Type(row.get::<_, Option<String>>(7)?.map(|k| Kind::parse(&k))),
                        ];
                        crate::filter::from_legacy(values, row.get(8)?, &row.get::<_, String>(9)?)?
                    }
                };
                Ok(SmartFilter { id: Uuid::parse_str(&row.get::<_, String>(0)?)?, name: row.get(1)?, rules })
            };
            read().map_err(|e| rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, e.into()))
        })?.collect::<Result<_, _>>()?;
        if let Some(saved) = self.setting(FILTER_ORDER_KEY)? {
            let mut ranks: HashMap<Id, usize> = HashMap::new();
            for (i, id) in saved.split(',').filter_map(|s| Uuid::parse_str(s.trim()).ok()).enumerate() {
                ranks.entry(id).or_insert(i);
            }
            filters.sort_by_key(|f| ranks.get(&f.id).copied().unwrap_or(usize::MAX));
        }
        Ok(filters)
    }

    /// Adds or replaces a filter. Legacy columns remain for reading older databases and sync rows.
    pub fn save_filter(&self, filter: &crate::filter::SmartFilter) -> Result<()> {
        anyhow::ensure!(!filter.name.trim().is_empty(), "a filter needs a name");
        let rules = filter.rules.to_storage()?;
        self.conn.execute(
            "INSERT INTO filters (id, name, lists, tags, date, priorities, keywords, kind, created_at, match_any, chain, rules)
             VALUES (?1, ?2, '', '', 'all', '', '', NULL, ?3, 0, '', ?4)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, rules = excluded.rules",
            params![filter.id.to_string(), filter.name.trim(), now_ms(), rules],
        )?;
        Ok(())
    }

    /// Persists the saved-filter display order in a single write.
    pub fn set_filter_order(&self, order: &[Id]) -> Result<()> {
        let value: Vec<String> = order.iter().map(Id::to_string).collect();
        self.set_setting(FILTER_ORDER_KEY, &value.join(","))?;
        Ok(())
    }

    /// Reorders a saved filter before (`!after`) or after (`after`) `target`.
    pub fn move_filter(&self, dragged: Id, target: Id, after: bool) -> Result<()> {
        if dragged == target {
            return Ok(());
        }
        let mut order: Vec<Id> = self.filters()?.into_iter().map(|f| f.id).collect();
        let Some(from) = order.iter().position(|&id| id == dragged) else { return Ok(()) };
        order.remove(from);
        let Some(to) = order.iter().position(|&id| id == target) else { return Ok(()) };
        order.insert(to + usize::from(after), dragged);
        self.set_filter_order(&order)
    }

    /// Moves a saved filter to the top of the Filters section.
    pub fn move_filter_first(&self, dragged: Id) -> Result<()> {
        let mut order: Vec<Id> = self.filters()?.into_iter().map(|f| f.id).collect();
        if !order.contains(&dragged) {
            return Ok(());
        }
        order.retain(|&id| id != dragged);
        order.insert(0, dragged);
        self.set_filter_order(&order)
    }

    pub fn delete_filter(&self, id: Id) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute("DELETE FROM filters WHERE id = ?1", [id.to_string()])?;
            if let Some(saved) = self.setting(FILTER_ORDER_KEY)? {
                let id_str = id.to_string();
                let updated: Vec<&str> = saved
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && *s != id_str)
                    .collect();
                self.set_setting(FILTER_ORDER_KEY, &updated.join(","))?;
            }
            Ok(())
        })
    }

    pub fn counts(&self, today: NaiveDate) -> Result<ViewCounts> {
        self.counts_from(&self.tasks()?, today)
    }

    /// [`Self::counts`] out of `tasks`, all of them as [`Self::tasks`] loaded them.
    pub fn counts_from(&self, tasks: &[Task], today: NaiveDate) -> Result<ViewCounts> {
        let hidden = self.hidden_lists()?;
        let count = |view: View| {
            let skip = |t: &Task| is_smart(&view) && t.list_id.is_some_and(|l| hidden.contains(&l));
            tasks.iter().filter(|t| !t.is_completed() && !skip(t) && matches_view(t, &view, today)).count()
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
        let id_str = id.to_string();
        let task = self
            .conn
            .prepare_cached(SELECT_TASK_BY_ID_SQL)?
            .query_row([&id_str], task_from_row)
            .optional()?;
        let Some(mut task) = task else { return Ok(None) };
        let mut stmt =
            self.conn.prepare_cached("SELECT tag FROM task_tags WHERE task_id = ?1 ORDER BY tag")?;
        task.tags = stmt.query_map([&id_str], |row| row.get(0))?.collect::<Result<_, _>>()?;
        let (done, total): (i64, i64) = self
            .conn
            .prepare_cached("SELECT COUNT(completed_at), COUNT(*) FROM checklist_items WHERE task_id = ?1")?
            .query_row([&id_str], |row| Ok((row.get(0)?, row.get(1)?)))?;
        task.checklist = (total > 0).then_some((done as usize, total as usize));
        Ok(Some(task))
    }

    pub fn add_task(&self, title: &str) -> Result<Task> {
        self.add_task_at(title, true)
    }

    /// Adds a task at the top of the lists, or at the bottom ("Default Add to" in the settings).
    pub fn add_task_at(&self, title: &str, top: bool) -> Result<Task> {
        self.create_task(TaskDraft { title: title.to_owned(), ..Default::default() }, top)
    }

    /// Creates the task, tags and checklist atomically so a failed submission can be retried.
    pub fn create_task(&self, draft: TaskDraft, top: bool) -> Result<Task> {
        self.in_tx(|| self.insert_task_draft(draft, top))
    }

    fn insert_task_draft(&self, mut draft: TaskDraft, top: bool) -> Result<Task> {
        let tags = std::mem::take(&mut draft.tags);
        let checklist = std::mem::take(&mut draft.checklist);
        let section = draft.section_id;
        let task = self.insert_task(draft, top)?;
        if section.is_none() && tags.is_empty() && checklist.is_empty() {
            return Ok(task);
        }
        if section.is_some() {
            self.set_section(task.id, section)?;
        }
        if !tags.is_empty() {
            self.replace_task_tags(task.id, &tags)?;
        }
        for (position, (title, completed)) in checklist.into_iter().enumerate() {
            self.conn.execute(
                "INSERT INTO checklist_items (id, task_id, title, sort_order, completed_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![Uuid::now_v7().to_string(), task.id.to_string(), title.trim(), position as i64, completed.then(now_ms), now_ms()],
            )?;
        }
        self.task(task.id)?.context("created task not found")
    }

    /// Keeps staged files available for retry and rolls back the task if any file cannot be
    /// published. The caller owns the staged files until the whole draft is committed.
    pub fn create_task_with_attachments(&self, draft: TaskDraft, top: bool, attachments: &mut [PreparedAttachment], dir: &Path) -> Result<Task> {
        if attachments.is_empty() {
            return self.create_task(draft, top);
        }
        std::fs::create_dir_all(dir)?;
        let result = self.in_tx(|| {
            let task = self.insert_task_draft(draft, top)?;
            for prepared in attachments.iter() {
                let attachment = prepared.metadata();
                self.conn.execute(
                    "INSERT INTO task_attachments (id, task_id, name, size, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![attachment.id.to_string(), task.id.to_string(), attachment.name, attachment.size as i64, attachment.created_at],
                )?;
            }
            for prepared in attachments.iter_mut() {
                prepared.publish(dir)?;
            }
            Ok(task)
        });
        for prepared in attachments {
            if result.is_ok() {
                prepared.keep_publication();
            } else if let Err(error) = prepared.rollback_publication() {
                eprintln!("failed to roll back attachment publication: {error:#}");
            }
        }
        result
    }

    fn insert_task(&self, draft: TaskDraft, top: bool) -> Result<Task> {
        let location = draft.location.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
        let timezone = draft
            .timezone
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && chrono_tz::Tz::from_str(s).is_ok())
            .map(str::to_owned);
        let horizon = if draft.due_date.is_some() || draft.start_date.is_some() {
            None
        } else {
            draft.horizon
        };
        let schedule_bucket = if horizon.is_some() {
            ScheduleBucket::None
        } else {
            draft.schedule_bucket
        };
        let task = Task {
            id: Uuid::now_v7(),
            list_id: draft.list_id,
            parent_id: None,
            kind: draft.kind,
            title: draft.title.trim().to_owned(),
            notes: draft.notes,
            priority: draft.priority,
            due_date: draft.due_date,
            due_time: draft.due_date.and(draft.due_time),
            repeat: draft.repeat,
            tags: Vec::new(),
            checklist: None,
            completed_at: None,
            deleted_at: None,
            wont_do_at: None,
            in_progress_at: None,
            end_date: draft.due_date.and(draft.end_date),
            end_time: draft.due_date.and(draft.end_date).and(draft.end_time),
            location,
            timezone,
            is_floating: draft.is_floating,
            snoozed_until: None,
            constant_reminder: draft.constant_reminder,
            modified_at: None,
            pinned_at: None,
            section_id: None,
            reminders: draft.reminders,
            estimate: None,
            start_date: draft.start_date,
            schedule_bucket,
            horizon,
            created_at: now_ms(),
        };
        if task.location.is_some()
            || task.timezone.is_some()
            || task.is_floating
            || task.constant_reminder
            || task.start_date.is_some()
            || task.schedule_bucket != ScheduleBucket::None
            || task.horizon.is_some()
        {
            let sql = if top { INSERT_TASK_FULL_TOP_SQL } else { INSERT_TASK_FULL_BOTTOM_SQL };
            self.conn.prepare_cached(sql)?.execute(params![
                task.id.to_string(), task.title, task.notes, task.list_id.map(|id| id.to_string()),
                task.kind.as_str(), task.priority.to_i64(),
                task.due_date.map(|date| date.format(DATE_FORMAT).to_string()),
                task.due_time.map(|time| time.format(TIME_FORMAT).to_string()),
                task.repeat.as_ref().map(ToString::to_string), crate::reminder::format(&task.reminders),
                task.created_at,
                task.end_date.map(|date| date.format(DATE_FORMAT).to_string()),
                task.end_time.map(|time| time.format(TIME_FORMAT).to_string()),
                task.location,
                task.timezone,
                task.is_floating,
                task.constant_reminder,
                task.start_date.map(|date| date.format(DATE_FORMAT).to_string()),
                task.schedule_bucket.as_str(),
                task.horizon.map(|h| h.as_code()),
            ])?;
        } else {
            let sql = if top { INSERT_TASK_BASIC_TOP_SQL } else { INSERT_TASK_BASIC_BOTTOM_SQL };
            self.conn.prepare_cached(sql)?.execute(params![
                task.id.to_string(), task.title, task.notes, task.list_id.map(|id| id.to_string()),
                task.kind.as_str(), task.priority.to_i64(),
                task.due_date.map(|date| date.format(DATE_FORMAT).to_string()),
                task.due_time.map(|time| time.format(TIME_FORMAT).to_string()),
                task.repeat.as_ref().map(ToString::to_string), crate::reminder::format(&task.reminders),
                task.created_at,
                task.end_date.map(|date| date.format(DATE_FORMAT).to_string()),
                task.end_time.map(|time| time.format(TIME_FORMAT).to_string()),
            ])?;
        }
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
        self.in_tx(|| {
            let mut stmt = self.conn.prepare_cached("SELECT id FROM tasks ORDER BY sort_order, rowid DESC")?;
            let mut order: Vec<String> = stmt.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
            let (id, target) = (id.to_string(), target.to_string());
            order.retain(|other| *other != id);
            let index = order.iter().position(|other| *other == target).context("target task not found")?;
            order.insert(if after { index + 1 } else { index }, id);

            let mut update = self
                .conn
                .prepare_cached("UPDATE tasks SET sort_order = ?2 WHERE id = ?1 AND sort_order != ?2")?;
            for (position, task) in order.iter().enumerate() {
                update.execute(params![task, position as i64 * 1024])?;
            }
            Ok(())
        })
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

    /// Creates a subtask in the parent's list and section (and inherits closed state if the parent is closed).
    pub fn add_subtask(&self, parent: Id, title: &str) -> Result<Task> {
        self.in_tx(|| {
            let parent_task = self.task(parent)?.ok_or_else(|| anyhow::anyhow!("parent task not found"))?;
            anyhow::ensure!(!parent_task.is_deleted(), "trashed tasks cannot have subtasks");
            anyhow::ensure!(parent_task.kind != Kind::Note, "notes cannot have subtasks");
            anyhow::ensure!(self.task_depth(parent)? < MAX_SUBTASK_DEPTH, "maximum subtask nesting depth reached");
            let task = Task {
                id: Uuid::now_v7(),
                list_id: parent_task.list_id,
                parent_id: Some(parent),
                kind: Kind::Task,
                title: title.trim().to_owned(),
                notes: String::new(),
                priority: Priority::None,
                due_date: None,
                due_time: None,
                repeat: None,
                tags: Vec::new(),
                checklist: None,
                completed_at: parent_task.completed_at,
                deleted_at: None,
                wont_do_at: parent_task.wont_do_at,
                in_progress_at: None,
                end_date: None,
                end_time: None,
                location: None,
                timezone: None,
                is_floating: false,
                snoozed_until: None,
                constant_reminder: false,
                modified_at: None,
                pinned_at: None,
                section_id: parent_task.section_id,
                reminders: Vec::new(),
                estimate: None,
                start_date: None,
                schedule_bucket: ScheduleBucket::None,
                horizon: None,
                created_at: now_ms(),
            };
            self.conn.execute(
                "INSERT INTO tasks (id, parent_id, title, notes, list_id, section_id, kind, priority,
                                    completed_at, wont_do_at, created_at, sort_order)
                 VALUES (?1, ?2, ?3, '', ?4, ?5, 'task', 0, ?6, ?7, ?8,
                         (SELECT COALESCE(MIN(sort_order), 0) - 1024 FROM tasks))",
                params![
                    task.id.to_string(),
                    parent.to_string(),
                    task.title,
                    task.list_id.map(|l| l.to_string()),
                    task.section_id.map(|s| s.to_string()),
                    task.completed_at,
                    task.wont_do_at,
                    task.created_at,
                ],
            )?;
            Ok(task)
        })
    }

    pub fn set_completed(&self, id: Id, completed: bool) -> Result<()> {
        self.complete(id, completed).map(|_| ())
    }

    /// Completes or reopens a task. Completing a repeating task keeps a completed copy and moves
    /// the task to its next date; the copy is returned so that it can be undone.
    pub fn complete(&self, id: Id, completed: bool) -> Result<Option<Id>> {
        self.complete_with_cascaded(id, completed).map(|(copy, _)| copy)
    }

    fn snapshot_repeat_copy(&self, id: Id) -> Result<Id> {
        let copy = Uuid::now_v7();
        self.conn
            .prepare_cached(INSERT_REPEAT_SNAPSHOT_SQL)?
            .execute(params![id.to_string(), copy.to_string(), now_ms()])?;
        self.conn.execute(
            "INSERT INTO task_tags (task_id, tag) SELECT ?2, tag FROM task_tags WHERE task_id = ?1",
            params![id.to_string(), copy.to_string()],
        )?;
        // The completed occurrence owns a snapshot of the checklist. Besides preserving
        // its history, this lets Undo restore the exact checked state transactionally.
        let items = {
            let mut statement = self.conn.prepare(
                "SELECT id, title, sort_order, completed_at, created_at FROM checklist_items WHERE task_id = ?1",
            )?;
            statement
                .query_map([id.to_string()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (source_id, title, order, completed_at, created_at) in items {
            let copy_id = Uuid::now_v7().to_string();
            self.conn.execute(
                "INSERT INTO checklist_items (id, task_id, title, sort_order, completed_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![copy_id, copy.to_string(), title, order, completed_at, created_at],
            )?;
            self.conn.execute(
                "INSERT INTO repeat_checklist_copies (copy_id, source_id) VALUES (?1, ?2)",
                params![copy_id, source_id],
            )?;
        }
        Ok(copy)
    }

    /// Completes or reopens a task, cascading completion to open descendants and reopening closed
    /// ancestors when a subtask is reopened. Returns the repeating copy (if
    /// any) and the list of descendants newly completed alongside `id`.
    pub fn complete_with_cascaded(&self, id: Id, completed: bool) -> Result<(Option<Id>, Vec<Id>)> {
        if completed && let Some(task) = self.task(id)? {
            if let (Some(repeat), Some(anchor_date)) = (task.repeat.clone(), task.due_date.or(task.start_date)) {
                let today = chrono::Local::now().date_naive();
                let holidays = if repeat.skip_holidays {
                    let anchor = if repeat.from_completion { today } else { anchor_date };
                    self.holiday_dates(anchor, anchor + Duration::days(400))?
                } else {
                    HashSet::new()
                };
                if let Some(next) = repeat.next_for_with_holidays(anchor_date, today, |d| holidays.contains(&d)) {
                    let copy = self.in_tx(|| {
                        let copy = self.snapshot_repeat_copy(id)?;
                        // Shift due_date, end_date, and start_date together by the same recurrence delta.
                        let delta = next - anchor_date;
                        let next_due = task.due_date.map(|d| (d + delta).format(DATE_FORMAT).to_string());
                        let next_end = task.end_date.map(|end| (end + delta).format(DATE_FORMAT).to_string());
                        let next_start = task.start_date.map(|start| (start + delta).format(DATE_FORMAT).to_string());
                        let next_bucket = if task.start_date.is_none()
                            && task.schedule_bucket == ScheduleBucket::Evening
                        {
                            ScheduleBucket::None
                        } else {
                            task.schedule_bucket
                        };
                        let next_repeat = repeat.forgetting_curve.then(|| repeat.advance().to_string());
                        self.conn.execute(
                            "UPDATE tasks SET due_date = ?2, end_date = ?3, start_date = ?4, schedule_bucket = ?5,
                                    snoozed_until = NULL, in_progress_at = NULL,
                                    repeat = COALESCE(?6, repeat) WHERE id = ?1",
                            params![
                                id.to_string(),
                                next_due,
                                next_end,
                                next_start,
                                next_bucket.as_str(),
                                next_repeat,
                            ],
                        )?;
                        self.conn.execute(
                            "UPDATE checklist_items SET completed_at = NULL WHERE task_id = ?1",
                            [id.to_string()],
                        )?;
                        Ok(copy)
                    })?;
                    return Ok((Some(copy), Vec::new()));
                }
            } else if task.due_date.is_none()
                && task.start_date.is_none()
                && let (Some(repeat), Some(horizon)) = (task.repeat.clone(), task.horizon)
                && let Some(next_horizon) = horizon.advance_for_recurrence(&repeat)
            {
                let copy = self.in_tx(|| {
                    let copy = self.snapshot_repeat_copy(id)?;
                    self.conn.execute(
                        "UPDATE tasks SET horizon = ?2, snoozed_until = NULL, in_progress_at = NULL WHERE id = ?1",
                        params![id.to_string(), next_horizon.as_code()],
                    )?;
                    self.conn.execute(
                        "UPDATE checklist_items SET completed_at = NULL WHERE task_id = ?1",
                        [id.to_string()],
                    )?;
                    Ok(copy)
                })?;
                return Ok((Some(copy), Vec::new()));
            }
        }
        if completed {
            let cascaded = self.close_with_descendants(id, true)?;
            Ok((None, cascaded))
        } else {
            self.reopen_tasks(&[id])?;
            Ok((None, Vec::new()))
        }
    }

    /// Marks `id` and all open non-deleted descendants completed (`completed = true`) or Won't Do
    /// (`completed = false`) atomically, returning the newly cascaded descendant IDs.
    fn close_with_descendants(&self, id: Id, completed: bool) -> Result<Vec<Id>> {
        self.in_tx(|| {
            let now = now_ms();
            let mut stmt = self.conn.prepare_cached(
                "WITH RECURSIVE sub(id) AS (
                     SELECT id FROM tasks WHERE parent_id = ?1
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )
                 SELECT t.id FROM tasks t JOIN sub ON t.id = sub.id
                 WHERE t.deleted_at IS NULL AND t.completed_at IS NULL AND t.wont_do_at IS NULL",
            )?;
            let open_descendants: Vec<Id> = stmt
                .query_map([id.to_string()], |row| {
                    let raw: String = row.get(0)?;
                    parse_uuid(&raw, 0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let sql = if completed {
                "UPDATE tasks SET completed_at = ?2, wont_do_at = NULL, in_progress_at = NULL, snoozed_until = NULL WHERE id = ?1"
            } else {
                "UPDATE tasks SET wont_do_at = ?2, completed_at = NULL, in_progress_at = NULL, snoozed_until = NULL WHERE id = ?1"
            };
            let mut update = self.conn.prepare_cached(sql)?;
            update.execute(params![id.to_string(), now])?;
            for &sub in &open_descendants {
                update.execute(params![sub.to_string(), now])?;
            }
            Ok(open_descendants)
        })
    }

    /// Reopens an explicit set of tasks (used by Undo after a cascading completion) together with
    /// any non-deleted closed ancestors so an open subtask is never left under a closed parent.
    pub fn reopen_tasks(&self, ids: &[Id]) -> Result<()> {
        self.in_tx(|| {
            let mut to_reopen: HashSet<Id> = ids.iter().copied().collect();
            let mut anc_stmt = self.conn.prepare_cached(
                "SELECT deleted_at IS NULL AND (completed_at IS NOT NULL OR wont_do_at IS NOT NULL)
                 FROM tasks WHERE id = ?1",
            )?;
            for &id in ids {
                for anc in self.ancestors(id)? {
                    if !to_reopen.contains(&anc)
                        && anc_stmt
                            .query_row([anc.to_string()], |row| row.get::<_, bool>(0))
                            .optional()?
                            .unwrap_or(false)
                    {
                        to_reopen.insert(anc);
                    }
                }
            }
            let mut update = self
                .conn
                .prepare_cached("UPDATE tasks SET completed_at = NULL, wont_do_at = NULL WHERE id = ?1")?;
            for id in to_reopen {
                update.execute([id.to_string()])?;
            }
            Ok(())
        })
    }

    /// Undoes completing a repeating task: removes the completed copy and puts the task back on
    /// its earlier dates or horizon (and repeat rule if stateful, e.g. forgetting curve).
    pub fn undo_repeat(
        &self,
        id: Id,
        copy: Id,
        due: Option<NaiveDate>,
        end: Option<NaiveDate>,
        repeat: Option<&Recurrence>,
    ) -> Result<()> {
        self.undo_repeat_inner(id, copy, due, end, None, repeat)
    }

    /// Like [`Self::undo_repeat`], also explicitly restoring `horizon` even if the completed copy
    /// row is no longer present.
    pub fn undo_repeat_with_horizon(
        &self,
        id: Id,
        copy: Id,
        due: Option<NaiveDate>,
        end: Option<NaiveDate>,
        horizon: Option<Horizon>,
        repeat: Option<&Recurrence>,
    ) -> Result<()> {
        self.undo_repeat_inner(id, copy, due, end, Some(horizon), repeat)
    }

    fn undo_repeat_inner(
        &self,
        id: Id,
        copy: Id,
        due: Option<NaiveDate>,
        end: Option<NaiveDate>,
        explicit_horizon: Option<Option<Horizon>>,
        repeat: Option<&Recurrence>,
    ) -> Result<()> {
        self.in_tx(|| {
            let parent_alive: bool = self
                .conn
                .query_row(
                    "SELECT 1 FROM tasks WHERE id = ?1 AND deleted_at IS NULL",
                    [id.to_string()],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            if !parent_alive {
                return Ok(());
            }
            let copy_schedule: Option<(Option<String>, String, Option<String>)> = self
                .conn
                .query_row(
                    "SELECT start_date, schedule_bucket, horizon FROM tasks WHERE id = ?1",
                    [copy.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            self.conn.execute(
                "UPDATE checklist_items AS original SET completed_at = copied.completed_at
                 FROM repeat_checklist_copies AS mapping JOIN checklist_items AS copied ON copied.id = mapping.copy_id
                 WHERE original.id = mapping.source_id AND original.task_id = ?1 AND copied.task_id = ?2",
                params![id.to_string(), copy.to_string()],
            )?;
            self.conn.execute("DELETE FROM tasks WHERE id = ?1", [copy.to_string()])?;
            if let Some((start_date, schedule_bucket, copy_horizon)) = copy_schedule {
                let restored_horizon = match explicit_horizon {
                    Some(h) => h.map(|v| v.as_code()),
                    None => copy_horizon,
                };
                self.conn.execute(
                    "UPDATE tasks SET due_date = ?2, end_date = ?3, repeat = COALESCE(?4, repeat),
                            start_date = ?5, schedule_bucket = ?6, horizon = ?7 WHERE id = ?1",
                    params![
                        id.to_string(),
                        due.map(|d| d.format(DATE_FORMAT).to_string()),
                        end.map(|d| d.format(DATE_FORMAT).to_string()),
                        repeat.map(ToString::to_string),
                        start_date,
                        schedule_bucket,
                        restored_horizon,
                    ],
                )?;
            } else if let Some(h) = explicit_horizon {
                self.conn.execute(
                    "UPDATE tasks SET due_date = ?2, end_date = ?3, repeat = COALESCE(?4, repeat),
                            horizon = ?5 WHERE id = ?1",
                    params![
                        id.to_string(),
                        due.map(|d| d.format(DATE_FORMAT).to_string()),
                        end.map(|d| d.format(DATE_FORMAT).to_string()),
                        repeat.map(ToString::to_string),
                        h.map(|v| v.as_code()),
                    ],
                )?;
            } else {
                self.conn.execute(
                    "UPDATE tasks SET due_date = ?2, end_date = ?3, repeat = COALESCE(?4, repeat) WHERE id = ?1",
                    params![
                        id.to_string(),
                        due.map(|d| d.format(DATE_FORMAT).to_string()),
                        end.map(|d| d.format(DATE_FORMAT).to_string()),
                        repeat.map(ToString::to_string),
                    ],
                )?;
            }
            Ok(())
        })
    }

    /// Sets or clears the repeat rule. A repeating task needs a due date or matching horizon;
    /// `today` is used when the task has neither.
    pub fn set_repeat(&self, id: Id, repeat: Option<Recurrence>, today: NaiveDate) -> Result<()> {
        if let Some(ref rule) = repeat
            && let Some(task) = self.task(id)?
            && task.due_date.is_none()
            && !task.horizon.is_some_and(|h| h.advance_for_recurrence(rule).is_some())
        {
            self.set_due(id, Some(today), task.due_time)?;
        }
        self.update(id, "repeat", repeat.map(|r| r.to_string()))
    }

    /// Marks a task "Won't Do" (cascading to open descendants), or reopens it (and closed ancestors).
    pub fn set_wont_do(&self, id: Id, wont_do: bool) -> Result<()> {
        if wont_do {
            self.close_with_descendants(id, false).map(|_| ())
        } else {
            self.reopen_tasks(&[id])
        }
    }

    /// Copies a task with its tags and checklist (not its subtasks) right below the original.
    pub fn duplicate_task(&self, id: Id) -> Result<Task> {
        self.in_tx(|| {
            let original = self.task(id)?.context("task not found")?;
            let copy = self.insert_task(TaskDraft { title: original.title.clone(), ..Default::default() }, true)?;
            self.conn.execute(
                "UPDATE tasks SET parent_id = o.parent_id, kind = o.kind, notes = o.notes,
                     priority = o.priority, due_date = o.due_date, due_time = o.due_time,
                     list_id = o.list_id, section_id = o.section_id, repeat = o.repeat,
                     reminders = o.reminders, end_date = o.end_date, end_time = o.end_time,
                     estimate = o.estimate, location = o.location, timezone = o.timezone,
                     is_floating = o.is_floating, constant_reminder = o.constant_reminder,
                     start_date = o.start_date, schedule_bucket = o.schedule_bucket,
                     horizon = o.horizon
                 FROM (SELECT * FROM tasks WHERE id = ?2) AS o WHERE tasks.id = ?1",
                params![copy.id.to_string(), id.to_string()],
            )?;
            self.replace_task_tags(copy.id, &original.tags)?;
            for item in self.checklist(id)? {
                let new = self.add_checklist_item(copy.id, &item.title, None)?;
                self.set_checklist_item_completed(new.id, item.completed)?;
            }
            self.move_task(copy.id, id, true)?;
            self.task(copy.id)?.context("copy not found")
        })
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
        if kind == Kind::Note {
            return self.in_tx(|| {
                if let Some(task) = self.task(id)? {
                    let has_active_subtask: bool = self.conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM tasks WHERE parent_id = ?1 AND deleted_at IS NULL)",
                        [id.to_string()],
                        |row| row.get(0),
                    )?;
                    anyhow::ensure!(
                        task.parent_id.is_none() && !has_active_subtask,
                        "nested tasks cannot be converted to notes"
                    );
                    // Detach any trashed children so restoring them later cannot attach a subtask to a note.
                    self.conn.execute(
                        "UPDATE tasks SET parent_id = NULL WHERE parent_id = ?1 AND deleted_at IS NOT NULL",
                        [id.to_string()],
                    )?;
                }
                self.update(id, "kind", kind.as_str())
            });
        }
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

    /// Moves both dates if the task has a duration, keeping their local times.
    pub fn move_dates(&self, id: Id, date: NaiveDate) -> Result<()> {
        let task = self.task(id)?.context("task not found")?;
        let end = match (task.due_date, task.end_date) {
            (Some(start), Some(end)) => Some(
                end.checked_add_signed(date - start).context("task duration is outside the calendar")?,
            ),
            _ => None,
        };
        self.conn.execute(
            "UPDATE tasks SET due_date = ?2, end_date = ?3, end_time = ?4, horizon = NULL WHERE id = ?1",
            params![
                id.to_string(),
                date.format(DATE_FORMAT).to_string(),
                end.map(|date| date.format(DATE_FORMAT).to_string()),
                end.and(task.end_time).map(|time| time.format(TIME_FORMAT).to_string()),
            ],
        )?;
        Ok(())
    }

    /// Resizes an all-day / date-span task to `[start, end]` inclusive.
    pub fn resize_dates(&self, id: Id, start: NaiveDate, end: NaiveDate) -> Result<()> {
        let task = self.task(id)?.context("task not found")?;
        let end = end.max(start);
        let stored_end_date = (end > start).then_some(end);
        let stored_end_time = if stored_end_date.is_some() {
            task.end_time
        } else {
            task.end_time.filter(|et| task.due_time.is_none_or(|st| *et > st))
        };
        self.conn.execute(
            "UPDATE tasks SET due_date = ?2, end_date = ?3, end_time = ?4, horizon = NULL WHERE id = ?1",
            params![
                id.to_string(),
                start.format(DATE_FORMAT).to_string(),
                stored_end_date.map(|d| d.format(DATE_FORMAT).to_string()),
                stored_end_time.map(|t| t.format(TIME_FORMAT).to_string()),
            ],
        )?;
        Ok(())
    }

    /// Resizes a timed task block to `[date start .. end_date end]`.
    pub fn resize_timed_range(&self, id: Id, date: NaiveDate, start: NaiveTime, end: NaiveTime, end_date: Option<NaiveDate>) -> Result<()> {
        let _ = self.task(id)?.context("task not found")?;
        let stored_end_date = end_date.filter(|d| *d > date);
        let end = if stored_end_date.is_none() && end <= start {
            let mins = (start.hour() * 60 + start.minute() + 15).min(23 * 60 + 59);
            NaiveTime::from_hms_opt(mins / 60, mins % 60, 0).unwrap_or(start)
        } else {
            end
        };
        self.conn.execute(
            "UPDATE tasks SET due_date = ?2, due_time = ?3, end_date = ?4, end_time = ?5, horizon = NULL WHERE id = ?1",
            params![
                id.to_string(),
                date.format(DATE_FORMAT).to_string(),
                start.format(TIME_FORMAT).to_string(),
                stored_end_date.map(|d| d.format(DATE_FORMAT).to_string()),
                end.format(TIME_FORMAT).to_string(),
            ],
        )?;
        Ok(())
    }

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
        let template = self.templates()?.into_iter().find(|t| t.id == id).context("template not found")?;
        let task = self.add_task(&template.title)?;
        self.set_notes(task.id, &template.notes)?;
        self.set_priority(task.id, template.priority)?;
        self.set_tags(task.id, &template.tags)?;
        for item in &template.checklist {
            self.add_checklist_item(task.id, item, None)?;
        }
        self.task(task.id)?.context("task not found")
    }

    /// Sections of a list ("Add Section"), oldest first.
    /// A list's sections, or the Inbox's for `None`.
    pub fn list_sections(&self, list: Option<Id>) -> Result<Vec<crate::model::ListSection>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, list_id, name FROM list_sections WHERE list_id IS ?1 ORDER BY sort_order, created_at, rowid",
        )?;
        let sections = stmt
            .query_map([list.map(|l| l.to_string())], |row| {
                Ok(crate::model::ListSection {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    list_id: row.get::<_, Option<String>>(1)?.and_then(|l| Uuid::parse_str(&l).ok()),
                    name: row.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(sections)
    }

    pub fn add_list_section(&self, list: Option<Id>, name: &str) -> Result<crate::model::ListSection> {
        self.insert_list_section(list, name, None)
    }

    /// Adds a section at `index` among the list's sections (last for `None`), as the kanban's
    /// "Add Section to Left / Right".
    pub fn insert_list_section(&self, list: Option<Id>, name: &str, index: Option<usize>) -> Result<crate::model::ListSection> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a section needs a name");
        let section = crate::model::ListSection { id: Uuid::now_v7(), list_id: list, name: name.to_owned() };
        self.in_tx(|| {
            let mut order: Vec<Id> = self.list_sections(list)?.into_iter().map(|s| s.id).collect();
            let at = index.unwrap_or(order.len()).min(order.len());
            order.insert(at, section.id);
            self.conn.execute(
                "INSERT INTO list_sections (id, list_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![section.id.to_string(), list.map(|l| l.to_string()), section.name, now_ms()],
            )?;
            self.write_sort_order(SortTable::ListSections, &order)?;
            Ok(())
        })?;
        Ok(section)
    }

    /// The kanban column menu's "Move to": the section and its tasks go to another list, after
    /// its sections.
    pub fn move_list_section(&self, id: Id, to: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT id FROM tasks WHERE section_id = ?1 AND parent_id IS NULL")?;
            let tasks: Vec<Id> = stmt
                .query_map([id.to_string()], |row| {
                    let raw: String = row.get(0)?;
                    parse_uuid(&raw, 0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let order: i64 = self.conn.query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM list_sections WHERE list_id IS ?1",
                [to.map(|l| l.to_string())],
                |row| row.get(0),
            )?;
            self.conn.execute(
                "UPDATE list_sections SET list_id = ?2, sort_order = ?3 WHERE id = ?1",
                params![id.to_string(), to.map(|l| l.to_string()), order],
            )?;
            for task in tasks {
                self.set_list(task, to)?;
                self.set_section(task, Some(id))?;
            }
            Ok(())
        })
    }

    pub fn rename_list_section(&self, id: Id, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a section needs a name");
        self.conn.execute("UPDATE list_sections SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(())
    }

    /// Reorders a section among its list's sections by moving `dragged` to `target`'s position.
    pub fn reorder_list_sections(&self, list: Option<Id>, dragged: Id, target: Id) -> Result<()> {
        if dragged == target {
            return Ok(());
        }
        self.in_tx(|| {
            let mut order: Vec<Id> = self.list_sections(list)?.into_iter().map(|s| s.id).collect();
            let Some(from) = order.iter().position(|&id| id == dragged) else { return Ok(()) };
            let Some(to) = order.iter().position(|&id| id == target) else { return Ok(()) };
            let id = order.remove(from);
            order.insert(to, id);
            self.write_sort_order(SortTable::ListSections, &order)
        })
    }

    /// Deletes a section; its tasks stay in the list without a section.
    pub fn delete_list_section(&self, id: Id) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute("UPDATE tasks SET section_id = NULL WHERE section_id = ?1", [id.to_string()])?;
            self.conn.execute("DELETE FROM list_sections WHERE id = ?1", [id.to_string()])?;
            Ok(())
        })
    }

    /// Puts a task (and its subtasks) into a section of its list, or out of any.
    pub fn set_section(&self, id: Id, section: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            let task = self.task(id)?.context("task not found")?;
            if let Some(section) = section {
                let list: Option<Option<String>> = self
                    .conn
                    .query_row("SELECT list_id FROM list_sections WHERE id = ?1", [section.to_string()], |row| row.get(0))
                    .optional()?;
                let task_list = task.list_id.map(|l| l.to_string());
                anyhow::ensure!(list == Some(task_list), "the section belongs to another list");
            }
            let section_str = section.map(|s| s.to_string());
            let unlink = match task.parent_id {
                Some(parent_id) => {
                    let parent_section: Option<Option<String>> = self
                        .conn
                        .prepare_cached("SELECT section_id FROM tasks WHERE id = ?1")?
                        .query_row([parent_id.to_string()], |row| row.get(0))
                        .optional()?;
                    parent_section.is_none_or(|ps| ps != section_str)
                }
                None => false,
            };
            if unlink {
                self.conn.execute("UPDATE tasks SET parent_id = NULL WHERE id = ?1", [id.to_string()])?;
            }
            let mut stmt = self.conn.prepare_cached("UPDATE tasks SET section_id = ?2 WHERE id = ?1")?;
            for sub in std::iter::once(id).chain(self.descendants(id)?) {
                stmt.execute(params![sub.to_string(), section_str])?;
            }
            Ok(())
        })
    }

    /// Pins a task to the top of its lists, or unpins it.
    /// "Estimation": the expected pomos or minutes; `None` clears it.
    pub fn set_estimate(&self, id: Id, estimate: Option<crate::model::Estimate>) -> Result<()> {
        self.update(id, "estimate", estimate.map(crate::model::Estimate::to_storage))
    }

    pub fn set_pinned(&self, id: Id, pinned: bool) -> Result<()> {
        self.update(id, "pinned_at", pinned.then(now_ms))
    }

    pub fn set_reminders(&self, id: Id, reminders: &[i64]) -> Result<()> {
        self.update(id, "reminders", crate::reminder::format(reminders))
    }

    pub fn set_due(&self, id: Id, date: Option<NaiveDate>, time: Option<NaiveTime>) -> Result<()> {
        let date_str = date.map(|d| d.format(DATE_FORMAT).to_string());
        let time_str = date.and(time).map(|t| t.format(TIME_FORMAT).to_string());
        if date.is_some() {
            self.conn.execute(
                "UPDATE tasks SET due_date = ?2, due_time = ?3, snoozed_until = NULL, horizon = NULL WHERE id = ?1",
                params![id.to_string(), date_str, time_str],
            )?;
        } else {
            self.conn.execute(
                "UPDATE tasks SET due_date = ?2, due_time = ?3, snoozed_until = NULL WHERE id = ?1",
                params![id.to_string(), date_str, time_str],
            )?;
        }
        Ok(())
    }

    /// Assigns or clears a task's horizon bucket. Assigning a horizon clears exact dates,
    /// snooze timestamp, and Things schedule bucket to maintain mutual exclusion.
    pub fn set_horizon(&self, id: Id, horizon: Option<Horizon>) -> Result<()> {
        match horizon {
            Some(h) => {
                self.conn.execute(
                    "UPDATE tasks
                     SET horizon = ?2, due_date = NULL, due_time = NULL,
                         end_date = NULL, end_time = NULL, start_date = NULL,
                         snoozed_until = NULL, schedule_bucket = ''
                     WHERE id = ?1",
                    params![id.to_string(), h.as_code()],
                )?;
            }
            None => {
                self.conn.execute("UPDATE tasks SET horizon = NULL WHERE id = ?1", [id.to_string()])?;
            }
        }
        Ok(())
    }

    /// Transactionally moves open non-deleted tasks in `ids` to `target` horizon, clearing exact dates.
    pub fn rollover_horizon_tasks(&self, ids: &[Id], target: Horizon) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let code = target.as_code();
        self.in_tx(|| {
            let mut stmt = self.conn.prepare_cached(
                "UPDATE tasks
                 SET horizon = ?2, due_date = NULL, due_time = NULL,
                     end_date = NULL, end_time = NULL, start_date = NULL,
                     snoozed_until = NULL, schedule_bucket = ''
                 WHERE id = ?1 AND deleted_at IS NULL AND completed_at IS NULL AND wont_do_at IS NULL",
            )?;
            for id in ids {
                stmt.execute(params![id.to_string(), &code])?;
            }
            Ok(())
        })
    }

    /// Transactionally moves open non-deleted overdue tasks in `ids` to `day`, preserving `due_time`
    /// and shifting `end_date` by the same delta when present, while clearing `horizon` and `snoozed_until`.
    pub fn rollover_overdue_to_day(&self, ids: &[Id], day: NaiveDate) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let day_str = day.format(DATE_FORMAT).to_string();
        self.in_tx(|| {
            let mut stmt = self.conn.prepare_cached(
                "UPDATE tasks
                 SET due_date = ?2, end_date = ?3, horizon = NULL, snoozed_until = NULL
                 WHERE id = ?1 AND deleted_at IS NULL AND completed_at IS NULL AND wont_do_at IS NULL",
            )?;
            for &id in ids {
                let Some(task) = self.task(id)? else {
                    continue;
                };
                if task.is_deleted() || task.is_closed() {
                    continue;
                }
                let next_end = match (task.due_date, task.end_date) {
                    (Some(old_due), Some(old_end)) => old_end
                        .checked_add_signed(day - old_due)
                        .map(|d| d.format(DATE_FORMAT).to_string()),
                    (_, Some(old_end)) => Some(old_end.format(DATE_FORMAT).to_string()),
                    _ => None,
                };
                stmt.execute(params![id.to_string(), &day_str, next_end])?;
            }
            Ok(())
        })
    }

    /// Marks a task explicitly "In Progress" (reopening it if closed), or clears that status.
    pub fn set_in_progress(&self, id: Id, in_progress: bool) -> Result<()> {
        if in_progress {
            self.conn.execute(
                "UPDATE tasks SET in_progress_at = ?2, completed_at = NULL, wont_do_at = NULL WHERE id = ?1",
                params![id.to_string(), now_ms()],
            )?;
        } else {
            self.conn.execute("UPDATE tasks SET in_progress_at = NULL WHERE id = ?1", [id.to_string()])?;
        }
        Ok(())
    }

    pub fn set_location(&self, id: Id, location: Option<&str>) -> Result<()> {
        let clean = location.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
        self.update(id, "location", clean)
    }

    pub fn set_timezone(&self, id: Id, timezone: Option<&str>, is_floating: bool) -> Result<()> {
        let clean = match timezone.map(str::trim).filter(|s| !s.is_empty()) {
            Some(tz) => {
                chrono_tz::Tz::from_str(tz).map_err(|e| anyhow::anyhow!("invalid time zone {tz}: {e}"))?;
                Some(tz.to_owned())
            }
            None => None,
        };
        self.conn.execute(
            "UPDATE tasks SET timezone = ?2, is_floating = ?3 WHERE id = ?1",
            params![id.to_string(), clean, is_floating],
        )?;
        Ok(())
    }

    pub fn snooze_task(&self, id: Id, at: NaiveDateTime) -> Result<()> {
        self.update(id, "snoozed_until", Some(at.format(SNOOZE_FORMAT).to_string()))
    }

    pub fn clear_task_snooze(&self, id: Id) -> Result<()> {
        self.update(id, "snoozed_until", None::<String>)
    }

    pub fn set_task_constant_reminder(&self, id: Id, on: bool) -> Result<()> {
        self.update(id, "constant_reminder", on)
    }

    /// Holiday dates collected from non-archived Holiday countdowns and non-hidden holiday ICS feeds.
    pub fn holiday_dates(&self, from: NaiveDate, until: NaiveDate) -> Result<HashSet<NaiveDate>> {
        use chrono::Datelike as _;
        let mut dates = HashSet::new();
        for countdown in self.countdowns()? {
            if countdown.archived || countdown.kind != CountdownKind::Holiday {
                continue;
            }
            if let Some(repeat) = &countdown.repeat {
                let mut cur = countdown.date;
                while cur <= until {
                    if cur >= from {
                        dates.insert(cur);
                    }
                    let next = repeat.next_after(cur);
                    if next <= cur {
                        break;
                    }
                    cur = next;
                }
            } else {
                for year in from.year().max(countdown.date.year())..=until.year() {
                    if let Some(date) = countdown.date.with_year(year).or_else(|| NaiveDate::from_ymd_opt(year, 2, 28))
                        && date >= from
                        && date <= until
                    {
                        dates.insert(date);
                    }
                }
            }
        }
        for sub in self.subscriptions()? {
            if sub.hidden || sub.feed.trim().is_empty() {
                continue;
            }
            let title_lower = sub.name.to_lowercase();
            let url_lower = sub.url.to_lowercase();
            let is_holiday_feed = title_lower.contains("holiday")
                || title_lower.contains("праздник")
                || title_lower.contains("节假日")
                || title_lower.contains("假日")
                || url_lower.contains("holiday");
            if !is_holiday_feed {
                continue;
            }
            for event in crate::ical::parse(&sub.feed) {
                if !event.all_day() {
                    continue;
                }
                for occ in event.occurrences_in(from, until) {
                    let mut d = occ.start.max(from);
                    let end = occ.end.min(until);
                    while d <= end {
                        dates.insert(d);
                        let Some(next) = d.succ_opt() else { break };
                        d = next;
                    }
                }
            }
        }
        Ok(dates)
    }

    /// The task's comments, oldest first.
    pub fn comments(&self, task: Id) -> Result<Vec<crate::model::Comment>> {
        let mut stmt = self.conn.prepare_cached("SELECT id, text, created_at FROM task_comments WHERE task_id = ?1 ORDER BY created_at, rowid")?;
        let comments = stmt
            .query_map([task.to_string()], |row| {
                Ok(crate::model::Comment { id: parse_uuid(&row.get::<_, String>(0)?, 0)?, task, text: row.get(1)?, created_at: row.get(2)? })
            })?
            .collect::<Result<_, _>>()?;
        Ok(comments)
    }

    pub fn add_comment(&self, task: Id, text: &str) -> Result<crate::model::Comment> {
        let text = text.trim();
        anyhow::ensure!(!text.is_empty(), "a comment needs text");
        let comment = crate::model::Comment { id: Uuid::now_v7(), task, text: text.to_owned(), created_at: now_ms() };
        self.conn.execute(
            "INSERT INTO task_comments (id, task_id, text, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![comment.id.to_string(), task.to_string(), comment.text, comment.created_at],
        )?;
        Ok(comment)
    }

    pub fn delete_comment(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM task_comments WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    /// The original's "Skip the Recurrence": a repeating task moves on to its next date without
    /// being completed; its time stays, and a range keeps its length.
    pub fn skip_recurrence(&self, id: Id) -> Result<()> {
        self.in_tx(|| {
            let task = self.task(id)?.context("task not found")?;
            let (Some(repeat), Some(due)) = (&task.repeat, task.due_date) else { anyhow::bail!("the task does not repeat") };
            let holidays = if repeat.skip_holidays {
                self.holiday_dates(due, due + Duration::days(400))?
            } else {
                HashSet::new()
            };
            let next = repeat.next_after_with_holidays(due, |d| holidays.contains(&d));
            anyhow::ensure!(next > due, "the task has no later date");
            self.set_due(id, Some(next), task.due_time)?;
            if let Some(end) = task.end_date {
                self.update(id, "end_date", (end + (next - due)).format(DATE_FORMAT).to_string())?;
            }
            if repeat.forgetting_curve {
                self.update(id, "repeat", repeat.advance().to_string())?;
            }
            Ok(())
        })
    }

    /// Completes a repeating task permanently, clearing its recurrence rule so no future
    /// occurrence is scheduled ("Complete All").
    pub fn complete_forever(&self, id: Id, today: NaiveDate) -> Result<()> {
        self.in_tx(|| {
            self.set_repeat(id, None, today)?;
            self.set_completed(id, true)?;
            Ok(())
        })
    }

    /// Ancestor IDs of `id`, from its immediate parent up to the root (cycle-safe).
    pub fn ancestors(&self, id: Id) -> Result<Vec<Id>> {
        let mut out = Vec::new();
        let mut seen = HashSet::from([id]);
        let mut current = id;
        while let Some(parent_str) = self
            .conn
            .prepare_cached("SELECT parent_id FROM tasks WHERE id = ?1")?
            .query_row([current.to_string()], |row| row.get::<_, Option<String>>(0))
            .optional()?
            .flatten()
        {
            let Ok(parent) = Uuid::parse_str(&parent_str) else { break };
            if !seen.insert(parent) {
                break;
            }
            out.push(parent);
            current = parent;
        }
        Ok(out)
    }

    /// Nesting depth of `id` (`0` for a top-level task, up to `MAX_SUBTASK_DEPTH`).
    pub fn task_depth(&self, id: Id) -> Result<usize> {
        Ok(self.ancestors(id)?.len())
    }

    /// Removes `id`'s parent link, promoting it (with its own subtasks) to a top-level task.
    pub fn unlink_parent(&self, id: Id) -> Result<()> {
        self.set_parent(id, None)
    }

    /// "Link Parent Task": makes a task a subtask of `parent`, moving it (and its descendants,
    /// including any flattened onto `parent` at `MAX_SUBTASK_DEPTH`) to the parent's list and
    /// section and clamping any descendants that exceed `MAX_SUBTASK_DEPTH`.
    pub fn link_parent(&self, id: Id, parent: Id) -> Result<()> {
        self.in_tx(|| {
            let parent_task = self.task(parent)?.context("parent task not found")?;
            // Capture the full subtree BEFORE `set_parent` flattens deep descendants onto `parent`.
            let subtree: Vec<Id> = std::iter::once(id).chain(self.descendants(id)?).collect();
            self.set_parent(id, Some(parent))?;
            let list = parent_task.list_id.map(|l| l.to_string());
            let section = parent_task.section_id.map(|s| s.to_string());
            let mut move_stmt = self
                .conn
                .prepare_cached("UPDATE tasks SET list_id = ?2, section_id = ?3 WHERE id = ?1")?;
            for task_id in &subtree {
                move_stmt.execute(params![task_id.to_string(), list, section])?;
            }
            if parent_task.is_completed() {
                let now = parent_task.completed_at.unwrap_or_else(now_ms);
                let mut close_stmt = self.conn.prepare_cached(
                    "UPDATE tasks SET completed_at = COALESCE(completed_at, ?2) WHERE id = ?1 AND wont_do_at IS NULL AND deleted_at IS NULL",
                )?;
                for task_id in &subtree {
                    close_stmt.execute(params![task_id.to_string(), now])?;
                }
            } else if parent_task.is_wont_do() {
                let now = parent_task.wont_do_at.unwrap_or_else(now_ms);
                let mut close_stmt = self.conn.prepare_cached(
                    "UPDATE tasks SET wont_do_at = COALESCE(wont_do_at, ?2) WHERE id = ?1 AND completed_at IS NULL AND deleted_at IS NULL",
                )?;
                for task_id in &subtree {
                    close_stmt.execute(params![task_id.to_string(), now])?;
                }
            }
            Ok(())
        })
    }

    /// Makes the task a subtask of `parent`, or a top-level task for `None`.
    /// Notes cannot be nested or hold subtasks, and descendants beyond `MAX_SUBTASK_DEPTH`
    /// are flattened onto level `MAX_SUBTASK_DEPTH`.
    pub fn set_parent(&self, id: Id, parent: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            if let Some(parent) = parent {
                let task = self.task(id)?.context("task not found")?;
                let parent_task = self.task(parent)?.context("parent task not found")?;
                anyhow::ensure!(!parent_task.is_deleted(), "trashed tasks cannot be parents");
                anyhow::ensure!(task.kind != Kind::Note && parent_task.kind != Kind::Note, "notes cannot be nested");
                let ancestors = self.ancestors(parent)?;
                anyhow::ensure!(
                    parent != id && !ancestors.contains(&id),
                    "a task cannot be nested inside itself"
                );
                let parent_depth = ancestors.len();
                anyhow::ensure!(parent_depth < MAX_SUBTASK_DEPTH, "maximum subtask nesting depth reached");
                self.update(id, "parent_id", Some(parent.to_string()))?;
                self.flatten_subtask_depth(id, parent_depth + 1, parent)?;
                return Ok(());
            }
            self.update(id, "parent_id", None::<String>)
        })
    }

    /// Flattens any descendants of `root` (currently at `root_depth`, child of `root_parent`)
    /// whose depth would exceed `MAX_SUBTASK_DEPTH` onto `MAX_SUBTASK_DEPTH`.
    fn flatten_subtask_depth(&self, root: Id, root_depth: usize, root_parent: Id) -> Result<()> {
        let mut stack: Vec<(Id, usize, Option<Id>)> = vec![(
            root,
            root_depth,
            if root_depth == MAX_SUBTASK_DEPTH {
                Some(root_parent)
            } else if root_depth == MAX_SUBTASK_DEPTH - 1 {
                Some(root)
            } else {
                None
            },
        )];
        let mut reparents: Vec<(Id, Id)> = Vec::new();
        let mut seen = HashSet::from([root]);
        while let Some((node, depth, cap_parent)) = stack.pop() {
            let mut stmt = self.conn.prepare_cached("SELECT id FROM tasks WHERE parent_id = ?1")?;
            let children: Vec<Id> = stmt
                .query_map([node.to_string()], |row| {
                    let raw: String = row.get(0)?;
                    parse_uuid(&raw, 0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for child in children {
                if !seen.insert(child) {
                    continue;
                }
                let child_depth = depth + 1;
                if child_depth > MAX_SUBTASK_DEPTH {
                    if let Some(target_parent) = cap_parent {
                        reparents.push((child, target_parent));
                    }
                    stack.push((child, child_depth, cap_parent));
                } else {
                    let next_cap = if child_depth == MAX_SUBTASK_DEPTH - 1 {
                        Some(child)
                    } else {
                        cap_parent
                    };
                    stack.push((child, child_depth, next_cap));
                }
            }
        }
        if !reparents.is_empty() {
            self.in_tx(|| {
                let mut stmt = self.conn.prepare_cached("UPDATE tasks SET parent_id = ?2 WHERE id = ?1")?;
                for (child, target_parent) in reparents {
                    stmt.execute(params![child.to_string(), target_parent.to_string()])?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Moves `id` to `new_parent` (nesting or unnesting) and optionally positions it relative to
    /// `target` (`(target_id, after)`). When nesting directly after `new_parent`, places `id` as
    /// the first child of `new_parent`.
    pub fn move_task_in_tree(&self, id: Id, new_parent: Option<Id>, target: Option<(Id, bool)>) -> Result<()> {
        self.in_tx(|| {
            match new_parent {
                Some(parent) => self.link_parent(id, parent)?,
                None => self.set_parent(id, None)?,
            }
            if let Some((target_id, after)) = target {
                if new_parent == Some(target_id) && after {
                    let first_child: Option<Id> = self
                        .conn
                        .prepare_cached("SELECT id FROM tasks WHERE parent_id = ?1 AND id != ?2 AND deleted_at IS NULL ORDER BY sort_order, rowid DESC LIMIT 1")?
                        .query_row(params![target_id.to_string(), id.to_string()], |row| row.get::<_, String>(0))
                        .optional()?
                        .and_then(|s| Uuid::parse_str(&s).ok());
                    if let Some(first_child) = first_child {
                        self.move_task(id, first_child, false)?;
                    } else if target_id != id {
                        self.move_task(id, target_id, true)?;
                    }
                } else if target_id != id {
                    self.move_task(id, target_id, after)?;
                }
            }
            Ok(())
        })
    }

    /// Replaces the task's tags; unknown tags are created.
    /// Tags match case-insensitively in Rust, because SQLite's `NOCASE` only folds ASCII.
    pub fn set_tags(&self, id: Id, tags: &[String]) -> Result<()> {
        self.in_tx(|| self.replace_task_tags(id, tags))
    }

    fn sanitize_tag_name(raw: &str) -> String {
        raw.chars().filter(|c| !c.is_ascii_control()).collect::<String>().trim().to_owned()
    }

    fn replace_task_tags(&self, id: Id, tags: &[String]) -> Result<()> {
        let mut existing: Vec<String> = self
            .conn
            .prepare_cached("SELECT name FROM tags")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let id = id.to_string();
        self.conn.execute("DELETE FROM task_tags WHERE task_id = ?1", [&id])?;
        for tag in tags.iter().map(|t| Self::sanitize_tag_name(t)).filter(|t| !t.is_empty()) {
            let tag = tag.as_str();
            let name = match existing.iter().find(|e| e.to_lowercase() == tag.to_lowercase()) {
                Some(name) => name.clone(),
                None => {
                    self.conn.execute(
                        "INSERT OR IGNORE INTO tags (name, created_at) VALUES (?1, ?2)",
                        params![tag, now_ms()],
                    )?;
                    existing.push(tag.to_owned());
                    tag.to_owned()
                }
            };
            self.conn.execute(
                "INSERT OR IGNORE INTO task_tags (task_id, tag) VALUES (?1, ?2)",
                params![id, name],
            )?;
        }
        Ok(())
    }

    pub fn tags(&self) -> Result<Vec<Tag>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.name, t.color, COUNT(tk.id), t.parent, t.pinned_at
             FROM tags t
             LEFT JOIN task_tags tt ON tt.tag = t.name
             LEFT JOIN tasks tk ON tk.id = tt.task_id AND tk.completed_at IS NULL
                                AND tk.deleted_at IS NULL
             GROUP BY t.name
             ORDER BY t.created_at, t.rowid",
        )?;
        let mut tags: Vec<Tag> = stmt
            .query_map([], |row| {
                Ok(Tag {
                    name: row.get(0)?,
                    color: row.get(1)?,
                    open_count: row.get::<_, i64>(2)? as usize,
                    parent: row.get(3)?,
                    pinned: row.get::<_, Option<i64>>(4)?.is_some(),
                })
            })?
            .collect::<Result<_, _>>()?;
        if let Some(saved) = self.setting(TAG_ORDER_KEY)? {
            let mut ranks: HashMap<&str, usize> = HashMap::new();
            for (i, name) in saved.lines().filter(|s| !s.is_empty()).enumerate() {
                ranks.entry(name).or_insert(i);
            }
            tags.sort_by_key(|t| ranks.get(t.name.as_str()).copied().unwrap_or(usize::MAX));
        }
        Ok(tags)
    }

    pub fn set_tag_color(&self, name: &str, color: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE tags SET color = ?2 WHERE name = ?1", params![name, color])?;
        Ok(())
    }

    /// Checks a parent for `tag`: one level deep, as in the original — the parent is a top-level
    /// tag other than the tag itself, and a tag with sub-tags cannot become one.
    fn check_parent(&self, tag: Option<&str>, parent: Option<&str>) -> Result<()> {
        let Some(parent) = parent else { return Ok(()) };
        let tags = self.tags()?;
        let found = tags.iter().find(|t| t.name == parent).context("no such parent tag")?;
        anyhow::ensure!(found.parent.is_none(), "a sub-tag cannot have sub-tags");
        if let Some(tag) = tag {
            anyhow::ensure!(tag != parent, "a tag cannot be its own parent");
            anyhow::ensure!(!tags.iter().any(|t| t.parent.as_deref() == Some(tag)), "a tag with sub-tags cannot become a sub-tag");
        }
        Ok(())
    }

    /// The original's Add Tags: a new tag with its color and parent.
    pub fn add_tag(&self, name: &str, color: Option<&str>, parent: Option<&str>) -> Result<()> {
        let name = name.trim().trim_start_matches('#').trim();
        anyhow::ensure!(!name.is_empty(), "a tag needs a name");
        anyhow::ensure!(!name.chars().any(|c| c.is_ascii_control()), "tag name cannot contain control characters");
        anyhow::ensure!(!self.tags()?.iter().any(|t| t.name.to_lowercase() == name.to_lowercase()), "the tag already exists");
        self.check_parent(None, parent)?;
        self.conn.execute(
            "INSERT INTO tags (name, color, parent, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, color, parent, now_ms()],
        )?;
        Ok(())
    }

    /// The original's Edit Tag: name, color and parent. A new name is a new row (the name is the
    /// key, also for sync), so the tag's tasks and sub-tags move over to it.
    pub fn update_tag(&self, name: &str, new_name: &str, color: Option<&str>, parent: Option<&str>) -> Result<()> {
        let new_name = new_name.trim().trim_start_matches('#').trim();
        anyhow::ensure!(!new_name.is_empty(), "a tag needs a name");
        anyhow::ensure!(!new_name.chars().any(|c| c.is_ascii_control()), "tag name cannot contain control characters");
        self.check_parent(Some(name), parent)?;
        self.in_tx(|| {
            if new_name != name {
                anyhow::ensure!(
                    !self.tags()?.iter().any(|t| t.name != name && t.name.to_lowercase() == new_name.to_lowercase()),
                    "the tag already exists"
                );
                self.conn.execute(
                    "INSERT INTO tags (name, color, parent, pinned_at, created_at) SELECT ?2, color, parent, pinned_at, created_at FROM tags WHERE name = ?1",
                    params![name, new_name],
                )?;
                self.conn.execute("INSERT OR IGNORE INTO task_tags (task_id, tag) SELECT task_id, ?2 FROM task_tags WHERE tag = ?1", params![name, new_name])?;
                self.conn.execute("DELETE FROM task_tags WHERE tag = ?1", [name])?;
                self.conn.execute("UPDATE tags SET parent = ?2 WHERE parent = ?1", params![name, new_name])?;
                self.conn.execute("DELETE FROM tags WHERE name = ?1", [name])?;
                if let Some(saved) = self.setting(TAG_ORDER_KEY)? {
                    let updated: Vec<&str> = saved.lines().map(|n| if n == name { new_name } else { n }).collect();
                    self.set_setting(TAG_ORDER_KEY, &updated.join("\n"))?;
                }
                self.update_matrix_rules_for_tag_rename(name, new_name)?;
            }
            self.conn.execute("UPDATE tags SET color = ?2, parent = ?3 WHERE name = ?1", params![new_name, color, parent])?;
            Ok(())
        })
    }

    pub fn set_tag_pinned(&self, name: &str, pinned: bool) -> Result<()> {
        self.conn.execute("UPDATE tags SET pinned_at = ?2 WHERE name = ?1", params![name, pinned.then(now_ms)])?;
        Ok(())
    }

    /// Reorders or nests `dragged` tag relative to `target` tag, matching TickTick's tag drag rules:
    /// - `nest = true`: makes `dragged` a sub-tag of `target` (1 level deep: ignored if `dragged`
    ///   has sub-tags or `target` is itself a sub-tag).
    /// - `nest = false`: moves `dragged` (together with its sub-tags if it is a parent) before
    ///   (`!after`) or after (`after`) `target`. A standalone/sub-tag adopts `target`'s parent, so
    ///   dropping a sub-tag onto a top-level tag's edge promotes it to top-level.
    pub fn move_tag(&self, dragged: &str, target: &str, after: bool, nest: bool) -> Result<()> {
        if dragged == target {
            return Ok(());
        }
        self.in_tx(|| {
            let tags = self.tags()?;
            let Some(dragged_tag) = tags.iter().find(|t| t.name == dragged) else { return Ok(()) };
            let Some(target_tag) = tags.iter().find(|t| t.name == target) else { return Ok(()) };

            let dragged_children: Vec<String> =
                tags.iter().filter(|t| t.parent.as_deref() == Some(dragged)).map(|t| t.name.clone()).collect();
            let dragged_is_parent = !dragged_children.is_empty();
            let target_parent = target_tag.parent.clone();

            if nest {
                if dragged_is_parent || target_parent.is_some() {
                    return Ok(());
                }
                let mut remaining: Vec<String> = tags.iter().filter(|t| t.name != dragged).map(|t| t.name.clone()).collect();
                let last_child_pos = remaining.iter().rposition(|name| {
                    tags.iter().any(|t| t.name == *name && t.parent.as_deref() == Some(target))
                });
                let insert_at = match last_child_pos {
                    Some(idx) => idx + 1,
                    None => remaining.iter().position(|name| name == target).map_or(remaining.len(), |idx| idx + 1),
                };
                remaining.insert(insert_at, dragged.to_owned());

                self.conn.execute(
                    "UPDATE tags SET parent = ?2, pinned_at = NULL WHERE name = ?1",
                    params![dragged, target],
                )?;
                self.set_setting(TAG_ORDER_KEY, &remaining.join("\n"))?;
                return Ok(());
            }

            if dragged_is_parent && target_parent.is_some() {
                return Ok(());
            }

            let mut block = Vec::with_capacity(1 + dragged_children.len());
            block.push(dragged.to_owned());
            block.extend(dragged_children);

            let mut remaining: Vec<String> =
                tags.iter().filter(|t| !block.contains(&t.name)).map(|t| t.name.clone()).collect();
            let Some(target_pos) = remaining.iter().position(|name| name == target) else {
                return Ok(());
            };
            let insert_at = if after && target_parent.is_none() {
                remaining
                    .iter()
                    .rposition(|name| tags.iter().any(|t| t.name == *name && t.parent.as_deref() == Some(target)))
                    .map_or(target_pos + 1, |idx| idx + 1)
            } else {
                target_pos + usize::from(after)
            };
            remaining.splice(insert_at..insert_at, block);

            if !dragged_is_parent && dragged_tag.parent != target_parent {
                self.conn.execute(
                    "UPDATE tags SET parent = ?2 WHERE name = ?1",
                    params![dragged, target_parent.as_deref()],
                )?;
            }
            if target_parent.is_some() {
                self.conn.execute("UPDATE tags SET pinned_at = NULL WHERE name = ?1", [dragged])?;
            } else if dragged_tag.pinned != target_tag.pinned {
                self.conn.execute(
                    "UPDATE tags SET pinned_at = ?2 WHERE name = ?1",
                    params![dragged, target_tag.pinned.then(now_ms)],
                )?;
            }
            self.set_setting(TAG_ORDER_KEY, &remaining.join("\n"))?;
            Ok(())
        })
    }

    /// Moves a tag (and its children if any) to the top of the Tags section, out of any parent.
    pub fn move_tag_first(&self, dragged: &str) -> Result<()> {
        self.in_tx(|| {
            let tags = self.tags()?;
            if !tags.iter().any(|t| t.name == dragged) {
                return Ok(());
            }
            let mut block = vec![dragged.to_owned()];
            block.extend(tags.iter().filter(|t| t.parent.as_deref() == Some(dragged)).map(|t| t.name.clone()));
            let mut remaining: Vec<String> =
                tags.iter().filter(|t| !block.contains(&t.name)).map(|t| t.name.clone()).collect();
            remaining.splice(0..0, block);

            self.conn.execute("UPDATE tags SET parent = NULL WHERE name = ?1", [dragged])?;
            self.set_setting(TAG_ORDER_KEY, &remaining.join("\n"))?;
            Ok(())
        })
    }

    /// Commits a live-reordered tag slice (hierarchy, pinned state, and display order) atomically.
    pub fn save_tag_hierarchy_and_order(&self, tags: &[Tag]) -> Result<()> {
        self.in_tx(|| {
            let existing = self.tags()?;
            let by_name: HashMap<&str, &Tag> = existing.iter().map(|t| (t.name.as_str(), t)).collect();
            let valid_roots: HashSet<&str> = tags
                .iter()
                .filter(|t| t.parent.is_none() && by_name.contains_key(t.name.as_str()))
                .map(|t| t.name.as_str())
                .collect();

            for tag in tags {
                let Some(prev) = by_name.get(tag.name.as_str()) else { continue };
                let parent = tag
                    .parent
                    .as_deref()
                    .filter(|&p| p != tag.name.as_str() && valid_roots.contains(p));
                let pinned = parent.is_none() && tag.pinned;
                if prev.parent.as_deref() != parent {
                    self.conn.execute(
                        "UPDATE tags SET parent = ?2 WHERE name = ?1",
                        params![tag.name, parent],
                    )?;
                }
                if prev.pinned != pinned {
                    self.conn.execute(
                        "UPDATE tags SET pinned_at = ?2 WHERE name = ?1",
                        params![tag.name, pinned.then(now_ms)],
                    )?;
                }
            }

            let order: Vec<&str> = tags
                .iter()
                .filter(|t| by_name.contains_key(t.name.as_str()))
                .map(|t| t.name.as_str())
                .collect();
            self.set_setting(TAG_ORDER_KEY, &order.join("\n"))?;
            Ok(())
        })
    }

    fn prune_tag_order(&self, removed: &str) -> Result<()> {
        if let Some(saved) = self.setting(TAG_ORDER_KEY)? {
            let updated: Vec<&str> = saved.lines().filter(|s| !s.is_empty() && *s != removed).collect();
            self.set_setting(TAG_ORDER_KEY, &updated.join("\n"))?;
        }
        Ok(())
    }

    /// The original's Merge Tags: `from`'s tasks get `into` instead, and `from` is deleted; its
    /// sub-tags go under `into` (or to the top when `into` is itself a sub-tag).
    pub fn merge_tag(&self, from: &str, into: &str) -> Result<()> {
        anyhow::ensure!(from != into, "a tag cannot be merged into itself");
        self.in_tx(|| {
            let tags = self.tags()?;
            let target = tags.iter().find(|t| t.name == into).context("no such tag")?;
            let new_parent = if target.parent.is_none() { Some(into) } else { None };
            self.conn.execute("INSERT OR IGNORE INTO task_tags (task_id, tag) SELECT task_id, ?2 FROM task_tags WHERE tag = ?1", params![from, into])?;
            self.conn.execute("DELETE FROM task_tags WHERE tag = ?1", [from])?;
            self.conn.execute("UPDATE tags SET parent = ?2 WHERE parent = ?1 AND name != ?3", params![from, new_parent, into])?;
            self.conn.execute("DELETE FROM tags WHERE name = ?1", [from])?;
            self.prune_tag_order(from)?;
            self.update_matrix_rules_for_tag_rename(from, into)?;
            Ok(())
        })
    }

    /// Deletes a tag: its tasks keep everything else, its sub-tags move to the top.
    pub fn delete_tag(&self, name: &str) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute("DELETE FROM task_tags WHERE tag = ?1", [name])?;
            self.conn.execute("UPDATE tags SET parent = NULL WHERE parent = ?1", [name])?;
            self.conn.execute("DELETE FROM tags WHERE name = ?1", [name])?;
            self.prune_tag_order(name)?;
            self.prune_matrix_rules_for_tag_delete(name)?;
            Ok(())
        })
    }

    /// A tag and all its descendant sub-tags (transitively, cycle-safe), lowercased: a parent tag's
    /// view shows its sub-tags' tasks too.
    fn tag_family(&self, tag: &str) -> Result<HashSet<String>> {
        let root = tag.to_lowercase();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        for t in self.tags()? {
            if let Some(parent) = t.parent {
                children.entry(parent.to_lowercase()).or_default().push(t.name.to_lowercase());
            }
        }
        let mut family = HashSet::from([root.clone()]);
        let mut stack = vec![root];
        while let Some(cur) = stack.pop() {
            if let Some(kids) = children.get(&cur) {
                for kid in kids {
                    if family.insert(kid.clone()) {
                        stack.push(kid.clone());
                    }
                }
            }
        }
        Ok(family)
    }

    /// The lists in their order, archived ones included (see `List::archived`).
    pub fn lists(&self) -> Result<Vec<List>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT l.id, l.name, l.color, COUNT(t.id), l.icon, l.folder_id, l.kind, l.in_smart_lists, l.archived_at,
                    l.notes, l.deadline, l.completed_at
             FROM lists l
             LEFT JOIN tasks t ON t.list_id = l.id AND t.completed_at IS NULL
                               AND t.deleted_at IS NULL
             GROUP BY l.id
             ORDER BY l.sort_order, l.created_at, l.rowid",
        )?;
        let lists = stmt
            .query_map([], |row| {
                Ok(List {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    open_count: row.get::<_, i64>(3)? as usize,
                    icon: row.get(4)?,
                    folder: row.get::<_, Option<String>>(5)?.and_then(|f| Uuid::parse_str(&f).ok()),
                    kind: Kind::parse(&row.get::<_, String>(6)?),
                    in_smart_lists: row.get(7)?,
                    archived: row.get::<_, Option<i64>>(8)?.is_some(),
                    notes: row.get(9)?,
                    deadline: row.get::<_, Option<String>>(10)?
                        .and_then(|d| NaiveDate::parse_from_str(&d, DATE_FORMAT).ok()),
                    completed_at: row.get(11)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(lists)
    }

    pub fn create_list(&self, name: &str) -> Result<List> {
        self.add_list(&ListDraft { name: name.to_owned(), ..ListDraft::default() })
    }

    /// Adds a list as the Add List dialog describes it, after the others.
    pub fn add_list(&self, draft: &ListDraft) -> Result<List> {
        let name = draft.name.trim();
        anyhow::ensure!(!name.is_empty(), "a list needs a name");
        let id = Uuid::now_v7();
        self.conn.execute(
            "INSERT INTO lists (id, name, created_at, sort_order) VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM lists))",
            params![id.to_string(), name, now_ms()],
        )?;
        self.update_list(id, draft)?;
        self.lists()?.into_iter().find(|l| l.id == id).context("list not found")
    }

    /// Saves the Edit List dialog's choices.
    pub fn update_list(&self, id: Id, draft: &ListDraft) -> Result<()> {
        let name = draft.name.trim();
        anyhow::ensure!(!name.is_empty(), "a list needs a name");
        self.conn.execute(
            "UPDATE lists SET name = ?2, icon = ?3, color = ?4, folder_id = ?5, kind = ?6, in_smart_lists = ?7,
                              notes = ?8, deadline = ?9, completed_at = ?10 WHERE id = ?1",
            params![
                id.to_string(),
                name,
                draft.icon.as_deref().filter(|i| !i.is_empty()),
                draft.color,
                draft.folder.map(|f| f.to_string()),
                draft.kind.as_str(),
                draft.in_smart_lists,
                draft.notes,
                draft.deadline.map(|d| d.format(DATE_FORMAT).to_string()),
                draft.completed_at,
            ],
        )?;
        Ok(())
    }

    /// Archives a list or brings it back.
    pub fn set_list_archived(&self, id: Id, archived: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE lists SET archived_at = ?2 WHERE id = ?1",
            params![id.to_string(), archived.then(now_ms)],
        )?;
        Ok(())
    }

    /// Returns a list's display name and whether it is archived.
    pub fn list_name_and_archived(&self, id: Id) -> Result<Option<(String, bool)>> {
        self.conn
            .prepare_cached("SELECT name, archived_at IS NOT NULL FROM lists WHERE id = ?1")?
            .query_row([id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(Into::into)
    }

    /// Whether a list is archived (and therefore its tasks are read-only, as in the original).
    pub fn is_list_archived(&self, id: Id) -> Result<bool> {
        Ok(self.list_name_and_archived(id)?.is_some_and(|(_, archived)| archived))
    }

    /// Whether a task is read-only: either in the trash or inside an archived list.
    pub fn is_task_read_only(&self, id: Id) -> Result<bool> {
        let row: Option<(bool, bool)> = self
            .conn
            .prepare_cached(
                "SELECT t.deleted_at IS NOT NULL, COALESCE(l.archived_at IS NOT NULL, 0)
                 FROM tasks t
                 LEFT JOIN lists l ON l.id = t.list_id
                 WHERE t.id = ?1",
            )?
            .query_row([id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        Ok(row.is_some_and(|(deleted, archived)| deleted || archived))
    }

    /// The original's Duplicate: a copy of the list, " copy" after its name, with copies of its
    /// open tasks (subtasks, tags and checklists included), right after it.
    pub fn duplicate_list(&self, id: Id, suffix: &str) -> Result<List> {
        let list = self.lists()?.into_iter().find(|l| l.id == id).context("list not found")?;
        let draft = ListDraft { name: format!("{}{suffix}", list.name), ..ListDraft::of(&list) };
        let copy = self.add_list(&draft)?;
        self.place_list(copy.id, Some(id), list.folder)?;
        for (name, _) in self.list_sections(Some(id))?.into_iter().map(|s| (s.name, s.id)) {
            self.add_list_section(Some(copy.id), &name)?;
        }
        let tasks = self.tasks()?;
        let roots = tasks.iter().filter(|t| t.list_id == Some(id) && t.parent_id.is_none() && !t.is_completed() && !t.is_deleted());
        for task in roots {
            let new = self.duplicate_task(task.id)?;
            self.set_list(new.id, Some(copy.id))?;
        }
        self.lists()?.into_iter().find(|l| l.id == copy.id).context("list not found")
    }

    /// Moves a list right after `after` (first when `None`) and into `folder`.
    pub fn place_list(&self, id: Id, after: Option<Id>, folder: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            let lists = self.lists()?;
            if !lists.iter().any(|l| l.id == id) {
                return Ok(());
            }
            if let Some(fid) = folder {
                anyhow::ensure!(
                    self.list_folders()?.iter().any(|f| f.id == fid),
                    "target folder not found"
                );
            }
            let mut order: Vec<Id> = lists.into_iter().map(|l| l.id).filter(|l| *l != id).collect();
            let at = after.and_then(|a| order.iter().position(|l| *l == a)).map_or(0, |i| i + 1);
            order.insert(at, id);
            self.write_sort_order(SortTable::Lists, &order)?;
            self.conn.execute(
                "UPDATE lists SET folder_id = ?2 WHERE id = ?1",
                params![id.to_string(), folder.map(|f| f.to_string())],
            )?;
            Ok(())
        })
    }

    /// Groups active lists into visual order so each folder's active lists sit contiguously at
    /// the position of its first active list, followed by archived lists in their existing order.
    fn visual_list_order(lists: &[List], folders: &[ListFolder]) -> Vec<Id> {
        let mut result = Vec::with_capacity(lists.len());
        let mut shown_folders: HashSet<Id> = HashSet::new();
        for list in lists.iter().filter(|l| !l.archived) {
            match list.folder.filter(|f| folders.iter().any(|x| x.id == *f)) {
                None => result.push(list.id),
                Some(fid) if shown_folders.insert(fid) => {
                    result.extend(
                        lists.iter().filter(|l| !l.archived && l.folder == Some(fid)).map(|l| l.id),
                    );
                }
                Some(_) => {}
            }
        }
        result.extend(lists.iter().filter(|l| l.archived).map(|l| l.id));
        result
    }

    /// Moves a list right before (`!after`) or right after (`after`) `target`, setting its `folder`.
    pub fn place_list_relative(&self, id: Id, target: Id, after: bool, folder: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            let lists = self.lists()?;
            if !lists.iter().any(|l| l.id == id) {
                return Ok(());
            }
            let folders = self.list_folders()?;
            if let Some(fid) = folder {
                anyhow::ensure!(folders.iter().any(|f| f.id == fid), "target folder not found");
            }
            if id == target {
                self.conn.execute(
                    "UPDATE lists SET folder_id = ?2 WHERE id = ?1",
                    params![id.to_string(), folder.map(|f| f.to_string())],
                )?;
                return Ok(());
            }
            let mut order: Vec<Id> =
                Self::visual_list_order(&lists, &folders).into_iter().filter(|l| *l != id).collect();
            let Some(pos) = order.iter().position(|l| *l == target) else {
                return self.place_list(id, None, folder);
            };
            order.insert(pos + usize::from(after), id);
            self.write_sort_order(SortTable::Lists, &order)?;
            self.conn.execute(
                "UPDATE lists SET folder_id = ?2 WHERE id = ?1",
                params![id.to_string(), folder.map(|f| f.to_string())],
            )?;
            Ok(())
        })
    }

    /// Moves a list out of any folder and places it right before (`!after`) or right after (`after`)
    /// `folder`'s block of active lists.
    pub fn place_list_around_folder(&self, id: Id, folder: Id, after: bool) -> Result<()> {
        self.in_tx(|| {
            let lists = self.lists()?;
            let Some(dragged_list) = lists.iter().find(|l| l.id == id) else {
                return Ok(());
            };
            let folders = self.list_folders()?;
            let was_in_folder = dragged_list.folder == Some(folder);
            let visual = Self::visual_list_order(&lists, &folders);
            let active_by_id: HashMap<Id, &List> = lists.iter().map(|l| (l.id, l)).collect();
            let mut remaining: Vec<Id> = visual.iter().copied().filter(|&lid| lid != id).collect();
            let first = remaining.iter().position(|lid| active_by_id.get(lid).is_some_and(|l| !l.archived && l.folder == Some(folder)));
            let last = remaining.iter().rposition(|lid| active_by_id.get(lid).is_some_and(|l| !l.archived && l.folder == Some(folder)));
            let insert_at = match (first, last) {
                (Some(f), Some(l)) => if !after { f } else { l + 1 },
                _ if was_in_folder => visual.iter().position(|&lid| lid == id).unwrap_or(remaining.len()).min(remaining.len()),
                _ => remaining
                    .iter()
                    .rposition(|lid| active_by_id.get(lid).is_some_and(|l| !l.archived))
                    .map_or(0, |p| p + 1),
            };
            remaining.insert(insert_at, id);
            self.write_sort_order(SortTable::Lists, &remaining)?;
            self.conn.execute("UPDATE lists SET folder_id = NULL WHERE id = ?1", [id.to_string()])?;
            Ok(())
        })
    }

    /// Drops `dragged` list onto top-level `target` list, creating a new folder containing both
    /// at `target`'s position (matching TickTick's `createProjectGroup`).
    pub fn group_lists_into_folder(&self, dragged: Id, target: Id, folder_name: &str) -> Result<ListFolder> {
        anyhow::ensure!(dragged != target, "cannot group a list with itself");
        let name = folder_name.trim();
        anyhow::ensure!(!name.is_empty(), "a folder needs a name");
        self.in_tx(|| {
            let lists = self.lists()?;
            let folders = self.list_folders()?;
            let target_list = lists.iter().find(|l| l.id == target).context("target list not found")?;
            let dragged_list = lists.iter().find(|l| l.id == dragged).context("dragged list not found")?;
            anyhow::ensure!(!target_list.archived && !dragged_list.archived, "cannot group archived lists");
            anyhow::ensure!(target_list.folder.is_none(), "target list is already inside a folder");

            let mut order: Vec<Id> =
                Self::visual_list_order(&lists, &folders).into_iter().filter(|&id| id != dragged).collect();
            let target_pos = order.iter().position(|&id| id == target).context("target list not found")?;
            order.insert(target_pos + 1, dragged);

            let folder = ListFolder { id: Uuid::now_v7(), name: name.to_owned() };
            self.conn.execute(
                "INSERT INTO list_folders (id, name, created_at, sort_order) VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM list_folders))",
                params![folder.id.to_string(), folder.name, now_ms()],
            )?;
            self.write_sort_order(SortTable::Lists, &order)?;
            let folder_id_str = folder.id.to_string();
            self.conn.execute(
                "UPDATE lists SET folder_id = ?1 WHERE id IN (?2, ?3)",
                params![folder_id_str, target.to_string(), dragged.to_string()],
            )?;
            Ok(folder)
        })
    }

    /// Reorders an entire folder (and all its active lists together) before (`!after`) or after
    /// (`after`) another top-level list or folder.
    pub fn reorder_list_folder(&self, dragged: Id, target: FolderDropTarget, after: bool) -> Result<()> {
        if target == FolderDropTarget::Folder(dragged) {
            return Ok(());
        }
        self.in_tx(|| {
            let lists = self.lists()?;
            let folders = self.list_folders()?;
            if !folders.iter().any(|f| f.id == dragged) {
                return Ok(());
            }

            let moving: Vec<Id> = lists.iter().filter(|l| !l.archived && l.folder == Some(dragged)).map(|l| l.id).collect();
            let remaining_lists: Vec<List> = lists.into_iter().filter(|l| l.archived || l.folder != Some(dragged)).collect();

            if let FolderDropTarget::List(target_list) = target {
                let Some(tl) = remaining_lists.iter().find(|l| l.id == target_list) else {
                    return Ok(());
                };
                if tl.archived || tl.folder.is_some() {
                    return Ok(());
                }
            }

            if !moving.is_empty() {
                let mut visual = Self::visual_list_order(&remaining_lists, &folders);
                let by_id: HashMap<Id, &List> = remaining_lists.iter().map(|l| (l.id, l)).collect();
                let insert_at = match target {
                    FolderDropTarget::List(target_list) => {
                        let Some(pos) = visual.iter().position(|&id| id == target_list) else {
                            return Ok(());
                        };
                        pos + usize::from(after)
                    }
                    FolderDropTarget::Folder(target_folder) => {
                        let pos = if !after {
                            visual.iter().position(|id| by_id.get(id).is_some_and(|l| !l.archived && l.folder == Some(target_folder)))
                        } else {
                            visual.iter().rposition(|id| by_id.get(id).is_some_and(|l| !l.archived && l.folder == Some(target_folder))).map(|p| p + 1)
                        };
                        pos.unwrap_or_else(|| {
                            visual.iter().rposition(|id| by_id.get(id).is_some_and(|l| !l.archived)).map_or(0, |p| p + 1)
                        })
                    }
                };
                visual.splice(insert_at..insert_at, moving);
                self.write_sort_order(SortTable::Lists, &visual)?;
            }

            let mut folder_order: Vec<Id> = folders.iter().map(|f| f.id).filter(|&id| id != dragged).collect();
            let folder_insert = match target {
                FolderDropTarget::Folder(target_folder) => folder_order
                    .iter()
                    .position(|&id| id == target_folder)
                    .map_or(folder_order.len(), |pos| pos + usize::from(after)),
                FolderDropTarget::List(_) => if after { folder_order.len() } else { 0 },
            };
            folder_order.insert(folder_insert, dragged);
            self.write_sort_order(SortTable::ListFolders, &folder_order)?;
            Ok(())
        })
    }

    /// Commits live-reordered lists (including `folder` membership) and `list_folders` order in a
    /// single transaction.
    pub fn save_list_and_folder_order(&self, lists: &[List], folders: &[ListFolder]) -> Result<()> {
        self.in_tx(|| {
            let visual = Self::visual_list_order(lists, folders);
            let by_id: HashMap<Id, &List> = lists.iter().map(|l| (l.id, l)).collect();
            let valid_folders: HashSet<Id> = folders.iter().map(|f| f.id).collect();
            for (i, list_id) in visual.iter().enumerate() {
                if let Some(list) = by_id.get(list_id) {
                    let folder = list.folder.filter(|f| valid_folders.contains(f));
                    self.conn.execute(
                        "UPDATE lists SET sort_order = ?2, folder_id = ?3
                         WHERE id = ?1 AND (sort_order != ?2 OR folder_id IS NOT ?3)",
                        params![list_id.to_string(), i as i64, folder.map(|f| f.to_string())],
                    )?;
                }
            }
            let folder_ids: Vec<Id> = folders.iter().map(|f| f.id).collect();
            self.write_sort_order(SortTable::ListFolders, &folder_ids)?;
            Ok(())
        })
    }

    /// The folders of lists, in their order.
    pub fn list_folders(&self) -> Result<Vec<ListFolder>> {
        let mut stmt = self.conn.prepare_cached("SELECT id, name FROM list_folders ORDER BY sort_order, created_at, rowid")?;
        let folders = stmt
            .query_map([], |row| Ok(ListFolder { id: parse_uuid(&row.get::<_, String>(0)?, 0)?, name: row.get(1)? }))?
            .collect::<Result<_, _>>()?;
        Ok(folders)
    }

    pub fn add_list_folder(&self, name: &str) -> Result<ListFolder> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a folder needs a name");
        let folder = ListFolder { id: Uuid::now_v7(), name: name.to_owned() };
        self.conn.execute(
            "INSERT INTO list_folders (id, name, created_at, sort_order) VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM list_folders))",
            params![folder.id.to_string(), folder.name, now_ms()],
        )?;
        Ok(folder)
    }

    pub fn rename_list_folder(&self, id: Id, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a folder needs a name");
        self.conn.execute("UPDATE list_folders SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(())
    }

    /// The original's Ungroup: the folder goes, its lists stay, out of any folder.
    pub fn ungroup_list_folder(&self, id: Id) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute("UPDATE lists SET folder_id = NULL WHERE folder_id = ?1", [id.to_string()])?;
            self.conn.execute("DELETE FROM list_folders WHERE id = ?1", [id.to_string()])?;
            Ok(())
        })
    }

    /// Lists whose tasks stay out of the smart lists: archived, completed projects, and those set not to show.
    fn hidden_lists(&self) -> Result<HashSet<Id>> {
        Ok(self
            .lists()?
            .into_iter()
            .filter(|l| l.archived || l.completed_at.is_some() || !l.in_smart_lists)
            .map(|l| l.id)
            .collect())
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

    /// Deletes the list together with its tasks and unlinks orphaned attachment files when an
    /// attachment directory is configured on this store.
    pub fn delete_list(&self, id: Id) -> Result<()> {
        self.delete_list_with_dir(id, None)
    }

    /// Deletes the list together with its tasks, pruning `sync_uploaded` and unlinking orphaned
    /// attachment files in `dir` (or the store's default attachment directory).
    pub fn delete_list_with_dir(&self, id: Id, dir: Option<&Path>) -> Result<()> {
        let removed_files = self.in_tx(|| {
            let files = self.collect_attachment_files_for_query(
                "WITH RECURSIVE sub(id) AS (
                     SELECT id FROM tasks WHERE list_id = ?1
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )
                 SELECT a.id || '-' || a.name FROM task_attachments a JOIN sub ON a.task_id = sub.id",
                params![id.to_string()],
            )?;
            self.conn.execute(
                "WITH RECURSIVE sub(id) AS (
                     SELECT id FROM tasks WHERE list_id = ?1
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )
                 DELETE FROM plugin_entity_meta
                 WHERE entity_type = 'task' AND entity_id IN (SELECT id FROM sub)",
                [id.to_string()],
            )?;
            self.conn.execute("DELETE FROM lists WHERE id = ?1", [id.to_string()])?;
            self.prune_sync_uploaded(&files)?;
            Ok(files)
        })?;
        self.unlink_attachment_files(&removed_files, dir);
        Ok(())
    }

    /// Moves the task and its subtasks to a list (`None` means the Inbox).
    /// Moving a subtask to a different list than its parent unlinks it from that parent.
    pub fn set_list(&self, id: Id, list: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            let Some(task) = self.task(id)? else { return Ok(()) };
            let list_str = list.map(|l| l.to_string());
            let unlink = match task.parent_id {
                Some(parent_id) => {
                    let parent_list: Option<Option<String>> = self
                        .conn
                        .prepare_cached("SELECT list_id FROM tasks WHERE id = ?1")?
                        .query_row([parent_id.to_string()], |row| row.get(0))
                        .optional()?;
                    parent_list.is_none_or(|pl| pl != list_str)
                }
                None => false,
            };
            if task.list_id == list && !unlink {
                return Ok(());
            }
            if unlink {
                self.conn.execute("UPDATE tasks SET parent_id = NULL WHERE id = ?1", [id.to_string()])?;
            }
            let list_changed = task.list_id != list;
            let sql = if list_changed {
                // A section belongs to its list, so moving to another list leaves it.
                "UPDATE tasks SET list_id = ?2, section_id = NULL WHERE id = ?1"
            } else {
                "UPDATE tasks SET list_id = ?2 WHERE id = ?1"
            };
            let mut stmt = self.conn.prepare_cached(sql)?;
            for sub in std::iter::once(id).chain(self.descendants(id)?) {
                stmt.execute(params![sub.to_string(), list_str])?;
            }
            Ok(())
        })
    }

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

    fn checklist_progress(&self) -> Result<HashMap<Id, (usize, usize)>> {
        let mut stmt = self.conn.prepare_cached(
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
            self.conn.prepare_cached("SELECT id, name, date, kind, icon, color, repeat, pinned_at, show_age, reminders, constant_reminder, show_in_list, plus_one_day, count_up, archived_at, background FROM countdowns
             ORDER BY pinned_at IS NULL, pinned_at DESC, created_at, rowid")?;
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
                    icon: row.get(4)?,
                    color: row.get(5)?,
                    repeat: row.get::<_, Option<String>>(6)?.and_then(|rule| rule.parse().ok()),
                    pinned_at: row.get(7)?,
                    show_age: row.get(8)?,
                    reminders: crate::countdown::parse_reminders(&row.get::<_, String>(9)?),
                    custom_reminders: crate::countdown::parse_custom_reminders(&row.get::<_, String>(9)?),
                    constant_reminder: row.get(10)?,
                    show_in_list: crate::countdown::ShowInList::parse(&row.get::<_, String>(11)?),
                    plus_one_day: row.get(12)?,
                    count_up: row.get(13)?,
                    archived: row.get::<_, Option<i64>>(14)?.is_some(),
                    background: row.get(15)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn add_countdown(&self, name: &str, date: NaiveDate, kind: CountdownKind) -> Result<Countdown> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a countdown needs a name");
        let countdown = Countdown { id: Uuid::now_v7(), name: name.to_owned(), date, kind, icon: None, color: None, repeat: None, pinned_at: None, show_age: false, reminders: Vec::new(), custom_reminders: Vec::new(), constant_reminder: false, show_in_list: Default::default(), plus_one_day: false, count_up: false, archived: false, background: None };
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

    /// Pins a countdown to the front of the grid (the latest pinned first), or unpins it.
    pub fn set_countdown_pinned(&self, id: Id, pinned: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE countdowns SET pinned_at = ?2 WHERE id = ?1",
            params![id.to_string(), pinned.then(now_ms)],
        )?;
        Ok(())
    }

    /// Edits a countdown: its name, date, kind and repeat.
    pub fn update_countdown(
        &self,
        id: Id,
        name: &str,
        date: NaiveDate,
        kind: CountdownKind,
        repeat: Option<&crate::recurrence::Recurrence>,
    ) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a countdown needs a name");
        self.conn.execute(
            "UPDATE countdowns SET name = ?2, date = ?3, kind = ?4, repeat = ?5 WHERE id = ?1",
            params![id.to_string(), name, date.format(DATE_FORMAT).to_string(), kind.as_str(), repeat.map(|r| r.to_string())],
        )?;
        Ok(())
    }

    /// A countdown's Reminder: the days before its date to be reminded, and whether they stay
    /// on screen until answered.
    pub fn set_countdown_reminders(&self, id: Id, days: &[u32], custom: &[(u32, chrono::NaiveTime)], constant: bool) -> Result<()> {
        let text = crate::countdown::reminders_to_storage(days, custom);
        self.conn.execute(
            "UPDATE countdowns SET reminders = ?2, constant_reminder = ?3 WHERE id = ?1",
            params![id.to_string(), text, constant],
        )?;
        Ok(())
    }

    /// A countdown's Show (in the Today list), Day Calculation Mode ("Standard +1 day") and a
    /// birthday's Counting Mode ("Count Up").
    pub fn set_countdown_display(&self, id: Id, show: crate::countdown::ShowInList, plus_one_day: bool, count_up: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE countdowns SET show_in_list = ?2, plus_one_day = ?3, count_up = ?4 WHERE id = ?1",
            params![id.to_string(), show.key(), plus_one_day, count_up],
        )?;
        Ok(())
    }

    /// A countdown's background picture: the file is copied into `dir` (the app's attachment
    /// folder) as `<id>-background-<unique>.<ext>` — a new name for every picture, so a changed
    /// one syncs; `None` takes the picture off. Returns the file's name.
    pub fn set_countdown_background(&self, id: Id, file: Option<&std::path::Path>, dir: &std::path::Path) -> Result<Option<String>> {
        let name = match file {
            Some(file) => {
                let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("png");
                let name = format!("{id}-background-{}.{ext}", Uuid::now_v7().simple());
                std::fs::create_dir_all(dir)?;
                std::fs::copy(file, dir.join(&name))?;
                Some(name)
            }
            None => None,
        };
        self.conn.execute("UPDATE countdowns SET background = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(name)
    }

    /// Archives a countdown (kept, listed under Archived) or restores it.
    pub fn set_countdown_archived(&self, id: Id, archived: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE countdowns SET archived_at = ?2 WHERE id = ?1",
            params![id.to_string(), archived.then(now_ms)],
        )?;
        Ok(())
    }

    /// A birthday's "Show Age".
    pub fn set_countdown_show_age(&self, id: Id, on: bool) -> Result<()> {
        self.conn.execute("UPDATE countdowns SET show_age = ?2 WHERE id = ?1", params![id.to_string(), on])?;
        Ok(())
    }

    /// A countdown's icon (an emoji or short text) and its background color; empty clears.
    pub fn set_countdown_look(&self, id: Id, icon: Option<&str>, color: Option<&str>) -> Result<()> {
        let icon = icon.map(str::trim).filter(|i| !i.is_empty());
        self.conn.execute(
            "UPDATE countdowns SET icon = ?2, color = ?3 WHERE id = ?1",
            params![id.to_string(), icon, color],
        )?;
        Ok(())
    }

    /// Copies a file or archives a directory in `dir` and lists it on the task.
    pub fn add_attachment(&self, task: Id, source: &Path, dir: &Path) -> Result<Attachment> {
        self.save_attachment(PreparedAttachment::copy(task, source, dir)?, dir)
    }

    /// Publishes a staged copy and records it on the task. Staging can run away from the UI.
    pub fn save_attachment(&self, mut prepared: PreparedAttachment, dir: &Path) -> Result<Attachment> {
        let attachment = prepared.metadata().clone();
        prepared.publish(dir)?;
        let inserted = self.conn.execute(
            "INSERT INTO task_attachments (id, task_id, name, size, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM tasks WHERE id = ?2 AND deleted_at IS NULL)",
            params![attachment.id.to_string(), attachment.task_id.to_string(), attachment.name, attachment.size as i64, attachment.created_at],
        ).map_err(anyhow::Error::from).and_then(|count| {
            anyhow::ensure!(count == 1, "cannot attach to a deleted or missing task");
            Ok(())
        });
        inserted?;
        prepared.keep_publication();
        Ok(attachment)
    }

    /// A task's attachments, oldest first.
    pub fn attachments(&self, task: Id) -> Result<Vec<Attachment>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, task_id, name, size, created_at FROM task_attachments WHERE task_id = ?1 ORDER BY created_at, rowid",
        )?;
        let rows = stmt
            .query_map([task.to_string()], |row| {
                Ok(Attachment {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    task_id: parse_uuid(&row.get::<_, String>(1)?, 1)?,
                    name: row.get(2)?,
                    size: row.get::<_, i64>(3)? as u64,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Removes an attachment and its copy in `dir`.
    pub fn delete_attachment(&self, attachment: &Attachment, dir: &std::path::Path) -> Result<()> {
        let storage_name = format!("{}-{}", attachment.id, attachment.name);
        self.in_tx(|| {
            self.conn.execute("DELETE FROM task_attachments WHERE id = ?1", [attachment.id.to_string()])?;
            self.prune_sync_uploaded(std::slice::from_ref(&storage_name))?;
            Ok(())
        })?;
        let file = attachment.path(dir);
        if let Err(err) = std::fs::remove_file(file)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            return Err(err.into());
        }
        Ok(())
    }

    fn collect_attachment_files_for_query<P: rusqlite::Params>(&self, sql: &str, params: P) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let files = stmt.query_map(params, |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(files)
    }

    fn prune_sync_uploaded(&self, files: &[String]) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }
        let mut stmt = self.conn.prepare_cached("DELETE FROM sync_uploaded WHERE name = ?1")?;
        for file in files {
            stmt.execute([file])?;
        }
        Ok(())
    }

    fn unlink_attachment_files(&self, files: &[String], dir: Option<&Path>) {
        let Some(dir) = dir.or(self.attachments_dir.as_deref()) else {
            return;
        };
        for name in files {
            if name.is_empty() || name.contains(['/', '\\', '\0']) {
                continue;
            }
            let _ = std::fs::remove_file(dir.join(name));
        }
    }

    /// Removes unreferenced files in `dir` (keeping files referenced by `task_attachments` or
    /// `countdowns.background`, and skipping subdirectories like `.pending`) and prunes stale
    /// `sync_uploaded` rows. Returns how many orphan files were deleted from disk.
    pub fn gc_orphan_attachments(&self, dir: &Path) -> Result<usize> {
        let mut live: HashSet<String> = HashSet::new();
        {
            let mut stmt = self.conn.prepare_cached("SELECT id || '-' || name FROM task_attachments")?;
            for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
                live.insert(row?);
            }
        }
        {
            let mut stmt = self.conn.prepare_cached("SELECT background FROM countdowns WHERE background IS NOT NULL")?;
            for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
                live.insert(row?);
            }
        }

        // Prune stale sync_uploaded entries no longer referenced in SQLite.
        let stale_uploaded: Vec<String> = {
            let mut stmt = self.conn.prepare_cached("SELECT name FROM sync_uploaded")?;
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|name| !live.contains(name))
                .collect()
        };
        if !stale_uploaded.is_empty() {
            self.in_tx(|| self.prune_sync_uploaded(&stale_uploaded))?;
        }

        if !dir.is_dir() {
            return Ok(0);
        }
        let mut removed = 0usize;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !live.contains(&name) {
                match std::fs::remove_file(entry.path()) {
                    Ok(()) => removed += 1,
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err.into()),
                }
            }
        }
        Ok(removed)
    }

    pub fn delete_countdown(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM countdowns WHERE id = ?1", [id.to_string()])?;
        Ok(())
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

    /// A user setting such as the UI language; `None` when never set.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0))
            .optional()?)
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
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

    /// Moves the task and its subtasks to the trash.
    pub fn delete_task(&self, id: Id) -> Result<()> {
        self.set_deleted(id, Some(now_ms()))
    }

    /// Brings the task and its subtasks back from the trash. If the task's parent is still in
    /// the trash or is a note, detaches `id` to top-level so emptying trash cannot cascade-delete it.
    pub fn restore_task(&self, id: Id) -> Result<()> {
        self.in_tx(|| {
            let parent_invalid = match self.task(id)?.and_then(|t| t.parent_id) {
                Some(parent_id) => self
                    .task(parent_id)?
                    .is_none_or(|p| p.is_deleted() || p.kind == Kind::Note),
                None => false,
            };
            if parent_invalid {
                self.conn.execute("UPDATE tasks SET parent_id = NULL WHERE id = ?1", [id.to_string()])?;
            }
            self.set_deleted(id, None)
        })
    }

    /// Deletes the task and its subtasks permanently, unlinking orphaned attachment files when an
    /// attachment directory is configured on this store.
    pub fn purge_task(&self, id: Id) -> Result<()> {
        self.purge_task_with_dir(id, None)
    }

    /// Deletes the task and its subtasks permanently, pruning `sync_uploaded` and unlinking
    /// orphaned attachment files in `dir` (or the store's default attachment directory).
    pub fn purge_task_with_dir(&self, id: Id, dir: Option<&Path>) -> Result<()> {
        let removed_files = self.in_tx(|| {
            let files = self.collect_attachment_files_for_query(
                "WITH RECURSIVE sub(id) AS (
                     SELECT ?1
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )
                 SELECT a.id || '-' || a.name FROM task_attachments a JOIN sub ON a.task_id = sub.id",
                params![id.to_string()],
            )?;
            self.conn.execute(
                "WITH RECURSIVE sub(id) AS (
                     SELECT ?1
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )
                 DELETE FROM plugin_entity_meta
                 WHERE entity_type = 'task' AND entity_id IN (SELECT id FROM sub)",
                [id.to_string()],
            )?;
            self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id.to_string()])?;
            self.prune_sync_uploaded(&files)?;
            Ok(files)
        })?;
        self.unlink_attachment_files(&removed_files, dir);
        Ok(())
    }

    /// Permanently deletes everything in the trash, unlinking orphaned attachment files when an
    /// attachment directory is configured on this store.
    pub fn empty_trash(&self) -> Result<()> {
        self.empty_trash_with_dir(None)
    }

    /// Permanently deletes everything in the trash, pruning `sync_uploaded` and unlinking
    /// orphaned attachment files in `dir` (or the store's default attachment directory).
    pub fn empty_trash_with_dir(&self, dir: Option<&Path>) -> Result<()> {
        let removed_files = self.in_tx(|| {
            let files = self.collect_attachment_files_for_query(
                "WITH RECURSIVE sub(id) AS (
                     SELECT id FROM tasks WHERE deleted_at IS NOT NULL
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )
                 SELECT a.id || '-' || a.name FROM task_attachments a JOIN sub ON a.task_id = sub.id",
                [],
            )?;
            self.conn.execute(
                "WITH RECURSIVE sub(id) AS (
                     SELECT id FROM tasks WHERE deleted_at IS NOT NULL
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )
                 DELETE FROM plugin_entity_meta
                 WHERE entity_type = 'task' AND entity_id IN (SELECT id FROM sub)",
                [],
            )?;
            self.conn.execute("DELETE FROM tasks WHERE deleted_at IS NOT NULL", [])?;
            self.prune_sync_uploaded(&files)?;
            Ok(files)
        })?;
        self.unlink_attachment_files(&removed_files, dir);
        Ok(())
    }


    fn set_deleted(&self, id: Id, deleted_at: Option<i64>) -> Result<()> {
        self.in_tx(|| {
            let mut stmt = self.conn.prepare_cached("UPDATE tasks SET deleted_at = ?2 WHERE id = ?1")?;
            for task in std::iter::once(id).chain(self.descendants(id)?) {
                stmt.execute(params![task.to_string(), deleted_at])?;
            }
            Ok(())
        })
    }

    fn update(&self, id: Id, column: &'static str, value: impl rusqlite::ToSql) -> Result<()> {
        self.conn.execute(
            &format!("UPDATE tasks SET {column} = ?2 WHERE id = ?1"),
            params![id.to_string(), value],
        )?;
        Ok(())
    }

    /// Subtasks of a task, at any depth (cycle-safe via `UNION` deduplication).
    pub fn descendants(&self, id: Id) -> Result<Vec<Id>> {
        let mut stmt = self.conn.prepare_cached(
            "WITH RECURSIVE sub(id) AS (
                 SELECT id FROM tasks WHERE parent_id = ?1
                 UNION
                 SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
             )
             SELECT id FROM sub",
        )?;
        let ids = stmt
            .query_map([id.to_string()], |row| {
                let raw: String = row.get(0)?;
                parse_uuid(&raw, 0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
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
#[cfg(test)]
mod tests;
