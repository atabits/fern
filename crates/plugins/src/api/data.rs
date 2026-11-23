//! Core domain data QuickJS host API bindings (`fern.tasks`, `lists`, `tags`,
//! `habits`, `focus`, `countdowns`, `batch`, `storage`, `meta`, `settings`).

use std::rc::Rc;

use anyhow::{Context as _, Result, bail, ensure};
use chrono::Local;
use fern_core::countdown::CountdownKind;
use fern_core::focus::focus_stats;
use rquickjs::function::Opt;
use rquickjs::{Ctx, Exception, Function, Object, Persistent, Value};
use serde_json::{Map, Value as JsonValue, json};

use super::PluginApiState;
use super::convert::{
    apply_task_patch, filter_focus_sessions, js_to_json, json_to_js, list_to_json,
    load_filtered_tasks, parse_date, parse_list_draft, parse_task_draft, parse_uuid, task_to_json,
    throw_scrubbed,
};
use crate::permissions::Permission;

pub(super) fn install_tasks<'js>(
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

pub(super) fn install_lists<'js>(
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

pub(super) fn install_tags<'js>(
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

pub(super) fn install_habits<'js>(
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

pub(super) fn install_focus<'js>(
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

pub(super) fn install_countdowns<'js>(
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

pub(super) fn install_batch<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    struct BatchStackGuard {
        state: Rc<PluginApiState>,
        armed: bool,
    }
    impl BatchStackGuard {
        fn push(state: Rc<PluginApiState>) -> Self {
            state.batch_dirty_stack.borrow_mut().push(false);
            Self { state, armed: true }
        }
        fn pop_on_commit(mut self) -> bool {
            self.armed = false;
            self.state.batch_dirty_stack.borrow_mut().pop().unwrap_or(false)
        }
    }
    impl Drop for BatchStackGuard {
        fn drop(&mut self) {
            if self.armed {
                let _ = self.state.batch_dirty_stack.borrow_mut().pop();
            }
        }
    }

    let st = Rc::clone(&state);
    let batch_fn = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, func: Function<'js>| -> rquickjs::Result<Value<'js>> {
            let mut result_slot: Option<Value<'js>> = None;
            let mut js_err_slot: Option<rquickjs::Error> = None;
            let mut committed_dirty = false;

            // `with_plugin_write_ctx` uses `Store::in_tx`, which begins a real SQLite
            // transaction for the outermost batch and a `SAVEPOINT` for nested batches.
            let tx_res = st
                .store
                .with_plugin_write_ctx(&st.plugin_id, st.write_depth.get(), || {
                    let guard = BatchStackGuard::push(Rc::clone(&st));
                    let call_res = func.call::<_, Value<'js>>(());
                    match call_res {
                        Ok(val) => {
                            if val.is_promise() {
                                anyhow::bail!("fern.batch(fn) callback must be synchronous");
                            }
                            committed_dirty = guard.pop_on_commit();
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
                    if committed_dirty {
                        let mut stack = st.batch_dirty_stack.borrow_mut();
                        if let Some(parent_dirty) = stack.last_mut() {
                            *parent_dirty = true;
                        } else {
                            st.data_dirty.set(true);
                        }
                    }
                    Ok(result_slot.unwrap_or_else(|| Value::new_undefined(ctx)))
                }
                Err(err) => Err(throw_scrubbed(&ctx, &st, err)),
            }
        },
    )?;
    fern.set("batch", batch_fn)?;
    Ok(())
}

pub(super) fn install_storage<'js>(
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

pub(super) fn install_meta<'js>(
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

pub(super) fn install_settings<'js>(
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
