//! Native GPUI plugin settings panel (`Installed`, `Scripts`, `Browse`).

mod browse_tab;
mod helpers;
mod installed_tab;
mod modals;
mod scripts_tab;
mod settings_form;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use desktop_common::actions::{DismissSettings, DismissTransient};
use desktop_common::text_input::{Changed, Mode, Submitted, TextInput};
use fern_core::Store;
use fern_core::plugin_store::UserScript;
use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, ScrollHandle, Styled, Subscription, Window, div,
};
use plugins::{LogEntry, PluginStatus, RegistrySource, ScriptRunResult, generate_sample_plugin};
use serde_json::{Map as JsonMap, Value as JsonValue};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{Chip, Color, Label, LabelSize, SegmentedTab, SegmentedTabs, v_scroll};

use self::browse_tab::CatalogItem;
use self::helpers::{SAFE_MODE_SETTING_KEY, load_registries_from_store};
use self::modals::PermissionPromptState;
use crate::host::PluginHost;
use crate::view::{PluginView, PluginViewEvent};

/// Top-level tabs of [`PluginSettingsView`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PluginSettingsTab {
    #[default]
    Installed,
    Scripts,
    Browse,
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
    _sub: Subscription,
}

struct ActivePluginPage {
    plugin_id: String,
    plugin_name: String,
    view_id: String,
    view: Entity<PluginView>,
    _view_sub: Subscription,
}

enum TogglePluginAction {
    NeedsApproval(Box<PermissionPromptState>),
    ResetCircuitBreaker,
    SetEnabled(bool),
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
    confirm_delete_script: Option<(String, String)>,

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

        let script_name_input = cx.new(|cx| {
            TextInput::new(Mode::SingleLine, i18n::t(cx).plugins_script_name, cx).without_cancel()
        });
        let script_trigger_arg_input =
            cx.new(|cx| TextInput::new(Mode::SingleLine, "task.created", cx).without_cancel());
        let script_code_input = cx.new(|cx| {
            TextInput::new(Mode::MultiLine, "// fern.tasks.list()\n", cx).without_cancel()
        });
        let browse_search_input = cx.new(|cx| {
            TextInput::new(Mode::SingleLine, i18n::t(cx).focus_picker_search, cx).without_cancel()
        });
        let registry_url_input = cx.new(|cx| {
            TextInput::new(Mode::SingleLine, i18n::t(cx).plugins_registry_add_url, cx)
                .without_cancel()
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
            confirm_delete_script: None,
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

        if let Some(first) = scripts.first() {
            view.load_script_into_editor(first, cx);
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
    pub(crate) fn confirm_delete_script_for_test(&self) -> Option<(&str, &str)> {
        self.confirm_delete_script
            .as_ref()
            .map(|(id, name)| (id.as_str(), name.as_str()))
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

    #[cfg(test)]
    pub(crate) fn setting_input_for_test(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Option<Entity<TextInput>> {
        self.setting_inputs
            .get(&(plugin_id.to_owned(), key.to_owned()))
            .map(|e| e.input.clone())
    }

    #[cfg(test)]
    pub(crate) fn secret_input_for_test(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Option<Entity<TextInput>> {
        self.secret_inputs
            .get(&(plugin_id.to_owned(), key.to_owned()))
            .map(|e| e.input.clone())
    }

    #[cfg(test)]
    pub(crate) fn script_name_input_for_test(&self) -> Entity<TextInput> {
        self.script_name_input.clone()
    }

    #[cfg(test)]
    pub(crate) fn browse_inputs_for_test(&self) -> (Entity<TextInput>, Entity<TextInput>) {
        (
            self.browse_search_input.clone(),
            self.registry_url_input.clone(),
        )
    }

    /// Handles an Escape press inside [`PluginSettingsView`].
    ///
    /// Dismisses transient UI in priority order (`pending_permission_modal` ->
    /// `confirm_delete_script` -> open `Select` popover -> `active_plugin_page`) and returns
    /// `true` if handled so the parent Settings window stays open.
    pub fn handle_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.pending_permission_modal.take().is_some() {
            cx.notify();
            return true;
        }
        if self.confirm_delete_script.take().is_some() {
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
        self.confirm_delete_script = None;
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
        let mut changed = self.confirm_delete_script.take().is_some();
        if let Some(prev) = self.active_plugin_page.take() {
            prev.view.update(cx, |v, cx| v.unmount(cx));
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    pub fn set_tab(&mut self, tab: PluginSettingsTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.close_plugin_page(cx);
        self.tab = tab;
        self.confirm_delete_script = None;
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

    fn toggle_plugin_enabled_by_id(&mut self, plugin_id: &str, cx: &mut Context<Self>) {
        let Some(action) = PluginHost::global(cx)
            .and_then(|h| h.plugin(plugin_id))
            .map(|info| match &info.status {
                PluginStatus::NeedsApproval {
                    added_permissions,
                    is_first_install,
                } => TogglePluginAction::NeedsApproval(Box::new(PermissionPromptState {
                    plugin_id: info.manifest.id.clone(),
                    plugin_name: info.manifest.name.clone(),
                    version: info.manifest.version.clone(),
                    added_permissions: added_permissions.clone(),
                    is_first_install: *is_first_install,
                    pending_zip_overwrite: None,
                })),
                PluginStatus::CircuitBroken { .. } => TogglePluginAction::ResetCircuitBreaker,
                _ => TogglePluginAction::SetEnabled(!info.enabled),
            })
        else {
            return;
        };

        let next_enabled = match action {
            TogglePluginAction::NeedsApproval(prompt) => {
                self.pending_permission_modal = Some(*prompt);
                cx.notify();
                return;
            }
            TogglePluginAction::ResetCircuitBreaker => {
                self.reset_and_enable_plugin(plugin_id.to_owned(), cx);
                return;
            }
            TogglePluginAction::SetEnabled(next) => next,
        };

        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        let pid = plugin_id.to_owned();
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
                .on_action(cx.listener(|this, _: &DismissTransient, _, cx| {
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

        let delete_script_modal = self
            .confirm_delete_script
            .as_ref()
            .map(|(_, name)| self.render_delete_script_modal(name, cx));

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
            .on_action(cx.listener(|this, _: &DismissTransient, _, cx| {
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
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Label::new(strings.plugins)
                                                .size(LabelSize::Default)
                                                .weight(FontWeight::MEDIUM),
                                        )
                                        .child(Chip::new(
                                            strings.beta_badge,
                                            cx.theme().status().warning,
                                        )),
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
            .children(delete_script_modal)
            .children(permission_modal)
    }
}
