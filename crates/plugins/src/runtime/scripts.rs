use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use fern_core::plugin_store::UserScript;
use rquickjs::context::EvalOptions;
use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::promise::MaybePromise;
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Function, Module, Value};
use serde_json::{Map, Value as JsonValue};

use super::events::schedule_outbox_drain;
use super::gate::{ClearPersistentGuard, ExecGate, GateOptions, run_with_gate};
use super::{
    DEFAULT_COMMAND_TIMEOUT, HostUpdate, PLUGIN_MEMORY_LIMIT_BYTES, PLUGIN_STACK_LIMIT_BYTES,
    ScriptRunResult, WorkerEnv, make_effect_emitter, make_service_caller,
};
use crate::api::{
    LogEntry, PluginApiInit, PluginApiState, PluginUiEffect, install_fern_bindings, js_to_json,
    json_to_js,
};
use crate::module_loader::{SandboxedLoader, VIRTUAL_FERN_MODULE, VIRTUAL_FERN_UI_MODULE};
use crate::permissions::PermissionSet;

/// Virtual-only module resolver for UserScripts: permits ONLY `"fern"` and `"fern:ui"` and
/// rejects all filesystem/relative imports on every OS (preventing cross-plugin imports).
struct VirtualOnlyResolver;

impl Resolver for VirtualOnlyResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        if name == VIRTUAL_FERN_MODULE || name == VIRTUAL_FERN_UI_MODULE {
            Ok(name.to_owned())
        } else {
            Err(rquickjs::Error::new_resolving_message(
                base,
                name,
                "filesystem imports are forbidden in user scripts; only 'fern' and 'fern:ui' are available",
            ))
        }
    }
}

/// Virtual-only module loader for UserScripts: serves only `"fern"` and `"fern:ui"`.
struct VirtualOnlyLoader(SandboxedLoader);

impl VirtualOnlyLoader {
    fn new() -> Self {
        Self(SandboxedLoader::new(PathBuf::new()))
    }
}

impl Loader for VirtualOnlyLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js>> {
        if name == VIRTUAL_FERN_MODULE || name == VIRTUAL_FERN_UI_MODULE {
            self.0.load(ctx, name, attributes)
        } else {
            Err(rquickjs::Error::new_loading_message(
                name,
                "filesystem imports are forbidden in user scripts",
            ))
        }
    }
}

/// Replaces JS comments and string/template literal interiors with spaces so syntax checks
/// (`looks_like_es_module`, `has_return_or_await`) never misfire on strings or comments.
fn strip_js_comments_and_strings(code: &str) -> String {
    let bytes = code.as_bytes();
    let mut out = String::with_capacity(code.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        // Line comment `// ...`
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            out.push_str("  ");
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        // Block comment `/* ... */`
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            out.push_str("  ");
            i += 2;
            while i < bytes.len() {
                if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    out.push_str("  ");
                    i += 2;
                    break;
                }
                out.push(if bytes[i] == b'\n' { '\n' } else { ' ' });
                i += 1;
            }
            continue;
        }
        // Single/double/template string literals
        if matches!(b, b'\'' | b'"' | b'`') {
            let quote = b;
            out.push(quote as char);
            i += 1;
            while i < bytes.len() {
                let c = bytes[i];
                if c == b'\\' && i + 1 < bytes.len() {
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                if c == quote {
                    out.push(quote as char);
                    i += 1;
                    break;
                }
                out.push(if c == b'\n' { '\n' } else { ' ' });
                i += 1;
            }
            continue;
        }
        let ch = code[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn looks_like_es_module(code: &str) -> bool {
    let stripped = strip_js_comments_and_strings(code);
    for line in stripped.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("import")
            && let Some(first) = rest.chars().next()
            && (first.is_whitespace() || matches!(first, '{' | '*' | '\'' | '"'))
        {
            return true;
        }
        if let Some(rest) = trimmed.strip_prefix("export")
            && let Some(first) = rest.chars().next()
            && (first.is_whitespace() || matches!(first, '{' | '*'))
        {
            return true;
        }
    }
    false
}

fn has_return_or_await(code: &str) -> bool {
    let stripped = strip_js_comments_and_strings(code);
    let bytes = stripped.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    let mut i = 0usize;
    while i < bytes.len() {
        if is_ident(bytes[i]) {
            let start = i;
            while i < bytes.len() && is_ident(bytes[i]) {
                i += 1;
            }
            let word = &stripped[start..i];
            if word == "return" || word == "await" {
                let prev_non_ws = stripped[..start].trim_end().as_bytes().last().copied();
                if prev_non_ws != Some(b'.') {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

pub(super) async fn run_user_script(
    env: &Rc<WorkerEnv>,
    script: &UserScript,
    arg: Option<JsonValue>,
    write_depth: u32,
) -> ScriptRunResult {
    let start = Instant::now();
    let script_id = format!("script:{}", script.id);
    let perms = PermissionSet::from_json_str(&script.permissions).unwrap_or_default();

    // Declare `rt` and `ctx` BEFORE `api` and `clear_guard` so drop order ALWAYS clears
    // `Persistent` handles before `ctx` and `rt` drop even if this task is cancelled mid-await.
    let Ok(rt) = AsyncRuntime::new() else {
        return ScriptRunResult {
            ok: false,
            return_value: None,
            logs: Vec::new(),
            error: Some("failed to create script runtime".into()),
            duration_ms: 0,
            data_changed: false,
        };
    };
    rt.set_memory_limit(PLUGIN_MEMORY_LIMIT_BYTES).await;
    rt.set_max_stack_size(PLUGIN_STACK_LIMIT_BYTES).await;
    rt.set_loader(VirtualOnlyResolver, VirtualOnlyLoader::new())
        .await;

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let gate = ExecGate::new(
        env.worker_epoch,
        Arc::clone(&env.shutdown_flag),
        Arc::clone(&cancel_flag),
    );
    let gate_int = Arc::clone(&gate);
    rt.set_interrupt_handler(Some(Box::new(move || gate_int.should_interrupt())))
        .await;

    let Ok(ctx) = AsyncContext::full(&rt).await else {
        return ScriptRunResult {
            ok: false,
            return_value: None,
            logs: Vec::new(),
            error: Some("failed to create script context".into()),
            duration_ms: 0,
            data_changed: false,
        };
    };

    let service_caller = make_service_caller(env);
    let effect_emitter = make_effect_emitter(env);

    let api = Rc::new(PluginApiState::new(PluginApiInit {
        plugin_id: script_id.clone(),
        permissions: perms,
        store: Rc::clone(&env.store),
        vault: env.vault.clone(),
        http_pool: Arc::clone(&env.http_pool),
        oauth_limiter: Arc::clone(&env.oauth_limiter),
        write_depth,
        enforce_declared_views: false,
        declared_views: HashSet::new(),
        settings: Map::new(),
        secret_keys: HashSet::new(),
        locale: Rc::clone(&env.locale),
        theme_appearance: Rc::clone(&env.theme_appearance),
        logs: VecDeque::new(),
        prompt_senders: Rc::clone(&env.prompt_senders),
        next_prompt_id: Rc::clone(&env.next_prompt_id),
        service_registrar: Rc::new(|_| Ok(())),
        service_caller,
        view_refresher: Rc::new(|_| {}),
        effect_emitter,
        shutdown_flag: Arc::clone(&env.shutdown_flag),
    }));

    let clear_guard = ClearPersistentGuard::new(
        Rc::clone(&api),
        Some(Arc::clone(&cancel_flag)),
        Some((Rc::clone(&env.prompt_senders), script_id.clone())),
    );

    let code = script.code.clone();
    let api_c = Rc::clone(&api);
    let res = run_with_gate(
        &ctx,
        &gate,
        GateOptions {
            budget: DEFAULT_COMMAND_TIMEOUT,
            driver_state: None,
            api: Some(Rc::clone(&api)),
            write_depth,
            service_depth: 0,
        },
        async move |ctx| {
            install_fern_bindings(&ctx, Rc::clone(&api_c), true)?;
            let js_arg = match arg {
                Some(a) => json_to_js(&ctx, &a)?,
                None => Value::new_undefined(ctx.clone()),
            };
            let val = if looks_like_es_module(&code) {
                let declared = Module::declare(ctx.clone(), "user_script.js", code)?;
                let (evaluated, promise) = declared.eval()?;
                promise.into_future::<()>().await?;
                let ns = evaluated.namespace()?;
                if let Ok(Some(default_fn)) = ns.get::<_, Option<Function>>("default") {
                    let ret: MaybePromise = default_fn.call((js_arg,))?;
                    ret.into_future::<Value>().await?
                } else {
                    Value::new_undefined(ctx.clone())
                }
            } else if has_return_or_await(&code) {
                ctx.globals().set("__script_arg", js_arg)?;
                let wrapped =
                    format!("(async function(event) {{\n{code}\n}})(globalThis.__script_arg)");
                let mut opts = EvalOptions::default();
                opts.global = true;
                opts.strict = true;
                let promise = ctx.eval_with_options::<MaybePromise, _>(wrapped, opts)?;
                promise.into_future::<Value>().await?
            } else {
                // Always bind `event` even for synchronous expression/statement scripts.
                ctx.globals().set("event", js_arg)?;
                let mut opts = EvalOptions::default();
                opts.global = true;
                opts.strict = true;
                let res: MaybePromise = ctx.eval_with_options(code, opts)?;
                res.into_future::<Value>().await?
            };
            if val.is_undefined() {
                Ok(None)
            } else {
                let json = js_to_json(&ctx, val)
                    .map_err(|e| rquickjs::Exception::throw_message(&ctx, &format!("{e:#}")))?;
                Ok(Some(json))
            }
        },
    )
    .await;

    let effects: Vec<PluginUiEffect> = api.pending_ui_effects.borrow_mut().drain(..).collect();
    let data_changed = api.data_dirty.get();
    let logs: Vec<LogEntry> = api.logs.borrow().iter().cloned().collect();
    drop(clear_guard);
    drop(api);
    drop(ctx);
    drop(rt);

    for eff in effects {
        env.emit(HostUpdate::UiEffect(eff));
    }
    if data_changed {
        env.emit(HostUpdate::DataChanged);
        if !env.config.safe_mode {
            env.outbox_dirty.set(true);
            schedule_outbox_drain(env);
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    match res {
        Ok(return_value) => ScriptRunResult {
            ok: true,
            return_value,
            logs,
            error: None,
            duration_ms,
            data_changed,
        },
        Err(err) => ScriptRunResult {
            ok: false,
            return_value: None,
            logs,
            error: Some(env.vault.scrub_secrets(&script_id, &format!("{err:#}"))),
            duration_ms,
            data_changed,
        },
    }
}
