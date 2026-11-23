//! Native GPUI plugin settings panel (`Installed`, `Scripts`, `Browse`).

mod browse_tab;
mod helpers;
mod modals;
mod scripts_tab;
mod settings_form;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use desktop_common::actions::DismissSettings;
use desktop_common::text_input::{Changed, Mode, Submitted, TextInput};
use fern_core::Store;
use fern_core::plugin_store::UserScript;
use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, ElementId, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use icons::IconName;
use plugins::{
    LogEntry, PluginInfo, PluginStatus, RegistrySource, ScriptRunResult, generate_sample_plugin,
};
use serde_json::{Map as JsonMap, Value as JsonValue};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Chip, Color, Divider, EmptyState, Icon, IconButton,
    IconSize, Label, LabelSize, SegmentedTab, SegmentedTabs, Switch, Tooltip, v_scroll,
};

use self::browse_tab::CatalogItem;
use self::helpers::{
    SAFE_MODE_SETTING_KEY, format_bytes, load_registries_from_store, render_log_entries_box,
};
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

    // -------------------------------------------------------------------------
    // Rendering
    // -------------------------------------------------------------------------

    fn render_installed_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();

        let safe_mode_active = PluginHost::global(cx).is_some_and(|h| h.is_safe_mode());
        let plugins: Arc<Vec<PluginInfo>> = PluginHost::global(cx)
            .map(PluginHost::plugins_arc)
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
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.install_from_zip_dialog(cx);
                            })),
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
                .map(|info| self.render_plugin_card(info, &theme, cx))
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
        theme: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = theme.colors();
        let status_colors = theme.status();
        let pid: SharedString = info.manifest.id.clone().into();

        let (status_label, status_color) = match &info.status {
            PluginStatus::Active => (strings.plugin_status_active, status_colors.success),
            PluginStatus::Idle => (strings.plugin_status_idle, status_colors.info),
            PluginStatus::Disabled => (strings.plugins_disabled, colors.text_muted),
            PluginStatus::NeedsApproval { .. } => {
                (strings.plugins_permissions_approve, status_colors.warning)
            }
            PluginStatus::CircuitBroken { .. } => {
                (strings.plugin_status_broken, status_colors.error)
            }
            PluginStatus::Error(_) => (strings.plugin_status_error, status_colors.error),
        };

        let metrics_text = strings
            .plugins_metrics
            .replace("{mem}", &format_bytes(info.metrics.memory_bytes))
            .replace("{ms}", &info.metrics.last_exec_ms.to_string());

        let primary_view = info
            .manifest
            .contributions
            .primary_view()
            .map(|(vid, _)| vid);
        let can_open_page = primary_view.is_some()
            && info.enabled
            && !matches!(
                info.status,
                PluginStatus::NeedsApproval { .. }
                    | PluginStatus::CircuitBroken { .. }
                    | PluginStatus::Disabled
            );
        let has_settings = !info.manifest.settings.is_empty();
        let settings_open = self.expanded_settings.as_deref() == Some(pid.as_ref());
        let logs_open = self.expanded_logs.as_deref() == Some(pid.as_ref());

        let pid_toggle = pid.clone();
        let pid_reload = pid.clone();
        let pid_settings = pid.clone();
        let pid_logs = pid.clone();
        let pid_uninstall = pid.clone();

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
                                    .when(
                                        info.manifest.version.to_ascii_lowercase().contains("beta"),
                                        |row| {
                                            row.child(Chip::new(
                                                strings.beta_badge,
                                                theme.status().warning,
                                            ))
                                        },
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
                        h_flex().gap_2().child(
                            div()
                                .id(ElementId::Name(SharedString::from(format!(
                                    "plugin-enable-{pid_toggle}"
                                ))))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_plugin_enabled_by_id(pid_toggle.as_ref(), cx);
                                }))
                                .child(Switch::new().toggle_state(info.enabled)),
                        ),
                    ),
            );

        if matches!(info.status, PluginStatus::NeedsApproval { .. }) {
            let pid_approve = pid.clone();
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
                            if let Some(prompt_state) = PluginHost::global(cx)
                                .and_then(|h| h.plugin(pid_approve.as_ref()))
                                .and_then(|info| {
                                    if let PluginStatus::NeedsApproval {
                                        added_permissions,
                                        is_first_install,
                                    } = &info.status
                                    {
                                        Some(PermissionPromptState {
                                            plugin_id: info.manifest.id.clone(),
                                            plugin_name: info.manifest.name.clone(),
                                            version: info.manifest.version.clone(),
                                            added_permissions: added_permissions.clone(),
                                            is_first_install: *is_first_install,
                                            pending_zip_overwrite: None,
                                        })
                                    } else {
                                        None
                                    }
                                })
                            {
                                this.pending_permission_modal = Some(prompt_state);
                                cx.notify();
                            }
                        })),
                    ),
            );
        }

        if let PluginStatus::CircuitBroken { last_error } = &info.status {
            let pid_reset = pid.clone();
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
                            this.reset_and_enable_plugin(pid_reset.as_ref().to_owned(), cx);
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
                    let pid_open = pid.clone();
                    let name_open: SharedString = info.manifest.name.clone().into();
                    let view_id_open: SharedString = view_id.to_owned().into();
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
                                pid_open.as_ref().to_owned(),
                                name_open.as_ref().to_owned(),
                                view_id_open.as_ref().to_owned(),
                                cx,
                            );
                        })),
                    )
                })
                .child(
                    Button::new(
                        ElementId::Name(SharedString::from(format!(
                            "plugin-reload-{pid_reload}"
                        ))),
                        strings.plugins_reload,
                    )
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(IconName::RefreshCw)
                    .on_click(move |_, _, cx| {
                        PluginHost::reload_plugin_async(pid_reload.as_ref().to_owned(), cx);
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
                            if this.expanded_settings.as_deref() == Some(pid_settings.as_ref()) {
                                this.expanded_settings = None;
                            } else {
                                this.expanded_settings = Some(pid_settings.as_ref().to_owned());
                            }
                            cx.notify();
                        })),
                    )
                })
                .child(
                    Button::new(
                        ElementId::Name(SharedString::from(format!(
                            "plugin-logs-btn-{pid_logs}"
                        ))),
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
                        if this.expanded_logs.as_deref() == Some(pid_logs.as_ref()) {
                            this.expanded_logs = None;
                        } else {
                            let pid_owned = pid_logs.as_ref().to_owned();
                            this.expanded_logs = Some(pid_owned.clone());
                            this.fetch_plugin_logs_async(pid_owned, cx);
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
                        this.uninstall_plugin(pid_uninstall.as_ref().to_owned(), cx);
                    })),
                ),
        );

        if settings_open && has_settings {
            let form = self.render_plugin_settings_form(info, cx);
            card = card.child(Divider::horizontal()).child(form);
        }

        if logs_open {
            let logs_panel = self.render_plugin_logs_panel(pid.as_ref(), theme, cx);
            card = card.child(Divider::horizontal()).child(logs_panel);
        }

        card.into_any_element()
    }

    fn render_plugin_logs_panel(
        &self,
        plugin_id: &str,
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
        let pid_refresh: SharedString = plugin_id.to_owned().into();
        let pid_clear = pid_refresh.clone();

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
                                    this.fetch_plugin_logs_async(
                                        pid_refresh.as_ref().to_owned(),
                                        cx,
                                    );
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
                                    this.clear_plugin_logs_async(
                                        pid_clear.as_ref().to_owned(),
                                        cx,
                                    );
                                })),
                            ),
                    ),
            )
            .child(render_log_entries_box(
                logs,
                strings.plugin_no_logs,
                colors,
                theme.status(),
            ))
            .into_any_element()
    }

    fn render_active_plugin_page(
        &self,
        page: &ActivePluginPage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        let plugin_info = PluginHost::global(cx).and_then(|h| h.plugin(&page.plugin_id));
        let has_settings = plugin_info
            .as_ref()
            .is_some_and(|p| !p.manifest.settings.is_empty());
        let is_beta = plugin_info
            .as_ref()
            .is_some_and(|p| p.manifest.version.to_ascii_lowercase().contains("beta"));
        let warning_color = cx.theme().status().warning;
        let pid: SharedString = page.plugin_id.clone().into();
        let pid_settings = pid.clone();
        let pid_logs = pid.clone();
        let pid_reload = pid;

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
                            )
                            .when(is_beta, |row| {
                                row.child(Chip::new(strings.beta_badge, warning_color))
                            }),
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
                                            this.expanded_settings =
                                                Some(pid_settings.as_ref().to_owned());
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
                                        this.open_logs_for(pid_logs.as_ref(), cx);
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
                                        PluginHost::reload_plugin_async(
                                            pid_reload.as_ref().to_owned(),
                                            cx,
                                        );
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
