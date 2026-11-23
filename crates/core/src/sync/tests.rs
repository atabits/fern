use super::*;
use crate::model::View;

/// Sends everything `from` changed since `since` to `to`, as a server would relay it.
fn relay(from: &Store, since: i64, to: &Store) -> i64 {
    let (changes, position) = from.changes_since(since).unwrap();
    let sent: Vec<Change> = changes.iter().map(|c| Change::from_json(&c.to_json()).unwrap()).collect();
    to.apply_changes(&sent).unwrap();
    position
}

#[test]
fn two_devices_meet() -> Result<()> {
    let laptop = Store::open_in_memory()?;
    let phone = Store::open_in_memory()?;

    let list = laptop.create_list("Home")?;
    let task = laptop.add_task("Fix the shelf")?;
    laptop.set_list(task.id, Some(list.id))?;
    laptop.set_tags(task.id, &["diy".to_owned()])?;
    laptop.add_checklist_item(task.id, "Buy screws", None)?;
    let sent = relay(&laptop, 0, &phone);

    let on_phone = phone.task(task.id)?.unwrap();
    assert_eq!((on_phone.title.as_str(), on_phone.list_id), ("Fix the shelf", Some(list.id)));
    assert_eq!(on_phone.tags, ["diy"]);
    assert_eq!(phone.checklist(task.id)?.len(), 1);
    // What was applied is not logged on the phone, so it is not sent back.
    assert!(phone.changes_since(0)?.0.is_empty());

    // An edit on the phone reaches the laptop; a deletion too.
    phone.set_title(task.id, "Fix the kitchen shelf")?;
    relay(&phone, 0, &laptop);
    assert_eq!(laptop.task(task.id)?.unwrap().title, "Fix the kitchen shelf");
    laptop.purge_task(task.id)?;
    relay(&laptop, sent, &phone);
    assert!(phone.task(task.id)?.is_none());
    assert!(phone.tasks_in(&View::Inbox, chrono::Local::now().date_naive())?.is_empty());
    Ok(())
}

#[test]
fn the_later_change_wins() -> Result<()> {
    let a = Store::open_in_memory()?;
    let b = Store::open_in_memory()?;
    let task = a.add_task("Draft")?;
    relay(&a, 0, &b);
    let start = b.sync_position()?;

    // Both edit; b's edit is newer, so a's older change does not overwrite it.
    let (mut older, _) = a.changes_since(0)?;
    a.set_title(task.id, "From a")?;
    b.set_title(task.id, "From b")?;
    older = older.into_iter().map(|mut c| { c.at = 1; c }).collect();
    b.apply_changes(&older)?;
    assert_eq!(b.task(task.id)?.unwrap().title, "From b");

    relay(&b, start, &a);
    assert_eq!(a.task(task.id)?.unwrap().title, "From b");
    Ok(())
}

#[test]
fn sync_settings_and_requests() -> Result<()> {
    let store = Store::open_in_memory()?;
    assert_eq!(store.sync_config()?, None);
    let config = SyncConfig {
        server: " https://fern.example.org ".into(),
        token: "t0ken".into(),
        ..Default::default()
    };
    assert!(config.is_configured());
    store.set_sync_config(&config)?;
    assert_eq!(store.sync_config()?.unwrap().server, "https://fern.example.org");
    let device = store.sync_device()?;
    assert_eq!(store.sync_device()?, device, "the device id stays");

    store.add_task("Water the plants")?;
    assert!(store.has_unsent_changes()?);
    let request = store.sync_request()?;
    assert_eq!(request.body["protocol"], SYNC_PROTOCOL_VERSION);
    assert_eq!(request.body["device"], device.as_str());
    assert_eq!(request.body["changes"].as_array().unwrap().len(), 1);

    // The server answers with nothing new: the change counts as sent.
    store.finish_sync(&request, &serde_json::json!({ "changes": [], "position": 7 }))?;
    assert!(!store.has_unsent_changes()?);
    assert_eq!(store.sync_request()?.body["since"], 7);
    assert!(store.finish_sync(&request, &serde_json::json!({ "oops": true })).is_err());

    // Another server starts over.
    store.set_sync_config(&SyncConfig {
        server: "https://other.example.org".into(),
        token: "x".into(),
        ..Default::default()
    })?;
    assert!(store.has_unsent_changes()?);
    Ok(())
}

#[test]
fn the_log_keeps_one_entry_per_row() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("Draft")?;
    for i in 0..20 {
        store.set_title(task.id, &format!("Draft {i}"))?;
    }
    let entries = |store: &Store| -> i64 { store.conn.query_row("SELECT COUNT(*) FROM sync_log", [], |r| r.get(0)).unwrap() };
    assert!(entries(&store) > 20);
    let (before, _) = store.changes_since(0)?;
    store.compact_sync_log()?;
    assert_eq!(entries(&store), 1);
    // What a sync sends is the same.
    let (after, position) = store.changes_since(0)?;
    assert_eq!(after, before);
    assert_eq!(position, store.sync_position()?);
    Ok(())
}

#[test]
fn attachments_and_their_files() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("fern-sync-files-{}", uuid::Uuid::now_v7()));
    let (laptop_dir, phone_dir) = (dir.join("laptop"), dir.join("phone"));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("plan.txt"), "plan")?;
    let laptop = Store::open_in_memory()?;
    let phone = Store::open_in_memory()?;
    let task = laptop.add_task("Trip")?;
    let attachment = laptop.add_attachment(task.id, &dir.join("plan.txt"), &laptop_dir)?;
    let file = attachment.path(&laptop_dir).file_name().unwrap().to_string_lossy().into_owned();

    // The laptop has a file to send; once sent, nothing.
    assert_eq!(laptop.files_to_sync(&laptop_dir)?, (vec![file.clone()], vec![]));
    laptop.mark_files_synced(std::slice::from_ref(&file))?;
    assert_eq!(laptop.files_to_sync(&laptop_dir)?, (vec![], vec![]));

    // The record reaches the phone, which then has a file to fetch.
    relay(&laptop, 0, &phone);
    assert_eq!(phone.attachments(task.id)?, [attachment]);
    assert_eq!(phone.files_to_sync(&phone_dir)?, (vec![], vec![file.clone()]));
    laptop.set_sync_config(&SyncConfig {
        server: "https://other.example.org".into(),
        token: "new".into(),
        ..Default::default()
    })?;
    assert_eq!(laptop.files_to_sync(&laptop_dir)?, (vec![file], vec![]));
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn focus_timers_travel() -> Result<()> {
    let laptop = Store::open_in_memory()?;
    let phone = Store::open_in_memory()?;
    let mut timer = crate::focus::FocusTimer::new("Reading");
    timer.icon = Some("📚".into());
    timer.minutes = 45;
    laptop.save_focus_timer(&timer)?;
    let sent = relay(&laptop, 0, &phone);
    assert_eq!(phone.focus_timers(false)?, [timer.clone()]);

    // Archived on the phone, then deleted on the laptop.
    phone.set_focus_timer_archived(timer.id, true)?;
    relay(&phone, 0, &laptop);
    assert!(laptop.focus_timers(false)?.is_empty() && laptop.focus_timers(true)?.len() == 1);
    laptop.delete_focus_timer(timer.id)?;
    relay(&laptop, sent, &phone);
    assert!(phone.focus_timers(true)?.is_empty());
    Ok(())
}

#[test]
fn matrix_rules_travel() -> Result<()> {
    use crate::matrix::QuadrantRule;
    let laptop = Store::open_in_memory()?;
    let phone = Store::open_in_memory()?;
    let rule = QuadrantRule { name: "Do now".into(), ..QuadrantRule::defaults()[0].clone() };
    laptop.save_matrix_rule(0, Some(&rule))?;
    let sent = relay(&laptop, 0, &phone);
    assert_eq!(phone.matrix_rules()?[0], rule);
    // "Restore" on the laptop brings the default back on the phone too.
    laptop.save_matrix_rule(0, None)?;
    relay(&laptop, sent, &phone);
    assert_eq!(phone.matrix_rules()?[0], QuadrantRule::defaults()[0]);
    Ok(())
}

#[test]
fn renamed_tags_travel() -> Result<()> {
    let laptop = Store::open_in_memory()?;
    let phone = Store::open_in_memory()?;
    let task = laptop.add_task("Run")?;
    laptop.set_tags(task.id, &["health".to_owned()])?;
    let sent = relay(&laptop, 0, &phone);
    laptop.update_tag("health", "fitness", None, None)?;
    relay(&laptop, sent, &phone);
    let names: Vec<String> = phone.tags()?.into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["fitness"], "the old name goes on the phone too");
    assert_eq!(phone.task(task.id)?.unwrap().tags, ["fitness"]);
    Ok(())
}

#[test]
fn list_folders_travel() -> Result<()> {
    let laptop = Store::open_in_memory()?;
    let phone = Store::open_in_memory()?;
    let work = laptop.add_list_folder("Work")?;
    let draft = crate::ListDraft { name: "Reports".into(), icon: Some("📊".into()), folder: Some(work.id), ..Default::default() };
    let reports = laptop.add_list(&draft)?;
    let sent = relay(&laptop, 0, &phone);
    assert_eq!(phone.list_folders()?, std::slice::from_ref(&work));
    assert_eq!(phone.lists()?[0], reports);
    // Ungrouping on the laptop takes the folder away on the phone and keeps the list.
    laptop.ungroup_list_folder(work.id)?;
    relay(&laptop, sent, &phone);
    assert!(phone.list_folders()?.is_empty());
    assert_eq!(phone.lists()?[0].folder, None);
    Ok(())
}

#[test]
fn countdown_pictures_travel() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("fern-sync-pictures-{}", uuid::Uuid::now_v7()));
    let (laptop_dir, phone_dir) = (dir.join("laptop"), dir.join("phone"));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("photo.jpg"), "jpeg")?;
    let laptop = Store::open_in_memory()?;
    let phone = Store::open_in_memory()?;
    let wedding = laptop.add_countdown("Wedding", chrono::NaiveDate::from_ymd_opt(2024, 8, 27).unwrap(), crate::CountdownKind::Anniversary)?;
    let file = laptop.set_countdown_background(wedding.id, Some(&dir.join("photo.jpg")), &laptop_dir)?.unwrap();
    assert_eq!(laptop.files_to_sync(&laptop_dir)?, (vec![file.clone()], vec![]));
    laptop.mark_files_synced(std::slice::from_ref(&file))?;

    // The phone gets the countdown, then has its picture to fetch.
    relay(&laptop, 0, &phone);
    assert_eq!(phone.files_to_sync(&phone_dir)?, (vec![], vec![file.clone()]));

    // A new picture is a new file, so it is sent again.
    let newer = laptop.set_countdown_background(wedding.id, Some(&dir.join("photo.jpg")), &laptop_dir)?.unwrap();
    assert_ne!(newer, file);
    assert_eq!(laptop.files_to_sync(&laptop_dir)?.0, [newer]);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn foreign_input_is_checked() {
    let store = Store::open_in_memory().unwrap();
    let unknown = Change { entity: "settings".into(), key: "theme".into(), at: 1, row: None };
    assert_eq!(store.apply_changes(&[unknown]).unwrap(), 0, "unknown/non-synced tables are safely skipped");
    let future = Change {
        entity: "plugin_kv".into(),
        key: "p1\x1fk".into(),
        at: 2,
        row: Some(Map::from_iter([("value".into(), Json::from("v"))])),
    };
    assert_eq!(store.apply_changes(&[future]).unwrap(), 0, "future synced entities are safely skipped");
    let mut row = Map::new();
    row.insert("id; DROP TABLE tasks".into(), Json::from("x"));
    row.insert("id".into(), Json::from("00000000-0000-0000-0000-000000000000"));
    row.insert("name".into(), Json::from("Odd"));
    row.insert("created_at".into(), Json::from(0));
    let odd = Change { entity: "lists".into(), key: "00000000-0000-0000-0000-000000000000".into(), at: 1, row: Some(row) };
    store.apply_changes(&[odd]).unwrap();
    assert_eq!(store.lists().unwrap().len(), 1, "odd column names are skipped");
    assert!(store.tasks().is_ok());
}

#[test]
fn stale_offline_edit_does_not_resurrect_over_newer_remote_edit() -> Result<()> {
    let a = Store::open_in_memory()?;
    let b = Store::open_in_memory()?;
    let c = Store::open_in_memory()?;

    let task = a.add_task("v1 from A")?;
    let key = task.id.to_string();
    // Normalize A's local edit timestamp to 1000.
    a.conn.execute("UPDATE sync_log SET at = 1000 WHERE entity = 'tasks' AND key = ?1", [&key])?;
    a.conn.execute("UPDATE sync_watermarks SET at = 1000 WHERE entity = 'tasks' AND key = ?1", [&key])?;

    // Relay initial task to B and C.
    relay(&a, 0, &b);
    relay(&a, 0, &c);

    // B edits at t=2000; A pulls B's newer edit.
    b.set_title(task.id, "v2 from B")?;
    let (mut b_changes, _) = b.changes_since(0)?;
    for ch in &mut b_changes {
        ch.at = 2000;
    }
    assert_eq!(a.apply_changes(&b_changes)?, 1);
    assert_eq!(a.task(task.id)?.unwrap().title, "v2 from B");
    // Applied remote edit must NOT be re-logged to sync_log.
    assert_eq!(a.changes_since(a.sync_position()?)?.0.len(), 0);

    // Offline device C delivers an older edit at t=1500 (newer than A's local t=1000, older than B's t=2000).
    c.set_title(task.id, "stale from C")?;
    let (mut c_changes, _) = c.changes_since(0)?;
    for ch in &mut c_changes {
        ch.at = 1500;
    }
    assert_eq!(a.apply_changes(&c_changes)?, 0);
    assert_eq!(a.task(task.id)?.unwrap().title, "v2 from B", "stale edit must not overwrite newer remote edit");

    // A genuinely newer edit at t=3000 still wins.
    for ch in &mut c_changes {
        ch.at = 3000;
    }
    assert_eq!(a.apply_changes(&c_changes)?, 1);
    assert_eq!(a.task(task.id)?.unwrap().title, "stale from C");
    Ok(())
}

#[test]
fn apply_changes_is_forward_compatible_with_future_columns_and_empty_rows() -> Result<()> {
    let store = Store::open_in_memory()?;
    let id = uuid::Uuid::now_v7().to_string();
    let mut row = Map::new();
    row.insert("id".into(), Json::from(id.clone()));
    row.insert("name".into(), Json::from("Future List"));
    row.insert("created_at".into(), Json::from(12345));
    row.insert("future_ai_priority_score".into(), Json::from(99));
    row.insert("another_v2_column".into(), Json::from("hello"));
    let future_change = Change { entity: "lists".into(), key: id, at: 500, row: Some(row) };

    // Empty row or row with only unknown columns is skipped cleanly without SQL errors.
    let empty_change = Change {
        entity: "lists".into(),
        key: uuid::Uuid::now_v7().to_string(),
        at: 500,
        row: Some(Map::new()),
    };
    let mut only_unknown = Map::new();
    only_unknown.insert("future_only".into(), Json::from(1));
    let unknown_only_change = Change {
        entity: "lists".into(),
        key: uuid::Uuid::now_v7().to_string(),
        at: 500,
        row: Some(only_unknown),
    };

    let applied = store.apply_changes(&[future_change, empty_change, unknown_only_change])?;
    assert_eq!(applied, 1);
    let lists = store.lists()?;
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].name, "Future List");
    Ok(())
}

#[test]
fn key_separator_in_tags_is_blocked_and_splitn_survives_legacy_rows() -> Result<()> {
    let laptop = Store::open_in_memory()?;
    let phone = Store::open_in_memory()?;

    // Direct tag creation / rename rejects U+001F and other ASCII control chars.
    assert!(laptop.add_tag("bad\u{1f}tag", None, None).is_err());
    assert!(laptop.add_tag("bad\x07tag", None, None).is_err());
    laptop.add_tag("clean", None, None)?;
    assert!(laptop.update_tag("clean", "bad\u{1f}rename", None, None).is_err());

    // set_tags and quick_add strip ASCII control characters.
    let task = laptop.add_task("Tagged")?;
    laptop.set_tags(task.id, &["alpha\u{1f}beta".into(), "\u{1f}".into()])?;
    assert_eq!(laptop.task(task.id)?.unwrap().tags, ["alphabeta"]);
    let parsed = crate::parse_quick_add("Buy milk #gro\u{1f}ceries", chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap(), &[]);
    assert_eq!(parsed.tags, ["groceries"]);

    // Even if a pre-existing DB already holds a tag containing U+001F, splitn(keys.len()) keeps sync working.
    laptop.conn.execute("INSERT INTO tags (name, created_at) VALUES (?1, 1)", ["legacy\u{1f}tag"])?;
    laptop.conn.execute(
        "INSERT INTO task_tags (task_id, tag) VALUES (?1, ?2)",
        rusqlite::params![task.id.to_string(), "legacy\u{1f}tag"],
    )?;
    let (changes, _) = laptop.changes_since(0)?;
    assert!(changes.iter().any(|c| c.entity == "task_tags" && c.key.ends_with("legacy\u{1f}tag")));
    phone.apply_changes(&changes)?;
    assert!(phone.task(task.id)?.unwrap().tags.contains(&"legacy\u{1f}tag".to_owned()));
    Ok(())
}

#[test]
fn finish_sync_is_atomic_when_apply_fails() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("Local")?;
    let request = store.sync_request()?;
    let good_list_id = uuid::Uuid::now_v7().to_string();
    let bad_answer = serde_json::json!({
        "position": 42,
        "changes": [
            {
                "entity": "lists",
                "key": good_list_id,
                "at": 100,
                "row": { "id": good_list_id, "name": "Partial", "created_at": 100 }
            },
            {
                "entity": "task_tags",
                "key": "malformed_key_missing_separator",
                "at": 100,
                "row": null
            }
        ]
    });
    assert!(store.finish_sync(&request, &bad_answer).is_err());
    // Neither the partial list nor the cursors should be persisted.
    assert!(store.lists()?.is_empty());
    assert!(store.has_unsent_changes()?);
    assert_eq!(store.sync_request()?.body["since"], 0);
    assert!(store.task(task.id)?.is_some());
    Ok(())
}

#[test]
fn protocol_version_mismatch_is_rejected() -> Result<()> {
    let store = Store::open_in_memory()?;
    let request = store.sync_request()?;
    assert_eq!(request.body["protocol"], SYNC_PROTOCOL_VERSION);

    // Server reporting future protocol version is rejected with IncompatibleProtocol.
    let future_answer = serde_json::json!({
        "protocol": 99,
        "min_protocol": 5,
        "position": 1,
        "changes": []
    });
    let err = store.finish_sync(&request, &future_answer).unwrap_err();
    let incompat = err.downcast_ref::<IncompatibleProtocol>().expect("expected IncompatibleProtocol");
    assert_eq!(incompat.client, SYNC_PROTOCOL_VERSION);
    assert_eq!(incompat.server, 99);
    assert_eq!(incompat.min_server, 5);

    // Server reporting protocol 0 (below MIN_SYNC_PROTOCOL_VERSION) is rejected.
    let old_answer = serde_json::json!({
        "protocol": 0,
        "position": 1,
        "changes": []
    });
    assert!(store.finish_sync(&request, &old_answer).unwrap_err().is::<IncompatibleProtocol>());

    // Protocol v1 and v2 are both accepted.
    store.finish_sync(&request, &serde_json::json!({ "protocol": 1, "position": 1, "changes": [] }))?;
    store.finish_sync(&request, &serde_json::json!({ "protocol": 2, "position": 2, "changes": [] }))?;
    Ok(())
}

#[test]
fn e2ee_sync_between_two_stores_and_wrong_passphrase_rejected() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("fern-e2ee-sync-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("secret.txt"), "top secret file")?;

    let laptop = Store::open_in_memory()?;
    let phone = Store::open_in_memory()?;
    let intruder = Store::open_in_memory()?;
    let no_pass = Store::open_in_memory()?;

    let cfg_ok = SyncConfig {
        server: "https://sync.example.org".into(),
        token: "tok".into(),
        passphrase: "correct-vault-passphrase".into(),
    };
    let cfg_wrong = SyncConfig {
        passphrase: "wrong-vault-passphrase".into(),
        ..cfg_ok.clone()
    };
    laptop.set_sync_config(&cfg_ok)?;
    phone.set_sync_config(&cfg_ok)?;
    intruder.set_sync_config(&cfg_wrong)?;

    let task = laptop.add_task("Confidential meeting")?;
    laptop.set_notes(task.id, "Bring encrypted ledger")?;
    let att = laptop.add_attachment(task.id, &dir.join("secret.txt"), &dir.join("laptop"))?;
    let stored_name = att.path(&dir.join("laptop")).file_name().unwrap().to_string_lossy().into_owned();

    let req = laptop.sync_request()?;
    let wire_changes = req.body["changes"].as_array().unwrap();
    assert_eq!(wire_changes.len(), 2);

    // Verify every wire row is encrypted and does not leak plaintext title/notes.
    let wire_str = req.body.to_string();
    assert!(!wire_str.contains("Confidential meeting"));
    assert!(!wire_str.contains("Bring encrypted ledger"));

    // Verify task_attachments change carries plaintext file_key outside encrypted row.
    let att_change = wire_changes.iter().find(|c| c["entity"] == "task_attachments").unwrap();
    assert_eq!(att_change["file_key"], file_key(&stored_name));
    assert!(e2ee::is_encrypted_row(att_change["row"].as_object().unwrap()));

    let server_answer = serde_json::json!({
        "protocol": SYNC_PROTOCOL_VERSION,
        "position": 10,
        "changes": wire_changes,
    });

    // Device without passphrase gets PassphraseRequired.
    let no_pass_req = no_pass.sync_request()?;
    let err = no_pass.finish_sync(&no_pass_req, &server_answer).unwrap_err();
    assert_eq!(err.downcast_ref::<E2eeError>(), Some(&E2eeError::PassphraseRequired));

    // Device with wrong passphrase gets WrongPassphrase.
    let intruder_req = intruder.sync_request()?;
    let err = intruder.finish_sync(&intruder_req, &server_answer).unwrap_err();
    assert_eq!(err.downcast_ref::<E2eeError>(), Some(&E2eeError::WrongPassphrase));

    // Device with matching passphrase decrypts cleanly.
    let phone_req = phone.sync_request()?;
    let outcome = phone.finish_sync_detailed(&phone_req, &server_answer)?;
    assert_eq!(outcome.applied, 2);
    assert_eq!(outcome.conflicts_preserved, 0);
    let synced_task = phone.task(task.id)?.unwrap();
    assert_eq!(synced_task.title, "Confidential meeting");
    assert_eq!(synced_task.notes, "Bring encrypted ledger");
    assert_eq!(phone.attachments(task.id)?, [att]);

    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

#[test]
fn offline_concurrent_note_edit_preserves_both_and_converges() -> Result<()> {
    let a = Store::open_in_memory()?;
    let b = Store::open_in_memory()?;

    // Initial task synced to both devices.
    let task = a.add_task("Architecture RFC")?;
    a.set_notes(task.id, "Initial outline")?;
    let req_a0 = a.sync_request()?;
    let initial_changes = req_a0.body["changes"].clone();
    a.finish_sync(&req_a0, &serde_json::json!({ "position": 1, "changes": [] }))?;
    let req_b0 = b.sync_request()?;
    b.finish_sync(&req_b0, &serde_json::json!({ "position": 1, "changes": initial_changes }))?;

    // Both devices go offline and edit the same task's notes concurrently.
    a.set_notes(task.id, "Offline notes from device A")?;
    a.conn.execute(
        "UPDATE sync_log SET at = 1000 WHERE entity = 'tasks' AND key = ?1",
        [task.id.to_string()],
    )?;
    a.conn.execute(
        "UPDATE sync_watermarks SET at = 1000 WHERE entity = 'tasks' AND key = ?1",
        [task.id.to_string()],
    )?;

    b.set_notes(task.id, "Offline notes from device B")?;
    b.conn.execute(
        "UPDATE sync_log SET at = 2000 WHERE entity = 'tasks' AND key = ?1",
        [task.id.to_string()],
    )?;
    b.conn.execute(
        "UPDATE sync_watermarks SET at = 2000 WHERE entity = 'tasks' AND key = ?1",
        [task.id.to_string()],
    )?;

    // Device B syncs first; server records B's version (at=2000).
    let req_b1 = b.sync_request()?;
    let b_wire_changes = req_b1.body["changes"].clone();
    b.finish_sync(&req_b1, &serde_json::json!({ "position": 2, "changes": [] }))?;

    // Device A comes online and syncs; B's version wins LWW (2000 > 1000),
    // but A preserves its own offline notes in a conflict section and re-logs the task.
    let req_a1 = a.sync_request()?;
    let outcome_a = a.finish_sync_detailed(
        &req_a1,
        &serde_json::json!({ "position": 2, "changes": b_wire_changes }),
    )?;
    assert_eq!(outcome_a.applied, 1);
    assert_eq!(outcome_a.conflicts_preserved, 1);

    let merged_on_a = a.task(task.id)?.unwrap().notes;
    assert!(merged_on_a.contains("Offline notes from device B"));
    assert!(merged_on_a.contains("Offline notes from device A"));
    assert!(merged_on_a.contains("Sync conflict ("));
    assert!(a.has_unsent_changes()?, "merged note must be queued to sync back");

    // Next sync: A pushes the merged note, B pulls it, and both converge with zero conflicts.
    let req_a2 = a.sync_request()?;
    let merged_wire_changes = req_a2.body["changes"].clone();
    a.finish_sync(&req_a2, &serde_json::json!({ "position": 3, "changes": [] }))?;
    assert!(!a.has_unsent_changes()?);

    let req_b2 = b.sync_request()?;
    let outcome_b = b.finish_sync_detailed(
        &req_b2,
        &serde_json::json!({ "position": 3, "changes": merged_wire_changes }),
    )?;
    assert_eq!(outcome_b.applied, 1);
    assert_eq!(outcome_b.conflicts_preserved, 0);
    assert_eq!(b.task(task.id)?.unwrap().notes, merged_on_a);
    assert!(!b.has_unsent_changes()?);
    Ok(())
}

#[test]
fn horizon_sync_and_legacy_client_preservation() -> Result<()> {
    use crate::horizon::Horizon;

    let a = Store::open_in_memory()?;
    let b = Store::open_in_memory()?;

    let w48 = Horizon::Week { year: 2026, week: 48 };
    let m12 = Horizon::Month { year: 2026, month: 12 };

    // Device A creates a horizon task and syncs to Device B.
    let task = a.create_task(
        crate::model::TaskDraft {
            title: "Ship Horizons".into(),
            horizon: Some(w48),
            ..Default::default()
        },
        true,
    )?;
    let pos_a1 = relay(&a, 0, &b);
    assert_eq!(b.task(task.id)?.unwrap().horizon, Some(w48));

    // Device A updates horizon to Month and syncs.
    a.set_horizon(task.id, Some(m12))?;
    let pos_a2 = relay(&a, pos_a1, &b);
    assert_eq!(b.task(task.id)?.unwrap().horizon, Some(m12));

    // Simulated older client sends a newer row WITHOUT the `horizon` column key.
    // Local `horizon` on Device B must NOT be erased!
    let (changes, _) = b.changes_since(0)?;
    let mut legacy_row = changes
        .into_iter()
        .find(|c| c.entity == "tasks" && c.key == task.id.to_string())
        .and_then(|c| c.row)
        .unwrap_or_else(|| {
            // Grab directly from A's last change if B had no local log entries.
            a.changes_since(0)
                .unwrap()
                .0
                .into_iter()
                .find(|c| c.entity == "tasks" && c.key == task.id.to_string())
                .unwrap()
                .row
                .unwrap()
        });
    assert!(legacy_row.remove("horizon").is_some(), "new client row must contain horizon");
    legacy_row.insert("title".into(), Json::from("Ship Horizons (edited on legacy client)"));
    let future_at = chrono::Utc::now().timestamp_millis() + 60_000;
    let legacy_change = Change {
        entity: "tasks".into(),
        key: task.id.to_string(),
        at: future_at,
        row: Some(legacy_row),
    };
    assert_eq!(b.apply_changes(&[legacy_change])?, 1);
    let after_legacy = b.task(task.id)?.unwrap();
    assert_eq!(after_legacy.title, "Ship Horizons (edited on legacy client)");
    assert_eq!(
        after_legacy.horizon,
        Some(m12),
        "legacy client row omitting horizon must preserve local horizon"
    );

    // Device A explicitly clears horizon (`horizon: null`) at an even later timestamp -> clears on B.
    a.set_horizon(task.id, None)?;
    let clear_at = future_at + 60_000;
    a.conn.execute(
        "UPDATE sync_log SET at = ?2 WHERE entity = 'tasks' AND key = ?1",
        params![task.id.to_string(), clear_at],
    )?;
    relay(&a, pos_a2, &b);
    assert_eq!(b.task(task.id)?.unwrap().horizon, None);
    Ok(())
}

#[test]
fn e2ee_rejects_plaintext_downgrade_injection() -> Result<()> {
    let plain_sender = Store::open_in_memory()?;
    let e2ee_client = Store::open_in_memory()?;
    e2ee_client.set_sync_config(&SyncConfig {
        server: "https://sync.example.org".into(),
        token: "tok".into(),
        passphrase: "correct-vault-passphrase".into(),
    })?;

    let injected = plain_sender.add_task("Injected plaintext task")?;
    let plain_changes = plain_sender.sync_request()?.body["changes"].clone();
    let req = e2ee_client.sync_request()?;
    let server_answer = serde_json::json!({
        "protocol": SYNC_PROTOCOL_VERSION,
        "position": 5,
        "changes": plain_changes,
    });

    let err = e2ee_client.finish_sync(&req, &server_answer).unwrap_err();
    assert!(matches!(err.downcast_ref::<E2eeError>(), Some(E2eeError::Malformed(_))));
    assert!(e2ee_client.task(injected.id)?.is_none());
    assert_eq!(e2ee_client.sync_request()?.body["since"], 0);
    Ok(())
}
