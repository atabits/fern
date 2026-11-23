//! `PluginManager` handle and single-thread `PluginWorker` runtime driving per-plugin
//! `rquickjs::AsyncRuntime` instances with memory/stack limits, interrupt deadlines,
//! circuit breaker, `plugin_outbox` echo/depth guards, `ScriptRunner`, and hot-reload.

mod commands;
mod events;
mod gate;
mod manager;
mod scripts;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context as TaskContext, Poll};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use fern_core::Store;
use fern_core::plugin_store::DEFAULT_OUTBOX_RETENTION;
use futures::channel::oneshot;
use futures::stream::{FuturesUnordered, StreamExt as _};
use futures::task::waker_ref;
use rquickjs::promise::MaybePromise;
use rquickjs::{AsyncContext, AsyncRuntime, Function, Module, Persistent, Value};
use serde_json::{Map, Value as JsonValue};

use crate::api::{
    EffectEmitterFn, LogEntry, LogLevel, OAuthConcurrencyLimiter, PluginApiState, PluginHooks,
    PluginUiEffect, PromptSenders, ServiceCallerFn, ServiceRegistrarFn, ViewRefresherFn,
    install_fern_bindings, js_to_json, json_to_js,
};
use crate::http::{HTTP_POOL_THREADS, HttpPool};
use crate::manifest::{LoadedManifest, PermissionApprovalStatus, validate_plugin_id};
use crate::module_loader::{SandboxedLoader, SandboxedResolver};
use crate::vault::SecretsVault;

use commands::{fire_due_timers, handle_worker_cmd, perform_hot_reload, render_plugin_view};
use events::schedule_outbox_drain;
use gate::{
    ClearPersistentGuard, DeactivateGateFuture, ExecGate, GateOptions, RuntimeDriverState, now_ms,
    run_with_gate, runtime_driver_task,
};
use manager::{CmdQueue, ReplyTx, WorkerCmd, WorkerWake};
pub use manager::{
    HostUpdate, HookDecision, PluginEngineConfig, PluginInfo, PluginManager, PluginMetrics,
    PluginStatus, ScriptRunResult, SearchHit,
};

pub const PLUGIN_MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
pub const PLUGIN_STACK_LIMIT_BYTES: usize = 512 * 1024;
pub const DEFAULT_EVENT_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
pub const CIRCUIT_BREAKER_THRESHOLD: i64 = 3;
pub const SHUTDOWN_BUDGET: Duration = Duration::from_millis(300);
pub const HOT_RELOAD_INTERVAL: Duration = Duration::from_secs(1);
const MAX_SERVICE_CALL_DEPTH: usize = 4;
pub(in crate::runtime) const OUTBOX_BATCH_SIZE: usize = 100;

/// Field declaration order guarantees `module_ns` and `api` (holding `Persistent` handles)
/// are ALWAYS cleared and dropped BEFORE `ctx` and `rt`, preventing `JS_FreeRuntime` aborts / UAF.
pub(in crate::runtime) struct LoadedRuntime {
    pub(in crate::runtime) generation: u64,
    cancel_flag: Arc<AtomicBool>,
    driver_state: Rc<RuntimeDriverState>,
    module_ns: Option<Persistent<rquickjs::Object<'static>>>,
    pub(in crate::runtime) api: Rc<PluginApiState>,
    gate: Arc<ExecGate>,
    ctx: AsyncContext,
    rt: AsyncRuntime,
}

impl Drop for LoadedRuntime {
    fn drop(&mut self) {
        self.cancel_flag.store(true, Ordering::Release);
        self.driver_state.stop();
        self.module_ns.take();
        self.api.clear_persistent_handles();
    }
}

pub(in crate::runtime) struct PluginSlot {
    pub(in crate::runtime) loaded_manifest: LoadedManifest,
    pub(in crate::runtime) enabled: bool,
    pub(in crate::runtime) status: PluginStatus,
    pub(in crate::runtime) metrics: PluginMetrics,
    pub(in crate::runtime) settings_values: Map<String, JsonValue>,
    pub(in crate::runtime) logs_cache: VecDeque<LogEntry>,
    pub(in crate::runtime) runtime: Option<LoadedRuntime>,
    pub(in crate::runtime) snapshot: DirFingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::runtime) struct DirFingerprint {
    files: BTreeMap<PathBuf, (Option<SystemTime>, u64)>,
}

impl DirFingerprint {
    pub(in crate::runtime) fn scan(dir: &Path) -> Self {
        let mut files = BTreeMap::new();
        collect_dir_fingerprint(dir, &mut files);
        Self { files }
    }
}

fn collect_dir_fingerprint(dir: &Path, out: &mut BTreeMap<PathBuf, (Option<SystemTime>, u64)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            let name = entry.file_name();
            if name.to_str().is_some_and(|s| s.starts_with('.')) {
                continue;
            }
            collect_dir_fingerprint(&path, out);
        } else if meta.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let is_manifest = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "manifest.json");
            if is_manifest || matches!(ext, "js" | "mjs" | "svg" | "json") {
                out.insert(path, (meta.modified().ok(), meta.len()));
            }
        }
    }
}

type LocalTask = std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>;
type ActivationWaitersMap = Rc<RefCell<HashMap<String, Vec<oneshot::Sender<Result<()>>>>>>;

pub(in crate::runtime) struct WorkerEnv {
    pub(in crate::runtime) config: PluginEngineConfig,
    pub(in crate::runtime) store: Rc<Store>,
    pub(in crate::runtime) vault: SecretsVault,
    pub(in crate::runtime) http_pool: Arc<HttpPool>,
    pub(in crate::runtime) oauth_limiter: Arc<OAuthConcurrencyLimiter>,
    pub(in crate::runtime) locale: Rc<RefCell<String>>,
    pub(in crate::runtime) theme_appearance: Rc<RefCell<String>>,
    pub(in crate::runtime) prompt_senders: PromptSenders,
    pub(in crate::runtime) next_prompt_id: Rc<Cell<u64>>,
    service_providers: Rc<RefCell<HashMap<String, String>>>,
    pub(in crate::runtime) slots: Rc<RefCell<BTreeMap<String, PluginSlot>>>,
    next_runtime_generation: Cell<u64>,
    activating: ActivationWaitersMap,
    pub(in crate::runtime) outbox_draining: Cell<bool>,
    pub(in crate::runtime) outbox_dirty: Cell<bool>,
    pub(in crate::runtime) outbox_waiters: RefCell<Vec<ReplyTx<usize>>>,
    tasks: Rc<RefCell<FuturesUnordered<LocalTask>>>,
    pending_tasks: Rc<RefCell<Vec<LocalTask>>>,
    pub(in crate::runtime) shutdown_flag: Arc<AtomicBool>,
    pub(in crate::runtime) worker_epoch: Instant,
    pub(in crate::runtime) update_tx: mpsc::Sender<HostUpdate>,
    pub(in crate::runtime) wake_callback: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl WorkerEnv {
    pub(in crate::runtime) fn emit(&self, update: HostUpdate) {
        let _ = self.update_tx.send(update);
        if let Some(cb) = &self.wake_callback {
            cb();
        }
    }

    pub(in crate::runtime) fn spawn_task(
        &self,
        fut: impl std::future::Future<Output = ()> + 'static,
    ) {
        self.pending_tasks.borrow_mut().push(Box::pin(fut));
    }

    fn next_generation(&self) -> u64 {
        let rt_gen = self.next_runtime_generation.get().max(1);
        self.next_runtime_generation.set(rt_gen.wrapping_add(1).max(1));
        rt_gen
    }

    pub(in crate::runtime) fn plugin_infos(&self) -> Vec<PluginInfo> {
        let slots = self.slots.borrow();
        slots
            .values()
            .map(|slot| slot_to_info(slot, &self.vault))
            .collect()
    }

    pub(in crate::runtime) fn flush_plugin_side_effects(self: &Rc<Self>, plugin_id: &str, expected_gen: Option<u64>) {
        let (effects, dirty) = {
            let slots = self.slots.borrow();
            let Some(slot) = slots.get(plugin_id) else {
                return;
            };
            let Some(rt) = &slot.runtime else {
                return;
            };
            if expected_gen.is_some_and(|g| rt.generation != g) {
                return;
            }
            let effects: Vec<PluginUiEffect> =
                rt.api.pending_ui_effects.borrow_mut().drain(..).collect();
            let dirty = rt.api.data_dirty.replace(false);
            (effects, dirty)
        };
        for eff in effects {
            self.emit(HostUpdate::UiEffect(eff));
        }
        if dirty {
            self.emit(HostUpdate::DataChanged);
            if !self.config.safe_mode {
                self.outbox_dirty.set(true);
                schedule_outbox_drain(self);
            }
        }
    }

    fn flush_all_pending_side_effects(self: &Rc<Self>) {
        let plugin_ids: Vec<(String, u64)> = {
            let slots = self.slots.borrow();
            slots
                .iter()
                .filter_map(|(id, slot)| {
                    let rt = slot.runtime.as_ref()?;
                    let has_work = !rt.api.pending_ui_effects.borrow().is_empty()
                        || rt.api.data_dirty.get();
                    has_work.then(|| (id.clone(), rt.generation))
                })
                .collect()
        };
        for (pid, rt_gen) in plugin_ids {
            self.flush_plugin_side_effects(&pid, Some(rt_gen));
        }
    }

    pub(in crate::runtime) fn discover_plugins_into_slots(&self) -> Result<()> {
        if self.config.safe_mode {
            return Ok(());
        }
        if !self.config.plugins_dir.exists() {
            fs::create_dir_all(&self.config.plugins_dir)?;
        }

        let entries = fs::read_dir(&self.config.plugins_dir).with_context(|| {
            format!(
                "cannot read plugins directory {}",
                self.config.plugins_dir.display()
            )
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = entry
                .file_name()
                .to_str()
                .unwrap_or_default()
                .to_owned();
            if dir_name.starts_with('.') || !validate_plugin_id(&dir_name) {
                continue;
            }
            match LoadedManifest::load_from_dir(&path) {
                Ok(loaded) => {
                    if loaded.manifest.id != dir_name {
                        continue;
                    }
                    let slot = self.build_slot(loaded)?;
                    self.slots
                        .borrow_mut()
                        .insert(slot.loaded_manifest.manifest.id.clone(), slot);
                }
                Err(err) => {
                    eprintln!(
                        "skipping invalid plugin directory {}: {err:#}",
                        path.display()
                    );
                }
            }
        }
        Ok(())
    }

    pub(in crate::runtime) fn build_slot(&self, loaded: LoadedManifest) -> Result<PluginSlot> {
        let id = &loaded.manifest.id;
        let state = self.store.plugin_state(id)?;
        let approval = loaded
            .manifest
            .check_permission_diff(&loaded.manifest_hash, state.as_ref());
        let consecutive_errors = state.as_ref().map_or(0, |s| s.consecutive_errors);
        let mut enabled = state.as_ref().is_some_and(|s| s.enabled);

        // Always evaluate permission approval BEFORE CircuitBroken so a circuit-broken
        // plugin that updates its manifest to escalate permissions cannot bypass approval
        // via Reset & Enable.
        let status = match approval {
            PermissionApprovalStatus::NeedsApproval {
                diff,
                is_first_install,
            } => {
                enabled = false;
                PluginStatus::NeedsApproval {
                    added_permissions: diff.added,
                    is_first_install,
                }
            }
            PermissionApprovalStatus::Approved => {
                if let Some(narrowed) = loaded
                    .manifest
                    .narrowed_permissions_to_persist(&loaded.manifest_hash, state.as_ref())
                {
                    let _ = self
                        .store
                        .set_plugin_permissions(id, &narrowed, &loaded.manifest_hash);
                }
                if consecutive_errors >= CIRCUIT_BREAKER_THRESHOLD {
                    enabled = false;
                    PluginStatus::CircuitBroken {
                        last_error: "auto-disabled after repeated crashes or timeouts".to_owned(),
                    }
                } else if enabled {
                    PluginStatus::Idle
                } else {
                    PluginStatus::Disabled
                }
            }
        };

        let stored_settings = state
            .as_ref()
            .map(|s| s.settings_json.as_str())
            .unwrap_or("{}");
        let settings_values = loaded.manifest.resolve_settings(stored_settings);
        let snapshot = DirFingerprint::scan(&loaded.dir);

        Ok(PluginSlot {
            loaded_manifest: loaded,
            enabled,
            status,
            metrics: PluginMetrics {
                memory_bytes: 0,
                last_exec_ms: 0,
                consecutive_errors,
            },
            settings_values,
            logs_cache: VecDeque::new(),
            runtime: None,
            snapshot,
        })
    }

    fn take_runtime_for_unload(&self, plugin_id: &str) -> Option<LoadedRuntime> {
        // Always cancel/purge pending prompts and service registrations owned by this plugin.
        self.service_providers
            .borrow_mut()
            .retain(|_, owner| owner != plugin_id);
        self.prompt_senders
            .borrow_mut()
            .retain(|_, (owner, _)| owner != plugin_id);

        let runtime_opt = {
            let mut slots = self.slots.borrow_mut();
            let slot = slots.get_mut(plugin_id)?;
            if let Some(rt) = &slot.runtime {
                slot.logs_cache = rt.api.logs.borrow().clone();
            }
            slot.runtime.take()
        };
        if let Some(rt) = &runtime_opt {
            rt.cancel_flag.store(true, Ordering::Release);
            rt.driver_state.stop();
        }
        runtime_opt
    }

    /// Synchronous shutdown-only unload called ONLY after `env.tasks` and `env.pending_tasks`
    /// have been cleared (so no suspended task holds `rt.inner.lock()`).
    fn unload_runtime_sync_shutdown(&self, plugin_id: &str, deact_budget: Option<Duration>) {
        if let Some(mut loaded) = self.take_runtime_for_unload(plugin_id) {
            if let Some(budget) = deact_budget
                && let Some(ns_pers) = loaded.module_ns.take()
            {
                loaded.gate.arm_deactivate(budget);
                let ctx = loaded.ctx.clone();
                futures::executor::block_on(ctx.with(|ctx| {
                    if let Ok(ns) = ns_pers.restore(&ctx)
                        && let Ok(Some(deact)) = ns.get::<_, Option<Function>>("deactivate")
                    {
                        let _ = deact.call::<_, Value>(());
                    }
                }));
                loaded.gate.disarm();
            }
            drop(loaded);
        }
    }

    pub(in crate::runtime) fn record_failure(&self, plugin_id: &str, expected_gen: Option<u64>, err_msg: &str) {
        let scrubbed = self.vault.scrub_secrets(plugin_id, err_msg);

        // Ignore stale async failures from a previous runtime generation.
        if let Some(rt_gen) = expected_gen {
            let slots = self.slots.borrow();
            let matches = slots
                .get(plugin_id)
                .and_then(|s| s.runtime.as_ref())
                .is_some_and(|rt| rt.generation == rt_gen);
            if !matches {
                return;
            }
        }

        let count = self
            .store
            .increment_plugin_errors(plugin_id)
            .unwrap_or(CIRCUIT_BREAKER_THRESHOLD);
        let tripped = count >= CIRCUIT_BREAKER_THRESHOLD;
        if tripped {
            let _ = self.store.set_plugin_enabled(plugin_id, false);
        }

        let (status, metrics) = {
            let mut slots = self.slots.borrow_mut();
            let Some(slot) = slots.get_mut(plugin_id) else {
                return;
            };
            if let Some(rt) = &slot.runtime {
                rt.api.push_log(LogLevel::Error, &scrubbed);
                slot.logs_cache = rt.api.logs.borrow().clone();
            } else {
                if slot.logs_cache.len() >= crate::api::LOG_RING_CAPACITY {
                    slot.logs_cache.pop_front();
                }
                slot.logs_cache.push_back(LogEntry {
                    timestamp_ms: now_ms(),
                    level: LogLevel::Error,
                    message: scrubbed.clone(),
                });
            }
            slot.metrics.consecutive_errors = count;
            if tripped {
                slot.enabled = false;
                slot.status = PluginStatus::CircuitBroken {
                    last_error: scrubbed,
                };
            } else {
                slot.status = PluginStatus::Error(scrubbed);
            }
            (slot.status.clone(), slot.metrics.clone())
        };

        if tripped {
            let _ = self.take_runtime_for_unload(plugin_id);
        }

        self.emit(HostUpdate::PluginStatusChanged {
            plugin_id: plugin_id.to_owned(),
            status,
            metrics,
        });
    }

    pub(in crate::runtime) fn record_success(
        &self,
        plugin_id: &str,
        expected_gen: Option<u64>,
        elapsed_ms: u64,
        mem_bytes: u64,
    ) {
        let mut need_notify = false;
        let status_metrics = {
            let mut slots = self.slots.borrow_mut();
            let Some(slot) = slots.get_mut(plugin_id) else {
                return;
            };
            if let Some(rt_gen) = expected_gen
                && !slot.runtime.as_ref().is_some_and(|rt| rt.generation == rt_gen)
            {
                return;
            }
            if !slot.enabled
                || matches!(
                    slot.status,
                    PluginStatus::Disabled
                        | PluginStatus::NeedsApproval { .. }
                        | PluginStatus::CircuitBroken { .. }
                )
            {
                return;
            }
            if slot.metrics.consecutive_errors > 0 {
                let _ = self.store.reset_plugin_errors(plugin_id);
                slot.metrics.consecutive_errors = 0;
                need_notify = true;
            }
            slot.metrics.last_exec_ms = elapsed_ms;
            slot.metrics.memory_bytes = mem_bytes;
            if !matches!(slot.status, PluginStatus::Active) {
                slot.status = PluginStatus::Active;
                need_notify = true;
            }
            (slot.status.clone(), slot.metrics.clone())
        };
        if need_notify {
            self.emit(HostUpdate::PluginStatusChanged {
                plugin_id: plugin_id.to_owned(),
                status: status_metrics.0,
                metrics: status_metrics.1,
            });
        }
    }
}

/// Async unload used during normal operation (disable/reload/purge) so `deactivate()` awaits
/// `ctx.with(...)` cleanly without `block_on` deadlocking on `rt.inner.lock()`.
pub(in crate::runtime) async fn unload_runtime_async(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    deact_budget: Option<Duration>,
) {
    if let Some(mut loaded) = env.take_runtime_for_unload(plugin_id) {
        if let Some(budget) = deact_budget
            && let Some(ns_pers) = loaded.module_ns.take()
        {
            let ctx = loaded.ctx.clone();
            let gate = Arc::clone(&loaded.gate);
            DeactivateGateFuture {
                fut: ctx.with(|ctx| {
                    if let Ok(ns) = ns_pers.restore(&ctx)
                        && let Ok(Some(deact)) = ns.get::<_, Option<Function>>("deactivate")
                    {
                        let _ = deact.call::<_, Value>(());
                    }
                }),
                gate,
                budget,
            }
            .await;
        }
        drop(loaded);
    }
}

pub(in crate::runtime) fn spawn_startup_activations(env: &Rc<WorkerEnv>) {
    if env.config.safe_mode {
        return;
    }
    let startup_ids: Vec<String> = env
        .slots
        .borrow()
        .values()
        .filter(|s| {
            s.enabled
                && s.runtime.is_none()
                && (s.loaded_manifest.manifest.matches_activation_event("onStartup")
                    || s.loaded_manifest.manifest.matches_activation_event("*"))
        })
        .map(|s| s.loaded_manifest.manifest.id.clone())
        .collect();
    for pid in startup_ids {
        let env_c = Rc::clone(env);
        env.spawn_task(async move {
            let _ = ensure_activated(&env_c, &pid).await;
        });
    }
}

pub(in crate::runtime) fn maybe_spawn_startup_activation(env: &Rc<WorkerEnv>, plugin_id: &str) {
    if env.config.safe_mode {
        return;
    }
    let should_activate = env.slots.borrow().get(plugin_id).is_some_and(|s| {
        s.enabled
            && s.runtime.is_none()
            && (s.loaded_manifest.manifest.matches_activation_event("onStartup")
                || s.loaded_manifest.manifest.matches_activation_event("*"))
    });
    if should_activate {
        let env_c = Rc::clone(env);
        let pid = plugin_id.to_owned();
        env.spawn_task(async move {
            let _ = ensure_activated(&env_c, &pid).await;
        });
    }
}

fn slot_to_info(slot: &PluginSlot, vault: &SecretsVault) -> PluginInfo {
    let mut secret_keys_present = BTreeMap::new();
    for spec in &slot.loaded_manifest.manifest.settings {
        if spec.kind == crate::manifest::SettingKind::Secret {
            let has = vault
                .has(&slot.loaded_manifest.manifest.id, &spec.key)
                .unwrap_or(false);
            secret_keys_present.insert(spec.key.clone(), has);
        }
    }
    PluginInfo {
        manifest: slot.loaded_manifest.manifest.clone(),
        manifest_hash: slot.loaded_manifest.manifest_hash.clone(),
        dir: slot.loaded_manifest.dir.clone(),
        sidebar_icons: slot.loaded_manifest.sidebar_icons.clone(),
        enabled: slot.enabled,
        status: slot.status.clone(),
        metrics: slot.metrics.clone(),
        settings_values: slot.settings_values.clone(),
        secret_keys_present,
    }
}

pub(in crate::runtime) fn worker_main(
    config: PluginEngineConfig,
    queue: Arc<CmdQueue>,
    wake: Arc<WorkerWake>,
    shutdown_flag: Arc<AtomicBool>,
    update_tx: mpsc::Sender<HostUpdate>,
    ready_tx: mpsc::SyncSender<Result<()>>,
) {
    struct QueueCloseGuard(Arc<CmdQueue>);
    impl Drop for QueueCloseGuard {
        fn drop(&mut self) {
            self.0.close_and_clear();
        }
    }
    let _queue_guard = QueueCloseGuard(Arc::clone(&queue));

    let init_res = (|| -> Result<WorkerEnv> {
        let store = match &config.db_path {
            Some(path) => Rc::new(Store::open(path)?),
            None => Rc::new(Store::open_in_memory()?),
        };
        let vault = SecretsVault::open(&config.secrets_vault_path, &config.secrets_key_path)?;
        let http_pool = Arc::new(HttpPool::new(HTTP_POOL_THREADS));
        let env = WorkerEnv {
            locale: Rc::new(RefCell::new(config.locale.clone())),
            theme_appearance: Rc::new(RefCell::new(config.theme_appearance.clone())),
            config,
            store,
            vault,
            http_pool,
            oauth_limiter: Arc::new(OAuthConcurrencyLimiter::default()),
            prompt_senders: Rc::new(RefCell::new(HashMap::new())),
            next_prompt_id: Rc::new(Cell::new(1)),
            service_providers: Rc::new(RefCell::new(HashMap::new())),
            slots: Rc::new(RefCell::new(BTreeMap::new())),
            next_runtime_generation: Cell::new(1),
            activating: Rc::new(RefCell::new(HashMap::new())),
            outbox_draining: Cell::new(false),
            outbox_dirty: Cell::new(false),
            outbox_waiters: RefCell::new(Vec::new()),
            tasks: Rc::new(RefCell::new(FuturesUnordered::new())),
            pending_tasks: Rc::new(RefCell::new(Vec::new())),
            shutdown_flag,
            worker_epoch: Instant::now(),
            update_tx,
            wake_callback: None,
        };
        Ok(env)
    })();

    let env = match init_res {
        Ok(mut e) => {
            e.wake_callback = e.config.wake_callback.clone();
            if let Err(err) = e.discover_plugins_into_slots() {
                let _ = ready_tx.send(Err(err));
                return;
            }
            // Startup outbox hygiene: clear if no enabled event subscribers; otherwise trim tail to 500.
            let has_event_subs = !e.config.safe_mode && has_any_event_subscribers(&e);
            if has_event_subs {
                let _ = e.store.trim_plugin_outbox(DEFAULT_OUTBOX_RETENTION);
            } else {
                let _ = e.store.trim_plugin_outbox(0);
            }
            let _ = ready_tx.send(Ok(()));
            e
        }
        Err(err) => {
            let _ = ready_tx.send(Err(err));
            return;
        }
    };

    let env = Rc::new(env);
    let waker = waker_ref(&wake);
    let mut task_cx = TaskContext::from_waker(&waker);

    // Activate any `onStartup` / `*` plugins immediately after worker boot.
    spawn_startup_activations(&env);

    let mut next_hot_reload = Instant::now() + HOT_RELOAD_INTERVAL;

    loop {
        if env.shutdown_flag.load(Ordering::Acquire) {
            break;
        }

        // 1. Drain commands.
        let mut should_exit = false;
        for cmd in queue.pop_all() {
            if matches!(cmd, WorkerCmd::Shutdown) {
                should_exit = true;
                break;
            }
            handle_worker_cmd(&env, cmd);
        }
        if should_exit || env.shutdown_flag.load(Ordering::Acquire) {
            break;
        }

        // 2. Drive spawned async plugin tasks (including per-runtime gated job drivers).
        // Drain newly spawned tasks from `pending_tasks` before each poll so callbacks
        // invoked inside a polled task (e.g. `fern.views.refresh`, `fern.services.call`)
        // never re-entrantly borrow `env.tasks`.
        loop {
            let poll_res = {
                let mut tasks = env.tasks.borrow_mut();
                for task in env.pending_tasks.borrow_mut().drain(..) {
                    tasks.push(task);
                }
                if tasks.is_empty() {
                    break;
                }
                tasks.poll_next_unpin(&mut task_cx)
            };
            let has_new = !env.pending_tasks.borrow().is_empty();
            match poll_res {
                Poll::Ready(Some(())) => continue,
                Poll::Ready(None) | Poll::Pending if has_new => continue,
                Poll::Ready(None) | Poll::Pending => break,
            }
        }

        // 3. Flush any UI effects / data_dirty flags queued before an async task suspended.
        env.flush_all_pending_side_effects();

        // 4. Fire due `fern.timers` intervals.
        fire_due_timers(&env);

        // 5. Check hot-reload if enabled.
        let now = Instant::now();
        if env.config.hot_reload && now >= next_hot_reload {
            next_hot_reload = now + HOT_RELOAD_INTERVAL;
            let _ = perform_hot_reload(&env);
        }

        // 6. Sleep until next command, async wakeup, timer, or hot-reload tick.
        if !wake.woken.swap(false, Ordering::AcqRel) && env.pending_tasks.borrow().is_empty() {
            let timeout = next_wakeup_delay(&env, next_hot_reload);
            thread::park_timeout(timeout);
        }
    }

    // Clean shutdown within <=300ms budget:
    // Close queue immediately so any in-flight `PluginManager::call` unblocks, cancel OAuth/HTTP
    // work, drop all pending async tasks (releasing any `rt.inner.lock()` and invoking RAII guards),
    // and then run each active plugin's `deactivate()` strictly within the remaining budget.
    queue.close_and_clear();
    env.http_pool.close_nonblocking();
    env.prompt_senders.borrow_mut().clear();
    for slot in env.slots.borrow().values() {
        if let Some(rt) = &slot.runtime {
            rt.cancel_flag.store(true, Ordering::Release);
            rt.driver_state.stop();
        }
    }
    env.pending_tasks.borrow_mut().clear();
    env.tasks.borrow_mut().clear();
    env.activating.borrow_mut().clear();
    env.outbox_waiters.borrow_mut().clear();

    let active_ids: Vec<String> = env.slots.borrow().keys().cloned().collect();
    let deadline = Instant::now() + Duration::from_millis(240);
    for id in active_ids {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let deact_budget = if remaining >= Duration::from_millis(5) {
            Some(remaining.min(Duration::from_millis(100)))
        } else {
            None
        };
        env.unload_runtime_sync_shutdown(&id, deact_budget);
    }
}

fn has_any_event_subscribers(env: &WorkerEnv) -> bool {
    let any_plugin = env.slots.borrow().values().any(|s| {
        s.enabled
            && (s.loaded_manifest.manifest.activation_events.is_empty()
                || s.loaded_manifest
                    .manifest
                    .activation_events
                    .iter()
                    .any(|ev| ev == "*" || ev == "onStartup" || ev.starts_with("onEvent:")))
    });
    if any_plugin {
        return true;
    }
    env.store
        .user_scripts()
        .unwrap_or_default()
        .into_iter()
        .any(|s| s.enabled && s.trigger_kind == "event")
}

fn next_wakeup_delay(env: &WorkerEnv, next_hot_reload: Instant) -> Duration {
    let now = Instant::now();
    let mut earliest = if env.config.hot_reload {
        next_hot_reload.saturating_duration_since(now)
    } else {
        Duration::from_millis(500)
    };
    let slots = env.slots.borrow();
    for slot in slots.values() {
        if let Some(rt) = &slot.runtime {
            for timer in rt.api.timers.borrow().values() {
                let d = timer.next_fire.saturating_duration_since(now);
                if d < earliest {
                    earliest = d;
                }
            }
        }
    }
    earliest.min(Duration::from_millis(500)).max(Duration::from_millis(5))
}

pub(in crate::runtime) async fn ensure_activated(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    ensure!(!env.config.safe_mode, "plugins are disabled in safe mode");
    let already_active = env
        .slots
        .borrow()
        .get(plugin_id)
        .is_some_and(|s| s.runtime.is_some());
    if already_active {
        return Ok(());
    }

    // Deduplicate concurrent activations for the same `plugin_id` so two callers never
    // race to construct duplicate QuickJS runtimes and overwrite `slot.runtime`.
    let maybe_waiter_rx = {
        let mut activating = env.activating.borrow_mut();
        if let Some(waiters) = activating.get_mut(plugin_id) {
            let (tx, rx) = oneshot::channel();
            waiters.push(tx);
            Some(rx)
        } else {
            activating.insert(plugin_id.to_owned(), Vec::new());
            None
        }
    };
    if let Some(rx) = maybe_waiter_rx {
        return match rx.await {
            Ok(res) => res,
            Err(_) => Err(anyhow!("plugin {plugin_id:?} activation cancelled")),
        };
    }

    struct ActivationGuard {
        env: Rc<WorkerEnv>,
        plugin_id: String,
        finished: bool,
    }
    impl ActivationGuard {
        fn finish(mut self, outcome: &Result<()>) {
            self.finished = true;
            let waiters = self
                .env
                .activating
                .borrow_mut()
                .remove(&self.plugin_id)
                .unwrap_or_default();
            for tx in waiters {
                let reply = match outcome {
                    Ok(()) => Ok(()),
                    Err(err) => Err(anyhow!("{err:#}")),
                };
                let _ = tx.send(reply);
            }
        }
    }
    impl Drop for ActivationGuard {
        fn drop(&mut self) {
            if !self.finished {
                let waiters = self
                    .env
                    .activating
                    .borrow_mut()
                    .remove(&self.plugin_id)
                    .unwrap_or_default();
                for tx in waiters {
                    let _ = tx.send(Err(anyhow!("plugin activation cancelled")));
                }
            }
        }
    }

    let act_guard = ActivationGuard {
        env: Rc::clone(env),
        plugin_id: plugin_id.to_owned(),
        finished: false,
    };

    let outcome = ensure_activated_inner(env, plugin_id).await;
    act_guard.finish(&outcome);
    outcome
}

async fn ensure_activated_inner(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    let (loaded, settings, logs_prev) = {
        let slots = env.slots.borrow();
        let slot = slots
            .get(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        if slot.runtime.is_some() {
            return Ok(());
        }
        ensure!(slot.enabled, "plugin {plugin_id:?} is disabled");
        (
            slot.loaded_manifest.clone(),
            slot.settings_values.clone(),
            slot.logs_cache.clone(),
        )
    };

    // Re-verify persisted permission approval right before launching the QuickJS runtime.
    let db_state = env.store.plugin_state(plugin_id)?;
    match loaded
        .manifest
        .check_permission_diff(&loaded.manifest_hash, db_state.as_ref())
    {
        PermissionApprovalStatus::NeedsApproval {
            diff,
            is_first_install,
        } => {
            let _ = env.store.set_plugin_enabled(plugin_id, false);
            if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
                slot.enabled = false;
                slot.status = PluginStatus::NeedsApproval {
                    added_permissions: diff.added,
                    is_first_install,
                };
            }
            env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
            bail!("plugin {plugin_id:?} requires permission approval");
        }
        PermissionApprovalStatus::Approved => {
            if let Some(narrowed) = loaded
                .manifest
                .narrowed_permissions_to_persist(&loaded.manifest_hash, db_state.as_ref())
            {
                let _ = env.store.set_plugin_permissions(
                    plugin_id,
                    &narrowed,
                    &loaded.manifest_hash,
                );
            }
        }
    }

    let entry_source = fs::read_to_string(&loaded.entry_path).with_context(|| {
        format!("cannot read entry file {}", loaded.entry_path.display())
    })?;
    let entry_name = loaded.entry_path.to_string_lossy().into_owned();

    // Declare `rt` and `ctx` BEFORE `api` and `clear_guard` so Rust's reverse-order local drop
    // ALWAYS clears all `Persistent` handles before `ctx` and `rt` drop (even on async cancel).
    let rt = AsyncRuntime::new().context("failed to create QuickJS AsyncRuntime")?;
    rt.set_memory_limit(PLUGIN_MEMORY_LIMIT_BYTES).await;
    rt.set_max_stack_size(PLUGIN_STACK_LIMIT_BYTES).await;
    rt.set_loader(
        SandboxedResolver::new(loaded.dir.clone()),
        SandboxedLoader::new(loaded.dir.clone()),
    )
    .await;

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let gate = ExecGate::new(
        env.worker_epoch,
        Arc::clone(&env.shutdown_flag),
        Arc::clone(&cancel_flag),
    );
    let gate_int = Arc::clone(&gate);
    rt.set_interrupt_handler(Some(Box::new(move || gate_int.should_interrupt())))
        .await;

    let ctx = AsyncContext::full(&rt)
        .await
        .context("failed to create QuickJS AsyncContext")?;

    let declared_views: HashSet<String> = loaded
        .manifest
        .contributions
        .sidebar
        .iter()
        .map(|v| v.id.clone())
        .chain(
            loaded
                .manifest
                .contributions
                .pages
                .iter()
                .map(|v| v.id.clone()),
        )
        .collect();
    let secret_keys: HashSet<String> = loaded
        .manifest
        .settings
        .iter()
        .filter(|s| s.kind == crate::manifest::SettingKind::Secret)
        .map(|s| s.key.clone())
        .collect();

    let pid_reg = plugin_id.to_owned();
    let providers_reg = Rc::clone(&env.service_providers);
    let service_registrar: ServiceRegistrarFn = Rc::new(move |svc_name| {
        providers_reg.borrow_mut().insert(svc_name, pid_reg.clone());
    });

    let env_for_svc = Rc::clone(env);
    let service_caller: ServiceCallerFn = Rc::new(move |svc_name, arg, depth| {
        let (tx, rx) = oneshot::channel();
        let env_call = Rc::clone(&env_for_svc);
        env_for_svc.spawn_task(async move {
            let res = invoke_service(&env_call, &svc_name, arg, depth).await;
            let _ = tx.send(res);
        });
        rx
    });

    let env_for_view = Rc::clone(env);
    let pid_for_view = plugin_id.to_owned();
    let view_refresher: ViewRefresherFn = Rc::new(move |view_id| {
        let env_r = Rc::clone(&env_for_view);
        let pid_r = pid_for_view.clone();
        env_for_view.spawn_task(async move {
            let _ = render_plugin_view(&env_r, &pid_r, &view_id, false).await;
        });
    });

    let tx_for_eff = env.update_tx.clone();
    let wake_for_eff = env.wake_callback.clone();
    let effect_emitter: EffectEmitterFn = Arc::new(move |eff| {
        let _ = tx_for_eff.send(HostUpdate::UiEffect(eff));
        if let Some(cb) = &wake_for_eff {
            cb();
        }
    });

    let api = Rc::new(PluginApiState {
        plugin_id: plugin_id.to_owned(),
        permissions: loaded.permissions.clone(),
        store: Rc::clone(&env.store),
        vault: env.vault.clone(),
        http_pool: Arc::clone(&env.http_pool),
        oauth_limiter: Arc::clone(&env.oauth_limiter),
        write_depth: Cell::new(0),
        active_service_depth: Cell::new(0),
        batch_dirty_stack: RefCell::new(Vec::new()),
        data_dirty: Cell::new(false),
        declared_views,
        settings: RefCell::new(settings),
        secret_keys,
        locale: Rc::clone(&env.locale),
        theme_appearance: Rc::clone(&env.theme_appearance),
        logs: RefCell::new(logs_prev),
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
        prompt_senders: Rc::clone(&env.prompt_senders),
        next_prompt_id: Rc::clone(&env.next_prompt_id),
        service_registrar,
        service_caller,
        view_refresher,
        effect_emitter,
        shutdown_flag: Arc::clone(&cancel_flag),
    });

    let mut clear_guard = ClearPersistentGuard::new(
        Rc::clone(&api),
        Some(Arc::clone(&cancel_flag)),
        Some((Rc::clone(&env.prompt_senders), plugin_id.to_owned())),
    );

    let start = Instant::now();
    let api_init = Rc::clone(&api);
    let loaded_ns_res = run_with_gate(
        &ctx,
        &gate,
        GateOptions {
            budget: DEFAULT_COMMAND_TIMEOUT,
            driver_state: None,
            api: Some(Rc::clone(&api)),
            write_depth: 0,
            service_depth: 0,
        },
        async move |ctx| {
            install_fern_bindings(&ctx, Rc::clone(&api_init), false)?;
            let declared = Module::declare(ctx.clone(), entry_name, entry_source)?;
            let (evaluated, promise) = declared.eval()?;
            promise.into_future::<()>().await?;
            let ns = evaluated.namespace()?;
            if let Ok(Some(activate_fn)) = ns.get::<_, Option<Function>>("activate") {
                let res: MaybePromise = activate_fn.call(())?;
                let _ = res.into_future::<Value>().await?;
            }
            Ok(Persistent::save(&ctx, ns))
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    match loaded_ns_res {
        Ok(module_ns) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            let generation = env.next_generation();
            let driver_state = RuntimeDriverState::new();
            let weak_rt = rt.weak();
            let gate_for_driver = Arc::clone(&gate);
            let driver_for_task = Rc::clone(&driver_state);

            let still_valid = {
                let mut slots = env.slots.borrow_mut();
                if let Some(slot) = slots.get_mut(plugin_id)
                    && slot.enabled
                    && slot.runtime.is_none()
                    && slot.loaded_manifest.manifest_hash == loaded.manifest_hash
                {
                    clear_guard.disarm();
                    slot.runtime = Some(LoadedRuntime {
                        generation,
                        cancel_flag,
                        driver_state,
                        module_ns: Some(module_ns),
                        api,
                        gate,
                        ctx,
                        rt,
                    });
                    true
                } else {
                    // Drop `module_ns` and clear `api` explicitly while `ctx`/`rt` are alive.
                    drop(module_ns);
                    drop(clear_guard);
                    drop(api);
                    drop(ctx);
                    drop(rt);
                    false
                }
            };

            if !still_valid {
                env.service_providers
                    .borrow_mut()
                    .retain(|_, owner| owner != plugin_id);
                bail!("plugin {plugin_id:?} was disabled or reloaded during activation");
            }

            // Spawn the single persistent per-runtime gated job driver task.
            let env_driver = Rc::clone(env);
            let pid_driver = plugin_id.to_owned();
            env.spawn_task(runtime_driver_task(
                env_driver,
                pid_driver,
                generation,
                weak_rt,
                gate_for_driver,
                driver_for_task,
            ));

            env.record_success(plugin_id, Some(generation), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(generation));
            Ok(())
        }
        Err(err) => {
            let scrubbed = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            // Preserve any console logs recorded before activation failed.
            if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
                slot.logs_cache = api.logs.borrow().clone();
            }
            // Drop `clear_guard`, `api`, `ctx`, `rt` cleanly in safe order before recording failure.
            drop(clear_guard);
            drop(api);
            drop(ctx);
            drop(rt);
            env.service_providers
                .borrow_mut()
                .retain(|_, owner| owner != plugin_id);
            env.record_failure(plugin_id, None, &scrubbed);
            Err(anyhow!("{scrubbed}"))
        }
    }
}

pub(in crate::runtime) async fn invoke_service(
    env: &Rc<WorkerEnv>,
    service_name: &str,
    arg: JsonValue,
    depth: usize,
) -> Result<JsonValue> {
    ensure!(
        depth < MAX_SERVICE_CALL_DEPTH,
        "inter-plugin service call depth exceeded {MAX_SERVICE_CALL_DEPTH}"
    );
    let provider_id = env
        .service_providers
        .borrow()
        .get(service_name)
        .cloned()
        .with_context(|| format!("service {service_name:?} is not registered"))?;

    let (rt_gen, rt, ctx, gate, api, driver_state) = active_runtime_handles(env, &provider_id)?;
    let svc_name = service_name.to_owned();
    let api_c = Rc::clone(&api);
    let start = Instant::now();
    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions {
            budget: DEFAULT_COMMAND_TIMEOUT,
            driver_state: Some(driver_state),
            api: Some(Rc::clone(&api)),
            write_depth: 0,
            service_depth: depth + 1,
        },
        async move |ctx| {
            let handler = api_c
                .services
                .borrow()
                .get(&svc_name)
                .cloned()
                .ok_or_else(|| {
                    rquickjs::Exception::throw_message(&ctx, &format!("service {svc_name:?} missing"))
                })?;
            let f = handler.restore(&ctx)?;
            let js_arg = json_to_js(&ctx, &arg)?;
            let raw: MaybePromise = f.call((js_arg,))?;
            let val = raw.into_future::<Value>().await?;
            js_to_json(&ctx, val)
                .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("{e:#}")))
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    match res {
        Ok(out) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(&provider_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(&provider_id, Some(rt_gen));
            Ok(out)
        }
        Err(err) => {
            // Scrub provider secrets BEFORE returning error across plugin boundaries.
            let scrubbed = env.vault.scrub_secrets(&provider_id, &format!("{err:#}"));
            env.record_failure(&provider_id, Some(rt_gen), &scrubbed);
            Err(anyhow!("{scrubbed}"))
        }
    }
}

pub(in crate::runtime) type RuntimeHandles = (
    u64,
    AsyncRuntime,
    AsyncContext,
    Arc<ExecGate>,
    Rc<PluginApiState>,
    Rc<RuntimeDriverState>,
);

pub(in crate::runtime) fn active_runtime_handles(env: &WorkerEnv, plugin_id: &str) -> Result<RuntimeHandles> {
    let slots = env.slots.borrow();
    let slot = slots
        .get(plugin_id)
        .with_context(|| format!("plugin {plugin_id:?} not found"))?;
    let rt = slot
        .runtime
        .as_ref()
        .with_context(|| format!("plugin {plugin_id:?} is not active"))?;
    Ok((
        rt.generation,
        rt.rt.clone(),
        rt.ctx.clone(),
        Arc::clone(&rt.gate),
        Rc::clone(&rt.api),
        Rc::clone(&rt.driver_state),
    ))
}

#[cfg(test)]
mod tests;
