//! GPUI global [`PluginHost`] managing [`PluginManager`], async [`HostUpdate`] dispatch,
//! `PluginBus` synchronization, `DataVersion` <-> outbox bridging, active-window hot-reload,
//! plugin theme loading, and reactive plugin view tree state.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(not(test))]
use std::future::Future;
use std::path::PathBuf;
#[cfg(not(test))]
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
#[cfg(not(test))]
use std::task::{Context as TaskContext, Poll};
use std::task::Waker;
use std::time::Duration;

use anyhow::Result;
use desktop_common::data_version::{DataVersion, changed_elsewhere, data_changed};
use desktop_common::paths;
use desktop_common::plugin_bus::{
    PluginBus, PluginCommandSpec, PluginHookDecision, PluginHooksGlobal, PluginModalKind,
    PluginModalSpec, PluginNavAction, PluginSearchHit, PluginSidebarViewSpec,
    ToastLevel as BusToastLevel,
};
use gpui::{
    App, AppContext as _, BorrowAppContext as _, ClipboardItem, Entity, EntityId, Global,
    Subscription,
};
use plugins::manifest::resolve_inside_dir;
use plugins::ui_ast::{UiEvent, UiTreePayload};
use plugins::{
    HookDecision, HostUpdate, PluginEngineConfig, PluginInfo, PluginManager, PluginStatus,
    PluginUiEffect, PromptReplyValue, RegistryClient, ToastLevel as PluginToastLevel,
};
use theme::{Theme, ThemeRegistry};

const TOAST_AUTO_DISMISS_DELAY: Duration = Duration::from_millis(4500);
const HOT_RELOAD_POLL_INTERVAL: Duration = Duration::from_millis(1200);
const MAX_PENDING_TOASTS: usize = 6;

/// Configuration used to initialize [`PluginHost`].
#[derive(Clone, Debug)]
pub struct PluginHostConfig {
    pub plugins_dir: PathBuf,
    pub registry_cache_dir: PathBuf,
    pub db_path: Option<PathBuf>,
    pub secrets_vault_path: PathBuf,
    pub secrets_key_path: PathBuf,
    pub safe_mode: bool,
    pub locale: String,
    pub theme_appearance: String,
    pub hot_reload: bool,
}

impl PluginHostConfig {
    /// Builds desktop configuration from standard Fern paths (`desktop_common::paths`).
    pub fn from_desktop_paths(
        db_path: Option<PathBuf>,
        safe_mode: bool,
        locale: impl Into<String>,
        theme_appearance: impl Into<String>,
        hot_reload: bool,
    ) -> Result<Self> {
        Ok(Self {
            plugins_dir: paths::plugins_dir()?,
            registry_cache_dir: paths::plugin_registry_cache_dir()?,
            db_path,
            secrets_vault_path: paths::plugin_secrets_path()?,
            secrets_key_path: paths::plugin_secrets_key_path()?,
            safe_mode,
            locale: locale.into(),
            theme_appearance: theme_appearance.into(),
            hot_reload,
        })
    }
}

/// Sentinel GPUI entity used as the `EntityId` source when plugins mutate SQLite data.
struct HostDataMarker;

struct WakeSignal {
    #[cfg(not(test))]
    woken: AtomicBool,
    closed: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

#[cfg(not(test))]
impl WakeSignal {
    fn wake(&self) {
        self.woken.store(true, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().ok().and_then(|mut guard| guard.take()) {
            waker.wake();
        }
    }
}

impl WakeSignal {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            #[cfg(not(test))]
            woken: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            waker: Mutex::new(None),
        })
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().ok().and_then(|mut guard| guard.take()) {
            waker.wake();
        }
    }
}

#[cfg(not(test))]
struct WaitForWake(Arc<WakeSignal>);

#[cfg(not(test))]
impl Future for WaitForWake {
    type Output = bool;

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        if self.0.closed.load(Ordering::SeqCst) {
            return Poll::Ready(false);
        }
        if self.0.woken.swap(false, Ordering::SeqCst) {
            return Poll::Ready(true);
        }
        if let Ok(mut guard) = self.0.waker.lock() {
            *guard = Some(cx.waker().clone());
        }
        if self.0.closed.load(Ordering::SeqCst) {
            return Poll::Ready(false);
        }
        if self.0.woken.swap(false, Ordering::SeqCst) {
            return Poll::Ready(true);
        }
        Poll::Pending
    }
}

#[derive(Default)]
struct ScheduledToastTimers(HashSet<u64>);

impl Global for ScheduledToastTimers {}

enum PluginViewOp {
    Mount { view_id: String, generation: u64 },
    Render { view_id: String, generation: u64 },
    Unmount { view_id: String },
    Reload,
    ReloadAndMount { view_id: String, generation: u64 },
    ResetAndMount { view_id: String, generation: u64 },
}

#[derive(Default)]
struct PluginOpState {
    view_generations: HashMap<(String, String), u64>,
    mounting_views: HashSet<(String, String)>,
    in_flight_reload_or_reset: HashSet<String>,
    queues: HashMap<String, VecDeque<PluginViewOp>>,
    running: HashSet<String>,
}

impl PluginOpState {
    fn next_view_gen(&mut self, plugin_id: &str, view_id: &str) -> u64 {
        let key = (plugin_id.to_owned(), view_id.to_owned());
        let entry = self.view_generations.entry(key).or_insert(0);
        *entry = entry.wrapping_add(1);
        *entry
    }

    fn invalidate_plugin_views(&mut self, plugin_id: &str) {
        for ((pid, _), generation) in &mut self.view_generations {
            if pid == plugin_id {
                *generation = generation.wrapping_add(1);
            }
        }
        self.mounting_views.retain(|(pid, _)| pid != plugin_id);
    }
}

type ThemeSourceSignature = Vec<(String, PathBuf, String)>;

/// Global GPUI host coordinating the plugin runtime, UI bus, view trees, themes, and registry.
pub struct PluginHost {
    config: PluginHostConfig,
    manager: Option<PluginManager>,
    registry_client: RegistryClient,
    plugins_dir: PathBuf,
    safe_mode: bool,
    hot_reload_enabled: bool,
    hot_reload_started: bool,
    plugins: Arc<Vec<PluginInfo>>,
    plugin_revisions: HashMap<String, u64>,
    views: HashMap<(String, String), Arc<UiTreePayload>>,
    view_errors: HashMap<(String, String), String>,
    view_revisions: HashMap<(String, String), u64>,
    op_state: RefCell<PluginOpState>,
    registry_change_seq: u64,
    pending_hot_reload: HashSet<String>,
    loaded_plugin_theme_ids: HashSet<String>,
    plugin_theme_errors: Vec<String>,
    loaded_theme_signature: Option<ThemeSourceSignature>,
    cached_plugin_themes: Vec<Theme>,
    theme_sync_seq: AtomicU64,
    requested_settings_plugin: Option<String>,
    requested_settings_open: Option<String>,
    external_data_seq: AtomicU64,
    notified_outbox_seq: AtomicU64,
    search_seq: AtomicU64,
    modal_generation: AtomicU64,
    hooks_active: Arc<AtomicBool>,
    _marker: Entity<HostDataMarker>,
    marker_id: EntityId,
    wake_signal: Arc<WakeSignal>,
    #[cfg(test)]
    test_update_rx: RefCell<Option<mpsc::Receiver<HostUpdate>>>,
    _data_sub: Subscription,
}

fn has_active_hook_plugins(safe_mode: bool, has_manager: bool, plugins: &[PluginInfo]) -> bool {
    !safe_mode
        && has_manager
        && plugins.iter().any(|info| {
            info.enabled
                && !matches!(
                    info.status,
                    PluginStatus::Disabled
                        | PluginStatus::NeedsApproval { .. }
                        | PluginStatus::CircuitBroken { .. }
                        | PluginStatus::Error(_)
                )
                && info
                    .manifest
                    .permissions
                    .iter()
                    .any(|perm| matches!(perm, plugins::Permission::HooksIntercept))
        })
}

fn map_hook_decision(decision: HookDecision) -> PluginHookDecision {
    match decision {
        HookDecision::Allow => PluginHookDecision::Allow,
        HookDecision::Modify(val) => PluginHookDecision::Modify(val),
        HookDecision::Cancel(reason) => PluginHookDecision::Cancel(reason),
    }
}

fn sync_hooks_global(
    mgr: Option<&PluginManager>,
    hooks_active: Arc<AtomicBool>,
    cx: &mut App,
) {
    let Some(mgr) = mgr else {
        hooks_active.store(false, Ordering::Release);
        PluginHooksGlobal::clear_global(cx);
        return;
    };
    let mgr_create = mgr.clone();
    let mgr_complete = mgr.clone();
    let mgr_quick_add = mgr.clone();
    let hooks = PluginHooksGlobal::new(
        Arc::new(move |draft| {
            mgr_create
                .run_before_task_create(draft)
                .map(map_hook_decision)
        }),
        Arc::new(move |task| {
            mgr_complete
                .run_before_task_complete(task)
                .map(map_hook_decision)
        }),
        Arc::new(move |raw| {
            let text = raw.as_str().unwrap_or_default().to_owned();
            mgr_quick_add
                .run_quick_add_transform(text)
                .map(map_hook_decision)
        }),
    )
    .with_active_check(Arc::new(move || hooks_active.load(Ordering::Acquire)));
    PluginHooksGlobal::set_global(hooks, cx);
}

impl Global for PluginHost {}

impl Drop for PluginHost {
    fn drop(&mut self) {
        self.wake_signal.close();
        if let Some(mgr) = self.manager.take() {
            mgr.shutdown();
        }
    }
}

impl PluginHost {
    fn build_host(
        config: PluginHostConfig,
        manager: Option<PluginManager>,
        registry_client: RegistryClient,
        initial_plugins: Vec<PluginInfo>,
        prev_themes: (HashSet<String>, Vec<String>),
        wake_signal: Arc<WakeSignal>,
        cx: &mut App,
    ) -> Self {
        let (prev_theme_ids, prev_theme_errors) = prev_themes;
        let marker = cx.new(|_| HostDataMarker);
        let marker_id = marker.entity_id();

        let data_sub = cx.observe_global::<DataVersion>(move |cx| {
            let Some(host) = cx.try_global::<PluginHost>() else {
                return;
            };
            if changed_elsewhere(host.marker_id, cx) {
                let next = host
                    .external_data_seq
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1);
                if let Some(mgr) = &host.manager {
                    mgr.notify_outbox_dirty();
                    host.notified_outbox_seq.store(next, Ordering::Release);
                }
            }
        });

        let mut plugin_revisions = HashMap::new();
        for info in &initial_plugins {
            plugin_revisions.insert(info.manifest.id.clone(), 1);
        }
        let hooks_active = Arc::new(AtomicBool::new(has_active_hook_plugins(
            config.safe_mode,
            manager.is_some(),
            &initial_plugins,
        )));

        Self {
            plugins_dir: config.plugins_dir.clone(),
            safe_mode: config.safe_mode,
            hot_reload_enabled: config.hot_reload && !config.safe_mode,
            hot_reload_started: false,
            config,
            manager,
            registry_client,
            plugins: Arc::new(initial_plugins),
            plugin_revisions,
            views: HashMap::new(),
            view_errors: HashMap::new(),
            view_revisions: HashMap::new(),
            op_state: RefCell::new(PluginOpState::default()),
            registry_change_seq: 0,
            pending_hot_reload: HashSet::new(),
            loaded_plugin_theme_ids: prev_theme_ids,
            plugin_theme_errors: prev_theme_errors,
            loaded_theme_signature: None,
            cached_plugin_themes: Vec::new(),
            theme_sync_seq: AtomicU64::new(0),
            requested_settings_plugin: None,
            requested_settings_open: None,
            external_data_seq: AtomicU64::new(0),
            notified_outbox_seq: AtomicU64::new(0),
            search_seq: AtomicU64::new(0),
            modal_generation: AtomicU64::new(0),
            hooks_active,
            _marker: marker,
            marker_id,
            wake_signal,
            #[cfg(test)]
            test_update_rx: RefCell::new(None),
            _data_sub: data_sub,
        }
    }

    /// Initializes [`PluginHost`] as a GPUI `Global`, spawning [`PluginManager`] unless in safe mode.
    pub fn init(config: PluginHostConfig, cx: &mut App) -> Result<()> {
        let mut prev_theme_ids = HashSet::new();
        let mut prev_theme_errors = Vec::new();
        if cx.try_global::<Self>().is_some() {
            dismiss_active_modal_on_shutdown(cx);
            let mut old = cx.remove_global::<Self>();
            prev_theme_ids = std::mem::take(&mut old.loaded_plugin_theme_ids);
            prev_theme_errors = std::mem::take(&mut old.plugin_theme_errors);
            old.shutdown_inner();
        }

        let wake_signal = WakeSignal::new();
        let registry_client =
            RegistryClient::new(&config.plugins_dir, &config.registry_cache_dir);

        let (manager, update_rx, initial_plugins) = if config.safe_mode {
            (None, None, Vec::new())
        } else {
            #[cfg(not(test))]
            let wake_callback: Option<Arc<dyn Fn() + Send + Sync>> = {
                let signal_for_worker = Arc::clone(&wake_signal);
                Some(Arc::new(move || signal_for_worker.wake()))
            };
            // Under GPUI's deterministic `TestScheduler`, external OS threads must not wake
            // `LocalExecutor` wakers directly; tests drain `test_update_rx` deterministically.
            #[cfg(test)]
            let wake_callback: Option<Arc<dyn Fn() + Send + Sync>> = None;
            let engine_cfg = PluginEngineConfig {
                plugins_dir: config.plugins_dir.clone(),
                db_path: config.db_path.clone(),
                secrets_vault_path: config.secrets_vault_path.clone(),
                secrets_key_path: config.secrets_key_path.clone(),
                safe_mode: false,
                locale: config.locale.clone(),
                theme_appearance: config.theme_appearance.clone(),
                // Driven via active-window GPUI poller instead of idle background disk scans.
                hot_reload: false,
                wake_callback,
            };
            match PluginManager::spawn(engine_cfg) {
                Ok((mgr, rx)) => {
                    let initial_plugins = mgr.plugins().unwrap_or_default();
                    (Some(mgr), Some(rx), initial_plugins)
                }
                Err(err) => {
                    // Restore a safe-mode host so the global is never left removed/bricked.
                    let mut fallback_cfg = config;
                    fallback_cfg.safe_mode = true;
                    let fallback = Self::build_host(
                        fallback_cfg,
                        None,
                        registry_client,
                        Vec::new(),
                        (prev_theme_ids, prev_theme_errors),
                        wake_signal,
                        cx,
                    );
                    cx.set_global(fallback);
                    PluginHooksGlobal::clear_global(cx);
                    sync_bus_contributions(&[], cx);
                    Self::sync_plugin_themes(cx);
                    return Err(err);
                }
            }
        };

        let hot_reload = config.hot_reload && !config.safe_mode;
        let host = Self::build_host(
            config,
            manager,
            registry_client,
            initial_plugins.clone(),
            (prev_theme_ids, prev_theme_errors),
            Arc::clone(&wake_signal),
            cx,
        );
        let hooks_active = Arc::clone(&host.hooks_active);
        let mgr_for_hooks = host.manager.clone();
        #[cfg(test)]
        {
            *host.test_update_rx.borrow_mut() = update_rx;
        }

        cx.set_global(host);
        sync_hooks_global(mgr_for_hooks.as_ref(), hooks_active, cx);
        sync_bus_contributions(&initial_plugins, cx);
        Self::sync_plugin_themes(cx);

        #[cfg(not(test))]
        if let Some(rx) = update_rx {
            spawn_update_consumer(rx, wake_signal, cx);
        }
        if hot_reload {
            Self::start_hot_reload_poller(cx);
        }

        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn drain_updates_for_test(cx: &mut App) {
        let mut batch = Vec::new();
        if let Some(host) = cx.try_global::<Self>()
            && let Some(rx) = host.test_update_rx.borrow().as_ref()
        {
            while let Ok(update) = rx.try_recv() {
                batch.push(update);
            }
        }
        for update in batch {
            apply_host_update(update, cx);
        }
    }

    /// Initializes [`PluginHost`] in safe mode without spawning any QuickJS worker.
    pub fn init_safe_mode(
        plugins_dir: PathBuf,
        registry_cache_dir: PathBuf,
        cx: &mut App,
    ) -> Result<()> {
        Self::init(
            PluginHostConfig {
                plugins_dir: plugins_dir.clone(),
                registry_cache_dir,
                db_path: None,
                secrets_vault_path: plugins_dir.join("safe-mode.vault"),
                secrets_key_path: plugins_dir.join("safe-mode.key"),
                safe_mode: true,
                locale: "en".to_owned(),
                theme_appearance: "light".to_owned(),
                hot_reload: false,
            },
            cx,
        )
    }

    /// Live-toggles Safe Mode, spawning or shutting down [`PluginManager`] without requiring restart.
    pub fn set_safe_mode(safe_mode: bool, cx: &mut App) -> Result<()> {
        use anyhow::Context as _;
        use theme::ActiveTheme as _;
        let mut next_cfg = cx
            .try_global::<Self>()
            .map(|h| h.config.clone())
            .context("plugin host is not initialized")?;
        if next_cfg.safe_mode == safe_mode {
            return Ok(());
        }
        next_cfg.safe_mode = safe_mode;
        next_cfg.theme_appearance = if cx.theme().is_dark() {
            "dark".to_owned()
        } else {
            "light".to_owned()
        };
        if let Some(loc) = cx.try_global::<fern_i18n::CurrentLocale>() {
            next_cfg.locale = loc.0.code().to_owned();
        }
        Self::init(next_cfg, cx)?;
        if !safe_mode
            && let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone())
        {
            cx.background_spawn(async move {
                let _ = mgr.emit_app_ready();
            })
            .detach();
        }
        Ok(())
    }

    /// Shuts down the active [`PluginManager`] and stops async host update tasks.
    pub fn shutdown(cx: &mut App) {
        dismiss_active_modal_on_shutdown(cx);
        PluginHooksGlobal::clear_global(cx);
        if cx.has_global::<Self>() {
            cx.update_global::<Self, _>(|host, _| {
                host.shutdown_inner();
            });
        }
    }

    fn shutdown_inner(&mut self) {
        self.hooks_active.store(false, Ordering::Release);
        self.wake_signal.close();
        if let Some(mgr) = self.manager.take() {
            mgr.shutdown();
        }
    }

    pub(crate) fn secrets_paths(&self) -> (PathBuf, PathBuf) {
        (
            self.config.secrets_vault_path.clone(),
            self.config.secrets_key_path.clone(),
        )
    }

    pub fn global(cx: &App) -> Option<&Self> {
        cx.try_global::<Self>()
    }

    pub fn manager(&self) -> Option<&PluginManager> {
        self.manager.as_ref()
    }

    pub fn registry_client(&self) -> &RegistryClient {
        &self.registry_client
    }

    pub fn plugins_dir(&self) -> &PathBuf {
        &self.plugins_dir
    }

    pub fn is_safe_mode(&self) -> bool {
        self.safe_mode
    }

    pub fn plugins(&self) -> &[PluginInfo] {
        &self.plugins
    }

    pub(crate) fn plugins_arc(&self) -> Arc<Vec<PluginInfo>> {
        Arc::clone(&self.plugins)
    }

    pub fn plugin(&self, plugin_id: &str) -> Option<&PluginInfo> {
        self.plugins.iter().find(|p| p.manifest.id == plugin_id)
    }

    pub fn plugin_revision(&self, plugin_id: &str) -> u64 {
        self.plugin_revisions.get(plugin_id).copied().unwrap_or(0)
    }

    pub(crate) fn view_revision(&self, plugin_id: &str, view_id: &str) -> Option<u64> {
        self.view_revisions
            .get(&(plugin_id.to_owned(), view_id.to_owned()))
            .copied()
    }

    pub(crate) fn is_view_mounting(&self, plugin_id: &str, view_id: &str) -> bool {
        self.op_state
            .borrow()
            .mounting_views
            .contains(&(plugin_id.to_owned(), view_id.to_owned()))
    }

    pub(crate) fn modal_generation(&self) -> u64 {
        self.modal_generation.load(Ordering::Acquire)
    }

    pub(crate) fn bump_modal_generation(cx: &App) {
        if let Some(host) = cx.try_global::<Self>() {
            host.modal_generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub(crate) fn apply_local_settings(
        plugin_id: &str,
        settings: serde_json::Map<String, serde_json::Value>,
        cx: &mut App,
    ) {
        if !cx.has_global::<Self>() {
            return;
        }
        let pid = plugin_id.to_owned();
        cx.update_global::<Self, _>(|host, _| {
            if let Some(info) = Arc::make_mut(&mut host.plugins)
                .iter_mut()
                .find(|p| p.manifest.id == pid)
            {
                info.settings_values = settings;
            }
        });
    }

    pub fn view_tree(&self, plugin_id: &str, view_id: &str) -> Option<&UiTreePayload> {
        self.views
            .get(&(plugin_id.to_owned(), view_id.to_owned()))
            .map(Arc::as_ref)
    }

    pub fn view_tree_arc(&self, plugin_id: &str, view_id: &str) -> Option<Arc<UiTreePayload>> {
        self.views
            .get(&(plugin_id.to_owned(), view_id.to_owned()))
            .cloned()
    }

    pub fn view_error(&self, plugin_id: &str, view_id: &str) -> Option<&str> {
        self.view_errors
            .get(&(plugin_id.to_owned(), view_id.to_owned()))
            .map(String::as_str)
    }

    pub fn plugin_theme_errors(&self) -> &[String] {
        &self.plugin_theme_errors
    }

    pub fn set_view_tree(
        plugin_id: impl Into<String>,
        view_id: impl Into<String>,
        payload: UiTreePayload,
        cx: &mut App,
    ) {
        if !cx.has_global::<Self>() {
            return;
        }
        let key = (plugin_id.into(), view_id.into());
        cx.update_global::<Self, _>(|host, _| {
            let rev = host.plugin_revision(&key.0);
            host.view_errors.remove(&key);
            host.views.insert(key.clone(), Arc::new(payload));
            host.view_revisions.insert(key, rev);
        });
    }

    pub fn set_view_error(
        plugin_id: impl Into<String>,
        view_id: impl Into<String>,
        error: String,
        cx: &mut App,
    ) {
        if !cx.has_global::<Self>() {
            return;
        }
        let key = (plugin_id.into(), view_id.into());
        cx.update_global::<Self, _>(|host, _| {
            host.view_errors.insert(key, error);
        });
    }

    pub fn clear_view_error(plugin_id: &str, view_id: &str, cx: &mut App) {
        let should_clear = cx.try_global::<Self>().is_some_and(|host| {
            host.view_errors
                .iter()
                .any(|((p, v), _)| p == plugin_id && v == view_id)
        });
        if !should_clear {
            return;
        }
        let key = (plugin_id.to_owned(), view_id.to_owned());
        cx.update_global::<Self, _>(|host, _| {
            host.view_errors.remove(&key);
        });
    }

    /// Requests that [`crate::PluginSettingsView`] open directly to `plugin_id`'s logs console.
    pub fn request_open_logs_for(plugin_id: impl Into<String>, cx: &mut App) {
        let plugin_id = plugin_id.into();
        if cx.has_global::<Self>() {
            cx.update_global::<Self, _>(|host, _| {
                host.requested_settings_plugin = Some(plugin_id);
            });
        }
        PluginBus::update(cx, |bus| {
            bus.push_nav(PluginNavAction::OpenSettingsPlugins);
        });
    }

    /// Requests that [`crate::PluginSettingsView`] open directly to `plugin_id`'s settings form.
    pub fn request_open_settings_for(plugin_id: impl Into<String>, cx: &mut App) {
        let plugin_id = plugin_id.into();
        if cx.has_global::<Self>() {
            cx.update_global::<Self, _>(|host, _| {
                host.requested_settings_open = Some(plugin_id);
            });
        }
        PluginBus::update(cx, |bus| {
            bus.push_nav(PluginNavAction::OpenSettingsPlugins);
        });
    }

    pub fn take_requested_settings_open(cx: &mut App) -> Option<String> {
        let has_request = cx
            .try_global::<Self>()
            .is_some_and(|host| host.requested_settings_open.is_some());
        if !has_request {
            return None;
        }
        cx.update_global::<Self, _>(|host, _| host.requested_settings_open.take())
    }

    pub fn take_requested_settings_plugin(cx: &mut App) -> Option<String> {
        let has_request = cx
            .try_global::<Self>()
            .is_some_and(|host| host.requested_settings_plugin.is_some());
        if !has_request {
            return None;
        }
        cx.update_global::<Self, _>(|host, _| host.requested_settings_plugin.take())
    }

    /// Bridges UI/sync SQLite mutations (`DataVersion`) into `PluginManager::notify_outbox_dirty()`.
    pub fn notify_data_version_if_changed(last_seen: &mut u64, cx: &mut App) -> bool {
        let Some(host) = cx.try_global::<Self>() else {
            return false;
        };
        let current_seq = host.external_data_seq.load(Ordering::Acquire);
        if *last_seen != current_seq {
            *last_seen = current_seq;
            if host.notified_outbox_seq.load(Ordering::Acquire) != current_seq {
                if let Some(mgr) = &host.manager {
                    mgr.notify_outbox_dirty();
                }
                host.notified_outbox_seq
                    .store(current_seq, Ordering::Release);
            }
            return true;
        }
        false
    }

    /// Updates locale / appearance forwarded to the plugin runtime (`fern.system`).
    pub fn set_appearance(
        locale: Option<String>,
        theme_appearance: Option<String>,
        cx: &App,
    ) {
        if let Some(mgr) = Self::global(cx).and_then(|h| h.manager.as_ref()) {
            mgr.set_appearance(locale, theme_appearance);
        }
    }

    /// Starts an active-window hot-reload poller that pauses when no Fern window is focused,
    /// and stops completely when zero plugins are installed.
    pub fn start_hot_reload_poller(cx: &mut App) {
        let Some(host) = cx.try_global::<Self>() else {
            return;
        };
        if host.safe_mode || host.manager.is_none() {
            return;
        }
        if !host.hot_reload_enabled {
            cx.update_global::<Self, _>(|host, _| {
                host.hot_reload_enabled = true;
            });
        }
        Self::ensure_hot_reload_poller(cx);
    }

    fn ensure_hot_reload_poller(cx: &mut App) {
        let Some(host) = cx.try_global::<Self>() else {
            return;
        };
        if !host.hot_reload_enabled
            || host.safe_mode
            || host.manager.is_none()
            || host.hot_reload_started
            || host.plugins.is_empty()
        {
            return;
        }
        let signal = Arc::clone(&host.wake_signal);
        cx.update_global::<Self, _>(|host, _| {
            host.hot_reload_started = true;
        });

        cx.spawn(async move |cx| {
            let bg = cx.background_executor().clone();
            let mut seen_window = false;
            loop {
                bg.timer(HOT_RELOAD_POLL_INTERVAL).await;
                if signal.closed.load(Ordering::Acquire) {
                    break;
                }
                enum PollState {
                    Stop,
                    Idle,
                    Active(PluginManager, u64),
                }
                let state = cx.update(|cx| {
                    let has_windows = !cx.windows().is_empty();
                    if has_windows {
                        seen_window = true;
                    } else if seen_window {
                        return PollState::Stop;
                    }
                    let Some(host) = cx.try_global::<PluginHost>() else {
                        return PollState::Stop;
                    };
                    if !host.hot_reload_enabled || host.safe_mode || host.plugins.is_empty() {
                        return PollState::Stop;
                    }
                    let Some(mgr) = host.manager.clone() else {
                        return PollState::Stop;
                    };
                    if cx.active_window().is_none() {
                        PollState::Idle
                    } else {
                        PollState::Active(mgr, host.registry_change_seq)
                    }
                });

                let (mgr, seq_before) = match state {
                    PollState::Stop => break,
                    PollState::Idle => continue,
                    PollState::Active(mgr, seq) => (mgr, seq),
                };

                let changed = bg.spawn(async move { mgr.check_hot_reload() }).await;
                if signal.closed.load(Ordering::Acquire) {
                    break;
                }
                if let Ok(changed_ids) = changed
                    && !changed_ids.is_empty()
                {
                    cx.update(|cx| {
                        handle_hot_reload_changed(changed_ids, seq_before, &signal, cx);
                    });
                }
            }
            cx.update(|cx| {
                let matches_signal = cx
                    .try_global::<PluginHost>()
                    .is_some_and(|h| Arc::ptr_eq(&h.wake_signal, &signal) && h.hot_reload_started);
                if matches_signal {
                    cx.update_global::<PluginHost, _>(|host, _| {
                        host.hot_reload_started = false;
                    });
                }
            });
        })
        .detach();
    }

    /// Triggers an immediate off-thread hot-reload check.
    pub fn check_hot_reload_now(cx: &mut App) {
        let Some((mgr, signal, seq_before)) = Self::global(cx).and_then(|h| {
            h.manager
                .clone()
                .map(|m| (m, Arc::clone(&h.wake_signal), h.registry_change_seq))
        }) else {
            return;
        };
        let task = cx.background_spawn(async move { mgr.check_hot_reload() });
        cx.spawn(async move |cx| {
            let changed = task.await;
            if signal.closed.load(Ordering::Acquire) {
                return;
            }
            if let Ok(changed_ids) = changed
                && !changed_ids.is_empty()
            {
                cx.update(|cx| {
                    handle_hot_reload_changed(changed_ids, seq_before, &signal, cx);
                });
            }
        })
        .detach();
    }

    fn enqueue_plugin_op(plugin_id: String, op: PluginViewOp, cx: &mut App) {
        let should_start = {
            let Some(host) = cx.try_global::<Self>() else {
                return;
            };
            if host.manager.is_none() {
                return;
            }
            let mut ops = host.op_state.borrow_mut();
            ops.queues
                .entry(plugin_id.clone())
                .or_default()
                .push_back(op);
            ops.running.insert(plugin_id.clone())
        };
        if !should_start {
            return;
        }

        cx.spawn(async move |cx| {
            let bg = cx.background_executor().clone();
            loop {
                let next = cx.update(|cx| {
                    let host = cx.try_global::<PluginHost>()?;
                    let mut ops = host.op_state.borrow_mut();
                    let Some(mgr) = host.manager.clone() else {
                        ops.queues.remove(&plugin_id);
                        ops.running.remove(&plugin_id);
                        return None;
                    };
                    let op = ops
                        .queues
                        .get_mut(&plugin_id)
                        .and_then(VecDeque::pop_front);
                    match op {
                        Some(op) => Some((op, mgr, Arc::clone(&host.wake_signal))),
                        None => {
                            ops.queues.remove(&plugin_id);
                            ops.running.remove(&plugin_id);
                            None
                        }
                    }
                });

                let Some((op, mgr, signal)) = next else {
                    break;
                };

                match op {
                    PluginViewOp::Mount {
                        view_id,
                        generation,
                    } => {
                        let pid = plugin_id.clone();
                        let vid = view_id.clone();
                        let res = bg.spawn(async move { mgr.mount_view(&pid, &vid) }).await;
                        if !signal.closed.load(Ordering::Acquire) {
                            cx.update(|cx| {
                                apply_view_render_result(
                                    &plugin_id, &view_id, generation, res, &signal, cx,
                                );
                            });
                        }
                    }
                    PluginViewOp::Render {
                        view_id,
                        generation,
                    } => {
                        let pid = plugin_id.clone();
                        let vid = view_id.clone();
                        let res = bg.spawn(async move { mgr.render_view(&pid, &vid) }).await;
                        if !signal.closed.load(Ordering::Acquire) {
                            cx.update(|cx| {
                                apply_view_render_result(
                                    &plugin_id, &view_id, generation, res, &signal, cx,
                                );
                            });
                        }
                    }
                    PluginViewOp::Unmount { view_id } => {
                        let pid = plugin_id.clone();
                        let _ = bg.spawn(async move { mgr.unmount_view(&pid, &view_id) }).await;
                    }
                    PluginViewOp::Reload => {
                        let pid = plugin_id.clone();
                        let res = bg.spawn(async move { mgr.reload_plugin(&pid) }).await;
                        if !signal.closed.load(Ordering::Acquire) {
                            cx.update(|cx| {
                                let matches = cx
                                    .try_global::<PluginHost>()
                                    .is_some_and(|h| Arc::ptr_eq(&h.wake_signal, &signal));
                                if matches {
                                    cx.update_global::<PluginHost, _>(|host, _| {
                                        host.loaded_theme_signature = None;
                                        host.views.retain(|(p, _), _| p != &plugin_id);
                                        host.view_errors.retain(|(p, _), _| p != &plugin_id);
                                        host.view_revisions.retain(|(p, _), _| p != &plugin_id);
                                        host.op_state
                                            .borrow_mut()
                                            .invalidate_plugin_views(&plugin_id);
                                        let rev = host
                                            .plugin_revisions
                                            .entry(plugin_id.clone())
                                            .or_insert(0);
                                        *rev = rev.wrapping_add(1);
                                        if let Err(err) = res {
                                            host.view_errors.insert(
                                                (plugin_id.clone(), String::new()),
                                                format!("{err:#}"),
                                            );
                                        }
                                    });
                                }
                            });
                        }
                    }
                    PluginViewOp::ReloadAndMount {
                        view_id,
                        generation,
                    } => {
                        cx.update(|cx| {
                            if let Some(host) = cx.try_global::<PluginHost>() {
                                host.op_state
                                    .borrow_mut()
                                    .in_flight_reload_or_reset
                                    .insert(plugin_id.clone());
                            }
                        });
                        let pid = plugin_id.clone();
                        let vid = view_id.clone();
                        let res = bg
                            .spawn(async move {
                                match mgr.reload_plugin(&pid) {
                                    Ok(()) => Ok(mgr.mount_view(&pid, &vid)),
                                    Err(err) => Err(err),
                                }
                            })
                            .await;
                        if !signal.closed.load(Ordering::Acquire) {
                            cx.update(|cx| {
                                let matches = cx
                                    .try_global::<PluginHost>()
                                    .is_some_and(|h| Arc::ptr_eq(&h.wake_signal, &signal));
                                if !matches {
                                    return;
                                }
                                cx.update_global::<PluginHost, _>(|host, _| {
                                    host.loaded_theme_signature = None;
                                    host.views.retain(|(p, _), _| p != &plugin_id);
                                    host.view_errors.retain(|(p, _), _| p != &plugin_id);
                                    host.view_revisions.retain(|(p, _), _| p != &plugin_id);
                                    let rev = host
                                        .plugin_revisions
                                        .entry(plugin_id.clone())
                                        .or_insert(0);
                                    *rev = rev.wrapping_add(1);
                                    let rev_val = *rev;
                                    let key = (plugin_id.clone(), view_id.clone());
                                    let mut ops = host.op_state.borrow_mut();
                                    ops.in_flight_reload_or_reset.remove(&plugin_id);
                                    let is_current =
                                        ops.view_generations.get(&key) == Some(&generation);
                                    if is_current {
                                        ops.mounting_views.remove(&key);
                                    }
                                    drop(ops);
                                    match res {
                                        Err(reload_err) => {
                                            host.view_errors.insert(
                                                (plugin_id.clone(), String::new()),
                                                format!("{reload_err:#}"),
                                            );
                                        }
                                        Ok(mount_res) => {
                                            if is_current {
                                                host.view_revisions.insert(key.clone(), rev_val);
                                                match mount_res {
                                                    Ok(payload) => {
                                                        host.views.insert(key, Arc::new(payload));
                                                    }
                                                    Err(mount_err) => {
                                                        host.view_errors.insert(
                                                            key,
                                                            format!("{mount_err:#}"),
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                });
                            });
                        }
                    }
                    PluginViewOp::ResetAndMount {
                        view_id,
                        generation,
                    } => {
                        cx.update(|cx| {
                            if let Some(host) = cx.try_global::<PluginHost>() {
                                host.op_state
                                    .borrow_mut()
                                    .in_flight_reload_or_reset
                                    .insert(plugin_id.clone());
                            }
                        });
                        let pid = plugin_id.clone();
                        let vid = view_id.clone();
                        let res = bg
                            .spawn(async move {
                                mgr.reset_and_enable_plugin(&pid)?;
                                mgr.mount_view(&pid, &vid)
                            })
                            .await;
                        if !signal.closed.load(Ordering::Acquire) {
                            cx.update(|cx| {
                                if let Some(host) = cx.try_global::<PluginHost>()
                                    && Arc::ptr_eq(&host.wake_signal, &signal)
                                {
                                    host.op_state
                                        .borrow_mut()
                                        .in_flight_reload_or_reset
                                        .remove(&plugin_id);
                                }
                                apply_view_render_result(
                                    &plugin_id, &view_id, generation, res, &signal, cx,
                                );
                            });
                        }
                    }
                }
            }
        })
        .detach();
    }

    /// Mounts and renders a plugin sidebar view off the UI thread, updating `PluginHost` state.
    pub fn mount_view_async(plugin_id: String, view_id: String, cx: &mut App) {
        Self::request_view_async(plugin_id, view_id, true, cx);
    }

    /// Re-renders an already mounted plugin view off the UI thread.
    pub fn render_view_async(plugin_id: String, view_id: String, cx: &mut App) {
        Self::request_view_async(plugin_id, view_id, false, cx);
    }

    fn request_view_async(plugin_id: String, view_id: String, mount: bool, cx: &mut App) {
        let generation = {
            let Some(host) = cx.try_global::<Self>() else {
                return;
            };
            if host.manager.is_none() {
                return;
            }
            let mut ops = host.op_state.borrow_mut();
            let generation = ops.next_view_gen(&plugin_id, &view_id);
            if mount {
                ops.mounting_views
                    .insert((plugin_id.clone(), view_id.clone()));
            }
            generation
        };
        let op = if mount {
            PluginViewOp::Mount {
                view_id,
                generation,
            }
        } else {
            PluginViewOp::Render {
                view_id,
                generation,
            }
        };
        Self::enqueue_plugin_op(plugin_id, op, cx);
    }

    /// Unmounts a plugin view off the UI thread and evicts its cached tree immediately.
    pub fn unmount_view_async(plugin_id: String, view_id: String, cx: &mut App) {
        let key = (plugin_id.clone(), view_id.clone());
        let had_cached = cx.try_global::<Self>().is_some_and(|h| {
            h.views.contains_key(&key)
                || h.view_errors.contains_key(&key)
                || h.view_revisions.contains_key(&key)
        });
        if had_cached {
            cx.update_global::<Self, _>(|host, _| {
                host.views.remove(&key);
                host.view_errors.remove(&key);
                host.view_revisions.remove(&key);
            });
        }
        let has_mgr = if let Some(host) = cx.try_global::<Self>() {
            let mut ops = host.op_state.borrow_mut();
            let _ = ops.next_view_gen(&plugin_id, &view_id);
            ops.mounting_views.remove(&key);
            host.manager.is_some()
        } else {
            false
        };
        if has_mgr {
            Self::enqueue_plugin_op(plugin_id, PluginViewOp::Unmount { view_id }, cx);
        }
    }

    /// Sequentially reloads a plugin and mounts `view_id` in a single queued task (no race or double-mount).
    pub fn reload_and_mount_view_async(plugin_id: String, view_id: String, cx: &mut App) {
        let key = (plugin_id.clone(), view_id.clone());
        let generation = {
            let Some(host) = cx.try_global::<Self>() else {
                return;
            };
            if host.manager.is_none() {
                return;
            }
            let mut ops = host.op_state.borrow_mut();
            let generation = ops.next_view_gen(&plugin_id, &view_id);
            ops.mounting_views.insert(key.clone());
            generation
        };
        cx.update_global::<Self, _>(|host, _| {
            host.views.remove(&key);
            host.view_errors.retain(|(p, _), _| p != &plugin_id);
            host.view_revisions.remove(&key);
        });
        Self::enqueue_plugin_op(
            plugin_id,
            PluginViewOp::ReloadAndMount {
                view_id,
                generation,
            },
            cx,
        );
    }

    /// Sequentially resets a circuit-broken plugin and mounts `view_id` without racing or double-mounting.
    pub fn reset_and_mount_view_async(plugin_id: String, view_id: String, cx: &mut App) {
        let key = (plugin_id.clone(), view_id.clone());
        let generation = {
            let Some(host) = cx.try_global::<Self>() else {
                return;
            };
            if host.manager.is_none() {
                return;
            }
            let mut ops = host.op_state.borrow_mut();
            let generation = ops.next_view_gen(&plugin_id, &view_id);
            ops.mounting_views.insert(key);
            generation
        };
        cx.update_global::<Self, _>(|host, _| {
            host.view_errors.retain(|(p, _), _| p != &plugin_id);
        });
        Self::enqueue_plugin_op(
            plugin_id,
            PluginViewOp::ResetAndMount {
                view_id,
                generation,
            },
            cx,
        );
    }

    /// Dispatches an interactive `UiEvent` from a native widget back to QuickJS off-thread.
    pub fn dispatch_ui_event_async(plugin_id: String, event: UiEvent, cx: &mut App) {
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            return;
        };
        let pid = plugin_id.clone();
        let vid = event.view_id.clone();
        let task = cx.background_spawn(async move { mgr.dispatch_ui_event(&pid, event) });
        cx.spawn(async move |cx| {
            let res = task.await;
            cx.update(|cx| match res {
                Ok(Some(payload)) => Self::set_view_tree(plugin_id, vid, payload, cx),
                Ok(None) => {
                    if vid == "__modal__" {
                        Self::clear_view_error(&plugin_id, &vid, cx);
                    }
                }
                Err(err) => {
                    let msg = format!("{err:#}");
                    if vid == "__modal__" {
                        let modal_open = PluginBus::global(cx)
                            .and_then(|b| b.active_modal.as_ref())
                            .is_some_and(|m| m.plugin_id == plugin_id && m.view_id == vid);
                        if modal_open {
                            Self::set_view_error(plugin_id, vid, msg, cx);
                        } else {
                            push_toast_capped(plugin_id, msg, BusToastLevel::Error, now_ms(), cx);
                        }
                    } else {
                        Self::set_view_error(plugin_id, vid, msg, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Resolves a pending `fern.ui.confirm` / `fern.ui.prompt` JS promise off-thread.
    pub fn resolve_prompt_async(request_id: u64, reply: PromptReplyValue, cx: &mut App) {
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            return;
        };
        cx.background_spawn(async move {
            let _ = mgr.resolve_prompt(request_id, reply);
        })
        .detach();
    }

    /// Executes a contributed plugin command off the UI thread.
    pub fn run_command_async(
        plugin_id: String,
        command_id: String,
        arg: Option<String>,
        cx: &mut App,
    ) {
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            return;
        };
        let pid = plugin_id.clone();
        let task = cx.background_spawn(async move { mgr.run_command(&pid, &command_id, arg) });
        cx.spawn(async move |cx| {
            if let Err(err) = task.await {
                let msg = format!("{err:#}");
                cx.update(|cx| {
                    push_toast_capped(plugin_id, msg, BusToastLevel::Error, now_ms(), cx);
                });
            }
        })
        .detach();
    }

    /// Queries enabled plugin search providers off-thread and updates `PluginBus::search_hits`.
    pub fn search_async(query: String, cx: &mut App) {
        let seq = Self::global(cx).map_or(0, |h| {
            h.search_seq.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
        });
        if query.trim().is_empty() {
            PluginBus::update(cx, |bus| bus.search_hits.clear());
            return;
        }
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            PluginBus::update(cx, |bus| bus.search_hits.clear());
            return;
        };
        let task = cx.background_spawn(async move { mgr.search(&query) });
        cx.spawn(async move |cx| {
            let res = task.await;
            cx.update(|cx| {
                let is_latest =
                    Self::global(cx).is_some_and(|h| h.search_seq.load(Ordering::Acquire) == seq);
                if !is_latest {
                    return;
                }
                match res {
                    Ok(hits) => {
                        let mapped: Vec<PluginSearchHit> = hits
                            .into_iter()
                            .map(|h| PluginSearchHit {
                                id: h.id,
                                plugin_id: h.plugin_id,
                                provider_id: h.provider_id,
                                title: h.title,
                                subtitle: h.subtitle,
                                badge: h.badge,
                                task_id: h.task_id,
                                command_id: h.command_id,
                                command_arg: h.command_arg,
                            })
                            .collect();
                        PluginBus::update(cx, |bus| {
                            bus.search_hits = mapped;
                        });
                    }
                    Err(_) => {
                        PluginBus::update(cx, |bus| bus.search_hits.clear());
                    }
                }
            });
        })
        .detach();
    }

    /// Reloads a single plugin off the UI thread and bumps its view revision once reload finishes.
    pub fn reload_plugin_async(plugin_id: String, cx: &mut App) {
        if Self::global(cx).and_then(|h| h.manager.as_ref()).is_none() {
            return;
        }
        Self::enqueue_plugin_op(plugin_id, PluginViewOp::Reload, cx);
    }

    /// Reloads all plugins from disk off the UI thread.
    pub fn reload_all_async(cx: &mut App) {
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            return;
        };
        if cx.has_global::<Self>() {
            cx.update_global::<Self, _>(|host, _| {
                host.loaded_theme_signature = None;
            });
        }
        let task = cx.background_spawn(async move { mgr.reload_all() });
        cx.spawn(async move |cx| {
            if let Ok(infos) = task.await {
                cx.update(|cx| {
                    apply_registry_changed(infos, cx);
                });
            }
        })
        .detach();
    }

    /// Loads `themes` contributions from all enabled plugins into [`ThemeRegistry`],
    /// preserving any user-defined custom themes already loaded from `<data_dir>/themes`.
    pub fn sync_plugin_themes(cx: &mut App) {
        if !cx.has_global::<ThemeRegistry>() {
            return;
        }
        let Some(host) = cx.try_global::<Self>() else {
            return;
        };
        let sig = theme_contribution_signature(&host.plugins);
        let (plugin_themes, errors) = if host.loaded_theme_signature.as_ref() == Some(&sig) {
            (
                host.cached_plugin_themes.clone(),
                host.plugin_theme_errors.clone(),
            )
        } else if sig.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let plugins = Arc::clone(&host.plugins);
            collect_plugin_themes(&plugins)
        };
        // Invalidate any in-flight async theme load so it cannot overwrite this synchronous sync.
        host.theme_sync_seq.fetch_add(1, Ordering::AcqRel);
        apply_plugin_themes(sig, plugin_themes, errors, cx);
    }
}

fn theme_contribution_signature(plugins: &[PluginInfo]) -> ThemeSourceSignature {
    plugins
        .iter()
        .filter(|p| p.enabled && !p.manifest.contributions.themes.is_empty())
        .map(|p| (p.manifest.id.clone(), p.dir.clone(), p.manifest_hash.clone()))
        .collect()
}

fn apply_plugin_themes(
    signature: ThemeSourceSignature,
    plugin_themes: Vec<Theme>,
    errors: Vec<String>,
    cx: &mut App,
) {
    if !cx.has_global::<ThemeRegistry>() {
        return;
    }
    let Some(host) = cx.try_global::<PluginHost>() else {
        return;
    };
    let prev_plugin_ids = host.loaded_plugin_theme_ids.clone();
    let prev_errors = host.plugin_theme_errors.clone();

    let next_plugin_ids: HashSet<String> =
        plugin_themes.iter().map(|t| t.id().to_owned()).collect();

    let registry = ThemeRegistry::global(cx);
    let mut combined: Vec<Theme> = registry
        .custom_themes()
        .iter()
        .filter(|t| !prev_plugin_ids.contains(t.id()))
        .map(|t| (**t).clone())
        .collect();
    combined.extend(plugin_themes.clone());

    let mut combined_errors: Vec<String> = registry
        .errors()
        .iter()
        .filter(|err| !prev_errors.contains(err))
        .cloned()
        .collect();
    combined_errors.extend(errors.clone());

    theme::reload_user_themes(combined, combined_errors, cx);
    cx.update_global::<PluginHost, _>(|host, _| {
        host.loaded_plugin_theme_ids = next_plugin_ids;
        host.plugin_theme_errors = errors;
        host.loaded_theme_signature = Some(signature);
        host.cached_plugin_themes = plugin_themes;
    });
}

fn sync_plugin_themes_async(
    plugins: Arc<Vec<PluginInfo>>,
    sig: ThemeSourceSignature,
    cx: &mut App,
) {
    if !cx.has_global::<ThemeRegistry>() {
        return;
    }
    if sig.is_empty() {
        PluginHost::sync_plugin_themes(cx);
        return;
    }
    let Some((seq, signal)) = cx.try_global::<PluginHost>().map(|h| {
        (
            h.theme_sync_seq
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1),
            Arc::clone(&h.wake_signal),
        )
    }) else {
        return;
    };

    let task = cx.background_spawn(async move { collect_plugin_themes(&plugins) });
    cx.spawn(async move |cx| {
        let (themes, errors) = task.await;
        if signal.closed.load(Ordering::Acquire) {
            return;
        }
        cx.update(|cx| {
            let is_latest = cx.try_global::<PluginHost>().is_some_and(|h| {
                Arc::ptr_eq(&h.wake_signal, &signal)
                    && h.theme_sync_seq.load(Ordering::Acquire) == seq
            });
            if is_latest {
                apply_plugin_themes(sig, themes, errors, cx);
            }
        });
    })
    .detach();
}

fn apply_view_render_result(
    plugin_id: &str,
    view_id: &str,
    generation: u64,
    res: Result<UiTreePayload>,
    signal: &Arc<WakeSignal>,
    cx: &mut App,
) {
    let key = (plugin_id.to_owned(), view_id.to_owned());
    let is_current = cx.try_global::<PluginHost>().is_some_and(|host| {
        if !Arc::ptr_eq(&host.wake_signal, signal) {
            return false;
        }
        let mut ops = host.op_state.borrow_mut();
        if ops.view_generations.get(&key) == Some(&generation) {
            ops.mounting_views.remove(&key);
            true
        } else {
            false
        }
    });
    if !is_current {
        return;
    }
    match res {
        Ok(payload) => PluginHost::set_view_tree(plugin_id, view_id, payload, cx),
        Err(err) => {
            cx.update_global::<PluginHost, _>(|host, _| {
                let rev = host.plugin_revision(&key.0);
                host.view_revisions.insert(key.clone(), rev);
                host.view_errors.insert(key, format!("{err:#}"));
            });
        }
    }
}

fn handle_hot_reload_changed(
    changed_ids: Vec<String>,
    seq_before: u64,
    signal: &Arc<WakeSignal>,
    cx: &mut App,
) {
    let matches = cx
        .try_global::<PluginHost>()
        .is_some_and(|h| Arc::ptr_eq(&h.wake_signal, signal));
    if !matches {
        return;
    }
    cx.update_global::<PluginHost, _>(|host, _| {
        host.loaded_theme_signature = None;
        let reload_already_delivered = host.registry_change_seq > seq_before;
        for id in &changed_ids {
            host.views.retain(|(p, _), _| p != id);
            host.view_errors.retain(|(p, _), _| p != id);
            host.view_revisions.retain(|(p, _), _| p != id);
            host.op_state.borrow_mut().invalidate_plugin_views(id);
            if reload_already_delivered {
                let entry = host.plugin_revisions.entry(id.clone()).or_insert(0);
                *entry = entry.wrapping_add(1);
            } else {
                // Worker spawned reload_single_plugin asynchronously; wait for its
                // RegistryChanged update before bumping plugin_revisions so views mount
                // against the NEW runtime rather than racing the old one.
                host.pending_hot_reload.insert(id.clone());
            }
        }
    });
}

/// Parses and validates all `themes` contributed by enabled plugins.
pub fn collect_plugin_themes(plugins: &[PluginInfo]) -> (Vec<Theme>, Vec<String>) {
    let mut themes = Vec::new();
    let mut errors = Vec::new();

    for info in plugins {
        if !info.enabled || info.manifest.contributions.themes.is_empty() {
            continue;
        }
        for contrib in &info.manifest.contributions.themes {
            if let Some(rel_path) = &contrib.path {
                match resolve_inside_dir(&info.dir, rel_path)
                    .and_then(|path| std::fs::read_to_string(&path).map_err(anyhow::Error::from))
                    .and_then(|raw| theme::load_theme_family_from_str(&raw))
                {
                    Ok((loaded, warnings)) => {
                        themes.extend(loaded);
                        for w in warnings {
                            errors.push(format!("{}: {w}", info.manifest.id));
                        }
                    }
                    Err(err) => {
                        errors.push(format!(
                            "{}: theme {:?} failed: {err:#}",
                            info.manifest.id, contrib.id
                        ));
                    }
                }
            } else if let Some(colors) = &contrib.colors {
                let doc = if colors.get("themes").is_some() || colors.get("style").is_some() {
                    colors.clone()
                } else {
                    serde_json::json!({
                        "id": contrib.id,
                        "name": contrib.name,
                        "appearance": contrib.appearance.as_deref().unwrap_or("dark"),
                        "author": info.manifest.author,
                        "style": colors,
                    })
                };
                match serde_json::to_string(&doc)
                    .map_err(anyhow::Error::from)
                    .and_then(|json| theme::load_theme_family_from_str(&json))
                {
                    Ok((loaded, warnings)) => {
                        themes.extend(loaded);
                        for w in warnings {
                            errors.push(format!("{}: {w}", info.manifest.id));
                        }
                    }
                    Err(err) => {
                        errors.push(format!(
                            "{}: inline theme {:?} failed: {err:#}",
                            info.manifest.id, contrib.id
                        ));
                    }
                }
            }
        }
    }

    (themes, errors)
}

#[cfg(not(test))]
fn spawn_update_consumer(
    rx: mpsc::Receiver<HostUpdate>,
    signal: Arc<WakeSignal>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        loop {
            let mut batch = Vec::new();
            let mut disconnected = false;
            loop {
                match rx.try_recv() {
                    Ok(update) => batch.push(update),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            if !batch.is_empty() {
                cx.update(|cx| {
                    for update in batch {
                        apply_host_update(update, cx);
                    }
                });
            }

            if disconnected || !WaitForWake(Arc::clone(&signal)).await {
                break;
            }
        }
    })
    .detach();
}

fn apply_host_update(update: HostUpdate, cx: &mut App) {
    match update {
        HostUpdate::RegistryChanged(infos) => {
            apply_registry_changed(infos, cx);
        }
        HostUpdate::PluginStatusChanged {
            plugin_id,
            status,
            metrics,
        } => {
            let mut updated_plugins = None;
            let mut should_sync_themes = false;
            if cx.has_global::<PluginHost>() {
                cx.update_global::<PluginHost, _>(|host, _| {
                    let deactivated = matches!(
                        status,
                        PluginStatus::Disabled
                            | PluginStatus::NeedsApproval { .. }
                            | PluginStatus::CircuitBroken { .. }
                    );
                    if let Some(info) = Arc::make_mut(&mut host.plugins)
                        .iter_mut()
                        .find(|p| p.manifest.id == plugin_id)
                    {
                        if deactivated {
                            if info.enabled && !info.manifest.contributions.themes.is_empty() {
                                should_sync_themes = true;
                            }
                            info.enabled = false;
                        }
                        info.status = status.clone();
                        info.metrics = metrics;
                    }
                    if deactivated {
                        host.views.retain(|(p, _), _| p != &plugin_id);
                        host.view_revisions.retain(|(p, _), _| p != &plugin_id);
                        host.op_state.borrow_mut().invalidate_plugin_views(&plugin_id);
                    }
                    if host.pending_hot_reload.remove(&plugin_id) {
                        host.views.retain(|(p, _), _| p != &plugin_id);
                        host.view_revisions.retain(|(p, _), _| p != &plugin_id);
                        host.op_state.borrow_mut().invalidate_plugin_views(&plugin_id);
                        let rev = host
                            .plugin_revisions
                            .entry(plugin_id.clone())
                            .or_insert(0);
                        *rev = rev.wrapping_add(1);
                    }
                    updated_plugins = Some(Arc::clone(&host.plugins));
                });
            }
            if let Some(plugins) = updated_plugins {
                sync_bus_contributions(&plugins, cx);
                if should_sync_themes {
                    let sig = theme_contribution_signature(&plugins);
                    sync_plugin_themes_async(plugins, sig, cx);
                }
                PluginHost::ensure_hot_reload_poller(cx);
            }
        }
        HostUpdate::ViewRendered(payload) => {
            PluginHost::set_view_tree(
                payload.plugin_id.clone(),
                payload.view_id.clone(),
                payload,
                cx,
            );
        }
        HostUpdate::ViewError {
            plugin_id,
            view_id,
            error,
        } => {
            PluginHost::set_view_error(plugin_id, view_id, error, cx);
        }
        HostUpdate::UiEffect(effect) => apply_ui_effect(effect, cx),
        HostUpdate::DataChanged => {
            if let Some(host) = cx.try_global::<PluginHost>() {
                let marker_id = host.marker_id;
                data_changed(marker_id, cx);
            }
        }
    }
}

fn apply_registry_changed(infos: Vec<PluginInfo>, cx: &mut App) {
    let infos = Arc::new(infos);
    let next_theme_sig = theme_contribution_signature(&infos);
    let mut should_sync_themes = false;

    if cx.has_global::<PluginHost>() {
        cx.update_global::<PluginHost, _>(|host, _| {
            host.registry_change_seq = host.registry_change_seq.wrapping_add(1);
            for info in infos.iter() {
                let prev = host
                    .plugins
                    .iter()
                    .find(|p| p.manifest.id == info.manifest.id);
                let prev_enabled = prev.is_some_and(|p| p.enabled);
                let prev_hash = prev.map(|p| p.manifest_hash.as_str());
                let was_hot_reloaded = host.pending_hot_reload.remove(&info.manifest.id);
                let changed = (!prev_enabled && info.enabled)
                    || prev_hash != Some(info.manifest_hash.as_str())
                    || was_hot_reloaded;
                let reloading_or_resetting = host
                    .op_state
                    .borrow()
                    .in_flight_reload_or_reset
                    .contains(&info.manifest.id);
                if changed && !reloading_or_resetting {
                    host.views.retain(|(pid, _), _| pid != &info.manifest.id);
                    host.view_revisions
                        .retain(|(pid, _), _| pid != &info.manifest.id);
                    host.op_state
                        .borrow_mut()
                        .invalidate_plugin_views(&info.manifest.id);
                }
                if changed || !host.plugin_revisions.contains_key(&info.manifest.id) {
                    let rev = host
                        .plugin_revisions
                        .entry(info.manifest.id.clone())
                        .or_insert(0);
                    *rev = rev.wrapping_add(1);
                }
                let inactive = !info.enabled
                    || matches!(
                        info.status,
                        PluginStatus::Disabled
                            | PluginStatus::NeedsApproval { .. }
                            | PluginStatus::CircuitBroken { .. }
                    );
                if inactive {
                    host.views.retain(|(pid, _), _| pid != &info.manifest.id);
                    host.view_errors
                        .retain(|(pid, _), _| pid != &info.manifest.id);
                    host.view_revisions
                        .retain(|(pid, _), _| pid != &info.manifest.id);
                    host.op_state
                        .borrow_mut()
                        .invalidate_plugin_views(&info.manifest.id);
                }
            }
            let alive: HashSet<&str> = infos.iter().map(|p| p.manifest.id.as_str()).collect();
            host.views.retain(|(pid, _), _| alive.contains(pid.as_str()));
            host.view_errors
                .retain(|(pid, _), _| alive.contains(pid.as_str()));
            host.view_revisions
                .retain(|(pid, _), _| alive.contains(pid.as_str()));
            host.pending_hot_reload
                .retain(|pid| alive.contains(pid.as_str()));
            should_sync_themes = host.loaded_theme_signature.as_ref() != Some(&next_theme_sig);
            host.hooks_active.store(
                has_active_hook_plugins(host.safe_mode, host.manager.is_some(), &infos),
                Ordering::Release,
            );
            host.plugins = Arc::clone(&infos);
        });
    }
    sync_bus_contributions(&infos, cx);
    if should_sync_themes {
        sync_plugin_themes_async(infos, next_theme_sig, cx);
    }
    PluginHost::ensure_hot_reload_poller(cx);
}

fn sync_bus_contributions(plugins: &[PluginInfo], cx: &mut App) {
    let mut sidebar_views = Vec::new();
    let mut commands = Vec::new();

    for info in plugins {
        if !info.enabled
            || matches!(
                info.status,
                PluginStatus::Disabled
                    | PluginStatus::NeedsApproval { .. }
                    | PluginStatus::CircuitBroken { .. }
            )
        {
            continue;
        }
        for view in &info.manifest.contributions.sidebar {
            sidebar_views.push(PluginSidebarViewSpec {
                plugin_id: info.manifest.id.clone(),
                view_id: view.id.clone(),
                title: view.title.clone(),
                icon_svg: info
                    .sidebar_icons
                    .get(&view.id)
                    .map(|svg| Arc::<str>::from(svg.as_str())),
            });
        }
        for cmd in &info.manifest.contributions.commands {
            commands.push(PluginCommandSpec {
                plugin_id: info.manifest.id.clone(),
                command_id: cmd.id.clone(),
                title: cmd.title.clone(),
                plugin_name: info.manifest.name.clone(),
            });
        }
    }

    PluginBus::update(cx, |bus| {
        bus.sidebar_views = sidebar_views;
        bus.commands = commands;
    });
}

/// Resolves a displaced or closed `Confirm`/`Prompt` modal so awaited JS Promises never hang,
/// and cleans up `"__modal__"` scope handlers for displaced `View` modals.
pub(crate) fn resolve_displaced_modal(
    prev: PluginModalSpec,
    replaced_by: Option<(&str, &str, bool)>,
    cx: &mut App,
) {
    match prev.kind {
        PluginModalKind::Confirm { request_id, .. } => {
            PluginHost::resolve_prompt_async(request_id, PromptReplyValue::Confirm(false), cx);
        }
        PluginModalKind::Prompt { request_id, .. } => {
            PluginHost::resolve_prompt_async(request_id, PromptReplyValue::Prompt(None), cx);
        }
        PluginModalKind::View { .. } => {
            PluginHost::clear_view_error(&prev.plugin_id, &prev.view_id, cx);
            let same_view_replacement = replaced_by.is_some_and(|(pid, vid, is_view)| {
                is_view && pid == prev.plugin_id && vid == prev.view_id
            });
            if !same_view_replacement {
                PluginHost::unmount_view_async(prev.plugin_id, prev.view_id, cx);
            }
        }
    }
}

fn dismiss_active_modal_on_shutdown(cx: &mut App) {
    let has_modal = PluginBus::global(cx).is_some_and(|bus| bus.active_modal.is_some());
    if !has_modal {
        return;
    }
    let prev = PluginBus::update(cx, |bus| bus.active_modal.take());
    PluginHost::bump_modal_generation(cx);
    if let Some(modal) = prev {
        match modal.kind {
            PluginModalKind::Confirm { request_id, .. } => {
                PluginHost::resolve_prompt_async(request_id, PromptReplyValue::Confirm(false), cx);
            }
            PluginModalKind::Prompt { request_id, .. } => {
                PluginHost::resolve_prompt_async(request_id, PromptReplyValue::Prompt(None), cx);
            }
            PluginModalKind::View { .. } => {
                PluginHost::clear_view_error(&modal.plugin_id, &modal.view_id, cx);
            }
        }
    }
}

pub(crate) fn push_toast_capped(
    plugin_id: String,
    message: String,
    level: BusToastLevel,
    created_at_ms: i64,
    cx: &mut App,
) -> u64 {
    let toast_id = PluginBus::update(cx, |bus| {
        while bus.toasts.len() >= MAX_PENDING_TOASTS {
            bus.toasts.remove(0);
        }
        bus.push_toast(plugin_id, message, level, created_at_ms)
    });
    schedule_toast_expiry(toast_id, cx);
    toast_id
}

pub(crate) fn apply_ui_effect(effect: PluginUiEffect, cx: &mut App) {
    match effect {
        PluginUiEffect::Toast {
            plugin_id,
            message,
            level,
            created_at_ms,
        } => {
            let bus_level = match level {
                PluginToastLevel::Info => BusToastLevel::Info,
                PluginToastLevel::Success => BusToastLevel::Success,
                PluginToastLevel::Warning => BusToastLevel::Warning,
                PluginToastLevel::Error => BusToastLevel::Error,
            };
            push_toast_capped(plugin_id, message, bus_level, created_at_ms, cx);
        }
        PluginUiEffect::Confirm {
            request_id,
            plugin_id,
            title,
            body,
        } => {
            let pid_ref = plugin_id.clone();
            let prev = PluginBus::update(cx, |bus| {
                let old = bus.active_modal.take();
                bus.open_modal(PluginModalSpec {
                    plugin_id,
                    view_id: "__confirm__".to_owned(),
                    title,
                    kind: PluginModalKind::Confirm { body, request_id },
                });
                old
            });
            PluginHost::bump_modal_generation(cx);
            if let Some(old) = prev {
                resolve_displaced_modal(old, Some((&pid_ref, "__confirm__", false)), cx);
            }
        }
        PluginUiEffect::Prompt {
            request_id,
            plugin_id,
            title,
            placeholder,
            default_value,
        } => {
            let pid_ref = plugin_id.clone();
            let prev = PluginBus::update(cx, |bus| {
                let old = bus.active_modal.take();
                bus.open_modal(PluginModalSpec {
                    plugin_id,
                    view_id: "__prompt__".to_owned(),
                    title,
                    kind: PluginModalKind::Prompt {
                        placeholder,
                        default_value,
                        request_id,
                    },
                });
                old
            });
            PluginHost::bump_modal_generation(cx);
            if let Some(old) = prev {
                resolve_displaced_modal(old, Some((&pid_ref, "__prompt__", false)), cx);
            }
        }
        PluginUiEffect::OpenModal {
            plugin_id,
            view_id,
            title,
            tree,
        } => {
            if let Ok(tree_json) = serde_json::to_value(&tree) {
                PluginHost::clear_view_error(&plugin_id, &view_id, cx);
                let pid_ref = plugin_id.clone();
                let vid_ref = view_id.clone();
                let prev = PluginBus::update(cx, |bus| {
                    let old = bus.active_modal.take();
                    bus.open_modal(PluginModalSpec {
                        plugin_id,
                        view_id,
                        title,
                        kind: PluginModalKind::View { tree: tree_json },
                    });
                    old
                });
                PluginHost::bump_modal_generation(cx);
                if let Some(old) = prev {
                    resolve_displaced_modal(old, Some((&pid_ref, &vid_ref, true)), cx);
                }
            }
        }
        PluginUiEffect::CloseModal { plugin_id } => {
            let matches_modal = PluginBus::global(cx)
                .and_then(|bus| bus.active_modal.as_ref())
                .is_some_and(|modal| modal.plugin_id == plugin_id);
            if matches_modal {
                let prev = PluginBus::update(cx, |bus| bus.active_modal.take());
                PluginHost::bump_modal_generation(cx);
                if let Some(old) = prev {
                    match old.kind {
                        PluginModalKind::Confirm { request_id, .. } => {
                            PluginHost::resolve_prompt_async(
                                request_id,
                                PromptReplyValue::Confirm(false),
                                cx,
                            );
                        }
                        PluginModalKind::Prompt { request_id, .. } => {
                            PluginHost::resolve_prompt_async(
                                request_id,
                                PromptReplyValue::Prompt(None),
                                cx,
                            );
                        }
                        PluginModalKind::View { .. } => {
                            PluginHost::clear_view_error(&old.plugin_id, &old.view_id, cx);
                        }
                    }
                }
            }
        }
        PluginUiEffect::OpenTask { task_id } => {
            PluginBus::update(cx, |bus| bus.push_nav(PluginNavAction::OpenTask(task_id)));
        }
        PluginUiEffect::OpenSettings { plugin_id } => {
            PluginHost::request_open_settings_for(plugin_id, cx);
        }
        PluginUiEffect::OpenExternal { url } => {
            let trimmed = url.trim();
            let lower = trimmed.to_ascii_lowercase();
            if lower.starts_with("https://") || lower.starts_with("http://") {
                cx.open_url(trimmed);
            }
        }
        PluginUiEffect::ClipboardWrite { text } => {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
}

pub(crate) fn schedule_toast_expiry(toast_id: u64, cx: &mut App) {
    let timers = cx.default_global::<ScheduledToastTimers>();
    if !timers.0.insert(toast_id) {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(TOAST_AUTO_DISMISS_DELAY)
            .await;
        cx.update(|cx| {
            if let Some(timers) = cx.try_global::<ScheduledToastTimers>()
                && timers.0.contains(&toast_id)
            {
                cx.update_global::<ScheduledToastTimers, _>(|timers, _| {
                    timers.0.remove(&toast_id);
                });
            }
            let still_present = PluginBus::global(cx)
                .is_some_and(|bus| bus.toasts.iter().any(|t| t.id == toast_id));
            if still_present {
                PluginBus::update(cx, |bus| bus.dismiss_toast(toast_id));
            }
        });
    })
    .detach();
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use fern_i18n::{CurrentLocale, Locale};
    use gpui::TestAppContext;

    #[gpui::test]
    fn hot_reload_poller_stays_stopped_with_zero_plugins_and_toggles_with_registry(
        cx: &mut TestAppContext,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        let cache_dir = plugins_dir.join(".registry-cache");
        std::fs::create_dir_all(&plugins_dir).unwrap();

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(CurrentLocale(Locale::En));
            PluginHost::init(
                PluginHostConfig {
                    plugins_dir: plugins_dir.clone(),
                    registry_cache_dir: cache_dir,
                    db_path: None,
                    secrets_vault_path: tmp.path().join("secrets.vault"),
                    secrets_key_path: tmp.path().join("secrets.key"),
                    safe_mode: false,
                    locale: "en".to_owned(),
                    theme_appearance: "light".to_owned(),
                    hot_reload: true,
                },
                cx,
            )
            .unwrap();
            let host = PluginHost::global(cx).unwrap();
            assert!(host.plugins().is_empty());
            assert!(host.hot_reload_enabled);
            assert!(!host.hot_reload_started);
            PluginHost::start_hot_reload_poller(cx);
            assert!(!PluginHost::global(cx).unwrap().hot_reload_started);
        });

        let plugin_dir = plugins_dir.join("com.test.poller");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{
                "id": "com.test.poller",
                "name": "Poller Test",
                "version": "1.0.0",
                "api_version": 1,
                "entry": "index.js",
                "permissions": []
            }"#,
        )
        .unwrap();
        std::fs::write(plugin_dir.join("index.js"), "export function activate() {}").unwrap();

        cx.update(PluginHost::reload_all_async);
        cx.run_until_parked();
        cx.update(|cx| {
            let host = PluginHost::global(cx).unwrap();
            assert_eq!(host.plugins().len(), 1);
            assert!(host.hot_reload_started);
        });

        std::fs::remove_dir_all(&plugin_dir).unwrap();
        cx.update(PluginHost::reload_all_async);
        cx.run_until_parked();
        cx.executor().advance_clock(HOT_RELOAD_POLL_INTERVAL);
        cx.run_until_parked();
        cx.update(|cx| {
            let host = PluginHost::global(cx).unwrap();
            assert!(host.plugins().is_empty());
            assert!(!host.hot_reload_started);
            PluginHost::shutdown(cx);
        });
    }
}
