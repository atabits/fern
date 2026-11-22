//! Native GPUI plugin settings panel (`Installed`, `Scripts`, `Browse`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone as _};
use desktop_common::actions::DismissSettings;
use desktop_common::data_version::data_changed;
use desktop_common::text_input::{Changed, Mode, Submitted, TextInput};
use fern_core::Store;
use fern_core::plugin_store::{PluginState, UserScript};
use fern_i18n::{self as i18n, Locale};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, ElementId, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, PathPromptOptions, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use icons::IconName;
use plugins::manifest::{SettingKind, SettingSpec};
use plugins::registry::{DEFAULT_REGISTRY_URL, RegistryClient};
use plugins::{
    LogEntry, LogLevel, Permission, PermissionApprovalStatus, PermissionSet, PluginInfo,
    PluginStatus, RegistryPluginEntry, RegistrySource, ScriptRunResult, generate_sample_plugin,
};
use serde_json::{Map as JsonMap, Value as JsonValue};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Checkbox, Chip, Color, Divider, EmptyState, Icon,
    IconButton, IconSize, Label, LabelSize, ListItem, Modal, Placement, PopoverAttachment,
    SegmentedTab, SegmentedTabs, Switch, Tooltip, popover, v_scroll,
};
use uuid::Uuid;

use crate::host::PluginHost;
use crate::view::{PluginView, PluginViewEvent};

const SAFE_MODE_SETTING_KEY: &str = "plugins_safe_mode";
const REGISTRIES_SETTING_KEY: &str = "plugin_registries";
const SETTING_SAVE_DEBOUNCE: Duration = Duration::from_millis(350);

/// Top-level tabs of [`PluginSettingsView`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PluginSettingsTab {
    #[default]
    Installed,
    Scripts,
    Browse,
}

#[derive(Clone)]
struct PendingZipOverwrite {
    zip_path: PathBuf,
    existing_state: Option<PluginState>,
}

#[derive(Clone)]
struct PermissionPromptState {
    plugin_id: String,
    plugin_name: String,
    version: String,
    added_permissions: Vec<Permission>,
    is_first_install: bool,
    pending_zip_overwrite: Option<PendingZipOverwrite>,
}

#[derive(Clone)]
struct CatalogItem {
    source_url: String,
    entry: RegistryPluginEntry,
}

struct SettingInputEntry {
    input: Entity<TextInput>,
    last_synced: String,
    save_task: Option<gpui::Task<()>>,
    _subs: [Subscription; 2],
}

struct SecretInputEntry {
    input: Entity<TextInput>,
    placeholder: String,
}

struct ActivePluginPage {
    plugin_id: String,
    plugin_name: String,
    view_id: String,
    view: Entity<PluginView>,
    _view_sub: Subscription,
}

/// Native plugin management, user script runner, and catalog browser panel.
pub struct PluginSettingsView {
    store: Rc<Store>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    tab: PluginSettingsTab,
    safe_mode_pref: bool,
    status_banner: Option<(String, bool)>,

    // Installed tab state
    active_plugin_page: Option<ActivePluginPage>,
    expanded_settings: Option<String>,
    expanded_logs: Option<String>,
    plugin_logs: HashMap<String, Vec<LogEntry>>,
    setting_inputs: HashMap<(String, String), SettingInputEntry>,
    secret_inputs: HashMap<(String, String), SecretInputEntry>,
    in_flight_secrets: HashSet<(String, String)>,
    pending_setting_overrides: HashMap<String, JsonMap<String, JsonValue>>,
    setting_update_in_flight: HashSet<String>,
    setting_update_dirty: HashSet<String>,
    open_select_setting: Option<(String, String)>,
    pending_permission_modal: Option<PermissionPromptState>,
    sample_counter: u32,

    // Scripts tab state
    scripts: Vec<UserScript>,
    selected_script_id: Option<String>,
    script_enabled: bool,
    script_trigger_kind: String,
    script_permissions: BTreeSet<String>,
    script_name_input: Entity<TextInput>,
    script_trigger_arg_input: Entity<TextInput>,
    script_code_input: Entity<TextInput>,
    script_code_scroll: ScrollHandle,
    script_running: bool,
    script_run_seq: u64,
    script_result: Option<ScriptRunResult>,

    // Browse tab state
    registries: Vec<RegistrySource>,
    catalog_items: Vec<CatalogItem>,
    registry_errors: Vec<String>,
    browse_loading: bool,
    browse_auto_refreshed: bool,
    browse_generation: u64,
    browse_fetch_seq: u64,
    browse_completed_fetch_seq: u64,
    browse_search_input: Entity<TextInput>,
    registry_url_input: Entity<TextInput>,
    installing_plugin_ids: HashSet<String>,

    _host_sub: Subscription,
}

impl Focusable for PluginSettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PluginSettingsView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let stored_safe_mode = store
            .setting(SAFE_MODE_SETTING_KEY)
            .ok()
            .flatten()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let host_safe_mode = PluginHost::global(cx).is_some_and(|h| h.is_safe_mode());
        let safe_mode_pref = stored_safe_mode || host_safe_mode;
        let scripts = store.user_scripts().unwrap_or_default();
        let registries = load_registries_from_store(&store);

        let script_name_input =
            cx.new(|cx| TextInput::new(Mode::SingleLine, i18n::t(cx).plugins_script_name, cx));
        let script_trigger_arg_input =
            cx.new(|cx| TextInput::new(Mode::SingleLine, "task.created", cx));
        let script_code_input = cx.new(|cx| {
            TextInput::new(Mode::MultiLine, "// fern.tasks.list()\n", cx).without_cancel()
        });
        let browse_search_input =
            cx.new(|cx| TextInput::new(Mode::SingleLine, i18n::t(cx).focus_picker_search, cx));
        let registry_url_input = cx.new(|cx| {
            TextInput::new(Mode::SingleLine, i18n::t(cx).plugins_registry_add_url, cx)
        });

        cx.subscribe(&browse_search_input, |_, _, _: &Changed, cx| cx.notify())
            .detach();
        cx.subscribe(&registry_url_input, |this, _, _: &Submitted, cx| {
            this.add_registry_url(cx);
        })
        .detach();

        let host_sub = cx.observe_global::<PluginHost>(|this, cx| {
            if let Some(host) = PluginHost::global(cx) {
                this.safe_mode_pref = host.is_safe_mode();
            }
            this.consume_requested_logs_plugin(cx);
            if let Some(pid) = this.expanded_logs.clone() {
                this.fetch_plugin_logs_async(pid, cx);
            }
            cx.notify();
        });

        let mut view = Self {
            store,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            tab: PluginSettingsTab::Installed,
            safe_mode_pref,
            status_banner: None,
            active_plugin_page: None,
            expanded_settings: None,
            expanded_logs: None,
            plugin_logs: HashMap::new(),
            setting_inputs: HashMap::new(),
            secret_inputs: HashMap::new(),
            in_flight_secrets: HashSet::new(),
            pending_setting_overrides: HashMap::new(),
            setting_update_in_flight: HashSet::new(),
            setting_update_dirty: HashSet::new(),
            open_select_setting: None,
            pending_permission_modal: None,
            sample_counter: 1,
            scripts: scripts.clone(),
            selected_script_id: None,
            script_enabled: true,
            script_trigger_kind: "manual".to_owned(),
            script_permissions: BTreeSet::from(["tasks:read".to_owned()]),
            script_name_input,
            script_trigger_arg_input,
            script_code_input,
            script_code_scroll: ScrollHandle::new(),
            script_running: false,
            script_run_seq: 0,
            script_result: None,
            registries,
            catalog_items: Vec::new(),
            registry_errors: Vec::new(),
            browse_loading: false,
            browse_auto_refreshed: false,
            browse_generation: 0,
            browse_fetch_seq: 0,
            browse_completed_fetch_seq: 0,
            browse_search_input,
            registry_url_input,
            installing_plugin_ids: HashSet::new(),
            _host_sub: host_sub,
        };

        if let Some(first) = scripts.first().cloned() {
            view.load_script_into_editor(&first, cx);
        } else {
            view.reset_new_script_editor(cx);
        }
        view.consume_requested_logs_plugin(cx);
        view
    }

    pub fn active_tab(&self) -> PluginSettingsTab {
        self.tab
    }

    pub fn active_plugin_page_id(&self) -> Option<(&str, &str)> {
        self.active_plugin_page
            .as_ref()
            .map(|p| (p.plugin_id.as_str(), p.view_id.as_str()))
    }

    #[cfg(test)]
    pub(crate) fn has_pending_permission_modal(&self) -> bool {
        self.pending_permission_modal.is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_cached_inputs_for(&self, plugin_id: &str) -> bool {
        self.setting_inputs.keys().any(|(p, _)| p == plugin_id)
            || self.secret_inputs.keys().any(|(p, _)| p == plugin_id)
    }

    #[cfg(test)]
    pub(crate) fn update_plugin_setting_value_for_test(
        &mut self,
        plugin_id: &str,
        key: &str,
        value: JsonValue,
        cx: &mut Context<Self>,
    ) {
        self.update_plugin_setting_value(plugin_id, key, value, cx);
    }

    /// Handles an Escape press inside [`PluginSettingsView`].
    ///
    /// Dismisses transient UI in priority order (`pending_permission_modal` -> open `Select`
    /// popover -> `active_plugin_page`) and returns `true` if handled so the parent Settings
    /// window stays open.
    pub fn handle_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.pending_permission_modal.take().is_some() {
            cx.notify();
            return true;
        }
        if self.open_select_setting.take().is_some() {
            cx.notify();
            return true;
        }
        if self.active_plugin_page.is_some() {
            self.close_plugin_page(cx);
            return true;
        }
        false
    }

    pub fn open_plugin_page(
        &mut self,
        plugin_id: String,
        plugin_name: String,
        view_id: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(prev) = self.active_plugin_page.take() {
            prev.view.update(cx, |v, cx| v.unmount(cx));
        }
        let pid_clone = plugin_id.clone();
        let vid_clone = view_id.clone();
        let view = cx.new(|cx| PluginView::new_embedded(pid_clone, vid_clone, cx));
        let view_sub = cx.subscribe(&view, |this, _, ev: &PluginViewEvent, cx| match ev {
            PluginViewEvent::OpenLogs { plugin_id } => {
                this.close_plugin_page(cx);
                this.open_logs_for(plugin_id, cx);
            }
        });
        self.tab = PluginSettingsTab::Installed;
        self.active_plugin_page = Some(ActivePluginPage {
            plugin_id,
            plugin_name,
            view_id,
            view,
            _view_sub: view_sub,
        });
        cx.notify();
    }

    pub fn close_plugin_page(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = self.active_plugin_page.take() {
            prev.view.update(cx, |v, cx| v.unmount(cx));
            cx.notify();
        }
    }

    pub fn set_tab(&mut self, tab: PluginSettingsTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.close_plugin_page(cx);
        self.tab = tab;
        self.open_select_setting = None;
        if tab == PluginSettingsTab::Scripts {
            self.scripts = self.store.user_scripts().unwrap_or_default();
        } else if tab == PluginSettingsTab::Browse {
            self.load_cached_registries_async(cx);
            if !cfg!(test) && !self.browse_auto_refreshed {
                self.browse_auto_refreshed = true;
                self.refresh_registries(cx);
            }
        }
        cx.notify();
    }

    pub fn open_logs_for(&mut self, plugin_id: &str, cx: &mut Context<Self>) {
        self.close_plugin_page(cx);
        self.tab = PluginSettingsTab::Installed;
        self.expanded_logs = Some(plugin_id.to_owned());
        self.fetch_plugin_logs_async(plugin_id.to_owned(), cx);
        cx.notify();
    }

    fn consume_requested_logs_plugin(&mut self, cx: &mut Context<Self>) {
        if let Some(pid) = PluginHost::take_requested_settings_open(cx) {
            self.close_plugin_page(cx);
            self.tab = PluginSettingsTab::Installed;
            self.expanded_settings = Some(pid);
            self.expanded_logs = None;
        }
        if let Some(pid) = PluginHost::take_requested_settings_plugin(cx) {
            self.close_plugin_page(cx);
            self.tab = PluginSettingsTab::Installed;
            self.expanded_logs = Some(pid.clone());
            self.fetch_plugin_logs_async(pid, cx);
        }
    }

    // -------------------------------------------------------------------------
    // Installed tab actions
    // -------------------------------------------------------------------------

    fn toggle_safe_mode_pref(&mut self, cx: &mut Context<Self>) {
        let next = !self.safe_mode_pref;
        let val = if next { "1" } else { "0" };
        match self.store.set_setting(SAFE_MODE_SETTING_KEY, val) {
            Ok(()) => {
                self.safe_mode_pref = next;
                if next {
                    self.close_plugin_page(cx);
                }
                match PluginHost::set_safe_mode(next, cx) {
                    Ok(()) => {
                        self.status_banner = None;
                    }
                    Err(err) => {
                        self.safe_mode_pref =
                            PluginHost::global(cx).is_some_and(|h| h.is_safe_mode());
                        let _ = self.store.set_setting(
                            SAFE_MODE_SETTING_KEY,
                            if self.safe_mode_pref { "1" } else { "0" },
                        );
                        self.status_banner = Some((format!("{err:#}"), true));
                    }
                }
            }
            Err(err) => {
                self.status_banner = Some((format!("{err:#}"), true));
            }
        }
        cx.notify();
    }

    fn create_sample_plugin(&mut self, cx: &mut Context<Self>) {
        let Some(plugins_dir) = PluginHost::global(cx).map(|h| h.plugins_dir().clone()) else {
            return;
        };
        let mgr = PluginHost::global(cx).and_then(|h| h.manager().cloned());
        let mut idx = self.sample_counter.max(1);
        while plugins_dir.join(format!("com.fern.sample-{idx}")).exists() {
            idx = idx.saturating_add(1);
        }
        self.sample_counter = idx.saturating_add(1);
        let pid = format!("com.fern.sample-{idx}");
        let name = format!("Sample Plugin {idx}");

        let task = cx.background_spawn(async move {
            let dir = generate_sample_plugin(&plugins_dir, &pid, &name)?;
            if let Some(mgr) = mgr {
                let _ = mgr.reload_all()?;
            }
            Ok::<_, anyhow::Error>((pid, dir))
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok((pid, dir)) => {
                        this.expanded_settings = Some(pid);
                        this.status_banner =
                            Some((format!("Created {}", dir.display()), false));
                    }
                    Err(err) => {
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install_from_zip_dialog(&mut self, cx: &mut Context<Self>) {
        if PluginHost::global(cx).is_none() {
            return;
        }
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });

        cx.spawn(async move |this, cx| {
            let Some(path) = chosen
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths| paths.into_iter().next())
            else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.install_from_zip_path(path, cx);
            });
        })
        .detach();
    }

    /// Validates a local `.zip` archive in an isolated staging directory before touching
    /// `plugins_dir/<id>`. Any overwrite or manifest hash change requires explicit re-approval
    /// before swapping files or running code.
    pub(crate) fn install_from_zip_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some((client, plugins_dir)) = PluginHost::global(cx)
            .map(|h| (h.registry_client().clone(), h.plugins_dir().clone()))
        else {
            return;
        };
        let mgr = PluginHost::global(cx).and_then(|h| h.manager().cloned());
        let path_for_inspect = path.clone();
        let plugins_dir_inspect = plugins_dir.clone();

        cx.spawn(async move |this, cx| {
            // Stage-inspect the zip in an isolated temp dir so we know its manifest ID, hash,
            // and permissions BEFORE overwriting `plugins_dir/<id>`.
            let preview_res = cx
                .background_spawn(async move {
                    std::fs::create_dir_all(&plugins_dir_inspect)?;
                    let inspect_dir = plugins_dir_inspect
                        .join(format!(".zip-inspect-{}", Uuid::now_v7()));
                    std::fs::create_dir_all(&inspect_dir)?;
                    struct InspectCleanup(PathBuf);
                    impl Drop for InspectCleanup {
                        fn drop(&mut self) {
                            let _ = std::fs::remove_dir_all(&self.0);
                        }
                    }
                    let _cleanup = InspectCleanup(inspect_dir.clone());
                    let staging_client = RegistryClient::new(&inspect_dir, &inspect_dir);
                    staging_client.install_from_zip_file(&path_for_inspect, None)
                })
                .await;

            let preview = match preview_res {
                Ok(out) => out,
                Err(err) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status_banner = Some((format!("{err:#}"), true));
                        cx.notify();
                    });
                    return;
                }
            };

            let decision = this.update(cx, |this, cx| {
                let existing_state = this.store.plugin_state(&preview.manifest.id).ok().flatten();
                let target_exists = plugins_dir.join(&preview.manifest.id).exists();
                let is_overwrite = target_exists || existing_state.is_some();
                let hash_changed = existing_state
                    .as_ref()
                    .is_some_and(|st| st.manifest_hash != preview.manifest_hash);
                let diff_status = preview
                    .manifest
                    .check_permission_diff(&preview.manifest_hash, existing_state.as_ref());

                if is_overwrite || hash_changed {
                    // Never overwrite or run unapproved local-ZIP code over an existing plugin
                    // before the user explicitly confirms and re-approves permissions.
                    let shown_permissions = match &diff_status {
                        PermissionApprovalStatus::NeedsApproval { diff, .. }
                            if !diff.added.is_empty() =>
                        {
                            diff.added.clone()
                        }
                        _ => preview.manifest.permissions.clone(),
                    };
                    this.pending_permission_modal = Some(PermissionPromptState {
                        plugin_id: preview.manifest.id.clone(),
                        plugin_name: preview.manifest.name.clone(),
                        version: preview.manifest.version.clone(),
                        added_permissions: shown_permissions,
                        is_first_install: false,
                        pending_zip_overwrite: Some(PendingZipOverwrite {
                            zip_path: path.clone(),
                            existing_state,
                        }),
                    });
                    cx.notify();
                    return false;
                }

                // Fresh install: guarantee plugin starts disabled until approved/enabled.
                let _ = this.store.set_plugin_enabled(&preview.manifest.id, false);
                true
            });

            if !matches!(decision, Ok(true)) {
                return;
            }

            let install_res = cx
                .background_spawn(async move {
                    let out = client.install_from_zip_file(&path, None)?;
                    let infos = if let Some(mgr) = mgr {
                        Some(mgr.reload_all()?)
                    } else {
                        None
                    };
                    Ok::<_, anyhow::Error>((out, infos))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match install_res {
                    Ok((out, infos)) => {
                        this.status_banner = Some((
                            format!("{} v{}", out.manifest.name, out.manifest.version),
                            false,
                        ));
                        let reloaded_status = infos
                            .as_ref()
                            .and_then(|list| list.iter().find(|p| p.manifest.id == out.manifest.id))
                            .map(|p| &p.status);
                        match reloaded_status {
                            Some(PluginStatus::NeedsApproval {
                                added_permissions,
                                is_first_install,
                            }) => {
                                this.pending_permission_modal = Some(PermissionPromptState {
                                    plugin_id: out.manifest.id,
                                    plugin_name: out.manifest.name,
                                    version: out.manifest.version,
                                    added_permissions: added_permissions.clone(),
                                    is_first_install: *is_first_install,
                                    pending_zip_overwrite: None,
                                });
                            }
                            None => {
                                if let PermissionApprovalStatus::NeedsApproval {
                                    diff,
                                    is_first_install,
                                } = out.approval_status
                                {
                                    this.pending_permission_modal = Some(PermissionPromptState {
                                        plugin_id: out.manifest.id,
                                        plugin_name: out.manifest.name,
                                        version: out.manifest.version,
                                        added_permissions: diff.added,
                                        is_first_install,
                                        pending_zip_overwrite: None,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(err) => {
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_plugins_folder(&self, cx: &mut Context<Self>) {
        let Some(dir) = PluginHost::global(cx).map(|h| h.plugins_dir().clone()) else {
            return;
        };
        let task = cx.background_spawn(async move {
            std::fs::create_dir_all(&dir)?;
            Ok::<_, anyhow::Error>(dir)
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| match res {
                Ok(dir) => cx.open_with_system(&dir),
                Err(err) => {
                    this.status_banner = Some((format!("{err:#}"), true));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn toggle_plugin_enabled(&mut self, info: &PluginInfo, cx: &mut Context<Self>) {
        if let PluginStatus::NeedsApproval {
            added_permissions,
            is_first_install,
        } = &info.status
        {
            self.pending_permission_modal = Some(PermissionPromptState {
                plugin_id: info.manifest.id.clone(),
                plugin_name: info.manifest.name.clone(),
                version: info.manifest.version.clone(),
                added_permissions: added_permissions.clone(),
                is_first_install: *is_first_install,
                pending_zip_overwrite: None,
            });
            cx.notify();
            return;
        }
        if matches!(info.status, PluginStatus::CircuitBroken { .. }) {
            self.reset_and_enable_plugin(info.manifest.id.clone(), cx);
            return;
        }

        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        let pid = info.manifest.id.clone();
        let next_enabled = !info.enabled;
        if !next_enabled
            && self
                .active_plugin_page
                .as_ref()
                .is_some_and(|p| p.plugin_id == pid)
        {
            self.close_plugin_page(cx);
        }
        let task = cx.background_spawn(async move { mgr.set_plugin_enabled(&pid, next_enabled) });
        cx.spawn(async move |this, cx| {
            if let Err(err) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.status_banner = Some((format!("{err:#}"), true));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn approve_pending_permissions(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_permission_modal.take() else {
            return;
        };
        if let Some(zip_overwrite) = pending.pending_zip_overwrite {
            let Some(client) = PluginHost::global(cx).map(|h| h.registry_client().clone()) else {
                cx.notify();
                return;
            };
            let mgr = PluginHost::global(cx).and_then(|h| h.manager().cloned());
            let pid = pending.plugin_id;
            if self
                .active_plugin_page
                .as_ref()
                .is_some_and(|p| p.plugin_id == pid)
            {
                self.close_plugin_page(cx);
            }
            // Disable in DB before swapping files so overwritten code never auto-activates if interrupted.
            let _ = self.store.set_plugin_enabled(&pid, false);
            let _ = self.store.set_plugin_permissions(&pid, "[]", "");
            let pid_bg = pid.clone();
            let task = cx.background_spawn(async move {
                if let Some(mgr) = &mgr {
                    let _ = mgr.set_plugin_enabled(&pid_bg, false);
                }
                let out = client.install_from_zip_file(
                    &zip_overwrite.zip_path,
                    zip_overwrite.existing_state.as_ref(),
                )?;
                if let Some(mgr) = &mgr {
                    let _ = mgr.reload_all()?;
                    mgr.approve_permissions(&pid_bg)?;
                }
                Ok::<_, anyhow::Error>(out)
            });
            cx.spawn(async move |this, cx| {
                let res = task.await;
                let _ = this.update(cx, |this, cx| {
                    match res {
                        Ok(out) => {
                            this.clear_plugin_ui_caches(&pid);
                            this.status_banner = Some((
                                format!("{} v{}", out.manifest.name, out.manifest.version),
                                false,
                            ));
                        }
                        Err(err) => {
                            this.status_banner = Some((format!("{err:#}"), true));
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
            cx.notify();
            return;
        }

        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            cx.notify();
            return;
        };
        let pid = pending.plugin_id;
        let task = cx.background_spawn(async move { mgr.approve_permissions(&pid) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(err) = res {
                    this.status_banner = Some((format!("{err:#}"), true));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn reset_and_enable_plugin(&mut self, plugin_id: String, cx: &mut Context<Self>) {
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        let task = cx.background_spawn(async move { mgr.reset_and_enable_plugin(&plugin_id) });
        cx.spawn(async move |this, cx| {
            if let Err(err) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.status_banner = Some((format!("{err:#}"), true));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn clear_plugin_ui_caches(&mut self, plugin_id: &str) {
        self.setting_inputs.retain(|(p, _), _| p != plugin_id);
        self.secret_inputs.retain(|(p, _), _| p != plugin_id);
        self.in_flight_secrets.retain(|(p, _)| p != plugin_id);
        self.pending_setting_overrides.remove(plugin_id);
        self.setting_update_in_flight.remove(plugin_id);
        self.setting_update_dirty.remove(plugin_id);
        if self
            .open_select_setting
            .as_ref()
            .is_some_and(|(p, _)| p == plugin_id)
        {
            self.open_select_setting = None;
        }
        if self
            .pending_permission_modal
            .as_ref()
            .is_some_and(|m| m.plugin_id == plugin_id)
        {
            self.pending_permission_modal = None;
        }
    }

    pub(crate) fn uninstall_plugin(&mut self, plugin_id: String, cx: &mut Context<Self>) {
        let Some(client) = PluginHost::global(cx).map(|h| h.registry_client().clone()) else {
            return;
        };
        if self
            .active_plugin_page
            .as_ref()
            .is_some_and(|p| p.plugin_id == plugin_id)
        {
            self.close_plugin_page(cx);
        }
        let mgr = PluginHost::global(cx).and_then(|h| h.manager().cloned());
        let pid = plugin_id.clone();
        let task = cx.background_spawn(async move {
            if let Some(mgr) = &mgr {
                let _ = mgr.set_plugin_enabled(&pid, false);
                // INTEGRATION(land-all): once `runtime-fix` merges, wire `mgr.purge_plugin_data(&pid)?;`
                // here to wipe `SecretsVault` entries alongside DB state/KV/entity metadata on the worker.
            }
            client.uninstall_plugin(&pid)?;
            if let Some(mgr) = &mgr {
                let _ = mgr.reload_all()?;
            }
            Ok::<_, anyhow::Error>(())
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        // Purge persisted plugin DB state, KV, and entity metadata (also covers Safe Mode
                        // where `mgr` is None). `land-all` wires `mgr.purge_plugin_data(&pid)` above for
                        // `SecretsVault` revocation.
                        let _ = this.store.delete_plugin_state(&plugin_id);
                        let _ = this.store.plugin_kv_clear(&plugin_id);
                        let _ = this.store.plugin_meta_clear(&plugin_id);
                        this.clear_plugin_ui_caches(&plugin_id);
                        if this.expanded_settings.as_deref() == Some(&plugin_id) {
                            this.expanded_settings = None;
                        }
                        if this.expanded_logs.as_deref() == Some(&plugin_id) {
                            this.expanded_logs = None;
                        }
                        this.plugin_logs.remove(&plugin_id);
                    }
                    Err(err) => {
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fetch_plugin_logs_async(&mut self, plugin_id: String, cx: &mut Context<Self>) {
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        let pid = plugin_id.clone();
        let task = cx.background_spawn(async move { mgr.plugin_logs(&pid) });
        cx.spawn(async move |this, cx| {
            if let Ok(logs) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.plugin_logs.insert(plugin_id, logs);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn clear_plugin_logs_async(&mut self, plugin_id: String, cx: &mut Context<Self>) {
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        let pid = plugin_id.clone();
        let task = cx.background_spawn(async move { mgr.clear_plugin_logs(&pid) });
        cx.spawn(async move |this, cx| {
            if task.await.is_ok() {
                let _ = this.update(cx, |this, cx| {
                    this.plugin_logs.insert(plugin_id, Vec::new());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn effective_setting_value<'a>(&'a self, info: &'a PluginInfo, key: &str) -> Option<&'a JsonValue> {
        self.pending_setting_overrides
            .get(&info.manifest.id)
            .and_then(|map| map.get(key))
            .or_else(|| info.settings_values.get(key))
    }

    fn update_plugin_setting_value(
        &mut self,
        plugin_id: &str,
        key: &str,
        value: JsonValue,
        cx: &mut Context<Self>,
    ) {
        if PluginHost::global(cx)
            .and_then(|h| h.manager())
            .is_none()
        {
            return;
        }
        self.pending_setting_overrides
            .entry(plugin_id.to_owned())
            .or_default()
            .insert(key.to_owned(), value);
        cx.notify();
        self.flush_plugin_settings(plugin_id, cx);
    }

    fn flush_plugin_settings(&mut self, plugin_id: &str, cx: &mut Context<Self>) {
        if self.setting_update_in_flight.contains(plugin_id) {
            self.setting_update_dirty.insert(plugin_id.to_owned());
            return;
        }
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        let mut merged = PluginHost::global(cx)
            .and_then(|h| h.plugin(plugin_id))
            .map(|p| p.settings_values.clone())
            .unwrap_or_default();
        let sent_overrides = self
            .pending_setting_overrides
            .get(plugin_id)
            .cloned()
            .unwrap_or_default();
        for (k, v) in &sent_overrides {
            merged.insert(k.clone(), v.clone());
        }
        let pid = plugin_id.to_owned();
        let merged_for_host = merged.clone();
        self.setting_update_in_flight.insert(pid.clone());
        let pid_bg = pid.clone();
        let task = cx.background_spawn(async move { mgr.update_settings(&pid_bg, merged) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.setting_update_in_flight.remove(&pid);
                match res {
                    Ok(()) => {
                        PluginHost::apply_local_settings(&pid, merged_for_host, cx);
                        if let Some(pending) = this.pending_setting_overrides.get_mut(&pid) {
                            for (k, v) in &sent_overrides {
                                if pending.get(k) == Some(v) {
                                    pending.remove(k);
                                }
                            }
                            if pending.is_empty() {
                                this.pending_setting_overrides.remove(&pid);
                            }
                        }
                    }
                    Err(err) => {
                        if let Some(pending) = this.pending_setting_overrides.get_mut(&pid) {
                            for (k, v) in &sent_overrides {
                                if pending.get(k) == Some(v) {
                                    pending.remove(k);
                                }
                            }
                            if pending.is_empty() {
                                this.pending_setting_overrides.remove(&pid);
                            }
                        }
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                if this.setting_update_dirty.remove(&pid) {
                    this.flush_plugin_settings(&pid, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_setting_input(
        &mut self,
        plugin_id: &str,
        spec: &SettingSpec,
        current_text: &str,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let map_key = (plugin_id.to_owned(), spec.key.clone());
        if let Some(entry) = self.setting_inputs.get_mut(&map_key) {
            if entry.last_synced != current_text && !entry.input.read(cx).text().eq(current_text) {
                // Only update external value when no active edit differs from last_synced.
                if entry.input.read(cx).text() == entry.last_synced {
                    let next = current_text.to_owned();
                    entry.last_synced = next.clone();
                    entry.input.update(cx, |inp, cx| inp.set_text(next, cx));
                }
            }
            return entry.input.clone();
        }

        let placeholder = spec.placeholder.clone().unwrap_or_default();
        let initial = current_text.to_owned();
        let input = cx.new(|cx| {
            let mut inp = TextInput::new(Mode::SingleLine, placeholder, cx);
            inp.set_text(initial, cx);
            inp
        });

        let pid_change = plugin_id.to_owned();
        let skey_change = spec.key.clone();
        let skind_change = spec.kind;
        let min_v = spec.min;
        let max_v = spec.max;
        let input_for_change = input.clone();
        let sub_changed = cx.subscribe(&input, move |this, _, _: &Changed, cx| {
            let pid = pid_change.clone();
            let skey = skey_change.clone();
            let inp = input_for_change.clone();
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SETTING_SAVE_DEBOUNCE).await;
                let _ = this.update(cx, |this, cx| {
                    let raw = inp.read(cx).text().to_owned();
                    if let Some(val) = parse_setting_input_value(skind_change, &raw, min_v, max_v) {
                        if let Some(entry) =
                            this.setting_inputs.get_mut(&(pid.clone(), skey.clone()))
                        {
                            entry.last_synced = raw;
                        }
                        this.update_plugin_setting_value(&pid, &skey, val, cx);
                    }
                });
            });
            if let Some(entry) = this
                .setting_inputs
                .get_mut(&(pid_change.clone(), skey_change.clone()))
            {
                entry.save_task = Some(task);
            }
        });

        let pid_sub = plugin_id.to_owned();
        let skey_sub = spec.key.clone();
        let skind_sub = spec.kind;
        let sub_submit = cx.subscribe(&input, move |this, _, ev: &Submitted, cx| {
            if let Some(entry) = this
                .setting_inputs
                .get_mut(&(pid_sub.clone(), skey_sub.clone()))
            {
                entry.save_task = None;
            }
            if let Some(val) = parse_setting_input_value(skind_sub, &ev.0, min_v, max_v) {
                if let Some(entry) = this
                    .setting_inputs
                    .get_mut(&(pid_sub.clone(), skey_sub.clone()))
                {
                    entry.last_synced = ev.0.clone();
                }
                this.update_plugin_setting_value(&pid_sub, &skey_sub, val, cx);
            }
        });

        self.setting_inputs.insert(
            map_key,
            SettingInputEntry {
                input: input.clone(),
                last_synced: current_text.to_owned(),
                save_task: None,
                _subs: [sub_changed, sub_submit],
            },
        );
        input
    }

    fn ensure_secret_input(
        &mut self,
        plugin_id: &str,
        spec: &SettingSpec,
        is_present: bool,
        strings: &i18n::Strings,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let map_key = (plugin_id.to_owned(), spec.key.clone());
        let desired_ph = if is_present {
            strings.plugins_secret_placeholder.to_owned()
        } else {
            spec.placeholder.clone().unwrap_or_default()
        };
        if let Some(entry) = self.secret_inputs.get_mut(&map_key) {
            if entry.placeholder != desired_ph {
                entry.placeholder = desired_ph.clone();
                entry
                    .input
                    .update(cx, |inp, cx| inp.set_placeholder(desired_ph, cx));
            }
            return entry.input.clone();
        }
        let ph_for_input = desired_ph.clone();
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, ph_for_input, cx).masked());
        self.secret_inputs.insert(
            map_key,
            SecretInputEntry {
                input: input.clone(),
                placeholder: desired_ph,
            },
        );
        input
    }

    fn save_secret_setting(&mut self, plugin_id: String, key: String, cx: &mut Context<Self>) {
        let map_key = (plugin_id.clone(), key.clone());
        if self.in_flight_secrets.contains(&map_key) {
            return;
        }
        let Some(input) = self.secret_inputs.get(&map_key).map(|e| e.input.clone()) else {
            return;
        };
        let val = input.read(cx).text().to_owned();
        if val.is_empty() {
            return;
        }
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        self.in_flight_secrets.insert(map_key.clone());
        cx.notify();
        let task = cx.background_spawn(async move { mgr.set_secret(&plugin_id, &key, &val) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.in_flight_secrets.remove(&map_key);
                match res {
                    Ok(()) => {
                        let placeholder = i18n::t(cx).plugins_secret_placeholder.to_owned();
                        if let Some(entry) = this.secret_inputs.get_mut(&map_key) {
                            entry.placeholder = placeholder.clone();
                            entry.input.update(cx, |inp, cx| {
                                inp.set_text("", cx);
                                inp.set_placeholder(placeholder, cx);
                            });
                        }
                        this.status_banner = None;
                    }
                    Err(err) => {
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete_secret_setting(&mut self, plugin_id: String, key: String, cx: &mut Context<Self>) {
        let map_key = (plugin_id.clone(), key.clone());
        if self.in_flight_secrets.contains(&map_key) {
            return;
        }
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        self.in_flight_secrets.insert(map_key.clone());
        cx.notify();
        let task = cx.background_spawn(async move { mgr.delete_secret(&plugin_id, &key) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.in_flight_secrets.remove(&map_key);
                if let Err(err) = res {
                    this.status_banner = Some((format!("{err:#}"), true));
                }
                cx.notify();
            });
        })
        .detach();
    }

    // -------------------------------------------------------------------------
    // Scripts tab actions
    // -------------------------------------------------------------------------

    fn load_script_into_editor(&mut self, script: &UserScript, cx: &mut Context<Self>) {
        self.selected_script_id = Some(script.id.clone());
        self.script_enabled = script.enabled;
        self.script_trigger_kind = if script.trigger_kind.is_empty() {
            "manual".to_owned()
        } else {
            script.trigger_kind.clone()
        };
        self.script_permissions = PermissionSet::from_json_str(&script.permissions)
            .unwrap_or_default()
            .to_vec()
            .into_iter()
            .map(|p| p.as_str().to_owned())
            .collect();
        let name = script.name.clone();
        let arg = script.trigger_arg.clone();
        let code = script.code.clone();
        self.script_name_input
            .update(cx, |inp, cx| inp.set_text(name, cx));
        self.script_trigger_arg_input
            .update(cx, |inp, cx| inp.set_text(arg, cx));
        self.script_code_input
            .update(cx, |inp, cx| inp.set_text(code, cx));
        self.script_run_seq = self.script_run_seq.wrapping_add(1);
        self.script_running = false;
        self.script_result = None;
        cx.notify();
    }

    fn reset_new_script_editor(&mut self, cx: &mut Context<Self>) {
        self.selected_script_id = None;
        self.script_enabled = true;
        self.script_trigger_kind = "manual".to_owned();
        self.script_permissions = BTreeSet::from(["tasks:read".to_owned()]);
        self.script_name_input
            .update(cx, |inp, cx| inp.set_text("New Script", cx));
        self.script_trigger_arg_input
            .update(cx, |inp, cx| inp.set_text("", cx));
        self.script_code_input.update(cx, |inp, cx| {
            inp.set_text(
                "const open = fern.tasks.list({ completed: false });\nconsole.log(\"Open tasks:\", open.length);\nreturn { open: open.length };\n",
                cx,
            );
        });
        self.script_run_seq = self.script_run_seq.wrapping_add(1);
        self.script_running = false;
        self.script_result = None;
        cx.notify();
    }

    fn build_draft_script(&self, cx: &App) -> UserScript {
        let id = self
            .selected_script_id
            .clone()
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        let name = self.script_name_input.read(cx).text().trim().to_owned();
        let final_name = if name.is_empty() {
            "Untitled Script".to_owned()
        } else {
            name
        };
        let perms: Vec<String> = self.script_permissions.iter().cloned().collect();
        let perms_json = serde_json::to_string(&perms).unwrap_or_else(|_| "[]".to_owned());

        UserScript {
            id,
            name: final_name,
            enabled: self.script_enabled,
            trigger_kind: self.script_trigger_kind.clone(),
            trigger_arg: self
                .script_trigger_arg_input
                .read(cx)
                .text()
                .trim()
                .to_owned(),
            permissions: perms_json,
            code: self.script_code_input.read(cx).text().to_owned(),
            updated_at: 0,
        }
    }

    fn save_current_script(&mut self, cx: &mut Context<Self>) {
        let draft = self.build_draft_script(cx);
        match self.store.save_user_script(&draft) {
            Ok(()) => {
                self.selected_script_id = Some(draft.id);
                self.scripts = self.store.user_scripts().unwrap_or_default();
                self.status_banner = None;
                data_changed(cx.entity_id(), cx);
            }
            Err(err) => {
                self.status_banner = Some((format!("{err:#}"), true));
            }
        }
        cx.notify();
    }

    fn delete_current_script(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_script_id.clone() else {
            return;
        };
        match self.store.delete_user_script(&id) {
            Ok(_) => {
                self.scripts = self.store.user_scripts().unwrap_or_default();
                if let Some(first) = self.scripts.first().cloned() {
                    self.load_script_into_editor(&first, cx);
                } else {
                    self.reset_new_script_editor(cx);
                }
                data_changed(cx.entity_id(), cx);
            }
            Err(err) => {
                self.status_banner = Some((format!("{err:#}"), true));
            }
        }
        cx.notify();
    }

    fn test_run_current_script(&mut self, cx: &mut Context<Self>) {
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            self.status_banner = Some((i18n::t(cx).plugins_safe_mode_active.to_owned(), true));
            cx.notify();
            return;
        };
        let draft = self.build_draft_script(cx);
        self.script_run_seq = self.script_run_seq.wrapping_add(1);
        let run_seq = self.script_run_seq;
        self.script_running = true;
        self.script_result = None;
        cx.notify();

        let task = cx.background_spawn(async move { mgr.test_run_script(draft, None) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(run_res) = &res
                    && run_res.data_changed
                {
                    data_changed(cx.entity_id(), cx);
                }
                if this.script_run_seq != run_seq {
                    return;
                }
                this.script_running = false;
                match res {
                    Ok(run_res) => {
                        this.script_result = Some(run_res);
                    }
                    Err(err) => {
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // -------------------------------------------------------------------------
    // Browse tab actions
    // -------------------------------------------------------------------------

    fn load_cached_registries_async(&mut self, cx: &mut Context<Self>) {
        let Some(client) = PluginHost::global(cx).map(|h| h.registry_client().clone()) else {
            return;
        };
        let enabled_urls: Vec<String> = self
            .registries
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.url.clone())
            .collect();
        let generation = self.browse_generation;
        let fetch_seq = self.browse_fetch_seq;

        let task = cx.background_spawn(async move {
            let mut items = Vec::new();
            for url in enabled_urls {
                if let Ok(Some(cached)) = client.load_cached_index(&url) {
                    for entry in cached.index.plugins {
                        items.push(CatalogItem {
                            source_url: url.clone(),
                            entry,
                        });
                    }
                }
            }
            items
        });
        cx.spawn(async move |this, cx| {
            let items = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.browse_generation == generation
                    && this.browse_completed_fetch_seq <= fetch_seq
                {
                    this.catalog_items = items;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn refresh_registries(&mut self, cx: &mut Context<Self>) {
        let Some(client) = PluginHost::global(cx).map(|h| h.registry_client().clone()) else {
            return;
        };
        let sources = self.registries.clone();
        self.browse_fetch_seq = self.browse_fetch_seq.wrapping_add(1);
        let generation = self.browse_generation;
        let fetch_seq = self.browse_fetch_seq;
        self.browse_loading = true;
        self.registry_errors.clear();
        cx.notify();

        let task = cx.background_spawn(async move { client.fetch_all_resilient(&sources) });
        cx.spawn(async move |this, cx| {
            let results = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.browse_generation != generation || this.browse_fetch_seq != fetch_seq {
                    return;
                }
                this.browse_loading = false;
                this.browse_completed_fetch_seq = fetch_seq;
                let mut items = Vec::new();
                let mut errors = Vec::new();
                for (url, res) in results {
                    match res {
                        Ok(idx) => {
                            for entry in idx.index.plugins {
                                items.push(CatalogItem {
                                    source_url: url.clone(),
                                    entry,
                                });
                            }
                        }
                        Err(err) => {
                            errors.push(format!("{url}: {err:#}"));
                        }
                    }
                }
                this.catalog_items = items;
                this.registry_errors = errors;
                cx.notify();
            });
        })
        .detach();
    }

    fn save_registries(&mut self) {
        if let Ok(json) = serde_json::to_string(&self.registries) {
            let _ = self.store.set_setting(REGISTRIES_SETTING_KEY, &json);
        }
    }

    fn add_registry_url(&mut self, cx: &mut Context<Self>) {
        let raw = self.registry_url_input.read(cx).text().trim().to_owned();
        if raw.is_empty() {
            return;
        }
        let lower = raw.to_ascii_lowercase();
        if !(lower.starts_with("https://") || lower.starts_with("http://")) {
            self.status_banner = Some(("URL must start with https:// or http://".to_owned(), true));
            cx.notify();
            return;
        }
        if !self.registries.iter().any(|s| s.url == raw) {
            self.registries.push(RegistrySource {
                url: raw,
                name: None,
                enabled: true,
            });
            self.browse_generation = self.browse_generation.wrapping_add(1);
            self.save_registries();
        }
        self.registry_url_input
            .update(cx, |inp, cx| inp.set_text("", cx));
        self.status_banner = None;
        self.refresh_registries(cx);
    }

    fn remove_registry_source(&mut self, url: &str, cx: &mut Context<Self>) {
        self.registries.retain(|s| s.url != url);
        if self.registries.is_empty() {
            self.registries.push(RegistrySource::default());
        }
        self.browse_generation = self.browse_generation.wrapping_add(1);
        self.browse_fetch_seq = self.browse_fetch_seq.wrapping_add(1);
        self.browse_loading = false;
        self.save_registries();
        self.load_cached_registries_async(cx);
        cx.notify();
    }

    fn install_from_registry(&mut self, entry: RegistryPluginEntry, cx: &mut Context<Self>) {
        let Some(client) = PluginHost::global(cx).map(|h| h.registry_client().clone()) else {
            return;
        };
        let pid = entry.id.clone();
        if !self.installing_plugin_ids.insert(pid.clone()) {
            return;
        }
        let mgr = PluginHost::global(cx).and_then(|h| h.manager().cloned());
        let existing_state = self.store.plugin_state(&entry.id).ok().flatten();
        cx.notify();

        let task = cx.background_spawn(async move {
            let outcome = client.install_from_registry(&entry, existing_state.as_ref())?;
            if let Some(mgr) = mgr {
                let _ = mgr.reload_all()?;
            }
            Ok::<_, anyhow::Error>(outcome)
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.installing_plugin_ids.remove(&pid);
                match res {
                    Ok(out) => {
                        this.status_banner = Some((
                            format!("{} v{}", out.manifest.name, out.manifest.version),
                            false,
                        ));
                        if let PermissionApprovalStatus::NeedsApproval {
                            diff,
                            is_first_install,
                        } = out.approval_status
                        {
                            this.pending_permission_modal = Some(PermissionPromptState {
                                plugin_id: out.manifest.id,
                                plugin_name: out.manifest.name,
                                version: out.manifest.version,
                                added_permissions: diff.added,
                                is_first_install,
                                pending_zip_overwrite: None,
                            });
                        }
                    }
                    Err(err) => {
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // -------------------------------------------------------------------------
    // Rendering
    // -------------------------------------------------------------------------

    fn render_installed_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let locale = i18n::locale(cx);
        let ex = ExtraLabels::for_locale(locale);
        let theme = cx.theme().clone();
        let colors = theme.colors();

        let safe_mode_active = PluginHost::global(cx).is_some_and(|h| h.is_safe_mode());
        let plugins: Vec<PluginInfo> = PluginHost::global(cx)
            .map(|h| h.plugins().to_vec())
            .unwrap_or_default();

        let safe_mode_card = v_flex()
            .overflow_hidden()
            .rounded(px(8.))
            .bg(colors.surface_background)
            .border_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .id("plugins-safe-mode-toggle")
                    .px_4()
                    .py_3()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.ghost_element_hover))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_safe_mode_pref(cx)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(Label::new(strings.plugins_safe_mode).weight(FontWeight::MEDIUM))
                            .child(
                                Label::new(strings.plugins_safe_mode_hint)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(Switch::new().toggle_state(self.safe_mode_pref)),
            )
            .when(safe_mode_active, |col| {
                col.child(Divider::horizontal()).child(
                    h_flex()
                        .px_4()
                        .py_2p5()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::CircleAlert)
                                .size(IconSize::Small)
                                .color(Color::Warning),
                        )
                        .child(
                            Label::new(strings.plugins_safe_mode_active)
                                .size(LabelSize::Small)
                                .color(Color::Warning),
                        ),
                )
            });

        let toolbar = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("plugins-new-template", strings.plugins_new_template)
                            .variant(ButtonVariant::Primary)
                            .size(ButtonSize::Sm)
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, _, cx| this.create_sample_plugin(cx))),
                    )
                    .child(
                        Button::new("plugins-install-zip", strings.plugins_install_zip)
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .icon(IconName::FolderOpen)
                            .on_click(cx.listener(|this, _, _, cx| this.install_from_zip_dialog(cx))),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        IconButton::new("plugins-open-folder", IconName::Folder)
                            .size(px(30.))
                            .icon_size(IconSize::Small)
                            .icon_color(Color::IconMuted)
                            .hover_icon_color(Color::Icon)
                            .tooltip(Tooltip::text(strings.plugins_open_folder))
                            .on_click(cx.listener(|this, _, _, cx| this.open_plugins_folder(cx))),
                    )
                    .child(
                        IconButton::new("plugins-reload-all", IconName::RefreshCw)
                            .size(px(30.))
                            .icon_size(IconSize::Small)
                            .icon_color(Color::IconMuted)
                            .hover_icon_color(Color::Icon)
                            .tooltip(Tooltip::text(strings.plugins_reload_all))
                            .disabled(safe_mode_active)
                            .when(!safe_mode_active, |btn| {
                                btn.on_click(|_, _, cx| PluginHost::reload_all_async(cx))
                            }),
                    ),
            );

        let cards: Vec<AnyElement> = if plugins.is_empty() {
            vec![
                div()
                    .w_full()
                    .py_10()
                    .child(EmptyState::new(IconName::Layers, strings.plugins_empty))
                    .into_any_element(),
            ]
        } else {
            plugins
                .iter()
                .map(|info| self.render_plugin_card(info, &ex, &theme, cx))
                .collect()
        };

        v_flex()
            .w_full()
            .gap_4()
            .child(safe_mode_card)
            .child(toolbar)
            .children(cards)
            .into_any_element()
    }

    fn render_plugin_card(
        &mut self,
        info: &PluginInfo,
        ex: &ExtraLabels,
        theme: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = theme.colors();
        let status_colors = theme.status();
        let pid = info.manifest.id.clone();

        let (status_label, status_color) = match &info.status {
            PluginStatus::Active => (ex.status_active, status_colors.success),
            PluginStatus::Idle => (ex.status_idle, status_colors.info),
            PluginStatus::Disabled => (crate::status_disabled_label(i18n::locale(cx)), colors.text_muted),
            PluginStatus::NeedsApproval { .. } => {
                (strings.plugins_permissions_approve, status_colors.warning)
            }
            PluginStatus::CircuitBroken { .. } => (ex.status_broken, status_colors.error),
            PluginStatus::Error(_) => (ex.status_error, status_colors.error),
        };

        let metrics_text = strings
            .plugins_metrics
            .replace("{mem}", &format_bytes(info.metrics.memory_bytes))
            .replace("{ms}", &info.metrics.last_exec_ms.to_string());

        let primary_view = info
            .manifest
            .contributions
            .primary_view()
            .map(|(vid, _)| vid.to_owned());
        let can_open_page = primary_view.is_some()
            && info.enabled
            && !matches!(
                info.status,
                PluginStatus::NeedsApproval { .. }
                    | PluginStatus::CircuitBroken { .. }
                    | PluginStatus::Disabled
            );
        let has_settings = !info.manifest.settings.is_empty();
        let settings_open = self.expanded_settings.as_deref() == Some(pid.as_str());
        let logs_open = self.expanded_logs.as_deref() == Some(pid.as_str());

        let info_toggle = info.clone();
        let pid_toggle = pid.clone();
        let pid_open = pid.clone();
        let name_open = info.manifest.name.clone();
        let pid_reload = pid.clone();
        let pid_settings = pid.clone();
        let pid_logs = pid.clone();
        let pid_uninstall = pid.clone();
        let pid_reset = pid.clone();

        let mut card = v_flex()
            .w_full()
            .p_4()
            .gap_3()
            .rounded(px(8.))
            .bg(colors.surface_background)
            .border_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_wrap()
                                    .child(
                                        Label::new(info.manifest.name.clone())
                                            .weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(
                                        Label::new(format!("v{}", info.manifest.version))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .child(Chip::new(status_label, status_color)),
                            )
                            .when(!info.manifest.description.is_empty(), |col| {
                                col.child(
                                    Label::new(info.manifest.description.clone())
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                            })
                            .child(
                                Label::new(format!(
                                    "{} · {}",
                                    info.manifest.id, metrics_text
                                ))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .id(ElementId::Name(SharedString::from(format!(
                                        "plugin-enable-{pid_toggle}"
                                    ))))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_plugin_enabled(&info_toggle, cx);
                                    }))
                                    .child(Switch::new().toggle_state(info.enabled)),
                            ),
                    ),
            );

        if let PluginStatus::NeedsApproval {
            added_permissions,
            is_first_install,
        } = &info.status
        {
            let prompt_state = PermissionPromptState {
                plugin_id: pid.clone(),
                plugin_name: info.manifest.name.clone(),
                version: info.manifest.version.clone(),
                added_permissions: added_permissions.clone(),
                is_first_install: *is_first_install,
                pending_zip_overwrite: None,
            };
            card = card.child(
                h_flex()
                    .p_2p5()
                    .gap_2()
                    .justify_between()
                    .rounded(px(6.))
                    .bg(status_colors.warning.opacity(0.12))
                    .child(
                        Label::new(strings.plugins_permissions_changed)
                            .size(LabelSize::Small)
                            .color(Color::Warning),
                    )
                    .child(
                        Button::new(
                            ElementId::Name(SharedString::from(format!(
                                "plugin-approve-{pid}"
                            ))),
                            strings.plugins_permissions_approve,
                        )
                        .variant(ButtonVariant::Primary)
                        .size(ButtonSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pending_permission_modal = Some(prompt_state.clone());
                            cx.notify();
                        })),
                    ),
            );
        }

        if let PluginStatus::CircuitBroken { last_error } = &info.status {
            card = card.child(
                h_flex()
                    .p_2p5()
                    .gap_2()
                    .justify_between()
                    .rounded(px(6.))
                    .bg(status_colors.error.opacity(0.12))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                Label::new(strings.plugins_circuit_broken)
                                    .size(LabelSize::Small)
                                    .color(Color::Error),
                            )
                            .child(
                                Label::new(last_error.clone())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .truncate(),
                            ),
                    )
                    .child(
                        Button::new(
                            ElementId::Name(SharedString::from(format!(
                                "plugin-reset-{pid_reset}"
                            ))),
                            strings.plugins_reset_enable,
                        )
                        .variant(ButtonVariant::Primary)
                        .size(ButtonSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reset_and_enable_plugin(pid_reset.clone(), cx);
                        })),
                    ),
            );
        } else if let PluginStatus::Error(err) = &info.status {
            card = card.child(
                Label::new(err.clone())
                    .size(LabelSize::XSmall)
                    .color(Color::Error),
            );
        }

        if !info.manifest.permissions.is_empty() {
            card = card.child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        Label::new(format!("{}:", strings.plugins_permissions))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .children(info.manifest.permissions.iter().map(|perm| {
                        Chip::new(perm.summary(), colors.accent)
                    })),
            );
        }

        card = card.child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .when_some(primary_view.filter(|_| can_open_page), |row, view_id| {
                    row.child(
                        Button::new(
                            ElementId::Name(SharedString::from(format!(
                                "plugin-open-{pid_open}"
                            ))),
                            strings.plugins_open,
                        )
                        .variant(ButtonVariant::Secondary)
                        .size(ButtonSize::Sm)
                        .icon(IconName::SquareArrowOutUpRight)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_plugin_page(
                                pid_open.clone(),
                                name_open.clone(),
                                view_id.clone(),
                                cx,
                            );
                        })),
                    )
                })
                .child(
                    Button::new(
                        ElementId::Name(SharedString::from(format!("plugin-reload-{pid_reload}"))),
                        strings.plugins_reload,
                    )
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(IconName::RefreshCw)
                    .on_click(move |_, _, cx| {
                        PluginHost::reload_plugin_async(pid_reload.clone(), cx);
                    }),
                )
                .when(has_settings, |row| {
                    row.child(
                        Button::new(
                            ElementId::Name(SharedString::from(format!(
                                "plugin-settings-btn-{pid_settings}"
                            ))),
                            strings.plugins_settings,
                        )
                        .variant(if settings_open {
                            ButtonVariant::Secondary
                        } else {
                            ButtonVariant::Ghost
                        })
                        .size(ButtonSize::Sm)
                        .icon(IconName::Settings2)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.expanded_settings.as_deref() == Some(pid_settings.as_str()) {
                                this.expanded_settings = None;
                            } else {
                                this.expanded_settings = Some(pid_settings.clone());
                            }
                            cx.notify();
                        })),
                    )
                })
                .child(
                    Button::new(
                        ElementId::Name(SharedString::from(format!("plugin-logs-btn-{pid_logs}"))),
                        strings.plugins_logs,
                    )
                    .variant(if logs_open {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Ghost
                    })
                    .size(ButtonSize::Sm)
                    .icon(IconName::FileText)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.expanded_logs.as_deref() == Some(pid_logs.as_str()) {
                            this.expanded_logs = None;
                        } else {
                            this.expanded_logs = Some(pid_logs.clone());
                            this.fetch_plugin_logs_async(pid_logs.clone(), cx);
                        }
                        cx.notify();
                    })),
                )
                .child(
                    Button::new(
                        ElementId::Name(SharedString::from(format!(
                            "plugin-uninstall-{pid_uninstall}"
                        ))),
                        strings.plugins_uninstall,
                    )
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(IconName::Trash)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.uninstall_plugin(pid_uninstall.clone(), cx);
                    })),
                ),
        );

        if settings_open && has_settings {
            let form = self.render_plugin_settings_form(info, ex, cx);
            card = card.child(Divider::horizontal()).child(form);
        }

        if logs_open {
            let logs_panel = self.render_plugin_logs_panel(&pid, ex, theme, cx);
            card = card.child(Divider::horizontal()).child(logs_panel);
        }

        card.into_any_element()
    }

    fn render_plugin_settings_form(
        &mut self,
        info: &PluginInfo,
        ex: &ExtraLabels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let pid = info.manifest.id.clone();
        let mut rows = Vec::new();

        for spec in &info.manifest.settings {
            let row = match spec.kind {
                SettingKind::Text => {
                    let current = self
                        .effective_setting_value(info, &spec.key)
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let input = self.ensure_setting_input(&pid, spec, &current, cx);
                    setting_row_shell(
                        spec,
                        div()
                            .w_full()
                            .h(px(30.))
                            .px_2p5()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .rounded(px(6.))
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border)
                            .child(input)
                            .into_any_element(),
                    )
                }
                SettingKind::Number => {
                    let num = self
                        .effective_setting_value(info, &spec.key)
                        .and_then(JsonValue::as_f64)
                        .unwrap_or(0.0);
                    let current_str = format_number_clean(num);
                    let input = self.ensure_setting_input(&pid, spec, &current_str, cx);
                    let step = spec.step.unwrap_or(1.0);
                    let min_v = spec.min;
                    let max_v = spec.max;
                    let pid_dec = pid.clone();
                    let key_dec = spec.key.clone();
                    let pid_inc = pid.clone();
                    let key_inc = spec.key.clone();
                    let inp_dec = input.clone();
                    let inp_inc = input.clone();

                    setting_row_shell(
                        spec,
                        h_flex()
                            .gap_1p5()
                            .child(
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "set-num-dec-{pid}-{}",
                                        spec.key
                                    ))),
                                    IconName::Minus,
                                )
                                .size(px(28.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let current = inp_dec
                                        .read(cx)
                                        .text()
                                        .trim()
                                        .parse::<f64>()
                                        .ok()
                                        .filter(|v| v.is_finite())
                                        .unwrap_or(num);
                                    let next = clamp_number(current - step, min_v, max_v);
                                    let text = format_number_clean(next);
                                    inp_dec.update(cx, |inp, cx| inp.set_text(text.clone(), cx));
                                    if let Some(entry) =
                                        this.setting_inputs.get_mut(&(pid_dec.clone(), key_dec.clone()))
                                    {
                                        entry.save_task = None;
                                        entry.last_synced = text;
                                    }
                                    if let Some(n) = serde_json::Number::from_f64(next) {
                                        this.update_plugin_setting_value(
                                            &pid_dec,
                                            &key_dec,
                                            JsonValue::Number(n),
                                            cx,
                                        );
                                    }
                                })),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(72.))
                                    .h(px(30.))
                                    .px_2p5()
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .rounded(px(6.))
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .child(input),
                            )
                            .child(
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "set-num-inc-{pid}-{}",
                                        spec.key
                                    ))),
                                    IconName::Plus,
                                )
                                .size(px(28.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let current = inp_inc
                                        .read(cx)
                                        .text()
                                        .trim()
                                        .parse::<f64>()
                                        .ok()
                                        .filter(|v| v.is_finite())
                                        .unwrap_or(num);
                                    let next = clamp_number(current + step, min_v, max_v);
                                    let text = format_number_clean(next);
                                    inp_inc.update(cx, |inp, cx| inp.set_text(text.clone(), cx));
                                    if let Some(entry) =
                                        this.setting_inputs.get_mut(&(pid_inc.clone(), key_inc.clone()))
                                    {
                                        entry.save_task = None;
                                        entry.last_synced = text;
                                    }
                                    if let Some(n) = serde_json::Number::from_f64(next) {
                                        this.update_plugin_setting_value(
                                            &pid_inc,
                                            &key_inc,
                                            JsonValue::Number(n),
                                            cx,
                                        );
                                    }
                                })),
                            )
                            .into_any_element(),
                    )
                }
                SettingKind::Boolean => {
                    let checked = self
                        .effective_setting_value(info, &spec.key)
                        .and_then(JsonValue::as_bool)
                        .unwrap_or(false);
                    let pid_b = pid.clone();
                    let key_b = spec.key.clone();
                    setting_row_shell(
                        spec,
                        div()
                            .id(ElementId::Name(SharedString::from(format!(
                                "set-bool-{pid_b}-{key_b}"
                            ))))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_plugin_setting_value(
                                    &pid_b,
                                    &key_b,
                                    JsonValue::Bool(!checked),
                                    cx,
                                );
                            }))
                            .child(Switch::new().toggle_state(checked))
                            .into_any_element(),
                    )
                }
                SettingKind::Select => {
                    let current_val = self
                        .effective_setting_value(info, &spec.key)
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let current_lbl = spec
                        .options
                        .iter()
                        .find(|o| o.value == current_val)
                        .map(|o| o.label.clone())
                        .unwrap_or_else(|| current_val.clone());
                    let is_open = self.open_select_setting.as_ref()
                        == Some(&(pid.clone(), spec.key.clone()));
                    let pid_open = pid.clone();
                    let key_open = spec.key.clone();
                    let menu = if is_open {
                        let items: Vec<AnyElement> = spec
                            .options
                            .iter()
                            .enumerate()
                            .map(|(i, opt)| {
                                let selected = opt.value == current_val;
                                let val = opt.value.clone();
                                let pid_pick = pid.clone();
                                let key_pick = spec.key.clone();
                                let mut item = ListItem::new(ElementId::Name(SharedString::from(
                                    format!("set-sel-{pid}-{}-{i}", spec.key),
                                )))
                                .toggle_state(selected);
                                if selected {
                                    item = item.end_slot(
                                        Icon::new(IconName::Check)
                                            .size(IconSize::XSmall)
                                            .color(Color::Accent),
                                    );
                                }
                                item.on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_select_setting = None;
                                    this.update_plugin_setting_value(
                                        &pid_pick,
                                        &key_pick,
                                        JsonValue::String(val.clone()),
                                        cx,
                                    );
                                    cx.notify();
                                }))
                                .child(Label::new(opt.label.clone()).size(LabelSize::Small))
                                .into_any_element()
                            })
                            .collect();
                        let scroll_id = ElementId::Name(SharedString::from(format!(
                            "set-sel-scroll-{pid}-{}",
                            spec.key
                        )));
                        let vp_id = ElementId::Name(SharedString::from(format!(
                            "set-sel-vp-{pid}-{}",
                            spec.key
                        )));
                        Some(PopoverAttachment::new(
                            Placement::BelowRight,
                            popover(cx)
                                .min_w(px(160.))
                                .max_w(px(280.))
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.open_select_setting = None;
                                    cx.stop_propagation();
                                    cx.notify();
                                }))
                                .child(
                                    v_scroll(scroll_id, v_flex().id(vp_id).w_full().children(items))
                                        .compact()
                                        .max_h(px(260.))
                                        .w_full(),
                                ),
                        ))
                    } else {
                        None
                    };

                    setting_row_shell(
                        spec,
                        div()
                            .relative()
                            .child(
                                h_flex()
                                    .id(ElementId::Name(SharedString::from(format!(
                                        "set-sel-btn-{pid}-{}",
                                        spec.key
                                    ))))
                                    .h(px(30.))
                                    .px_2p5()
                                    .gap_2()
                                    .justify_between()
                                    .rounded(px(6.))
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors.element_hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let target = (pid_open.clone(), key_open.clone());
                                        if this.open_select_setting.as_ref() == Some(&target) {
                                            this.open_select_setting = None;
                                        } else {
                                            this.open_select_setting = Some(target);
                                        }
                                        cx.notify();
                                    }))
                                    .child(Label::new(current_lbl).size(LabelSize::Small).truncate())
                                    .child(
                                        Icon::new(IconName::ChevronDown)
                                            .size(IconSize::XSmall)
                                            .color(Color::IconMuted),
                                    ),
                            )
                            .children(menu)
                            .into_any_element(),
                    )
                }
                SettingKind::Secret => {
                    let is_present = info
                        .secret_keys_present
                        .get(&spec.key)
                        .copied()
                        .unwrap_or(false);
                    let secret_busy = self
                        .in_flight_secrets
                        .contains(&(pid.clone(), spec.key.clone()));
                    let input = self.ensure_secret_input(&pid, spec, is_present, strings, cx);
                    let pid_save = pid.clone();
                    let key_save = spec.key.clone();
                    let pid_del = pid.clone();
                    let key_del = spec.key.clone();

                    setting_row_shell(
                        spec,
                        h_flex()
                            .gap_1p5()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(30.))
                                    .px_2p5()
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .rounded(px(6.))
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .child(input),
                            )
                            .child(
                                Button::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "set-secret-save-{pid}-{}",
                                        spec.key
                                    ))),
                                    strings.save,
                                )
                                .variant(ButtonVariant::Secondary)
                                .size(ButtonSize::Sm)
                                .disabled(secret_busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_secret_setting(
                                        pid_save.clone(),
                                        key_save.clone(),
                                        cx,
                                    );
                                })),
                            )
                            .when(is_present, |row| {
                                row.child(
                                    IconButton::new(
                                        ElementId::Name(SharedString::from(format!(
                                            "set-secret-del-{pid}-{}",
                                            spec.key
                                        ))),
                                        IconName::Trash,
                                    )
                                    .size(px(28.))
                                    .disabled(secret_busy)
                                    .tooltip(Tooltip::text(ex.clear_secret))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.delete_secret_setting(
                                            pid_del.clone(),
                                            key_del.clone(),
                                            cx,
                                        );
                                    })),
                                )
                            })
                            .into_any_element(),
                    )
                }
            };
            rows.push(row);
        }

        v_flex().w_full().gap_2p5().pt_1().children(rows).into_any_element()
    }

    fn render_plugin_logs_panel(
        &self,
        plugin_id: &str,
        ex: &ExtraLabels,
        theme: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = theme.colors();
        let logs = self
            .plugin_logs
            .get(plugin_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let pid_refresh = plugin_id.to_owned();
        let pid_clear = plugin_id.to_owned();

        v_flex()
            .w_full()
            .gap_2()
            .pt_1()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        Label::new(strings.plugins_logs)
                            .size(LabelSize::Small)
                            .weight(FontWeight::MEDIUM),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "logs-refresh-{plugin_id}"
                                    ))),
                                    IconName::RefreshCw,
                                )
                                .size(px(24.))
                                .icon_size(IconSize::XSmall)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.fetch_plugin_logs_async(pid_refresh.clone(), cx);
                                })),
                            )
                            .child(
                                Button::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "logs-clear-{plugin_id}"
                                    ))),
                                    strings.plugins_clear_logs,
                                )
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.clear_plugin_logs_async(pid_clear.clone(), cx);
                                })),
                            ),
                    ),
            )
            .child(render_log_entries_box(
                logs,
                ex.no_logs,
                colors,
                theme.status(),
            ))
            .into_any_element()
    }

    fn render_scripts_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let ex = ExtraLabels::for_locale(i18n::locale(cx));
        let theme = cx.theme().clone();
        let colors = theme.colors();

        let list_items: Vec<AnyElement> = if self.scripts.is_empty() {
            vec![
                div()
                    .p_3()
                    .child(
                        Label::new(strings.plugins_script_empty)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            ]
        } else {
            self.scripts
                .iter()
                .map(|script| {
                    let selected =
                        self.selected_script_id.as_deref() == Some(script.id.as_str());
                    let sc = script.clone();
                    let sid_toggle = script.id.clone();
                    let enabled = script.enabled;
                    ListItem::new(ElementId::Name(SharedString::from(format!(
                        "script-row-{}",
                        script.id
                    ))))
                    .toggle_state(selected)
                    .height(px(38.))
                    .end_slot(
                        h_flex()
                            .gap_2()
                            .child(Chip::new(script.trigger_kind.clone(), colors.accent))
                            .child(
                                div()
                                    .id(ElementId::Name(SharedString::from(format!(
                                        "script-enable-{}",
                                        script.id
                                    ))))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let _ = this
                                            .store
                                            .set_user_script_enabled(&sid_toggle, !enabled);
                                        this.scripts =
                                            this.store.user_scripts().unwrap_or_default();
                                        if this.selected_script_id.as_deref()
                                            == Some(sid_toggle.as_str())
                                        {
                                            this.script_enabled = !enabled;
                                        }
                                        data_changed(cx.entity_id(), cx);
                                        cx.notify();
                                    }))
                                    .child(Switch::new().toggle_state(enabled)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_script_into_editor(&sc, cx);
                    }))
                    .child(Label::new(script.name.clone()).size(LabelSize::Small).truncate())
                    .into_any_element()
                })
                .collect()
        };

        let triggers = ["manual", "command", "event", "interval"];
        let mut trigger_tabs = SegmentedTabs::new();
        for trig in triggers {
            let trig_own = trig.to_owned();
            trigger_tabs = trigger_tabs.tab(
                SegmentedTab::new(
                    ElementId::Name(SharedString::from(format!("script-trig-{trig}"))),
                    trig,
                )
                .toggle_state(self.script_trigger_kind == trig)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.script_trigger_kind = trig_own.clone();
                    cx.notify();
                })),
            );
        }

        let perm_choices = [
            "tasks:read",
            "tasks:write",
            "lists:read",
            "lists:write",
            "tags:read",
            "habits:read",
            "habits:write",
            "focus:read",
            "countdowns:read",
            "storage",
            "meta",
            "notifications",
            "clipboard",
        ];

        let perm_chips = h_flex().flex_wrap().gap_1p5().children(perm_choices.iter().map(|perm| {
            let p_str = (*perm).to_owned();
            let active = self.script_permissions.contains(*perm);
            div()
                .id(ElementId::Name(SharedString::from(format!("script-perm-{perm}"))))
                .px_2()
                .h(px(24.))
                .flex()
                .items_center()
                .gap_1()
                .rounded(px(5.))
                .border_1()
                .border_color(if active { colors.accent } else { colors.border })
                .when(active, |el| el.bg(colors.accent.opacity(0.14)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.script_permissions.remove(&p_str) {
                        this.script_permissions.insert(p_str.clone());
                    }
                    cx.notify();
                }))
                .child(Checkbox::new(ElementId::Name(SharedString::from(format!("script-perm-cb-{perm}")))).size(px(12.)).accent().toggle_state(active))
                .child(Label::new(*perm).size(LabelSize::XSmall))
        }));

        let result_panel = self.script_result.as_ref().map(|res| {
            let status_color = if res.ok {
                theme.status().success
            } else {
                theme.status().error
            };
            let ret_str = res
                .return_value
                .as_ref()
                .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()));
            v_flex()
                .w_full()
                .p_3()
                .gap_2()
                .rounded(px(8.))
                .bg(colors.surface_background)
                .border_1()
                .border_color(colors.border_variant)
                .child(
                    h_flex()
                        .justify_between()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Chip::new(
                                    if res.ok { ex.run_ok } else { ex.run_err },
                                    status_color,
                                ))
                                .child(
                                    Label::new(format!("{} ms", res.duration_ms))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        ),
                )
                .when_some(res.error.clone(), |col, err| {
                    col.child(
                        Label::new(err)
                            .size(LabelSize::Small)
                            .color(Color::Error),
                    )
                })
                .when_some(ret_str, |col, val| {
                    col.child(
                        div()
                            .p_2()
                            .rounded(px(6.))
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border)
                            .text_size(LabelSize::XSmall.pixels())
                            .text_color(colors.text)
                            .child(val),
                    )
                })
                .child(render_log_entries_box(
                    &res.logs,
                    ex.no_logs,
                    colors,
                    theme.status(),
                ))
        });

        v_flex()
            .w_full()
            .gap_4()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Label::new(strings.plugins_scripts)
                            .size(LabelSize::Default)
                            .weight(FontWeight::MEDIUM),
                    )
                    .child(
                        Button::new("scripts-new", strings.plugins_script_new)
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reset_new_script_editor(cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .p_1p5()
                    .gap_1()
                    .rounded(px(8.))
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.border_variant)
                    .children(list_items),
            )
            .child(
                v_flex()
                    .w_full()
                    .p_4()
                    .gap_3()
                    .rounded(px(8.))
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.border_variant)
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(110.))
                                    .child(Label::new(strings.plugins_script_name).size(LabelSize::Small)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(30.))
                                    .px_2p5()
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .rounded(px(6.))
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .child(self.script_name_input.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div().w(px(110.)).child(
                                    Label::new(strings.plugins_script_trigger)
                                        .size(LabelSize::Small),
                                ),
                            )
                            .child(trigger_tabs),
                    )
                    .when(self.script_trigger_kind != "manual", |col| {
                        col.child(
                            h_flex()
                                .gap_3()
                                .child(
                                    div()
                                        .w(px(110.))
                                        .child(Label::new(ex.trigger_arg).size(LabelSize::Small)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .h(px(30.))
                                        .px_2p5()
                                        .flex()
                                        .items_center()
                                        .overflow_hidden()
                                        .rounded(px(6.))
                                        .bg(colors.background)
                                        .border_1()
                                        .border_color(colors.border)
                                        .child(self.script_trigger_arg_input.clone()),
                                ),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new(strings.plugins_permissions)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(perm_chips),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new(ex.code_label)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(
                                v_scroll(
                                    "script-code-scroll",
                                    div()
                                        .id("script-code-vp")
                                        .w_full()
                                        .p_2()
                                        .child(self.script_code_input.clone()),
                                )
                                .track_scroll(&self.script_code_scroll)
                                .h(px(180.))
                                .w_full()
                                .rounded(px(6.))
                                .bg(colors.background)
                                .border_1()
                                .border_color(colors.border),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("script-save", strings.plugins_script_save)
                                            .variant(ButtonVariant::Primary)
                                            .size(ButtonSize::Sm)
                                            .icon(IconName::Check)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.save_current_script(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("script-run", strings.plugins_script_run)
                                            .variant(ButtonVariant::Secondary)
                                            .size(ButtonSize::Sm)
                                            .icon(IconName::Play)
                                            .disabled(self.script_running)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.test_run_current_script(cx);
                                            })),
                                    ),
                            )
                            .when(self.selected_script_id.is_some(), |row| {
                                row.child(
                                    Button::new("script-delete", strings.plugins_script_delete)
                                        .variant(ButtonVariant::Destructive)
                                        .size(ButtonSize::Sm)
                                        .icon(IconName::Trash)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.delete_current_script(cx);
                                        })),
                                )
                            }),
                    ),
            )
            .children(result_panel)
            .into_any_element()
    }

    fn render_browse_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let ex = ExtraLabels::for_locale(i18n::locale(cx));
        let theme = cx.theme().clone();
        let colors = theme.colors();

        let installed_versions: HashMap<String, String> = PluginHost::global(cx)
            .map(|h| {
                h.plugins()
                    .iter()
                    .map(|p| (p.manifest.id.clone(), p.manifest.version.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let query = self
            .browse_search_input
            .read(cx)
            .text()
            .trim()
            .to_ascii_lowercase();

        let filtered: Vec<CatalogItem> = self
            .catalog_items
            .iter()
            .filter(|item| {
                if query.is_empty() {
                    return true;
                }
                let e = &item.entry;
                e.name.to_ascii_lowercase().contains(&query)
                    || e.id.to_ascii_lowercase().contains(&query)
                    || e.description.to_ascii_lowercase().contains(&query)
                    || e.author.to_ascii_lowercase().contains(&query)
                    || e.tags.iter().any(|t| t.to_ascii_lowercase().contains(&query))
            })
            .cloned()
            .collect();

        let sources_panel = v_flex()
            .w_full()
            .p_4()
            .gap_2p5()
            .rounded(px(8.))
            .bg(colors.surface_background)
            .border_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        Label::new(strings.plugins_registries)
                            .size(LabelSize::Small)
                            .weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        Button::new("browse-refresh", strings.plugins_registry_refresh)
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .icon(IconName::RefreshCw)
                            .disabled(self.browse_loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_registries(cx);
                            })),
                    ),
            )
            .children(self.registries.iter().enumerate().map(|(idx, src)| {
                let url = src.url.clone();
                let url_del = url.clone();
                let is_default = url == DEFAULT_REGISTRY_URL;
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .id(ElementId::Name(SharedString::from(format!(
                                        "reg-toggle-{idx}"
                                    ))))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(s) = this.registries.get_mut(idx) {
                                            s.enabled = !s.enabled;
                                        }
                                        this.browse_generation =
                                            this.browse_generation.wrapping_add(1);
                                        this.browse_fetch_seq =
                                            this.browse_fetch_seq.wrapping_add(1);
                                        this.browse_loading = false;
                                        this.save_registries();
                                        this.load_cached_registries_async(cx);
                                        cx.notify();
                                    }))
                                    .child(Switch::new().toggle_state(src.enabled)),
                            )
                            .child(
                                Label::new(
                                    src.name
                                        .clone()
                                        .map(|n| format!("{n} ({})", src.url))
                                        .unwrap_or_else(|| src.url.clone()),
                                )
                                .size(LabelSize::XSmall)
                                .color(Color::Muted)
                                .truncate(),
                            ),
                    )
                    .when(!is_default, |row| {
                        row.child(
                            IconButton::new(
                                ElementId::Name(SharedString::from(format!("reg-remove-{idx}"))),
                                IconName::Trash,
                            )
                            .size(px(24.))
                            .icon_size(IconSize::XSmall)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_registry_source(&url_del, cx);
                            })),
                        )
                    })
            }))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(30.))
                            .px_2p5()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .rounded(px(6.))
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border)
                            .child(self.registry_url_input.clone()),
                    )
                    .child(
                        Button::new("reg-add-btn", strings.add)
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.add_registry_url(cx);
                            })),
                    ),
            );

        let search_bar = div()
            .w_full()
            .h(px(32.))
            .px_2p5()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(6.))
            .bg(colors.background)
            .border_1()
            .border_color(colors.border)
            .child(
                Icon::new(IconName::Search)
                    .size(IconSize::Small)
                    .color(Color::IconMuted),
            )
            .child(div().flex_1().min_w_0().child(self.browse_search_input.clone()));

        let catalog_cards: Vec<AnyElement> = if filtered.is_empty() {
            vec![
                div()
                    .w_full()
                    .py_10()
                    .child(EmptyState::new(
                        IconName::Search,
                        if self.browse_loading {
                            ex.loading
                        } else {
                            strings.nothing_found
                        },
                    ))
                    .into_any_element(),
            ]
        } else {
            filtered
                .into_iter()
                .enumerate()
                .map(|(cat_idx, item)| {
                    let e = item.entry;
                    let installed_ver = installed_versions.get(&e.id).cloned();
                    let is_busy = self.installing_plugin_ids.contains(&e.id);
                    let entry_for_install = e.clone();
                    let pid_remove = e.id.clone();

                    let action_btn = match installed_ver.as_deref() {
                        None => Button::new(
                            ElementId::Name(SharedString::from(format!(
                                "cat-inst-{cat_idx}-{}",
                                e.id
                            ))),
                            strings.plugins_registry_install,
                        )
                        .variant(ButtonVariant::Primary)
                        .size(ButtonSize::Sm)
                        .disabled(is_busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.install_from_registry(entry_for_install.clone(), cx);
                        }))
                        .into_any_element(),
                        Some(v) if v != e.version => h_flex()
                            .gap_1p5()
                            .child(
                                Button::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "cat-upd-{cat_idx}-{}",
                                        e.id
                                    ))),
                                    strings.plugins_registry_update,
                                )
                                .variant(ButtonVariant::Primary)
                                .size(ButtonSize::Sm)
                                .disabled(is_busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.install_from_registry(entry_for_install.clone(), cx);
                                })),
                            )
                            .child(
                                Button::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "cat-rem-{cat_idx}-{}",
                                        e.id
                                    ))),
                                    strings.plugins_uninstall,
                                )
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.uninstall_plugin(pid_remove.clone(), cx);
                                })),
                            )
                            .into_any_element(),
                        Some(_) => h_flex()
                            .gap_1p5()
                            .child(Chip::new(
                                strings.plugins_registry_installed,
                                theme.status().success,
                            ))
                            .child(
                                Button::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "cat-rem-{cat_idx}-{}",
                                        e.id
                                    ))),
                                    strings.plugins_uninstall,
                                )
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.uninstall_plugin(pid_remove.clone(), cx);
                                })),
                            )
                            .into_any_element(),
                    };

                    v_flex()
                        .w_full()
                        .p_4()
                        .gap_2p5()
                        .rounded(px(8.))
                        .bg(colors.surface_background)
                        .border_1()
                        .border_color(colors.border_variant)
                        .child(
                            h_flex()
                                .justify_between()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_0p5()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .flex_wrap()
                                                .child(
                                                    Label::new(e.name.clone())
                                                        .weight(FontWeight::SEMIBOLD),
                                                )
                                                .child(
                                                    Label::new(format!("v{}", e.version))
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Muted),
                                                )
                                                .when(!e.author.is_empty(), |row| {
                                                    row.child(
                                                        Label::new(e.author.clone())
                                                            .size(LabelSize::XSmall)
                                                            .color(Color::Muted),
                                                    )
                                                }),
                                        )
                                        .when(!e.description.is_empty(), |col| {
                                            col.child(
                                                Label::new(e.description.clone())
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted),
                                            )
                                        }),
                                )
                                .child(action_btn),
                        )
                        .when(!e.tags.is_empty(), |col| {
                            col.child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .children(e.tags.iter().map(|t| Chip::new(t.clone(), colors.accent))),
                            )
                        })
                        .when(!e.permissions.is_empty(), |col| {
                            col.child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .child(
                                        Label::new(format!("{}:", strings.plugins_permissions))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .children(
                                        e.permissions
                                            .iter()
                                            .map(|p| Chip::new(p.summary(), colors.accent)),
                                    ),
                            )
                        })
                        .child(
                            Label::new(item.source_url)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted)
                                .truncate(),
                        )
                        .into_any_element()
                })
                .collect()
        };

        v_flex()
            .w_full()
            .gap_4()
            .child(sources_panel)
            .when(!self.registry_errors.is_empty(), |col| {
                col.children(self.registry_errors.iter().map(|err| {
                    Label::new(err.clone())
                        .size(LabelSize::XSmall)
                        .color(Color::Error)
                }))
            })
            .child(search_bar)
            .children(catalog_cards)
            .into_any_element()
    }

    fn render_permission_modal(&self, pending: &PermissionPromptState, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();

        let perms_list = if pending.added_permissions.is_empty() {
            v_flex()
                .child(
                    Label::new(strings.plugins_permissions_changed)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
        } else {
            v_flex()
                .w_full()
                .gap_1p5()
                .children(pending.added_permissions.iter().map(|perm| {
                    h_flex()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Check)
                                .size(IconSize::XSmall)
                                .color(Color::Accent),
                        )
                        .child(Label::new(perm.summary()).size(LabelSize::Small))
                }))
        };

        Modal::new("plugin-permissions-modal")
            .width(px(440.))
            .on_dismiss(cx.listener(|this, _, _, cx| {
                this.pending_permission_modal = None;
                cx.notify();
            }))
            .child(
                Label::new(strings.plugins_permissions_approve)
                    .size(LabelSize::Default)
                    .weight(FontWeight::SEMIBOLD),
            )
            .child(
                Label::new(format!(
                    "{} ({} v{})",
                    pending.plugin_name, pending.plugin_id, pending.version
                ))
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .when(!pending.is_first_install, |m| {
                m.child(
                    Label::new(strings.plugins_permissions_changed)
                        .size(LabelSize::Small)
                        .color(Color::Warning),
                )
            })
            .child(
                div()
                    .w_full()
                    .p_3()
                    .rounded(px(6.))
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.border_variant)
                    .child(perms_list),
            )
            .child(
                h_flex()
                    .w_full()
                    .pt_1()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("perm-modal-cancel", strings.cancel)
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pending_permission_modal = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("perm-modal-approve", strings.plugins_permissions_approve)
                            .variant(ButtonVariant::Primary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.approve_pending_permissions(cx);
                            })),
                    ),
            )
            .into_any_element()
    }
    fn render_active_plugin_page(
        &self,
        page: &ActivePluginPage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        let has_settings = PluginHost::global(cx)
            .and_then(|h| h.plugin(&page.plugin_id))
            .is_some_and(|p| !p.manifest.settings.is_empty());
        let pid_settings = page.plugin_id.clone();
        let pid_logs = page.plugin_id.clone();
        let pid_reload = page.plugin_id.clone();

        v_flex()
            .size_full()
            .min_h_0()
            .when(cfg!(test), |el| {
                el.debug_selector(|| "plugin-settings-active-page".to_owned())
            })
            .child(
                h_flex()
                    .w_full()
                    .px_6()
                    .pt_1()
                    .pb_2p5()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                Button::new("plugin-page-back", strings.plugins_back)
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::Sm)
                                    .icon(IconName::ChevronLeft)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_plugin_page(cx);
                                    })),
                            )
                            .child(
                                Label::new(page.plugin_name.clone())
                                    .size(LabelSize::Default)
                                    .weight(FontWeight::SEMIBOLD)
                                    .truncate(),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .when(has_settings, |row| {
                                row.child(
                                    Button::new("plugin-page-settings", strings.plugins_settings)
                                        .variant(ButtonVariant::Ghost)
                                        .size(ButtonSize::Sm)
                                        .icon(IconName::Settings2)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.close_plugin_page(cx);
                                            this.tab = PluginSettingsTab::Installed;
                                            this.expanded_settings = Some(pid_settings.clone());
                                            this.expanded_logs = None;
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                Button::new("plugin-page-logs", strings.plugins_logs)
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::Sm)
                                    .icon(IconName::FileText)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_logs_for(&pid_logs, cx);
                                    })),
                            )
                            .child(
                                IconButton::new("plugin-page-reload", IconName::RefreshCw)
                                    .size(px(28.))
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::IconMuted)
                                    .hover_icon_color(Color::Icon)
                                    .tooltip(Tooltip::text(strings.plugins_reload))
                                    .on_click(move |_, _, cx| {
                                        PluginHost::reload_plugin_async(pid_reload.clone(), cx);
                                    }),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().w_full().child(page.view.clone()))
            .into_any_element()
    }
}

impl Render for PluginSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);

        let permission_modal = self
            .pending_permission_modal
            .as_ref()
            .map(|pending| self.render_permission_modal(pending, cx));

        if let Some(page) = &self.active_plugin_page {
            let page_el = self.render_active_plugin_page(page, cx);
            return div()
                .relative()
                .size_full()
                .track_focus(&self.focus_handle)
                .on_action(cx.listener(|this, _: &DismissSettings, _, cx| {
                    if !this.handle_escape(cx) {
                        cx.propagate();
                    }
                }))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape"
                        && !event.keystroke.modifiers.modified()
                        && this.handle_escape(cx)
                    {
                        cx.stop_propagation();
                    }
                }))
                .when(cfg!(test), |el| {
                    el.debug_selector(|| "plugin-settings-view".to_owned())
                })
                .child(page_el)
                .children(permission_modal);
        }

        let tabs = SegmentedTabs::new()
            .tab(
                SegmentedTab::new("plugins-tab-installed", strings.plugins_installed)
                    .toggle_state(self.tab == PluginSettingsTab::Installed)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_tab(PluginSettingsTab::Installed, cx);
                    })),
            )
            .tab(
                SegmentedTab::new("plugins-tab-scripts", strings.plugins_scripts)
                    .toggle_state(self.tab == PluginSettingsTab::Scripts)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_tab(PluginSettingsTab::Scripts, cx);
                    })),
            )
            .tab(
                SegmentedTab::new("plugins-tab-browse", strings.plugins_browse)
                    .toggle_state(self.tab == PluginSettingsTab::Browse)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_tab(PluginSettingsTab::Browse, cx);
                    })),
            );

        let body = match self.tab {
            PluginSettingsTab::Installed => self.render_installed_tab(cx),
            PluginSettingsTab::Scripts => self.render_scripts_tab(cx),
            PluginSettingsTab::Browse => self.render_browse_tab(cx),
        };

        div()
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &DismissSettings, _, cx| {
                if !this.handle_escape(cx) {
                    cx.propagate();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape"
                    && !event.keystroke.modifiers.modified()
                    && this.handle_escape(cx)
                {
                    cx.stop_propagation();
                }
            }))
            .when(cfg!(test), |el| {
                el.debug_selector(|| "plugin-settings-view".to_owned())
            })
            .child(
                v_scroll(
                    "plugin-settings-scroll",
                    v_flex()
                        .id("plugin-settings-vp")
                        .w_full()
                        .pt_1()
                        .px_6()
                        .pb_6()
                        .gap_4()
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    Label::new(strings.plugins)
                                        .size(LabelSize::Default)
                                        .weight(FontWeight::MEDIUM),
                                )
                                .child(tabs),
                        )
                        .when_some(self.status_banner.as_ref(), |col, (msg, is_err)| {
                            col.child(
                                Label::new(msg.clone())
                                    .size(LabelSize::Small)
                                    .color(if *is_err {
                                        Color::Error
                                    } else {
                                        Color::Success
                                    }),
                            )
                        })
                        .child(body),
                )
                .track_scroll(&self.scroll_handle)
                .size_full(),
            )
            .children(permission_modal)
    }
}

fn setting_row_shell(spec: &SettingSpec, control: AnyElement) -> AnyElement {
    h_flex()
        .w_full()
        .gap_3()
        .items_center()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(Label::new(spec.label.clone()).size(LabelSize::Small))
                .when_some(spec.description.clone(), |col, desc| {
                    col.child(
                        Label::new(desc)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                }),
        )
        .child(div().w(px(240.)).flex_none().child(control))
        .into_any_element()
}

fn render_log_entries_box(
    logs: &[LogEntry],
    empty_label: &'static str,
    colors: &theme::ThemeColors,
    status: &theme::StatusColors,
) -> AnyElement {
    div()
        .w_full()
        .p_2p5()
        .rounded(px(6.))
        .bg(colors.background)
        .border_1()
        .border_color(colors.border)
        .child(if logs.is_empty() {
            Label::new(empty_label)
                .size(LabelSize::XSmall)
                .color(Color::Muted)
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .gap_1()
                .children(logs.iter().rev().take(60).rev().map(|entry| {
                    let (lvl_text, lvl_color) = match entry.level {
                        LogLevel::Debug => ("DBG", colors.text_muted),
                        LogLevel::Info => ("INF", status.info),
                        LogLevel::Warn => ("WRN", status.warning),
                        LogLevel::Error => ("ERR", status.error),
                    };
                    let ts = format_timestamp(entry.timestamp_ms);
                    h_flex()
                        .w_full()
                        .items_start()
                        .gap_2()
                        .child(
                            Label::new(ts)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new(lvl_text)
                                .size(LabelSize::XSmall)
                                .color(Color::Custom(lvl_color))
                                .weight(FontWeight::SEMIBOLD),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(LabelSize::XSmall.pixels())
                                .text_color(colors.text)
                                .child(entry.message.clone()),
                        )
                }))
                .into_any_element()
        })
        .into_any_element()
}

fn load_registries_from_store(store: &Store) -> Vec<RegistrySource> {
    let Ok(Some(raw)) = store.setting(REGISTRIES_SETTING_KEY) else {
        return vec![RegistrySource::default()];
    };
    let parsed: Vec<RegistrySource> = serde_json::from_str(&raw).unwrap_or_default();
    if parsed.is_empty() {
        vec![RegistrySource::default()]
    } else {
        parsed
    }
}

fn parse_setting_input_value(
    kind: SettingKind,
    raw: &str,
    min: Option<f64>,
    max: Option<f64>,
) -> Option<JsonValue> {
    match kind {
        SettingKind::Text => Some(JsonValue::String(raw.to_owned())),
        SettingKind::Number => {
            let parsed = raw.trim().parse::<f64>().ok()?;
            if !parsed.is_finite() {
                return None;
            }
            let clamped = clamp_number(parsed, min, max);
            serde_json::Number::from_f64(clamped).map(JsonValue::Number)
        }
        SettingKind::Boolean | SettingKind::Select | SettingKind::Secret => None,
    }
}

fn clamp_number(val: f64, min: Option<f64>, max: Option<f64>) -> f64 {
    let mut out = val;
    if let Some(lo) = min {
        out = out.max(lo);
    }
    if let Some(hi) = max {
        out = out.min(hi);
    }
    out
}

fn format_number_clean(val: f64) -> String {
    if val.fract().abs() < 1e-9 {
        format!("{}", val.round() as i64)
    } else {
        format!("{val}")
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

fn format_timestamp(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%H:%M:%S").to_string(),
        None => {
            let dt: DateTime<Local> = Local::now();
            dt.format("%H:%M:%S").to_string()
        }
    }
}

struct ExtraLabels {
    status_active: &'static str,
    status_idle: &'static str,
    status_broken: &'static str,
    status_error: &'static str,
    clear_secret: &'static str,
    no_logs: &'static str,
    trigger_arg: &'static str,
    code_label: &'static str,
    run_ok: &'static str,
    run_err: &'static str,
    loading: &'static str,
}

impl ExtraLabels {
    fn for_locale(locale: Locale) -> Self {
        match locale {
            Locale::En => Self {
                status_active: "Active",
                status_idle: "Idle",
                status_broken: "Circuit Broken",
                status_error: "Error",
                clear_secret: "Clear secret",
                no_logs: "No log entries yet.",
                trigger_arg: "Trigger Arg",
                code_label: "JavaScript Code",
                run_ok: "Success",
                run_err: "Failed",
                loading: "Loading…",
            },
            Locale::Ru => Self {
                status_active: "Активен",
                status_idle: "Ожидание",
                status_broken: "Остановлен",
                status_error: "Ошибка",
                clear_secret: "Удалить секрет",
                no_logs: "Логов пока нет.",
                trigger_arg: "Аргумент триггера",
                code_label: "Код JavaScript",
                run_ok: "Успех",
                run_err: "Ошибка",
                loading: "Загрузка…",
            },
            Locale::ZhCn => Self {
                status_active: "运行中",
                status_idle: "空闲",
                status_broken: "已熔断",
                status_error: "错误",
                clear_secret: "清除密钥",
                no_logs: "暂无日志。",
                trigger_arg: "触发参数",
                code_label: "JavaScript 代码",
                run_ok: "成功",
                run_err: "失败",
                loading: "加载中…",
            },
        }
    }
}
