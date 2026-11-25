use std::collections::HashSet;

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::params;
use uuid::Uuid;

use crate::countdown::{Countdown, CountdownKind};
use crate::model::Id;

use super::{DATE_FORMAT, Store, now_ms, parse_uuid};

impl Store {
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
        self.in_tx(|| {
            let id_str = id.to_string();
            self.conn.execute(
                "DELETE FROM plugin_entity_meta WHERE entity_type = 'countdown' AND entity_id = ?1",
                [&id_str],
            )?;
            self.conn.execute("DELETE FROM countdowns WHERE id = ?1", [&id_str])?;
            Ok(())
        })
    }
}
