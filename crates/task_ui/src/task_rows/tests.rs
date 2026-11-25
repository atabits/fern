use super::*;
use crate::i18n::{EN, RU};
use uuid::Uuid;

fn task(title: &str) -> Task {
    Task {
        id: Uuid::now_v7(),
        list_id: None,
        parent_id: None,
        kind: Kind::Task,
        title: title.into(),
        notes: String::new(),
        priority: Priority::None,
        due_date: None,
        due_time: None,
        repeat: None,
        tags: Vec::new(),
        checklist: None,
        completed_at: None,
        deleted_at: None,
        wont_do_at: None,
        in_progress_at: None,
        end_date: None,
        end_time: None,
        location: None,
        timezone: None,
        is_floating: false,
        snoozed_until: None,
        constant_reminder: false,
        modified_at: None,
        pinned_at: None,
        section_id: None,
        reminders: Vec::new(),
        estimate: None,
        start_date: None,
        schedule_bucket: fern_core::ScheduleBucket::None,
        horizon: None,
        created_at: 0,
    }
}

fn shape(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            Row::Section { section, count, .. } => format!("# {} {count}", section.title(&EN)),
            Row::Day { label, count, .. } => format!("## {label} {count}"),
            Row::Task { task, depth, .. } => format!("{}{}", "  ".repeat(*depth), task.title),
        })
        .collect()
}

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

#[test]
fn single_section_has_no_header_and_nests_children() {
    let parent = task("parent");
    let mut child = task("child");
    child.parent_id = Some(parent.id);
    let mut grandchild = task("grandchild");
    grandchild.parent_id = Some(child.id);
    let rows = build_rows(&[grandchild, parent.clone(), child.clone()], &HashSet::new(), &HashSet::new(), true);
    assert_eq!(shape(&rows), ["parent", "  child", "    grandchild"]);

    let rows = build_rows(
        &[parent.clone(), child],
        &HashSet::from([parent.id]),
        &HashSet::new(),
        true,
    );
    assert_eq!(shape(&rows), ["parent"]);
    assert!(matches!(rows[0], Row::Task { has_children: true, collapsed: true, .. }));
}

#[test]
fn sections_split_by_kind_and_completion() {
    let a = task("a task");
    let mut note = task("a note");
    note.kind = Kind::Note;
    let mut done = task("done");
    done.completed_at = Some(1);
    let rows = build_rows(&[a, note, done], &HashSet::new(), &HashSet::from([Section::Completed]), true);
    assert_eq!(shape(&rows), ["# Tasks 1", "a task", "# Notes 1", "a note", "# Completed 1"]);
}

#[test]
fn match_ranges_ignore_case_and_handle_unicode() {
    assert_eq!(match_ranges("Buy Milk and milk", "MILK"), [4..8, 13..17]);
    let text = "Купить хлеб";
    let ranges = match_ranges(text, "ХЛЕБ");
    assert_eq!(&text[ranges[0].clone()], "хлеб");
    assert!(match_ranges("abc", "  ").is_empty());
}

#[test]
fn snippets_cut_around_the_match() {
    assert_eq!(snippet("one two three four", "three", 4).as_deref(), Some("…two three fou…"));
    assert_eq!(snippet("short", "sho", 10).as_deref(), Some("short"));
    assert_eq!(snippet("line one\nline two", "two", 20).as_deref(), Some("line one line two"));
    assert_eq!(snippet("nothing", "zzz", 5), None);
}

#[test]
fn rows_by_list_section() {
    let list = Uuid::now_v7();
    let kitchen = ListSection { id: Uuid::now_v7(), list_id: Some(list), name: "Kitchen".into() };
    let garden = ListSection { id: Uuid::now_v7(), list_id: Some(list), name: "Garden".into() };
    let mut tap = task("fix the tap");
    tap.section_id = Some(kitchen.id);
    let mut done = task("done");
    done.completed_at = Some(1);
    let rows = section_rows(&[task("loose"), tap, done], &[kitchen, garden], &HashSet::new(), &EN);
    assert_eq!(shape(&rows), [
        "## Not Sectioned 1", "loose", "## Kitchen 1", "fix the tap", "## Garden 0", "## Completed 1", "done"
    ]);
}

#[test]
fn kanban_columns_follow_the_groups() {
    let mut child = task("child");
    child.parent_id = Some(Uuid::now_v7());
    let rows = vec![
        Row::Section { section: Section::Tasks, count: 2, collapsed: false },
        Row::Task { task: task("parent"), depth: 0, has_children: true, collapsed: false },
        Row::Task { task: child, depth: 1, has_children: false, collapsed: false },
        Row::Task { task: task("solo"), depth: 0, has_children: false, collapsed: false },
        Row::Section { section: Section::Notes, count: 0, collapsed: true },
    ];
    let columns = kanban_columns(rows, &EN);
    let shape: Vec<(String, Vec<(String, usize)>)> = columns
        .into_iter()
        .map(|(title, _, cards)| (title, cards.into_iter().map(|(t, subs)| (t.title, subs.len())).collect()))
        .collect();
    assert_eq!(shape, [
        ("Tasks".to_owned(), vec![("parent".to_owned(), 1), ("solo".to_owned(), 0)]),
        ("Notes".to_owned(), vec![]),
    ]);
    assert_eq!(kanban_columns(vec![Row::Task { task: task("a"), depth: 0, has_children: false, collapsed: false }], &EN)[0].0, "");
}

#[test]
fn pinned_tasks_go_first() {
    let mut tasks = vec![task("a"), task("b"), task("c"), task("d")];
    tasks[2].pinned_at = Some(1);
    tasks[3].pinned_at = Some(2);
    pinned_first(&mut tasks);
    let titles: Vec<&str> = tasks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["d", "c", "a", "b"]);
}

#[test]
fn sorting_and_grouping() {
    let today = d(2026, 10, 18);
    let mut a = task("banana");
    a.due_date = Some(d(2026, 10, 17));
    a.priority = Priority::Low;
    let mut b = task("Apple");
    b.due_date = Some(today);
    b.priority = Priority::High;
    b.tags = vec!["work".into()];
    let mut c = task("cherry");
    c.tags = vec!["home".into()];
    let mut done = task("done");
    done.completed_at = Some(1);
    let tasks = vec![a, b, c, done];

    let titles = |tasks: &[Task]| tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>();
    let mut sorted = tasks.clone();
    order_tasks(&mut sorted, SortBy::Title, true);
    assert!(sorted.windows(2).all(|w| w[0].title.to_lowercase() >= w[1].title.to_lowercase()), "Descending turns the sort around");
    sort_tasks(&mut sorted, SortBy::Title);
    assert_eq!(titles(&sorted), ["Apple", "banana", "cherry", "done"]);
    sort_tasks(&mut sorted, SortBy::Priority);
    assert_eq!(titles(&sorted)[..2], ["Apple", "banana"]);
    sort_tasks(&mut sorted, SortBy::Date);
    assert_eq!(titles(&sorted)[..2], ["banana", "Apple"]);
    sort_tasks(&mut sorted, SortBy::Tag);
    assert_eq!(titles(&sorted)[..2], ["cherry", "Apple"]);
    sorted[3].modified_at = Some(10);
    sort_tasks(&mut sorted, SortBy::Modified);
    assert_eq!(sorted[0].modified_at, Some(10));

    assert_eq!(shape(&grouped_rows_with(&tasks, GroupBy::Date, today, false, true, &EN)), [
        "## Overdue 1", "banana", "## Today 1", "Apple", "## No Date 1", "cherry", "## Completed 1", "done"
    ]);
    assert_eq!(shape(&grouped_rows_with(&tasks, GroupBy::Tag, today, false, true, &EN)), [
        "## home 1", "cherry", "## work 1", "Apple", "## No Tags 1", "banana", "## Completed 1", "done"
    ]);
    assert_eq!(shape(&grouped_rows_with(&tasks, GroupBy::None, today, false, true, &EN)), ["banana", "Apple", "cherry", "done"]);
    let work = fern_core::Id::now_v7();
    let mut in_work = tasks.clone();
    in_work[0].list_id = Some(work);
    let by_list = shape(&grouped_rows_in_with(&in_work, GroupBy::List, today, false, true, &[(work, "Work".into())], &EN));
    assert!(by_list[0].starts_with("## Inbox"), "the Inbox first, then the lists in their order");
    assert!(by_list.iter().any(|l| l == "## Work 1"));
    // "Overdue Section shows at: Bottom of List".
    assert_eq!(shape(&grouped_rows_with(&tasks, GroupBy::Date, today, true, true, &EN)), [
        "## Today 1", "Apple", "## No Date 1", "cherry", "## Overdue 1", "banana", "## Completed 1", "done"
    ]);
    let drops = |rows: Vec<Row>| -> Vec<Option<DropTarget>> {
        rows.into_iter().filter_map(|r| match r { Row::Day { drop, .. } => Some(drop), _ => None }).collect()
    };
    let targets = drops(grouped_rows_in_with(&in_work, GroupBy::List, today, false, true, &[(work, "Work".into())], &EN));
    assert!(targets.contains(&Some(DropTarget::List(Some(work)))) && targets.contains(&Some(DropTarget::List(None))), "a list column takes cards into its list");
    assert_eq!(drops(grouped_rows_with(&tasks, GroupBy::Date, today, false, true, &EN)), [
        None,
        Some(DropTarget::Due(Some(today))),
        Some(DropTarget::Due(None)),
        Some(DropTarget::Complete),
    ]);
    assert_eq!(drops(grouped_rows_with(&tasks, GroupBy::Tag, today, false, true, &EN))[..2], [
        Some(DropTarget::Tag(Some("home".into()))),
        Some(DropTarget::Tag(Some("work".into()))),
    ]);
    assert_eq!(SortBy::from_key(SortBy::Tag.key()), SortBy::Tag);
    assert_eq!(GroupBy::from_key("nonsense"), GroupBy::Custom);
}

#[test]
fn closed_ranges() {
    let today = d(2026, 10, 18); // a Sunday
    let r = |range: ClosedRange, day| range.contains(day, today, chrono::Weekday::Mon);
    assert!(r(ClosedRange::ThisWeek, d(2026, 10, 12)) && !r(ClosedRange::ThisWeek, d(2026, 10, 11)));
    assert!(r(ClosedRange::LastWeek, d(2026, 10, 5)) && r(ClosedRange::LastWeek, d(2026, 10, 11)));
    assert!(!r(ClosedRange::LastWeek, d(2026, 10, 12)) && !r(ClosedRange::LastWeek, d(2026, 10, 4)));
    assert!(r(ClosedRange::ThisMonth, d(2026, 10, 1)) && !r(ClosedRange::ThisMonth, d(2025, 10, 1)));
    assert!(r(ClosedRange::All, d(2000, 1, 1)));
    let custom = ClosedRange::custom(d(2026, 10, 9), d(2026, 9, 28));
    assert_eq!(custom, ClosedRange::Custom(d(2026, 9, 28), d(2026, 10, 9)));
    assert!(r(custom, d(2026, 9, 28)) && r(custom, d(2026, 10, 9)) && !r(custom, d(2026, 10, 10)));
    assert_eq!(custom.label(&EN), "Sep 28 - Oct 9");
}

#[test]
fn day_headers() {
    let today = d(2026, 10, 18);
    assert_eq!(day_header(d(2026, 9, 27), today, &EN), "Sunday, September 27");
    assert_eq!(day_header(d(2025, 9, 26), today, &EN), "Friday, September 26, 2025");
    assert_eq!(day_header(d(2026, 9, 27), today, &RU), "воскресенье, 27 сентября");
}

#[test]
fn next_days_groups() {
    let today = d(2026, 10, 18);
    let mon = chrono::Weekday::Mon;
    assert_eq!(next_days_header(today, today, mon, &EN), "Sunday, Today");
    assert_eq!(next_days_header(d(2026, 10, 19), today, mon, &EN), "Monday, Tomorrow");
    assert_eq!(next_days_header(d(2026, 10, 24), today, mon, &EN), "Next Saturday, October 24");
    let mut late = task("late");
    late.due_date = Some(d(2026, 10, 17));
    let mut sat = task("sat");
    sat.due_date = Some(d(2026, 10, 24));
    assert_eq!(shape(&next_days_rows_with(&[sat, late], today, mon, true, &EN)), ["## Overdue 1", "late", "## Next Saturday, October 24 1", "sat"]);
}

#[test]
fn ungrouped_lists_have_no_headers() {
    let a = task("open");
    let mut done = task("done");
    done.completed_at = Some(1);
    let rows = build_rows(&[a, done], &HashSet::new(), &HashSet::new(), false);
    assert_eq!(shape(&rows), ["open", "done"]);
}

#[test]
fn child_in_other_section_goes_top_level() {
    let parent = task("parent");
    let mut child = task("finished child");
    child.parent_id = Some(parent.id);
    child.completed_at = Some(1);
    let rows = build_rows(&[parent, child], &HashSet::new(), &HashSet::new(), true);
    assert_eq!(shape(&rows), ["# Tasks 1", "parent", "# Completed 1", "finished child"]);
}

#[test]
fn due_labels_english() {
    let today = d(2026, 10, 18); // a Sunday
    let label = |date| due_label_with(date, None, today, &EN, false);
    assert_eq!(label(today), ("Today".into(), false));
    assert_eq!(label(d(2026, 10, 19)), ("Tomorrow".into(), false));
    assert_eq!(label(d(2026, 10, 17)), ("Yesterday".into(), true));
    assert_eq!(label(d(2026, 10, 24)), ("Sat".into(), false));
    assert_eq!(label(d(2026, 11, 13)), ("Nov 13".into(), false));
    assert_eq!(label(d(2026, 10, 1)), ("Oct 1".into(), true));
    assert_eq!(label(d(2027, 1, 5)), ("Jan 5 2027".into(), false));
    assert_eq!(
        due_label_with(today, NaiveTime::from_hms_opt(18, 30, 0), today, &EN, false),
        ("Today 18:30".into(), false)
    );
}

#[test]
fn detail_due_labels() {
    let today = d(2026, 10, 18);
    assert_eq!(detail_due_label("Nov 13".into(), d(2026, 11, 13), today, &EN), "Nov 13, 26d left");
    assert_eq!(detail_due_label("Tomorrow".into(), d(2026, 10, 19), today, &EN), "Tomorrow");
    assert_eq!(detail_due_label("Yesterday".into(), d(2026, 10, 17), today, &EN), "Yesterday");
    assert_eq!(detail_due_label("Oct 13".into(), d(2026, 10, 13), today, &EN), "Oct 13, 5d overdue");
}

#[test]
fn card_due_labels() {
    let today = d(2026, 10, 18);
    assert_eq!(card_due_label_with(d(2026, 10, 24), None, today, &EN, false), "Sat, Oct 24");
    assert_eq!(card_due_label_with(d(2027, 1, 4), None, today, &EN, false), "Mon, Jan 4, 2027");
}

#[test]
fn due_labels_russian() {
    let today = d(2026, 10, 18);
    assert_eq!(row_due_label_with(today + chrono::Duration::days(3), None, today, &EN, true, false).0, "in 3 days");
    assert_eq!(row_due_label_with(today - chrono::Duration::days(2), None, today, &EN, true, false), ("2 days ago".to_owned(), true));
    assert_eq!(row_due_label_with(today, None, today, &EN, true, false).0, "Today");
    assert_eq!(closed_due_label(today, today, &EN), "Oct 18", "a closed task shows the date itself");
    assert_eq!(closed_due_label(d(2025, 9, 26), today, &EN), "Sep 26, 2025");
    assert_eq!(closed_due_label(d(2025, 9, 24), today, &RU), "24.09.2025", "as the Russian original");
    assert_eq!(closed_day(d(2026, 10, 17), today, &RU), "17 окт Сб");
    let label = |date| due_label_with(date, None, today, &RU, false).0;
    assert_eq!(label(today), "Сегодня");
    assert_eq!(label(d(2026, 10, 24)), "Сб");
    assert_eq!(label(d(2026, 11, 13)), "13 ноя");
    assert_eq!(label(d(2027, 1, 5)), "5 янв 2027");
}

#[test]
fn plan_task_drop_unnests_in_two_task_list_by_all_gestures() {
    let parent = task("parent");
    let mut child = task("child");
    child.parent_id = Some(parent.id);
    let rows = build_rows(&[parent.clone(), child.clone()], &HashSet::new(), &HashSet::new(), true);

    // 1) Drag child left on its own row -> top-level right after parent.
    let plan = plan_task_drop(&rows, child.id, child.id, false, -24.0, 60.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, 0);
    assert_eq!(plan.new_parent, None);
    assert_eq!(plan.order_target, Some((parent.id, true)));
    assert_eq!(plan.target_id, child.id);
    assert!(plan.after);

    // 2) Drag child slightly left near the left gutter -> snaps to root.
    let plan = plan_task_drop(&rows, child.id, child.id, false, -8.0, 30.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, 0);
    assert_eq!(plan.new_parent, None);

    // 3) Drag child above parent -> top-level right before parent regardless of X.
    let plan = plan_task_drop(&rows, child.id, parent.id, false, 0.0, 120.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, 0);
    assert_eq!(plan.new_parent, None);
    assert_eq!(plan.order_target, Some((parent.id, false)));
    assert_eq!(plan.target_id, parent.id);
    assert!(!plan.after);

    // 4) Drag child onto parent's bottom half without moving right -> top-level right after parent.
    let plan = plan_task_drop(&rows, child.id, parent.id, true, 0.0, 120.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, 0);
    assert_eq!(plan.new_parent, None);
    assert_eq!(plan.order_target, Some((parent.id, true)));

    // 5) Drag child into empty space below the list -> top-level right after parent.
    let plan = plan_task_drop_at_end(&rows, child.id).unwrap();
    assert_eq!(plan.target_level, 0);
    assert_eq!(plan.new_parent, None);
    assert_eq!(plan.order_target, Some((parent.id, true)));

    // Staying put on own row with no horizontal movement is a no-op.
    assert!(plan_task_drop(&rows, child.id, child.id, false, 0.0, 120.0, 24.0, 20.0, true).is_none());
}

#[test]
fn plan_task_drop_nests_outdents_multi_level_and_guards_cycles_and_notes() {
    let a = task("a");
    let mut b = task("b");
    b.parent_id = Some(a.id);
    let mut c = task("c");
    c.parent_id = Some(b.id);
    let solo = task("solo");
    let rows = build_rows(&[a.clone(), b.clone(), c.clone(), solo.clone()], &HashSet::new(), &HashSet::new(), true);

    // Cannot drop parent onto its own descendant.
    assert!(plan_task_drop(&rows, a.id, b.id, true, 0.0, 100.0, 24.0, 20.0, true).is_none());
    assert!(plan_task_drop(&rows, a.id, c.id, false, 0.0, 100.0, 24.0, 20.0, true).is_none());

    // Drag solo right onto its own row after c (depth 2) -> nests under c (depth 3)?
    // Wait: solo is depth 0; prev is c (depth 2), so on solo's own row base_level = 0, dx = +24 -> depth 1 (under a!).
    let plan = plan_task_drop(&rows, solo.id, solo.id, false, 24.0, 120.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, 1);
    assert_eq!(plan.new_parent, Some(a.id));
    assert_eq!(plan.order_target, Some((b.id, true)));

    // Drag solo onto bottom half of c with dx = +24 -> nests under c (depth 3).
    let plan = plan_task_drop(&rows, solo.id, c.id, true, 24.0, 140.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, 3);
    assert_eq!(plan.new_parent, Some(c.id));

    // Drag solo onto bottom half of a (expanded, next is b at depth 1) -> forced to depth 1 before b.
    let plan = plan_task_drop(&rows, solo.id, a.id, true, 0.0, 100.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, 1);
    assert_eq!(plan.new_parent, Some(a.id));
    assert_eq!(plan.order_target, Some((b.id, false)));

    // Drag c left by 1 step on own row -> becomes sibling of b under a, right after b.
    let plan = plan_task_drop(&rows, c.id, c.id, false, -24.0, 100.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, 1);
    assert_eq!(plan.new_parent, Some(a.id));
    assert_eq!(plan.order_target, Some((b.id, true)));

    // Drag c left by 2 steps on own row -> becomes root right before solo.
    let plan = plan_task_drop(&rows, c.id, c.id, false, -48.0, 40.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, 0);
    assert_eq!(plan.new_parent, None);
    assert_eq!(plan.order_target, Some((solo.id, false)));

    // Dragging onto a collapsed parent requests auto-expanding it.
    let collapsed_rows = build_rows(&[a.clone(), b.clone(), solo.clone()], &HashSet::from([a.id]), &HashSet::new(), true);
    let plan = plan_task_drop(&collapsed_rows, solo.id, a.id, true, 24.0, 120.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.new_parent, Some(a.id));
    assert_eq!(plan.expand_parent, Some(a.id));

    // Notes cannot nest or be dropped inside an expanded subtask tree.
    let mut note = task("note");
    note.kind = Kind::Note;
    let note_rows = build_rows(&[a.clone(), b, note.clone()], &HashSet::new(), &HashSet::new(), false);
    assert!(plan_task_drop(&note_rows, note.id, a.id, true, 24.0, 120.0, 24.0, 20.0, true).is_none());
    let plan = plan_task_drop(&note_rows, a.id, note.id, true, 24.0, 120.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, 0);
    assert_eq!(plan.new_parent, None);
}

#[test]
fn plan_task_drop_respects_max_subtask_depth() {
    let mut chain: Vec<Task> = Vec::new();
    for i in 0..=fern_core::MAX_SUBTASK_DEPTH {
        let mut t = task(&format!("L{i}"));
        if let Some(prev) = chain.last() {
            t.parent_id = Some(prev.id);
        }
        chain.push(t);
    }
    let extra = task("extra");
    let mut all = chain.clone();
    all.push(extra.clone());
    let rows = build_rows(&all, &HashSet::new(), &HashSet::new(), true);
    let deepest = chain.last().unwrap();
    let parent_of_deepest = &chain[fern_core::MAX_SUBTASK_DEPTH - 1];

    // Even dragging right under a depth-4 task caps at MAX_SUBTASK_DEPTH (4), sibling of deepest.
    let plan = plan_task_drop(&rows, extra.id, deepest.id, true, 48.0, 220.0, 24.0, 20.0, true).unwrap();
    assert_eq!(plan.target_level, fern_core::MAX_SUBTASK_DEPTH);
    assert_eq!(plan.new_parent, Some(parent_of_deepest.id));
}

#[test]
fn horizon_badge_label_shows_only_when_undated_with_horizon() {
    let mut t = task("Quarterly roadmap");
    assert_eq!(horizon_badge_label(&t, &EN), None);

    t.horizon = Some(fern_core::Horizon::Week { year: 2026, week: 48 });
    assert_eq!(horizon_badge_label(&t, &EN).as_deref(), Some("W48"));
    assert_eq!(horizon_badge_label(&t, &RU).as_deref(), Some("Н48"));

    t.horizon = Some(fern_core::Horizon::Life);
    assert_eq!(horizon_badge_label(&t, &RU).as_deref(), Some("Жизнь"));

    // Exact due date takes precedence over horizon badge.
    t.due_date = Some(d(2026, 11, 23));
    assert_eq!(horizon_badge_label(&t, &EN), None);
}

#[test]
fn date_grouping_and_next_days_group_horizons_chronologically() {
    let today = d(2026, 11, 23); // Mon of 2026-W48
    let mut t_today = task("Today item");
    t_today.due_date = Some(today);
    let mut t_week = task("Week item");
    t_week.horizon = Some(fern_core::Horizon::current_week(today));
    let mut t_month = task("Month item");
    t_month.horizon = Some(fern_core::Horizon::current_month(today));
    let mut t_quarter = task("Quarter item");
    t_quarter.horizon = Some(fern_core::Horizon::current_quarter(today));
    let mut t_year = task("Year item");
    t_year.horizon = Some(fern_core::Horizon::current_year(today));
    let mut t_future = task("Next month item");
    t_future.horizon = Some(fern_core::Horizon::current_month(today).shift(1));
    let mut t_life = task("Life item");
    t_life.horizon = Some(fern_core::Horizon::Life);
    let t_nodate = task("Undated item");

    let rows = grouped_rows_with(
        &[
            t_nodate,
            t_life,
            t_future,
            t_year,
            t_quarter,
            t_month,
            t_week.clone(),
            t_today.clone(),
        ],
        GroupBy::Date,
        today,
        false,
        true,
        &EN,
    );
    assert_eq!(
        shape(&rows),
        [
            "## Today 1",
            "Today item",
            "## This week · W48 1",
            "Week item",
            "## This month 1",
            "Month item",
            "## This quarter 1",
            "Quarter item",
            "## This year 1",
            "Year item",
            "## Later horizons 1",
            "Next month item",
            "## Life 1",
            "Life item",
            "## No Date 1",
            "Undated item",
        ]
    );

    let next7 = next_days_rows_with(&[t_today, t_week], today, chrono::Weekday::Mon, true, &EN);
    assert_eq!(
        shape(&next7),
        [
            "## Monday, Today 1",
            "Today item",
            "## This week · W48 1",
            "Week item",
        ]
    );
}
