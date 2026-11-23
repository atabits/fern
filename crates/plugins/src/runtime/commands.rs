use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use rquickjs::promise::MaybePromise;
use rquickjs::{Function, Persistent, Value};
use serde_json::{Map, Value as JsonValue};
use uuid::Uuid;

use super::events::{emit_simple_event, run_hooks, schedule_outbox_drain};
use super::gate::{GateOptions, run_with_gate};
use super::manager::WorkerCmd;
use super::scripts::run_user_script;
use super::{
    CIRCUIT_BREAKER_THRESHOLD, DEFAULT_COMMAND_TIMEOUT, DEFAULT_EVENT_TIMEOUT, DirFingerprint,
    HostUpdate, PluginInfo, PluginStatus, SearchHit, WorkerEnv, active_runtime_handles,
    ensure_activated, maybe_spawn_startup_activation, spawn_startup_activations,
    unload_runtime_async,
};
use crate::api::{
    LogLevel, MAX_TIMER_INTERVAL_MS, MIN_TIMER_INTERVAL_MS, PluginUiEffect, RenderScopeGuard,
    js_to_json, js_value_to_ui_node, json_to_js,
};
use crate::manifest::{LoadedManifest, PermissionApprovalStatus, validate_plugin_id};
use crate::ui_ast::{UiEvent, UiTreePayload};

pub(super) fn handle_worker_cmd(env: &Rc<WorkerEnv>, cmd: WorkerCmd) {
    match cmd {
        WorkerCmd::GetPlugins(reply) => {
            let _ = reply.send(Ok(env.plugin_infos()));
        }
        WorkerCmd::ReloadAll(reply) => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = reload_all_plugins_async(&env_c).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::ReloadPlugin { plugin_id, reply } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = reload_single_plugin(&env_c, &plugin_id).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::SetEnabled {
            plugin_id,
            enabled,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = set_plugin_enabled_async(&env_c, &plugin_id, enabled).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::ApprovePermissions { plugin_id, reply } => {
            let res = approve_plugin_permissions(env, &plugin_id);
            let _ = reply.send(res);
        }
        WorkerCmd::ResetAndEnable { plugin_id, reply } => {
            let res = reset_and_enable(env, &plugin_id);
            let _ = reply.send(res);
        }
        WorkerCmd::UpdateSettings {
            plugin_id,
            settings,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = update_plugin_settings(&env_c, &plugin_id, settings).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::SetSecret {
            plugin_id,
            key,
            value,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = set_plugin_secret(&env_c, &plugin_id, &key, value).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::GetLogs { plugin_id, reply } => {
            let slots = env.slots.borrow();
            let logs = match slots.get(&plugin_id) {
                Some(slot) => match &slot.runtime {
                    Some(rt) => rt.api.logs.borrow().iter().cloned().collect(),
                    None => slot.logs_cache.iter().cloned().collect(),
                },
                None => Vec::new(),
            };
            let _ = reply.send(Ok(logs));
        }
        WorkerCmd::ClearLogs { plugin_id, reply } => {
            let mut slots = env.slots.borrow_mut();
            if let Some(slot) = slots.get_mut(&plugin_id) {
                slot.logs_cache.clear();
                if let Some(rt) = &slot.runtime {
                    rt.api.logs.borrow_mut().clear();
                }
            }
            let _ = reply.send(Ok(()));
        }
        WorkerCmd::RenderView {
            plugin_id,
            view_id,
            mount,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = render_plugin_view(&env_c, &plugin_id, &view_id, mount).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::UnmountView {
            plugin_id,
            view_id,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = unmount_plugin_view(&env_c, &plugin_id, &view_id).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::DispatchUiEvent {
            plugin_id,
            event,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = handle_ui_event(&env_c, &plugin_id, event).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::RunCommand {
            plugin_id,
            command_id,
            arg,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = execute_plugin_command(&env_c, &plugin_id, &command_id, arg).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::Search { query, reply } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = run_search_providers(&env_c, &query).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::ResolvePrompt {
            request_id,
            reply_val,
            reply,
        } => {
            if let Some((_owner, tx)) = env.prompt_senders.borrow_mut().remove(&request_id) {
                let _ = tx.send(reply_val);
            }
            let _ = reply.send(Ok(()));
        }
        WorkerCmd::DispatchOutbox(reply) => {
            env.outbox_waiters.borrow_mut().push(reply);
            env.outbox_dirty.set(true);
            schedule_outbox_drain(env);
        }
        WorkerCmd::EmitAppReady(reply) => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = emit_simple_event(&env_c, "app.ready", JsonValue::Null).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::RunHook {
            hook,
            payload,
            reply,
        } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = run_hooks(&env_c, hook, payload).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::TestRunScript { script, arg, reply } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = run_user_script(&env_c, &script, arg, 0).await;
                let _ = reply.send(Ok(res));
            });
        }
        WorkerCmd::SetAppearance {
            locale,
            theme_appearance,
        } => {
            if let Some(loc) = locale {
                *env.locale.borrow_mut() = loc;
            }
            if let Some(theme) = theme_appearance {
                *env.theme_appearance.borrow_mut() = theme;
            }
        }
        WorkerCmd::CheckHotReload(reply) => {
            let _ = reply.send(perform_hot_reload(env));
        }
        WorkerCmd::PurgePluginData { plugin_id, reply } => {
            let env_c = Rc::clone(env);
            env.spawn_task(async move {
                let res = purge_plugin_data_async(&env_c, &plugin_id).await;
                let _ = reply.send(res);
            });
        }
        WorkerCmd::Shutdown => {}
    }
}

async fn reload_all_plugins_async(env: &Rc<WorkerEnv>) -> Result<Vec<PluginInfo>> {
    let old_ids: Vec<String> = env.slots.borrow().keys().cloned().collect();
    for id in old_ids {
        unload_runtime_async(env, &id, None).await;
    }
    env.slots.borrow_mut().clear();
    env.discover_plugins_into_slots()?;
    spawn_startup_activations(env);
    let infos = env.plugin_infos();
    env.emit(HostUpdate::RegistryChanged(infos.clone()));
    Ok(infos)
}

async fn purge_plugin_data_async(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    let pid = plugin_id.trim();
    ensure!(!pid.is_empty(), "plugin_id must not be empty");
    unload_runtime_async(env, pid, None).await;
    env.slots.borrow_mut().remove(pid);
    env.activating.borrow_mut().remove(pid);
    env.vault.delete_plugin(pid)?;
    env.store.delete_plugin_all_data(pid)?;
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn set_plugin_enabled_async(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    enabled: bool,
) -> Result<()> {
    if enabled {
        let (loaded_manifest, current_status) = {
            let slots = env.slots.borrow();
            let slot = slots
                .get(plugin_id)
                .with_context(|| format!("plugin {plugin_id:?} not found"))?;
            (slot.loaded_manifest.clone(), slot.status.clone())
        };
        if matches!(
            current_status,
            PluginStatus::NeedsApproval { .. } | PluginStatus::CircuitBroken { .. }
        ) {
            bail!(
                "plugin {plugin_id} requires permission approval or circuit breaker reset before enabling"
            );
        }
        let state = env.store.plugin_state(plugin_id)?;
        match loaded_manifest
            .manifest
            .check_permission_diff(&loaded_manifest.manifest_hash, state.as_ref())
        {
            PermissionApprovalStatus::NeedsApproval {
                diff,
                is_first_install,
            } => {
                if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
                    slot.enabled = false;
                    slot.status = PluginStatus::NeedsApproval {
                        added_permissions: diff.added,
                        is_first_install,
                    };
                }
                env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
                bail!("plugin {plugin_id} requires permission approval before enabling");
            }
            PermissionApprovalStatus::Approved => {
                if let Some(narrowed) = loaded_manifest
                    .manifest
                    .narrowed_permissions_to_persist(&loaded_manifest.manifest_hash, state.as_ref())
                {
                    let _ = env.store.set_plugin_permissions(
                        plugin_id,
                        &narrowed,
                        &loaded_manifest.manifest_hash,
                    );
                }
            }
        }
        if state
            .as_ref()
            .is_some_and(|s| s.consecutive_errors >= CIRCUIT_BREAKER_THRESHOLD)
        {
            bail!("plugin {plugin_id} requires circuit breaker reset before enabling");
        }
    } else {
        let exists = env.slots.borrow().contains_key(plugin_id);
        ensure!(exists, "plugin {plugin_id:?} not found");
    }

    env.store.set_plugin_enabled(plugin_id, enabled)?;
    if !enabled {
        unload_runtime_async(env, plugin_id, Some(Duration::from_millis(100))).await;
    }
    if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
        slot.enabled = enabled;
        if !enabled {
            slot.status = PluginStatus::Disabled;
        } else if slot.runtime.is_none() {
            slot.status = PluginStatus::Idle;
        }
    }
    if enabled {
        maybe_spawn_startup_activation(env, plugin_id);
    }
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

fn approve_plugin_permissions(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    let (perms_json, hash) = {
        let slots = env.slots.borrow();
        let slot = slots
            .get(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        (
            slot.loaded_manifest.permissions.to_json_string()?,
            slot.loaded_manifest.manifest_hash.clone(),
        )
    };
    env.store
        .set_plugin_permissions(plugin_id, &perms_json, &hash)?;
    env.store.reset_plugin_errors(plugin_id)?;
    env.store.set_plugin_enabled(plugin_id, true)?;
    if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
        slot.enabled = true;
        slot.metrics.consecutive_errors = 0;
        if slot.runtime.is_none() {
            slot.status = PluginStatus::Idle;
        }
    }
    maybe_spawn_startup_activation(env, plugin_id);
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

fn reset_and_enable(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    let loaded_manifest = {
        let slots = env.slots.borrow();
        let slot = slots
            .get(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        if matches!(slot.status, PluginStatus::NeedsApproval { .. }) {
            bail!("plugin {plugin_id} requires permission approval first");
        }
        slot.loaded_manifest.clone()
    };

    // Re-verify persisted permission diff before enabling so a circuit-broken plugin whose
    // manifest changed on disk cannot escalate permissions via Reset & Enable.
    let state = env.store.plugin_state(plugin_id)?;
    match loaded_manifest
        .manifest
        .check_permission_diff(&loaded_manifest.manifest_hash, state.as_ref())
    {
        PermissionApprovalStatus::NeedsApproval {
            diff,
            is_first_install,
        } => {
            if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
                slot.enabled = false;
                slot.status = PluginStatus::NeedsApproval {
                    added_permissions: diff.added,
                    is_first_install,
                };
            }
            env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
            bail!("plugin {plugin_id} requires permission approval first");
        }
        PermissionApprovalStatus::Approved => {
            if let Some(narrowed) = loaded_manifest
                .manifest
                .narrowed_permissions_to_persist(&loaded_manifest.manifest_hash, state.as_ref())
            {
                let _ = env.store.set_plugin_permissions(
                    plugin_id,
                    &narrowed,
                    &loaded_manifest.manifest_hash,
                );
            }
        }
    }

    env.store.reset_plugin_errors(plugin_id)?;
    env.store.set_plugin_enabled(plugin_id, true)?;
    if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id) {
        slot.enabled = true;
        slot.metrics.consecutive_errors = 0;
        slot.status = PluginStatus::Idle;
    }
    maybe_spawn_startup_activation(env, plugin_id);
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn reload_single_plugin(env: &Rc<WorkerEnv>, plugin_id: &str) -> Result<()> {
    ensure!(
        validate_plugin_id(plugin_id),
        "invalid plugin id {plugin_id:?}"
    );
    let dir = {
        let slots = env.slots.borrow();
        slots
            .get(plugin_id)
            .map(|s| s.loaded_manifest.dir.clone())
            .unwrap_or_else(|| env.config.plugins_dir.join(plugin_id))
    };
    unload_runtime_async(env, plugin_id, Some(Duration::from_millis(100))).await;

    let loaded_res = LoadedManifest::load_from_dir(&dir).and_then(|loaded| {
        ensure!(
            loaded.manifest.id == plugin_id,
            "manifest id {:?} does not match plugin directory {:?}",
            loaded.manifest.id,
            plugin_id
        );
        Ok(loaded)
    });

    let loaded = match loaded_res {
        Ok(loaded) => loaded,
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            let next_snapshot = DirFingerprint::scan(&dir);
            let updated_status = {
                let mut slots = env.slots.borrow_mut();
                if let Some(slot) = slots.get_mut(plugin_id) {
                    slot.snapshot = next_snapshot;
                    slot.status = PluginStatus::Error(msg.clone());
                    Some((slot.status.clone(), slot.metrics.clone()))
                } else {
                    None
                }
            };
            if let Some((status, metrics)) = updated_status {
                env.emit(HostUpdate::PluginStatusChanged {
                    plugin_id: plugin_id.to_owned(),
                    status,
                    metrics,
                });
                env.emit(HostUpdate::ViewError {
                    plugin_id: plugin_id.to_owned(),
                    view_id: String::new(),
                    error: msg.clone(),
                });
                env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
            }
            return Err(anyhow!("{msg}"));
        }
    };

    let prev_logs = env
        .slots
        .borrow()
        .get(plugin_id)
        .map(|s| s.logs_cache.clone())
        .unwrap_or_default();
    let mut new_slot = env.build_slot(loaded)?;
    new_slot.logs_cache = prev_logs;
    let should_startup = new_slot.enabled
        && (new_slot.loaded_manifest.manifest.matches_activation_event("onStartup")
            || new_slot.loaded_manifest.manifest.matches_activation_event("*"));
    env.slots.borrow_mut().insert(plugin_id.to_owned(), new_slot);
    if should_startup {
        ensure_activated(env, plugin_id).await?;
    }
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn update_plugin_settings(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    settings: Map<String, JsonValue>,
) -> Result<()> {
    let filtered = {
        let slots = env.slots.borrow();
        let slot = slots
            .get(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        let secret_keys: HashSet<&str> = slot
            .loaded_manifest
            .manifest
            .settings
            .iter()
            .filter(|s| s.kind == crate::manifest::SettingKind::Secret)
            .map(|s| s.key.as_str())
            .collect();
        let mut clean = Map::new();
        for (k, v) in settings {
            if !secret_keys.contains(k.as_str()) {
                clean.insert(k, v);
            }
        }
        clean
    };

    let json_text = serde_json::to_string(&filtered)?;
    env.store.set_plugin_settings(plugin_id, &json_text)?;

    let resolved = {
        let mut slots = env.slots.borrow_mut();
        let slot = slots
            .get_mut(plugin_id)
            .with_context(|| format!("plugin {plugin_id:?} not found"))?;
        let next = slot.loaded_manifest.manifest.resolve_settings(&json_text);
        slot.settings_values = next.clone();
        if let Some(rt) = &slot.runtime {
            *rt.api.settings.borrow_mut() = next.clone();
        }
        next
    };

    notify_settings_listeners(env, plugin_id, resolved).await;
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn set_plugin_secret(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    key: &str,
    value: Option<String>,
) -> Result<()> {
    match value {
        Some(v) => env.vault.set(plugin_id, key, &v)?,
        None => {
            env.vault.delete(plugin_id, key)?;
        }
    }
    let resolved = env
        .slots
        .borrow()
        .get(plugin_id)
        .map(|s| s.settings_values.clone())
        .unwrap_or_default();
    notify_settings_listeners(env, plugin_id, resolved).await;
    env.emit(HostUpdate::RegistryChanged(env.plugin_infos()));
    Ok(())
}

async fn notify_settings_listeners(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    resolved: Map<String, JsonValue>,
) {
    // Notify registered `fern.settings.onChange` callbacks if runtime is active.
    let Ok((rt_gen, _rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, plugin_id) else {
        return;
    };
    let api_c = Rc::clone(&api);
    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            let listeners = api_c.settings_listeners.borrow().clone();
            let val = json_to_js(&ctx, &JsonValue::Object(resolved))?;
            for listener in listeners {
                if let Ok(func) = listener.restore(&ctx) {
                    let res: MaybePromise = func.call((val.clone(),))?;
                    let _ = res.into_future::<Value>().await?;
                }
            }
            Ok(())
        },
    )
    .await;
    if let Err(err) = res {
        let scrubbed = env.vault.scrub_secrets(
            plugin_id,
            &format!("settings.onChange listener error: {err:#}"),
        );
        api.push_log(LogLevel::Error, &scrubbed);
        if let Some(slot) = env.slots.borrow_mut().get_mut(plugin_id)
            && slot.runtime.as_ref().is_some_and(|r| r.generation == rt_gen)
        {
            slot.logs_cache = api.logs.borrow().clone();
        }
    }
    env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
}

pub(super) async fn render_plugin_view(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    view_id: &str,
    mount: bool,
) -> Result<UiTreePayload> {
    ensure_activated(env, plugin_id).await?;
    let (rt_gen, rt, ctx, gate, api, driver_state) = active_runtime_handles(env, plugin_id)?;
    let start = Instant::now();
    let view_id_own = view_id.to_owned();
    let api_c = Rc::clone(&api);

    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            let (render_fn, mount_fn) = {
                let views = api_c.views.borrow();
                let reg = views.get(&view_id_own).ok_or_else(|| {
                    rquickjs::Error::new_loading_message(&view_id_own, "view is not registered")
                })?;
                (reg.render.clone(), if mount { reg.mount.clone() } else { None })
            };
            if let Some(mount_pers) = mount_fn {
                let f = mount_pers.restore(&ctx)?;
                let r: MaybePromise = f.call(())?;
                let _ = r.into_future::<Value>().await?;
            }
            api_c.clear_scope_handlers(&view_id_own);
            let raw_node: MaybePromise = {
                let _scope_guard = RenderScopeGuard::enter(Rc::clone(&api_c), view_id_own.clone());
                let f = render_fn.restore(&ctx)?;
                f.call(())?
            };
            let val = raw_node.into_future::<Value>().await?;
            let parsed = {
                let _scope_guard = RenderScopeGuard::enter(Rc::clone(&api_c), view_id_own.clone());
                js_value_to_ui_node(&ctx, val, &api_c, &view_id_own)
            };
            match parsed {
                Ok(root) => Ok(UiTreePayload {
                    plugin_id: api_c.plugin_id.clone(),
                    view_id: view_id_own,
                    root,
                }),
                Err(err) => Err(rquickjs::Exception::throw_message(&ctx, &format!("{err:#}"))),
            }
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    let gen_current = env
        .slots
        .borrow()
        .get(plugin_id)
        .and_then(|s| s.runtime.as_ref())
        .is_some_and(|r| r.generation == rt_gen);
    if !gen_current {
        bail!("plugin {plugin_id:?} was unloaded while rendering view {view_id:?}");
    }

    match res {
        Ok(payload) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
            env.emit(HostUpdate::ViewRendered(payload.clone()));
            Ok(payload)
        }
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            env.record_failure(plugin_id, Some(rt_gen), &msg);
            env.emit(HostUpdate::ViewError {
                plugin_id: plugin_id.to_owned(),
                view_id: view_id.to_owned(),
                error: msg.clone(),
            });
            Err(anyhow!("{msg}"))
        }
    }
}

async fn render_modal_view(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    expected_gen: u64,
) -> Result<Option<UiTreePayload>> {
    let Ok((rt_gen, rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, plugin_id) else {
        return Ok(None);
    };
    if rt_gen != expected_gen {
        return Ok(None);
    }
    let modal_info = {
        let guard = api.modal_view.borrow();
        guard
            .as_ref()
            .map(|(title, view)| (title.clone(), view.render.clone()))
    };
    let Some((title, render_fn)) = modal_info else {
        return Ok(None);
    };

    let start = Instant::now();
    let api_c = Rc::clone(&api);
    let modal_scope = "__modal__".to_owned();
    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            api_c.clear_scope_handlers(&modal_scope);
            let raw_node: MaybePromise = {
                let _scope_guard = RenderScopeGuard::enter(Rc::clone(&api_c), modal_scope.clone());
                let f = render_fn.restore(&ctx)?;
                f.call(())?
            };
            let val = raw_node.into_future::<Value>().await?;
            let parsed = {
                let _scope_guard = RenderScopeGuard::enter(Rc::clone(&api_c), modal_scope.clone());
                js_value_to_ui_node(&ctx, val, &api_c, &modal_scope)
            };
            match parsed {
                Ok(root) => Ok(UiTreePayload {
                    plugin_id: api_c.plugin_id.clone(),
                    view_id: modal_scope,
                    root,
                }),
                Err(err) => Err(rquickjs::Exception::throw_message(&ctx, &format!("{err:#}"))),
            }
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    let gen_current = env
        .slots
        .borrow()
        .get(plugin_id)
        .and_then(|s| s.runtime.as_ref())
        .is_some_and(|r| r.generation == rt_gen);
    if !gen_current {
        return Ok(None);
    }

    match res {
        Ok(payload) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
            env.emit(HostUpdate::UiEffect(PluginUiEffect::OpenModal {
                plugin_id: plugin_id.to_owned(),
                view_id: payload.view_id.clone(),
                title,
                tree: payload.root.clone(),
            }));
            Ok(Some(payload))
        }
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            env.record_failure(plugin_id, Some(rt_gen), &msg);
            Err(anyhow!("{msg}"))
        }
    }
}

async fn unmount_plugin_view(env: &Rc<WorkerEnv>, plugin_id: &str, view_id: &str) -> Result<()> {
    let Ok((rt_gen, _rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, plugin_id) else {
        return Ok(());
    };
    let view_id_own = view_id.to_owned();
    let api_c = Rc::clone(&api);
    let _ = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            let unmount = if view_id_own == "__modal__" {
                api_c
                    .modal_view
                    .borrow_mut()
                    .take()
                    .and_then(|(_, v)| v.unmount)
            } else {
                api_c
                    .views
                    .borrow()
                    .get(&view_id_own)
                    .and_then(|v| v.unmount.clone())
            };
            if let Some(unmount_pers) = unmount {
                let f = unmount_pers.restore(&ctx)?;
                let r: MaybePromise = f.call(())?;
                let _ = r.into_future::<Value>().await?;
            }
            api_c.clear_scope_handlers(&view_id_own);
            Ok(())
        },
    )
    .await;
    env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
    Ok(())
}

async fn handle_ui_event(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    event: UiEvent,
) -> Result<Option<UiTreePayload>> {
    ensure_activated(env, plugin_id).await?;
    let (rt_gen, rt, ctx, gate, api, driver_state) = active_runtime_handles(env, plugin_id)?;
    let start = Instant::now();
    let scope = event.view_id.clone();
    let hid = event.handler_id.0;
    let payload = event.payload.clone();
    let api_c = Rc::clone(&api);

    let call_res = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            let handler = {
                let map = api_c.handlers.borrow();
                map.get(&scope).and_then(|s| s.get(&hid)).cloned()
            };
            let Some(handler_pers) = handler else {
                return Ok(false);
            };
            let f = handler_pers.restore(&ctx)?;
            let arg = json_to_js(&ctx, &payload)?;
            let res: MaybePromise = if payload.is_null() {
                f.call(())?
            } else {
                f.call((arg,))?
            };
            let _ = res.into_future::<Value>().await?;
            Ok(true)
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    let gen_current = env
        .slots
        .borrow()
        .get(plugin_id)
        .and_then(|s| s.runtime.as_ref())
        .is_some_and(|r| r.generation == rt_gen);
    if !gen_current {
        return Ok(None);
    }

    match call_res {
        Ok(_ran) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
            if event.view_id == "__modal__" && api.modal_view.borrow().is_some() {
                return render_modal_view(env, plugin_id, rt_gen).await;
            }
            let has_view = api.views.borrow().contains_key(&event.view_id);
            if has_view {
                let tree = render_plugin_view(env, plugin_id, &event.view_id, false).await?;
                Ok(Some(tree))
            } else {
                Ok(None)
            }
        }
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            env.record_failure(plugin_id, Some(rt_gen), &msg);
            Err(anyhow!("{msg}"))
        }
    }
}

async fn execute_plugin_command(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    command_id: &str,
    arg: Option<String>,
) -> Result<JsonValue> {
    ensure_activated(env, plugin_id).await?;
    let (rt_gen, rt, ctx, gate, api, driver_state) = active_runtime_handles(env, plugin_id)?;
    let start = Instant::now();
    let cmd_id = command_id.to_owned();
    let api_c = Rc::clone(&api);

    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions::for_runtime(DEFAULT_COMMAND_TIMEOUT, driver_state, Rc::clone(&api)),
        async move |ctx| {
            let handler = api_c.commands.borrow().get(&cmd_id).cloned().ok_or_else(|| {
                rquickjs::Exception::throw_message(&ctx, &format!("command {cmd_id:?} not registered"))
            })?;
            let f = handler.restore(&ctx)?;
            let raw: MaybePromise = match arg {
                Some(a) => f.call((a,))?,
                None => f.call(())?,
            };
            let val = raw.into_future::<Value>().await?;
            js_to_json(&ctx, val)
                .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("{e:#}")))
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    match res {
        Ok(val) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
            Ok(val)
        }
        Err(err) => {
            let msg = env.vault.scrub_secrets(plugin_id, &format!("{err:#}"));
            env.record_failure(plugin_id, Some(rt_gen), &msg);
            Err(anyhow!("{msg}"))
        }
    }
}

async fn run_search_providers(env: &Rc<WorkerEnv>, query: &str) -> Result<Vec<SearchHit>> {
    if env.config.safe_mode {
        return Ok(Vec::new());
    }
    let candidate_ids: Vec<String> = env
        .slots
        .borrow()
        .values()
        .filter(|s| {
            s.enabled
                && !s
                    .loaded_manifest
                    .manifest
                    .contributions
                    .search_providers
                    .is_empty()
        })
        .map(|s| s.loaded_manifest.manifest.id.clone())
        .collect();

    let mut all_hits = Vec::new();
    for pid in candidate_ids {
        if ensure_activated(env, &pid).await.is_err() {
            continue;
        }
        let Ok((rt_gen, rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, &pid) else {
            continue;
        };
        let q = query.to_owned();
        let api_c = Rc::clone(&api);
        let start = Instant::now();
        let res = run_with_gate(
            &ctx,
            &gate,
            GateOptions::for_runtime(DEFAULT_COMMAND_TIMEOUT, driver_state, Rc::clone(&api)),
            async move |ctx| {
                let providers: Vec<(String, Persistent<Function<'static>>)> = api_c
                    .search_providers
                    .borrow()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let mut hits = Vec::new();
                for (provider_id, func_pers) in providers {
                    let func = func_pers.restore(&ctx)?;
                    let res: MaybePromise = func.call((q.as_str(),))?;
                    let val = res.into_future::<Value>().await?;
                    let json = js_to_json(&ctx, val)
                        .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("{e:#}")))?;
                    if let Some(arr) = json.as_array() {
                        for item in arr {
                            let Some(title) = item.get("title").and_then(JsonValue::as_str) else {
                                continue;
                            };
                            let id = item
                                .get("id")
                                .and_then(JsonValue::as_str)
                                .map(str::to_owned)
                                .unwrap_or_else(|| Uuid::now_v7().to_string());
                            let task_id = item
                                .get("taskId")
                                .or_else(|| item.get("task_id"))
                                .and_then(JsonValue::as_str)
                                .and_then(|s| Uuid::parse_str(s).ok());
                            hits.push(SearchHit {
                                id,
                                plugin_id: api_c.plugin_id.clone(),
                                provider_id: provider_id.clone(),
                                title: title.to_owned(),
                                subtitle: item
                                    .get("subtitle")
                                    .and_then(JsonValue::as_str)
                                    .map(str::to_owned),
                                badge: item
                                    .get("badge")
                                    .and_then(JsonValue::as_str)
                                    .map(str::to_owned),
                                task_id,
                                command_id: item
                                    .get("commandId")
                                    .or_else(|| item.get("command_id"))
                                    .and_then(JsonValue::as_str)
                                    .map(str::to_owned),
                                command_arg: item
                                    .get("commandArg")
                                    .or_else(|| item.get("command_arg"))
                                    .and_then(JsonValue::as_str)
                                    .map(str::to_owned),
                            });
                        }
                    }
                }
                Ok(hits)
            },
        )
        .await;

        let elapsed = start.elapsed().as_millis() as u64;
        match res {
            Ok(mut hits) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, Some(rt_gen), elapsed, mem);
                all_hits.append(&mut hits);
            }
            Err(err) => {
                env.record_failure(&pid, Some(rt_gen), &format!("{err:#}"));
            }
        }
    }
    Ok(all_hits)
}

pub(super) fn fire_due_timers(env: &Rc<WorkerEnv>) {
    let now = Instant::now();
    // Pass only `(plugin_id, timer_id, generation)` — never clone `Persistent<Function>`
    // outside a task holding `rt`/`ctx`, avoiding drop-after-`AsyncRuntime` aborts / UAF.
    let mut due: Vec<(String, String, u64)> = Vec::new();
    {
        let slots = env.slots.borrow();
        for (pid, slot) in slots.iter() {
            let Some(rt) = &slot.runtime else {
                continue;
            };
            let rt_gen = rt.generation;
            for timer in rt.api.timers.borrow_mut().values_mut() {
                if timer.next_fire <= now {
                    let clamped_ms = timer
                        .interval_ms
                        .clamp(MIN_TIMER_INTERVAL_MS, MAX_TIMER_INTERVAL_MS);
                    timer.next_fire = now
                        .checked_add(Duration::from_millis(clamped_ms))
                        .unwrap_or(now);
                    due.push((pid.clone(), timer.id.clone(), rt_gen));
                }
            }
        }
    }
    for (pid, tid, expected_gen) in due {
        let env_c = Rc::clone(env);
        env.spawn_task(async move {
            let Ok((rt_gen, rt, ctx, gate, api, driver_state)) = active_runtime_handles(&env_c, &pid)
            else {
                return;
            };
            if rt_gen != expected_gen {
                return;
            }
            let api_c = Rc::clone(&api);
            let start = Instant::now();
            let res = run_with_gate(
                &ctx,
                &gate,
                GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
                async move |ctx| {
                    let cb = {
                        let timers = api_c.timers.borrow();
                        timers.get(&tid).map(|t| t.callback.clone())
                    };
                    let Some(cb) = cb else {
                        return Ok(());
                    };
                    let func = cb.restore(&ctx)?;
                    let raw: MaybePromise = func.call(())?;
                    let _ = raw.into_future::<Value>().await?;
                    Ok(())
                },
            )
            .await;
            let elapsed = start.elapsed().as_millis() as u64;
            match res {
                Ok(()) => {
                    let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                    env_c.record_success(&pid, Some(rt_gen), elapsed, mem);
                    env_c.flush_plugin_side_effects(&pid, Some(rt_gen));
                }
                Err(err) => {
                    env_c.record_failure(&pid, Some(rt_gen), &format!("{err:#}"));
                }
            }
        });
    }
}

pub(super) fn perform_hot_reload(env: &Rc<WorkerEnv>) -> Result<Vec<String>> {
    if env.config.safe_mode {
        return Ok(Vec::new());
    }
    let changed_ids: Vec<String> = {
        let slots = env.slots.borrow();
        slots
            .iter()
            .filter_map(|(id, slot)| {
                let current = DirFingerprint::scan(&slot.loaded_manifest.dir);
                (current != slot.snapshot).then_some(id.clone())
            })
            .collect()
    };
    if changed_ids.is_empty() {
        return Ok(Vec::new());
    }
    for pid in &changed_ids {
        let env_c = Rc::clone(env);
        let pid_own = pid.clone();
        env.spawn_task(async move {
            let _ = reload_single_plugin(&env_c, &pid_own).await;
        });
    }
    Ok(changed_ids)
}
