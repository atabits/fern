use super::*;
use super::migrations::MIGRATIONS;
use crate::attachment::PreparedAttachment;
use crate::countdown::CountdownKind;
use crate::habit::{Checkin, CheckinMode, CheckinStatus, Goal, HabitDraft, HabitSection};
use crate::model::{ListDraft, TaskDraft};
use crate::recurrence::Recurrence;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn titles(tasks: &[Task]) -> Vec<&str> {
    tasks.iter().map(|t| t.title.as_str()).collect()
}

#[test]
fn add_complete_delete() -> Result<()> {
    let store = Store::open_in_memory()?;
    let a = store.add_task("  buy milk ")?;
    let b = store.add_task("call the bank")?;
    assert_eq!(a.title, "buy milk");

    store.set_completed(a.id, true)?;
    let tasks = store.tasks()?;
    assert_eq!(tasks.iter().map(|t| t.id).collect::<Vec<_>>(), [b.id, a.id]);
    assert!(store.task(a.id)?.unwrap().is_completed());

    store.set_completed(a.id, false)?;
    assert!(!store.task(a.id)?.unwrap().is_completed());

    store.set_title(b.id, "call the dentist")?;
    store.set_notes(b.id, "after 6 pm\nask about weekends")?;
    let b2 = store.task(b.id)?.unwrap();
    assert_eq!(b2.title, "call the dentist");
    assert_eq!(b2.notes, "after 6 pm\nask about weekends");

    store.purge_task(b.id)?;
    assert_eq!(store.tasks()?.len(), 1);
    Ok(())
}

#[test]
fn reopen_keeps_data() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("fern-test-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("tasks.db");
    Store::open(&path)?.add_task("x")?;
    assert_eq!(Store::open(&path)?.tasks()?.len(), 1);
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn migrates_v1_database() -> Result<()> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(MIGRATIONS[0])?;
    conn.pragma_update(None, "user_version", 1)?;
    conn.execute(
        "INSERT INTO tasks VALUES ('0192f000-0000-7000-8000-000000000001', 'legacy', NULL, 1)",
        [],
    )?;
    let store = Store::init(conn, None)?;
    let task = &store.tasks()?[0];
    assert_eq!(
        (task.title.as_str(), task.kind, task.priority),
        ("legacy", Kind::Task, Priority::None)
    );
    Ok(())
}

#[test]
fn priority_kind_and_due() -> Result<()> {
    let store = Store::open_in_memory()?;
    let t = store.add_task("report")?;
    store.set_priority(t.id, Priority::High)?;
    store.set_kind(t.id, Kind::Note)?;
    store.set_due(t.id, Some(d(2026, 10, 20)), NaiveTime::from_hms_opt(18, 30, 0))?;
    let t = store.task(t.id)?.unwrap();
    assert_eq!(t.priority, Priority::High);
    assert_eq!(t.kind, Kind::Note);
    assert_eq!(t.due_date, Some(d(2026, 10, 20)));
    assert_eq!(t.due_time, NaiveTime::from_hms_opt(18, 30, 0));

    // A time without a date is not stored.
    store.set_due(t.id, None, NaiveTime::from_hms_opt(9, 0, 0))?;
    let t = store.task(t.id)?.unwrap();
    assert_eq!((t.due_date, t.due_time), (None, None));
    Ok(())
}

#[test]
fn smart_views_and_counts() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = d(2026, 10, 18);
    let overdue = store.add_task("overdue")?;
    let now = store.add_task("today")?;
    let tomorrow = store.add_task("tomorrow")?;
    let week = store.add_task("in 6 days")?;
    let later = store.add_task("in a week")?;
    let done = store.add_task("done today")?;
    store.add_task("no due date")?;
    store.set_due(overdue.id, Some(d(2026, 10, 15)), None)?;
    store.set_due(now.id, Some(today), None)?;
    store.set_due(tomorrow.id, Some(d(2026, 10, 19)), None)?;
    store.set_due(week.id, Some(d(2026, 10, 24)), None)?;
    store.set_due(later.id, Some(d(2026, 10, 25)), None)?;
    store.set_due(done.id, Some(today), None)?;
    store.set_completed(done.id, true)?;

    assert_eq!(titles(&store.tasks_in(&View::Today, today)?), ["today", "overdue"]);
    let with_completed = store.tasks_in_from_with_today_completed(store.tasks()?, &View::Today, today, true)?;
    assert!(with_completed.iter().any(|task| task.id == done.id));
    assert_eq!(with_completed.len(), 3);
    assert_eq!(titles(&store.tasks_in(&View::Tomorrow, today)?), ["tomorrow"]);
    let done_tomorrow = store.add_task("done tomorrow")?;
    store.set_due(done_tomorrow.id, Some(today + Duration::days(1)), None)?;
    store.complete(done_tomorrow.id, true)?;
    assert_eq!(titles(&store.tasks_in(&View::Tomorrow, today)?), ["tomorrow"]);
    let tomorrow_with_completed = store.tasks_in_from_with_today_completed(
        store.tasks()?,
        &View::Tomorrow,
        today,
        true,
    )?;
    assert!(tomorrow_with_completed.iter().any(|task| task.id == done_tomorrow.id));
    let next7_with_completed = store.tasks_in_from_with_today_completed(
        store.tasks()?,
        &View::Next7Days,
        today,
        true,
    )?;
    assert!(next7_with_completed.iter().any(|task| task.id == done_tomorrow.id));
    assert!(next7_with_completed.iter().any(|task| task.id == done.id));
    store.purge_task(done_tomorrow.id)?;
    assert_eq!(
        titles(&store.tasks_in(&View::Next7Days, today)?),
        ["in 6 days", "tomorrow", "today", "overdue"]
    );
    assert_eq!(titles(&store.tasks_in(&View::Completed, today)?), ["done today"]);
    assert_eq!(
        store.counts(today)?,
        ViewCounts { inbox: 6, all: 6, today: 2, tomorrow: 1, next_7_days: 4 }
    );
    Ok(())
}

#[test]
fn tags_are_shared_case_insensitive_and_counted() -> Result<()> {
    let store = Store::open_in_memory()?;
    let a = store.add_task("a")?;
    let b = store.add_task("b")?;
    store.set_tags(a.id, &["Élan".into(), "focus".into()])?;
    store.set_tags(b.id, &["élan".into()])?;
    store.set_tag_color("Élan", Some("#3B82F6"))?;

    assert_eq!(store.task(b.id)?.unwrap().tags, ["Élan"]);
    let tags = store.tags()?;
    assert_eq!(
        tags.iter().map(|t| (t.name.as_str(), t.open_count)).collect::<Vec<_>>(),
        [("Élan", 2), ("focus", 1)]
    );
    assert_eq!(tags[0].color.as_deref(), Some("#3B82F6"));

    store.set_completed(b.id, true)?;
    assert_eq!(store.tags()?[0].open_count, 1);
    assert_eq!(titles(&store.tasks_in(&View::Tag("ÉLAN".into()), d(2026, 1, 1))?), ["a", "b"]);

    store.set_tags(a.id, &[])?;
    assert!(store.task(a.id)?.unwrap().tags.is_empty());
    Ok(())
}

#[test]
fn lists_scope_inbox_and_move_subtasks() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = d(2026, 10, 18);
    let work = store.create_list(" Work ")?;
    assert_eq!(work.name, "Work");
    assert!(store.create_list("  ").is_err());

    let inbox_task = store.add_task("in the inbox")?;
    let parent = store.add_task("report")?;
    let child = store.add_task("collect numbers")?;
    store.set_parent(child.id, Some(parent.id))?;
    store.set_list(parent.id, Some(work.id))?;

    assert_eq!(store.task(child.id)?.unwrap().list_id, Some(work.id));
    assert_eq!(titles(&store.tasks_in(&View::Inbox, today)?), [inbox_task.title.as_str()]);
    assert_eq!(titles(&store.tasks_in(&View::List(work.id), today)?), ["collect numbers", "report"]);
    assert_eq!(store.tasks_in(&View::All, today)?.len(), 3);
    assert_eq!(store.counts(today)?.inbox, 1);
    assert_eq!(store.lists()?[0].open_count, 2);

    store.rename_list(work.id, "Office")?;
    assert_eq!(store.lists()?[0].name, "Office");
    assert!(store.rename_list(work.id, " ").is_err());
    store.set_list_color(work.id, Some("#22C55E"))?;
    assert_eq!(store.lists()?[0].color.as_deref(), Some("#22C55E"));
    store.set_list_color(work.id, None)?;
    assert_eq!(store.lists()?[0].color, None);

    store.delete_list(work.id)?;
    assert!(store.lists()?.is_empty());
    assert_eq!(titles(&store.tasks()?), ["in the inbox"]);
    Ok(())
}

#[test]
fn checklist_items_order_progress_and_cascade() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("pack for the trip")?;
    let a = store.add_checklist_item(task.id, " passport ", None)?;
    let c = store.add_checklist_item(task.id, "charger", None)?;
    let b = store.add_checklist_item(task.id, "tickets", Some(a.id))?;
    let titles = |store: &Store| -> Result<Vec<String>> {
        Ok(store.checklist(task.id)?.into_iter().map(|i| i.title).collect())
    };
    assert_eq!(titles(&store)?, ["passport", "tickets", "charger"]);

    store.set_checklist_item_completed(b.id, true)?;
    store.set_checklist_item_title(c.id, "phone charger")?;
    assert_eq!(store.task(task.id)?.unwrap().checklist, Some((1, 3)));
    assert_eq!(store.tasks()?[0].checklist, Some((1, 3)));
    assert!(store.checklist(task.id)?[1].completed);

    store.delete_checklist_item(a.id)?;
    assert_eq!(titles(&store)?, ["tickets", "phone charger"]);
    store.purge_task(task.id)?;
    assert!(store.checklist(task.id)?.is_empty());
    Ok(())
}

#[test]
fn trash_hides_restores_and_purges() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = d(2026, 10, 18);
    let parent = store.add_task("old project")?;
    let child = store.add_task("its step")?;
    let kept = store.add_task("keep me")?;
    store.set_parent(child.id, Some(parent.id))?;
    store.set_tags(parent.id, &["work".into()])?;

    store.delete_task(parent.id)?;
    assert_eq!(titles(&store.tasks_in(&View::Inbox, today)?), ["keep me"]);
    assert_eq!(titles(&store.tasks_in(&View::Trash, today)?), ["its step", "old project"]);
    assert_eq!(store.counts(today)?.inbox, 1);
    assert_eq!(store.tags()?[0].open_count, 0);

    store.restore_task(parent.id)?;
    assert_eq!(store.tasks_in(&View::Inbox, today)?.len(), 3);
    assert!(store.tasks_in(&View::Trash, today)?.is_empty());

    store.delete_task(parent.id)?;
    store.empty_trash()?;
    assert_eq!(titles(&store.tasks()?), ["keep me"]);
    assert!(store.task(kept.id)?.is_some());
    Ok(())
}

#[test]
fn completing_a_repeating_task_moves_it_forward() -> Result<()> {
    use crate::recurrence::Frequency;

    let store = Store::open_in_memory()?;
    let today = d(2026, 10, 18);
    let task = store.add_task("water the plants")?;
    store.set_tags(task.id, &["home".into()])?;
    store.set_repeat(task.id, Some(Recurrence::new(Frequency::Weekly, 1)), today)?;
    assert_eq!(store.task(task.id)?.unwrap().due_date, Some(today));
    let item = store.add_checklist_item(task.id, "balcony", None)?;
    store.set_checklist_item_completed(item.id, true)?;

    store.set_completed(task.id, true)?;
    let task_now = store.task(task.id)?.unwrap();
    assert!(!task_now.is_completed());
    assert_eq!(task_now.due_date, Some(d(2026, 10, 25)));
    assert_eq!(task_now.checklist, Some((0, 1)));

    let history = store.tasks_in(&View::Completed, today)?;
    assert_eq!(titles(&history), ["water the plants"]);
    assert_eq!(history[0].due_date, Some(today));
    assert_eq!(history[0].repeat, None);
    assert_eq!(history[0].tags, ["home"]);

    store.set_repeat(task.id, None, today)?;
    store.set_completed(task.id, true)?;
    assert!(store.task(task.id)?.unwrap().is_completed());
    Ok(())
}

#[test]
fn search_matches_title_notes_and_tags() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = d(2026, 10, 18);
    let a = store.add_task("Buy Milk")?;
    let b = store.add_task("Call the bank")?;
    let c = store.add_task("Купить хлеб")?;
    let gone = store.add_task("milk in the trash")?;
    store.set_notes(b.id, "ask about the milk subscription")?;
    store.set_tags(c.id, &["Groceries".into()])?;
    store.set_completed(a.id, true)?;
    store.delete_task(gone.id)?;

    let search = |q: &str| -> Result<Vec<String>> {
        Ok(store.tasks_in(&View::Search(q.into()), today)?.into_iter().map(|t| t.title).collect())
    };
    assert_eq!(search("MILK")?, ["Call the bank", "Buy Milk"]);
    assert_eq!(search("ХЛЕБ")?, ["Купить хлеб"]);
    assert_eq!(search("grocer")?, ["Купить хлеб"]);
    assert!(search("   ")?.is_empty());
    Ok(())
}

#[test]
fn manual_order() -> Result<()> {
    let store = Store::open_in_memory()?;
    let a = store.add_task("a")?;
    let b = store.add_task("b")?;
    let c = store.add_task("c")?;
    assert_eq!(titles(&store.tasks()?), ["c", "b", "a"]);

    store.move_task(c.id, a.id, true)?;
    assert_eq!(titles(&store.tasks()?), ["b", "a", "c"]);
    store.move_task(a.id, b.id, false)?;
    assert_eq!(titles(&store.tasks()?), ["a", "b", "c"]);

    // New tasks still land on top after renumbering.
    store.add_task("d")?;
    assert_eq!(titles(&store.tasks()?), ["d", "a", "b", "c"]);
    assert!(store.move_task(a.id, Uuid::now_v7(), false).is_err());
    Ok(())
}

#[test]
fn tasks_due_in_a_range() -> Result<()> {
    let store = Store::open_in_memory()?;
    let evening = store.add_task("evening")?;
    let morning = store.add_task("morning")?;
    let all_day = store.add_task("all day")?;
    let outside = store.add_task("outside")?;
    let trashed = store.add_task("trashed")?;
    store.set_due(evening.id, Some(d(2026, 10, 20)), NaiveTime::from_hms_opt(19, 0, 0))?;
    store.set_due(morning.id, Some(d(2026, 10, 20)), NaiveTime::from_hms_opt(8, 0, 0))?;
    store.set_due(all_day.id, Some(d(2026, 10, 20)), None)?;
    store.set_due(outside.id, Some(d(2026, 11, 2)), None)?;
    store.set_due(trashed.id, Some(d(2026, 10, 21)), None)?;
    store.delete_task(trashed.id)?;
    store.set_completed(morning.id, true)?;

    let tasks = store.tasks_due_between(d(2026, 10, 1), d(2026, 10, 31))?;
    assert_eq!(titles(&tasks), ["all day", "morning", "evening"]);
    Ok(())
}

#[test]
fn countdowns_crud() -> Result<()> {
    let store = Store::open_in_memory()?;
    let trip = store.add_countdown(" Trip ", d(2027, 1, 1), CountdownKind::Countdown)?;
    store.add_countdown("Moved in", d(2025, 3, 1), CountdownKind::Anniversary)?;
    assert!(store.add_countdown(" ", d(2027, 1, 1), CountdownKind::Holiday).is_err());
    let all = store.countdowns()?;
    assert_eq!(all.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Trip", "Moved in"]);
    assert_eq!(all[1].kind, CountdownKind::Anniversary);
    store.delete_countdown(trip.id)?;
    assert_eq!(store.countdowns()?.len(), 1);
    Ok(())
}

#[test]
fn habits_and_checkins() -> Result<()> {
    let store = Store::open_in_memory()?;
    let floss = store.add_habit(" Floss ")?;
    let read = store.add_habit("Read")?;
    assert!(store.add_habit("").is_err());
    assert!(store.toggle_checkin(floss.id, d(2026, 10, 17))?);
    assert!(store.toggle_checkin(floss.id, d(2026, 10, 18))?);
    assert!(!store.toggle_checkin(floss.id, d(2026, 10, 18))?);
    assert_eq!(store.checkins(floss.id)?, [d(2026, 10, 17)].into());
    assert!(store.checkins(read.id)?.is_empty());
    store.delete_habit(floss.id)?;
    assert_eq!(store.habits()?.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["Read"]);
    Ok(())
}

#[test]
fn templates() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("weekly review")?;
    store.set_notes(task.id, "Look back, plan ahead")?;
    store.set_priority(task.id, Priority::Medium)?;
    store.set_tags(task.id, &["work".to_owned()])?;
    store.add_checklist_item(task.id, "inbox zero", None)?;
    store.add_checklist_item(task.id, "plan the week", None)?;
    let template = store.save_template(task.id)?;
    assert_eq!(store.templates()?, std::slice::from_ref(&template));

    let copy = store.add_from_template(template.id)?;
    assert_ne!(copy.id, task.id);
    assert_eq!((copy.title.as_str(), copy.notes.as_str(), copy.priority), ("weekly review", "Look back, plan ahead", Priority::Medium));
    assert_eq!(copy.tags, ["work"]);
    let items: Vec<String> = store.checklist(copy.id)?.into_iter().map(|i| i.title).collect();
    assert_eq!(items, ["inbox zero", "plan the week"]);

    store.delete_template(template.id)?;
    assert!(store.templates()?.is_empty());
    Ok(())
}

#[test]
fn comments_on_a_task() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("Fix the sink")?;
    assert!(store.add_comment(task.id, "  ").is_err());
    let first = store.add_comment(task.id, "Bought a new washer")?;
    store.add_comment(task.id, "Done, no leak")?;
    let texts = |store: &Store| store.comments(task.id).unwrap().into_iter().map(|c| c.text).collect::<Vec<_>>();
    assert_eq!(texts(&store), ["Bought a new washer", "Done, no leak"]);
    store.delete_comment(first.id)?;
    assert_eq!(texts(&store), ["Done, no leak"]);
    Ok(())
}

#[test]
fn skipping_the_recurrence() -> Result<()> {
    use crate::recurrence::{Frequency, Recurrence};
    let store = Store::open_in_memory()?;
    let d = |m, day| NaiveDate::from_ymd_opt(2026, m, day).unwrap();
    let meter = store.add_task("Read the meter")?;
    store.set_due(meter.id, Some(d(10, 24)), NaiveTime::from_hms_opt(9, 0, 0))?;
    store.set_repeat(meter.id, Some(Recurrence::new(Frequency::Weekly, 1)), d(10, 18))?;
    store.skip_recurrence(meter.id)?;
    let task = store.task(meter.id)?.unwrap();
    assert_eq!((task.due_date, task.due_time, task.is_completed()), (Some(d(10, 31)), NaiveTime::from_hms_opt(9, 0, 0), false));
    let once = store.add_task("Once")?;
    store.set_due(once.id, Some(d(10, 24)), None)?;
    assert!(store.skip_recurrence(once.id).is_err(), "only repeating tasks skip");
    Ok(())
}

#[test]
fn sub_tags_merge_rename_and_delete() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    let tagged = |title: &str, tags: &[&str]| -> Result<Task> {
        let task = store.add_task(title)?;
        store.set_tags(task.id, &tags.iter().map(|t| t.to_string()).collect::<Vec<_>>())?;
        Ok(task)
    };
    tagged("Run", &["health"])?;
    tagged("Dentist", &["teeth"])?;
    tagged("Gym", &["sport"])?;
    store.add_tag("#home", Some("#3B82F6"), None)?;
    assert!(store.add_tag("Home", None, None).is_err(), "names are unique whatever the case");

    // A sub-tag's tasks show in its parent's view; one level deep only.
    store.update_tag("teeth", "teeth", None, Some("health"))?;
    let titles = |view: View| {
        let mut titles = store.tasks_in(&view, today).unwrap().into_iter().map(|t| t.title).collect::<Vec<_>>();
        titles.sort();
        titles
    };
    assert_eq!(titles(View::Tag("Health".into())), ["Dentist", "Run"]);
    assert_eq!(titles(View::Tag("teeth".into())), ["Dentist"]);
    assert!(store.update_tag("sport", "sport", None, Some("teeth")).is_err(), "teeth is a sub-tag");
    assert!(store.update_tag("health", "health", None, Some("sport")).is_err(), "health has sub-tags");

    // Renaming keeps the tasks, the sub-tags and the color.
    store.update_tag("health", "wellbeing", Some("#22C55E"), None)?;
    let tags = store.tags()?;
    assert!(!tags.iter().any(|t| t.name == "health"));
    assert_eq!(tags.iter().find(|t| t.name == "teeth").unwrap().parent.as_deref(), Some("wellbeing"));
    assert_eq!(titles(View::Tag("wellbeing".into())), ["Dentist", "Run"]);

    // Merging moves the tasks over and deletes the tag.
    store.merge_tag("sport", "wellbeing")?;
    assert!(!store.tags()?.iter().any(|t| t.name == "sport"));
    assert_eq!(titles(View::Tag("wellbeing".into())), ["Dentist", "Gym", "Run"]);

    store.set_tag_pinned("home", true)?;
    assert!(store.tags()?.iter().find(|t| t.name == "home").unwrap().pinned);

    // Deleting a parent leaves its sub-tags at the top and its tasks untagged.
    store.delete_tag("wellbeing")?;
    assert_eq!(store.tags()?.iter().find(|t| t.name == "teeth").unwrap().parent, None);
    assert!(store.tasks()?.iter().find(|t| t.title == "Run").unwrap().tags.is_empty());
    Ok(())
}

#[test]
fn list_settings_folders_and_archive() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    let home = store.add_list_folder("Home")?;
    let draft = ListDraft {
        name: " Groceries ".into(),
        icon: Some("🛒".into()),
        color: Some("#ff6b6b".into()),
        folder: Some(home.id),
        kind: Kind::Note,
        in_smart_lists: true,
        ..Default::default()
    };
    let groceries = store.add_list(&draft)?;
    assert_eq!((groceries.name.as_str(), groceries.icon.as_deref(), groceries.folder, groceries.kind), ("Groceries", Some("🛒"), Some(home.id), Kind::Note));
    let work = store.create_list("Work")?;
    let milk = store.add_task("Milk")?;
    store.set_list(milk.id, Some(groceries.id))?;
    store.set_due(milk.id, Some(today), None)?;
    let report = store.add_task("Report")?;
    store.set_list(report.id, Some(work.id))?;
    store.set_due(report.id, Some(today), None)?;

    // "Show in Smart List: Do not show" keeps the list's tasks out of Today and All.
    store.update_list(groceries.id, &ListDraft { in_smart_lists: false, ..ListDraft::of(&groceries) })?;
    assert_eq!(store.tasks_in(&View::Today, today)?.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Report"]);
    assert_eq!(store.counts(today)?.all, 1);
    assert_eq!(store.tasks_in(&View::List(groceries.id), today)?.len(), 1, "the list itself still shows them");
    // Archived lists leave the smart lists too, and their tasks become read-only.
    assert!(!store.is_list_archived(work.id)?);
    assert!(!store.is_task_read_only(report.id)?);
    store.set_list_archived(work.id, true)?;
    assert!(store.tasks_in(&View::Today, today)?.is_empty());
    assert!(store.lists()?.iter().find(|l| l.id == work.id).unwrap().archived);
    assert!(store.is_list_archived(work.id)?);
    assert_eq!(store.list_name_and_archived(work.id)?, Some(("Work".to_owned(), true)));
    assert!(store.is_task_read_only(report.id)?);
    store.set_list_archived(work.id, false)?;
    assert!(!store.is_task_read_only(report.id)?);
    // Trashed tasks are read-only as well.
    store.delete_task(report.id)?;
    assert!(store.is_task_read_only(report.id)?);
    store.restore_task(report.id)?;
    assert!(!store.is_task_read_only(report.id)?);

    // Order: a list placed first comes first; Duplicate goes right after its list.
    store.place_list(work.id, None, None)?;
    let names = |store: &Store| store.lists().unwrap().into_iter().map(|l| l.name).collect::<Vec<_>>();
    assert_eq!(names(&store), ["Work", "Groceries"]);
    let copy = store.duplicate_list(work.id, " copy")?;
    assert_eq!(names(&store), ["Work", "Work copy", "Groceries"]);
    assert_eq!(store.tasks_in(&View::List(copy.id), today)?.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Report"]);

    // Ungroup keeps the lists and drops the folder.
    store.ungroup_list_folder(home.id)?;
    assert!(store.list_folders()?.is_empty());
    assert_eq!(store.lists()?.iter().find(|l| l.id == groceries.id).unwrap().folder, None);
    Ok(())
}

#[test]
fn sections_left_right_and_moved() -> Result<()> {
    let store = Store::open_in_memory()?;
    let work = store.create_list("Work")?;
    let home = store.create_list("Home")?;
    let doing = store.add_list_section(Some(work.id), "Doing")?;
    store.insert_list_section(Some(work.id), "To do", Some(0))?;
    store.insert_list_section(Some(work.id), "Done", None)?;
    let names = |list| store.list_sections(Some(list)).unwrap().into_iter().map(|s| s.name).collect::<Vec<_>>();
    assert_eq!(names(work.id), ["To do", "Doing", "Done"]);
    let task = store.add_task("Paint")?;
    store.set_list(task.id, Some(work.id))?;
    store.set_section(task.id, Some(doing.id))?;
    store.move_list_section(doing.id, Some(home.id))?;
    assert_eq!(names(work.id), ["To do", "Done"]);
    assert_eq!(names(home.id), ["Doing"]);
    let moved = store.task(task.id)?.unwrap();
    assert_eq!((moved.list_id, moved.section_id), (Some(home.id), Some(doing.id)));
    // On to the Inbox, tasks and all.
    store.move_list_section(doing.id, None)?;
    assert_eq!(store.list_sections(None)?.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Doing"]);
    let moved = store.task(task.id)?.unwrap();
    assert_eq!((moved.list_id, moved.section_id), (None, Some(doing.id)));
    Ok(())
}

#[test]
fn inbox_sections_migration_keeps_tasks_in_their_sections() -> Result<()> {
    // A database just before the inbox sections' migration, with a task in a list's section.
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", true)?;
    let before = MIGRATIONS.iter().position(|sql| sql.contains("PRAGMA foreign_keys = OFF")).unwrap();
    for sql in &MIGRATIONS[..before] {
        conn.execute_batch(sql)?;
    }
    conn.pragma_update(None, "user_version", before as i64)?;
    conn.execute_batch(
        "INSERT INTO lists (id, name, created_at) VALUES ('l', 'Home', 0);
         INSERT INTO list_sections (id, list_id, name, created_at) VALUES ('s', 'l', 'Kitchen', 0);",
    )?;
    let store = Store { conn, attachments_dir: None };
    let task = store.add_task("Dishes")?;
    store.conn.execute("UPDATE tasks SET list_id = 'l', section_id = 's' WHERE id = ?1", [task.id.to_string()])?;
    Store::migrate(&store.conn)?;
    let section: Option<String> = store.conn.query_row("SELECT section_id FROM tasks WHERE id = ?1", [task.id.to_string()], |r| r.get(0))?;
    assert_eq!(section.as_deref(), Some("s"), "the task keeps its section");
    let fk: bool = store.conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
    assert!(fk, "the foreign keys are on again");
    Ok(())
}

#[test]
fn inbox_sections() -> Result<()> {
    let store = Store::open_in_memory()?;
    let tasks = store.add_list_section(None, "Tasks")?;
    let notes = store.add_list_section(None, "Notes")?;
    let milk = store.add_task("Milk")?;
    store.set_section(milk.id, Some(tasks.id))?;
    assert_eq!(store.list_sections(None)?, [tasks.clone(), notes]);
    assert_eq!(store.task(milk.id)?.unwrap().section_id, Some(tasks.id));
    let work = store.create_list("Work")?;
    assert!(store.list_sections(Some(work.id))?.is_empty(), "the Inbox's sections are its own");
    Ok(())
}

#[test]
fn list_sections() -> Result<()> {
    let store = Store::open_in_memory()?;
    let home = store.create_list("Home")?;
    let work = store.create_list("Work")?;
    let kitchen = store.add_list_section(Some(home.id), " Kitchen ")?;
    assert!(store.add_list_section(Some(home.id), " ").is_err());
    assert_eq!(store.list_sections(Some(home.id))?, std::slice::from_ref(&kitchen));
    assert!(store.list_sections(Some(work.id))?.is_empty());

    let task = store.add_task("fix the tap")?;
    assert!(store.set_section(task.id, Some(kitchen.id)).is_err(), "the task is not in that list");
    store.set_list(task.id, Some(home.id))?;
    store.set_section(task.id, Some(kitchen.id))?;
    assert_eq!(store.task(task.id)?.unwrap().section_id, Some(kitchen.id));
    // Moving to another list leaves the section.
    store.set_list(task.id, Some(work.id))?;
    assert_eq!(store.task(task.id)?.unwrap().section_id, None);

    store.rename_list_section(kitchen.id, " Kitchen & pantry ")?;
    assert!(store.rename_list_section(kitchen.id, "").is_err());
    assert_eq!(store.list_sections(Some(home.id))?[0].name, "Kitchen & pantry");
    store.set_list(task.id, Some(home.id))?;
    store.set_section(task.id, Some(kitchen.id))?;
    store.delete_list_section(kitchen.id)?;
    assert_eq!(store.task(task.id)?.unwrap().section_id, None);
    assert!(store.list_sections(Some(home.id))?.is_empty());
    Ok(())
}

#[test]
fn completing_a_repeating_task_can_be_undone() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("water the plants")?;
    store.set_due(task.id, Some(d(2026, 10, 18)), None)?;
    store.set_repeat(task.id, Some(crate::Recurrence::new(crate::Frequency::Daily, 1)), d(2026, 10, 18))?;
    let copy = store.complete(task.id, true)?.expect("a repeating task leaves a completed copy");
    assert_eq!(store.task(task.id)?.unwrap().due_date, Some(d(2026, 10, 19)));
    store.undo_repeat(task.id, copy, Some(d(2026, 10, 18)), None, None)?;
    assert_eq!(store.task(task.id)?.unwrap().due_date, Some(d(2026, 10, 18)));
    assert!(store.task(copy)?.is_none());
    assert_eq!(store.complete(store.add_task("once")?.id, true)?, None);
    Ok(())
}

#[test]
fn pinning() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("pay rent")?;
    store.set_pinned(task.id, true)?;
    assert!(store.task(task.id)?.unwrap().pinned_at.is_some());
    store.set_pinned(task.id, false)?;
    assert_eq!(store.task(task.id)?.unwrap().pinned_at, None);
    Ok(())
}

#[test]
fn task_activity_is_recorded() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("water the plants")?;
    store.set_title(task.id, "water all the plants")?;
    store.set_due(task.id, Some(d(2026, 10, 20)), None)?;
    store.set_priority(task.id, crate::Priority::High)?;
    store.set_completed(task.id, true)?;
    store.set_completed(task.id, false)?;
    store.delete_task(task.id)?;
    store.restore_task(task.id)?;
    let kinds: Vec<String> = store.task_activity(task.id)?.into_iter().map(|a| a.kind).collect();
    assert_eq!(kinds, ["created", "renamed", "date", "priority", "completed", "reopened", "deleted", "restored"]);

    // The list's log: newest first, only its own tasks.
    let home = store.create_list("Home")?;
    let other = store.add_task("other")?;
    store.set_list(other.id, Some(home.id))?;
    let inbox = store.list_activity(None, 3)?;
    assert_eq!(inbox.len(), 3);
    assert_eq!((inbox[0].0.kind.as_str(), inbox[0].2.as_str()), ("restored", "water all the plants"));
    let in_home: Vec<String> = store.list_activity(Some(home.id), 10)?.into_iter().map(|(a, _, _)| a.kind).collect();
    assert_eq!(in_home, ["moved", "created"]);
    Ok(())
}

#[test]
fn edits_update_the_modified_time() -> Result<()> {
    let store = Store::open_in_memory()?;
    let a = store.add_task("a")?;
    let b = store.add_task("b")?;
    assert_eq!(store.task(a.id)?.unwrap().modified_at, None);
    store.set_title(a.id, "a, renamed")?;
    let modified = store.task(a.id)?.unwrap().modified_at;
    assert!(modified.is_some_and(|m| m >= a.created_at - 1000));
    // Reordering is not an edit.
    store.move_task(b.id, a.id, false)?;
    assert_eq!(store.task(b.id)?.unwrap().modified_at, None);
    store.set_tags(b.id, &["home".to_owned()])?;
    assert!(store.task(b.id)?.unwrap().modified_at.is_some());
    Ok(())
}

#[test]
fn durations_count_on_every_day() -> Result<()> {
    let store = Store::open_in_memory()?;
    let trip = store.add_task("trip")?;
    store.set_due(trip.id, Some(d(2026, 10, 20)), None)?;
    store.set_due_end(trip.id, Some((d(2026, 10, 23), None)))?;
    let trip = store.task(trip.id)?.unwrap();
    assert!(!trip.occurs_on(d(2026, 10, 19)));
    assert!(trip.occurs_on(d(2026, 10, 20)) && trip.occurs_on(d(2026, 10, 22)) && trip.occurs_on(d(2026, 10, 23)));
    assert!(!trip.occurs_on(d(2026, 10, 24)));
    // A week view starting after the trip began still shows it.
    assert_eq!(store.tasks_due_between(d(2026, 10, 22), d(2026, 10, 28))?.len(), 1);
    assert!(store.tasks_due_between(d(2026, 10, 24), d(2026, 10, 30))?.is_empty());
    Ok(())
}

#[test]
fn durations_move_with_repeats() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("conference")?;
    let t = |h| NaiveTime::from_hms_opt(h, 0, 0).unwrap();
    store.set_due(task.id, Some(d(2026, 10, 20)), Some(t(9)))?;
    store.set_due_end(task.id, Some((d(2026, 10, 22), Some(t(17)))))?;
    let stored = store.task(task.id)?.unwrap();
    assert_eq!((stored.end_date, stored.end_time), (Some(d(2026, 10, 22)), Some(t(17))));
    assert_eq!(store.duplicate_task(task.id)?.end_date, Some(d(2026, 10, 22)));
    store.set_repeat(task.id, Some(crate::Recurrence::new(crate::Frequency::Weekly, 1)), d(2026, 10, 18))?;
    store.set_completed(task.id, true)?;
    let next = store.task(task.id)?.unwrap();
    assert_eq!((next.due_date, next.end_date), (Some(d(2026, 10, 27)), Some(d(2026, 10, 29))));
    store.set_due_end(task.id, None)?;
    assert_eq!(store.task(task.id)?.unwrap().end_date, None);
    Ok(())
}

#[test]
fn reminders_are_stored_and_kept_by_repeats() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("water the plants")?;
    store.set_due(task.id, Some(d(2026, 10, 20)), None)?;
    store.set_repeat(task.id, Some(crate::Recurrence::new(crate::Frequency::Daily, 1)), d(2026, 10, 18))?;
    store.set_reminders(task.id, &[0, 24 * 60])?;
    assert_eq!(store.task(task.id)?.unwrap().reminders, [0, 24 * 60]);
    assert_eq!(store.duplicate_task(task.id)?.reminders, [0, 24 * 60]);
    // Completing a repeating task keeps the reminders on the next occurrence only.
    store.set_completed(task.id, true)?;
    let tasks = store.tasks()?;
    let done = tasks.iter().find(|t| t.is_completed()).unwrap();
    assert!(done.reminders.is_empty());
    assert_eq!(store.task(task.id)?.unwrap().reminders, [0, 24 * 60]);
    store.set_reminders(task.id, &[])?;
    assert!(store.task(task.id)?.unwrap().reminders.is_empty());
    Ok(())
}

#[test]
fn habit_goals_and_notes() -> Result<()> {
    let store = Store::open_in_memory()?;
    let water = store.add_habit("Water")?;
    let goal = Goal::new(8.0, "glasses");
    store.update_habit(water.id, "Drink water", Some(&goal))?;
    assert!(store.update_habit(water.id, "Drink water", Some(&Goal::new(0.0, ""))).is_err());
    let habit = store.habits()?.remove(0);
    assert_eq!((habit.name.as_str(), habit.goal), ("Drink water", Some(goal)));

    store.toggle_checkin(water.id, d(2026, 10, 18))?;
    assert!(store.set_checkin_note(water.id, d(2026, 10, 17), "no check-in").is_err());
    store.set_checkin_note(water.id, d(2026, 10, 18), " Felt great ")?;
    let records = store.checkin_records(water.id)?;
    assert_eq!(
        records,
        [Checkin {
            date: d(2026, 10, 18),
            value: 8.0,
            goal: 8.0,
            status: CheckinStatus::Done,
            note: Some("Felt great".into()),
            mood: 0,
        }]
    );

    // Partial amounts count towards the goal but do not complete the day.
    let day = d(2026, 10, 17);
    assert_eq!(store.step_checkin(water.id, day)?, 1.0);
    assert_eq!(store.step_checkin(water.id, day)?, 2.0);
    assert_eq!(store.checkins(water.id)?, [d(2026, 10, 18)].into());
    for _ in 2..8 {
        store.step_checkin(water.id, day)?;
    }
    assert_eq!(store.checkins(water.id)?, [d(2026, 10, 17), d(2026, 10, 18)].into());
    assert_eq!(store.step_checkin(water.id, day)?, 0.0, "a completed day starts over");

    store.update_habit(water.id, "Drink water", None)?;
    assert_eq!(store.habits()?[0].goal, None);

    let t = |h| NaiveTime::from_hms_opt(h, 0, 0).unwrap();
    store.set_habit_reminders(water.id, &[t(21), t(9), t(21)])?;
    assert_eq!(store.habits()?[0].reminders, [t(9), t(21)]);
    store.set_habit_reminders(water.id, &[])?;
    assert!(store.habits()?[0].reminders.is_empty());

    let weekly = crate::habit::Frequency::Weekly { days: 3 };
    assert_eq!(store.habits()?[0].frequency, Default::default());
    store.set_habit_frequency(water.id, weekly)?;
    assert_eq!(store.habits()?[0].frequency, weekly);

    store.set_habit_look(water.id, Some(" 💧 "), Some("#06B6D4"))?;
    let habit = store.habits()?.remove(0);
    assert_eq!((habit.icon.as_deref(), habit.color.as_deref()), (Some("💧"), Some("#06B6D4")));
    store.set_habit_look(water.id, Some("Water"), None)?;
    assert_eq!(store.habits()?[0].icon.as_deref(), Some("Wa"));
    store.set_habit_look(water.id, Some(""), None)?;
    assert_eq!(store.habits()?[0].icon, None);

    store.set_habit_span(water.id, Some(d(2026, 10, 1)), Some(21))?;
    let habit = store.habits()?.remove(0);
    assert_eq!((habit.start_date, habit.goal_days), (Some(d(2026, 10, 1)), Some(21)));
    store.set_habit_section(water.id, crate::habit::HabitSection::Morning)?;
    store.set_habit_archived(water.id, true)?;
    let habit = store.habits()?.remove(0);
    assert_eq!((habit.section, habit.archived), (crate::habit::HabitSection::Morning, true));
    assert!(!habit.icon_checkins);
    store.set_habit_icon_checkins(water.id, true)?;
    assert!(store.habits()?[0].icon_checkins);
    assert!(!habit.constant_reminder);
    store.set_habit_constant_reminder(water.id, true)?;
    assert!(store.habits()?[0].constant_reminder);
    store.set_habit_archived(water.id, false)?;
    assert!(!store.habits()?[0].archived);

    assert!(store.add_habit_section(" ").is_err());
    let pets = store.add_habit_section(" Pets ")?;
    assert_eq!(store.habit_sections()?, std::slice::from_ref(&pets));
    store.set_habit_section(water.id, crate::habit::HabitSection::Custom(pets.id))?;
    assert_eq!(store.habits()?[0].section, crate::habit::HabitSection::Custom(pets.id));
    store.rename_habit_section(pets.id, " Animals ")?;
    assert_eq!(store.habit_sections()?[0].name, "Animals");
    assert!(store.rename_habit_section(pets.id, "").is_err());
    store.delete_habit_section(pets.id)?;
    assert!(store.habit_sections()?.is_empty());
    assert_eq!(store.habits()?[0].section, crate::habit::HabitSection::Others);
    store.set_habit_span(water.id, None, Some(0))?;
    let habit = store.habits()?.remove(0);
    assert_eq!((habit.start_date, habit.goal_days), (None, None));
    Ok(())
}

#[test]
fn focus_sessions_are_stored_newest_first() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("write report")?;
    store.add_focus_session(Some(task.id), "write report", 1_000, 1_500_000)?;
    store.add_focus_session(None, "reading", 2_000_000, 3_500_000)?;
    assert!(store.add_focus_session(None, "x", 10, 5).is_err());
    store.add_focus_session_as(None, "timed", 4_000_000, 4_100_000, true)?;
    assert!(store.focus_sessions()?[0].stopwatch);
    store.set_focus_note(store.focus_sessions()?[0].id, " went well ")?;
    assert_eq!(store.focus_sessions()?[0].note, "went well");
    store.delete_focus_session(store.focus_sessions()?[0].id)?;
    let sessions = store.focus_sessions()?;
    assert_eq!(sessions.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["reading", "write report"]);
    assert_eq!(sessions[1].task_id, Some(task.id));
    // Deleting the task keeps the record.
    store.purge_task(task.id)?;
    assert_eq!(store.focus_sessions()?[1].task_id, None);
    // A timer can also finish after its task has already been deleted.
    let finished = store.add_focus_session_as(Some(task.id), "write report", 5_000_000, 5_300_000, true)?;
    assert_eq!(finished.task_id, None);
    assert_eq!(store.focus_sessions()?[0], finished);
    let all = store.focus_sessions()?;
    store.delete_focus_session(all[0].id)?;
    assert_eq!(store.focus_sessions()?.len(), all.len() - 1);
    store.delete_all_focus_sessions()?;
    assert!(store.focus_sessions()?.is_empty());
    Ok(())
}

#[test]
fn wont_do_leaves_open_views() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = d(2026, 10, 18);
    let task = store.add_task("learn the banjo")?;
    store.set_due(task.id, Some(today), None)?;
    store.set_wont_do(task.id, true)?;
    assert!(store.tasks_in(&View::Inbox, today)?.is_empty());
    assert!(store.tasks_in(&View::Today, today)?.is_empty());
    assert!(store.tasks_in(&View::Completed, today)?.is_empty());
    assert_eq!(store.tasks_in(&View::WontDo, today)?.len(), 1);
    assert_eq!(store.counts(today)?.today, 0);
    store.set_wont_do(task.id, false)?;
    assert_eq!(store.tasks_in(&View::Today, today)?.len(), 1);
    Ok(())
}

#[test]
fn duplicate_copies_fields_tags_and_checklist() -> Result<()> {
    let store = Store::open_in_memory()?;
    let list = store.create_list("Home")?;
    let task = store.add_task("pack")?;
    store.set_list(task.id, Some(list.id))?;
    store.set_priority(task.id, Priority::High)?;
    store.set_tags(task.id, &["trip".into()])?;
    let item = store.add_checklist_item(task.id, "socks", None)?;
    store.set_checklist_item_completed(item.id, true)?;
    let other = store.add_task("other")?;

    let copy = store.duplicate_task(task.id)?;
    assert_ne!(copy.id, task.id);
    assert_eq!((copy.title.as_str(), copy.priority, copy.list_id), ("pack", Priority::High, Some(list.id)));
    assert_eq!(copy.tags, ["trip"]);
    assert_eq!(copy.checklist, Some((1, 1)));
    // The copy sits right below the original.
    assert_eq!(titles(&store.tasks()?), ["other", "pack", "pack"]);
    let _ = other;
    Ok(())
}

#[test]
fn day_view_lists_the_days_tasks() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = chrono::Local::now().date_naive();
    let day = today + Duration::days(3);
    let due = store.add_task("due that day")?;
    store.set_due(due.id, Some(day), None)?;
    let done = store.add_task("done that day")?;
    store.set_due(done.id, Some(day), None)?;
    store.set_completed(done.id, true)?;
    store.add_task("undated")?;
    let titles: Vec<String> = store.tasks_in(&View::Day(day), today)?.into_iter().map(|t| t.title).collect();
    assert_eq!(titles.len(), 2);
    assert!(titles.contains(&"due that day".to_owned()) && titles.contains(&"done that day".to_owned()));
    Ok(())
}

#[test]
fn typed_check_in_amounts() -> Result<()> {
    let store = Store::open_in_memory()?;
    let water = store.add_habit("Water")?;
    store.update_habit(water.id, "Water", Some(&Goal::new(8.0, "glasses")))?;
    let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    store.set_checkin_amount(water.id, day, 5.0)?;
    assert_eq!(store.step_checkin(water.id, day)?, 6.0);
    store.set_checkin_amount(water.id, day, 0.0)?;
    assert_eq!(store.step_checkin(water.id, day)?, 1.0);
    Ok(())
}

#[test]
fn task_draft_commits_duration_section_checklist_and_files_together() -> Result<()> {
    let store = Store::open_in_memory()?;
    let list = store.create_list("Work")?;
    let section = store.insert_list_section(Some(list.id), "Next", None)?;
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("brief.txt");
    std::fs::write(&source, "Task brief")?;
    let attachments_dir = dir.path().join("attachments");
    let mut prepared = PreparedAttachment::copy(Uuid::now_v7(), &source, &attachments_dir)?;
    assert_eq!(prepared.path().extension().unwrap(), "txt");
    let destination = prepared.metadata().path(&attachments_dir);
    let fields = TaskDraft {
        title: "Plan".into(), list_id: Some(list.id), section_id: Some(section.id),
        due_date: NaiveDate::from_ymd_opt(2028, 7, 6), due_time: NaiveTime::from_hms_opt(9, 0, 0),
        end_date: NaiveDate::from_ymd_opt(2028, 7, 7), end_time: NaiveTime::from_hms_opt(10, 0, 0),
        tags: vec!["work".into()], reminders: vec![15],
        checklist: vec![("First".into(), true), ("Second".into(), false)],
        ..Default::default()
    };
    store.conn.execute_batch("CREATE TEMP TRIGGER reject_attachment BEFORE INSERT ON task_attachments BEGIN SELECT RAISE(ABORT, 'file unavailable'); END;")?;
    assert!(store.create_task_with_attachments(fields.clone(), true, std::slice::from_mut(&mut prepared), &attachments_dir).is_err());
    assert!(store.tasks()?.is_empty());
    assert!(!destination.exists());
    assert!(prepared.path().exists(), "a failed save keeps the staged copy for retry");
    store.conn.execute_batch("DROP TRIGGER reject_attachment")?;
    let task = store.create_task_with_attachments(fields.clone(), true, std::slice::from_mut(&mut prepared), &attachments_dir)?;
    assert_eq!(task.section_id, Some(section.id));
    assert_eq!(task.end_date, fields.end_date);
    assert_eq!(task.end_time, fields.end_time);
    assert_eq!(task.checklist, Some((1, 2)));
    assert_eq!(task.tags, ["work"]);
    assert_eq!(task.reminders, [15]);
    assert_eq!(store.attachments(task.id)?.len(), 1);
    assert_eq!(std::fs::read_to_string(destination)?, "Task brief");
    assert!(store.create_task(TaskDraft { section_id: Some(section.id), ..Default::default() }, true).is_err());
    assert_eq!(store.tasks()?.len(), 1, "an invalid section cannot leave an extra task");
    Ok(())
}

#[test]
fn task_draft_rolls_back_files_published_before_a_collision() -> Result<()> {
    let store = Store::open_in_memory()?;
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("plan.txt");
    let files = dir.path().join("attachments");
    std::fs::write(&source, "plan")?;
    let provisional_id = Uuid::now_v7();
    let mut staged = [
        PreparedAttachment::copy(provisional_id, &source, &files)?,
        PreparedAttachment::copy(provisional_id, &source, &files)?,
    ];
    let first = staged[0].metadata().path(&files);
    let second = staged[1].metadata().path(&files);
    std::fs::write(&second, "existing file")?;
    let draft = TaskDraft { title: "Plan".into(), ..TaskDraft::default() };
    assert!(store.create_task_with_attachments(draft.clone(), true, &mut staged, &files).is_err());
    assert!(store.tasks()?.is_empty());
    assert!(!first.exists());
    assert_eq!(std::fs::read_to_string(&second)?, "existing file");
    assert!(staged.iter().all(|attachment| attachment.path().exists()));
    std::fs::remove_file(&second)?;
    let task = store.create_task_with_attachments(draft, true, &mut staged, &files)?;
    assert_eq!(store.attachments(task.id)?.len(), 2);
    assert_eq!(std::fs::read_to_string(first)?, "plan");
    assert_eq!(std::fs::read_to_string(second)?, "plan");
    Ok(())
}

#[test]
fn task_creation_rolls_back_when_tag_insertion_fails() -> Result<()> {
    let store = Store::open_in_memory()?;
    store.conn.execute_batch(
        "CREATE TEMP TRIGGER reject_tag BEFORE INSERT ON task_tags
         BEGIN SELECT RAISE(ABORT, 'tag could not be saved'); END;",
    )?;
    let draft = || TaskDraft {
        title: "Submit once".into(),
        tags: vec!["work".into()],
        ..TaskDraft::default()
    };
    assert!(store.create_task(draft(), true).is_err());
    assert!(store.tasks()?.is_empty());
    assert!(store.changes_since(0)?.0.is_empty());

    store.conn.execute_batch("DROP TRIGGER reject_tag")?;
    let task = store.create_task(draft(), true)?;
    assert_eq!(store.tasks()?, [task]);
    Ok(())
}

#[test]
fn attachments_are_copied_and_removed() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("fern-attach-test-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir)?;
    let source = dir.join("receipt.txt");
    std::fs::write(&source, "paid")?;
    let store = Store::open_in_memory()?;
    let task = store.add_task("Pay rent")?;
    let folder = dir.join("attachments");
    let attachment = store.add_attachment(task.id, &source, &folder)?;
    assert_eq!((attachment.name.as_str(), attachment.size), ("receipt.txt", 4));
    assert_eq!(std::fs::read_to_string(attachment.path(&folder))?, "paid");
    assert_eq!(store.attachments(task.id)?, std::slice::from_ref(&attachment));
    store.delete_attachment(&attachment, &folder)?;
    assert!(store.attachments(task.id)?.is_empty());
    assert!(!attachment.path(&folder).exists());
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn directory_attachment_keeps_nested_files_and_empty_folders() -> Result<()> {
    use std::io::Read as _;

    let dir = tempfile::tempdir()?;
    let source = dir.path().join("trip");
    std::fs::create_dir_all(source.join("plans/empty"))?;
    std::fs::write(source.join("plans/tickets.txt"), "seat 14C")?;
    let store = Store::open_in_memory()?;
    let task = store.add_task("Pack for the trip")?;
    let folder = source.join("attachments");

    let attachment = store.add_attachment(task.id, &source, &folder)?;
    assert_eq!(attachment.name, "trip.zip");
    assert_eq!(store.attachments(task.id)?, std::slice::from_ref(&attachment));
    let mut zip = zip::ZipArchive::new(std::fs::File::open(attachment.path(&folder))?)?;
    let mut ticket = String::new();
    zip.by_name("trip/plans/tickets.txt")?.read_to_string(&mut ticket)?;
    assert_eq!(ticket, "seat 14C");
    assert!(zip.by_name("trip/plans/empty/")?.is_dir());
    assert!(zip.by_name("trip/attachments/.pending/").is_err());
    drop(zip);
    store.delete_attachment(&attachment, &folder)?;
    assert!(!attachment.path(&folder).exists());
    Ok(())
}

#[test]
fn oversized_attachments_leave_no_file_or_record() -> Result<()> {
    use crate::attachment::{AttachmentTooLarge, MAX_ATTACHMENT_BYTES};
    use std::io::Write as _;

    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source");
    let attachments = dir.path().join("attachments");
    std::fs::create_dir_all(&source)?;
    let file_path = source.join("data.bin");
    let mut file = std::fs::File::create(&file_path)?;
    file.set_len(MAX_ATTACHMENT_BYTES + 1)?;
    let store = Store::open_in_memory()?;
    let task = store.add_task("Has attachments")?;
    let error = store.add_attachment(task.id, &file_path, &attachments).unwrap_err();
    assert!(error.is::<AttachmentTooLarge>());

    // Incompressible data makes the finished ZIP exceed the same limit.
    let mut state = 1u64;
    let mut block = [0; 64 * 1024];
    for _ in 0..(MAX_ATTACHMENT_BYTES / block.len() as u64 + 1) {
        for bytes in block.as_chunks_mut::<8>().0 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            bytes.copy_from_slice(&state.to_le_bytes());
        }
        file.write_all(&block)?;
    }
    drop(file);
    let error = store.add_attachment(task.id, &source, &attachments).unwrap_err();
    assert!(error.is::<AttachmentTooLarge>());
    assert!(store.attachments(task.id)?.is_empty());
    assert_eq!(attachments.read_dir()?.count(), 1, "only the staging directory remains");
    assert_eq!(attachments.join(".pending").read_dir()?.count(), 0);
    Ok(())
}

#[test]
fn focus_timers() -> Result<()> {
    use crate::focus::FocusTimer;
    let store = Store::open_in_memory()?;
    let mut timer = FocusTimer::new("Planning");
    assert_eq!((timer.minutes, timer.stopwatch), (25, false));
    store.save_focus_timer(&timer)?;
    timer.name = "Weekly planning".into();
    timer.minutes = 45;
    timer.icon = Some("📚".into());
    timer.color = Some("#4772fa".into());
    store.save_focus_timer(&timer)?;
    assert_eq!(store.focus_timers(false)?, [timer.clone()]);
    store.set_focus_timer_archived(timer.id, true)?;
    assert!(store.focus_timers(false)?.is_empty());
    assert_eq!(store.focus_timers(true)?[0].id, timer.id);
    store.delete_focus_timer(timer.id)?;
    assert!(store.focus_timers(true)?.is_empty());
    assert!(store.save_focus_timer(&FocusTimer::new(" ")).is_err());
    Ok(())
}

#[test]
fn deleting_chosen_focus_records() -> Result<()> {
    let store = Store::open_in_memory()?;
    let a = store.add_focus_session(None, "a", 0, 60_000)?;
    let b = store.add_focus_session(None, "b", 120_000, 180_000)?;
    let c = store.add_focus_session(None, "c", 240_000, 300_000)?;
    store.delete_focus_sessions(&[a.id, c.id])?;
    let left: Vec<Id> = store.focus_sessions()?.into_iter().map(|s| s.id).collect();
    assert_eq!(left, [b.id]);
    Ok(())
}

#[test]
fn countdown_look() -> Result<()> {
    let store = Store::open_in_memory()?;
    let release = store.add_countdown("Release", chrono::NaiveDate::from_ymd_opt(2026, 11, 19).unwrap(), CountdownKind::Countdown)?;
    store.set_countdown_look(release.id, Some("🚀"), Some("#4c8bf5"))?;
    let saved = store.countdowns()?.remove(0);
    assert_eq!((saved.icon.as_deref(), saved.color.as_deref()), (Some("🚀"), Some("#4c8bf5")));
    let monthly = crate::recurrence::Recurrence::new(crate::recurrence::Frequency::Monthly, 1);
    store.update_countdown(release.id, "Release day", saved.date, CountdownKind::Countdown, Some(&monthly))?;
    let edited = store.countdowns()?.remove(0);
    assert_eq!((edited.name.as_str(), edited.repeat), ("Release day", Some(monthly)));
    assert!(store.update_countdown(release.id, " ", saved.date, CountdownKind::Countdown, None).is_err());
    let other = store.add_countdown("Trip", saved.date, CountdownKind::Countdown)?;
    assert_eq!(store.countdowns()?[1].id, other.id);
    store.set_countdown_pinned(other.id, true)?;
    let pinned = store.countdowns()?;
    assert_eq!(pinned[0].id, other.id, "a pinned countdown comes first");
    assert!(pinned[0].pinned_at.is_some());
    store.set_countdown_pinned(other.id, false)?;
    assert_eq!(store.countdowns()?[0].id, release.id);
    store.set_countdown_look(release.id, Some("  "), None)?;
    let saved = store.countdowns()?.remove(0);
    assert_eq!((saved.icon, saved.color), (None, None));
    store.set_countdown_show_age(release.id, true)?;
    assert!(store.countdowns()?[0].show_age);
    store.set_countdown_reminders(release.id, &[3, 0, 3], &[], true)?;
    let saved = store.countdowns()?.remove(0);
    assert_eq!((saved.reminders, saved.constant_reminder), (vec![0, 3], true));
    store.set_countdown_display(release.id, crate::countdown::ShowInList::Always, true, true)?;
    let saved = store.countdowns()?.remove(0);
    assert_eq!((saved.show_in_list, saved.plus_one_day, saved.count_up), (crate::countdown::ShowInList::Always, true, true));
    store.set_countdown_archived(release.id, true)?;
    assert!(store.countdowns()?.iter().find(|c| c.id == release.id).unwrap().archived);
    store.set_countdown_archived(release.id, false)?;
    assert!(!store.countdowns()?.iter().find(|c| c.id == release.id).unwrap().archived);

    // A background picture is copied into the folder and named after the countdown.
    let dir = std::env::temp_dir().join(format!("fern-backgrounds-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir)?;
    let photo = dir.join("photo.jpg");
    std::fs::write(&photo, b"jpeg")?;
    let name = store.set_countdown_background(release.id, Some(&photo), &dir.join("files"))?.unwrap();
    assert!(name.starts_with(&format!("{}-background-", release.id)) && name.ends_with(".jpg"));
    assert!(dir.join("files").join(&name).exists());
    assert_eq!(store.countdowns()?.iter().find(|c| c.id == release.id).unwrap().background.as_deref(), Some(name.as_str()));
    store.set_countdown_background(release.id, None, &dir)?;
    assert!(store.countdowns()?.iter().find(|c| c.id == release.id).unwrap().background.is_none());
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn reminder_plus_one_and_snooze() -> Result<()> {
    let store = Store::open_in_memory()?;
    let water = store.add_habit("Water")?;
    store.update_habit(water.id, "Water", Some(&Goal::new(2.0, "glasses")))?;
    let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    assert_eq!(store.add_checkin_amount(water.id, day, 1.0)?, 1.0);
    assert_eq!(store.add_checkin_amount(water.id, day, 1.0)?, 2.0);
    assert_eq!(store.add_checkin_amount(water.id, day, 1.0)?, 3.0, "+1 goes past the goal");

    let at = |h, m| day.and_hms_opt(h, m, 0).unwrap();
    store.snooze_habit(water.id, at(9, 15))?;
    assert_eq!(store.habit_snoozes()?, [(water.id, at(9, 15))]);
    // Snoozing the next day drops the old snooze.
    store.snooze_habit(water.id, at(9, 15) + Duration::days(2))?;
    assert_eq!(store.habit_snoozes()?.len(), 1);
    store.delete_habit(water.id)?;
    assert!(store.habit_snoozes()?.is_empty());
    Ok(())
}

#[test]
fn habit_draft_modes_logs_skips_and_ordering() -> Result<()> {
    let store = Store::open_in_memory()?;
    let day1 = d(2026, 10, 18);
    let day2 = d(2026, 10, 19);
    let draft = HabitDraft {
        name: "Run".into(),
        goal: Some(Goal { amount: 5.0, unit: "km".into(), mode: CheckinMode::Auto { step: 2.5 } }),
        reminders: vec![NaiveTime::from_hms_opt(8, 0, 0).unwrap()],
        frequency: crate::habit::Frequency::default(),
        icon: Some("🏃".into()),
        color: Some("#22C55E".into()),
        start_date: Some(day1),
        goal_days: Some(21),
        section: HabitSection::Morning,
        constant_reminder: true,
        record_enable: true,
    };
    let run = store.create_habit(&draft)?;
    assert_eq!(run.goal.as_ref().unwrap().mode, CheckinMode::Auto { step: 2.5 });
    assert!(run.record_enable && run.constant_reminder);

    // Auto step increments by 2.5 -> partial then done.
    assert_eq!(store.step_checkin(run.id, day1)?, 2.5);
    assert!(store.checkins(run.id)?.is_empty());
    assert_eq!(store.step_checkin(run.id, day1)?, 5.0);
    assert_eq!(store.checkins(run.id)?, [day1].into());

    // Habit Log with mood survives check-in reset.
    store.set_checkin_log(run.id, day1, Some("Great pace"), 50)?;
    store.reset_checkin(run.id, day1, true)?;
    let rec = store.checkin_records(run.id)?;
    assert_eq!(rec.len(), 1);
    assert_eq!(rec[0].status, CheckinStatus::Unlabeled);
    assert_eq!(rec[0].value, 0.0);
    assert_eq!(rec[0].mood, 50);
    assert_eq!(rec[0].note.as_deref(), Some("Great pace"));

    // Deleting log when unchecked removes the empty row.
    store.delete_checkin_log(run.id, day1)?;
    assert!(store.checkin_records(run.id)?.is_empty());

    // Standalone log without check-in, then mark Undone.
    store.set_checkin_log(run.id, day2, Some("Rainy"), 20)?;
    store.mark_checkin_undone(run.id, day2)?;
    let rec2 = &store.checkin_records(run.id)?[0];
    assert_eq!(rec2.status, CheckinStatus::Undone);
    assert_eq!(rec2.mood, 20);
    assert_eq!(rec2.note.as_deref(), Some("Rainy"));

    // Skipping a day adds to ex_dates; checking in un-skips it.
    store.skip_habit_date(run.id, day1)?;
    assert!(store.habits()?[0].is_skipped(day1));
    store.toggle_checkin(run.id, day1)?;
    assert!(!store.habits()?[0].is_skipped(day1));
    assert!(store.checkins(run.id)?.contains(&day1));

    // Batch query returns records and done dates.
    assert_eq!(store.all_habit_checkins()?.get(&run.id).map(Vec::len), Some(2));
    assert_eq!(store.all_habit_done_dates()?.get(&run.id), Some(&HashSet::from([day1])));

    // Reordering habits and custom sections persists.
    let read = store.add_habit("Read")?;
    let sec_a = store.add_habit_section("Evening")?;
    let sec_b = store.add_habit_section("Work")?;
    store.reorder_habit_sections(&[sec_b.id, sec_a.id])?;
    assert_eq!(store.habit_sections()?.iter().map(|s| s.id).collect::<Vec<_>>(), [sec_b.id, sec_a.id]);
    store.reorder_habits(&[(read.id, HabitSection::Custom(sec_b.id), 0), (run.id, HabitSection::Custom(sec_b.id), 1)])?;
    assert_eq!(store.habits()?.iter().map(|h| h.id).collect::<Vec<_>>(), [read.id, run.id]);

    // Focus session linked to habit persists habit_id.
    let sess = store.add_focus_session_for(None, Some(run.id), "Run", 1000, 1000 + 25 * 60_000, false)?;
    assert_eq!(sess.habit_id, Some(run.id));
    assert_eq!(store.focus_sessions()?[0].habit_id, Some(run.id));
    Ok(())
}

#[test]
fn daily_backups() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("fern-daily-test-{}", Uuid::now_v7()));
    let store = Store::open_in_memory()?;
    store.add_task("first")?;
    let day = |d: u32| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
    store.daily_backup(&dir, day(1), 2)?;
    // A second start the same day keeps the morning's copy.
    store.add_task("second")?;
    store.daily_backup(&dir, day(1), 2)?;
    Store::check_backup(&dir.join("fern-2026-10-01.db"))?;
    let copy = Store::open(dir.join("fern-2026-10-01.db"))?;
    assert_eq!(copy.tasks()?.len(), 1);
    drop(copy);
    // Only the newest copies stay.
    store.daily_backup(&dir, day(2), 2)?;
    store.daily_backup(&dir, day(5), 2)?;
    let mut names: Vec<String> = std::fs::read_dir(&dir)?.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["fern-2026-10-02.db", "fern-2026-10-05.db"]);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn backup_and_restore() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("fern-backup-test-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("backup.db");

    let store = Store::open_in_memory()?;
    let task = store.add_task("kept in the backup")?;
    let source = dir.join("ticket.txt");
    std::fs::write(&source, "seat 14C")?;
    let attachments = dir.join("attachments");
    let attachment = store.add_attachment(task.id, &source, &attachments)?;
    store.backup_to(&file, Some(&attachments))?;

    // Restoring elsewhere brings the tasks and the attached files back, and the folder
    // loses what the backup does not have.
    let mut other = Store::open_in_memory()?;
    other.add_task("replaced")?;
    let other_files = dir.join("other-attachments");
    std::fs::create_dir_all(&other_files)?;
    std::fs::write(other_files.join("stale.txt"), "old")?;
    other.restore_from(&file, Some(&other_files))?;
    let titles: Vec<String> = other.tasks()?.into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["kept in the backup"]);
    assert_eq!(other.attachments(task.id)?, std::slice::from_ref(&attachment));
    assert_eq!(std::fs::read_to_string(attachment.path(&other_files))?, "seat 14C");
    assert!(!other_files.join("stale.txt").exists());
    let leftover: bool = other
        .conn
        .query_row("SELECT 1 FROM sqlite_master WHERE name = 'backup_files'", [], |_| Ok(true))
        .optional()?
        .unwrap_or(false);
    assert!(!leftover, "the files table does not stay in the data");

    // Something that is not a backup changes nothing.
    let junk = dir.join("junk.db");
    Connection::open(&junk)?.execute_batch("CREATE TABLE notes (x TEXT);")?;
    assert!(other.restore_from(&junk, None).is_err());
    assert_eq!(other.tasks()?.len(), 1);
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

#[test]
fn estimates() -> Result<()> {
    use crate::model::Estimate;
    let store = Store::open_in_memory()?;
    let task = store.add_task("write the report")?;
    assert_eq!(store.task(task.id)?.unwrap().estimate, None);
    store.set_estimate(task.id, Some(Estimate::Pomos(3)))?;
    assert_eq!(store.task(task.id)?.unwrap().estimate, Some(Estimate::Pomos(3)));
    store.set_estimate(task.id, Some(Estimate::Minutes(90)))?;
    assert_eq!(store.task(task.id)?.unwrap().estimate, Some(Estimate::Minutes(90)));
    store.set_estimate(task.id, None)?;
    assert_eq!(store.task(task.id)?.unwrap().estimate, None);
    assert_eq!(Estimate::from_storage("p:0"), None);
    Ok(())
}

#[test]
fn new_tasks_at_the_top_or_bottom() -> Result<()> {
    let store = Store::open_in_memory()?;
    store.add_task("first")?;
    store.add_task_at("top", true)?;
    store.add_task_at("bottom", false)?;
    let titles: Vec<String> = store.tasks_in(&View::Inbox, chrono::Local::now().date_naive())?.into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["top", "first", "bottom"]);
    Ok(())
}

#[test]
fn settings_roundtrip() -> Result<()> {
    let store = Store::open_in_memory()?;
    assert_eq!(store.setting("locale")?, None);
    store.set_setting("locale", "ru")?;
    store.set_setting("locale", "en")?;
    assert_eq!(store.setting("locale")?.as_deref(), Some("en"));
    Ok(())
}

#[test]
fn subtasks_are_created_in_the_parent_list() -> Result<()> {
    let store = Store::open_in_memory()?;
    let list = store.create_list("Trip")?;
    let parent = store.add_task("pack")?;
    store.set_list(parent.id, Some(list.id))?;
    let first = store.add_subtask(parent.id, "passport")?;
    let second = store.add_subtask(parent.id, "charger")?;
    assert_eq!(store.task(first.id)?.unwrap().list_id, Some(list.id));
    assert_eq!(titles(&store.subtasks(parent.id)?), ["passport", "charger"]);

    store.delete_task(second.id)?;
    assert_eq!(titles(&store.subtasks(parent.id)?), ["passport"]);
    assert!(store.add_subtask(Uuid::now_v7(), "orphan").is_err());
    Ok(())
}

#[test]
fn subtasks_cascade_and_no_cycles() -> Result<()> {
    let store = Store::open_in_memory()?;
    let parent = store.add_task("plan a trip")?;
    let child = store.add_task("book flights")?;
    let grandchild = store.add_task("compare prices")?;
    store.set_parent(child.id, Some(parent.id))?;
    store.set_parent(grandchild.id, Some(child.id))?;
    assert_eq!(store.task(child.id)?.unwrap().parent_id, Some(parent.id));

    assert!(store.set_parent(parent.id, Some(grandchild.id)).is_err());
    assert!(store.set_parent(parent.id, Some(parent.id)).is_err());

    // Linking a parent moves the task into the parent's list.
    let list = store.create_list("Home")?;
    let other = store.add_task("other")?;
    store.set_list(parent.id, Some(list.id))?;
    store.link_parent(other.id, parent.id)?;
    let other = store.task(other.id)?.unwrap();
    assert_eq!((other.parent_id, other.list_id), (Some(parent.id), Some(list.id)));
    assert!(store.link_parent(parent.id, grandchild.id).is_err());

    store.purge_task(parent.id)?;
    assert!(store.tasks()?.is_empty());
    Ok(())
}

#[test]
fn list_and_folder_drag_reordering_and_grouping() -> Result<()> {
    let store = Store::open_in_memory()?;
    let a = store.create_list("A")?;
    let b = store.create_list("B")?;
    let c = store.create_list("C")?;
    let d = store.create_list("D")?;

    let names = |store: &Store| -> Result<Vec<(String, Option<Id>)>> {
        Ok(store.lists()?.into_iter().map(|l| (l.name, l.folder)).collect())
    };

    // Drop D before B.
    store.place_list_relative(d.id, b.id, false, None)?;
    assert_eq!(
        names(&store)?,
        vec![("A".into(), None), ("D".into(), None), ("B".into(), None), ("C".into(), None)]
    );

    // Drop C onto A (middle zone) -> creates a folder at A's position containing A then C.
    let f1 = store.group_lists_into_folder(c.id, a.id, "New Folder")?;
    assert_eq!(
        names(&store)?,
        vec![
            ("A".into(), Some(f1.id)),
            ("C".into(), Some(f1.id)),
            ("D".into(), None),
            ("B".into(), None),
        ]
    );

    // Cannot group onto a list already inside a folder.
    assert!(store.group_lists_into_folder(d.id, a.id, "Nested").is_err());

    // Move folder f1 after B.
    store.reorder_list_folder(f1.id, FolderDropTarget::List(b.id), true)?;
    assert_eq!(
        names(&store)?,
        vec![
            ("D".into(), None),
            ("B".into(), None),
            ("A".into(), Some(f1.id)),
            ("C".into(), Some(f1.id)),
        ]
    );

    // Place D before folder f1 (out of any folder) and C out after folder f1.
    store.place_list_around_folder(d.id, f1.id, false)?;
    assert_eq!(
        names(&store)?,
        vec![
            ("B".into(), None),
            ("D".into(), None),
            ("A".into(), Some(f1.id)),
            ("C".into(), Some(f1.id)),
        ]
    );
    store.place_list_around_folder(c.id, f1.id, true)?;
    assert_eq!(
        names(&store)?,
        vec![
            ("B".into(), None),
            ("D".into(), None),
            ("A".into(), Some(f1.id)),
            ("C".into(), None),
        ]
    );

    // Create second folder f2 around B and D, then move f2 after f1.
    let f2 = store.group_lists_into_folder(d.id, b.id, "Second")?;
    store.reorder_list_folder(f2.id, FolderDropTarget::Folder(f1.id), true)?;
    assert_eq!(
        names(&store)?,
        vec![
            ("A".into(), Some(f1.id)),
            ("B".into(), Some(f2.id)),
            ("D".into(), Some(f2.id)),
            ("C".into(), None),
        ]
    );

    // Empty folders reorder among themselves via list_folders sort_order.
    let e1 = store.add_list_folder("Empty 1")?;
    let e2 = store.add_list_folder("Empty 2")?;
    store.reorder_list_folder(e2.id, FolderDropTarget::Folder(e1.id), false)?;
    let empty_order: Vec<Id> = store
        .list_folders()?
        .into_iter()
        .filter(|f| f.id == e1.id || f.id == e2.id)
        .map(|f| f.id)
        .collect();
    assert_eq!(empty_order, vec![e2.id, e1.id]);
    Ok(())
}

#[test]
fn tag_drag_reordering_nesting_and_promotion() -> Result<()> {
    let store = Store::open_in_memory()?;
    store.add_tag("work", None, None)?;
    store.add_tag("home", None, None)?;
    store.add_tag("errands", None, None)?;
    store.add_tag("urgent", None, None)?;

    let top_order = |store: &Store| -> Result<Vec<String>> {
        Ok(store.tags()?.into_iter().filter(|t| t.parent.is_none()).map(|t| t.name).collect())
    };
    let children_of = |store: &Store, parent: &str| -> Result<Vec<String>> {
        Ok(store.tags()?.into_iter().filter(|t| t.parent.as_deref() == Some(parent)).map(|t| t.name).collect())
    };

    // Nest errands and urgent under work.
    store.move_tag("errands", "work", false, true)?;
    store.move_tag("urgent", "work", false, true)?;
    assert_eq!(top_order(&store)?, vec!["work", "home"]);
    assert_eq!(children_of(&store, "work")?, vec!["errands", "urgent"]);

    // Reorder sub-tags inside work.
    store.move_tag("urgent", "errands", false, false)?;
    assert_eq!(children_of(&store, "work")?, vec!["urgent", "errands"]);

    // 2-level nesting is rejected: parent tag cannot nest under home, nor sub-tag under sub-tag.
    store.move_tag("work", "home", false, true)?;
    assert_eq!(store.tags()?.into_iter().find(|t| t.name == "work").unwrap().parent, None);
    store.move_tag("errands", "urgent", false, true)?;
    assert_eq!(store.tags()?.into_iter().find(|t| t.name == "errands").unwrap().parent.as_deref(), Some("work"));

    // Move parent tag work after home.
    store.move_tag("work", "home", true, false)?;
    assert_eq!(top_order(&store)?, vec!["home", "work"]);
    assert_eq!(children_of(&store, "work")?, vec!["urgent", "errands"]);

    // Promote sub-tag errands to top-level before home.
    store.move_tag("errands", "home", false, false)?;
    assert_eq!(top_order(&store)?, vec!["errands", "home", "work"]);
    assert_eq!(children_of(&store, "work")?, vec!["urgent"]);

    // Drop a sub-tag on Tags heading promotes it to first.
    store.move_tag_first("urgent")?;
    assert_eq!(top_order(&store)?, vec!["urgent", "errands", "home", "work"]);

    // Renaming a tag preserves its custom order.
    store.update_tag("home", "house", None, None)?;
    assert_eq!(top_order(&store)?, vec!["urgent", "errands", "house", "work"]);

    // Pinning boundary: dragging an unpinned tag onto a pinned tag adopts pinned state.
    store.set_tag_pinned("urgent", true)?;
    store.move_tag("work", "urgent", false, false)?;
    assert!(store.tags()?.into_iter().find(|t| t.name == "work").unwrap().pinned);
    Ok(())
}

#[test]
fn filter_drag_reordering() -> Result<()> {
    use crate::filter::{FilterRules, SmartFilter};
    let store = Store::open_in_memory()?;
    let f1 = SmartFilter { id: Uuid::now_v7(), name: "First".into(), rules: FilterRules::default() };
    let f2 = SmartFilter { id: Uuid::now_v7(), name: "Second".into(), rules: FilterRules::default() };
    let f3 = SmartFilter { id: Uuid::now_v7(), name: "Third".into(), rules: FilterRules::default() };
    store.save_filter(&f1)?;
    store.save_filter(&f2)?;
    store.save_filter(&f3)?;

    let names = |store: &Store| -> Result<Vec<String>> {
        Ok(store.filters()?.into_iter().map(|f| f.name).collect())
    };
    assert_eq!(names(&store)?, vec!["First", "Second", "Third"]);

    store.move_filter(f3.id, f1.id, false)?;
    assert_eq!(names(&store)?, vec!["Third", "First", "Second"]);

    store.move_filter(f3.id, f2.id, true)?;
    assert_eq!(names(&store)?, vec!["First", "Second", "Third"]);

    store.move_filter_first(f2.id)?;
    assert_eq!(names(&store)?, vec!["Second", "First", "Third"]);

    store.set_filter_order(&[f3.id, f1.id, f2.id])?;
    assert_eq!(names(&store)?, vec!["Third", "First", "Second"]);
    Ok(())
}

#[test]
fn batch_save_list_folder_and_tag_order_commits_atomically() -> Result<()> {
    let store = Store::open_in_memory()?;
    let a = store.create_list("A")?;
    let b = store.create_list("B")?;
    let c = store.create_list("C")?;
    let f1 = store.add_list_folder("F1")?;
    let f2 = store.add_list_folder("F2")?;

    let mut lists = store.lists()?;
    lists.iter_mut().find(|l| l.id == b.id).unwrap().folder = Some(f1.id);
    // Reorder to [C, B(in F1), A] and folders [F2, F1].
    let ordered_lists = vec![
        lists.iter().find(|l| l.id == c.id).unwrap().clone(),
        lists.iter().find(|l| l.id == b.id).unwrap().clone(),
        lists.iter().find(|l| l.id == a.id).unwrap().clone(),
    ];
    store.save_list_and_folder_order(&ordered_lists, &[f2.clone(), f1.clone()])?;

    let saved_lists: Vec<(String, Option<Id>)> =
        store.lists()?.into_iter().map(|l| (l.name, l.folder)).collect();
    assert_eq!(
        saved_lists,
        vec![("C".into(), None), ("B".into(), Some(f1.id)), ("A".into(), None)]
    );
    let saved_folders: Vec<Id> = store.list_folders()?.into_iter().map(|f| f.id).collect();
    assert_eq!(saved_folders, vec![f2.id, f1.id]);

    // Re-saving an identical order must not dirty any sync_log rows.
    store.conn.execute("DELETE FROM sync_log", [])?;
    store.save_list_and_folder_order(&store.lists()?, &store.list_folders()?)?;
    let dirty: i64 = store.conn.query_row("SELECT COUNT(*) FROM sync_log", [], |r| r.get(0))?;
    assert_eq!(dirty, 0, "no-op order save leaves sync_log untouched");

    store.add_tag("t1", None, None)?;
    store.add_tag("t2", None, None)?;
    store.add_tag("t3", None, None)?;
    let mut tags = store.tags()?;
    tags.iter_mut().find(|t| t.name == "t3").unwrap().pinned = true;
    tags.iter_mut().find(|t| t.name == "t1").unwrap().parent = Some("t2".into());
    let ordered_tags = vec![
        tags.iter().find(|t| t.name == "t3").unwrap().clone(),
        tags.iter().find(|t| t.name == "t2").unwrap().clone(),
        tags.iter().find(|t| t.name == "t1").unwrap().clone(),
    ];
    store.save_tag_hierarchy_and_order(&ordered_tags)?;
    let reloaded = store.tags()?;
    let summary: Vec<(String, Option<String>, bool)> =
        reloaded.into_iter().map(|t| (t.name, t.parent, t.pinned)).collect();
    assert_eq!(
        summary,
        vec![
            ("t3".into(), None, true),
            ("t2".into(), None, false),
            ("t1".into(), Some("t2".into()), false),
        ]
    );
    Ok(())
}

#[test]
fn nested_tasks_depth_unlink_move_notes_and_completion_cascade() -> Result<()> {
    let store = Store::open_in_memory()?;
    // 5 levels: 0 -> 1 -> 2 -> 3 -> 4 (MAX_SUBTASK_DEPTH).
    let l0 = store.add_task("L0")?;
    let l1 = store.add_subtask(l0.id, "L1")?;
    let l2 = store.add_subtask(l1.id, "L2")?;
    let l3 = store.add_subtask(l2.id, "L3")?;
    let l4 = store.add_subtask(l3.id, "L4")?;
    assert_eq!(store.task_depth(l0.id)?, 0);
    assert_eq!(store.task_depth(l4.id)?, MAX_SUBTASK_DEPTH);
    assert!(store.add_subtask(l4.id, "L5").is_err(), "cannot nest past level 5");

    // Linking a 3-level subtree under L2 flattens overflow descendants onto level 5 (depth 4).
    let sub_root = store.add_task("SR")?;
    let sub_child = store.add_subtask(sub_root.id, "SC")?;
    let sub_grand = store.add_subtask(sub_child.id, "SG")?;
    store.link_parent(sub_root.id, l2.id)?;
    assert_eq!(store.task_depth(sub_root.id)?, 3);
    assert_eq!(store.task_depth(sub_child.id)?, 4);
    assert_eq!(store.task_depth(sub_grand.id)?, 4);
    assert_eq!(store.task(sub_grand.id)?.unwrap().parent_id, Some(sub_root.id));

    // Notes cannot have subtasks, cannot be nested, and nested tasks cannot become notes.
    let note = store.add_task("My note")?;
    store.set_kind(note.id, Kind::Note)?;
    assert!(store.add_subtask(note.id, "nope").is_err());
    assert!(store.set_parent(note.id, Some(l0.id)).is_err());
    assert!(store.set_kind(l0.id, Kind::Note).is_err());
    assert!(store.set_kind(l1.id, Kind::Note).is_err());

    // Unlink promotes a subtask to top-level while keeping its own children attached.
    store.unlink_parent(sub_root.id)?;
    assert_eq!(store.task(sub_root.id)?.unwrap().parent_id, None);
    assert_eq!(store.task(sub_child.id)?.unwrap().parent_id, Some(sub_root.id));

    // Moving a subtask to another list unlinks it from its old parent and moves its subtree.
    let other = store.create_list("Other")?;
    store.set_list(sub_child.id, Some(other.id))?;
    assert_eq!(store.task(sub_child.id)?.unwrap().parent_id, None);
    assert_eq!(store.task(sub_child.id)?.unwrap().list_id, Some(other.id));

    // Moving a subtask to another section unlinks it from its parent in the old section.
    let sec = store.add_list_section(Some(other.id), "Done soon")?;
    let sec_child = store.add_subtask(sub_child.id, "In section")?;
    store.set_section(sec_child.id, Some(sec.id))?;
    assert_eq!(store.task(sec_child.id)?.unwrap().parent_id, None);
    assert_eq!(store.task(sec_child.id)?.unwrap().section_id, Some(sec.id));

    // Completing a parent cascades to open descendants; reopening a subtask reopens ancestors.
    let p = store.add_task("Parent")?;
    let c1 = store.add_subtask(p.id, "Child 1")?;
    let c2 = store.add_subtask(p.id, "Child 2")?;
    store.set_completed(c1.id, true)?;
    let (_, cascaded) = store.complete_with_cascaded(p.id, true)?;
    assert_eq!(cascaded, vec![c2.id]);
    assert!(store.task(p.id)?.unwrap().is_completed());
    assert!(store.task(c2.id)?.unwrap().is_completed());
    // Undo reopens only parent and newly cascaded c2, leaving pre-completed c1 completed.
    store.reopen_tasks(&[p.id, c2.id])?;
    assert!(!store.task(p.id)?.unwrap().is_completed());
    assert!(store.task(c1.id)?.unwrap().is_completed());
    assert!(!store.task(c2.id)?.unwrap().is_completed());
    // Completing p again and reopening c2 reopens p as well.
    store.set_completed(p.id, true)?;
    store.set_completed(c2.id, false)?;
    assert!(!store.task(p.id)?.unwrap().is_completed());
    assert!(!store.task(c2.id)?.unwrap().is_completed());

    // move_task_in_tree nests and unnests cleanly.
    let a = store.add_task("A")?;
    let b = store.add_task("B")?;
    store.move_task_in_tree(b.id, Some(a.id), Some((a.id, true)))?;
    assert_eq!(store.task(b.id)?.unwrap().parent_id, Some(a.id));
    store.move_task_in_tree(b.id, None, Some((a.id, false)))?;
    assert_eq!(store.task(b.id)?.unwrap().parent_id, None);
    let ids: Vec<Id> = store.tasks()?.into_iter().map(|t| t.id).collect();
    let pos_b = ids.iter().position(|&id| id == b.id).unwrap();
    let pos_a = ids.iter().position(|&id| id == a.id).unwrap();
    assert!(pos_b < pos_a, "unnested B moved before A");
    Ok(())
}

#[test]
fn link_parent_at_max_depth_syncs_flattened_descendants_list_section_and_completion() -> Result<()> {
    let store = Store::open_in_memory()?;
    let work = store.create_list("Work")?;
    let sec = store.add_list_section(Some(work.id), "Active")?;
    let l0 = store.add_task("L0")?;
    store.set_list(l0.id, Some(work.id))?;
    store.set_section(l0.id, Some(sec.id))?;
    let l1 = store.add_subtask(l0.id, "L1")?;
    let l2 = store.add_subtask(l1.id, "L2")?;
    let l3 = store.add_subtask(l2.id, "L3")?; // depth 3 == MAX_SUBTASK_DEPTH - 1
    store.set_completed(l3.id, true)?;

    // Source subtree lives in Inbox (list = None, section = None), open.
    let src_root = store.add_task("SR")?;
    let src_child = store.add_subtask(src_root.id, "SC")?;
    let src_grand = store.add_subtask(src_child.id, "SG")?;

    store.link_parent(src_root.id, l3.id)?;
    // Flattened children are reparented onto l3, and MUST also move into Work/sec and close.
    for id in [src_root.id, src_child.id, src_grand.id] {
        let t = store.task(id)?.unwrap();
        assert_eq!(t.parent_id, Some(l3.id));
        assert_eq!(t.list_id, Some(work.id));
        assert_eq!(t.section_id, Some(sec.id));
        assert!(t.is_completed());
    }
    Ok(())
}

#[test]
fn restore_subtask_detaches_from_trashed_parent_and_survives_empty_trash() -> Result<()> {
    let store = Store::open_in_memory()?;
    let parent = store.add_task("Parent")?;
    let child = store.add_subtask(parent.id, "Child")?;
    let grand = store.add_subtask(child.id, "Grand")?;

    store.delete_task(parent.id)?;
    // Restore only child; it must detach from trashed parent so empty_trash cannot CASCADE it.
    store.restore_task(child.id)?;
    assert_eq!(store.task(child.id)?.unwrap().parent_id, None);
    assert_eq!(store.task(grand.id)?.unwrap().parent_id, Some(child.id));

    store.empty_trash()?;
    assert!(store.task(parent.id)?.is_none());
    assert!(store.task(child.id)?.is_some());
    assert!(store.task(grand.id)?.is_some());
    // Cannot attach a subtask to a trashed parent.
    let doomed = store.add_task("Doomed")?;
    store.delete_task(doomed.id)?;
    assert!(store.add_subtask(doomed.id, "Nope").is_err());
    assert!(store.set_parent(child.id, Some(doomed.id)).is_err());
    Ok(())
}

#[test]
fn reorder_list_sections_and_move_task_avoid_redundant_sync_log_writes() -> Result<()> {
    let store = Store::open_in_memory()?;
    let list = store.create_list("Board")?;
    let s1 = store.add_list_section(Some(list.id), "S1")?;
    let s2 = store.add_list_section(Some(list.id), "S2")?;
    let s3 = store.add_list_section(Some(list.id), "S3")?;

    store.reorder_list_sections(Some(list.id), s3.id, s1.id)?;
    let order: Vec<Id> = store.list_sections(Some(list.id))?.into_iter().map(|s| s.id).collect();
    assert_eq!(order, vec![s3.id, s1.id, s2.id]);

    // Reordering two adjacent tasks in a 5-task DB should only dirty changed sort_orders.
    let t1 = store.add_task("T1")?;
    let t2 = store.add_task("T2")?;
    let t3 = store.add_task("T3")?;
    let t4 = store.add_task("T4")?;
    store.move_task(t1.id, t2.id, true)?; // normalizes sort_orders to 0, 1024, 2048, 3072
    store.conn.execute("DELETE FROM sync_log", [])?;
    store.move_task(t3.id, t4.id, true)?; // swaps only last two (or first two), unchanged rows skipped
    let dirty: i64 = store.conn.query_row("SELECT COUNT(*) FROM sync_log WHERE entity = 'tasks'", [], |r| r.get(0))?;
    assert!(dirty <= 2, "expected at most 2 dirty sync_log rows, got {dirty}");
    Ok(())
}

#[test]
fn descendants_terminates_on_synced_cycle_and_same_list_preserves_section() -> Result<()> {
    let store = Store::open_in_memory()?;
    let list = store.create_list("Proj")?;
    let sec = store.add_list_section(Some(list.id), "Backlog")?;
    let p = store.add_task("P")?;
    store.set_list(p.id, Some(list.id))?;
    store.set_section(p.id, Some(sec.id))?;
    let c = store.add_subtask(p.id, "C")?;
    // Calling set_list with the same list must preserve section_id and parent_id.
    store.set_list(c.id, Some(list.id))?;
    let c_after = store.task(c.id)?.unwrap();
    assert_eq!(c_after.parent_id, Some(p.id));
    assert_eq!(c_after.section_id, Some(sec.id));

    // Simulate a corrupted cycle introduced by concurrent sync and verify descendants terminates.
    store.conn.execute(
        "UPDATE tasks SET parent_id = ?2 WHERE id = ?1",
        params![p.id.to_string(), c.id.to_string()],
    )?;
    let desc = store.descendants(p.id)?;
    assert!(desc.contains(&c.id));
    assert!(desc.len() <= 2);
    Ok(())
}

#[test]
fn in_progress_location_timezone_snooze_and_batch_operations() -> Result<()> {
    let store = Store::open_in_memory()?;
    let a = store.add_task("Read paper")?;
    let b = store.add_task("Draft notes")?;
    let c = store.add_task("Review PR")?;

    // Explicit InProgress + activity trigger + cleared on close.
    store.set_in_progress(a.id, true)?;
    assert!(store.task(a.id)?.unwrap().is_in_progress());
    assert!(store.task_activity(a.id)?.iter().any(|entry| entry.kind == "in_progress"));
    store.set_completed(a.id, true)?;
    assert!(!store.task(a.id)?.unwrap().is_in_progress());
    store.set_completed(a.id, false)?;

    // Location + Search matching location.
    store.set_location(a.id, Some("  Library Desk 4  "))?;
    assert_eq!(store.task(a.id)?.unwrap().location.as_deref(), Some("Library Desk 4"));
    let hits = store.tasks_in(&View::Search("library".into()), d(2026, 10, 18))?;
    assert_eq!(hits.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a.id]);

    // Timezone validation & floating flag.
    assert!(store.set_timezone(a.id, Some("Not/A_Zone"), false).is_err());
    store.set_timezone(a.id, Some("America/New_York"), true)?;
    let loaded = store.task(a.id)?.unwrap();
    assert_eq!(loaded.timezone.as_deref(), Some("America/New_York"));
    assert!(loaded.is_floating);

    // Snooze + constant reminder.
    let snooze_at = d(2026, 10, 18).and_hms_opt(10, 15, 0).unwrap();
    store.snooze_task(a.id, snooze_at)?;
    store.set_task_constant_reminder(a.id, true)?;
    let loaded = store.task(a.id)?.unwrap();
    assert_eq!(loaded.snoozed_until, Some(snooze_at));
    assert!(loaded.constant_reminder);
    store.clear_task_snooze(a.id)?;
    assert_eq!(store.task(a.id)?.unwrap().snoozed_until, None);

    // Batch operations.
    let ids = [a.id, b.id];
    store.batch_set_priority(&ids, Priority::High)?;
    store.batch_add_tag(&ids, "urgent")?;
    store.batch_set_in_progress(&ids, true)?;
    assert_eq!(store.task(a.id)?.unwrap().priority, Priority::High);
    assert_eq!(store.task(b.id)?.unwrap().tags, vec!["urgent"]);
    assert!(store.task(b.id)?.unwrap().is_in_progress());
    assert!(!store.task(c.id)?.unwrap().is_in_progress());
    store.batch_remove_tag(&ids, "urgent")?;
    assert!(store.task(a.id)?.unwrap().tags.is_empty());

    let work = store.create_list("Work")?;
    store.batch_set_list(&ids, Some(work.id))?;
    assert_eq!(store.task(b.id)?.unwrap().list_id, Some(work.id));
    store.batch_delete(&ids)?;
    assert!(store.task(a.id)?.unwrap().is_deleted());
    store.batch_restore(&ids)?;
    assert!(!store.task(a.id)?.unwrap().is_deleted());
    store.batch_purge(&[b.id])?;
    assert!(store.task(b.id)?.is_none());
    Ok(())
}

#[test]
fn holiday_skip_and_forgetting_curve_advance() -> Result<()> {
    let store = Store::open_in_memory()?;
    store.add_countdown("Holiday", d(2026, 10, 19), CountdownKind::Holiday)?;
    let task = store.add_task("Vocab")?;
    store.set_due(task.id, Some(d(2026, 10, 16)), None)?;
    let mut fc = crate::Recurrence::forgetting_curve();
    fc.skip_holidays = true;
    fc.skip_weekends = true;
    store.set_repeat(task.id, Some(fc.clone()), d(2026, 10, 16))?;
    let copy = store.complete(task.id, true)?.unwrap();
    let after = store.task(task.id)?.unwrap();
    // +1 day = Sat Oct 17 -> skips weekend to Mon Oct 19 -> skips holiday to Tue Oct 20.
    assert_eq!(after.due_date, Some(d(2026, 10, 20)));
    assert_eq!(after.repeat.as_ref().map(|r| r.cycle), Some(1));
    store.undo_repeat(task.id, copy, Some(d(2026, 10, 16)), None, Some(&fc))?;
    let reverted = store.task(task.id)?.unwrap();
    assert_eq!(reverted.due_date, Some(d(2026, 10, 16)));
    assert_eq!(reverted.repeat.as_ref().map(|r| r.cycle), Some(0));
    Ok(())
}

#[test]
fn resize_dates_and_timed_range_update_span() -> Result<()> {
    let store = Store::open_in_memory()?;
    let t = store.add_task("Workshop")?;
    store.set_due(t.id, Some(d(2026, 11, 10)), None)?;

    // Expand all-day span to 3 days, then shrink back to single day.
    store.resize_dates(t.id, d(2026, 11, 10), d(2026, 11, 12))?;
    let loaded = store.task(t.id)?.unwrap();
    assert_eq!(loaded.due_date, Some(d(2026, 11, 10)));
    assert_eq!(loaded.end_date, Some(d(2026, 11, 12)));

    store.resize_dates(t.id, d(2026, 11, 11), d(2026, 11, 11))?;
    let single = store.task(t.id)?.unwrap();
    assert_eq!(single.due_date, Some(d(2026, 11, 11)));
    assert_eq!(single.end_date, None);

    // Resize timed range on a day.
    let s = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
    let e = NaiveTime::from_hms_opt(11, 30, 0).unwrap();
    store.resize_timed_range(t.id, d(2026, 11, 11), s, e, None)?;
    let timed = store.task(t.id)?.unwrap();
    assert_eq!(timed.due_date, Some(d(2026, 11, 11)));
    assert_eq!(timed.due_time, Some(s));
    assert_eq!(timed.end_time, Some(e));
    assert_eq!(timed.end_date, None);
    Ok(())
}

#[test]
fn caldav_event_queue_and_snapshot_preserve_dirty_edits() -> Result<()> {
    let store = Store::open_in_memory()?;
    let sub_id = store.add_caldav_subscription(
        "Work CalDAV",
        "https://dav.example.com/cal/",
        Some("#3B82F6"),
        crate::ical::CalendarProvider::CalDav,
        "alice",
        "pw",
        true,
    )?;
    let remote_ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:r1\r\nSUMMARY:Remote\r\nDTSTART:20261110T090000\r\nDTEND:20261110T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let local_ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:l1\r\nSUMMARY:Offline Draft\r\nDTSTART:20261111T100000\r\nDTEND:20261111T110000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    assert!(store.replace_caldav_snapshot(
        sub_id,
        "https://dav.example.com/cal/",
        &[crate::ical::StoredEvent {
            subscription_id: sub_id,
            uid: "r1".into(),
            href: "/cal/r1.ics".into(),
            etag: Some("\"e1\"".into()),
            ics: remote_ics.into(),
            dirty: crate::ical::EventDirty::Clean,
        }],
        Some("tok-1"),
    )?);

    store.upsert_local_calendar_event(sub_id, "l1", "/cal/l1.ics", local_ics)?;
    // Concurrent snapshot must keep dirty local edit l1 intact.
    store.replace_caldav_snapshot(sub_id, "https://dav.example.com/cal/", &[], Some("tok-2"))?;
    let events = store.calendar_events(sub_id)?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].uid, "l1");
    assert_eq!(events[0].dirty, crate::ical::EventDirty::Upsert);

    // Marking an unpushed draft deleted drops it immediately.
    assert!(store.mark_calendar_event_deleted(sub_id, "l1")?);
    assert!(store.calendar_events(sub_id)?.is_empty());
    Ok(())
}

#[test]
fn batch_merge_tasks_combines_checklist_tags_and_notes() -> Result<()> {
    let store = Store::open_in_memory()?;
    let a = store.add_task("Trip prep")?;
    store.set_tags(a.id, &["travel".into()])?;
    store.set_notes(a.id, "Main note")?;
    let b = store.add_task("Pack passport")?;
    store.set_tags(b.id, &["Travel".into(), "urgent".into()])?;
    store.set_notes(b.id, "Passport note")?;
    let sub = store.add_checklist_item(b.id, "Check expiry", None)?;
    store.set_checklist_item_completed(sub.id, true)?;
    let c = store.add_task("Book taxi")?;
    store.set_completed(c.id, true)?;

    let merged_id = store.batch_merge_tasks(&[a.id, b.id, c.id])?;
    assert_eq!(merged_id, Some(a.id));

    let merged = store.task(a.id)?.unwrap();
    assert_eq!(merged.tags, vec!["travel".to_owned(), "urgent".to_owned()]);
    assert_eq!(merged.notes, "Main note\n\nPassport note");

    let items = store.checklist(a.id)?;
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].title, "Pack passport");
    assert!(!items[0].completed);
    assert_eq!(items[1].title, "Check expiry");
    assert!(items[1].completed);
    assert_eq!(items[2].title, "Book taxi");
    assert!(items[2].completed);

    assert!(store.task(b.id)?.unwrap().is_deleted());
    assert!(store.task(c.id)?.unwrap().is_deleted());
    Ok(())
}

#[test]
fn batch_duplicate_and_convert_kind() -> Result<()> {
    let store = Store::open_in_memory()?;
    let a = store.add_task("Alpha")?;
    let b = store.add_task("Beta")?;

    let copies = store.batch_duplicate(&[a.id, b.id])?;
    assert_eq!(copies.len(), 2);
    assert_eq!(store.tasks()?.len(), 4);

    store.batch_set_kind(&[a.id, b.id], Kind::Note)?;
    assert_eq!(store.task(a.id)?.unwrap().kind, Kind::Note);
    assert_eq!(store.task(b.id)?.unwrap().kind, Kind::Note);

    store.batch_set_kind(&[a.id, b.id], Kind::Task)?;
    assert_eq!(store.task(a.id)?.unwrap().kind, Kind::Task);
    Ok(())
}

#[test]
fn purge_empty_trash_and_delete_list_unlink_attachments_and_prune_sync_uploaded() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("tasks.db");
    let attachments_dir = dir.path().join("attachments");
    let source = dir.path().join("doc.txt");
    std::fs::write(&source, "hello attachment")?;

    let store = Store::open(&db_path)?;
    assert_eq!(store.attachments_dir(), Some(attachments_dir.as_path()));

    // 1. purge_task unlinks both parent and subtask attachment files and prunes sync_uploaded.
    let parent = store.add_task("Parent")?;
    let child = store.add_subtask(parent.id, "Child")?;
    let att_parent = store.add_attachment(parent.id, &source, &attachments_dir)?;
    let att_child = store.add_attachment(child.id, &source, &attachments_dir)?;
    let name_parent = format!("{}-{}", att_parent.id, att_parent.name);
    let name_child = format!("{}-{}", att_child.id, att_child.name);
    store.mark_files_synced(&[name_parent, name_child])?;
    assert!(att_parent.path(&attachments_dir).exists());
    assert!(att_child.path(&attachments_dir).exists());

    store.purge_task(parent.id)?;
    assert!(!att_parent.path(&attachments_dir).exists());
    assert!(!att_child.path(&attachments_dir).exists());
    let uploaded_count: i64 = store.conn.query_row("SELECT COUNT(*) FROM sync_uploaded", [], |r| r.get(0))?;
    assert_eq!(uploaded_count, 0);

    // 2. empty_trash unlinks attachments of trashed tasks.
    let trashed = store.add_task("Trashed")?;
    let att_trashed = store.add_attachment(trashed.id, &source, &attachments_dir)?;
    let name_trashed = format!("{}-{}", att_trashed.id, att_trashed.name);
    store.mark_files_synced(std::slice::from_ref(&name_trashed))?;
    store.delete_task(trashed.id)?;
    assert!(att_trashed.path(&attachments_dir).exists(), "soft delete keeps file for restore");
    store.empty_trash()?;
    assert!(!att_trashed.path(&attachments_dir).exists());
    let uploaded_count: i64 = store.conn.query_row("SELECT COUNT(*) FROM sync_uploaded", [], |r| r.get(0))?;
    assert_eq!(uploaded_count, 0);

    // 3. delete_list unlinks attachments of tasks belonging to that list.
    let list = store.create_list("Project")?;
    let in_list = store.add_task("In list")?;
    store.set_list(in_list.id, Some(list.id))?;
    let att_list = store.add_attachment(in_list.id, &source, &attachments_dir)?;
    let name_list = format!("{}-{}", att_list.id, att_list.name);
    store.mark_files_synced(std::slice::from_ref(&name_list))?;
    store.delete_list(list.id)?;
    assert!(!att_list.path(&attachments_dir).exists());
    let uploaded_count: i64 = store.conn.query_row("SELECT COUNT(*) FROM sync_uploaded", [], |r| r.get(0))?;
    assert_eq!(uploaded_count, 0);
    Ok(())
}

#[test]
fn gc_orphan_attachments_cleans_unreferenced_files_and_keeps_live_files_and_countdowns() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let attachments_dir = dir.path().join("attachments");
    let source = dir.path().join("live.txt");
    let photo = dir.path().join("bg.jpg");
    std::fs::write(&source, "live")?;
    std::fs::write(&photo, "jpg")?;

    let store = Store::open_in_memory()?.with_attachments_dir(&attachments_dir);
    let task = store.add_task("Keep")?;
    let att = store.add_attachment(task.id, &source, &attachments_dir)?;
    let countdown = store.add_countdown("Trip", d(2026, 12, 1), CountdownKind::Countdown)?;
    let bg = store.set_countdown_background(countdown.id, Some(&photo), &attachments_dir)?.unwrap();

    // Create an orphan file, a .pending staging dir, and a stale sync_uploaded entry.
    let orphan = attachments_dir.join("orphan-file.txt");
    std::fs::write(&orphan, "stale")?;
    std::fs::create_dir_all(attachments_dir.join(".pending"))?;
    std::fs::write(attachments_dir.join(".pending").join("staged.txt"), "staging")?;
    store.mark_files_synced(&[format!("{}-{}", att.id, att.name), bg.clone(), "orphan-file.txt".into()])?;

    let removed = store.gc_orphan_attachments(&attachments_dir)?;
    assert_eq!(removed, 1);
    assert!(!orphan.exists());
    assert!(att.path(&attachments_dir).exists());
    assert!(attachments_dir.join(&bg).exists());
    assert!(attachments_dir.join(".pending").join("staged.txt").exists());
    let remaining: Vec<String> = store
        .conn
        .prepare("SELECT name FROM sync_uploaded ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    assert_eq!(remaining.len(), 2);
    assert!(!remaining.contains(&"orphan-file.txt".to_owned()));
    Ok(())
}

#[test]
fn tag_family_traverses_multi_level_descendants_and_guards_cycles() -> Result<()> {
    let store = Store::open_in_memory()?;
    // Insert a 3-level hierarchy Work -> Backend -> Rust directly (as synced data could carry).
    store.conn.execute("INSERT INTO tags (name, parent, created_at) VALUES ('Work', NULL, 1)", [])?;
    store.conn.execute("INSERT INTO tags (name, parent, created_at) VALUES ('Backend', 'Work', 2)", [])?;
    store.conn.execute("INSERT INTO tags (name, parent, created_at) VALUES ('Rust', 'Backend', 3)", [])?;
    let task = store.add_task("Write compiler")?;
    store.set_tags(task.id, &["Rust".into()])?;

    let found = store.tasks_in(&View::Tag("Work".into()), d(2026, 10, 18))?;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, task.id);

    // Exact matches_view semantics for View::Tag: completed stays included; deleted and won't-do excluded.
    let done_task = store.add_task("Shipped compiler")?;
    store.set_tags(done_task.id, &["rust".into()])?;
    store.set_completed(done_task.id, true)?;
    let wont_do_task = store.add_task("Dropped")?;
    store.set_tags(wont_do_task.id, &["Backend".into()])?;
    store.set_wont_do(wont_do_task.id, true)?;
    let trashed_task = store.add_task("Deleted")?;
    store.set_tags(trashed_task.id, &["Work".into()])?;
    store.delete_task(trashed_task.id)?;
    let found_ids: HashSet<Id> = store
        .tasks_in(&View::Tag("Work".into()), d(2026, 10, 18))?
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(found_ids, HashSet::from([task.id, done_task.id]));

    // Cycle guard: A -> B -> A must terminate cleanly.
    store.conn.execute("INSERT INTO tags (name, parent, created_at) VALUES ('CycleA', 'CycleB', 4)", [])?;
    store.conn.execute("INSERT INTO tags (name, parent, created_at) VALUES ('CycleB', 'CycleA', 5)", [])?;
    let fam = store.tag_family("CycleA")?;
    assert!(fam.contains("cyclea") && fam.contains("cycleb"));
    Ok(())
}

#[test]
fn nested_in_tx_works_across_store_mutations_and_rolls_back() -> Result<()> {
    let store = Store::open_in_memory()?;
    let res: Result<()> = store.in_tx(|| {
        let task = store.create_task(TaskDraft { title: "Nested".into(), ..Default::default() }, true)?;
        store.set_tags(task.id, &["nested".into()])?;
        let folder = store.add_list_folder("F")?;
        store.ungroup_list_folder(folder.id)?;
        store.insert_list_section(None, "Sec", None)?;
        store.batch_duplicate(&[task.id])?;
        store.batch_set_kind(&[task.id], Kind::Note)?;
        anyhow::bail!("abort outer transaction")
    });
    assert!(res.is_err());
    assert!(store.tasks()?.is_empty());
    assert!(store.list_sections(None)?.is_empty());
    Ok(())
}

#[test]
fn undo_repeat_preserves_completed_copy_if_parent_was_purged() -> Result<()> {
    let store = Store::open_in_memory()?;
    let task = store.add_task("Daily standup")?;
    store.set_due(task.id, Some(d(2026, 10, 18)), None)?;
    store.set_repeat(task.id, Some(Recurrence::new(crate::Frequency::Daily, 1)), d(2026, 10, 18))?;
    let copy = store.complete(task.id, true)?.expect("repeating task creates completed copy");
    store.purge_task(task.id)?;
    // Undoing after parent was purged must NOT delete the completed history record.
    store.undo_repeat(task.id, copy, Some(d(2026, 10, 18)), None, None)?;
    assert!(store.task(copy)?.is_some());
    Ok(())
}

#[test]
fn completing_repeating_task_shifts_start_date_and_supports_start_date_only_recurrence() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = chrono::Local::now().date_naive();
    let tomorrow = today + Duration::days(1);

    // (a) Repeating task with both start_date and due_date shifts both and leaves Things Today.
    let both = store.create_task(
        TaskDraft {
            title: "Daily review".into(),
            due_date: Some(today),
            start_date: Some(today),
            schedule_bucket: ScheduleBucket::Evening,
            ..Default::default()
        },
        true,
    )?;
    store.set_repeat(both.id, Some(Recurrence::new(crate::Frequency::Daily, 1)), today)?;
    assert!(
        store
            .things_tasks(&crate::things::ThingsViewKind::Today, today)?
            .iter()
            .any(|t| t.id == both.id)
    );
    let copy_a = store.complete(both.id, true)?.expect("must create completed occurrence");
    let after_a = store.task(both.id)?.unwrap();
    assert_eq!(after_a.due_date, Some(tomorrow));
    assert_eq!(after_a.start_date, Some(tomorrow));
    assert_eq!(after_a.schedule_bucket, ScheduleBucket::Evening);
    assert!(
        !store
            .things_tasks(&crate::things::ThingsViewKind::Today, today)?
            .iter()
            .any(|t| t.id == both.id),
        "repeating task must leave Today after completion"
    );
    assert!(
        store
            .things_tasks(&crate::things::ThingsViewKind::Upcoming, today)?
            .iter()
            .any(|t| t.id == both.id)
    );
    // Undo restores start_date, due_date, and schedule_bucket.
    store.undo_repeat(both.id, copy_a, Some(today), None, None)?;
    let undone_a = store.task(both.id)?.unwrap();
    assert_eq!(undone_a.due_date, Some(today));
    assert_eq!(undone_a.start_date, Some(today));
    assert_eq!(undone_a.schedule_bucket, ScheduleBucket::Evening);
    assert!(store.task(copy_a)?.is_none());

    // (b) Repeating task with start_date only (due_date = None) repeats instead of closing.
    let start_only = store.create_task(
        TaskDraft {
            title: "Morning stretch".into(),
            start_date: Some(today),
            repeat: Some(Recurrence::new(crate::Frequency::Daily, 1)),
            ..Default::default()
        },
        true,
    )?;
    let copy_b = store
        .complete(start_only.id, true)?
        .expect("start_date-only repeating task must repeat");
    let after_b = store.task(start_only.id)?.unwrap();
    assert!(!after_b.is_completed());
    assert_eq!(after_b.due_date, None);
    assert_eq!(after_b.start_date, Some(tomorrow));
    let copy_b_row = store.task(copy_b)?.unwrap();
    assert!(copy_b_row.is_completed());
    assert_eq!(copy_b_row.start_date, Some(today));
    store.undo_repeat(start_only.id, copy_b, None, None, None)?;
    let undone_b = store.task(start_only.id)?.unwrap();
    assert_eq!(undone_b.start_date, Some(today));
    assert_eq!(undone_b.due_date, None);

    // (c) Due-date-only Evening repeating task clears Evening on advance and restores on undo.
    let due_evening = store.create_task(
        TaskDraft {
            title: "Evening meds".into(),
            due_date: Some(today),
            schedule_bucket: ScheduleBucket::Evening,
            repeat: Some(Recurrence::new(crate::Frequency::Daily, 1)),
            ..Default::default()
        },
        true,
    )?;
    let copy_c = store.complete(due_evening.id, true)?.expect("must repeat");
    let after_c = store.task(due_evening.id)?.unwrap();
    assert_eq!(after_c.due_date, Some(tomorrow));
    assert_eq!(after_c.schedule_bucket, ScheduleBucket::None);
    assert!(
        !store
            .things_tasks(&crate::things::ThingsViewKind::Today, today)?
            .iter()
            .any(|t| t.id == due_evening.id)
    );
    store.undo_repeat(due_evening.id, copy_c, Some(today), None, None)?;
    let undone_c = store.task(due_evening.id)?.unwrap();
    assert_eq!(undone_c.due_date, Some(today));
    assert_eq!(undone_c.schedule_bucket, ScheduleBucket::Evening);
    Ok(())
}

#[test]
fn hidden_lists_excludes_completed_projects_from_smart_lists() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = d(2026, 11, 22);
    let project = store.add_list(&ListDraft { name: "Ship v2".into(), ..Default::default() })?;
    let task = store.create_task(
        TaskDraft { title: "Polish docs".into(), list_id: Some(project.id), due_date: Some(today), ..Default::default() },
        true,
    )?;
    assert!(store.tasks_in(&View::All, today)?.iter().any(|t| t.id == task.id));
    assert!(store.tasks_in(&View::Today, today)?.iter().any(|t| t.id == task.id));

    store.set_list_project_fields(project.id, "", None, Some(123_456))?;
    assert!(!store.tasks_in(&View::All, today)?.iter().any(|t| t.id == task.id));
    assert!(!store.tasks_in(&View::Today, today)?.iter().any(|t| t.id == task.id));
    // Direct list view still displays tasks inside the completed project.
    assert!(store.tasks_in(&View::List(project.id), today)?.iter().any(|t| t.id == task.id));
    Ok(())
}

#[test]
fn concurrent_store_open_migrates_fresh_db_without_racing() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("fern-migrate-race-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir)?;
    let db_path = dir.join("tasks.db");

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let path = db_path.clone();
        let b = std::sync::Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || -> Result<(usize, bool, i64, i64, Vec<String>)> {
            b.wait();
            let store = Store::open(&path)?;
            let ver = store.schema_version()?;
            let fk: bool = store.conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
            let temp_store: i64 = store.conn.pragma_query_value(None, "temp_store", |r| r.get(0))?;
            let cache_size: i64 = store.conn.pragma_query_value(None, "cache_size", |r| r.get(0))?;
            let quick = store.quick_check()?;
            Ok((ver, fk, temp_store, cache_size, quick))
        }));
    }
    for handle in handles {
        let (ver, fk, temp_store, cache_size, quick) = handle.join().expect("thread must not panic")?;
        assert_eq!(ver, SCHEMA_VERSION);
        assert!(fk, "foreign_keys must remain enabled after concurrent migrations");
        assert_eq!(temp_store, 2, "temp_store must be MEMORY (2)");
        assert_eq!(cache_size, -8000, "cache_size must be -8000 KiB");
        assert_eq!(quick, vec!["ok"]);
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn horizon_mutual_exclusion_and_inbox_isolation() -> Result<()> {
    let store = Store::open_in_memory()?;
    let t = |h| NaiveTime::from_hms_opt(h, 0, 0).unwrap();
    let today = d(2026, 11, 23);
    let w48 = Horizon::Week { year: 2026, week: 48 };
    let m12 = Horizon::Month { year: 2026, month: 12 };

    // Creating a task with both due_date and horizon drops horizon (date wins).
    let dated = store.create_task(
        TaskDraft {
            title: "Dated wins".into(),
            due_date: Some(today),
            horizon: Some(w48),
            ..Default::default()
        },
        true,
    )?;
    assert_eq!(dated.due_date, Some(today));
    assert_eq!(dated.horizon, None);
    assert_eq!(store.task(dated.id)?.unwrap().horizon, None);

    // Creating a task with start_date and horizon drops horizon.
    let started = store.create_task(
        TaskDraft {
            title: "Start wins".into(),
            start_date: Some(today),
            horizon: Some(w48),
            ..Default::default()
        },
        true,
    )?;
    assert_eq!(started.start_date, Some(today));
    assert_eq!(started.horizon, None);

    // Creating a task with horizon + schedule_bucket clears schedule_bucket.
    let h_task = store.create_task(
        TaskDraft {
            title: "Horizon task".into(),
            horizon: Some(w48),
            schedule_bucket: ScheduleBucket::Evening,
            ..Default::default()
        },
        true,
    )?;
    assert_eq!(h_task.horizon, Some(w48));
    assert_eq!(h_task.schedule_bucket, ScheduleBucket::None);
    let loaded = store.task(h_task.id)?.unwrap();
    assert_eq!(loaded.horizon, Some(w48));
    assert_eq!(loaded.schedule_bucket, ScheduleBucket::None);

    // Duplicate preserves horizon.
    let dup = store.duplicate_task(h_task.id)?;
    assert_eq!(dup.horizon, Some(w48));

    // Inbox behavior: in classic Tasks, Inbox means "no list", so ungrouped horizon tasks
    // stay visible in View::Inbox and View::All; ThingsViewKind::Inbox stays GTD-clean.
    let inbox_plain = store.add_task("Plain inbox")?;
    let inbox_ids: Vec<Id> = store.tasks_in(&View::Inbox, today)?.into_iter().map(|t| t.id).collect();
    assert!(inbox_ids.contains(&inbox_plain.id));
    assert!(inbox_ids.contains(&h_task.id));
    assert!(store.tasks_in(&View::All, today)?.iter().any(|t| t.id == h_task.id));
    assert!(store.tasks_in(&View::Horizons, today)?.is_empty());
    assert!(!store
        .things_tasks(&crate::things::ThingsViewKind::Inbox, today)?
        .iter()
        .any(|t| t.id == h_task.id));

    // set_due(Some) clears horizon; set_due(None) leaves horizon intact.
    store.set_due(h_task.id, None, None)?;
    assert_eq!(store.task(h_task.id)?.unwrap().horizon, Some(w48));
    store.set_due(h_task.id, Some(today), Some(t(9)))?;
    let after_due = store.task(h_task.id)?.unwrap();
    assert_eq!(after_due.horizon, None);
    assert_eq!(after_due.due_date, Some(today));
    assert_eq!(after_due.due_time, Some(t(9)));

    // set_horizon(Some) clears due_date, due_time, end_date, end_time, start_date, snoozed_until, schedule_bucket.
    store.set_due_end(h_task.id, Some((d(2026, 11, 25), Some(t(17)))))?;
    store.set_task_schedule(h_task.id, Some(today), ScheduleBucket::Evening)?;
    store.snooze_task(h_task.id, today.and_hms_opt(11, 0, 0).unwrap())?;
    store.set_horizon(h_task.id, Some(m12))?;
    let after_h = store.task(h_task.id)?.unwrap();
    assert_eq!(after_h.horizon, Some(m12));
    assert_eq!(after_h.due_date, None);
    assert_eq!(after_h.due_time, None);
    assert_eq!(after_h.end_date, None);
    assert_eq!(after_h.end_time, None);
    assert_eq!(after_h.start_date, None);
    assert_eq!(after_h.snoozed_until, None);
    assert_eq!(after_h.schedule_bucket, ScheduleBucket::None);

    // move_dates, resize_dates, resize_timed_range, and set_task_schedule clear horizon.
    store.move_dates(h_task.id, today)?;
    assert_eq!(store.task(h_task.id)?.unwrap().horizon, None);
    store.set_horizon(h_task.id, Some(m12))?;
    store.resize_dates(h_task.id, today, d(2026, 11, 25))?;
    assert_eq!(store.task(h_task.id)?.unwrap().horizon, None);
    store.set_horizon(h_task.id, Some(m12))?;
    store.resize_timed_range(h_task.id, today, t(9), t(10), None)?;
    assert_eq!(store.task(h_task.id)?.unwrap().horizon, None);
    store.set_horizon(h_task.id, Some(m12))?;
    store.set_task_schedule(h_task.id, None, ScheduleBucket::Someday)?;
    assert_eq!(store.task(h_task.id)?.unwrap().horizon, None);

    // set_horizon(None) returns ungrouped task back to Inbox.
    store.set_horizon(dup.id, None)?;
    assert_eq!(store.task(dup.id)?.unwrap().horizon, None);
    assert!(store.tasks_in(&View::Inbox, today)?.iter().any(|t| t.id == dup.id));
    Ok(())
}

#[test]
fn horizon_recurrence_and_undo_restores_bucket_and_checklist() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = d(2026, 11, 23);
    let w53 = Horizon::Week { year: 2026, week: 53 };
    let w01_next = Horizon::Week { year: 2027, week: 1 };

    // Weekly repeat on 2026-W53 advances across ISO 53-week boundary to 2027-W01.
    let weekly = store.create_task(
        TaskDraft {
            title: "Weekly review".into(),
            horizon: Some(w53),
            checklist: vec![("Inbox zero".into(), true), ("Plan next week".into(), false)],
            ..Default::default()
        },
        true,
    )?;
    // set_repeat with compatible Weekly rule preserves horizon (does not overwrite with due_date).
    store.set_repeat(weekly.id, Some(Recurrence::new(crate::Frequency::Weekly, 1)), today)?;
    let before = store.task(weekly.id)?.unwrap();
    assert_eq!(before.horizon, Some(w53));
    assert_eq!(before.due_date, None);

    let copy_id = store.complete(weekly.id, true)?.expect("repeating horizon task creates copy");
    let after = store.task(weekly.id)?.unwrap();
    assert!(!after.is_completed());
    assert_eq!(after.horizon, Some(w01_next));
    assert_eq!(after.due_date, None);
    assert!(store.checklist(weekly.id)?.iter().all(|c| !c.completed));

    let copy = store.task(copy_id)?.unwrap();
    assert!(copy.is_completed());
    assert_eq!(copy.horizon, Some(w53));
    assert_eq!(copy.repeat, None);

    // undo_repeat restores horizon from copy and restores checklist checkmarks.
    store.undo_repeat(weekly.id, copy_id, None, None, None)?;
    let undone = store.task(weekly.id)?.unwrap();
    assert_eq!(undone.horizon, Some(w53));
    assert!(store.task(copy_id)?.is_none());
    let restored_cl = store.checklist(weekly.id)?;
    assert_eq!(restored_cl.iter().map(|c| c.completed).collect::<Vec<_>>(), vec![true, false]);

    // undo_repeat_with_horizon restores explicit horizon even if copy is already gone.
    let copy2 = store.complete(weekly.id, true)?.expect("second completion creates copy");
    store.purge_task(copy2)?;
    store.undo_repeat_with_horizon(weekly.id, copy2, None, None, Some(w53), None)?;
    assert_eq!(store.task(weekly.id)?.unwrap().horizon, Some(w53));

    // Quarter + Monthly(interval=3) advances by 1 quarter.
    let q_task = store.create_task(
        TaskDraft {
            title: "Quarterly OKRs".into(),
            horizon: Some(Horizon::Quarter { year: 2026, quarter: 4 }),
            repeat: Some(Recurrence::new(crate::Frequency::Monthly, 3)),
            ..Default::default()
        },
        true,
    )?;
    assert!(store.complete(q_task.id, true)?.is_some());
    assert_eq!(
        store.task(q_task.id)?.unwrap().horizon,
        Some(Horizon::Quarter { year: 2027, quarter: 1 })
    );

    // Incompatible granularity (Life + Weekly) closes normally instead of advancing.
    let life_task = store.create_task(
        TaskDraft {
            title: "Write a book".into(),
            horizon: Some(Horizon::Life),
            repeat: Some(Recurrence::new(crate::Frequency::Weekly, 1)),
            ..Default::default()
        },
        true,
    )?;
    assert_eq!(store.complete(life_task.id, true)?, None);
    assert!(store.task(life_task.id)?.unwrap().is_completed());
    Ok(())
}

#[test]
fn horizon_rollover_helpers_update_open_tasks_transactionally() -> Result<()> {
    let store = Store::open_in_memory()?;
    let t = |h| NaiveTime::from_hms_opt(h, 0, 0).unwrap();
    let today = d(2026, 11, 23);
    let past_due = d(2026, 11, 20);
    let past_end = d(2026, 11, 22);
    let w47 = Horizon::Week { year: 2026, week: 47 };
    let w48 = Horizon::Week { year: 2026, week: 48 };

    let open_past = store.create_task(
        TaskDraft { title: "Open past week".into(), horizon: Some(w47), ..Default::default() },
        true,
    )?;
    let done_past = store.create_task(
        TaskDraft { title: "Done past week".into(), horizon: Some(w47), ..Default::default() },
        true,
    )?;
    store.set_completed(done_past.id, true)?;
    let trashed_past = store.create_task(
        TaskDraft { title: "Trashed past week".into(), horizon: Some(w47), ..Default::default() },
        true,
    )?;
    store.delete_task(trashed_past.id)?;

    store.rollover_horizon_tasks(&[open_past.id, done_past.id, trashed_past.id], w48)?;
    assert_eq!(store.task(open_past.id)?.unwrap().horizon, Some(w48));
    assert_eq!(store.task(done_past.id)?.unwrap().horizon, Some(w47));
    assert_eq!(store.task(trashed_past.id)?.unwrap().horizon, Some(w47));

    // Overdue rollover shifts due_date and end_date together, preserving due_time and clearing snooze.
    let overdue = store.create_task(
        TaskDraft {
            title: "Overdue span".into(),
            due_date: Some(past_due),
            due_time: Some(t(10)),
            end_date: Some(past_end),
            end_time: Some(t(14)),
            ..Default::default()
        },
        true,
    )?;
    store.snooze_task(overdue.id, past_due.and_hms_opt(11, 0, 0).unwrap())?;
    let overdue_done = store.create_task(
        TaskDraft { title: "Done overdue".into(), due_date: Some(past_due), ..Default::default() },
        true,
    )?;
    store.set_completed(overdue_done.id, true)?;

    store.rollover_overdue_to_day(&[overdue.id, overdue_done.id], today)?;
    let rolled = store.task(overdue.id)?.unwrap();
    assert_eq!(rolled.due_date, Some(today));
    assert_eq!(rolled.due_time, Some(t(10)));
    assert_eq!(rolled.end_date, Some(d(2026, 11, 25)));
    assert_eq!(rolled.end_time, Some(t(14)));
    assert_eq!(rolled.snoozed_until, None);
    assert_eq!(rolled.horizon, None);
    assert_eq!(store.task(overdue_done.id)?.unwrap().due_date, Some(past_due));
    Ok(())
}

#[test]
fn next_7_days_includes_current_and_past_week_horizons_only() -> Result<()> {
    let store = Store::open_in_memory()?;
    let today = d(2026, 11, 23); // 2026-W48
    let cur_w = Horizon::current_week(today);
    let past_w = cur_w.shift(-1);
    let next_w = cur_w.shift(1);

    let t_cur_w = store.create_task(
        TaskDraft { title: "Current week".into(), horizon: Some(cur_w), ..Default::default() },
        true,
    )?;
    let t_past_w = store.create_task(
        TaskDraft { title: "Past week".into(), horizon: Some(past_w), ..Default::default() },
        true,
    )?;
    let t_done_w = store.create_task(
        TaskDraft { title: "Done week".into(), horizon: Some(cur_w), ..Default::default() },
        true,
    )?;
    store.set_completed(t_done_w.id, true)?;
    let t_next_w = store.create_task(
        TaskDraft { title: "Next week".into(), horizon: Some(next_w), ..Default::default() },
        true,
    )?;
    let t_month = store.create_task(
        TaskDraft {
            title: "This month".into(),
            horizon: Some(Horizon::current_month(today)),
            ..Default::default()
        },
        true,
    )?;
    let t_year = store.create_task(
        TaskDraft {
            title: "This year".into(),
            horizon: Some(Horizon::current_year(today)),
            ..Default::default()
        },
        true,
    )?;

    let ids: Vec<Id> = store.tasks_in(&View::Next7Days, today)?.into_iter().map(|t| t.id).collect();
    assert!(ids.contains(&t_cur_w.id));
    assert!(ids.contains(&t_past_w.id));
    assert!(!ids.contains(&t_done_w.id));
    assert!(!ids.contains(&t_next_w.id));
    assert!(!ids.contains(&t_month.id));
    assert!(!ids.contains(&t_year.id));
    assert_eq!(store.counts(today)?.next_7_days, 2);
    Ok(())
}
