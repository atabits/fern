use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use rusqlite::{Connection, OptionalExtension, Row, params};
use uuid::Uuid;

use crate::countdown::{Countdown, CountdownKind};
use crate::focus::FocusSession;
use crate::habit::{Checkin, Goal, Habit};
use crate::model::{Attachment, ChecklistItem, Id, Kind, List, Priority, Tag, Task, View, ViewCounts};
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
];

const TASK_COLUMNS: &str = "id, parent_id, kind, title, notes, priority, due_date, due_time, \
                            completed_at, created_at, list_id, deleted_at, repeat, wont_do_at, reminders, end_date, end_time, modified_at, \
                            pinned_at, section_id, estimate";

const DATE_FORMAT: &str = "%Y-%m-%d";
const TIME_FORMAT: &str = "%H:%M";
const SNOOZE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

pub struct Store {
    pub(crate) conn: Connection,
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
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    fn migrate(conn: &Connection) -> Result<()> {
        let version: usize = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        anyhow::ensure!(version <= MIGRATIONS.len(), "the data comes from a newer version of Fern");
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            conn.execute_batch(sql)?;
            conn.pragma_update(None, "user_version", i + 1)?;
        }
        Ok(())
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
                let mut stmt = self.conn.prepare("SELECT name, data FROM backup_files")?;
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
        if let View::Filter(id) = view {
            let filter = self.filters()?.into_iter().find(|f| f.id == *id).context("filter not found")?;
            return Ok(self.tasks()?.into_iter().filter(|task| filter.matches(task, today)).collect());
        }
        Ok(self.tasks()?.into_iter().filter(|task| matches_view(task, view, today)).collect())
    }

    /// Saved filters, oldest first.
    pub fn filters(&self) -> Result<Vec<crate::filter::SmartFilter>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, lists, tags, date, priorities, keywords, kind, match_any, chain FROM filters ORDER BY created_at, rowid",
        )?;
        let filters = stmt
            .query_map([], |row| {
                let lists: String = row.get(2)?;
                let tags: String = row.get(3)?;
                let priorities: String = row.get(5)?;
                Ok(crate::filter::SmartFilter {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    lists: lists
                        .split(',')
                        .filter(|l| !l.is_empty())
                        .map(|l| if l == "inbox" { None } else { Uuid::parse_str(l).ok() })
                        .collect(),
                    tags: tags.lines().filter(|t| !t.is_empty()).map(str::to_owned).collect(),
                    date: crate::filter::DateRule::from_key(&row.get::<_, String>(4)?),
                    priorities: priorities.split(',').filter_map(|p| p.parse().ok()).map(Priority::from_i64).collect(),
                    keywords: row.get(6)?,
                    kind: row.get::<_, Option<String>>(7)?.map(|k| Kind::parse(&k)),
                    match_any: row.get(8)?,
                    chain: crate::filter::chain_from_storage(&row.get::<_, String>(9)?),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(filters)
    }

    /// Adds a filter, or replaces the one with the same id.
    pub fn save_filter(&self, filter: &crate::filter::SmartFilter) -> Result<()> {
        anyhow::ensure!(!filter.name.trim().is_empty(), "a filter needs a name");
        let lists: Vec<String> =
            filter.lists.iter().map(|l| l.map_or_else(|| "inbox".to_owned(), |id| id.to_string())).collect();
        let priorities: Vec<String> = filter.priorities.iter().map(|p| p.to_i64().to_string()).collect();
        self.conn.execute(
            "INSERT INTO filters (id, name, lists, tags, date, priorities, keywords, kind, created_at, match_any, chain)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, lists = excluded.lists, tags = excluded.tags,
                 date = excluded.date, priorities = excluded.priorities, keywords = excluded.keywords,
                 kind = excluded.kind, match_any = excluded.match_any, chain = excluded.chain",
            params![
                filter.id.to_string(),
                filter.name.trim(),
                lists.join(","),
                filter.tags.join("\n"),
                filter.date.key(),
                priorities.join(","),
                filter.keywords.trim(),
                filter.kind.map(|k| k.as_str()),
                now_ms(),
                filter.match_any,
                crate::filter::chain_to_storage(&filter.chain)
            ],
        )?;
        Ok(())
    }

    pub fn delete_filter(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM filters WHERE id = ?1", [id.to_string()])?;
        Ok(())
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
        self.add_task_at(title, true)
    }

    /// Adds a task at the top of the lists, or at the bottom ("Default Add to" in the settings).
    pub fn add_task_at(&self, title: &str, top: bool) -> Result<Task> {
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
            modified_at: None,
            pinned_at: None,
            section_id: None,
            reminders: Vec::new(),
            estimate: None,
            created_at: now_ms(),
        };
        let order = if top {
            "(SELECT COALESCE(MIN(sort_order), 0) - 1024 FROM tasks)"
        } else {
            "(SELECT COALESCE(MAX(sort_order), 0) + 1024 FROM tasks)"
        };
        self.conn.execute(
            &format!("INSERT INTO tasks (id, title, created_at, sort_order) VALUES (?1, ?2, ?3, {order})"),
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
        self.complete(id, completed).map(|_| ())
    }

    /// Completes or reopens a task. Completing a repeating task keeps a completed copy and moves
    /// the task to its next date; the copy is returned so that it can be undone.
    pub fn complete(&self, id: Id, completed: bool) -> Result<Option<Id>> {
        if completed
            && let Some(task) = self.task(id)?
            && let (Some(repeat), Some(due)) = (task.repeat.clone(), task.due_date)
            && let Some(next) = repeat.next_for(due, chrono::Local::now().date_naive())
        {
            let tx = self.conn.unchecked_transaction()?;
            let copy = Uuid::now_v7();
            tx.execute(
                &format!(
                    "INSERT INTO tasks ({TASK_COLUMNS})
                     SELECT ?2, parent_id, kind, title, notes, priority, due_date, due_time,
                            ?3, created_at, list_id, NULL, NULL, NULL, NULL, end_date, end_time, ?3, NULL,
                            section_id, estimate
                     FROM tasks WHERE id = ?1"
                ),
                params![id.to_string(), copy.to_string(), now_ms()],
            )?;
            tx.execute(
                "INSERT INTO task_tags (task_id, tag) SELECT ?2, tag FROM task_tags WHERE task_id = ?1",
                params![id.to_string(), copy.to_string()],
            )?;
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
            return Ok(Some(copy));
        }
        self.update(id, "completed_at", completed.then(now_ms)).map(|_| None)
    }

    /// Undoes completing a repeating task: removes the completed copy and puts the task back on
    /// its earlier dates.
    pub fn undo_repeat(&self, id: Id, copy: Id, due: Option<NaiveDate>, end: Option<NaiveDate>) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM tasks WHERE id = ?1", [copy.to_string()])?;
        tx.execute(
            "UPDATE tasks SET due_date = ?2, end_date = ?3 WHERE id = ?1",
            params![
                id.to_string(),
                due.map(|d| d.format(DATE_FORMAT).to_string()),
                end.map(|d| d.format(DATE_FORMAT).to_string())
            ],
        )?;
        tx.commit()?;
        Ok(())
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

    /// Moves a task to start on `date`, keeping its time and the length of its duration: a bar
    /// dragged in the Timeline.
    pub fn move_dates(&self, id: Id, date: NaiveDate) -> Result<()> {
        let task = self.task(id)?.context("task not found")?;
        let shift = task.due_date.map(|due| date - due);
        let end = match (task.end_date, shift) {
            (Some(end), Some(shift)) => Some((end + shift, task.end_time)),
            _ => None,
        };
        self.set_due(id, Some(date), task.due_time)?;
        if task.end_date.is_some() {
            self.set_due_end(id, end)?;
        }
        Ok(())
    }

    /// "Save as Template": keeps a task's title, notes, priority, tags and checklist.
    pub fn save_template(&self, task: Id) -> Result<crate::model::Template> {
        let task = self.task(task)?.context("task not found")?;
        let checklist: Vec<String> = self.checklist(task.id)?.into_iter().map(|item| item.title).collect();
        let template = crate::model::Template {
            id: Uuid::now_v7(),
            title: task.title.clone(),
            notes: task.notes.clone(),
            priority: task.priority,
            tags: task.tags.clone(),
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
            .prepare("SELECT id, title, notes, priority, tags, checklist FROM templates ORDER BY created_at DESC, rowid DESC")?;
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
    pub fn list_sections(&self, list: Id) -> Result<Vec<crate::model::ListSection>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, list_id, name FROM list_sections WHERE list_id = ?1 ORDER BY created_at, rowid",
        )?;
        let sections = stmt
            .query_map([list.to_string()], |row| {
                Ok(crate::model::ListSection {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    list_id: parse_uuid(&row.get::<_, String>(1)?, 1)?,
                    name: row.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(sections)
    }

    pub fn add_list_section(&self, list: Id, name: &str) -> Result<crate::model::ListSection> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a section needs a name");
        let section = crate::model::ListSection { id: Uuid::now_v7(), list_id: list, name: name.to_owned() };
        self.conn.execute(
            "INSERT INTO list_sections (id, list_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![section.id.to_string(), list.to_string(), section.name, now_ms()],
        )?;
        Ok(section)
    }

    pub fn rename_list_section(&self, id: Id, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a section needs a name");
        self.conn.execute("UPDATE list_sections SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(())
    }

    /// Deletes a section; its tasks stay in the list without a section.
    pub fn delete_list_section(&self, id: Id) -> Result<()> {
        self.conn.execute("UPDATE tasks SET section_id = NULL WHERE section_id = ?1", [id.to_string()])?;
        self.conn.execute("DELETE FROM list_sections WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    /// Puts a task (and its subtasks) into a section of its list, or out of any.
    pub fn set_section(&self, id: Id, section: Option<Id>) -> Result<()> {
        if let Some(section) = section {
            let list: Option<String> = self
                .conn
                .query_row("SELECT list_id FROM list_sections WHERE id = ?1", [section.to_string()], |row| row.get(0))
                .optional()?;
            let task_list = self.task(id)?.context("task not found")?.list_id.map(|l| l.to_string());
            anyhow::ensure!(list.is_some() && list == task_list, "the section belongs to another list");
        }
        let section = section.map(|s| s.to_string());
        for task in std::iter::once(id).chain(self.descendants(id)?) {
            self.conn.execute("UPDATE tasks SET section_id = ?2 WHERE id = ?1", params![task.to_string(), section])?;
        }
        Ok(())
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
            "UPDATE tasks SET due_date = ?2, due_time = ?3 WHERE id = ?1",
            params![id.to_string(), date_str, time_str],
        )?;
        Ok(())
    }

    /// Makes the task a subtask of `parent`, or a top-level task for `None`.
    /// "Link Parent Task": makes a task a subtask of `parent`, moving it to the parent's list.
    pub fn link_parent(&self, id: Id, parent: Id) -> Result<()> {
        let list = self.task(parent)?.context("parent task not found")?.list_id;
        self.set_parent(id, Some(parent))?;
        self.set_list(id, list)
    }

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
            // A section belongs to its list, so moving to another list leaves it.
            tx.execute(
                "UPDATE tasks SET list_id = ?2, section_id = NULL WHERE id = ?1",
                params![task.to_string(), list],
            )?;
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
            self.conn.prepare("SELECT id, name, date, kind, icon, color, repeat, pinned_at FROM countdowns
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
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn add_countdown(&self, name: &str, date: NaiveDate, kind: CountdownKind) -> Result<Countdown> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a countdown needs a name");
        let countdown = Countdown { id: Uuid::now_v7(), name: name.to_owned(), date, kind, icon: None, color: None, repeat: None, pinned_at: None };
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

    /// A countdown's icon (an emoji or short text) and its background color; empty clears.
    pub fn set_countdown_look(&self, id: Id, icon: Option<&str>, color: Option<&str>) -> Result<()> {
        let icon = icon.map(str::trim).filter(|i| !i.is_empty());
        self.conn.execute(
            "UPDATE countdowns SET icon = ?2, color = ?3 WHERE id = ?1",
            params![id.to_string(), icon, color],
        )?;
        Ok(())
    }

    /// "Upload Attachment": copies a file into `dir` (the app's attachment folder) as
    /// `<id>-<name>` and lists it on the task.
    pub fn add_attachment(&self, task: Id, file: &std::path::Path, dir: &std::path::Path) -> Result<Attachment> {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .context("the file has no name")?
            .to_owned();
        let id = Uuid::now_v7();
        std::fs::create_dir_all(dir)?;
        let size = std::fs::copy(file, dir.join(format!("{id}-{name}")))?;
        let attachment = Attachment { id, task_id: task, name, size, created_at: now_ms() };
        self.conn.execute(
            "INSERT INTO task_attachments (id, task_id, name, size, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id.to_string(), task.to_string(), attachment.name, size as i64, attachment.created_at],
        )?;
        Ok(attachment)
    }

    /// A task's attachments, oldest first.
    pub fn attachments(&self, task: Id) -> Result<Vec<Attachment>> {
        let mut stmt = self.conn.prepare(
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
        self.conn.execute("DELETE FROM task_attachments WHERE id = ?1", [attachment.id.to_string()])?;
        let file = attachment.path(dir);
        if file.exists() {
            std::fs::remove_file(file)?;
        }
        Ok(())
    }

    pub fn delete_countdown(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM countdowns WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    pub fn habits(&self) -> Result<Vec<Habit>> {
        let mut stmt =
            self.conn.prepare("SELECT id, name, goal_amount, goal_unit, reminders, frequency, icon, color, start_date, goal_days, section, archived_at, icon_checkins FROM habits ORDER BY created_at, rowid")?;
        let habits = stmt
            .query_map([], |row| {
                let goal = row.get::<_, Option<u32>>(2)?.map(|amount| Goal { amount, unit: row.get::<_, Option<String>>(3).ok().flatten().unwrap_or_default() });
                let reminders = row
                    .get::<_, Option<String>>(4)?
                    .map(|times| {
                        times.split(',').filter_map(|t| NaiveTime::parse_from_str(t.trim(), TIME_FORMAT).ok()).collect()
                    })
                    .unwrap_or_default();
                let frequency = row
                    .get::<_, Option<String>>(5)?
                    .map(|f| crate::habit::Frequency::from_storage(&f))
                    .unwrap_or_default();
                Ok(Habit {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    goal,
                    reminders,
                    frequency,
                    icon: row.get(6)?,
                    color: row.get(7)?,
                    start_date: row
                        .get::<_, Option<String>>(8)?
                        .and_then(|d| NaiveDate::parse_from_str(&d, DATE_FORMAT).ok()),
                    goal_days: row.get(9)?,
                    section: row
                        .get::<_, Option<String>>(10)?
                        .map(|s| crate::habit::HabitSection::from_key(&s))
                        .unwrap_or_default(),
                    archived: row.get::<_, Option<i64>>(11)?.is_some(),
                    icon_checkins: row.get(12)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(habits)
    }

    pub fn add_habit(&self, name: &str) -> Result<Habit> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a habit needs a name");
        let habit = Habit {
            id: Uuid::now_v7(),
            name: name.to_owned(),
            goal: None,
            reminders: Vec::new(),
            frequency: Default::default(),
            icon: None,
            color: None,
            start_date: None,
            goal_days: None,
            section: Default::default(),
            archived: false,
            icon_checkins: false,
        };
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

    pub fn set_habit_section(&self, id: Id, section: crate::habit::HabitSection) -> Result<()> {
        self.conn.execute("UPDATE habits SET section = ?2 WHERE id = ?1", params![id.to_string(), section.key()])?;
        Ok(())
    }

    /// Sections the user added, oldest first.
    pub fn habit_sections(&self) -> Result<Vec<crate::habit::CustomSection>> {
        let mut stmt = self.conn.prepare("SELECT id, name FROM habit_sections ORDER BY created_at, rowid")?;
        let sections = stmt
            .query_map([], |row| {
                Ok(crate::habit::CustomSection { id: parse_uuid(&row.get::<_, String>(0)?, 0)?, name: row.get(1)? })
            })?
            .collect::<Result<_, _>>()?;
        Ok(sections)
    }

    pub fn add_habit_section(&self, name: &str) -> Result<crate::habit::CustomSection> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a section needs a name");
        let section = crate::habit::CustomSection { id: Uuid::now_v7(), name: name.to_owned() };
        self.conn.execute(
            "INSERT INTO habit_sections (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![section.id.to_string(), section.name, now_ms()],
        )?;
        Ok(section)
    }

    pub fn rename_habit_section(&self, id: Id, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a section needs a name");
        self.conn.execute("UPDATE habit_sections SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(())
    }

    /// Deletes a section; its habits move to Others.
    pub fn delete_habit_section(&self, id: Id) -> Result<()> {
        let key = crate::habit::HabitSection::Custom(id).key();
        self.conn.execute("UPDATE habits SET section = NULL WHERE section = ?1", [key])?;
        self.conn.execute("DELETE FROM habit_sections WHERE id = ?1", [id.to_string()])?;
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
            params![id.to_string(), frequency.to_storage()],
        )?;
        Ok(())
    }

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
        let mut stmt = self.conn.prepare("SELECT habit_id, at FROM habit_snoozes ORDER BY at")?;
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

    /// "+1" in a goal habit's reminder: adds to the day's amount, past the goal too, unlike
    /// [`Self::step_checkin`] which starts a completed day over. Returns the new amount.
    pub fn add_checkin_amount(&self, habit: Id, date: NaiveDate, delta: u32) -> Result<u32> {
        let date = date.format(DATE_FORMAT).to_string();
        self.conn.execute(
            "INSERT INTO habit_checkins (habit_id, date, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(habit_id, date) DO UPDATE SET value = value + excluded.value",
            params![habit.to_string(), date, delta],
        )?;
        Ok(self.conn.query_row(
            "SELECT value FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
            params![habit.to_string(), date],
            |row| row.get(0),
        )?)
    }

    /// Writes the note of an existing check-in; an empty note removes it.
    /// Records an amount for a goal habit's day, typed rather than stepped; 0 clears the day.
    pub fn set_checkin_amount(&self, habit: Id, date: NaiveDate, amount: u32) -> Result<()> {
        let date = date.format(DATE_FORMAT).to_string();
        if amount == 0 {
            self.conn.execute(
                "DELETE FROM habit_checkins WHERE habit_id = ?1 AND date = ?2",
                params![habit.to_string(), date],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO habit_checkins (habit_id, date, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(habit_id, date) DO UPDATE SET value = excluded.value",
                params![habit.to_string(), date, amount],
            )?;
        }
        Ok(())
    }

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
        self.add_focus_session_as(task_id, title, started_at, ended_at, false)
    }

    /// A focus session, timed as a Pomodoro or with the stopwatch.
    pub fn add_focus_session_as(
        &self,
        task_id: Option<Id>,
        title: &str,
        started_at: i64,
        ended_at: i64,
        stopwatch: bool,
    ) -> Result<FocusSession> {
        anyhow::ensure!(ended_at >= started_at, "a focus session cannot end before it starts");
        let session =
            FocusSession { id: Uuid::now_v7(), task_id, title: title.trim().to_owned(), started_at, ended_at, stopwatch, note: String::new() };
        self.conn.execute(
            "INSERT INTO focus_sessions (id, task_id, title, started_at, ended_at, stopwatch) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session.id.to_string(), task_id.map(|t| t.to_string()), session.title, started_at, ended_at, stopwatch],
        )?;
        Ok(session)
    }

    /// "List Activities": what happened to the tasks of a list (`None` is the inbox), newest
    /// first, with each task's id and title.
    pub fn list_activity(&self, list: Option<Id>, limit: usize) -> Result<Vec<(crate::model::Activity, Id, String)>> {
        let mut stmt = self.conn.prepare(
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
            self.conn.prepare("SELECT at, kind FROM task_activity WHERE task_id = ?1 ORDER BY at, rowid")?;
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
        let mut stmt = self.conn.prepare(
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
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM focus_sessions WHERE id = ?1", [id.to_string()])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The Focus Record's "Delete All".
    pub fn delete_all_focus_sessions(&self) -> Result<()> {
        self.conn.execute("DELETE FROM focus_sessions", [])?;
        Ok(())
    }

    /// Focus sessions, newest first.
    pub fn focus_sessions(&self) -> Result<Vec<FocusSession>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, title, started_at, ended_at, stopwatch, note FROM focus_sessions ORDER BY started_at DESC",
        )?;
        let sessions = stmt
            .query_map([], |row| {
                Ok(FocusSession {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    task_id: row.get::<_, Option<String>>(1)?.map(|t| parse_uuid(&t, 1)).transpose()?,
                    title: row.get(2)?,
                    started_at: row.get(3)?,
                    ended_at: row.get(4)?,
                    stopwatch: row.get(5)?,
                    note: row.get(6)?,
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

    /// Subtasks of a task, at any depth.
    pub fn descendants(&self, id: Id) -> Result<Vec<Id>> {
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
        // Filters are matched in `Store::tasks_in`, which knows their conditions.
        View::Filter(_) | View::Summary => false,
        View::Day(day) => task.due_date == Some(*day),
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
        modified_at: row.get(17)?,
        pinned_at: row.get(18)?,
        section_id: row.get::<_, Option<String>>(19)?.map(|s| parse_uuid(&s, 19)).transpose()?,
        estimate: row.get::<_, Option<String>>(20)?.and_then(|e| crate::model::Estimate::from_storage(&e)),
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
    fn filters_are_stored_and_listed() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = d(2026, 10, 18);
        let work = store.create_list("Work")?;
        let report = store.add_task("report")?;
        store.set_list(report.id, Some(work.id))?;
        store.set_priority(report.id, Priority::High)?;
        store.add_task("milk")?;
        let mut filter = crate::filter::SmartFilter {
            id: Uuid::now_v7(),
            name: "Urgent work".into(),
            lists: vec![Some(work.id), None],
            priorities: vec![Priority::High],
            tags: vec!["a".into(), "b c".into()],
            kind: Some(Kind::Task),
            ..Default::default()
        };
        assert!(store.save_filter(&crate::filter::SmartFilter { name: " ".into(), ..filter.clone() }).is_err());
        store.save_filter(&filter)?;
        assert_eq!(store.filters()?, [filter.clone()]);
        filter.tags.clear();
        filter.name = "Urgent".into();
        store.save_filter(&filter)?;
        assert_eq!(store.filters()?, [filter.clone()]);
        let any = crate::filter::SmartFilter { id: Uuid::now_v7(), name: "Any".into(), match_any: true, ..filter.clone() };
        store.save_filter(&any)?;
        assert!(store.filters()?[1].match_any);
        let chained = crate::filter::SmartFilter {
            id: Uuid::now_v7(),
            name: "Chain".into(),
            chain: crate::filter::chain_from_storage("lists or:tags!"),
            ..filter.clone()
        };
        store.save_filter(&chained)?;
        assert_eq!(store.filters()?[2].chain, chained.chain);
        store.delete_filter(chained.id)?;
        store.delete_filter(any.id)?;
        assert_eq!(titles(&store.tasks_in(&View::Filter(filter.id), today)?), ["report"]);
        store.delete_filter(filter.id)?;
        assert!(store.filters()?.is_empty());
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
        assert_eq!(store.templates()?, [template.clone()]);

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
    fn list_sections() -> Result<()> {
        let store = Store::open_in_memory()?;
        let home = store.create_list("Home")?;
        let work = store.create_list("Work")?;
        let kitchen = store.add_list_section(home.id, " Kitchen ")?;
        assert!(store.add_list_section(home.id, " ").is_err());
        assert_eq!(store.list_sections(home.id)?, [kitchen.clone()]);
        assert!(store.list_sections(work.id)?.is_empty());

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
        assert_eq!(store.list_sections(home.id)?[0].name, "Kitchen & pantry");
        store.set_list(task.id, Some(home.id))?;
        store.set_section(task.id, Some(kitchen.id))?;
        store.delete_list_section(kitchen.id)?;
        assert_eq!(store.task(task.id)?.unwrap().section_id, None);
        assert!(store.list_sections(home.id)?.is_empty());
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
        store.undo_repeat(task.id, copy, Some(d(2026, 10, 18)), None)?;
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
        store.set_habit_archived(water.id, false)?;
        assert!(!store.habits()?[0].archived);

        assert!(store.add_habit_section(" ").is_err());
        let pets = store.add_habit_section(" Pets ")?;
        assert_eq!(store.habit_sections()?, [pets.clone()]);
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
        store.update_habit(water.id, "Water", Some(&crate::habit::Goal { amount: 8, unit: "glasses".into() }))?;
        let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        store.set_checkin_amount(water.id, day, 5)?;
        assert_eq!(store.step_checkin(water.id, day)?, 6);
        store.set_checkin_amount(water.id, day, 0)?;
        assert_eq!(store.step_checkin(water.id, day)?, 1);
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
        assert_eq!(store.attachments(task.id)?, [attachment.clone()]);
        store.delete_attachment(&attachment, &folder)?;
        assert!(store.attachments(task.id)?.is_empty());
        assert!(!attachment.path(&folder).exists());
        std::fs::remove_dir_all(&dir)?;
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
        Ok(())
    }

    #[test]
    fn reminder_plus_one_and_snooze() -> Result<()> {
        let store = Store::open_in_memory()?;
        let water = store.add_habit("Water")?;
        store.update_habit(water.id, "Water", Some(&crate::habit::Goal { amount: 2, unit: "glasses".into() }))?;
        let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        assert_eq!(store.add_checkin_amount(water.id, day, 1)?, 1);
        assert_eq!(store.add_checkin_amount(water.id, day, 1)?, 2);
        assert_eq!(store.add_checkin_amount(water.id, day, 1)?, 3, "+1 goes past the goal");

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
        assert_eq!(other.attachments(task.id)?, [attachment.clone()]);
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
}
