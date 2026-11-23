use desktop_common::plugin_bus::{PluginBus, PluginNavAction, PluginSidebarViewSpec};
use plugin_ui::{PluginHost, PluginView, PluginViewEvent};
use ui::prelude::*;

use crate::settings_window;
use crate::sidebar::Section;

use super::{AppView, first_enabled_task_section};

fn is_plugin_view_reachable(
    plugin_id: &str,
    view_id: &str,
    active_views: &[PluginSidebarViewSpec],
    cx: &App,
) -> bool {
    if active_views
        .iter()
        .any(|spec| spec.plugin_id == plugin_id && spec.view_id == view_id)
    {
        return true;
    }
    PluginHost::global(cx)
        .and_then(|host| host.plugin(plugin_id))
        .is_some_and(|info| {
            matches!(info.status, plugin_ui::PluginStatus::CircuitBroken { .. })
                && info
                    .manifest
                    .contributions
                    .sidebar
                    .iter()
                    .any(|v| v.id == view_id)
        })
}

impl AppView {
    pub(super) fn unmount_all_plugin_views(&mut self, cx: &mut Context<Self>) {
        let views: Vec<_> = self.plugin_views.drain().map(|(_, view)| view).collect();
        for view in views {
            view.update(cx, |view, cx| view.unmount(cx));
        }
    }

    pub(super) fn ensure_plugin_view(
        &mut self,
        plugin_id: &str,
        view_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<PluginView> {
        let key = (plugin_id.to_owned(), view_id.to_owned());
        if let Some(existing) = self.plugin_views.get(&key) {
            return existing.clone();
        }
        let pid = plugin_id.to_owned();
        let vid = view_id.to_owned();
        let entity = cx.new(|cx| PluginView::new(pid, vid, cx));
        cx.subscribe_in(&entity, window, |this, _, event: &PluginViewEvent, window, cx| {
            match event {
                PluginViewEvent::OpenLogs { plugin_id } => {
                    // `PluginView` may also have queued `OpenSettingsPlugins` + `requested_settings_plugin`
                    // via `PluginHost::request_open_logs_for`; consume both so Settings opens and
                    // fetches logs exactly once instead of double-dispatching on the next bus tick.
                    let _ = PluginHost::take_requested_settings_plugin(cx);
                    if cx
                        .try_global::<PluginBus>()
                        .is_some_and(|bus| {
                            bus.pending_nav
                                .iter()
                                .any(|a| matches!(a, PluginNavAction::OpenSettingsPlugins))
                        })
                    {
                        PluginBus::update(cx, |bus| {
                            bus.pending_nav
                                .retain(|a| !matches!(a, PluginNavAction::OpenSettingsPlugins));
                        });
                    }
                    this.open_plugin_logs(plugin_id.clone(), window, cx);
                }
            }
        })
        .detach();
        self.plugin_views.insert(key, entity.clone());
        entity
    }

    fn open_plugin_logs(&mut self, plugin_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_at(None, Some(settings_window::Page::Plugins), window, cx);
        if let Some(settings) = self.settings {
            let _ = settings.update(cx, |settings, _, cx| {
                settings.open_plugin_logs(&plugin_id, cx);
            });
        }
    }

    pub(super) fn on_plugin_bus_changed(&mut self, cx: &mut Context<Self>) {
        let mut actions = Vec::new();
        if cx
            .try_global::<PluginBus>()
            .is_some_and(|bus| !bus.pending_nav.is_empty())
        {
            actions = PluginBus::update(cx, |bus| bus.drain_nav());
        }

        if let Some(active_views) = cx
            .try_global::<PluginBus>()
            .map(|bus| bus.sidebar_views.clone())
        {
            let evicted_keys: Vec<(String, String)> = self
                .plugin_views
                .keys()
                .filter(|(pid, vid)| !is_plugin_view_reachable(pid, vid, &active_views, cx))
                .cloned()
                .collect();
            for key in evicted_keys {
                if let Some(view) = self.plugin_views.remove(&key) {
                    view.update(cx, |view, cx| view.unmount(cx));
                }
            }
            if PluginHost::global(cx).is_some()
                && let Section::Plugin {
                    ref plugin_id,
                    ref view_id,
                } = self.section
                && !is_plugin_view_reachable(plugin_id.as_ref(), view_id.as_ref(), &active_views, cx)
            {
                self.section = first_enabled_task_section(self.features);
                self.sync_active_section(cx);
            }
        }

        let mut deferred = Vec::new();
        for action in actions {
            match action {
                PluginNavAction::RunCommand {
                    plugin_id,
                    command_id,
                    arg,
                } => {
                    PluginHost::run_command_async(plugin_id, command_id, arg, cx);
                }
                other => deferred.push(other),
            }
        }
        if !deferred.is_empty() && let Some(win) = self.window_handle {
            cx.spawn(async move |_, cx| {
                let _ = win.update(cx, |view, window, cx| {
                    for action in deferred {
                        view.handle_plugin_nav(action, window, cx);
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn handle_plugin_nav(
        &mut self,
        action: PluginNavAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            PluginNavAction::OpenTask(id) => self.open_task(id, window, cx),
            PluginNavAction::OpenPluginView { plugin_id, view_id } => {
                let active_views = cx
                    .try_global::<PluginBus>()
                    .map(|bus| bus.sidebar_views.clone())
                    .unwrap_or_default();
                if is_plugin_view_reachable(&plugin_id, &view_id, &active_views, cx) {
                    self.select(
                        Section::Plugin {
                            plugin_id: plugin_id.into(),
                            view_id: view_id.into(),
                        },
                        window,
                        cx,
                    );
                }
            }
            PluginNavAction::OpenSettingsPlugins => {
                if let Some(plugin_id) = PluginHost::take_requested_settings_plugin(cx) {
                    self.open_plugin_logs(plugin_id, window, cx);
                } else {
                    self.open_settings_page(settings_window::Page::Plugins, window, cx);
                }
            }
            PluginNavAction::RunCommand {
                plugin_id,
                command_id,
                arg,
            } => {
                PluginHost::run_command_async(plugin_id, command_id, arg, cx);
            }
        }
    }
}
