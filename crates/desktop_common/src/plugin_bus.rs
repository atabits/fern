//! Shared GPUI global event/UI bus between `plugin_ui` and `apps/desktop`.

use std::collections::VecDeque;
use std::str::FromStr as _;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use chrono::{NaiveDate, NaiveTime, Timelike as _};
use fern_core::{Horizon, Id, Kind, Priority, Recurrence, ScheduleBucket, Task, TaskDraft};
use fern_i18n as i18n;
use gpui::{App, AppContext as _, Global};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastLevel {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginToast {
    pub id: u64,
    pub plugin_id: String,
    pub message: String,
    pub level: ToastLevel,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PluginModalKind {
    /// Declarative `UiNode` tree serialized as JSON (parsed by `plugin_ui`).
    View { tree: serde_json::Value },
    /// Yes/No confirmation prompt.
    Confirm { body: String, request_id: u64 },
    /// Text input prompt.
    Prompt { placeholder: String, default_value: String, request_id: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginModalSpec {
    pub plugin_id: String,
    pub view_id: String,
    pub title: String,
    pub kind: PluginModalKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginCommandSpec {
    pub plugin_id: String,
    pub command_id: String,
    pub title: String,
    pub plugin_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSidebarViewSpec {
    pub plugin_id: String,
    pub view_id: String,
    pub title: String,
    pub icon_svg: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginSearchHit {
    pub id: String,
    pub plugin_id: String,
    #[serde(default)]
    pub provider_id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub badge: Option<String>,
    #[serde(default)]
    pub task_id: Option<Id>,
    #[serde(default)]
    pub command_id: Option<String>,
    #[serde(default)]
    pub command_arg: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginNavAction {
    OpenTask(Id),
    OpenPluginView { plugin_id: String, view_id: String },
    OpenSettingsPlugins,
    RunCommand { plugin_id: String, command_id: String, arg: Option<String> },
}

/// Reserved for future Yellow-Zone task row badges.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskBadgeSpec {
    pub id: String,
    pub plugin_id: String,
    pub label: String,
    pub color: Option<String>,
}

/// Reserved for future Yellow-Zone task context/detail actions.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskActionSpec {
    pub id: String,
    pub plugin_id: String,
    pub title: String,
    pub icon: Option<String>,
}

/// Reserved for future Yellow-Zone plugin-backed virtual lists.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VirtualListSpec {
    pub id: String,
    pub plugin_id: String,
    pub title: String,
    pub icon: Option<String>,
}

/// GPUI global bus coordinating plugin UI notifications, modals, commands, search, and navigation.
#[derive(Default)]
pub struct PluginBus {
    pub revision: u64,
    next_toast_id: u64,
    pub toasts: Vec<PluginToast>,
    pub active_modal: Option<PluginModalSpec>,
    pub commands: Vec<PluginCommandSpec>,
    pub sidebar_views: Vec<PluginSidebarViewSpec>,
    pub search_hits: Vec<PluginSearchHit>,
    pub pending_nav: VecDeque<PluginNavAction>,
    // Reserved Yellow-Zone contribution registries.
    pub task_badges: Vec<TaskBadgeSpec>,
    pub task_actions: Vec<TaskActionSpec>,
    pub virtual_lists: Vec<VirtualListSpec>,
}

impl Global for PluginBus {}

impl PluginBus {
    pub fn global(cx: &App) -> Option<&Self> {
        cx.try_global::<Self>()
    }

    pub fn update<R>(cx: &mut App, f: impl FnOnce(&mut Self) -> R) -> R {
        let bus = cx.default_global::<Self>();
        bus.revision = bus.revision.wrapping_add(1);
        f(bus)
    }

    pub fn push_toast(
        &mut self,
        plugin_id: impl Into<String>,
        message: impl Into<String>,
        level: ToastLevel,
        created_at_ms: i64,
    ) -> u64 {
        self.next_toast_id += 1;
        let id = self.next_toast_id;
        self.toasts.push(PluginToast {
            id,
            plugin_id: plugin_id.into(),
            message: message.into(),
            level,
            created_at_ms,
        });
        id
    }

    pub fn dismiss_toast(&mut self, id: u64) {
        self.toasts.retain(|t| t.id != id);
    }

    pub fn open_modal(&mut self, modal: PluginModalSpec) {
        self.active_modal = Some(modal);
    }

    pub fn close_modal(&mut self) {
        self.active_modal = None;
    }

    pub fn push_nav(&mut self, action: PluginNavAction) {
        self.pending_nav.push_back(action);
    }

    pub fn drain_nav(&mut self) -> Vec<PluginNavAction> {
        self.pending_nav.drain(..).collect()
    }
}

/// Outcome of running a plugin lifecycle/transform hook (`beforeTaskCreate`, `beforeTaskComplete`,
/// `quickAddTransform`). Mirrors `plugins::runtime::HookDecision` without requiring feature crates
/// to depend on `plugins` or `plugin_ui`.
#[derive(Clone, Debug, PartialEq)]
pub enum PluginHookDecision {
    Allow,
    Modify(serde_json::Value),
    Cancel(String),
}

/// Thread-safe hook closure taking a JSON payload (task draft, task snapshot, or raw quick-add
/// string as `Value::String`) and returning a [`PluginHookDecision`].
pub type PluginHookFn = Arc<dyn Fn(serde_json::Value) -> Result<PluginHookDecision> + Send + Sync>;

/// Optional GPUI global hook bridge registered by `apps/desktop` (or `plugin_ui`) so `task_ui`,
/// `todo_ui`, `things_ui`, and desktop modals can invoke plugin hooks off the UI thread without
/// cross-UI crate dependencies.
#[derive(Clone)]
pub struct PluginHooksGlobal {
    pub before_task_create: PluginHookFn,
    pub before_task_complete: PluginHookFn,
    pub quick_add_transform: PluginHookFn,
    is_active: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl Global for PluginHooksGlobal {}

impl Default for PluginHooksGlobal {
    fn default() -> Self {
        let allow: PluginHookFn = Arc::new(|_| Ok(PluginHookDecision::Allow));
        Self {
            before_task_create: Arc::clone(&allow),
            before_task_complete: Arc::clone(&allow),
            quick_add_transform: allow,
            is_active: Some(Arc::new(|| false)),
        }
    }
}

impl PluginHooksGlobal {
    pub fn new(
        before_task_create: PluginHookFn,
        before_task_complete: PluginHookFn,
        quick_add_transform: PluginHookFn,
    ) -> Self {
        Self {
            before_task_create,
            before_task_complete,
            quick_add_transform,
            is_active: None,
        }
    }

    /// Attaches a fast lock-free/cheap check indicating whether plugin hooks are currently active
    /// (for example `false` in Safe Mode or when no plugins are enabled).
    pub fn with_active_check(
        mut self,
        is_active: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        self.is_active = Some(is_active);
        self
    }

    /// Clones the registered hook bridge so callers can move it straight into `cx.background_spawn`.
    pub fn global(cx: &App) -> Option<Self> {
        cx.try_global::<Self>().cloned()
    }

    /// Registers or replaces the global plugin hook bridge.
    pub fn set_global(hooks: Self, cx: &mut App) {
        cx.set_global(hooks);
    }

    /// Removes the global plugin hook bridge (e.g. on shutdown or entering Safe Mode).
    pub fn clear_global(cx: &mut App) {
        if cx.has_global::<Self>() {
            cx.remove_global::<Self>();
        }
    }

    /// Returns `true` when hooks should be invoked (defaults to `true` when registered without a
    /// custom active predicate).
    pub fn is_active(&self) -> bool {
        self.is_active.as_ref().is_none_or(|check| check())
    }

    pub fn run_before_task_create(&self, draft: serde_json::Value) -> Result<PluginHookDecision> {
        if !self.is_active() {
            return Ok(PluginHookDecision::Allow);
        }
        (self.before_task_create)(draft)
    }

    pub fn run_before_task_complete(&self, task: serde_json::Value) -> Result<PluginHookDecision> {
        if !self.is_active() {
            return Ok(PluginHookDecision::Allow);
        }
        (self.before_task_complete)(task)
    }

    pub fn run_quick_add_transform(
        &self,
        raw_text: impl Into<String>,
    ) -> Result<PluginHookDecision> {
        if !self.is_active() {
            return Ok(PluginHookDecision::Allow);
        }
        (self.quick_add_transform)(serde_json::Value::String(raw_text.into()))
    }
}

/// Serializes a [`Task`] into the canonical JSON shape expected by plugin `beforeTaskComplete`
/// hooks (matching `plugins::api::task_to_json`).
pub fn task_to_hook_json(task: &Task) -> serde_json::Value {
    let bucket = match task.schedule_bucket {
        ScheduleBucket::None => "none",
        ScheduleBucket::Evening => "evening",
        ScheduleBucket::Someday => "someday",
    };
    serde_json::json!({
        "id": task.id.to_string(),
        "title": task.title,
        "notes": task.notes,
        "listId": task.list_id.map(|i| i.to_string()),
        "parentId": task.parent_id.map(|i| i.to_string()),
        "sectionId": task.section_id.map(|i| i.to_string()),
        "kind": task.kind.as_str(),
        "priority": match task.priority {
            Priority::None => "none",
            Priority::Low => "low",
            Priority::Medium => "medium",
            Priority::High => "high",
        },
        "dueDate": task.due_date.map(|d| d.format("%Y-%m-%d").to_string()),
        "dueTime": task.due_time.map(|t| t.format("%H:%M").to_string()),
        "endDate": task.end_date.map(|d| d.format("%Y-%m-%d").to_string()),
        "endTime": task.end_time.map(|t| t.format("%H:%M").to_string()),
        "startDate": task.start_date.map(|d| d.format("%Y-%m-%d").to_string()),
        "scheduleBucket": bucket,
        "horizon": task.horizon.map(|h| h.as_code()),
        "repeat": task.repeat.as_ref().map(ToString::to_string),
        "tags": task.tags,
        "checklist": task.checklist.map(|(done, total)| serde_json::json!({ "done": done, "total": total })),
        "completed": task.is_completed(),
        "completedAt": task.completed_at,
        "deletedAt": task.deleted_at,
        "wontDoAt": task.wont_do_at,
        "inProgressAt": task.in_progress_at,
        "pinnedAt": task.pinned_at,
        "createdAt": task.created_at,
        "modifiedAt": task.modified_at,
        "location": task.location,
        "timezone": task.timezone,
        "isFloating": task.is_floating,
        "constantReminder": task.constant_reminder,
        "reminders": task.reminders,
    })
}

/// Formats a plugin hook cancel message, falling back to `fallback` when `reason` is empty.
pub fn format_hook_cancel_message(reason: &str, fallback: &str) -> String {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Shows a warning toast on [`PluginBus`] when a plugin hook cancels an operation, falling back
/// to a localized message when the plugin provided an empty reason.
pub fn push_hook_cancel_toast(reason: &str, fallback: &str, cx: &mut App) {
    let msg = format_hook_cancel_message(reason, fallback);
    PluginBus::update(cx, |bus| {
        bus.push_toast(
            "plugins",
            msg,
            ToastLevel::Warning,
            chrono::Utc::now().timestamp_millis(),
        );
    });
}

/// Runs active `beforeTaskComplete` plugin hooks off the UI thread, calling `on_done(true, cx)`
/// when allowed (or synchronously when no active hook bridge is registered) and `on_done(false, cx)`
/// after posting a warning toast when cancelled.
pub fn run_before_task_complete_hook(
    task: &Task,
    cx: &mut App,
    on_done: impl FnOnce(bool, &mut App) + 'static,
) {
    let Some(hooks) = PluginHooksGlobal::global(cx).filter(PluginHooksGlobal::is_active) else {
        on_done(true, cx);
        return;
    };
    let task_json = task_to_hook_json(task);
    let bg_task = cx.background_spawn(async move { hooks.run_before_task_complete(task_json) });
    cx.spawn(async move |cx| {
        let res = bg_task.await;
        cx.update(|cx| match res {
            Ok(PluginHookDecision::Cancel(reason)) => {
                let fallback = i18n::t(cx).plugins_task_complete_cancelled;
                push_hook_cancel_toast(&reason, fallback, cx);
                on_done(false, cx);
            }
            Ok(PluginHookDecision::Allow | PluginHookDecision::Modify(_)) => {
                on_done(true, cx);
            }
            Err(err) => {
                eprintln!("plugin beforeTaskComplete hook failed: {err:#}");
                on_done(true, cx);
            }
        });
    })
    .detach();
}

/// Extracts a transformed quick-add string returned by a `quickAddTransform` plugin hook.
pub fn extract_transformed_quick_add(val: &serde_json::Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_owned());
    }
    if let Some(obj) = val.as_object() {
        for key in ["text", "input"] {
            if let Some(s) = obj.get(key).and_then(serde_json::Value::as_str) {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_owned());
                }
            }
        }
    }
    None
}

/// Serializes a [`TaskDraft`] + `top` insertion hint into the canonical JSON shape expected by
/// `beforeTaskCreate` plugin hooks.
pub fn task_draft_to_hook_json(draft: &TaskDraft, top: bool) -> serde_json::Value {
    let priority = match draft.priority {
        Priority::None => "none",
        Priority::Low => "low",
        Priority::Medium => "medium",
        Priority::High => "high",
    };
    let bucket = match draft.schedule_bucket {
        ScheduleBucket::None => "none",
        ScheduleBucket::Evening => "evening",
        ScheduleBucket::Someday => "someday",
    };
    let mut obj = serde_json::Map::new();
    obj.insert("title".to_owned(), serde_json::Value::String(draft.title.clone()));
    if !draft.notes.is_empty() {
        obj.insert("notes".to_owned(), serde_json::Value::String(draft.notes.clone()));
    }
    obj.insert("kind".to_owned(), serde_json::Value::String(draft.kind.as_str().to_owned()));
    obj.insert("priority".to_owned(), serde_json::Value::String(priority.to_owned()));
    obj.insert("scheduleBucket".to_owned(), serde_json::Value::String(bucket.to_owned()));
    obj.insert("top".to_owned(), serde_json::Value::Bool(top));
    if let Some(id) = draft.list_id {
        obj.insert("listId".to_owned(), serde_json::Value::String(id.to_string()));
    }
    if let Some(id) = draft.section_id {
        obj.insert("sectionId".to_owned(), serde_json::Value::String(id.to_string()));
    }
    if let Some(d) = draft.due_date {
        obj.insert("dueDate".to_owned(), serde_json::Value::String(d.format("%Y-%m-%d").to_string()));
    }
    if let Some(t) = draft.due_time {
        obj.insert("dueTime".to_owned(), serde_json::Value::String(format!("{:02}:{:02}", t.hour(), t.minute())));
    }
    if let Some(d) = draft.end_date {
        obj.insert("endDate".to_owned(), serde_json::Value::String(d.format("%Y-%m-%d").to_string()));
    }
    if let Some(t) = draft.end_time {
        obj.insert("endTime".to_owned(), serde_json::Value::String(format!("{:02}:{:02}", t.hour(), t.minute())));
    }
    if let Some(d) = draft.start_date {
        obj.insert("startDate".to_owned(), serde_json::Value::String(d.format("%Y-%m-%d").to_string()));
    }
    if let Some(h) = draft.horizon {
        obj.insert("horizon".to_owned(), serde_json::Value::String(h.as_code()));
    }
    if let Some(rep) = &draft.repeat {
        obj.insert("repeat".to_owned(), serde_json::Value::String(rep.to_string()));
    }
    if !draft.reminders.is_empty() {
        obj.insert(
            "reminders".to_owned(),
            serde_json::Value::Array(draft.reminders.iter().copied().map(serde_json::Value::from).collect()),
        );
    }
    if !draft.tags.is_empty() {
        obj.insert(
            "tags".to_owned(),
            serde_json::Value::Array(draft.tags.iter().cloned().map(serde_json::Value::String).collect()),
        );
    }
    if !draft.checklist.is_empty() {
        obj.insert(
            "checklist".to_owned(),
            serde_json::Value::Array(
                draft
                    .checklist
                    .iter()
                    .map(|(title, completed)| serde_json::json!({ "title": title, "completed": completed }))
                    .collect(),
            ),
        );
    }
    if let Some(loc) = &draft.location {
        obj.insert("location".to_owned(), serde_json::Value::String(loc.clone()));
    }
    if let Some(tz) = &draft.timezone {
        obj.insert("timezone".to_owned(), serde_json::Value::String(tz.clone()));
    }
    if draft.is_floating {
        obj.insert("isFloating".to_owned(), serde_json::Value::Bool(true));
    }
    if draft.constant_reminder {
        obj.insert("constantReminder".to_owned(), serde_json::Value::Bool(true));
    }
    serde_json::Value::Object(obj)
}

fn parse_hook_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .with_context(|| format!("invalid date {s:?}, expected YYYY-MM-DD"))
}

fn parse_hook_time(s: &str) -> Result<NaiveTime> {
    let trimmed = s.trim();
    NaiveTime::parse_from_str(trimmed, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(trimmed, "%H:%M:%S"))
        .with_context(|| format!("invalid time {s:?}, expected HH:MM"))
}

fn parse_hook_priority(v: &serde_json::Value) -> Result<Priority> {
    if let Some(s) = v.as_str() {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "" => Ok(Priority::None),
            "low" => Ok(Priority::Low),
            "medium" | "med" => Ok(Priority::Medium),
            "high" => Ok(Priority::High),
            other => bail!("invalid priority {other:?}"),
        }
    } else if let Some(n) = v.as_i64() {
        Ok(Priority::from_i64(n))
    } else {
        bail!("priority must be a string or integer")
    }
}

/// Parses a plugin-modified `beforeTaskCreate` JSON payload into `(TaskDraft, top)`.
pub fn parse_task_draft_from_hook_json(
    input: &serde_json::Value,
    default_top: bool,
) -> Result<(TaskDraft, bool)> {
    if let Some(title) = input.as_str() {
        let trimmed = title.trim();
        ensure!(!trimmed.is_empty(), "task title must not be empty");
        return Ok((
            TaskDraft {
                title: trimmed.to_owned(),
                ..Default::default()
            },
            default_top,
        ));
    }
    let obj = input
        .as_object()
        .context("task draft must be a string or object")?;
    let title = obj
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .context("task title must not be empty")?
        .to_owned();

    let mut draft = TaskDraft {
        title,
        ..Default::default()
    };
    if let Some(notes) = obj.get("notes").and_then(serde_json::Value::as_str) {
        draft.notes = notes.to_owned();
    }
    if let Some(list_id) = obj.get("listId").and_then(serde_json::Value::as_str)
        && !list_id.trim().is_empty()
    {
        draft.list_id = Some(
            list_id
                .trim()
                .parse::<Id>()
                .with_context(|| format!("invalid listId {list_id:?}"))?,
        );
    }
    if let Some(section_id) = obj.get("sectionId").and_then(serde_json::Value::as_str)
        && !section_id.trim().is_empty()
    {
        draft.section_id = Some(
            section_id
                .trim()
                .parse::<Id>()
                .with_context(|| format!("invalid sectionId {section_id:?}"))?,
        );
    }
    if let Some(kind) = obj.get("kind").and_then(serde_json::Value::as_str) {
        draft.kind = Kind::parse(kind);
    }
    if let Some(prio) = obj.get("priority")
        && !prio.is_null()
    {
        draft.priority = parse_hook_priority(prio)?;
    }
    if let Some(d) = obj.get("dueDate").and_then(serde_json::Value::as_str)
        && !d.trim().is_empty()
    {
        draft.due_date = Some(parse_hook_date(d)?);
    }
    if let Some(t) = obj.get("dueTime").and_then(serde_json::Value::as_str)
        && !t.trim().is_empty()
    {
        draft.due_time = Some(parse_hook_time(t)?);
    }
    if let Some(d) = obj.get("endDate").and_then(serde_json::Value::as_str)
        && !d.trim().is_empty()
    {
        draft.end_date = Some(parse_hook_date(d)?);
    }
    if let Some(t) = obj.get("endTime").and_then(serde_json::Value::as_str)
        && !t.trim().is_empty()
    {
        draft.end_time = Some(parse_hook_time(t)?);
    }
    if let Some(d) = obj.get("startDate").and_then(serde_json::Value::as_str)
        && !d.trim().is_empty()
    {
        draft.start_date = Some(parse_hook_date(d)?);
    }
    if let Some(b) = obj.get("scheduleBucket").and_then(serde_json::Value::as_str) {
        draft.schedule_bucket = match b.trim().to_ascii_lowercase().as_str() {
            "none" | "" => ScheduleBucket::None,
            "evening" => ScheduleBucket::Evening,
            "someday" => ScheduleBucket::Someday,
            other => bail!("invalid scheduleBucket {other:?}"),
        };
    }
    if let Some(h_val) = obj.get("horizon") {
        match h_val {
            serde_json::Value::Null => {}
            serde_json::Value::String(s) if s.trim().is_empty() => {}
            serde_json::Value::String(s) => {
                draft.horizon = Some(
                    Horizon::parse(s).with_context(|| format!("invalid horizon {s:?}"))?,
                );
            }
            _ => bail!("invalid horizon"),
        }
    }
    if let Some(rep) = obj.get("repeat").and_then(serde_json::Value::as_str)
        && !rep.trim().is_empty()
    {
        draft.repeat = Some(
            Recurrence::from_str(rep).map_err(|e| anyhow!("invalid repeat rule: {e}"))?,
        );
    }
    if let Some(tags) = obj.get("tags").and_then(serde_json::Value::as_array) {
        draft.tags = tags
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
    }
    if let Some(checklist) = obj.get("checklist").and_then(serde_json::Value::as_array) {
        for item in checklist {
            if let Some(text) = item.as_str() {
                draft.checklist.push((text.to_owned(), false));
            } else if let Some(item_obj) = item.as_object()
                && let Some(title) = item_obj.get("title").and_then(serde_json::Value::as_str)
            {
                let done = item_obj
                    .get("completed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                draft.checklist.push((title.to_owned(), done));
            }
        }
    }
    if let Some(loc) = obj.get("location").and_then(serde_json::Value::as_str) {
        draft.location = Some(loc.to_owned());
    }
    if let Some(tz) = obj.get("timezone").and_then(serde_json::Value::as_str) {
        draft.timezone = Some(tz.to_owned());
    }
    if let Some(fl) = obj.get("isFloating").and_then(serde_json::Value::as_bool) {
        draft.is_floating = fl;
    }
    if let Some(cr) = obj.get("constantReminder").and_then(serde_json::Value::as_bool) {
        draft.constant_reminder = cr;
    }
    if let Some(rems) = obj.get("reminders").and_then(serde_json::Value::as_array) {
        draft.reminders = rems.iter().filter_map(serde_json::Value::as_i64).collect();
    }
    let top = obj
        .get("top")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(default_top);
    Ok((draft, top))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn plugin_hooks_global_dispatches_and_respects_active_check(cx: &mut TestAppContext) {
        let active = Arc::new(AtomicBool::new(true));
        let active_flag = Arc::clone(&active);

        cx.update(|cx| {
            assert!(PluginHooksGlobal::global(cx).is_none());
            let hooks = PluginHooksGlobal::new(
                Arc::new(|mut draft| {
                    if let Some(obj) = draft.as_object_mut() {
                        obj.insert("title".into(), serde_json::json!("Transformed"));
                    }
                    Ok(PluginHookDecision::Modify(draft))
                }),
                Arc::new(|task| {
                    if task.get("title").and_then(|v| v.as_str()) == Some("Blocked") {
                        Ok(PluginHookDecision::Cancel("not allowed".into()))
                    } else {
                        Ok(PluginHookDecision::Allow)
                    }
                }),
                Arc::new(|raw| {
                    let s = raw.as_str().unwrap_or_default();
                    Ok(PluginHookDecision::Modify(serde_json::json!(format!("{s} #auto"))))
                }),
            )
            .with_active_check(Arc::new(move || active_flag.load(Ordering::Relaxed)));
            PluginHooksGlobal::set_global(hooks, cx);
        });

        let cloned = cx.update(|cx| PluginHooksGlobal::global(cx)).expect("global must exist");
        assert!(cloned.is_active());
        assert_eq!(
            cloned.run_before_task_create(serde_json::json!({"title": "Init"})).unwrap(),
            PluginHookDecision::Modify(serde_json::json!({"title": "Transformed"}))
        );
        assert_eq!(
            cloned.run_before_task_complete(serde_json::json!({"title": "Blocked"})).unwrap(),
            PluginHookDecision::Cancel("not allowed".into())
        );
        assert_eq!(
            cloned.run_quick_add_transform("Buy milk").unwrap(),
            PluginHookDecision::Modify(serde_json::json!("Buy milk #auto"))
        );

        // When active check turns false (e.g., Safe Mode), all hooks short-circuit to Allow.
        active.store(false, Ordering::Relaxed);
        assert!(!cloned.is_active());
        assert_eq!(
            cloned.run_before_task_complete(serde_json::json!({"title": "Blocked"})).unwrap(),
            PluginHookDecision::Allow
        );

        cx.update(PluginHooksGlobal::clear_global);
        assert!(cx.update(|cx| PluginHooksGlobal::global(cx)).is_none());

        // Verify task_to_hook_json includes canonical plugin fields.
        let store = fern_core::Store::open_in_memory().unwrap();
        let task = store.add_task("Hook payload").unwrap();
        let json = task_to_hook_json(&task);
        assert_eq!(json["id"], task.id.to_string());
        assert_eq!(json["title"], "Hook payload");
        assert_eq!(json["scheduleBucket"], "none");
        assert_eq!(json["completed"], false);

        // Verify task_draft_to_hook_json <-> parse_task_draft_from_hook_json round-trip.
        let draft = TaskDraft {
            title: "Draft payload".into(),
            notes: "Details".into(),
            priority: Priority::High,
            due_date: NaiveDate::from_ymd_opt(2026, 12, 5),
            due_time: NaiveTime::from_hms_opt(14, 30, 0),
            start_date: NaiveDate::from_ymd_opt(2026, 12, 1),
            schedule_bucket: ScheduleBucket::Evening,
            tags: vec!["ops".into()],
            reminders: vec![0, 15],
            ..Default::default()
        };
        let draft_json = task_draft_to_hook_json(&draft, false);
        let (parsed, top) = parse_task_draft_from_hook_json(&draft_json, true).unwrap();
        assert!(!top);
        assert_eq!(parsed.title, "Draft payload");
        assert_eq!(parsed.notes, "Details");
        assert_eq!(parsed.priority, Priority::High);
        assert_eq!(parsed.due_date, draft.due_date);
        assert_eq!(parsed.due_time, draft.due_time);
        assert_eq!(parsed.start_date, draft.start_date);
        assert_eq!(parsed.schedule_bucket, ScheduleBucket::Evening);
        assert_eq!(parsed.tags, vec!["ops".to_owned()]);
        assert_eq!(parsed.reminders, vec![0, 15]);

        // Verify horizon serialization & parsing in plugin hook JSON.
        let w48 = Horizon::Week { year: 2026, week: 48 };
        let h_task = store
            .create_task(
                TaskDraft {
                    title: "Horizon hook".into(),
                    horizon: Some(w48),
                    ..Default::default()
                },
                true,
            )
            .unwrap();
        assert_eq!(task_to_hook_json(&h_task)["horizon"], "W2026-48");
        assert_eq!(json["horizon"], serde_json::Value::Null);

        let h_draft = TaskDraft {
            title: "Horizon draft".into(),
            horizon: Some(w48),
            ..Default::default()
        };
        let h_draft_json = task_draft_to_hook_json(&h_draft, true);
        assert_eq!(h_draft_json["horizon"], "W2026-48");
        let (parsed_h, _) = parse_task_draft_from_hook_json(&h_draft_json, true).unwrap();
        assert_eq!(parsed_h.horizon, Some(w48));
        assert!(
            parse_task_draft_from_hook_json(&serde_json::json!({"title": "Bad", "horizon": "W2025-53"}), true)
                .is_err()
        );
    }
}
