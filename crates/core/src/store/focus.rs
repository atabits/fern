use anyhow::Result;
use rusqlite::{OptionalExtension as _, params};
use uuid::Uuid;

use crate::focus::FocusSession;
use crate::model::Id;

use super::{Store, now_ms, parse_uuid};

fn focus_session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FocusSession> {
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
}

impl Store {
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
            .query_map([], focus_session_from_row)?
            .collect::<Result<_, _>>()?;
        Ok(sessions)
    }

    /// Loads a single focus session by ID without scanning the whole table.
    pub fn focus_session(&self, id: Id) -> Result<Option<FocusSession>> {
        self.conn
            .prepare_cached(
                "SELECT id, task_id, habit_id, title, started_at, ended_at, stopwatch, note FROM focus_sessions WHERE id = ?1",
            )?
            .query_row([id.to_string()], focus_session_from_row)
            .optional()
            .map_err(Into::into)
    }
}
