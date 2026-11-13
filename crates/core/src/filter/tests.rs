use super::*;
use crate::{Store, View};
fn date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}
fn filter(values: Vec<ConditionValue>) -> SmartFilter {
    SmartFilter {
        name: "Test".into(),
        rules: FilterRules {
            conditions: values.into_iter().map(Condition::new).collect(),
            ..FilterRules::default()
        },
        ..SmartFilter::new()
    }
}
#[test]
fn intervals_and_week_settings() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = date("2026-11-13");
    let task = store.add_task("Spanning")?;
    store.set_due(task.id, Some(date("2026-11-12")), None)?;
    store.set_due_end(task.id, Some((date("2026-11-15"), None)))?;
    let task = store.task(task.id)?.unwrap();
    for (rule, expected) in [
        (DateRule::NextDays(14), true),
        (DateRule::Today, true),
        (DateRule::Overdue, false),
    ] {
        assert_eq!(
            filter(vec![ConditionValue::Date(vec![rule])]).matches(&task, today, Weekday::Mon),
            expected
        );
    }
    let sunday = store.add_task("Sunday")?;
    store.set_due(sunday.id, Some(date("2026-11-15")), None)?;
    let week = filter(vec![ConditionValue::Date(vec![DateRule::NextWeek])]);
    store.save_filter(&week)?;
    let mut prefs = store.preferences()?;
    prefs.week_start = Weekday::Sun;
    store.save_preferences(&prefs)?;
    assert!(
        store
            .tasks_in(&View::Filter(week.id), today)?
            .iter()
            .any(|t| t.id == sunday.id)
    );
    prefs.week_start = Weekday::Mon;
    store.save_preferences(&prefs)?;
    assert!(
        !store
            .tasks_in(&View::Filter(week.id), today)?
            .iter()
            .any(|t| t.id == sunday.id)
    );
    Ok(())
}

#[test]
fn date_presets_cover_month_boundaries_and_open_ranges() -> Result<()> {
    let store = Store::open_in_memory()?;
    let mut task = store.add_task("Fixture")?;
    let today = date("2024-01-31");
    for (due, rule, expected) in [
        (None, DateRule::NoDate, true),
        (None, DateRule::Tomorrow, false),
        (Some("2024-01-31"), DateRule::ThisMonth, true),
        (Some("2024-02-01"), DateRule::Tomorrow, true),
        (Some("2024-02-01"), DateRule::ThisMonth, false),
        (Some("2024-02-29"), DateRule::NextMonth, true),
        (Some("2024-03-01"), DateRule::NextMonth, false),
        (Some("2024-01-31"), DateRule::Span { from: None, to: Some(0) }, true),
        (Some("2024-02-01"), DateRule::Span { from: None, to: Some(0) }, false),
        (Some("2024-01-31"), DateRule::Span { from: Some(1), to: None }, false),
        (Some("2024-02-01"), DateRule::Span { from: Some(1), to: None }, true),
    ] {
        task.due_date = due.map(date);
        assert_eq!(rule.matches(&task, today, Weekday::Mon, false, false), expected, "{due:?}, {rule:?}");
    }
    task.due_date = None;
    assert!(!DateRule::Recurring.matches(&task, today, Weekday::Mon, false, false));
    task.repeat = Some(crate::Recurrence::new(crate::Frequency::Daily, 1));
    assert!(DateRule::Recurring.matches(&task, today, Weekday::Mon, false, false));
    Ok(())
}
#[test]
fn separate_tags_and_multiple_dates() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("Report")?;
    store.set_tags(task.id, &["work".into()])?;
    let mut f = filter(vec![
        ConditionValue::Tags {
            names: vec!["work".into()],
            untagged: false,
        },
        ConditionValue::Tags {
            names: vec!["urgent".into()],
            untagged: false,
        },
    ]);
    f.rules.advanced = true;
    let today = date("2026-11-13");
    assert!(!f.matches(&store.task(task.id)?.unwrap(), today, Weekday::Mon));
    store.set_tags(task.id, &["work".into(), "urgent".into()])?;
    assert!(f.matches(&store.task(task.id)?.unwrap(), today, Weekday::Mon));
    f.rules.conditions[1].not = true;
    assert!(!f.matches(&store.task(task.id)?.unwrap(), today, Weekday::Mon));
    store.save_filter(&f)?;
    assert_eq!(store.filters()?, [f]);
    let dates = filter(vec![
        ConditionValue::Date(vec![DateRule::Overdue, DateRule::NextDays(14)]),
        ConditionValue::Priority(vec![Priority::High]),
    ]);
    store.set_priority(task.id, Priority::High)?;
    for (due, expected) in [
        ("2026-11-12", true),
        ("2026-11-13", true),
        ("2026-11-26", true),
        ("2026-11-27", false),
    ] {
        store.set_due(task.id, Some(date(due)), None)?;
        assert_eq!(
            dates.matches(&store.task(task.id)?.unwrap(), today, Weekday::Mon),
            expected
        );
    }
    Ok(())
}
#[test]
fn storage_and_validation() -> Result<()> {
    for invalid in [
        "next_days:0",
        "next_days:-2",
        "next_days:abc",
        "future_unknown",
    ] {
        assert!(DateRule::from_key(invalid).is_err());
    }
    for (from, to) in [
        ("-31", "2"),
        ("0", "31"),
        ("3", "2"),
        ("x", ""),
        ("1.5", "2"),
    ] {
        assert!(DateRule::parse_span(from, to).is_err());
    }
    for (from, to) in [("-30", "30"), ("", ""), ("", "0"), ("14", ""), ("3", "3")] {
        assert!(DateRule::parse_span(from, to).is_ok());
    }
    let f = filter(vec![ConditionValue::Date(vec![
        DateRule::Span {
            from: Some(-3),
            to: Some(7),
        },
        DateRule::Recurring,
    ])]);
    let path = std::env::temp_dir().join(format!("fern-filter-{}.db", uuid::Uuid::now_v7()));
    {
        Store::open(&path)?.save_filter(&f)?;
    }
    {
        let store = Store::open(&path)?;
        assert_eq!(store.filters()?, [f.clone()]);
        let replica = Store::open_in_memory()?;
        replica.apply_changes(&store.changes_since(0)?.0)?;
        assert_eq!(replica.filters()?, [f]);
    }
    std::fs::remove_file(path)?;
    assert!(
        FilterRules::from_storage(r#"{"version":2,"advanced":false,"conditions":[]}"#).is_err()
    );
    Ok(())
}

#[test]
fn legacy_database_migrates_and_preserves_filters() -> Result<()> {
    let path =
        std::env::temp_dir().join(format!("fern-filter-migration-{}.db", uuid::Uuid::now_v7()));
    let id = uuid::Uuid::now_v7();
    {
        let store = Store::open(&path)?;
        store.conn.execute("INSERT INTO filters (id,name,lists,tags,date,priorities,keywords,kind,created_at,match_any,chain) VALUES (?1,'Legacy','','','overdue','','annual report',NULL,0,0,'include and:date unknown')", [id.to_string()])?;
        let version: u32 = store
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        store
            .conn
            .execute_batch("ALTER TABLE filters DROP COLUMN rules;")?;
        store
            .conn
            .pragma_update(None, "user_version", version - 1)?;
    }
    {
        let store = Store::open(&path)?;
        let old = store.filters()?.remove(0);
        assert_eq!(old.id, id);
        assert!(old.rules.advanced);
        assert_eq!(old.rules.conditions.len(), 2);
        assert_eq!(
            old.rules.conditions[1].value,
            ConditionValue::Date(vec![DateRule::Overdue])
        );
        let today = date("2026-11-13");
        let matching = store.add_task("Annual report")?;
        let partial = store.add_task("Annual party")?;
        for task in [&matching, &partial] {
            store.set_due(task.id, Some(date("2026-11-12")), None)?;
            store.set_due_end(task.id, Some((date("2026-11-14"), None)))?;
        }
        let matches = || -> Result<Vec<Id>> {
            Ok(store.tasks_in(&View::Filter(id), today)?.into_iter().map(|t| t.id).collect())
        };
        assert_eq!(matches()?, [matching.id]);
        store.save_filter(&old)?;
        assert_eq!(store.filters()?, [old]);
        assert_eq!(matches()?, [matching.id]);
    }
    std::fs::remove_file(path)?;
    Ok(())
}
#[test]
fn malformed_sync_is_rejected_atomically_and_legacy_sync_is_readable() -> Result<()> {
    let source = Store::open_in_memory()?;
    let target = Store::open_in_memory()?;
    let original = filter(vec![ConditionValue::Date(vec![DateRule::NextDays(14)])]);
    source.save_filter(&original)?;
    let changes = source.changes_since(0)?.0;
    target.apply_changes(&changes)?;
    for bad in [
        r#"{"version":99,"advanced":true,"conditions":[]}"#,
        r#"{"version":1,"advanced":true,"conditions":[{"join":"and","not":false,"value":{"field":"date","value":[{"next_days":0}]}}]}"#,
        "broken",
    ] {
        let mut change = changes
            .iter()
            .find(|c| c.entity == "filters")
            .unwrap()
            .clone();
        change.at += 1000;
        change.row.as_mut().unwrap().insert(
            "chain".into(),
            format!("lists! fern_rules_v1:{}", hex::encode(bad)).into(),
        );
        assert!(target.apply_changes(&[change]).is_err());
        assert_eq!(target.filters()?, [original.clone()]);
    }
    let mut legacy = changes
        .iter()
        .find(|c| c.entity == "filters")
        .unwrap()
        .clone();
    legacy.at += 2000;
    let row = legacy.row.as_mut().unwrap();
    row.insert("chain".into(), "".into());
    row.insert("date".into(), "today".into());
    target.apply_changes(&[legacy])?;
    assert!(
        target.filters()?[0]
            .rules
            .conditions
            .iter()
            .any(|c| c.value == ConditionValue::Date(vec![DateRule::Today]))
    );
    Ok(())
}

#[test]
fn outgoing_filters_fit_the_old_schema_without_losing_the_new_rules() -> Result<()> {
    let source = Store::open_in_memory()?;
    let filter = filter(vec![ConditionValue::Include("слово or:type".into())]);
    source.save_filter(&filter)?;
    let task = source.add_task("Unrelated task")?;
    let (changes, _) = source.changes_since(0)?;
    let row = changes.iter().find(|c| c.entity == "filters").unwrap().row.as_ref().unwrap();
    let old_schema = rusqlite::Connection::open_in_memory()?;
    old_schema.execute_batch(
        "CREATE TABLE filters (
            id TEXT PRIMARY KEY, name TEXT, lists TEXT, tags TEXT, date TEXT, priorities TEXT,
            keywords TEXT, kind TEXT, created_at INTEGER, match_any INTEGER, chain TEXT
        );",
    )?;
    let columns = row.keys().map(String::as_str).collect::<Vec<_>>().join(", ");
    let placeholders = vec!["?"; row.len()].join(", ");
    old_schema.execute(
        &format!("INSERT INTO filters ({columns}) VALUES ({placeholders})"),
        rusqlite::params_from_iter(row.values().map(|value| {
            if let Some(number) = value.as_i64() {
                rusqlite::types::Value::Integer(number)
            } else {
                value.as_str().map_or(rusqlite::types::Value::Null, |text| text.to_owned().into())
            }
        })),
    )?;
    let chain = row["chain"].as_str().unwrap();
    let legacy = SmartFilter {
        rules: from_legacy(vec![ConditionValue::Lists(vec![])], false, chain)?,
        ..SmartFilter::new()
    };
    assert!(!legacy.matches(&task, date("2026-11-13"), Weekday::Mon));

    let replica = Store::open_in_memory()?;
    replica.apply_changes(&changes)?;
    assert_eq!(replica.filters()?, [filter]);
    assert!(replica.task(task.id)?.is_some());
    Ok(())
}

#[test]
fn one_custom_range_per_condition_keeps_the_editor_round_trip_lossless() {
    let rules = filter(vec![ConditionValue::Date(vec![
        DateRule::NextDays(30),
        DateRule::NextDays(7),
    ])])
    .rules;
    assert!(rules.to_storage().is_err());
}
#[test]
fn logical_precedence_negation_keywords_and_closed_tasks() -> Result<()> {
    let store = Store::open_in_memory()?;
    let base = store.add_task("Alpha")?;
    let today = date("2026-11-13");
    let tag = |name: &str| {
        Condition::new(ConditionValue::Tags {
            names: vec![name.into()],
            untagged: false,
        })
    };
    let f = SmartFilter {
        name: "Logic".into(),
        rules: FilterRules {
            advanced: true,
            conditions: vec![
                tag("a"),
                Condition {
                    join: Join::Or,
                    ..tag("b")
                },
                tag("c"),
            ],
            ..FilterRules::default()
        },
        ..SmartFilter::new()
    };
    for a in [false, true] {
        for b in [false, true] {
            for c in [false, true] {
                let mut task = base.clone();
                task.tags = [(a, "a"), (b, "b"), (c, "c")]
                    .into_iter()
                    .filter(|(on, _)| *on)
                    .map(|(_, s)| s.into())
                    .collect();
                assert_eq!(f.matches(&task, today, Weekday::Mon), a || (b && c));
            }
        }
    }
    let keyword = filter(vec![ConditionValue::Include("alpha beta".into())]);
    assert!(
        keyword.matches(&base, today, Weekday::Mon),
        "a new keyword condition matches any entered word"
    );
    let mut task = base;
    task.completed_at = Some(1);
    assert!(!keyword.matches(&task, today, Weekday::Mon));
    task.completed_at = None;
    task.deleted_at = Some(1);
    assert!(!keyword.matches(&task, today, Weekday::Mon));
    task.deleted_at = None;
    task.wont_do_at = Some(1);
    assert!(!keyword.matches(&task, today, Weekday::Mon));
    Ok(())
}
#[test]
fn mode_conversion_is_lossless_or_refused_and_conditions_are_independent() -> Result<()> {
    let mut normal = filter(vec![
        ConditionValue::Date(vec![DateRule::NextDays(14)]),
        ConditionValue::Priority(vec![Priority::High]),
    ])
    .rules;
    normal.set_advanced(true)?;
    normal.conditions[0].join = Join::And;
    normal.conditions[1].join = Join::Or;
    let unchanged = normal.clone();
    assert!(normal.set_advanced(false).is_err());
    assert_eq!(normal, unchanged);
    normal.conditions[1].join = Join::And;
    normal.set_advanced(false)?;
    assert_eq!(normal.conditions.len(), Field::ALL.len());
    let legacy = from_legacy(
        vec![ConditionValue::Tags {
            names: vec!["old".into()],
            untagged: false,
        }],
        false,
        "tags and:tags!",
    )?;
    let mut rules = legacy;
    rules.conditions[0].value = ConditionValue::Tags {
        names: vec!["new".into()],
        untagged: false,
    };
    assert_eq!(
        rules.conditions[1].value,
        ConditionValue::Tags {
            names: vec!["old".into()],
            untagged: false
        }
    );
    rules.conditions.remove(0);
    assert_eq!(rules.conditions.len(), 1);
    Ok(())
}
#[test]
fn examples_and_preview_use_the_same_evaluator() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("Today")?;
    store.set_due(task.id, Some(date("2026-11-13")), None)?;
    store.set_priority(task.id, Priority::High)?;
    let tasks = store.tasks()?;
    let mut f = examples()[0].clone();
    f.id = uuid::Uuid::now_v7();
    f.name = "Example".into();
    store.save_filter(&f)?;
    let ids: Vec<_> = f
        .preview(&tasks, date("2026-11-13"), Weekday::Mon)
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, [task.id]);
    assert_eq!(
        ids,
        store
            .tasks_in(&View::Filter(f.id), date("2026-11-13"))?
            .into_iter()
            .map(|t| t.id)
            .collect::<Vec<_>>()
    );
    store.delete_filter(f.id)?;
    assert!(store.filters()?.is_empty());
    Ok(())
}
