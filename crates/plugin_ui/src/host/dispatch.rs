use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Result;
use desktop_common::plugin_bus::{PluginBus, PluginSearchHit, ToastLevel as BusToastLevel};
use gpui::{App, AppContext as _, BorrowAppContext as _};
use plugins::PromptReplyValue;
use plugins::ui_ast::{UiEvent, UiTreePayload};

use super::updates::{apply_registry_changed, push_toast_capped};
use super::{PluginHost, PluginViewOp, WakeSignal};

impl PluginHost {
    pub(super) fn enqueue_plugin_op(plugin_id: String, op: PluginViewOp, cx: &mut App) {
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
