use chrono::{Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone as _};
use uuid::Uuid;

use crate::horizon::Horizon;
use crate::recurrence::Recurrence;

pub type Id = Uuid;

/// Stored as 0/1/3/5, the same values TickTick uses, so imports need no mapping table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    #[default]
    None,
    Low,
    Medium,
    High,
}

impl Priority {
    pub fn to_i64(self) -> i64 {
        match self {
            Priority::None => 0,
            Priority::Low => 1,
            Priority::Medium => 3,
            Priority::High => 5,
        }
    }

    pub fn from_i64(value: i64) -> Self {
        match value {
            5.. => Priority::High,
            3..=4 => Priority::Medium,
            1..=2 => Priority::Low,
            _ => Priority::None,
        }
    }
}

/// A checkable task or a note (TickTick's `kind` TEXT / NOTE).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Task,
    Note,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Task => "task",
            Kind::Note => "note",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "note" => Kind::Note,
            _ => Kind::Task,
        }
    }
}

/// Optional Things-style schedule bucket (`""`, `"evening"`, or `"someday"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleBucket {
    #[default]
    None,
    Evening,
    Someday,
}

impl ScheduleBucket {
    pub fn as_str(self) -> &'static str {
        match self {
            ScheduleBucket::None => "",
            ScheduleBucket::Evening => "evening",
            ScheduleBucket::Someday => "someday",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "evening" => ScheduleBucket::Evening,
            "someday" => ScheduleBucket::Someday,
            _ => ScheduleBucket::None,
        }
    }
}

/// Fields written together when creating a task from an editor or quick-add form.
#[derive(Clone, Default)]
pub struct TaskDraft {
    pub title: String,
    pub notes: String,
    pub list_id: Option<Id>,
    pub kind: Kind,
    pub priority: Priority,
    pub due_date: Option<NaiveDate>,
    pub due_time: Option<NaiveTime>,
    pub repeat: Option<Recurrence>,
    pub reminders: Vec<i64>,
    pub tags: Vec<String>,
    pub end_date: Option<NaiveDate>,
    pub end_time: Option<NaiveTime>,
    pub section_id: Option<Id>,
    pub checklist: Vec<(String, bool)>,
    pub location: Option<String>,
    pub timezone: Option<String>,
    pub is_floating: bool,
    pub constant_reminder: bool,
    pub start_date: Option<NaiveDate>,
    pub schedule_bucket: ScheduleBucket,
    pub horizon: Option<Horizon>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: Id,
    /// `None` means the task is in the Inbox.
    pub list_id: Option<Id>,
    pub parent_id: Option<Id>,
    pub kind: Kind,
    pub title: String,
    pub notes: String,
    pub priority: Priority,
    pub due_date: Option<NaiveDate>,
    /// `None` with a date set means an all-day task.
    pub due_time: Option<NaiveTime>,
    /// Repeat rule; completing the task moves its due date to the next occurrence.
    pub repeat: Option<Recurrence>,
    pub tags: Vec<String>,
    /// Checklist progress as (done, total); `None` when the task has no checklist items.
    pub checklist: Option<(usize, usize)>,
    pub completed_at: Option<i64>,
    /// When the task was moved to the trash.
    pub deleted_at: Option<i64>,
    /// When the task was marked "Won't Do".
    pub wont_do_at: Option<i64>,
    /// When the task was explicitly marked "In Progress".
    pub in_progress_at: Option<i64>,
    /// The end of a duration ("Duration" tab): the task spans from the due date and time to here.
    pub end_date: Option<NaiveDate>,
    pub end_time: Option<NaiveTime>,
    /// Optional place / location associated with the task.
    pub location: Option<String>,
    /// Optional IANA time zone (e.g. "America/New_York"); `None` uses local time.
    pub timezone: Option<String>,
    /// Floating tasks keep their wall-clock time regardless of `timezone`.
    pub is_floating: bool,
    /// Snoozed reminder timestamp in local time, if snoozed.
    pub snoozed_until: Option<NaiveDateTime>,
    /// Whether this task's reminders repeat persistently until dismissed/snoozed/completed.
    pub constant_reminder: bool,
    /// When the task was pinned to the top of its lists.
    pub pinned_at: Option<i64>,
    /// The section of its list ("Add Section"), if any.
    pub section_id: Option<Id>,
    /// Unix milliseconds of the last edit; `None` for tasks never edited since they were added.
    pub modified_at: Option<i64>,
    /// Reminders as minutes before the due time (09:00 on the due day for all-day tasks).
    pub reminders: Vec<i64>,
    /// "Estimation" from Start Focus: how much focus the task is expected to take.
    pub estimate: Option<Estimate>,
    /// Things-style scheduled start date ("When").
    pub start_date: Option<NaiveDate>,
    /// Things-style schedule bucket (`None`, `Evening`, `Someday`).
    pub schedule_bucket: ScheduleBucket,
    /// Optional horizon planning bucket (`Week`, `Month`, `Quarter`, `Year`, `Decade`, `Life`).
    pub horizon: Option<Horizon>,
    pub created_at: i64,
}

/// The original's "Estimated Pomo" or "Estimated Duration".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Estimate {
    Pomos(u32),
    Minutes(u32),
}

impl Estimate {
    /// Stored as "p:3" or "m:90".
    pub fn to_storage(self) -> String {
        match self {
            Estimate::Pomos(n) => format!("p:{n}"),
            Estimate::Minutes(n) => format!("m:{n}"),
        }
    }

    pub fn from_storage(value: &str) -> Option<Self> {
        let (kind, n) = value.split_once(':')?;
        let n = n.parse().ok().filter(|n| *n > 0)?;
        match kind {
            "p" => Some(Estimate::Pomos(n)),
            "m" => Some(Estimate::Minutes(n)),
            _ => None,
        }
    }
}

/// One entry of a task's activity: when and what ("created", "renamed", "completed", "reopened",
/// "date", "date_cleared", "priority", "moved", "deleted", "restored", "wont_do").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    /// Unix milliseconds.
    pub at: i64,
    pub kind: String,
}

/// A file attached to a task, kept as a copy in the app's attachment folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub id: Id,
    pub task_id: Id,
    pub name: String,
    /// Bytes.
    pub size: u64,
    pub created_at: i64,
}

impl Attachment {
    /// Where the copy lives in the attachment folder `dir`.
    pub fn path(&self, dir: &std::path::Path) -> std::path::PathBuf {
        dir.join(format!("{}-{}", self.id, self.name))
    }
}

/// One item of a task's checklist (TickTick's `items`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecklistItem {
    pub id: Id,
    pub task_id: Id,
    pub title: String,
    pub completed: bool,
}

impl Task {
    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }

    /// Whether the task falls on `date`: its due date, or any day of its duration.
    pub fn occurs_on(&self, date: NaiveDate) -> bool {
        self.due_date.is_some_and(|start| start <= date && date <= self.end_date.unwrap_or(start).max(start))
    }

    pub fn is_wont_do(&self) -> bool {
        self.wont_do_at.is_some()
    }

    pub fn is_in_progress(&self) -> bool {
        !self.is_closed() && self.in_progress_at.is_some()
    }

    /// Due timestamp converted into the local time zone when a timed task specifies an IANA zone
    /// and is not floating; all-day tasks anchor at `ALL_DAY_REMINDER_HOUR` local time.
    pub fn effective_due_local(&self) -> Option<NaiveDateTime> {
        self.effective_due_in(&Local)
    }

    /// Due timestamp converted into `target` time zone (useful for deterministic timezone tests).
    pub fn effective_due_in<T: chrono::TimeZone>(&self, target: &T) -> Option<NaiveDateTime> {
        let date = self.due_date?;
        let Some(time) = self.due_time else {
            return NaiveTime::from_hms_opt(crate::reminder::ALL_DAY_REMINDER_HOUR, 0, 0).map(|t| date.and_time(t));
        };
        let wall = date.and_time(time);
        if !self.is_floating
            && let Some(tz_name) = self.timezone.as_deref()
            && let Ok(tz) = tz_name.parse::<chrono_tz::Tz>()
            && let Some(dt) = tz.from_local_datetime(&wall).earliest()
        {
            return Some(dt.with_timezone(target).naive_local());
        }
        Some(wall)
    }

    /// Completed or marked "Won't Do": no longer something to do.
    pub fn is_closed(&self) -> bool {
        self.is_completed() || self.is_wont_do()
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        !self.is_completed() && self.due_date.is_some_and(|due| due < today)
    }
}

/// A user-defined list (a "project" in TickTick).
/// "Save as Template": what a new task made from it starts with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub id: Id,
    pub title: String,
    pub notes: String,
    pub priority: Priority,
    pub tags: Vec<String>,
    pub checklist: Vec<String>,
}

/// A section of a list: a named group of its tasks, a column in the kanban view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListSection {
    pub id: Id,
    /// The list, or `None` for the Inbox.
    pub list_id: Option<Id>,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    pub id: Id,
    pub name: String,
    pub color: Option<String>,
    /// Number of open tasks in the list.
    pub open_count: usize,
    /// An emoji shown instead of the list icon, as the original's lists have.
    pub icon: Option<String>,
    /// The folder the list is grouped in.
    pub folder: Option<Id>,
    /// The original's List Type: a note list adds notes rather than tasks.
    pub kind: Kind,
    /// The original's "Show in Smart List": whether the list's tasks appear in All, Today,
    /// Tomorrow and Next 7 Days.
    pub in_smart_lists: bool,
    /// Archived lists leave the lists column for "Archived Lists" and the smart lists.
    pub archived: bool,
    /// Project notes (used by Things frontend).
    pub notes: String,
    /// Optional project deadline (used by Things frontend).
    pub deadline: Option<NaiveDate>,
    /// Unix milliseconds when the project was completed (used by Things frontend).
    pub completed_at: Option<i64>,
}

/// A comment on a task, as the original's comments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub id: Id,
    pub task: Id,
    pub text: String,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// A folder of lists in the lists column, as the original's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListFolder {
    pub id: Id,
    pub name: String,
}

/// What the Add List / Edit List dialog sets on a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListDraft {
    pub name: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub folder: Option<Id>,
    pub kind: Kind,
    pub in_smart_lists: bool,
    pub notes: String,
    pub deadline: Option<NaiveDate>,
    pub completed_at: Option<i64>,
}

impl Default for ListDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            icon: None,
            color: None,
            folder: None,
            kind: Kind::Task,
            in_smart_lists: true,
            notes: String::new(),
            deadline: None,
            completed_at: None,
        }
    }
}

impl ListDraft {
    pub fn of(list: &List) -> Self {
        Self {
            name: list.name.clone(),
            icon: list.icon.clone(),
            color: list.color.clone(),
            folder: list.folder,
            kind: list.kind,
            in_smart_lists: list.in_smart_lists,
            notes: list.notes.clone(),
            deadline: list.deadline,
            completed_at: list.completed_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// Color as `#RRGGBB`.
    pub color: Option<String>,
    /// Number of open tasks with this tag.
    pub open_count: usize,
    /// The original's Parent Tag: a sub-tag sits under it in the lists column, one level deep.
    pub parent: Option<String>,
    /// Pinned tags come first among the tags.
    pub pinned: bool,
}

/// What the task list shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Inbox,
    All,
    Today,
    Tomorrow,
    Next7Days,
    Completed,
    WontDo,
    Trash,
    List(Id),
    Tag(String),
    /// Tasks whose title, notes or tags contain the text, ignoring case.
    Search(String),
    /// A saved filter ("Filters" in the lists column).
    Filter(Id),
    /// The panoramic Horizons board; it lists no tasks of its own.
    Horizons,
    /// The Summary report; it lists no tasks of its own.
    Summary,
    /// A day picked in the mini calendar: the tasks due that day.
    Day(NaiveDate),
}

impl View {
    /// Whether the view gathers tasks from several lists, so that its rows name each task's list
    /// after the tags, as the original's smart lists, tags and searches do.
    pub fn shows_list_names(&self) -> bool {
        !matches!(self, Self::Inbox | Self::List(_) | Self::Horizons | Self::Summary)
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;

    #[test]
    fn list_names_in_views_across_lists() {
        assert!(View::Next7Days.shows_list_names());
        assert!(View::Tag("work".into()).shows_list_names());
        assert!(View::Completed.shows_list_names());
        assert!(!View::Inbox.shows_list_names());
        assert!(!View::List(uuid::Uuid::now_v7()).shows_list_names());
    }
}

/// Open-task counters for the lists column.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewCounts {
    pub inbox: usize,
    pub all: usize,
    pub today: usize,
    pub tomorrow: usize,
    pub next_7_days: usize,
}
