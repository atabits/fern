use std::collections::{HashMap, HashSet};

use anyhow::{Context as _, Result};
use chrono::NaiveDate;
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::countdown::{Countdown, CountdownKind};
use crate::model::{Id, Kind, Priority, Task};

use super::{DATE_FORMAT, Store, now_ms, parse_uuid};

const FILTER_ORDER_KEY: &str = "filter_order";

impl Store {
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
        self.in_tx(|| {
            let template = self.templates()?.into_iter().find(|t| t.id == id).context("template not found")?;
            let task = self.add_task(&template.title)?;
            self.set_notes(task.id, &template.notes)?;
            self.set_priority(task.id, template.priority)?;
            self.set_tags(task.id, &template.tags)?;
            if !template.checklist.is_empty() {
                let mut item_stmt = self.conn.prepare_cached(
                    "INSERT INTO checklist_items (id, task_id, title, sort_order, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                let task_id_str = task.id.to_string();
                let now = now_ms();
                for (position, item) in template.checklist.iter().enumerate() {
                    item_stmt.execute(params![
                        Uuid::now_v7().to_string(),
                        &task_id_str,
                        item.trim(),
                        position as i64,
                        now,
                    ])?;
                }
            }
            self.task(task.id)?.context("task not found")
        })
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

    pub fn delete_countdown(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM countdowns WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    /// A user setting such as the UI language; `None` when never set.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([key], |row| row.get(0))
            .optional()?)
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )?
            .execute(params![key, value])?;
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
}
