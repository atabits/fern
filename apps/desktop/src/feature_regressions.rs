//! Cross-feature regression checks against the real in-memory store.
use fern_core::*;
use fern_core::summary::{Field as SummaryField, Period, StatusFilter, SummaryOptions, summarize};
use chrono::{Local, Weekday};

#[test]
fn backup_import_preserves_folders_columns_and_floating_dates() {
    let store = Store::open_in_memory().unwrap();
    let untouched = store.create_list("Work").unwrap();
    let csv = "Folder Name,List Name,Title,Column Name,Column Order,Due Date,Is All Day,Is Floating,taskId,parentId\n\
        Office,Work,Review,Done,2,2026-11-13T00:00:00+14:00,true,false,a,\n\
        Office,Work,Write,Draft,1,2026-11-13T09:30:00-12:00,false,true,b,\n\
        Personal,Work,Home,Draft,1,,,,c,\n";
    let summary = store.import_ticktick(csv).unwrap();
    assert_eq!((summary.tasks, summary.lists), (3, 2));
    let folders = store.list_folders().unwrap();
    assert_eq!(folders.len(), 2);
    assert_eq!(store.lists().unwrap().iter().find(|l| l.id == untouched.id).unwrap().folder, None);
    let office = store.lists().unwrap().into_iter().find(|l| l.folder == Some(folders[0].id)).unwrap();
    let sections = store.list_sections(Some(office.id)).unwrap();
    assert_eq!(sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Draft", "Done"]);
    let tasks = store.tasks().unwrap();
    let review = tasks.iter().find(|t| t.title == "Review").unwrap();
    assert_eq!(review.section_id, Some(sections[1].id));
    assert_eq!(review.due_date.unwrap().to_string(), "2026-11-13");
    let write = tasks.iter().find(|t| t.title == "Write").unwrap();
    assert_eq!(write.due_time.unwrap().to_string(), "09:30:00");
    store.import_ticktick(csv).unwrap();
    assert_eq!(store.list_folders().unwrap().len(), 2);
    assert_eq!(store.list_sections(Some(office.id)).unwrap().len(), 2);
}

#[test]
fn malformed_backup_hierarchy_is_rejected_before_any_writes() {
    let store = Store::open_in_memory().unwrap();
    for rows in ["New,One,a,b\nNew,Two,b,a", "New,One,a,\nNew,Two,a,", "New,One,a,a"] {
        let csv = format!("List Name,Title,taskId,parentId\n{rows}");
        assert!(store.import_ticktick(&csv).is_err());
        assert!(store.tasks().unwrap().is_empty());
        assert!(store.lists().unwrap().is_empty());
    }
}

#[test]
fn repeating_occurrences_keep_history_and_undo_restores_checklist() {
    let store = Store::open_in_memory().unwrap();
    let day = Local::now().date_naive();
    let task = store.add_task("Daily checklist").unwrap();
    let first = store.add_checklist_item(task.id, "First", None).unwrap();
    let second = store.add_checklist_item(task.id, "Second", None).unwrap();
    store.set_checklist_item_completed(second.id, true).unwrap();
    store.set_repeat(task.id, Some(Recurrence::new(Frequency::Daily, 1)), day).unwrap();
    let copy = store.complete(task.id, true).unwrap().unwrap();
    assert_eq!(store.task(copy).unwrap().unwrap().checklist, Some((1, 2)));
    assert_eq!(store.task(task.id).unwrap().unwrap().checklist, Some((0, 2)));
    let inserted = store.add_checklist_item(task.id, "Inserted", Some(first.id)).unwrap();
    store.undo_repeat(task.id, copy, Some(day), None).unwrap();
    let checklist = store.checklist(task.id).unwrap();
    assert_eq!(
        checklist.iter().map(|item| (item.id, item.completed)).collect::<Vec<_>>(),
        [(first.id, false), (inserted.id, false), (second.id, true)],
    );
    assert!(store.task(copy).unwrap().is_none());
}

#[test]
fn summary_in_progress_excludes_unstarted_completed_and_trash() {
    let store = Store::open_in_memory().unwrap();
    let today = Local::now().date_naive();
    for (name, checked, closed, deleted) in [("Started", true, false, false), ("Unstarted", false, false, false), ("Done", true, true, false), ("Trash", true, false, true)] {
        let task = store.add_task(name).unwrap();
        store.set_due(task.id, Some(today), None).unwrap();
        let item = store.add_checklist_item(task.id, "First", None).unwrap();
        store.add_checklist_item(task.id, "Second", None).unwrap();
        if checked { store.set_checklist_item_completed(item.id, true).unwrap(); }
        if closed { store.set_completed(task.id, true).unwrap(); }
        if deleted { store.delete_task(task.id).unwrap(); }
    }
    let options = SummaryOptions { period: Period::Today, status: StatusFilter::InProgress, ..Default::default() };
    let groups = summarize(&store.tasks().unwrap(), &options, today, Weekday::Mon);
    let names: Vec<_> = groups.iter().flat_map(|g| &g.tasks).map(|t| t.title.as_str()).collect();
    assert_eq!(names, ["Started"]);
    let legacy = SummaryOptions::from_storage("fields=status,list;period=this_week");
    assert!(legacy.fields.contains(&SummaryField::Title));
    let mut new = SummaryOptions::default();
    new.fields.retain(|f| *f != SummaryField::Title);
    assert_eq!(SummaryOptions::from_storage(&new.to_storage()), new);
}
