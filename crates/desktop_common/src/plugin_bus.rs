//! Shared GPUI global event/UI bus between `plugin_ui` and `apps/desktop`.

use std::collections::VecDeque;
use std::sync::Arc;

use fern_core::Id;
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
