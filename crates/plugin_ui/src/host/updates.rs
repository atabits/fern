use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
#[cfg(not(test))]
use std::sync::mpsc;
use std::time::Duration;

use desktop_common::data_version::data_changed;
use desktop_common::plugin_bus::{
    PluginBus, PluginCommandSpec, PluginModalKind, PluginModalSpec, PluginNavAction,
    PluginSidebarViewSpec, ToastLevel as BusToastLevel,
};
use gpui::{App, BorrowAppContext as _, ClipboardItem, Global};
use plugins::{
    HostUpdate, PluginInfo, PluginStatus, PluginUiEffect, PromptReplyValue,
    ToastLevel as PluginToastLevel,
};

use super::themes::{sync_plugin_themes_async, theme_contribution_signature};
#[cfg(not(test))]
use super::{WaitForWake, WakeSignal};
use super::{PluginHost, has_active_hook_plugins};

const TOAST_AUTO_DISMISS_DELAY: Duration = Duration::from_millis(4500);
const MAX_PENDING_TOASTS: usize = 6;

#[derive(Default)]
struct ScheduledToastTimers(HashSet<u64>);

impl Global for ScheduledToastTimers {}

#[cfg(not(test))]
pub(super) fn spawn_update_consumer(
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

pub(super) fn apply_host_update(update: HostUpdate, cx: &mut App) {
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
                        host.views.remove(&plugin_id);
                        host.view_revisions.remove(&plugin_id);
                        host.op_state.borrow_mut().invalidate_plugin_views(&plugin_id);
                    }
                    if host.pending_hot_reload.remove(&plugin_id) {
                        host.views.remove(&plugin_id);
                        host.view_revisions.remove(&plugin_id);
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

pub(super) fn apply_registry_changed(infos: Vec<PluginInfo>, cx: &mut App) {
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
                    host.views.remove(&info.manifest.id);
                    host.view_revisions.remove(&info.manifest.id);
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
                    host.views.remove(&info.manifest.id);
                    host.view_errors.remove(&info.manifest.id);
                    host.view_revisions.remove(&info.manifest.id);
                    host.op_state
                        .borrow_mut()
                        .invalidate_plugin_views(&info.manifest.id);
                }
            }
            let alive: HashSet<&str> = infos.iter().map(|p| p.manifest.id.as_str()).collect();
            host.views.retain(|pid, _| alive.contains(pid.as_str()));
            host.view_errors.retain(|pid, _| alive.contains(pid.as_str()));
            host.view_revisions
                .retain(|pid, _| alive.contains(pid.as_str()));
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

pub(super) fn sync_bus_contributions(plugins: &[PluginInfo], cx: &mut App) {
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

pub(super) fn dismiss_active_modal_on_shutdown(cx: &mut App) {
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

pub(super) fn push_toast_capped(
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
