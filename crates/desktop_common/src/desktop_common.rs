pub mod actions;
pub mod active_preferences;
pub mod calendar;
#[cfg(not(target_arch = "wasm32"))]
pub mod cloud_vault;
pub mod colors;
pub mod completion_mark;
pub mod countdown_helpers;
pub mod data_version;
pub mod db_runner;
pub mod diagnostics;
pub mod emoji;
pub mod export;
pub mod habit_helpers;
pub mod keymap;
pub mod layout;
pub mod logging;
pub mod markdown_toolbar;
pub mod month_calendar;
pub mod paths;
pub mod plugin_bus;
pub mod portable;
pub mod release_notes;
pub mod resize;
pub mod runtime_config;
pub mod sound;
pub mod sticky_opener;
pub mod sticky_settings;
pub mod sticky_windows;
pub mod sync_types;
pub mod sync {
    pub use crate::sync_types::{
        CloudApiError, CloudAttachmentBridge, CloudAttachmentOpenFn, CloudAttachmentUploadFn,
        CloudSyncSnapshot, LiveCloudSnapshot, RealtimeStatus, SyncStatus, attachment_error,
        cloud_api_error, is_e2ee_error, is_grace_conflict, is_incompatible_protocol,
        is_session_expired, rate_limit_retry_after,
    };
}
pub mod task_ops;
pub mod text_input;
pub mod touch;
pub mod touch_keyboard;
pub mod undo;
pub mod user_themes;
#[cfg(windows)]
pub mod win32;
pub mod window_chrome;
pub mod window_state;

pub use active_preferences::ActivePreferences;
pub use db_runner::{DbPath, DbRunner};
pub use fern_i18n as i18n;
pub use paths::{
    DAILY_BACKUPS, DEFAULT_APP_ID, app_id, attachments_dir, backups_dir, data_dir,
    desktop_icon_name, is_flatpak, is_wayland_session, pending_restore_path,
    plugin_registry_cache_dir, plugin_secrets_key_path, plugin_secrets_path, plugins_dir,
    scratch_dir,
};
pub use runtime_config::{RuntimeConfig, dev_state_value, is_dev_state};
pub use plugin_bus::{
    PluginBus, PluginCommandSpec, PluginHookDecision, PluginHookFn, PluginHooksGlobal,
    PluginModalKind, PluginModalSpec, PluginNavAction, PluginSearchHit, PluginSidebarViewSpec,
    PluginToast, QuickAddCreation, TaskActionSpec, TaskBadgeSpec, ToastLevel, VirtualListSpec,
    build_quick_add_draft_json, extract_transformed_quick_add, format_hook_cancel_message,
    parse_task_draft_from_hook_json, push_hook_cancel_toast, run_before_task_complete_hook,
    run_quick_add_hooks, task_draft_to_hook_json, task_to_hook_json,
};
pub use sticky_opener::{StickyOpener, open_sticky_note};
