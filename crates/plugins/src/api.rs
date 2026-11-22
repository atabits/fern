//! QuickJS Host API bindings (`fern.*`, `console.*`, and `UiNode` callback extraction).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use chrono::{Local, NaiveDate, NaiveTime};
use fern_core::Store;
use fern_core::countdown::CountdownKind;
use fern_core::focus::focus_stats;
use fern_core::model::{Id, Kind, ListDraft, Priority, ScheduleBucket, Task, TaskDraft, View};
use fern_core::recurrence::Recurrence;
use futures::channel::oneshot;
use rquickjs::function::{Opt, Rest};
use rquickjs::promise::Promised;
use rquickjs::{Array, Ctx, Exception, Function, Object, Persistent, Type, Value};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue, json};
use uuid::Uuid;

use crate::http::{HttpPool, HttpRequest};
use crate::oauth::{
    DEFAULT_OAUTH_TIMEOUT_SEC, DEFAULT_REDIRECT_HOST, DEFAULT_REDIRECT_PATH, DEFAULT_REDIRECT_PORT,
    OAuthAuthorizeRequest, OAuthRefreshRequest, OAuthTokenAuth, run_oauth_authorize,
    run_oauth_refresh,
};
use crate::permissions::{Permission, PermissionSet};
use crate::ui_ast::{HandlerId, UiNode};
use crate::vault::SecretsVault;

pub const LOG_RING_CAPACITY: usize = 200;
pub const MIN_TIMER_INTERVAL_MS: u64 = 10_000;
pub const MAX_TIMERS_PER_PLUGIN: usize = 8;
const MAX_JSON_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp_ms: i64,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastLevel {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl ToastLevel {
    fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).unwrap_or("info") {
            "success" => Self::Success,
            "warning" | "warn" => Self::Warning,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }
}

/// UI / host side-effect queued by a plugin during JS execution.
#[derive(Clone, Debug, PartialEq)]
pub enum PluginUiEffect {
    Toast {
        plugin_id: String,
        message: String,
        level: ToastLevel,
        created_at_ms: i64,
    },
    Confirm {
        request_id: u64,
        plugin_id: String,
        title: String,
        body: String,
    },
    Prompt {
        request_id: u64,
        plugin_id: String,
        title: String,
        placeholder: String,
        default_value: String,
    },
    OpenModal {
        plugin_id: String,
        view_id: String,
        title: String,
        tree: UiNode,
    },
    CloseModal {
        plugin_id: String,
    },
    OpenTask {
        task_id: Id,
    },
    OpenExternal {
        url: String,
    },
    ClipboardWrite {
        text: String,
    },
}

/// Reply sent from UI back to a suspended `fern.ui.confirm` / `fern.ui.prompt` Promise.
#[derive(Clone, Debug)]
pub enum PromptReplyValue {
    Confirm(bool),
    Prompt(Option<String>),
}

pub type PromptSenders = Rc<RefCell<HashMap<u64, oneshot::Sender<PromptReplyValue>>>>;
pub type ServiceCallerFn =
    Rc<dyn Fn(String, JsonValue) -> oneshot::Receiver<Result<JsonValue>>>;
pub type ServiceRegistrarFn = Rc<dyn Fn(String)>;
pub type ViewRefresherFn = Rc<dyn Fn(String)>;
pub type EffectEmitterFn = Arc<dyn Fn(PluginUiEffect) + Send + Sync>;

pub struct RegisteredView {
    pub render: Persistent<Function<'static>>,
    pub mount: Option<Persistent<Function<'static>>>,
    pub unmount: Option<Persistent<Function<'static>>>,
}

pub struct RegisteredTimer {
    pub id: String,
    pub interval_ms: u64,
    pub next_fire: Instant,
    pub callback: Persistent<Function<'static>>,
}

#[derive(Default)]
pub struct PluginHooks {
    pub before_task_create: Vec<Persistent<Function<'static>>>,
    pub before_task_complete: Vec<Persistent<Function<'static>>>,
    pub quick_add_transform: Vec<Persistent<Function<'static>>>,
    pub on_reminder: Vec<Persistent<Function<'static>>>,
}

pub type EventListenerMap = HashMap<String, Vec<(u64, Persistent<Function<'static>>)>>;

/// Shared per-plugin (or per-script) host bridge accessed by QuickJS bindings on `PluginWorker`.
pub struct PluginApiState {
    pub plugin_id: String,
    pub permissions: PermissionSet,
    pub store: Rc<Store>,
    pub vault: SecretsVault,
    pub http_pool: Arc<HttpPool>,
    pub write_depth: Cell<u32>,
    pub in_batch: Cell<bool>,
    pub data_dirty: Cell<bool>,
    pub declared_views: HashSet<String>,
    pub settings: RefCell<Map<String, JsonValue>>,
    pub secret_keys: HashSet<String>,
    pub locale: Rc<RefCell<String>>,
    pub theme_appearance: Rc<RefCell<String>>,
    pub logs: RefCell<VecDeque<LogEntry>>,
    pub active_render_scope: RefCell<Option<String>>,
    pub handlers: RefCell<HashMap<String, HashMap<u32, Persistent<Function<'static>>>>>,
    pub next_handler_id: Cell<u32>,
    pub views: RefCell<HashMap<String, RegisteredView>>,
    pub commands: RefCell<HashMap<String, Persistent<Function<'static>>>>,
    pub search_providers: RefCell<HashMap<String, Persistent<Function<'static>>>>,
    pub event_listeners: RefCell<EventListenerMap>,
    pub next_listener_id: Cell<u64>,
    pub settings_listeners: RefCell<Vec<Persistent<Function<'static>>>>,
    pub hooks: RefCell<PluginHooks>,
    pub timers: RefCell<HashMap<String, RegisteredTimer>>,
    pub services: RefCell<HashMap<String, Persistent<Function<'static>>>>,
    pub pending_ui_effects: RefCell<VecDeque<PluginUiEffect>>,
    pub prompt_senders: PromptSenders,
    pub next_prompt_id: Rc<Cell<u64>>,
    pub service_registrar: ServiceRegistrarFn,
    pub service_caller: ServiceCallerFn,
    pub view_refresher: ViewRefresherFn,
    pub effect_emitter: EffectEmitterFn,
    pub shutdown_flag: Arc<AtomicBool>,
}

impl PluginApiState {
    pub fn push_log(&self, level: LogLevel, raw_msg: &str) {
        let scrubbed = self.vault.scrub_secrets(&self.plugin_id, raw_msg);
        let mut logs = self.logs.borrow_mut();
        if logs.len() >= LOG_RING_CAPACITY {
            logs.pop_front();
        }
        logs.push_back(LogEntry {
            timestamp_ms: now_ms(),
            level,
            message: scrubbed,
        });
    }

    pub fn scrub(&self, text: &str) -> String {
        self.vault.scrub_secrets(&self.plugin_id, text)
    }

    pub fn clear_scope_handlers(&self, scope: &str) {
        self.handlers.borrow_mut().remove(scope);
    }

    pub fn set_active_scope(&self, scope: Option<String>) {
        *self.active_render_scope.borrow_mut() = scope;
    }

    pub fn register_handler<'js>(
        &self,
        ctx: &Ctx<'js>,
        scope: &str,
        func: Function<'js>,
    ) -> u32 {
        let id = self.next_handler_id.get().max(1);
        self.next_handler_id.set(id.wrapping_add(1).max(1));
        let persistent = Persistent::save(ctx, func);
        self.handlers
            .borrow_mut()
            .entry(scope.to_owned())
            .or_default()
            .insert(id, persistent);
        id
    }

    /// Drops every `Persistent<Function>` handle before dropping the QuickJS `AsyncRuntime`.
    pub fn clear_persistent_handles(&self) {
        self.handlers.borrow_mut().clear();
        self.views.borrow_mut().clear();
        self.commands.borrow_mut().clear();
        self.search_providers.borrow_mut().clear();
        self.event_listeners.borrow_mut().clear();
        self.settings_listeners.borrow_mut().clear();
        {
            let mut hooks = self.hooks.borrow_mut();
            hooks.before_task_create.clear();
            hooks.before_task_complete.clear();
            hooks.quick_add_transform.clear();
            hooks.on_reminder.clear();
        }
        self.timers.borrow_mut().clear();
        self.services.borrow_mut().clear();
    }

    fn require_perm<'js>(&self, ctx: &Ctx<'js>, perm: &Permission) -> rquickjs::Result<()> {
        if self.permissions.has(perm) {
            return Ok(());
        }
        Err(Exception::throw_message(
            ctx,
            &format!("permission denied: requires {}", perm.summary()),
        ))
    }

    fn run_write<T>(
        &self,
        ctx: &Ctx<'_>,
        f: impl FnOnce() -> Result<T>,
    ) -> rquickjs::Result<T> {
        let res = if self.in_batch.get() {
            f()
        } else {
            self.store
                .with_plugin_write_ctx(&self.plugin_id, self.write_depth.get(), f)
        };
        match res {
            Ok(v) => {
                self.data_dirty.set(true);
                Ok(v)
            }
            Err(err) => Err(throw_scrubbed(ctx, self, err)),
        }
    }
}

pub fn install_fern_bindings<'js>(
    ctx: &Ctx<'js>,
    state: Rc<PluginApiState>,
    expose_global_fern: bool,
) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let fern = Object::new(ctx.clone())?;

    install_tasks(ctx, &fern, Rc::clone(&state))?;
    install_lists(ctx, &fern, Rc::clone(&state))?;
    install_tags(ctx, &fern, Rc::clone(&state))?;
    install_habits(ctx, &fern, Rc::clone(&state))?;
    install_focus(ctx, &fern, Rc::clone(&state))?;
    install_countdowns(ctx, &fern, Rc::clone(&state))?;
    install_batch(ctx, &fern, Rc::clone(&state))?;
    install_storage(ctx, &fern, Rc::clone(&state))?;
    install_meta(ctx, &fern, Rc::clone(&state))?;
    install_settings(ctx, &fern, Rc::clone(&state))?;
    install_http(ctx, &fern, Rc::clone(&state))?;
    install_oauth(ctx, &fern, Rc::clone(&state))?;
    install_events(ctx, &fern, Rc::clone(&state))?;
    install_hooks(ctx, &fern, Rc::clone(&state))?;
    install_timers(ctx, &fern, Rc::clone(&state))?;
    install_commands(ctx, &fern, Rc::clone(&state))?;
    install_search(ctx, &fern, Rc::clone(&state))?;
    install_services(ctx, &fern, Rc::clone(&state))?;
    install_views(ctx, &fern, Rc::clone(&state))?;
    install_ui(ctx, &fern, Rc::clone(&state))?;
    install_system(ctx, &fern, Rc::clone(&state))?;

    globals.set("__fern", fern.clone())?;
    if expose_global_fern {
        globals.set("fern", fern)?;
    }

    let reg_state = Rc::clone(&state);
    globals.set(
        "__fern_reg_handler",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, func: Function<'js>| -> rquickjs::Result<u32> {
            let scope = reg_state
                .active_render_scope
                .borrow()
                .clone()
                .unwrap_or_else(|| "__default__".to_owned());
            Ok(reg_state.register_handler(&ctx, &scope, func))
        })?,
    )?;

    install_console(ctx, &globals, state)?;
    Ok(())
}

fn install_console<'js>(
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

fn install_tasks<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    // list(filter?)
    {
        let st = Rc::clone(&state);
        obj.set(
            "list",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, filter: Opt<Value<'js>>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::TasksRead)?;
                let filter_json = match filter.0 {
                    Some(v) if !v.is_undefined() && !v.is_null() => {
                        js_to_json(&ctx, v).map_err(|e| throw_scrubbed(&ctx, &st, e))?
                    }
                    _ => JsonValue::Null,
                };
                let tasks = load_filtered_tasks(&st.store, &filter_json)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let arr = JsonValue::Array(tasks.iter().map(task_to_json).collect());
                json_to_js(&ctx, &arr)
            })?,
        )?;
    }

    // get(id)
    {
        let st = Rc::clone(&state);
        obj.set(
            "get",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::TasksRead)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let task = st.store.task(id).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                match task {
                    Some(t) => json_to_js(&ctx, &task_to_json(&t)),
                    None => Ok(Value::new_null(ctx)),
                }
            })?,
        )?;
    }

    // create(draft)
    {
        let st = Rc::clone(&state);
        obj.set(
            "create",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, input: Value<'js>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::TasksWrite)?;
                let json_in = js_to_json(&ctx, input).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let (draft, top) = parse_task_draft(&json_in).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let created = st.run_write(&ctx, || st.store.create_task(draft, top))?;
                json_to_js(&ctx, &task_to_json(&created))
            })?,
        )?;
    }

    // update(id, patch)
    {
        let st = Rc::clone(&state);
        obj.set(
            "update",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String, patch_val: Value<'js>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::TasksWrite)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let patch = js_to_json(&ctx, patch_val).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let updated = st.run_write(&ctx, || apply_task_patch(&st.store, id, &patch))?;
                json_to_js(&ctx, &task_to_json(&updated))
            })?,
        )?;
    }

    // complete(id)
    {
        let st = Rc::clone(&state);
        obj.set(
            "complete",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::TasksWrite)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let (copy_id, cascaded, task) = st.run_write(&ctx, || {
                    let (copy_id, cascaded) = st.store.complete_with_cascaded(id, true)?;
                    let task = st.store.task(id)?.context("task not found")?;
                    Ok((copy_id, cascaded, task))
                })?;
                let mut val = task_to_json(&task);
                if let Some(obj) = val.as_object_mut() {
                    if let Some(cid) = copy_id {
                        obj.insert("copyId".into(), json!(cid.to_string()));
                    }
                    if !cascaded.is_empty() {
                        obj.insert(
                            "cascadedIds".into(),
                            json!(cascaded.into_iter().map(|i| i.to_string()).collect::<Vec<_>>()),
                        );
                    }
                }
                json_to_js(&ctx, &val)
            })?,
        )?;
    }

    // reopen(id)
    {
        let st = Rc::clone(&state);
        obj.set(
            "reopen",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::TasksWrite)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let task = st.run_write(&ctx, || {
                    st.store.complete(id, false)?;
                    st.store.task(id)?.context("task not found")
                })?;
                json_to_js(&ctx, &task_to_json(&task))
            })?,
        )?;
    }

    // delete(id)
    {
        let st = Rc::clone(&state);
        obj.set(
            "delete",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String| -> rquickjs::Result<bool> {
                st.require_perm(&ctx, &Permission::TasksWrite)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                st.run_write(&ctx, || {
                    st.store.delete_task(id)?;
                    Ok(true)
                })
            })?,
        )?;
    }

    // subtasks(id)
    {
        let st = Rc::clone(&state);
        obj.set(
            "subtasks",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::TasksRead)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let subs = st.store.subtasks(id).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                json_to_js(&ctx, &JsonValue::Array(subs.iter().map(task_to_json).collect()))
            })?,
        )?;
    }

    // addSubtask(parentId, title)
    {
        let st = Rc::clone(&state);
        obj.set(
            "addSubtask",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, parent_str: String, title: String| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::TasksWrite)?;
                let parent_id = parse_uuid(&parent_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let sub = st.run_write(&ctx, || st.store.add_subtask(parent_id, &title))?;
                json_to_js(&ctx, &task_to_json(&sub))
            })?,
        )?;
    }

    // checklist(id)
    {
        let st = Rc::clone(&state);
        obj.set(
            "checklist",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::TasksRead)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let items = st.store.checklist(id).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let arr: Vec<JsonValue> = items
                    .into_iter()
                    .map(|item| {
                        json!({
                            "id": item.id.to_string(),
                            "taskId": item.task_id.to_string(),
                            "title": item.title,
                            "completed": item.completed,
                        })
                    })
                    .collect();
                json_to_js(&ctx, &JsonValue::Array(arr))
            })?,
        )?;
    }

    fern.set("tasks", obj)?;
    Ok(())
}

fn install_lists<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "all",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::ListsRead)?;
                let lists = st.store.lists().map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let arr: Vec<JsonValue> = lists.iter().map(list_to_json).collect();
                json_to_js(&ctx, &JsonValue::Array(arr))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "folders",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::ListsRead)?;
                let folders = st.store.list_folders().map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let arr: Vec<JsonValue> = folders
                    .into_iter()
                    .map(|f| json!({ "id": f.id.to_string(), "name": f.name }))
                    .collect();
                json_to_js(&ctx, &JsonValue::Array(arr))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "sections",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, list_id: Opt<Option<String>>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::ListsRead)?;
                let lid = list_id
                    .0
                    .flatten()
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .map(parse_uuid)
                    .transpose()
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let sections = st
                    .store
                    .list_sections(lid)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let arr: Vec<JsonValue> = sections
                    .into_iter()
                    .map(|s| {
                        json!({
                            "id": s.id.to_string(),
                            "listId": s.list_id.map(|i| i.to_string()),
                            "name": s.name,
                        })
                    })
                    .collect();
                json_to_js(&ctx, &JsonValue::Array(arr))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "create",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, input: Value<'js>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::ListsWrite)?;
                let json_in = js_to_json(&ctx, input).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let draft = parse_list_draft(&json_in).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let created = st.run_write(&ctx, || st.store.add_list(&draft))?;
                json_to_js(&ctx, &list_to_json(&created))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "rename",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String, name: String| -> rquickjs::Result<bool> {
                st.require_perm(&ctx, &Permission::ListsWrite)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                st.run_write(&ctx, || {
                    st.store.rename_list(id, &name)?;
                    Ok(true)
                })
            })?,
        )?;
    }

    fern.set("lists", obj)?;
    Ok(())
}

fn install_tags<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "all",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::TagsRead)?;
                let tags = st.store.tags().map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let arr: Vec<JsonValue> = tags
                    .into_iter()
                    .map(|t| {
                        json!({
                            "name": t.name,
                            "color": t.color,
                            "openCount": t.open_count,
                            "parent": t.parent,
                            "pinned": t.pinned,
                        })
                    })
                    .collect();
                json_to_js(&ctx, &JsonValue::Array(arr))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "setTaskTags",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String, tags: Vec<String>| -> rquickjs::Result<bool> {
                st.require_perm(&ctx, &Permission::TagsWrite)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                st.run_write(&ctx, || {
                    st.store.set_tags(id, &tags)?;
                    Ok(true)
                })
            })?,
        )?;
    }

    fern.set("tags", obj)?;
    Ok(())
}

fn install_habits<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "all",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::HabitsRead)?;
                let habits = st.store.habits().map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let arr: Vec<JsonValue> = habits
                    .into_iter()
                    .map(|h| {
                        json!({
                            "id": h.id.to_string(),
                            "name": h.name,
                            "icon": h.icon,
                            "color": h.color,
                            "startDate": h.start_date.map(|d| d.format("%Y-%m-%d").to_string()),
                            "goalDays": h.goal_days,
                            "archived": h.archived,
                            "goal": h.goal.map(|g| json!({ "amount": g.amount, "unit": g.unit })),
                            "section": h.section.key(),
                        })
                    })
                    .collect();
                json_to_js(&ctx, &JsonValue::Array(arr))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "stats",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::HabitsRead)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let habits = st.store.habits().map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let habit = habits
                    .into_iter()
                    .find(|h| h.id == id)
                    .ok_or_else(|| Exception::throw_message(&ctx, "habit not found"))?;
                let done = st.store.checkins(id).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let week_start = st
                    .store
                    .preferences()
                    .map(|p| p.week_start)
                    .unwrap_or(chrono::Weekday::Mon);
                let today = Local::now().date_naive();
                let stats = habit.stats(&done, today, week_start);
                json_to_js(
                    &ctx,
                    &json!({
                        "totalDays": stats.total_days,
                        "streak": stats.streak,
                        "bestStreak": stats.best_streak,
                    }),
                )
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "checkin",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      id_str: String,
                      date_str: String,
                      status_val: Value<'js>,
                      amount: Opt<Option<f64>>|
                      -> rquickjs::Result<bool> {
                    st.require_perm(&ctx, &Permission::HabitsWrite)?;
                    let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let date = parse_date(&date_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let status_json = js_to_json(&ctx, status_val)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    st.run_write(&ctx, || {
                        if let Some(val) = amount.0.flatten() {
                            st.store.set_checkin_amount(id, date, val)?;
                            return Ok(true);
                        }
                        match status_json {
                            JsonValue::Bool(true) => {
                                if !st.store.checkins(id)?.contains(&date) {
                                    st.store.toggle_checkin(id, date)?;
                                }
                            }
                            JsonValue::Bool(false) => {
                                st.store.reset_checkin(id, date, true)?;
                            }
                            JsonValue::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                                "done" | "checked" => {
                                    if !st.store.checkins(id)?.contains(&date) {
                                        st.store.toggle_checkin(id, date)?;
                                    }
                                }
                                "undone" | "failed" => {
                                    st.store.mark_checkin_undone(id, date)?;
                                }
                                "skip" | "skipped" => {
                                    st.store.skip_habit_date(id, date)?;
                                }
                                "reset" | "clear" => {
                                    st.store.reset_checkin(id, date, true)?;
                                }
                                "toggle" => {
                                    st.store.toggle_checkin(id, date)?;
                                }
                                other => bail!("unknown habit checkin status {other:?}"),
                            },
                            _ => bail!("invalid habit checkin status"),
                        }
                        Ok(true)
                    })
                },
            )?,
        )?;
    }

    fern.set("habits", obj)?;
    Ok(())
}

fn install_focus<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "sessions",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, range: Opt<Value<'js>>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::FocusRead)?;
                let range_json = match range.0 {
                    Some(v) if !v.is_undefined() && !v.is_null() => {
                        js_to_json(&ctx, v).map_err(|e| throw_scrubbed(&ctx, &st, e))?
                    }
                    _ => JsonValue::Null,
                };
                let all = st
                    .store
                    .focus_sessions()
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let filtered = filter_focus_sessions(&all, &range_json)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let arr: Vec<JsonValue> = filtered
                    .into_iter()
                    .map(|s| {
                        json!({
                            "id": s.id.to_string(),
                            "taskId": s.task_id.map(|i| i.to_string()),
                            "habitId": s.habit_id.map(|i| i.to_string()),
                            "title": s.title,
                            "startedAt": s.started_at,
                            "endedAt": s.ended_at,
                            "minutes": s.minutes(),
                            "stopwatch": s.stopwatch,
                            "note": s.note,
                        })
                    })
                    .collect();
                json_to_js(&ctx, &JsonValue::Array(arr))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "stats",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, range: Opt<Value<'js>>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::FocusRead)?;
                let range_json = match range.0 {
                    Some(v) if !v.is_undefined() && !v.is_null() => {
                        js_to_json(&ctx, v).map_err(|e| throw_scrubbed(&ctx, &st, e))?
                    }
                    _ => JsonValue::Null,
                };
                let all = st
                    .store
                    .focus_sessions()
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let filtered = filter_focus_sessions(&all, &range_json)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let today = Local::now().date_naive();
                let stats = focus_stats(&filtered, today);
                json_to_js(
                    &ctx,
                    &json!({
                        "todayCount": stats.today_count,
                        "todayMinutes": stats.today_minutes,
                        "totalCount": stats.total_count,
                        "totalMinutes": stats.total_minutes,
                    }),
                )
            })?,
        )?;
    }

    fern.set("focus", obj)?;
    Ok(())
}

fn install_countdowns<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "all",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::CountdownsRead)?;
                let items = st
                    .store
                    .countdowns()
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let today = Local::now().date_naive();
                let arr: Vec<JsonValue> = items
                    .into_iter()
                    .map(|c| {
                        let span = c.span(today);
                        json!({
                            "id": c.id.to_string(),
                            "name": c.name,
                            "date": c.date.format("%Y-%m-%d").to_string(),
                            "kind": c.kind.as_str(),
                            "icon": c.icon,
                            "color": c.color,
                            "archived": c.archived,
                            "pinnedAt": c.pinned_at,
                            "totalDays": span.total_days,
                            "past": span.past,
                        })
                    })
                    .collect();
                json_to_js(&ctx, &JsonValue::Array(arr))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "create",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, input: Value<'js>| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::CountdownsWrite)?;
                let j = js_to_json(&ctx, input).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let name = j
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| Exception::throw_message(&ctx, "countdown name is required"))?
                    .to_owned();
                let date_str = j
                    .get("date")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| Exception::throw_message(&ctx, "countdown date is required"))?;
                let date = parse_date(date_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let kind = j
                    .get("kind")
                    .and_then(JsonValue::as_str)
                    .map(CountdownKind::parse)
                    .unwrap_or(CountdownKind::Countdown);
                let created = st.run_write(&ctx, || st.store.add_countdown(&name, date, kind))?;
                json_to_js(
                    &ctx,
                    &json!({
                        "id": created.id.to_string(),
                        "name": created.name,
                        "date": created.date.format("%Y-%m-%d").to_string(),
                        "kind": created.kind.as_str(),
                    }),
                )
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "delete",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id_str: String| -> rquickjs::Result<bool> {
                st.require_perm(&ctx, &Permission::CountdownsWrite)?;
                let id = parse_uuid(&id_str).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                st.run_write(&ctx, || {
                    st.store.delete_countdown(id)?;
                    Ok(true)
                })
            })?,
        )?;
    }

    fern.set("countdowns", obj)?;
    Ok(())
}

fn install_batch<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let st = Rc::clone(&state);
    let batch_fn = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, func: Function<'js>| -> rquickjs::Result<Value<'js>> {
            if st.in_batch.get() {
                return func.call(());
            }
            let mut result_slot: Option<Value<'js>> = None;
            let mut js_err_slot: Option<rquickjs::Error> = None;

            let tx_res = st
                .store
                .with_plugin_write_ctx(&st.plugin_id, st.write_depth.get(), || {
                    st.in_batch.set(true);
                    let call_res = func.call::<_, Value<'js>>(());
                    st.in_batch.set(false);
                    match call_res {
                        Ok(val) => {
                            if val.is_promise() {
                                anyhow::bail!("fern.batch(fn) callback must be synchronous");
                            }
                            result_slot = Some(val);
                            Ok(())
                        }
                        Err(err) => {
                            js_err_slot = Some(err);
                            anyhow::bail!("batch callback aborted")
                        }
                    }
                });

            if let Some(js_err) = js_err_slot {
                return Err(js_err);
            }
            match tx_res {
                Ok(()) => {
                    st.data_dirty.set(true);
                    Ok(result_slot.unwrap_or_else(|| Value::new_undefined(ctx)))
                }
                Err(err) => Err(throw_scrubbed(&ctx, &st, err)),
            }
        },
    )?;
    fern.set("batch", batch_fn)?;
    Ok(())
}

fn install_storage<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "get",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, key: String| -> rquickjs::Result<Value<'js>> {
                st.require_perm(&ctx, &Permission::Storage)?;
                let raw = st
                    .store
                    .plugin_kv_get(&st.plugin_id, &key)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                match raw {
                    Some(text) => {
                        let json_val: JsonValue = serde_json::from_str(&text)
                            .unwrap_or(JsonValue::String(text));
                        json_to_js(&ctx, &json_val)
                    }
                    None => Ok(Value::new_null(ctx)),
                }
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "set",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, key: String, val: Value<'js>| -> rquickjs::Result<bool> {
                st.require_perm(&ctx, &Permission::Storage)?;
                let json_val = js_to_json(&ctx, val).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                let encoded = serde_json::to_string(&json_val)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e.into()))?;
                st.store
                    .plugin_kv_set(&st.plugin_id, &key, &encoded)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                Ok(true)
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "delete",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, key: String| -> rquickjs::Result<bool> {
                st.require_perm(&ctx, &Permission::Storage)?;
                st.store
                    .plugin_kv_delete(&st.plugin_id, &key)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "keys",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, prefix: Opt<Option<String>>| -> rquickjs::Result<Vec<String>> {
                st.require_perm(&ctx, &Permission::Storage)?;
                let prefix_owned = prefix.0.flatten();
                st.store
                    .plugin_kv_keys(&st.plugin_id, prefix_owned.as_deref())
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))
            })?,
        )?;
    }

    fern.set("storage", obj)?;
    Ok(())
}

fn validate_meta_entity_type(entity_type: &str) -> Result<&str> {
    let norm = entity_type.trim();
    ensure!(
        matches!(norm, "task" | "list" | "habit" | "countdown" | "day"),
        "invalid meta entity_type {norm:?} (expected task|list|habit|countdown|day)"
    );
    Ok(norm)
}

fn install_meta<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "get",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, entity_type: String, entity_id: String, key: String| -> rquickjs::Result<Value<'js>> {
                    st.require_perm(&ctx, &Permission::Meta)?;
                    let et = validate_meta_entity_type(&entity_type)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let raw = st
                        .store
                        .plugin_meta_get(&st.plugin_id, et, &entity_id, &key)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    match raw {
                        Some(text) => {
                            let val: JsonValue = serde_json::from_str(&text)
                                .unwrap_or(JsonValue::String(text));
                            json_to_js(&ctx, &val)
                        }
                        None => Ok(Value::new_null(ctx)),
                    }
                },
            )?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "set",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, entity_type: String, entity_id: String, key: String, val: Value<'js>| -> rquickjs::Result<bool> {
                    st.require_perm(&ctx, &Permission::Meta)?;
                    let et = validate_meta_entity_type(&entity_type)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let json_val = js_to_json(&ctx, val).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let encoded = serde_json::to_string(&json_val)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e.into()))?;
                    st.store
                        .plugin_meta_set(&st.plugin_id, et, &entity_id, &key, &encoded)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    Ok(true)
                },
            )?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "list",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, entity_type: String, entity_id: String| -> rquickjs::Result<Value<'js>> {
                    st.require_perm(&ctx, &Permission::Meta)?;
                    let et = validate_meta_entity_type(&entity_type)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let pairs = st
                        .store
                        .plugin_meta_list(&st.plugin_id, et, &entity_id)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let mut map = Map::new();
                    for (k, v) in pairs {
                        let parsed = serde_json::from_str(&v).unwrap_or(JsonValue::String(v));
                        map.insert(k, parsed);
                    }
                    json_to_js(&ctx, &JsonValue::Object(map))
                },
            )?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "delete",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, entity_type: String, entity_id: String, key: String| -> rquickjs::Result<bool> {
                    st.require_perm(&ctx, &Permission::Meta)?;
                    let et = validate_meta_entity_type(&entity_type)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    st.store
                        .plugin_meta_delete(&st.plugin_id, et, &entity_id, &key)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))
                },
            )?,
        )?;
    }

    fern.set("meta", obj)?;
    Ok(())
}

fn install_settings<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    {
        let st = Rc::clone(&state);
        obj.set(
            "get",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, key: String| -> rquickjs::Result<Value<'js>> {
                if st.secret_keys.contains(&key) {
                    return Err(Exception::throw_message(
                        &ctx,
                        "secret settings cannot be read directly from JS; use ${secret:key} in fern.http headers",
                    ));
                }
                let settings = st.settings.borrow();
                match settings.get(&key) {
                    Some(v) => json_to_js(&ctx, v),
                    None => Ok(Value::new_null(ctx)),
                }
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "all",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                let settings = st.settings.borrow();
                json_to_js(&ctx, &JsonValue::Object(settings.clone()))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "hasSecret",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, key: String| -> rquickjs::Result<bool> {
                st.vault
                    .has(&st.plugin_id, &key)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))
            })?,
        )?;
    }

    {
        let st = Rc::clone(&state);
        obj.set(
            "onChange",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, cb: Function<'js>| {
                st.settings_listeners
                    .borrow_mut()
                    .push(Persistent::save(&ctx, cb));
            })?,
        )?;
    }

    fern.set("settings", obj)?;
    Ok(())
}

fn install_http<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;
    let st = Rc::clone(&state);

    obj.set(
        "fetch",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, url: String, opts: Opt<Value<'js>>| -> rquickjs::Result<Value<'js>> {
                let uri: ureq::http::Uri = url
                    .trim()
                    .parse()
                    .map_err(|e| Exception::throw_message(&ctx, &format!("invalid URL: {e}")))?;
                crate::http::validate_target_uri(&uri, &st.permissions)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;

                let opts_json = match opts.0 {
                    Some(v) if !v.is_undefined() && !v.is_null() => {
                        js_to_json(&ctx, v).map_err(|e| throw_scrubbed(&ctx, &st, e))?
                    }
                    _ => JsonValue::Null,
                };
                let method = opts_json
                    .get("method")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("GET")
                    .to_owned();
                let timeout_ms = opts_json.get("timeoutMs").and_then(JsonValue::as_u64);
                let mut headers = Vec::new();
                if let Some(hdrs) = opts_json.get("headers").and_then(JsonValue::as_object) {
                    for (k, v) in hdrs {
                        if let Some(vs) = v.as_str() {
                            headers.push((k.clone(), vs.to_owned()));
                        }
                    }
                }
                let body = match opts_json.get("body") {
                    Some(JsonValue::String(s)) => Some(s.as_bytes().to_vec()),
                    Some(JsonValue::Null) | None => None,
                    Some(other) => Some(serde_json::to_vec(other).unwrap_or_default()),
                };

                let req = HttpRequest {
                    method,
                    url,
                    headers,
                    body,
                    timeout_ms,
                };
                let rx = st.http_pool.fetch(
                    st.plugin_id.clone(),
                    st.permissions.clone(),
                    st.vault.clone(),
                    req,
                );
                let st_for_fut = Rc::clone(&st);
                let ctx_for_fut = ctx.clone();

                let promised = Promised::from(async move {
                    let resp = match rx.await {
                        Ok(Ok(r)) => r,
                        Ok(Err(err)) => return Err(throw_scrubbed(&ctx_for_fut, &st_for_fut, err)),
                        Err(_) => {
                            return Err(Exception::throw_message(
                                &ctx_for_fut,
                                "HTTP request cancelled",
                            ));
                        }
                    };
                    let body_text = resp.body_text();
                    let mut hdr_map = Map::new();
                    for (k, v) in &resp.headers {
                        hdr_map.insert(k.to_ascii_lowercase(), JsonValue::String(v.clone()));
                    }
                    let obj = Object::new(ctx_for_fut.clone())?;
                    obj.set("ok", resp.ok())?;
                    obj.set("status", i32::from(resp.status))?;
                    obj.set("url", resp.url)?;
                    obj.set("headers", json_to_js(&ctx_for_fut, &JsonValue::Object(hdr_map))?)?;
                    obj.set("body", body_text.clone())?;
                    let body_for_text = body_text.clone();
                    obj.set(
                        "text",
                        Function::new(ctx_for_fut.clone(), move || body_for_text.clone())?,
                    )?;
                    obj.set(
                        "json",
                        Function::new(ctx_for_fut.clone(), move |ctx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                            let parsed: JsonValue = serde_json::from_str(&body_text).map_err(|err| {
                                Exception::throw_message(&ctx, &format!("invalid JSON body: {err}"))
                            })?;
                            json_to_js(&ctx, &parsed)
                        })?,
                    )?;
                    Ok(obj.into_value())
                });

                rquickjs::IntoJs::into_js(promised, &ctx)
            },
        )?,
    )?;

    fern.set("http", obj)?;
    Ok(())
}

fn parse_oauth_scopes(val: Option<&JsonValue>) -> Vec<String> {
    match val {
        Some(JsonValue::Array(arr)) => arr
            .iter()
            .filter_map(JsonValue::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(JsonValue::String(s)) => s
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn install_oauth<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    // authorize(opts) -> Promise<{ ok, tokenType, scope, expiresIn, hasRefreshToken }>
    {
        let st = Rc::clone(&state);
        obj.set(
            "authorize",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, opts_val: Value<'js>| -> rquickjs::Result<Value<'js>> {
                    let opts = js_to_json(&ctx, opts_val)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let authorize_url = opts
                        .get("authorizeUrl")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let token_url = opts
                        .get("tokenUrl")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let client_id = opts
                        .get("clientId")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let client_secret_key = opts
                        .get("clientSecretKey")
                        .and_then(JsonValue::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let scopes = parse_oauth_scopes(opts.get("scopes").or_else(|| opts.get("scope")));
                    let redirect_host = opts
                        .get("redirectHost")
                        .and_then(JsonValue::as_str)
                        .unwrap_or(DEFAULT_REDIRECT_HOST)
                        .to_owned();
                    let redirect_port = opts
                        .get("redirectPort")
                        .and_then(JsonValue::as_u64)
                        .and_then(|p| u16::try_from(p).ok())
                        .unwrap_or(DEFAULT_REDIRECT_PORT);
                    let redirect_path = opts
                        .get("redirectPath")
                        .and_then(JsonValue::as_str)
                        .unwrap_or(DEFAULT_REDIRECT_PATH)
                        .to_owned();
                    let token_auth = OAuthTokenAuth::parse(
                        opts.get("tokenAuth").and_then(JsonValue::as_str),
                    )
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let save_access_token_as = opts
                        .get("saveAccessTokenAs")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("access_token")
                        .to_owned();
                    let save_refresh_token_as = match opts.get("saveRefreshTokenAs") {
                        Some(JsonValue::Null) => None,
                        Some(JsonValue::String(s)) if s.trim().is_empty() => None,
                        Some(JsonValue::String(s)) => Some(s.trim().to_owned()),
                        _ => Some("refresh_token".to_owned()),
                    };
                    let mut extra_authorize_params = Vec::new();
                    if let Some(extra) = opts
                        .get("extraAuthorizeParams")
                        .and_then(JsonValue::as_object)
                    {
                        for (k, v) in extra {
                            if let Some(vs) = v.as_str() {
                                extra_authorize_params.push((k.clone(), vs.to_owned()));
                            }
                        }
                    }
                    let timeout_sec = opts
                        .get("timeoutSec")
                        .and_then(JsonValue::as_u64)
                        .unwrap_or(DEFAULT_OAUTH_TIMEOUT_SEC);

                    let req = OAuthAuthorizeRequest {
                        authorize_url,
                        token_url,
                        client_id,
                        client_secret_key,
                        scopes,
                        redirect_host,
                        redirect_port,
                        redirect_path,
                        token_auth,
                        save_access_token_as,
                        save_refresh_token_as,
                        extra_authorize_params,
                        timeout_sec,
                    };

                    let (tx, rx) = oneshot::channel();
                    let plugin_id = st.plugin_id.clone();
                    let permissions = st.permissions.clone();
                    let vault = st.vault.clone();
                    let cancel = Arc::clone(&st.shutdown_flag);
                    let emitter = Arc::clone(&st.effect_emitter);

                    if let Err(err) = thread::Builder::new()
                        .name("fern-oauth".to_owned())
                        .spawn(move || {
                            let res = run_oauth_authorize(
                                &plugin_id,
                                &permissions,
                                &vault,
                                &req,
                                &cancel,
                                |url| {
                                    emitter(PluginUiEffect::OpenExternal { url });
                                },
                            );
                            let _ = tx.send(res);
                        })
                    {
                        return Err(Exception::throw_message(
                            &ctx,
                            &format!("failed to start OAuth worker: {err}"),
                        ));
                    }

                    let st_for_fut = Rc::clone(&st);
                    let ctx_for_fut = ctx.clone();
                    let promised = Promised::from(async move {
                        let summary = match rx.await {
                            Ok(Ok(s)) => s,
                            Ok(Err(err)) => return Err(throw_scrubbed(&ctx_for_fut, &st_for_fut, err)),
                            Err(_) => {
                                return Err(Exception::throw_message(
                                    &ctx_for_fut,
                                    "OAuth authorization cancelled",
                                ));
                            }
                        };
                        let val = json!({
                            "ok": summary.ok,
                            "tokenType": summary.token_type,
                            "scope": summary.scope,
                            "expiresIn": summary.expires_in,
                            "hasRefreshToken": summary.has_refresh_token,
                        });
                        json_to_js(&ctx_for_fut, &val)
                    });
                    rquickjs::IntoJs::into_js(promised, &ctx)
                },
            )?,
        )?;
    }

    // refresh(opts) -> Promise<{ ok, tokenType, scope, expiresIn, hasRefreshToken }>
    {
        let st = Rc::clone(&state);
        obj.set(
            "refresh",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, opts_val: Value<'js>| -> rquickjs::Result<Value<'js>> {
                    let opts = js_to_json(&ctx, opts_val)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let token_url = opts
                        .get("tokenUrl")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let client_id = opts
                        .get("clientId")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let client_secret_key = opts
                        .get("clientSecretKey")
                        .and_then(JsonValue::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let refresh_token_key = opts
                        .get("refreshTokenKey")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("refresh_token")
                        .to_owned();
                    let scopes = parse_oauth_scopes(opts.get("scopes").or_else(|| opts.get("scope")));
                    let token_auth = OAuthTokenAuth::parse(
                        opts.get("tokenAuth").and_then(JsonValue::as_str),
                    )
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let save_access_token_as = opts
                        .get("saveAccessTokenAs")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("access_token")
                        .to_owned();
                    let save_refresh_token_as = match opts.get("saveRefreshTokenAs") {
                        Some(JsonValue::Null) => None,
                        Some(JsonValue::String(s)) if s.trim().is_empty() => None,
                        Some(JsonValue::String(s)) => Some(s.trim().to_owned()),
                        _ => Some("refresh_token".to_owned()),
                    };

                    let req = OAuthRefreshRequest {
                        token_url,
                        client_id,
                        client_secret_key,
                        refresh_token_key,
                        scopes,
                        token_auth,
                        save_access_token_as,
                        save_refresh_token_as,
                    };

                    let (tx, rx) = oneshot::channel();
                    let plugin_id = st.plugin_id.clone();
                    let permissions = st.permissions.clone();
                    let vault = st.vault.clone();

                    if let Err(err) = thread::Builder::new()
                        .name("fern-oauth-ref".to_owned())
                        .spawn(move || {
                            let res = run_oauth_refresh(&plugin_id, &permissions, &vault, &req);
                            let _ = tx.send(res);
                        })
                    {
                        return Err(Exception::throw_message(
                            &ctx,
                            &format!("failed to start OAuth refresh worker: {err}"),
                        ));
                    }

                    let st_for_fut = Rc::clone(&st);
                    let ctx_for_fut = ctx.clone();
                    let promised = Promised::from(async move {
                        let summary = match rx.await {
                            Ok(Ok(s)) => s,
                            Ok(Err(err)) => return Err(throw_scrubbed(&ctx_for_fut, &st_for_fut, err)),
                            Err(_) => {
                                return Err(Exception::throw_message(
                                    &ctx_for_fut,
                                    "OAuth refresh cancelled",
                                ));
                            }
                        };
                        let val = json!({
                            "ok": summary.ok,
                            "tokenType": summary.token_type,
                            "scope": summary.scope,
                            "expiresIn": summary.expires_in,
                            "hasRefreshToken": summary.has_refresh_token,
                        });
                        json_to_js(&ctx_for_fut, &val)
                    });
                    rquickjs::IntoJs::into_js(promised, &ctx)
                },
            )?,
        )?;
    }

    // clearTokens(keys?) -> number (deletes stored OAuth tokens from SecretsVault without reading them)
    {
        let st = Rc::clone(&state);
        obj.set(
            "clearTokens",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, keys_opt: Opt<Value<'js>>| -> rquickjs::Result<i32> {
                    let keys: Vec<String> = match keys_opt.0 {
                        Some(v) if !v.is_undefined() && !v.is_null() => {
                            let j = js_to_json(&ctx, v).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                            match j {
                                JsonValue::Array(arr) => arr
                                    .into_iter()
                                    .filter_map(|item| item.as_str().map(str::to_owned))
                                    .collect(),
                                JsonValue::String(s) => vec![s],
                                _ => vec!["access_token".to_owned(), "refresh_token".to_owned()],
                            }
                        }
                        _ => vec!["access_token".to_owned(), "refresh_token".to_owned()],
                    };
                    let mut removed = 0i32;
                    for key in keys {
                        if !key.trim().is_empty()
                            && st
                                .vault
                                .delete(&st.plugin_id, key.trim())
                                .map_err(|e| throw_scrubbed(&ctx, &st, e))?
                        {
                            removed += 1;
                        }
                    }
                    Ok(removed)
                },
            )?,
        )?;
    }

    fern.set("oauth", obj)?;
    Ok(())
}

fn event_permission(kind: &str) -> Option<Permission> {
    if kind.starts_with("task.") || kind == "task" {
        Some(Permission::TasksRead)
    } else if kind.starts_with("habit.") || kind == "habit" {
        Some(Permission::HabitsRead)
    } else if kind.starts_with("focus.") || kind == "focus" {
        Some(Permission::FocusRead)
    } else {
        None
    }
}

fn install_events<'js>(
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

fn install_hooks<'js>(
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

fn install_timers<'js>(
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
                move |ctx: Ctx<'js>, id: String, ms: u64, cb: Function<'js>| -> rquickjs::Result<()> {
                    let id = id.trim().to_owned();
                    if id.is_empty() {
                        return Err(Exception::throw_message(&ctx, "timer id must not be empty"));
                    }
                    if ms < MIN_TIMER_INTERVAL_MS {
                        return Err(Exception::throw_message(
                            &ctx,
                            &format!("timer interval must be >= {MIN_TIMER_INTERVAL_MS} ms"),
                        ));
                    }
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
                        next_fire: Instant::now() + Duration::from_millis(ms),
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

fn install_commands<'js>(
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

fn install_search<'js>(
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

fn install_services<'js>(
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
                    let rx = (st.service_caller)(name, arg_json);
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

fn install_views<'js>(
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
                        &format!("view {id:?} is not declared in manifest.contributions.sidebar"),
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

fn install_ui<'js>(
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
                    st.prompt_senders.borrow_mut().insert(req_id, tx);
                    st.pending_ui_effects
                        .borrow_mut()
                        .push_back(PluginUiEffect::Confirm {
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
                    st.prompt_senders.borrow_mut().insert(req_id, tx);
                    st.pending_ui_effects
                        .borrow_mut()
                        .push_back(PluginUiEffect::Prompt {
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
                    st.clear_scope_handlers(modal_scope);
                    let prev_scope = st.active_render_scope.borrow().clone();
                    st.set_active_scope(Some(modal_scope.to_owned()));

                    let node_val = if let Some(func) = view_or_node.as_function() {
                        func.call::<_, Value<'js>>(())?
                    } else if let Some(obj) = view_or_node.as_object()
                        && let Ok(render_fn) = obj.get::<_, Function>("render")
                    {
                        render_fn.call::<_, Value<'js>>(())?
                    } else {
                        view_or_node
                    };

                    let tree = js_value_to_ui_node(&ctx, node_val, &st, modal_scope);
                    st.set_active_scope(prev_scope);
                    let tree = tree.map_err(|e| throw_scrubbed(&ctx, &st, e))?;

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
            Function::new(ctx.clone(), move || {
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

    fern.set("ui", obj)?;
    Ok(())
}

fn install_system<'js>(
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

/// Converts a JS `UiNode` value (including inline closures on `onClick`/`onChange`/`onSubmit`/`onAction`)
/// into a validated, SVG-sanitized [`UiNode`].
pub fn js_value_to_ui_node<'js>(
    ctx: &Ctx<'js>,
    val: Value<'js>,
    state: &PluginApiState,
    scope: &str,
) -> Result<UiNode> {
    let json_val = js_ui_to_json(ctx, val, state, scope, 0)?;
    let mut node: UiNode = serde_json::from_value(json_val)
        .context("view render() returned invalid UiNode tree")?;
    node.sanitize_tree()?;
    Ok(node)
}

fn js_ui_to_json<'js>(
    ctx: &Ctx<'js>,
    val: Value<'js>,
    state: &PluginApiState,
    scope: &str,
    depth: usize,
) -> Result<JsonValue> {
    ensure!(depth < MAX_JSON_DEPTH, "UI tree nesting exceeds {MAX_JSON_DEPTH}");
    if let Some(func) = val.as_function() {
        let id = state.register_handler(ctx, scope, func.clone());
        return Ok(JsonValue::from(HandlerId(id).0));
    }
    if val.is_null() || val.is_undefined() {
        return Ok(JsonValue::Null);
    }
    if let Some(b) = val.as_bool() {
        return Ok(JsonValue::Bool(b));
    }
    if let Some(i) = val.as_int() {
        return Ok(JsonValue::from(i));
    }
    if let Some(f) = val.as_float() {
        return Ok(serde_json::Number::from_f64(f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null));
    }
    if let Some(s) = val.as_string() {
        return Ok(JsonValue::String(s.to_string()?));
    }
    if val.is_array() {
        let arr = Array::from_value(val)?;
        let mut out = Vec::with_capacity(arr.len());
        for item in arr.iter::<Value<'js>>() {
            out.push(js_ui_to_json(ctx, item?, state, scope, depth + 1)?);
        }
        return Ok(JsonValue::Array(out));
    }
    if let Some(obj) = val.as_object() {
        let mut map = Map::new();
        for prop in obj.props::<String, Value<'js>>() {
            let (k, v) = prop?;
            if v.is_undefined() {
                continue;
            }
            if matches!(
                k.as_str(),
                "onClick"
                    | "on_click"
                    | "onChange"
                    | "on_change"
                    | "onSubmit"
                    | "on_submit"
                    | "onAction"
                    | "on_action"
            ) && let Some(func) = v.as_function()
            {
                let id = state.register_handler(ctx, scope, func.clone());
                map.insert(k, JsonValue::from(id));
                continue;
            }
            if v.is_function() {
                continue;
            }
            map.insert(k, js_ui_to_json(ctx, v, state, scope, depth + 1)?);
        }
        return Ok(JsonValue::Object(map));
    }
    Ok(JsonValue::Null)
}

pub fn js_to_json<'js>(ctx: &Ctx<'js>, val: Value<'js>) -> Result<JsonValue> {
    js_to_json_inner(ctx, val, 0)
}

fn js_to_json_inner<'js>(ctx: &Ctx<'js>, val: Value<'js>, depth: usize) -> Result<JsonValue> {
    ensure!(depth < MAX_JSON_DEPTH, "JSON nesting exceeds {MAX_JSON_DEPTH}");
    if val.is_null() || val.is_undefined() {
        return Ok(JsonValue::Null);
    }
    if let Some(b) = val.as_bool() {
        return Ok(JsonValue::Bool(b));
    }
    if let Some(i) = val.as_int() {
        return Ok(JsonValue::from(i));
    }
    if let Some(f) = val.as_float() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Ok(JsonValue::Number(n));
        }
        return Ok(JsonValue::Null);
    }
    if let Some(s) = val.as_string() {
        return Ok(JsonValue::String(s.to_string()?));
    }
    if val.is_array() {
        let arr = Array::from_value(val)?;
        let mut out = Vec::with_capacity(arr.len());
        for item in arr.iter::<Value<'js>>() {
            let item = item?;
            if item.is_undefined() || item.is_function() {
                out.push(JsonValue::Null);
            } else {
                out.push(js_to_json_inner(ctx, item, depth + 1)?);
            }
        }
        return Ok(JsonValue::Array(out));
    }
    if let Some(obj) = val.as_object() {
        if obj.is_function() {
            return Ok(JsonValue::Null);
        }
        let mut map = Map::new();
        for prop in obj.props::<String, Value<'js>>() {
            let (k, v) = prop?;
            if v.is_undefined() || v.is_function() {
                continue;
            }
            map.insert(k, js_to_json_inner(ctx, v, depth + 1)?);
        }
        return Ok(JsonValue::Object(map));
    }
    let _ = ctx;
    Ok(JsonValue::Null)
}

pub fn json_to_js<'js>(ctx: &Ctx<'js>, val: &JsonValue) -> rquickjs::Result<Value<'js>> {
    match val {
        JsonValue::Null => Ok(Value::new_null(ctx.clone())),
        JsonValue::Bool(b) => rquickjs::IntoJs::into_js(*b, ctx),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                if let Ok(i32v) = i32::try_from(i) {
                    return rquickjs::IntoJs::into_js(i32v, ctx);
                }
                return rquickjs::IntoJs::into_js(i as f64, ctx);
            }
            if let Some(u) = n.as_u64() {
                return rquickjs::IntoJs::into_js(u as f64, ctx);
            }
            rquickjs::IntoJs::into_js(n.as_f64().unwrap_or(0.0), ctx)
        }
        JsonValue::String(s) => rquickjs::IntoJs::into_js(s.as_str(), ctx),
        JsonValue::Array(items) => {
            let arr = Array::new(ctx.clone())?;
            for (idx, item) in items.iter().enumerate() {
                arr.set(idx, json_to_js(ctx, item)?)?;
            }
            Ok(arr.into_value())
        }
        JsonValue::Object(map) => {
            let obj = Object::new(ctx.clone())?;
            for (k, v) in map {
                obj.set(k.as_str(), json_to_js(ctx, v)?)?;
            }
            Ok(obj.into_value())
        }
    }
}

fn format_js_value<'js>(ctx: &Ctx<'js>, val: Value<'js>) -> String {
    if val.is_undefined() {
        return "undefined".to_owned();
    }
    if val.is_null() {
        return "null".to_owned();
    }
    if let Some(s) = val.as_string() {
        return s.to_string().unwrap_or_default();
    }
    if let Some(b) = val.as_bool() {
        return b.to_string();
    }
    if let Some(i) = val.as_int() {
        return i.to_string();
    }
    if let Some(f) = val.as_float() {
        return f.to_string();
    }
    if matches!(val.type_of(), Type::Object | Type::Array)
        && let Ok(Some(json_str)) = ctx.json_stringify(val.clone())
        && let Ok(s) = json_str.to_string()
    {
        return s;
    }
    format!("{val:?}")
}

fn throw_scrubbed(ctx: &Ctx<'_>, state: &PluginApiState, err: anyhow::Error) -> rquickjs::Error {
    let scrubbed = state.scrub(&format!("{err:#}"));
    Exception::throw_message(ctx, &scrubbed)
}

fn parse_uuid(raw: &str) -> Result<Id> {
    Uuid::parse_str(raw.trim()).with_context(|| format!("invalid UUID {raw:?}"))
}

fn parse_date(raw: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
        .with_context(|| format!("invalid date {raw:?} (expected YYYY-MM-DD)"))
}

fn parse_time(raw: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(raw.trim(), "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(raw.trim(), "%H:%M:%S"))
        .with_context(|| format!("invalid time {raw:?} (expected HH:MM)"))
}

fn parse_priority(v: &JsonValue) -> Result<Priority> {
    match v {
        JsonValue::Number(n) => Ok(Priority::from_i64(n.as_i64().unwrap_or(0))),
        JsonValue::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "none" | "" => Ok(Priority::None),
            "low" => Ok(Priority::Low),
            "medium" | "med" => Ok(Priority::Medium),
            "high" => Ok(Priority::High),
            other => bail!("invalid priority {other:?}"),
        },
        _ => bail!("invalid priority value"),
    }
}

fn parse_schedule_bucket(raw: &str) -> Result<ScheduleBucket> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "none" => Ok(ScheduleBucket::None),
        "evening" => Ok(ScheduleBucket::Evening),
        "someday" => Ok(ScheduleBucket::Someday),
        other => bail!("invalid scheduleBucket {other:?}"),
    }
}

fn load_filtered_tasks(store: &Store, filter: &JsonValue) -> Result<Vec<Task>> {
    let today = Local::now().date_naive();
    let mut tasks = if let Some(view_name) = filter.get("view").and_then(JsonValue::as_str) {
        let view = match view_name.trim() {
            "inbox" => View::Inbox,
            "all" => View::All,
            "today" => View::Today,
            "tomorrow" => View::Tomorrow,
            "next_7_days" | "next7days" => View::Next7Days,
            "completed" => View::Completed,
            "wont_do" => View::WontDo,
            "trash" => View::Trash,
            other => bail!("unknown task view {other:?}"),
        };
        store.tasks_in(&view, today)?
    } else if let Some(list_id_str) = filter.get("listId").and_then(JsonValue::as_str) {
        let list_id = parse_uuid(list_id_str)?;
        store.tasks_in(&View::List(list_id), today)?
    } else if let Some(tag) = filter.get("tag").and_then(JsonValue::as_str) {
        store.tasks_in(&View::Tag(tag.to_owned()), today)?
    } else {
        store.tasks()?
    };

    let include_deleted = filter
        .get("includeDeleted")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    if !include_deleted && filter.get("view").and_then(JsonValue::as_str) != Some("trash") {
        tasks.retain(|t| !t.is_deleted());
    }
    if let Some(completed) = filter.get("completed").and_then(JsonValue::as_bool) {
        tasks.retain(|t| t.is_completed() == completed);
    }
    if let Some(search) = filter.get("search").and_then(JsonValue::as_str) {
        let q = search.trim().to_ascii_lowercase();
        if !q.is_empty() {
            tasks.retain(|t| {
                t.title.to_ascii_lowercase().contains(&q)
                    || t.notes.to_ascii_lowercase().contains(&q)
                    || t.tags.iter().any(|tag| tag.to_ascii_lowercase().contains(&q))
            });
        }
    }
    Ok(tasks)
}

pub fn task_to_json(task: &Task) -> JsonValue {
    let bucket = match task.schedule_bucket {
        ScheduleBucket::None => "none",
        ScheduleBucket::Evening => "evening",
        ScheduleBucket::Someday => "someday",
    };
    json!({
        "id": task.id.to_string(),
        "title": task.title,
        "notes": task.notes,
        "listId": task.list_id.map(|i| i.to_string()),
        "parentId": task.parent_id.map(|i| i.to_string()),
        "sectionId": task.section_id.map(|i| i.to_string()),
        "kind": task.kind.as_str(),
        "priority": match task.priority {
            Priority::None => "none",
            Priority::Low => "low",
            Priority::Medium => "medium",
            Priority::High => "high",
        },
        "dueDate": task.due_date.map(|d| d.format("%Y-%m-%d").to_string()),
        "dueTime": task.due_time.map(|t| t.format("%H:%M").to_string()),
        "endDate": task.end_date.map(|d| d.format("%Y-%m-%d").to_string()),
        "endTime": task.end_time.map(|t| t.format("%H:%M").to_string()),
        "startDate": task.start_date.map(|d| d.format("%Y-%m-%d").to_string()),
        "scheduleBucket": bucket,
        "repeat": task.repeat.as_ref().map(ToString::to_string),
        "tags": task.tags,
        "checklist": task.checklist.map(|(done, total)| json!({ "done": done, "total": total })),
        "completed": task.is_completed(),
        "completedAt": task.completed_at,
        "deletedAt": task.deleted_at,
        "wontDoAt": task.wont_do_at,
        "inProgressAt": task.in_progress_at,
        "pinnedAt": task.pinned_at,
        "createdAt": task.created_at,
        "modifiedAt": task.modified_at,
        "location": task.location,
        "timezone": task.timezone,
        "isFloating": task.is_floating,
        "constantReminder": task.constant_reminder,
        "reminders": task.reminders,
    })
}

fn list_to_json(list: &fern_core::model::List) -> JsonValue {
    json!({
        "id": list.id.to_string(),
        "name": list.name,
        "color": list.color,
        "openCount": list.open_count,
        "icon": list.icon,
        "folderId": list.folder.map(|f| f.to_string()),
        "kind": list.kind.as_str(),
        "inSmartLists": list.in_smart_lists,
        "archived": list.archived,
        "notes": list.notes,
        "deadline": list.deadline.map(|d| d.format("%Y-%m-%d").to_string()),
        "completedAt": list.completed_at,
    })
}

pub fn parse_task_draft(input: &JsonValue) -> Result<(TaskDraft, bool)> {
    if let Some(title) = input.as_str() {
        let trimmed = title.trim();
        ensure!(!trimmed.is_empty(), "task title must not be empty");
        return Ok((
            TaskDraft {
                title: trimmed.to_owned(),
                ..Default::default()
            },
            true,
        ));
    }
    let obj = input
        .as_object()
        .context("task draft must be a string or object")?;
    let title = obj
        .get("title")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .context("task title must not be empty")?
        .to_owned();

    let mut draft = TaskDraft {
        title,
        ..Default::default()
    };
    if let Some(notes) = obj.get("notes").and_then(JsonValue::as_str) {
        draft.notes = notes.to_owned();
    }
    if let Some(list_id) = obj.get("listId").and_then(JsonValue::as_str)
        && !list_id.trim().is_empty()
    {
        draft.list_id = Some(parse_uuid(list_id)?);
    }
    if let Some(section_id) = obj.get("sectionId").and_then(JsonValue::as_str)
        && !section_id.trim().is_empty()
    {
        draft.section_id = Some(parse_uuid(section_id)?);
    }
    if let Some(kind) = obj.get("kind").and_then(JsonValue::as_str) {
        draft.kind = Kind::parse(kind);
    }
    if let Some(prio) = obj.get("priority")
        && !prio.is_null()
    {
        draft.priority = parse_priority(prio)?;
    }
    if let Some(d) = obj.get("dueDate").and_then(JsonValue::as_str)
        && !d.trim().is_empty()
    {
        draft.due_date = Some(parse_date(d)?);
    }
    if let Some(t) = obj.get("dueTime").and_then(JsonValue::as_str)
        && !t.trim().is_empty()
    {
        draft.due_time = Some(parse_time(t)?);
    }
    if let Some(d) = obj.get("endDate").and_then(JsonValue::as_str)
        && !d.trim().is_empty()
    {
        draft.end_date = Some(parse_date(d)?);
    }
    if let Some(t) = obj.get("endTime").and_then(JsonValue::as_str)
        && !t.trim().is_empty()
    {
        draft.end_time = Some(parse_time(t)?);
    }
    if let Some(d) = obj.get("startDate").and_then(JsonValue::as_str)
        && !d.trim().is_empty()
    {
        draft.start_date = Some(parse_date(d)?);
    }
    if let Some(b) = obj.get("scheduleBucket").and_then(JsonValue::as_str) {
        draft.schedule_bucket = parse_schedule_bucket(b)?;
    }
    if let Some(rep) = obj.get("repeat").and_then(JsonValue::as_str)
        && !rep.trim().is_empty()
    {
        draft.repeat = Some(
            Recurrence::from_str(rep).map_err(|e| anyhow!("invalid repeat rule: {e}"))?,
        );
    }
    if let Some(tags) = obj.get("tags").and_then(JsonValue::as_array) {
        draft.tags = tags
            .iter()
            .filter_map(JsonValue::as_str)
            .map(str::to_owned)
            .collect();
    }
    if let Some(checklist) = obj.get("checklist").and_then(JsonValue::as_array) {
        for item in checklist {
            if let Some(text) = item.as_str() {
                draft.checklist.push((text.to_owned(), false));
            } else if let Some(item_obj) = item.as_object()
                && let Some(title) = item_obj.get("title").and_then(JsonValue::as_str)
            {
                let done = item_obj
                    .get("completed")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false);
                draft.checklist.push((title.to_owned(), done));
            }
        }
    }
    if let Some(loc) = obj.get("location").and_then(JsonValue::as_str) {
        draft.location = Some(loc.to_owned());
    }
    if let Some(tz) = obj.get("timezone").and_then(JsonValue::as_str) {
        draft.timezone = Some(tz.to_owned());
    }
    if let Some(fl) = obj.get("isFloating").and_then(JsonValue::as_bool) {
        draft.is_floating = fl;
    }
    if let Some(cr) = obj.get("constantReminder").and_then(JsonValue::as_bool) {
        draft.constant_reminder = cr;
    }
    if let Some(rems) = obj.get("reminders").and_then(JsonValue::as_array) {
        draft.reminders = rems.iter().filter_map(JsonValue::as_i64).collect();
    }
    let top = obj.get("top").and_then(JsonValue::as_bool).unwrap_or(true);
    Ok((draft, top))
}

fn apply_task_patch(store: &Store, id: Id, patch: &JsonValue) -> Result<Task> {
    let obj = patch
        .as_object()
        .context("task patch must be an object")?;
    let existing = store.task(id)?.context("task not found")?;

    if let Some(title) = obj.get("title").and_then(JsonValue::as_str) {
        let trimmed = title.trim();
        ensure!(!trimmed.is_empty(), "task title must not be empty");
        store.set_title(id, trimmed)?;
    }
    if let Some(notes) = obj.get("notes").and_then(JsonValue::as_str) {
        store.set_notes(id, notes)?;
    }
    if let Some(prio) = obj.get("priority")
        && !prio.is_null()
    {
        store.set_priority(id, parse_priority(prio)?)?;
    }
    if let Some(kind) = obj.get("kind").and_then(JsonValue::as_str) {
        store.set_kind(id, Kind::parse(kind))?;
    }
    if let Some(list_val) = obj.get("listId") {
        let list_id = match list_val {
            JsonValue::Null => None,
            JsonValue::String(s) if s.trim().is_empty() => None,
            JsonValue::String(s) => Some(parse_uuid(s)?),
            _ => bail!("invalid listId"),
        };
        store.set_list(id, list_id)?;
    }
    if let Some(sec_val) = obj.get("sectionId") {
        let sec_id = match sec_val {
            JsonValue::Null => None,
            JsonValue::String(s) if s.trim().is_empty() => None,
            JsonValue::String(s) => Some(parse_uuid(s)?),
            _ => bail!("invalid sectionId"),
        };
        store.set_section(id, sec_id)?;
    }
    if obj.contains_key("dueDate") || obj.contains_key("dueTime") {
        let next_date = match obj.get("dueDate") {
            Some(JsonValue::Null) => None,
            Some(JsonValue::String(s)) if s.trim().is_empty() => None,
            Some(JsonValue::String(s)) => Some(parse_date(s)?),
            Some(_) => bail!("invalid dueDate"),
            None => existing.due_date,
        };
        let next_time = match obj.get("dueTime") {
            Some(JsonValue::Null) => None,
            Some(JsonValue::String(s)) if s.trim().is_empty() => None,
            Some(JsonValue::String(s)) => Some(parse_time(s)?),
            Some(_) => bail!("invalid dueTime"),
            None => existing.due_time,
        };
        store.set_due(id, next_date, next_time)?;
    }
    if obj.contains_key("endDate") || obj.contains_key("endTime") {
        let next_end_date = match obj.get("endDate") {
            Some(JsonValue::Null) => None,
            Some(JsonValue::String(s)) if s.trim().is_empty() => None,
            Some(JsonValue::String(s)) => Some(parse_date(s)?),
            Some(_) => bail!("invalid endDate"),
            None => existing.end_date,
        };
        let next_end_time = match obj.get("endTime") {
            Some(JsonValue::Null) => None,
            Some(JsonValue::String(s)) if s.trim().is_empty() => None,
            Some(JsonValue::String(s)) => Some(parse_time(s)?),
            Some(_) => bail!("invalid endTime"),
            None => existing.end_time,
        };
        store.set_due_end(id, next_end_date.map(|d| (d, next_end_time)))?;
    }
    if obj.contains_key("startDate") || obj.contains_key("scheduleBucket") {
        let next_start = match obj.get("startDate") {
            Some(JsonValue::Null) => None,
            Some(JsonValue::String(s)) if s.trim().is_empty() => None,
            Some(JsonValue::String(s)) => Some(parse_date(s)?),
            Some(_) => bail!("invalid startDate"),
            None => existing.start_date,
        };
        let next_bucket = match obj.get("scheduleBucket") {
            Some(JsonValue::Null) => ScheduleBucket::None,
            Some(JsonValue::String(s)) => parse_schedule_bucket(s)?,
            Some(_) => bail!("invalid scheduleBucket"),
            None => existing.schedule_bucket,
        };
        store.set_task_schedule(id, next_start, next_bucket)?;
    }
    if let Some(rep_val) = obj.get("repeat") {
        let today = Local::now().date_naive();
        let rule = match rep_val {
            JsonValue::Null => None,
            JsonValue::String(s) if s.trim().is_empty() => None,
            JsonValue::String(s) => {
                Some(Recurrence::from_str(s).map_err(|e| anyhow!("invalid repeat: {e}"))?)
            }
            _ => bail!("invalid repeat"),
        };
        store.set_repeat(id, rule, today)?;
    }
    if let Some(tags_arr) = obj.get("tags").and_then(JsonValue::as_array) {
        let tags: Vec<String> = tags_arr
            .iter()
            .filter_map(JsonValue::as_str)
            .map(str::to_owned)
            .collect();
        store.set_tags(id, &tags)?;
    }
    if let Some(pinned) = obj.get("pinned").and_then(JsonValue::as_bool) {
        store.set_pinned(id, pinned)?;
    }
    if let Some(in_prog) = obj.get("inProgress").and_then(JsonValue::as_bool) {
        store.set_in_progress(id, in_prog)?;
    }
    if let Some(wont_do) = obj.get("wontDo").and_then(JsonValue::as_bool) {
        store.set_wont_do(id, wont_do)?;
    }
    if let Some(loc_val) = obj.get("location") {
        let loc = match loc_val {
            JsonValue::Null => None,
            JsonValue::String(s) => Some(s.as_str()),
            _ => bail!("invalid location"),
        };
        store.set_location(id, loc)?;
    }
    if obj.contains_key("timezone") || obj.contains_key("isFloating") {
        let tz = match obj.get("timezone") {
            Some(JsonValue::Null) => None,
            Some(JsonValue::String(s)) => Some(s.to_owned()),
            Some(_) => bail!("invalid timezone"),
            None => existing.timezone.clone(),
        };
        let floating = obj
            .get("isFloating")
            .and_then(JsonValue::as_bool)
            .unwrap_or(existing.is_floating);
        store.set_timezone(id, tz.as_deref(), floating)?;
    }
    if let Some(cr) = obj.get("constantReminder").and_then(JsonValue::as_bool) {
        store.set_task_constant_reminder(id, cr)?;
    }
    if let Some(rems) = obj.get("reminders").and_then(JsonValue::as_array) {
        let list: Vec<i64> = rems.iter().filter_map(JsonValue::as_i64).collect();
        store.set_reminders(id, &list)?;
    }
    if let Some(checklist_arr) = obj.get("checklist").and_then(JsonValue::as_array) {
        let mut desired: Vec<(String, bool)> = Vec::with_capacity(checklist_arr.len());
        for item in checklist_arr {
            if let Some(text) = item.as_str() {
                if !text.trim().is_empty() {
                    desired.push((text.to_owned(), false));
                }
            } else if let Some(item_obj) = item.as_object()
                && let Some(title) = item_obj.get("title").and_then(JsonValue::as_str)
                && !title.trim().is_empty()
            {
                let done = item_obj
                    .get("completed")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false);
                desired.push((title.to_owned(), done));
            }
        }
        let existing_items = store.checklist(id)?;
        let mut last_id: Option<Id> = None;
        for (idx, (title, done)) in desired.iter().enumerate() {
            if let Some(cur) = existing_items.get(idx) {
                if cur.title != *title {
                    store.set_checklist_item_title(cur.id, title)?;
                }
                if cur.completed != *done {
                    store.set_checklist_item_completed(cur.id, *done)?;
                }
                last_id = Some(cur.id);
            } else {
                let added = store.add_checklist_item(id, title, last_id)?;
                if *done {
                    store.set_checklist_item_completed(added.id, true)?;
                }
                last_id = Some(added.id);
            }
        }
        for stale in existing_items.iter().skip(desired.len()) {
            store.delete_checklist_item(stale.id)?;
        }
    }
    if let Some(parent_val) = obj.get("parentId") {
        let parent_id = match parent_val {
            JsonValue::Null => None,
            JsonValue::String(s) if s.trim().is_empty() => None,
            JsonValue::String(s) => Some(parse_uuid(s)?),
            _ => bail!("invalid parentId"),
        };
        store.set_parent(id, parent_id)?;
    }
    if let Some(completed) = obj.get("completed").and_then(JsonValue::as_bool) {
        store.complete(id, completed)?;
    }
    store.task(id)?.context("task not found")
}

fn parse_list_draft(input: &JsonValue) -> Result<ListDraft> {
    if let Some(name) = input.as_str() {
        let trimmed = name.trim();
        ensure!(!trimmed.is_empty(), "list name must not be empty");
        return Ok(ListDraft {
            name: trimmed.to_owned(),
            ..Default::default()
        });
    }
    let obj = input
        .as_object()
        .context("list draft must be a string or object")?;
    let name = obj
        .get("name")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .context("list name must not be empty")?
        .to_owned();
    let mut draft = ListDraft {
        name,
        ..Default::default()
    };
    if let Some(color) = obj.get("color").and_then(JsonValue::as_str) {
        draft.color = Some(color.to_owned());
    }
    if let Some(icon) = obj.get("icon").and_then(JsonValue::as_str) {
        draft.icon = Some(icon.to_owned());
    }
    if let Some(folder_id) = obj.get("folderId").and_then(JsonValue::as_str)
        && !folder_id.trim().is_empty()
    {
        draft.folder = Some(parse_uuid(folder_id)?);
    }
    if let Some(kind) = obj.get("kind").and_then(JsonValue::as_str) {
        draft.kind = Kind::parse(kind);
    }
    if let Some(smart) = obj.get("inSmartLists").and_then(JsonValue::as_bool) {
        draft.in_smart_lists = smart;
    }
    if let Some(notes) = obj.get("notes").and_then(JsonValue::as_str) {
        draft.notes = notes.to_owned();
    }
    if let Some(d) = obj.get("deadline").and_then(JsonValue::as_str)
        && !d.trim().is_empty()
    {
        draft.deadline = Some(parse_date(d)?);
    }
    Ok(draft)
}

fn filter_focus_sessions(
    sessions: &[fern_core::FocusSession],
    range: &JsonValue,
) -> Result<Vec<fern_core::FocusSession>> {
    let from_ms = match range.get("from") {
        Some(JsonValue::Number(n)) => n.as_i64(),
        Some(JsonValue::String(s)) => {
            let d = parse_date(s)?;
            d.and_hms_opt(0, 0, 0)
                .and_then(|dt| dt.and_local_timezone(Local).earliest())
                .map(|dt| dt.timestamp_millis())
        }
        _ => None,
    };
    let to_ms = match range.get("to") {
        Some(JsonValue::Number(n)) => n.as_i64(),
        Some(JsonValue::String(s)) => {
            let d = parse_date(s)?;
            d.and_hms_opt(23, 59, 59)
                .and_then(|dt| dt.and_local_timezone(Local).latest())
                .map(|dt| dt.timestamp_millis() + 999)
        }
        _ => None,
    };
    Ok(sessions
        .iter()
        .filter(|s| {
            from_ms.is_none_or(|f| s.started_at >= f) && to_ms.is_none_or(|t| s.ended_at <= t)
        })
        .cloned()
        .collect())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
