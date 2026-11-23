use chrono::NaiveDate;
use fern_core::store::{
    SCHEMA_VERSION, Store, TableCounts, apply_migrations_up_to, list_backups,
};
use rusqlite::{Connection, params};
use tempfile::TempDir;
use uuid::Uuid;

fn assert_foreign_keys_clean(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open db for foreign_key_check");
    let mut stmt = conn.prepare("PRAGMA foreign_key_check").expect("prepare fk check");
    let violations: Vec<String> = stmt
        .query_map([], |row| {
            let table: String = row.get(0)?;
            let rowid: i64 = row.get(1)?;
            let parent: String = row.get(2)?;
            Ok(format!("{table}#{rowid}->{parent}"))
        })
        .expect("query fk check")
        .collect::<Result<_, _>>()
        .expect("collect fk violations");
    assert!(violations.is_empty(), "foreign key violations: {violations:?}");
}

#[test]
fn migrate_forward_from_every_historical_schema_version() {
    let dir = TempDir::new().expect("tempdir");
    for start_version in 1..=SCHEMA_VERSION {
        let path = dir.path().join(format!("v{start_version}.db"));
        {
            let conn = Connection::open(&path).expect("open sqlite");
            conn.pragma_update(None, "foreign_keys", true).expect("enable fk");
            apply_migrations_up_to(&conn, start_version)
                .unwrap_or_else(|err| panic!("failed applying up to v{start_version}: {err:#}"));
            let current: usize = conn
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .expect("read user_version");
            assert_eq!(current, start_version);
        }

        let store = Store::open(&path)
            .unwrap_or_else(|err| panic!("failed migrating v{start_version} -> v{SCHEMA_VERSION}: {err:#}"));
        assert_eq!(store.schema_version().expect("schema_version"), SCHEMA_VERSION);
        assert_eq!(store.integrity_check().expect("integrity_check"), vec!["ok"]);
        drop(store);
        assert_foreign_keys_clean(&path);
    }
}

#[test]
fn migrate_v1_fixture_data_survives() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("v1_fixture.db");
    let task_open = Uuid::now_v7();
    let task_done = Uuid::now_v7();

    {
        let conn = Connection::open(&path).expect("open v1");
        conn.pragma_update(None, "foreign_keys", true).expect("enable fk");
        apply_migrations_up_to(&conn, 1).expect("apply v1");
        conn.execute(
            "INSERT INTO tasks (id, title, completed_at, created_at) VALUES (?1, ?2, NULL, 1000)",
            params![task_open.to_string(), "First v1 task"],
        )
        .expect("insert open task");
        conn.execute(
            "INSERT INTO tasks (id, title, completed_at, created_at) VALUES (?1, ?2, 2000, 1500)",
            params![task_done.to_string(), "Completed v1 task"],
        )
        .expect("insert done task");
    }

    let store = Store::open(&path).expect("migrate v1");
    assert_eq!(store.schema_version().expect("schema_version"), SCHEMA_VERSION);
    assert_eq!(store.integrity_check().expect("integrity_check"), vec!["ok"]);
    assert_eq!(store.quick_check().expect("quick_check"), vec!["ok"]);
    assert_eq!(
        store.table_counts().expect("counts"),
        TableCounts { tasks: 2, ..Default::default() }
    );
    let t1 = store.task(task_open).expect("read t1").expect("t1 exists");
    assert_eq!(t1.title, "First v1 task");
    assert!(!t1.is_completed());
    let t2 = store.task(task_done).expect("read t2").expect("t2 exists");
    assert_eq!(t2.title, "Completed v1 task");
    assert!(t2.is_completed());
    drop(store);
    assert_foreign_keys_clean(&path);
}

#[test]
fn migrate_v11_fixture_lists_tags_habits_countdowns_settings() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("v11_fixture.db");
    let list_id = Uuid::now_v7();
    let task_id = Uuid::now_v7();
    let checklist_id = Uuid::now_v7();
    let countdown_id = Uuid::now_v7();
    let habit_id = Uuid::now_v7();

    {
        let conn = Connection::open(&path).expect("open v11");
        conn.pragma_update(None, "foreign_keys", true).expect("enable fk");
        apply_migrations_up_to(&conn, 11).expect("apply v11");

        conn.execute(
            "INSERT INTO lists (id, name, color, created_at) VALUES (?1, 'Work', '#3366ff', 1000)",
            [list_id.to_string()],
        )
        .expect("insert list");
        conn.execute(
            "INSERT INTO tags (name, color, created_at) VALUES ('urgent', '#ff0000', 1000)",
            [],
        )
        .expect("insert tag");
        conn.execute(
            "INSERT INTO tasks (id, title, notes, list_id, priority, due_date, created_at, sort_order)
             VALUES (?1, 'Ship release', 'Check notes', ?2, 3, '2026-12-01', 1000, -1000)",
            params![task_id.to_string(), list_id.to_string()],
        )
        .expect("insert task");
        conn.execute(
            "INSERT INTO task_tags (task_id, tag) VALUES (?1, 'urgent')",
            [task_id.to_string()],
        )
        .expect("insert task_tag");
        conn.execute(
            "INSERT INTO checklist_items (id, task_id, title, completed_at, sort_order, created_at)
             VALUES (?1, ?2, 'Run tests', NULL, 0, 1000)",
            params![checklist_id.to_string(), task_id.to_string()],
        )
        .expect("insert checklist");
        conn.execute(
            "INSERT INTO countdowns (id, name, date, kind, created_at)
             VALUES (?1, 'Launch', '2027-01-01', 'countdown', 1000)",
            [countdown_id.to_string()],
        )
        .expect("insert countdown");
        conn.execute(
            "INSERT INTO habits (id, name, created_at) VALUES (?1, 'Read 20m', 1000)",
            [habit_id.to_string()],
        )
        .expect("insert habit");
        conn.execute(
            "INSERT INTO habit_checkins (habit_id, date) VALUES (?1, '2026-11-20')",
            [habit_id.to_string()],
        )
        .expect("insert checkin");
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('theme_mode', 'dark')",
            [],
        )
        .expect("insert setting");
    }

    let store = Store::open(&path).expect("migrate v11");
    assert_eq!(store.schema_version().expect("schema_version"), SCHEMA_VERSION);
    assert_eq!(store.integrity_check().expect("integrity_check"), vec!["ok"]);
    assert_eq!(
        store.table_counts().expect("counts"),
        TableCounts {
            tasks: 1,
            lists: 1,
            habits: 1,
            attachments: 0,
            countdowns: 1,
        }
    );

    let task = store.task(task_id).expect("read task").expect("task exists");
    assert_eq!(task.title, "Ship release");
    assert_eq!(task.notes, "Check notes");
    assert_eq!(task.list_id, Some(list_id));
    assert_eq!(task.tags, vec!["urgent"]);
    assert_eq!(store.checklist(task_id).expect("checklist").len(), 1);
    assert_eq!(store.lists().expect("lists").len(), 1);
    assert_eq!(store.habits().expect("habits").len(), 1);
    assert_eq!(store.countdowns().expect("countdowns").len(), 1);
    assert_eq!(store.setting("theme_mode").expect("setting").as_deref(), Some("dark"));

    drop(store);
    assert_foreign_keys_clean(&path);
}

#[test]
fn migrate_v37_fixture_attachments_sections_and_sync_backfill() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("v37_fixture.db");
    let list_id = Uuid::now_v7();
    let section_id = Uuid::now_v7();
    let task_id = Uuid::now_v7();
    let att_id = Uuid::now_v7();

    {
        let conn = Connection::open(&path).expect("open v37");
        conn.pragma_update(None, "foreign_keys", true).expect("enable fk");
        // Migration 37 adds task_attachments right before sync_log (migration 38).
        apply_migrations_up_to(&conn, 37).expect("apply v37");

        conn.execute(
            "INSERT INTO lists (id, name, color, created_at) VALUES (?1, 'Projects', NULL, 1000)",
            [list_id.to_string()],
        )
        .expect("insert list");
        conn.execute(
            "INSERT INTO list_sections (id, list_id, name, created_at) VALUES (?1, ?2, 'In Progress', 1000)",
            params![section_id.to_string(), list_id.to_string()],
        )
        .expect("insert section");
        conn.execute(
            "INSERT INTO tasks (id, title, notes, list_id, section_id, created_at, sort_order)
             VALUES (?1, 'Migrate DB', 'Keep attachment', ?2, ?3, 1000, -1000)",
            params![task_id.to_string(), list_id.to_string(), section_id.to_string()],
        )
        .expect("insert task");
        conn.execute(
            "INSERT INTO task_attachments (id, task_id, name, size, created_at)
             VALUES (?1, ?2, 'spec.pdf', 4096, 1000)",
            params![att_id.to_string(), task_id.to_string()],
        )
        .expect("insert attachment");
    }

    let store = Store::open(&path).expect("migrate v36");
    assert_eq!(store.schema_version().expect("schema_version"), SCHEMA_VERSION);
    assert_eq!(store.integrity_check().expect("integrity_check"), vec!["ok"]);
    assert_eq!(
        store.table_counts().expect("counts"),
        TableCounts {
            tasks: 1,
            lists: 1,
            habits: 0,
            attachments: 1,
            countdowns: 0,
        }
    );

    let task = store.task(task_id).expect("read task").expect("task exists");
    assert_eq!(task.section_id, Some(section_id), "section_id must survive list_sections table rebuild");
    let atts = store.attachments(task_id).expect("attachments");
    assert_eq!(atts.len(), 1);
    assert_eq!(atts[0].name, "spec.pdf");
    assert_eq!(atts[0].size, 4096);

    drop(store);
    assert_foreign_keys_clean(&path);
}

#[test]
fn list_backups_sorts_newest_first_and_parses_dates() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("fern-2026-10-01.db"), b"older").unwrap();
    std::fs::write(dir.path().join("fern-2026-11-22.db"), b"newest-backup").unwrap();
    std::fs::write(dir.path().join("fern-2026-11-15.db"), b"mid").unwrap();
    std::fs::write(dir.path().join("fern-manual.db"), b"no-date").unwrap();
    std::fs::write(dir.path().join("fern-2026-11-22.db-wal"), b"ignored-wal").unwrap();
    std::fs::write(dir.path().join("other.db"), b"ignored").unwrap();
    std::fs::create_dir(dir.path().join("fern-2026-12-01.db")).unwrap();

    let entries = list_backups(dir.path());
    let names: Vec<&str> = entries.iter().map(|e| e.filename.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "fern-2026-11-22.db",
            "fern-2026-11-15.db",
            "fern-2026-10-01.db",
            "fern-manual.db",
        ]
    );
    assert_eq!(entries[0].date, NaiveDate::from_ymd_opt(2026, 11, 22));
    assert_eq!(entries[0].size_bytes, b"newest-backup".len() as u64);
    assert_eq!(entries[3].date, None);
    assert!(list_backups(&dir.path().join("missing")).is_empty());
}
