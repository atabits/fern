//! GPUI global [`PluginHost`] managing [`PluginManager`], async [`HostUpdate`] dispatch,
//! `PluginBus` synchronization, `DataVersion` <-> outbox bridging, active-window hot-reload,
//! plugin theme loading, and reactive plugin view tree state.

mod dispatch;
mod themes;
mod updates;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(not(test))]
use std::future::Future;
use std::path::PathBuf;
#[cfg(not(test))]
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(test)]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
#[cfg(not(test))]
use std::task::{Context as TaskContext, Poll};
use std::task::Waker;
use std::time::Duration;

use anyhow::Result;
use desktop_common::data_version::{DataVersion, changed_elsewhere};
use desktop_common::paths;
use desktop_common::plugin_bus::{PluginBus, PluginHookDecision, PluginHooksGlobal, PluginNavAction};
use gpui::{
    App, AppContext as _, BorrowAppContext as _, Entity, EntityId, Global, Subscription,
};
use plugins::ui_ast::UiTreePayload;
#[cfg(test)]
use plugins::HostUpdate;
use plugins::{
    HookDecision, PluginEngineConfig, PluginInfo, PluginManager, PluginStatus, RegistryClient,
};
use theme::Theme;

pub use themes::collect_plugin_themes;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use updates::{apply_ui_effect, resolve_displaced_modal, schedule_toast_expiry};

use themes::ThemeSourceSignature;
#[cfg(test)]
use updates::apply_host_update;
#[cfg(not(test))]
use updates::spawn_update_consumer;
use updates::{dismiss_active_modal_on_shutdown, sync_bus_contributions};

const HOT_RELOAD_POLL_INTERVAL: Duration = Duration::from_millis(1200);

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

pub(super) struct WakeSignal {
    #[cfg(not(test))]
    woken: AtomicBool,
    pub(super) closed: AtomicBool,
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
pub(super) struct WaitForWake(pub(super) Arc<WakeSignal>);

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

pub(super) enum PluginViewOp {
    Mount { view_id: String, generation: u64 },
    Render { view_id: String, generation: u64 },
    Unmount { view_id: String },
    Reload,
    ReloadAndMount { view_id: String, generation: u64 },
    ResetAndMount { view_id: String, generation: u64 },
}

#[derive(Default)]
pub(super) struct PluginOpState {
    pub(super) view_generations: HashMap<(String, String), u64>,
    pub(super) mounting_views: HashSet<(String, String)>,
    pub(super) in_flight_reload_or_reset: HashSet<String>,
    pub(super) queues: HashMap<String, VecDeque<PluginViewOp>>,
    pub(super) running: HashSet<String>,
}

impl PluginOpState {
    pub(super) fn next_view_gen(&mut self, plugin_id: &str, view_id: &str) -> u64 {
        let key = (plugin_id.to_owned(), view_id.to_owned());
        let entry = self.view_generations.entry(key).or_insert(0);
        *entry = entry.wrapping_add(1);
        *entry
    }

    pub(super) fn invalidate_plugin_views(&mut self, plugin_id: &str) {
        for ((pid, _), generation) in &mut self.view_generations {
            if pid == plugin_id {
                *generation = generation.wrapping_add(1);
            }
        }
        self.mounting_views.retain(|(pid, _)| pid != plugin_id);
    }
}

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

pub(super) fn has_active_hook_plugins(safe_mode: bool, has_manager: bool, plugins: &[PluginInfo]) -> bool {
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

    pub(super) fn ensure_hot_reload_poller(cx: &mut App) {
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
