use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone as _, Timelike as _};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::attachment::PreparedAttachment;
use crate::countdown::{Countdown, CountdownKind};
use crate::focus::FocusSession;
use crate::habit::{Checkin, CheckinMode, CheckinStatus, Goal, Habit, HabitDraft, HabitSection, MAX_CUSTOM_SECTIONS, MAX_LOG_NOTE_CHARS, MAX_REMINDERS, MOODS, polish_amount};
use crate::model::{Attachment, ChecklistItem, Id, Kind, List, ListDraft, ListFolder, Priority, ScheduleBucket, Tag, Task, TaskDraft, View, ViewCounts};
use crate::recurrence::Recurrence;

/// Maximum nesting depth for subtasks (`0..=4`, i.e. 5 levels total).
pub const MAX_SUBTASK_DEPTH: usize = 4;

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
    "
    ALTER TABLE habits ADD COLUMN frequency TEXT;
    ",
    "
    ALTER TABLE habits ADD COLUMN icon TEXT;
    ALTER TABLE habits ADD COLUMN color TEXT;
    ",
    "
    ALTER TABLE habits ADD COLUMN start_date TEXT;
    ALTER TABLE habits ADD COLUMN goal_days INTEGER;
    ",
    "
    ALTER TABLE habits ADD COLUMN section TEXT;
    ALTER TABLE habits ADD COLUMN archived_at INTEGER;
    ",
    "
    CREATE TABLE habit_sections (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    ",
    // When a task last changed, kept by triggers so every write path counts: its fields (not
    // its position in the list), its tags and its checklist.
    "
    ALTER TABLE tasks ADD COLUMN modified_at INTEGER;
    CREATE TRIGGER tasks_modified AFTER UPDATE OF
        title, notes, priority, kind, parent_id, list_id, due_date, due_time, end_date, end_time,
        repeat, reminders, completed_at, deleted_at, wont_do_at ON tasks
    BEGIN
        UPDATE tasks SET modified_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = NEW.id;
    END;
    CREATE TRIGGER task_tags_added AFTER INSERT ON task_tags
    BEGIN
        UPDATE tasks SET modified_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = NEW.task_id;
    END;
    CREATE TRIGGER task_tags_removed AFTER DELETE ON task_tags
    BEGIN
        UPDATE tasks SET modified_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = OLD.task_id;
    END;
    CREATE TRIGGER checklist_changed AFTER UPDATE ON checklist_items
    BEGIN
        UPDATE tasks SET modified_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = NEW.task_id;
    END;
    CREATE TRIGGER checklist_added AFTER INSERT ON checklist_items
    BEGIN
        UPDATE tasks SET modified_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = NEW.task_id;
    END;
    ",
    // "Task Activities": what happened to a task and when, recorded by triggers.
    "
    CREATE TABLE task_activity (
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        at      INTEGER NOT NULL,
        kind    TEXT NOT NULL
    );
    CREATE INDEX task_activity_task ON task_activity (task_id, at);
    CREATE TRIGGER activity_created AFTER INSERT ON tasks
    BEGIN
        INSERT INTO task_activity (task_id, at, kind) VALUES (NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER), 'created');
    END;
    CREATE TRIGGER activity_renamed AFTER UPDATE OF title ON tasks
    WHEN OLD.title IS NOT NEW.title AND OLD.title <> ''
    BEGIN
        INSERT INTO task_activity (task_id, at, kind) VALUES (NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER), 'renamed');
    END;
    CREATE TRIGGER activity_completed AFTER UPDATE OF completed_at ON tasks
    WHEN OLD.completed_at IS NOT NEW.completed_at
    BEGIN
        INSERT INTO task_activity (task_id, at, kind) VALUES (NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER), CASE WHEN NEW.completed_at IS NULL THEN 'reopened' ELSE 'completed' END);
    END;
    CREATE TRIGGER activity_date AFTER UPDATE OF due_date, due_time ON tasks
    WHEN OLD.due_date IS NOT NEW.due_date OR OLD.due_time IS NOT NEW.due_time
    BEGIN
        INSERT INTO task_activity (task_id, at, kind) VALUES (NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER), CASE WHEN NEW.due_date IS NULL THEN 'date_cleared' ELSE 'date' END);
    END;
    CREATE TRIGGER activity_priority AFTER UPDATE OF priority ON tasks
    WHEN OLD.priority IS NOT NEW.priority
    BEGIN
        INSERT INTO task_activity (task_id, at, kind) VALUES (NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER), 'priority');
    END;
    CREATE TRIGGER activity_moved AFTER UPDATE OF list_id ON tasks
    WHEN OLD.list_id IS NOT NEW.list_id
    BEGIN
        INSERT INTO task_activity (task_id, at, kind) VALUES (NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER), 'moved');
    END;
    CREATE TRIGGER activity_deleted AFTER UPDATE OF deleted_at ON tasks
    WHEN OLD.deleted_at IS NOT NEW.deleted_at
    BEGIN
        INSERT INTO task_activity (task_id, at, kind) VALUES (NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER), CASE WHEN NEW.deleted_at IS NULL THEN 'restored' ELSE 'deleted' END);
    END;
    CREATE TRIGGER activity_wont_do AFTER UPDATE OF wont_do_at ON tasks
    WHEN OLD.wont_do_at IS NOT NEW.wont_do_at
    BEGIN
        INSERT INTO task_activity (task_id, at, kind) VALUES (NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER), CASE WHEN NEW.wont_do_at IS NULL THEN 'reopened' ELSE 'wont_do' END);
    END;
    ",
    "
    ALTER TABLE tasks ADD COLUMN pinned_at INTEGER;
    ",
    "
    CREATE TABLE list_sections (
        id         TEXT PRIMARY KEY,
        list_id    TEXT NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    ALTER TABLE tasks ADD COLUMN section_id TEXT REFERENCES list_sections(id) ON DELETE SET NULL;
    ",
    "
    CREATE TABLE templates (
        id         TEXT PRIMARY KEY,
        title      TEXT NOT NULL,
        notes      TEXT NOT NULL,
        priority   INTEGER NOT NULL,
        tags       TEXT NOT NULL,
        checklist  TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    ",
    "
    CREATE TABLE filters (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        lists      TEXT NOT NULL,
        tags       TEXT NOT NULL,
        date       TEXT NOT NULL,
        priorities TEXT NOT NULL,
        keywords   TEXT NOT NULL,
        kind       TEXT,
        created_at INTEGER NOT NULL
    );
    ",
    "
    ALTER TABLE filters ADD COLUMN match_any INTEGER NOT NULL DEFAULT 0;
    ",
    "
    ALTER TABLE habits ADD COLUMN icon_checkins INTEGER NOT NULL DEFAULT 0;
    ",
    "
    ALTER TABLE tasks ADD COLUMN estimate TEXT;
    ",
    "
    CREATE TABLE summary_templates (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        options    TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    ",
    "
    ALTER TABLE focus_sessions ADD COLUMN stopwatch INTEGER NOT NULL DEFAULT 0;
    ",
    "
    ALTER TABLE focus_sessions ADD COLUMN note TEXT NOT NULL DEFAULT '';
    ",
    "
    CREATE TABLE habit_snoozes (
        habit_id TEXT NOT NULL REFERENCES habits(id) ON DELETE CASCADE,
        at       TEXT NOT NULL
    );
    ",
    "
    ALTER TABLE countdowns ADD COLUMN icon TEXT;
    ALTER TABLE countdowns ADD COLUMN color TEXT;
    ",
    "
    CREATE TABLE task_attachments (
        id         TEXT PRIMARY KEY,
        task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        name       TEXT NOT NULL,
        size       INTEGER NOT NULL,
        created_at INTEGER NOT NULL
    );
    ",
    // Sync: every change to synced data is logged with its time, by triggers, so that every write
    // path counts; changes applied from another device are not logged again (see `sync.rs`).
    "
    CREATE TABLE sync_log (
        seq     INTEGER PRIMARY KEY AUTOINCREMENT,
        entity  TEXT NOT NULL,
        key     TEXT NOT NULL,
        at      INTEGER NOT NULL
    );
    CREATE INDEX sync_log_key ON sync_log(entity, key);
    CREATE TABLE sync_applying (x INTEGER);
    -- What exists already counts as changed long ago: any later edit elsewhere wins.
    INSERT INTO sync_log (entity, key, at) SELECT 'lists', id, 0 FROM lists;
    INSERT INTO sync_log (entity, key, at) SELECT 'list_sections', id, 0 FROM list_sections;
    INSERT INTO sync_log (entity, key, at) SELECT 'tags', name, 0 FROM tags;
    INSERT INTO sync_log (entity, key, at) SELECT 'tasks', id, 0 FROM tasks;
    INSERT INTO sync_log (entity, key, at) SELECT 'task_tags', task_id || char(31) || tag, 0 FROM task_tags;
    INSERT INTO sync_log (entity, key, at) SELECT 'checklist_items', id, 0 FROM checklist_items;
    INSERT INTO sync_log (entity, key, at) SELECT 'countdowns', id, 0 FROM countdowns;
    INSERT INTO sync_log (entity, key, at) SELECT 'habit_sections', id, 0 FROM habit_sections;
    INSERT INTO sync_log (entity, key, at) SELECT 'habits', id, 0 FROM habits;
    INSERT INTO sync_log (entity, key, at) SELECT 'habit_checkins', habit_id || char(31) || date, 0 FROM habit_checkins;
    INSERT INTO sync_log (entity, key, at) SELECT 'focus_sessions', id, 0 FROM focus_sessions;
    INSERT INTO sync_log (entity, key, at) SELECT 'templates', id, 0 FROM templates;
    INSERT INTO sync_log (entity, key, at) SELECT 'filters', id, 0 FROM filters;
    INSERT INTO sync_log (entity, key, at) SELECT 'summary_templates', id, 0 FROM summary_templates;
    CREATE TRIGGER sync_lists_insert AFTER INSERT ON lists
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('lists', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_lists_update AFTER UPDATE ON lists
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('lists', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_lists_delete AFTER DELETE ON lists
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('lists', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_list_sections_insert AFTER INSERT ON list_sections
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('list_sections', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_list_sections_update AFTER UPDATE ON list_sections
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('list_sections', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_list_sections_delete AFTER DELETE ON list_sections
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('list_sections', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_tags_insert AFTER INSERT ON tags
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('tags', NEW.name, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_tags_update AFTER UPDATE ON tags
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('tags', NEW.name, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_tags_delete AFTER DELETE ON tags
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('tags', OLD.name, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_tasks_insert AFTER INSERT ON tasks
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('tasks', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_tasks_update AFTER UPDATE ON tasks
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('tasks', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_tasks_delete AFTER DELETE ON tasks
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('tasks', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_task_tags_insert AFTER INSERT ON task_tags
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('task_tags', NEW.task_id || char(31) || NEW.tag, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_task_tags_update AFTER UPDATE ON task_tags
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('task_tags', NEW.task_id || char(31) || NEW.tag, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_task_tags_delete AFTER DELETE ON task_tags
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('task_tags', OLD.task_id || char(31) || OLD.tag, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_checklist_items_insert AFTER INSERT ON checklist_items
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('checklist_items', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_checklist_items_update AFTER UPDATE ON checklist_items
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('checklist_items', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_checklist_items_delete AFTER DELETE ON checklist_items
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('checklist_items', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_countdowns_insert AFTER INSERT ON countdowns
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('countdowns', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_countdowns_update AFTER UPDATE ON countdowns
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('countdowns', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_countdowns_delete AFTER DELETE ON countdowns
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('countdowns', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_habit_sections_insert AFTER INSERT ON habit_sections
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('habit_sections', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_habit_sections_update AFTER UPDATE ON habit_sections
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('habit_sections', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_habit_sections_delete AFTER DELETE ON habit_sections
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('habit_sections', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_habits_insert AFTER INSERT ON habits
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('habits', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_habits_update AFTER UPDATE ON habits
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('habits', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_habits_delete AFTER DELETE ON habits
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('habits', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_habit_checkins_insert AFTER INSERT ON habit_checkins
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('habit_checkins', NEW.habit_id || char(31) || NEW.date, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_habit_checkins_update AFTER UPDATE ON habit_checkins
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('habit_checkins', NEW.habit_id || char(31) || NEW.date, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_habit_checkins_delete AFTER DELETE ON habit_checkins
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('habit_checkins', OLD.habit_id || char(31) || OLD.date, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_focus_sessions_insert AFTER INSERT ON focus_sessions
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('focus_sessions', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_focus_sessions_update AFTER UPDATE ON focus_sessions
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('focus_sessions', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_focus_sessions_delete AFTER DELETE ON focus_sessions
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('focus_sessions', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_templates_insert AFTER INSERT ON templates
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('templates', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_templates_update AFTER UPDATE ON templates
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('templates', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_templates_delete AFTER DELETE ON templates
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('templates', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_filters_insert AFTER INSERT ON filters
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('filters', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_filters_update AFTER UPDATE ON filters
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('filters', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_filters_delete AFTER DELETE ON filters
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('filters', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_summary_templates_insert AFTER INSERT ON summary_templates
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('summary_templates', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_summary_templates_update AFTER UPDATE ON summary_templates
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('summary_templates', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_summary_templates_delete AFTER DELETE ON summary_templates
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('summary_templates', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    ",
    // Attachments sync too: their records like other rows, their files through the server's
    // file store; `sync_uploaded` lists the files the server has.
    "
    INSERT INTO sync_log (entity, key, at) SELECT 'task_attachments', id, 0 FROM task_attachments;
    CREATE TRIGGER sync_task_attachments_insert AFTER INSERT ON task_attachments
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('task_attachments', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_task_attachments_update AFTER UPDATE ON task_attachments
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('task_attachments', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_task_attachments_delete AFTER DELETE ON task_attachments
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('task_attachments', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TABLE sync_uploaded (name TEXT PRIMARY KEY);
    ",
    "
    ALTER TABLE countdowns ADD COLUMN repeat TEXT;
    ",
    "
    ALTER TABLE countdowns ADD COLUMN pinned_at INTEGER;
    ",
    "
    CREATE TABLE focus_timers (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        icon        TEXT,
        minutes     INTEGER NOT NULL,
        stopwatch   INTEGER NOT NULL DEFAULT 0,
        archived_at INTEGER,
        created_at  INTEGER NOT NULL
    );
    ",
    "
    ALTER TABLE focus_timers ADD COLUMN color TEXT;
    ",
    "
    ALTER TABLE filters ADD COLUMN chain TEXT NOT NULL DEFAULT '';
    ",
    // Focus timers sync like the other rows.
    "
    INSERT INTO sync_log (entity, key, at) SELECT 'focus_timers', id, 0 FROM focus_timers;
    CREATE TRIGGER sync_focus_timers_insert AFTER INSERT ON focus_timers
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('focus_timers', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_focus_timers_update AFTER UPDATE ON focus_timers
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('focus_timers', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_focus_timers_delete AFTER DELETE ON focus_timers
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('focus_timers', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    ",
    // The Eisenhower Matrix's quadrant rules move out of the settings into a synced table.
    "
    CREATE TABLE matrix_rules (
        quadrant TEXT PRIMARY KEY,
        rule     TEXT NOT NULL
    );
    INSERT INTO matrix_rules (quadrant, rule)
        SELECT substr(key, 13), value FROM settings WHERE key GLOB 'matrix_rule_[0-3]';
    DELETE FROM settings WHERE key GLOB 'matrix_rule_[0-3]';
    INSERT INTO sync_log (entity, key, at) SELECT 'matrix_rules', quadrant, 0 FROM matrix_rules;
    CREATE TRIGGER sync_matrix_rules_insert AFTER INSERT ON matrix_rules
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('matrix_rules', NEW.quadrant, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_matrix_rules_update AFTER UPDATE ON matrix_rules
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('matrix_rules', NEW.quadrant, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_matrix_rules_delete AFTER DELETE ON matrix_rules
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('matrix_rules', OLD.quadrant, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    ",
    "
    ALTER TABLE habits ADD COLUMN constant_reminder INTEGER NOT NULL DEFAULT 0;
    ",
    "
    ALTER TABLE countdowns ADD COLUMN show_age INTEGER NOT NULL DEFAULT 0;
    ",
    "
    ALTER TABLE countdowns ADD COLUMN reminders TEXT NOT NULL DEFAULT '';
    ALTER TABLE countdowns ADD COLUMN constant_reminder INTEGER NOT NULL DEFAULT 0;
    ",
    "
    ALTER TABLE countdowns ADD COLUMN show_in_list TEXT NOT NULL DEFAULT 'day';
    ALTER TABLE countdowns ADD COLUMN plus_one_day INTEGER NOT NULL DEFAULT 0;
    ",
    "
    ALTER TABLE countdowns ADD COLUMN count_up INTEGER NOT NULL DEFAULT 0;
    ",
    "
    ALTER TABLE countdowns ADD COLUMN archived_at INTEGER;
    ",
    "
    ALTER TABLE countdowns ADD COLUMN background TEXT;
    ",
    // The original's list settings: an emoji, a folder, the list type, "Show in Smart List",
    // archiving and the lists' own order; and the folders themselves.
    "
    CREATE TABLE list_folders (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        sort_order INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL
    );
    ALTER TABLE lists ADD COLUMN icon TEXT;
    ALTER TABLE lists ADD COLUMN folder_id TEXT;
    ALTER TABLE lists ADD COLUMN kind TEXT NOT NULL DEFAULT 'task';
    ALTER TABLE lists ADD COLUMN in_smart_lists INTEGER NOT NULL DEFAULT 1;
    ALTER TABLE lists ADD COLUMN archived_at INTEGER;
    ALTER TABLE lists ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
    UPDATE lists SET sort_order = (SELECT COUNT(*) FROM lists l WHERE l.created_at < lists.created_at OR (l.created_at = lists.created_at AND l.rowid < lists.rowid));
    CREATE TRIGGER sync_list_folders_insert AFTER INSERT ON list_folders
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('list_folders', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_list_folders_update AFTER UPDATE ON list_folders
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('list_folders', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_list_folders_delete AFTER DELETE ON list_folders
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('list_folders', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    ",
    // The original's sub-tags and pinned tags.
    "
    ALTER TABLE tags ADD COLUMN parent TEXT;
    ALTER TABLE tags ADD COLUMN pinned_at INTEGER;
    ",
    // The original's comments on a task.
    "
    CREATE TABLE task_comments (
        id         TEXT PRIMARY KEY,
        task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        text       TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX task_comments_task ON task_comments(task_id);
    CREATE TRIGGER sync_task_comments_insert AFTER INSERT ON task_comments
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('task_comments', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_task_comments_update AFTER UPDATE ON task_comments
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('task_comments', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_task_comments_delete AFTER DELETE ON task_comments
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('task_comments', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    ",
    // The kanban's "Add Section to Left / Right": sections keep their own order.
    "
    ALTER TABLE list_sections ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
    UPDATE list_sections SET sort_order = (SELECT COUNT(*) FROM list_sections s WHERE s.list_id = list_sections.list_id AND (s.created_at < list_sections.created_at OR (s.created_at = list_sections.created_at AND s.rowid < list_sections.rowid)));
    ",
    // The original's Inbox has sections too: a section's list may be none, the Inbox. The table
    // is rebuilt with the foreign keys off, so that the tasks keep their sections.
    "
    PRAGMA foreign_keys = OFF;
    CREATE TABLE list_sections_new (
        id         TEXT PRIMARY KEY,
        list_id    TEXT REFERENCES lists(id) ON DELETE CASCADE,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        sort_order INTEGER NOT NULL DEFAULT 0
    );
    INSERT INTO list_sections_new (id, list_id, name, created_at, sort_order)
        SELECT id, list_id, name, created_at, sort_order FROM list_sections;
    DROP TABLE list_sections;
    ALTER TABLE list_sections_new RENAME TO list_sections;
    CREATE TRIGGER sync_list_sections_insert AFTER INSERT ON list_sections
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('list_sections', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_list_sections_update AFTER UPDATE ON list_sections
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('list_sections', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_list_sections_delete AFTER DELETE ON list_sections
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('list_sections', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    PRAGMA foreign_keys = ON;
    ",
    // Tag counts and renames look tasks up by tag.
    "CREATE INDEX task_tags_tag ON task_tags(tag, task_id);",
    // Subscribed calendars (the original's URL calendars) with the last feed read from each, kept
    // on this device.
    "
    CREATE TABLE calendar_subscriptions (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        url        TEXT NOT NULL,
        color      TEXT,
        hidden     INTEGER NOT NULL DEFAULT 0,
        feed       TEXT NOT NULL DEFAULT '',
        fetched_at INTEGER,
        error      TEXT,
        created_at INTEGER NOT NULL
    );
    ",
    "
    CREATE TABLE repeat_checklist_copies (
        copy_id TEXT PRIMARY KEY REFERENCES checklist_items(id) ON DELETE CASCADE,
        source_id TEXT NOT NULL REFERENCES checklist_items(id) ON DELETE CASCADE
    );
    CREATE INDEX repeat_checklist_copies_source ON repeat_checklist_copies(source_id);
    ",
    // Versioned, independent filter clauses. NULL rows retain their legacy representation.
    "ALTER TABLE filters ADD COLUMN rules TEXT;",
    // Indexes for section lookups, manual task ordering, and list-folder membership.
    "
    CREATE INDEX IF NOT EXISTS tasks_section ON tasks(section_id);
    CREATE INDEX IF NOT EXISTS tasks_sort_order ON tasks(sort_order);
    CREATE INDEX IF NOT EXISTS list_sections_list_order ON list_sections(list_id, sort_order);
    CREATE INDEX IF NOT EXISTS lists_folder ON lists(folder_id);
    ",
    // Habits parity: check-in modes/steps, auto-pop log, skipped dates, manual ordering,
    // check-in status/goal snapshot/mood, and focus-session habit links.
    "
    ALTER TABLE habits ADD COLUMN goal_step REAL NOT NULL DEFAULT 1.0;
    ALTER TABLE habits ADD COLUMN record_enable INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE habits ADD COLUMN ex_dates TEXT;
    ALTER TABLE habits ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
    UPDATE habits SET sort_order = (
        SELECT COUNT(*) FROM habits h
        WHERE h.created_at < habits.created_at OR (h.created_at = habits.created_at AND h.rowid < habits.rowid)
    );
    ALTER TABLE habit_sections ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
    UPDATE habit_sections SET sort_order = (
        SELECT COUNT(*) FROM habit_sections s
        WHERE s.created_at < habit_sections.created_at OR (s.created_at = habit_sections.created_at AND s.rowid < habit_sections.rowid)
    );
    ALTER TABLE habit_checkins ADD COLUMN goal REAL;
    ALTER TABLE habit_checkins ADD COLUMN status INTEGER NOT NULL DEFAULT 2;
    ALTER TABLE habit_checkins ADD COLUMN mood INTEGER NOT NULL DEFAULT 0;
    UPDATE habit_checkins SET
        goal = COALESCE((SELECT goal_amount FROM habits WHERE id = habit_checkins.habit_id), 1.0),
        status = CASE
            WHEN value >= COALESCE((SELECT goal_amount FROM habits WHERE id = habit_checkins.habit_id), 1.0) THEN 2
            ELSE 0
        END;
    ALTER TABLE focus_sessions ADD COLUMN habit_id TEXT;
    ",
    // Task statuses, locations, timezones, snooze timestamps, and per-task constant reminders.
    "
    ALTER TABLE tasks ADD COLUMN in_progress_at INTEGER;
    ALTER TABLE tasks ADD COLUMN location TEXT;
    ALTER TABLE tasks ADD COLUMN timezone TEXT;
    ALTER TABLE tasks ADD COLUMN is_floating INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE tasks ADD COLUMN snoozed_until TEXT;
    ALTER TABLE tasks ADD COLUMN constant_reminder INTEGER NOT NULL DEFAULT 0;
    DROP TRIGGER IF EXISTS tasks_modified;
    CREATE TRIGGER tasks_modified AFTER UPDATE OF
        title, notes, priority, kind, parent_id, list_id, due_date, due_time, end_date, end_time,
        repeat, reminders, completed_at, deleted_at, wont_do_at,
        in_progress_at, location, timezone, is_floating, snoozed_until, constant_reminder ON tasks
    BEGIN
        UPDATE tasks SET modified_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = NEW.id;
    END;
    CREATE TRIGGER activity_in_progress AFTER UPDATE OF in_progress_at ON tasks
    WHEN OLD.in_progress_at IS NOT NEW.in_progress_at AND NEW.in_progress_at IS NOT NULL
    BEGIN
        INSERT INTO task_activity (task_id, at, kind) VALUES (NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER), 'in_progress');
    END;
    ",
    "
    ALTER TABLE calendar_subscriptions ADD COLUMN kind TEXT NOT NULL DEFAULT 'ics';
    ALTER TABLE calendar_subscriptions ADD COLUMN provider TEXT NOT NULL DEFAULT 'url';
    ALTER TABLE calendar_subscriptions ADD COLUMN username TEXT NOT NULL DEFAULT '';
    ALTER TABLE calendar_subscriptions ADD COLUMN password TEXT NOT NULL DEFAULT '';
    ALTER TABLE calendar_subscriptions ADD COLUMN writable INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE calendar_subscriptions ADD COLUMN sync_token TEXT;

    CREATE TABLE calendar_events (
        subscription_id TEXT NOT NULL REFERENCES calendar_subscriptions(id) ON DELETE CASCADE,
        uid TEXT NOT NULL,
        href TEXT NOT NULL DEFAULT '',
        etag TEXT,
        ics TEXT NOT NULL,
        dirty INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (subscription_id, uid)
    );

    CREATE TABLE matrix_profiles (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        rules_json TEXT NOT NULL,
        sort_order INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL DEFAULT 0
    );

    CREATE TRIGGER sync_matrix_profiles_insert AFTER INSERT ON matrix_profiles
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('matrix_profiles', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_matrix_profiles_update AFTER UPDATE ON matrix_profiles
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('matrix_profiles', NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    CREATE TRIGGER sync_matrix_profiles_delete AFTER DELETE ON matrix_profiles
    WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    BEGIN
        INSERT INTO sync_log (entity, key, at) VALUES ('matrix_profiles', OLD.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
    END;
    ",
    // Per-(entity, key) high-water mark updated on both local writes (via sync_log) and applied
    // remote changes so stale offline edits can never overwrite newer applied remote state.
    "
    CREATE TABLE sync_watermarks (
        entity TEXT NOT NULL,
        key    TEXT NOT NULL,
        at     INTEGER NOT NULL,
        PRIMARY KEY (entity, key)
    );
    INSERT INTO sync_watermarks (entity, key, at)
        SELECT entity, key, MAX(at) FROM sync_log GROUP BY entity, key;
    CREATE TRIGGER sync_watermarks_from_log AFTER INSERT ON sync_log
    BEGIN
        INSERT INTO sync_watermarks (entity, key, at) VALUES (NEW.entity, NEW.key, NEW.at)
        ON CONFLICT(entity, key) DO UPDATE SET at = MAX(sync_watermarks.at, excluded.at);
    END;
    ",
    // Multi-frontend scheduling columns on tasks and project fields on lists (Things 3 support).
    "
    ALTER TABLE tasks ADD COLUMN start_date TEXT;
    ALTER TABLE tasks ADD COLUMN schedule_bucket TEXT NOT NULL DEFAULT '';
    ALTER TABLE lists ADD COLUMN notes TEXT NOT NULL DEFAULT '';
    ALTER TABLE lists ADD COLUMN deadline TEXT;
    ALTER TABLE lists ADD COLUMN completed_at INTEGER;
    CREATE TRIGGER tasks_modified_schedule AFTER UPDATE OF start_date, schedule_bucket ON tasks
    BEGIN
        UPDATE tasks SET modified_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = NEW.id;
    END;
    ",
    // Local plugin state, KV, entity metadata, user scripts, write context, and event outbox.
    "
    CREATE TABLE plugin_state (
        plugin_id           TEXT PRIMARY KEY,
        enabled             INTEGER NOT NULL DEFAULT 0,
        granted_permissions TEXT NOT NULL DEFAULT '[]',
        manifest_hash       TEXT NOT NULL DEFAULT '',
        settings_json       TEXT NOT NULL DEFAULT '{}',
        consecutive_errors  INTEGER NOT NULL DEFAULT 0,
        updated_at          INTEGER NOT NULL
    );
    CREATE TABLE plugin_kv (
        plugin_id   TEXT NOT NULL,
        key         TEXT NOT NULL,
        value       TEXT NOT NULL,
        updated_at  INTEGER NOT NULL,
        PRIMARY KEY (plugin_id, key)
    );
    CREATE TABLE plugin_entity_meta (
        plugin_id   TEXT NOT NULL,
        entity_type TEXT NOT NULL,
        entity_id   TEXT NOT NULL,
        key         TEXT NOT NULL,
        value       TEXT NOT NULL,
        updated_at  INTEGER NOT NULL,
        PRIMARY KEY (plugin_id, entity_type, entity_id, key)
    );
    CREATE INDEX plugin_entity_meta_lookup ON plugin_entity_meta(entity_type, entity_id);
    CREATE TABLE user_scripts (
        id           TEXT PRIMARY KEY,
        name         TEXT NOT NULL,
        enabled      INTEGER NOT NULL DEFAULT 0,
        trigger_kind TEXT NOT NULL,
        trigger_arg  TEXT NOT NULL DEFAULT '',
        permissions  TEXT NOT NULL DEFAULT '[\"tasks:read\"]',
        code         TEXT NOT NULL,
        updated_at   INTEGER NOT NULL
    );
    CREATE TABLE plugin_write_ctx (
        plugin_id TEXT NOT NULL,
        depth     INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE plugin_outbox (
        seq           INTEGER PRIMARY KEY AUTOINCREMENT,
        event_kind    TEXT NOT NULL,
        entity_id     TEXT NOT NULL,
        origin_plugin TEXT,
        depth         INTEGER NOT NULL DEFAULT 0,
        at            INTEGER NOT NULL
    );
    CREATE TRIGGER outbox_tasks_insert AFTER INSERT ON tasks
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.created',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_completed AFTER UPDATE OF completed_at ON tasks
    WHEN OLD.completed_at IS NULL AND NEW.completed_at IS NOT NULL
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.completed',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_reopened AFTER UPDATE OF completed_at ON tasks
    WHEN OLD.completed_at IS NOT NULL AND NEW.completed_at IS NULL
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.reopened',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_deleted AFTER UPDATE OF deleted_at ON tasks
    WHEN OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.deleted',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_purged AFTER DELETE ON tasks
    WHEN OLD.deleted_at IS NULL
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.deleted',
            OLD.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_updated AFTER UPDATE OF
        title, notes, priority, due_date, due_time, end_date, end_time,
        list_id, section_id, repeat, wont_do_at, start_date, schedule_bucket ON tasks
    WHEN OLD.title IS NOT NEW.title
        OR OLD.notes IS NOT NEW.notes
        OR OLD.priority IS NOT NEW.priority
        OR OLD.due_date IS NOT NEW.due_date
        OR OLD.due_time IS NOT NEW.due_time
        OR OLD.end_date IS NOT NEW.end_date
        OR OLD.end_time IS NOT NEW.end_time
        OR OLD.list_id IS NOT NEW.list_id
        OR OLD.section_id IS NOT NEW.section_id
        OR OLD.repeat IS NOT NEW.repeat
        OR OLD.wont_do_at IS NOT NEW.wont_do_at
        OR OLD.start_date IS NOT NEW.start_date
        OR OLD.schedule_bucket IS NOT NEW.schedule_bucket
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.updated',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_habit_checkins_insert AFTER INSERT ON habit_checkins
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'habit.checkin',
            NEW.habit_id || char(31) || NEW.date,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_habit_checkins_update AFTER UPDATE ON habit_checkins
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'habit.checkin',
            NEW.habit_id || char(31) || NEW.date,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_focus_sessions_insert AFTER INSERT ON focus_sessions
    WHEN NEW.ended_at IS NOT NULL
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'focus.completed',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_focus_sessions_update AFTER UPDATE OF ended_at ON focus_sessions
    WHEN NEW.ended_at IS NOT NULL AND OLD.ended_at IS NOT NEW.ended_at
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'focus.completed',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    ",
    // Outbox reliability (split completed/open task inserts, restore_task event, Safe Mode / pause
    // guard, automatic growth cap) and plugin_entity_meta orphan cleanup triggers.
    "
    DROP TRIGGER IF EXISTS outbox_tasks_insert;
    DROP TRIGGER IF EXISTS outbox_tasks_insert_completed;
    DROP TRIGGER IF EXISTS outbox_tasks_completed;
    DROP TRIGGER IF EXISTS outbox_tasks_reopened;
    DROP TRIGGER IF EXISTS outbox_tasks_deleted;
    DROP TRIGGER IF EXISTS outbox_tasks_restored;
    DROP TRIGGER IF EXISTS outbox_tasks_purged;
    DROP TRIGGER IF EXISTS outbox_tasks_updated;
    DROP TRIGGER IF EXISTS outbox_habit_checkins_insert;
    DROP TRIGGER IF EXISTS outbox_habit_checkins_update;
    DROP TRIGGER IF EXISTS outbox_focus_sessions_insert;
    DROP TRIGGER IF EXISTS outbox_focus_sessions_update;
    DROP TRIGGER IF EXISTS outbox_cap_growth;

    CREATE TRIGGER outbox_tasks_insert AFTER INSERT ON tasks
    WHEN NEW.deleted_at IS NULL AND NEW.completed_at IS NULL
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.created',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_insert_completed AFTER INSERT ON tasks
    WHEN NEW.deleted_at IS NULL AND NEW.completed_at IS NOT NULL
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.completed',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_completed AFTER UPDATE OF completed_at ON tasks
    WHEN OLD.completed_at IS NULL AND NEW.completed_at IS NOT NULL
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.completed',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_reopened AFTER UPDATE OF completed_at ON tasks
    WHEN OLD.completed_at IS NOT NULL AND NEW.completed_at IS NULL
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.reopened',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_deleted AFTER UPDATE OF deleted_at ON tasks
    WHEN OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.deleted',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_restored AFTER UPDATE OF deleted_at ON tasks
    WHEN OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS NULL
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.updated',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_purged AFTER DELETE ON tasks
    WHEN OLD.deleted_at IS NULL
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.deleted',
            OLD.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_tasks_updated AFTER UPDATE OF
        title, notes, priority, due_date, due_time, end_date, end_time,
        list_id, section_id, repeat, wont_do_at, start_date, schedule_bucket ON tasks
    WHEN (
            OLD.title IS NOT NEW.title
            OR OLD.notes IS NOT NEW.notes
            OR OLD.priority IS NOT NEW.priority
            OR OLD.due_date IS NOT NEW.due_date
            OR OLD.due_time IS NOT NEW.due_time
            OR OLD.end_date IS NOT NEW.end_date
            OR OLD.end_time IS NOT NEW.end_time
            OR OLD.list_id IS NOT NEW.list_id
            OR OLD.section_id IS NOT NEW.section_id
            OR OLD.repeat IS NOT NEW.repeat
            OR OLD.wont_do_at IS NOT NEW.wont_do_at
            OR OLD.start_date IS NOT NEW.start_date
            OR OLD.schedule_bucket IS NOT NEW.schedule_bucket
        )
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.updated',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_habit_checkins_insert AFTER INSERT ON habit_checkins
    WHEN NOT EXISTS (
        SELECT 1 FROM settings
        WHERE (key = 'plugins_safe_mode' AND value = '1')
           OR (key = 'plugins_outbox_paused' AND value = '1')
    )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'habit.checkin',
            NEW.habit_id || char(31) || NEW.date,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_habit_checkins_update AFTER UPDATE ON habit_checkins
    WHEN NOT EXISTS (
        SELECT 1 FROM settings
        WHERE (key = 'plugins_safe_mode' AND value = '1')
           OR (key = 'plugins_outbox_paused' AND value = '1')
    )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'habit.checkin',
            NEW.habit_id || char(31) || NEW.date,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_focus_sessions_insert AFTER INSERT ON focus_sessions
    WHEN NEW.ended_at IS NOT NULL
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'focus.completed',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_focus_sessions_update AFTER UPDATE OF ended_at ON focus_sessions
    WHEN NEW.ended_at IS NOT NULL AND OLD.ended_at IS NOT NEW.ended_at
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'focus.completed',
            NEW.id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    CREATE TRIGGER outbox_cap_growth AFTER INSERT ON plugin_outbox
    WHEN NEW.seq % 128 = 0
    BEGIN
        DELETE FROM plugin_outbox
        WHERE seq <= (
            SELECT seq FROM plugin_outbox ORDER BY seq DESC LIMIT 1 OFFSET 2000
        );
    END;

    CREATE TRIGGER plugin_meta_cleanup_tasks AFTER DELETE ON tasks
    BEGIN
        DELETE FROM plugin_entity_meta WHERE entity_type = 'task' AND entity_id = OLD.id;
    END;
    CREATE TRIGGER plugin_meta_cleanup_list_tasks BEFORE DELETE ON lists
    BEGIN
        DELETE FROM plugin_entity_meta
        WHERE entity_type = 'task'
          AND entity_id IN (SELECT id FROM tasks WHERE list_id = OLD.id);
    END;
    CREATE TRIGGER plugin_meta_cleanup_lists AFTER DELETE ON lists
    BEGIN
        DELETE FROM plugin_entity_meta WHERE entity_type = 'list' AND entity_id = OLD.id;
    END;
    CREATE TRIGGER plugin_meta_cleanup_habits AFTER DELETE ON habits
    BEGIN
        DELETE FROM plugin_entity_meta WHERE entity_type = 'habit' AND entity_id = OLD.id;
    END;
    CREATE TRIGGER plugin_meta_cleanup_countdowns AFTER DELETE ON countdowns
    BEGIN
        DELETE FROM plugin_entity_meta WHERE entity_type = 'countdown' AND entity_id = OLD.id;
    END;
    ",
];

/// Current SQLite schema version (`PRAGMA user_version`).
pub const SCHEMA_VERSION: usize = MIGRATIONS.len();

fn is_sqlite_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _)
            if matches!(e.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

fn retry_sqlite_busy<T>(mut op: impl FnMut() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    for attempt in 0..50_u32 {
        match op() {
            Ok(val) => return Ok(val),
            Err(err) if is_sqlite_busy(&err) && attempt < 49 => {
                std::thread::sleep(std::time::Duration::from_millis(
                    10 + u64::from(attempt.min(20)) * 5,
                ));
            }
            Err(err) => return Err(err),
        }
    }
    op()
}

/// Applies migrations up to `target_version` on `conn`. Exposed for migration tests.
#[doc(hidden)]
pub fn apply_migrations_up_to(conn: &Connection, target_version: usize) -> Result<()> {
    anyhow::ensure!(
        target_version <= MIGRATIONS.len(),
        "target schema version {target_version} exceeds {}",
        MIGRATIONS.len()
    );
    let mut version: usize =
        retry_sqlite_busy(|| conn.pragma_query_value(None, "user_version", |row| row.get(0)))?;
    anyhow::ensure!(version <= MIGRATIONS.len(), "the data comes from a newer version of Fern");
    while version < target_version {
        let i = version;
        let sql = MIGRATIONS[i];
        // The historical section rebuild toggles foreign keys, which SQLite can only do
        // outside a transaction. Disable FK before opening the IMMEDIATE transaction, then
        // re-enable afterwards. All other contiguous migrations run together inside a single
        // IMMEDIATE transaction so concurrent Store::open calls never race or ping-pong locks.
        if sql.contains("PRAGMA foreign_keys = OFF") {
            let prev_fk: bool = conn.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
            conn.pragma_update(None, "foreign_keys", false)?;
            struct RestoreFk<'a> {
                conn: &'a Connection,
                enable: bool,
            }
            impl Drop for RestoreFk<'_> {
                fn drop(&mut self) {
                    if self.enable {
                        let _ = self.conn.pragma_update(None, "foreign_keys", true);
                    }
                }
            }
            let mut restore_guard = RestoreFk { conn, enable: prev_fk };
            let (next_version, applied) = {
                let tx = retry_sqlite_busy(|| {
                    Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                })?;
                let current: usize = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
                anyhow::ensure!(
                    current <= MIGRATIONS.len(),
                    "the data comes from a newer version of Fern"
                );
                let outcome = if current <= i {
                    tx.execute_batch(sql)?;
                    tx.pragma_update(None, "user_version", i + 1)?;
                    (i + 1, true)
                } else {
                    (current, false)
                };
                tx.commit()?;
                outcome
            };
            restore_guard.enable = false;
            if prev_fk || (applied && sql.contains("PRAGMA foreign_keys = ON")) {
                conn.pragma_update(None, "foreign_keys", true)?;
            }
            version = next_version;
        } else {
            let tx = retry_sqlite_busy(|| {
                Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
            })?;
            let mut current: usize = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
            anyhow::ensure!(
                current <= MIGRATIONS.len(),
                "the data comes from a newer version of Fern"
            );
            while current < target_version && !MIGRATIONS[current].contains("PRAGMA foreign_keys = OFF") {
                tx.execute_batch(MIGRATIONS[current])?;
                current += 1;
                tx.pragma_update(None, "user_version", current)?;
            }
            tx.commit()?;
            version = current;
        }
    }
    Ok(())
}

/// Summary row counts for the primary user-facing tables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableCounts {
    pub tasks: usize,
    pub lists: usize,
    pub habits: usize,
    pub attachments: usize,
    pub countdowns: usize,
}

/// A daily backup file (`fern-*.db`) discovered in a backups directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupEntry {
    pub path: PathBuf,
    pub filename: String,
    pub date: Option<NaiveDate>,
    pub size_bytes: u64,
}

/// Lists `fern-*.db` backup files in `dir`, sorted newest-first by date and filename.
pub fn list_backups(dir: &Path) -> Vec<BackupEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut backups = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(filename) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !filename.starts_with("fern-") || !filename.ends_with(".db") {
            continue;
        }
        let stem = &filename["fern-".len()..filename.len() - ".db".len()];
        let date = stem
            .get(..10)
            .and_then(|s| NaiveDate::parse_from_str(s, DATE_FORMAT).ok());
        backups.push(BackupEntry {
            path: entry.path(),
            filename,
            date,
            size_bytes: meta.len(),
        });
    }
    backups.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.filename.cmp(&a.filename)));
    backups
}

#[cfg(test)]
pub(crate) fn init_legacy_db_before_filter_rules(path: &Path) -> Result<Connection> {
    let version = MIGRATIONS
        .iter()
        .position(|sql| sql.contains("ALTER TABLE filters ADD COLUMN rules TEXT"))
        .expect("filter rules migration must exist");
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    apply_migrations_up_to(&conn, version)?;
    Ok(conn)
}

/// Drop target when dragging a list folder relative to another list or folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderDropTarget {
    List(Id),
    Folder(Id),
}

const TAG_ORDER_KEY: &str = "tag_order";
const FILTER_ORDER_KEY: &str = "filter_order";

const TASK_COLUMNS: &str = "id, parent_id, kind, title, notes, priority, due_date, due_time, \
                            completed_at, created_at, list_id, deleted_at, repeat, wont_do_at, reminders, end_date, end_time, modified_at, \
                            pinned_at, section_id, estimate, in_progress_at, location, timezone, is_floating, snoozed_until, constant_reminder, \
                            start_date, schedule_bucket";

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

    fn write_sort_order(&self, table: &'static str, ids: &[Id]) -> Result<()> {
        let sql = format!("UPDATE {table} SET sort_order = ?2 WHERE id = ?1 AND sort_order != ?2");
        let mut stmt = self.conn.prepare_cached(&sql)?;
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
        // The views run the same queries on every change; parse each once.
        conn.set_prepared_statement_cache_capacity(64);
        Self::migrate(&conn)?;
        Ok(Self { conn, attachments_dir })
    }

    fn migrate(conn: &Connection) -> Result<()> {
        apply_migrations_up_to(conn, MIGRATIONS.len())
    }

    /// Runs `PRAGMA integrity_check(10)` and returns the resulting messages (`["ok"]` when healthy).
    pub fn integrity_check(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("PRAGMA integrity_check(10)")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Returns the database's current `PRAGMA user_version`.
    pub fn schema_version(&self) -> Result<usize> {
        Ok(self.conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    /// Returns row counts for the primary user-facing tables.
    pub fn table_counts(&self) -> Result<TableCounts> {
        let count = |table: &str| -> Result<usize> {
            let n: i64 = self.conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
            Ok(usize::try_from(n).unwrap_or(0))
        };
        Ok(TableCounts {
            tasks: count("tasks")?,
            lists: count("lists")?,
            habits: count("habits")?,
            attachments: count("task_attachments")?,
            countdowns: count("countdowns")?,
        })
    }

    /// Whether `path` is a Fern backup this version can restore.
    pub fn check_backup(path: &std::path::Path) -> Result<()> {
        let backup = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let is_fern: bool = backup
            .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tasks'", [], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        anyhow::ensure!(is_fern, "not a Fern backup");
        let version: usize = backup.pragma_query_value(None, "user_version", |row| row.get(0))?;
        anyhow::ensure!(version <= MIGRATIONS.len(), "the backup comes from a newer version of Fern");
        Ok(())
    }

    /// "Generate Backup": a copy of all data in one SQLite file at `path`. With `attachments`
    /// (the attachment folder), the attached files go into the copy too, in a `backup_files`
    /// table, so that the backup stays one file.
    pub fn backup_to(&self, path: &std::path::Path, attachments: Option<&std::path::Path>) -> Result<()> {
        self.conn.backup(rusqlite::MAIN_DB, path, None)?;
        if let Some(dir) = attachments.filter(|dir| dir.is_dir()) {
            let copy = Connection::open(path)?;
            copy.execute_batch("CREATE TABLE backup_files (name TEXT PRIMARY KEY, data BLOB NOT NULL);")?;
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                if entry.file_type()?.is_file()
                    && let Some(name) = entry.file_name().to_str()
                {
                    copy.execute(
                        "INSERT INTO backup_files (name, data) VALUES (?1, ?2)",
                        params![name, std::fs::read(entry.path())?],
                    )?;
                }
            }
        }
        Ok(())
    }

    /// The daily copy of the data: `fern-<date>.db` in `dir`, made once a day, keeping the
    /// last `keep` copies. Attached files are not copied: they stay in their own folder.
    pub fn daily_backup(&self, dir: &std::path::Path, date: NaiveDate, keep: usize) -> Result<()> {
        let path = dir.join(format!("fern-{}.db", date.format(DATE_FORMAT)));
        if path.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(dir)?;
        self.backup_to(&path, None)?;
        let mut copies: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("fern-") && n.ends_with(".db")))
            .collect();
        copies.sort();
        for old in copies.iter().rev().skip(keep) {
            std::fs::remove_file(old)?;
            // What SQLite leaves beside a copy that was opened.
            for suffix in ["-wal", "-shm"] {
                let mut side = old.clone().into_os_string();
                side.push(suffix);
                std::fs::remove_file(side).ok();
            }
        }
        Ok(())
    }

    /// "Import Backups": replaces all data with a backup made by `backup_to`, bringing an older
    /// backup up to date. A file that is not a Fern backup is refused and nothing changes. When
    /// the backup carries attached files, the attachment folder `attachments` is replaced by them.
    pub fn restore_from(&mut self, path: &std::path::Path, attachments: Option<&std::path::Path>) -> Result<()> {
        Self::check_backup(path)?;
        self.conn.restore(rusqlite::MAIN_DB, path, None::<fn(rusqlite::backup::Progress)>)?;
        let has_files: bool = self
            .conn
            .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'backup_files'", [], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        if has_files {
            if let Some(dir) = attachments {
                if dir.is_dir() {
                    std::fs::remove_dir_all(dir)?;
                }
                std::fs::create_dir_all(dir)?;
                let mut stmt = self.conn.prepare_cached("SELECT name, data FROM backup_files")?;
                let files = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)))?;
                for file in files {
                    let (name, data) = file?;
                    // Only plain names: a backup must not write outside the folder.
                    if !name.contains(['/', '\\']) && name != ".." {
                        std::fs::write(dir.join(name), data)?;
                    }
                }
            }
            self.conn.execute_batch("DROP TABLE backup_files;")?;
        }
        Self::migrate(&self.conn)
    }

    /// All tasks: open ones first (newest on top), then completed ones (most recent on top).
    pub fn tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks
             ORDER BY completed_at IS NOT NULL, completed_at DESC, sort_order, rowid DESC"
        ))?;
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
                .filter(|task| family.iter().any(|t| matches_view(task, &View::Tag(t.clone()), today)))
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
            self.conn.prepare_cached("SELECT tag FROM task_tags WHERE task_id = ?1 ORDER BY tag")?;
        task.tags = stmt.query_map([id.to_string()], |row| row.get(0))?.collect::<Result<_, _>>()?;
        let (done, total): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(completed_at), COUNT(*) FROM checklist_items WHERE task_id = ?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
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
            schedule_bucket: draft.schedule_bucket,
            created_at: now_ms(),
        };
        let order = if top {
            "(SELECT COALESCE(MIN(sort_order), 0) - 1024 FROM tasks)"
        } else {
            "(SELECT COALESCE(MAX(sort_order), 0) + 1024 FROM tasks)"
        };
        if task.location.is_some()
            || task.timezone.is_some()
            || task.is_floating
            || task.constant_reminder
            || task.start_date.is_some()
            || task.schedule_bucket != ScheduleBucket::None
        {
            self.conn.execute(
                &format!("INSERT INTO tasks (id, title, notes, list_id, kind, priority, due_date, due_time,
                          repeat, reminders, created_at, end_date, end_time, location, timezone, is_floating,
                          constant_reminder, start_date, schedule_bucket, sort_order)
                          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, {order})"),
                params![
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
                ],
            )?;
        } else {
            self.conn.execute(
                &format!("INSERT INTO tasks (id, title, notes, list_id, kind, priority, due_date, due_time,
                          repeat, reminders, created_at, end_date, end_time, sort_order)
                          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, {order})"),
                params![
                    task.id.to_string(), task.title, task.notes, task.list_id.map(|id| id.to_string()),
                    task.kind.as_str(), task.priority.to_i64(),
                    task.due_date.map(|date| date.format(DATE_FORMAT).to_string()),
                    task.due_time.map(|time| time.format(TIME_FORMAT).to_string()),
                    task.repeat.as_ref().map(ToString::to_string), crate::reminder::format(&task.reminders),
                    task.created_at,
                    task.end_date.map(|date| date.format(DATE_FORMAT).to_string()),
                    task.end_time.map(|time| time.format(TIME_FORMAT).to_string()),
                ],
            )?;
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

    /// Completes or reopens a task, cascading completion to open descendants and reopening closed
    /// ancestors when a subtask is reopened. Returns the repeating copy (if
    /// any) and the list of descendants newly completed alongside `id`.
    pub fn complete_with_cascaded(&self, id: Id, completed: bool) -> Result<(Option<Id>, Vec<Id>)> {
        if completed
            && let Some(task) = self.task(id)?
            && let (Some(repeat), Some(anchor_date)) = (task.repeat.clone(), task.due_date.or(task.start_date))
        {
            let today = chrono::Local::now().date_naive();
            let holidays = if repeat.skip_holidays {
                let anchor = if repeat.from_completion { today } else { anchor_date };
                self.holiday_dates(anchor, anchor + Duration::days(400))?
            } else {
                HashSet::new()
            };
            if let Some(next) = repeat.next_for_with_holidays(anchor_date, today, |d| holidays.contains(&d)) {
                let copy = self.in_tx(|| {
                    let copy = Uuid::now_v7();
                    self.conn.execute(
                        &format!(
                            "INSERT INTO tasks ({TASK_COLUMNS})
                             SELECT ?2, parent_id, kind, title, notes, priority, due_date, due_time,
                                    ?3, created_at, list_id, NULL, NULL, NULL, NULL, end_date, end_time, ?3, NULL,
                                    section_id, estimate, NULL, location, timezone, is_floating, NULL, 0,
                                    start_date, schedule_bucket
                             FROM tasks WHERE id = ?1"
                        ),
                        params![id.to_string(), copy.to_string(), now_ms()],
                    )?;
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
    /// its earlier dates (and repeat rule if stateful, e.g. forgetting curve).
    pub fn undo_repeat(
        &self,
        id: Id,
        copy: Id,
        due: Option<NaiveDate>,
        end: Option<NaiveDate>,
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
            let copy_schedule: Option<(Option<String>, String)> = self
                .conn
                .query_row(
                    "SELECT start_date, schedule_bucket FROM tasks WHERE id = ?1",
                    [copy.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            self.conn.execute(
                "UPDATE checklist_items AS original SET completed_at = copied.completed_at
                 FROM repeat_checklist_copies AS mapping JOIN checklist_items AS copied ON copied.id = mapping.copy_id
                 WHERE original.id = mapping.source_id AND original.task_id = ?1 AND copied.task_id = ?2",
                params![id.to_string(), copy.to_string()],
            )?;
            self.conn.execute("DELETE FROM tasks WHERE id = ?1", [copy.to_string()])?;
            if let Some((start_date, schedule_bucket)) = copy_schedule {
                self.conn.execute(
                    "UPDATE tasks SET due_date = ?2, end_date = ?3, repeat = COALESCE(?4, repeat),
                            start_date = ?5, schedule_bucket = ?6 WHERE id = ?1",
                    params![
                        id.to_string(),
                        due.map(|d| d.format(DATE_FORMAT).to_string()),
                        end.map(|d| d.format(DATE_FORMAT).to_string()),
                        repeat.map(ToString::to_string),
                        start_date,
                        schedule_bucket,
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
                     start_date = o.start_date, schedule_bucket = o.schedule_bucket
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
            "UPDATE tasks SET due_date = ?2, end_date = ?3, end_time = ?4 WHERE id = ?1",
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
            "UPDATE tasks SET due_date = ?2, end_date = ?3, end_time = ?4 WHERE id = ?1",
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
            "UPDATE tasks SET due_date = ?2, due_time = ?3, end_date = ?4, end_time = ?5 WHERE id = ?1",
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
            self.write_sort_order("list_sections", &order)?;
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
            self.write_sort_order("list_sections", &order)
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
        self.conn.execute(
            "UPDATE tasks SET due_date = ?2, due_time = ?3, snoozed_until = NULL WHERE id = ?1",
            params![id.to_string(), date_str, time_str],
        )?;
        Ok(())
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
            self.write_sort_order("lists", &order)?;
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
            self.write_sort_order("lists", &order)?;
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
            self.write_sort_order("lists", &remaining)?;
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
            self.write_sort_order("lists", &order)?;
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
                self.write_sort_order("lists", &visual)?;
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
            self.write_sort_order("list_folders", &folder_order)?;
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
            self.write_sort_order("list_folders", &folder_ids)?;
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
    /// The subscribed calendars, in the order they were added.
    pub fn subscriptions(&self) -> Result<Vec<crate::ical::Subscription>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, url, color, hidden, feed, fetched_at, error, kind, provider, username, password, writable, sync_token
             FROM calendar_subscriptions ORDER BY created_at, rowid",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let kind_str: String = row.get(8)?;
                let provider_str: String = row.get(9)?;
                Ok(crate::ical::Subscription {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    url: row.get(2)?,
                    color: row.get(3)?,
                    hidden: row.get(4)?,
                    feed: row.get(5)?,
                    fetched_at: row.get(6)?,
                    error: row.get(7)?,
                    kind: crate::ical::CalendarKind::from_code(&kind_str),
                    provider: crate::ical::CalendarProvider::from_code(&provider_str),
                    username: row.get(10)?,
                    password: row.get(11)?,
                    writable: row.get::<_, i64>(12)? != 0,
                    sync_token: row.get(13)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn add_subscription(&self, name: &str, url: &str, color: Option<&str>) -> Result<Id> {
        self.insert_subscription(name, url, color, None)
    }

    pub fn add_subscription_with_feed(&self, name: &str, url: &str, color: Option<&str>, feed: &str) -> Result<Id> {
        self.insert_subscription(name, url, color, Some(feed))
    }

    fn insert_subscription(&self, name: &str, url: &str, color: Option<&str>, feed: Option<&str>) -> Result<Id> {
        let url = url.trim();
        anyhow::ensure!(!url.is_empty(), "a subscription needs an address");
        let id = Uuid::now_v7();
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO calendar_subscriptions (id, name, url, color, created_at, feed, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id.to_string(), name.trim(), url, color, now, feed.unwrap_or_default(), feed.map(|_| now)],
        )?;
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_caldav_subscription(
        &self,
        name: &str,
        url: &str,
        color: Option<&str>,
        provider: crate::ical::CalendarProvider,
        username: &str,
        password: &str,
        writable: bool,
    ) -> Result<Id> {
        let url = url.trim();
        anyhow::ensure!(!url.is_empty(), "a CalDAV calendar needs an address");
        let id = Uuid::now_v7();
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO calendar_subscriptions (id, name, url, color, created_at, feed, fetched_at, kind, provider, username, password, writable)
             VALUES (?1, ?2, ?3, ?4, ?5, '', NULL, 'caldav', ?6, ?7, ?8, ?9)",
            params![
                id.to_string(),
                name.trim(),
                url,
                color,
                now,
                provider.code(),
                username.trim(),
                password,
                i64::from(writable),
            ],
        )?;
        Ok(id)
    }

    /// Changes a subscription; a new address drops the feed read from the old one.
    pub fn update_subscription(&self, id: Id, name: &str, url: &str, color: Option<&str>, hidden: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE calendar_subscriptions SET name = ?2, color = ?4, hidden = ?5,
                 feed = CASE WHEN url = ?3 THEN feed ELSE '' END,
                 fetched_at = CASE WHEN url = ?3 THEN fetched_at END,
                 url = ?3
             WHERE id = ?1",
            params![id.to_string(), name.trim(), url.trim(), color, hidden],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_caldav_subscription(
        &self,
        id: Id,
        name: &str,
        url: &str,
        color: Option<&str>,
        hidden: bool,
        provider: crate::ical::CalendarProvider,
        username: &str,
        password: &str,
        writable: bool,
    ) -> Result<()> {
        let url = url.trim();
        let prev_url: Option<String> = self
            .conn
            .query_row("SELECT url FROM calendar_subscriptions WHERE id = ?1", params![id.to_string()], |r| r.get(0))
            .optional()?;
        let url_changed = prev_url.as_deref() != Some(url);
        self.in_tx(|| {
            self.conn.execute(
                "UPDATE calendar_subscriptions
                 SET name = ?2, color = ?4, hidden = ?5, kind = 'caldav',
                     provider = ?6, username = ?7, password = ?8, writable = ?9,
                     feed = CASE WHEN url = ?3 THEN feed ELSE '' END,
                     fetched_at = CASE WHEN url = ?3 THEN fetched_at ELSE NULL END,
                     sync_token = CASE WHEN url = ?3 THEN sync_token ELSE NULL END,
                     error = CASE WHEN url = ?3 THEN error ELSE NULL END,
                     url = ?3
                 WHERE id = ?1",
                params![
                    id.to_string(),
                    name.trim(),
                    url,
                    color,
                    hidden,
                    provider.code(),
                    username.trim(),
                    password,
                    i64::from(writable),
                ],
            )?;
            if url_changed {
                self.conn.execute("DELETE FROM calendar_events WHERE subscription_id = ?1", params![id.to_string()])?;
            }
            Ok(())
        })
    }

    pub fn delete_subscription(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM calendar_subscriptions WHERE id = ?1", params![id.to_string()])?;
        Ok(())
    }

    /// Keeps the subscription's identity and preferences while replacing its address and feed together.
    pub fn replace_subscription_source(&self, id: Id, previous_url: &str, url: &str, feed: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE calendar_subscriptions SET url = ?3, feed = ?4, fetched_at = ?5, error = NULL
             WHERE id = ?1 AND url = ?2 AND NOT EXISTS (
                 SELECT 1 FROM calendar_subscriptions WHERE url = ?3 AND id != ?1
             )",
            params![id.to_string(), previous_url, url, feed, now_ms()],
        )?;
        Ok(changed == 1)
    }

    /// Keeps what was read from a subscription's address: the feed, or the error that kept it
    /// (the last feed read then stays).
    pub fn save_subscription_feed(&self, id: Id, url: &str, feed: Result<&str, &str>) -> Result<bool> {
        let changed = match feed {
            Ok(feed) => self.conn.execute(
                "UPDATE calendar_subscriptions SET feed = ?3, fetched_at = ?4, error = NULL WHERE id = ?1 AND url = ?2",
                params![id.to_string(), url, feed, now_ms()],
            )?,
            Err(error) => self.conn.execute(
                "UPDATE calendar_subscriptions SET error = ?3 WHERE id = ?1 AND url = ?2",
                params![id.to_string(), url, error],
            )?,
        };
        Ok(changed == 1)
    }

    /// Cached CalDAV events for a subscription, including locally queued deletions.
    pub fn calendar_events(&self, subscription_id: Id) -> Result<Vec<crate::ical::StoredEvent>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT subscription_id, uid, href, etag, ics, dirty FROM calendar_events
             WHERE subscription_id = ?1 ORDER BY updated_at, uid",
        )?;
        let rows = stmt
            .query_map(params![subscription_id.to_string()], |row| {
                Ok(crate::ical::StoredEvent {
                    subscription_id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    uid: row.get(1)?,
                    href: row.get(2)?,
                    etag: row.get(3)?,
                    ics: row.get(4)?,
                    dirty: crate::ical::EventDirty::from_i64(row.get(5)?),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Queues or updates a local CalDAV event edit (`dirty = Upsert`) and refreshes the combined feed.
    pub fn upsert_local_calendar_event(&self, subscription_id: Id, uid: &str, href: &str, ics: &str) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute(
                "INSERT INTO calendar_events (subscription_id, uid, href, etag, ics, dirty, updated_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, 1, ?5)
                 ON CONFLICT(subscription_id, uid) DO UPDATE SET
                     href = CASE WHEN excluded.href != '' THEN excluded.href ELSE calendar_events.href END,
                     ics = excluded.ics,
                     dirty = 1,
                     updated_at = excluded.updated_at",
                params![subscription_id.to_string(), uid, href, ics, now_ms()],
            )?;
            rebuild_caldav_feed(&self.conn, subscription_id)?;
            Ok(())
        })
    }

    /// Marks a CalDAV event for remote deletion (or drops it immediately if never pushed).
    pub fn mark_calendar_event_deleted(&self, subscription_id: Id, uid: &str) -> Result<bool> {
        self.in_tx(|| {
            let existing: Option<(Option<String>, i64)> = self
                .conn
                .query_row(
                    "SELECT etag, dirty FROM calendar_events WHERE subscription_id = ?1 AND uid = ?2",
                    params![subscription_id.to_string(), uid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((etag, dirty)) = existing else {
                return Ok(false);
            };
            if etag.is_none() && dirty == crate::ical::EventDirty::Upsert.to_i64() {
                self.conn.execute(
                    "DELETE FROM calendar_events WHERE subscription_id = ?1 AND uid = ?2",
                    params![subscription_id.to_string(), uid],
                )?;
            } else {
                self.conn.execute(
                    "UPDATE calendar_events SET dirty = 2, updated_at = ?3 WHERE subscription_id = ?1 AND uid = ?2",
                    params![subscription_id.to_string(), uid, now_ms()],
                )?;
            }
            rebuild_caldav_feed(&self.conn, subscription_id)?;
            Ok(true)
        })
    }

    /// Marks a dirty local event as synced cleanly after `PUT`.
    pub fn mark_caldav_event_clean(&self, subscription_id: Id, uid: &str, href: &str, etag: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE calendar_events SET href = ?3, etag = ?4, dirty = 0, updated_at = ?5
             WHERE subscription_id = ?1 AND uid = ?2 AND dirty = 1",
            params![subscription_id.to_string(), uid, href, etag, now_ms()],
        )?;
        Ok(())
    }

    /// Removes a deleted event record after remote `DELETE` succeeds.
    pub fn remove_caldav_event(&self, subscription_id: Id, uid: &str) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute(
                "DELETE FROM calendar_events WHERE subscription_id = ?1 AND uid = ?2 AND dirty = 2",
                params![subscription_id.to_string(), uid],
            )?;
            rebuild_caldav_feed(&self.conn, subscription_id)?;
            Ok(())
        })
    }

    /// Replaces clean remote CalDAV events from a remote fetch while keeping any still-dirty local edits.
    pub fn replace_caldav_snapshot(
        &self,
        subscription_id: Id,
        url: &str,
        remote: &[crate::ical::StoredEvent],
        sync_token: Option<&str>,
    ) -> Result<bool> {
        self.in_tx(|| {
            let still_matches: Option<String> = self
                .conn
                .query_row(
                    "SELECT url FROM calendar_subscriptions WHERE id = ?1",
                    params![subscription_id.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            if still_matches.as_deref() != Some(url) {
                return Ok(false);
            }

            let now = now_ms();
            self.conn.execute("DELETE FROM calendar_events WHERE subscription_id = ?1 AND dirty = 0", params![subscription_id.to_string()])?;
            for ev in remote {
                self.conn.execute(
                    "INSERT INTO calendar_events (subscription_id, uid, href, etag, ics, dirty, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)
                     ON CONFLICT(subscription_id, uid) DO UPDATE SET
                         href = excluded.href,
                         etag = excluded.etag,
                         ics = excluded.ics,
                         dirty = 0,
                         updated_at = excluded.updated_at
                     WHERE calendar_events.dirty = 0",
                    params![subscription_id.to_string(), ev.uid, ev.href, ev.etag, ev.ics, now],
                )?;
            }
            rebuild_caldav_feed(&self.conn, subscription_id)?;
            self.conn.execute(
                "UPDATE calendar_subscriptions SET fetched_at = ?2, error = NULL, sync_token = ?3 WHERE id = ?1 AND url = ?4",
                params![subscription_id.to_string(), now, sync_token, url],
            )?;
            Ok(true)
        })
    }

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

    pub fn habits(&self) -> Result<Vec<Habit>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, goal_amount, goal_unit, goal_step, reminders, frequency, icon, color,
                    start_date, goal_days, section, archived_at, icon_checkins, constant_reminder,
                    record_enable, ex_dates, sort_order
             FROM habits
             ORDER BY sort_order, created_at, rowid",
        )?;
        let habits = stmt
            .query_map([], |row| {
                let step = row.get::<_, Option<f64>>(4)?.unwrap_or(1.0);
                let goal = row.get::<_, Option<f64>>(2)?.map(|amount| Goal {
                    amount: polish_amount(amount).max(0.01),
                    unit: row.get::<_, Option<String>>(3).ok().flatten().unwrap_or_default(),
                    mode: CheckinMode::from_storage(step),
                });
                let reminders = row
                    .get::<_, Option<String>>(5)?
                    .map(|times| {
                        times
                            .split(',')
                            .filter_map(|t| NaiveTime::parse_from_str(t.trim(), TIME_FORMAT).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                let frequency = row
                    .get::<_, Option<String>>(6)?
                    .map(|f| crate::habit::Frequency::from_storage(&f))
                    .unwrap_or_default();
                let archived_at: Option<i64> = row.get(12)?;
                let archived_date = archived_at
                    .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
                    .map(|dt| dt.date_naive());
                let mut ex_dates: Vec<NaiveDate> = row
                    .get::<_, Option<String>>(16)?
                    .map(|raw| {
                        raw.split(',')
                            .filter_map(|d| NaiveDate::parse_from_str(d.trim(), DATE_FORMAT).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                ex_dates.sort_unstable();
                ex_dates.dedup();
                Ok(Habit {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    goal,
                    reminders,
                    frequency,
                    icon: row.get(7)?,
                    color: row.get(8)?,
                    start_date: row
                        .get::<_, Option<String>>(9)?
                        .and_then(|d| NaiveDate::parse_from_str(&d, DATE_FORMAT).ok()),
                    goal_days: row.get(10)?,
                    section: row
                        .get::<_, Option<String>>(11)?
                        .map(|s| HabitSection::from_key(&s))
                        .unwrap_or_default(),
                    archived: archived_at.is_some(),
                    archived_date,
                    icon_checkins: row.get(13)?,
                    constant_reminder: row.get(14)?,
                    record_enable: row.get::<_, Option<bool>>(15)?.unwrap_or(false),
                    ex_dates,
                    sort_order: row.get::<_, Option<i64>>(17)?.unwrap_or(0),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(habits)
    }

    /// Creates a habit atomically with all fields from `draft`.
    pub fn create_habit(&self, draft: &HabitDraft) -> Result<Habit> {
        let name = draft.name.trim();
        anyhow::ensure!(!name.is_empty(), "a habit needs a name");
        let goal = match &draft.goal {
            Some(g) => {
                let amount = polish_amount(g.amount);
                anyhow::ensure!(amount > 0.0, "a goal needs an amount");
                let mode = match g.mode {
                    CheckinMode::Auto { step } => CheckinMode::Auto { step: polish_amount(step).max(0.01) },
                    other => other,
                };
                Some(Goal { amount, unit: g.unit.trim().to_owned(), mode })
            }
            None => None,
        };
        let mut reminders = draft.reminders.clone();
        reminders.sort();
        reminders.dedup();
        reminders.truncate(MAX_REMINDERS);
        let reminders_text: Vec<String> = reminders.iter().map(|t| t.format(TIME_FORMAT).to_string()).collect();
        let constant_reminder = !reminders.is_empty() && draft.constant_reminder;
        let frequency = draft.frequency.normalize();
        let icon = draft
            .icon
            .as_deref()
            .map(str::trim)
            .filter(|i| !i.is_empty())
            .map(|i| i.chars().take(2).collect::<String>());
        let color = draft.color.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(str::to_owned);
        let goal_days = draft.goal_days.filter(|n| *n > 0);
        let id = Uuid::now_v7();

        let sort_order = self.in_tx(|| {
            let next_order: i64 = self.conn.query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM habits",
                [],
                |row| row.get(0),
            )?;
            self.conn.execute(
                "INSERT INTO habits (
                    id, name, goal_amount, goal_unit, goal_step, reminders, frequency,
                    icon, color, start_date, goal_days, section, constant_reminder,
                    record_enable, sort_order, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    id.to_string(),
                    name,
                    goal.as_ref().map(|g| g.amount),
                    goal.as_ref().map(|g| g.unit.as_str()),
                    goal.as_ref().map_or(1.0, |g| g.mode.to_storage()),
                    (!reminders_text.is_empty()).then(|| reminders_text.join(",")),
                    frequency.to_storage(),
                    icon,
                    color,
                    draft.start_date.map(|d| d.format(DATE_FORMAT).to_string()),
                    goal_days,
                    draft.section.key(),
                    constant_reminder,
                    draft.record_enable,
                    next_order,
                    now_ms(),
                ],
            )?;
            Ok(next_order)
        })?;

        Ok(Habit {
            id,
            name: name.to_owned(),
            goal,
            reminders,
            frequency,
            icon,
            color,
            start_date: draft.start_date,
            goal_days,
            section: draft.section,
            archived: false,
            archived_date: None,
            icon_checkins: false,
            constant_reminder,
            record_enable: draft.record_enable,
            ex_dates: Vec::new(),
            sort_order,
        })
    }

    /// Updates all editable fields of a habit atomically in a single statement.
    pub fn save_habit(&self, id: Id, draft: &HabitDraft) -> Result<()> {
        let name = draft.name.trim();
        anyhow::ensure!(!name.is_empty(), "a habit needs a name");
        let goal = match &draft.goal {
            Some(g) => {
                let amount = polish_amount(g.amount);
                anyhow::ensure!(amount > 0.0, "a goal needs an amount");
                let mode = match g.mode {
                    CheckinMode::Auto { step } => CheckinMode::Auto { step: polish_amount(step).max(0.01) },
                    other => other,
                };
                Some(Goal { amount, unit: g.unit.trim().to_owned(), mode })
            }
            None => None,
        };
        let mut reminders = draft.reminders.clone();
        reminders.sort();
        reminders.dedup();
        reminders.truncate(MAX_REMINDERS);
        let reminders_text: Vec<String> = reminders.iter().map(|t| t.format(TIME_FORMAT).to_string()).collect();
        let constant_reminder = !reminders.is_empty() && draft.constant_reminder;
        let frequency = draft.frequency.normalize();
        let icon = draft
            .icon
            .as_deref()
            .map(str::trim)
            .filter(|i| !i.is_empty())
            .map(|i| i.chars().take(2).collect::<String>());
        let color = draft.color.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(str::to_owned);
        let goal_days = draft.goal_days.filter(|n| *n > 0);

        self.conn.execute(
            "UPDATE habits SET
                name = ?2,
                goal_amount = ?3,
                goal_unit = ?4,
                goal_step = ?5,
                reminders = ?6,
                frequency = ?7,
                icon = ?8,
                color = ?9,
                start_date = ?10,
                goal_days = ?11,
                section = ?12,
                constant_reminder = ?13,
                record_enable = ?14
             WHERE id = ?1",
            params![
                id.to_string(),
                name,
                goal.as_ref().map(|g| g.amount),
                goal.as_ref().map(|g| g.unit.as_str()),
                goal.as_ref().map_or(1.0, |g| g.mode.to_storage()),
                (!reminders_text.is_empty()).then(|| reminders_text.join(",")),
                frequency.to_storage(),
                icon,
                color,
                draft.start_date.map(|d| d.format(DATE_FORMAT).to_string()),
                goal_days,
                draft.section.key(),
                constant_reminder,
                draft.record_enable,
            ],
        )?;
        Ok(())
    }

    pub fn add_habit(&self, name: &str) -> Result<Habit> {
        self.create_habit(&HabitDraft { name: name.to_owned(), ..Default::default() })
    }

    pub fn delete_habit(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM habits WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    /// Sets a habit's icon (an emoji or up to two characters; empty for the initial) and color.
    pub fn set_habit_look(&self, id: Id, icon: Option<&str>, color: Option<&str>) -> Result<()> {
        let icon = icon.map(str::trim).filter(|i| !i.is_empty()).map(|i| i.chars().take(2).collect::<String>());
        self.conn.execute(
            "UPDATE habits SET icon = ?2, color = ?3 WHERE id = ?1",
            params![id.to_string(), icon, color],
        )?;
        Ok(())
    }

    /// Sets when a habit starts and its "Goal Days" (`None`: from creation, forever).
    pub fn set_habit_span(&self, id: Id, start: Option<NaiveDate>, goal_days: Option<u32>) -> Result<()> {
        self.conn.execute(
            "UPDATE habits SET start_date = ?2, goal_days = ?3 WHERE id = ?1",
            params![id.to_string(), start.map(|d| d.format(DATE_FORMAT).to_string()), goal_days.filter(|n| *n > 0)],
        )?;
        Ok(())
    }

    pub fn set_habit_section(&self, id: Id, section: HabitSection) -> Result<()> {
        self.conn.execute("UPDATE habits SET section = ?2 WHERE id = ?1", params![id.to_string(), section.key()])?;
        Ok(())
    }

    pub fn set_habit_record_enable(&self, id: Id, on: bool) -> Result<()> {
        self.conn.execute("UPDATE habits SET record_enable = ?2 WHERE id = ?1", params![id.to_string(), on])?;
        Ok(())
    }

    /// Reorders habits and optionally moves them between sections in one transaction.
    pub fn reorder_habits(&self, updates: &[(Id, HabitSection, i64)]) -> Result<()> {
        self.in_tx(|| {
            let mut stmt = self.conn.prepare_cached("UPDATE habits SET section = ?2, sort_order = ?3 WHERE id = ?1")?;
            for (id, section, order) in updates {
                stmt.execute(params![id.to_string(), section.key(), order])?;
            }
            Ok(())
        })
    }

    /// Sections the user added, in display order.
    pub fn habit_sections(&self) -> Result<Vec<crate::habit::CustomSection>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, name, sort_order FROM habit_sections ORDER BY sort_order, created_at, rowid")?;
        let sections = stmt
            .query_map([], |row| {
                Ok(crate::habit::CustomSection {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    sort_order: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(sections)
    }

    pub fn add_habit_section(&self, name: &str) -> Result<crate::habit::CustomSection> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a section needs a name");
        let existing_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM habit_sections", [], |row| row.get(0))?;
        anyhow::ensure!((existing_count as usize) < MAX_CUSTOM_SECTIONS, "too many custom sections");
        let next_order: i64 =
            self.conn.query_row("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM habit_sections", [], |row| row.get(0))?;
        let section = crate::habit::CustomSection { id: Uuid::now_v7(), name: name.to_owned(), sort_order: next_order };
        self.conn.execute(
            "INSERT INTO habit_sections (id, name, sort_order, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![section.id.to_string(), section.name, section.sort_order, now_ms()],
        )?;
        Ok(section)
    }

    pub fn rename_habit_section(&self, id: Id, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a section needs a name");
        self.conn.execute("UPDATE habit_sections SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(())
    }

    /// Reorders custom habit sections in one transaction.
    pub fn reorder_habit_sections(&self, ordered_ids: &[Id]) -> Result<()> {
        self.in_tx(|| {
            let mut stmt = self.conn.prepare_cached("UPDATE habit_sections SET sort_order = ?2 WHERE id = ?1")?;
            for (index, id) in ordered_ids.iter().enumerate() {
                stmt.execute(params![id.to_string(), index as i64])?;
            }
            Ok(())
        })
    }

    /// Deletes a section; its habits move to Others.
    pub fn delete_habit_section(&self, id: Id) -> Result<()> {
        let key = HabitSection::Custom(id).key();
        self.in_tx(|| {
            self.conn.execute("UPDATE habits SET section = NULL WHERE section = ?1", [key])?;
            self.conn.execute("DELETE FROM habit_sections WHERE id = ?1", [id.to_string()])?;
            Ok(())
        })
    }

    /// "Constant Reminder": the habit's reminders stay on screen until answered.
    pub fn set_habit_constant_reminder(&self, id: Id, on: bool) -> Result<()> {
        self.conn.execute("UPDATE habits SET constant_reminder = ?2 WHERE id = ?1", params![id.to_string(), on])?;
        Ok(())
    }

    /// "Checked-in Style": show checked-in days with the habit's icon instead of a checkmark.
    pub fn set_habit_icon_checkins(&self, id: Id, on: bool) -> Result<()> {
        self.conn.execute("UPDATE habits SET icon_checkins = ?2 WHERE id = ?1", params![id.to_string(), on])?;
        Ok(())
    }

    pub fn set_habit_archived(&self, id: Id, archived: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE habits SET archived_at = ?2 WHERE id = ?1",
            params![id.to_string(), archived.then(now_ms)],
        )?;
        Ok(())
    }

    pub fn set_habit_frequency(&self, id: Id, frequency: crate::habit::Frequency) -> Result<()> {
        self.conn.execute(
            "UPDATE habits SET frequency = ?2 WHERE id = ?1",
            params![id.to_string(), frequency.normalize().to_storage()],
        )?;
        Ok(())
    }

    /// Sets a habit's reminder times; they are kept sorted, deduplicated, and capped.
    pub fn set_habit_reminders(&self, id: Id, times: &[NaiveTime]) -> Result<()> {
        let mut times = times.to_vec();
        times.sort();
        times.dedup();
        times.truncate(MAX_REMINDERS);
        let text: Vec<String> = times.iter().map(|t| t.format(TIME_FORMAT).to_string()).collect();
        self.conn.execute(
            "UPDATE habits SET reminders = ?2 WHERE id = ?1",
            params![id.to_string(), (!text.is_empty()).then(|| text.join(","))],
        )?;
        Ok(())
    }

    /// "Snooze" in a habit's reminder: remind again at `at`. Snoozes more than a day old are
    /// dropped on the way.
    pub fn snooze_habit(&self, id: Id, at: NaiveDateTime) -> Result<()> {
        let stale = (at - chrono::Duration::days(1)).format(SNOOZE_FORMAT).to_string();
        self.conn.execute("DELETE FROM habit_snoozes WHERE at < ?1", [stale])?;
        self.conn.execute(
            "INSERT INTO habit_snoozes (habit_id, at) VALUES (?1, ?2)",
            params![id.to_string(), at.format(SNOOZE_FORMAT).to_string()],
        )?;
        Ok(())
    }

    /// Snoozed habit reminders, as (habit, when to remind again).
    pub fn habit_snoozes(&self) -> Result<Vec<(Id, NaiveDateTime)>> {
        let mut stmt = self.conn.prepare_cached("SELECT habit_id, at FROM habit_snoozes ORDER BY at")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        let mut snoozes = Vec::new();
        for row in rows {
            let (id, at) = row?;
            if let (Ok(id), Ok(at)) = (id.parse(), NaiveDateTime::parse_from_str(&at, SNOOZE_FORMAT)) {
                snoozes.push((id, at));
            }
        }
        Ok(snoozes)
    }

    pub fn update_habit(&self, id: Id, name: &str, goal: Option<&Goal>) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a habit needs a name");
        let goal_amount = match goal {
            Some(g) => {
                let amount = polish_amount(g.amount);
                anyhow::ensure!(amount > 0.0, "a goal needs an amount");
                Some(amount)
            }
            None => None,
        };
        self.conn.execute(
            "UPDATE habits SET name = ?2, goal_amount = ?3, goal_unit = ?4, goal_step = ?5 WHERE id = ?1",
            params![
                id.to_string(),
                name,
                goal_amount,
                goal.map(|g| g.unit.trim()),
                goal.map_or(1.0, |g| g.mode.to_storage()),
            ],
        )?;
        Ok(())
    }

    fn habit_goal_and_mode(&self, habit: Id) -> Result<(f64, CheckinMode)> {
        let (amount, step): (Option<f64>, Option<f64>) = self.conn.query_row(
            "SELECT goal_amount, goal_step FROM habits WHERE id = ?1",
            [habit.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let goal = amount.map(|a| polish_amount(a).max(0.01)).unwrap_or(1.0);
        let mode = if amount.is_some() {
            CheckinMode::from_storage(step.unwrap_or(1.0))
        } else {
            CheckinMode::All
        };
        Ok((goal, mode))
    }

    fn read_ex_dates(&self, habit: Id) -> Result<Vec<NaiveDate>> {
        let raw: Option<String> = self
            .conn
            .query_row("SELECT ex_dates FROM habits WHERE id = ?1", [habit.to_string()], |row| row.get(0))
            .optional()?
            .flatten();
        let mut dates: Vec<NaiveDate> = raw
            .map(|s| {
                s.split(',')
                    .filter_map(|d| NaiveDate::parse_from_str(d.trim(), DATE_FORMAT).ok())
                    .collect()
            })
            .unwrap_or_default();
        dates.sort_unstable();
        dates.dedup();
        Ok(dates)
    }

    fn write_ex_dates(&self, habit: Id, dates: &[NaiveDate]) -> Result<()> {
        let serialized: Option<String> = (!dates.is_empty())
            .then(|| dates.iter().map(|d| d.format(DATE_FORMAT).to_string()).collect::<Vec<_>>().join(","));
        self.conn.execute(
            "UPDATE habits SET ex_dates = ?2 WHERE id = ?1",
            params![habit.to_string(), serialized],
        )?;
        Ok(())
    }

    /// Marks `date` as skipped for `habit`, clearing any check-in status (while preserving logs if any).
    pub fn skip_habit_date(&self, habit: Id, date: NaiveDate) -> Result<()> {
        self.in_tx(|| {
            let mut dates = self.read_ex_dates(habit)?;
            if dates.binary_search(&date).is_err() {
                dates.push(date);
                dates.sort_unstable();
                self.write_ex_dates(habit, &dates)?;
            }
            self.reset_checkin_row(habit, date, true)?;
            Ok(())
        })
    }

    /// Removes `date` from `habit`'s skipped dates.
    pub fn unskip_habit_date(&self, habit: Id, date: NaiveDate) -> Result<()> {
        let mut dates = self.read_ex_dates(habit)?;
        let before = dates.len();
        dates.retain(|d| *d != date);
        if dates.len() != before {
            self.write_ex_dates(habit, &dates)?;
        }
        Ok(())
    }

    fn reset_checkin_row(&self, habit: Id, date: NaiveDate, keep_log: bool) -> Result<()> {
        let date_str = date.format(DATE_FORMAT).to_string();
        if !keep_log {
            self.conn.execute(
                "DELETE FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                params![habit.to_string(), date_str],
            )?;
            return Ok(());
        }
        let existing: Option<(Option<String>, i64)> = self
            .conn
            .query_row(
                "SELECT note, mood FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                params![habit.to_string(), date_str],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((note, mood)) = existing {
            let has_log = mood != 0 || note.as_deref().is_some_and(|n| !n.trim().is_empty());
            if has_log {
                self.conn.execute(
                    "UPDATE habit_checkins SET value = 0, status = 0 WHERE habit_id = ?1 AND date = ?2",
                    params![habit.to_string(), date_str],
                )?;
            } else {
                self.conn.execute(
                    "DELETE FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                    params![habit.to_string(), date_str],
                )?;
            }
        }
        Ok(())
    }

    /// Checks a habit in for `date` (reaching its goal at once), or resets the check-in when it
    /// is already `Done` (preserving any habit log/mood).
    pub fn toggle_checkin(&self, habit: Id, date: NaiveDate) -> Result<bool> {
        self.in_tx(|| {
            let date_str = date.format(DATE_FORMAT).to_string();
            let (goal, _) = self.habit_goal_and_mode(habit)?;
            let existing: Option<(i64, Option<f64>)> = self
                .conn
                .query_row(
                    "SELECT status, goal FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                    params![habit.to_string(), date_str],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((status, _)) = existing
                && CheckinStatus::from_i64(status) == CheckinStatus::Done
            {
                self.reset_checkin_row(habit, date, true)?;
                return Ok(false);
            }
            let eff_goal = existing.and_then(|(_, g)| g).filter(|g| *g > 0.0).unwrap_or(goal);
            self.conn.execute(
                "INSERT INTO habit_checkins (habit_id, date, value, goal, status) VALUES (?1, ?2, ?3, ?4, 2)
                 ON CONFLICT(habit_id, date) DO UPDATE SET value = excluded.value, goal = COALESCE(habit_checkins.goal, excluded.goal), status = 2",
                params![habit.to_string(), date_str, eff_goal, eff_goal],
            )?;
            self.unskip_habit_date(habit, date)?;
            Ok(true)
        })
    }

    /// Advances a habit's daily amount by its configured step (`Auto` step, `All` full goal, or `1.0`).
    /// A day that already reached `Done` resets back to `0` (preserving logs). Returns the new amount.
    pub fn step_checkin(&self, habit: Id, date: NaiveDate) -> Result<f64> {
        self.in_tx(|| {
            let date_str = date.format(DATE_FORMAT).to_string();
            let (goal, mode) = self.habit_goal_and_mode(habit)?;
            let step = match mode {
                CheckinMode::Auto { step } => step,
                CheckinMode::Manual => 1.0,
                CheckinMode::All => goal,
            };
            let existing: Option<(f64, i64, Option<f64>)> = self
                .conn
                .query_row(
                    "SELECT value, status, goal FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                    params![habit.to_string(), date_str],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let eff_goal = existing.and_then(|(_, _, g)| g).filter(|g| *g > 0.0).unwrap_or(goal);
            let (current, status) = existing.map_or((0.0, CheckinStatus::Unlabeled), |(v, s, _)| {
                (v, CheckinStatus::from_i64(s))
            });
            if status == CheckinStatus::Done {
                self.reset_checkin_row(habit, date, true)?;
                return Ok(0.0);
            }
            let base = if status == CheckinStatus::Undone { 0.0 } else { current };
            let next = polish_amount(base + step);
            let next_status = if next >= eff_goal { CheckinStatus::Done } else { CheckinStatus::Unlabeled };
            self.conn.execute(
                "INSERT INTO habit_checkins (habit_id, date, value, goal, status) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(habit_id, date) DO UPDATE SET
                     value = excluded.value,
                     goal = COALESCE(habit_checkins.goal, excluded.goal),
                     status = excluded.status",
                params![habit.to_string(), date_str, next, eff_goal, next_status.to_i64()],
            )?;
            self.unskip_habit_date(habit, date)?;
            Ok(next)
        })
    }

    /// Adds `delta` to the day's amount (past the goal too), setting status to `Done` once the
    /// goal is reached. Returns the new amount.
    pub fn add_checkin_amount(&self, habit: Id, date: NaiveDate, delta: f64) -> Result<f64> {
        let delta = polish_amount(delta);
        if delta <= 0.0 {
            return Ok(0.0);
        }
        self.in_tx(|| {
            let date_str = date.format(DATE_FORMAT).to_string();
            let (goal, _) = self.habit_goal_and_mode(habit)?;
            let existing: Option<(f64, i64, Option<f64>)> = self
                .conn
                .query_row(
                    "SELECT value, status, goal FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                    params![habit.to_string(), date_str],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let eff_goal = existing.and_then(|(_, _, g)| g).filter(|g| *g > 0.0).unwrap_or(goal);
            let base = existing
                .filter(|(_, s, _)| CheckinStatus::from_i64(*s) != CheckinStatus::Undone)
                .map_or(0.0, |(v, _, _)| v);
            let next = polish_amount(base + delta);
            let next_status = if next >= eff_goal { CheckinStatus::Done } else { CheckinStatus::Unlabeled };
            self.conn.execute(
                "INSERT INTO habit_checkins (habit_id, date, value, goal, status) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(habit_id, date) DO UPDATE SET
                     value = excluded.value,
                     goal = COALESCE(habit_checkins.goal, excluded.goal),
                     status = excluded.status",
                params![habit.to_string(), date_str, next, eff_goal, next_status.to_i64()],
            )?;
            self.unskip_habit_date(habit, date)?;
            Ok(next)
        })
    }

    /// Sets a habit's exact daily amount; `0` resets the day's check-in (preserving any log).
    pub fn set_checkin_amount(&self, habit: Id, date: NaiveDate, amount: f64) -> Result<()> {
        let amount = polish_amount(amount);
        self.in_tx(|| {
            if amount <= 0.0 {
                self.reset_checkin_row(habit, date, true)?;
                self.unskip_habit_date(habit, date)?;
                return Ok(());
            }
            let date_str = date.format(DATE_FORMAT).to_string();
            let (goal, _) = self.habit_goal_and_mode(habit)?;
            let existing_goal: Option<f64> = self
                .conn
                .query_row(
                    "SELECT goal FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                    params![habit.to_string(), date_str],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            let eff_goal = existing_goal.filter(|g| *g > 0.0).unwrap_or(goal);
            let status = if amount >= eff_goal { CheckinStatus::Done } else { CheckinStatus::Unlabeled };
            self.conn.execute(
                "INSERT INTO habit_checkins (habit_id, date, value, goal, status) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(habit_id, date) DO UPDATE SET
                     value = excluded.value,
                     goal = COALESCE(habit_checkins.goal, excluded.goal),
                     status = excluded.status",
                params![habit.to_string(), date_str, amount, eff_goal, status.to_i64()],
            )?;
            self.unskip_habit_date(habit, date)?;
            Ok(())
        })
    }

    /// Explicitly marks `date` as Unachieved / failed (value = 0, status = Undone), preserving logs.
    pub fn mark_checkin_undone(&self, habit: Id, date: NaiveDate) -> Result<()> {
        self.in_tx(|| {
            let date_str = date.format(DATE_FORMAT).to_string();
            let (goal, _) = self.habit_goal_and_mode(habit)?;
            self.conn.execute(
                "INSERT INTO habit_checkins (habit_id, date, value, goal, status) VALUES (?1, ?2, 0, ?3, 1)
                 ON CONFLICT(habit_id, date) DO UPDATE SET
                     value = 0,
                     goal = COALESCE(habit_checkins.goal, excluded.goal),
                     status = 1",
                params![habit.to_string(), date_str, goal],
            )?;
            self.unskip_habit_date(habit, date)?;
            Ok(())
        })
    }

    /// Resets `date` to unchecked (and un-skips it if skipped). When `keep_log` is true, any
    /// existing log note or mood is preserved.
    pub fn reset_checkin(&self, habit: Id, date: NaiveDate, keep_log: bool) -> Result<()> {
        self.in_tx(|| {
            self.reset_checkin_row(habit, date, keep_log)?;
            self.unskip_habit_date(habit, date)?;
            Ok(())
        })
    }

    /// Creates or updates the Habit Log (note and mood) for `date`, even if no check-in exists yet.
    pub fn set_checkin_log(&self, habit: Id, date: NaiveDate, note: Option<&str>, mood: u8) -> Result<()> {
        let note: Option<String> = note
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| n.chars().take(MAX_LOG_NOTE_CHARS).collect());
        let mood = if mood == 0 || MOODS.contains(&mood) { mood } else { 0 };
        let date_str = date.format(DATE_FORMAT).to_string();
        self.in_tx(|| {
            let existing: Option<(f64, i64)> = self
                .conn
                .query_row(
                    "SELECT value, status FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                    params![habit.to_string(), date_str],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match existing {
                Some((value, status)) => {
                    if note.is_none() && mood == 0 && value <= 0.0 && CheckinStatus::from_i64(status) == CheckinStatus::Unlabeled {
                        self.conn.execute(
                            "DELETE FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                            params![habit.to_string(), date_str],
                        )?;
                    } else {
                        self.conn.execute(
                            "UPDATE habit_checkins SET note = ?3, mood = ?4 WHERE habit_id = ?1 AND date = ?2",
                            params![habit.to_string(), date_str, note, mood as i64],
                        )?;
                    }
                }
                None if note.is_some() || mood != 0 => {
                    let (goal, _) = self.habit_goal_and_mode(habit)?;
                    self.conn.execute(
                        "INSERT INTO habit_checkins (habit_id, date, value, goal, status, note, mood)
                         VALUES (?1, ?2, 0, ?3, 0, ?4, ?5)",
                        params![habit.to_string(), date_str, goal, note, mood as i64],
                    )?;
                }
                None => {}
            }
            Ok(())
        })
    }

    /// Deletes the Habit Log (note and mood) for `date`, keeping any check-in status/value intact.
    pub fn delete_checkin_log(&self, habit: Id, date: NaiveDate) -> Result<()> {
        self.set_checkin_log(habit, date, None, 0)
    }

    /// Writes the note of an existing check-in; an empty note removes it.
    pub fn set_checkin_note(&self, habit: Id, date: NaiveDate, note: &str) -> Result<()> {
        let note = note.trim();
        let trimmed: Option<String> = (!note.is_empty()).then(|| note.chars().take(MAX_LOG_NOTE_CHARS).collect());
        let updated = self.conn.execute(
            "UPDATE habit_checkins SET note = ?3 WHERE habit_id = ?1 AND date = ?2",
            params![habit.to_string(), date.format(DATE_FORMAT).to_string(), trimmed],
        )?;
        anyhow::ensure!(updated == 1, "a note needs a check-in on that day");
        Ok(())
    }

    /// The days a habit was completed (`Done`).
    pub fn checkins(&self, habit: Id) -> Result<std::collections::HashSet<NaiveDate>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT date FROM habit_checkins WHERE habit_id = ?1 AND status = 2",
        )?;
        let dates = stmt
            .query_map([habit.to_string()], |row| row.get::<_, String>(0))?
            .filter_map(|r| NaiveDate::parse_from_str(&r.ok()?, DATE_FORMAT).ok())
            .collect();
        Ok(dates)
    }

    /// A habit's check-in records (including partial, undone, and log-only rows), oldest first.
    pub fn checkin_records(&self, habit: Id) -> Result<Vec<Checkin>> {
        let (default_goal, _) = self.habit_goal_and_mode(habit).unwrap_or((1.0, CheckinMode::All));
        let mut stmt = self.conn.prepare_cached(
            "SELECT date, value, goal, status, note, mood FROM habit_checkins WHERE habit_id = ?1 ORDER BY date",
        )?;
        let records = stmt
            .query_map([habit.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .filter_map(|r| {
                let (date, value, goal, status, note, mood) = r.ok()?;
                Some(Checkin {
                    date: NaiveDate::parse_from_str(&date, DATE_FORMAT).ok()?,
                    value: polish_amount(value),
                    goal: goal.filter(|g| *g > 0.0).unwrap_or(default_goal),
                    status: CheckinStatus::from_i64(status),
                    note,
                    mood: mood.clamp(0, 255) as u8,
                })
            })
            .collect();
        Ok(records)
    }

    /// Loads all check-in records across all habits in one query, grouped by habit id, oldest first.
    pub fn all_habit_checkins(&self) -> Result<HashMap<Id, Vec<Checkin>>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.habit_id, c.date, c.value, COALESCE(c.goal, h.goal_amount, 1.0), c.status, c.note, c.mood
             FROM habit_checkins c
             LEFT JOIN habits h ON h.id = c.habit_id
             ORDER BY c.date",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        let mut out: HashMap<Id, Vec<Checkin>> = HashMap::new();
        for row in rows {
            let (habit_id, date, value, goal, status, note, mood) = row?;
            if let (Ok(id), Ok(date)) = (habit_id.parse(), NaiveDate::parse_from_str(&date, DATE_FORMAT)) {
                out.entry(id).or_default().push(Checkin {
                    date,
                    value: polish_amount(value),
                    goal: polish_amount(goal).max(0.01),
                    status: CheckinStatus::from_i64(status),
                    note,
                    mood: mood.clamp(0, 255) as u8,
                });
            }
        }
        Ok(out)
    }

    /// Loads completed (`Done`) dates for all habits in one query.
    pub fn all_habit_done_dates(&self) -> Result<HashMap<Id, HashSet<NaiveDate>>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT habit_id, date FROM habit_checkins WHERE status = 2",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        let mut out: HashMap<Id, HashSet<NaiveDate>> = HashMap::new();
        for row in rows {
            let (habit_id, date) = row?;
            if let (Ok(id), Ok(date)) = (habit_id.parse(), NaiveDate::parse_from_str(&date, DATE_FORMAT)) {
                out.entry(id).or_default().insert(date);
            }
        }
        Ok(out)
    }

    /// Records a finished focus session.
    pub fn add_focus_session(&self, task_id: Option<Id>, title: &str, started_at: i64, ended_at: i64) -> Result<FocusSession> {
        self.add_focus_session_for(task_id, None, title, started_at, ended_at, false)
    }

    /// A focus session, timed as a Pomodoro or with the stopwatch. If the task
    /// was removed while the timer ran, the session still keeps its title.
    pub fn add_focus_session_as(
        &self,
        task_id: Option<Id>,
        title: &str,
        started_at: i64,
        ended_at: i64,
        stopwatch: bool,
    ) -> Result<FocusSession> {
        self.add_focus_session_for(task_id, None, title, started_at, ended_at, stopwatch)
    }

    /// Records a focus session linked to an optional task or habit.
    pub fn add_focus_session_for(
        &self,
        task_id: Option<Id>,
        habit_id: Option<Id>,
        title: &str,
        started_at: i64,
        ended_at: i64,
        stopwatch: bool,
    ) -> Result<FocusSession> {
        anyhow::ensure!(ended_at >= started_at, "a focus session cannot end before it starts");
        let id = Uuid::now_v7();
        let title = title.trim().to_owned();
        let (task_id, habit_id) = self.conn.query_row(
            "INSERT INTO focus_sessions (id, task_id, habit_id, title, started_at, ended_at, stopwatch)
             VALUES (?1, (SELECT id FROM tasks WHERE id = ?2), (SELECT id FROM habits WHERE id = ?3), ?4, ?5, ?6, ?7)
             RETURNING task_id, habit_id",
            params![
                id.to_string(),
                task_id.map(|t| t.to_string()),
                habit_id.map(|h| h.to_string()),
                title,
                started_at,
                ended_at,
                stopwatch,
            ],
            |row| {
                let t = row.get::<_, Option<String>>(0)?.map(|id| parse_uuid(&id, 0)).transpose()?;
                let h = row.get::<_, Option<String>>(1)?.map(|id| parse_uuid(&id, 1)).transpose()?;
                Ok((t, h))
            },
        )?;
        Ok(FocusSession { id, task_id, habit_id, title, started_at, ended_at, stopwatch, note: String::new() })
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

    /// The note of a focus record ("What do you have in mind?").
    pub fn set_focus_note(&self, id: Id, note: &str) -> Result<()> {
        self.conn.execute("UPDATE focus_sessions SET note = ?2 WHERE id = ?1", params![id.to_string(), note.trim()])?;
        Ok(())
    }

    pub fn delete_focus_session(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM focus_sessions WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    /// The Pomodoro list's timers ("Add Timer"), oldest first; active ones or archived ones.
    pub fn focus_timers(&self, archived: bool) -> Result<Vec<crate::focus::FocusTimer>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, icon, minutes, stopwatch, archived_at, color FROM focus_timers
             WHERE (archived_at IS NOT NULL) = ?1 ORDER BY created_at, rowid",
        )?;
        let rows = stmt
            .query_map([archived], |row| {
                Ok(crate::focus::FocusTimer {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    icon: row.get(2)?,
                    minutes: row.get(3)?,
                    stopwatch: row.get(4)?,
                    archived: row.get::<_, Option<i64>>(5)?.is_some(),
                    color: row.get(6)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Adds or edits a timer: its name, icon and color, Pomo length in minutes or stopwatch.
    pub fn save_focus_timer(&self, timer: &crate::focus::FocusTimer) -> Result<()> {
        let name = timer.name.trim();
        anyhow::ensure!(!name.is_empty(), "a timer needs a name");
        self.conn.execute(
            "INSERT INTO focus_timers (id, name, icon, minutes, stopwatch, created_at, color) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, icon = excluded.icon, color = excluded.color,
                 minutes = excluded.minutes, stopwatch = excluded.stopwatch",
            params![timer.id.to_string(), name, timer.icon, timer.minutes.max(1), timer.stopwatch, now_ms(), timer.color],
        )?;
        Ok(())
    }

    pub fn set_focus_timer_archived(&self, id: Id, archived: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE focus_timers SET archived_at = ?2 WHERE id = ?1",
            params![id.to_string(), archived.then(now_ms)],
        )?;
        Ok(())
    }

    pub fn delete_focus_timer(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM focus_timers WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    /// The Focus Record's "Batch Management" Delete: these records go.
    pub fn delete_focus_sessions(&self, ids: &[Id]) -> Result<()> {
        self.in_tx(|| {
            let mut stmt = self.conn.prepare_cached("DELETE FROM focus_sessions WHERE id = ?1")?;
            for id in ids {
                stmt.execute([id.to_string()])?;
            }
            Ok(())
        })
    }

    /// The Focus Record's "Delete All".
    pub fn delete_all_focus_sessions(&self) -> Result<()> {
        self.conn.execute("DELETE FROM focus_sessions", [])?;
        Ok(())
    }

    /// Focus sessions, newest first.
    pub fn focus_sessions(&self) -> Result<Vec<FocusSession>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, task_id, habit_id, title, started_at, ended_at, stopwatch, note FROM focus_sessions ORDER BY started_at DESC",
        )?;
        let sessions = stmt
            .query_map([], |row| {
                Ok(FocusSession {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    task_id: row.get::<_, Option<String>>(1)?.map(|t| parse_uuid(&t, 1)).transpose()?,
                    habit_id: row.get::<_, Option<String>>(2)?.map(|h| parse_uuid(&h, 2)).transpose()?,
                    title: row.get(3)?,
                    started_at: row.get(4)?,
                    ended_at: row.get(5)?,
                    stopwatch: row.get(6)?,
                    note: row.get(7)?,
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

    /// Completes or reopens all given tasks inside one transaction.
    pub fn batch_complete(&self, ids: &[Id], completed: bool) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_completed(id, completed)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_wont_do(&self, ids: &[Id], wont_do: bool) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_wont_do(id, wont_do)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_in_progress(&self, ids: &[Id], in_progress: bool) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_in_progress(id, in_progress)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_due(&self, ids: &[Id], date: Option<NaiveDate>, time: Option<NaiveTime>) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_due(id, date, time)?;
            }
            Ok(())
        })
    }

    pub fn batch_move_dates(&self, ids: &[Id], date: NaiveDate) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.move_dates(id, date)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_priority(&self, ids: &[Id], priority: Priority) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_priority(id, priority)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_list(&self, ids: &[Id], list_id: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_list(id, list_id)?;
            }
            Ok(())
        })
    }

    pub fn batch_add_tag(&self, ids: &[Id], tag: &str) -> Result<()> {
        let tag = Self::sanitize_tag_name(tag);
        if tag.is_empty() {
            return Ok(());
        }
        let tag = tag.as_str();
        self.in_tx(|| {
            for &id in ids {
                let Some(task) = self.task(id)? else { continue };
                if !task.tags.iter().any(|existing| existing.to_lowercase() == tag.to_lowercase()) {
                    let mut next = task.tags;
                    next.push(tag.to_owned());
                    self.replace_task_tags(id, &next)?;
                }
            }
            Ok(())
        })
    }

    pub fn batch_remove_tag(&self, ids: &[Id], tag: &str) -> Result<()> {
        let tag = Self::sanitize_tag_name(tag);
        if tag.is_empty() {
            return Ok(());
        }
        let tag = tag.as_str();
        self.in_tx(|| {
            for &id in ids {
                let Some(task) = self.task(id)? else { continue };
                let filtered: Vec<String> = task
                    .tags
                    .into_iter()
                    .filter(|existing| existing.to_lowercase() != tag.to_lowercase())
                    .collect();
                self.replace_task_tags(id, &filtered)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_pinned(&self, ids: &[Id], pinned: bool) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_pinned(id, pinned)?;
            }
            Ok(())
        })
    }

    pub fn batch_delete(&self, ids: &[Id]) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.delete_task(id)?;
            }
            Ok(())
        })
    }

    pub fn batch_restore(&self, ids: &[Id]) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.restore_task(id)?;
            }
            Ok(())
        })
    }

    pub fn batch_purge(&self, ids: &[Id]) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.purge_task(id)?;
            }
            Ok(())
        })
    }

    pub fn batch_duplicate(&self, ids: &[Id]) -> Result<Vec<Task>> {
        self.in_tx(|| {
            let mut created = Vec::with_capacity(ids.len());
            for &id in ids {
                if self.task(id)?.is_some() {
                    created.push(self.duplicate_task(id)?);
                }
            }
            Ok(created)
        })
    }

    pub fn batch_set_kind(&self, ids: &[Id], kind: Kind) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_kind(id, kind)?;
            }
            Ok(())
        })
    }

    /// Merges multiple selected tasks into the first task: subsequent tasks (and their checklist
    /// items) become checklist items on the first task, their tags and notes are merged in, and
    /// the absorbed tasks are moved to the Trash.
    pub fn batch_merge_tasks(&self, ids: &[Id]) -> Result<Option<Id>> {
        if ids.len() < 2 {
            return Ok(None);
        }
        self.in_tx(|| {
            let Some(target) = self.task(ids[0])? else {
                return Ok(None);
            };
            let mut tags = target.tags.clone();
            let mut tags_changed = false;
            let mut notes = target.notes.trim_end().to_owned();
            let mut notes_changed = false;

            for &other_id in &ids[1..] {
                if other_id == target.id {
                    continue;
                }
                let Some(other) = self.task(other_id)? else {
                    continue;
                };
                if !other.title.trim().is_empty() {
                    let added = self.add_checklist_item(target.id, other.title.trim(), None)?;
                    if other.is_completed() {
                        self.set_checklist_item_completed(added.id, true)?;
                    }
                }
                for item in self.checklist(other.id)? {
                    let copied = self.add_checklist_item(target.id, &item.title, None)?;
                    if item.completed {
                        self.set_checklist_item_completed(copied.id, true)?;
                    }
                }
                for tag in other.tags {
                    if !tags.iter().any(|existing| existing.to_lowercase() == tag.to_lowercase()) {
                        tags.push(tag);
                        tags_changed = true;
                    }
                }
                let other_notes = other.notes.trim();
                if !other_notes.is_empty() {
                    if !notes.is_empty() {
                        notes.push_str("\n\n");
                    }
                    notes.push_str(other_notes);
                    notes_changed = true;
                }
                self.delete_task(other.id)?;
            }
            if tags_changed {
                self.replace_task_tags(target.id, &tags)?;
            }
            if notes_changed {
                self.set_notes(target.id, &notes)?;
            }
            Ok(Some(target.id))
        })
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
        // Filters are matched in `Store::tasks_in`, which knows their conditions.
        View::Filter(_) | View::Summary => false,
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
    })
}

fn rebuild_caldav_feed(tx: &Connection, subscription_id: Id) -> Result<()> {
    let mut stmt = tx.prepare(
        "SELECT ics FROM calendar_events WHERE subscription_id = ?1 AND dirty != 2 ORDER BY uid",
    )?;
    let entries: Vec<String> = stmt.query_map(params![subscription_id.to_string()], |r| r.get(0))?.collect::<Result<_, _>>()?;
    drop(stmt);

    let mut combined = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Fern//Calendar//EN\r\n");
    for ics in &entries {
        let normalized = ics.replace("\r\n", "\n");
        let mut inside_component = 0usize;
        for line in normalized.lines() {
            let upper = line.to_ascii_uppercase();
            if upper == "BEGIN:VCALENDAR" || upper == "END:VCALENDAR" {
                continue;
            }
            if upper.starts_with("BEGIN:") {
                inside_component += 1;
            }
            if inside_component > 0 {
                combined.push_str(line);
                combined.push_str("\r\n");
            }
            if upper.starts_with("END:") {
                inside_component = inside_component.saturating_sub(1);
            }
        }
    }
    combined.push_str("END:VCALENDAR\r\n");

    tx.execute(
        "UPDATE calendar_subscriptions SET feed = ?2 WHERE id = ?1",
        params![subscription_id.to_string(), combined],
    )?;
    Ok(())
}

pub(crate) fn now_ms() -> i64 {
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
        let store = Store::init(conn, None)?;
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
        let with_completed = store.tasks_in_from_with_today_completed(store.tasks()?, &View::Today, today, true)?;
        assert!(with_completed.iter().any(|task| task.id == done.id));
        assert_eq!(with_completed.len(), 3);
        assert_eq!(titles(&store.tasks_in(&View::Tomorrow, today)?), ["tomorrow"]);
        let done_tomorrow = store.add_task("done tomorrow")?;
        store.set_due(done_tomorrow.id, Some(today + Duration::days(1)), None)?;
        store.complete(done_tomorrow.id, true)?;
        assert_eq!(titles(&store.tasks_in(&View::Tomorrow, today)?), ["tomorrow"]);
        let tomorrow_with_completed = store.tasks_in_from_with_today_completed(
            store.tasks()?,
            &View::Tomorrow,
            today,
            true,
        )?;
        assert!(tomorrow_with_completed.iter().any(|task| task.id == done_tomorrow.id));
        let next7_with_completed = store.tasks_in_from_with_today_completed(
            store.tasks()?,
            &View::Next7Days,
            today,
            true,
        )?;
        assert!(next7_with_completed.iter().any(|task| task.id == done_tomorrow.id));
        assert!(next7_with_completed.iter().any(|task| task.id == done.id));
        store.purge_task(done_tomorrow.id)?;
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
    fn templates() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("weekly review")?;
        store.set_notes(task.id, "Look back, plan ahead")?;
        store.set_priority(task.id, Priority::Medium)?;
        store.set_tags(task.id, &["work".to_owned()])?;
        store.add_checklist_item(task.id, "inbox zero", None)?;
        store.add_checklist_item(task.id, "plan the week", None)?;
        let template = store.save_template(task.id)?;
        assert_eq!(store.templates()?, std::slice::from_ref(&template));

        let copy = store.add_from_template(template.id)?;
        assert_ne!(copy.id, task.id);
        assert_eq!((copy.title.as_str(), copy.notes.as_str(), copy.priority), ("weekly review", "Look back, plan ahead", Priority::Medium));
        assert_eq!(copy.tags, ["work"]);
        let items: Vec<String> = store.checklist(copy.id)?.into_iter().map(|i| i.title).collect();
        assert_eq!(items, ["inbox zero", "plan the week"]);

        store.delete_template(template.id)?;
        assert!(store.templates()?.is_empty());
        Ok(())
    }

    #[test]
    fn comments_on_a_task() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("Fix the sink")?;
        assert!(store.add_comment(task.id, "  ").is_err());
        let first = store.add_comment(task.id, "Bought a new washer")?;
        store.add_comment(task.id, "Done, no leak")?;
        let texts = |store: &Store| store.comments(task.id).unwrap().into_iter().map(|c| c.text).collect::<Vec<_>>();
        assert_eq!(texts(&store), ["Bought a new washer", "Done, no leak"]);
        store.delete_comment(first.id)?;
        assert_eq!(texts(&store), ["Done, no leak"]);
        Ok(())
    }

    #[test]
    fn skipping_the_recurrence() -> Result<()> {
        use crate::recurrence::{Frequency, Recurrence};
        let store = Store::open_in_memory()?;
        let d = |m, day| NaiveDate::from_ymd_opt(2026, m, day).unwrap();
        let meter = store.add_task("Read the meter")?;
        store.set_due(meter.id, Some(d(10, 24)), NaiveTime::from_hms_opt(9, 0, 0))?;
        store.set_repeat(meter.id, Some(Recurrence::new(Frequency::Weekly, 1)), d(10, 18))?;
        store.skip_recurrence(meter.id)?;
        let task = store.task(meter.id)?.unwrap();
        assert_eq!((task.due_date, task.due_time, task.is_completed()), (Some(d(10, 31)), NaiveTime::from_hms_opt(9, 0, 0), false));
        let once = store.add_task("Once")?;
        store.set_due(once.id, Some(d(10, 24)), None)?;
        assert!(store.skip_recurrence(once.id).is_err(), "only repeating tasks skip");
        Ok(())
    }

    #[test]
    fn sub_tags_merge_rename_and_delete() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let tagged = |title: &str, tags: &[&str]| -> Result<Task> {
            let task = store.add_task(title)?;
            store.set_tags(task.id, &tags.iter().map(|t| t.to_string()).collect::<Vec<_>>())?;
            Ok(task)
        };
        tagged("Run", &["health"])?;
        tagged("Dentist", &["teeth"])?;
        tagged("Gym", &["sport"])?;
        store.add_tag("#home", Some("#3B82F6"), None)?;
        assert!(store.add_tag("Home", None, None).is_err(), "names are unique whatever the case");

        // A sub-tag's tasks show in its parent's view; one level deep only.
        store.update_tag("teeth", "teeth", None, Some("health"))?;
        let titles = |view: View| {
            let mut titles = store.tasks_in(&view, today).unwrap().into_iter().map(|t| t.title).collect::<Vec<_>>();
            titles.sort();
            titles
        };
        assert_eq!(titles(View::Tag("Health".into())), ["Dentist", "Run"]);
        assert_eq!(titles(View::Tag("teeth".into())), ["Dentist"]);
        assert!(store.update_tag("sport", "sport", None, Some("teeth")).is_err(), "teeth is a sub-tag");
        assert!(store.update_tag("health", "health", None, Some("sport")).is_err(), "health has sub-tags");

        // Renaming keeps the tasks, the sub-tags and the color.
        store.update_tag("health", "wellbeing", Some("#22C55E"), None)?;
        let tags = store.tags()?;
        assert!(!tags.iter().any(|t| t.name == "health"));
        assert_eq!(tags.iter().find(|t| t.name == "teeth").unwrap().parent.as_deref(), Some("wellbeing"));
        assert_eq!(titles(View::Tag("wellbeing".into())), ["Dentist", "Run"]);

        // Merging moves the tasks over and deletes the tag.
        store.merge_tag("sport", "wellbeing")?;
        assert!(!store.tags()?.iter().any(|t| t.name == "sport"));
        assert_eq!(titles(View::Tag("wellbeing".into())), ["Dentist", "Gym", "Run"]);

        store.set_tag_pinned("home", true)?;
        assert!(store.tags()?.iter().find(|t| t.name == "home").unwrap().pinned);

        // Deleting a parent leaves its sub-tags at the top and its tasks untagged.
        store.delete_tag("wellbeing")?;
        assert_eq!(store.tags()?.iter().find(|t| t.name == "teeth").unwrap().parent, None);
        assert!(store.tasks()?.iter().find(|t| t.title == "Run").unwrap().tags.is_empty());
        Ok(())
    }

    #[test]
    fn list_settings_folders_and_archive() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let home = store.add_list_folder("Home")?;
        let draft = ListDraft {
            name: " Groceries ".into(),
            icon: Some("🛒".into()),
            color: Some("#ff6b6b".into()),
            folder: Some(home.id),
            kind: Kind::Note,
            in_smart_lists: true,
            ..Default::default()
        };
        let groceries = store.add_list(&draft)?;
        assert_eq!((groceries.name.as_str(), groceries.icon.as_deref(), groceries.folder, groceries.kind), ("Groceries", Some("🛒"), Some(home.id), Kind::Note));
        let work = store.create_list("Work")?;
        let milk = store.add_task("Milk")?;
        store.set_list(milk.id, Some(groceries.id))?;
        store.set_due(milk.id, Some(today), None)?;
        let report = store.add_task("Report")?;
        store.set_list(report.id, Some(work.id))?;
        store.set_due(report.id, Some(today), None)?;

        // "Show in Smart List: Do not show" keeps the list's tasks out of Today and All.
        store.update_list(groceries.id, &ListDraft { in_smart_lists: false, ..ListDraft::of(&groceries) })?;
        assert_eq!(store.tasks_in(&View::Today, today)?.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Report"]);
        assert_eq!(store.counts(today)?.all, 1);
        assert_eq!(store.tasks_in(&View::List(groceries.id), today)?.len(), 1, "the list itself still shows them");
        // Archived lists leave the smart lists too, and their tasks become read-only.
        assert!(!store.is_list_archived(work.id)?);
        assert!(!store.is_task_read_only(report.id)?);
        store.set_list_archived(work.id, true)?;
        assert!(store.tasks_in(&View::Today, today)?.is_empty());
        assert!(store.lists()?.iter().find(|l| l.id == work.id).unwrap().archived);
        assert!(store.is_list_archived(work.id)?);
        assert_eq!(store.list_name_and_archived(work.id)?, Some(("Work".to_owned(), true)));
        assert!(store.is_task_read_only(report.id)?);
        store.set_list_archived(work.id, false)?;
        assert!(!store.is_task_read_only(report.id)?);
        // Trashed tasks are read-only as well.
        store.delete_task(report.id)?;
        assert!(store.is_task_read_only(report.id)?);
        store.restore_task(report.id)?;
        assert!(!store.is_task_read_only(report.id)?);

        // Order: a list placed first comes first; Duplicate goes right after its list.
        store.place_list(work.id, None, None)?;
        let names = |store: &Store| store.lists().unwrap().into_iter().map(|l| l.name).collect::<Vec<_>>();
        assert_eq!(names(&store), ["Work", "Groceries"]);
        let copy = store.duplicate_list(work.id, " copy")?;
        assert_eq!(names(&store), ["Work", "Work copy", "Groceries"]);
        assert_eq!(store.tasks_in(&View::List(copy.id), today)?.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Report"]);

        // Ungroup keeps the lists and drops the folder.
        store.ungroup_list_folder(home.id)?;
        assert!(store.list_folders()?.is_empty());
        assert_eq!(store.lists()?.iter().find(|l| l.id == groceries.id).unwrap().folder, None);
        Ok(())
    }

    #[test]
    fn sections_left_right_and_moved() -> Result<()> {
        let store = Store::open_in_memory()?;
        let work = store.create_list("Work")?;
        let home = store.create_list("Home")?;
        let doing = store.add_list_section(Some(work.id), "Doing")?;
        store.insert_list_section(Some(work.id), "To do", Some(0))?;
        store.insert_list_section(Some(work.id), "Done", None)?;
        let names = |list| store.list_sections(Some(list)).unwrap().into_iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names(work.id), ["To do", "Doing", "Done"]);
        let task = store.add_task("Paint")?;
        store.set_list(task.id, Some(work.id))?;
        store.set_section(task.id, Some(doing.id))?;
        store.move_list_section(doing.id, Some(home.id))?;
        assert_eq!(names(work.id), ["To do", "Done"]);
        assert_eq!(names(home.id), ["Doing"]);
        let moved = store.task(task.id)?.unwrap();
        assert_eq!((moved.list_id, moved.section_id), (Some(home.id), Some(doing.id)));
        // On to the Inbox, tasks and all.
        store.move_list_section(doing.id, None)?;
        assert_eq!(store.list_sections(None)?.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Doing"]);
        let moved = store.task(task.id)?.unwrap();
        assert_eq!((moved.list_id, moved.section_id), (None, Some(doing.id)));
        Ok(())
    }

    #[test]
    fn inbox_sections_migration_keeps_tasks_in_their_sections() -> Result<()> {
        // A database just before the inbox sections' migration, with a task in a list's section.
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let before = MIGRATIONS.iter().position(|sql| sql.contains("PRAGMA foreign_keys = OFF")).unwrap();
        for sql in &MIGRATIONS[..before] {
            conn.execute_batch(sql)?;
        }
        conn.pragma_update(None, "user_version", before)?;
        conn.execute_batch(
            "INSERT INTO lists (id, name, created_at) VALUES ('l', 'Home', 0);
             INSERT INTO list_sections (id, list_id, name, created_at) VALUES ('s', 'l', 'Kitchen', 0);",
        )?;
        let store = Store { conn, attachments_dir: None };
        let task = store.add_task("Dishes")?;
        store.conn.execute("UPDATE tasks SET list_id = 'l', section_id = 's' WHERE id = ?1", [task.id.to_string()])?;
        Store::migrate(&store.conn)?;
        let section: Option<String> = store.conn.query_row("SELECT section_id FROM tasks WHERE id = ?1", [task.id.to_string()], |r| r.get(0))?;
        assert_eq!(section.as_deref(), Some("s"), "the task keeps its section");
        let fk: bool = store.conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
        assert!(fk, "the foreign keys are on again");
        Ok(())
    }

    #[test]
    fn inbox_sections() -> Result<()> {
        let store = Store::open_in_memory()?;
        let tasks = store.add_list_section(None, "Tasks")?;
        let notes = store.add_list_section(None, "Notes")?;
        let milk = store.add_task("Milk")?;
        store.set_section(milk.id, Some(tasks.id))?;
        assert_eq!(store.list_sections(None)?, [tasks.clone(), notes]);
        assert_eq!(store.task(milk.id)?.unwrap().section_id, Some(tasks.id));
        let work = store.create_list("Work")?;
        assert!(store.list_sections(Some(work.id))?.is_empty(), "the Inbox's sections are its own");
        Ok(())
    }

    #[test]
    fn list_sections() -> Result<()> {
        let store = Store::open_in_memory()?;
        let home = store.create_list("Home")?;
        let work = store.create_list("Work")?;
        let kitchen = store.add_list_section(Some(home.id), " Kitchen ")?;
        assert!(store.add_list_section(Some(home.id), " ").is_err());
        assert_eq!(store.list_sections(Some(home.id))?, std::slice::from_ref(&kitchen));
        assert!(store.list_sections(Some(work.id))?.is_empty());

        let task = store.add_task("fix the tap")?;
        assert!(store.set_section(task.id, Some(kitchen.id)).is_err(), "the task is not in that list");
        store.set_list(task.id, Some(home.id))?;
        store.set_section(task.id, Some(kitchen.id))?;
        assert_eq!(store.task(task.id)?.unwrap().section_id, Some(kitchen.id));
        // Moving to another list leaves the section.
        store.set_list(task.id, Some(work.id))?;
        assert_eq!(store.task(task.id)?.unwrap().section_id, None);

        store.rename_list_section(kitchen.id, " Kitchen & pantry ")?;
        assert!(store.rename_list_section(kitchen.id, "").is_err());
        assert_eq!(store.list_sections(Some(home.id))?[0].name, "Kitchen & pantry");
        store.set_list(task.id, Some(home.id))?;
        store.set_section(task.id, Some(kitchen.id))?;
        store.delete_list_section(kitchen.id)?;
        assert_eq!(store.task(task.id)?.unwrap().section_id, None);
        assert!(store.list_sections(Some(home.id))?.is_empty());
        Ok(())
    }

    #[test]
    fn completing_a_repeating_task_can_be_undone() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("water the plants")?;
        store.set_due(task.id, Some(d(2026, 10, 18)), None)?;
        store.set_repeat(task.id, Some(crate::Recurrence::new(crate::Frequency::Daily, 1)), d(2026, 10, 18))?;
        let copy = store.complete(task.id, true)?.expect("a repeating task leaves a completed copy");
        assert_eq!(store.task(task.id)?.unwrap().due_date, Some(d(2026, 10, 19)));
        store.undo_repeat(task.id, copy, Some(d(2026, 10, 18)), None, None)?;
        assert_eq!(store.task(task.id)?.unwrap().due_date, Some(d(2026, 10, 18)));
        assert!(store.task(copy)?.is_none());
        assert_eq!(store.complete(store.add_task("once")?.id, true)?, None);
        Ok(())
    }

    #[test]
    fn pinning() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("pay rent")?;
        store.set_pinned(task.id, true)?;
        assert!(store.task(task.id)?.unwrap().pinned_at.is_some());
        store.set_pinned(task.id, false)?;
        assert_eq!(store.task(task.id)?.unwrap().pinned_at, None);
        Ok(())
    }

    #[test]
    fn task_activity_is_recorded() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("water the plants")?;
        store.set_title(task.id, "water all the plants")?;
        store.set_due(task.id, Some(d(2026, 10, 20)), None)?;
        store.set_priority(task.id, crate::Priority::High)?;
        store.set_completed(task.id, true)?;
        store.set_completed(task.id, false)?;
        store.delete_task(task.id)?;
        store.restore_task(task.id)?;
        let kinds: Vec<String> = store.task_activity(task.id)?.into_iter().map(|a| a.kind).collect();
        assert_eq!(kinds, ["created", "renamed", "date", "priority", "completed", "reopened", "deleted", "restored"]);

        // The list's log: newest first, only its own tasks.
        let home = store.create_list("Home")?;
        let other = store.add_task("other")?;
        store.set_list(other.id, Some(home.id))?;
        let inbox = store.list_activity(None, 3)?;
        assert_eq!(inbox.len(), 3);
        assert_eq!((inbox[0].0.kind.as_str(), inbox[0].2.as_str()), ("restored", "water all the plants"));
        let in_home: Vec<String> = store.list_activity(Some(home.id), 10)?.into_iter().map(|(a, _, _)| a.kind).collect();
        assert_eq!(in_home, ["moved", "created"]);
        Ok(())
    }

    #[test]
    fn edits_update_the_modified_time() -> Result<()> {
        let store = Store::open_in_memory()?;
        let a = store.add_task("a")?;
        let b = store.add_task("b")?;
        assert_eq!(store.task(a.id)?.unwrap().modified_at, None);
        store.set_title(a.id, "a, renamed")?;
        let modified = store.task(a.id)?.unwrap().modified_at;
        assert!(modified.is_some_and(|m| m >= a.created_at - 1000));
        // Reordering is not an edit.
        store.move_task(b.id, a.id, false)?;
        assert_eq!(store.task(b.id)?.unwrap().modified_at, None);
        store.set_tags(b.id, &["home".to_owned()])?;
        assert!(store.task(b.id)?.unwrap().modified_at.is_some());
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
        let goal = Goal::new(8.0, "glasses");
        store.update_habit(water.id, "Drink water", Some(&goal))?;
        assert!(store.update_habit(water.id, "Drink water", Some(&Goal::new(0.0, ""))).is_err());
        let habit = store.habits()?.remove(0);
        assert_eq!((habit.name.as_str(), habit.goal), ("Drink water", Some(goal)));

        store.toggle_checkin(water.id, d(2026, 10, 18))?;
        assert!(store.set_checkin_note(water.id, d(2026, 10, 17), "no check-in").is_err());
        store.set_checkin_note(water.id, d(2026, 10, 18), " Felt great ")?;
        let records = store.checkin_records(water.id)?;
        assert_eq!(
            records,
            [Checkin {
                date: d(2026, 10, 18),
                value: 8.0,
                goal: 8.0,
                status: CheckinStatus::Done,
                note: Some("Felt great".into()),
                mood: 0,
            }]
        );

        // Partial amounts count towards the goal but do not complete the day.
        let day = d(2026, 10, 17);
        assert_eq!(store.step_checkin(water.id, day)?, 1.0);
        assert_eq!(store.step_checkin(water.id, day)?, 2.0);
        assert_eq!(store.checkins(water.id)?, [d(2026, 10, 18)].into());
        for _ in 2..8 {
            store.step_checkin(water.id, day)?;
        }
        assert_eq!(store.checkins(water.id)?, [d(2026, 10, 17), d(2026, 10, 18)].into());
        assert_eq!(store.step_checkin(water.id, day)?, 0.0, "a completed day starts over");

        store.update_habit(water.id, "Drink water", None)?;
        assert_eq!(store.habits()?[0].goal, None);

        let t = |h| NaiveTime::from_hms_opt(h, 0, 0).unwrap();
        store.set_habit_reminders(water.id, &[t(21), t(9), t(21)])?;
        assert_eq!(store.habits()?[0].reminders, [t(9), t(21)]);
        store.set_habit_reminders(water.id, &[])?;
        assert!(store.habits()?[0].reminders.is_empty());

        let weekly = crate::habit::Frequency::Weekly { days: 3 };
        assert_eq!(store.habits()?[0].frequency, Default::default());
        store.set_habit_frequency(water.id, weekly)?;
        assert_eq!(store.habits()?[0].frequency, weekly);

        store.set_habit_look(water.id, Some(" 💧 "), Some("#06B6D4"))?;
        let habit = store.habits()?.remove(0);
        assert_eq!((habit.icon.as_deref(), habit.color.as_deref()), (Some("💧"), Some("#06B6D4")));
        store.set_habit_look(water.id, Some("Water"), None)?;
        assert_eq!(store.habits()?[0].icon.as_deref(), Some("Wa"));
        store.set_habit_look(water.id, Some(""), None)?;
        assert_eq!(store.habits()?[0].icon, None);

        store.set_habit_span(water.id, Some(d(2026, 10, 1)), Some(21))?;
        let habit = store.habits()?.remove(0);
        assert_eq!((habit.start_date, habit.goal_days), (Some(d(2026, 10, 1)), Some(21)));
        store.set_habit_section(water.id, crate::habit::HabitSection::Morning)?;
        store.set_habit_archived(water.id, true)?;
        let habit = store.habits()?.remove(0);
        assert_eq!((habit.section, habit.archived), (crate::habit::HabitSection::Morning, true));
        assert!(!habit.icon_checkins);
        store.set_habit_icon_checkins(water.id, true)?;
        assert!(store.habits()?[0].icon_checkins);
        assert!(!habit.constant_reminder);
        store.set_habit_constant_reminder(water.id, true)?;
        assert!(store.habits()?[0].constant_reminder);
        store.set_habit_archived(water.id, false)?;
        assert!(!store.habits()?[0].archived);

        assert!(store.add_habit_section(" ").is_err());
        let pets = store.add_habit_section(" Pets ")?;
        assert_eq!(store.habit_sections()?, std::slice::from_ref(&pets));
        store.set_habit_section(water.id, crate::habit::HabitSection::Custom(pets.id))?;
        assert_eq!(store.habits()?[0].section, crate::habit::HabitSection::Custom(pets.id));
        store.rename_habit_section(pets.id, " Animals ")?;
        assert_eq!(store.habit_sections()?[0].name, "Animals");
        assert!(store.rename_habit_section(pets.id, "").is_err());
        store.delete_habit_section(pets.id)?;
        assert!(store.habit_sections()?.is_empty());
        assert_eq!(store.habits()?[0].section, crate::habit::HabitSection::Others);
        store.set_habit_span(water.id, None, Some(0))?;
        let habit = store.habits()?.remove(0);
        assert_eq!((habit.start_date, habit.goal_days), (None, None));
        Ok(())
    }

    #[test]
    fn focus_sessions_are_stored_newest_first() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("write report")?;
        store.add_focus_session(Some(task.id), "write report", 1_000, 1_500_000)?;
        store.add_focus_session(None, "reading", 2_000_000, 3_500_000)?;
        assert!(store.add_focus_session(None, "x", 10, 5).is_err());
        store.add_focus_session_as(None, "timed", 4_000_000, 4_100_000, true)?;
        assert!(store.focus_sessions()?[0].stopwatch);
        store.set_focus_note(store.focus_sessions()?[0].id, " went well ")?;
        assert_eq!(store.focus_sessions()?[0].note, "went well");
        store.delete_focus_session(store.focus_sessions()?[0].id)?;
        let sessions = store.focus_sessions()?;
        assert_eq!(sessions.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["reading", "write report"]);
        assert_eq!(sessions[1].task_id, Some(task.id));
        // Deleting the task keeps the record.
        store.purge_task(task.id)?;
        assert_eq!(store.focus_sessions()?[1].task_id, None);
        // A timer can also finish after its task has already been deleted.
        let finished = store.add_focus_session_as(Some(task.id), "write report", 5_000_000, 5_300_000, true)?;
        assert_eq!(finished.task_id, None);
        assert_eq!(store.focus_sessions()?[0], finished);
        let all = store.focus_sessions()?;
        store.delete_focus_session(all[0].id)?;
        assert_eq!(store.focus_sessions()?.len(), all.len() - 1);
        store.delete_all_focus_sessions()?;
        assert!(store.focus_sessions()?.is_empty());
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
    fn day_view_lists_the_days_tasks() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = chrono::Local::now().date_naive();
        let day = today + Duration::days(3);
        let due = store.add_task("due that day")?;
        store.set_due(due.id, Some(day), None)?;
        let done = store.add_task("done that day")?;
        store.set_due(done.id, Some(day), None)?;
        store.set_completed(done.id, true)?;
        store.add_task("undated")?;
        let titles: Vec<String> = store.tasks_in(&View::Day(day), today)?.into_iter().map(|t| t.title).collect();
        assert_eq!(titles.len(), 2);
        assert!(titles.contains(&"due that day".to_owned()) && titles.contains(&"done that day".to_owned()));
        Ok(())
    }

    #[test]
    fn typed_check_in_amounts() -> Result<()> {
        let store = Store::open_in_memory()?;
        let water = store.add_habit("Water")?;
        store.update_habit(water.id, "Water", Some(&Goal::new(8.0, "glasses")))?;
        let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        store.set_checkin_amount(water.id, day, 5.0)?;
        assert_eq!(store.step_checkin(water.id, day)?, 6.0);
        store.set_checkin_amount(water.id, day, 0.0)?;
        assert_eq!(store.step_checkin(water.id, day)?, 1.0);
        Ok(())
    }

    #[test]
    fn task_draft_commits_duration_section_checklist_and_files_together() -> Result<()> {
        let store = Store::open_in_memory()?;
        let list = store.create_list("Work")?;
        let section = store.insert_list_section(Some(list.id), "Next", None)?;
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("brief.txt");
        std::fs::write(&source, "Task brief")?;
        let attachments_dir = dir.path().join("attachments");
        let mut prepared = PreparedAttachment::copy(Uuid::now_v7(), &source, &attachments_dir)?;
        assert_eq!(prepared.path().extension().unwrap(), "txt");
        let destination = prepared.metadata().path(&attachments_dir);
        let fields = TaskDraft {
            title: "Plan".into(), list_id: Some(list.id), section_id: Some(section.id),
            due_date: NaiveDate::from_ymd_opt(2028, 7, 6), due_time: NaiveTime::from_hms_opt(9, 0, 0),
            end_date: NaiveDate::from_ymd_opt(2028, 7, 7), end_time: NaiveTime::from_hms_opt(10, 0, 0),
            tags: vec!["work".into()], reminders: vec![15],
            checklist: vec![("First".into(), true), ("Second".into(), false)],
            ..Default::default()
        };
        store.conn.execute_batch("CREATE TEMP TRIGGER reject_attachment BEFORE INSERT ON task_attachments BEGIN SELECT RAISE(ABORT, 'file unavailable'); END;")?;
        assert!(store.create_task_with_attachments(fields.clone(), true, std::slice::from_mut(&mut prepared), &attachments_dir).is_err());
        assert!(store.tasks()?.is_empty());
        assert!(!destination.exists());
        assert!(prepared.path().exists(), "a failed save keeps the staged copy for retry");
        store.conn.execute_batch("DROP TRIGGER reject_attachment")?;
        let task = store.create_task_with_attachments(fields.clone(), true, std::slice::from_mut(&mut prepared), &attachments_dir)?;
        assert_eq!(task.section_id, Some(section.id));
        assert_eq!(task.end_date, fields.end_date);
        assert_eq!(task.end_time, fields.end_time);
        assert_eq!(task.checklist, Some((1, 2)));
        assert_eq!(task.tags, ["work"]);
        assert_eq!(task.reminders, [15]);
        assert_eq!(store.attachments(task.id)?.len(), 1);
        assert_eq!(std::fs::read_to_string(destination)?, "Task brief");
        assert!(store.create_task(TaskDraft { section_id: Some(section.id), ..Default::default() }, true).is_err());
        assert_eq!(store.tasks()?.len(), 1, "an invalid section cannot leave an extra task");
        Ok(())
    }

    #[test]
    fn task_draft_rolls_back_files_published_before_a_collision() -> Result<()> {
        let store = Store::open_in_memory()?;
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("plan.txt");
        let files = dir.path().join("attachments");
        std::fs::write(&source, "plan")?;
        let provisional_id = Uuid::now_v7();
        let mut staged = [
            PreparedAttachment::copy(provisional_id, &source, &files)?,
            PreparedAttachment::copy(provisional_id, &source, &files)?,
        ];
        let first = staged[0].metadata().path(&files);
        let second = staged[1].metadata().path(&files);
        std::fs::write(&second, "existing file")?;
        let draft = TaskDraft { title: "Plan".into(), ..TaskDraft::default() };
        assert!(store.create_task_with_attachments(draft.clone(), true, &mut staged, &files).is_err());
        assert!(store.tasks()?.is_empty());
        assert!(!first.exists());
        assert_eq!(std::fs::read_to_string(&second)?, "existing file");
        assert!(staged.iter().all(|attachment| attachment.path().exists()));
        std::fs::remove_file(&second)?;
        let task = store.create_task_with_attachments(draft, true, &mut staged, &files)?;
        assert_eq!(store.attachments(task.id)?.len(), 2);
        assert_eq!(std::fs::read_to_string(first)?, "plan");
        assert_eq!(std::fs::read_to_string(second)?, "plan");
        Ok(())
    }

    #[test]
    fn task_creation_rolls_back_when_tag_insertion_fails() -> Result<()> {
        let store = Store::open_in_memory()?;
        store.conn.execute_batch(
            "CREATE TEMP TRIGGER reject_tag BEFORE INSERT ON task_tags
             BEGIN SELECT RAISE(ABORT, 'tag could not be saved'); END;",
        )?;
        let draft = || TaskDraft {
            title: "Submit once".into(),
            tags: vec!["work".into()],
            ..TaskDraft::default()
        };
        assert!(store.create_task(draft(), true).is_err());
        assert!(store.tasks()?.is_empty());
        assert!(store.changes_since(0)?.0.is_empty());

        store.conn.execute_batch("DROP TRIGGER reject_tag")?;
        let task = store.create_task(draft(), true)?;
        assert_eq!(store.tasks()?, [task]);
        Ok(())
    }

    #[test]
    fn attachments_are_copied_and_removed() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-attach-test-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        let source = dir.join("receipt.txt");
        std::fs::write(&source, "paid")?;
        let store = Store::open_in_memory()?;
        let task = store.add_task("Pay rent")?;
        let folder = dir.join("attachments");
        let attachment = store.add_attachment(task.id, &source, &folder)?;
        assert_eq!((attachment.name.as_str(), attachment.size), ("receipt.txt", 4));
        assert_eq!(std::fs::read_to_string(attachment.path(&folder))?, "paid");
        assert_eq!(store.attachments(task.id)?, std::slice::from_ref(&attachment));
        store.delete_attachment(&attachment, &folder)?;
        assert!(store.attachments(task.id)?.is_empty());
        assert!(!attachment.path(&folder).exists());
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn directory_attachment_keeps_nested_files_and_empty_folders() -> Result<()> {
        use std::io::Read as _;

        let dir = tempfile::tempdir()?;
        let source = dir.path().join("trip");
        std::fs::create_dir_all(source.join("plans/empty"))?;
        std::fs::write(source.join("plans/tickets.txt"), "seat 14C")?;
        let store = Store::open_in_memory()?;
        let task = store.add_task("Pack for the trip")?;
        let folder = source.join("attachments");

        let attachment = store.add_attachment(task.id, &source, &folder)?;
        assert_eq!(attachment.name, "trip.zip");
        assert_eq!(store.attachments(task.id)?, std::slice::from_ref(&attachment));
        let mut zip = zip::ZipArchive::new(std::fs::File::open(attachment.path(&folder))?)?;
        let mut ticket = String::new();
        zip.by_name("trip/plans/tickets.txt")?.read_to_string(&mut ticket)?;
        assert_eq!(ticket, "seat 14C");
        assert!(zip.by_name("trip/plans/empty/")?.is_dir());
        assert!(zip.by_name("trip/attachments/.pending/").is_err());
        drop(zip);
        store.delete_attachment(&attachment, &folder)?;
        assert!(!attachment.path(&folder).exists());
        Ok(())
    }

    #[test]
    fn oversized_attachments_leave_no_file_or_record() -> Result<()> {
        use crate::attachment::{AttachmentTooLarge, MAX_ATTACHMENT_BYTES};
        use std::io::Write as _;

        let dir = tempfile::tempdir()?;
        let source = dir.path().join("source");
        let attachments = dir.path().join("attachments");
        std::fs::create_dir_all(&source)?;
        let file_path = source.join("data.bin");
        let mut file = std::fs::File::create(&file_path)?;
        file.set_len(MAX_ATTACHMENT_BYTES + 1)?;
        let store = Store::open_in_memory()?;
        let task = store.add_task("Has attachments")?;
        let error = store.add_attachment(task.id, &file_path, &attachments).unwrap_err();
        assert!(error.is::<AttachmentTooLarge>());

        // Incompressible data makes the finished ZIP exceed the same limit.
        let mut state = 1u64;
        let mut block = [0; 64 * 1024];
        for _ in 0..(MAX_ATTACHMENT_BYTES / block.len() as u64 + 1) {
            for bytes in block.as_chunks_mut::<8>().0 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                bytes.copy_from_slice(&state.to_le_bytes());
            }
            file.write_all(&block)?;
        }
        drop(file);
        let error = store.add_attachment(task.id, &source, &attachments).unwrap_err();
        assert!(error.is::<AttachmentTooLarge>());
        assert!(store.attachments(task.id)?.is_empty());
        assert_eq!(attachments.read_dir()?.count(), 1, "only the staging directory remains");
        assert_eq!(attachments.join(".pending").read_dir()?.count(), 0);
        Ok(())
    }

    #[test]
    fn focus_timers() -> Result<()> {
        use crate::focus::FocusTimer;
        let store = Store::open_in_memory()?;
        let mut timer = FocusTimer::new("Planning");
        assert_eq!((timer.minutes, timer.stopwatch), (25, false));
        store.save_focus_timer(&timer)?;
        timer.name = "Weekly planning".into();
        timer.minutes = 45;
        timer.icon = Some("📚".into());
        timer.color = Some("#4772fa".into());
        store.save_focus_timer(&timer)?;
        assert_eq!(store.focus_timers(false)?, [timer.clone()]);
        store.set_focus_timer_archived(timer.id, true)?;
        assert!(store.focus_timers(false)?.is_empty());
        assert_eq!(store.focus_timers(true)?[0].id, timer.id);
        store.delete_focus_timer(timer.id)?;
        assert!(store.focus_timers(true)?.is_empty());
        assert!(store.save_focus_timer(&FocusTimer::new(" ")).is_err());
        Ok(())
    }

    #[test]
    fn deleting_chosen_focus_records() -> Result<()> {
        let store = Store::open_in_memory()?;
        let a = store.add_focus_session(None, "a", 0, 60_000)?;
        let b = store.add_focus_session(None, "b", 120_000, 180_000)?;
        let c = store.add_focus_session(None, "c", 240_000, 300_000)?;
        store.delete_focus_sessions(&[a.id, c.id])?;
        let left: Vec<Id> = store.focus_sessions()?.into_iter().map(|s| s.id).collect();
        assert_eq!(left, [b.id]);
        Ok(())
    }

    #[test]
    fn countdown_look() -> Result<()> {
        let store = Store::open_in_memory()?;
        let release = store.add_countdown("Release", chrono::NaiveDate::from_ymd_opt(2026, 11, 19).unwrap(), CountdownKind::Countdown)?;
        store.set_countdown_look(release.id, Some("🚀"), Some("#4c8bf5"))?;
        let saved = store.countdowns()?.remove(0);
        assert_eq!((saved.icon.as_deref(), saved.color.as_deref()), (Some("🚀"), Some("#4c8bf5")));
        let monthly = crate::recurrence::Recurrence::new(crate::recurrence::Frequency::Monthly, 1);
        store.update_countdown(release.id, "Release day", saved.date, CountdownKind::Countdown, Some(&monthly))?;
        let edited = store.countdowns()?.remove(0);
        assert_eq!((edited.name.as_str(), edited.repeat), ("Release day", Some(monthly)));
        assert!(store.update_countdown(release.id, " ", saved.date, CountdownKind::Countdown, None).is_err());
        let other = store.add_countdown("Trip", saved.date, CountdownKind::Countdown)?;
        assert_eq!(store.countdowns()?[1].id, other.id);
        store.set_countdown_pinned(other.id, true)?;
        let pinned = store.countdowns()?;
        assert_eq!(pinned[0].id, other.id, "a pinned countdown comes first");
        assert!(pinned[0].pinned_at.is_some());
        store.set_countdown_pinned(other.id, false)?;
        assert_eq!(store.countdowns()?[0].id, release.id);
        store.set_countdown_look(release.id, Some("  "), None)?;
        let saved = store.countdowns()?.remove(0);
        assert_eq!((saved.icon, saved.color), (None, None));
        store.set_countdown_show_age(release.id, true)?;
        assert!(store.countdowns()?[0].show_age);
        store.set_countdown_reminders(release.id, &[3, 0, 3], &[], true)?;
        let saved = store.countdowns()?.remove(0);
        assert_eq!((saved.reminders, saved.constant_reminder), (vec![0, 3], true));
        store.set_countdown_display(release.id, crate::countdown::ShowInList::Always, true, true)?;
        let saved = store.countdowns()?.remove(0);
        assert_eq!((saved.show_in_list, saved.plus_one_day, saved.count_up), (crate::countdown::ShowInList::Always, true, true));
        store.set_countdown_archived(release.id, true)?;
        assert!(store.countdowns()?.iter().find(|c| c.id == release.id).unwrap().archived);
        store.set_countdown_archived(release.id, false)?;
        assert!(!store.countdowns()?.iter().find(|c| c.id == release.id).unwrap().archived);

        // A background picture is copied into the folder and named after the countdown.
        let dir = std::env::temp_dir().join(format!("fern-backgrounds-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        let photo = dir.join("photo.jpg");
        std::fs::write(&photo, b"jpeg")?;
        let name = store.set_countdown_background(release.id, Some(&photo), &dir.join("files"))?.unwrap();
        assert!(name.starts_with(&format!("{}-background-", release.id)) && name.ends_with(".jpg"));
        assert!(dir.join("files").join(&name).exists());
        assert_eq!(store.countdowns()?.iter().find(|c| c.id == release.id).unwrap().background.as_deref(), Some(name.as_str()));
        store.set_countdown_background(release.id, None, &dir)?;
        assert!(store.countdowns()?.iter().find(|c| c.id == release.id).unwrap().background.is_none());
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn reminder_plus_one_and_snooze() -> Result<()> {
        let store = Store::open_in_memory()?;
        let water = store.add_habit("Water")?;
        store.update_habit(water.id, "Water", Some(&Goal::new(2.0, "glasses")))?;
        let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        assert_eq!(store.add_checkin_amount(water.id, day, 1.0)?, 1.0);
        assert_eq!(store.add_checkin_amount(water.id, day, 1.0)?, 2.0);
        assert_eq!(store.add_checkin_amount(water.id, day, 1.0)?, 3.0, "+1 goes past the goal");

        let at = |h, m| day.and_hms_opt(h, m, 0).unwrap();
        store.snooze_habit(water.id, at(9, 15))?;
        assert_eq!(store.habit_snoozes()?, [(water.id, at(9, 15))]);
        // Snoozing the next day drops the old snooze.
        store.snooze_habit(water.id, at(9, 15) + Duration::days(2))?;
        assert_eq!(store.habit_snoozes()?.len(), 1);
        store.delete_habit(water.id)?;
        assert!(store.habit_snoozes()?.is_empty());
        Ok(())
    }

    #[test]
    fn habit_draft_modes_logs_skips_and_ordering() -> Result<()> {
        let store = Store::open_in_memory()?;
        let day1 = d(2026, 10, 18);
        let day2 = d(2026, 10, 19);
        let draft = HabitDraft {
            name: "Run".into(),
            goal: Some(Goal { amount: 5.0, unit: "km".into(), mode: CheckinMode::Auto { step: 2.5 } }),
            reminders: vec![NaiveTime::from_hms_opt(8, 0, 0).unwrap()],
            frequency: crate::habit::Frequency::default(),
            icon: Some("🏃".into()),
            color: Some("#22C55E".into()),
            start_date: Some(day1),
            goal_days: Some(21),
            section: HabitSection::Morning,
            constant_reminder: true,
            record_enable: true,
        };
        let run = store.create_habit(&draft)?;
        assert_eq!(run.goal.as_ref().unwrap().mode, CheckinMode::Auto { step: 2.5 });
        assert!(run.record_enable && run.constant_reminder);

        // Auto step increments by 2.5 -> partial then done.
        assert_eq!(store.step_checkin(run.id, day1)?, 2.5);
        assert!(store.checkins(run.id)?.is_empty());
        assert_eq!(store.step_checkin(run.id, day1)?, 5.0);
        assert_eq!(store.checkins(run.id)?, [day1].into());

        // Habit Log with mood survives check-in reset.
        store.set_checkin_log(run.id, day1, Some("Great pace"), 50)?;
        store.reset_checkin(run.id, day1, true)?;
        let rec = store.checkin_records(run.id)?;
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].status, CheckinStatus::Unlabeled);
        assert_eq!(rec[0].value, 0.0);
        assert_eq!(rec[0].mood, 50);
        assert_eq!(rec[0].note.as_deref(), Some("Great pace"));

        // Deleting log when unchecked removes the empty row.
        store.delete_checkin_log(run.id, day1)?;
        assert!(store.checkin_records(run.id)?.is_empty());

        // Standalone log without check-in, then mark Undone.
        store.set_checkin_log(run.id, day2, Some("Rainy"), 20)?;
        store.mark_checkin_undone(run.id, day2)?;
        let rec2 = &store.checkin_records(run.id)?[0];
        assert_eq!(rec2.status, CheckinStatus::Undone);
        assert_eq!(rec2.mood, 20);
        assert_eq!(rec2.note.as_deref(), Some("Rainy"));

        // Skipping a day adds to ex_dates; checking in un-skips it.
        store.skip_habit_date(run.id, day1)?;
        assert!(store.habits()?[0].is_skipped(day1));
        store.toggle_checkin(run.id, day1)?;
        assert!(!store.habits()?[0].is_skipped(day1));
        assert!(store.checkins(run.id)?.contains(&day1));

        // Batch query returns records and done dates.
        assert_eq!(store.all_habit_checkins()?.get(&run.id).map(Vec::len), Some(2));
        assert_eq!(store.all_habit_done_dates()?.get(&run.id), Some(&HashSet::from([day1])));

        // Reordering habits and custom sections persists.
        let read = store.add_habit("Read")?;
        let sec_a = store.add_habit_section("Evening")?;
        let sec_b = store.add_habit_section("Work")?;
        store.reorder_habit_sections(&[sec_b.id, sec_a.id])?;
        assert_eq!(store.habit_sections()?.iter().map(|s| s.id).collect::<Vec<_>>(), [sec_b.id, sec_a.id]);
        store.reorder_habits(&[(read.id, HabitSection::Custom(sec_b.id), 0), (run.id, HabitSection::Custom(sec_b.id), 1)])?;
        assert_eq!(store.habits()?.iter().map(|h| h.id).collect::<Vec<_>>(), [read.id, run.id]);

        // Focus session linked to habit persists habit_id.
        let sess = store.add_focus_session_for(None, Some(run.id), "Run", 1000, 1000 + 25 * 60_000, false)?;
        assert_eq!(sess.habit_id, Some(run.id));
        assert_eq!(store.focus_sessions()?[0].habit_id, Some(run.id));
        Ok(())
    }

    #[test]
    fn daily_backups() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-daily-test-{}", Uuid::now_v7()));
        let store = Store::open_in_memory()?;
        store.add_task("first")?;
        let day = |d: u32| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
        store.daily_backup(&dir, day(1), 2)?;
        // A second start the same day keeps the morning's copy.
        store.add_task("second")?;
        store.daily_backup(&dir, day(1), 2)?;
        Store::check_backup(&dir.join("fern-2026-10-01.db"))?;
        let copy = Store::open(dir.join("fern-2026-10-01.db"))?;
        assert_eq!(copy.tasks()?.len(), 1);
        drop(copy);
        // Only the newest copies stay.
        store.daily_backup(&dir, day(2), 2)?;
        store.daily_backup(&dir, day(5), 2)?;
        let mut names: Vec<String> = std::fs::read_dir(&dir)?.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["fern-2026-10-02.db", "fern-2026-10-05.db"]);
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn backup_and_restore() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-backup-test-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("backup.db");

        let store = Store::open_in_memory()?;
        let task = store.add_task("kept in the backup")?;
        let source = dir.join("ticket.txt");
        std::fs::write(&source, "seat 14C")?;
        let attachments = dir.join("attachments");
        let attachment = store.add_attachment(task.id, &source, &attachments)?;
        store.backup_to(&file, Some(&attachments))?;

        // Restoring elsewhere brings the tasks and the attached files back, and the folder
        // loses what the backup does not have.
        let mut other = Store::open_in_memory()?;
        other.add_task("replaced")?;
        let other_files = dir.join("other-attachments");
        std::fs::create_dir_all(&other_files)?;
        std::fs::write(other_files.join("stale.txt"), "old")?;
        other.restore_from(&file, Some(&other_files))?;
        let titles: Vec<String> = other.tasks()?.into_iter().map(|t| t.title).collect();
        assert_eq!(titles, ["kept in the backup"]);
        assert_eq!(other.attachments(task.id)?, std::slice::from_ref(&attachment));
        assert_eq!(std::fs::read_to_string(attachment.path(&other_files))?, "seat 14C");
        assert!(!other_files.join("stale.txt").exists());
        let leftover: bool = other
            .conn
            .query_row("SELECT 1 FROM sqlite_master WHERE name = 'backup_files'", [], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        assert!(!leftover, "the files table does not stay in the data");

        // Something that is not a backup changes nothing.
        let junk = dir.join("junk.db");
        Connection::open(&junk)?.execute_batch("CREATE TABLE notes (x TEXT);")?;
        assert!(other.restore_from(&junk, None).is_err());
        assert_eq!(other.tasks()?.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn estimates() -> Result<()> {
        use crate::model::Estimate;
        let store = Store::open_in_memory()?;
        let task = store.add_task("write the report")?;
        assert_eq!(store.task(task.id)?.unwrap().estimate, None);
        store.set_estimate(task.id, Some(Estimate::Pomos(3)))?;
        assert_eq!(store.task(task.id)?.unwrap().estimate, Some(Estimate::Pomos(3)));
        store.set_estimate(task.id, Some(Estimate::Minutes(90)))?;
        assert_eq!(store.task(task.id)?.unwrap().estimate, Some(Estimate::Minutes(90)));
        store.set_estimate(task.id, None)?;
        assert_eq!(store.task(task.id)?.unwrap().estimate, None);
        assert_eq!(Estimate::from_storage("p:0"), None);
        Ok(())
    }

    #[test]
    fn new_tasks_at_the_top_or_bottom() -> Result<()> {
        let store = Store::open_in_memory()?;
        store.add_task("first")?;
        store.add_task_at("top", true)?;
        store.add_task_at("bottom", false)?;
        let titles: Vec<String> = store.tasks_in(&View::Inbox, chrono::Local::now().date_naive())?.into_iter().map(|t| t.title).collect();
        assert_eq!(titles, ["top", "first", "bottom"]);
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

        // Linking a parent moves the task into the parent's list.
        let list = store.create_list("Home")?;
        let other = store.add_task("other")?;
        store.set_list(parent.id, Some(list.id))?;
        store.link_parent(other.id, parent.id)?;
        let other = store.task(other.id)?.unwrap();
        assert_eq!((other.parent_id, other.list_id), (Some(parent.id), Some(list.id)));
        assert!(store.link_parent(parent.id, grandchild.id).is_err());

        store.purge_task(parent.id)?;
        assert!(store.tasks()?.is_empty());
        Ok(())
    }

    #[test]
    fn list_and_folder_drag_reordering_and_grouping() -> Result<()> {
        let store = Store::open_in_memory()?;
        let a = store.create_list("A")?;
        let b = store.create_list("B")?;
        let c = store.create_list("C")?;
        let d = store.create_list("D")?;

        let names = |store: &Store| -> Result<Vec<(String, Option<Id>)>> {
            Ok(store.lists()?.into_iter().map(|l| (l.name, l.folder)).collect())
        };

        // Drop D before B.
        store.place_list_relative(d.id, b.id, false, None)?;
        assert_eq!(
            names(&store)?,
            vec![("A".into(), None), ("D".into(), None), ("B".into(), None), ("C".into(), None)]
        );

        // Drop C onto A (middle zone) -> creates a folder at A's position containing A then C.
        let f1 = store.group_lists_into_folder(c.id, a.id, "New Folder")?;
        assert_eq!(
            names(&store)?,
            vec![
                ("A".into(), Some(f1.id)),
                ("C".into(), Some(f1.id)),
                ("D".into(), None),
                ("B".into(), None),
            ]
        );

        // Cannot group onto a list already inside a folder.
        assert!(store.group_lists_into_folder(d.id, a.id, "Nested").is_err());

        // Move folder f1 after B.
        store.reorder_list_folder(f1.id, FolderDropTarget::List(b.id), true)?;
        assert_eq!(
            names(&store)?,
            vec![
                ("D".into(), None),
                ("B".into(), None),
                ("A".into(), Some(f1.id)),
                ("C".into(), Some(f1.id)),
            ]
        );

        // Place D before folder f1 (out of any folder) and C out after folder f1.
        store.place_list_around_folder(d.id, f1.id, false)?;
        assert_eq!(
            names(&store)?,
            vec![
                ("B".into(), None),
                ("D".into(), None),
                ("A".into(), Some(f1.id)),
                ("C".into(), Some(f1.id)),
            ]
        );
        store.place_list_around_folder(c.id, f1.id, true)?;
        assert_eq!(
            names(&store)?,
            vec![
                ("B".into(), None),
                ("D".into(), None),
                ("A".into(), Some(f1.id)),
                ("C".into(), None),
            ]
        );

        // Create second folder f2 around B and D, then move f2 after f1.
        let f2 = store.group_lists_into_folder(d.id, b.id, "Second")?;
        store.reorder_list_folder(f2.id, FolderDropTarget::Folder(f1.id), true)?;
        assert_eq!(
            names(&store)?,
            vec![
                ("A".into(), Some(f1.id)),
                ("B".into(), Some(f2.id)),
                ("D".into(), Some(f2.id)),
                ("C".into(), None),
            ]
        );

        // Empty folders reorder among themselves via list_folders sort_order.
        let e1 = store.add_list_folder("Empty 1")?;
        let e2 = store.add_list_folder("Empty 2")?;
        store.reorder_list_folder(e2.id, FolderDropTarget::Folder(e1.id), false)?;
        let empty_order: Vec<Id> = store
            .list_folders()?
            .into_iter()
            .filter(|f| f.id == e1.id || f.id == e2.id)
            .map(|f| f.id)
            .collect();
        assert_eq!(empty_order, vec![e2.id, e1.id]);
        Ok(())
    }

    #[test]
    fn tag_drag_reordering_nesting_and_promotion() -> Result<()> {
        let store = Store::open_in_memory()?;
        store.add_tag("work", None, None)?;
        store.add_tag("home", None, None)?;
        store.add_tag("errands", None, None)?;
        store.add_tag("urgent", None, None)?;

        let top_order = |store: &Store| -> Result<Vec<String>> {
            Ok(store.tags()?.into_iter().filter(|t| t.parent.is_none()).map(|t| t.name).collect())
        };
        let children_of = |store: &Store, parent: &str| -> Result<Vec<String>> {
            Ok(store.tags()?.into_iter().filter(|t| t.parent.as_deref() == Some(parent)).map(|t| t.name).collect())
        };

        // Nest errands and urgent under work.
        store.move_tag("errands", "work", false, true)?;
        store.move_tag("urgent", "work", false, true)?;
        assert_eq!(top_order(&store)?, vec!["work", "home"]);
        assert_eq!(children_of(&store, "work")?, vec!["errands", "urgent"]);

        // Reorder sub-tags inside work.
        store.move_tag("urgent", "errands", false, false)?;
        assert_eq!(children_of(&store, "work")?, vec!["urgent", "errands"]);

        // 2-level nesting is rejected: parent tag cannot nest under home, nor sub-tag under sub-tag.
        store.move_tag("work", "home", false, true)?;
        assert_eq!(store.tags()?.into_iter().find(|t| t.name == "work").unwrap().parent, None);
        store.move_tag("errands", "urgent", false, true)?;
        assert_eq!(store.tags()?.into_iter().find(|t| t.name == "errands").unwrap().parent.as_deref(), Some("work"));

        // Move parent tag work after home.
        store.move_tag("work", "home", true, false)?;
        assert_eq!(top_order(&store)?, vec!["home", "work"]);
        assert_eq!(children_of(&store, "work")?, vec!["urgent", "errands"]);

        // Promote sub-tag errands to top-level before home.
        store.move_tag("errands", "home", false, false)?;
        assert_eq!(top_order(&store)?, vec!["errands", "home", "work"]);
        assert_eq!(children_of(&store, "work")?, vec!["urgent"]);

        // Drop a sub-tag on Tags heading promotes it to first.
        store.move_tag_first("urgent")?;
        assert_eq!(top_order(&store)?, vec!["urgent", "errands", "home", "work"]);

        // Renaming a tag preserves its custom order.
        store.update_tag("home", "house", None, None)?;
        assert_eq!(top_order(&store)?, vec!["urgent", "errands", "house", "work"]);

        // Pinning boundary: dragging an unpinned tag onto a pinned tag adopts pinned state.
        store.set_tag_pinned("urgent", true)?;
        store.move_tag("work", "urgent", false, false)?;
        assert!(store.tags()?.into_iter().find(|t| t.name == "work").unwrap().pinned);
        Ok(())
    }

    #[test]
    fn filter_drag_reordering() -> Result<()> {
        use crate::filter::{FilterRules, SmartFilter};
        let store = Store::open_in_memory()?;
        let f1 = SmartFilter { id: Uuid::now_v7(), name: "First".into(), rules: FilterRules::default() };
        let f2 = SmartFilter { id: Uuid::now_v7(), name: "Second".into(), rules: FilterRules::default() };
        let f3 = SmartFilter { id: Uuid::now_v7(), name: "Third".into(), rules: FilterRules::default() };
        store.save_filter(&f1)?;
        store.save_filter(&f2)?;
        store.save_filter(&f3)?;

        let names = |store: &Store| -> Result<Vec<String>> {
            Ok(store.filters()?.into_iter().map(|f| f.name).collect())
        };
        assert_eq!(names(&store)?, vec!["First", "Second", "Third"]);

        store.move_filter(f3.id, f1.id, false)?;
        assert_eq!(names(&store)?, vec!["Third", "First", "Second"]);

        store.move_filter(f3.id, f2.id, true)?;
        assert_eq!(names(&store)?, vec!["First", "Second", "Third"]);

        store.move_filter_first(f2.id)?;
        assert_eq!(names(&store)?, vec!["Second", "First", "Third"]);

        store.set_filter_order(&[f3.id, f1.id, f2.id])?;
        assert_eq!(names(&store)?, vec!["Third", "First", "Second"]);
        Ok(())
    }

    #[test]
    fn batch_save_list_folder_and_tag_order_commits_atomically() -> Result<()> {
        let store = Store::open_in_memory()?;
        let a = store.create_list("A")?;
        let b = store.create_list("B")?;
        let c = store.create_list("C")?;
        let f1 = store.add_list_folder("F1")?;
        let f2 = store.add_list_folder("F2")?;

        let mut lists = store.lists()?;
        lists.iter_mut().find(|l| l.id == b.id).unwrap().folder = Some(f1.id);
        // Reorder to [C, B(in F1), A] and folders [F2, F1].
        let ordered_lists = vec![
            lists.iter().find(|l| l.id == c.id).unwrap().clone(),
            lists.iter().find(|l| l.id == b.id).unwrap().clone(),
            lists.iter().find(|l| l.id == a.id).unwrap().clone(),
        ];
        store.save_list_and_folder_order(&ordered_lists, &[f2.clone(), f1.clone()])?;

        let saved_lists: Vec<(String, Option<Id>)> =
            store.lists()?.into_iter().map(|l| (l.name, l.folder)).collect();
        assert_eq!(
            saved_lists,
            vec![("C".into(), None), ("B".into(), Some(f1.id)), ("A".into(), None)]
        );
        let saved_folders: Vec<Id> = store.list_folders()?.into_iter().map(|f| f.id).collect();
        assert_eq!(saved_folders, vec![f2.id, f1.id]);

        // Re-saving an identical order must not dirty any sync_log rows.
        store.conn.execute("DELETE FROM sync_log", [])?;
        store.save_list_and_folder_order(&store.lists()?, &store.list_folders()?)?;
        let dirty: i64 = store.conn.query_row("SELECT COUNT(*) FROM sync_log", [], |r| r.get(0))?;
        assert_eq!(dirty, 0, "no-op order save leaves sync_log untouched");

        store.add_tag("t1", None, None)?;
        store.add_tag("t2", None, None)?;
        store.add_tag("t3", None, None)?;
        let mut tags = store.tags()?;
        tags.iter_mut().find(|t| t.name == "t3").unwrap().pinned = true;
        tags.iter_mut().find(|t| t.name == "t1").unwrap().parent = Some("t2".into());
        let ordered_tags = vec![
            tags.iter().find(|t| t.name == "t3").unwrap().clone(),
            tags.iter().find(|t| t.name == "t2").unwrap().clone(),
            tags.iter().find(|t| t.name == "t1").unwrap().clone(),
        ];
        store.save_tag_hierarchy_and_order(&ordered_tags)?;
        let reloaded = store.tags()?;
        let summary: Vec<(String, Option<String>, bool)> =
            reloaded.into_iter().map(|t| (t.name, t.parent, t.pinned)).collect();
        assert_eq!(
            summary,
            vec![
                ("t3".into(), None, true),
                ("t2".into(), None, false),
                ("t1".into(), Some("t2".into()), false),
            ]
        );
        Ok(())
    }

    #[test]
    fn nested_tasks_depth_unlink_move_notes_and_completion_cascade() -> Result<()> {
        let store = Store::open_in_memory()?;
        // 5 levels: 0 -> 1 -> 2 -> 3 -> 4 (MAX_SUBTASK_DEPTH).
        let l0 = store.add_task("L0")?;
        let l1 = store.add_subtask(l0.id, "L1")?;
        let l2 = store.add_subtask(l1.id, "L2")?;
        let l3 = store.add_subtask(l2.id, "L3")?;
        let l4 = store.add_subtask(l3.id, "L4")?;
        assert_eq!(store.task_depth(l0.id)?, 0);
        assert_eq!(store.task_depth(l4.id)?, MAX_SUBTASK_DEPTH);
        assert!(store.add_subtask(l4.id, "L5").is_err(), "cannot nest past level 5");

        // Linking a 3-level subtree under L2 flattens overflow descendants onto level 5 (depth 4).
        let sub_root = store.add_task("SR")?;
        let sub_child = store.add_subtask(sub_root.id, "SC")?;
        let sub_grand = store.add_subtask(sub_child.id, "SG")?;
        store.link_parent(sub_root.id, l2.id)?;
        assert_eq!(store.task_depth(sub_root.id)?, 3);
        assert_eq!(store.task_depth(sub_child.id)?, 4);
        assert_eq!(store.task_depth(sub_grand.id)?, 4);
        assert_eq!(store.task(sub_grand.id)?.unwrap().parent_id, Some(sub_root.id));

        // Notes cannot have subtasks, cannot be nested, and nested tasks cannot become notes.
        let note = store.add_task("My note")?;
        store.set_kind(note.id, Kind::Note)?;
        assert!(store.add_subtask(note.id, "nope").is_err());
        assert!(store.set_parent(note.id, Some(l0.id)).is_err());
        assert!(store.set_kind(l0.id, Kind::Note).is_err());
        assert!(store.set_kind(l1.id, Kind::Note).is_err());

        // Unlink promotes a subtask to top-level while keeping its own children attached.
        store.unlink_parent(sub_root.id)?;
        assert_eq!(store.task(sub_root.id)?.unwrap().parent_id, None);
        assert_eq!(store.task(sub_child.id)?.unwrap().parent_id, Some(sub_root.id));

        // Moving a subtask to another list unlinks it from its old parent and moves its subtree.
        let other = store.create_list("Other")?;
        store.set_list(sub_child.id, Some(other.id))?;
        assert_eq!(store.task(sub_child.id)?.unwrap().parent_id, None);
        assert_eq!(store.task(sub_child.id)?.unwrap().list_id, Some(other.id));

        // Moving a subtask to another section unlinks it from its parent in the old section.
        let sec = store.add_list_section(Some(other.id), "Done soon")?;
        let sec_child = store.add_subtask(sub_child.id, "In section")?;
        store.set_section(sec_child.id, Some(sec.id))?;
        assert_eq!(store.task(sec_child.id)?.unwrap().parent_id, None);
        assert_eq!(store.task(sec_child.id)?.unwrap().section_id, Some(sec.id));

        // Completing a parent cascades to open descendants; reopening a subtask reopens ancestors.
        let p = store.add_task("Parent")?;
        let c1 = store.add_subtask(p.id, "Child 1")?;
        let c2 = store.add_subtask(p.id, "Child 2")?;
        store.set_completed(c1.id, true)?;
        let (_, cascaded) = store.complete_with_cascaded(p.id, true)?;
        assert_eq!(cascaded, vec![c2.id]);
        assert!(store.task(p.id)?.unwrap().is_completed());
        assert!(store.task(c2.id)?.unwrap().is_completed());
        // Undo reopens only parent and newly cascaded c2, leaving pre-completed c1 completed.
        store.reopen_tasks(&[p.id, c2.id])?;
        assert!(!store.task(p.id)?.unwrap().is_completed());
        assert!(store.task(c1.id)?.unwrap().is_completed());
        assert!(!store.task(c2.id)?.unwrap().is_completed());
        // Completing p again and reopening c2 reopens p as well.
        store.set_completed(p.id, true)?;
        store.set_completed(c2.id, false)?;
        assert!(!store.task(p.id)?.unwrap().is_completed());
        assert!(!store.task(c2.id)?.unwrap().is_completed());

        // move_task_in_tree nests and unnests cleanly.
        let a = store.add_task("A")?;
        let b = store.add_task("B")?;
        store.move_task_in_tree(b.id, Some(a.id), Some((a.id, true)))?;
        assert_eq!(store.task(b.id)?.unwrap().parent_id, Some(a.id));
        store.move_task_in_tree(b.id, None, Some((a.id, false)))?;
        assert_eq!(store.task(b.id)?.unwrap().parent_id, None);
        let ids: Vec<Id> = store.tasks()?.into_iter().map(|t| t.id).collect();
        let pos_b = ids.iter().position(|&id| id == b.id).unwrap();
        let pos_a = ids.iter().position(|&id| id == a.id).unwrap();
        assert!(pos_b < pos_a, "unnested B moved before A");
        Ok(())
    }

    #[test]
    fn link_parent_at_max_depth_syncs_flattened_descendants_list_section_and_completion() -> Result<()> {
        let store = Store::open_in_memory()?;
        let work = store.create_list("Work")?;
        let sec = store.add_list_section(Some(work.id), "Active")?;
        let l0 = store.add_task("L0")?;
        store.set_list(l0.id, Some(work.id))?;
        store.set_section(l0.id, Some(sec.id))?;
        let l1 = store.add_subtask(l0.id, "L1")?;
        let l2 = store.add_subtask(l1.id, "L2")?;
        let l3 = store.add_subtask(l2.id, "L3")?; // depth 3 == MAX_SUBTASK_DEPTH - 1
        store.set_completed(l3.id, true)?;

        // Source subtree lives in Inbox (list = None, section = None), open.
        let src_root = store.add_task("SR")?;
        let src_child = store.add_subtask(src_root.id, "SC")?;
        let src_grand = store.add_subtask(src_child.id, "SG")?;

        store.link_parent(src_root.id, l3.id)?;
        // Flattened children are reparented onto l3, and MUST also move into Work/sec and close.
        for id in [src_root.id, src_child.id, src_grand.id] {
            let t = store.task(id)?.unwrap();
            assert_eq!(t.parent_id, Some(l3.id));
            assert_eq!(t.list_id, Some(work.id));
            assert_eq!(t.section_id, Some(sec.id));
            assert!(t.is_completed());
        }
        Ok(())
    }

    #[test]
    fn restore_subtask_detaches_from_trashed_parent_and_survives_empty_trash() -> Result<()> {
        let store = Store::open_in_memory()?;
        let parent = store.add_task("Parent")?;
        let child = store.add_subtask(parent.id, "Child")?;
        let grand = store.add_subtask(child.id, "Grand")?;

        store.delete_task(parent.id)?;
        // Restore only child; it must detach from trashed parent so empty_trash cannot CASCADE it.
        store.restore_task(child.id)?;
        assert_eq!(store.task(child.id)?.unwrap().parent_id, None);
        assert_eq!(store.task(grand.id)?.unwrap().parent_id, Some(child.id));

        store.empty_trash()?;
        assert!(store.task(parent.id)?.is_none());
        assert!(store.task(child.id)?.is_some());
        assert!(store.task(grand.id)?.is_some());
        // Cannot attach a subtask to a trashed parent.
        let doomed = store.add_task("Doomed")?;
        store.delete_task(doomed.id)?;
        assert!(store.add_subtask(doomed.id, "Nope").is_err());
        assert!(store.set_parent(child.id, Some(doomed.id)).is_err());
        Ok(())
    }

    #[test]
    fn reorder_list_sections_and_move_task_avoid_redundant_sync_log_writes() -> Result<()> {
        let store = Store::open_in_memory()?;
        let list = store.create_list("Board")?;
        let s1 = store.add_list_section(Some(list.id), "S1")?;
        let s2 = store.add_list_section(Some(list.id), "S2")?;
        let s3 = store.add_list_section(Some(list.id), "S3")?;

        store.reorder_list_sections(Some(list.id), s3.id, s1.id)?;
        let order: Vec<Id> = store.list_sections(Some(list.id))?.into_iter().map(|s| s.id).collect();
        assert_eq!(order, vec![s3.id, s1.id, s2.id]);

        // Reordering two adjacent tasks in a 5-task DB should only dirty changed sort_orders.
        let t1 = store.add_task("T1")?;
        let t2 = store.add_task("T2")?;
        let t3 = store.add_task("T3")?;
        let t4 = store.add_task("T4")?;
        store.move_task(t1.id, t2.id, true)?; // normalizes sort_orders to 0, 1024, 2048, 3072
        store.conn.execute("DELETE FROM sync_log", [])?;
        store.move_task(t3.id, t4.id, true)?; // swaps only last two (or first two), unchanged rows skipped
        let dirty: i64 = store.conn.query_row("SELECT COUNT(*) FROM sync_log WHERE entity = 'tasks'", [], |r| r.get(0))?;
        assert!(dirty <= 2, "expected at most 2 dirty sync_log rows, got {dirty}");
        Ok(())
    }

    #[test]
    fn descendants_terminates_on_synced_cycle_and_same_list_preserves_section() -> Result<()> {
        let store = Store::open_in_memory()?;
        let list = store.create_list("Proj")?;
        let sec = store.add_list_section(Some(list.id), "Backlog")?;
        let p = store.add_task("P")?;
        store.set_list(p.id, Some(list.id))?;
        store.set_section(p.id, Some(sec.id))?;
        let c = store.add_subtask(p.id, "C")?;
        // Calling set_list with the same list must preserve section_id and parent_id.
        store.set_list(c.id, Some(list.id))?;
        let c_after = store.task(c.id)?.unwrap();
        assert_eq!(c_after.parent_id, Some(p.id));
        assert_eq!(c_after.section_id, Some(sec.id));

        // Simulate a corrupted cycle introduced by concurrent sync and verify descendants terminates.
        store.conn.execute(
            "UPDATE tasks SET parent_id = ?2 WHERE id = ?1",
            params![p.id.to_string(), c.id.to_string()],
        )?;
        let desc = store.descendants(p.id)?;
        assert!(desc.contains(&c.id));
        assert!(desc.len() <= 2);
        Ok(())
    }

    #[test]
    fn in_progress_location_timezone_snooze_and_batch_operations() -> Result<()> {
        let store = Store::open_in_memory()?;
        let a = store.add_task("Read paper")?;
        let b = store.add_task("Draft notes")?;
        let c = store.add_task("Review PR")?;

        // Explicit InProgress + activity trigger + cleared on close.
        store.set_in_progress(a.id, true)?;
        assert!(store.task(a.id)?.unwrap().is_in_progress());
        assert!(store.task_activity(a.id)?.iter().any(|entry| entry.kind == "in_progress"));
        store.set_completed(a.id, true)?;
        assert!(!store.task(a.id)?.unwrap().is_in_progress());
        store.set_completed(a.id, false)?;

        // Location + Search matching location.
        store.set_location(a.id, Some("  Library Desk 4  "))?;
        assert_eq!(store.task(a.id)?.unwrap().location.as_deref(), Some("Library Desk 4"));
        let hits = store.tasks_in(&View::Search("library".into()), d(2026, 10, 18))?;
        assert_eq!(hits.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a.id]);

        // Timezone validation & floating flag.
        assert!(store.set_timezone(a.id, Some("Not/A_Zone"), false).is_err());
        store.set_timezone(a.id, Some("America/New_York"), true)?;
        let loaded = store.task(a.id)?.unwrap();
        assert_eq!(loaded.timezone.as_deref(), Some("America/New_York"));
        assert!(loaded.is_floating);

        // Snooze + constant reminder.
        let snooze_at = d(2026, 10, 18).and_hms_opt(10, 15, 0).unwrap();
        store.snooze_task(a.id, snooze_at)?;
        store.set_task_constant_reminder(a.id, true)?;
        let loaded = store.task(a.id)?.unwrap();
        assert_eq!(loaded.snoozed_until, Some(snooze_at));
        assert!(loaded.constant_reminder);
        store.clear_task_snooze(a.id)?;
        assert_eq!(store.task(a.id)?.unwrap().snoozed_until, None);

        // Batch operations.
        let ids = [a.id, b.id];
        store.batch_set_priority(&ids, Priority::High)?;
        store.batch_add_tag(&ids, "urgent")?;
        store.batch_set_in_progress(&ids, true)?;
        assert_eq!(store.task(a.id)?.unwrap().priority, Priority::High);
        assert_eq!(store.task(b.id)?.unwrap().tags, vec!["urgent"]);
        assert!(store.task(b.id)?.unwrap().is_in_progress());
        assert!(!store.task(c.id)?.unwrap().is_in_progress());
        store.batch_remove_tag(&ids, "urgent")?;
        assert!(store.task(a.id)?.unwrap().tags.is_empty());

        let work = store.create_list("Work")?;
        store.batch_set_list(&ids, Some(work.id))?;
        assert_eq!(store.task(b.id)?.unwrap().list_id, Some(work.id));
        store.batch_delete(&ids)?;
        assert!(store.task(a.id)?.unwrap().is_deleted());
        store.batch_restore(&ids)?;
        assert!(!store.task(a.id)?.unwrap().is_deleted());
        store.batch_purge(&[b.id])?;
        assert!(store.task(b.id)?.is_none());
        Ok(())
    }

    #[test]
    fn holiday_skip_and_forgetting_curve_advance() -> Result<()> {
        let store = Store::open_in_memory()?;
        store.add_countdown("Holiday", d(2026, 10, 19), CountdownKind::Holiday)?;
        let task = store.add_task("Vocab")?;
        store.set_due(task.id, Some(d(2026, 10, 16)), None)?;
        let mut fc = crate::Recurrence::forgetting_curve();
        fc.skip_holidays = true;
        fc.skip_weekends = true;
        store.set_repeat(task.id, Some(fc.clone()), d(2026, 10, 16))?;
        let copy = store.complete(task.id, true)?.unwrap();
        let after = store.task(task.id)?.unwrap();
        // +1 day = Sat Oct 17 -> skips weekend to Mon Oct 19 -> skips holiday to Tue Oct 20.
        assert_eq!(after.due_date, Some(d(2026, 10, 20)));
        assert_eq!(after.repeat.as_ref().map(|r| r.cycle), Some(1));
        store.undo_repeat(task.id, copy, Some(d(2026, 10, 16)), None, Some(&fc))?;
        let reverted = store.task(task.id)?.unwrap();
        assert_eq!(reverted.due_date, Some(d(2026, 10, 16)));
        assert_eq!(reverted.repeat.as_ref().map(|r| r.cycle), Some(0));
        Ok(())
    }

    #[test]
    fn resize_dates_and_timed_range_update_span() -> Result<()> {
        let store = Store::open_in_memory()?;
        let t = store.add_task("Workshop")?;
        store.set_due(t.id, Some(d(2026, 11, 10)), None)?;

        // Expand all-day span to 3 days, then shrink back to single day.
        store.resize_dates(t.id, d(2026, 11, 10), d(2026, 11, 12))?;
        let loaded = store.task(t.id)?.unwrap();
        assert_eq!(loaded.due_date, Some(d(2026, 11, 10)));
        assert_eq!(loaded.end_date, Some(d(2026, 11, 12)));

        store.resize_dates(t.id, d(2026, 11, 11), d(2026, 11, 11))?;
        let single = store.task(t.id)?.unwrap();
        assert_eq!(single.due_date, Some(d(2026, 11, 11)));
        assert_eq!(single.end_date, None);

        // Resize timed range on a day.
        let s = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let e = NaiveTime::from_hms_opt(11, 30, 0).unwrap();
        store.resize_timed_range(t.id, d(2026, 11, 11), s, e, None)?;
        let timed = store.task(t.id)?.unwrap();
        assert_eq!(timed.due_date, Some(d(2026, 11, 11)));
        assert_eq!(timed.due_time, Some(s));
        assert_eq!(timed.end_time, Some(e));
        assert_eq!(timed.end_date, None);
        Ok(())
    }

    #[test]
    fn caldav_event_queue_and_snapshot_preserve_dirty_edits() -> Result<()> {
        let store = Store::open_in_memory()?;
        let sub_id = store.add_caldav_subscription(
            "Work CalDAV",
            "https://dav.example.com/cal/",
            Some("#3B82F6"),
            crate::ical::CalendarProvider::CalDav,
            "alice",
            "pw",
            true,
        )?;
        let remote_ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:r1\r\nSUMMARY:Remote\r\nDTSTART:20261110T090000\r\nDTEND:20261110T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let local_ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:l1\r\nSUMMARY:Offline Draft\r\nDTSTART:20261111T100000\r\nDTEND:20261111T110000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        assert!(store.replace_caldav_snapshot(
            sub_id,
            "https://dav.example.com/cal/",
            &[crate::ical::StoredEvent {
                subscription_id: sub_id,
                uid: "r1".into(),
                href: "/cal/r1.ics".into(),
                etag: Some("\"e1\"".into()),
                ics: remote_ics.into(),
                dirty: crate::ical::EventDirty::Clean,
            }],
            Some("tok-1"),
        )?);

        store.upsert_local_calendar_event(sub_id, "l1", "/cal/l1.ics", local_ics)?;
        // Concurrent snapshot must keep dirty local edit l1 intact.
        store.replace_caldav_snapshot(sub_id, "https://dav.example.com/cal/", &[], Some("tok-2"))?;
        let events = store.calendar_events(sub_id)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].uid, "l1");
        assert_eq!(events[0].dirty, crate::ical::EventDirty::Upsert);

        // Marking an unpushed draft deleted drops it immediately.
        assert!(store.mark_calendar_event_deleted(sub_id, "l1")?);
        assert!(store.calendar_events(sub_id)?.is_empty());
        Ok(())
    }

    #[test]
    fn batch_merge_tasks_combines_checklist_tags_and_notes() -> Result<()> {
        let store = Store::open_in_memory()?;
        let a = store.add_task("Trip prep")?;
        store.set_tags(a.id, &["travel".into()])?;
        store.set_notes(a.id, "Main note")?;
        let b = store.add_task("Pack passport")?;
        store.set_tags(b.id, &["Travel".into(), "urgent".into()])?;
        store.set_notes(b.id, "Passport note")?;
        let sub = store.add_checklist_item(b.id, "Check expiry", None)?;
        store.set_checklist_item_completed(sub.id, true)?;
        let c = store.add_task("Book taxi")?;
        store.set_completed(c.id, true)?;

        let merged_id = store.batch_merge_tasks(&[a.id, b.id, c.id])?;
        assert_eq!(merged_id, Some(a.id));

        let merged = store.task(a.id)?.unwrap();
        assert_eq!(merged.tags, vec!["travel".to_owned(), "urgent".to_owned()]);
        assert_eq!(merged.notes, "Main note\n\nPassport note");

        let items = store.checklist(a.id)?;
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].title, "Pack passport");
        assert!(!items[0].completed);
        assert_eq!(items[1].title, "Check expiry");
        assert!(items[1].completed);
        assert_eq!(items[2].title, "Book taxi");
        assert!(items[2].completed);

        assert!(store.task(b.id)?.unwrap().is_deleted());
        assert!(store.task(c.id)?.unwrap().is_deleted());
        Ok(())
    }

    #[test]
    fn batch_duplicate_and_convert_kind() -> Result<()> {
        let store = Store::open_in_memory()?;
        let a = store.add_task("Alpha")?;
        let b = store.add_task("Beta")?;

        let copies = store.batch_duplicate(&[a.id, b.id])?;
        assert_eq!(copies.len(), 2);
        assert_eq!(store.tasks()?.len(), 4);

        store.batch_set_kind(&[a.id, b.id], Kind::Note)?;
        assert_eq!(store.task(a.id)?.unwrap().kind, Kind::Note);
        assert_eq!(store.task(b.id)?.unwrap().kind, Kind::Note);

        store.batch_set_kind(&[a.id, b.id], Kind::Task)?;
        assert_eq!(store.task(a.id)?.unwrap().kind, Kind::Task);
        Ok(())
    }

    #[test]
    fn purge_empty_trash_and_delete_list_unlink_attachments_and_prune_sync_uploaded() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join("tasks.db");
        let attachments_dir = dir.path().join("attachments");
        let source = dir.path().join("doc.txt");
        std::fs::write(&source, "hello attachment")?;

        let store = Store::open(&db_path)?;
        assert_eq!(store.attachments_dir(), Some(attachments_dir.as_path()));

        // 1. purge_task unlinks both parent and subtask attachment files and prunes sync_uploaded.
        let parent = store.add_task("Parent")?;
        let child = store.add_subtask(parent.id, "Child")?;
        let att_parent = store.add_attachment(parent.id, &source, &attachments_dir)?;
        let att_child = store.add_attachment(child.id, &source, &attachments_dir)?;
        let name_parent = format!("{}-{}", att_parent.id, att_parent.name);
        let name_child = format!("{}-{}", att_child.id, att_child.name);
        store.mark_files_synced(&[name_parent, name_child])?;
        assert!(att_parent.path(&attachments_dir).exists());
        assert!(att_child.path(&attachments_dir).exists());

        store.purge_task(parent.id)?;
        assert!(!att_parent.path(&attachments_dir).exists());
        assert!(!att_child.path(&attachments_dir).exists());
        let uploaded_count: i64 = store.conn.query_row("SELECT COUNT(*) FROM sync_uploaded", [], |r| r.get(0))?;
        assert_eq!(uploaded_count, 0);

        // 2. empty_trash unlinks attachments of trashed tasks.
        let trashed = store.add_task("Trashed")?;
        let att_trashed = store.add_attachment(trashed.id, &source, &attachments_dir)?;
        let name_trashed = format!("{}-{}", att_trashed.id, att_trashed.name);
        store.mark_files_synced(std::slice::from_ref(&name_trashed))?;
        store.delete_task(trashed.id)?;
        assert!(att_trashed.path(&attachments_dir).exists(), "soft delete keeps file for restore");
        store.empty_trash()?;
        assert!(!att_trashed.path(&attachments_dir).exists());
        let uploaded_count: i64 = store.conn.query_row("SELECT COUNT(*) FROM sync_uploaded", [], |r| r.get(0))?;
        assert_eq!(uploaded_count, 0);

        // 3. delete_list unlinks attachments of tasks belonging to that list.
        let list = store.create_list("Project")?;
        let in_list = store.add_task("In list")?;
        store.set_list(in_list.id, Some(list.id))?;
        let att_list = store.add_attachment(in_list.id, &source, &attachments_dir)?;
        let name_list = format!("{}-{}", att_list.id, att_list.name);
        store.mark_files_synced(std::slice::from_ref(&name_list))?;
        store.delete_list(list.id)?;
        assert!(!att_list.path(&attachments_dir).exists());
        let uploaded_count: i64 = store.conn.query_row("SELECT COUNT(*) FROM sync_uploaded", [], |r| r.get(0))?;
        assert_eq!(uploaded_count, 0);
        Ok(())
    }

    #[test]
    fn gc_orphan_attachments_cleans_unreferenced_files_and_keeps_live_files_and_countdowns() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let attachments_dir = dir.path().join("attachments");
        let source = dir.path().join("live.txt");
        let photo = dir.path().join("bg.jpg");
        std::fs::write(&source, "live")?;
        std::fs::write(&photo, "jpg")?;

        let store = Store::open_in_memory()?.with_attachments_dir(&attachments_dir);
        let task = store.add_task("Keep")?;
        let att = store.add_attachment(task.id, &source, &attachments_dir)?;
        let countdown = store.add_countdown("Trip", d(2026, 12, 1), CountdownKind::Countdown)?;
        let bg = store.set_countdown_background(countdown.id, Some(&photo), &attachments_dir)?.unwrap();

        // Create an orphan file, a .pending staging dir, and a stale sync_uploaded entry.
        let orphan = attachments_dir.join("orphan-file.txt");
        std::fs::write(&orphan, "stale")?;
        std::fs::create_dir_all(attachments_dir.join(".pending"))?;
        std::fs::write(attachments_dir.join(".pending").join("staged.txt"), "staging")?;
        store.mark_files_synced(&[format!("{}-{}", att.id, att.name), bg.clone(), "orphan-file.txt".into()])?;

        let removed = store.gc_orphan_attachments(&attachments_dir)?;
        assert_eq!(removed, 1);
        assert!(!orphan.exists());
        assert!(att.path(&attachments_dir).exists());
        assert!(attachments_dir.join(&bg).exists());
        assert!(attachments_dir.join(".pending").join("staged.txt").exists());
        let remaining: Vec<String> = store
            .conn
            .prepare("SELECT name FROM sync_uploaded ORDER BY name")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        assert_eq!(remaining.len(), 2);
        assert!(!remaining.contains(&"orphan-file.txt".to_owned()));
        Ok(())
    }

    #[test]
    fn tag_family_traverses_multi_level_descendants_and_guards_cycles() -> Result<()> {
        let store = Store::open_in_memory()?;
        // Insert a 3-level hierarchy Work -> Backend -> Rust directly (as synced data could carry).
        store.conn.execute("INSERT INTO tags (name, parent, created_at) VALUES ('Work', NULL, 1)", [])?;
        store.conn.execute("INSERT INTO tags (name, parent, created_at) VALUES ('Backend', 'Work', 2)", [])?;
        store.conn.execute("INSERT INTO tags (name, parent, created_at) VALUES ('Rust', 'Backend', 3)", [])?;
        let task = store.add_task("Write compiler")?;
        store.set_tags(task.id, &["Rust".into()])?;

        let found = store.tasks_in(&View::Tag("Work".into()), d(2026, 10, 18))?;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, task.id);

        // Cycle guard: A -> B -> A must terminate cleanly.
        store.conn.execute("INSERT INTO tags (name, parent, created_at) VALUES ('CycleA', 'CycleB', 4)", [])?;
        store.conn.execute("INSERT INTO tags (name, parent, created_at) VALUES ('CycleB', 'CycleA', 5)", [])?;
        let fam = store.tag_family("CycleA")?;
        assert!(fam.contains("cyclea") && fam.contains("cycleb"));
        Ok(())
    }

    #[test]
    fn nested_in_tx_works_across_store_mutations_and_rolls_back() -> Result<()> {
        let store = Store::open_in_memory()?;
        let res: Result<()> = store.in_tx(|| {
            let task = store.create_task(TaskDraft { title: "Nested".into(), ..Default::default() }, true)?;
            store.set_tags(task.id, &["nested".into()])?;
            let folder = store.add_list_folder("F")?;
            store.ungroup_list_folder(folder.id)?;
            store.insert_list_section(None, "Sec", None)?;
            store.batch_duplicate(&[task.id])?;
            store.batch_set_kind(&[task.id], Kind::Note)?;
            anyhow::bail!("abort outer transaction")
        });
        assert!(res.is_err());
        assert!(store.tasks()?.is_empty());
        assert!(store.list_sections(None)?.is_empty());
        Ok(())
    }

    #[test]
    fn undo_repeat_preserves_completed_copy_if_parent_was_purged() -> Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("Daily standup")?;
        store.set_due(task.id, Some(d(2026, 10, 18)), None)?;
        store.set_repeat(task.id, Some(Recurrence::new(crate::Frequency::Daily, 1)), d(2026, 10, 18))?;
        let copy = store.complete(task.id, true)?.expect("repeating task creates completed copy");
        store.purge_task(task.id)?;
        // Undoing after parent was purged must NOT delete the completed history record.
        store.undo_repeat(task.id, copy, Some(d(2026, 10, 18)), None, None)?;
        assert!(store.task(copy)?.is_some());
        Ok(())
    }

    #[test]
    fn completing_repeating_task_shifts_start_date_and_supports_start_date_only_recurrence() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = chrono::Local::now().date_naive();
        let tomorrow = today + Duration::days(1);

        // (a) Repeating task with both start_date and due_date shifts both and leaves Things Today.
        let both = store.create_task(
            TaskDraft {
                title: "Daily review".into(),
                due_date: Some(today),
                start_date: Some(today),
                schedule_bucket: ScheduleBucket::Evening,
                ..Default::default()
            },
            true,
        )?;
        store.set_repeat(both.id, Some(Recurrence::new(crate::Frequency::Daily, 1)), today)?;
        assert!(
            store
                .things_tasks(&crate::things::ThingsViewKind::Today, today)?
                .iter()
                .any(|t| t.id == both.id)
        );
        let copy_a = store.complete(both.id, true)?.expect("must create completed occurrence");
        let after_a = store.task(both.id)?.unwrap();
        assert_eq!(after_a.due_date, Some(tomorrow));
        assert_eq!(after_a.start_date, Some(tomorrow));
        assert_eq!(after_a.schedule_bucket, ScheduleBucket::Evening);
        assert!(
            !store
                .things_tasks(&crate::things::ThingsViewKind::Today, today)?
                .iter()
                .any(|t| t.id == both.id),
            "repeating task must leave Today after completion"
        );
        assert!(
            store
                .things_tasks(&crate::things::ThingsViewKind::Upcoming, today)?
                .iter()
                .any(|t| t.id == both.id)
        );
        // Undo restores start_date, due_date, and schedule_bucket.
        store.undo_repeat(both.id, copy_a, Some(today), None, None)?;
        let undone_a = store.task(both.id)?.unwrap();
        assert_eq!(undone_a.due_date, Some(today));
        assert_eq!(undone_a.start_date, Some(today));
        assert_eq!(undone_a.schedule_bucket, ScheduleBucket::Evening);
        assert!(store.task(copy_a)?.is_none());

        // (b) Repeating task with start_date only (due_date = None) repeats instead of closing.
        let start_only = store.create_task(
            TaskDraft {
                title: "Morning stretch".into(),
                start_date: Some(today),
                repeat: Some(Recurrence::new(crate::Frequency::Daily, 1)),
                ..Default::default()
            },
            true,
        )?;
        let copy_b = store
            .complete(start_only.id, true)?
            .expect("start_date-only repeating task must repeat");
        let after_b = store.task(start_only.id)?.unwrap();
        assert!(!after_b.is_completed());
        assert_eq!(after_b.due_date, None);
        assert_eq!(after_b.start_date, Some(tomorrow));
        let copy_b_row = store.task(copy_b)?.unwrap();
        assert!(copy_b_row.is_completed());
        assert_eq!(copy_b_row.start_date, Some(today));
        store.undo_repeat(start_only.id, copy_b, None, None, None)?;
        let undone_b = store.task(start_only.id)?.unwrap();
        assert_eq!(undone_b.start_date, Some(today));
        assert_eq!(undone_b.due_date, None);

        // (c) Due-date-only Evening repeating task clears Evening on advance and restores on undo.
        let due_evening = store.create_task(
            TaskDraft {
                title: "Evening meds".into(),
                due_date: Some(today),
                schedule_bucket: ScheduleBucket::Evening,
                repeat: Some(Recurrence::new(crate::Frequency::Daily, 1)),
                ..Default::default()
            },
            true,
        )?;
        let copy_c = store.complete(due_evening.id, true)?.expect("must repeat");
        let after_c = store.task(due_evening.id)?.unwrap();
        assert_eq!(after_c.due_date, Some(tomorrow));
        assert_eq!(after_c.schedule_bucket, ScheduleBucket::None);
        assert!(
            !store
                .things_tasks(&crate::things::ThingsViewKind::Today, today)?
                .iter()
                .any(|t| t.id == due_evening.id)
        );
        store.undo_repeat(due_evening.id, copy_c, Some(today), None, None)?;
        let undone_c = store.task(due_evening.id)?.unwrap();
        assert_eq!(undone_c.due_date, Some(today));
        assert_eq!(undone_c.schedule_bucket, ScheduleBucket::Evening);
        Ok(())
    }

    #[test]
    fn hidden_lists_excludes_completed_projects_from_smart_lists() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = d(2026, 11, 22);
        let project = store.add_list(&ListDraft { name: "Ship v2".into(), ..Default::default() })?;
        let task = store.create_task(
            TaskDraft { title: "Polish docs".into(), list_id: Some(project.id), due_date: Some(today), ..Default::default() },
            true,
        )?;
        assert!(store.tasks_in(&View::All, today)?.iter().any(|t| t.id == task.id));
        assert!(store.tasks_in(&View::Today, today)?.iter().any(|t| t.id == task.id));

        store.set_list_project_fields(project.id, "", None, Some(123_456))?;
        assert!(!store.tasks_in(&View::All, today)?.iter().any(|t| t.id == task.id));
        assert!(!store.tasks_in(&View::Today, today)?.iter().any(|t| t.id == task.id));
        // Direct list view still displays tasks inside the completed project.
        assert!(store.tasks_in(&View::List(project.id), today)?.iter().any(|t| t.id == task.id));
        Ok(())
    }

    #[test]
    fn concurrent_store_open_migrates_fresh_db_without_racing() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-migrate-race-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join("tasks.db");

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let path = db_path.clone();
            let b = std::sync::Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || -> Result<(usize, bool)> {
                b.wait();
                let store = Store::open(&path)?;
                let ver = store.schema_version()?;
                let fk: bool = store.conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
                Ok((ver, fk))
            }));
        }
        for handle in handles {
            let (ver, fk) = handle.join().expect("thread must not panic")?;
            assert_eq!(ver, SCHEMA_VERSION);
            assert!(fk, "foreign_keys must remain enabled after concurrent migrations");
        }
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
