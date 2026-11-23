use std::rc::Rc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use fern_core::Store;
use fern_core::plugin_store::MAX_EVENT_DEPTH;
use rquickjs::promise::MaybePromise;
use rquickjs::{Function, Persistent, Value};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use super::gate::{GateOptions, run_with_gate};
use super::manager::{HookDecision, ReplyTx};
use super::scripts::run_user_script;
use super::{
    DEFAULT_EVENT_TIMEOUT, OUTBOX_BATCH_SIZE, WorkerEnv, active_runtime_handles, ensure_activated,
};
use crate::api::{event_permission, js_to_json, json_to_js, task_to_json};
use crate::permissions::{Permission, PermissionSet};

#[derive(Clone, Copy, Debug)]
pub(super) enum HookKind {
    BeforeTaskCreate,
    BeforeTaskComplete,
    QuickAddTransform,
    OnReminder,
}

pub(super) fn schedule_outbox_drain(env: &Rc<WorkerEnv>) {
    if env.outbox_draining.replace(true) {
        return;
    }
    let env_c = Rc::clone(env);
    env.spawn_task(async move {
        struct DrainGuard(Rc<WorkerEnv>);
        impl Drop for DrainGuard {
            fn drop(&mut self) {
                self.0.outbox_draining.set(false);
                for reply in self.0.outbox_waiters.borrow_mut().drain(..) {
                    let _ = reply.send(Err(anyhow!("outbox drain cancelled")));
                }
            }
        }
        let _guard = DrainGuard(Rc::clone(&env_c));
        let mut total_delivered = 0usize;
        let mut last_err: Option<anyhow::Error> = None;

        loop {
            env_c.outbox_dirty.set(false);
            match dispatch_outbox_batch(&env_c).await {
                Ok((batch_events, batch_delivered)) => {
                    total_delivered += batch_delivered;
                    if batch_events >= OUTBOX_BATCH_SIZE || env_c.outbox_dirty.get() {
                        continue;
                    }
                    break;
                }
                Err(err) => {
                    last_err = Some(err);
                    break;
                }
            }
        }

        let waiters: Vec<ReplyTx<usize>> = env_c.outbox_waiters.borrow_mut().drain(..).collect();
        env_c.outbox_draining.set(false);
        match last_err {
            Some(err) => {
                let msg = format!("{err:#}");
                for reply in waiters {
                    let _ = reply.send(Err(anyhow!("{msg}")));
                }
            }
            None => {
                for reply in waiters {
                    let _ = reply.send(Ok(total_delivered));
                }
            }
        }
        if env_c.outbox_dirty.get() || !env_c.outbox_waiters.borrow().is_empty() {
            schedule_outbox_drain(&env_c);
        }
    });
}

async fn dispatch_outbox_batch(env: &Rc<WorkerEnv>) -> Result<(usize, usize)> {
    if env.config.safe_mode {
        return Ok((0, 0));
    }
    let events = env.store.drain_plugin_outbox(OUTBOX_BATCH_SIZE)?;
    let event_count = events.len();
    if events.is_empty() {
        return Ok((0, 0));
    }
    let last_seq = events.last().map(|e| e.seq).unwrap_or(0);
    let mut delivered = 0usize;

    let user_scripts = env.store.user_scripts().unwrap_or_default();

    for ev in events {
        if ev.depth >= MAX_EVENT_DEPTH {
            eprintln!(
                "plugin_outbox stopping causal event {:?} at depth {} (>= MAX_EVENT_DEPTH {})",
                ev.event_kind, ev.depth, MAX_EVENT_DEPTH
            );
            continue;
        }
        let next_depth = ev.depth.saturating_add(1);
        let required_perm = event_permission(&ev.event_kind);
        let entity_json = load_outbox_entity_json(&env.store, &ev.event_kind, &ev.entity_id);
        let payload = json!({
            "kind": ev.event_kind,
            "entityId": ev.entity_id,
            "originPlugin": ev.origin_plugin,
            "depth": ev.depth,
            "at": ev.at,
            "entity": entity_json,
        });

        let activation_token = format!("onEvent:{}", ev.event_kind);
        let candidate_ids: Vec<String> = env
            .slots
            .borrow()
            .values()
            .filter(|s| {
                s.enabled
                    && required_perm
                        .as_ref()
                        .is_none_or(|p| s.loaded_manifest.permissions.has(p))
                    && ev.origin_plugin.as_deref() != Some(s.loaded_manifest.manifest.id.as_str())
                    && (s.runtime.is_some()
                        || s.loaded_manifest
                            .manifest
                            .matches_activation_event(&activation_token))
            })
            .map(|s| s.loaded_manifest.manifest.id.clone())
            .collect();

        for pid in candidate_ids {
            if ensure_activated(env, &pid).await.is_err() {
                continue;
            }
            if deliver_event_to_plugin(env, &pid, &ev.event_kind, payload.clone(), next_depth)
                .await
            {
                delivered += 1;
            }
        }

        // Trigger matching enabled user scripts (`trigger_kind == "event"`), enforcing event permissions.
        for script in user_scripts.iter().filter(|s| {
            s.enabled
                && s.trigger_kind == "event"
                && (s.trigger_arg == ev.event_kind || s.trigger_arg == "*")
        }) {
            let script_origin = format!("script:{}", script.id);
            if ev.origin_plugin.as_deref() == Some(script_origin.as_str()) {
                continue;
            }
            let script_perms =
                PermissionSet::from_json_str(&script.permissions).unwrap_or_default();
            if let Some(req_perm) = &required_perm
                && !script_perms.has(req_perm)
            {
                continue;
            }
            let _ = run_user_script(env, script, Some(payload.clone()), next_depth).await;
            delivered += 1;
        }
    }

    env.store.ack_plugin_outbox(last_seq)?;
    Ok((event_count, delivered))
}

pub(super) async fn emit_simple_event(
    env: &Rc<WorkerEnv>,
    kind: &str,
    payload: JsonValue,
) -> Result<()> {
    if env.config.safe_mode {
        return Ok(());
    }
    let activation_token = format!("onEvent:{kind}");
    let required_perm = event_permission(kind);
    let candidate_ids: Vec<String> = env
        .slots
        .borrow()
        .values()
        .filter(|s| {
            s.enabled
                && required_perm
                    .as_ref()
                    .is_none_or(|p| s.loaded_manifest.permissions.has(p))
                && (s.runtime.is_some()
                    || s.loaded_manifest
                        .manifest
                        .matches_activation_event(&activation_token))
        })
        .map(|s| s.loaded_manifest.manifest.id.clone())
        .collect();
    for pid in candidate_ids {
        if ensure_activated(env, &pid).await.is_ok() {
            let _ = deliver_event_to_plugin(env, &pid, kind, payload.clone(), 0).await;
        }
    }
    Ok(())
}

async fn deliver_event_to_plugin(
    env: &Rc<WorkerEnv>,
    plugin_id: &str,
    event_kind: &str,
    payload: JsonValue,
    write_depth: u32,
) -> bool {
    let Ok((rt_gen, rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, plugin_id)
    else {
        return false;
    };
    // Enforce event capability permission on delivery so `fern.events.on("*")` cannot bypass
    // `tasks:read` / `habits:read` / `focus:read` / etc.
    if let Some(req_perm) = event_permission(event_kind)
        && !api.permissions.has(&req_perm)
    {
        return false;
    }

    let listeners: Vec<Persistent<Function<'static>>> = {
        let map = api.event_listeners.borrow();
        let mut out = Vec::new();
        if let Some(list) = map.get(event_kind) {
            out.extend(list.iter().map(|(_, f)| f.clone()));
        }
        if let Some(prefix) = event_kind.split('.').next()
            && let Some(list) = map.get(&format!("{prefix}.*"))
        {
            out.extend(list.iter().map(|(_, f)| f.clone()));
        }
        if let Some(list) = map.get("*") {
            out.extend(list.iter().map(|(_, f)| f.clone()));
        }
        out
    };
    if listeners.is_empty() {
        return false;
    }

    let start = Instant::now();
    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions {
            budget: DEFAULT_EVENT_TIMEOUT,
            driver_state: Some(driver_state),
            api: Some(Rc::clone(&api)),
            write_depth,
            service_depth: 0,
        },
        async move |ctx| {
            let js_payload = json_to_js(&ctx, &payload)?;
            for func_pers in listeners {
                let func = func_pers.restore(&ctx)?;
                let r: MaybePromise = func.call((js_payload.clone(),))?;
                let _ = r.into_future::<Value>().await?;
            }
            Ok(())
        },
    )
    .await;

    let elapsed = start.elapsed().as_millis() as u64;
    match res {
        Ok(()) => {
            let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
            env.record_success(plugin_id, Some(rt_gen), elapsed, mem);
            env.flush_plugin_side_effects(plugin_id, Some(rt_gen));
            true
        }
        Err(err) => {
            env.record_failure(plugin_id, Some(rt_gen), &format!("{err:#}"));
            false
        }
    }
}

pub(super) async fn run_hooks(
    env: &Rc<WorkerEnv>,
    hook: HookKind,
    payload: JsonValue,
) -> Result<HookDecision> {
    if env.config.safe_mode {
        return Ok(HookDecision::Allow);
    }
    let hook_token = match hook {
        HookKind::BeforeTaskCreate => "onHook:beforeTaskCreate",
        HookKind::BeforeTaskComplete => "onHook:beforeTaskComplete",
        HookKind::QuickAddTransform => "onHook:quickAddTransform",
        HookKind::OnReminder => "onHook:onReminder",
    };

    let candidate_ids: Vec<String> = env
        .slots
        .borrow()
        .values()
        .filter(|s| {
            s.enabled
                && s.loaded_manifest
                    .permissions
                    .has(&Permission::HooksIntercept)
                && (s.runtime.is_some()
                    || s.loaded_manifest
                        .manifest
                        .matches_activation_event(hook_token))
        })
        .map(|s| s.loaded_manifest.manifest.id.clone())
        .collect();

    let mut current = payload;
    let mut modified = false;

    for pid in candidate_ids {
        if ensure_activated(env, &pid).await.is_err() {
            continue;
        }
        let Ok((rt_gen, rt, ctx, gate, api, driver_state)) = active_runtime_handles(env, &pid)
        else {
            continue;
        };
        let handlers: Vec<Persistent<Function<'static>>> = {
            let hooks = api.hooks.borrow();
            match hook {
                HookKind::BeforeTaskCreate => hooks.before_task_create.clone(),
                HookKind::BeforeTaskComplete => hooks.before_task_complete.clone(),
                HookKind::QuickAddTransform => hooks.quick_add_transform.clone(),
                HookKind::OnReminder => hooks.on_reminder.clone(),
            }
        };
        if handlers.is_empty() {
            continue;
        }

        let arg_payload = current.clone();
        let start = Instant::now();
        let res = run_with_gate(
            &ctx,
            &gate,
            GateOptions::for_runtime(DEFAULT_EVENT_TIMEOUT, driver_state, Rc::clone(&api)),
            async move |ctx| {
                let mut cur_val = arg_payload;
                let mut changed = false;
                for func_pers in handlers {
                    let func = func_pers.restore(&ctx)?;
                    let js_arg = json_to_js(&ctx, &cur_val)?;
                    let raw: MaybePromise = func.call((js_arg,))?;
                    let val = raw.into_future::<Value>().await?;
                    if val.is_undefined() || val.is_null() {
                        continue;
                    }
                    let out_json = js_to_json(&ctx, val)
                        .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("{e:#}")))?;
                    if matches!(hook, HookKind::OnReminder) {
                        // Reminders are observational notifications; one plugin's return value
                        // must not cancel or alter delivery to subsequent plugins.
                        continue;
                    }
                    if out_json == JsonValue::Bool(false) {
                        return Ok(HookDecision::Cancel("cancelled by plugin hook".to_owned()));
                    }
                    if let Some(cancel_flag) = out_json.get("cancel").and_then(JsonValue::as_bool) {
                        if cancel_flag {
                            let reason = out_json
                                .get("reason")
                                .and_then(JsonValue::as_str)
                                .unwrap_or("cancelled by plugin hook")
                                .to_owned();
                            return Ok(HookDecision::Cancel(reason));
                        }
                        // `{ cancel: false }` without `draft`/`value` means Allow, not Modify.
                        if let Some(next_val) =
                            out_json.get("draft").or_else(|| out_json.get("value"))
                            && hook_payload_matches(hook, next_val)
                        {
                            cur_val = next_val.clone();
                            changed = true;
                        }
                        continue;
                    }
                    if let Some(next_val) = out_json.get("draft").or_else(|| out_json.get("value"))
                    {
                        if hook_payload_matches(hook, next_val) {
                            cur_val = next_val.clone();
                            changed = true;
                        }
                    } else if hook_payload_matches(hook, &out_json) {
                        cur_val = out_json;
                        changed = true;
                    }
                }
                if changed {
                    Ok(HookDecision::Modify(cur_val))
                } else {
                    Ok(HookDecision::Allow)
                }
            },
        )
        .await;

        let elapsed = start.elapsed().as_millis() as u64;
        match res {
            Ok(HookDecision::Allow) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, Some(rt_gen), elapsed, mem);
                env.flush_plugin_side_effects(&pid, Some(rt_gen));
            }
            Ok(HookDecision::Modify(next)) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, Some(rt_gen), elapsed, mem);
                env.flush_plugin_side_effects(&pid, Some(rt_gen));
                current = next;
                modified = true;
            }
            Ok(HookDecision::Cancel(reason)) => {
                let mem = rt.memory_usage().await.malloc_size.max(0) as u64;
                env.record_success(&pid, Some(rt_gen), elapsed, mem);
                env.flush_plugin_side_effects(&pid, Some(rt_gen));
                if !matches!(hook, HookKind::OnReminder) {
                    return Ok(HookDecision::Cancel(reason));
                }
            }
            Err(err) => {
                env.record_failure(&pid, Some(rt_gen), &format!("{err:#}"));
            }
        }
    }

    if modified {
        Ok(HookDecision::Modify(current))
    } else {
        Ok(HookDecision::Allow)
    }
}

fn hook_payload_matches(hook: HookKind, val: &JsonValue) -> bool {
    match hook {
        HookKind::QuickAddTransform => val.is_string(),
        HookKind::BeforeTaskCreate | HookKind::BeforeTaskComplete => val.is_object(),
        HookKind::OnReminder => false,
    }
}

fn load_outbox_entity_json(store: &Store, event_kind: &str, entity_id: &str) -> JsonValue {
    if event_kind.starts_with("task.")
        && let Ok(id) = Uuid::parse_str(entity_id)
        && let Ok(Some(task)) = store.task(id)
    {
        return task_to_json(&task);
    }
    if event_kind == "habit.checkin"
        && let Some((habit_id_str, date_str)) = entity_id.split_once('\x1f')
    {
        let habit_obj = Uuid::parse_str(habit_id_str)
            .ok()
            .and_then(|hid| {
                store
                    .habits()
                    .ok()
                    .and_then(|list| list.into_iter().find(|h| h.id == hid))
            })
            .map(|h| {
                json!({
                    "id": h.id.to_string(),
                    "name": h.name,
                    "archived": h.archived,
                    "section": h.section.key(),
                })
            });
        return json!({
            "habitId": habit_id_str,
            "date": date_str,
            "habit": habit_obj,
        });
    }
    if event_kind == "focus.completed"
        && let Ok(id) = Uuid::parse_str(entity_id)
        && let Ok(sessions) = store.focus_sessions()
        && let Some(s) = sessions.into_iter().find(|s| s.id == id)
    {
        return json!({
            "id": s.id.to_string(),
            "taskId": s.task_id.map(|i| i.to_string()),
            "habitId": s.habit_id.map(|i| i.to_string()),
            "title": s.title,
            "startedAt": s.started_at,
            "endedAt": s.ended_at,
            "minutes": s.minutes(),
            "stopwatch": s.stopwatch,
            "note": s.note,
        });
    }
    JsonValue::Null
}
