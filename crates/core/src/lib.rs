//! Shared core of the task tracker: data model, local storage and quick-add parsing.
//! Used by the desktop app and, later, by the mobile clients.

pub mod countdown;
pub mod focus;
pub mod habit;
pub mod model;
pub mod quick_add;
pub mod recurrence;
pub mod reminder;
pub mod store;

pub use countdown::{Countdown, CountdownKind, Span, span};
pub use focus::{FocusSession, FocusStats, focus_stats, format_minutes};
pub use habit::{Checkin, Goal, Habit, HabitStats, MonthStats};
pub use habit::Frequency as HabitFrequency;
pub use model::{ChecklistItem, Id, Kind, List, Priority, Tag, Task, View, ViewCounts};
pub use quick_add::{ParsedTask, parse_quick_add};
pub use recurrence::{Frequency, Recurrence};
pub use store::Store;
