use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use fern_core::Store;
use fern_core::plugin_store::{MAX_EVENT_DEPTH, UserScript};
use serde_json::{Map, Value as JsonValue, json};
use tempfile::TempDir;

use super::{
    CIRCUIT_BREAKER_THRESHOLD, HostUpdate, HookDecision, PluginEngineConfig, PluginManager,
    PluginStatus, SHUTDOWN_BUDGET,
};
use crate::api::{PluginUiEffect, PromptReplyValue};
use crate::permissions::Permission;
use crate::scaffold::generate_sample_plugin;
use crate::ui_ast::{HandlerId, UiEvent, UiNode, UiTreePayload};
use crate::vault::SecretsVault;

struct TestEnv {
    _tmp: TempDir,
    plugins_dir: PathBuf,
    db_path: PathBuf,
    store: Store,
    vault_path: PathBuf,
    key_path: PathBuf,
}

impl TestEnv {
    fn new() -> Result<Self> {
        let tmp = tempfile::tempdir()?;
        let plugins_dir = tmp.path().join("plugins");
        fs::create_dir_all(&plugins_dir)?;
        let db_path = tmp.path().join("tasks.db");
        let store = Store::open(&db_path)?;
        let vault_path = tmp.path().join("plugin-secrets.enc");
        let key_path = tmp.path().join("plugin-secrets.key");
        Ok(Self {
            _tmp: tmp,
            plugins_dir,
            db_path,
            store,
            vault_path,
            key_path,
        })
    }

    fn engine_config(&self) -> PluginEngineConfig {
        PluginEngineConfig {
            plugins_dir: self.plugins_dir.clone(),
            db_path: Some(self.db_path.clone()),
            secrets_vault_path: self.vault_path.clone(),
            secrets_key_path: self.key_path.clone(),
            safe_mode: false,
            locale: "en".to_owned(),
            theme_appearance: "light".to_owned(),
            hot_reload: false,
            wake_callback: None,
        }
    }

    fn write_plugin(&self, id: &str, manifest_json: &str, files: &[(&str, &str)]) -> Result<()> {
        let dir = self.plugins_dir.join(id);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("manifest.json"), manifest_json)?;
        for (rel, content) in files {
            let target = dir.join(rel);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(target, content)?;
        }
        Ok(())
    }
}

#[test]
fn js_crud_canonical_recurrence_batch_rollback_and_outbox_echo_depth_guard() -> Result<()> {
    let env = TestEnv::new()?;

    env.write_plugin(
        "com.test.producer",
        r#"{
            "api_version": 1,
            "id": "com.test.producer",
            "name": "Producer",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["tasks:read", "tasks:write", "storage"],
            "activation_events": ["onCommand:produce", "onEvent:task.created"],
            "contributions": {
                "commands": [{ "id": "produce", "title": "Produce" }]
            }
        }"#,
        &[(
            "index.js",
            r#"
            import { tasks, batch, storage, commands, events } from "fern";
            export function activate() {
                events.on("task.created", (ev) => {
                    const prev = storage.get("echoHits") ?? [];
                    prev.push(ev);
                    storage.set("echoHits", prev);
                    // Ping-pong with consumer to exercise MAX_EVENT_DEPTH termination.
                    if (ev.originPlugin === "com.test.consumer") {
                        tasks.create({ title: `Pong ${ev.depth}` });
                    }
                });
                commands.register("produce", () => {
                    const t = tasks.create({
                        title: "Recurring Task",
                        dueDate: "2026-11-22",
                        repeat: "FREQ=DAILY;INTERVAL=1"
                    });
                    const completed = tasks.complete(t.id);
                    try {
                        batch(() => {
                            tasks.create({ title: "Should Rollback" });
                            throw new Error("abort batch");
                        });
                    } catch (_e) {}
                    return { createdId: t.id, completedCount: completed.length };
                });
            }
            "#,
        )],
    )?;

    env.write_plugin(
        "com.test.consumer",
        r#"{
            "api_version": 1,
            "id": "com.test.consumer",
            "name": "Consumer",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["tasks:read", "tasks:write", "storage"],
            "activation_events": ["onEvent:task.created"]
        }"#,
        &[(
            "index.js",
            r#"
            import { tasks, storage, events } from "fern";
            export function activate() {
                events.on("task.created", (ev) => {
                    const seen = storage.get("seen") ?? [];
                    seen.push(ev);
                    storage.set("seen", seen);
                    tasks.create({ title: `Followup ${ev.depth}` });
                });
            }
            "#,
        )],
    )?;

    let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.producer")?;
    mgr.approve_permissions("com.test.consumer")?;

    let res = mgr.run_command("com.test.producer", "produce", None)?;
    assert!(res["createdId"].as_str().is_some());

    // Canonical complete generated the next repeating task; rolled-back batch task is absent.
    let all_tasks = env.store.tasks()?;
    assert!(!all_tasks.iter().any(|t| t.title == "Should Rollback"));
    assert_eq!(
        all_tasks.iter().filter(|t| t.title == "Recurring Task").count(),
        2
    );

    // Drain outbox cascade across rounds until MAX_EVENT_DEPTH stops the ping-pong loop.
    for _ in 0..8 {
        let _ = mgr.dispatch_outbox()?;
    }
    assert_eq!(mgr.dispatch_outbox()?, 0);

    // Producer never received its own depth-0 event; every event it got was from consumer.
    let producer_hits = env
        .store
        .plugin_kv_get("com.test.producer", "echoHits")?
        .unwrap_or_default();
    let p_arr: Vec<JsonValue> = serde_json::from_str(&producer_hits)?;
    assert!(!p_arr.is_empty());
    assert!(
        p_arr
            .iter()
            .all(|ev| ev["originPlugin"].as_str() != Some("com.test.producer"))
    );

    let consumer_hits = env
        .store
        .plugin_kv_get("com.test.consumer", "seen")?
        .unwrap_or_default();
    let c_arr: Vec<JsonValue> = serde_json::from_str(&consumer_hits)?;
    assert!(!c_arr.is_empty());
    assert!(
        c_arr
            .iter()
            .all(|ev| ev["originPlugin"].as_str() != Some("com.test.consumer"))
    );
    assert!(
        c_arr
            .iter()
            .chain(p_arr.iter())
            .all(|ev| ev["depth"].as_i64().unwrap_or(99) < i64::from(MAX_EVENT_DEPTH))
    );

    mgr.shutdown();
    Ok(())
}

#[test]
fn interrupt_timeout_circuit_breaker_and_recovery() -> Result<()> {
    let env = TestEnv::new()?;
    env.write_plugin(
        "com.test.looper",
        r#"{
            "api_version": 1,
            "id": "com.test.looper",
            "name": "Looper",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["hooks:intercept", "storage"],
            "activation_events": ["onHook:beforeTaskCreate"]
        }"#,
        &[(
            "index.js",
            r#"
            import { hooks, storage } from "fern";
            export function activate() {
                let count = storage.get("calls") ?? 0;
                hooks.beforeTaskCreate(() => {
                    count += 1;
                    storage.set("calls", count);
                    if (count === 1) {
                        while (true) {}
                    }
                    throw new Error("boom " + count);
                });
            }
            "#,
        )],
    )?;

    let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.looper")?;

    // 1st call: while(true) is interrupted around the 2s hook deadline.
    let t0 = Instant::now();
    let d1 = mgr.run_before_task_create(json!({ "title": "A" }))?;
    let elapsed = t0.elapsed();
    assert_eq!(d1, HookDecision::Allow);
    assert!(
        elapsed >= Duration::from_millis(1500) && elapsed < Duration::from_secs(6),
        "unexpected interrupt duration: {elapsed:?}"
    );

    // 2nd and 3rd calls throw JS errors -> trips Circuit Breaker at threshold = 3.
    let _ = mgr.run_before_task_create(json!({ "title": "B" }))?;
    let _ = mgr.run_before_task_create(json!({ "title": "C" }))?;

    let infos = mgr.plugins()?;
    let info = infos
        .iter()
        .find(|p| p.manifest.id == "com.test.looper")
        .ok_or_else(|| anyhow!("missing plugin info"))?;
    assert!(!info.enabled);
    assert!(matches!(info.status, PluginStatus::CircuitBroken { .. }));

    let db_state = env
        .store
        .plugin_state("com.test.looper")?
        .ok_or_else(|| anyhow!("missing db state"))?;
    assert!(!db_state.enabled);
    assert_eq!(db_state.consecutive_errors, CIRCUIT_BREAKER_THRESHOLD);

    mgr.reset_and_enable_plugin("com.test.looper")?;
    let after = mgr.plugins()?;
    let recovered = after
        .iter()
        .find(|p| p.manifest.id == "com.test.looper")
        .ok_or_else(|| anyhow!("missing recovered plugin"))?;
    assert!(recovered.enabled);
    assert!(!matches!(recovered.status, PluginStatus::CircuitBroken { .. }));

    mgr.shutdown();
    Ok(())
}

#[test]
fn sandboxed_imports_ui_closure_handlers_and_async_prompt() -> Result<()> {
    let env = TestEnv::new()?;
    fs::write(env._tmp.path().join("outside.js"), "export const secret = 1;")?;

    env.write_plugin(
        "com.test.escape",
        r#"{
            "api_version": 1,
            "id": "com.test.escape",
            "name": "Escape",
            "version": "1.0.0",
            "entry": "index.js",
            "activation_events": ["onCommand:noop"],
            "contributions": { "commands": [{ "id": "noop", "title": "Noop" }] }
        }"#,
        &[(
            "index.js",
            r#"
            import { secret } from "../outside.js";
            export function activate() { return secret; }
            "#,
        )],
    )?;

    env.write_plugin(
        "com.test.panel",
        r#"{
            "api_version": 1,
            "id": "com.test.panel",
            "name": "Panel",
            "version": "1.0.0",
            "entry": "index.js",
            "activation_events": ["onCommand:confirmTest"],
            "contributions": {
                "sidebar": [{ "id": "main", "title": "Panel", "icon": "icon.svg" }],
                "commands": [{ "id": "confirmTest", "title": "Confirm" }]
            }
        }"#,
        &[
            ("icon.svg", r#"<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6"/></svg>"#),
            ("helper.js", r#"export const prefix = "Count";"#),
            (
                "index.js",
                r#"
                import { views, ui, commands } from "fern";
                import { vstack, heading, button, svg } from "fern:ui";
                import { prefix } from "./helper.js";

                export function activate() {
                    let count = 0;
                    views.register("main", {
                        render: () => vstack({ gap: 8 }, [
                            heading(`${prefix}: ${count}`),
                            svg({ content: '<svg viewBox="0 0 10 10"><script>alert(1)</script><circle cx="5" cy="5" r="4"/></svg>' }),
                            button({
                                label: "Increment",
                                onClick: () => { count += 1; }
                            })
                        ])
                    });
                    commands.register("confirmTest", async () => {
                        return await ui.confirm("Proceed?", "Confirm action");
                    });
                }
                "#,
            ),
        ],
    )?;

    let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.escape")?;
    mgr.approve_permissions("com.test.panel")?;

    // Path traversal import fails cleanly at activation time.
    let escape_err = mgr.run_command("com.test.escape", "noop", None).unwrap_err();
    let escape_msg = format!("{escape_err:#}");
    assert!(
        escape_msg.contains("forbidden") || escape_msg.contains("outside.js"),
        "unexpected escape error: {escape_msg}"
    );

    // Mount view -> captures HandlerId closure + sanitized SVG.
    let tree = mgr.mount_view("com.test.panel", "main")?;
    let handler_id = match &tree.root {
        UiNode::VStack { children, .. } => {
            assert!(matches!(&children[0], UiNode::Heading { text, .. } if text == "Count: 0"));
            match &children[1] {
                UiNode::Svg { content, .. } => assert!(!content.contains("<script")),
                other => bail!("expected Svg node, got {other:?}"),
            }
            match &children[2] {
                UiNode::Button { on_click: Some(id), .. } => *id,
                other => bail!("expected Button with on_click, got {other:?}"),
            }
        }
        other => bail!("expected VStack root, got {other:?}"),
    };

    // Dispatch click -> runs closure -> auto re-renders with updated state.
    let rerendered = mgr
        .dispatch_ui_event(
            "com.test.panel",
            UiEvent::click("main", handler_id),
        )?
        .ok_or_else(|| anyhow!("expected rerendered tree"))?;
    match &rerendered.root {
        UiNode::VStack { children, .. } => {
            assert!(matches!(&children[0], UiNode::Heading { text, .. } if text == "Count: 1"));
        }
        other => bail!("expected VStack root, got {other:?}"),
    }

    // Async confirm prompt resolves from host thread while command awaits.
    let mgr_c = mgr.clone();
    let cmd_thread = std::thread::spawn(move || {
        mgr_c.run_command("com.test.panel", "confirmTest", None)
    });
    let req_id = loop {
        match rx.recv_timeout(Duration::from_secs(3))? {
            HostUpdate::UiEffect(PluginUiEffect::Confirm { request_id, .. }) => break request_id,
            _ => continue,
        }
    };
    mgr.resolve_prompt(req_id, PromptReplyValue::Confirm(true))?;
    let confirmed = cmd_thread
        .join()
        .map_err(|_| anyhow!("command thread panicked"))??;
    assert_eq!(confirmed, JsonValue::Bool(true));

    mgr.shutdown();
    Ok(())
}

#[test]
fn script_runner_vault_scrubbing_scaffold_and_fast_shutdown() -> Result<()> {
    let env = TestEnv::new()?;
    let sample_dir = generate_sample_plugin(&env.plugins_dir, "com.test.sample", "Sample")?;
    assert!(sample_dir.join("manifest.json").is_file());
    assert!(sample_dir.join("fern.d.ts").is_file());
    assert!(sample_dir.join("jsconfig.json").is_file());

    let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.sample")?;
    let cmd_res = mgr.run_command("com.test.sample", "quick_note", None)?;
    assert!(cmd_res.as_str().is_some());

    // Create a draft task and run ScriptRunner with a secret stored for `script:s1`.
    let _ = env.store.add_task("Seed Task")?;
    mgr.set_secret("script:s1", "api_key", "sk_live_topsecret_99")?;

    let script = UserScript {
        id: "s1".to_owned(),
        name: "Scrubbed Script".to_owned(),
        trigger_kind: "manual".to_owned(),
        trigger_arg: String::new(),
        permissions: r#"["tasks:read","tasks:write"]"#.to_owned(),
        code: r#"
            console.log("token=sk_live_topsecret_99");
            const t = fern.tasks.create({ title: "Script Task" });
            return { createdTitle: t.title, count: fern.tasks.list().length };
        "#
        .to_owned(),
        enabled: true,
        updated_at: 0,
    };
    let run_res = mgr.test_run_script(script, None)?;
    assert!(run_res.ok, "script failed: {:?}", run_res.error);
    assert!(run_res.data_changed);
    assert_eq!(
        run_res.return_value,
        Some(json!({ "createdTitle": "Script Task", "count": 3 }))
    );
    assert!(
        run_res
            .logs
            .iter()
            .any(|l| l.message.contains("***") && !l.message.contains("sk_live_topsecret_99")),
        "expected scrubbed secret in logs: {:?}",
        run_res.logs
    );

    let shutdown_start = Instant::now();
    mgr.shutdown();
    let shutdown_elapsed = shutdown_start.elapsed();
    assert!(
        shutdown_elapsed <= SHUTDOWN_BUDGET,
        "shutdown exceeded 300ms budget: {shutdown_elapsed:?}"
    );
    Ok(())
}

#[test]
fn view_refresh_from_settings_change_and_async_click_never_panics() -> Result<()> {
    let env = TestEnv::new()?;
    env.write_plugin(
        "com.test.refresh",
        r#"{
            "id": "com.test.refresh",
            "name": "Refresh Test",
            "version": "0.1.0",
            "api_version": 1,
            "entry": "index.js",
            "permissions": ["storage"],
            "settings": [
                { "key": "client_id", "type": "text", "label": "Client ID", "default": "" },
                { "key": "client_secret", "type": "secret", "label": "Client Secret" }
            ],
            "contributions": {
                "sidebar": [{ "id": "sync", "title": "Sync" }]
            }
        }"#,
        &[(
            "index.js",
            r#"
            import fern from "fern";
            import * as ui from "fern:ui";

            let inProgress = false;
            let done = false;

            export function activate() {
                fern.views.register("sync", {
                    render() {
                        const cid = String(fern.settings.get("client_id") || "");
                        const hasSec = fern.settings.hasSecret("client_secret");
                        const status = done
                            ? "done"
                            : inProgress
                              ? "working"
                              : cid && hasSec
                                ? "ready"
                                : "setup";
                        return ui.vstack([
                            ui.heading(status, 1),
                            ui.button({
                                label: "Go",
                                onClick: async () => {
                                    inProgress = true;
                                    fern.views.refresh("sync");
                                    const ok = await fern.ui.confirm("Go?", "Proceed");
                                    inProgress = false;
                                    done = Boolean(ok);
                                    fern.views.refresh("sync");
                                }
                            })
                        ]);
                    }
                });
                fern.settings.onChange(() => {
                    fern.views.refresh("sync");
                });
            }
            "#,
        )],
    )?;

    let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.refresh")?;

    let extract_heading_and_btn = |tree: &UiTreePayload| -> Result<(String, HandlerId)> {
        match &tree.root {
            UiNode::VStack { children, .. } => {
                let heading = match &children[0] {
                    UiNode::Heading { text, .. } => text.clone(),
                    other => bail!("expected Heading, got {other:?}"),
                };
                let btn = match &children[1] {
                    UiNode::Button { on_click: Some(id), .. } => *id,
                    other => bail!("expected Button, got {other:?}"),
                };
                Ok((heading, btn))
            }
            other => bail!("expected VStack, got {other:?}"),
        }
    };

    // 1. Initially in setup state.
    let initial = mgr.mount_view("com.test.refresh", "sync")?;
    let (h0, _) = extract_heading_and_btn(&initial)?;
    assert_eq!(h0, "setup");

    // 2. Updating client_id triggers settings.onChange -> fern.views.refresh("sync") inside task poll.
    let mut settings = Map::new();
    settings.insert("client_id".to_owned(), JsonValue::String("cid-123".to_owned()));
    mgr.update_settings("com.test.refresh", settings)?;

    // 3. Saving secret also notifies settings.onChange -> view transitions to "ready" automatically.
    mgr.set_secret("com.test.refresh", "client_secret", "sec-xyz")?;
    let ready_tree = loop {
        match rx.recv_timeout(Duration::from_secs(3))? {
            HostUpdate::ViewRendered(p) if p.view_id == "sync" => {
                let (h, _) = extract_heading_and_btn(&p)?;
                if h == "ready" {
                    break p;
                }
            }
            HostUpdate::ViewError { error, .. } => bail!("unexpected view error: {error}"),
            _ => continue,
        }
    };
    let (_, btn_id) = extract_heading_and_btn(&ready_tree)?;

    // 4. Async onClick calls fern.views.refresh("sync") BEFORE awaiting fern.ui.confirm(...).
    let mgr_c = mgr.clone();
    let click_thread = std::thread::spawn(move || {
        mgr_c.dispatch_ui_event("com.test.refresh", UiEvent::click("sync", btn_id))
    });

    let mut saw_working = false;
    let mut prompt_id = None;
    let deadline = Instant::now() + Duration::from_secs(3);
    while (!saw_working || prompt_id.is_none()) && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(HostUpdate::ViewRendered(p)) if p.view_id == "sync" => {
                let (h, _) = extract_heading_and_btn(&p)?;
                if h == "working" {
                    saw_working = true;
                }
            }
            Ok(HostUpdate::UiEffect(PluginUiEffect::Confirm { request_id, .. })) => {
                prompt_id = Some(request_id);
            }
            Ok(HostUpdate::ViewError { error, .. }) => bail!("unexpected view error: {error}"),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(saw_working, "expected mid-async fern.views.refresh to render 'working'");
    let req_id = prompt_id.ok_or_else(|| anyhow!("expected confirm prompt"))?;

    mgr.resolve_prompt(req_id, PromptReplyValue::Confirm(true))?;
    let final_tree = click_thread
        .join()
        .map_err(|_| anyhow!("click thread panicked"))??
        .ok_or_else(|| anyhow!("expected final rerendered tree"))?;
    let (h_final, _) = extract_heading_and_btn(&final_tree)?;
    assert_eq!(h_final, "done");

    mgr.shutdown();
    Ok(())
}

#[test]
fn c2_purge_plugin_data_wipes_all_state_and_manifest_permission_shrink() -> Result<()> {
    let env = TestEnv::new()?;
    env.write_plugin(
        "com.test.purge",
        r#"{
            "api_version": 1,
            "id": "com.test.purge",
            "name": "Purge",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["tasks:read", "storage", "meta"],
            "activation_events": ["onCommand:seed"],
            "contributions": { "commands": [{ "id": "seed", "title": "Seed" }] }
        }"#,
        &[(
            "index.js",
            r#"
            import { storage, meta, commands } from "fern";
            export function activate() {
                commands.register("seed", () => {
                    storage.set("k1", { v: 42 });
                    meta.set("task", "00000000-0000-0000-0000-000000000001", "sync", "ok");
                    return true;
                });
            }
            "#,
        )],
    )?;

    env.write_plugin(
        "com.test.shrink",
        r#"{
            "api_version": 1,
            "id": "com.test.shrink",
            "name": "Shrink",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["tasks:read", "storage"],
            "activation_events": []
        }"#,
        &[("index.js", "export function activate() {}")],
    )?;

    let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.purge")?;
    mgr.approve_permissions("com.test.shrink")?;

    mgr.set_secret("com.test.purge", "api_token", "top_secret_val")?;
    mgr.run_command("com.test.purge", "seed", None)?;

    assert!(env.store.plugin_state("com.test.purge")?.is_some());
    assert!(env.store.plugin_kv_get("com.test.purge", "k1")?.is_some());
    assert!(!env.store.plugin_meta_list("com.test.purge", "task", "00000000-0000-0000-0000-000000000001")?.is_empty());

    // Drain prior updates before purging.
    while rx.try_recv().is_ok() {}
    mgr.purge_plugin_data("com.test.purge")?;

    assert!(env.store.plugin_state("com.test.purge")?.is_none());
    assert!(env.store.plugin_kv_get("com.test.purge", "k1")?.is_none());
    assert!(env.store.plugin_meta_list("com.test.purge", "task", "00000000-0000-0000-0000-000000000001")?.is_empty());
    let vault_check = SecretsVault::open(&env.vault_path, &env.key_path)?;
    assert!(!vault_check.has("com.test.purge", "api_token")?);
    let mut saw_registry_changed = false;
    while let Ok(upd) = rx.try_recv() {
        if let HostUpdate::RegistryChanged(list) = upd
            && !list.iter().any(|p| p.manifest.id == "com.test.purge")
        {
            saw_registry_changed = true;
        }
    }
    assert!(saw_registry_changed, "expected RegistryChanged after purge_plugin_data");

    // Manifest drops `"storage"` -> persisted granted_permissions shrinks to `["tasks:read"]`.
    fs::write(
        env.plugins_dir.join("com.test.shrink/manifest.json"),
        r#"{
            "api_version": 1,
            "id": "com.test.shrink",
            "name": "Shrink",
            "version": "1.0.1",
            "entry": "index.js",
            "permissions": ["tasks:read"],
            "activation_events": []
        }"#,
    )?;
    mgr.reload_plugin("com.test.shrink")?;
    let state_after_drop = env
        .store
        .plugin_state("com.test.shrink")?
        .ok_or_else(|| anyhow!("missing shrink state"))?;
    assert!(!state_after_drop.granted_permissions.contains("storage"));

    // Re-adding `"storage"` later must require re-approval (`NeedsApproval`).
    fs::write(
        env.plugins_dir.join("com.test.shrink/manifest.json"),
        r#"{
            "api_version": 1,
            "id": "com.test.shrink",
            "name": "Shrink",
            "version": "1.0.2",
            "entry": "index.js",
            "permissions": ["tasks:read", "storage"],
            "activation_events": []
        }"#,
    )?;
    mgr.reload_plugin("com.test.shrink")?;
    let info = mgr
        .plugins()?
        .into_iter()
        .find(|p| p.manifest.id == "com.test.shrink")
        .ok_or_else(|| anyhow!("missing shrink info"))?;
    assert!(!info.enabled);
    assert!(matches!(
        info.status,
        PluginStatus::NeedsApproval { ref added_permissions, is_first_install: false }
            if added_permissions.contains(&Permission::Storage)
    ));

    mgr.shutdown();
    Ok(())
}

#[test]
fn c3_gated_async_driver_interrupts_detached_loop_and_never_clobbers_confirm_wakeup() -> Result<()> {
    let env = TestEnv::new()?;
    env.write_plugin(
        "com.test.async_c3",
        r#"{
            "api_version": 1,
            "id": "com.test.async_c3",
            "name": "AsyncC3",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": [],
            "activation_events": ["onCommand:waitConfirm", "onCommand:ping", "onCommand:detachedLoop"],
            "contributions": {
                "sidebar": [{ "id": "panel", "title": "Panel" }],
                "commands": [
                    { "id": "waitConfirm", "title": "Wait" },
                    { "id": "ping", "title": "Ping" },
                    { "id": "detachedLoop", "title": "DetachedLoop" }
                ]
            }
        }"#,
        &[(
            "index.js",
            r#"
            import { commands, ui, views } from "fern";
            import { heading } from "fern:ui";
            let renders = 0;
            export function activate() {
                views.register("panel", {
                    render() {
                        renders += 1;
                        return heading("Render " + renders);
                    }
                });
                commands.register("ping", () => "pong:" + renders);
                commands.register("waitConfirm", async () => {
                    const ok = await ui.confirm("Hold", "Waiting");
                    return { ok, renders };
                });
                commands.register("detachedLoop", () => {
                    Promise.resolve().then(() => { while (true) {} });
                    return "scheduled";
                });
            }
            "#,
        )],
    )?;

    let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.async_c3")?;

    // 1. Start an async command awaiting `fern.ui.confirm`.
    let mgr_c = mgr.clone();
    let wait_thread = std::thread::spawn(move || {
        mgr_c.run_command("com.test.async_c3", "waitConfirm", None)
    });
    let req_id = loop {
        match rx.recv_timeout(Duration::from_secs(3))? {
            HostUpdate::UiEffect(PluginUiEffect::Confirm { request_id, .. }) => break request_id,
            _ => continue,
        }
    };

    // 2. While `waitConfirm` is suspended, run multiple concurrent view renders & commands
    //    that used to overwrite QuickJS's single `AtomicWaker` and wedge `waitConfirm`.
    for _ in 0..3 {
        let _ = mgr.render_view("com.test.async_c3", "panel")?;
        assert!(mgr.run_command("com.test.async_c3", "ping", None)?.as_str().unwrap_or("").starts_with("pong:"));
    }

    // 3. Resolve confirm -> `waitConfirm` must wake immediately and complete cleanly.
    mgr.resolve_prompt(req_id, PromptReplyValue::Confirm(true))?;
    let confirm_res = wait_thread
        .join()
        .map_err(|_| anyhow!("wait thread panicked"))??;
    assert_eq!(confirm_res["ok"], JsonValue::Bool(true));

    // 4. Detached microtask infinite loop must be interrupted by ExecGate and recorded as failure.
    let sched = mgr.run_command("com.test.async_c3", "detachedLoop", None)?;
    assert_eq!(sched, json!("scheduled"));
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut saw_failure = false;
    while Instant::now() < deadline {
        if let Ok(HostUpdate::PluginStatusChanged {
            plugin_id,
            status,
            metrics,
        }) = rx.recv_timeout(Duration::from_millis(250))
            && plugin_id == "com.test.async_c3"
            && metrics.consecutive_errors >= 1
            && matches!(
                status,
                PluginStatus::Error(_) | PluginStatus::CircuitBroken { .. }
            )
        {
            saw_failure = true;
            break;
        }
    }
    assert!(saw_failure, "expected detached microtask loop to trip ExecGate and record_failure");

    mgr.shutdown();
    Ok(())
}

#[test]
fn c4_concurrent_activation_deduplicated_and_timer_unload_drops_cleanly() -> Result<()> {
    let env = TestEnv::new()?;
    env.write_plugin(
        "com.test.dedup",
        r#"{
            "api_version": 1,
            "id": "com.test.dedup",
            "name": "Dedup",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["storage"],
            "activation_events": ["onCommand:id"],
            "contributions": { "commands": [{ "id": "id", "title": "ID" }] }
        }"#,
        &[(
            "index.js",
            r#"
            import { commands, storage, ui, timers } from "fern";
            export async function activate() {
                const n = (storage.get("actCount") ?? 0) + 1;
                storage.set("actCount", n);
                await ui.confirm("Activate?", "Once");
                timers.setInterval("heartbeat", 10000, () => {});
                commands.register("id", () => storage.get("actCount"));
            }
            "#,
        )],
    )?;

    let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.dedup")?;

    // Launch two concurrent commands before the plugin is activated.
    let m1 = mgr.clone();
    let m2 = mgr.clone();
    let t1 = std::thread::spawn(move || m1.run_command("com.test.dedup", "id", None));
    let t2 = std::thread::spawn(move || m2.run_command("com.test.dedup", "id", None));

    let req_id = loop {
        match rx.recv_timeout(Duration::from_secs(3))? {
            HostUpdate::UiEffect(PluginUiEffect::Confirm { request_id, .. }) => break request_id,
            _ => continue,
        }
    };
    // Wait briefly to confirm no duplicate activation confirm arrives.
    std::thread::sleep(Duration::from_millis(80));
    let mut extra_confirms = 0usize;
    while let Ok(upd) = rx.try_recv() {
        if matches!(upd, HostUpdate::UiEffect(PluginUiEffect::Confirm { .. })) {
            extra_confirms += 1;
        }
    }
    assert_eq!(extra_confirms, 0, "concurrent ensure_activated must run activate() only once");

    mgr.resolve_prompt(req_id, PromptReplyValue::Confirm(true))?;
    let r1 = t1.join().map_err(|_| anyhow!("t1 panicked"))??;
    let r2 = t2.join().map_err(|_| anyhow!("t2 panicked"))??;
    assert_eq!(r1, json!(1));
    assert_eq!(r2, json!(1));

    // Let the timer tick once, then disable and reload to exercise Persistent drop order.
    std::thread::sleep(Duration::from_millis(250));
    mgr.set_plugin_enabled("com.test.dedup", false)?;
    mgr.reload_plugin("com.test.dedup")?;
    mgr.shutdown();
    Ok(())
}

#[test]
fn c5_concurrent_calls_during_shutdown_never_deadlock() -> Result<()> {
    let env = TestEnv::new()?;
    let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
    let mut handles = Vec::new();
    for _ in 0..6 {
        let m = mgr.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..50 {
                if m.plugins().is_err() {
                    break;
                }
            }
        }));
    }
    std::thread::sleep(Duration::from_millis(5));
    mgr.shutdown();
    for h in handles {
        h.join().map_err(|_| anyhow!("caller thread panicked"))?;
    }
    assert!(mgr.plugins().is_err(), "calls after shutdown must fail immediately");
    Ok(())
}

#[test]
fn c6_timer_interval_cap_and_js_to_json_sparse_array_and_dag_guards() -> Result<()> {
    let env = TestEnv::new()?;
    env.write_plugin(
        "com.test.dos",
        r#"{
            "api_version": 1,
            "id": "com.test.dos",
            "name": "DoS",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["storage"],
            "activation_events": ["onCommand:testAll"],
            "contributions": { "commands": [{ "id": "testAll", "title": "TestAll" }] }
        }"#,
        &[(
            "index.js",
            r#"
            import { commands, timers, storage, ui } from "fern";
            export function activate() {
                commands.register("testAll", () => {
                    const results = {};
                    try {
                        timers.setInterval("huge", Number.MAX_SAFE_INTEGER, () => {});
                        results.timerHuge = "unexpected_ok";
                    } catch (e) {
                        results.timerHuge = String(e.message || e);
                    }
                    try {
                        timers.setInterval("nan", NaN, () => {});
                        results.timerNan = "unexpected_ok";
                    } catch (e) {
                        results.timerNan = String(e.message || e);
                    }
                    try {
                        const sparse = new Array(2000000000);
                        storage.set("sparse", sparse);
                        results.sparse = "unexpected_ok";
                    } catch (e) {
                        results.sparse = String(e.message || e);
                    }
                    try {
                        let dag = { leaf: 1 };
                        for (let i = 0; i < 24; i++) {
                            dag = { l: dag, r: dag };
                        }
                        storage.set("dag", dag);
                        results.dag = "unexpected_ok";
                    } catch (e) {
                        results.dag = String(e.message || e);
                    }
                    try {
                        let uiDag = { type: "text", text: "x" };
                        for (let i = 0; i < 24; i++) {
                            uiDag = { type: "vstack", children: [uiDag, uiDag] };
                        }
                        ui.openModal("DoS", uiDag);
                        results.uiDag = "unexpected_ok";
                    } catch (e) {
                        results.uiDag = String(e.message || e);
                    }
                    return results;
                });
            }
            "#,
        )],
    )?;

    let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.dos")?;
    let res = mgr.run_command("com.test.dos", "testAll", None)?;
    assert_ne!(res["timerHuge"], json!("unexpected_ok"));
    assert_ne!(res["timerNan"], json!("unexpected_ok"));
    assert_ne!(res["sparse"], json!("unexpected_ok"));
    assert_ne!(res["dag"], json!("unexpected_ok"));
    assert_ne!(res["uiDag"], json!("unexpected_ok"));

    mgr.shutdown();
    Ok(())
}

#[test]
fn h1_event_permission_enforced_for_wildcard_plugin_and_user_script() -> Result<()> {
    let env = TestEnv::new()?;
    // Wildcard subscriber WITHOUT `tasks:read`
    env.write_plugin(
        "com.test.noperm",
        r#"{
            "api_version": 1,
            "id": "com.test.noperm",
            "name": "NoPerm",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["storage"],
            "activation_events": ["onStartup"]
        }"#,
        &[(
            "index.js",
            r#"
            import { events, storage } from "fern";
            export function activate() {
                events.on("*", (ev) => {
                    const arr = storage.get("events") ?? [];
                    arr.push(ev.kind);
                    storage.set("events", arr);
                });
            }
            "#,
        )],
    )?;
    // Wildcard subscriber WITH `tasks:read`
    env.write_plugin(
        "com.test.withperm",
        r#"{
            "api_version": 1,
            "id": "com.test.withperm",
            "name": "WithPerm",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["tasks:read", "storage"],
            "activation_events": ["onStartup"]
        }"#,
        &[(
            "index.js",
            r#"
            import { events, storage } from "fern";
            export function activate() {
                events.on("*", (ev) => {
                    const arr = storage.get("events") ?? [];
                    arr.push(ev.kind);
                    storage.set("events", arr);
                });
            }
            "#,
        )],
    )?;

    // Event-triggered UserScript WITHOUT `tasks:read` (only `countdowns:write`).
    env.store.save_user_script(&UserScript {
        id: "ev_noperm".to_owned(),
        name: "NoPerm Script".to_owned(),
        trigger_kind: "event".to_owned(),
        trigger_arg: "*".to_owned(),
        permissions: r#"["countdowns:write"]"#.to_owned(),
        code: r#"fern.countdowns.create({ name: "Leaked", date: "2027-01-01" });"#.to_owned(),
        enabled: true,
        updated_at: 0,
    })?;

    let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.noperm")?;
    mgr.approve_permissions("com.test.withperm")?;
    // Give onStartup activations a brief moment to complete.
    std::thread::sleep(Duration::from_millis(120));

    let _ = env.store.add_task("Sensitive Task")?;
    let _ = mgr.dispatch_outbox()?;

    assert!(
        env.store.plugin_kv_get("com.test.noperm", "events")?.is_none(),
        "plugin without tasks:read must not receive task.created via events.on('*')"
    );
    assert!(
        env.store.countdowns()?.is_empty(),
        "event UserScript without tasks:read must not run on task.created"
    );
    let permitted = env
        .store
        .plugin_kv_get("com.test.withperm", "events")?
        .unwrap_or_default();
    assert!(
        permitted.contains("task.created"),
        "plugin with tasks:read should receive task.created, got: {permitted}"
    );

    mgr.shutdown();
    Ok(())
}

#[test]
fn h2_h3_circuit_broken_escalation_requires_approval_and_reload_validation_no_spin() -> Result<()> {
    let env = TestEnv::new()?;
    env.write_plugin(
        "com.test.cb",
        r#"{
            "api_version": 1,
            "id": "com.test.cb",
            "name": "CB",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["storage"],
            "activation_events": ["onCommand:fail"],
            "contributions": { "commands": [{ "id": "fail", "title": "Fail" }] }
        }"#,
        &[(
            "index.js",
            r#"
            import { commands } from "fern";
            export function activate() {
                commands.register("fail", () => { throw new Error("kaboom"); });
            }
            "#,
        )],
    )?;

    let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.cb")?;

    // Trip circuit breaker (3 errors).
    for _ in 0..CIRCUIT_BREAKER_THRESHOLD {
        let _ = mgr.run_command("com.test.cb", "fail", None);
    }
    let tripped = mgr
        .plugins()?
        .into_iter()
        .find(|p| p.manifest.id == "com.test.cb")
        .ok_or_else(|| anyhow!("missing cb plugin"))?;
    assert!(matches!(tripped.status, PluginStatus::CircuitBroken { .. }));

    // Escalate manifest permissions on disk while circuit-broken.
    fs::write(
        env.plugins_dir.join("com.test.cb/manifest.json"),
        r#"{
            "api_version": 1,
            "id": "com.test.cb",
            "name": "CB",
            "version": "1.0.1",
            "entry": "index.js",
            "permissions": ["storage", "tasks:write"],
            "activation_events": ["onCommand:fail"],
            "contributions": { "commands": [{ "id": "fail", "title": "Fail" }] }
        }"#,
    )?;
    mgr.reload_plugin("com.test.cb")?;

    let after_reload = mgr
        .plugins()?
        .into_iter()
        .find(|p| p.manifest.id == "com.test.cb")
        .ok_or_else(|| anyhow!("missing cb plugin after reload"))?;
    assert!(
        matches!(after_reload.status, PluginStatus::NeedsApproval { .. }),
        "expected NeedsApproval before CircuitBroken, got {:?}",
        after_reload.status
    );
    assert!(
        mgr.reset_and_enable_plugin("com.test.cb").is_err(),
        "reset_and_enable must reject escalated manifest until re-approved"
    );

    // H3: invalid plugin id rejected.
    assert!(mgr.reload_plugin("../outside").is_err());

    // H3: mismatched manifest id fails, sets Error, and does not spin check_hot_reload.
    fs::write(
        env.plugins_dir.join("com.test.cb/manifest.json"),
        r#"{
            "api_version": 1,
            "id": "com.other.spoof",
            "name": "Spoof",
            "version": "1.0.2",
            "entry": "index.js"
        }"#,
    )?;
    assert!(mgr.reload_plugin("com.test.cb").is_err());
    let err_info = mgr
        .plugins()?
        .into_iter()
        .find(|p| p.manifest.id == "com.test.cb")
        .ok_or_else(|| anyhow!("missing cb plugin"))?;
    assert!(matches!(err_info.status, PluginStatus::Error(_)));
    assert!(mgr.check_hot_reload()?.is_empty(), "hot reload must not spin on unchanged broken manifest");

    // H3: malformed JSON manifest also updates snapshot and stops hot reload spin.
    fs::write(env.plugins_dir.join("com.test.cb/manifest.json"), "{ invalid json")?;
    assert!(mgr.reload_plugin("com.test.cb").is_err());
    assert!(mgr.check_hot_reload()?.is_empty());

    mgr.shutdown();
    Ok(())
}

#[test]
fn h9_h19_service_error_scrubbing_depth_guard_and_activation_logs() -> Result<()> {
    let env = TestEnv::new()?;
    env.write_plugin(
        "com.test.provider",
        r#"{
            "api_version": 1,
            "id": "com.test.provider",
            "name": "Provider",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["services"],
            "activation_events": ["onStartup"],
            "settings": [{ "key": "api_key", "type": "secret", "label": "Key" }]
        }"#,
        &[(
            "index.js",
            r#"
            import { services } from "fern";
            export function activate() {
                services.provide("prov.leak", () => {
                    throw new Error("failed with prov_secret_xyz_999");
                });
                services.provide("prov.recurse", async (n) => {
                    if (n <= 0) return "done";
                    return await services.call("prov.recurse", n - 1);
                });
            }
            "#,
        )],
    )?;

    env.write_plugin(
        "com.test.caller",
        r#"{
            "api_version": 1,
            "id": "com.test.caller",
            "name": "Caller",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["services"],
            "activation_events": ["onCommand:callLeak", "onCommand:callRec"],
            "contributions": {
                "commands": [
                    { "id": "callLeak", "title": "CallLeak" },
                    { "id": "callRec", "title": "CallRec" }
                ]
            }
        }"#,
        &[(
            "index.js",
            r#"
            import { commands, services } from "fern";
            export function activate() {
                commands.register("callLeak", async () => {
                    try {
                        await services.call("prov.leak", null);
                        return "unexpected_ok";
                    } catch (e) {
                        return String(e.message || e);
                    }
                });
                commands.register("callRec", async (arg) => {
                    return await services.call("prov.recurse", Number(arg || 0));
                });
            }
            "#,
        )],
    )?;

    env.write_plugin(
        "com.test.badact",
        r#"{
            "api_version": 1,
            "id": "com.test.badact",
            "name": "BadAct",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": [],
            "activation_events": ["onCommand:noop"],
            "settings": [{ "key": "tok", "type": "secret", "label": "Tok" }],
            "contributions": { "commands": [{ "id": "noop", "title": "Noop" }] }
        }"#,
        &[(
            "index.js",
            r#"
            export function activate() {
                console.log("before activation crash");
                throw new Error("leak act_secret_abc_777");
            }
            "#,
        )],
    )?;

    let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
    mgr.set_secret("com.test.provider", "api_key", "prov_secret_xyz_999")?;
    mgr.set_secret("com.test.badact", "tok", "act_secret_abc_777")?;
    mgr.approve_permissions("com.test.provider")?;
    mgr.approve_permissions("com.test.caller")?;
    mgr.approve_permissions("com.test.badact")?;
    std::thread::sleep(Duration::from_millis(120));

    // H9: provider secret is scrubbed before reaching caller plugin.
    let leak_msg = mgr
        .run_command("com.test.caller", "callLeak", None)?
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        leak_msg.contains("***") && !leak_msg.contains("prov_secret_xyz_999"),
        "provider secret was not scrubbed: {leak_msg}"
    );

    // H19: valid chain succeeds; deep recursion (> 4) fails cleanly (never hangs even if it
    // trips circuit breaker mid-recursion) and after reset/enable does not leak depth.
    assert_eq!(
        mgr.run_command("com.test.caller", "callRec", Some("2".to_owned()))?,
        json!("done")
    );
    assert!(mgr.run_command("com.test.caller", "callRec", Some("6".to_owned())).is_err());
    let _ = mgr.reset_and_enable_plugin("com.test.provider");
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(
        mgr.run_command("com.test.caller", "callRec", Some("2".to_owned()))?,
        json!("done")
    );

    // H9: activation error scrubbed + pre-failure console logs preserved.
    let act_err = mgr.run_command("com.test.badact", "noop", None).unwrap_err();
    let act_err_str = format!("{act_err:#}");
    assert!(
        act_err_str.contains("***") && !act_err_str.contains("act_secret_abc_777"),
        "activation secret was not scrubbed: {act_err_str}"
    );
    let logs = mgr.plugin_logs("com.test.badact")?;
    assert!(
        logs.iter().any(|l| l.message.contains("before activation crash")),
        "expected pre-failure logs preserved, got: {logs:?}"
    );

    mgr.shutdown();
    Ok(())
}

#[test]
fn h14_deactivate_executes_on_shutdown_and_looping_deactivate_respects_budget() -> Result<()> {
    let env = TestEnv::new()?;
    env.write_plugin(
        "com.test.deact_ok",
        r#"{
            "api_version": 1,
            "id": "com.test.deact_ok",
            "name": "DeactOk",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["storage"],
            "activation_events": ["onCommand:wake"],
            "contributions": { "commands": [{ "id": "wake", "title": "Wake" }] }
        }"#,
        &[(
            "index.js",
            r#"
            import { commands, storage } from "fern";
            export function activate() {
                commands.register("wake", () => true);
            }
            export function deactivate() {
                storage.set("clean_shutdown", "yes");
            }
            "#,
        )],
    )?;
    env.write_plugin(
        "com.test.deact_spin",
        r#"{
            "api_version": 1,
            "id": "com.test.deact_spin",
            "name": "DeactSpin",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": [],
            "activation_events": ["onCommand:wake"],
            "contributions": { "commands": [{ "id": "wake", "title": "Wake" }] }
        }"#,
        &[(
            "index.js",
            r#"
            import { commands } from "fern";
            export function activate() {
                commands.register("wake", () => true);
            }
            export function deactivate() {
                while (true) {}
            }
            "#,
        )],
    )?;

    let (mgr, _rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.deact_ok")?;
    mgr.approve_permissions("com.test.deact_spin")?;
    mgr.run_command("com.test.deact_ok", "wake", None)?;
    mgr.run_command("com.test.deact_spin", "wake", None)?;

    let t0 = Instant::now();
    mgr.shutdown();
    let elapsed = t0.elapsed();
    assert!(
        elapsed <= Duration::from_millis(450),
        "shutdown with looping deactivate exceeded budget: {elapsed:?}"
    );
    assert_eq!(
        env.store.plugin_kv_get("com.test.deact_ok", "clean_shutdown")?.as_deref(),
        Some("\"yes\""),
        "deactivate() must execute during shutdown"
    );
    Ok(())
}

#[test]
fn h15_h16_outbox_single_flight_over_100_rows_and_disable_cancels_inflight_prompt() -> Result<()> {
    let env = TestEnv::new()?;
    env.write_plugin(
        "com.test.outbox_big",
        r#"{
            "api_version": 1,
            "id": "com.test.outbox_big",
            "name": "OutboxBig",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["tasks:read", "storage"],
            "activation_events": ["onEvent:task.created", "onCommand:waitPrompt"],
            "contributions": { "commands": [{ "id": "waitPrompt", "title": "WaitPrompt" }] }
        }"#,
        &[(
            "index.js",
            r#"
            import { events, storage, commands, ui } from "fern";
            export function activate() {
                events.on("task.created", (ev) => {
                    const seen = storage.get("ids") ?? [];
                    seen.push(ev.entityId);
                    storage.set("ids", seen);
                });
                commands.register("waitPrompt", async () => {
                    return await ui.confirm("Hold", "Cancel on disable");
                });
            }
            "#,
        )],
    )?;

    let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.outbox_big")?;

    // H15: Insert 120 tasks (> 100 batch size) and dispatch concurrently from 2 threads.
    for i in 0..120 {
        let _ = env.store.add_task(&format!("Bulk Task {i}"))?;
    }
    let m1 = mgr.clone();
    let m2 = mgr.clone();
    let d1 = std::thread::spawn(move || m1.dispatch_outbox());
    let d2 = std::thread::spawn(move || m2.dispatch_outbox());
    let _ = d1.join().map_err(|_| anyhow!("d1 panicked"))??;
    let _ = d2.join().map_err(|_| anyhow!("d2 panicked"))??;
    assert_eq!(mgr.dispatch_outbox()?, 0);

    let raw_ids = env
        .store
        .plugin_kv_get("com.test.outbox_big", "ids")?
        .unwrap_or_default();
    let ids: Vec<String> = serde_json::from_str(&raw_ids)?;
    let unique: HashSet<&String> = ids.iter().collect();
    assert_eq!(ids.len(), 120, "expected all 120 outbox events drained without duplicates");
    assert_eq!(unique.len(), 120);

    // H16: Disabling plugin while `waitPrompt` awaits confirm cancels prompt and ignores stale completion.
    let m_wait = mgr.clone();
    let wait_thread = std::thread::spawn(move || {
        m_wait.run_command("com.test.outbox_big", "waitPrompt", None)
    });
    loop {
        match rx.recv_timeout(Duration::from_secs(3))? {
            HostUpdate::UiEffect(PluginUiEffect::Confirm { .. }) => break,
            _ => continue,
        }
    }
    mgr.set_plugin_enabled("com.test.outbox_big", false)?;
    let _ = wait_thread.join().map_err(|_| anyhow!("wait thread panicked"))?;

    let info = mgr
        .plugins()?
        .into_iter()
        .find(|p| p.manifest.id == "com.test.outbox_big")
        .ok_or_else(|| anyhow!("missing outbox_big info"))?;
    assert!(!info.enabled);
    assert!(matches!(info.status, PluginStatus::Disabled));
    assert_eq!(info.metrics.consecutive_errors, 0);

    mgr.shutdown();
    Ok(())
}

#[test]
fn h17_h18_safe_integers_focus_ui_revision_and_modal_handlers_and_scope_guard() -> Result<()> {
    let env = TestEnv::new()?;
    let _ = env.store.add_focus_session(
        None,
        "Deep Work",
        1_760_000_000_000,
        1_760_001_500_000,
    )?;

    env.write_plugin(
        "com.test.ui_int",
        r#"{
            "api_version": 1,
            "id": "com.test.ui_int",
            "name": "UiInt",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["focus:read"],
            "activation_events": ["onCommand:checkFocus", "onCommand:openStaticModal", "onCommand:openReactiveModal", "onCommand:badModal", "onCommand:capTest"],
            "contributions": {
                "sidebar": [{ "id": "main", "title": "Main" }],
                "commands": [
                    { "id": "checkFocus", "title": "CheckFocus" },
                    { "id": "openStaticModal", "title": "OpenStaticModal" },
                    { "id": "openReactiveModal", "title": "OpenReactiveModal" },
                    { "id": "badModal", "title": "BadModal" },
                    { "id": "capTest", "title": "CapTest" }
                ]
            }
        }"#,
        &[(
            "index.js",
            r#"
            import fern from "fern";
            import * as ui from "fern:ui";

            let staticClicks = 0;
            let dynCount = 0;

            export function activate() {
                fern.views.register("main", {
                    render() {
                        return ui.vstack([
                            ui.input({ key: "inp", value: "hi", revision: Date.now() }),
                            ui.heading("static=" + staticClicks + ",dyn=" + dynCount, 2),
                        ]);
                    }
                });
                fern.commands.register("checkFocus", () => {
                    const sessions = fern.focus.sessions({ from: 1750000000000, to: 1770000000000 });
                    const stats = fern.focus.stats({ from: 1750000000000 });
                    return { count: sessions.length, totalCount: stats.totalCount, staticClicks, dynCount };
                });
                fern.commands.register("openStaticModal", () => {
                    const prebuilt = ui.vstack([
                        ui.button({ label: "StaticBtn", onClick: () => { staticClicks += 1; } })
                    ]);
                    fern.ui.openModal("Static", prebuilt);
                    return true;
                });
                fern.commands.register("openReactiveModal", () => {
                    fern.ui.openModal("Reactive", () => ui.vstack([
                        ui.heading("n=" + dynCount, 2),
                        ui.button({ label: "Inc", onClick: () => { dynCount += 1; } })
                    ]));
                    return true;
                });
                fern.commands.register("badModal", () => {
                    try {
                        fern.ui.openModal("Bad", () => { throw new Error("modal render boom"); });
                    } catch (_e) {}
                    return true;
                });
                fern.commands.register("capTest", () => {
                    try {
                        for (let i = 0; i < 300; i++) {
                            ui.registerHandler(() => i);
                        }
                        return "unexpected_ok";
                    } catch (e) {
                        return String(e.message || e);
                    }
                });
            }
            "#,
        )],
    )?;

    let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
    mgr.approve_permissions("com.test.ui_int")?;

    // H17: Focus query with >i32::MAX epoch ms and UI `input({ revision: Date.now() })`.
    let focus_res = mgr.run_command("com.test.ui_int", "checkFocus", None)?;
    assert_eq!(focus_res["count"], json!(1));
    assert_eq!(focus_res["totalCount"], json!(1));

    let view_tree = mgr.mount_view("com.test.ui_int", "main")?;
    match &view_tree.root {
        UiNode::VStack { children, .. } => match &children[0] {
            UiNode::Input { revision, .. } => {
                assert!(*revision > u64::from(u32::MAX), "expected Date.now() revision, got {revision}");
            }
            other => bail!("expected Input, got {other:?}"),
        },
        other => bail!("expected VStack, got {other:?}"),
    }

    // H18: Pre-built UiNode passed to `openModal` dispatches click under `"__modal__"`.
    while rx.try_recv().is_ok() {}
    mgr.run_command("com.test.ui_int", "openStaticModal", None)?;
    let static_hid = loop {
        match rx.recv_timeout(Duration::from_secs(3))? {
            HostUpdate::UiEffect(PluginUiEffect::OpenModal { view_id, tree, .. }) => {
                assert_eq!(view_id, "__modal__");
                match tree {
                    UiNode::VStack { children, .. } => match &children[0] {
                        UiNode::Button { on_click: Some(hid), .. } => break *hid,
                        other => bail!("expected Button, got {other:?}"),
                    },
                    other => bail!("expected VStack, got {other:?}"),
                }
            }
            _ => continue,
        }
    };
    let _ = mgr.dispatch_ui_event("com.test.ui_int", UiEvent::click("__modal__", static_hid))?;
    let after_static = mgr.run_command("com.test.ui_int", "checkFocus", None)?;
    assert_eq!(after_static["staticClicks"], json!(1));

    // H18: Reactive modal re-renders on click and unmounts cleanly.
    while rx.try_recv().is_ok() {}
    mgr.run_command("com.test.ui_int", "openReactiveModal", None)?;
    let dyn_hid = loop {
        match rx.recv_timeout(Duration::from_secs(3))? {
            HostUpdate::UiEffect(PluginUiEffect::OpenModal { tree, .. }) => match tree {
                UiNode::VStack { children, .. } => match &children[1] {
                    UiNode::Button { on_click: Some(hid), .. } => break *hid,
                    other => bail!("expected Button, got {other:?}"),
                },
                other => bail!("expected VStack, got {other:?}"),
            },
            _ => continue,
        }
    };
    let rerendered_modal = mgr
        .dispatch_ui_event("com.test.ui_int", UiEvent::click("__modal__", dyn_hid))?
        .ok_or_else(|| anyhow!("expected rerendered reactive modal"))?;
    match &rerendered_modal.root {
        UiNode::VStack { children, .. } => {
            assert!(matches!(&children[0], UiNode::Heading { text, .. } if text == "n=1"));
        }
        other => bail!("expected VStack, got {other:?}"),
    }
    mgr.unmount_view("com.test.ui_int", "__modal__")?;

    // H18: Throwing inside openModal restores active_render_scope + capTest hits outside-render cap.
    mgr.run_command("com.test.ui_int", "badModal", None)?;
    let cap_msg = mgr.run_command("com.test.ui_int", "capTest", None)?;
    assert_ne!(cap_msg, json!("unexpected_ok"));

    mgr.shutdown();
    Ok(())
}

#[test]
fn medium_batch_savepoint_filters_scripts_startup_and_http_secret_scrubbing() -> Result<()> {
    let env = TestEnv::new()?;
    env.store.add_tag("work", None, None)?;
    env.store.add_tag("sub", None, Some("work"))?;
    let today = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();

    // Start a local HTTP echo server that reflects request Authorization header into body & headers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server_handle = std::thread::spawn(move || {
        use std::io::{Read, Write};
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let auth_line = req
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                .unwrap_or("Authorization: none")
                .to_owned();
            let body = serde_json::json!({ "echoed": auth_line }).to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Reflected: {auth_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    env.write_plugin(
        "com.test.medium",
        r#"{
            "api_version": 1,
            "id": "com.test.medium",
            "name": "Medium",
            "version": "1.0.0",
            "entry": "index.js",
            "permissions": ["tasks:read", "tasks:write", "lists:read", "lists:write", "storage", "network:local"],
            "activation_events": ["onStartup", "onCommand:runBatch", "onCommand:runFilters", "onCommand:runFetch"],
            "settings": [{ "key": "api_token", "type": "secret", "label": "Token" }],
            "contributions": {
                "commands": [
                    { "id": "runBatch", "title": "RunBatch" },
                    { "id": "runFilters", "title": "RunFilters" },
                    { "id": "runFetch", "title": "RunFetch" }
                ]
            }
        }"#,
        &[(
            "index.js",
            &format!(
                r#"
            import fern from "fern";
            export function activate() {{
                fern.storage.set("booted", true);
                fern.commands.register("runBatch", () => {{
                    const readOnly = fern.batch(() => fern.tasks.list().length);
                    let asyncRejected = false;
                    try {{
                        fern.batch(async () => 1);
                    }} catch (e) {{
                        asyncRejected = String(e.message || e).includes("synchronous");
                    }}
                    let rolledBack = false;
                    try {{
                        fern.batch(() => {{
                            fern.tasks.create({{ title: "OuterRollback" }});
                            throw new Error("abort outer");
                        }});
                    }} catch (_e) {{
                        rolledBack = true;
                    }}
                    fern.batch(() => {{
                        fern.tasks.create({{ title: "OuterCommit" }});
                        try {{
                            fern.batch(() => {{
                                fern.tasks.create({{ title: "InnerSavepointRollback" }});
                                throw new Error("abort inner");
                            }});
                        }} catch (_e) {{}}
                    }});
                    return {{ readOnly, asyncRejected, rolledBack }};
                }});
                fern.commands.register("runFilters", () => {{
                    const listA = fern.lists.create({{ name: "Work" }});
                    const listB = fern.lists.create({{ name: "Home" }});
                    fern.tasks.create({{ title: "Match", listId: listA.id, dueDate: "{today}", tags: ["sub"] }});
                    fern.tasks.create({{ title: "WrongList", listId: listB.id, dueDate: "{today}", tags: ["work"] }});
                    fern.tasks.create({{ title: "WrongTag", listId: listA.id, dueDate: "{today}", tags: ["other"] }});
                    fern.tasks.create({{ title: "WrongDate", listId: listA.id, dueDate: "2099-01-01", tags: ["work"] }});
                    return fern.tasks.list({{ view: "today", listId: listA.id, tag: "work" }}).map((t) => t.title);
                }});
                fern.commands.register("runFetch", async () => {{
                    const r = await fern.http.fetch("http://127.0.0.1:{port}/echo", {{
                        headers: {{ Authorization: "Bearer ${{secret:api_token}}" }}
                    }});
                    return {{
                        body: r.body,
                        text: r.text(),
                        json: r.json(),
                        reflectedHeader: r.headers["x-reflected"] || "",
                    }};
                }});
            }}
            "#
            ),
        )],
    )?;

    let (mgr, rx) = PluginManager::spawn(env.engine_config())?;
    mgr.set_secret("com.test.medium", "api_token", "sk_super_secret_12345")?;
    mgr.approve_permissions("com.test.medium")?;

    // M18: onStartup activates automatically after approve_permissions.
    let boot_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < boot_deadline {
        if env.store.plugin_kv_get("com.test.medium", "booted")?.as_deref() == Some("true") {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(
        env.store.plugin_kv_get("com.test.medium", "booted")?.as_deref(),
        Some("true"),
        "expected onStartup plugin to activate automatically after approve_permissions"
    );

    // M16: batch SAVEPOINT + Promise rejection + read-only / rollback data_dirty behavior.
    let batch_res = mgr.run_command("com.test.medium", "runBatch", None)?;
    assert_eq!(batch_res["asyncRejected"], json!(true));
    assert_eq!(batch_res["rolledBack"], json!(true));
    let titles: Vec<String> = env.store.tasks()?.into_iter().map(|t| t.title).collect();
    assert!(titles.iter().any(|t| t == "OuterCommit"));
    assert!(!titles.iter().any(|t| t == "OuterRollback"));
    assert!(!titles.iter().any(|t| t == "InnerSavepointRollback"));

    // Read-only batch must NOT emit DataChanged.
    while rx.try_recv().is_ok() {}
    let ro_script = UserScript {
        id: "ro_batch".to_owned(),
        name: "RO Batch".to_owned(),
        trigger_kind: "manual".to_owned(),
        trigger_arg: String::new(),
        permissions: r#"["tasks:read","tasks:write"]"#.to_owned(),
        code: r#"
            fern.batch(() => fern.tasks.list());
            try {
                fern.batch(() => {
                    fern.tasks.create("RolledBack");
                    throw new Error("nope");
                });
            } catch (_e) {}
            return "ok";
        "#
        .to_owned(),
        enabled: true,
        updated_at: 0,
    };
    let ro_res = mgr.test_run_script(ro_script, None)?;
    assert!(ro_res.ok, "ro_script failed: {:?}", ro_res.error);
    assert!(!ro_res.data_changed, "read-only/rolled-back batch must not mark data_changed");

    // M20: combined view + listId + tag (with child tag family) filter.
    let filtered = mgr.run_command("com.test.medium", "runFilters", None)?;
    assert_eq!(filtered, json!(["Match"]));

    // M9: reflected ${secret:...} scrubbed from body, text(), json(), and headers.
    let fetch_res = mgr.run_command("com.test.medium", "runFetch", None)?;
    let fetch_str = fetch_res.to_string();
    assert!(
        fetch_str.contains("***") && !fetch_str.contains("sk_super_secret_12345"),
        "expected reflected vault secret scrubbed from http response: {fetch_str}"
    );
    let _ = server_handle.join();

    // M21: UserScript sync `event` binding, `return;`, comment/string containing import/export,
    // and blocking relative filesystem imports.
    let sync_ev_script = UserScript {
        id: "sync_ev".to_owned(),
        name: "Sync Event".to_owned(),
        trigger_kind: "manual".to_owned(),
        trigger_arg: String::new(),
        permissions: "[]".to_owned(),
        code: r#"
            // import foo from "./bar.js"
            const note = "export default 123";
            if (!event || !event.val) { return; }
            return { got: event.val, note };
        "#
        .to_owned(),
        enabled: true,
        updated_at: 0,
    };
    let sync_res = mgr.test_run_script(sync_ev_script, Some(json!({ "val": 99 })))?;
    assert!(sync_res.ok, "sync event script failed: {:?}", sync_res.error);
    assert_eq!(
        sync_res.return_value,
        Some(json!({ "got": 99, "note": "export default 123" }))
    );

    let import_escape_script = UserScript {
        id: "imp_esc".to_owned(),
        name: "Import Escape".to_owned(),
        trigger_kind: "manual".to_owned(),
        trigger_arg: String::new(),
        permissions: "[]".to_owned(),
        code: r#"import * as m from "./com.test.medium/index.js"; export default m;"#.to_owned(),
        enabled: true,
        updated_at: 0,
    };
    let esc_res = mgr.test_run_script(import_escape_script, None)?;
    assert!(!esc_res.ok, "UserScript filesystem import must be blocked");

    mgr.shutdown();
    Ok(())
}
