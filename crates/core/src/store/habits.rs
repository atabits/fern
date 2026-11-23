use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeZone as _};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::habit::{
    Checkin, CheckinMode, CheckinStatus, Goal, Habit, HabitDraft, HabitSection,
    MAX_CUSTOM_SECTIONS, MAX_LOG_NOTE_CHARS, MAX_REMINDERS, MOODS, polish_amount,
};
use crate::model::Id;

use super::{DATE_FORMAT, SNOOZE_FORMAT, Store, TIME_FORMAT, now_ms, parse_uuid};

impl Store {
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
}
