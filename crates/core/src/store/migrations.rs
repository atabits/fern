#[cfg(test)]
use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, Transaction, TransactionBehavior};

pub(super) const MIGRATIONS: &[&str] = &[
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
    // Ensure tag, checklist, reminder, and parent edits emit `task.updated` into `plugin_outbox`
    // (and checklist deletions refresh `tasks.modified_at`).
    "
    DROP TRIGGER IF EXISTS outbox_tasks_updated;
    DROP TRIGGER IF EXISTS outbox_task_tags_insert;
    DROP TRIGGER IF EXISTS outbox_task_tags_update;
    DROP TRIGGER IF EXISTS outbox_task_tags_delete;
    DROP TRIGGER IF EXISTS outbox_checklist_insert;
    DROP TRIGGER IF EXISTS outbox_checklist_update;
    DROP TRIGGER IF EXISTS outbox_checklist_delete;
    DROP TRIGGER IF EXISTS checklist_deleted;

    CREATE TRIGGER checklist_deleted AFTER DELETE ON checklist_items
    WHEN EXISTS (SELECT 1 FROM tasks WHERE id = OLD.task_id)
    BEGIN
        UPDATE tasks SET modified_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = OLD.task_id;
    END;

    CREATE TRIGGER outbox_tasks_updated AFTER UPDATE OF
        title, notes, priority, due_date, due_time, end_date, end_time,
        list_id, section_id, parent_id, repeat, reminders, wont_do_at, start_date, schedule_bucket ON tasks
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
            OR OLD.parent_id IS NOT NEW.parent_id
            OR OLD.repeat IS NOT NEW.repeat
            OR OLD.reminders IS NOT NEW.reminders
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

    CREATE TRIGGER outbox_task_tags_insert AFTER INSERT ON task_tags
    WHEN EXISTS (SELECT 1 FROM tasks WHERE id = NEW.task_id AND deleted_at IS NULL)
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.updated',
            NEW.task_id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;

    CREATE TRIGGER outbox_task_tags_update AFTER UPDATE OF tag ON task_tags
    WHEN OLD.tag IS NOT NEW.tag
        AND EXISTS (SELECT 1 FROM tasks WHERE id = NEW.task_id AND deleted_at IS NULL)
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.updated',
            NEW.task_id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;

    CREATE TRIGGER outbox_task_tags_delete AFTER DELETE ON task_tags
    WHEN EXISTS (SELECT 1 FROM tasks WHERE id = OLD.task_id AND deleted_at IS NULL)
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.updated',
            OLD.task_id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;

    CREATE TRIGGER outbox_checklist_insert AFTER INSERT ON checklist_items
    WHEN EXISTS (SELECT 1 FROM tasks WHERE id = NEW.task_id AND deleted_at IS NULL)
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.updated',
            NEW.task_id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;

    CREATE TRIGGER outbox_checklist_update AFTER UPDATE OF title, completed_at, sort_order ON checklist_items
    WHEN (
            OLD.title IS NOT NEW.title
            OR OLD.completed_at IS NOT NEW.completed_at
            OR OLD.sort_order IS NOT NEW.sort_order
        )
        AND EXISTS (SELECT 1 FROM tasks WHERE id = NEW.task_id AND deleted_at IS NULL)
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.updated',
            NEW.task_id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;

    CREATE TRIGGER outbox_checklist_delete AFTER DELETE ON checklist_items
    WHEN EXISTS (SELECT 1 FROM tasks WHERE id = OLD.task_id AND deleted_at IS NULL)
        AND NOT EXISTS (
            SELECT 1 FROM settings
            WHERE (key = 'plugins_safe_mode' AND value = '1')
               OR (key = 'plugins_outbox_paused' AND value = '1')
        )
    BEGIN
        INSERT INTO plugin_outbox (event_kind, entity_id, origin_plugin, depth, at)
        VALUES (
            'task.updated',
            OLD.task_id,
            CASE WHEN EXISTS (SELECT 1 FROM sync_applying) THEN '__sync__' ELSE (SELECT plugin_id FROM plugin_write_ctx LIMIT 1) END,
            COALESCE((SELECT depth FROM plugin_write_ctx LIMIT 1), 0),
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
        );
    END;
    ",
    // Horizon planning bucket column, index, modified_at trigger, and plugin outbox trigger update.
    "
    ALTER TABLE tasks ADD COLUMN horizon TEXT;
    CREATE INDEX IF NOT EXISTS idx_tasks_horizon ON tasks(horizon) WHERE horizon IS NOT NULL;
    CREATE TRIGGER tasks_modified_horizon AFTER UPDATE OF horizon ON tasks
    WHEN OLD.horizon IS NOT NEW.horizon
    BEGIN
        UPDATE tasks SET modified_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = NEW.id;
    END;
    DROP TRIGGER IF EXISTS outbox_tasks_updated;
    CREATE TRIGGER outbox_tasks_updated AFTER UPDATE OF
        title, notes, priority, due_date, due_time, end_date, end_time,
        list_id, section_id, parent_id, repeat, reminders, wont_do_at, start_date, schedule_bucket, horizon ON tasks
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
            OR OLD.parent_id IS NOT NEW.parent_id
            OR OLD.repeat IS NOT NEW.repeat
            OR OLD.reminders IS NOT NEW.reminders
            OR OLD.wont_do_at IS NOT NEW.wont_do_at
            OR OLD.start_date IS NOT NEW.start_date
            OR OLD.schedule_bucket IS NOT NEW.schedule_bucket
            OR OLD.horizon IS NOT NEW.horizon
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
    ",
    // Targeted indexes for open dated tasks, focus session ordering/FKs, and task attachments
    // (habit_checkins already has PRIMARY KEY (habit_id, date)).
    "
    CREATE INDEX IF NOT EXISTS idx_tasks_open_due ON tasks(due_date)
        WHERE deleted_at IS NULL AND completed_at IS NULL AND due_date IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_focus_sessions_started ON focus_sessions(started_at DESC);
    CREATE INDEX IF NOT EXISTS idx_focus_sessions_task ON focus_sessions(task_id);
    CREATE INDEX IF NOT EXISTS idx_task_attachments_task ON task_attachments(task_id, created_at);
    ",
    // Cover dated queries across completed/open tasks and missing FK indexes for habits/focus.
    "
    CREATE INDEX IF NOT EXISTS idx_tasks_due_date ON tasks(due_date)
        WHERE deleted_at IS NULL AND due_date IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_focus_sessions_habit ON focus_sessions(habit_id)
        WHERE habit_id IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_habit_snoozes_habit ON habit_snoozes(habit_id);
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

pub(super) fn retry_sqlite_busy<T>(mut op: impl FnMut() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
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
    let mut version: usize = usize::try_from(retry_sqlite_busy(|| {
        conn.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
    })?)
    .unwrap_or(0);
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
                let current: usize = usize::try_from(
                    tx.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))?,
                )
                .unwrap_or(0);
                anyhow::ensure!(
                    current <= MIGRATIONS.len(),
                    "the data comes from a newer version of Fern"
                );
                let outcome = if current <= i {
                    tx.execute_batch(sql)?;
                    tx.pragma_update(None, "user_version", (i + 1) as i64)?;
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
            let mut current: usize = usize::try_from(
                tx.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))?,
            )
            .unwrap_or(0);
            anyhow::ensure!(
                current <= MIGRATIONS.len(),
                "the data comes from a newer version of Fern"
            );
            while current < target_version && !MIGRATIONS[current].contains("PRAGMA foreign_keys = OFF") {
                tx.execute_batch(MIGRATIONS[current])?;
                current += 1;
                tx.pragma_update(None, "user_version", current as i64)?;
            }
            tx.commit()?;
            version = current;
        }
    }
    Ok(())
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
