//! `PluginManager` handle and single-thread `PluginWorker` runtime driving per-plugin
//! `rquickjs::AsyncRuntime` instances with memory/stack limits, interrupt deadlines,
//! circuit breaker, `plugin_outbox` echo/depth guards, `ScriptRunner`, and hot-reload.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context as TaskContext, Poll};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use fern_core::Store;
use fern_core::model::Id;
use fern_core::plugin_store::{DEFAULT_OUTBOX_RETENTION, MAX_EVENT_DEPTH, UserScript};
use futures::channel::oneshot;
use futures::stream::{FuturesUnordered, StreamExt as _};
use futures::task::{ArcWake, waker_ref};
use rquickjs::context::EvalOptions;
use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::promise::MaybePromise;
use rquickjs::runtime::AsyncWeakRuntime;
use rquickjs::{
    AsyncContext, AsyncRuntime, CaughtError, Ctx, Function, Module, Persistent, Value,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue, json};
use uuid::Uuid;

use crate::api::{
    EffectEmitterFn, LogEntry, LogLevel, MAX_TIMER_INTERVAL_MS, MIN_TIMER_INTERVAL_MS,
    OAuthConcurrencyLimiter, PluginApiState, PluginHooks, PluginUiEffect, PromptReplyValue,
    PromptSenders, RenderScopeGuard, ServiceCallerFn, ServiceRegistrarFn, ViewRefresherFn,
    event_permission, install_fern_bindings, js_to_json, js_value_to_ui_node, json_to_js,
    task_to_json,
};
use crate::http::{HTTP_POOL_THREADS, HttpPool};
use crate::manifest::{
    LoadedManifest, PermissionApprovalStatus, PluginManifest, validate_plugin_id,
};
use crate::module_loader::{
    SandboxedLoader, SandboxedResolver, VIRTUAL_FERN_MODULE, VIRTUAL_FERN_UI_MODULE,
};
use crate::permissions::{Permission, PermissionSet};
use crate::ui_ast::{UiEvent, UiTreePayload};
use crate::vault::SecretsVault;

pub const PLUGIN_MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
pub const PLUGIN_STACK_LIMIT_BYTES: usize = 512 * 1024;
pub const DEFAULT_EVENT_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
pub const CIRCUIT_BREAKER_THRESHOLD: i64 = 3;
pub const SHUTDOWN_BUDGET: Duration = Duration::from_millis(300);
pub const HOT_RELOAD_INTERVAL: Duration = Duration::from_secs(1);
const MAX_SERVICE_CALL_DEPTH: usize = 4;
const OUTBOX_BATCH_SIZE: usize = 100;

#[derive(Clone)]
pub struct PluginEngineConfig {
    pub plugins_dir: PathBuf,
    pub db_path: Option<PathBuf>,
    pub secrets_vault_path: PathBuf,
    pub secrets_key_path: PathBuf,
    pub safe_mode: bool,
    pub locale: String,
    pub theme_appearance: String,
    pub hot_reload: bool,
    pub wake_callback: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginStatus {
    Disabled,
    Idle,
    Active,
    NeedsApproval {
        added_permissions: Vec<Permission>,
        is_first_install: bool,
    },
    CircuitBroken {
        last_error: String,
    },
    Error(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMetrics {
    pub memory_bytes: u64,
    pub last_exec_ms: u64,
    pub consecutive_errors: i64,
}

#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub manifest: PluginManifest,
    pub manifest_hash: String,
    pub dir: PathBuf,
    pub sidebar_icons: BTreeMap<String, String>,
    pub enabled: bool,
    pub status: PluginStatus,
    pub metrics: PluginMetrics,
    pub settings_values: Map<String, JsonValue>,
    pub secret_keys_present: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
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

#[derive(Clone, Debug, PartialEq)]
pub enum HookDecision {
    Allow,
    Modify(JsonValue),
    Cancel(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptRunResult {
    pub ok: bool,
    pub return_value: Option<JsonValue>,
    pub logs: Vec<LogEntry>,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub data_changed: bool,
}

#[derive(Clone, Debug)]
pub enum HostUpdate {
    RegistryChanged(Vec<PluginInfo>),
    PluginStatusChanged {
        plugin_id: String,
        status: PluginStatus,
        metrics: PluginMetrics,
    },
    ViewRendered(UiTreePayload),
    ViewError {
        plugin_id: String,
        view_id: String,
        error: String,
    },
    UiEffect(PluginUiEffect),
    DataChanged,
}

type ReplyTx<T> = std::sync::mpsc::SyncSender<Result<T>>;

enum WorkerCmd {
    GetPlugins(ReplyTx<Vec<PluginInfo>>),
    ReloadAll(ReplyTx<Vec<PluginInfo>>),
    ReloadPlugin {
        plugin_id: String,
        reply: ReplyTx<()>,
    },
    SetEnabled {
        plugin_id: String,
        enabled: bool,
        reply: ReplyTx<()>,
    },
    ApprovePermissions {
        plugin_id: String,
        reply: ReplyTx<()>,
    },
    ResetAndEnable {
        plugin_id: String,
        reply: ReplyTx<()>,
    },
    UpdateSettings {
        plugin_id: String,
        settings: Map<String, JsonValue>,
        reply: ReplyTx<()>,
    },
    SetSecret {
        plugin_id: String,
        key: String,
        value: Option<String>,
        reply: ReplyTx<()>,
    },
    GetLogs {
        plugin_id: String,
        reply: ReplyTx<Vec<LogEntry>>,
    },
    ClearLogs {
        plugin_id: String,
        reply: ReplyTx<()>,
    },
    RenderView {
        plugin_id: String,
        view_id: String,
        mount: bool,
        reply: ReplyTx<UiTreePayload>,
    },
    UnmountView {
        plugin_id: String,
        view_id: String,
        reply: ReplyTx<()>,
    },
    DispatchUiEvent {
        plugin_id: String,
        event: UiEvent,
        reply: ReplyTx<Option<UiTreePayload>>,
    },
    RunCommand {
        plugin_id: String,
        command_id: String,
        arg: Option<String>,
        reply: ReplyTx<JsonValue>,
    },
    Search {
        query: String,
        reply: ReplyTx<Vec<SearchHit>>,
    },
    ResolvePrompt {
        request_id: u64,
        reply_val: PromptReplyValue,
        reply: ReplyTx<()>,
    },
    DispatchOutbox(ReplyTx<usize>),
    EmitAppReady(ReplyTx<()>),
    RunHook {
        hook: HookKind,
        payload: JsonValue,
        reply: ReplyTx<HookDecision>,
    },
    TestRunScript {
        script: UserScript,
        arg: Option<JsonValue>,
        reply: ReplyTx<ScriptRunResult>,
    },
    SetAppearance {
        locale: Option<String>,
        theme_appearance: Option<String>,
    },
    CheckHotReload(ReplyTx<Vec<String>>),
    PurgePluginData {
        plugin_id: String,
        reply: ReplyTx<()>,
    },
    Shutdown,
}

#[derive(Clone, Copy, Debug)]
enum HookKind {
    BeforeTaskCreate,
    BeforeTaskComplete,
    QuickAddTransform,
    OnReminder,
}

struct WorkerWake {
    thread: Thread,
    woken: AtomicBool,
}

impl ArcWake for WorkerWake {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.woken.store(true, Ordering::Release);
        arc_self.thread.unpark();
    }
}

struct CmdQueueInner {
    items: VecDeque<WorkerCmd>,
    closed: bool,
}

struct CmdQueue {
    inner: Mutex<CmdQueueInner>,
    wake: Arc<WorkerWake>,
}

impl CmdQueue {
    /// Enqueues a command unless the queue has been closed; returns `false` if closed.
    fn push(&self, cmd: WorkerCmd) -> bool {
        let pushed = if let Ok(mut guard) = self.inner.lock() {
            if guard.closed {
                false
            } else {
                guard.items.push_back(cmd);
                true
            }
        } else {
            false
        };
        if pushed {
            ArcWake::wake_by_ref(&self.wake);
        }
        pushed
    }

    fn pop_all(&self) -> Vec<WorkerCmd> {
        let Ok(mut guard) = self.inner.lock() else {
            return Vec::new();
        };
        guard.items.drain(..).collect()
    }

    /// Marks the queue closed and immediately drops any queued commands so callers waiting
    /// on `ReplyTx` channels unblock without deadlocking.
    fn close_and_clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.closed = true;
            guard.items.clear();
        }
        ArcWake::wake_by_ref(&self.wake);
    }
}

/// Thread-safe handle used by `plugin_ui` and application code to interact with `PluginWorker`.
#[derive(Clone)]
pub struct PluginManager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    queue: Arc<CmdQueue>,
    shutdown_flag: Arc<AtomicBool>,
    stopped: AtomicBool,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for ManagerInner {
    fn drop(&mut self) {
        self.shutdown_internal();
    }
}

impl ManagerInner {
    fn shutdown_internal(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.shutdown_flag.store(true, Ordering::Release);
        let _ = self.queue.push(WorkerCmd::Shutdown);
        self.queue.close_and_clear();
        if let Ok(mut guard) = self.handle.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
        self.queue.close_and_clear();
    }
}

impl PluginManager {
    /// Spawns the single `PluginWorker` OS thread and initializes plugin discovery.
    pub fn spawn(config: PluginEngineConfig) -> Result<(Self, mpsc::Receiver<HostUpdate>)> {
        let (update_tx, update_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown_flag);

        let (queue_tx, queue_rx) = mpsc::sync_channel::<Arc<CmdQueue>>(1);

        let join = thread::Builder::new()
            .name("fern-plugin-worker".to_owned())
            .spawn(move || {
                let wake = Arc::new(WorkerWake {
                    thread: thread::current(),
                    woken: AtomicBool::new(false),
                });
                let queue = Arc::new(CmdQueue {
                    inner: Mutex::new(CmdQueueInner {
                        items: VecDeque::new(),
                        closed: false,
                    }),
                    wake: Arc::clone(&wake),
                });
                let _ = queue_tx.send(Arc::clone(&queue));
                worker_main(config, queue, wake, shutdown_clone, update_tx, ready_tx);
            })
            .context("failed to spawn fern-plugin-worker thread")?;

        let queue = queue_rx
            .recv()
            .map_err(|_| anyhow!("plugin worker failed before queue initialization"))?;
        ready_rx
            .recv()
            .map_err(|_| anyhow!("plugin worker failed during startup"))??;

        Ok((
            Self {
                inner: Arc::new(ManagerInner {
                    queue,
                    shutdown_flag,
                    stopped: AtomicBool::new(false),
                    handle: Mutex::new(Some(join)),
                }),
            },
            update_rx,
        ))
    }

    fn call<T>(&self, build: impl FnOnce(ReplyTx<T>) -> WorkerCmd) -> Result<T> {
        ensure!(
            !self.inner.stopped.load(Ordering::Acquire),
            "plugin manager is shut down"
        );
        let (tx, rx) = mpsc::sync_channel(1);
        ensure!(
            self.inner.queue.push(build(tx)),
            "plugin manager is shut down"
        );
        rx.recv()
            .map_err(|_| anyhow!("plugin worker exited before replying"))?
    }

    pub fn plugins(&self) -> Result<Vec<PluginInfo>> {
        self.call(WorkerCmd::GetPlugins)
    }

    pub fn reload_all(&self) -> Result<Vec<PluginInfo>> {
        self.call(WorkerCmd::ReloadAll)
    }

    pub fn reload_plugin(&self, plugin_id: &str) -> Result<()> {
        let plugin_id = plugin_id.to_owned();
        self.call(|reply| WorkerCmd::ReloadPlugin { plugin_id, reply })
    }

    pub fn set_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> Result<()> {
        let plugin_id = plugin_id.to_owned();
        self.call(|reply| WorkerCmd::SetEnabled {
            plugin_id,
            enabled,
            reply,
        })
    }

    pub fn approve_permissions(&self, plugin_id: &str) -> Result<()> {
        let plugin_id = plugin_id.to_owned();
        self.call(|reply| WorkerCmd::ApprovePermissions { plugin_id, reply })
    }

    pub fn reset_and_enable_plugin(&self, plugin_id: &str) -> Result<()> {
        let plugin_id = plugin_id.to_owned();
        self.call(|reply| WorkerCmd::ResetAndEnable { plugin_id, reply })
    }

    pub fn update_settings(
        &self,
        plugin_id: &str,
        settings: Map<String, JsonValue>,
    ) -> Result<()> {
        let plugin_id = plugin_id.to_owned();
        self.call(|reply| WorkerCmd::UpdateSettings {
            plugin_id,
            settings,
            reply,
        })
    }

    pub fn set_secret(&self, plugin_id: &str, key: &str, value: &str) -> Result<()> {
        let plugin_id = plugin_id.to_owned();
        let key = key.to_owned();
        let value = Some(value.to_owned());
        self.call(|reply| WorkerCmd::SetSecret {
            plugin_id,
            key,
            value,
            reply,
        })
    }

    pub fn delete_secret(&self, plugin_id: &str, key: &str) -> Result<()> {
        let plugin_id = plugin_id.to_owned();
        let key = key.to_owned();
        self.call(|reply| WorkerCmd::SetSecret {
            plugin_id,
            key,
            value: None,
            reply,
        })
    }

    pub fn plugin_logs(&self, plugin_id: &str) -> Result<Vec<LogEntry>> {
        let plugin_id = plugin_id.to_owned();
        self.call(|reply| WorkerCmd::GetLogs { plugin_id, reply })
    }

    pub fn clear_plugin_logs(&self, plugin_id: &str) -> Result<()> {
        let plugin_id = plugin_id.to_owned();
        self.call(|reply| WorkerCmd::ClearLogs { plugin_id, reply })
    }

    pub fn render_view(&self, plugin_id: &str, view_id: &str) -> Result<UiTreePayload> {
        let plugin_id = plugin_id.to_owned();
        let view_id = view_id.to_owned();
        self.call(|reply| WorkerCmd::RenderView {
            plugin_id,
            view_id,
            mount: false,
            reply,
        })
    }

    pub fn mount_view(&self, plugin_id: &str, view_id: &str) -> Result<UiTreePayload> {
        let plugin_id = plugin_id.to_owned();
        let view_id = view_id.to_owned();
        self.call(|reply| WorkerCmd::RenderView {
            plugin_id,
            view_id,
            mount: true,
            reply,
        })
    }

    pub fn unmount_view(&self, plugin_id: &str, view_id: &str) -> Result<()> {
        let plugin_id = plugin_id.to_owned();
        let view_id = view_id.to_owned();
        self.call(|reply| WorkerCmd::UnmountView {
            plugin_id,
            view_id,
            reply,
        })
    }

    pub fn dispatch_ui_event(
        &self,
        plugin_id: &str,
        event: UiEvent,
    ) -> Result<Option<UiTreePayload>> {
        let plugin_id = plugin_id.to_owned();
        self.call(|reply| WorkerCmd::DispatchUiEvent {
            plugin_id,
            event,
            reply,
        })
    }

    pub fn run_command(
        &self,
        plugin_id: &str,
        command_id: &str,
        arg: Option<String>,
    ) -> Result<JsonValue> {
        let plugin_id = plugin_id.to_owned();
        let command_id = command_id.to_owned();
        self.call(|reply| WorkerCmd::RunCommand {
            plugin_id,
            command_id,
            arg,
            reply,
        })
    }

    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let query = query.to_owned();
        self.call(|reply| WorkerCmd::Search { query, reply })
    }

    pub fn resolve_prompt(&self, request_id: u64, reply_val: PromptReplyValue) -> Result<()> {
        self.call(|reply| WorkerCmd::ResolvePrompt {
            request_id,
            reply_val,
            reply,
        })
    }

    pub fn dispatch_outbox(&self) -> Result<usize> {
        self.call(WorkerCmd::DispatchOutbox)
    }

    pub fn notify_outbox_dirty(&self) {
        if !self.inner.stopped.load(Ordering::Acquire) {
            let (tx, _rx) = mpsc::sync_channel(1);
            let _ = self.inner.queue.push(WorkerCmd::DispatchOutbox(tx));
        }
    }

    pub fn emit_app_ready(&self) -> Result<()> {
        self.call(WorkerCmd::EmitAppReady)
    }

    pub fn run_before_task_create(&self, draft_json: JsonValue) -> Result<HookDecision> {
        self.call(|reply| WorkerCmd::RunHook {
            hook: HookKind::BeforeTaskCreate,
            payload: draft_json,
            reply,
        })
    }

    pub fn run_before_task_complete(&self, task_json: JsonValue) -> Result<HookDecision> {
        self.call(|reply| WorkerCmd::RunHook {
            hook: HookKind::BeforeTaskComplete,
            payload: task_json,
            reply,
        })
    }

    pub fn run_quick_add_transform(&self, raw_text: String) -> Result<HookDecision> {
        self.call(|reply| WorkerCmd::RunHook {
            hook: HookKind::QuickAddTransform,
            payload: JsonValue::String(raw_text),
            reply,
        })
    }

    pub fn run_on_reminder(&self, reminder_json: JsonValue) -> Result<()> {
        let _ = self.call(|reply| WorkerCmd::RunHook {
            hook: HookKind::OnReminder,
            payload: reminder_json,
            reply,
        })?;
        Ok(())
    }

    pub fn test_run_script(
        &self,
        script: UserScript,
        arg: Option<JsonValue>,
    ) -> Result<ScriptRunResult> {
        self.call(|reply| WorkerCmd::TestRunScript { script, arg, reply })
    }

    pub fn set_appearance(&self, locale: Option<String>, theme_appearance: Option<String>) {
        if !self.inner.stopped.load(Ordering::Acquire) {
            let _ = self.inner.queue.push(WorkerCmd::SetAppearance {
                locale,
                theme_appearance,
            });
        }
    }

    pub fn check_hot_reload(&self) -> Result<Vec<String>> {
        self.call(WorkerCmd::CheckHotReload)
    }

    /// Unloads a plugin runtime (if active), wipes its secrets from `SecretsVault`, clears its
    /// persisted `plugin_state`, KV, and entity metadata in SQLite, and emits `RegistryChanged`.
    pub fn purge_plugin_data(&self, plugin_id: &str) -> Result<()> {
        let plugin_id = plugin_id.to_owned();
        self.call(|reply| WorkerCmd::PurgePluginData { plugin_id, reply })
    }

    /// Cleanly interrupts running JS, gives active plugins up to 300ms to deactivate, and joins the worker thread.
    pub fn shutdown(&self) {
        self.inner.shutdown_internal();
    }
}

struct ExecGate {
    /// `0` = disarmed, `1` = immediate interrupt, `>= 2` = `(epoch_ms + 2)` deadline.
    deadline_ms: AtomicU64,
    deactivating: AtomicBool,
    interrupted: AtomicBool,
    global_shutdown: Arc<AtomicBool>,
    runtime_cancel: Arc<AtomicBool>,
    epoch: Instant,
}

impl ExecGate {
    fn new(
        epoch: Instant,
        global_shutdown: Arc<AtomicBool>,
        runtime_cancel: Arc<AtomicBool>,
    ) -> Arc<Self> {
        Arc::new(Self {
            deadline_ms: AtomicU64::new(0),
            deactivating: AtomicBool::new(false),
            interrupted: AtomicBool::new(false),
            global_shutdown,
            runtime_cancel,
            epoch,
        })
    }

    fn now_tick(&self) -> u64 {
        (self.epoch.elapsed().as_millis() as u64).saturating_add(2)
    }

    fn arm(&self, budget: Duration) {
        self.interrupted.store(false, Ordering::Release);
        self.deactivating.store(false, Ordering::Release);
        let target = self
            .now_tick()
            .saturating_add(budget.as_millis().max(1) as u64);
        self.deadline_ms.store(target.max(2), Ordering::Release);
    }

    fn arm_immediate(&self) {
        self.interrupted.store(false, Ordering::Release);
        self.deactivating.store(false, Ordering::Release);
        self.deadline_ms.store(1, Ordering::Release);
    }

    /// Arms a bounded deadline specifically for `deactivate()` during shutdown or unload.
    /// Ignores `global_shutdown` and `runtime_cancel` so `deactivate()` can execute cleanly
    /// within the remaining shutdown budget.
    fn arm_deactivate(&self, budget: Duration) {
        self.interrupted.store(false, Ordering::Release);
        let target = self
            .now_tick()
            .saturating_add(budget.as_millis().max(1) as u64);
        self.deadline_ms.store(target.max(2), Ordering::Release);
        self.deactivating.store(true, Ordering::Release);
    }

    fn disarm(&self) {
        self.deactivating.store(false, Ordering::Release);
        self.deadline_ms.store(0, Ordering::Release);
    }

    fn was_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Acquire)
    }

    fn should_interrupt(&self) -> bool {
        if !self.deactivating.load(Ordering::Acquire)
            && (self.global_shutdown.load(Ordering::Acquire)
                || self.runtime_cancel.load(Ordering::Acquire))
        {
            self.interrupted.store(true, Ordering::Release);
            return true;
        }
        let dl = self.deadline_ms.load(Ordering::Acquire);
        if dl == 0 {
            return false;
        }
        if dl == 1 || self.now_tick() >= dl {
            self.interrupted.store(true, Ordering::Release);
            return true;
        }
        false
    }
}

/// Coordinates a persistent per-`LoadedRuntime` job driver task in `env.tasks`.
struct RuntimeDriverState {
    unloaded: Cell<bool>,
    needs_pump: Cell<bool>,
    waker: RefCell<Option<std::task::Waker>>,
}

impl RuntimeDriverState {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            unloaded: Cell::new(false),
            needs_pump: Cell::new(false),
            waker: RefCell::new(None),
        })
    }

    fn wake(&self) {
        self.needs_pump.set(true);
        if let Some(w) = self.waker.borrow_mut().take() {
            w.wake();
        }
    }

    /// Marks the owning `LoadedRuntime` as unloaded and wakes the driver so it drains/cancels
    /// any lingering `Promised` futures and exits once all strong `AsyncRuntime` handles drop.
    fn stop(&self) {
        self.unloaded.set(true);
        self.wake();
    }
}

/// Field declaration order guarantees `module_ns` and `api` (holding `Persistent` handles)
/// are ALWAYS cleared and dropped BEFORE `ctx` and `rt`, preventing `JS_FreeRuntime` aborts / UAF.
struct LoadedRuntime {
    generation: u64,
    cancel_flag: Arc<AtomicBool>,
    driver_state: Rc<RuntimeDriverState>,
    module_ns: Option<Persistent<rquickjs::Object<'static>>>,
    api: Rc<PluginApiState>,
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

struct ClearPersistentGuard {
    api: Rc<PluginApiState>,
    cancel_flag: Option<Arc<AtomicBool>>,
    prompt_cleanup: Option<(PromptSenders, String)>,
    armed: bool,
}

impl ClearPersistentGuard {
    fn new(
        api: Rc<PluginApiState>,
        cancel_flag: Option<Arc<AtomicBool>>,
        prompt_cleanup: Option<(PromptSenders, String)>,
    ) -> Self {
        Self {
            api,
            cancel_flag,
            prompt_cleanup,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ClearPersistentGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Some(flag) = &self.cancel_flag {
                flag.store(true, Ordering::Release);
            }
            if let Some((senders, owner_id)) = &self.prompt_cleanup {
                senders.borrow_mut().retain(|_, (owner, _)| owner != owner_id);
            }
            self.api.clear_persistent_handles();
        }
    }
}

struct PluginSlot {
    loaded_manifest: LoadedManifest,
    enabled: bool,
    status: PluginStatus,
    metrics: PluginMetrics,
    settings_values: Map<String, JsonValue>,
    logs_cache: VecDeque<LogEntry>,
    runtime: Option<LoadedRuntime>,
    snapshot: DirFingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DirFingerprint {
    files: BTreeMap<PathBuf, (Option<SystemTime>, u64)>,
}

impl DirFingerprint {
    fn scan(dir: &Path) -> Self {
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

struct WorkerEnv {
    config: PluginEngineConfig,
    store: Rc<Store>,
    vault: SecretsVault,
    http_pool: Arc<HttpPool>,
    oauth_limiter: Arc<OAuthConcurrencyLimiter>,
    locale: Rc<RefCell<String>>,
    theme_appearance: Rc<RefCell<String>>,
    prompt_senders: PromptSenders,
    next_prompt_id: Rc<Cell<u64>>,
    service_providers: Rc<RefCell<HashMap<String, String>>>,
    slots: Rc<RefCell<BTreeMap<String, PluginSlot>>>,
    next_runtime_generation: Cell<u64>,
    activating: ActivationWaitersMap,
    outbox_draining: Cell<bool>,
    outbox_dirty: Cell<bool>,
    outbox_waiters: RefCell<Vec<ReplyTx<usize>>>,
    tasks: Rc<RefCell<FuturesUnordered<LocalTask>>>,
    pending_tasks: Rc<RefCell<Vec<LocalTask>>>,
    shutdown_flag: Arc<AtomicBool>,
    worker_epoch: Instant,
    update_tx: mpsc::Sender<HostUpdate>,
    wake_callback: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl WorkerEnv {
    fn emit(&self, update: HostUpdate) {
        let _ = self.update_tx.send(update);
        if let Some(cb) = &self.wake_callback {
            cb();
        }
    }

    fn spawn_task(&self, fut: impl std::future::Future<Output = ()> + 'static) {
        self.pending_tasks.borrow_mut().push(Box::pin(fut));
    }

    fn next_generation(&self) -> u64 {
        let rt_gen = self.next_runtime_generation.get().max(1);
        self.next_runtime_generation.set(rt_gen.wrapping_add(1).max(1));
        rt_gen
    }

    fn plugin_infos(&self) -> Vec<PluginInfo> {
        let slots = self.slots.borrow();
        slots
            .values()
            .map(|slot| slot_to_info(slot, &self.vault))
            .collect()
    }

    fn flush_plugin_side_effects(self: &Rc<Self>, plugin_id: &str, expected_gen: Option<u64>) {
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

    fn discover_plugins_into_slots(&self) -> Result<()> {
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

    fn build_slot(&self, loaded: LoadedManifest) -> Result<PluginSlot> {
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

    fn record_failure(&self, plugin_id: &str, expected_gen: Option<u64>, err_msg: &str) {
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

    fn record_success(
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
async fn unload_runtime_async(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    deact_budget: Option<Duration>,
) {
    struct DeactivateGateFuture<Fut> {
        fut: Fut,
        gate: Arc<ExecGate>,
        budget: Duration,
    }
    impl<Fut: std::future::Future> std::future::Future for DeactivateGateFuture<Fut> {
        type Output = Fut::Output;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
        ) -> Poll<Self::Output> {
            // SAFETY: projecting pinned field `fut`; `gate` and `budget` are not pinned.
            let (fut, gate, budget) = unsafe {
                let this = self.get_unchecked_mut();
                (
                    std::pin::Pin::new_unchecked(&mut this.fut),
                    &this.gate,
                    this.budget,
                )
            };
            gate.arm_deactivate(budget);
            let res = fut.poll(cx);
            gate.disarm();
            res
        }
    }

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

fn spawn_startup_activations(env: &Rc<WorkerEnv>) {
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

fn maybe_spawn_startup_activation(env: &Rc<WorkerEnv>, plugin_id: &str) {
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

fn worker_main(
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

fn handle_worker_cmd(env: &Rc<WorkerEnv>, cmd: WorkerCmd) {
    match cmd {
        WorkerCmd::GetPlugins(reply) => {
            let _ = reply.send(Ok(env.plugin_infos()));
        }
        WorkerCmd::ReloadAll(reply) => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = reload_all_plugins_async(&env_c).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::ReloadPlugin { plugin_id, reply } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = reload_single_plugin(&env_c, &plugin_id).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::SetEnabled {
            plugin_id,
            enabled,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = set_plugin_enabled_async(&env_c, &plugin_id, enabled).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::ApprovePermissions { plugin_id, reply } => {
            let res = approve_plugin_permissions(env, &plugin_id);
            let _ = reply.send(res);
        }
        WorkerCmd::ResetAndEnable { plugin_id, reply } => {
            let res = reset_and_enable(env, &plugin_id);
            let _ = reply.send(res);
        }
        WorkerCmd::UpdateSettings {
            plugin_id,
            settings,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = update_plugin_settings(&env_c, &plugin_id, settings).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::SetSecret {
            plugin_id,
            key,
            value,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = set_plugin_secret(&env_c, &plugin_id, &key, value).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::GetLogs { plugin_id, reply } => {
            let slots = env.slots.borrow();
            let logs = match slots.get(&plugin_id) {
                Some(slot) => match &slot.runtime {
                    Some(rt) => rt.api.logs.borrow().iter().cloned().collect(),
                    None => slot.logs_cache.iter().cloned().collect(),
                },
                None => Vec::new(),
            };
            let _ = reply.send(Ok(logs));
        }
        WorkerCmd::ClearLogs { plugin_id, reply } => {
            let mut slots = env.slots.borrow_mut();
            if let Some(slot) = slots.get_mut(&plugin_id) {
                slot.logs_cache.clear();
                if let Some(rt) = &slot.runtime {
                    rt.api.logs.borrow_mut().clear();
                }
            }
            let _ = reply.send(Ok(()));
        }
        WorkerCmd::RenderView {
            plugin_id,
            view_id,
            mount,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = render_plugin_view(&env_c, &plugin_id, &view_id, mount).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::UnmountView {
            plugin_id,
            view_id,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = unmount_plugin_view(&env_c, &plugin_id, &view_id).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::DispatchUiEvent {
            plugin_id,
            event,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = handle_ui_event(&env_c, &plugin_id, event).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::RunCommand {
            plugin_id,
            command_id,
            arg,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = execute_plugin_command(&env_c, &plugin_id, &command_id, arg).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::Search { query, reply } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = run_search_providers(&env_c, &query).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::ResolvePrompt {
            request_id,
            reply_val,
            reply,
        } => {
            if let Some((_owner, tx)) = env.prompt_senders.borrow_mut().remove(&request_id) {
                let _ = tx.send(reply_val);
            }
            let _ = reply.send(Ok(()));
        }
        WorkerCmd::DispatchOutbox(reply) => {
            env.outbox_waiters.borrow_mut().push(reply);
            env.outbox_dirty.set(true);
            schedule_outbox_drain(env);
        }
        WorkerCmd::EmitAppReady(reply) => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = emit_simple_event(&env_c, "app.ready", JsonValue::Null).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::RunHook {
            hook,
            payload,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = run_hooks(&env_c, hook, payload).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::TestRunScript { script, arg, reply } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = run_user_script(&env_c, &script, arg, 0).await;
                let _ = reply.send(Ok(res));
            });
        }
        WorkerCmd::SetAppearance {
            locale,
            theme_appearance,
        } => {
            if let Some(loc) = locale {
                *env.locale.borrow_mut() = loc;
            }
            if let Some(theme) = theme_appearance {
                *env.theme_appearance.borrow_mut() = theme;
            }
        }
        WorkerCmd::CheckHotReload(reply) => {
            let _ = reply.send(perform_hot_reload(env));
        }
        WorkerCmd::PurgePluginData { plugin_id, reply } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = purge_plugin_data_async(&env_c, &plugin_id).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::Shutdown => {}
    }
}

async fn reload_all_plugins_async(env: &Rc<WorkerEnv>) -> Result<Vec<PluginInfo>> {
    let old_ids: Vec<String> = env.slots.borrow().keys().cloned().collect();
    for id in old_ids {
        unload_runtime_async(env, &id, None).await;
    }
    env.slots.borrow_mut().clear();
    env.discover_plugins_into_slots()?;
    spawn_startup_activations(env);
    let infos = env.plugin_infos();
    env.emit(HostUpdate::RegistryChanged(infos.clone()));
    Ok(infos)
}

async fn purge_plugin_data_async(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    let pid = plugin_id.trim();
    ensure!(!pid.is_empty(), "plugin_id must not be empty");
    unload_runtime_async(env, pid, None).await;
    env.slots.borrow_mut().remove(pid);
    env.activating.borrow_mut().remove(pid);
    env.vault.delete_plugin(pid)?;
    // Note: once `core-fix`'s `Store::delete_plugin_all_data` merges, these 3 calls can be
    // consolidated into a single transaction helper if desired.
    env.store.delete_plugin_state(pid)?;
    env.store.plugin_kv_clear(pid)?;
    env.store.plugin_meta_clear(pid)?;
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn set_plugin_enabled_async(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    enabled: bool,
) -> Result<()> {
    if enabled {
        let (loaded_manifest, current_status) = {
            let slots = env.slots.borrow();
            let slot = slots
                .get(plugin_id)
                .with_context(|| format!("plugin {plugin_id:?} not found"))?;
            (slot.loaded_manifest.clone(), slot.status.clone())
        };
        if matches!(
            current_status,
            PluginStatus::NeedsApproval { .. } | PluginStatus::CircuitBroken { .. }
        ) {
            bail!(
                "plugin {plugin_id} requires permission approval or circuit breaker reset before enabling"
            );
        }
        let state = env.store.plugin_state(plugin_id)?;
        match loaded_manifest
            .manifest
            .check_permission_diff(&loaded_manifest.manifest_hash, state.as_ref())
        {
            PermissionApprovalStatus::NeedsApproval {
                diff,
                is_first_install,
            } => {
                if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
                    slot.enabled = false;
                    slot.status = PluginStatus::NeedsApproval {
                        added_permissions: diff.added,
                        is_first_install,
                    };
                }
                env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
                bail!("plugin {plugin_id} requires permission approval before enabling");
            }
            PermissionApprovalStatus::Approved => {
                if let Some(narrowed) = loaded_manifest
                    .manifest
                    .narrowed_permissions_to_persist(&loaded_manifest.manifest_hash, state.as_ref())
                {
                    let _ = env.store.set_plugin_permissions(
                        plugin_id,
                        &narrowed,
                        &loaded_manifest.manifest_hash,
                    );
                }
            }
        }
        if state
            .as_ref()
            .is_some_and(|s| s.consecutive_errors >= CIRCUIT_BREAKER_THRESHOLD)
        {
            bail!("plugin {plugin_id} requires circuit breaker reset before enabling");
        }
    } else {
        let exists = env.slots.borrow().contains_key(plugin_id);
        ensure!(exists, "plugin {plugin_id:?} not found");
    }

    env.store.set_plugin_enabled(plugin_id, enabled)?;
    if !enabled {
        unload_runtime_async(env, plugin_id, Some(Duration::from_millis(100))).await;
    }
    if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
        slot.enabled = enabled;
        if !enabled {
            slot.status = PluginStatus::Disabled;
        } else if slot.runtime.is_none() {
            slot.status = PluginStatus::Idle;
        }
    }
    if enabled {
        maybe_spawn_startup_activation(env, plugin_id);
    }
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

fn approve_plugin_permissions(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    let (perms_json, hash) = {
        let slots = env.slots.borrow();
        let slot = slots
            .get(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        (
            slot.loaded_manifest.permissions.to_json_string()?,
            slot.loaded_manifest.manifest_hash.clone(),
        )
    };
    env.store
        .set_plugin_permissions(plugin_id, &perms_json, &hash)?;
    env.store.reset_plugin_errors(plugin_id)?;
    env.store.set_plugin_enabled(plugin_id, true)?;
    if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
        slot.enabled = true;
        slot.metrics.consecutive_errors = 0;
        if slot.runtime.is_none() {
            slot.status = PluginStatus::Idle;
        }
    }
    maybe_spawn_startup_activation(env, plugin_id);
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

fn reset_and_enable(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    let loaded_manifest = {
        let slots = env.slots.borrow();
        let slot = slots
            .get(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        if matches!(slot.status, PluginStatus::NeedsApproval { .. }) {
            bail!("plugin {plugin_id} requires permission approval first");
        }
        slot.loaded_manifest.clone()
    };

    // Re-verify persisted permission diff before enabling so a circuit-broken plugin whose
    // manifest changed on disk cannot escalate permissions via Reset & Enable.
    let state = env.store.plugin_state(plugin_id)?;
    match loaded_manifest
        .manifest
        .check_permission_diff(&loaded_manifest.manifest_hash, state.as_ref())
    {
        PermissionApprovalStatus::NeedsApproval {
            diff,
            is_first_install,
        } => {
            if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
                slot.enabled = false;
                slot.status = PluginStatus::NeedsApproval {
                    added_permissions: diff.added,
                    is_first_install,
                };
            }
            env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
            bail!("plugin {plugin_id} requires permission approval first");
        }
        PermissionApprovalStatus::Approved => {
            if let Some(narrowed) = loaded_manifest
                .manifest
                .narrowed_permissions_to_persist(&loaded_manifest.manifest_hash, state.as_ref())
            {
                let _ = env.store.set_plugin_permissions(
                    plugin_id,
                    &narrowed,
                    &loaded_manifest.manifest_hash,
                );
            }
        }
    }

    env.store.reset_plugin_errors(plugin_id)?;
    env.store.set_plugin_enabled(plugin_id, true)?;
    if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
        slot.enabled = true;
        slot.metrics.consecutive_errors = 0;
        slot.status = PluginStatus::Idle;
    }
    maybe_spawn_startup_activation(env, plugin_id);
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn reload_single_plugin(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    ensure!(
        validate_plugin_id(plugin_id),
        "invalid plugin id {plugin_id:?}"
    );
    let dir = {
        let slots = env.slots.borrow();
        slots
            .get(plugin_id)
            .map(|s| s.loaded_manifest.dir.clone())
            .unwrap_or_else(|| env.config.plugins_dir.join(plugin_id))
    };
    unload_runtime_async(env, plugin_id, Some(Duration::from_millis(100))).await;

    let loaded_res = LoadedManifest::load_from_dir(&dir).and_then(|loaded| {
        ensure!(
            loaded.manifest.id == plugin_id,
            "manifest id {:?} does not match plugin directory {:?}",
            loaded.manifest.id,
            plugin_id
        );
        Ok(loaded)
    });

    let loaded = match loaded_res {
        Ok(loaded) => loaded,
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            let next_snapshot = DirFingerprint::scan(&dir);
            let updated_status = {
                let mut slots = env.slots.borrow_mut();
                if let Some(slot) = slots.get_mut(plugin_id) {
                    slot.snapshot = next_snapshot;
                    slot.status = PluginStatus::Error(msg.clone());
                    Some((slot.status.clone(), slot.metrics.clone()))
                } else {
                    None
                }
            };
            if let Some((status, metrics)) = updated_status {
                env.emit(HostUpdate::PluginStatusChanged {
                    plugin_id: plugin_id.to_owned(),
                    status,
                    metrics,
                });
                env.emit(HostUpdate::ViewError {
                    plugin_id: plugin_id.to_owned(),
                    view_id: String::new(),
                    error: msg.clone(),
                });
                env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
            }
            return Err(anyhow!("{msg}"));
        }
    };

    let prev_logs = env
        .slots
        .borrow()
        .get(plugin_id)
        .map(|s| s.logs_cache.clone())
        .unwrap_or_default();
    let mut new_slot = env.build_slot(loaded)?;
    new_slot.logs_cache = prev_logs;
    let should_startup = new_slot.enabled
        && (new_slot.loaded_manifest.manifest.matches_activation_event("onStartup")
            || new_slot.loaded_manifest.manifest.matches_activation_event("*"));
    env.slots.borrow_mut().insert(plugin_id.to_owned(), new_slot);
    if should_startup {
        ensure_activated(env, plugin_id).await?;
    }
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn update_plugin_settings(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    settings: Map<String, JsonValue>,
) -> Result<()> {
    let filtered = {
        let slots = env.slots.borrow();
        let slot = slots
            .get(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        let secret_keys: HashSet<&str> = slot
            .loaded_manifest
            .manifest
            .settings
            .iter()
            .filter(|s| s.kind == crate::manifest::SettingKind::Secret)
            .map(|s| s.key.as_str())
            .collect();
        let mut clean = Map::new();
        for (k, v) in settings {
            if !secret_keys.contains(k.as_str()) {
                clean.insert(k, v);
            }
        }
        clean
    };

    let json_text = serde_json::to_string(&filtered)?;
    env.store.set_plugin_settings(plugin_id, &json_text)?;

    let resolved = {
        let mut slots = env.slots.borrow_mut();
        let slot = slots
            .get_mut(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        let next = slot.loaded_manifest.manifest.resolve_settings(&json_text);
        slot.settings_values = next.clone();
        if let Some(rt) = &slot.runtime {
            *rt.api.settings.borrow_mut() = next.clone();
        }
        next
    };

    notify_settings_listeners(env, plugin_id, resolved).await;
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn set_plugin_secret(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    key: &str,
    value: Option<String>,
) -> Result<()> {
    match value {
        Some(v) => env.vault.set(plugin_id, key, &v)?,
        None => {
            env.vault.delete(plugin_id, key)?;
        }
    }
    let resolved = env
        .slots
        .borrow()
        .get(plugin_id)
        .map(|s| s.settings_values.clone())
        .unwrap_or_default();
    notify_settings_listeners(env, plugin_id, resolved).await;
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn notify_settings_listeners(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    resolved: Map<String, JsonValue>,
) {
    // Notify registered `fern.settings.onChange` callbacks if runtime is active.
    let Ok((rt_gen, _rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, plugin_id) else {
        return;
    };
    let api_c = Rc::clone(&api);
    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            let listeners = api_c.settings_listeners.borrow().clone();
            let val = json_to_js(&ctx, &JsonValue::Object(resolved))?;
            for listener in listeners {
                if let Ok(func) = listener.restore(&ctx) {
                    let res: MaybePromise = func.call((val.clone(),))?;
                    let _ = res.into_future::<Value>().await?;
                }
            }
            Ok(())
        },
    )
    .await;
    if let Err(err) = res {
        let scrubbed = env.vault.scrub_secrets(
            plugin_id,
            &format!("settings.onChange listener error: {err:#}"),
        );
        api.push_log(LogLevel::Error, &scrubbed);
        if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id)
            && slot.runtime.as_ref().is_some_and(|r| r.generation == rt_gen)
        {
            slot.logs_cache = api.logs.borrow().clone();
        }
    }
    env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
}

async fn ensure_activated(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
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

async fn render_plugin_view(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    view_id: &str,
    mount: bool,
) -> Result<UiTreePayload> {
    ensure_activated(env, plugin_id).await?;
    let (rt_gen, rt, ctx, gate, api, driver_state) = active_runtime_handles(env, plugin_id)?;
    let start = Instant::now();
    let view_id_own = view_id.to_owned();
    let api_c = Rc::clone(&api);

    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            let (render_fn, mount_fn) = {
                let views = api_c.views.borrow();
                let reg = views.get(&view_id_own).ok_or_else(|| {
                    rquickjs::Error::new_loading_message(&view_id_own, "view is not registered")
                })?;
                (reg.render.clone(), if mount { reg.mount.clone() } else { None })
            };
            if let Some(mount_pers) = mount_fn {
                let f = mount_pers.restore(&ctx)?;
                let r: MaybePromise = f.call(())?;
                let _ = r.into_future::<Value>().await?;
            }
            api_c.clear_scope_handlers(&view_id_own);
            let raw_node: MaybePromise = {
                let _scope_guard = RenderScopeGuard::enter(Rc::clone(&api_c), view_id_own.clone());
                let f = render_fn.restore(&ctx)?;
                f.call(())?
            };
            let val = raw_node.into_future::<Value>().await?;
            let parsed = {
                let _scope_guard = RenderScopeGuard::enter(Rc::clone(&api_c), view_id_own.clone());
                js_value_to_ui_node(&ctx, val, &api_c, &view_id_own)
            };
            match parsed {
                Ok(root) => Ok(UiTreePayload {
                    plugin_id: api_c.plugin_id.clone(),
                    view_id: view_id_own,
                    root,
                }),
                Err(err) => Err(rquickjs::Exception::throw_message(&ctx, &format!("{err:#}"))),
            }
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    let gen_current = env
        .slots
        .borrow()
        .get(plugin_id)
        .and_then(|s| s.runtime.as_ref())
        .is_some_and(|r| r.generation == rt_gen);
    if !gen_current {
        bail!("plugin {plugin_id:?} was unloaded while rendering view {view_id:?}");
    }

    match res {
        Ok(payload) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
            env.emit(HostUpdate::ViewRendered(payload.clone()));
            Ok(payload)
        }
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            env.record_failure(plugin_id, Some(rt_gen), &msg);
            env.emit(HostUpdate::ViewError {
                plugin_id: plugin_id.to_owned(),
                view_id: view_id.to_owned(),
                error: msg.clone(),
            });
            Err(anyhow!("{msg}"))
        }
    }
}

async fn render_modal_view(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    expected_gen: u64,
) -> Result<Option<UiTreePayload>> {
    let Ok((rt_gen, rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, plugin_id) else {
        return Ok(None);
    };
    if rt_gen != expected_gen {
        return Ok(None);
    }
    let modal_info = {
        let guard = api.modal_view.borrow();
        guard
            .as_ref()
            .map(|(title, view)| (title.clone(), view.render.clone()))
    };
    let Some((title, render_fn)) = modal_info else {
        return Ok(None);
    };

    let start = Instant::now();
    let api_c = Rc::clone(&api);
    let modal_scope = "__modal__".to_owned();
    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            api_c.clear_scope_handlers(&modal_scope);
            let raw_node: MaybePromise = {
                let _scope_guard = RenderScopeGuard::enter(Rc::clone(&api_c), modal_scope.clone());
                let f = render_fn.restore(&ctx)?;
                f.call(())?
            };
            let val = raw_node.into_future::<Value>().await?;
            let parsed = {
                let _scope_guard = RenderScopeGuard::enter(Rc::clone(&api_c), modal_scope.clone());
                js_value_to_ui_node(&ctx, val, &api_c, &modal_scope)
            };
            match parsed {
                Ok(root) => Ok(UiTreePayload {
                    plugin_id: api_c.plugin_id.clone(),
                    view_id: modal_scope,
                    root,
                }),
                Err(err) => Err(rquickjs::Exception::throw_message(&ctx, &format!("{err:#}"))),
            }
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    let gen_current = env
        .slots
        .borrow()
        .get(plugin_id)
        .and_then(|s| s.runtime.as_ref())
        .is_some_and(|r| r.generation == rt_gen);
    if !gen_current {
        return Ok(None);
    }

    match res {
        Ok(payload) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
            env.emit(HostUpdate::UiEffect(PluginUiEffect::OpenModal {
                plugin_id: plugin_id.to_owned(),
                view_id: payload.view_id.clone(),
                title,
                tree: payload.root.clone(),
            }));
            Ok(Some(payload))
        }
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            env.record_failure(plugin_id, Some(rt_gen), &msg);
            Err(anyhow!("{msg}"))
        }
    }
}

async fn unmount_plugin_view(env: &Rc<WorkerEnv>, plugin_id: &str, view_id: &str) -> Result<()> {
    let Ok((rt_gen, _rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, plugin_id) else {
        return Ok(());
    };
    let view_id_own = view_id.to_owned();
    let api_c = Rc::clone(&api);
    let _ = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            let unmount = if view_id_own == "__modal__" {
                api_c
                    .modal_view
                    .borrow_mut()
                    .take()
                    .and_then(|(_, v)| v.unmount)
            } else {
                api_c
                    .views
                    .borrow()
                    .get(&view_id_own)
                    .and_then(|v| v.unmount.clone())
            };
            if let Some(unmount_pers) = unmount {
                let f = unmount_pers.restore(&ctx)?;
                let r: MaybePromise = f.call(())?;
                let _ = r.into_future::<Value>().await?;
            }
            api_c.clear_scope_handlers(&view_id_own);
            Ok(())
        },
    )
    .await;
    env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
    Ok(())
}

async fn handle_ui_event(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    event: UiEvent,
) -> Result<Option<UiTreePayload>> {
    ensure_activated(env, plugin_id).await?;
    let (rt_gen, rt, ctx, gate, api, driver_state) = active_runtime_handles(env, plugin_id)?;
    let start = Instant::now();
    let scope = event.view_id.clone();
    let hid = event.handler_id.0;
    let payload = event.payload.clone();
    let api_c = Rc::clone(&api);

    let call_res = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            let handler = {
                let map = api_c.handlers.borrow();
                map.get(&scope).and_then(|s| s.get(&hid)).cloned()
            };
            let Some(handler_pers) = handler else {
                return Ok(false);
            };
            let f = handler_pers.restore(&ctx)?;
            let arg = json_to_js(&ctx, &payload)?;
            let res: MaybePromise = if payload.is_null() {
                f.call(())?
            } else {
                f.call((arg,))?
            };
            let _ = res.into_future::<Value>().await?;
            Ok(true)
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    let gen_current = env
        .slots
        .borrow()
        .get(plugin_id)
        .and_then(|s| s.runtime.as_ref())
        .is_some_and(|r| r.generation == rt_gen);
    if !gen_current {
        return Ok(None);
    }

    match call_res {
        Ok(_ran) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
            if event.view_id == "__modal__" && api.modal_view.borrow().is_some() {
                return render_modal_view(env, plugin_id, rt_gen).await;
            }
            let has_view = api.views.borrow().contains_key(&event.view_id);
            if has_view {
                let tree = render_plugin_view(env, plugin_id, &event.view_id, false).await?;
                Ok(Some(tree))
            } else {
                Ok(None)
            }
        }
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            env.record_failure(plugin_id, Some(rt_gen), &msg);
            Err(anyhow!("{msg}"))
        }
    }
}

async fn execute_plugin_command(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    command_id: &str,
    arg: Option<String>,
) -> Result<JsonValue> {
    ensure_activated(env, plugin_id).await?;
    let (rt_gen, rt, ctx, gate, api, driver_state) = active_runtime_handles(env, plugin_id)?;
    let start = Instant::now();
    let cmd_id = command_id.to_owned();
    let api_c = Rc::clone(&api);

    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_COMMAND_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            let handler = api_c.commands.borrow().get(&cmd_id).cloned().ok_or_else(|| {
                rquickjs::Exception::throw_message(&ctx, &format!("command {cmd_id:?} not registered"))
            })?;
            let f = handler.restore(&ctx)?;
            let raw: MaybePromise = match arg {
                Some(a) => f.call((a,))?,
                None => f.call(())?,
            };
            let val = raw.into_future::<Value>().await?;
            js_to_json(&ctx, val)
                .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("{e:#}")))
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    match res {
        Ok(val) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
            Ok(val)
        }
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            env.record_failure(plugin_id, Some(rt_gen), &msg);
            Err(anyhow!("{msg}"))
        }
    }
}

async fn run_search_providers(env: &Rc<WorkerEnv>, query: &str) -> Result<Vec<SearchHit>> {
    if env.config.safe_mode {
        return Ok(Vec::new());
    }
    let candidate_ids: Vec<String> = env
        .slots
        .borrow()
        .values()
        .filter(|s| {
            s.enabled
                && !s
                    .loaded_manifest
                    .manifest
                    .contributions
                    .search_providers
                    .is_empty()
        })
        .map(|s| s.loaded_manifest.manifest.id.clone())
        .collect();

    let mut all_hits = Vec::new();
    for pid in candidate_ids {
        if ensure_activated(env, &pid).await.is_err() {
            continue;
        }
        let Ok((rt_gen, rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, &pid) else {
            continue;
        };
        let q = query.to_owned();
        let api_c = Rc::clone(&api);
        let start = Instant::now();
        let res = run_with_gate(
            &ctx,
            &gate,
            GateOptions::for_runtime(DEFAULT_COMMAND_TIMEOUT, driver_state, Rc::clone(&api)),
            async move |ctx| {
                let providers: Vec<(String, Persistent<Function<'static>>)> = api_c
                    .search_providers
                    .borrow()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let mut hits = Vec::new();
                for (provider_id, func_pers) in providers {
                    let func = func_pers.restore(&ctx)?;
                    let res: MaybePromise = func.call((q.as_str(),))?;
                    let val = res.into_future::<Value>().await?;
                    let json = js_to_json(&ctx, val)
                        .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("{e:#}")))?;
                    if let Some(arr) = json.as_array() {
                        for item in arr {
                            let Some(title) = item.get("title").and_then(JsonValue::as_str) else {
                                continue;
                            };
                            let id = item
                                .get("id")
                                .and_then(JsonValue::as_str)
                                .map(str::to_owned)
                                .unwrap_or_else(|| Uuid::now_v7().to_string());
                            let task_id = item
                                .get("taskId")
                                .or_else(|| item.get("task_id"))
                                .and_then(JsonValue::as_str)
                                .and_then(|s| Uuid::parse_str(s).ok());
                            hits.push(SearchHit {
                                id,
                                plugin_id: api_c.plugin_id.clone(),
                                provider_id: provider_id.clone(),
                                title: title.to_owned(),
                                subtitle: item
                                    .get("subtitle")
                                    .and_then(JsonValue::as_str)
                                    .map(str::to_owned),
                                badge: item
                                    .get("badge")
                                    .and_then(JsonValue::as_str)
                                    .map(str::to_owned),
                                task_id,
                                command_id: item
                                    .get("commandId")
                                    .or_else(|| item.get("command_id"))
                                    .and_then(JsonValue::as_str)
                                    .map(str::to_owned),
                                command_arg: item
                                    .get("commandArg")
                                    .or_else(|| item.get("command_arg"))
                                    .and_then(JsonValue::as_str)
                                    .map(str::to_owned),
                            });
                        }
                    }
                }
                Ok(hits)
            },
        )
        .await;

        let elapsed = start.elapsed().as_millis() as u64;
        match res {
            Ok(mut hits) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, Some(rt_gen), elapsed, mem);
                all_hits.append(&mut hits);
            }
            Err(err) => {
                env.record_failure(&pid, Some(rt_gen), &format!("{err:#}"));
            }
        }
    }
    Ok(all_hits)
}

async fn invoke_service(
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

fn schedule_outbox_drain(env: &Rc<WorkerEnv>) {
    if env.outbox_draining.replace(true) {
        return;
    }
    let env_c = Rc::clone(env);
    env.spawn_task(async move {
        struct DrainGuard(Rc<WorkerEnv>);
        impl Drop for DrainGuard {
            fn drop(&mut self) {
                self.0.outbox_draining.set(false);
                for reply in self.0.outbox_waiters.borrow_mut().drain(..) {
                    let _ = reply.send(Err(anyhow!("outbox drain cancelled")));
                }
            }
        }
        let _guard = DrainGuard(Rc::clone(&env_c));
        let mut total_delivered = 0usize;
        let mut last_err: Option<anyhow::Error> = None;

        loop {
            env_c.outbox_dirty.set(false);
            match dispatch_outbox_batch(&env_c).await {
                Ok((batch_events, batch_delivered)) => {
                    total_delivered += batch_delivered;
                    if batch_events >= OUTBOX_BATCH_SIZE || env_c.outbox_dirty.get() {
                        continue;
                    }
                    break;
                }
                Err(err) => {
                    last_err = Some(err);
                    break;
                }
            }
        }

        let waiters: Vec<ReplyTx<usize>> = env_c.outbox_waiters.borrow_mut().drain(..).collect();
        env_c.outbox_draining.set(false);
        match last_err {
            Some(err) => {
                let msg = format!("{err:#}");
                for reply in waiters {
                    let _ = reply.send(Err(anyhow!("{msg}")));
                }
            }
            None => {
                for reply in waiters {
                    let _ = reply.send(Ok(total_delivered));
                }
            }
        }
        if env_c.outbox_dirty.get() || !env_c.outbox_waiters.borrow().is_empty() {
            schedule_outbox_drain(&env_c);
        }
    });
}

async fn dispatch_outbox_batch(env: &Rc<WorkerEnv>) -> Result<(usize, usize)> {
    if env.config.safe_mode {
        return Ok((0, 0));
    }
    let events = env.store.drain_plugin_outbox(OUTBOX_BATCH_SIZE)?;
    let event_count = events.len();
    if events.is_empty() {
        return Ok((0, 0));
    }
    let last_seq = events.last().map(|e| e.seq).unwrap_or(0);
    let mut delivered = 0usize;

    let user_scripts = env.store.user_scripts().unwrap_or_default();

    for ev in events {
        if ev.depth >= MAX_EVENT_DEPTH {
            eprintln!(
                "plugin_outbox stopping causal event {:?} at depth {} (>= MAX_EVENT_DEPTH {})",
                ev.event_kind, ev.depth, MAX_EVENT_DEPTH
            );
            continue;
        }
        let next_depth = ev.depth.saturating_add(1);
        let required_perm = event_permission(&ev.event_kind);
        let entity_json = load_outbox_entity_json(&env.store, &ev.event_kind, &ev.entity_id);
        let payload = json!({
            "kind": ev.event_kind,
            "entityId": ev.entity_id,
            "originPlugin": ev.origin_plugin,
            "depth": ev.depth,
            "at": ev.at,
            "entity": entity_json,
        });

        let activation_token = format!("onEvent:{}", ev.event_kind);
        let candidate_ids: Vec<String> = env
            .slots
            .borrow()
            .values()
            .filter(|s| {
                s.enabled
                    && required_perm
                        .as_ref()
                        .is_none_or(|p| s.loaded_manifest.permissions.has(p))
                    && ev.origin_plugin.as_deref() != Some(s.loaded_manifest.manifest.id.as_str())
                    && (s.runtime.is_some()
                        || s.loaded_manifest
                            .manifest
                            .matches_activation_event(&activation_token))
            })
            .map(|s| s.loaded_manifest.manifest.id.clone())
            .collect();

        for pid in candidate_ids {
            if ensure_activated(env, &pid).await.is_err() {
                continue;
            }
            if deliver_event_to_plugin(env, &pid, &ev.event_kind, payload.clone(), next_depth)
                .await
            {
                delivered += 1;
            }
        }

        // Trigger matching enabled user scripts (`trigger_kind == "event"`), enforcing event permissions.
        for script in user_scripts.iter().filter(|s| {
            s.enabled
                && s.trigger_kind == "event"
                && (s.trigger_arg == ev.event_kind || s.trigger_arg == "*")
        }) {
            let script_origin = format!("script:{}", script.id);
            if ev.origin_plugin.as_deref() == Some(script_origin.as_str()) {
                continue;
            }
            let script_perms =
                PermissionSet::from_json_str(&script.permissions).unwrap_or_default();
            if let Some(req_perm) = &required_perm
                && !script_perms.has(req_perm)
            {
                continue;
            }
            let _ = run_user_script(env, script, Some(payload.clone()), next_depth).await;
            delivered += 1;
        }
    }

    env.store.ack_plugin_outbox(last_seq)?;
    Ok((event_count, delivered))
}

async fn emit_simple_event(env: &Rc<WorkerEnv>, kind: &str, payload: JsonValue) -> Result<()> {
    if env.config.safe_mode {
        return Ok(());
    }
    let activation_token = format!("onEvent:{kind}");
    let required_perm = event_permission(kind);
    let candidate_ids: Vec<String> = env
        .slots
        .borrow()
        .values()
        .filter(|s| {
            s.enabled
                && required_perm
                    .as_ref()
                    .is_none_or(|p| s.loaded_manifest.permissions.has(p))
                && (s.runtime.is_some()
                    || s.loaded_manifest
                        .manifest
                        .matches_activation_event(&activation_token))
        })
        .map(|s| s.loaded_manifest.manifest.id.clone())
        .collect();
    for pid in candidate_ids {
        if ensure_activated(env, &pid).await.is_ok() {
            let _ = deliver_event_to_plugin(env, &pid, kind, payload.clone(), 0).await;
        }
    }
    Ok(())
}

async fn deliver_event_to_plugin(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    event_kind: &str,
    payload: JsonValue,
    write_depth: u32,
) -> bool {
    let Ok((rt_gen, rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, plugin_id) else {
        return false;
    };
    // Enforce event capability permission on delivery so `fern.events.on("*")` cannot bypass
    // `tasks:read` / `habits:read` / `focus:read` / etc.
    if let Some(req_perm) = event_permission(event_kind)
        && !api.permissions.has(&req_perm)
    {
        return false;
    }

    let listeners: Vec<Persistent<Function<'static>>> = {
        let map = api.event_listeners.borrow();
        let mut out = Vec::new();
        if let Some(list) = map.get(event_kind) {
            out.extend(list.iter().map(|(_, f)| f.clone()));
        }
        if let Some(prefix) = event_kind.split('.').next()
            && let Some(list) = map.get(&format!("{prefix}.*"))
        {
            out.extend(list.iter().map(|(_, f)| f.clone()));
        }
        if let Some(list) = map.get("*") {
            out.extend(list.iter().map(|(_, f)| f.clone()));
        }
        out
    };
    if listeners.is_empty() {
        return false;
    }

    let start = Instant::now();
    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions {
            budget: DEFAULT_EVENT_TIMEOUT,
            driver_state: Some(driver_state),
            api: Some(Rc::clone(&api)),
            write_depth,
            service_depth: 0,
        },
        async move |ctx| {
            let js_payload = json_to_js(&ctx, &payload)?;
            for func_pers in listeners {
                let func = func_pers.restore(&ctx)?;
                let r: MaybePromise = func.call((js_payload.clone(),))?;
                let _ = r.into_future::<Value>().await?;
            }
            Ok(())
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    match res {
        Ok(()) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
            true
        }
        Err(err) => {
            env.record_failure(plugin_id, Some(rt_gen), &format!("{err:#}"));
            false
        }
    }
}

async fn run_hooks(env: &Rc<WorkerEnv>, hook: HookKind, payload: JsonValue) -> Result<HookDecision> {
    if env.config.safe_mode {
        return Ok(HookDecision::Allow);
    }
    let hook_token = match hook {
        HookKind::BeforeTaskCreate => "onHook:beforeTaskCreate",
        HookKind::BeforeTaskComplete => "onHook:beforeTaskComplete",
        HookKind::QuickAddTransform => "onHook:quickAddTransform",
        HookKind::OnReminder => "onHook:onReminder",
    };

    let candidate_ids: Vec<String> = env
        .slots
        .borrow()
        .values()
        .filter(|s| {
            s.enabled
                && s.loaded_manifest
                    .permissions
                    .has(&Permission::HooksIntercept)
                && (s.runtime.is_some()
                    || s.loaded_manifest
                        .manifest
                        .matches_activation_event(hook_token))
        })
        .map(|s| s.loaded_manifest.manifest.id.clone())
        .collect();

    let mut current = payload;
    let mut modified = false;

    for pid in candidate_ids {
        if ensure_activated(env, &pid).await.is_err() {
            continue;
        }
        let Ok((rt_gen, rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, &pid) else {
            continue;
        };
        let handlers: Vec<Persistent<Function<'static>>> = {
            let hooks = api.hooks.borrow();
            match hook {
                HookKind::BeforeTaskCreate => hooks.before_task_create.clone(),
                HookKind::BeforeTaskComplete => hooks.before_task_complete.clone(),
                HookKind::QuickAddTransform => hooks.quick_add_transform.clone(),
                HookKind::OnReminder => hooks.on_reminder.clone(),
            }
        };
        if handlers.is_empty() {
            continue;
        }

        let arg_payload = current.clone();
        let start = Instant::now();
        let res = run_with_gate(
            &ctx,
            &gate,
            GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
            async move |ctx| {
                let mut cur_val = arg_payload;
                let mut changed = false;
                for func_pers in handlers {
                    let func = func_pers.restore(&ctx)?;
                    let js_arg = json_to_js(&ctx, &cur_val)?;
                    let raw: MaybePromise = func.call((js_arg,))?;
                    let val = raw.into_future::<Value>().await?;
                    if val.is_undefined() || val.is_null() {
                        continue;
                    }
                    let out_json = js_to_json(&ctx, val)
                        .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("{e:#}")))?;
                    if matches!(hook, HookKind::OnReminder) {
                        // Reminders are observational notifications; one plugin's return value
                        // must not cancel or alter delivery to subsequent plugins.
                        continue;
                    }
                    if out_json == JsonValue::Bool(false) {
                        return Ok(HookDecision::Cancel("cancelled by plugin hook".to_owned()));
                    }
                    if let Some(cancel_flag) = out_json.get("cancel").and_then(JsonValue::as_bool) {
                        if cancel_flag {
                            let reason = out_json
                                .get("reason")
                                .and_then(JsonValue::as_str)
                                .unwrap_or("cancelled by plugin hook")
                                .to_owned();
                            return Ok(HookDecision::Cancel(reason));
                        }
                        // `{ cancel: false }` without `draft`/`value` means Allow, not Modify.
                        if let Some(next_val) =
                            out_json.get("draft").or_else(|| out_json.get("value"))
                            && hook_payload_matches(hook, next_val)
                        {
                            cur_val = next_val.clone();
                            changed = true;
                        }
                        continue;
                    }
                    if let Some(next_val) = out_json.get("draft").or_else(|| out_json.get("value"))
                    {
                        if hook_payload_matches(hook, next_val) {
                            cur_val = next_val.clone();
                            changed = true;
                        }
                    } else if hook_payload_matches(hook, &out_json) {
                        cur_val = out_json;
                        changed = true;
                    }
                }
                if changed {
                    Ok(HookDecision::Modify(cur_val))
                } else {
                    Ok(HookDecision::Allow)
                }
            },
        )
        .await;

        let elapsed = start.elapsed().as_millis() as u64;
        match res {
            Ok(HookDecision::Allow) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, Some(rt_gen), elapsed, mem);
                env.flush_plugin_side_effects(&pid, Some(rt_gen));
            }
            Ok(HookDecision::Modify(next)) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, Some(rt_gen), elapsed, mem);
                env.flush_plugin_side_effects(&pid, Some(rt_gen));
                current = next;
                modified = true;
            }
            Ok(HookDecision::Cancel(reason)) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, Some(rt_gen), elapsed, mem);
                env.flush_plugin_side_effects(&pid, Some(rt_gen));
                if !matches!(hook, HookKind::OnReminder) {
                    return Ok(HookDecision::Cancel(reason));
                }
            }
            Err(err) => {
                env.record_failure(&pid, Some(rt_gen), &format!("{err:#}"));
            }
        }
    }

    if modified {
        Ok(HookDecision::Modify(current))
    } else {
        Ok(HookDecision::Allow)
    }
}

fn hook_payload_matches(hook: HookKind, val: &JsonValue) -> bool {
    match hook {
        HookKind::QuickAddTransform => val.is_string(),
        HookKind::BeforeTaskCreate | HookKind::BeforeTaskComplete => val.is_object(),
        HookKind::OnReminder => false,
    }
}

/// Virtual-only module resolver for UserScripts: permits ONLY `"fern"` and `"fern:ui"` and
/// rejects all filesystem/relative imports on every OS (preventing cross-plugin imports).
struct VirtualOnlyResolver;

impl Resolver for VirtualOnlyResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        if name == VIRTUAL_FERN_MODULE || name == VIRTUAL_FERN_UI_MODULE {
            Ok(name.to_owned())
        } else {
            Err(rquickjs::Error::new_resolving_message(
                base,
                name,
                "filesystem imports are forbidden in user scripts; only 'fern' and 'fern:ui' are available",
            ))
        }
    }
}

/// Virtual-only module loader for UserScripts: serves only `"fern"` and `"fern:ui"`.
struct VirtualOnlyLoader(SandboxedLoader);

impl VirtualOnlyLoader {
    fn new() -> Self {
        Self(SandboxedLoader::new(PathBuf::new()))
    }
}

impl Loader for VirtualOnlyLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js>> {
        if name == VIRTUAL_FERN_MODULE || name == VIRTUAL_FERN_UI_MODULE {
            self.0.load(ctx, name, attributes)
        } else {
            Err(rquickjs::Error::new_loading_message(
                name,
                "filesystem imports are forbidden in user scripts",
            ))
        }
    }
}

/// Replaces JS comments and string/template literal interiors with spaces so syntax checks
/// (`looks_like_es_module`, `has_return_or_await`) never misfire on strings or comments.
fn strip_js_comments_and_strings(code: &str) -> String {
    let bytes = code.as_bytes();
    let mut out = String::with_capacity(code.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        // Line comment `// ...`
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            out.push_str("  ");
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        // Block comment `/* ... */`
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            out.push_str("  ");
            i += 2;
            while i < bytes.len() {
                if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    out.push_str("  ");
                    i += 2;
                    break;
                }
                out.push(if bytes[i] == b'\n' { '\n' } else { ' ' });
                i += 1;
            }
            continue;
        }
        // Single/double/template string literals
        if matches!(b, b'\'' | b'"' | b'`') {
            let quote = b;
            out.push(quote as char);
            i += 1;
            while i < bytes.len() {
                let c = bytes[i];
                if c == b'\\' && i + 1 < bytes.len() {
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                if c == quote {
                    out.push(quote as char);
                    i += 1;
                    break;
                }
                out.push(if c == b'\n' { '\n' } else { ' ' });
                i += 1;
            }
            continue;
        }
        let ch = code[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn looks_like_es_module(code: &str) -> bool {
    let stripped = strip_js_comments_and_strings(code);
    for line in stripped.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("import")
            && let Some(first) = rest.chars().next()
            && (first.is_whitespace() || matches!(first, '{' | '*' | '\'' | '"'))
        {
            return true;
        }
        if let Some(rest) = trimmed.strip_prefix("export")
            && let Some(first) = rest.chars().next()
            && (first.is_whitespace() || matches!(first, '{' | '*'))
        {
            return true;
        }
    }
    false
}

fn has_return_or_await(code: &str) -> bool {
    let stripped = strip_js_comments_and_strings(code);
    let bytes = stripped.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    let mut i = 0usize;
    while i < bytes.len() {
        if is_ident(bytes[i]) {
            let start = i;
            while i < bytes.len() && is_ident(bytes[i]) {
                i += 1;
            }
            let word = &stripped[start..i];
            if word == "return" || word == "await" {
                let prev_non_ws = stripped[..start].trim_end().as_bytes().last().copied();
                if prev_non_ws != Some(b'.') {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

async fn run_user_script(
    env: &Rc<WorkerEnv>,
    script: &UserScript,
    arg: Option<JsonValue>,
    write_depth: u32,
) -> ScriptRunResult {
    let start = Instant::now();
    let script_id = format!("script:{}", script.id);
    let perms = PermissionSet::from_json_str(&script.permissions).unwrap_or_default();

    // Declare `rt` and `ctx` BEFORE `api` and `clear_guard` so drop order ALWAYS clears
    // `Persistent` handles before `ctx` and `rt` drop even if this task is cancelled mid-await.
    let Ok(rt) = AsyncRuntime::new() else {
        return ScriptRunResult {
            ok: false,
            return_value: None,
            logs: Vec::new(),
            error: Some("failed to create script runtime".into()),
            duration_ms: 0,
            data_changed: false,
        };
    };
    rt.set_memory_limit(PLUGIN_MEMORY_LIMIT_BYTES).await;
    rt.set_max_stack_size(PLUGIN_STACK_LIMIT_BYTES).await;
    rt.set_loader(VirtualOnlyResolver, VirtualOnlyLoader::new())
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

    let Ok(ctx) = AsyncContext::full(&rt).await else {
        return ScriptRunResult {
            ok: false,
            return_value: None,
            logs: Vec::new(),
            error: Some("failed to create script context".into()),
            duration_ms: 0,
            data_changed: false,
        };
    };

    let env_for_svc = Rc::clone(env);
    let service_caller: ServiceCallerFn = Rc::new(move |svc_name, svc_arg, depth| {
        let (tx, rx) = oneshot::channel();
        let env_call = Rc::clone(&env_for_svc);
        env_for_svc.spawn_task(async move {
            let res = invoke_service(&env_call, &svc_name, svc_arg, depth).await;
            let _ = tx.send(res);
        });
        rx
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
        plugin_id: script_id.clone(),
        permissions: perms,
        store: Rc::clone(&env.store),
        vault: env.vault.clone(),
        http_pool: Arc::clone(&env.http_pool),
        oauth_limiter: Arc::clone(&env.oauth_limiter),
        write_depth: Cell::new(write_depth),
        active_service_depth: Cell::new(0),
        batch_dirty_stack: RefCell::new(Vec::new()),
        data_dirty: Cell::new(false),
        declared_views: HashSet::new(),
        settings: RefCell::new(Map::new()),
        secret_keys: HashSet::new(),
        locale: Rc::clone(&env.locale),
        theme_appearance: Rc::clone(&env.theme_appearance),
        logs: RefCell::new(VecDeque::new()),
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
        service_registrar: Rc::new(|_| {}),
        service_caller,
        view_refresher: Rc::new(|_| {}),
        effect_emitter,
        shutdown_flag: Arc::clone(&env.shutdown_flag),
    });

    let clear_guard = ClearPersistentGuard::new(
        Rc::clone(&api),
        Some(Arc::clone(&cancel_flag)),
        Some((Rc::clone(&env.prompt_senders), script_id.clone())),
    );

    let code = script.code.clone();
    let api_c = Rc::clone(&api);
    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions {
            budget: DEFAULT_COMMAND_TIMEOUT,
            driver_state: None,
            api: Some(Rc::clone(&api)),
            write_depth,
            service_depth: 0,
        },
        async move |ctx| {
            install_fern_bindings(&ctx, Rc::clone(&api_c), true)?;
            let js_arg = match arg {
                Some(a) => json_to_js(&ctx, &a)?,
                None => Value::new_undefined(ctx.clone()),
            };
            let val = if looks_like_es_module(&code) {
                let declared = Module::declare(ctx.clone(), "user_script.js", code)?;
                let (evaluated, promise) = declared.eval()?;
                promise.into_future::<()>().await?;
                let ns = evaluated.namespace()?;
                if let Ok(Some(default_fn)) = ns.get::<_, Option<Function>>("default") {
                    let ret: MaybePromise = default_fn.call((js_arg,))?;
                    ret.into_future::<Value>().await?
                } else {
                    Value::new_undefined(ctx.clone())
                }
            } else if has_return_or_await(&code) {
                ctx.globals().set("__script_arg", js_arg)?;
                let wrapped =
                    format!("(async function(event) {{\n{code}\n}})(globalThis.__script_arg)");
                let mut opts = EvalOptions::default();
                opts.global = true;
                opts.strict = true;
                let promise = ctx.eval_with_options::<MaybePromise, _>(wrapped, opts)?;
                promise.into_future::<Value>().await?
            } else {
                // Always bind `event` even for synchronous expression/statement scripts.
                ctx.globals().set("event", js_arg)?;
                let mut opts = EvalOptions::default();
                opts.global = true;
                opts.strict = true;
                let res: MaybePromise = ctx.eval_with_options(code, opts)?;
                res.into_future::<Value>().await?
            };
            if val.is_undefined() {
                Ok(None)
            } else {
                let json = js_to_json(&ctx, val)
                    .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("{e:#}")))?;
                Ok(Some(json))
            }
        },
    )
    .await;

    let effects: Vec<PluginUiEffect> = api.pending_ui_effects.borrow_mut().drain(..).collect();
    let data_changed = api.data_dirty.get();
    let logs: Vec<LogEntry> = api.logs.borrow().iter().cloned().collect();
    drop(clear_guard);
    drop(api);
    drop(ctx);
    drop(rt);

    for eff in effects {
        env.emit(HostUpdate::UiEffect(eff));
    }
    if data_changed {
        env.emit(HostUpdate::DataChanged);
        if !env.config.safe_mode {
            env.outbox_dirty.set(true);
            schedule_outbox_drain(env);
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    match res {
        Ok(return_value) => ScriptRunResult {
            ok: true,
            return_value,
            logs,
            error: None,
            duration_ms,
            data_changed,
        },
        Err(err) => ScriptRunResult {
            ok: false,
            return_value: None,
            logs,
            error: Some(env.vault.scrub_secrets(&script_id, &format!("{err:#}"))),
            duration_ms,
            data_changed,
        },
    }
}

struct WaitDriverWake {
    state: Rc<RuntimeDriverState>,
    yielded: bool,
}

impl std::future::Future for WaitDriverWake {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<()> {
        if self.state.needs_pump.replace(false) || self.yielded {
            self.state.waker.borrow_mut().take();
            return Poll::Ready(());
        }
        *self.state.waker.borrow_mut() = Some(cx.waker().clone());
        self.yielded = true;
        Poll::Pending
    }
}

/// Persistent per-`LoadedRuntime` gated job driver.
///
/// - Never calls `block_on` (zero deadlock risk on `rt.inner.lock()`).
/// - Always arms `ExecGate` around `execute_pending_job()` and bounds microtask bursts.
/// - Parks with 0% CPU while idle or awaiting external futures (HTTP/OAuth/confirm/services).
/// - Owns a permanent `FuturesUnordered` slot waker so `rquickjs::Schedular`'s single
///   `AtomicWaker` is re-registered whenever any transient `GateFuture` finishes.
async fn runtime_driver_task(
    env: Rc<WorkerEnv>,
    plugin_id: String,
    generation: u64,
    weak_rt: AsyncWeakRuntime,
    gate: Arc<ExecGate>,
    driver_state: Rc<RuntimeDriverState>,
) {
    const MAX_JOBS_PER_BURST: usize = 10_000;

    struct DriverJobFuture<Fut> {
        fut: Fut,
        gate: Arc<ExecGate>,
        budget: Duration,
    }
    impl<Fut: std::future::Future> std::future::Future for DriverJobFuture<Fut> {
        type Output = Fut::Output;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
        ) -> Poll<Self::Output> {
            // SAFETY: projecting pinned field `fut`; `gate` and `budget` are not pinned.
            let (fut, gate, budget) = unsafe {
                let this = self.get_unchecked_mut();
                (
                    std::pin::Pin::new_unchecked(&mut this.fut),
                    &this.gate,
                    this.budget,
                )
            };
            gate.arm(budget);
            let res = fut.poll(cx);
            gate.disarm();
            res
        }
    }

    loop {
        if env.shutdown_flag.load(Ordering::Acquire) {
            break;
        }
        driver_state.needs_pump.set(false);
        // Stay alive until every strong `AsyncRuntime` handle (including any in-flight
        // `GateFuture` unwinding after disable/circuit-break) has dropped, so suspended
        // external futures in `rquickjs::Schedular` never lose their wakeup path.
        let Some(rt) = weak_rt.try_ref() else {
            break;
        };

        let burst_start = Instant::now();
        let mut jobs_ran = 0usize;
        let mut ran_any = false;
        let mut burst_err: Option<String> = None;

        loop {
            if env.shutdown_flag.load(Ordering::Acquire) {
                break;
            }
            let remaining = DEFAULT_EVENT_TIMEOUT.saturating_sub(burst_start.elapsed());
            if remaining.is_zero() || jobs_ran >= MAX_JOBS_PER_BURST {
                if !driver_state.unloaded.get() {
                    burst_err = Some(format!(
                        "plugin async job execution exceeded {}ms budget",
                        DEFAULT_EVENT_TIMEOUT.as_millis()
                    ));
                }
                break;
            }

            let job_res = DriverJobFuture {
                fut: rt.execute_pending_job(),
                gate: Arc::clone(&gate),
                budget: remaining,
            }
            .await;

            if gate.was_interrupted()
                && !driver_state.unloaded.get()
                && !env.shutdown_flag.load(Ordering::Acquire)
            {
                burst_err = Some(format!(
                    "plugin async job execution exceeded {}ms budget",
                    DEFAULT_EVENT_TIMEOUT.as_millis()
                ));
                break;
            }

            match job_res {
                Ok(true) => {
                    ran_any = true;
                    jobs_ran += 1;
                    continue;
                }
                Ok(false) => break,
                Err(err_ctx) => {
                    ran_any = true;
                    if !driver_state.unloaded.get() {
                        let msg: String = err_ctx
                            .0
                            .with(|ctx| format_caught_error(&ctx, rquickjs::Error::Exception))
                            .await;
                        burst_err = Some(msg);
                    }
                    break;
                }
            }
        }

        drop(rt);

        if env.shutdown_flag.load(Ordering::Acquire) {
            break;
        }
        if !driver_state.unloaded.get() {
            if let Some(msg) = burst_err {
                env.record_failure(&plugin_id, Some(generation), &msg);
            } else if ran_any {
                env.flush_plugin_side_effects(&plugin_id, Some(generation));
            }
        } else if weak_rt.try_ref().is_none() {
            break;
        }

        if env.shutdown_flag.load(Ordering::Acquire) {
            break;
        }

        WaitDriverWake {
            state: Rc::clone(&driver_state),
            yielded: false,
        }
        .await;
    }
}

fn fire_due_timers(env: &Rc<WorkerEnv>) {
    let now = Instant::now();
    // Pass only `(plugin_id, timer_id, generation)` — never clone `Persistent<Function>`
    // outside a task holding `rt`/`ctx`, avoiding drop-after-`AsyncRuntime` aborts / UAF.
    let mut due: Vec<(String, String, u64)> = Vec::new();
    {
        let slots = env.slots.borrow();
        for (pid, slot) in slots.iter() {
            let Some(rt) = &slot.runtime else {
                continue;
            };
            let rt_gen = rt.generation;
            for timer in rt.api.timers.borrow_mut().values_mut() {
                if timer.next_fire <= now {
                    let clamped_ms = timer
                        .interval_ms
                        .clamp(MIN_TIMER_INTERVAL_MS, MAX_TIMER_INTERVAL_MS);
                    timer.next_fire = now
                        .checked_add(Duration::from_millis(clamped_ms))
                        .unwrap_or(now);
                    due.push((pid.clone(), timer.id.clone(), rt_gen));
                }
            }
        }
    }
    for (pid, tid, expected_gen) in due {
        let env_c = Rc::clone(env);
        env.spawn_task(async move {
            let Ok((rt_gen, rt, ctx, gate, api, driver_state)) = active_runtime_handles(&env_c, &pid)
            else {
                return;
            };
            if rt_gen != expected_gen {
                return;
            }
            let api_c = Rc::clone(&api);
            let start = Instant::now();
            let res = run_with_gate(
                &ctx,
                &gate,
                GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
                async move |ctx| {
                    let cb = {
                        let timers = api_c.timers.borrow();
                        timers.get(&tid).map(|t| t.callback.clone())
                    };
                    let Some(cb) = cb else {
                        return Ok(());
                    };
                    let func = cb.restore(&ctx)?;
                    let raw: MaybePromise = func.call(())?;
                    let _ = raw.into_future::<Value>().await?;
                    Ok(())
                },
            )
            .await;
            let elapsed = start.elapsed().as_millis() as u64;
            match res {
                Ok(()) => {
                    let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                    env_c.record_success(&pid, Some(rt_gen), elapsed, mem);
                    env_c.flush_plugin_side_effects(&pid, Some(rt_gen));
                }
                Err(err) => {
                    env_c.record_failure(&pid, Some(rt_gen), &format!("{err:#}"));
                }
            }
        });
    }
}

fn perform_hot_reload(env: &Rc<WorkerEnv>) -> Result<Vec<String>> {
    if env.config.safe_mode {
        return Ok(Vec::new());
    }
    let changed_ids: Vec<String> = {
        let slots = env.slots.borrow();
        slots
            .iter()
            .filter_map(|(id, slot)| {
                let current = DirFingerprint::scan(&slot.loaded_manifest.dir);
                (current != slot.snapshot).then_some(id.clone())
            })
            .collect()
    };
    if changed_ids.is_empty() {
        return Ok(Vec::new());
    }
    for pid in &changed_ids {
        let env_c = Rc::clone(env);
        let pid_own = pid.clone();
        env.spawn_task(async move {
            let _ = reload_single_plugin(&env_c, &pid_own).await;
        });
    }
    Ok(changed_ids)
}

type RuntimeHandles = (
    u64,
    AsyncRuntime,
    AsyncContext,
    Arc<ExecGate>,
    Rc<PluginApiState>,
    Rc<RuntimeDriverState>,
);

fn active_runtime_handles(env: &WorkerEnv, plugin_id: &str) -> Result<RuntimeHandles> {
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

struct GateOptions {
    budget: Duration,
    driver_state: Option<Rc<RuntimeDriverState>>,
    api: Option<Rc<PluginApiState>>,
    write_depth: u32,
    service_depth: usize,
}

impl GateOptions {
    fn for_runtime(
        budget: Duration,
        driver_state: Rc<RuntimeDriverState>,
        api: Rc<PluginApiState>,
    ) -> Self {
        Self {
            budget,
            driver_state: Some(driver_state),
            api: Some(api),
            write_depth: 0,
            service_depth: 0,
        }
    }
}

async fn run_with_gate<F, R>(
    ctx: &AsyncContext,
    gate: &Arc<ExecGate>,
    opts: GateOptions,
    f: F,
) -> Result<R>
where
    F: for<'js> AsyncFnOnce(Ctx<'js>) -> rquickjs::Result<R>,
    R: 'static,
{
    struct DepthRestoreGuard {
        api: Option<Rc<PluginApiState>>,
        prev_write: u32,
        prev_service: usize,
    }
    impl Drop for DepthRestoreGuard {
        fn drop(&mut self) {
            if let Some(api) = &self.api {
                api.write_depth.set(self.prev_write);
                api.active_service_depth.set(self.prev_service);
            }
        }
    }

    struct GateFuture<Fut> {
        fut: Fut,
        gate: Arc<ExecGate>,
        budget: Duration,
        cpu_used: Duration,
        driver_state: Option<Rc<RuntimeDriverState>>,
        api: Option<Rc<PluginApiState>>,
        write_depth: u32,
        service_depth: usize,
    }
    impl<Fut> Drop for GateFuture<Fut> {
        fn drop(&mut self) {
            // Wake the persistent per-runtime driver whenever any `GateFuture` completes or
            // is cancelled so it re-registers its permanent waker on `rquickjs::Schedular`
            // and drains any trailing microtasks.
            if let Some(driver) = &self.driver_state {
                driver.wake();
            }
        }
    }
    impl<Fut: std::future::Future> std::future::Future for GateFuture<Fut> {
        type Output = Fut::Output;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> Poll<Self::Output> {
            // SAFETY: projecting pinned field `fut`; all other fields are `Unpin` / not pinned.
            let (fut, gate, budget, cpu_used, api, write_depth, service_depth) = unsafe {
                let this = self.get_unchecked_mut();
                (
                    std::pin::Pin::new_unchecked(&mut this.fut),
                    &this.gate,
                    this.budget,
                    &mut this.cpu_used,
                    this.api.as_ref().map(Rc::clone),
                    this.write_depth,
                    this.service_depth,
                )
            };
            let _depth_guard = if let Some(api_ref) = api {
                let prev_write = api_ref.write_depth.replace(write_depth);
                let prev_service = api_ref.active_service_depth.replace(service_depth);
                DepthRestoreGuard {
                    api: Some(api_ref),
                    prev_write,
                    prev_service,
                }
            } else {
                DepthRestoreGuard {
                    api: None,
                    prev_write: 0,
                    prev_service: 0,
                }
            };
            let remaining = budget.saturating_sub(*cpu_used);
            if remaining.is_zero() {
                gate.arm_immediate();
            } else {
                gate.arm(remaining);
            }
            let poll_start = Instant::now();
            let res = fut.poll(cx);
            *cpu_used = cpu_used.saturating_add(poll_start.elapsed());
            gate.disarm();
            res
        }
    }

    let inner = ctx.async_with(async move |ctx| match f(ctx.clone()).await {
        Ok(v) => Ok(v),
        Err(err) => Err(format_caught_error(&ctx, err)),
    });
    GateFuture {
        fut: inner,
        gate: Arc::clone(gate),
        budget: opts.budget,
        cpu_used: Duration::ZERO,
        driver_state: opts.driver_state,
        api: opts.api,
        write_depth: opts.write_depth,
        service_depth: opts.service_depth,
    }
    .await
    .map_err(|msg| anyhow!("{msg}"))
}

fn format_caught_error<'js>(ctx: &Ctx<'js>, err: rquickjs::Error) -> String {
    match CaughtError::from_error(ctx, err) {
        CaughtError::Error(e) => format!("{e}"),
        CaughtError::Exception(ex) => {
            let msg = ex.message().unwrap_or_else(|| "JS Exception".to_owned());
            match ex.stack().filter(|s| !s.trim().is_empty()) {
                Some(stack) => format!("{msg}\n{stack}"),
                None => msg,
            }
        }
        CaughtError::Value(val) => {
            if let Some(s) = val.as_string().and_then(|s| s.to_string().ok()) {
                return s;
            }
            if val.type_of() == rquickjs::Type::Unknown {
                return "Internal plugin host error while executing JS callback".to_owned();
            }
            if let Some(obj) = val.as_object()
                && let Ok(msg) = obj.get::<_, String>("message")
                && !msg.trim().is_empty()
            {
                return msg;
            }
            if matches!(val.type_of(), rquickjs::Type::Object | rquickjs::Type::Array)
                && let Ok(Some(json_str)) = ctx.json_stringify(val.clone())
                && let Ok(s) = json_str.to_string()
            {
                return s;
            }
            format!("{val:?}")
        }
    }
}

fn load_outbox_entity_json(store: &Store, event_kind: &str, entity_id: &str) -> JsonValue {
    if event_kind.starts_with("task.")
        && let Ok(id) = Uuid::parse_str(entity_id)
        && let Ok(Some(task)) = store.task(id)
    {
        return task_to_json(&task);
    }
    if event_kind == "habit.checkin"
        && let Some((habit_id_str, date_str)) = entity_id.split_once('\x1f')
    {
        let habit_obj = Uuid::parse_str(habit_id_str)
            .ok()
            .and_then(|hid| {
                store
                    .habits()
                    .ok()
                    .and_then(|list| list.into_iter().find(|h| h.id == hid))
            })
            .map(|h| {
                json!({
                    "id": h.id.to_string(),
                    "name": h.name,
                    "archived": h.archived,
                    "section": h.section.key(),
                })
            });
        return json!({
            "habitId": habit_id_str,
            "date": date_str,
            "habit": habit_obj,
        });
    }
    if event_kind == "focus.completed"
        && let Ok(id) = Uuid::parse_str(entity_id)
        && let Ok(sessions) = store.focus_sessions()
        && let Some(s) = sessions.into_iter().find(|s| s.id == id)
    {
        return json!({
            "id": s.id.to_string(),
            "taskId": s.task_id.map(|i| i.to_string()),
            "habitId": s.habit_id.map(|i| i.to_string()),
            "title": s.title,
            "startedAt": s.started_at,
            "endedAt": s.ended_at,
            "minutes": s.minutes(),
            "stopwatch": s.stopwatch,
            "note": s.note,
        });
    }
    JsonValue::Null
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use fern_core::plugin_store::UserScript;
    use tempfile::TempDir;

    use super::*;
    use crate::scaffold::generate_sample_plugin;
    use crate::ui_ast::{HandlerId, UiNode};

    struct TestEnv {
        _tmp: TempDir,
        plugins_dir: PathBuf,
        db_path: PathBuf,
        store: Store,
        vault_path: PathBuf,
        key_path: PathBuf,
    }

    impl TestEnv {
        fn new() -> Result<Self> {
            let tmp = tempfile::tempdir()?;
            let plugins_dir = tmp.path().join("plugins");
            fs::create_dir_all(&plugins_dir)?;
            let db_path = tmp.path().join("tasks.db");
            let store = Store::open(&db_path)?;
            let vault_path = tmp.path().join("plugin-secrets.enc");
            let key_path = tmp.path().join("plugin-secrets.key");
            Ok(Self {
                _tmp: tmp,
                plugins_dir,
                db_path,
                store,
                vault_path,
                key_path,
            })
        }

        fn engine_config(&self) -> PluginEngineConfig {
            PluginEngineConfig {
                plugins_dir: self.plugins_dir.clone(),
                db_path: Some(self.db_path.clone()),
                secrets_vault_path: self.vault_path.clone(),
                secrets_key_path: self.key_path.clone(),
                safe_mode: false,
                locale: "en".to_owned(),
                theme_appearance: "light".to_owned(),
                hot_reload: false,
                wake_callback: None,
            }
        }

        fn write_plugin(&self, id: &str, manifest_json: &str, files: &[(&str, &str)]) -> Result<()> {
            let dir = self.plugins_dir.join(id);
            fs::create_dir_all(&dir)?;
            fs::write(dir.join("manifest.json"), manifest_json)?;
            for (rel, content) in files {
                let target = dir.join(rel);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(target, content)?;
            }
            Ok(())
        }
    }

    #[test]
    fn js_crud_canonical_recurrence_batch_rollback_and_outbox_echo_depth_guard() -> Result<()> {
        let env = TestEnv::new()?;

        env.write_plugin(
            "com.test.producer",
            r#"{
                "api_version": 1,
                "id": "com.test.producer",
                "name": "Producer",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["tasks:read", "tasks:write", "storage"],
                "activation_events": ["onCommand:produce", "onEvent:task.created"],
                "contributions": {
                    "commands": [{ "id": "produce", "title": "Produce" }]
                }
            }"#,
            &[(
                "index.js",
                r#"
                import { tasks, batch, storage, commands, events } from "fern";
                export function activate() {
                    events.on("task.created", (ev) => {
                        const prev = storage.get("echoHits") ?? [];
                        prev.push(ev);
                        storage.set("echoHits", prev);
                        // Ping-pong with consumer to exercise MAX_EVENT_DEPTH termination.
                        if (ev.originPlugin === "com.test.consumer") {
                            tasks.create({ title: `Pong ${ev.depth}` });
                        }
                    });
                    commands.register("produce", () => {
                        const t = tasks.create({
                            title: "Recurring Task",
                            dueDate: "2026-11-22",
                            repeat: "FREQ=DAILY;INTERVAL=1"
                        });
                        const completed = tasks.complete(t.id);
                        try {
                            batch(() => {
                                tasks.create({ title: "Should Rollback" });
                                throw new Error("abort batch");
                            });
                        } catch (_e) {}
                        return { createdId: t.id, completedCount: completed.length };
                    });
                }
                "#,
            )],
        )?;

        env.write_plugin(
            "com.test.consumer",
            r#"{
                "api_version": 1,
                "id": "com.test.consumer",
                "name": "Consumer",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["tasks:read", "tasks:write", "storage"],
                "activation_events": ["onEvent:task.created"]
            }"#,
            &[(
                "index.js",
                r#"
                import { tasks, storage, events } from "fern";
                export function activate() {
                    events.on("task.created", (ev) => {
                        const seen = storage.get("seen") ?? [];
                        seen.push(ev);
                        storage.set("seen", seen);
                        tasks.create({ title: `Followup ${ev.depth}` });
                    });
                }
                "#,
            )],
        )?;

        let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.producer")?;
        mgr.approve_permissions("com.test.consumer")?;

        let res = mgr.run_command("com.test.producer", "produce", None)?;
        assert!(res["createdId"].as_str().is_some());

        // Canonical complete generated the next repeating task; rolled-back batch task is absent.
        let all_tasks = env.store.tasks()?;
        assert!(!all_tasks.iter().any(|t| t.title == "Should Rollback"));
        assert_eq!(
            all_tasks.iter().filter(|t| t.title == "Recurring Task").count(),
            2
        );

        // Drain outbox cascade across rounds until MAX_EVENT_DEPTH stops the ping-pong loop.
        for _ in 0..8 {
            let _ = mgr.dispatch_outbox()?;
        }
        assert_eq!(mgr.dispatch_outbox()?, 0);

        // Producer never received its own depth-0 event; every event it got was from consumer.
        let producer_hits = env
            .store
            .plugin_kv_get("com.test.producer", "echoHits")?
            .unwrap_or_default();
        let p_arr: Vec<JsonValue> = serde_json::from_str(&producer_hits)?;
        assert!(!p_arr.is_empty());
        assert!(
            p_arr
                .iter()
                .all(|ev| ev["originPlugin"].as_str() != Some("com.test.producer"))
        );

        let consumer_hits = env
            .store
            .plugin_kv_get("com.test.consumer", "seen")?
            .unwrap_or_default();
        let c_arr: Vec<JsonValue> = serde_json::from_str(&consumer_hits)?;
        assert!(!c_arr.is_empty());
        assert!(
            c_arr
                .iter()
                .all(|ev| ev["originPlugin"].as_str() != Some("com.test.consumer"))
        );
        assert!(
            c_arr
                .iter()
                .chain(p_arr.iter())
                .all(|ev| ev["depth"].as_i64().unwrap_or(99) < i64::from(MAX_EVENT_DEPTH))
        );

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn interrupt_timeout_circuit_breaker_and_recovery() -> Result<()> {
        let env = TestEnv::new()?;
        env.write_plugin(
            "com.test.looper",
            r#"{
                "api_version": 1,
                "id": "com.test.looper",
                "name": "Looper",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["hooks:intercept", "storage"],
                "activation_events": ["onHook:beforeTaskCreate"]
            }"#,
            &[(
                "index.js",
                r#"
                import { hooks, storage } from "fern";
                export function activate() {
                    let count = storage.get("calls") ?? 0;
                    hooks.beforeTaskCreate(() => {
                        count += 1;
                        storage.set("calls", count);
                        if (count === 1) {
                            while (true) {}
                        }
                        throw new Error("boom " + count);
                    });
                }
                "#,
            )],
        )?;

        let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.looper")?;

        // 1st call: while(true) is interrupted around the 2s hook deadline.
        let t0 = Instant::now();
        let d1 = mgr.run_before_task_create(json!({ "title": "A" }))?;
        let elapsed = t0.elapsed();
        assert_eq!(d1, HookDecision::Allow);
        assert!(
            elapsed >= Duration::from_millis(1500) && elapsed < Duration::from_secs(6),
            "unexpected interrupt duration: {elapsed:?}"
        );

        // 2nd and 3rd calls throw JS errors -> trips Circuit Breaker at threshold = 3.
        let _ = mgr.run_before_task_create(json!({ "title": "B" }))?;
        let _ = mgr.run_before_task_create(json!({ "title": "C" }))?;

        let infos = mgr.plugins()?;
        let info = infos
            .iter()
            .find(|p| p.manifest.id == "com.test.looper")
            .ok_or_else(|| anyhow!("missing plugin info"))?;
        assert!(!info.enabled);
        assert!(matches!(info.status, PluginStatus::CircuitBroken { .. }));

        let db_state = env
            .store
            .plugin_state("com.test.looper")?
            .ok_or_else(|| anyhow!("missing db state"))?;
        assert!(!db_state.enabled);
        assert_eq!(db_state.consecutive_errors, CIRCUIT_BREAKER_THRESHOLD);

        mgr.reset_and_enable_plugin("com.test.looper")?;
        let after = mgr.plugins()?;
        let recovered = after
            .iter()
            .find(|p| p.manifest.id == "com.test.looper")
            .ok_or_else(|| anyhow!("missing recovered plugin"))?;
        assert!(recovered.enabled);
        assert!(!matches!(recovered.status, PluginStatus::CircuitBroken { .. }));

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn sandboxed_imports_ui_closure_handlers_and_async_prompt() -> Result<()> {
        let env = TestEnv::new()?;
        fs::write(env._tmp.path().join("outside.js"), "export const secret = 1;")?;

        env.write_plugin(
            "com.test.escape",
            r#"{
                "api_version": 1,
                "id": "com.test.escape",
                "name": "Escape",
                "version": "1.0.0",
                "entry": "index.js",
                "activation_events": ["onCommand:noop"],
                "contributions": { "commands": [{ "id": "noop", "title": "Noop" }] }
            }"#,
            &[(
                "index.js",
                r#"
                import { secret } from "../outside.js";
                export function activate() { return secret; }
                "#,
            )],
        )?;

        env.write_plugin(
            "com.test.panel",
            r#"{
                "api_version": 1,
                "id": "com.test.panel",
                "name": "Panel",
                "version": "1.0.0",
                "entry": "index.js",
                "activation_events": ["onCommand:confirmTest"],
                "contributions": {
                    "sidebar": [{ "id": "main", "title": "Panel", "icon": "icon.svg" }],
                    "commands": [{ "id": "confirmTest", "title": "Confirm" }]
                }
            }"#,
            &[
                ("icon.svg", r#"<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6"/></svg>"#),
                ("helper.js", r#"export const prefix = "Count";"#),
                (
                    "index.js",
                    r#"
                    import { views, ui, commands } from "fern";
                    import { vstack, heading, button, svg } from "fern:ui";
                    import { prefix } from "./helper.js";

                    export function activate() {
                        let count = 0;
                        views.register("main", {
                            render: () => vstack({ gap: 8 }, [
                                heading(`${prefix}: ${count}`),
                                svg({ content: '<svg viewBox="0 0 10 10"><script>alert(1)</script><circle cx="5" cy="5" r="4"/></svg>' }),
                                button({
                                    label: "Increment",
                                    onClick: () => { count += 1; }
                                })
                            ])
                        });
                        commands.register("confirmTest", async () => {
                            return await ui.confirm("Proceed?", "Confirm action");
                        });
                    }
                    "#,
                ),
            ],
        )?;

        let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.escape")?;
        mgr.approve_permissions("com.test.panel")?;

        // Path traversal import fails cleanly at activation time.
        let escape_err = mgr.run_command("com.test.escape", "noop", None).unwrap_err();
        let escape_msg = format!("{escape_err:#}");
        assert!(
            escape_msg.contains("forbidden") || escape_msg.contains("outside.js"),
            "unexpected escape error: {escape_msg}"
        );

        // Mount view -> captures HandlerId closure + sanitized SVG.
        let tree = mgr.mount_view("com.test.panel", "main")?;
        let handler_id = match &tree.root {
            UiNode::VStack { children, .. } => {
                assert!(matches!(&children[0], UiNode::Heading { text, .. } if text == "Count: 0"));
                match &children[1] {
                    UiNode::Svg { content, .. } => assert!(!content.contains("<script")),
                    other => bail!("expected Svg node, got {other:?}"),
                }
                match &children[2] {
                    UiNode::Button { on_click: Some(id), .. } => *id,
                    other => bail!("expected Button with on_click, got {other:?}"),
                }
            }
            other => bail!("expected VStack root, got {other:?}"),
        };

        // Dispatch click -> runs closure -> auto re-renders with updated state.
        let rerendered = mgr
            .dispatch_ui_event(
                "com.test.panel",
                UiEvent::click("main", handler_id),
            )?
            .ok_or_else(|| anyhow!("expected rerendered tree"))?;
        match &rerendered.root {
            UiNode::VStack { children, .. } => {
                assert!(matches!(&children[0], UiNode::Heading { text, .. } if text == "Count: 1"));
            }
            other => bail!("expected VStack root, got {other:?}"),
        }

        // Async confirm prompt resolves from host thread while command awaits.
        let mgr_c = mgr.clone();
        let cmd_thread = std::thread::spawn(move || {
            mgr_c.run_command("com.test.panel", "confirmTest", None)
        });
        let req_id = loop {
            match rx.recv_timeout(Duration::from_secs(3))? {
                HostUpdate::UiEffect(PluginUiEffect::Confirm { request_id, .. }) => break request_id,
                _ => continue,
            }
        };
        mgr.resolve_prompt(req_id, PromptReplyValue::Confirm(true))?;
        let confirmed = cmd_thread
            .join()
            .map_err(|_| anyhow!("command thread panicked"))??;
        assert_eq!(confirmed, JsonValue::Bool(true));

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn script_runner_vault_scrubbing_scaffold_and_fast_shutdown() -> Result<()> {
        let env = TestEnv::new()?;
        let sample_dir = generate_sample_plugin(&env.plugins_dir, "com.test.sample", "Sample")?;
        assert!(sample_dir.join("manifest.json").is_file());
        assert!(sample_dir.join("fern.d.ts").is_file());
        assert!(sample_dir.join("jsconfig.json").is_file());

        let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.sample")?;
        let cmd_res = mgr.run_command("com.test.sample", "quick_note", None)?;
        assert!(cmd_res.as_str().is_some());

        // Create a draft task and run ScriptRunner with a secret stored for `script:s1`.
        let _ = env.store.add_task("Seed Task")?;
        mgr.set_secret("script:s1", "api_key", "sk_live_topsecret_99")?;

        let script = UserScript {
            id: "s1".to_owned(),
            name: "Scrubbed Script".to_owned(),
            trigger_kind: "manual".to_owned(),
            trigger_arg: String::new(),
            permissions: r#"["tasks:read","tasks:write"]"#.to_owned(),
            code: r#"
                console.log("token=sk_live_topsecret_99");
                const t = fern.tasks.create({ title: "Script Task" });
                return { createdTitle: t.title, count: fern.tasks.list().length };
            "#
            .to_owned(),
            enabled: true,
            updated_at: 0,
        };
        let run_res = mgr.test_run_script(script, None)?;
        assert!(run_res.ok, "script failed: {:?}", run_res.error);
        assert!(run_res.data_changed);
        assert_eq!(
            run_res.return_value,
            Some(json!({ "createdTitle": "Script Task", "count": 3 }))
        );
        assert!(
            run_res
                .logs
                .iter()
                .any(|l| l.message.contains("***") && !l.message.contains("sk_live_topsecret_99")),
            "expected scrubbed secret in logs: {:?}",
            run_res.logs
        );

        let shutdown_start = Instant::now();
        mgr.shutdown();
        let shutdown_elapsed = shutdown_start.elapsed();
        assert!(
            shutdown_elapsed <= SHUTDOWN_BUDGET,
            "shutdown exceeded 300ms budget: {shutdown_elapsed:?}"
        );
        Ok(())
    }

    #[test]
    fn view_refresh_from_settings_change_and_async_click_never_panics() -> Result<()> {
        let env = TestEnv::new()?;
        env.write_plugin(
            "com.test.refresh",
            r#"{
                "id": "com.test.refresh",
                "name": "Refresh Test",
                "version": "0.1.0",
                "api_version": 1,
                "entry": "index.js",
                "permissions": ["storage"],
                "settings": [
                    { "key": "client_id", "type": "text", "label": "Client ID", "default": "" },
                    { "key": "client_secret", "type": "secret", "label": "Client Secret" }
                ],
                "contributions": {
                    "sidebar": [{ "id": "sync", "title": "Sync" }]
                }
            }"#,
            &[(
                "index.js",
                r#"
                import fern from "fern";
                import * as ui from "fern:ui";

                let inProgress = false;
                let done = false;

                export function activate() {
                    fern.views.register("sync", {
                        render() {
                            const cid = String(fern.settings.get("client_id") || "");
                            const hasSec = fern.settings.hasSecret("client_secret");
                            const status = done
                                ? "done"
                                : inProgress
                                  ? "working"
                                  : cid && hasSec
                                    ? "ready"
                                    : "setup";
                            return ui.vstack([
                                ui.heading(status, 1),
                                ui.button({
                                    label: "Go",
                                    onClick: async () => {
                                        inProgress = true;
                                        fern.views.refresh("sync");
                                        const ok = await fern.ui.confirm("Go?", "Proceed");
                                        inProgress = false;
                                        done = Boolean(ok);
                                        fern.views.refresh("sync");
                                    }
                                })
                            ]);
                        }
                    });
                    fern.settings.onChange(() => {
                        fern.views.refresh("sync");
                    });
                }
                "#,
            )],
        )?;

        let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.refresh")?;

        let extract_heading_and_btn = |tree: &UiTreePayload| -> Result<(String, HandlerId)> {
            match &tree.root {
                UiNode::VStack { children, .. } => {
                    let heading = match &children[0] {
                        UiNode::Heading { text, .. } => text.clone(),
                        other => bail!("expected Heading, got {other:?}"),
                    };
                    let btn = match &children[1] {
                        UiNode::Button { on_click: Some(id), .. } => *id,
                        other => bail!("expected Button, got {other:?}"),
                    };
                    Ok((heading, btn))
                }
                other => bail!("expected VStack, got {other:?}"),
            }
        };

        // 1. Initially in setup state.
        let initial = mgr.mount_view("com.test.refresh", "sync")?;
        let (h0, _) = extract_heading_and_btn(&initial)?;
        assert_eq!(h0, "setup");

        // 2. Updating client_id triggers settings.onChange -> fern.views.refresh("sync") inside task poll.
        let mut settings = Map::new();
        settings.insert("client_id".to_owned(), JsonValue::String("cid-123".to_owned()));
        mgr.update_settings("com.test.refresh", settings)?;

        // 3. Saving secret also notifies settings.onChange -> view transitions to "ready" automatically.
        mgr.set_secret("com.test.refresh", "client_secret", "sec-xyz")?;
        let ready_tree = loop {
            match rx.recv_timeout(Duration::from_secs(3))? {
                HostUpdate::ViewRendered(p) if p.view_id == "sync" => {
                    let (h, _) = extract_heading_and_btn(&p)?;
                    if h == "ready" {
                        break p;
                    }
                }
                HostUpdate::ViewError { error, .. } => bail!("unexpected view error: {error}"),
                _ => continue,
            }
        };
        let (_, btn_id) = extract_heading_and_btn(&ready_tree)?;

        // 4. Async onClick calls fern.views.refresh("sync") BEFORE awaiting fern.ui.confirm(...).
        let mgr_c = mgr.clone();
        let click_thread = std::thread::spawn(move || {
            mgr_c.dispatch_ui_event("com.test.refresh", UiEvent::click("sync", btn_id))
        });

        let mut saw_working = false;
        let mut prompt_id = None;
        let deadline = Instant::now() + Duration::from_secs(3);
        while (!saw_working || prompt_id.is_none()) && Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(HostUpdate::ViewRendered(p)) if p.view_id == "sync" => {
                    let (h, _) = extract_heading_and_btn(&p)?;
                    if h == "working" {
                        saw_working = true;
                    }
                }
                Ok(HostUpdate::UiEffect(PluginUiEffect::Confirm { request_id, .. })) => {
                    prompt_id = Some(request_id);
                }
                Ok(HostUpdate::ViewError { error, .. }) => bail!("unexpected view error: {error}"),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(saw_working, "expected mid-async fern.views.refresh to render 'working'");
        let req_id = prompt_id.ok_or_else(|| anyhow!("expected confirm prompt"))?;

        mgr.resolve_prompt(req_id, PromptReplyValue::Confirm(true))?;
        let final_tree = click_thread
            .join()
            .map_err(|_| anyhow!("click thread panicked"))??
            .ok_or_else(|| anyhow!("expected final rerendered tree"))?;
        let (h_final, _) = extract_heading_and_btn(&final_tree)?;
        assert_eq!(h_final, "done");

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn c2_purge_plugin_data_wipes_all_state_and_manifest_permission_shrink() -> Result<()> {
        let env = TestEnv::new()?;
        env.write_plugin(
            "com.test.purge",
            r#"{
                "api_version": 1,
                "id": "com.test.purge",
                "name": "Purge",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["tasks:read", "storage", "meta"],
                "activation_events": ["onCommand:seed"],
                "contributions": { "commands": [{ "id": "seed", "title": "Seed" }] }
            }"#,
            &[(
                "index.js",
                r#"
                import { storage, meta, commands } from "fern";
                export function activate() {
                    commands.register("seed", () => {
                        storage.set("k1", { v: 42 });
                        meta.set("task", "00000000-0000-0000-0000-000000000001", "sync", "ok");
                        return true;
                    });
                }
                "#,
            )],
        )?;

        env.write_plugin(
            "com.test.shrink",
            r#"{
                "api_version": 1,
                "id": "com.test.shrink",
                "name": "Shrink",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["tasks:read", "storage"],
                "activation_events": []
            }"#,
            &[("index.js", "export function activate() {}")],
        )?;

        let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.purge")?;
        mgr.approve_permissions("com.test.shrink")?;

        mgr.set_secret("com.test.purge", "api_token", "top_secret_val")?;
        mgr.run_command("com.test.purge", "seed", None)?;

        assert!(env.store.plugin_state("com.test.purge")?.is_some());
        assert!(env.store.plugin_kv_get("com.test.purge", "k1")?.is_some());
        assert!(!env.store.plugin_meta_list("com.test.purge", "task", "00000000-0000-0000-0000-000000000001")?.is_empty());

        // Drain prior updates before purging.
        while rx.try_recv().is_ok() {}
        mgr.purge_plugin_data("com.test.purge")?;

        assert!(env.store.plugin_state("com.test.purge")?.is_none());
        assert!(env.store.plugin_kv_get("com.test.purge", "k1")?.is_none());
        assert!(env.store.plugin_meta_list("com.test.purge", "task", "00000000-0000-0000-0000-000000000001")?.is_empty());
        let vault_check = SecretsVault::open(&env.vault_path, &env.key_path)?;
        assert!(!vault_check.has("com.test.purge", "api_token")?);
        let mut saw_registry_changed = false;
        while let Ok(upd) = rx.try_recv() {
            if let HostUpdate::RegistryChanged(list) = upd
                && !list.iter().any(|p| p.manifest.id == "com.test.purge")
            {
                saw_registry_changed = true;
            }
        }
        assert!(saw_registry_changed, "expected RegistryChanged after purge_plugin_data");

        // Manifest drops `"storage"` -> persisted granted_permissions shrinks to `["tasks:read"]`.
        fs::write(
            env.plugins_dir.join("com.test.shrink/manifest.json"),
            r#"{
                "api_version": 1,
                "id": "com.test.shrink",
                "name": "Shrink",
                "version": "1.0.1",
                "entry": "index.js",
                "permissions": ["tasks:read"],
                "activation_events": []
            }"#,
        )?;
        mgr.reload_plugin("com.test.shrink")?;
        let state_after_drop = env
            .store
            .plugin_state("com.test.shrink")?
            .ok_or_else(|| anyhow!("missing shrink state"))?;
        assert!(!state_after_drop.granted_permissions.contains("storage"));

        // Re-adding `"storage"` later must require re-approval (`NeedsApproval`).
        fs::write(
            env.plugins_dir.join("com.test.shrink/manifest.json"),
            r#"{
                "api_version": 1,
                "id": "com.test.shrink",
                "name": "Shrink",
                "version": "1.0.2",
                "entry": "index.js",
                "permissions": ["tasks:read", "storage"],
                "activation_events": []
            }"#,
        )?;
        mgr.reload_plugin("com.test.shrink")?;
        let info = mgr
            .plugins()?
            .into_iter()
            .find(|p| p.manifest.id == "com.test.shrink")
            .ok_or_else(|| anyhow!("missing shrink info"))?;
        assert!(!info.enabled);
        assert!(matches!(
            info.status,
            PluginStatus::NeedsApproval { ref added_permissions, is_first_install: false }
                if added_permissions.contains(&Permission::Storage)
        ));

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn c3_gated_async_driver_interrupts_detached_loop_and_never_clobbers_confirm_wakeup() -> Result<()> {
        let env = TestEnv::new()?;
        env.write_plugin(
            "com.test.async_c3",
            r#"{
                "api_version": 1,
                "id": "com.test.async_c3",
                "name": "AsyncC3",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": [],
                "activation_events": ["onCommand:waitConfirm", "onCommand:ping", "onCommand:detachedLoop"],
                "contributions": {
                    "sidebar": [{ "id": "panel", "title": "Panel" }],
                    "commands": [
                        { "id": "waitConfirm", "title": "Wait" },
                        { "id": "ping", "title": "Ping" },
                        { "id": "detachedLoop", "title": "DetachedLoop" }
                    ]
                }
            }"#,
            &[(
                "index.js",
                r#"
                import { commands, ui, views } from "fern";
                import { heading } from "fern:ui";
                let renders = 0;
                export function activate() {
                    views.register("panel", {
                        render() {
                            renders += 1;
                            return heading("Render " + renders);
                        }
                    });
                    commands.register("ping", () => "pong:" + renders);
                    commands.register("waitConfirm", async () => {
                        const ok = await ui.confirm("Hold", "Waiting");
                        return { ok, renders };
                    });
                    commands.register("detachedLoop", () => {
                        Promise.resolve().then(() => { while (true) {} });
                        return "scheduled";
                    });
                }
                "#,
            )],
        )?;

        let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.async_c3")?;

        // 1. Start an async command awaiting `fern.ui.confirm`.
        let mgr_c = mgr.clone();
        let wait_thread = std::thread::spawn(move || {
            mgr_c.run_command("com.test.async_c3", "waitConfirm", None)
        });
        let req_id = loop {
            match rx.recv_timeout(Duration::from_secs(3))? {
                HostUpdate::UiEffect(PluginUiEffect::Confirm { request_id, .. }) => break request_id,
                _ => continue,
            }
        };

        // 2. While `waitConfirm` is suspended, run multiple concurrent view renders & commands
        //    that used to overwrite QuickJS's single `AtomicWaker` and wedge `waitConfirm`.
        for _ in 0..3 {
            let _ = mgr.render_view("com.test.async_c3", "panel")?;
            assert!(mgr.run_command("com.test.async_c3", "ping", None)?.as_str().unwrap_or("").starts_with("pong:"));
        }

        // 3. Resolve confirm -> `waitConfirm` must wake immediately and complete cleanly.
        mgr.resolve_prompt(req_id, PromptReplyValue::Confirm(true))?;
        let confirm_res = wait_thread
            .join()
            .map_err(|_| anyhow!("wait thread panicked"))??;
        assert_eq!(confirm_res["ok"], JsonValue::Bool(true));

        // 4. Detached microtask infinite loop must be interrupted by ExecGate and recorded as failure.
        let sched = mgr.run_command("com.test.async_c3", "detachedLoop", None)?;
        assert_eq!(sched, json!("scheduled"));
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut saw_failure = false;
        while Instant::now() < deadline {
            if let Ok(HostUpdate::PluginStatusChanged {
                plugin_id,
                status,
                metrics,
            }) = rx.recv_timeout(Duration::from_millis(250))
                && plugin_id == "com.test.async_c3"
                && metrics.consecutive_errors >= 1
                && matches!(
                    status,
                    PluginStatus::Error(_) | PluginStatus::CircuitBroken { .. }
                )
            {
                saw_failure = true;
                break;
            }
        }
        assert!(saw_failure, "expected detached microtask loop to trip ExecGate and record_failure");

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn c4_concurrent_activation_deduplicated_and_timer_unload_drops_cleanly() -> Result<()> {
        let env = TestEnv::new()?;
        env.write_plugin(
            "com.test.dedup",
            r#"{
                "api_version": 1,
                "id": "com.test.dedup",
                "name": "Dedup",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["storage"],
                "activation_events": ["onCommand:id"],
                "contributions": { "commands": [{ "id": "id", "title": "ID" }] }
            }"#,
            &[(
                "index.js",
                r#"
                import { commands, storage, ui, timers } from "fern";
                export async function activate() {
                    const n = (storage.get("actCount") ?? 0) + 1;
                    storage.set("actCount", n);
                    await ui.confirm("Activate?", "Once");
                    timers.setInterval("heartbeat", 10000, () => {});
                    commands.register("id", () => storage.get("actCount"));
                }
                "#,
            )],
        )?;

        let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.dedup")?;

        // Launch two concurrent commands before the plugin is activated.
        let m1 = mgr.clone();
        let m2 = mgr.clone();
        let t1 = std::thread::spawn(move || m1.run_command("com.test.dedup", "id", None));
        let t2 = std::thread::spawn(move || m2.run_command("com.test.dedup", "id", None));

        let req_id = loop {
            match rx.recv_timeout(Duration::from_secs(3))? {
                HostUpdate::UiEffect(PluginUiEffect::Confirm { request_id, .. }) => break request_id,
                _ => continue,
            }
        };
        // Wait briefly to confirm no duplicate activation confirm arrives.
        std::thread::sleep(Duration::from_millis(80));
        let mut extra_confirms = 0usize;
        while let Ok(upd) = rx.try_recv() {
            if matches!(upd, HostUpdate::UiEffect(PluginUiEffect::Confirm { .. })) {
                extra_confirms += 1;
            }
        }
        assert_eq!(extra_confirms, 0, "concurrent ensure_activated must run activate() only once");

        mgr.resolve_prompt(req_id, PromptReplyValue::Confirm(true))?;
        let r1 = t1.join().map_err(|_| anyhow!("t1 panicked"))??;
        let r2 = t2.join().map_err(|_| anyhow!("t2 panicked"))??;
        assert_eq!(r1, json!(1));
        assert_eq!(r2, json!(1));

        // Let the timer tick once, then disable and reload to exercise Persistent drop order.
        std::thread::sleep(Duration::from_millis(250));
        mgr.set_plugin_enabled("com.test.dedup", false)?;
        mgr.reload_plugin("com.test.dedup")?;
        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn c5_concurrent_calls_during_shutdown_never_deadlock() -> Result<()> {
        let env = TestEnv::new()?;
        let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
        let mut handles = Vec::new();
        for _ in 0..6 {
            let m = mgr.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..50 {
                    if m.plugins().is_err() {
                        break;
                    }
                }
            }));
        }
        std::thread::sleep(Duration::from_millis(5));
        mgr.shutdown();
        for h in handles {
            h.join().map_err(|_| anyhow!("caller thread panicked"))?;
        }
        assert!(mgr.plugins().is_err(), "calls after shutdown must fail immediately");
        Ok(())
    }

    #[test]
    fn c6_timer_interval_cap_and_js_to_json_sparse_array_and_dag_guards() -> Result<()> {
        let env = TestEnv::new()?;
        env.write_plugin(
            "com.test.dos",
            r#"{
                "api_version": 1,
                "id": "com.test.dos",
                "name": "DoS",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["storage"],
                "activation_events": ["onCommand:testAll"],
                "contributions": { "commands": [{ "id": "testAll", "title": "TestAll" }] }
            }"#,
            &[(
                "index.js",
                r#"
                import { commands, timers, storage, ui } from "fern";
                export function activate() {
                    commands.register("testAll", () => {
                        const results = {};
                        try {
                            timers.setInterval("huge", Number.MAX_SAFE_INTEGER, () => {});
                            results.timerHuge = "unexpected_ok";
                        } catch (e) {
                            results.timerHuge = String(e.message || e);
                        }
                        try {
                            timers.setInterval("nan", NaN, () => {});
                            results.timerNan = "unexpected_ok";
                        } catch (e) {
                            results.timerNan = String(e.message || e);
                        }
                        try {
                            const sparse = new Array(2000000000);
                            storage.set("sparse", sparse);
                            results.sparse = "unexpected_ok";
                        } catch (e) {
                            results.sparse = String(e.message || e);
                        }
                        try {
                            let dag = { leaf: 1 };
                            for (let i = 0; i < 24; i++) {
                                dag = { l: dag, r: dag };
                            }
                            storage.set("dag", dag);
                            results.dag = "unexpected_ok";
                        } catch (e) {
                            results.dag = String(e.message || e);
                        }
                        try {
                            let uiDag = { type: "text", text: "x" };
                            for (let i = 0; i < 24; i++) {
                                uiDag = { type: "vstack", children: [uiDag, uiDag] };
                            }
                            ui.openModal("DoS", uiDag);
                            results.uiDag = "unexpected_ok";
                        } catch (e) {
                            results.uiDag = String(e.message || e);
                        }
                        return results;
                    });
                }
                "#,
            )],
        )?;

        let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.dos")?;
        let res = mgr.run_command("com.test.dos", "testAll", None)?;
        assert_ne!(res["timerHuge"], json!("unexpected_ok"));
        assert_ne!(res["timerNan"], json!("unexpected_ok"));
        assert_ne!(res["sparse"], json!("unexpected_ok"));
        assert_ne!(res["dag"], json!("unexpected_ok"));
        assert_ne!(res["uiDag"], json!("unexpected_ok"));

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn h1_event_permission_enforced_for_wildcard_plugin_and_user_script() -> Result<()> {
        let env = TestEnv::new()?;
        // Wildcard subscriber WITHOUT `tasks:read`
        env.write_plugin(
            "com.test.noperm",
            r#"{
                "api_version": 1,
                "id": "com.test.noperm",
                "name": "NoPerm",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["storage"],
                "activation_events": ["onStartup"]
            }"#,
            &[(
                "index.js",
                r#"
                import { events, storage } from "fern";
                export function activate() {
                    events.on("*", (ev) => {
                        const arr = storage.get("events") ?? [];
                        arr.push(ev.kind);
                        storage.set("events", arr);
                    });
                }
                "#,
            )],
        )?;
        // Wildcard subscriber WITH `tasks:read`
        env.write_plugin(
            "com.test.withperm",
            r#"{
                "api_version": 1,
                "id": "com.test.withperm",
                "name": "WithPerm",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["tasks:read", "storage"],
                "activation_events": ["onStartup"]
            }"#,
            &[(
                "index.js",
                r#"
                import { events, storage } from "fern";
                export function activate() {
                    events.on("*", (ev) => {
                        const arr = storage.get("events") ?? [];
                        arr.push(ev.kind);
                        storage.set("events", arr);
                    });
                }
                "#,
            )],
        )?;

        // Event-triggered UserScript WITHOUT `tasks:read` (only `countdowns:write`).
        env.store.save_user_script(&UserScript {
            id: "ev_noperm".to_owned(),
            name: "NoPerm Script".to_owned(),
            trigger_kind: "event".to_owned(),
            trigger_arg: "*".to_owned(),
            permissions: r#"["countdowns:write"]"#.to_owned(),
            code: r#"fern.countdowns.create({ name: "Leaked", date: "2027-01-01" });"#.to_owned(),
            enabled: true,
            updated_at: 0,
        })?;

        let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.noperm")?;
        mgr.approve_permissions("com.test.withperm")?;
        // Give onStartup activations a brief moment to complete.
        std::thread::sleep(Duration::from_millis(120));

        let _ = env.store.add_task("Sensitive Task")?;
        let _ = mgr.dispatch_outbox()?;

        assert!(
            env.store.plugin_kv_get("com.test.noperm", "events")?.is_none(),
            "plugin without tasks:read must not receive task.created via events.on('*')"
        );
        assert!(
            env.store.countdowns()?.is_empty(),
            "event UserScript without tasks:read must not run on task.created"
        );
        let permitted = env
            .store
            .plugin_kv_get("com.test.withperm", "events")?
            .unwrap_or_default();
        assert!(
            permitted.contains("task.created"),
            "plugin with tasks:read should receive task.created, got: {permitted}"
        );

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn h2_h3_circuit_broken_escalation_requires_approval_and_reload_validation_no_spin() -> Result<()> {
        let env = TestEnv::new()?;
        env.write_plugin(
            "com.test.cb",
            r#"{
                "api_version": 1,
                "id": "com.test.cb",
                "name": "CB",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["storage"],
                "activation_events": ["onCommand:fail"],
                "contributions": { "commands": [{ "id": "fail", "title": "Fail" }] }
            }"#,
            &[(
                "index.js",
                r#"
                import { commands } from "fern";
                export function activate() {
                    commands.register("fail", () => { throw new Error("kaboom"); });
                }
                "#,
            )],
        )?;

        let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.cb")?;

        // Trip circuit breaker (3 errors).
        for _ in 0..CIRCUIT_BREAKER_THRESHOLD {
            let _ = mgr.run_command("com.test.cb", "fail", None);
        }
        let tripped = mgr
            .plugins()?
            .into_iter()
            .find(|p| p.manifest.id == "com.test.cb")
            .ok_or_else(|| anyhow!("missing cb plugin"))?;
        assert!(matches!(tripped.status, PluginStatus::CircuitBroken { .. }));

        // Escalate manifest permissions on disk while circuit-broken.
        fs::write(
            env.plugins_dir.join("com.test.cb/manifest.json"),
            r#"{
                "api_version": 1,
                "id": "com.test.cb",
                "name": "CB",
                "version": "1.0.1",
                "entry": "index.js",
                "permissions": ["storage", "tasks:write"],
                "activation_events": ["onCommand:fail"],
                "contributions": { "commands": [{ "id": "fail", "title": "Fail" }] }
            }"#,
        )?;
        mgr.reload_plugin("com.test.cb")?;

        let after_reload = mgr
            .plugins()?
            .into_iter()
            .find(|p| p.manifest.id == "com.test.cb")
            .ok_or_else(|| anyhow!("missing cb plugin after reload"))?;
        assert!(
            matches!(after_reload.status, PluginStatus::NeedsApproval { .. }),
            "expected NeedsApproval before CircuitBroken, got {:?}",
            after_reload.status
        );
        assert!(
            mgr.reset_and_enable_plugin("com.test.cb").is_err(),
            "reset_and_enable must reject escalated manifest until re-approved"
        );

        // H3: invalid plugin id rejected.
        assert!(mgr.reload_plugin("../outside").is_err());

        // H3: mismatched manifest id fails, sets Error, and does not spin check_hot_reload.
        fs::write(
            env.plugins_dir.join("com.test.cb/manifest.json"),
            r#"{
                "api_version": 1,
                "id": "com.other.spoof",
                "name": "Spoof",
                "version": "1.0.2",
                "entry": "index.js"
            }"#,
        )?;
        assert!(mgr.reload_plugin("com.test.cb").is_err());
        let err_info = mgr
            .plugins()?
            .into_iter()
            .find(|p| p.manifest.id == "com.test.cb")
            .ok_or_else(|| anyhow!("missing cb plugin"))?;
        assert!(matches!(err_info.status, PluginStatus::Error(_)));
        assert!(mgr.check_hot_reload()?.is_empty(), "hot reload must not spin on unchanged broken manifest");

        // H3: malformed JSON manifest also updates snapshot and stops hot reload spin.
        fs::write(env.plugins_dir.join("com.test.cb/manifest.json"), "{ invalid json")?;
        assert!(mgr.reload_plugin("com.test.cb").is_err());
        assert!(mgr.check_hot_reload()?.is_empty());

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn h9_h19_service_error_scrubbing_depth_guard_and_activation_logs() -> Result<()> {
        let env = TestEnv::new()?;
        env.write_plugin(
            "com.test.provider",
            r#"{
                "api_version": 1,
                "id": "com.test.provider",
                "name": "Provider",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["services"],
                "activation_events": ["onStartup"],
                "settings": [{ "key": "api_key", "type": "secret", "label": "Key" }]
            }"#,
            &[(
                "index.js",
                r#"
                import { services } from "fern";
                export function activate() {
                    services.provide("prov.leak", () => {
                        throw new Error("failed with prov_secret_xyz_999");
                    });
                    services.provide("prov.recurse", async (n) => {
                        if (n <= 0) return "done";
                        return await services.call("prov.recurse", n - 1);
                    });
                }
                "#,
            )],
        )?;

        env.write_plugin(
            "com.test.caller",
            r#"{
                "api_version": 1,
                "id": "com.test.caller",
                "name": "Caller",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["services"],
                "activation_events": ["onCommand:callLeak", "onCommand:callRec"],
                "contributions": {
                    "commands": [
                        { "id": "callLeak", "title": "CallLeak" },
                        { "id": "callRec", "title": "CallRec" }
                    ]
                }
            }"#,
            &[(
                "index.js",
                r#"
                import { commands, services } from "fern";
                export function activate() {
                    commands.register("callLeak", async () => {
                        try {
                            await services.call("prov.leak", null);
                            return "unexpected_ok";
                        } catch (e) {
                            return String(e.message || e);
                        }
                    });
                    commands.register("callRec", async (arg) => {
                        return await services.call("prov.recurse", Number(arg || 0));
                    });
                }
                "#,
            )],
        )?;

        env.write_plugin(
            "com.test.badact",
            r#"{
                "api_version": 1,
                "id": "com.test.badact",
                "name": "BadAct",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": [],
                "activation_events": ["onCommand:noop"],
                "settings": [{ "key": "tok", "type": "secret", "label": "Tok" }],
                "contributions": { "commands": [{ "id": "noop", "title": "Noop" }] }
            }"#,
            &[(
                "index.js",
                r#"
                export function activate() {
                    console.log("before activation crash");
                    throw new Error("leak act_secret_abc_777");
                }
                "#,
            )],
        )?;

        let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
        mgr.set_secret("com.test.provider", "api_key", "prov_secret_xyz_999")?;
        mgr.set_secret("com.test.badact", "tok", "act_secret_abc_777")?;
        mgr.approve_permissions("com.test.provider")?;
        mgr.approve_permissions("com.test.caller")?;
        mgr.approve_permissions("com.test.badact")?;
        std::thread::sleep(Duration::from_millis(120));

        // H9: provider secret is scrubbed before reaching caller plugin.
        let leak_msg = mgr
            .run_command("com.test.caller", "callLeak", None)?
            .as_str()
            .unwrap_or("")
            .to_owned();
        assert!(
            leak_msg.contains("***") && !leak_msg.contains("prov_secret_xyz_999"),
            "provider secret was not scrubbed: {leak_msg}"
        );

        // H19: valid chain succeeds; deep recursion (> 4) fails cleanly (never hangs even if it
        // trips circuit breaker mid-recursion) and after reset/enable does not leak depth.
        assert_eq!(
            mgr.run_command("com.test.caller", "callRec", Some("2".to_owned()))?,
            json!("done")
        );
        assert!(mgr.run_command("com.test.caller", "callRec", Some("6".to_owned())).is_err());
        let _ = mgr.reset_and_enable_plugin("com.test.provider");
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(
            mgr.run_command("com.test.caller", "callRec", Some("2".to_owned()))?,
            json!("done")
        );

        // H9: activation error scrubbed + pre-failure console logs preserved.
        let act_err = mgr.run_command("com.test.badact", "noop", None).unwrap_err();
        let act_err_str = format!("{act_err:#}");
        assert!(
            act_err_str.contains("***") && !act_err_str.contains("act_secret_abc_777"),
            "activation secret was not scrubbed: {act_err_str}"
        );
        let logs = mgr.plugin_logs("com.test.badact")?;
        assert!(
            logs.iter().any(|l| l.message.contains("before activation crash")),
            "expected pre-failure logs preserved, got: {logs:?}"
        );

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn h14_deactivate_executes_on_shutdown_and_looping_deactivate_respects_budget() -> Result<()> {
        let env = TestEnv::new()?;
        env.write_plugin(
            "com.test.deact_ok",
            r#"{
                "api_version": 1,
                "id": "com.test.deact_ok",
                "name": "DeactOk",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["storage"],
                "activation_events": ["onCommand:wake"],
                "contributions": { "commands": [{ "id": "wake", "title": "Wake" }] }
            }"#,
            &[(
                "index.js",
                r#"
                import { commands, storage } from "fern";
                export function activate() {
                    commands.register("wake", () => true);
                }
                export function deactivate() {
                    storage.set("clean_shutdown", "yes");
                }
                "#,
            )],
        )?;
        env.write_plugin(
            "com.test.deact_spin",
            r#"{
                "api_version": 1,
                "id": "com.test.deact_spin",
                "name": "DeactSpin",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": [],
                "activation_events": ["onCommand:wake"],
                "contributions": { "commands": [{ "id": "wake", "title": "Wake" }] }
            }"#,
            &[(
                "index.js",
                r#"
                import { commands } from "fern";
                export function activate() {
                    commands.register("wake", () => true);
                }
                export function deactivate() {
                    while (true) {}
                }
                "#,
            )],
        )?;

        let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.deact_ok")?;
        mgr.approve_permissions("com.test.deact_spin")?;
        mgr.run_command("com.test.deact_ok", "wake", None)?;
        mgr.run_command("com.test.deact_spin", "wake", None)?;

        let t0 = Instant::now();
        mgr.shutdown();
        let elapsed = t0.elapsed();
        assert!(
            elapsed <= Duration::from_millis(450),
            "shutdown with looping deactivate exceeded budget: {elapsed:?}"
        );
        assert_eq!(
            env.store.plugin_kv_get("com.test.deact_ok", "clean_shutdown")?.as_deref(),
            Some("\"yes\""),
            "deactivate() must execute during shutdown"
        );
        Ok(())
    }

    #[test]
    fn h15_h16_outbox_single_flight_over_100_rows_and_disable_cancels_inflight_prompt() -> Result<()> {
        let env = TestEnv::new()?;
        env.write_plugin(
            "com.test.outbox_big",
            r#"{
                "api_version": 1,
                "id": "com.test.outbox_big",
                "name": "OutboxBig",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["tasks:read", "storage"],
                "activation_events": ["onEvent:task.created", "onCommand:waitPrompt"],
                "contributions": { "commands": [{ "id": "waitPrompt", "title": "WaitPrompt" }] }
            }"#,
            &[(
                "index.js",
                r#"
                import { events, storage, commands, ui } from "fern";
                export function activate() {
                    events.on("task.created", (ev) => {
                        const seen = storage.get("ids") ?? [];
                        seen.push(ev.entityId);
                        storage.set("ids", seen);
                    });
                    commands.register("waitPrompt", async () => {
                        return await ui.confirm("Hold", "Cancel on disable");
                    });
                }
                "#,
            )],
        )?;

        let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.outbox_big")?;

        // H15: Insert 120 tasks (> 100 batch size) and dispatch concurrently from 2 threads.
        for i in 0..120 {
            let _ = env.store.add_task(&format!("Bulk Task {i}"))?;
        }
        let m1 = mgr.clone();
        let m2 = mgr.clone();
        let d1 = std::thread::spawn(move || m1.dispatch_outbox());
        let d2 = std::thread::spawn(move || m2.dispatch_outbox());
        let _ = d1.join().map_err(|_| anyhow!("d1 panicked"))??;
        let _ = d2.join().map_err(|_| anyhow!("d2 panicked"))??;
        assert_eq!(mgr.dispatch_outbox()?, 0);

        let raw_ids = env
            .store
            .plugin_kv_get("com.test.outbox_big", "ids")?
            .unwrap_or_default();
        let ids: Vec<String> = serde_json::from_str(&raw_ids)?;
        let unique: HashSet<&String> = ids.iter().collect();
        assert_eq!(ids.len(), 120, "expected all 120 outbox events drained without duplicates");
        assert_eq!(unique.len(), 120);

        // H16: Disabling plugin while `waitPrompt` awaits confirm cancels prompt and ignores stale completion.
        let m_wait = mgr.clone();
        let wait_thread = std::thread::spawn(move || {
            m_wait.run_command("com.test.outbox_big", "waitPrompt", None)
        });
        loop {
            match rx.recv_timeout(Duration::from_secs(3))? {
                HostUpdate::UiEffect(PluginUiEffect::Confirm { .. }) => break,
                _ => continue,
            }
        }
        mgr.set_plugin_enabled("com.test.outbox_big", false)?;
        let _ = wait_thread.join().map_err(|_| anyhow!("wait thread panicked"))?;

        let info = mgr
            .plugins()?
            .into_iter()
            .find(|p| p.manifest.id == "com.test.outbox_big")
            .ok_or_else(|| anyhow!("missing outbox_big info"))?;
        assert!(!info.enabled);
        assert!(matches!(info.status, PluginStatus::Disabled));
        assert_eq!(info.metrics.consecutive_errors, 0);

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn h17_h18_safe_integers_focus_ui_revision_and_modal_handlers_and_scope_guard() -> Result<()> {
        let env = TestEnv::new()?;
        let _ = env.store.add_focus_session(
            None,
            "Deep Work",
            1_760_000_000_000,
            1_760_001_500_000,
        )?;

        env.write_plugin(
            "com.test.ui_int",
            r#"{
                "api_version": 1,
                "id": "com.test.ui_int",
                "name": "UiInt",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["focus:read"],
                "activation_events": ["onCommand:checkFocus", "onCommand:openStaticModal", "onCommand:openReactiveModal", "onCommand:badModal", "onCommand:capTest"],
                "contributions": {
                    "sidebar": [{ "id": "main", "title": "Main" }],
                    "commands": [
                        { "id": "checkFocus", "title": "CheckFocus" },
                        { "id": "openStaticModal", "title": "OpenStaticModal" },
                        { "id": "openReactiveModal", "title": "OpenReactiveModal" },
                        { "id": "badModal", "title": "BadModal" },
                        { "id": "capTest", "title": "CapTest" }
                    ]
                }
            }"#,
            &[(
                "index.js",
                r#"
                import fern from "fern";
                import * as ui from "fern:ui";

                let staticClicks = 0;
                let dynCount = 0;

                export function activate() {
                    fern.views.register("main", {
                        render() {
                            return ui.vstack([
                                ui.input({ key: "inp", value: "hi", revision: Date.now() }),
                                ui.heading("static=" + staticClicks + ",dyn=" + dynCount, 2),
                            ]);
                        }
                    });
                    fern.commands.register("checkFocus", () => {
                        const sessions = fern.focus.sessions({ from: 1750000000000, to: 1770000000000 });
                        const stats = fern.focus.stats({ from: 1750000000000 });
                        return { count: sessions.length, totalCount: stats.totalCount, staticClicks, dynCount };
                    });
                    fern.commands.register("openStaticModal", () => {
                        const prebuilt = ui.vstack([
                            ui.button({ label: "StaticBtn", onClick: () => { staticClicks += 1; } })
                        ]);
                        fern.ui.openModal("Static", prebuilt);
                        return true;
                    });
                    fern.commands.register("openReactiveModal", () => {
                        fern.ui.openModal("Reactive", () => ui.vstack([
                            ui.heading("n=" + dynCount, 2),
                            ui.button({ label: "Inc", onClick: () => { dynCount += 1; } })
                        ]));
                        return true;
                    });
                    fern.commands.register("badModal", () => {
                        try {
                            fern.ui.openModal("Bad", () => { throw new Error("modal render boom"); });
                        } catch (_e) {}
                        return true;
                    });
                    fern.commands.register("capTest", () => {
                        try {
                            for (let i = 0; i < 300; i++) {
                                ui.registerHandler(() => i);
                            }
                            return "unexpected_ok";
                        } catch (e) {
                            return String(e.message || e);
                        }
                    });
                }
                "#,
            )],
        )?;

        let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
        mgr.approve_permissions("com.test.ui_int")?;

        // H17: Focus query with >i32::MAX epoch ms and UI `input({ revision: Date.now() })`.
        let focus_res = mgr.run_command("com.test.ui_int", "checkFocus", None)?;
        assert_eq!(focus_res["count"], json!(1));
        assert_eq!(focus_res["totalCount"], json!(1));

        let view_tree = mgr.mount_view("com.test.ui_int", "main")?;
        match &view_tree.root {
            UiNode::VStack { children, .. } => match &children[0] {
                UiNode::Input { revision, .. } => {
                    assert!(*revision > u64::from(u32::MAX), "expected Date.now() revision, got {revision}");
                }
                other => bail!("expected Input, got {other:?}"),
            },
            other => bail!("expected VStack, got {other:?}"),
        }

        // H18: Pre-built UiNode passed to `openModal` dispatches click under `"__modal__"`.
        while rx.try_recv().is_ok() {}
        mgr.run_command("com.test.ui_int", "openStaticModal", None)?;
        let static_hid = loop {
            match rx.recv_timeout(Duration::from_secs(3))? {
                HostUpdate::UiEffect(PluginUiEffect::OpenModal { view_id, tree, .. }) => {
                    assert_eq!(view_id, "__modal__");
                    match tree {
                        UiNode::VStack { children, .. } => match &children[0] {
                            UiNode::Button { on_click: Some(hid), .. } => break *hid,
                            other => bail!("expected Button, got {other:?}"),
                        },
                        other => bail!("expected VStack, got {other:?}"),
                    }
                }
                _ => continue,
            }
        };
        let _ = mgr.dispatch_ui_event("com.test.ui_int", UiEvent::click("__modal__", static_hid))?;
        let after_static = mgr.run_command("com.test.ui_int", "checkFocus", None)?;
        assert_eq!(after_static["staticClicks"], json!(1));

        // H18: Reactive modal re-renders on click and unmounts cleanly.
        while rx.try_recv().is_ok() {}
        mgr.run_command("com.test.ui_int", "openReactiveModal", None)?;
        let dyn_hid = loop {
            match rx.recv_timeout(Duration::from_secs(3))? {
                HostUpdate::UiEffect(PluginUiEffect::OpenModal { tree, .. }) => match tree {
                    UiNode::VStack { children, .. } => match &children[1] {
                        UiNode::Button { on_click: Some(hid), .. } => break *hid,
                        other => bail!("expected Button, got {other:?}"),
                    },
                    other => bail!("expected VStack, got {other:?}"),
                },
                _ => continue,
            }
        };
        let rerendered_modal = mgr
            .dispatch_ui_event("com.test.ui_int", UiEvent::click("__modal__", dyn_hid))?
            .ok_or_else(|| anyhow!("expected rerendered reactive modal"))?;
        match &rerendered_modal.root {
            UiNode::VStack { children, .. } => {
                assert!(matches!(&children[0], UiNode::Heading { text, .. } if text == "n=1"));
            }
            other => bail!("expected VStack, got {other:?}"),
        }
        mgr.unmount_view("com.test.ui_int", "__modal__")?;

        // H18: Throwing inside openModal restores active_render_scope + capTest hits outside-render cap.
        mgr.run_command("com.test.ui_int", "badModal", None)?;
        let cap_msg = mgr.run_command("com.test.ui_int", "capTest", None)?;
        assert_ne!(cap_msg, json!("unexpected_ok"));

        mgr.shutdown();
        Ok(())
    }

    #[test]
    fn medium_batch_savepoint_filters_scripts_startup_and_http_secret_scrubbing() -> Result<()> {
        let env = TestEnv::new()?;
        env.store.add_tag("work", None, None)?;
        env.store.add_tag("sub", None, Some("work"))?;
        let today = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();

        // Start a local HTTP echo server that reflects request Authorization header into body & headers.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server_handle = std::thread::spawn(move || {
            use std::io::{Read, Write};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let auth_line = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("Authorization: none")
                    .to_owned();
                let body = serde_json::json!({ "echoed": auth_line }).to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Reflected: {auth_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        env.write_plugin(
            "com.test.medium",
            r#"{
                "api_version": 1,
                "id": "com.test.medium",
                "name": "Medium",
                "version": "1.0.0",
                "entry": "index.js",
                "permissions": ["tasks:read", "tasks:write", "lists:read", "lists:write", "storage", "network:local"],
                "activation_events": ["onStartup", "onCommand:runBatch", "onCommand:runFilters", "onCommand:runFetch"],
                "settings": [{ "key": "api_token", "type": "secret", "label": "Token" }],
                "contributions": {
                    "commands": [
                        { "id": "runBatch", "title": "RunBatch" },
                        { "id": "runFilters", "title": "RunFilters" },
                        { "id": "runFetch", "title": "RunFetch" }
                    ]
                }
            }"#,
            &[(
                "index.js",
                &format!(
                    r#"
                import fern from "fern";
                export function activate() {{
                    fern.storage.set("booted", true);
                    fern.commands.register("runBatch", () => {{
                        const readOnly = fern.batch(() => fern.tasks.list().length);
                        let asyncRejected = false;
                        try {{
                            fern.batch(async () => 1);
                        }} catch (e) {{
                            asyncRejected = String(e.message || e).includes("synchronous");
                        }}
                        let rolledBack = false;
                        try {{
                            fern.batch(() => {{
                                fern.tasks.create({{ title: "OuterRollback" }});
                                throw new Error("abort outer");
                            }});
                        }} catch (_e) {{
                            rolledBack = true;
                        }}
                        fern.batch(() => {{
                            fern.tasks.create({{ title: "OuterCommit" }});
                            try {{
                                fern.batch(() => {{
                                    fern.tasks.create({{ title: "InnerSavepointRollback" }});
                                    throw new Error("abort inner");
                                }});
                            }} catch (_e) {{}}
                        }});
                        return {{ readOnly, asyncRejected, rolledBack }};
                    }});
                    fern.commands.register("runFilters", () => {{
                        const listA = fern.lists.create({{ name: "Work" }});
                        const listB = fern.lists.create({{ name: "Home" }});
                        fern.tasks.create({{ title: "Match", listId: listA.id, dueDate: "{today}", tags: ["sub"] }});
                        fern.tasks.create({{ title: "WrongList", listId: listB.id, dueDate: "{today}", tags: ["work"] }});
                        fern.tasks.create({{ title: "WrongTag", listId: listA.id, dueDate: "{today}", tags: ["other"] }});
                        fern.tasks.create({{ title: "WrongDate", listId: listA.id, dueDate: "2099-01-01", tags: ["work"] }});
                        return fern.tasks.list({{ view: "today", listId: listA.id, tag: "work" }}).map((t) => t.title);
                    }});
                    fern.commands.register("runFetch", async () => {{
                        const r = await fern.http.fetch("http://127.0.0.1:{port}/echo", {{
                            headers: {{ Authorization: "Bearer ${{secret:api_token}}" }}
                        }});
                        return {{
                            body: r.body,
                            text: r.text(),
                            json: r.json(),
                            reflectedHeader: r.headers["x-reflected"] || "",
                        }};
                    }});
                }}
                "#
                ),
            )],
        )?;

        let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
        mgr.set_secret("com.test.medium", "api_token", "sk_super_secret_12345")?;
        mgr.approve_permissions("com.test.medium")?;

        // M18: onStartup activates automatically after approve_permissions.
        let boot_deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < boot_deadline {
            if env.store.plugin_kv_get("com.test.medium", "booted")?.as_deref() == Some("true") {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(
            env.store.plugin_kv_get("com.test.medium", "booted")?.as_deref(),
            Some("true"),
            "expected onStartup plugin to activate automatically after approve_permissions"
        );

        // M16: batch SAVEPOINT + Promise rejection + read-only / rollback data_dirty behavior.
        let batch_res = mgr.run_command("com.test.medium", "runBatch", None)?;
        assert_eq!(batch_res["asyncRejected"], json!(true));
        assert_eq!(batch_res["rolledBack"], json!(true));
        let titles: Vec<String> = env.store.tasks()?.into_iter().map(|t| t.title).collect();
        assert!(titles.iter().any(|t| t == "OuterCommit"));
        assert!(!titles.iter().any(|t| t == "OuterRollback"));
        assert!(!titles.iter().any(|t| t == "InnerSavepointRollback"));

        // Read-only batch must NOT emit DataChanged.
        while rx.try_recv().is_ok() {}
        let ro_script = UserScript {
            id: "ro_batch".to_owned(),
            name: "RO Batch".to_owned(),
            trigger_kind: "manual".to_owned(),
            trigger_arg: String::new(),
            permissions: r#"["tasks:read","tasks:write"]"#.to_owned(),
            code: r#"
                fern.batch(() => fern.tasks.list());
                try {
                    fern.batch(() => {
                        fern.tasks.create("RolledBack");
                        throw new Error("nope");
                    });
                } catch (_e) {}
                return "ok";
            "#
            .to_owned(),
            enabled: true,
            updated_at: 0,
        };
        let ro_res = mgr.test_run_script(ro_script, None)?;
        assert!(ro_res.ok, "ro_script failed: {:?}", ro_res.error);
        assert!(!ro_res.data_changed, "read-only/rolled-back batch must not mark data_changed");

        // M20: combined view + listId + tag (with child tag family) filter.
        let filtered = mgr.run_command("com.test.medium", "runFilters", None)?;
        assert_eq!(filtered, json!(["Match"]));

        // M9: reflected ${secret:...} scrubbed from body, text(), json(), and headers.
        let fetch_res = mgr.run_command("com.test.medium", "runFetch", None)?;
        let fetch_str = fetch_res.to_string();
        assert!(
            fetch_str.contains("***") && !fetch_str.contains("sk_super_secret_12345"),
            "expected reflected vault secret scrubbed from http response: {fetch_str}"
        );
        let _ = server_handle.join();

        // M21: UserScript sync `event` binding, `return;`, comment/string containing import/export,
        // and blocking relative filesystem imports.
        let sync_ev_script = UserScript {
            id: "sync_ev".to_owned(),
            name: "Sync Event".to_owned(),
            trigger_kind: "manual".to_owned(),
            trigger_arg: String::new(),
            permissions: "[]".to_owned(),
            code: r#"
                // import foo from "./bar.js"
                const note = "export default 123";
                if (!event || !event.val) { return; }
                return { got: event.val, note };
            "#
            .to_owned(),
            enabled: true,
            updated_at: 0,
        };
        let sync_res = mgr.test_run_script(sync_ev_script, Some(json!({ "val": 99 })))?;
        assert!(sync_res.ok, "sync event script failed: {:?}", sync_res.error);
        assert_eq!(
            sync_res.return_value,
            Some(json!({ "got": 99, "note": "export default 123" }))
        );

        let import_escape_script = UserScript {
            id: "imp_esc".to_owned(),
            name: "Import Escape".to_owned(),
            trigger_kind: "manual".to_owned(),
            trigger_arg: String::new(),
            permissions: "[]".to_owned(),
            code: r#"import * as m from "./com.test.medium/index.js"; export default m;"#.to_owned(),
            enabled: true,
            updated_at: 0,
        };
        let esc_res = mgr.test_run_script(import_escape_script, None)?;
        assert!(!esc_res.ok, "UserScript filesystem import must be blocked");

        mgr.shutdown();
        Ok(())
    }
}
