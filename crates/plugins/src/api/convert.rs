//! JS<->JSON, UI AST, and domain model conversion / query helpers for QuickJS host bindings.

use std::collections::HashSet;
use std::str::FromStr as _;

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use chrono::{Local, NaiveDate, NaiveTime};
use fern_core::model::{Id, Kind, ListDraft, Priority, ScheduleBucket, Task, TaskDraft, View};
use fern_core::recurrence::Recurrence;
use fern_core::{Horizon, Store};
use rquickjs::{Array, Ctx, Exception, Object, Type, Value};
use serde_json::{Map, Value as JsonValue, json};
use uuid::Uuid;

use super::PluginApiState;
use crate::ui_ast::{HandlerId, UiNode};

const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_NODES: usize = 65_536;
const MAX_JSON_ARRAY_LEN: usize = 65_536;
const MAX_JSON_PREALLOC: usize = 1024;
const JS_MAX_SAFE_INT_F64: f64 = 9_007_199_254_740_991.0;

pub(super) fn js_float_to_json(f: f64) -> JsonValue {
    if !f.is_finite() {
        return JsonValue::Null;
    }
    if f.fract() == 0.0 && (-JS_MAX_SAFE_INT_F64..=JS_MAX_SAFE_INT_F64).contains(&f) {
        return JsonValue::from(f as i64);
    }
    serde_json::Number::from_f64(f)
        .map(JsonValue::Number)
        .unwrap_or(JsonValue::Null)
}

/// Converts a JS `UiNode` value (including inline closures on `onClick`/`onChange`/`onSubmit`/`onAction`)
/// into a validated, SVG-sanitized [`UiNode`].
pub fn js_value_to_ui_node<'js>(
    ctx: &Ctx<'js>,
    val: Value<'js>,
    state: &PluginApiState,
    scope: &str,
) -> Result<UiNode> {
    let mut visited = 0usize;
    let json_val = js_to_json_with_mode(ctx, val, JsWalkMode::Ui { state, scope }, 0, &mut visited)?;
    let mut node: UiNode = serde_json::from_value(json_val)
        .context("view render() returned invalid UiNode tree")?;
    node.sanitize_tree()?;
    Ok(node)
}

fn is_ui_handler_prop(key: &str) -> bool {
    matches!(
        key,
        "onClick"
            | "on_click"
            | "onChange"
            | "on_change"
            | "onSubmit"
            | "on_submit"
            | "onAction"
            | "on_action"
    )
}

#[derive(Clone, Copy)]
enum JsWalkMode<'a> {
    Plain,
    Ui {
        state: &'a PluginApiState,
        scope: &'a str,
    },
}

pub fn js_to_json<'js>(ctx: &Ctx<'js>, val: Value<'js>) -> Result<JsonValue> {
    let mut visited = 0usize;
    js_to_json_with_mode(ctx, val, JsWalkMode::Plain, 0, &mut visited)
}

fn js_to_json_with_mode<'js>(
    ctx: &Ctx<'js>,
    val: Value<'js>,
    mode: JsWalkMode<'_>,
    depth: usize,
    visited: &mut usize,
) -> Result<JsonValue> {
    let kind_label = match mode {
        JsWalkMode::Plain => "JSON value",
        JsWalkMode::Ui { .. } => "UI tree",
    };
    *visited += 1;
    ensure!(
        *visited <= MAX_JSON_NODES,
        "{kind_label} exceeds maximum node budget ({MAX_JSON_NODES})"
    );
    ensure!(
        depth < MAX_JSON_DEPTH,
        "{kind_label} nesting exceeds {MAX_JSON_DEPTH}"
    );
    if let JsWalkMode::Ui { state, scope } = mode
        && let Some(func) = val.as_function()
    {
        let id = state.register_handler(ctx, scope, func.clone())?;
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
        return Ok(js_float_to_json(f));
    }
    if let Some(s) = val.as_string() {
        return Ok(JsonValue::String(s.to_string()?));
    }
    if val.is_array() {
        let arr = Array::from_value(val)?;
        let len = arr.len();
        ensure!(
            len <= MAX_JSON_ARRAY_LEN,
            "array length {len} exceeds maximum allowed ({MAX_JSON_ARRAY_LEN})"
        );
        let mut out = Vec::with_capacity(len.min(MAX_JSON_PREALLOC));
        for item in arr.iter::<Value<'js>>() {
            let item = item?;
            if matches!(mode, JsWalkMode::Plain) && (item.is_undefined() || item.is_function()) {
                *visited += 1;
                ensure!(
                    *visited <= MAX_JSON_NODES,
                    "{kind_label} exceeds maximum node budget ({MAX_JSON_NODES})"
                );
                out.push(JsonValue::Null);
            } else {
                out.push(js_to_json_with_mode(ctx, item, mode, depth + 1, visited)?);
            }
        }
        return Ok(JsonValue::Array(out));
    }
    if let Some(obj) = val.as_object() {
        if matches!(mode, JsWalkMode::Plain) && obj.is_function() {
            return Ok(JsonValue::Null);
        }
        let mut map = Map::new();
        for prop in obj.props::<String, Value<'js>>() {
            *visited += 1;
            ensure!(
                *visited <= MAX_JSON_NODES,
                "{kind_label} exceeds maximum node budget ({MAX_JSON_NODES})"
            );
            let (k, v) = prop?;
            if v.is_undefined() {
                continue;
            }
            if let JsWalkMode::Ui { state, scope } = mode
                && is_ui_handler_prop(&k)
            {
                if let Some(func) = v.as_function() {
                    let id = state.register_handler(ctx, scope, func.clone())?;
                    map.insert(k, JsonValue::from(id));
                    continue;
                }
                if let Some(hid) = v.as_int().and_then(|n| u32::try_from(n).ok()).or_else(|| {
                    v.as_float().and_then(|f| {
                        (f.is_finite() && f.fract() == 0.0 && f >= 0.0 && f <= f64::from(u32::MAX))
                            .then_some(f as u32)
                    })
                }) {
                    state.rehome_default_handler(scope, hid)?;
                    map.insert(k, JsonValue::from(hid));
                    continue;
                }
            }
            if v.is_function() {
                continue;
            }
            map.insert(k, js_to_json_with_mode(ctx, v, mode, depth + 1, visited)?);
        }
        return Ok(JsonValue::Object(map));
    }
    Ok(JsonValue::Null)
}

pub fn json_to_js<'js>(ctx: &Ctx<'js>, val: &JsonValue) -> rquickjs::Result<Value<'js>> {
    json_to_js_inner(ctx, val, 0)
}

fn json_to_js_inner<'js>(
    ctx: &Ctx<'js>,
    val: &JsonValue,
    depth: usize,
) -> rquickjs::Result<Value<'js>> {
    if depth >= MAX_JSON_DEPTH {
        return Err(Exception::throw_message(
            ctx,
            &format!("JSON nesting exceeds {MAX_JSON_DEPTH}"),
        ));
    }
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
                arr.set(idx, json_to_js_inner(ctx, item, depth + 1)?)?;
            }
            Ok(arr.into_value())
        }
        JsonValue::Object(map) => {
            let obj = Object::new(ctx.clone())?;
            for (k, v) in map {
                obj.set(k.as_str(), json_to_js_inner(ctx, v, depth + 1)?)?;
            }
            Ok(obj.into_value())
        }
    }
}

pub(super) fn format_js_value<'js>(ctx: &Ctx<'js>, val: Value<'js>) -> String {
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

pub(super) fn throw_scrubbed(
    ctx: &Ctx<'_>,
    state: &PluginApiState,
    err: anyhow::Error,
) -> rquickjs::Error {
    let scrubbed = state.scrub(&format!("{err:#}"));
    Exception::throw_message(ctx, &scrubbed)
}

pub(super) fn parse_uuid(raw: &str) -> Result<Id> {
    Uuid::parse_str(raw.trim()).with_context(|| format!("invalid UUID {raw:?}"))
}

pub(super) fn parse_date(raw: &str) -> Result<NaiveDate> {
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

fn collect_tag_family_lower(store: &Store, root_tag: &str) -> HashSet<String> {
    let root_lower = root_tag.to_lowercase();
    let mut family = HashSet::new();
    family.insert(root_lower);
    if let Ok(all_tags) = store.tags() {
        loop {
            let mut added = false;
            for tag in &all_tags {
                let name_lower = tag.name.to_lowercase();
                if family.contains(&name_lower) {
                    continue;
                }
                if tag
                    .parent
                    .as_deref()
                    .is_some_and(|p| family.contains(&p.to_lowercase()))
                {
                    family.insert(name_lower);
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
    }
    family
}

pub(super) fn load_filtered_tasks(store: &Store, filter: &JsonValue) -> Result<Vec<Task>> {
    let today = Local::now().date_naive();
    let view_opt = filter
        .get("view")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let list_id_opt = filter
        .get("listId")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(parse_uuid)
        .transpose()?;
    let tag_opt = filter
        .get("tag")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut tasks = if let Some(view_name) = view_opt {
        let view = match view_name {
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
    } else if let Some(list_id) = list_id_opt {
        store.tasks_in(&View::List(list_id), today)?
    } else if let Some(tag) = tag_opt {
        store.tasks_in(&View::Tag(tag.to_owned()), today)?
    } else {
        store.tasks()?
    };

    if view_opt.is_some()
        && let Some(list_id) = list_id_opt
    {
        tasks.retain(|t| t.list_id == Some(list_id));
    }
    if (view_opt.is_some() || list_id_opt.is_some())
        && let Some(tag) = tag_opt
    {
        let family = collect_tag_family_lower(store, tag);
        tasks.retain(|t| t.tags.iter().any(|tg| family.contains(&tg.to_lowercase())));
    }

    let include_deleted = filter
        .get("includeDeleted")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    if !include_deleted && view_opt != Some("trash") {
        tasks.retain(|t| !t.is_deleted());
    }
    if let Some(completed) = filter.get("completed").and_then(JsonValue::as_bool) {
        tasks.retain(|t| t.is_completed() == completed);
    }
    if let Some(h_val) = filter.get("horizon") {
        match h_val {
            JsonValue::Null => tasks.retain(|t| t.horizon.is_none()),
            JsonValue::String(s) if s.trim().is_empty() => {}
            JsonValue::String(s) => {
                let target = Horizon::parse(s).with_context(|| format!("invalid horizon {s:?}"))?;
                tasks.retain(|t| t.horizon == Some(target));
            }
            _ => bail!("invalid horizon filter"),
        }
    }
    if let Some(search) = filter.get("search").and_then(JsonValue::as_str) {
        let q = search.trim().to_lowercase();
        if !q.is_empty() {
            tasks.retain(|t| {
                t.title.to_lowercase().contains(&q)
                    || t.notes.to_lowercase().contains(&q)
                    || t.tags.iter().any(|tag| tag.to_lowercase().contains(&q))
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
        "horizon": task.horizon.map(|h| h.as_code()),
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

pub(super) fn list_to_json(list: &fern_core::model::List) -> JsonValue {
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
    if let Some(h_val) = obj.get("horizon") {
        match h_val {
            JsonValue::Null => {}
            JsonValue::String(s) if s.trim().is_empty() => {}
            JsonValue::String(s) => {
                draft.horizon = Some(
                    Horizon::parse(s).with_context(|| format!("invalid horizon {s:?}"))?,
                );
            }
            _ => bail!("invalid horizon"),
        }
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

pub(super) fn apply_task_patch(store: &Store, id: Id, patch: &JsonValue) -> Result<Task> {
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
    if let Some(h_val) = obj.get("horizon") {
        let next_horizon = match h_val {
            JsonValue::Null => None,
            JsonValue::String(s) if s.trim().is_empty() => None,
            JsonValue::String(s) => {
                Some(Horizon::parse(s).with_context(|| format!("invalid horizon {s:?}"))?)
            }
            _ => bail!("invalid horizon"),
        };
        store.set_horizon(id, next_horizon)?;
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

pub(super) fn parse_list_draft(input: &JsonValue) -> Result<ListDraft> {
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

pub(super) fn filter_focus_sessions(
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

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
