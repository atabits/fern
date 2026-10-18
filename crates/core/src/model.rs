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
    pub created_at: i64,
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

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        !self.is_completed() && self.due_date.is_some_and(|due| due < today)
    }
}

/// A user-defined list (a "project" in TickTick).
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
    Trash,
    List(Id),
    Tag(String),
    /// Tasks whose title, notes or tags contain the text, ignoring case.
    Search(String),
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
