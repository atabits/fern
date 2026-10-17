use chrono::{NaiveDate, NaiveTime};
use uuid::Uuid;

pub type Id = Uuid;

/// Значения совпадают с TickTick (0/1/3/5), чтобы импорт переносился без таблиц соответствия.
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

/// Задача с чекбоксом или заметка (в TickTick — `kind` TEXT / NOTE).
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
    pub parent_id: Option<Id>,
    pub kind: Kind,
    pub title: String,
    pub notes: String,
    pub priority: Priority,
    pub due_date: Option<NaiveDate>,
    /// `None` при заданной дате — задача «на весь день».
    pub due_time: Option<NaiveTime>,
    pub tags: Vec<String>,
    pub completed_at: Option<i64>,
    pub created_at: i64,
}

impl Task {
    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }

    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        !self.is_completed() && self.due_date.is_some_and(|due| due < today)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// Цвет в виде `#RRGGBB`.
    pub color: Option<String>,
    /// Количество невыполненных задач с этим тегом.
    pub open_count: usize,
}

/// Что показывается в списке задач.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Inbox,
    All,
    Today,
    Tomorrow,
    Next7Days,
    Completed,
    Tag(String),
}

/// Счётчики невыполненных задач для колонки списков.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewCounts {
    pub inbox: usize,
    pub all: usize,
    pub today: usize,
    pub tomorrow: usize,
    pub next_7_days: usize,
}
