//! Shared core of the task tracker: data model, local storage and quick-add parsing.
//! Used by the desktop app and, later, by the mobile clients.

pub mod agenda;
pub mod countdown;
pub mod filter;
pub mod focus;
pub mod habit;
pub mod import;
pub mod link;
pub mod markdown;
pub mod matrix;
pub mod model;
pub mod preferences;
pub mod quick_add;
pub mod recurrence;
pub mod reminder;
pub mod stats;
pub mod store;
pub mod summary;
pub mod sync;
pub mod timeline;

pub use countdown::{Countdown, CountdownKind, Span, span};
pub use filter::{DateRule, Field, Join, Link, SmartFilter};
pub use focus::{FocusSession, FocusStats, focus_stats, format_minutes};
pub use habit::{Checkin, Goal, Habit, HabitStats, MonthStats};
pub use habit::Frequency as HabitFrequency;
pub use habit::{CustomSection, HabitSection};
pub use model::{Activity, Attachment, Comment, Estimate, ChecklistItem, ListSection, Template, Id, Kind, List, ListDraft, ListFolder, Priority, Tag, Task, View, ViewCounts};
pub use preferences::{DURATIONS, Features, Notifications, DateFormat, DefaultDate, Preferences, TaskDefaults, SmartList, ThemeMode, Visibility, WEEK_STARTS};
pub use quick_add::{ParsedTask, Recognition, parse_quick_add, parse_quick_add_with};
pub use recurrence::{Frequency, Recurrence};
pub use store::Store;
