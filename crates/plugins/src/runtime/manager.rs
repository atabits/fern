use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle, Thread};

use anyhow::{Context as _, Result, anyhow, ensure};
use fern_core::model::Id;
use fern_core::plugin_store::UserScript;
use futures::task::ArcWake;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue};

use super::events::HookKind;
use super::worker_main;
use crate::api::{LogEntry, PluginUiEffect, PromptReplyValue};
use crate::manifest::PluginManifest;
use crate::permissions::Permission;
use crate::ui_ast::{UiEvent, UiTreePayload};

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

pub(super) type ReplyTx<T> = std::sync::mpsc::SyncSender<Result<T>>;

pub(super) enum WorkerCmd {
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

pub(super) struct WorkerWake {
    thread: Thread,
    pub(super) woken: AtomicBool,
}

impl WorkerWake {
    fn new(thread: Thread) -> Arc<Self> {
        Arc::new(Self {
            thread,
            woken: AtomicBool::new(false),
        })
    }
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

pub(super) struct CmdQueue {
    inner: Mutex<CmdQueueInner>,
    wake: Arc<WorkerWake>,
}

impl CmdQueue {
    fn new(wake: Arc<WorkerWake>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(CmdQueueInner {
                items: VecDeque::new(),
                closed: false,
            }),
            wake,
        })
    }

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

    pub(super) fn pop_all(&self) -> Vec<WorkerCmd> {
        let Ok(mut guard) = self.inner.lock() else {
            return Vec::new();
        };
        guard.items.drain(..).collect()
    }

    /// Marks the queue closed and immediately drops any queued commands so callers waiting
    /// on `ReplyTx` channels unblock without deadlocking.
    pub(super) fn close_and_clear(&self) {
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
                let wake = WorkerWake::new(thread::current());
                let queue = CmdQueue::new(Arc::clone(&wake));
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
