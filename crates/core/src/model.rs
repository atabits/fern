use chrono::{NaiveDate, NaiveTime};
use uuid::Uuid;

use crate::recurrence::Recurrence;

pub type Id = Uuid;

/// Stored as 0/1/3/5, the same values TickTick uses, so imports need no mapping table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
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
    /// The end of a duration ("Duration" tab): the task spans from the due date and time to here.
    pub end_date: Option<NaiveDate>,
    pub end_time: Option<NaiveTime>,
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
    pub list_id: Id,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    pub id: Id,
    pub name: String,
    pub color: Option<String>,
    /// Number of open tasks in the list.
    pub open_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// Color as `#RRGGBB`.
    pub color: Option<String>,
    /// Number of open tasks with this tag.
    pub open_count: usize,
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
    /// The Summary report; it lists no tasks of its own.
    Summary,
    /// A day picked in the mini calendar: the tasks due that day.
    Day(NaiveDate),
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
