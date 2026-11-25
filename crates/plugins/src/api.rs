//! QuickJS Host API bindings (`fern.*`, `console.*`, and `UiNode` callback extraction).

mod convert;
mod data;
mod network;
mod ui_bindings;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, ensure};
use fern_core::Store;
use fern_core::model::Id;
use futures::channel::oneshot;
use rquickjs::{Ctx, Exception, Function, Object, Persistent};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue};

use crate::http::HttpPool;
use crate::permissions::{Permission, PermissionSet};
use crate::ui_ast::UiNode;
use crate::vault::SecretsVault;

pub use convert::{
    js_to_json, js_value_to_ui_node, json_to_js, parse_task_draft, task_to_json,
};
use convert::{now_ms, throw_scrubbed};
use data::{
    install_batch, install_countdowns, install_focus, install_habits, install_lists, install_meta,
    install_settings, install_storage, install_tags, install_tasks,
};
use network::{install_http, install_oauth};
pub(crate) use ui_bindings::event_permission;
use ui_bindings::{
    install_commands, install_console, install_events, install_hooks, install_search,
    install_services, install_system, install_timers, install_ui, install_views,
};

pub const LOG_RING_CAPACITY: usize = 200;
pub const MIN_TIMER_INTERVAL_MS: u64 = 10_000;
pub const MAX_TIMER_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;
pub const MAX_TIMERS_PER_PLUGIN: usize = 8;
pub const MAX_OAUTH_PER_PLUGIN: usize = 2;
pub const MAX_OAUTH_GLOBAL: usize = 4;
pub const MAX_PENDING_UI_EFFECTS: usize = 64;
const MAX_OPEN_EXTERNAL_PER_WINDOW: usize = 3;
const OPEN_EXTERNAL_WINDOW: Duration = Duration::from_secs(5);
const MAX_DEFAULT_SCOPE_HANDLERS: usize = 256;
const MAX_SCOPE_HANDLERS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp_ms: i64,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastLevel {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl ToastLevel {
    pub(super) fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).unwrap_or("info") {
            "success" => Self::Success,
            "warning" | "warn" => Self::Warning,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }
}

/// UI / host side-effect queued by a plugin during JS execution.
#[derive(Clone, Debug, PartialEq)]
pub enum PluginUiEffect {
    Toast {
        plugin_id: String,
        message: String,
        level: ToastLevel,
        created_at_ms: i64,
    },
    Confirm {
        request_id: u64,
        plugin_id: String,
        title: String,
        body: String,
    },
    Prompt {
        request_id: u64,
        plugin_id: String,
        title: String,
        placeholder: String,
        default_value: String,
    },
    OpenModal {
        plugin_id: String,
        view_id: String,
        title: String,
        tree: UiNode,
    },
    CloseModal {
        plugin_id: String,
    },
    OpenTask {
        task_id: Id,
    },
    OpenSettings {
        plugin_id: String,
    },
    OpenExternal {
        url: String,
    },
    ClipboardWrite {
        text: String,
    },
}

/// Reply sent from UI back to a suspended `fern.ui.confirm` / `fern.ui.prompt` Promise.
#[derive(Clone, Debug)]
pub enum PromptReplyValue {
    Confirm(bool),
    Prompt(Option<String>),
}

pub type PromptSenders = Rc<RefCell<HashMap<u64, (String, oneshot::Sender<PromptReplyValue>)>>>;
pub type ServiceCallerFn =
    Rc<dyn Fn(String, JsonValue, usize) -> oneshot::Receiver<Result<JsonValue>>>;
pub type ServiceRegistrarFn = Rc<dyn Fn(String) -> Result<()>>;
pub type ViewRefresherFn = Rc<dyn Fn(String)>;
pub type EffectEmitterFn = Arc<dyn Fn(PluginUiEffect) + Send + Sync>;

#[derive(Default)]
struct OAuthLimiterState {
    global: usize,
    per_plugin: HashMap<String, usize>,
}

/// Bounds concurrent detached OAuth worker threads globally and per plugin.
#[derive(Default)]
pub struct OAuthConcurrencyLimiter {
    state: Mutex<OAuthLimiterState>,
}

pub struct OAuthPermit {
    limiter: Arc<OAuthConcurrencyLimiter>,
    plugin_id: String,
}

impl Drop for OAuthPermit {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.limiter.state.lock() {
            guard.global = guard.global.saturating_sub(1);
            if let Some(count) = guard.per_plugin.get_mut(&self.plugin_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    guard.per_plugin.remove(&self.plugin_id);
                }
            }
        }
    }
}

impl OAuthConcurrencyLimiter {
    pub fn try_acquire(self: &Arc<Self>, plugin_id: &str) -> Result<OAuthPermit> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| anyhow!("OAuth concurrency limiter lock poisoned"))?;
        let plugin_count = guard.per_plugin.get(plugin_id).copied().unwrap_or(0);
        ensure!(
            plugin_count < MAX_OAUTH_PER_PLUGIN,
            "too many concurrent OAuth requests for plugin (max {MAX_OAUTH_PER_PLUGIN})"
        );
        ensure!(
            guard.global < MAX_OAUTH_GLOBAL,
            "too many concurrent OAuth requests across plugins (max {MAX_OAUTH_GLOBAL})"
        );
        guard.global += 1;
        *guard.per_plugin.entry(plugin_id.to_owned()).or_insert(0) += 1;
        Ok(OAuthPermit {
            limiter: Arc::clone(self),
            plugin_id: plugin_id.to_owned(),
        })
    }
}

/// RAII guard that sets `active_render_scope` and always restores the previous scope on drop.
pub struct RenderScopeGuard {
    api: Rc<PluginApiState>,
    prev_scope: Option<String>,
}

impl RenderScopeGuard {
    pub fn enter(api: Rc<PluginApiState>, scope: String) -> Self {
        let prev_scope = api.active_render_scope.borrow_mut().replace(scope);
        Self { api, prev_scope }
    }
}

impl Drop for RenderScopeGuard {
    fn drop(&mut self) {
        *self.api.active_render_scope.borrow_mut() = self.prev_scope.take();
    }
}

pub struct RegisteredView {
    pub render: Persistent<Function<'static>>,
    pub mount: Option<Persistent<Function<'static>>>,
    pub unmount: Option<Persistent<Function<'static>>>,
}

pub struct RegisteredTimer {
    pub id: String,
    pub interval_ms: u64,
    pub next_fire: Instant,
    pub callback: Persistent<Function<'static>>,
}

#[derive(Default)]
pub struct PluginHooks {
    pub before_task_create: Vec<Persistent<Function<'static>>>,
    pub before_task_complete: Vec<Persistent<Function<'static>>>,
    pub quick_add_transform: Vec<Persistent<Function<'static>>>,
    pub on_reminder: Vec<Persistent<Function<'static>>>,
}

pub type EventListenerMap = HashMap<String, Vec<(u64, Persistent<Function<'static>>)>>;

pub struct PluginApiInit {
    pub plugin_id: String,
    pub permissions: PermissionSet,
    pub store: Rc<Store>,
    pub vault: SecretsVault,
    pub http_pool: Arc<HttpPool>,
    pub oauth_limiter: Arc<OAuthConcurrencyLimiter>,
    pub write_depth: u32,
    pub enforce_declared_views: bool,
    pub declared_views: HashSet<String>,
    pub settings: Map<String, JsonValue>,
    pub secret_keys: HashSet<String>,
    pub locale: Rc<RefCell<String>>,
    pub theme_appearance: Rc<RefCell<String>>,
    pub logs: VecDeque<LogEntry>,
    pub prompt_senders: PromptSenders,
    pub next_prompt_id: Rc<Cell<u64>>,
    pub service_registrar: ServiceRegistrarFn,
    pub service_caller: ServiceCallerFn,
    pub view_refresher: ViewRefresherFn,
    pub effect_emitter: EffectEmitterFn,
    pub shutdown_flag: Arc<AtomicBool>,
}

/// Shared per-plugin (or per-script) host bridge accessed by QuickJS bindings on `PluginWorker`.
pub struct PluginApiState {
    pub plugin_id: String,
    pub permissions: PermissionSet,
    pub store: Rc<Store>,
    pub vault: SecretsVault,
    pub http_pool: Arc<HttpPool>,
    pub oauth_limiter: Arc<OAuthConcurrencyLimiter>,
    pub write_depth: Cell<u32>,
    pub active_service_depth: Cell<usize>,
    pub batch_dirty_stack: RefCell<Vec<bool>>,
    pub data_dirty: Cell<bool>,
    pub enforce_declared_views: bool,
    pub declared_views: HashSet<String>,
    pub settings: RefCell<Map<String, JsonValue>>,
    pub secret_keys: HashSet<String>,
    pub locale: Rc<RefCell<String>>,
    pub theme_appearance: Rc<RefCell<String>>,
    pub logs: RefCell<VecDeque<LogEntry>>,
    pub active_render_scope: RefCell<Option<String>>,
    pub handlers: RefCell<HashMap<String, HashMap<u32, Persistent<Function<'static>>>>>,
    pub next_handler_id: Cell<u32>,
    pub views: RefCell<HashMap<String, RegisteredView>>,
    pub modal_view: RefCell<Option<(String, RegisteredView)>>,
    pub commands: RefCell<HashMap<String, Persistent<Function<'static>>>>,
    pub search_providers: RefCell<HashMap<String, Persistent<Function<'static>>>>,
    pub event_listeners: RefCell<EventListenerMap>,
    pub next_listener_id: Cell<u64>,
    pub settings_listeners: RefCell<Vec<Persistent<Function<'static>>>>,
    pub hooks: RefCell<PluginHooks>,
    pub timers: RefCell<HashMap<String, RegisteredTimer>>,
    pub services: RefCell<HashMap<String, Persistent<Function<'static>>>>,
    pub pending_ui_effects: RefCell<VecDeque<PluginUiEffect>>,
    pub open_external_timestamps: RefCell<VecDeque<Instant>>,
    pub prompt_senders: PromptSenders,
    pub next_prompt_id: Rc<Cell<u64>>,
    pub service_registrar: ServiceRegistrarFn,
    pub service_caller: ServiceCallerFn,
    pub view_refresher: ViewRefresherFn,
    pub effect_emitter: EffectEmitterFn,
    pub shutdown_flag: Arc<AtomicBool>,
}

impl PluginApiState {
    pub fn new(init: PluginApiInit) -> Self {
        Self {
            plugin_id: init.plugin_id,
            permissions: init.permissions,
            store: init.store,
            vault: init.vault,
            http_pool: init.http_pool,
            oauth_limiter: init.oauth_limiter,
            write_depth: Cell::new(init.write_depth),
            active_service_depth: Cell::new(0),
            batch_dirty_stack: RefCell::new(Vec::new()),
            data_dirty: Cell::new(false),
            enforce_declared_views: init.enforce_declared_views,
            declared_views: init.declared_views,
            settings: RefCell::new(init.settings),
            secret_keys: init.secret_keys,
            locale: init.locale,
            theme_appearance: init.theme_appearance,
            logs: RefCell::new(init.logs),
            active_render_scope: RefCell::new(None),
            handlers: RefCell::new(HashMap::new()),
            next_handler_id: Cell::new(1),
            views: RefCell::new(HashMap::new()),
            modal_view: RefCell::new(None),
            commands: RefCell::new(HashMap::new()),
            search_providers: RefCell::new(HashMap::new()),
            event_listeners: RefCell::new(HashMap::new()),
            next_listener_id: Cell::new(1),
            settings_listeners: RefCell::new(Vec::new()),
            hooks: RefCell::new(PluginHooks::default()),
            timers: RefCell::new(HashMap::new()),
            services: RefCell::new(HashMap::new()),
            pending_ui_effects: RefCell::new(VecDeque::new()),
            open_external_timestamps: RefCell::new(VecDeque::new()),
            prompt_senders: init.prompt_senders,
            next_prompt_id: init.next_prompt_id,
            service_registrar: init.service_registrar,
            service_caller: init.service_caller,
            view_refresher: init.view_refresher,
            effect_emitter: init.effect_emitter,
            shutdown_flag: init.shutdown_flag,
        }
    }

    pub fn push_log(&self, level: LogLevel, raw_msg: &str) {
        let scrubbed = self.vault.scrub_secrets(&self.plugin_id, raw_msg);
        let mut logs = self.logs.borrow_mut();
        if logs.len() >= LOG_RING_CAPACITY {
            logs.pop_front();
        }
        logs.push_back(LogEntry {
            timestamp_ms: now_ms(),
            level,
            message: scrubbed,
        });
    }

    pub fn push_ui_effect(&self, effect: PluginUiEffect) -> Result<()> {
        let mut queue = self.pending_ui_effects.borrow_mut();
        if queue.len() >= MAX_PENDING_UI_EFFECTS {
            if matches!(effect, PluginUiEffect::Toast { .. })
                && let Some(oldest_toast) =
                    queue.iter().position(|e| matches!(e, PluginUiEffect::Toast { .. }))
            {
                queue.remove(oldest_toast);
            } else {
                anyhow::bail!(
                    "pending UI effect queue full (max {MAX_PENDING_UI_EFFECTS} effects per invocation)"
                );
            }
        }
        queue.push_back(effect);
        Ok(())
    }

    pub fn check_open_external_rate_limit(&self) -> Result<()> {
        let now = Instant::now();
        let mut history = self.open_external_timestamps.borrow_mut();
        while history
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= OPEN_EXTERNAL_WINDOW)
        {
            history.pop_front();
        }
        ensure!(
            history.len() < MAX_OPEN_EXTERNAL_PER_WINDOW,
            "openExternal rate limit exceeded (max {MAX_OPEN_EXTERNAL_PER_WINDOW} per {}s)",
            OPEN_EXTERNAL_WINDOW.as_secs()
        );
        history.push_back(now);
        Ok(())
    }

    pub fn scrub(&self, text: &str) -> String {
        self.vault.scrub_secrets(&self.plugin_id, text)
    }

    pub fn clear_scope_handlers(&self, scope: &str) {
        self.handlers.borrow_mut().remove(scope);
    }

    pub fn swap_scope_handlers(&self, staging_scope: &str, target_scope: &str) {
        if staging_scope == target_scope {
            return;
        }
        let mut handlers = self.handlers.borrow_mut();
        if let Some(staged) = handlers.remove(staging_scope) {
            handlers.insert(target_scope.to_owned(), staged);
        } else {
            handlers.remove(target_scope);
        }
    }

    pub fn set_active_scope(&self, scope: Option<String>) {
        *self.active_render_scope.borrow_mut() = scope;
    }

    pub fn register_handler<'js>(
        &self,
        ctx: &Ctx<'js>,
        scope: &str,
        func: Function<'js>,
    ) -> Result<u32> {
        let cap = if scope == "__default__" {
            MAX_DEFAULT_SCOPE_HANDLERS
        } else {
            MAX_SCOPE_HANDLERS
        };
        let mut handlers = self.handlers.borrow_mut();
        let scope_map = handlers.entry(scope.to_owned()).or_default();
        ensure!(
            scope_map.len() < cap,
            "handler limit ({cap}) exceeded for scope {scope:?}"
        );
        let id = self.next_handler_id.get().max(1);
        self.next_handler_id.set(id.wrapping_add(1).max(1));
        let persistent = Persistent::save(ctx, func);
        scope_map.insert(id, persistent);
        Ok(id)
    }

    /// Moves a handler ID previously registered under `"__default__"` into `target_scope`
    /// (e.g. `"__modal__"` when a plugin pre-built a `UiNode` before calling `fern.ui.openModal`).
    pub fn rehome_default_handler(&self, target_scope: &str, handler_id: u32) -> Result<()> {
        if target_scope == "__default__" {
            return Ok(());
        }
        let mut handlers = self.handlers.borrow_mut();
        let moved = handlers
            .get_mut("__default__")
            .and_then(|default_map| default_map.remove(&handler_id));
        if let Some(persistent) = moved {
            let scope_map = handlers.entry(target_scope.to_owned()).or_default();
            ensure!(
                scope_map.len() < MAX_SCOPE_HANDLERS,
                "handler limit ({MAX_SCOPE_HANDLERS}) exceeded for scope {target_scope:?}"
            );
            scope_map.insert(handler_id, persistent);
        }
        Ok(())
    }

    /// Drops every `Persistent<Function>` handle before dropping the QuickJS `AsyncRuntime`.
    pub fn clear_persistent_handles(&self) {
        self.handlers.borrow_mut().clear();
        self.views.borrow_mut().clear();
        self.modal_view.borrow_mut().take();
        self.commands.borrow_mut().clear();
        self.search_providers.borrow_mut().clear();
        self.event_listeners.borrow_mut().clear();
        self.settings_listeners.borrow_mut().clear();
        {
            let mut hooks = self.hooks.borrow_mut();
            hooks.before_task_create.clear();
            hooks.before_task_complete.clear();
            hooks.quick_add_transform.clear();
            hooks.on_reminder.clear();
        }
        self.timers.borrow_mut().clear();
        self.services.borrow_mut().clear();
    }

    pub(super) fn require_perm<'js>(&self, ctx: &Ctx<'js>, perm: &Permission) -> rquickjs::Result<()> {
        if self.permissions.has(perm) {
            return Ok(());
        }
        Err(Exception::throw_message(
            ctx,
            &format!("permission denied: requires {}", perm.summary()),
        ))
    }

    pub(super) fn run_write<T>(
        &self,
        ctx: &Ctx<'_>,
        f: impl FnOnce() -> Result<T>,
    ) -> rquickjs::Result<T> {
        let in_batch = !self.batch_dirty_stack.borrow().is_empty();
        let res = if in_batch {
            f()
        } else {
            self.store
                .with_plugin_write_ctx(&self.plugin_id, self.write_depth.get(), f)
        };
        match res {
            Ok(v) => {
                if in_batch {
                    if let Some(top) = self.batch_dirty_stack.borrow_mut().last_mut() {
                        *top = true;
                    }
                } else {
                    self.data_dirty.set(true);
                }
                Ok(v)
            }
            Err(err) => Err(throw_scrubbed(ctx, self, err)),
        }
    }
}

pub fn install_fern_bindings<'js>(
    ctx: &Ctx<'js>,
    state: Rc<PluginApiState>,
    expose_global_fern: bool,
) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let fern = Object::new(ctx.clone())?;

    install_tasks(ctx, &fern, Rc::clone(&state))?;
    install_lists(ctx, &fern, Rc::clone(&state))?;
    install_tags(ctx, &fern, Rc::clone(&state))?;
    install_habits(ctx, &fern, Rc::clone(&state))?;
    install_focus(ctx, &fern, Rc::clone(&state))?;
    install_countdowns(ctx, &fern, Rc::clone(&state))?;
    install_batch(ctx, &fern, Rc::clone(&state))?;
    install_storage(ctx, &fern, Rc::clone(&state))?;
    install_meta(ctx, &fern, Rc::clone(&state))?;
    install_settings(ctx, &fern, Rc::clone(&state))?;
    install_http(ctx, &fern, Rc::clone(&state))?;
    install_oauth(ctx, &fern, Rc::clone(&state))?;
    install_events(ctx, &fern, Rc::clone(&state))?;
    install_hooks(ctx, &fern, Rc::clone(&state))?;
    install_timers(ctx, &fern, Rc::clone(&state))?;
    install_commands(ctx, &fern, Rc::clone(&state))?;
    install_search(ctx, &fern, Rc::clone(&state))?;
    install_services(ctx, &fern, Rc::clone(&state))?;
    install_views(ctx, &fern, Rc::clone(&state))?;
    install_ui(ctx, &fern, Rc::clone(&state))?;
    install_system(ctx, &fern, Rc::clone(&state))?;

    globals.set("__fern", fern.clone())?;
    if expose_global_fern {
        globals.set("fern", fern)?;
    }

    let reg_state = Rc::clone(&state);
    globals.set(
        "__fern_reg_handler",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, func: Function<'js>| -> rquickjs::Result<u32> {
            let scope = reg_state
                .active_render_scope
                .borrow()
                .clone()
                .unwrap_or_else(|| "__default__".to_owned());
            reg_state
                .register_handler(&ctx, &scope, func)
                .map_err(|e| throw_scrubbed(&ctx, &reg_state, e))
        })?,
    )?;

    install_console(ctx, &globals, state)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use fern_core::Horizon;
    use serde_json::json;

    use super::convert::{apply_task_patch, js_float_to_json, load_filtered_tasks};
    use super::*;
    use crate::http::validate_target_uri_syntax;

    #[test]
    fn oauth_concurrency_limiter_enforces_per_plugin_and_global_caps() {
        let limiter = Arc::new(OAuthConcurrencyLimiter::default());
        let p1 = limiter.try_acquire("com.test.a").unwrap();
        let p2 = limiter.try_acquire("com.test.a").unwrap();
        // 3rd concurrent request for the same plugin is rejected (MAX_OAUTH_PER_PLUGIN = 2).
        assert!(limiter.try_acquire("com.test.a").is_err());

        // Another plugin can acquire up to global limit (MAX_OAUTH_GLOBAL = 4).
        let p3 = limiter.try_acquire("com.test.b").unwrap();
        let p4 = limiter.try_acquire("com.test.c").unwrap();
        assert!(limiter.try_acquire("com.test.d").is_err());

        // Dropping permits releases both per-plugin and global slots.
        drop(p1);
        let p5 = limiter.try_acquire("com.test.a").unwrap();
        drop((p2, p3, p4, p5));
        assert!(limiter.try_acquire("com.test.d").is_ok());
    }

    #[test]
    fn js_float_to_json_preserves_safe_integers_above_i32_max() {
        // Epoch millis (`Date.now()`) is > i32::MAX but exact integer in JS safe range.
        let epoch_ms = 1_760_000_000_123.0_f64;
        assert_eq!(js_float_to_json(epoch_ms), JsonValue::from(1_760_000_000_123_i64));
        assert_eq!(js_float_to_json(-9_007_199_254_740_991.0), JsonValue::from(-9_007_199_254_740_991_i64));
        assert_eq!(js_float_to_json(0.0), JsonValue::from(0_i64));
        assert_eq!(js_float_to_json(-0.0), JsonValue::from(0_i64));
        // Non-integral floats stay floats.
        assert_eq!(js_float_to_json(3.25).as_f64(), Some(3.25));
        // NaN / Infinity serialize as null.
        assert_eq!(js_float_to_json(f64::NAN), JsonValue::Null);
        assert_eq!(js_float_to_json(f64::INFINITY), JsonValue::Null);
    }

    #[test]
    fn validate_http_uri_syntax_enforces_scheme_host_and_literal_ip_without_dns() {
        let perms = PermissionSet::new(&[Permission::Network(vec!["api.example.com".to_owned()])]);
        let ok_uri: ureq::http::Uri = "https://api.example.com/v1/items".parse().unwrap();
        assert!(validate_target_uri_syntax(&ok_uri, &perms).is_ok());

        let denied_host: ureq::http::Uri = "https://evil.example.org/x".parse().unwrap();
        assert!(validate_target_uri_syntax(&denied_host, &perms).is_err());

        let bad_scheme: ureq::http::Uri = "ftp://api.example.com/file".parse().unwrap();
        assert!(validate_target_uri_syntax(&bad_scheme, &perms).is_err());

        let any_perms = PermissionSet::new(&[Permission::Network(vec!["127.0.0.1".to_owned()])]);
        let loopback: ureq::http::Uri = "http://127.0.0.1:8080/secret".parse().unwrap();
        assert!(validate_target_uri_syntax(&loopback, &any_perms).is_err());
    }

    #[test]
    fn task_horizon_json_draft_patch_and_filter() {
        let store = Store::open_in_memory().unwrap();
        let w48 = Horizon::Week { year: 2026, week: 48 };
        let q4 = Horizon::Quarter { year: 2026, quarter: 4 };

        let (draft, top) = parse_task_draft(&json!({
            "title": "Plugin horizon task",
            "horizon": "W2026-48",
        }))
        .unwrap();
        assert!(top);
        assert_eq!(draft.horizon, Some(w48));
        let created = store.create_task(draft, top).unwrap();
        assert_eq!(task_to_json(&created)["horizon"], "W2026-48");

        // Invalid horizon rejected on draft and patch.
        assert!(parse_task_draft(&json!({"title": "Bad", "horizon": "W2025-53"})).is_err());
        assert!(apply_task_patch(&store, created.id, &json!({"horizon": "D2025"})).is_err());

        // Patching horizon updates cleanly and filtering by horizon finds it.
        let updated = apply_task_patch(&store, created.id, &json!({"horizon": "Q2026-4"})).unwrap();
        assert_eq!(updated.horizon, Some(q4));
        assert_eq!(task_to_json(&updated)["horizon"], "Q2026-4");

        let plain = store.add_task("Plain").unwrap();
        let only_q4 = load_filtered_tasks(&store, &json!({"horizon": "Q2026-4"})).unwrap();
        assert_eq!(only_q4.iter().map(|t| t.id).collect::<Vec<_>>(), vec![created.id]);
        let only_none = load_filtered_tasks(&store, &json!({"horizon": null})).unwrap();
        assert_eq!(only_none.iter().map(|t| t.id).collect::<Vec<_>>(), vec![plain.id]);

        // Patching dueDate clears horizon; patching horizon back to null clears it.
        let dated = apply_task_patch(&store, created.id, &json!({"dueDate": "2026-11-23"})).unwrap();
        assert_eq!(dated.horizon, None);
        assert_eq!(task_to_json(&dated)["horizon"], JsonValue::Null);
        let re_horizon = apply_task_patch(&store, created.id, &json!({"horizon": "LIFE"})).unwrap();
        assert_eq!(re_horizon.horizon, Some(Horizon::Life));
        assert_eq!(re_horizon.due_date, None);
        let cleared = apply_task_patch(&store, created.id, &json!({"horizon": null})).unwrap();
        assert_eq!(cleared.horizon, None);
    }
}
