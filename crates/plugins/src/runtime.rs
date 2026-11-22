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
use rquickjs::promise::MaybePromise;
use rquickjs::{AsyncContext, AsyncRuntime, CaughtError, Ctx, Function, Module, Persistent, Value};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue, json};
use uuid::Uuid;

use crate::api::{
    EffectEmitterFn, LogEntry, LogLevel, PluginApiState, PluginHooks, PluginUiEffect,
    PromptReplyValue, PromptSenders, ServiceCallerFn, ServiceRegistrarFn, ViewRefresherFn,
    install_fern_bindings, js_to_json, js_value_to_ui_node, json_to_js, task_to_json,
};
use crate::http::{HTTP_POOL_THREADS, HttpPool};
use crate::manifest::{
    LoadedManifest, PermissionApprovalStatus, PluginManifest, validate_plugin_id,
};
use crate::module_loader::{SandboxedLoader, SandboxedResolver};
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

struct CmdQueue {
    items: Mutex<VecDeque<WorkerCmd>>,
    wake: Arc<WorkerWake>,
}

impl CmdQueue {
    fn push(&self, cmd: WorkerCmd) {
        if let Ok(mut guard) = self.items.lock() {
            guard.push_back(cmd);
        }
        ArcWake::wake_by_ref(&self.wake);
    }

    fn pop_all(&self) -> Vec<WorkerCmd> {
        let Ok(mut guard) = self.items.lock() else {
            return Vec::new();
        };
        guard.drain(..).collect()
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
        self.queue.push(WorkerCmd::Shutdown);
        if let Ok(mut guard) = self.handle.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
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
                    items: Mutex::new(VecDeque::new()),
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
        self.inner.queue.push(build(tx));
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
            self.inner.queue.push(WorkerCmd::DispatchOutbox(tx));
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
            self.inner.queue.push(WorkerCmd::SetAppearance {
                locale,
                theme_appearance,
            });
        }
    }

    pub fn check_hot_reload(&self) -> Result<Vec<String>> {
        self.call(WorkerCmd::CheckHotReload)
    }

    /// Cleanly interrupts running JS, gives active plugins up to 300ms to deactivate, and joins the worker thread.
    pub fn shutdown(&self) {
        self.inner.shutdown_internal();
    }
}

struct ExecGate {
    deadline_ms: AtomicU64,
    cancelled: Arc<AtomicBool>,
    epoch: Instant,
}

impl ExecGate {
    fn new(epoch: Instant, cancelled: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            deadline_ms: AtomicU64::new(0),
            cancelled,
            epoch,
        })
    }

    fn arm(&self, budget: Duration) {
        let now_ms = self.epoch.elapsed().as_millis() as u64;
        let target = now_ms.saturating_add(budget.as_millis().max(1) as u64);
        self.deadline_ms.store(target.max(1), Ordering::Release);
    }

    fn disarm(&self) {
        self.deadline_ms.store(0, Ordering::Release);
    }

    fn should_interrupt(&self) -> bool {
        if self.cancelled.load(Ordering::Acquire) {
            return true;
        }
        let dl = self.deadline_ms.load(Ordering::Acquire);
        if dl == 0 {
            return false;
        }
        let now_ms = self.epoch.elapsed().as_millis() as u64;
        now_ms >= dl
    }
}

struct LoadedRuntime {
    rt: AsyncRuntime,
    ctx: AsyncContext,
    api: Rc<PluginApiState>,
    gate: Arc<ExecGate>,
    module_ns: Option<Persistent<rquickjs::Object<'static>>>,
}

impl LoadedRuntime {
    fn shutdown_drop(mut self) {
        self.module_ns.take();
        self.api.clear_persistent_handles();
        drop(self.ctx);
        drop(self.rt);
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

struct WorkerEnv {
    config: PluginEngineConfig,
    store: Rc<Store>,
    vault: SecretsVault,
    http_pool: Arc<HttpPool>,
    locale: Rc<RefCell<String>>,
    theme_appearance: Rc<RefCell<String>>,
    prompt_senders: PromptSenders,
    next_prompt_id: Rc<Cell<u64>>,
    service_providers: Rc<RefCell<HashMap<String, String>>>,
    service_depth: Rc<Cell<usize>>,
    slots: Rc<RefCell<BTreeMap<String, PluginSlot>>>,
    tasks: Rc<RefCell<FuturesUnordered<LocalTask>>>,
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
        self.tasks.borrow_mut().push(Box::pin(fut));
    }

    fn plugin_infos(&self) -> Vec<PluginInfo> {
        let slots = self.slots.borrow();
        slots
            .values()
            .map(|slot| slot_to_info(slot, &self.vault))
            .collect()
    }

    fn flush_plugin_side_effects(&self, plugin_id: &str) {
        let (effects, dirty) = {
            let slots = self.slots.borrow();
            let Some(slot) = slots.get(plugin_id) else {
                return;
            };
            let Some(rt) = &slot.runtime else {
                return;
            };
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
        }
    }

    fn flush_all_pending_side_effects(&self) {
        let plugin_ids: Vec<String> = {
            let slots = self.slots.borrow();
            slots
                .iter()
                .filter_map(|(id, slot)| {
                    let rt = slot.runtime.as_ref()?;
                    let has_work = !rt.api.pending_ui_effects.borrow().is_empty()
                        || rt.api.data_dirty.get();
                    has_work.then(|| id.clone())
                })
                .collect()
        };
        for pid in plugin_ids {
            self.flush_plugin_side_effects(&pid);
        }
    }

    fn discover_plugins(&self) -> Result<()> {
        if self.config.safe_mode {
            return Ok(());
        }
        if !self.config.plugins_dir.exists() {
            fs::create_dir_all(&self.config.plugins_dir)?;
        }

        // Unload existing active runtimes cleanly before rescanning.
        let old_ids: Vec<String> = self.slots.borrow().keys().cloned().collect();
        for id in old_ids {
            self.unload_runtime(&id, false);
        }
        self.slots.borrow_mut().clear();

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

        let status = if consecutive_errors >= CIRCUIT_BREAKER_THRESHOLD {
            enabled = false;
            PluginStatus::CircuitBroken {
                last_error: "auto-disabled after repeated crashes or timeouts".to_owned(),
            }
        } else {
            match approval {
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
                    if enabled {
                        PluginStatus::Idle
                    } else {
                        PluginStatus::Disabled
                    }
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

    fn unload_runtime(&self, plugin_id: &str, call_deactivate: bool) {
        let mut runtime_opt = {
            let mut slots = self.slots.borrow_mut();
            let Some(slot) = slots.get_mut(plugin_id) else {
                return;
            };
            if let Some(rt) = &slot.runtime {
                slot.logs_cache = rt.api.logs.borrow().clone();
            }
            slot.runtime.take()
        };

        if let Some(ref mut loaded) = runtime_opt {
            self.service_providers
                .borrow_mut()
                .retain(|_, owner| owner != plugin_id);

            if call_deactivate
                && let Some(ns_pers) = loaded.module_ns.take()
            {
                loaded.gate.arm(Duration::from_millis(100));
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
            if let Some(loaded) = runtime_opt {
                loaded.shutdown_drop();
            }
        }
    }

    fn record_failure(&self, plugin_id: &str, err_msg: &str) {
        let scrubbed = self.vault.scrub_secrets(plugin_id, err_msg);
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
            self.unload_runtime(plugin_id, false);
        }

        self.emit(HostUpdate::PluginStatusChanged {
            plugin_id: plugin_id.to_owned(),
            status,
            metrics,
        });
    }

    fn record_success(&self, plugin_id: &str, elapsed_ms: u64, mem_bytes: u64) {
        let mut need_notify = false;
        let status_metrics = {
            let mut slots = self.slots.borrow_mut();
            let Some(slot) = slots.get_mut(plugin_id) else {
                return;
            };
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
            prompt_senders: Rc::new(RefCell::new(HashMap::new())),
            next_prompt_id: Rc::new(Cell::new(1)),
            service_providers: Rc::new(RefCell::new(HashMap::new())),
            service_depth: Rc::new(Cell::new(0)),
            slots: Rc::new(RefCell::new(BTreeMap::new())),
            tasks: Rc::new(RefCell::new(FuturesUnordered::new())),
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
            if let Err(err) = e.discover_plugins() {
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
    if !env.config.safe_mode {
        let startup_ids: Vec<String> = env
            .slots
            .borrow()
            .values()
            .filter(|s| {
                s.enabled
                    && (s.loaded_manifest.manifest.matches_activation_event("onStartup")
                        || s.loaded_manifest.manifest.matches_activation_event("*"))
            })
            .map(|s| s.loaded_manifest.manifest.id.clone())
            .collect();
        for pid in startup_ids {
            let env_c = Rc::clone(&env);
            env.spawn_task(async move {
                let _ = ensure_activated(&env_c, &pid).await;
            });
        }
    }

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

        // 2. Drive spawned async plugin tasks.
        loop {
            let poll_res = {
                let mut tasks = env.tasks.borrow_mut();
                if tasks.is_empty() {
                    break;
                }
                tasks.poll_next_unpin(&mut task_cx)
            };
            match poll_res {
                Poll::Ready(Some(())) => continue,
                Poll::Ready(None) | Poll::Pending => break,
            }
        }

        // 3. Flush any UI effects queued before an async task suspended, then pump microtasks.
        env.flush_all_pending_side_effects();
        pump_active_plugin_jobs(&env);

        // 4. Fire due `fern.timers` intervals.
        fire_due_timers(&env);

        // 5. Check hot-reload if enabled.
        let now = Instant::now();
        if env.config.hot_reload && now >= next_hot_reload {
            next_hot_reload = now + HOT_RELOAD_INTERVAL;
            let _ = perform_hot_reload(&env);
        }

        // 6. Sleep until next command, async wakeup, timer, or hot-reload tick.
        if !wake.woken.swap(false, Ordering::AcqRel) {
            let timeout = next_wakeup_delay(&env, next_hot_reload);
            thread::park_timeout(timeout);
        }
    }

    // Clean shutdown within <=300ms budget.
    env.http_pool.close_nonblocking();
    env.prompt_senders.borrow_mut().clear();
    env.tasks.borrow_mut().clear();
    let active_ids: Vec<String> = env.slots.borrow().keys().cloned().collect();
    let deadline = Instant::now() + Duration::from_millis(240);
    for id in active_ids {
        let allow_deact = Instant::now() < deadline;
        env.unload_runtime(&id, allow_deact);
    }
}

fn has_any_event_subscribers(env: &WorkerEnv) -> bool {
    let any_plugin = env.slots.borrow().values().any(|s| {
        s.enabled
            && s.loaded_manifest
                .manifest
                .activation_events
                .iter()
                .any(|ev| ev == "*" || ev == "onStartup" || ev.starts_with("onEvent:"))
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
            let res = env.discover_plugins().map(|()| {
                let infos = env.plugin_infos();
                env.emit(HostUpdate::RegistryChanged(infos.clone()));
                infos
            });
            let _ = reply.send(res);
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
            let res = set_plugin_enabled_sync(env, &plugin_id, enabled);
            let _ = reply.send(res);
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
            let res = match value {
                Some(v) => env.vault.set(&plugin_id, &key, &v),
                None => env.vault.delete(&plugin_id, &key).map(|_| ()),
            };
            if res.is_ok() {
                env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
            }
            let _ = reply.send(res);
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
            if let Some(tx) = env.prompt_senders.borrow_mut().remove(&request_id) {
                let _ = tx.send(reply_val);
            }
            let _ = reply.send(Ok(()));
        }
        WorkerCmd::DispatchOutbox(reply) => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = dispatch_outbox_events(&env_c).await;
                let _ = reply.send(res);
            });
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
        WorkerCmd::Shutdown => {}
    }
}

fn set_plugin_enabled_sync(env: &WorkerEnv, plugin_id: &str, enabled: bool) -> Result<()> {
    {
        let slots = env.slots.borrow();
        let slot = slots
            .get(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        if enabled
            && matches!(
                slot.status,
                PluginStatus::NeedsApproval { .. } | PluginStatus::CircuitBroken { .. }
            )
        {
            bail!("plugin {plugin_id} requires permission approval or circuit breaker reset before enabling");
        }
    }
    env.store.set_plugin_enabled(plugin_id, enabled)?;
    if !enabled {
        env.unload_runtime(plugin_id, true);
    }
    if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
        slot.enabled = enabled;
        if !enabled {
            slot.status = PluginStatus::Disabled;
        } else if slot.runtime.is_none() {
            slot.status = PluginStatus::Idle;
        }
    }
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

fn approve_plugin_permissions(env: &WorkerEnv, plugin_id: &str) -> Result<()> {
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
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

fn reset_and_enable(env: &WorkerEnv, plugin_id: &str) -> Result<()> {
    {
        let slots = env.slots.borrow();
        let slot = slots
            .get(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        if matches!(slot.status, PluginStatus::NeedsApproval { .. }) {
            bail!("plugin {plugin_id} requires permission approval first");
        }
    }
    env.store.reset_plugin_errors(plugin_id)?;
    env.store.set_plugin_enabled(plugin_id, true)?;
    if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
        slot.enabled = true;
        slot.metrics.consecutive_errors = 0;
        slot.status = PluginStatus::Idle;
    }
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn reload_single_plugin(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    let dir = {
        let slots = env.slots.borrow();
        slots
            .get(plugin_id)
            .map(|s| s.loaded_manifest.dir.clone())
            .unwrap_or_else(|| env.config.plugins_dir.join(plugin_id))
    };
    env.unload_runtime(plugin_id, true);
    let loaded = LoadedManifest::load_from_dir(&dir)?;
    let new_slot = env.build_slot(loaded)?;
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

    // Notify registered `fern.settings.onChange` callbacks if runtime is active.
    let maybe_rt = {
        let slots = env.slots.borrow();
        slots
            .get(plugin_id)
            .and_then(|s| s.runtime.as_ref())
            .map(|r| (r.ctx.clone(), r.gate.clone(), Rc::clone(&r.api)))
    };
    if let Some((ctx, gate, api)) = maybe_rt {
        let _ = run_with_gate(&ctx, &gate, DEFAULT_EVENT_TIMEOUT, async move |ctx| {
            let listeners = api.settings_listeners.borrow().clone();
            let val = json_to_js(&ctx, &JsonValue::Object(resolved))?;
            for listener in listeners {
                if let Ok(func) = listener.restore(&ctx) {
                    let res: MaybePromise = func.call((val.clone(),))?;
                    let _ = res.into_future::<Value>().await?;
                }
            }
            Ok(())
        })
        .await;
    }

    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
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

    let (loaded, settings, logs_prev) = {
        let slots = env.slots.borrow();
        let slot = slots
            .get(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        ensure!(slot.enabled, "plugin {plugin_id:?} is disabled");
        (
            slot.loaded_manifest.clone(),
            slot.settings_values.clone(),
            slot.logs_cache.clone(),
        )
    };

    let rt = AsyncRuntime::new().context("failed to create QuickJS AsyncRuntime")?;
    rt.set_memory_limit(PLUGIN_MEMORY_LIMIT_BYTES).await;
    rt.set_max_stack_size(PLUGIN_STACK_LIMIT_BYTES).await;
    rt.set_loader(
        SandboxedResolver::new(loaded.dir.clone()),
        SandboxedLoader::new(loaded.dir.clone()),
    )
    .await;

    let gate = ExecGate::new(env.worker_epoch, Arc::clone(&env.shutdown_flag));
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
    let service_caller: ServiceCallerFn = Rc::new(move |svc_name, arg| {
        let (tx, rx) = oneshot::channel();
        let env_call = Rc::clone(&env_for_svc);
        env_for_svc.spawn_task(async move {
            let res = invoke_service(&env_call, &svc_name, arg).await;
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
        write_depth: Cell::new(0),
        in_batch: Cell::new(false),
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
        shutdown_flag: Arc::clone(&env.shutdown_flag),
    });

    let start = Instant::now();
    let entry_source = fs::read_to_string(&loaded.entry_path).with_context(|| {
        format!("cannot read entry file {}", loaded.entry_path.display())
    })?;
    let entry_name = loaded.entry_path.to_string_lossy().into_owned();
    let api_init = Rc::clone(&api);
    let loaded_ns_res = run_with_gate(&ctx, &gate, DEFAULT_COMMAND_TIMEOUT, async move |ctx| {
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
    })
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    match loaded_ns_res {
        Ok(module_ns) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            {
                let mut slots = env.slots.borrow_mut();
                if let Some(slot) = slots.get_mut(plugin_id) {
                    slot.runtime = Some(LoadedRuntime {
                        rt,
                        ctx,
                        api,
                        gate,
                        module_ns: Some(module_ns),
                    });
                }
            }
            env.record_success(plugin_id, elapsed, mem);
            env.flush_plugin_side_effects(plugin_id);
            Ok(())
        }
        Err(err) => {
            let msg = format!("{err:#}");
            api.clear_persistent_handles();
            drop(ctx);
            drop(rt);
            env.record_failure(plugin_id, &msg);
            Err(anyhow!("{msg}"))
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
    let (rt, ctx, gate, api) = active_runtime_handles(env, plugin_id)?;
    let start = Instant::now();
    let view_id_own = view_id.to_owned();
    let api_c = Rc::clone(&api);

    let res = run_with_gate(&ctx, &gate, DEFAULT_EVENT_TIMEOUT, async move |ctx| {
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
        api_c.set_active_scope(Some(view_id_own.clone()));
        let f = render_fn.restore(&ctx)?;
        let raw_node: MaybePromise = f.call(())?;
        let val = raw_node.into_future::<Value>().await?;
        let parsed = js_value_to_ui_node(&ctx, val, &api_c, &view_id_own);
        api_c.set_active_scope(None);
        match parsed {
            Ok(root) => Ok(UiTreePayload {
                plugin_id: api_c.plugin_id.clone(),
                view_id: view_id_own,
                root,
            }),
            Err(err) => Err(rquickjs::Exception::throw_message(&ctx, &format!("{err:#}"))),
        }
    })
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    match res {
        Ok(payload) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, elapsed, mem);
            env.flush_plugin_side_effects(plugin_id);
            env.emit(HostUpdate::ViewRendered(payload.clone()));
            Ok(payload)
        }
        Err(err) => {
            api.set_active_scope(None);
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            env.record_failure(plugin_id, &msg);
            env.emit(HostUpdate::ViewError {
                plugin_id: plugin_id.to_owned(),
                view_id: view_id.to_owned(),
                error: msg.clone(),
            });
            Err(anyhow!("{msg}"))
        }
    }
}

async fn unmount_plugin_view(env: &Rc<WorkerEnv>, plugin_id: &str, view_id: &str) -> Result<()> {
    let Ok((_rt, ctx, gate, api)) = active_runtime_handles(env, plugin_id) else {
        return Ok(());
    };
    let view_id_own = view_id.to_owned();
    let api_c = Rc::clone(&api);
    let _ = run_with_gate(&ctx, &gate, DEFAULT_EVENT_TIMEOUT, async move |ctx| {
        let unmount = {
            let views = api_c.views.borrow();
            views.get(&view_id_own).and_then(|v| v.unmount.clone())
        };
        if let Some(unmount_pers) = unmount {
            let f = unmount_pers.restore(&ctx)?;
            let r: MaybePromise = f.call(())?;
            let _ = r.into_future::<Value>().await?;
        }
        api_c.clear_scope_handlers(&view_id_own);
        Ok(())
    })
    .await;
    Ok(())
}

async fn handle_ui_event(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    event: UiEvent,
) -> Result<Option<UiTreePayload>> {
    ensure_activated(env, plugin_id).await?;
    let (rt, ctx, gate, api) = active_runtime_handles(env, plugin_id)?;
    let start = Instant::now();
    let scope = event.view_id.clone();
    let hid = event.handler_id.0;
    let payload = event.payload.clone();
    let api_c = Rc::clone(&api);

    let call_res = run_with_gate(&ctx, &gate, DEFAULT_EVENT_TIMEOUT, async move |ctx| {
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
    })
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    match call_res {
        Ok(_ran) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, elapsed, mem);
            env.flush_plugin_side_effects(plugin_id);
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
            env.record_failure(plugin_id, &msg);
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
    let (rt, ctx, gate, api) = active_runtime_handles(env, plugin_id)?;
    let start = Instant::now();
    let cmd_id = command_id.to_owned();
    let api_c = Rc::clone(&api);

    let res = run_with_gate(&ctx, &gate, DEFAULT_COMMAND_TIMEOUT, async move |ctx| {
        let handler = api_c.commands.borrow().get(&cmd_id).cloned().ok_or_else(|| {
            rquickjs::Exception::throw_message(&ctx, &format!("command {cmd_id:?} not registered"))
        })?;
        let f = handler.restore(&ctx)?;
        let raw: MaybePromise = match arg {
            Some(a) => f.call((a,))?,
            None => f.call(())?,
        };
        let val = raw.into_future::<Value>().await?;
        js_to_json(&ctx, val).map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("{e:#}")))
    })
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    match res {
        Ok(val) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, elapsed, mem);
            env.flush_plugin_side_effects(plugin_id);
            Ok(val)
        }
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            env.record_failure(plugin_id, &msg);
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
        let Ok((rt, ctx, gate, api)) = active_runtime_handles(env, &pid) else {
            continue;
        };
        let q = query.to_owned();
        let api_c = Rc::clone(&api);
        let start = Instant::now();
        let res = run_with_gate(&ctx, &gate, DEFAULT_COMMAND_TIMEOUT, async move |ctx| {
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
        })
        .await;

        let elapsed = start.elapsed().as_millis() as u64;
        match res {
            Ok(mut hits) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, elapsed, mem);
                all_hits.append(&mut hits);
            }
            Err(err) => {
                env.record_failure(&pid, &format!("{err:#}"));
            }
        }
    }
    Ok(all_hits)
}

async fn invoke_service(env: &Rc<WorkerEnv>, service_name: &str, arg: JsonValue) -> Result<JsonValue> {
    let current_depth = env.service_depth.get();
    ensure!(
        current_depth < MAX_SERVICE_CALL_DEPTH,
        "inter-plugin service call depth exceeded {MAX_SERVICE_CALL_DEPTH}"
    );
    let provider_id = env
        .service_providers
        .borrow()
        .get(service_name)
        .cloned()
        .with_context(|| format!("service {service_name:?} is not registered"))?;

    let (rt, ctx, gate, api) = active_runtime_handles(env, &provider_id)?;
    let svc_name = service_name.to_owned();
    let api_c = Rc::clone(&api);
    let start = Instant::now();
    env.service_depth.set(current_depth + 1);
    let res = run_with_gate(&ctx, &gate, DEFAULT_COMMAND_TIMEOUT, async move |ctx| {
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
    })
    .await;
    env.service_depth.set(current_depth);

    let elapsed = start.elapsed().as_millis() as u64;
    match res {
        Ok(out) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(&provider_id, elapsed, mem);
            env.flush_plugin_side_effects(&provider_id);
            Ok(out)
        }
        Err(err) => {
            let msg = format!("{err:#}");
            env.record_failure(&provider_id, &msg);
            Err(anyhow!("{msg}"))
        }
    }
}

async fn dispatch_outbox_events(env: &Rc<WorkerEnv>) -> Result<usize> {
    if env.config.safe_mode {
        return Ok(0);
    }
    let events = env.store.drain_plugin_outbox(100)?;
    if events.is_empty() {
        return Ok(0);
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

        // Trigger matching enabled user scripts (`trigger_kind == "event"`).
        for script in user_scripts.iter().filter(|s| {
            s.enabled
                && s.trigger_kind == "event"
                && (s.trigger_arg == ev.event_kind || s.trigger_arg == "*")
        }) {
            let script_origin = format!("script:{}", script.id);
            if ev.origin_plugin.as_deref() == Some(script_origin.as_str()) {
                continue;
            }
            let _ = run_user_script(env, script, Some(payload.clone()), next_depth).await;
            delivered += 1;
        }
    }

    env.store.ack_plugin_outbox(last_seq)?;
    Ok(delivered)
}

async fn emit_simple_event(env: &Rc<WorkerEnv>, kind: &str, payload: JsonValue) -> Result<()> {
    if env.config.safe_mode {
        return Ok(());
    }
    let activation_token = format!("onEvent:{kind}");
    let candidate_ids: Vec<String> = env
        .slots
        .borrow()
        .values()
        .filter(|s| {
            s.enabled
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
    let Ok((rt, ctx, gate, api)) = active_runtime_handles(env, plugin_id) else {
        return false;
    };
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

    let prev_depth = api.write_depth.replace(write_depth);
    let start = Instant::now();
    let res = run_with_gate(&ctx, &gate, DEFAULT_EVENT_TIMEOUT, async move |ctx| {
        let js_payload = json_to_js(&ctx, &payload)?;
        for func_pers in listeners {
            let func = func_pers.restore(&ctx)?;
            let r: MaybePromise = func.call((js_payload.clone(),))?;
            let _ = r.into_future::<Value>().await?;
        }
        Ok(())
    })
    .await;
    api.write_depth.set(prev_depth);

    let elapsed = start.elapsed().as_millis() as u64;
    match res {
        Ok(()) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, elapsed, mem);
            env.flush_plugin_side_effects(plugin_id);
            true
        }
        Err(err) => {
            env.record_failure(plugin_id, &format!("{err:#}"));
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
        let Ok((rt, ctx, gate, api)) = active_runtime_handles(env, &pid) else {
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
        let res = run_with_gate(&ctx, &gate, DEFAULT_EVENT_TIMEOUT, async move |ctx| {
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
                if out_json == JsonValue::Bool(false) {
                    return Ok(HookDecision::Cancel("cancelled by plugin hook".to_owned()));
                }
                if out_json
                    .get("cancel")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
                {
                    let reason = out_json
                        .get("reason")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("cancelled by plugin hook")
                        .to_owned();
                    return Ok(HookDecision::Cancel(reason));
                }
                if let Some(next_val) = out_json.get("draft").or_else(|| out_json.get("value")) {
                    cur_val = next_val.clone();
                    changed = true;
                } else if out_json.is_object() || out_json.is_string() {
                    cur_val = out_json;
                    changed = true;
                }
            }
            if changed {
                Ok(HookDecision::Modify(cur_val))
            } else {
                Ok(HookDecision::Allow)
            }
        })
        .await;

        let elapsed = start.elapsed().as_millis() as u64;
        match res {
            Ok(HookDecision::Allow) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, elapsed, mem);
                env.flush_plugin_side_effects(&pid);
            }
            Ok(HookDecision::Modify(next)) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, elapsed, mem);
                env.flush_plugin_side_effects(&pid);
                current = next;
                modified = true;
            }
            Ok(HookDecision::Cancel(reason)) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, elapsed, mem);
                env.flush_plugin_side_effects(&pid);
                return Ok(HookDecision::Cancel(reason));
            }
            Err(err) => {
                env.record_failure(&pid, &format!("{err:#}"));
            }
        }
    }

    if modified {
        Ok(HookDecision::Modify(current))
    } else {
        Ok(HookDecision::Allow)
    }
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
    let empty_dir = env.config.plugins_dir.clone();
    rt.set_loader(
        SandboxedResolver::new(empty_dir.clone()),
        SandboxedLoader::new(empty_dir),
    )
    .await;

    let gate = ExecGate::new(env.worker_epoch, Arc::clone(&env.shutdown_flag));
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
    let service_caller: ServiceCallerFn = Rc::new(move |svc_name, svc_arg| {
        let (tx, rx) = oneshot::channel();
        let env_call = Rc::clone(&env_for_svc);
        env_for_svc.spawn_task(async move {
            let res = invoke_service(&env_call, &svc_name, svc_arg).await;
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
        write_depth: Cell::new(write_depth),
        in_batch: Cell::new(false),
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

    let code = script.code.clone();
    let api_c = Rc::clone(&api);
    let res = run_with_gate(&ctx, &gate, DEFAULT_COMMAND_TIMEOUT, async move |ctx| {
        install_fern_bindings(&ctx, Rc::clone(&api_c), true)?;
        let looks_like_module = code.contains("export ") || code.contains("import ");
        let val = if looks_like_module {
            let declared = Module::declare(ctx.clone(), "user_script.js", code)?;
            let (evaluated, promise) = declared.eval()?;
            promise.into_future::<()>().await?;
            let ns = evaluated.namespace()?;
            if let Ok(Some(default_fn)) = ns.get::<_, Option<Function>>("default") {
                let js_arg = match arg {
                    Some(a) => json_to_js(&ctx, &a)?,
                    None => Value::new_undefined(ctx.clone()),
                };
                let ret: MaybePromise = default_fn.call((js_arg,))?;
                ret.into_future::<Value>().await?
            } else {
                Value::new_undefined(ctx.clone())
            }
        } else if code.contains("await ") || code.contains("return ") {
            let js_arg = match arg {
                Some(a) => json_to_js(&ctx, &a)?,
                None => Value::new_undefined(ctx.clone()),
            };
            ctx.globals().set("__script_arg", js_arg)?;
            let wrapped = format!("(async function(event) {{\n{code}\n}})(globalThis.__script_arg)");
            let mut opts = EvalOptions::default();
            opts.global = true;
            opts.strict = true;
            let promise = ctx.eval_with_options::<MaybePromise, _>(wrapped, opts)?;
            promise.into_future::<Value>().await?
        } else {
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
    })
    .await;

    let effects: Vec<PluginUiEffect> = api.pending_ui_effects.borrow_mut().drain(..).collect();
    let data_changed = api.data_dirty.get();
    let logs: Vec<LogEntry> = api.logs.borrow().iter().cloned().collect();
    api.clear_persistent_handles();
    drop(ctx);
    drop(rt);

    for eff in effects {
        env.emit(HostUpdate::UiEffect(eff));
    }
    if data_changed {
        env.emit(HostUpdate::DataChanged);
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

fn pump_active_plugin_jobs(env: &Rc<WorkerEnv>) {
    let active: Vec<(String, AsyncRuntime)> = env
        .slots
        .borrow()
        .iter()
        .filter_map(|(id, slot)| slot.runtime.as_ref().map(|r| (id.clone(), r.rt.clone())))
        .collect();

    for (pid, rt) in active {
        if futures::executor::block_on(rt.is_job_pending()) {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                while rt.is_job_pending().await {
                    match rt.execute_pending_job().await {
                        Ok(true) => continue,
                        Ok(false) => break,
                        Err(err_ctx) => {
                            let msg: String = err_ctx
                                .0
                                .with(|ctx| format_caught_error(&ctx, rquickjs::Error::Exception))
                                .await;
                            env_c.record_failure(&pid, &msg);
                            break;
                        }
                    }
                }
                env_c.flush_plugin_side_effects(&pid);
            });
        }
    }
}

fn fire_due_timers(env: &Rc<WorkerEnv>) {
    let now = Instant::now();
    let mut due: Vec<(String, String, Persistent<Function<'static>>)> = Vec::new();
    {
        let slots = env.slots.borrow();
        for (pid, slot) in slots.iter() {
            let Some(rt) = &slot.runtime else {
                continue;
            };
            for timer in rt.api.timers.borrow_mut().values_mut() {
                if timer.next_fire <= now {
                    timer.next_fire = now + Duration::from_millis(timer.interval_ms);
                    due.push((pid.clone(), timer.id.clone(), timer.callback.clone()));
                }
            }
        }
    }
    for (pid, _tid, cb) in due {
        let env_c = Rc::clone(env);
        env.spawn_task(async move {
            let Ok((rt, ctx, gate, _api)) = active_runtime_handles(&env_c, &pid) else {
                return;
            };
            let start = Instant::now();
            let res = run_with_gate(&ctx, &gate, DEFAULT_EVENT_TIMEOUT, async move |ctx| {
                let func = cb.restore(&ctx)?;
                let raw: MaybePromise = func.call(())?;
                let _ = raw.into_future::<Value>().await?;
                Ok(())
            })
            .await;
            let elapsed = start.elapsed().as_millis() as u64;
            match res {
                Ok(()) => {
                    let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                    env_c.record_success(&pid, elapsed, mem);
                    env_c.flush_plugin_side_effects(&pid);
                }
                Err(err) => {
                    env_c.record_failure(&pid, &format!("{err:#}"));
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

fn active_runtime_handles(
    env: &WorkerEnv,
    plugin_id: &str,
) -> Result<(AsyncRuntime, AsyncContext, Arc<ExecGate>, Rc<PluginApiState>)> {
    let slots = env.slots.borrow();
    let slot = slots
        .get(plugin_id)
        .with_context(|| format!("plugin {plugin_id:?} not found"))?;
    let rt = slot
        .runtime
        .as_ref()
        .with_context(|| format!("plugin {plugin_id:?} is not active"))?;
    Ok((
        rt.rt.clone(),
        rt.ctx.clone(),
        Arc::clone(&rt.gate),
        Rc::clone(&rt.api),
    ))
}

async fn run_with_gate<F, R>(
    ctx: &AsyncContext,
    gate: &Arc<ExecGate>,
    budget: Duration,
    f: F,
) -> Result<R>
where
    F: for<'js> AsyncFnOnce(Ctx<'js>) -> rquickjs::Result<R>,
    R: 'static,
{
    struct GateFuture<Fut> {
        fut: Fut,
        gate: Arc<ExecGate>,
        budget: Duration,
    }
    impl<Fut: std::future::Future> std::future::Future for GateFuture<Fut> {
        type Output = Fut::Output;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
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

    let inner = ctx.async_with(async move |ctx| match f(ctx.clone()).await {
        Ok(v) => Ok(v),
        Err(err) => Err(format_caught_error(&ctx, err)),
    });
    GateFuture {
        fut: inner,
        gate: Arc::clone(gate),
        budget,
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
            val.as_string()
                .and_then(|s| s.to_string().ok())
                .unwrap_or_else(|| format!("{val:?}"))
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
    use crate::ui_ast::UiNode;

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
}
