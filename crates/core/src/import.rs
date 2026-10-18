//! Import of a TickTick backup: the CSV file its "Generate Backup" saves. The file starts with a
//! few lines about the backup, then a header row ("Folder Name", "List Name", "Title", …) and one
//! row per task. Columns are found by their names, so their order does not matter.

use std::collections::HashMap;

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset, Local, NaiveDateTime};
use rusqlite::params;

use crate::model::{Id, Kind, Priority};
use crate::recurrence::Recurrence;
use crate::store::Store;

/// One task of a backup, as written there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackupTask {
    /// "Inbox" for tasks in the Inbox.
    pub list: String,
    pub title: String,
    pub kind: Kind,
    pub tags: Vec<String>,
    pub content: String,
    /// The checklist, as (title, done), for "Is Check list" tasks.
    pub checklist: Vec<(String, bool)>,
    pub start: Option<DateTime<FixedOffset>>,
    pub due: Option<DateTime<FixedOffset>>,
    pub all_day: bool,
    /// Reminder triggers, e.g. "TRIGGER:-PT15M".
    pub reminders: Vec<String>,
    pub repeat: Option<Recurrence>,
    pub priority: Priority,
    pub status: Status,
    pub created: Option<DateTime<FixedOffset>>,
    pub completed: Option<DateTime<FixedOffset>>,
    pub id: String,
    pub parent_id: String,
}

/// The backup's "Status": 0 normal, 1 and 2 completed (2 archived), -1 won't do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Open,
    Completed,
    WontDo,
}

/// What an import added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub tasks: usize,
    pub lists: usize,
}

/// Splits CSV text into rows of fields: fields may be quoted, with `""` for a quote and line
/// breaks inside quotes.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            (true, '"') => quoted = false,
            (true, c) => field.push(c),
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut field)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (false, c) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn parse_time(value: &str) -> Option<DateTime<FixedOffset>> {
    let value = value.trim();
    DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%z").ok()
}

/// Reads the tasks of a TickTick backup.
pub fn parse_backup(text: &str) -> Result<Vec<BackupTask>> {
    let rows = parse_csv(text);
    let header_at = rows
        .iter()
        .position(|row| row.iter().any(|c| c == "Title") && row.iter().any(|c| c == "List Name"))
        .context("not a TickTick backup: no header row with \"List Name\" and \"Title\"")?;
    let columns: HashMap<&str, usize> = rows[header_at].iter().enumerate().map(|(i, name)| (name.as_str(), i)).collect();
    let mut tasks = Vec::new();
    for row in &rows[header_at + 1..] {
        let get = |name: &str| columns.get(name).and_then(|i| row.get(*i)).map(|s| s.as_str()).unwrap_or("");
        let title = get("Title").trim();
        if title.is_empty() && get("Content").trim().is_empty() {
            continue;
        }
        let is_checklist = get("Is Check list").eq_ignore_ascii_case("Y");
        let (content, checklist) = if is_checklist { split_checklist(get("Content")) } else { (get("Content").to_owned(), Vec::new()) };
        tasks.push(BackupTask {
            list: get("List Name").trim().to_owned(),
            title: title.to_owned(),
            kind: if get("Kind").eq_ignore_ascii_case("NOTE") { Kind::Note } else { Kind::Task },
            tags: get("Tags")
                .split(',')
                .map(|t| t.trim().trim_start_matches('#').to_owned())
                .filter(|t| !t.is_empty())
                .collect(),
            content,
            checklist,
            start: parse_time(get("Start Date")),
            due: parse_time(get("Due Date")),
            all_day: get("Is All Day").eq_ignore_ascii_case("true"),
            reminders: get("Reminder").split(',').map(|r| r.trim().to_owned()).filter(|r| !r.is_empty()).collect(),
            repeat: get("Repeat").trim().parse().ok(),
            priority: Priority::from_i64(get("Priority").trim().parse().unwrap_or(0)),
            status: match get("Status").trim() {
                "1" | "2" => Status::Completed,
                "-1" => Status::WontDo,
                _ => Status::Open,
            },
            created: parse_time(get("Created Time")),
            completed: parse_time(get("Completed Time")),
            id: get("taskId").trim().to_owned(),
            parent_id: get("parentId").trim().to_owned(),
        });
    }
    Ok(tasks)
}

/// A checklist task's content: lines starting with "▫" (open) or "▪" (done) are its items, the
/// rest is its description.
fn split_checklist(content: &str) -> (String, Vec<(String, bool)>) {
    let mut notes = Vec::new();
    let mut items = Vec::new();
    for line in content.lines() {
        if let Some(item) = line.strip_prefix('▫') {
            items.push((item.trim().to_owned(), false));
        } else if let Some(item) = line.strip_prefix('▪') {
            items.push((item.trim().to_owned(), true));
        } else {
            notes.push(line);
        }
    }
    (notes.join("\n").trim().to_owned(), items)
}

/// A reminder trigger as minutes before the due time (before 09:00 on the due day for all-day
/// tasks, which TickTick counts from midnight): "-PT15M" → 15, "PT0S" → 0, "-P1DT15H0M0S" on an
/// all-day task → two days before at 09:00.
pub fn reminder_minutes(trigger: &str, all_day: bool) -> Option<i64> {
    let value = trigger.trim().trim_start_matches("TRIGGER:");
    let (sign, rest) = match value.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value.trim_start_matches('+')),
    };
    let rest = rest.strip_prefix('P')?;
    let (days, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut minutes = 0i64;
    if let Some(d) = days.strip_suffix('D') {
        minutes += d.parse::<i64>().ok()? * 24 * 60;
    } else if let Some(w) = days.strip_suffix('W') {
        minutes += w.parse::<i64>().ok()? * 7 * 24 * 60;
    } else if !days.is_empty() {
        return None;
    }
    let mut number = String::new();
    for c in time.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let n: i64 = std::mem::take(&mut number).parse().ok()?;
        match c {
            'H' => minutes += n * 60,
            'M' => minutes += n,
            'S' => {}
            _ => return None,
        }
    }
    let offset = sign * minutes;
    Some(if all_day { 9 * 60 - offset } else { -offset })
}

fn local(time: DateTime<FixedOffset>) -> NaiveDateTime {
    time.with_timezone(&Local).naive_local()
}

impl Store {
    /// Adds the tasks of a TickTick backup to the ones here: lists are matched by name or
    /// created, subtasks keep their parents, completed tasks keep their completion time. The
    /// backup is read whole before anything is added; the adding itself is not one transaction,
    /// as the store's own steps open theirs.
    pub fn import_ticktick(&self, text: &str) -> Result<ImportSummary> {
        let backup = parse_backup(text)?;
        let mut summary = ImportSummary::default();
        let mut lists: HashMap<String, Id> = self.lists()?.into_iter().map(|l| (l.name.to_lowercase(), l.id)).collect();
        let mut ids: HashMap<&str, Id> = HashMap::new();
        for item in &backup {
            let list = if item.list.is_empty() || item.list.eq_ignore_ascii_case("inbox") {
                None
            } else if let Some(id) = lists.get(&item.list.to_lowercase()) {
                Some(*id)
            } else {
                let list = self.create_list(&item.list)?;
                summary.lists += 1;
                lists.insert(item.list.to_lowercase(), list.id);
                Some(list.id)
            };
            let task = self.add_task_at(&item.title, false)?;
            if !item.id.is_empty() {
                ids.insert(&item.id, task.id);
            }
            self.set_list(task.id, list)?;
            if item.kind == Kind::Note {
                self.set_kind(task.id, Kind::Note)?;
            }
            if !item.content.is_empty() {
                self.set_notes(task.id, &item.content)?;
            }
            if item.priority != Priority::None {
                self.set_priority(task.id, item.priority)?;
            }
            if !item.tags.is_empty() {
                self.set_tags(task.id, &item.tags)?;
            }
            let mut after = None;
            for (title, done) in &item.checklist {
                let entry = self.add_checklist_item(task.id, title, after)?;
                if *done {
                    self.set_checklist_item_completed(entry.id, true)?;
                }
                after = Some(entry.id);
            }
            if let Some(start) = item.start.or(item.due).map(local) {
                let time = (!item.all_day).then_some(start.time());
                self.set_due(task.id, Some(start.date()), time)?;
                if let (Some(_), Some(end)) = (item.start, item.due.map(local)) {
                    // A due date after the start date is the end of a duration; all-day spans end
                    // at midnight after their last day.
                    let end = if item.all_day { end - chrono::Duration::days(1) } else { end };
                    if end > start {
                        self.set_due_end(task.id, Some((end.date(), time.map(|_| end.time()))))?;
                    }
                }
                if let Some(repeat) = item.repeat {
                    self.conn.execute(
                        "UPDATE tasks SET repeat = ?2 WHERE id = ?1",
                        params![task.id.to_string(), repeat.to_string()],
                    )?;
                }
                let reminders: Vec<i64> = item.reminders.iter().filter_map(|r| reminder_minutes(r, item.all_day)).collect();
                if !reminders.is_empty() {
                    self.set_reminders(task.id, &reminders)?;
                }
            }
            let created = item.created.map(|t| t.timestamp_millis());
            let closed = item.completed.map(|t| t.timestamp_millis()).or(created).unwrap_or_else(|| Local::now().timestamp_millis());
            self.conn.execute(
                "UPDATE tasks SET created_at = COALESCE(?2, created_at), completed_at = ?3, wont_do_at = ?4 WHERE id = ?1",
                params![
                    task.id.to_string(),
                    created,
                    (item.status == Status::Completed).then_some(closed),
                    (item.status == Status::WontDo).then_some(closed),
                ],
            )?;
            summary.tasks += 1;
        }
        for item in &backup {
            if let (Some(id), Some(parent)) = (ids.get(item.id.as_str()), ids.get(item.parent_id.as_str())) {
                self.conn.execute(
                    "UPDATE tasks SET parent_id = ?2 WHERE id = ?1",
                    params![id.to_string(), parent.to_string()],
                )?;
            }
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::model::View;
    use crate::recurrence::Frequency;

    const BACKUP: &str = "\u{feff}\"Date: 2026-10-18+0000\"
\"Version: 7.1\"
\"Status:
0 Normal
1 Completed
2 Archived\"
\"Folder Name\",\"List Name\",\"Title\",\"Kind\",\"Tags\",\"Content\",\"Is Check list\",\"Start Date\",\"Due Date\",\"Reminder\",\"Repeat\",\"Priority\",\"Status\",\"Created Time\",\"Completed Time\",\"Order\",\"Timezone\",\"Is All Day\",\"Is Floating\",\"Column Name\",\"Column Order\",\"View Mode\",\"taskId\",\"parentId\"
\"\",\"Inbox\",\"Pay rent\",\"TEXT\",\"home, money\",\"Transfer, then \"\"save\"\" the receipt\",\"N\",\"2026-11-13T12:00:00+0000\",\"2026-11-13T12:00:00+0000\",\"TRIGGER:P0DT9H0M0S\",\"RRULE:FREQ=MONTHLY;INTERVAL=1\",\"5\",\"0\",\"2026-10-01T08:00:00+0000\",\"\",\"0\",\"UTC\",\"true\",\"false\",\"\",\"\",\"list\",\"1\",\"\"
\"Home\",\"Chores\",\"Clean the kitchen\",\"CHECKLIST\",\"\",\"Before guests
▫Dishes
▪Floor\",\"Y\",\"\",\"\",\"\",\"\",\"0\",\"0\",\"\",\"\",\"1\",\"UTC\",\"\",\"false\",\"\",\"\",\"list\",\"2\",\"\"
\"Home\",\"Chores\",\"Buy soap\",\"TEXT\",\"\",\"\",\"N\",\"\",\"\",\"\",\"\",\"1\",\"2\",\"2026-10-02T08:00:00+0000\",\"2026-10-03T08:00:00+0000\",\"2\",\"UTC\",\"\",\"false\",\"\",\"\",\"list\",\"3\",\"2\"
\"\",\"Inbox\",\"Ideas\",\"NOTE\",\"\",\"Write more\",\"N\",\"\",\"\",\"\",\"\",\"0\",\"0\",\"\",\"\",\"3\",\"UTC\",\"\",\"false\",\"\",\"\",\"list\",\"4\",\"\"
";

    #[test]
    fn csv_quotes_and_line_breaks() {
        let rows = parse_csv("a,\"b,c\",\"d\"\"e\"\n\"f\ng\",h\r\n");
        assert_eq!(rows, [vec!["a", "b,c", "d\"e"], vec!["f\ng", "h"]]);
    }

    #[test]
    fn reads_a_backup() {
        let tasks = parse_backup(BACKUP).unwrap();
        assert_eq!(tasks.len(), 4);
        let rent = &tasks[0];
        assert_eq!(rent.list, "Inbox");
        assert_eq!(rent.tags, ["home", "money"]);
        assert_eq!(rent.content, "Transfer, then \"save\" the receipt");
        assert_eq!(rent.priority, Priority::High);
        assert_eq!(rent.repeat.map(|r| r.frequency), Some(Frequency::Monthly));
        assert!(rent.all_day);
        let chores = &tasks[1];
        assert_eq!(chores.content, "Before guests");
        assert_eq!(chores.checklist, [("Dishes".to_owned(), false), ("Floor".to_owned(), true)]);
        assert_eq!(tasks[2].status, Status::Completed);
        assert_eq!(tasks[2].parent_id, "2");
        assert_eq!(tasks[3].kind, Kind::Note);
        assert!(parse_backup("just,some\ncsv,file\n").is_err());
    }

    #[test]
    fn reminder_triggers() {
        assert_eq!(reminder_minutes("TRIGGER:-PT15M", false), Some(15));
        assert_eq!(reminder_minutes("TRIGGER:PT0S", false), Some(0));
        assert_eq!(reminder_minutes("TRIGGER:-P1D", false), Some(24 * 60));
        assert_eq!(reminder_minutes("TRIGGER:P0DT9H0M0S", true), Some(0));
        assert_eq!(reminder_minutes("TRIGGER:-P1DT15H0M0S", true), Some(2 * 24 * 60));
        assert_eq!(reminder_minutes("TRIGGER:-PT15H", true), Some(24 * 60));
        assert_eq!(reminder_minutes("nonsense", false), None);
    }

    #[test]
    fn imports_into_the_store() -> Result<()> {
        let store = Store::open_in_memory()?;
        let existing = store.create_list("chores")?;
        let summary = store.import_ticktick(BACKUP)?;
        assert_eq!(summary, ImportSummary { tasks: 4, lists: 0 }, "lists match by name, ignoring case");

        let tasks = store.tasks()?;
        let find = |title: &str| tasks.iter().find(|t| t.title == title).cloned().unwrap();
        let rent = find("Pay rent");
        assert_eq!(rent.list_id, None);
        assert_eq!(rent.due_date, Some(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap()));
        assert_eq!(rent.due_time, None);
        assert_eq!(rent.reminders, [0]);
        assert_eq!(rent.priority, Priority::High);
        assert!(rent.repeat.is_some());
        let mut tags = rent.tags.clone();
        tags.sort();
        assert_eq!(tags, ["home", "money"]);

        let kitchen = find("Clean the kitchen");
        assert_eq!(kitchen.list_id, Some(existing.id));
        assert_eq!(kitchen.notes, "Before guests");
        assert_eq!(kitchen.checklist, Some((1, 2)));
        let soap = find("Buy soap");
        assert!(soap.is_completed());
        assert_eq!(soap.parent_id, Some(kitchen.id));
        assert_eq!(find("Ideas").kind, Kind::Note);

        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        assert_eq!(store.tasks_in(&View::Inbox, today)?.len(), 2);
        Ok(())
    }
}
