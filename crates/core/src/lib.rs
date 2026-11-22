//! Shared core of the task tracker: data model, local storage and quick-add parsing.
//! Used by the desktop app and, later, by the mobile clients.

pub mod agenda;
pub mod attachment;
pub mod caldav;
pub mod calendar_options;
pub mod countdown;
pub mod e2ee;
pub mod filter;
pub mod focus;
pub mod habit;
pub mod ical;
pub mod import;
pub mod link;
pub mod lunar;
pub mod markdown;
pub mod matrix;
pub mod model;
pub mod preferences;
pub mod quick_add;
pub mod recurrence;
pub mod reminder;
pub mod search;
pub mod stats;
pub mod store;
pub mod suggest;
pub mod summary;
pub mod sync;
pub mod task_tree;
pub mod timeline;

pub use calendar_options::{CalendarOptions, ColorBy, CountdownColor, HabitColor};
pub use countdown::{Countdown, CountdownKind, Span, span};
pub use filter::{Condition, ConditionValue, DateRule, Field, FilterRules, Join, SmartFilter};
pub use focus::{FocusSession, FocusStats, focus_stats, format_minutes};
pub use habit::{
    Checkin, CheckinMode, CheckinStatus, CustomSection, CycleSummary, DayProgress, Goal, Habit, HabitCycle,
    HabitDraft, HabitSection, HabitStats, MonthStats,
};
pub use habit::Frequency as HabitFrequency;
pub use matrix::{DEFAULT_MATRIX_PROFILE_ID, MatrixProfile};
pub use model::{Activity, Attachment, Comment, Estimate, ChecklistItem, ListSection, Template, Id, Kind, List, ListDraft, ListFolder, Priority, Tag, Task, TaskDraft, View, ViewCounts};
pub use preferences::{DURATIONS, Features, Notifications, DateFormat, DefaultDate, Preferences, QuickDate, RailSection, SidebarSection, TaskDefaults, SmartList, ThemeMode, ThemeSelection, Visibility, WEEK_STARTS, next_week_date};
pub use quick_add::{ParsedTask, Recognition, parse_quick_add, parse_quick_add_on_day, parse_quick_add_with};
pub use recurrence::{Frequency, Recurrence};
pub use store::{BackupEntry, FolderDropTarget, MAX_SUBTASK_DEPTH, SCHEMA_VERSION, Store, TableCounts, list_backups};
pub use task_tree::TaskTree;
