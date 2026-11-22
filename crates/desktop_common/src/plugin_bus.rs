//! Shared GPUI global event/UI bus between `plugin_ui` and `apps/desktop`.

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::Result;
use fern_core::{Id, Priority, ScheduleBucket, Task};
use gpui::{App, Global};

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
    }
}
