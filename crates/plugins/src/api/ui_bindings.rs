//! UI, hooks, events, timers, commands, search, services, console, and system
//! QuickJS host API bindings.

use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use rquickjs::function::{Opt, Rest};
use rquickjs::promise::Promised;
use rquickjs::{Ctx, Exception, Function, Object, Persistent, Value};
use serde_json::Value as JsonValue;

use super::convert::{
    format_js_value, js_to_json, js_value_to_ui_node, json_to_js, now_ms, parse_uuid,
    throw_scrubbed,
};
use super::{
    LogLevel, MAX_TIMER_INTERVAL_MS, MAX_TIMERS_PER_PLUGIN, MIN_TIMER_INTERVAL_MS, PluginApiState,
    PluginUiEffect, PromptReplyValue, RegisteredTimer, RegisteredView, RenderScopeGuard,
    ToastLevel,
};
use crate::permissions::Permission;

pub(super) fn install_console<'js>(
    ctx: &Ctx<'js>,
    globals: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let console = Object::new(ctx.clone())?;
    for (name, level) in [
        ("log", LogLevel::Info),
        ("info", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("error", LogLevel::Error),
        ("debug", LogLevel::Debug),
    ] {
        let st = Rc::clone(&state);
        console.set(
            name,
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                let mut parts = Vec::with_capacity(args.0.len());
                for val in args.0 {
                    parts.push(format_js_value(&ctx, val));
                }
                st.push_log(level, &parts.join(" "));
            })?,
        )?;
    }
    globals.set("console", console)?;
    Ok(())
}

pub(crate) fn event_permission(kind: &str) -> Option<Permission> {
    if kind.starts_with("task.") || kind == "task" {
        Some(Permission::TasksRead)
    } else if kind.starts_with("habit.") || kind == "habit" {
        Some(Permission::HabitsRead)
    } else if kind.starts_with("focus.") || kind == "focus" {
        Some(Permission::FocusRead)
    } else if kind.starts_with("list.") || kind == "list" {
        Some(Permission::ListsRead)
    } else if kind.starts_with("countdown.") || kind == "countdown" {
        Some(Permission::CountdownsRead)
    } else {
        None
    }
}

pub(super) fn install_events<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;
    let st = Rc::clone(&state);

    obj.set(
        "on",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, kind: String, cb: Function<'js>| -> rquickjs::Result<Function<'js>> {
                if let Some(req) = event_permission(&kind) {
                    st.require_perm(&ctx, &req)?;
                }
                let id = st.next_listener_id.get().max(1);
                st.next_listener_id.set(id + 1);
                let persistent = Persistent::save(&ctx, cb);
                st.event_listeners
                    .borrow_mut()
                    .entry(kind.clone())
                    .or_default()
                    .push((id, persistent));

                let st_unsub = Rc::clone(&st);
                Function::new(ctx, move || {
                    if let Some(list) = st_unsub.event_listeners.borrow_mut().get_mut(&kind) {
                        list.retain(|(entry_id, _)| *entry_id != id);
                    }
                })
            },
        )?,
    )?;

    fern.set("events", obj)?;
    Ok(())
}

pub(super) fn install_hooks<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    for name in [
        "beforeTaskCreate",
        "beforeTaskComplete",
        "quickAddTransform",
        "onReminder",
    ] {
        let st = Rc::clone(&state);
        obj.set(
            name,
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, cb: Function<'js>| -> rquickjs::Result<()> {
                st.require_perm(&ctx, &Permission::HooksIntercept)?;
                let persistent = Persistent::save(&ctx, cb);
                let mut hooks = st.hooks.borrow_mut();
                match name {
                    "beforeTaskCreate" => hooks.before_task_create.push(persistent),
                    "beforeTaskComplete" => hooks.before_task_complete.push(persistent),
                    "quickAddTransform" => hooks.quick_add_transform.push(persistent),
                    "onReminder" => hooks.on_reminder.push(persistent),
                    _ => {}
                }
                Ok(())
            })?,
        )?;
    }

    fern.set("hooks", obj)?;
    Ok(())
}

pub(super) fn install_timers<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "setInterval",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, id: String, ms_f64: f64, cb: Function<'js>| -> rquickjs::Result<()> {
                    let id = id.trim().to_owned();
                    if id.is_empty() {
                        return Err(Exception::throw_message(&ctx, "timer id must not be empty"));
                    }
                    if !ms_f64.is_finite()
                        || ms_f64 < MIN_TIMER_INTERVAL_MS as f64
                        || ms_f64 > MAX_TIMER_INTERVAL_MS as f64
                    {
                        return Err(Exception::throw_message(
                            &ctx,
                            &format!(
                                "timer interval must be between {MIN_TIMER_INTERVAL_MS} ms and {MAX_TIMER_INTERVAL_MS} ms"
                            ),
                        ));
                    }
                    let ms = ms_f64 as u64;
                    let next_fire = Instant::now()
                        .checked_add(Duration::from_millis(ms))
                        .ok_or_else(|| {
                            Exception::throw_message(&ctx, "timer interval overflow")
                        })?;
                    let mut timers = st.timers.borrow_mut();
                    if !timers.contains_key(&id) && timers.len() >= MAX_TIMERS_PER_PLUGIN {
                        return Err(Exception::throw_message(
                            &ctx,
                            &format!("maximum of {MAX_TIMERS_PER_PLUGIN} timers per plugin reached"),
                        ));
                    }
                    let timer = RegisteredTimer {
                        id: id.clone(),
                        interval_ms: ms,
                        next_fire,
                        callback: Persistent::save(&ctx, cb),
                    };
                    timers.insert(id, timer);
                    Ok(())
                },
            )?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "clearInterval",
            Function::new(ctx.clone(), move |id: String| -> bool {
                st.timers.borrow_mut().remove(id.trim()).is_some()
            })?,
        )?;
    }

    fern.set("timers", obj)?;
    Ok(())
}

pub(super) fn install_commands<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;
    let st = Rc::clone(&state);
    obj.set(
        "register",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, command_id: String, handler: Function<'js>| -> rquickjs::Result<()> {
                let id = command_id.trim();
                if id.is_empty() {
                    return Err(Exception::throw_message(&ctx, "command id must not be empty"));
                }
                st.commands
                    .borrow_mut()
                    .insert(id.to_owned(), Persistent::save(&ctx, handler));
                Ok(())
            },
        )?,
    )?;
    fern.set("commands", obj)?;
    Ok(())
}

pub(super) fn install_search<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;
    let st = Rc::clone(&state);
    obj.set(
        "registerProvider",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, provider_id: String, handler: Function<'js>| -> rquickjs::Result<()> {
                let id = provider_id.trim();
                if id.is_empty() {
                    return Err(Exception::throw_message(
                        &ctx,
                        "search provider id must not be empty",
                    ));
                }
                st.search_providers
                    .borrow_mut()
                    .insert(id.to_owned(), Persistent::save(&ctx, handler));
                Ok(())
            },
        )?,
    )?;
    fern.set("search", obj)?;
    Ok(())
}

pub(super) fn install_services<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "provide",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, name: String, handler: Function<'js>| -> rquickjs::Result<()> {
                    st.require_perm(&ctx, &Permission::Services)?;
                    let trimmed = name.trim();
                    if trimmed.is_empty() {
                        return Err(Exception::throw_message(&ctx, "service name must not be empty"));
                    }
                    st.services
                        .borrow_mut()
                        .insert(trimmed.to_owned(), Persistent::save(&ctx, handler));
                    (st.service_registrar)(trimmed.to_owned());
                    Ok(())
                },
            )?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "call",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, name: String, arg: Opt<Value<'js>>| -> rquickjs::Result<Value<'js>> {
                    st.require_perm(&ctx, &Permission::Services)?;
                    let arg_json = match arg.0 {
                        Some(v) if !v.is_undefined() => {
                            js_to_json(&ctx, v).map_err(|e| throw_scrubbed(&ctx, &st, e))?
                        }
                        _ => JsonValue::Null,
                    };
                    let rx = (st.service_caller)(name, arg_json, st.active_service_depth.get());
                    let st_fut = Rc::clone(&st);
                    let ctx_fut = ctx.clone();
                    let promised = Promised::from(async move {
                        match rx.await {
                            Ok(Ok(val)) => json_to_js(&ctx_fut, &val),
                            Ok(Err(err)) => Err(throw_scrubbed(&ctx_fut, &st_fut, err)),
                            Err(_) => Err(Exception::throw_message(
                                &ctx_fut,
                                "service call cancelled",
                            )),
                        }
                    });
                    rquickjs::IntoJs::into_js(promised, &ctx)
                },
            )?,
        )?;
    }

    fern.set("services", obj)?;
    Ok(())
}

pub(super) fn install_views<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;
    let st = Rc::clone(&state);

    obj.set(
        "register",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, view_id: String, spec: Object<'js>| -> rquickjs::Result<()> {
                let id = view_id.trim();
                if id.is_empty() {
                    return Err(Exception::throw_message(&ctx, "view id must not be empty"));
                }
                if !st.declared_views.is_empty() && !st.declared_views.contains(id) {
                    return Err(Exception::throw_message(
                        &ctx,
                        &format!(
                            "view {id:?} is not declared in manifest.contributions.sidebar or manifest.contributions.pages"
                        ),
                    ));
                }
                let render: Function = spec.get("render").map_err(|_| {
                    Exception::throw_message(&ctx, "view registration requires a render() function")
                })?;
                let mount = spec
                    .get::<_, Option<Function>>("mount")?
                    .map(|f| Persistent::save(&ctx, f));
                let unmount = spec
                    .get::<_, Option<Function>>("unmount")?
                    .map(|f| Persistent::save(&ctx, f));
                st.views.borrow_mut().insert(
                    id.to_owned(),
                    RegisteredView {
                        render: Persistent::save(&ctx, render),
                        mount,
                        unmount,
                    },
                );
                Ok(())
            },
        )?,
    )?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "refresh",
            Function::new(ctx.clone(), move |view_id: String| {
                let id = view_id.trim();
                if !id.is_empty() && st.views.borrow().contains_key(id) {
                    (st.view_refresher)(id.to_owned());
                }
            })?,
        )?;
    }

    fern.set("views", obj)?;
    Ok(())
}

pub(super) fn install_ui<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "toast",
            Function::new(ctx.clone(), move |message: String, level: Opt<Option<String>>| {
                let level_owned = level.0.flatten();
                st.pending_ui_effects.borrow_mut().push_back(PluginUiEffect::Toast {
                    plugin_id: st.plugin_id.clone(),
                    message,
                    level: ToastLevel::parse(level_owned.as_deref()),
                    created_at_ms: now_ms(),
                });
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "confirm",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, title: String, body: String| -> rquickjs::Result<Value<'js>> {
                    let req_id = st.next_prompt_id.get().max(1);
                    st.next_prompt_id.set(req_id + 1);
                    let (tx, rx) = oneshot::channel();
                    st.prompt_senders
                        .borrow_mut()
                        .insert(req_id, (st.plugin_id.clone(), tx));
                    // Emit immediately so confirm() works inside `async activate()` and `UserScript`s
                    // where `slot.runtime` is not yet registered for deferred side-effect flushing.
                    (st.effect_emitter)(PluginUiEffect::Confirm {
                        request_id: req_id,
                        plugin_id: st.plugin_id.clone(),
                        title,
                        body,
                    });
                    let promised = Promised::from(async move {
                        match rx.await {
                            Ok(PromptReplyValue::Confirm(ok)) => Ok::<bool, rquickjs::Error>(ok),
                            _ => Ok::<bool, rquickjs::Error>(false),
                        }
                    });
                    rquickjs::IntoJs::into_js(promised, &ctx)
                },
            )?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "prompt",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      title: String,
                      placeholder: Opt<Option<String>>,
                      default_value: Opt<Option<String>>|
                      -> rquickjs::Result<Value<'js>> {
                    let req_id = st.next_prompt_id.get().max(1);
                    st.next_prompt_id.set(req_id + 1);
                    let (tx, rx) = oneshot::channel();
                    st.prompt_senders
                        .borrow_mut()
                        .insert(req_id, (st.plugin_id.clone(), tx));
                    // Emit immediately so prompt() works inside `async activate()` and `UserScript`s.
                    (st.effect_emitter)(PluginUiEffect::Prompt {
                        request_id: req_id,
                        plugin_id: st.plugin_id.clone(),
                        title,
                        placeholder: placeholder.0.flatten().unwrap_or_default(),
                        default_value: default_value.0.flatten().unwrap_or_default(),
                    });
                    let promised = Promised::from(async move {
                        match rx.await {
                            Ok(PromptReplyValue::Prompt(val)) => Ok::<Option<String>, rquickjs::Error>(val),
                            _ => Ok::<Option<String>, rquickjs::Error>(None),
                        }
                    });
                    rquickjs::IntoJs::into_js(promised, &ctx)
                },
            )?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "openModal",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, title: String, view_or_node: Value<'js>| -> rquickjs::Result<()> {
                    let modal_scope = "__modal__";
                    // Clean up any previously mounted reactive modal view before replacing.
                    if let Some((_, prev_view)) = st.modal_view.borrow_mut().take()
                        && let Some(unmount_pers) = prev_view.unmount
                        && let Ok(unmount_fn) = unmount_pers.restore(&ctx)
                    {
                        let _ = unmount_fn.call::<_, Value<'js>>(());
                    }
                    st.clear_scope_handlers(modal_scope);
                    let _scope_guard = RenderScopeGuard::enter(Rc::clone(&st), modal_scope.to_owned());

                    let (node_val, pending_modal_view) = if let Some(func) = view_or_node.as_function() {
                        let reg = RegisteredView {
                            render: Persistent::save(&ctx, func.clone()),
                            mount: None,
                            unmount: None,
                        };
                        (func.call::<_, Value<'js>>(())?, Some((title.clone(), reg)))
                    } else if let Some(obj) = view_or_node.as_object()
                        && let Ok(render_fn) = obj.get::<_, Function>("render")
                    {
                        let mount_fn = obj.get::<_, Option<Function>>("mount")?;
                        let unmount_fn = obj.get::<_, Option<Function>>("unmount")?;
                        let reg = RegisteredView {
                            render: Persistent::save(&ctx, render_fn.clone()),
                            mount: mount_fn.as_ref().map(|f| Persistent::save(&ctx, f.clone())),
                            unmount: unmount_fn.as_ref().map(|f| Persistent::save(&ctx, f.clone())),
                        };
                        if let Some(m) = mount_fn {
                            let _ = m.call::<_, Value<'js>>(())?;
                        }
                        let val = match render_fn.call::<_, Value<'js>>(()) {
                            Ok(v) => v,
                            Err(err) => {
                                if let Some(u) = unmount_fn {
                                    let _ = u.call::<_, Value<'js>>(());
                                }
                                st.clear_scope_handlers(modal_scope);
                                return Err(err);
                            }
                        };
                        (val, Some((title.clone(), reg)))
                    } else {
                        (view_or_node, None)
                    };

                    let tree = match js_value_to_ui_node(&ctx, node_val, &st, modal_scope) {
                        Ok(tree) => tree,
                        Err(e) => {
                            if let Some((_, reg)) = pending_modal_view
                                && let Some(unmount_pers) = reg.unmount
                                && let Ok(unmount_fn) = unmount_pers.restore(&ctx)
                            {
                                let _ = unmount_fn.call::<_, Value<'js>>(());
                            }
                            st.clear_scope_handlers(modal_scope);
                            return Err(throw_scrubbed(&ctx, &st, e));
                        }
                    };
                    *st.modal_view.borrow_mut() = pending_modal_view;

                    st.pending_ui_effects
                        .borrow_mut()
                        .push_back(PluginUiEffect::OpenModal {
                            plugin_id: st.plugin_id.clone(),
                            view_id: modal_scope.to_owned(),
                            title,
                            tree,
                        });
                    Ok(())
                },
            )?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "closeModal",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                if let Some((_, prev_view)) = st.modal_view.borrow_mut().take()
                    && let Some(unmount_pers) = prev_view.unmount
                    && let Ok(unmount_fn) = unmount_pers.restore(&ctx)
                {
                    let _ = unmount_fn.call::<_, Value<'js>>(());
                }
                st.clear_scope_handlers("__modal__");
                st.pending_ui_effects
                    .borrow_mut()
                    .push_back(PluginUiEffect::CloseModal {
                        plugin_id: st.plugin_id.clone(),
                    });
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "openTask",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String| -> rquickjs::Result<()> {
                let task_id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                st.pending_ui_effects
                    .borrow_mut()
                    .push_back(PluginUiEffect::OpenTask { task_id });
                Ok(())
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "openSettings",
            Function::new(ctx.clone(), move || {
                st.pending_ui_effects
                    .borrow_mut()
                    .push_back(PluginUiEffect::OpenSettings {
                        plugin_id: st.plugin_id.clone(),
                    });
            })?,
        )?;
    }

    fern.set("ui", obj)?;
    Ok(())
}

pub(super) fn install_system<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "locale",
            Function::new(ctx.clone(), move || -> String { st.locale.borrow().clone() })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "themeAppearance",
            Function::new(ctx.clone(), move || -> String {
                st.theme_appearance.borrow().clone()
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "openExternal",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, url: String| -> rquickjs::Result<()> {
                let trimmed = url.trim();
                let lower = trimmed.to_ascii_lowercase();
                if !(lower.starts_with("https://") || lower.starts_with("http://")) {
                    return Err(Exception::throw_message(
                        &ctx,
                        "openExternal only allows http:// or https:// URLs",
                    ));
                }
                st.pending_ui_effects
                    .borrow_mut()
                    .push_back(PluginUiEffect::OpenExternal {
                        url: trimmed.to_owned(),
                    });
                Ok(())
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "clipboardWrite",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, text: String| -> rquickjs::Result<()> {
                st.require_perm(&ctx, &Permission::Clipboard)?;
                st.pending_ui_effects
                    .borrow_mut()
                    .push_back(PluginUiEffect::ClipboardWrite { text });
                Ok(())
            })?,
        )?;
    }

    fern.set("system", obj)?;
    Ok(())
}
