//! Runtime-agnostic plugin system for Fern (QuickJS-NG, permissions, vault, HTTP, registry).

pub mod api;
pub mod http;
pub mod manifest;
pub mod module_loader;
pub mod permissions;
pub mod registry;
pub mod runtime;
pub mod scaffold;
pub mod ui_ast;
pub mod vault;

pub use api::{LogEntry, LogLevel, PluginUiEffect, PromptReplyValue, ToastLevel};
pub use http::{HttpPool, HttpRequest, HttpResponse, execute_http_request, is_ip_blocked};
pub use manifest::{
    CURRENT_API_VERSION, CommandContribution, ImporterContribution, LoadedManifest,
    LocaleContribution, PermissionApprovalStatus, PluginContributions, PluginManifest,
    ReservedContribution, SearchProviderContribution, SettingKind, SettingOption, SettingSpec,
    SidebarContribution, ThemeContribution, manifest_sha256, validate_plugin_id,
};
pub use permissions::{Permission, PermissionDiff, PermissionSet};
pub use registry::{
    CachedRegistryIndex, DEFAULT_REGISTRY_URL, InstallOutcome, RegistryClient, RegistryIndex,
    RegistryPluginEntry, RegistrySource,
};
pub use runtime::{
    CIRCUIT_BREAKER_THRESHOLD, HostUpdate, HookDecision, PLUGIN_MEMORY_LIMIT_BYTES,
    PLUGIN_STACK_LIMIT_BYTES, PluginEngineConfig, PluginInfo, PluginManager, PluginMetrics,
    PluginStatus, ScriptRunResult, SearchHit,
};
pub use scaffold::{FERN_D_TS, JSCONFIG_JSON, generate_sample_plugin};
pub use ui_ast::{
    HandlerId, MAX_SVG_BYTES, UiAlign, UiButtonSize, UiButtonVariant, UiEvent, UiJustify, UiNode,
    UiSelectOption, UiTextSize, UiTreePayload, UiWeight, sanitize_svg,
};
pub use vault::SecretsVault;
