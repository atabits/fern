//! Loads, resolves, and edits JSON/JSONC keymaps: a list of sections, each with an optional
//! `context` predicate and `bindings` from keystrokes to action names (or `null` to unbind).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui::{
    Action as _, App, AssetSource, Global, KeyBinding, KeyBindingContextPredicate,
    KeyBindingMetaIndex, Keystroke, NoAction,
};
use serde::{Deserialize, Serialize};

/// Precedence layers matching GPUI's `KeyBindingMetaIndex` convention (lower index wins).
const USER_META: KeyBindingMetaIndex = KeyBindingMetaIndex(0);
const DEFAULT_META: KeyBindingMetaIndex = KeyBindingMetaIndex(1);

pub const DEFAULT_KEYMAP_PATH: &str = "keymaps/default.json";
pub const USER_KEYMAP_FILE: &str = "keymap.json";
pub const USER_KEYMAP_TEMPLATE: &str = r#"// Fern user keymap — overrides the built-in default bindings.
// Shortcuts changed in Settings → Shortcuts are saved here automatically.
// After editing this file manually, click "Reload keymap" in Settings → Shortcuts.
//
// Format: JSONC (comments // and /* */ are allowed) containing an array of sections:
//   {
//     "context": "Workspace && !TextInput",   // optional; omit for app-wide binding
//     "bindings": {
//       "escape": "fern::MinimizeWindow",     // bind a keystroke to an action
//       "secondary-q": null                   // set to null to unbind a default key
//     }
//   }
//
// Keystroke syntax:
//   Modifiers: secondary- (Ctrl on Windows/Linux, Cmd on macOS), ctrl-, alt-, shift-, win-, cmd-
//   Chords (sequences): space-separated keystrokes, e.g. "g t", "tab p", "v k"
//   Special keys: escape, space, enter, tab, backspace, delete,
//                 up, down, left, right, home, end, pageup, pagedown, f1..f12
//
// Contexts (support &&, ||, !):
//   Workspace      Main window (all sections)
//   Tasks          Tasks module (lists sidebar + task list + details)
//   ThingsView     Things view module (Inbox / Today / Upcoming / Anytime / Someday)
//   TaskList       Focused task list (not while typing in a text field)
//   TaskDetail     Task details panel / sticky note editor
//   TaskDraft      Calendar / quick draft editor
//   Calendar       Calendar view (Day / Week / Month / Agenda)
//   CalendarYear   Calendar Year view
//   SummaryEditor  Markdown note / summary editor
//   Habits         Habits module
//   Matrix         Eisenhower Matrix module
//   Focus          Focus timer page
//   FocusMini      Floating Focus mini window
//   Countdown      Countdown module
//   StickyNote     Floating Sticky Note window
//   Settings       Settings window
//   TextInput      Any focused text input field (combine as "!TextInput" to avoid hijacking typing)
//
// ============================================================================
// All available actions
// ============================================================================
//
// App & Navigation (context: "Workspace", or "Workspace && !TextInput" for bare keys/chords):
//   fern::NewTask               Focus the quick-add field to create a task
//   fern::Search                Open command menu & search
//   fern::SyncNow               Trigger sync with configured server
//   fern::ToggleSidebar         Show or hide the lists sidebar
//   fern::ShowAll               Go to "All" smart list
//   fern::ShowInbox             Go to "Inbox"
//   fern::ShowToday             Go to "Today"
//   fern::ShowTomorrow          Go to "Tomorrow"
//   fern::ShowNext7Days         Go to "Next 7 Days"
//   fern::ShowCompleted         Go to "Completed"
//   fern::ShowWontDo            Go to "Won't Do"
//   fern::ShowTrash             Go to "Trash"
//   fern::PrintList             Print the current task list (or calendar year)
//   fern::PrintDetail           Print the selected task's details
//   fern::ShowShortcuts         Open Settings → Shortcuts
//   fern::OpenSettings          Open the Settings window
//   fern::DismissTransient      Close the active menu, popover, or dialog
//   fern::MinimizeWindow        Minimize the main window (after dismissing open popovers)
//   fern::CloseWindow           Close the active window
//   fern::Quit                  Quit Fern
//
// Task List & Editing (context: "TaskList", or "TaskList && !TextInput"):
//   task_list::SelectNext               Select the next task
//   task_list::SelectPrevious           Select the previous task
//   task_list::SelectAllTasks           Select all visible tasks
//   task_list::ExtendSelectionNext      Extend multi-selection downward
//   task_list::ExtendSelectionPrevious  Extend multi-selection upward
//   task_list::ToggleComplete           Mark selected task(s) complete / incomplete
//   task_list::ToggleInProgress         Toggle "In Progress" state on selected task
//   task_list::DeleteTask               Move selected task(s) to Trash
//   task_list::PinTask                  Pin / unpin selected task
//   task_list::SetDate                  Open due date picker for selected task
//   task_list::ClearDate                Clear due date on selected task
//   task_list::SetToday                 Set selected task due date to Today
//   task_list::SetTomorrow              Set selected task due date to Tomorrow
//   task_list::SetNextWeek              Set selected task due date to Next Week
//   task_list::PriorityNone             Set priority to None
//   task_list::PriorityLow              Set priority to Low
//   task_list::PriorityMedium           Set priority to Medium
//   task_list::PriorityHigh             Set priority to High
//   task_list::ToggleAllGroups          Expand / collapse all groups and sections
//   task_list::ToggleAllSubtasks        Expand / collapse all subtasks
//   task_list::AddSubtask               Add a subtask under the selected task
//   task_list::OpenAsSticky             Open the selected task as a floating Sticky Note
//   task_list::ViewList                 Switch current list to List view
//   task_list::ViewKanban               Switch current list to Kanban view
//   task_list::ViewTimeline             Switch current list to Timeline view
//   task_list::Undo                     Undo the last task action
//   task_list::Redo                     Redo the last undone task action
//
// Calendar (context: "Calendar" / "CalendarYear"):
//   calendar::DayView           Switch calendar to Day view
//   calendar::WeekView          Switch calendar to Week view
//   calendar::MonthView         Switch calendar to Month view
//   calendar::YearView          Switch calendar to Year view
//   calendar::AgendaView        Switch calendar to Agenda view
//   calendar::PreviousPeriod    Navigate to previous day / week / month
//   calendar::NextPeriod        Navigate to next day / week / month
//   calendar::PreviousYear      Navigate to previous year (CalendarYear)
//   calendar::NextYear          Navigate to next year (CalendarYear)
//   calendar::CalendarToday     Jump back to Today
//   calendar::PrintYear         Print the Year view (CalendarYear)
//
// Focus Timer (context: app-wide / "Focus" / "FocusMini"):
//   focus::TogglePause          Pause or resume the active timer (or start next)
//   focus::StartStop            Start a focus session or end the running one
//
// Sticky Note Window (context: "StickyNote"):
//   sticky::TogglePin           Toggle always-on-top pin
//   sticky::ToggleColor         Open / close the color palette
//   sticky::ToggleCollapse      Collapse / expand the sticky note
//   sticky::AlignTop            Snap window to the top edge of the screen (Windows)
//   sticky::AlignLeft           Snap window to the left edge of the screen (Windows)
//   sticky::AlignRight          Snap window to the right edge of the screen (Windows)
//
// Habits (context: "Habits"):
//   habits::SelectNextHabit       Select the next habit
//   habits::SelectPreviousHabit   Select the previous habit
//   habits::CheckInSelectedHabit  Toggle check-in for the selected habit
//   habits::NewHabit              Open the New Habit dialog
//
// Eisenhower Matrix (context: "Matrix"):
//   matrix::SelectNextMatrixTask      Select the next task in the matrix
//   matrix::SelectPreviousMatrixTask  Select the previous task in the matrix
//   matrix::ToggleMatrixTask          Complete / reopen the selected matrix task
//   matrix::OpenMatrixTask            Open details for the selected matrix task
//   matrix::DeleteMatrixTask          Delete the selected matrix task
//
// Markdown Editor (context: "SummaryEditor"):
//   markdown::Bold, markdown::Italic, markdown::Underline, markdown::Strike,
//   markdown::Highlight, markdown::CheckItem, markdown::BulletedList,
//   markdown::NumberedList, markdown::Link, markdown::Code, markdown::Quote
//
// Drafts & Dialogs:
//   things::OpenWhen            Open the When schedule picker for the active task (context: "ThingsView")
//   task_detail::SaveDraft      Save the active draft (context: "TaskDraft")
//   settings::DismissSettings   Dismiss popup or close Settings (context: "Settings")
//
// Text Inputs (context: "TextInput"):
//   Standard editing actions: text_input::Backspace, Delete, Copy, Cut, Paste,
//   Undo, Redo, SelectAll, Left, Right, Up, Down, Home, End, Enter, Cancel, etc.
//
[
]
"#;

/// Visual sections in Settings → Shortcuts, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutSection {
    General,
    NewTask,
    EditTask,
    Navigation,
    SwitchView,
    Focus,
    StickyNote,
}

/// Specification for one customizable in-app shortcut row in Settings → Shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutSpec {
    pub action: &'static str,
    pub section: ShortcutSection,
    /// Primary GPUI context where direct bindings edited from the UI are stored.
    pub context: Option<&'static str>,
    /// Optional built-in two-key chord displayed on the left of "or" (e.g. `("Workspace && !TextInput", "g t")`).
    pub fixed_chord: Option<(&'static str, &'static str)>,
    /// Whether UI shortcut recording accepts unmodified single keys (e.g. `Space`, `D`, `1`).
    pub allow_unmodified: bool,
}

pub const SHORTCUT_COUNT: usize = 56;

/// All customizable in-app shortcuts in Settings → Shortcuts, matching `i18n::Strings::shortcut_labels` 1:1.
pub const SHORTCUT_SPECS: [ShortcutSpec; SHORTCUT_COUNT] = [
    // General (0..10)
    ShortcutSpec { action: "fern::SyncNow", section: ShortcutSection::General, context: Some("Workspace"), fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "fern::DismissTransient", section: ShortcutSection::General, context: None, fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "task_list::Undo", section: ShortcutSection::General, context: Some("TaskList"), fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "task_list::Redo", section: ShortcutSection::General, context: Some("TaskList"), fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "fern::PrintList", section: ShortcutSection::General, context: Some("Workspace"), fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "fern::PrintDetail", section: ShortcutSection::General, context: Some("Workspace"), fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "fern::Search", section: ShortcutSection::General, context: Some("Workspace"), fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "fern::ShowShortcuts", section: ShortcutSection::General, context: Some("Workspace"), fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "fern::MinimizeWindow", section: ShortcutSection::General, context: Some("Workspace && !TextInput"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "fern::Quit", section: ShortcutSection::General, context: None, fixed_chord: None, allow_unmodified: false },
    // New Task (10..16)
    ShortcutSpec { action: "fern::NewTask", section: ShortcutSection::NewTask, context: Some("Workspace"), fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "task_list::AddSubtask", section: ShortcutSection::NewTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::ToggleAllGroups", section: ShortcutSection::NewTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::ToggleAllSubtasks", section: ShortcutSection::NewTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::SelectNext", section: ShortcutSection::NewTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::SelectPrevious", section: ShortcutSection::NewTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    // Edit Task (16..29)
    ShortcutSpec { action: "task_list::ToggleComplete", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::PinTask", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::DeleteTask", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::SetDate", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::ClearDate", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::SetToday", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::SetTomorrow", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::SetNextWeek", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::PriorityNone", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::PriorityLow", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::PriorityMedium", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::PriorityHigh", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "task_list::OpenAsSticky", section: ShortcutSection::EditTask, context: Some("TaskList"), fixed_chord: None, allow_unmodified: true },
    // Navigation (29..39)
    ShortcutSpec { action: "fern::ToggleSidebar", section: ShortcutSection::Navigation, context: Some("Workspace"), fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "fern::OpenSettings", section: ShortcutSection::Navigation, context: Some("Workspace"), fixed_chord: Some(("Workspace && !TextInput", "g s")), allow_unmodified: false },
    ShortcutSpec { action: "fern::ShowAll", section: ShortcutSection::Navigation, context: Some("Workspace"), fixed_chord: Some(("Workspace && !TextInput", "g a")), allow_unmodified: false },
    ShortcutSpec { action: "fern::ShowInbox", section: ShortcutSection::Navigation, context: Some("Workspace"), fixed_chord: Some(("Workspace && !TextInput", "g i")), allow_unmodified: false },
    ShortcutSpec { action: "fern::ShowToday", section: ShortcutSection::Navigation, context: Some("Workspace"), fixed_chord: Some(("Workspace && !TextInput", "g t")), allow_unmodified: false },
    ShortcutSpec { action: "fern::ShowTomorrow", section: ShortcutSection::Navigation, context: Some("Workspace"), fixed_chord: Some(("Workspace && !TextInput", "g r")), allow_unmodified: false },
    ShortcutSpec { action: "fern::ShowNext7Days", section: ShortcutSection::Navigation, context: Some("Workspace"), fixed_chord: Some(("Workspace && !TextInput", "g n")), allow_unmodified: false },
    ShortcutSpec { action: "fern::ShowCompleted", section: ShortcutSection::Navigation, context: Some("Workspace"), fixed_chord: Some(("Workspace && !TextInput", "g c")), allow_unmodified: false },
    ShortcutSpec { action: "fern::ShowWontDo", section: ShortcutSection::Navigation, context: Some("Workspace"), fixed_chord: Some(("Workspace && !TextInput", "g w")), allow_unmodified: false },
    ShortcutSpec { action: "fern::ShowTrash", section: ShortcutSection::Navigation, context: Some("Workspace"), fixed_chord: Some(("Workspace && !TextInput", "g g")), allow_unmodified: false },
    // Switch Views (39..48)
    ShortcutSpec { action: "task_list::ViewList", section: ShortcutSection::SwitchView, context: Some("TaskList"), fixed_chord: Some(("TaskList && !TextInput", "v l")), allow_unmodified: true },
    ShortcutSpec { action: "task_list::ViewKanban", section: ShortcutSection::SwitchView, context: Some("TaskList"), fixed_chord: Some(("TaskList && !TextInput", "v k")), allow_unmodified: true },
    ShortcutSpec { action: "task_list::ViewTimeline", section: ShortcutSection::SwitchView, context: Some("TaskList"), fixed_chord: Some(("TaskList && !TextInput", "v t")), allow_unmodified: true },
    ShortcutSpec { action: "calendar::DayView", section: ShortcutSection::SwitchView, context: Some("Calendar"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "calendar::WeekView", section: ShortcutSection::SwitchView, context: Some("Calendar"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "calendar::MonthView", section: ShortcutSection::SwitchView, context: Some("Calendar"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "calendar::YearView", section: ShortcutSection::SwitchView, context: Some("Calendar"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "calendar::AgendaView", section: ShortcutSection::SwitchView, context: Some("Calendar"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "calendar::CalendarToday", section: ShortcutSection::SwitchView, context: Some("Calendar"), fixed_chord: None, allow_unmodified: true },
    // Focus (48..50)
    ShortcutSpec { action: "focus::TogglePause", section: ShortcutSection::Focus, context: None, fixed_chord: None, allow_unmodified: false },
    ShortcutSpec { action: "focus::StartStop", section: ShortcutSection::Focus, context: None, fixed_chord: None, allow_unmodified: false },
    // Sticky Note (50..56)
    ShortcutSpec { action: "sticky::TogglePin", section: ShortcutSection::StickyNote, context: Some("StickyNote"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "sticky::ToggleColor", section: ShortcutSection::StickyNote, context: Some("StickyNote"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "sticky::ToggleCollapse", section: ShortcutSection::StickyNote, context: Some("StickyNote"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "sticky::AlignTop", section: ShortcutSection::StickyNote, context: Some("StickyNote"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "sticky::AlignLeft", section: ShortcutSection::StickyNote, context: Some("StickyNote"), fixed_chord: None, allow_unmodified: true },
    ShortcutSpec { action: "sticky::AlignRight", section: ShortcutSection::StickyNote, context: Some("StickyNote"), fixed_chord: None, allow_unmodified: true },
];

/// Resolved runtime display state for one row in `SHORTCUT_SPECS`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedShortcut {
    pub index: usize,
    pub spec: ShortcutSpec,
    /// Formatted parts of an active `fixed_chord`, e.g. `Some(("G", "T"))` for `"g t"`.
    pub fixed_chord_parts: Option<(String, String)>,
    /// Active direct shortcut labels (excluding `fixed_chord`), e.g. `["Ctrl+S"]` or `["D", "1"]`.
    pub keys: Vec<String>,
    /// Indices into `SHORTCUT_SPECS` of other actions bound to the same key in an overlapping context.
    pub conflicts: Vec<usize>,
}

impl ResolvedShortcut {
    /// Joins direct shortcut labels for display inside a pill box.
    pub fn display_direct(&self) -> Option<String> {
        if self.keys.is_empty() {
            return None;
        }
        let sep = if self.keys.iter().any(|k| k.contains('/')) { " · " } else { " / " };
        Some(self.keys.join(sep))
    }
}

#[derive(Default)]
struct KeymapErrors(Vec<String>);

impl Global for KeymapErrors {}

#[derive(Default)]
struct CachedShortcuts(Vec<ResolvedShortcut>);

impl Global for CachedShortcuts {}

/// Any errors collected during the last keymap load or reload.
pub fn errors(cx: &App) -> &[String] {
    cx.try_global::<KeymapErrors>().map_or(&[], |state| state.0.as_slice())
}

pub fn set_errors(errors: Vec<String>, cx: &mut App) {
    cx.set_global(KeymapErrors(errors));
}

/// Cached resolved shortcut rows from the last keymap load/reload (no disk I/O).
pub fn cached_resolved_shortcuts(cx: &App) -> &[ResolvedShortcut] {
    cx.try_global::<CachedShortcuts>()
        .map_or(&[], |state| state.0.as_slice())
}

pub fn user_keymap_path() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join(USER_KEYMAP_FILE))
}

/// Reads `<data_dir>/keymap.json` if present (`Ok(None)` when absent). Intended for background I/O.
pub fn read_user_keymap_file() -> Result<Option<String>> {
    let path = user_keymap_path()?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => Ok(Some(contents)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Ensures `<data_dir>/keymap.json` exists with a valid empty JSON array if absent, returning its path.
pub fn ensure_user_keymap_file() -> Result<PathBuf> {
    let path = user_keymap_path()?;
    ensure_user_keymap_file_at(&path)?;
    Ok(path)
}

fn ensure_user_keymap_file_at(path: &Path) -> Result<()> {
    if !path.exists() {
        write_file_creating_parents(path, USER_KEYMAP_TEMPLATE)?;
        return Ok(());
    }
    // If the file is still an unmodified empty default template from an earlier version, refresh
    // its commented action reference without ever touching custom bindings or user-edited headers.
    if let Ok(existing) = std::fs::read_to_string(path)
        && existing.starts_with("// Fern user keymap")
        && !existing.contains("All available actions")
        && let Ok(cleaned) = theme::strip_jsonc(&existing)
        && let Ok(sections) = serde_json::from_str::<Vec<KeymapSection>>(&cleaned)
        && sections.is_empty()
    {
        write_file_creating_parents(path, USER_KEYMAP_TEMPLATE)?;
    }
    Ok(())
}

fn write_file_creating_parents(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    std::fs::write(path, contents)
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Resets `<data_dir>/keymap.json` back to the clean default template (clearing custom overrides).
pub fn reset_user_keymap_file() -> Result<()> {
    let path = user_keymap_path()?;
    write_file_creating_parents(&path, USER_KEYMAP_TEMPLATE)
}

/// Updates one action's direct binding in `<data_dir>/keymap.json` (background-safe) and returns
/// the new file contents so the caller can immediately reload GPUI bindings on the foreground thread.
pub fn write_action_shortcut_to_file(
    default_source: &str,
    action: &str,
    new_key: Option<&str>,
) -> Result<String> {
    let path = user_keymap_path()?;
    let existing = match std::fs::read_to_string(&path) {
        Ok(contents) => Some(contents),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err).with_context(|| format!("cannot read {}", path.display())),
    };
    let updated = update_user_keymap_source(default_source, existing.as_deref(), action, new_key)?;
    write_file_creating_parents(&path, &updated)?;
    Ok(updated)
}

/// Clears active bindings and reloads `default.json` plus optional `<data_dir>/keymap.json`,
/// storing any parse or action errors and the resolved shortcut list in app state.
pub fn reload(assets: &dyn AssetSource, cx: &mut App) -> Result<Vec<String>> {
    let mut read_errors = Vec::new();
    let user_keymap = match read_user_keymap_file() {
        Ok(contents) => contents,
        Err(err) => {
            read_errors.push(format!("{USER_KEYMAP_FILE}: {err}"));
            None
        }
    };
    cx.clear_key_bindings();
    match load_with_user(assets, user_keymap.as_deref(), cx) {
        Ok(mut errors) => {
            read_errors.append(&mut errors);
            set_errors(read_errors.clone(), cx);
            Ok(read_errors)
        }
        Err(err) => {
            read_errors.push(err.to_string());
            set_errors(read_errors, cx);
            Err(err)
        }
    }
}

/// Normalized comparison key for a GPUI keystroke sequence (resolves `secondary-*` to the active
/// platform modifier so `"secondary-s"` equals `"ctrl-s"` on Windows/Linux and `"cmd-s"` on macOS).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct CanonicalKeystroke {
    control: bool,
    alt: bool,
    shift: bool,
    platform: bool,
    function: bool,
    key: String,
}

fn canonicalize_sequence(raw: &str) -> Option<Vec<CanonicalKeystroke>> {
    let mut seq = Vec::new();
    for token in raw.split_whitespace() {
        let ks = Keystroke::parse(token).ok()?;
        seq.push(CanonicalKeystroke {
            control: ks.modifiers.control,
            alt: ks.modifiers.alt,
            shift: ks.modifiers.shift,
            platform: ks.modifiers.platform,
            function: ks.modifiers.function,
            key: ks.key.to_lowercase(),
        });
    }
    (!seq.is_empty()).then_some(seq)
}

fn same_keystroke_sequence(a: &str, b: &str) -> bool {
    match (canonicalize_sequence(a), canonicalize_sequence(b)) {
        (Some(ca), Some(cb)) => ca == cb,
        _ => a.trim() == b.trim(),
    }
}

/// Result of attempting to record a keystroke in the Settings → Shortcuts UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    /// Bare modifier press (`Ctrl`, `Shift`, `Alt`, `Win`, lock keys) — stay listening silently.
    IgnoreModifier,
    /// Valid shortcut converted to canonical keymap JSON syntax.
    Accepted(String),
    /// Non-modifier key rejected by this shortcut's recording constraints.
    Rejected,
}

/// Returns true when `ks` is a bare modifier or lock key that should not finalize shortcut recording.
pub fn is_bare_modifier(ks: &Keystroke) -> bool {
    let key = ks.key.to_lowercase();
    key.is_empty()
        || matches!(
            key.as_str(),
            "control"
                | "ctrl"
                | "shift"
                | "alt"
                | "option"
                | "platform"
                | "meta"
                | "super"
                | "command"
                | "cmd"
                | "win"
                | "capslock"
                | "numlock"
                | "scrolllock"
        )
}

/// Classifies a captured GPUI `Keystroke` for recording in Settings → Shortcuts.
///
/// Bare modifiers yield `IgnoreModifier` so holding `Ctrl` before pressing `S` does not flash an
/// error. Disallowed non-modifier keys yield `Rejected` so the UI can explain the constraint.
pub fn classify_keystroke(ks: &Keystroke, allow_unmodified: bool) -> RecordOutcome {
    if is_bare_modifier(ks) {
        return RecordOutcome::IgnoreModifier;
    }
    let key = ks.key.to_lowercase();
    let m = &ks.modifiers;
    let has_primary_mod = m.control || m.alt || m.platform;
    let has_any_mod = has_primary_mod || m.shift;

    // Unmodified Escape cancels recording; unmodified Backspace/Delete clears the binding.
    if !has_any_mod && matches!(key.as_str(), "escape" | "backspace" | "delete") {
        return RecordOutcome::Rejected;
    }

    let is_function_key = key
        .strip_prefix('f')
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| (1..=12).contains(&n));

    let is_named_nav = matches!(
        key.as_str(),
        "enter"
            | "tab"
            | "space"
            | "delete"
            | "backspace"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
    );

    let is_single_allowed = key.len() == 1
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '?');

    let allowed = has_primary_mod
        || is_function_key
        || (m.shift && is_named_nav)
        || (allow_unmodified && !has_any_mod && (is_named_nav || is_single_allowed));

    if !allowed {
        return RecordOutcome::Rejected;
    }

    let mut parts: Vec<&str> = Vec::new();
    if m.control {
        parts.push("ctrl");
    }
    if m.alt {
        parts.push("alt");
    }
    if m.shift && key != "?" {
        parts.push("shift");
    }
    if m.platform {
        parts.push(if cfg!(target_os = "macos") { "cmd" } else { "win" });
    }
    parts.push(&key);
    let formatted = parts.join("-");
    if Keystroke::parse(&formatted).is_err() {
        return RecordOutcome::Rejected;
    }
    RecordOutcome::Accepted(formatted)
}

/// Formats a captured GPUI `Keystroke` into canonical keymap JSON syntax when suitable for binding.
#[cfg(test)]
pub fn record_keystroke(ks: &Keystroke, allow_unmodified: bool) -> Option<String> {
    match classify_keystroke(ks, allow_unmodified) {
        RecordOutcome::Accepted(key) => Some(key),
        RecordOutcome::IgnoreModifier | RecordOutcome::Rejected => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EffectiveEntry {
    context: Option<String>,
    raw_keys: String,
    canonical: Option<Vec<CanonicalKeystroke>>,
    action: Option<String>,
}

fn resolve_effective_entries(
    default_source: &str,
    user_source: Option<&str>,
) -> Result<Vec<EffectiveEntry>> {
    let cleaned_default = theme::strip_jsonc(default_source).context("invalid keymap JSONC")?;
    let mut sections: Vec<KeymapSection> =
        serde_json::from_str(&cleaned_default).context("invalid keymap JSON")?;
    if let Some(user_source) = user_source
        && let Ok(cleaned_user) = theme::strip_jsonc(user_source)
        && !cleaned_user.trim().is_empty()
        && let Ok(user_sections) = serde_json::from_str::<Vec<KeymapSection>>(&cleaned_user)
    {
        sections.extend(user_sections);
    }

    let mut effective: Vec<EffectiveEntry> = Vec::new();
    for section in sections {
        let context = section.context.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty());
        for (raw_keys, action) in section.bindings {
            let canonical = canonicalize_sequence(&raw_keys);
            if context.is_none() && action.is_none() {
                effective.retain(|entry| !entries_same_key(entry, &raw_keys, &canonical));
            } else {
                effective.retain(|entry| {
                    !(entry.context == context && entries_same_key(entry, &raw_keys, &canonical))
                });
            }
            effective.push(EffectiveEntry { context: context.clone(), raw_keys, canonical, action });
        }
    }
    Ok(effective)
}

fn entries_same_key(
    entry: &EffectiveEntry,
    raw_keys: &str,
    canonical: &Option<Vec<CanonicalKeystroke>>,
) -> bool {
    match (&entry.canonical, canonical) {
        (Some(a), Some(b)) => a == b,
        _ => entry.raw_keys == raw_keys,
    }
}

/// Returns whether two GPUI context strings can be active at the same time in a user window.
fn contexts_overlap(a: Option<&str>, b: Option<&str>) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return true;
    };
    if a == b {
        return true;
    }
    fn root(ctx: &str) -> &str {
        ctx.split("&&").next().unwrap_or(ctx).trim()
    }
    let ra = root(a);
    let rb = root(b);
    if ra == rb {
        return true;
    }
    let in_main_workspace = |r: &str| {
        matches!(
            r,
            "Workspace" | "Tasks" | "ThingsView" | "TaskList" | "TaskDetail" | "Calendar" | "CalendarYear" | "Habits" | "Matrix" | "Focus" | "Countdown"
        )
    };
    (ra == "Workspace" && in_main_workspace(rb)) || (rb == "Workspace" && in_main_workspace(ra))
}

fn is_fixed_chord_match(spec: &ShortcutSpec, entry: &EffectiveEntry) -> bool {
    let Some((fixed_ctx, fixed_keys)) = spec.fixed_chord else {
        return false;
    };
    entry.context.as_deref() == Some(fixed_ctx) && same_keystroke_sequence(&entry.raw_keys, fixed_keys)
}

/// Resolves every row in `SHORTCUT_SPECS` after applying optional user `keymap.json` overrides,
/// including active fixed chords, direct bindings, and cross-action conflicts.
pub fn resolve_shortcuts_with_user(
    default_source: &str,
    user_source: Option<&str>,
) -> Result<Vec<ResolvedShortcut>> {
    let effective = resolve_effective_entries(default_source, user_source)?;

    // Track active listed entries for conflict detection: (spec_index, context, canonical_keys).
    let mut active_listed: Vec<(usize, Option<&str>, &Vec<CanonicalKeystroke>)> = Vec::new();
    let mut resolved: Vec<ResolvedShortcut> = SHORTCUT_SPECS
        .iter()
        .enumerate()
        .map(|(index, &spec)| {
            let mut fixed_chord_active = false;
            let mut keys = Vec::new();
            for entry in &effective {
                if entry.action.as_deref() != Some(spec.action) {
                    continue;
                }
                if let Some(canonical) = &entry.canonical {
                    active_listed.push((index, entry.context.as_deref(), canonical));
                }
                if is_fixed_chord_match(&spec, entry) {
                    fixed_chord_active = true;
                    continue;
                }
                let display = display_keys(&entry.raw_keys);
                if !keys.contains(&display) {
                    keys.push(display);
                }
            }
            let fixed_chord_parts = fixed_chord_active
                .then(|| spec.fixed_chord.and_then(|(_, raw)| split_chord_display(raw)))
                .flatten();
            ResolvedShortcut {
                index,
                spec,
                fixed_chord_parts,
                keys,
                conflicts: Vec::new(),
            }
        })
        .collect();

    // Detect conflicts between distinct listed actions that share a keystroke in overlapping contexts.
    for i in 0..active_listed.len() {
        let (idx_a, ctx_a, key_a) = active_listed[i];
        let act_a = SHORTCUT_SPECS[idx_a].action;
        for &(idx_b, ctx_b, key_b) in &active_listed[(i + 1)..] {
            if idx_a == idx_b || key_a != key_b {
                continue;
            }
            let act_b = SHORTCUT_SPECS[idx_b].action;
            // `DismissTransient` intentionally propagates to `MinimizeWindow` when nothing transient is open.
            let is_dismiss_minimize = (act_a == "fern::DismissTransient"
                && act_b == "fern::MinimizeWindow")
                || (act_b == "fern::DismissTransient" && act_a == "fern::MinimizeWindow");
            if is_dismiss_minimize {
                continue;
            }
            if contexts_overlap(ctx_a, ctx_b) {
                if !resolved[idx_a].conflicts.contains(&idx_b) {
                    resolved[idx_a].conflicts.push(idx_b);
                }
                if !resolved[idx_b].conflicts.contains(&idx_a) {
                    resolved[idx_b].conflicts.push(idx_a);
                }
            }
        }
    }

    Ok(resolved)
}

/// Splits a two-key chord like `"g t"` into `("G", "T")` so callers can localize `"G then T"`.
fn split_chord_display(raw: &str) -> Option<(String, String)> {
    let mut parts = raw.split_whitespace();
    let first = display_single_keystroke(parts.next()?);
    let second = display_single_keystroke(parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some((first, second))
}

/// Extracts any leading whitespace and `//` / `/* ... */` comments before the JSON array `[`.
fn extract_leading_comments(source: &str) -> &str {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            _ => break,
        }
    }
    &source[..i]
}

/// Updates one action's direct binding in user `keymap.json` source while preserving:
/// - leading JSONC header comments,
/// - all other user sections and overrides,
/// - built-in fixed chords (`g t`, `v l`, etc.).
///
/// Returns an error without modifying anything if `user_source` contains invalid JSON/JSONC.
pub fn update_user_keymap_source(
    default_source: &str,
    user_source: Option<&str>,
    action: &str,
    new_key: Option<&str>,
) -> Result<String> {
    let spec = SHORTCUT_SPECS
        .iter()
        .copied()
        .find(|s| s.action == action)
        .with_context(|| format!("unknown shortcut action: {action}"))?;

    let cleaned_default = theme::strip_jsonc(default_source).context("invalid default keymap JSONC")?;
    let default_sections: Vec<KeymapSection> =
        serde_json::from_str(&cleaned_default).context("invalid default keymap JSON")?;

    // Collect all default direct bindings `(context, raw_key)` for this action (excluding fixed_chord).
    let mut default_direct: Vec<(Option<String>, String)> = Vec::new();
    for section in &default_sections {
        let ctx = section.context.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(str::to_owned);
        for (raw_key, bound) in &section.bindings {
            if bound.as_deref() != Some(action) {
                continue;
            }
            if let Some((fixed_ctx, fixed_key)) = spec.fixed_chord
                && ctx.as_deref() == Some(fixed_ctx)
                && same_keystroke_sequence(raw_key, fixed_key)
            {
                continue;
            }
            default_direct.push((ctx.clone(), raw_key.clone()));
        }
    }

    let (header, mut user_sections) = match user_source {
        Some(src) if !src.trim().is_empty() => {
            let header = extract_leading_comments(src);
            let cleaned = theme::strip_jsonc(src)
                .with_context(|| format!("{USER_KEYMAP_FILE}: invalid JSONC"))?;
            let sections: Vec<KeymapSection> = if cleaned.trim().is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&cleaned)
                    .with_context(|| format!("{USER_KEYMAP_FILE}: invalid JSON"))?
            };
            (header, sections)
        }
        _ => (extract_leading_comments(USER_KEYMAP_TEMPLATE), Vec::new()),
    };

    // Contexts affected by this action's direct bindings.
    let mut target_contexts: BTreeSet<Option<String>> = BTreeSet::new();
    target_contexts.insert(spec.context.map(str::to_owned));
    for (ctx, _) in &default_direct {
        target_contexts.insert(ctx.clone());
    }

    // Remove existing user overrides for this action's direct bindings (both `Some(action)` and
    // previous `null` unbindings of its default keys), leaving fixed chords & other actions untouched.
    for section in &mut user_sections {
        let sec_ctx = section.context.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(str::to_owned);
        section.bindings.retain(|raw_key, bound| {
            if let Some((fixed_ctx, fixed_key)) = spec.fixed_chord
                && sec_ctx.as_deref() == Some(fixed_ctx)
                && same_keystroke_sequence(raw_key, fixed_key)
            {
                return true;
            }
            if bound.as_deref() == Some(action) {
                return false;
            }
            if bound.is_none()
                && default_direct
                    .iter()
                    .any(|(def_ctx, def_key)| {
                        (sec_ctx.is_none() || *def_ctx == sec_ctx)
                            && same_keystroke_sequence(raw_key, def_key)
                    })
            {
                return false;
            }
            true
        });
    }

    let mut set_binding = |ctx: Option<String>, key: String, val: Option<String>| {
        if let Some(section) = user_sections.iter_mut().find(|s| {
            s.context.as_deref().map(str::trim).filter(|c| !c.is_empty()) == ctx.as_deref()
        }) {
            section.bindings.insert(key, val);
        } else {
            let mut bindings = BTreeMap::new();
            bindings.insert(key, val);
            user_sections.push(KeymapSection { context: ctx, bindings });
        }
    };

    match new_key {
        None => {
            // Unbind every built-in direct key for this action.
            for (def_ctx, def_key) in &default_direct {
                set_binding(def_ctx.clone(), def_key.clone(), None);
            }
        }
        Some(new_key) => {
            let mut already_bound_in_primary = false;
            for (def_ctx, def_key) in &default_direct {
                if same_keystroke_sequence(def_key, new_key) {
                    if def_ctx.as_deref() == spec.context {
                        already_bound_in_primary = true;
                    }
                } else {
                    set_binding(def_ctx.clone(), def_key.clone(), None);
                }
            }
            // Write the new key in the primary context (and mirror to any sibling window contexts
            // like `StickyNote` that share this action in `default.json`).
            if !already_bound_in_primary {
                set_binding(
                    spec.context.map(str::to_owned),
                    new_key.to_owned(),
                    Some(action.to_owned()),
                );
            }
            for ctx in &target_contexts {
                if ctx.as_deref() == spec.context
                    || ctx.as_deref() == Some("Workspace && !TextInput")
                {
                    continue;
                }
                let already_there = default_direct.iter().any(|(def_ctx, def_key)| {
                    def_ctx == ctx && same_keystroke_sequence(def_key, new_key)
                });
                if !already_there {
                    set_binding(ctx.clone(), new_key.to_owned(), Some(action.to_owned()));
                }
            }
        }
    }

    user_sections.retain(|s| !s.bindings.is_empty());

    let json = serde_json::to_string_pretty(&user_sections)
        .context("failed to serialize keymap sections")?;
    let mut out = String::with_capacity(header.len() + json.len() + 1);
    out.push_str(header);
    out.push_str(&json);
    out.push('\n');
    Ok(out)
}

fn format_key_name(key: &str) -> String {
    match key {
        "down" => "↓".to_owned(),
        "up" => "↑".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "delete" => "Del".to_owned(),
        "escape" => "Esc".to_owned(),
        "space" => "Space".to_owned(),
        "enter" => "Enter".to_owned(),
        "tab" => "Tab".to_owned(),
        "backspace" => "Backspace".to_owned(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
        }
    }
}

fn display_single_keystroke(raw: &str) -> String {
    if let Ok(ks) = Keystroke::parse(raw) {
        let mut parts: Vec<String> = Vec::new();
        if cfg!(target_os = "macos") {
            if ks.modifiers.control {
                parts.push("⌃".to_owned());
            }
            if ks.modifiers.alt {
                parts.push("⌥".to_owned());
            }
            if ks.modifiers.shift {
                parts.push("⇧".to_owned());
            }
            if ks.modifiers.platform {
                parts.push("⌘".to_owned());
            }
            parts.push(format_key_name(&ks.key));
            return parts.concat();
        }
        if ks.modifiers.control {
            parts.push("Ctrl".to_owned());
        }
        if ks.modifiers.alt {
            parts.push("Alt".to_owned());
        }
        if ks.modifiers.shift {
            parts.push("Shift".to_owned());
        }
        if ks.modifiers.platform {
            parts.push("Win".to_owned());
        }
        parts.push(format_key_name(&ks.key));
        return parts.join("+");
    }
    // Fallback for any unparseable keystroke token.
    format_key_name(raw)
}

/// "secondary-n" → "Ctrl+N" (`⌘N` on macOS), "down" → "↓", "space" → "Space".
pub fn display_keys(keystroke: &str) -> String {
    let chords: Vec<String> = keystroke
        .split_whitespace()
        .map(display_single_keystroke)
        .collect();
    chords.join(", ")
}

#[derive(Deserialize, Serialize)]
struct KeymapSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    #[serde(default)]
    bindings: BTreeMap<String, Option<String>>,
}

/// Loads the default keymap from the assets. Returns the bindings that could not be loaded,
/// so a typo in the keymap does not break the rest of it.
pub fn load_default(assets: &dyn AssetSource, cx: &mut App) -> Result<Vec<String>> {
    load_with_user(assets, None, cx)
}

/// Loads the default keymap from assets and layers an optional user `keymap.json` on top of it.
/// Any syntax or binding error in the user keymap is reported in the returned error list without
/// breaking the default keymap.
pub fn load_with_user(
    assets: &dyn AssetSource,
    user_source: Option<&str>,
    cx: &mut App,
) -> Result<Vec<String>> {
    let source = assets
        .load(DEFAULT_KEYMAP_PATH)?
        .with_context(|| format!("{DEFAULT_KEYMAP_PATH} is missing"))?;
    let default_str = std::str::from_utf8(&source)?;
    let mut errors = load_with_meta(default_str, DEFAULT_META, cx)?;
    let trimmed_user = user_source.map(str::trim).filter(|s| !s.is_empty());
    if let Some(user_src) = trimmed_user {
        match load_with_meta(user_src, USER_META, cx) {
            Ok(user_errors) => errors.extend(user_errors),
            Err(err) => errors.push(format!("{USER_KEYMAP_FILE}: {err}")),
        }
    }
    if let Ok(resolved) = resolve_shortcuts_with_user(default_str, trimmed_user) {
        cx.set_global(CachedShortcuts(resolved));
    }
    Ok(errors)
}

pub fn load(source: &str, cx: &mut App) -> Result<Vec<String>> {
    load_with_meta(source, USER_META, cx)
}

fn load_with_meta(source: &str, meta: KeyBindingMetaIndex, cx: &mut App) -> Result<Vec<String>> {
    let cleaned = theme::strip_jsonc(source).context("invalid keymap JSONC")?;
    let sections: Vec<KeymapSection> = serde_json::from_str(&cleaned).context("invalid keymap JSON")?;
    let mut bindings = Vec::new();
    let mut errors = Vec::new();
    for section in sections {
        let predicate = match section.context.as_deref().map(KeyBindingContextPredicate::parse) {
            Some(Ok(predicate)) => Some(Rc::new(predicate)),
            Some(Err(err)) => {
                errors.push(format!("context {:?}: {err}", section.context));
                continue;
            }
            None => None,
        };
        for (keystrokes, action_name) in section.bindings {
            let action = match action_name {
                Some(action_name) => match cx.build_action(&action_name, None) {
                    Ok(action) => action,
                    Err(err) => {
                        errors.push(format!("{keystrokes}: {action_name}: {err}"));
                        continue;
                    }
                },
                None => NoAction.boxed_clone(),
            };
            match KeyBinding::load(
                &keystrokes,
                action,
                predicate.clone(),
                false,
                None,
                cx.keyboard_mapper().as_ref(),
            ) {
                Ok(binding) => bindings.push(binding.with_meta(meta)),
                Err(err) => errors.push(format!("{keystrokes}: {err}")),
            }
        }
    }
    cx.bind_keys(bindings);
    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    const DEFAULT_JSON: &str = include_str!("../../../assets/keymaps/default.json");

    #[test]
    fn resolved_shortcuts_read_defaults_without_conflicts() {
        let list = resolve_shortcuts_with_user(DEFAULT_JSON, None).unwrap();
        assert_eq!(list.len(), SHORTCUT_SPECS.len());
        assert_eq!(SHORTCUT_SPECS.len(), fern_i18n::Locale::En.strings().shortcut_labels.len());
        let find = |action: &str| list.iter().find(|r| r.spec.action == action).unwrap();

        assert!(!find("fern::NewTask").keys.is_empty());
        assert_eq!(find("fern::DismissTransient").keys, ["Esc"]);
        let mut next = find("task_list::SelectNext").keys.clone();
        next.sort();
        assert_eq!(next, ["J", "↓"]);
        assert_eq!(find("task_list::PinTask").keys, ["Tab, P"]);
        assert_eq!(
            find("fern::ShowToday").fixed_chord_parts,
            Some(("G".to_owned(), "T".to_owned()))
        );
        assert_eq!(
            find("task_list::ViewKanban").fixed_chord_parts,
            Some(("V".to_owned(), "K".to_owned()))
        );
        assert_eq!(find("sticky::TogglePin").keys, ["F12"]);
        for row in &list {
            assert!(
                row.conflicts.is_empty(),
                "default keymap should not have conflicts, got {:?} for {}",
                row.conflicts,
                row.spec.action
            );
        }
        if cfg!(target_os = "macos") {
            assert_eq!(display_keys("secondary-shift-p"), "⇧⌘P");
            assert_eq!(display_keys("ctrl-shift-m"), "⌃⇧M");
            assert_eq!(display_keys("secondary--"), "⌘-");
        } else {
            assert_eq!(display_keys("secondary-shift-p"), "Ctrl+Shift+P");
            assert_eq!(display_keys("ctrl-shift-m"), "Ctrl+Shift+M");
            assert_eq!(display_keys("secondary--"), "Ctrl+-");
        }
    }

    #[gpui::test]
    fn default_keymap_loads_without_errors(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let errors = load_default(&assets::Assets, cx).unwrap();
            assert!(errors.is_empty(), "keymap errors: {errors:?}");
        });
    }

    #[gpui::test]
    fn unknown_actions_are_reported(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let errors = load(r#"[{"bindings": {"ctrl-x": "fern::NoSuchAction"}}]"#, cx).unwrap();
            assert_eq!(errors.len(), 1);
        });
    }

    #[gpui::test]
    fn user_keymap_overrides_unbinds_and_adds_minimize_window(cx: &mut TestAppContext) {
        let user_keymap = r#"[
            {
                "context": "Workspace && !TextInput",
                "bindings": {
                    "escape": "fern::MinimizeWindow"
                }
            },
            {
                "context": "TaskList",
                "bindings": {
                    "space": null
                }
            },
            {
                "bindings": {
                    "secondary-q": null,
                    "ctrl-shift-m": "fern::NewTask"
                }
            }
        ]"#;

        cx.update(|cx| {
            let errors = load_with_user(&assets::Assets, Some(user_keymap), cx).unwrap();
            assert!(errors.is_empty(), "user keymap errors: {errors:?}");

            // Broken user JSON reports an error without failing default bindings.
            let broken_errors = load_with_user(&assets::Assets, Some("{not valid json"), cx).unwrap();
            assert_eq!(broken_errors.len(), 1);
            assert!(broken_errors[0].contains(USER_KEYMAP_FILE));
        });

        let list = resolve_shortcuts_with_user(DEFAULT_JSON, Some(user_keymap)).unwrap();
        let find = |action: &str| list.iter().find(|r| r.spec.action == action).unwrap();
        assert_eq!(find("fern::MinimizeWindow").keys, ["Esc"]);
        assert_eq!(find("fern::DismissTransient").keys, ["Esc"]);
        assert!(find("fern::MinimizeWindow").conflicts.is_empty());
        assert!(find("fern::Quit").keys.is_empty());
        assert!(find("fern::NewTask").keys.contains(&display_keys("ctrl-shift-m")));
        assert_eq!(find("task_list::ToggleComplete").keys, ["Tab, M"]);
    }

    #[test]
    fn update_user_keymap_preserves_comments_other_rules_and_fixed_chords() {
        let existing = r#"// Personal header comment
[
  {
    "context": "Workspace && !TextInput",
    "bindings": {
      "escape": "fern::MinimizeWindow"
    }
  }
]
"#;
        // Rebind ShowToday direct slot without touching `g t` or the custom MinimizeWindow rule.
        let updated = update_user_keymap_source(
            DEFAULT_JSON,
            Some(existing),
            "fern::ShowToday",
            Some("ctrl-alt-2"),
        )
        .unwrap();
        assert!(updated.starts_with("// Personal header comment\n"));
        let resolved = resolve_shortcuts_with_user(DEFAULT_JSON, Some(&updated)).unwrap();
        let find = |action: &str| resolved.iter().find(|r| r.spec.action == action).unwrap();
        assert_eq!(find("fern::MinimizeWindow").keys, ["Esc"]);
        assert_eq!(
            find("fern::ShowToday").fixed_chord_parts,
            Some(("G".to_owned(), "T".to_owned()))
        );
        assert_eq!(find("fern::ShowToday").keys, [display_keys("ctrl-alt-2")]);

        // Clearing ShowToday direct slot unbinds default direct key while keeping `g t`.
        let cleared =
            update_user_keymap_source(DEFAULT_JSON, Some(&updated), "fern::ShowToday", None)
                .unwrap();
        let resolved_cleared = resolve_shortcuts_with_user(DEFAULT_JSON, Some(&cleared)).unwrap();
        let today = resolved_cleared
            .iter()
            .find(|r| r.spec.action == "fern::ShowToday")
            .unwrap();
        assert_eq!(today.fixed_chord_parts, Some(("G".to_owned(), "T".to_owned())));
        assert!(today.keys.is_empty());

        // Setting ShowToday back to its platform default removes the Workspace override cleanly.
        let default_key = if cfg!(target_os = "macos") { "cmd-2" } else { "ctrl-2" };
        let restored = update_user_keymap_source(
            DEFAULT_JSON,
            Some(&cleared),
            "fern::ShowToday",
            Some(default_key),
        )
        .unwrap();
        assert!(!restored.contains("ShowToday"));
        assert!(restored.contains("fern::MinimizeWindow"));

        // Broken user JSON is never silently overwritten.
        assert!(
            update_user_keymap_source(DEFAULT_JSON, Some("{broken"), "fern::SyncNow", Some("ctrl-r"))
                .is_err()
        );
    }

    #[test]
    fn conflict_detection_flags_overlapping_contexts_only() {
        let conflicting = r#"[
            {
                "context": "Workspace",
                "bindings": {
                    "secondary-s": "fern::PrintList"
                }
            },
            {
                "context": "StickyNote",
                "bindings": {
                    "secondary-n": "sticky::TogglePin"
                }
            }
        ]"#;
        // Note: binding secondary-s in Workspace replaces SyncNow in effective resolution; to create
        // a real conflict between two distinct active actions, bind PrintList globally to secondary-n
        // while NewTask is bound in Workspace.
        let overlap = r#"[
            {
                "bindings": {
                    "secondary-n": "fern::PrintList"
                }
            },
            {
                "context": "Workspace",
                "bindings": {
                    "secondary-n": "fern::NewTask"
                }
            },
            {
                "context": "StickyNote",
                "bindings": {
                    "secondary-n": "sticky::TogglePin"
                }
            }
        ]"#;
        let _ = conflicting;
        let list = resolve_shortcuts_with_user(DEFAULT_JSON, Some(overlap)).unwrap();
        let idx = |action: &str| SHORTCUT_SPECS.iter().position(|s| s.action == action).unwrap();
        let print_idx = idx("fern::PrintList");
        let new_task_idx = idx("fern::NewTask");
        let sticky_idx = idx("sticky::TogglePin");
        assert!(list[print_idx].conflicts.contains(&new_task_idx));
        assert!(list[new_task_idx].conflicts.contains(&print_idx));
        // StickyNote overlaps with global `None` (PrintList), but NOT with Workspace (NewTask).
        assert!(!list[sticky_idx].conflicts.contains(&new_task_idx));
    }

    #[test]
    fn record_keystroke_handles_modifiers_function_keys_and_rejections() {
        assert_eq!(
            record_keystroke(&Keystroke::parse("ctrl-shift-p").unwrap(), false).as_deref(),
            Some("ctrl-shift-p")
        );
        assert_eq!(
            record_keystroke(&Keystroke::parse("alt-delete").unwrap(), false).as_deref(),
            Some("alt-delete")
        );
        assert_eq!(
            record_keystroke(&Keystroke::parse("f12").unwrap(), false).as_deref(),
            Some("f12")
        );
        assert_eq!(
            record_keystroke(&Keystroke::parse("shift-enter").unwrap(), false).as_deref(),
            Some("shift-enter")
        );
        // Unmodified Escape / Backspace / Delete are reserved for cancel / clear.
        assert_eq!(record_keystroke(&Keystroke::parse("escape").unwrap(), true), None);
        assert_eq!(record_keystroke(&Keystroke::parse("backspace").unwrap(), true), None);
        assert_eq!(record_keystroke(&Keystroke::parse("delete").unwrap(), true), None);
        // Bare letters rejected in Workspace, accepted in allow_unmodified contexts.
        assert_eq!(
            classify_keystroke(&Keystroke::parse("a").unwrap(), false),
            RecordOutcome::Rejected
        );
        assert_eq!(record_keystroke(&Keystroke::parse("a").unwrap(), true).as_deref(), Some("a"));
        assert_eq!(
            classify_keystroke(&Keystroke::parse("space").unwrap(), false),
            RecordOutcome::Rejected
        );
        assert_eq!(
            record_keystroke(&Keystroke::parse("space").unwrap(), true).as_deref(),
            Some("space")
        );
        assert_eq!(
            classify_keystroke(
                &Keystroke { key: "control".to_owned(), ..Default::default() },
                false
            ),
            RecordOutcome::IgnoreModifier
        );
    }

    #[test]
    fn user_keymap_template_documents_all_shortcut_specs_and_upgrades_empty_legacy_header() {
        for spec in SHORTCUT_SPECS {
            assert!(
                USER_KEYMAP_TEMPLATE.contains(spec.action),
                "USER_KEYMAP_TEMPLATE should document {}",
                spec.action
            );
        }
        let dir = std::env::temp_dir().join(format!("fern-keymap-upgrade-{}", fern_core::Id::now_v7()));
        let path = dir.join(USER_KEYMAP_FILE);
        // Old empty auto-generated template gets upgraded cleanly.
        write_file_creating_parents(&path, "// Fern user keymap — old header\n[\n]\n").unwrap();
        ensure_user_keymap_file_at(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), USER_KEYMAP_TEMPLATE);

        // User-written empty file with custom comment is never overwritten.
        let custom_comment = "// My own notes\n[]\n";
        std::fs::write(&path, custom_comment).unwrap();
        ensure_user_keymap_file_at(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom_comment);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_user_keymap_creates_valid_template_without_overwriting() {
        let dir = std::env::temp_dir().join(format!("fern-keymap-test-{}", fern_core::Id::now_v7()));
        let path = dir.join(USER_KEYMAP_FILE);
        ensure_user_keymap_file_at(&path).unwrap();
        let created = std::fs::read_to_string(&path).unwrap();
        assert_eq!(created, USER_KEYMAP_TEMPLATE);
        assert!(resolve_shortcuts_with_user(DEFAULT_JSON, Some(&created)).is_ok());

        let commented = r#"
            // Comment before array
            [
              {
                /* Block comment */
                "context": "Workspace && !TextInput",
                "bindings": {
                  "escape": "fern::MinimizeWindow" // Inline comment
                }
              }
            ]
        "#;
        let list = resolve_shortcuts_with_user(DEFAULT_JSON, Some(commented)).unwrap();
        let minimize = list
            .iter()
            .find(|r| r.spec.action == "fern::MinimizeWindow")
            .unwrap();
        assert_eq!(minimize.keys, ["Esc"]);

        let custom = r#"[{"bindings": {"escape": "fern::MinimizeWindow"}}]"#;
        std::fs::write(&path, custom).unwrap();
        ensure_user_keymap_file_at(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn things_view_secondary_s_opens_when_without_breaking_workspace_sync_now(
        cx: &mut TestAppContext,
    ) {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use gpui::{
            Context, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render,
            Window, div,
        };

        struct Harness {
            workspace_focus: FocusHandle,
            things_focus: FocusHandle,
            propagate_when: bool,
            when_hits: Arc<AtomicUsize>,
            sync_hits: Arc<AtomicUsize>,
        }

        impl Render for Harness {
            fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
                let when_hits = Arc::clone(&self.when_hits);
                let sync_hits = Arc::clone(&self.sync_hits);
                let propagate_when = self.propagate_when;
                div()
                    .key_context("Workspace")
                    .track_focus(&self.workspace_focus)
                    .on_action(move |_: &crate::actions::SyncNow, _, _| {
                        sync_hits.fetch_add(1, Ordering::SeqCst);
                    })
                    .child(
                        div()
                            .key_context("ThingsView")
                            .track_focus(&self.things_focus)
                            .on_action(move |_: &crate::actions::OpenWhen, _, cx| {
                                when_hits.fetch_add(1, Ordering::SeqCst);
                                if propagate_when {
                                    cx.propagate();
                                }
                            }),
                    )
            }
        }

        cx.update(|cx| {
            let errors = load_default(&assets::Assets, cx).unwrap();
            assert!(errors.is_empty(), "keymap errors: {errors:?}");
        });

        let when_hits = Arc::new(AtomicUsize::new(0));
        let sync_hits = Arc::new(AtomicUsize::new(0));
        let (view, wcx) = cx.add_window_view(|_, cx| Harness {
            workspace_focus: cx.focus_handle(),
            things_focus: cx.focus_handle(),
            propagate_when: false,
            when_hits: Arc::clone(&when_hits),
            sync_hits: Arc::clone(&sync_hits),
        });

        // 1. Inside ThingsView, secondary-s dispatches things::OpenWhen (NOT fern::SyncNow).
        wcx.update(|window, cx| {
            let handle = view.read(cx).things_focus.clone();
            handle.focus(window, cx);
        });
        wcx.simulate_keystrokes("secondary-s");
        assert_eq!(when_hits.load(Ordering::SeqCst), 1);
        assert_eq!(sync_hits.load(Ordering::SeqCst), 0);

        // 2. When ThingsView propagates OpenWhen (e.g. no task active), Workspace SyncNow runs.
        wcx.update(|_, cx| {
            view.update(cx, |this, cx| {
                this.propagate_when = true;
                cx.notify();
            });
        });
        wcx.run_until_parked();
        wcx.simulate_keystrokes("secondary-s");
        assert_eq!(when_hits.load(Ordering::SeqCst), 2);
        assert_eq!(sync_hits.load(Ordering::SeqCst), 1);

        // 3. Outside ThingsView (focus on Workspace), secondary-s dispatches fern::SyncNow directly.
        wcx.update(|window, cx| {
            let handle = view.read(cx).workspace_focus.clone();
            handle.focus(window, cx);
        });
        wcx.simulate_keystrokes("secondary-s");
        assert_eq!(when_hits.load(Ordering::SeqCst), 2);
        assert_eq!(sync_hits.load(Ordering::SeqCst), 2);
    }
}
