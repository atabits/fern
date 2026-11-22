//! GPUI global [`PluginHost`] managing [`PluginManager`], async [`HostUpdate`] dispatch,
//! `PluginBus` synchronization, `DataVersion` <-> outbox bridging, active-window hot-reload,
//! plugin theme loading, and reactive plugin view tree state.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;

use anyhow::Result;
use desktop_common::data_version::{DataVersion, changed_elsewhere, data_changed};
use desktop_common::paths;
use desktop_common::plugin_bus::{
    PluginBus, PluginCommandSpec, PluginModalKind, PluginModalSpec, PluginNavAction,
    PluginSearchHit, PluginSidebarViewSpec, ToastLevel as BusToastLevel,
};
use gpui::{
    App, AppContext as _, BorrowAppContext as _, ClipboardItem, Entity, EntityId, Global,
    Subscription,
};
use plugins::manifest::resolve_inside_dir;
use plugins::ui_ast::{UiEvent, UiTreePayload};
use plugins::{
    HostUpdate, PluginEngineConfig, PluginInfo, PluginManager, PluginStatus, PluginUiEffect,
    PromptReplyValue, RegistryClient, ToastLevel as PluginToastLevel,
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
    woken: AtomicBool,
    closed: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl WakeSignal {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            woken: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            waker: Mutex::new(None),
        })
    }

    fn wake(&self) {
        self.woken.store(true, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().ok().and_then(|mut guard| guard.take()) {
            waker.wake();
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().ok().and_then(|mut guard| guard.take()) {
            waker.wake();
        }
    }
}

struct WaitForWake(Arc<WakeSignal>);

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

/// Global GPUI host coordinating the plugin runtime, UI bus, view trees, themes, and registry.
pub struct PluginHost {
    config: PluginHostConfig,
    manager: Option<PluginManager>,
    registry_client: RegistryClient,
    plugins_dir: PathBuf,
    safe_mode: bool,
    hot_reload_enabled: bool,
    hot_reload_started: bool,
    plugins: Vec<PluginInfo>,
    plugin_revisions: HashMap<String, u64>,
    views: HashMap<(String, String), UiTreePayload>,
    view_errors: HashMap<(String, String), String>,
    loaded_plugin_theme_ids: HashSet<String>,
    plugin_theme_errors: Vec<String>,
    requested_settings_plugin: Option<String>,
    requested_settings_open: Option<String>,
    external_data_seq: u64,
    notified_outbox_seq: u64,
    _marker: Entity<HostDataMarker>,
    marker_id: EntityId,
    wake_signal: Arc<WakeSignal>,
    _data_sub: Subscription,
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
    /// Initializes [`PluginHost`] as a GPUI `Global`, spawning [`PluginManager`] unless in safe mode.
    pub fn init(config: PluginHostConfig, cx: &mut App) -> Result<()> {
        if cx.try_global::<Self>().is_some() {
            let mut old = cx.remove_global::<Self>();
            old.shutdown_inner();
        }

        let wake_signal = WakeSignal::new();
        let registry_client =
            RegistryClient::new(&config.plugins_dir, &config.registry_cache_dir);

        let (manager, update_rx, initial_plugins) = if config.safe_mode {
            (None, None, Vec::new())
        } else {
            let signal_for_worker = Arc::clone(&wake_signal);
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
                wake_callback: Some(Arc::new(move || signal_for_worker.wake())),
            };
            let (mgr, rx) = PluginManager::spawn(engine_cfg)?;
            let initial_plugins = mgr.plugins().unwrap_or_default();
            (Some(mgr), Some(rx), initial_plugins)
        };

        let marker = cx.new(|_| HostDataMarker);
        let marker_id = marker.entity_id();

        let data_sub = cx.observe_global::<DataVersion>(move |cx| {
            let Some(host) = cx.try_global::<PluginHost>() else {
                return;
            };
            let marker_id = host.marker_id;
            if changed_elsewhere(marker_id, cx) {
                cx.update_global::<PluginHost, _>(|host, _| {
                    host.external_data_seq = host.external_data_seq.wrapping_add(1);
                    if let Some(mgr) = &host.manager {
                        mgr.notify_outbox_dirty();
                        host.notified_outbox_seq = host.external_data_seq;
                    }
                });
            }
        });

        let mut plugin_revisions = HashMap::new();
        for info in &initial_plugins {
            plugin_revisions.insert(info.manifest.id.clone(), 1);
        }

        let host = Self {
            config: config.clone(),
            manager,
            registry_client,
            plugins_dir: config.plugins_dir,
            safe_mode: config.safe_mode,
            hot_reload_enabled: config.hot_reload && !config.safe_mode,
            hot_reload_started: false,
            plugins: initial_plugins.clone(),
            plugin_revisions,
            views: HashMap::new(),
            view_errors: HashMap::new(),
            loaded_plugin_theme_ids: HashSet::new(),
            plugin_theme_errors: Vec::new(),
            requested_settings_plugin: None,
            requested_settings_open: None,
            external_data_seq: 0,
            notified_outbox_seq: 0,
            _marker: marker,
            marker_id,
            wake_signal: Arc::clone(&wake_signal),
            _data_sub: data_sub,
        };

        cx.set_global(host);
        sync_bus_contributions(&initial_plugins, cx);
        Self::sync_plugin_themes(cx);

        if let Some(rx) = update_rx {
            spawn_update_consumer(rx, wake_signal, cx);
        }
        if config.hot_reload && !config.safe_mode {
            Self::start_hot_reload_poller(cx);
        }

        Ok(())
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
        Self::init(next_cfg, cx)
    }

    /// Shuts down the active [`PluginManager`] and stops async host update tasks.
    pub fn shutdown(cx: &mut App) {
        if cx.has_global::<Self>() {
            cx.update_global::<Self, _>(|host, _| {
                host.shutdown_inner();
            });
        }
    }

    fn shutdown_inner(&mut self) {
        self.wake_signal.close();
        if let Some(mgr) = self.manager.take() {
            mgr.shutdown();
        }
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

    pub fn plugin(&self, plugin_id: &str) -> Option<&PluginInfo> {
        self.plugins.iter().find(|p| p.manifest.id == plugin_id)
    }

    pub fn plugin_revision(&self, plugin_id: &str) -> u64 {
        self.plugin_revisions.get(plugin_id).copied().unwrap_or(0)
    }

    pub fn view_tree(&self, plugin_id: &str, view_id: &str) -> Option<&UiTreePayload> {
        self.views.get(&(plugin_id.to_owned(), view_id.to_owned()))
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
            host.view_errors.remove(&key);
            host.views.insert(key, payload);
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
        let marker_id = host.marker_id;
        let current_seq = host.external_data_seq;
        let changed_by_other = changed_elsewhere(marker_id, cx);

        if *last_seen != current_seq {
            *last_seen = current_seq;
            cx.update_global::<Self, _>(|host, _| {
                if host.notified_outbox_seq < host.external_data_seq {
                    if let Some(mgr) = &host.manager {
                        mgr.notify_outbox_dirty();
                    }
                    host.notified_outbox_seq = host.external_data_seq;
                }
            });
            return true;
        }

        if changed_by_other && host.notified_outbox_seq == current_seq {
            let mut next_seq = current_seq;
            cx.update_global::<Self, _>(|host, _| {
                host.external_data_seq = host.external_data_seq.wrapping_add(1);
                next_seq = host.external_data_seq;
                if let Some(mgr) = &host.manager {
                    mgr.notify_outbox_dirty();
                    host.notified_outbox_seq = host.external_data_seq;
                }
            });
            *last_seen = next_seq;
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

    /// Starts an active-window hot-reload poller that pauses when no Fern window is focused.
    pub fn start_hot_reload_poller(cx: &mut App) {
        let Some(host) = cx.try_global::<Self>() else {
            return;
        };
        if host.safe_mode || host.manager.is_none() || host.hot_reload_started {
            return;
        }
        let signal = Arc::clone(&host.wake_signal);
        cx.update_global::<Self, _>(|host, _| {
            host.hot_reload_started = true;
            host.hot_reload_enabled = true;
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
                    Active(PluginManager),
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
                    if !host.hot_reload_enabled || host.safe_mode {
                        return PollState::Stop;
                    }
                    let Some(mgr) = host.manager.clone() else {
                        return PollState::Stop;
                    };
                    if cx.active_window().is_none() {
                        PollState::Idle
                    } else {
                        PollState::Active(mgr)
                    }
                });

                let mgr = match state {
                    PollState::Stop => break,
                    PollState::Idle => continue,
                    PollState::Active(mgr) => mgr,
                };

                let changed = bg.spawn(async move { mgr.check_hot_reload() }).await;
                if let Ok(changed_ids) = changed
                    && !changed_ids.is_empty()
                {
                    cx.update(|cx| {
                        if cx.has_global::<PluginHost>() {
                            cx.update_global::<PluginHost, _>(|host, _| {
                                for id in &changed_ids {
                                    let entry =
                                        host.plugin_revisions.entry(id.clone()).or_insert(0);
                                    *entry = entry.wrapping_add(1);
                                }
                            });
                        }
                    });
                }
            }
        })
        .detach();
    }

    /// Triggers an immediate off-thread hot-reload check.
    pub fn check_hot_reload_now(cx: &mut App) {
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            return;
        };
        let task = cx.background_spawn(async move { mgr.check_hot_reload() });
        cx.spawn(async move |cx| {
            if let Ok(Ok(changed_ids)) = Ok::<_, anyhow::Error>(task.await)
                && !changed_ids.is_empty()
            {
                cx.update(|cx| {
                    if cx.has_global::<PluginHost>() {
                        cx.update_global::<PluginHost, _>(|host, _| {
                            for id in changed_ids {
                                let entry = host.plugin_revisions.entry(id).or_insert(0);
                                *entry = entry.wrapping_add(1);
                            }
                        });
                    }
                });
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
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            return;
        };
        let pid = plugin_id.clone();
        let vid = view_id.clone();
        let task = cx.background_spawn(async move {
            if mount {
                mgr.mount_view(&pid, &vid)
            } else {
                mgr.render_view(&pid, &vid)
            }
        });
        cx.spawn(async move |cx| {
            let res = task.await;
            cx.update(|cx| match res {
                Ok(payload) => Self::set_view_tree(plugin_id, view_id, payload, cx),
                Err(err) => Self::set_view_error(plugin_id, view_id, format!("{err:#}"), cx),
            });
        })
        .detach();
    }

    /// Unmounts a plugin sidebar view off the UI thread.
    pub fn unmount_view_async(plugin_id: String, view_id: String, cx: &mut App) {
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            return;
        };
        cx.background_spawn(async move {
            let _ = mgr.unmount_view(&plugin_id, &view_id);
        })
        .detach();
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
                Ok(None) => {}
                Err(err) => Self::set_view_error(plugin_id, vid, format!("{err:#}"), cx),
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
                    let id = PluginBus::update(cx, |bus| {
                        bus.push_toast(
                            plugin_id,
                            msg,
                            BusToastLevel::Error,
                            now_ms(),
                        )
                    });
                    schedule_toast_expiry(id, cx);
                });
            }
        })
        .detach();
    }

    /// Queries enabled plugin search providers off-thread and updates `PluginBus::search_hits`.
    pub fn search_async(query: String, cx: &mut App) {
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            PluginBus::update(cx, |bus| bus.search_hits.clear());
            return;
        };
        let task = cx.background_spawn(async move { mgr.search(&query) });
        cx.spawn(async move |cx| {
            if let Ok(hits) = task.await {
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
                cx.update(|cx| {
                    PluginBus::update(cx, |bus| {
                        bus.search_hits = mapped;
                    });
                });
            }
        })
        .detach();
    }

    /// Reloads a single plugin off the UI thread and bumps its view revision.
    pub fn reload_plugin_async(plugin_id: String, cx: &mut App) {
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            return;
        };
        let pid = plugin_id.clone();
        let task = cx.background_spawn(async move { mgr.reload_plugin(&pid) });
        cx.spawn(async move |cx| {
            let res = task.await;
            cx.update(|cx| {
                if cx.has_global::<PluginHost>() {
                    cx.update_global::<PluginHost, _>(|host, _| {
                        host.view_errors.retain(|(p, _), _| p != &plugin_id);
                        let rev = host.plugin_revisions.entry(plugin_id.clone()).or_insert(0);
                        *rev = rev.wrapping_add(1);
                        if let Err(err) = res {
                            host.view_errors
                                .insert((plugin_id.clone(), String::new()), format!("{err:#}"));
                        }
                    });
                }
            });
        })
        .detach();
    }

    /// Reloads all plugins from disk off the UI thread.
    pub fn reload_all_async(cx: &mut App) {
        let Some(mgr) = Self::global(cx).and_then(|h| h.manager.clone()) else {
            return;
        };
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
        let plugins = host.plugins.clone();
        let prev_plugin_ids = host.loaded_plugin_theme_ids.clone();

        let (plugin_themes, errors) = collect_plugin_themes(&plugins);
        let next_plugin_ids: HashSet<String> =
            plugin_themes.iter().map(|t| t.id().to_owned()).collect();

        let registry = ThemeRegistry::global(cx);
        let mut combined: Vec<Theme> = registry
            .custom_themes()
            .iter()
            .filter(|t| !prev_plugin_ids.contains(t.id()))
            .map(|t| (**t).clone())
            .collect();
        combined.extend(plugin_themes);

        let mut combined_errors: Vec<String> = registry
            .errors()
            .iter()
            .filter(|err| !host.plugin_theme_errors.contains(err))
            .cloned()
            .collect();
        combined_errors.extend(errors.clone());

        theme::reload_user_themes(combined, combined_errors, cx);
        cx.update_global::<Self, _>(|host, _| {
            host.loaded_plugin_theme_ids = next_plugin_ids;
            host.plugin_theme_errors = errors;
        });
    }
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
            if cx.has_global::<PluginHost>() {
                cx.update_global::<PluginHost, _>(|host, _| {
                    if let Some(info) = host
                        .plugins
                        .iter_mut()
                        .find(|p| p.manifest.id == plugin_id)
                    {
                        if matches!(
                            status,
                            PluginStatus::Disabled
                                | PluginStatus::NeedsApproval { .. }
                                | PluginStatus::CircuitBroken { .. }
                        ) {
                            info.enabled = false;
                        }
                        info.status = status.clone();
                        info.metrics = metrics;
                    }
                    updated_plugins = Some(host.plugins.clone());
                });
            }
            if let Some(plugins) = updated_plugins {
                sync_bus_contributions(&plugins, cx);
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
    if cx.has_global::<PluginHost>() {
        cx.update_global::<PluginHost, _>(|host, _| {
            for info in &infos {
                let prev_enabled = host
                    .plugins
                    .iter()
                    .find(|p| p.manifest.id == info.manifest.id)
                    .is_some_and(|p| p.enabled);
                let prev_hash = host
                    .plugins
                    .iter()
                    .find(|p| p.manifest.id == info.manifest.id)
                    .map(|p| p.manifest_hash.as_str());
                let changed = !prev_enabled && info.enabled
                    || prev_hash != Some(info.manifest_hash.as_str());
                if changed || !host.plugin_revisions.contains_key(&info.manifest.id) {
                    let rev = host
                        .plugin_revisions
                        .entry(info.manifest.id.clone())
                        .or_insert(0);
                    *rev = rev.wrapping_add(1);
                }
                if !info.enabled {
                    host.views.retain(|(pid, _), _| pid != &info.manifest.id);
                    host.view_errors
                        .retain(|(pid, _), _| pid != &info.manifest.id);
                }
            }
            let alive: HashSet<&str> = infos.iter().map(|p| p.manifest.id.as_str()).collect();
            host.views.retain(|(pid, _), _| alive.contains(pid.as_str()));
            host.view_errors
                .retain(|(pid, _), _| alive.contains(pid.as_str()));
            host.plugins = infos.clone();
        });
    }
    sync_bus_contributions(&infos, cx);
    PluginHost::sync_plugin_themes(cx);
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

fn apply_ui_effect(effect: PluginUiEffect, cx: &mut App) {
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
            let toast_id = PluginBus::update(cx, |bus| {
                while bus.toasts.len() >= MAX_PENDING_TOASTS {
                    bus.toasts.remove(0);
                }
                bus.push_toast(plugin_id, message, bus_level, created_at_ms)
            });
            schedule_toast_expiry(toast_id, cx);
        }
        PluginUiEffect::Confirm {
            request_id,
            plugin_id,
            title,
            body,
        } => {
            PluginBus::update(cx, |bus| {
                bus.open_modal(PluginModalSpec {
                    plugin_id,
                    view_id: "__confirm__".to_owned(),
                    title,
                    kind: PluginModalKind::Confirm { body, request_id },
                });
            });
        }
        PluginUiEffect::Prompt {
            request_id,
            plugin_id,
            title,
            placeholder,
            default_value,
        } => {
            PluginBus::update(cx, |bus| {
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
            });
        }
        PluginUiEffect::OpenModal {
            plugin_id,
            view_id,
            title,
            tree,
        } => {
            if let Ok(tree_json) = serde_json::to_value(&tree) {
                PluginBus::update(cx, |bus| {
                    bus.open_modal(PluginModalSpec {
                        plugin_id,
                        view_id,
                        title,
                        kind: PluginModalKind::View { tree: tree_json },
                    });
                });
            }
        }
        PluginUiEffect::CloseModal { plugin_id } => {
            let matches_modal = PluginBus::global(cx)
                .and_then(|bus| bus.active_modal.as_ref())
                .is_some_and(|modal| modal.plugin_id == plugin_id);
            if matches_modal {
                PluginBus::update(cx, |bus| bus.close_modal());
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
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(TOAST_AUTO_DISMISS_DELAY)
            .await;
        cx.update(|cx| {
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
