use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context as _, Result};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike as _};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::attachment::PreparedAttachment;
use crate::horizon::Horizon;
use crate::model::{Attachment, Id, Kind, Priority, ScheduleBucket, Task, TaskDraft};
use crate::recurrence::Recurrence;

use super::{DATE_FORMAT, SNOOZE_FORMAT, Store, TIME_FORMAT, now_ms, parse_uuid};

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

impl Store {
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
            } else {
                let _ = prepared.rollback_publication();
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
            let mut statement = self.conn.prepare_cached(
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
        self.in_tx(|| {
            if let Some(ref rule) = repeat
                && let Some(task) = self.task(id)?
                && task.due_date.is_none()
                && !task.horizon.is_some_and(|h| h.advance_for_recurrence(rule).is_some())
            {
                self.set_due(id, Some(today), task.due_time)?;
            }
            self.update(id, "repeat", repeat.map(|r| r.to_string()))
        })
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
        self.in_tx(|| {
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
        })
    }

    /// Resizes an all-day / date-span task to `[start, end]` inclusive.
    pub fn resize_dates(&self, id: Id, start: NaiveDate, end: NaiveDate) -> Result<()> {
        self.in_tx(|| {
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
        })
    }

    /// Resizes a timed task block to `[date start .. end_date end]`.
    pub fn resize_timed_range(&self, id: Id, date: NaiveDate, start: NaiveTime, end: NaiveTime, end_date: Option<NaiveDate>) -> Result<()> {
        self.in_tx(|| {
            let exists: Option<i64> = self
                .conn
                .prepare_cached("SELECT 1 FROM tasks WHERE id = ?1")?
                .query_row([id.to_string()], |row| row.get(0))
                .optional()?;
            exists.context("task not found")?;
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
        })
    }

    /// "Estimation": the expected pomos or minutes; `None` clears it.
    pub fn set_estimate(&self, id: Id, estimate: Option<crate::model::Estimate>) -> Result<()> {
        self.update(id, "estimate", estimate.map(crate::model::Estimate::to_storage))
    }

    /// Pins a task to the top of its lists, or unpins it.
    pub fn set_pinned(&self, id: Id, pinned: bool) -> Result<()> {
        self.update(id, "pinned_at", pinned.then(now_ms))
    }

    pub fn set_reminders(&self, id: Id, reminders: &[i64]) -> Result<()> {
        self.update(id, "reminders", crate::reminder::format(reminders))
    }

    /// Sets the due date. A time without a date is meaningless and is dropped.
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
            let mut select = self.conn.prepare_cached(
                "SELECT due_date, end_date FROM tasks
                 WHERE id = ?1 AND deleted_at IS NULL AND completed_at IS NULL AND wont_do_at IS NULL",
            )?;
            let mut update = self.conn.prepare_cached(
                "UPDATE tasks
                 SET due_date = ?2, end_date = ?3, horizon = NULL, snoozed_until = NULL
                 WHERE id = ?1 AND deleted_at IS NULL AND completed_at IS NULL AND wont_do_at IS NULL",
            )?;
            for &id in ids {
                let id_str = id.to_string();
                let Some((old_due_str, old_end_str)): Option<(Option<String>, Option<String>)> = select
                    .query_row([&id_str], |row| Ok((row.get(0)?, row.get(1)?)))
                    .optional()?
                else {
                    continue;
                };
                let old_due = old_due_str.and_then(|s| NaiveDate::parse_from_str(&s, DATE_FORMAT).ok());
                let old_end = old_end_str.and_then(|s| NaiveDate::parse_from_str(&s, DATE_FORMAT).ok());
                let next_end = match (old_due, old_end) {
                    (Some(old_due), Some(old_end)) => old_end
                        .checked_add_signed(day - old_due)
                        .map(|d| d.format(DATE_FORMAT).to_string()),
                    (_, Some(old_end)) => Some(old_end.format(DATE_FORMAT).to_string()),
                    _ => None,
                };
                update.execute(params![&id_str, &day_str, next_end])?;
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

    /// Records metadata for a file already encrypted and uploaded directly to Fern Cloud
    /// (used by browser WASM builds without a host filesystem attachments directory).
    pub fn save_cloud_attachment_record(&self, attachment: &Attachment) -> Result<Attachment> {
        let count = self.conn.execute(
            "INSERT INTO task_attachments (id, task_id, name, size, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM tasks WHERE id = ?2 AND deleted_at IS NULL)",
            params![
                attachment.id.to_string(),
                attachment.task_id.to_string(),
                attachment.name,
                attachment.size as i64,
                attachment.created_at,
            ],
        )?;
        anyhow::ensure!(count == 1, "cannot attach to a deleted or missing task");
        Ok(attachment.clone())
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

    pub(super) fn collect_attachment_files_for_query<P: rusqlite::Params>(&self, sql: &str, params: P) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let files = stmt.query_map(params, |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(files)
    }

    pub(super) fn prune_sync_uploaded(&self, files: &[String]) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }
        let mut stmt = self.conn.prepare_cached("DELETE FROM sync_uploaded WHERE name = ?1")?;
        for file in files {
            stmt.execute([file])?;
        }
        Ok(())
    }

    pub(super) fn unlink_attachment_files(&self, files: &[String], dir: Option<&Path>) {
        let Some(dir) = dir.or_else(|| self.attachments_dir()) else {
            return;
        };
        for name in files {
            if !super::is_safe_attachment_name(name) {
                continue;
            }
            let _ = std::fs::remove_file(dir.join(name));
        }
    }

    pub(super) fn cleanup_task_subtree_artifacts(
        &self,
        sub_cte_sql: &str,
        params: impl rusqlite::Params + Clone,
    ) -> Result<Vec<String>> {
        let files_sql = format!(
            "{sub_cte_sql} SELECT a.id || '-' || a.name FROM task_attachments a JOIN sub ON a.task_id = sub.id"
        );
        let files = self.collect_attachment_files_for_query(&files_sql, params.clone())?;
        let meta_sql = format!(
            "{sub_cte_sql} DELETE FROM plugin_entity_meta WHERE entity_type = 'task' AND entity_id IN (SELECT id FROM sub)"
        );
        self.conn.execute(&meta_sql, params)?;
        self.prune_sync_uploaded(&files)?;
        Ok(files)
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
            let files = self.cleanup_task_subtree_artifacts(
                "WITH RECURSIVE sub(id) AS (
                     SELECT ?1
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )",
                params![id.to_string()],
            )?;
            self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id.to_string()])?;
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
            let files = self.cleanup_task_subtree_artifacts(
                "WITH RECURSIVE sub(id) AS (
                     SELECT id FROM tasks WHERE deleted_at IS NOT NULL
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )",
                [],
            )?;
            self.conn.execute("DELETE FROM tasks WHERE deleted_at IS NOT NULL", [])?;
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
}
