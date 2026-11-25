use super::*;
use super::week_grid::{LanePlacement, TimedInterval, assign_timed_lanes, dual_tz_reference_day};
use crate::drag::DraggedEvent;
use chrono::TimeZone as _;
use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, point, size};

#[gpui::test]
fn subscribed_event_bars_use_each_occurrences_local_time(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    store.add_subscription_with_feed("Synthetic meetings", "https://example.com/test.ics", None, concat!(
        "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:weekly-ny\nSUMMARY:Weekly meeting\n",
        "DTSTART;TZID=America/New_York:20261025T090000\n",
        "DTEND;TZID=America/New_York:20261025T100000\n",
        "RRULE:FREQ=WEEKLY;BYDAY=SU;COUNT=3\nEND:VEVENT\nEND:VCALENDAR",
    )).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        crate::subscriptions::init(store.clone(), cx);
    });
    let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store, window, cx));
    let expected = [(10, 25, 13), (11, 1, 14), (11, 8, 14)].map(|(month, day, hour)| {
        chrono::Utc.with_ymd_and_hms(2026, month, day, hour, 0, 0).unwrap().with_timezone(&Local)
    });
    cx.update(|_, cx| {
        let mut events = view.read(cx).events_between(expected[0].date_naive(), expected[2].date_naive(), cx);
        events.sort_by_key(|event| (event.start, event.start_time));
        assert_eq!(events.len(), expected.len());
        for (event, start) in events.iter().zip(expected) {
            let end = start + chrono::Duration::hours(1);
            assert_eq!((event.start, event.start_time), (start.date_naive(), Some(start.time())));
            assert_eq!((event.end, event.end_time), (end.date_naive(), Some(end.time())));
        }
    });
}

#[gpui::test]
fn the_whole_calendar_year_fits_and_scrolls_by_month_rows(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        cx.set_reduce_motion(true);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = CalendarView::new(store, window, cx);
        view.sidebar_open = true;
        view.arrange = Some(0);
        view.set_mode(CalendarMode::Year, cx);
        window.focus(&view.focus_handle, cx);
        view
    });
    cx.simulate_resize(size(px(850.), px(368.)));
    cx.run_until_parked();
    let grid = cx.debug_bounds("calendar-year").unwrap();
    let year = cx.update(|_, cx| view.read(cx).month.year());
    let months: Vec<_> = (1..=12).map(|month| {
        let selector = format!("year-month-{year}-{month}").leak();
        let bounds = cx.debug_bounds(selector).unwrap();
        assert!(bounds.left() >= grid.left() && bounds.right() <= grid.right());
        assert!(bounds.top() >= grid.top() && bounds.bottom() <= grid.bottom());
        bounds
    }).collect();
    assert_eq!(months[0].top(), months[3].top());
    assert!(months[4].top() > months[0].top());
    let position = grid.center();
    cx.simulate_mouse_move(position, None, Modifiers::default());
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Lines(point(0., -1.)),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    cx.update(|_, cx| assert_eq!(view.read(cx).month, NaiveDate::from_ymd_opt(year, 5, 1).unwrap()));
    assert_eq!(cx.debug_bounds("calendar-year"), Some(grid));
    cx.simulate_keystrokes("right");
    cx.update(|_, cx| assert_eq!(view.read(cx).month, NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap()));
    cx.simulate_keystrokes("left");
    cx.update(|_, cx| assert_eq!(view.read(cx).month.year(), year));

    cx.simulate_resize(size(px(850.), px(900.)));
    cx.run_until_parked();
    let grid = cx.debug_bounds("calendar-year").unwrap();
    let january = cx.debug_bounds(format!("year-month-{year}-1").leak()).unwrap();
    let april = cx.debug_bounds(format!("year-month-{year}-4").leak()).unwrap();
    let december = cx.debug_bounds(format!("year-month-{year}-12").leak()).unwrap();
    assert_eq!(january.left(), april.left());
    assert!(april.top() > january.top());
    assert!(december.bottom() <= grid.bottom());
}

#[gpui::test]
fn a_task_dropped_on_the_year_grid_keeps_its_duration(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let start = NaiveDate::from_ymd_opt(2032, 1, 12).unwrap();
    let target = NaiveDate::from_ymd_opt(2032, 12, 10).unwrap();
    let time = chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap();
    let task = store.add_task("A two-day trip").unwrap();
    store.set_due(task.id, Some(start), Some(time)).unwrap();
    store.set_due_end(task.id, Some((start.succ_opt().unwrap(), Some(time)))).unwrap();
    cx.update(theme::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let mut view = CalendarView::new(store.clone(), window, cx);
        view.month = calendar::month_start(start);
        view.set_mode(CalendarMode::Year, cx);
        view
    });
    let start = format!("year-date-{}-{start}", calendar::month_start(start)).leak();
    let end = format!("year-date-{}-{target}", calendar::month_start(target)).leak();
    let date = cx.debug_bounds(start).unwrap().center();
    let modifiers = Modifiers::default();
    cx.simulate_mouse_move(date, None, modifiers);
    cx.simulate_click(date, modifiers);
    let selector = format!("calendar-task-{}", task.id).leak();
    let bar = cx.debug_bounds(selector).unwrap().center();
    let end = cx.debug_bounds(end).unwrap().center();
    cx.simulate_mouse_move(bar, None, modifiers);
    cx.simulate_mouse_down(bar, MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(bar.x + px(5.), bar.y), Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_move(end, Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_up(end, MouseButton::Left, modifiers);
    let task = store.task(task.id).unwrap().unwrap();
    assert_eq!((task.due_date, task.due_time), (Some(target), Some(time)));
    assert_eq!((task.end_date, task.end_time), (target.succ_opt(), Some(time)));
}

#[gpui::test]
fn calendar_year_day_card_and_jumps_use_the_clicked_date(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let day = NaiveDate::from_ymd_opt(2032, 4, 13).unwrap();
    let month = calendar::month_start(day);
    let task = store.add_task("Prepare the report").unwrap();
    store.set_due(task.id, Some(day), None).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = CalendarView::new(store, window, cx);
        window.focus(&view.focus_handle, cx);
        view.set_mode(CalendarMode::Year, cx);
        view.open_year_jump(window, cx);
        view
    });
    cx.simulate_input("2032-04-13");
    cx.simulate_keystrokes("enter");
    assert!(cx.debug_bounds("calendar-year-jump").is_none());
    cx.update(|_, cx| assert_eq!(view.read(cx).new_task_day(Local::now().date_naive(), cx), day));

    let selector = format!("year-date-{month}-{day}").leak();
    let modifiers = Modifiers::default();
    let date = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_mouse_move(date, None, modifiers);
    cx.simulate_click(date, modifiers);
    assert!(cx.debug_bounds("year-day-card").is_some());
    let task_selector = format!("calendar-task-{}", task.id).leak();
    assert!(cx.debug_bounds(task_selector).is_some());
    let task_position = cx.debug_bounds(task_selector).unwrap().center();
    cx.simulate_mouse_move(task_position, None, modifiers);
    cx.simulate_click(task_position, modifiers);
    cx.update(|_, cx| assert_eq!(view.read(cx).card, Some((task.id, day))));
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("year-day-card").is_some());
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("year-day-card").is_none());

    cx.simulate_mouse_move(date, None, modifiers);
    cx.simulate_event(MouseDownEvent { position: date, button: MouseButton::Left, modifiers, click_count: 2, first_mouse: false });
    cx.simulate_event(MouseUpEvent { position: date, button: MouseButton::Left, modifiers, click_count: 2 });
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.mode, CalendarMode::Week);
        assert!(view.shown_days(cx).contains(&day));
    });
    cx.simulate_keystrokes("y");
    let october = cx.debug_bounds("year-heading-2032-10").unwrap().center();
    cx.simulate_mouse_move(october, None, modifiers);
    cx.simulate_click(october, modifiers);
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.mode, CalendarMode::Month);
        assert_eq!(view.month, NaiveDate::from_ymd_opt(2032, 10, 1).unwrap());
    });
}

#[gpui::test]
fn escape_dismisses_calendar_overlays_in_order(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        let errors = crate::keymap::load_default(&assets::Assets, cx).unwrap();
        assert!(errors.is_empty(), "keymap errors: {errors:?}");
    });

    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Calendar task").unwrap();
    let today = Local::now().date_naive();
    store.set_due(task.id, Some(today), None).unwrap();
    let (calendar, cx) = cx.add_window_view(|window, cx| {
        let mut calendar = CalendarView::new(store, window, cx);
        window.focus(&calendar.focus_handle, cx);
        calendar.open_card(task.id, today, cx);
        calendar.detail.update(cx, |detail, cx| detail.open_due(cx));
        calendar.open_options(Page::Main, cx);
        calendar
    });

    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        let calendar = calendar.read(cx);
        assert!(calendar.options_dialog.is_none());
        assert!(calendar.detail.read(cx).has_popup());
        assert!(calendar.card.is_some());
    });

    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        let calendar = calendar.read(cx);
        assert!(!calendar.detail.read(cx).has_popup());
        assert!(calendar.card.is_some());
    });

    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        let calendar = calendar.read(cx);
        assert!(calendar.card.is_none());
        assert!(calendar.focus_handle.is_focused(window));
    });

    let modifiers = Modifiers::default();
    let more = cx.debug_bounds("calendar-more-button").unwrap().center();
    cx.simulate_click(more, modifiers);
    cx.update(|_, cx| assert!(calendar.read(cx).more_menu));
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| assert!(!calendar.read(cx).more_menu));

    let task_bar = cx.debug_bounds(format!("calendar-task-{}", task.id).leak()).unwrap().center();
    cx.simulate_mouse_move(task_bar, None, modifiers);
    cx.simulate_mouse_down(task_bar, MouseButton::Right, modifiers);
    cx.simulate_mouse_up(task_bar, MouseButton::Right, modifiers);
    cx.update(|_, cx| assert!(calendar.read(cx).context_menu.is_some()));
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| assert!(calendar.read(cx).context_menu.is_none()));
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| assert!(calendar.read(cx).focus_handle.is_focused(window)));
}

#[gpui::test]
fn calendar_mode_menu_keeps_its_bounds_while_adjusting_days_and_weeks(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::Ru));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = CalendarView::new(store, window, cx);
        view.multi_days = 9;
        view
    });
    let modifiers = Modifiers::default();
    let button = cx.debug_bounds("calendar-mode-button").unwrap().center();
    cx.simulate_mouse_move(button, None, modifiers);
    cx.simulate_click(button, modifiers);
    let menu = cx.debug_bounds("calendar-mode-menu").unwrap();

    for (row, more, mode) in [
        ("calendar-multi-day", "calendar-multi-day-more", CalendarMode::MultiDay(10)),
        ("calendar-multi-week", "calendar-multi-week-more", CalendarMode::MultiWeek(6)),
    ] {
        let row = cx.debug_bounds(row).unwrap().center();
        cx.simulate_mouse_move(row, None, modifiers);
        assert_eq!(cx.debug_bounds("calendar-mode-menu"), Some(menu));

        let more = cx.debug_bounds(more).expect("stepper button appears on hover").center();
        cx.simulate_mouse_move(more, None, modifiers);
        cx.simulate_click(more, modifiers);
        cx.update(|_, cx| assert_eq!(view.read(cx).mode, mode));
        assert_eq!(cx.debug_bounds("calendar-mode-menu"), Some(menu));
    }

    let more = cx.debug_bounds("calendar-multi-week-more").unwrap().center();
    cx.simulate_click(more, modifiers);
    cx.update(|_, cx| assert_eq!(view.read(cx).mode, CalendarMode::MultiWeek(6)));
    assert_eq!(cx.debug_bounds("calendar-mode-menu"), Some(menu));

    cx.simulate_mouse_move(button, None, modifiers);
    assert_eq!(cx.debug_bounds("calendar-mode-menu"), Some(menu));
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("calendar-mode-menu").is_none());
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.multi_days, 10);
        assert_eq!(view.multi_weeks, 6);
        assert!(view.focus_handle.is_focused(window));
    });
}

#[gpui::test]
fn month_cell_creates_a_task_for_its_date_and_list_without_intercepting_task_clicks(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let list = store.create_list("Work").unwrap();
    let day = NaiveDate::from_ymd_opt(2028, 7, 6).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = CalendarView::new(store.clone(), window, cx);
        view.month = calendar::month_start(day);
        view.set_list_filter(Some(Some(list.id)), cx);
        view
    });
    cx.simulate_resize(size(px(1200.), px(600.)));
    let cell = cx.debug_bounds("calendar-day-2028-07-06").unwrap();
    let modifiers = Modifiers::default();
    cx.simulate_mouse_move(cell.center(), None, modifiers);
    cx.simulate_click(cell.center(), modifiers);
    assert!(store.tasks().unwrap().is_empty());
    let editor = cx.debug_bounds("calendar-task-editor").unwrap();
    assert!(editor.right() < cell.left() || editor.left() > cell.right());
    assert!(editor.top() >= px(8.) && editor.bottom() <= px(592.));
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.mode, CalendarMode::Month);
        assert_eq!(view.task_editor.as_ref().unwrap().day, Some(day));
        assert!(view.focus_handle(cx).is_focused(window));
    });
    cx.simulate_input("Plan 9am #work");
    cx.simulate_keystrokes("enter");
    assert!(cx.debug_bounds("calendar-task-editor").is_none());
    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.title, "Plan");
    assert_eq!(task.due_date, Some(day));
    assert_eq!(task.list_id, Some(list.id));
    assert_eq!(task.due_time, chrono::NaiveTime::from_hms_opt(9, 0, 0));
    let bar = cx.debug_bounds(format!("calendar-task-{}", task.id).leak()).unwrap().center();
    cx.simulate_mouse_move(bar, None, modifiers);
    cx.simulate_click(bar, modifiers);
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.card, Some((task.id, day)));
        assert!(view.task_editor.is_none());
    });
    cx.simulate_keystrokes("escape");
    store.set_due_end(task.id, Some((day.succ_opt().unwrap(), None))).unwrap();
    cx.update(|_, cx| view.update(cx, |view, cx| view.reload(cx)));
    let target = cx.debug_bounds("calendar-day-2028-07-13").unwrap().center();
    cx.simulate_mouse_move(bar, None, modifiers);
    cx.simulate_mouse_down(bar, MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(bar.x + px(5.), bar.y), Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_move(target, Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_up(target, MouseButton::Left, modifiers);
    assert_eq!(store.tasks().unwrap().len(), 1);
    let moved = store.task(task.id).unwrap().unwrap();
    assert_eq!(moved.due_date, NaiveDate::from_ymd_opt(2028, 7, 13));
    assert_eq!(moved.end_date, NaiveDate::from_ymd_opt(2028, 7, 14));
    assert_eq!(moved.due_time, task.due_time);
    cx.update(|_, cx| assert!(view.read(cx).task_editor.is_none()));
}

#[gpui::test]
fn calendar_navigation_saves_cell_draft_with_a_date_picker_open(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let day = NaiveDate::from_ymd_opt(2028, 7, 6).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = CalendarView::new(store.clone(), window, cx);
        view.month = calendar::month_start(day);
        view.reload(cx);
        view
    });
    cx.simulate_resize(size(px(1200.), px(650.)));
    let modifiers = Modifiers::default();
    let cell = cx.debug_bounds("calendar-day-2028-07-06").unwrap().center();
    cx.simulate_click(cell, modifiers);
    cx.simulate_input("Save before navigating");
    let due = cx.debug_bounds("task-detail-due-date").unwrap().center();
    cx.simulate_click(due, modifiers);
    assert!(cx.debug_bounds("due-popup-ok").is_some());
    let next = cx.debug_bounds("calendar-next").unwrap().center();
    cx.simulate_click(next, modifiers);
    assert!(cx.debug_bounds("calendar-task-editor").is_none());
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.month, NaiveDate::from_ymd_opt(2028, 8, 1).unwrap());
        assert!(view.task_editor.is_none());
        assert!(view.focus_handle.is_focused(window));
    });
    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].due_date, Some(day));
    let add = cx.debug_bounds("calendar-add-frame").unwrap().center();
    cx.simulate_click(add, modifiers);
    assert!(cx.debug_bounds("calendar-task-editor").is_some());
}

#[gpui::test]
fn month_draft_keeps_adjacent_month_dates_and_saves_once_when_another_day_is_clicked(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let month = NaiveDate::from_ymd_opt(2028, 7, 1).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = CalendarView::new(store.clone(), window, cx);
        view.month = month;
        view.reload(cx);
        view
    });
    let modifiers = Modifiers::default();
    let first = cx.debug_bounds("calendar-day-2028-06-26").unwrap().center();
    cx.simulate_mouse_move(first, None, modifiers);
    cx.simulate_click(first, modifiers);
    cx.simulate_input("June draft");
    let second = cx.debug_bounds("calendar-day-2028-07-17").unwrap().center();
    cx.simulate_mouse_move(second, None, modifiers);
    cx.simulate_click(second, modifiers);
    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].due_date, Some(NaiveDate::from_ymd_opt(2028, 6, 26).unwrap()));
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.month, month);
        assert_eq!(view.task_editor.as_ref().unwrap().day, Some(NaiveDate::from_ymd_opt(2028, 7, 17).unwrap()));
        assert!(view.focus_handle(cx).is_focused(window));
    });
    cx.simulate_input("Save when closed");
    cx.simulate_keystrokes("escape");
    assert_eq!(store.tasks().unwrap().len(), 2);
    assert!(cx.debug_bounds("calendar-task-editor").is_none());
    cx.update(|window, cx| assert!(view.read(cx).focus_handle.is_focused(window)));
}

#[gpui::test]
fn month_draft_uses_the_shared_card_and_keeps_notes_without_a_title(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = CalendarView::new(store.clone(), window, cx);
        view.month = NaiveDate::from_ymd_opt(2028, 7, 1).unwrap();
        view.reload(cx);
        view
    });
    cx.simulate_resize(size(px(850.), px(368.)));
    let modifiers = Modifiers::default();
    let cell = cx.debug_bounds("calendar-day-2028-07-30").unwrap().center();
    cx.simulate_mouse_move(cell, None, modifiers);
    cx.simulate_click(cell, modifiers);
    let original = cx.update(|_, cx| view.read(cx).task_editor.as_ref().unwrap().view.clone());
    let editor = cx.debug_bounds("calendar-task-editor").unwrap();
    assert!(editor.left() >= px(8.) && editor.right() <= px(842.));
    assert!(editor.top() >= px(8.) && editor.bottom() <= px(360.));
    assert!(cx.debug_bounds("task-detail-header").is_some());
    assert!(cx.debug_bounds("task-detail-priority").is_some());
    assert!(cx.debug_bounds("task-detail-checklist-toggle").is_some());
    assert!(cx.debug_bounds("task-detail-list").is_some());
    let date = cx.debug_bounds("task-detail-due-date").unwrap().center();
    cx.simulate_mouse_move(date, None, modifiers);
    cx.simulate_click(date, modifiers);
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| assert_eq!(view.read(cx).task_editor.as_ref().unwrap().view, original));
    let notes = cx.debug_bounds("task-notes-editor").unwrap().center();
    cx.simulate_mouse_move(notes, None, modifiers);
    cx.simulate_click(notes, modifiers);
    cx.simulate_input("Keep these notes");
    let outside = cx.debug_bounds("calendar-day-2028-07-03").unwrap().center();
    cx.simulate_mouse_move(outside, None, modifiers);
    cx.simulate_click(outside, modifiers);
    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].title.is_empty());
    assert_eq!(tasks[0].notes, "Keep these notes");
    assert_eq!(tasks[0].due_date, Some(NaiveDate::from_ymd_opt(2028, 7, 30).unwrap()));
    cx.update(|_, cx| assert_ne!(view.read(cx).task_editor.as_ref().unwrap().view, original));
    cx.simulate_keystrokes("escape");
    assert_eq!(store.tasks().unwrap().len(), 1, "closing an empty draft creates no task");
}

#[gpui::test]
fn calendar_plus_opens_an_editor_with_the_selected_day_and_list(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let list = store.create_list("Work").unwrap();
    let hidden_list = store.create_list("Personal").unwrap();
    let today = Local::now().date_naive();
    let day = today.succ_opt().unwrap();
    let inbox_task = store.add_task("Inbox").unwrap();
    store.set_due(inbox_task.id, Some(day), None).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut calendar = CalendarView::new(store.clone(), window, cx);
        calendar.set_list_filter(Some(Some(list.id)), cx);
        calendar.month = calendar::month_start(day);
        calendar.reload(cx);
        window.focus(&calendar.focus_handle, cx);
        calendar
    });
    let selector = format!("calendar-select-day-{day}").leak();
    let modifiers = Modifiers::default();
    let date = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_mouse_move(date, None, modifiers);
    cx.simulate_click(date, modifiers);
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.selected_day, Some(day));
        assert!(view.tasks_on(day).is_empty(), "Inbox is filtered out");
    });
    let add = cx.debug_bounds("calendar-add-frame").unwrap().center();
    cx.simulate_mouse_move(add, None, modifiers);
    cx.simulate_click(add, modifiers);
    assert!(cx.debug_bounds("calendar-task-editor").is_some());
    cx.simulate_click(add, modifiers);
    assert!(cx.debug_bounds("calendar-task-editor").is_none(), "clicking the add button again closes the editor");
    cx.simulate_click(add, modifiers);
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("calendar-task-editor").is_none());
    assert_eq!(store.tasks().unwrap().len(), 1);
    cx.update(|window, cx| assert!(view.read(cx).focus_handle.is_focused(window)));

    cx.simulate_mouse_move(add, None, modifiers);
    cx.simulate_click(add, modifiers);
    cx.simulate_input("Review 9am #work");
    cx.simulate_keystrokes("enter");
    assert!(cx.debug_bounds("calendar-task-editor").is_none());
    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 2);
    let task = tasks.iter().find(|task| task.id != inbox_task.id).unwrap();
    assert_eq!(task.title, "Review");
    assert_eq!(task.list_id, Some(list.id));
    assert_eq!(task.due_date, Some(day));
    assert_eq!(task.due_time, chrono::NaiveTime::from_hms_opt(9, 0, 0));
    assert_eq!(task.tags, ["work"]);
    cx.update(|_, cx| assert_eq!(view.read(cx).tasks_on(day)[0].id, task.id));
    cx.simulate_click(add, modifiers);
    cx.simulate_input("Save elsewhere ^Personal");
    cx.simulate_keystrokes("enter");
    let elsewhere = store.tasks().unwrap().into_iter().find(|task| task.title == "Save elsewhere").unwrap();
    assert_eq!(elsewhere.list_id, Some(hidden_list.id));
    assert!(cx.debug_bounds("calendar-saved-hint").is_some());
}

#[gpui::test]
fn unchecking_a_calendar_card_reopens_the_task(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Reopen in calendar").unwrap();
    let today = Local::now().date_naive();
    store.set_due(task.id, Some(today), None).unwrap();
    store.set_completed(task.id, true).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        desktop_common::ActivePreferences::update(cx, |p| p.horizons_enabled = true);
    });
    let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
    let selector = format!("calendar-task-{}", task.id).leak();
    let position = cx.debug_bounds(selector).expect("completed task is rendered in the calendar").center();
    let modifiers = Modifiers::default();
    cx.simulate_mouse_move(position, None, modifiers);
    cx.simulate_click(position, modifiers);
    cx.update(|_, cx| assert_eq!(view.read(cx).card, Some((task.id, today))));
    let header = cx.debug_bounds("task-detail-header").expect("calendar task card is rendered");
    let position = point(header.left() + px(27.), header.center().y);
    cx.simulate_mouse_move(position, None, modifiers);
    cx.simulate_click(position, modifiers);
    let reopened = store.task(task.id).unwrap().unwrap();
    assert!(!reopened.is_closed());
    assert_eq!(reopened.due_date, Some(today));
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.card, Some((task.id, today)));
        assert!(view.tasks_on(today).iter().any(|task| !task.is_closed()));
    });
}

#[gpui::test]
#[cfg(debug_assertions)]
fn calendar_row_checkbox_toggles_completion_without_opening_card_and_notes_skip_checkbox(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let task = store.add_task("Bar task").unwrap();
    store.set_due(task.id, Some(today), None).unwrap();
    store.add_checklist_item(task.id, "Step 1", None).unwrap();
    let note = store.add_task("Calendar note").unwrap();
    store.set_due(note.id, Some(today), None).unwrap();
    store.set_kind(note.id, fern_core::Kind::Note).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        desktop_common::ActivePreferences::update(cx, |p| p.horizons_enabled = true);
    });
    let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
    let note_task: &'static str = format!("calendar-task-{}", note.id).leak();
    let note_checkbox: &'static str = format!("calendar-task-checkbox-{}", note.id).leak();
    assert!(cx.debug_bounds(note_task).is_some());
    assert!(cx.debug_bounds(note_checkbox).is_none(), "notes render a page icon rather than a checkbox");

    let box_selector: &'static str = format!("calendar-task-checkbox-{}", task.id).leak();
    let modifiers = Modifiers::default();
    let box_pos = cx.debug_bounds(box_selector).expect("task checkbox is rendered").center();
    cx.simulate_mouse_move(box_pos, None, modifiers);
    cx.simulate_click(box_pos, modifiers);
    assert!(store.task(task.id).unwrap().unwrap().is_completed());
    cx.update(|_, cx| assert_eq!(view.read(cx).card, None, "clicking the checkbox must not open the task card"));

    let box_pos = cx.debug_bounds(box_selector).expect("completed task checkbox remains rendered").center();
    cx.simulate_click(box_pos, modifiers);
    assert!(!store.task(task.id).unwrap().unwrap().is_closed());
}

#[gpui::test]
#[cfg(debug_assertions)]
fn calendar_sidebar_is_full_height_and_header_month_picker_navigates(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let day = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
    let work = store.create_list("Work").unwrap();
    let t1 = store.add_task("Tagged task").unwrap();
    store.set_due(t1.id, Some(day), None).unwrap();
    store.set_tags(t1.id, &["health".to_owned()]).unwrap();
    let t2 = store.add_task("Work task").unwrap();
    store.set_due(t2.id, Some(day), None).unwrap();
    store.set_list(t2.id, Some(work.id)).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut calendar = CalendarView::new(store.clone(), window, cx);
        calendar.sidebar_open = true;
        calendar.month = calendar::month_start(day);
        calendar.anchor = day;
        calendar.reload(cx);
        calendar
    });
    let sidebar = cx.debug_bounds("calendar-sidebar").expect("calendar sidebar is rendered");
    let toolbar = cx.debug_bounds("calendar-toolbar").expect("calendar toolbar is rendered");
    assert_eq!(sidebar.top(), px(0.), "sidebar extends to the top edge of the calendar view");
    assert!(toolbar.left() >= sidebar.right() - px(1.), "header sits beside the full-height sidebar");

    // Tag filter checkbox narrows tasks to matching tags; All resets.
    let modifiers = Modifiers::default();
    let tag_row = cx.debug_bounds("calendar-filter-tag-health").expect("tag filter row").center();
    cx.simulate_click(tag_row, modifiers);
    cx.update(|_, cx| {
        let ids: Vec<_> = view.read(cx).tasks_on(day).into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![t1.id]);
    });
    let all_row = cx.debug_bounds("calendar-filter-all").expect("all filter row").center();
    cx.simulate_click(all_row, modifiers);
    cx.update(|_, cx| assert_eq!(view.read(cx).tasks_on(day).len(), 2));

    // Clicking the header month title opens the 4x3 month picker and jumps to the picked month.
    let title = cx.debug_bounds("calendar-month-title").expect("clickable month title").center();
    cx.simulate_click(title, modifiers);
    assert!(cx.debug_bounds("calendar-month-picker").is_some());
    let march = cx.debug_bounds("calendar-month-pick-2026-3").expect("March cell").center();
    cx.simulate_click(march, modifiers);
    assert!(cx.debug_bounds("calendar-month-picker").is_none());
    cx.update(|_, cx| assert_eq!(view.read(cx).month, NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()));
}

#[gpui::test]
fn drag_resize_span_and_timed_range_and_dual_tz_options(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
    let t_span = store.add_task("Multi-day sprint").unwrap();
    store.set_due(t_span.id, Some(day), None).unwrap();

    let t_timed = store.add_task("Deep block").unwrap();
    let t_start = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
    let t_end = NaiveTime::from_hms_opt(11, 0, 0).unwrap();
    store.set_due(t_timed.id, Some(day), Some(t_start)).unwrap();
    store.set_due_end(t_timed.id, Some((day, Some(t_end)))).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut calendar = CalendarView::new(store.clone(), window, cx);
        calendar.mode = CalendarMode::Week;
        calendar.anchor = day;
        calendar.options.secondary_timezone = Some("UTC".into());
        store.save_calendar_options(&calendar.options).unwrap();
        calendar.reload(cx);
        calendar
    });

    // Horizontal date-span resize from end edge and start edge.
    let end_day = day + chrono::Duration::days(2);
    let earlier_day = day - chrono::Duration::days(1);
    view.update(cx, |cal, cx| {
        let r_end = ResizedTask {
            id: t_span.id,
            title: "Multi-day sprint".into(),
            start: day,
            end: day,
            edge: ResizeEdge::End,
        };
        cal.resize_task_to_day(&r_end, end_day, cx);
        let after_end = store.task(t_span.id).unwrap().unwrap();
        assert_eq!((after_end.due_date, after_end.end_date), (Some(day), Some(end_day)));

        let r_start = ResizedTask {
            id: t_span.id,
            title: "Multi-day sprint".into(),
            start: day,
            end: end_day,
            edge: ResizeEdge::Start,
        };
        cal.resize_task_to_day(&r_start, earlier_day, cx);
        let after_start = store.task(t_span.id).unwrap().unwrap();
        assert_eq!((after_start.due_date, after_start.end_date), (Some(earlier_day), Some(end_day)));
    });

    // Vertical timed resize: drag bottom +60px (1h) and commit, then Escape cancels a second drag.
    view.update(cx, |cal, cx| {
        cal.start_timed_resize(t_timed.id, day, ResizeEdge::End, t_start, t_end, None, px(100.), cx);
        cal.update_timed_resize(px(100. + week_axis::HOUR_HEIGHT), cx);
        cal.finish_timed_resize(cx);
        let saved = store.task(t_timed.id).unwrap().unwrap();
        assert_eq!(saved.due_time, Some(NaiveTime::from_hms_opt(10, 0, 0).unwrap()));
        assert_eq!(saved.end_time, Some(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
    });
    cx.update(|window, cx| {
        view.update(cx, |cal, cx| {
            cal.start_timed_resize(t_timed.id, day, ResizeEdge::End, t_start, NaiveTime::from_hms_opt(12, 0, 0).unwrap(), None, px(100.), cx);
            cal.update_timed_resize(px(220.), cx);
            assert!(cal.try_dismiss_transient(window, cx));
            assert!(cal.timed_resize.is_none());
        });
        let unchanged = store.task(t_timed.id).unwrap().unwrap();
        assert_eq!(unchanged.end_time, Some(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
    });
}

#[gpui::test]
fn writable_caldav_event_crud_and_drag_move(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let sub_id = store
        .add_caldav_subscription(
            "Team",
            "http://127.0.0.1:9/dav/cal/",
            Some("#3B82F6"),
            fern_core::ical::CalendarProvider::CalDav,
            "alice",
            "pw",
            true,
        )
        .unwrap();
    let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
    let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:cal-ev-1\r\nSUMMARY:Design Sync\r\nDTSTART:20261118T100000\r\nDTEND:20261118T110000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    store.upsert_local_calendar_event(sub_id, "cal-ev-1", "http://127.0.0.1:9/dav/cal/cal-ev-1.ics", ics).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        crate::subscriptions::init(store.clone(), cx);
    });
    let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1000.), px(700.)));
    cx.run_until_parked();
    let next_day = day + chrono::Duration::days(1);
    cx.update(|_, cx| {
        let bars = view.read(cx).events_on(day, cx);
        assert_eq!(bars.len(), 1);
        assert!(bars[0].writable);
        assert_eq!(bars[0].subscription_id, sub_id);

        view.update(cx, |cal, cx| {
            cal.move_event_to_day(&DraggedEvent { subscription_id: sub_id, uid: "cal-ev-1".into(), title: "Design Sync".into() }, next_day, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(view.read(cx).events_on(next_day, cx).len(), 1);
        assert!(view.read(cx).events_on(day, cx).is_empty());

        view.update(cx, |cal, cx| {
            cal.request_delete_caldav_event(
                sub_id,
                "cal-ev-1".into(),
                "Design Sync".into(),
                "Team".into(),
                cx,
            );
        });
    });
    cx.run_until_parked();
    // Cancelling via backdrop dismiss keeps the remote event intact.
    let modal_bounds = cx
        .debug_bounds("calendar-delete-event-modal")
        .expect("delete event confirmation modal");
    assert_eq!(view.read_with(cx, |cal, cx| cal.events_on(next_day, cx).len()), 1);
    cx.simulate_click(point(px(10.), px(10.)), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |cal, cx| cal.events_on(next_day, cx).len()), 1);
    assert!(cx.debug_bounds("calendar-delete-event-modal").is_none());

    // Confirming the dialog deletes the CalDAV event.
    view.update(cx, |cal, cx| {
        cal.request_delete_caldav_event(
            sub_id,
            "cal-ev-1".into(),
            "Design Sync".into(),
            "Team".into(),
            cx,
        );
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar-delete-event-modal").is_some());
    let confirm_pos = point(modal_bounds.center().x + px(125.), modal_bounds.center().y + px(34.));
    cx.simulate_click(confirm_pos, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar-delete-event-modal").is_none());
    cx.update(|_, cx| {
        assert!(view.read(cx).events_on(next_day, cx).is_empty());
    });
}

#[test]
fn timed_lane_layout_splits_overlapping_clusters_and_keeps_adjacent_full_width() {
    assert!(assign_timed_lanes(&[]).is_empty());

    // Back-to-back intervals (touching at end == next.start) stay full width.
    let back_to_back = assign_timed_lanes(&[
        TimedInterval { start: 0., end: 60. },
        TimedInterval { start: 60., end: 120. },
    ]);
    assert_eq!(
        back_to_back,
        vec![
            LanePlacement { lane: 0, lane_count: 1 },
            LanePlacement { lane: 0, lane_count: 1 },
        ]
    );

    // Connected staircase cluster: A=[0, 120], B=[30, 90], C=[90, 150] -> 2 lanes (C reuses B's lane).
    // Independent D=[200, 260] -> full width (1 lane).
    let mixed = assign_timed_lanes(&[
        TimedInterval { start: 0., end: 120. },
        TimedInterval { start: 30., end: 90. },
        TimedInterval { start: 90., end: 150. },
        TimedInterval { start: 200., end: 260. },
    ]);
    assert_eq!(mixed[0], LanePlacement { lane: 0, lane_count: 2 });
    assert_eq!(mixed[1], LanePlacement { lane: 1, lane_count: 2 });
    assert_eq!(mixed[2], LanePlacement { lane: 1, lane_count: 2 });
    assert_eq!(mixed[3], LanePlacement { lane: 0, lane_count: 1 });

    // Three mutually overlapping intervals get 3 distinct lanes.
    let triple = assign_timed_lanes(&[
        TimedInterval { start: 100., end: 160. },
        TimedInterval { start: 100., end: 220. },
        TimedInterval { start: 110., end: 170. },
    ]);
    assert_eq!(triple.iter().map(|p| p.lane_count).collect::<Vec<_>>(), vec![3, 3, 3]);
    let lanes: HashSet<usize> = triple.iter().map(|p| p.lane).collect();
    assert_eq!(lanes.len(), 3);
    // Longest starting at 100 gets lane 0.
    assert_eq!(triple[1].lane, 0);
}

#[test]
fn timed_resize_clamps_near_midnight_without_wrapping() {
    let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
    let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();

    // Resizing Start when orig_end is 00:10 must clamp to 00:00, never wrap to 23:55.
    let near_midnight_start = TimedResize {
        id: Id::now_v7(),
        date: day,
        edge: ResizeEdge::Start,
        orig_start: t(0, 0),
        orig_end: t(0, 10),
        end_date: None,
        start_y: px(0.),
        current_time: t(23, 0),
    };
    let (s, e) = near_midnight_start.snapped_range();
    assert_eq!((s, e), (t(0, 0), t(0, 10)));
    assert!(s <= e);

    // Resizing End when orig_start is 23:50 must clamp to 23:59, never wrap to 00:05.
    let near_midnight_end = TimedResize {
        id: Id::now_v7(),
        date: day,
        edge: ResizeEdge::End,
        orig_start: t(23, 50),
        orig_end: t(23, 59),
        end_date: None,
        start_y: px(0.),
        current_time: t(0, 15),
    };
    let (s, e) = near_midnight_end.snapped_range();
    assert_eq!((s, e), (t(23, 50), t(23, 59)));
    assert!(s <= e);

    // Mid-day shrink enforces 15-minute minimum span.
    let shrink_end = TimedResize {
        id: Id::now_v7(),
        date: day,
        edge: ResizeEdge::End,
        orig_start: t(10, 0),
        orig_end: t(11, 0),
        end_date: None,
        start_y: px(0.),
        current_time: t(9, 0),
    };
    assert_eq!(shrink_end.snapped_range(), (t(10, 0), t(10, 15)));

    let shrink_start = TimedResize {
        id: Id::now_v7(),
        date: day,
        edge: ResizeEdge::Start,
        orig_start: t(10, 0),
        orig_end: t(11, 0),
        end_date: None,
        start_y: px(0.),
        current_time: t(12, 0),
    };
    assert_eq!(shrink_start.snapped_range(), (t(10, 45), t(11, 0)));
}

#[gpui::test]
fn multi_day_timed_resize_only_allows_real_start_and_end_edges(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let mon = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
    let tue = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
    let wed = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
    let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();

    let task = store.add_task("Multi-day deploy").unwrap();
    store.set_due(task.id, Some(mon), Some(t(14, 0))).unwrap();
    store.set_due_end(task.id, Some((wed, Some(t(11, 0))))).unwrap();

    cx.update(theme::init);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut cal = CalendarView::new(store.clone(), window, cx);
        cal.mode = CalendarMode::Week;
        cal.anchor = mon;
        cal.reload(cx);
        cal
    });

    view.update(cx, |cal, cx| {
        // Invalid edges are rejected cleanly without starting a misleading preview.
        cal.start_timed_resize(task.id, mon, ResizeEdge::End, t(14, 0), t(23, 59), Some(wed), px(100.), cx);
        assert!(cal.timed_resize.is_none());
        cal.start_timed_resize(task.id, tue, ResizeEdge::Start, t(0, 0), t(23, 59), Some(wed), px(100.), cx);
        assert!(cal.timed_resize.is_none());
        cal.start_timed_resize(task.id, tue, ResizeEdge::End, t(0, 0), t(23, 59), Some(wed), px(100.), cx);
        assert!(cal.timed_resize.is_none());
        cal.start_timed_resize(task.id, wed, ResizeEdge::Start, t(0, 0), t(11, 0), Some(wed), px(100.), cx);
        assert!(cal.timed_resize.is_none());

        // Valid Start resize on start day (Mon) moves due_time +1h and keeps Wed 11:00.
        cal.start_timed_resize(task.id, mon, ResizeEdge::Start, t(14, 0), t(23, 59), Some(wed), px(100.), cx);
        assert!(cal.timed_resize.is_some());
        cal.update_timed_resize(px(100. + week_axis::HOUR_HEIGHT), cx);
        cal.finish_timed_resize(cx);
        let saved = store.task(task.id).unwrap().unwrap();
        assert_eq!((saved.due_date, saved.due_time), (Some(mon), Some(t(15, 0))));
        assert_eq!((saved.end_date, saved.end_time), (Some(wed), Some(t(11, 0))));

        // Valid End resize on end day (Wed) moves end_time +1h and keeps Mon 15:00.
        cal.start_timed_resize(task.id, wed, ResizeEdge::End, t(0, 0), t(11, 0), Some(wed), px(100.), cx);
        assert!(cal.timed_resize.is_some());
        cal.update_timed_resize(px(100. + week_axis::HOUR_HEIGHT), cx);
        cal.finish_timed_resize(cx);
        let saved = store.task(task.id).unwrap().unwrap();
        assert_eq!((saved.due_date, saved.due_time), (Some(mon), Some(t(15, 0))));
        assert_eq!((saved.end_date, saved.end_time), (Some(wed), Some(t(12, 0))));
    });
}

#[gpui::test]
fn overlapping_timed_tasks_render_side_by_side_in_week_view(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
    let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();

    let first = store.add_task("Overlap A").unwrap();
    store.set_due(first.id, Some(day), Some(t(10, 0))).unwrap();
    store.set_due_end(first.id, Some((day, Some(t(11, 0))))).unwrap();

    let second = store.add_task("Overlap B").unwrap();
    store.set_due(second.id, Some(day), Some(t(10, 30))).unwrap();
    store.set_due_end(second.id, Some((day, Some(t(11, 30))))).unwrap();

    cx.update(theme::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let mut cal = CalendarView::new(store.clone(), window, cx);
        cal.mode = CalendarMode::Day;
        cal.anchor = day;
        cal.reload(cx);
        cal
    });
    cx.simulate_resize(size(px(800.), px(700.)));
    cx.run_until_parked();

    let a = cx.debug_bounds(format!("calendar-task-{}", first.id).leak()).expect("first task bounds");
    let b = cx.debug_bounds(format!("calendar-task-{}", second.id).leak()).expect("second task bounds");
    assert!(
        a.right() <= b.left() + px(1.) || b.right() <= a.left() + px(1.),
        "overlapping timed tasks should sit side-by-side: a={a:?}, b={b:?}"
    );
}

#[gpui::test]
fn caldav_event_editor_preserves_draft_on_error_and_preserves_tzid(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let sub_id = store
        .add_caldav_subscription(
            "Team",
            "http://127.0.0.1:9/dav/cal/",
            Some("#3B82F6"),
            fern_core::ical::CalendarProvider::CalDav,
            "alice",
            "pw",
            true,
        )
        .unwrap();
    let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:tz-ev-1\r\nSUMMARY:NY Sync\r\nLOCATION:Room 1\r\nDTSTART;TZID=America/New_York:20261118T090000\r\nDTEND;TZID=America/New_York:20261118T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    store
        .upsert_local_calendar_event(sub_id, "tz-ev-1", "http://127.0.0.1:9/dav/cal/tz-ev-1.ics", ics)
        .unwrap();

    cx.update(|cx| {
        theme::init(cx);
        crate::subscriptions::init(store.clone(), cx);
    });
    let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
    cx.run_until_parked();

    let search_from = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
    let search_to = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();

    cx.update(|window, cx| {
        let bars = view.read(cx).events_between(search_from, search_to, cx);
        assert_eq!(bars.len(), 1);
        let event = bars[0].clone();

        view.update(cx, |cal, cx| {
            cal.open_edit_event_editor(&event, window, cx);
            let ed = cal.event_editor.as_ref().expect("editor opened");
            assert_eq!(ed.title.read(cx).text(), "NY Sync");
            assert_eq!(ed.location.read(cx).text(), "Room 1");
            assert_eq!(ed.timezone.as_deref(), Some("America/New_York"));

            // Invalid date must keep the draft open with an error message.
            ed.title.update(cx, |input, cx| input.set_text("Updated NY Sync", cx));
            ed.date.update(cx, |input, cx| input.set_text("not-a-date", cx));
            cal.save_event_editor(window, cx);
            let ed = cal.event_editor.as_ref().expect("draft preserved on invalid date");
            assert!(ed.error.is_some());
            assert_eq!(ed.title.read(cx).text(), "Updated NY Sync");

            // Backend save failure (unknown subscription) must also preserve the draft.
            let valid_date = event.start.format("%Y-%m-%d").to_string();
            ed.date.update(cx, |input, cx| input.set_text(valid_date, cx));
            if let Some(ed_mut) = cal.event_editor.as_mut() {
                ed_mut.subscription_id = Id::now_v7();
            }
            cal.save_event_editor(window, cx);
            let ed = cal.event_editor.as_ref().expect("draft preserved on save failure");
            assert!(ed.error.is_some());
            assert_eq!(ed.title.read(cx).text(), "Updated NY Sync");

            // Restore valid subscription and save -> closes editor and preserves TZID.
            if let Some(ed_mut) = cal.event_editor.as_mut() {
                ed_mut.subscription_id = sub_id;
            }
            cal.save_event_editor(window, cx);
            assert!(cal.event_editor.is_none());
        });
    });

    let saved = store
        .calendar_events(sub_id)
        .unwrap()
        .into_iter()
        .find(|ev| ev.uid == "tz-ev-1")
        .expect("stored event");
    assert!(saved.ics.contains("SUMMARY:Updated NY Sync"), "ics: {}", saved.ics);
    assert!(saved.ics.contains("TZID=America/New_York"), "TZID must be preserved: {}", saved.ics);
}

#[gpui::test]
fn caldav_recurring_event_is_protected_from_clobbering_rrule(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let sub_id = store
        .add_caldav_subscription(
            "Team",
            "http://127.0.0.1:9/dav/cal/",
            Some("#3B82F6"),
            fern_core::ical::CalendarProvider::CalDav,
            "alice",
            "pw",
            true,
        )
        .unwrap();
    let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
    let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:recur-1\r\nSUMMARY:Weekly Standup\r\nDTSTART:20261118T090000\r\nDTEND:20261118T093000\r\nRRULE:FREQ=WEEKLY;BYDAY=WE;COUNT=4\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    store
        .upsert_local_calendar_event(sub_id, "recur-1", "http://127.0.0.1:9/dav/cal/recur-1.ics", ics)
        .unwrap();

    cx.update(|cx| {
        theme::init(cx);
        crate::subscriptions::init(store.clone(), cx);
    });
    let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
    cx.run_until_parked();

    cx.update(|window, cx| {
        let bars = view.read(cx).events_on(day, cx);
        assert_eq!(bars.len(), 1);
        assert!(bars[0].recurring);
        let event = bars[0].clone();

        view.update(cx, |cal, cx| {
            cal.open_edit_event_editor(&event, window, cx);
            assert!(cal.event_editor.is_none(), "recurring master must not open destructive editor");

            cal.move_event_to_day(
                &DraggedEvent { subscription_id: sub_id, uid: "recur-1".into(), title: "Weekly Standup".into() },
                day + chrono::Duration::days(1),
                cx,
            );
        });
    });

    let stored = store
        .calendar_events(sub_id)
        .unwrap()
        .into_iter()
        .find(|ev| ev.uid == "recur-1")
        .expect("recurring event still stored");
    assert!(stored.ics.contains("RRULE:FREQ=WEEKLY;BYDAY=WE;COUNT=4"));
}

#[test]
fn dual_tz_reference_day_and_dst_offset_shift_detection() {
    let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
    let week = vec![
        d(2026, 10, 26),
        d(2026, 10, 27),
        d(2026, 10, 28),
        d(2026, 10, 29),
        d(2026, 10, 30),
        d(2026, 10, 31),
        d(2026, 11, 1),
    ];

    // Priority: selected_day > today > anchor > first visible day.
    assert_eq!(dual_tz_reference_day(&week, Some(d(2026, 11, 1)), d(2026, 10, 26), d(2026, 10, 28)), d(2026, 11, 1));
    assert_eq!(dual_tz_reference_day(&week, Some(d(2026, 12, 1)), d(2026, 10, 26), d(2026, 10, 28)), d(2026, 10, 28));
    assert_eq!(dual_tz_reference_day(&week, None, d(2026, 10, 30), d(2026, 12, 1)), d(2026, 10, 30));
    assert_eq!(dual_tz_reference_day(&week, None, d(2026, 12, 1), d(2026, 12, 2)), d(2026, 10, 26));

    // US DST fall-back on 2026-11-01 shifts America/New_York vs UTC from -240m (EDT) to -300m (EST).
    let ny = "America/New_York";
    assert_eq!(calendar::timezone_offset_diff_minutes(d(2026, 10, 31), &chrono::Utc, ny), Some(-240));
    assert_eq!(calendar::timezone_offset_diff_minutes(d(2026, 11, 1), &chrono::Utc, ny), Some(-300));
    assert_eq!(calendar::timezone_offset_label_on(d(2026, 10, 31), ny), "UTC-4");
    assert_eq!(calendar::timezone_offset_label_on(d(2026, 11, 1), ny), "UTC-5");
    assert_eq!(calendar::timezone_offset_label_on(d(2026, 11, 1), "Asia/Kolkata"), "UTC+5:30");
}

#[gpui::test]
fn horizon_goal_banners_render_in_week_month_year_and_respect_horizons_enabled(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let cur_week = fern_core::Horizon::current_week(today);
    let past_week = cur_week.shift(-1);
    let next_week = cur_week.shift(1);
    let cur_month = fern_core::Horizon::current_month(today);
    let past_month = cur_month.shift(-1);
    let cur_year = fern_core::Horizon::current_year(today);

    let week_goal = store.add_task("Ship week milestone").unwrap();
    store.set_horizon(week_goal.id, Some(cur_week)).unwrap();

    let past_week_goal = store.add_task("Carryover week goal").unwrap();
    store.set_horizon(past_week_goal.id, Some(past_week)).unwrap();

    let next_week_goal = store.add_task("Future week goal").unwrap();
    store.set_horizon(next_week_goal.id, Some(next_week)).unwrap();

    let done_week_goal = store.add_task("Finished week goal").unwrap();
    store.set_horizon(done_week_goal.id, Some(cur_week)).unwrap();
    store.set_completed(done_week_goal.id, true).unwrap();

    let month_goal = store.add_task("Close month target").unwrap();
    store.set_horizon(month_goal.id, Some(cur_month)).unwrap();

    let past_month_goal = store.add_task("Carryover month goal").unwrap();
    store.set_horizon(past_month_goal.id, Some(past_month)).unwrap();

    let year_goal = store.add_task("Annual objective").unwrap();
    store.set_horizon(year_goal.id, Some(cur_year)).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        desktop_common::ActivePreferences::update(cx, |p| p.horizons_enabled = true);
    });
    let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    let opened: Rc<std::cell::RefCell<Vec<Id>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    let opened_sub = opened.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, OpenTask(id), _| {
            opened_sub.borrow_mut().push(*id);
        })
        .detach();
    });

    let month_chip: &'static str = format!("calendar-horizon-chip-{}", month_goal.id).leak();
    let past_month_chip: &'static str = format!("calendar-horizon-chip-{}", past_month_goal.id).leak();
    let week_chip: &'static str = format!("calendar-horizon-chip-{}", week_goal.id).leak();
    let past_week_chip: &'static str = format!("calendar-horizon-chip-{}", past_week_goal.id).leak();
    let past_week_check: &'static str = format!("calendar-horizon-check-{}", past_week_goal.id).leak();
    let next_week_chip: &'static str = format!("calendar-horizon-chip-{}", next_week_goal.id).leak();
    let done_week_chip: &'static str = format!("calendar-horizon-chip-{}", done_week_goal.id).leak();
    let year_chip: &'static str = format!("calendar-horizon-chip-{}", year_goal.id).leak();

    // Month view: shows current month + past unfinished month goals only.
    let month_banner = cx
        .debug_bounds("calendar-month-goals-banner")
        .expect("month goals banner renders when horizons enabled");
    assert!(month_banner.size.height > px(0.));
    assert!(cx.debug_bounds(month_chip).is_some());
    assert!(cx.debug_bounds(past_month_chip).is_some());
    assert!(cx.debug_bounds(week_chip).is_none());

    // Navigating to next month hides past-month rollovers when no goals exist for that month.
    cx.update(|_, cx| view.update(cx, |cal, cx| cal.shift(1, cx)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar-month-goals-banner").is_none());
    cx.update(|_, cx| view.update(cx, |cal, cx| cal.go_to_today(cx)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar-month-goals-banner").is_some());

    // Week view: shows current week + past unfinished week goals, excludes future & completed.
    cx.update(|_, cx| view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Week, cx)));
    cx.run_until_parked();
    let week_banner = cx
        .debug_bounds("calendar-week-goals-banner")
        .expect("week goals banner renders when horizons enabled");
    assert!(week_banner.size.height > px(0.));
    assert!(cx.debug_bounds(week_chip).is_some());
    assert!(cx.debug_bounds(past_week_chip).is_some());
    assert!(cx.debug_bounds(next_week_chip).is_none());
    assert!(cx.debug_bounds(done_week_chip).is_none());

    // Clicking mini checkbox completes task and removes it from the banner.
    let check_pos = cx.debug_bounds(past_week_check).expect("week goal checkbox").center();
    cx.simulate_click(check_pos, Modifiers::default());
    cx.run_until_parked();
    assert!(store.task(past_week_goal.id).unwrap().unwrap().is_completed());
    assert!(cx.debug_bounds(past_week_chip).is_none());

    // Clicking the goal chip emits OpenTask.
    let chip_pos = cx.debug_bounds(week_chip).expect("week goal chip").center();
    cx.simulate_click(chip_pos, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(*opened.borrow(), vec![week_goal.id]);

    // Year view: shows displayed year's goals cleanly in the Year header strip.
    cx.update(|_, cx| view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Year, cx)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar-year-goals-banner").is_some());
    assert!(cx.debug_bounds(year_chip).is_some());

    // Disabling Horizons hides banners completely across Year, Month, and Week views.
    cx.update(|_, cx| {
        desktop_common::ActivePreferences::update(cx, |p| p.horizons_enabled = false);
        view.update(cx, |cal, cx| cal.reload(cx));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar-year-goals-banner").is_none());

    cx.update(|_, cx| view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Month, cx)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar-month-goals-banner").is_none());

    cx.update(|_, cx| view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Week, cx)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar-week-goals-banner").is_none());
}

#[gpui::test]
fn dragging_horizon_goal_chip_onto_calendar_day_schedules_and_clears_horizon(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let cur_month = fern_core::Horizon::current_month(today);
    let cur_week = fern_core::Horizon::current_week(today);

    let month_goal = store.add_task("Drag month goal").unwrap();
    store.set_horizon(month_goal.id, Some(cur_month)).unwrap();

    let week_goal = store.add_task("Drag week goal").unwrap();
    store.set_horizon(week_goal.id, Some(cur_week)).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        desktop_common::ActivePreferences::update(cx, |p| p.horizons_enabled = true);
    });
    let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    // Drag month goal chip onto today's day cell in Month view.
    let modifiers = Modifiers::default();
    let month_chip_sel: &'static str = format!("calendar-horizon-chip-{}", month_goal.id).leak();
    let day_cell_sel: &'static str = format!("calendar-day-{today}").leak();
    let chip_pos = cx.debug_bounds(month_chip_sel).expect("month goal chip").center();
    let cell_pos = cx.debug_bounds(day_cell_sel).expect("today day cell").center();
    cx.simulate_mouse_move(chip_pos, None, modifiers);
    cx.simulate_mouse_down(chip_pos, MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(chip_pos.x + px(6.), chip_pos.y), Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_move(cell_pos, Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_up(cell_pos, MouseButton::Left, modifiers);
    cx.run_until_parked();

    let scheduled_month = store.task(month_goal.id).unwrap().unwrap();
    assert_eq!(scheduled_month.due_date, Some(today));
    assert_eq!(scheduled_month.horizon, None);
    assert!(cx.debug_bounds("calendar-month-goals-banner").is_none());
    let scheduled_bar: &'static str = format!("calendar-task-{}", month_goal.id).leak();
    assert!(cx.debug_bounds(scheduled_bar).is_some());

    // Drag week goal chip onto a day column in Week view.
    cx.update(|_, cx| view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Week, cx)));
    cx.run_until_parked();
    let week_chip_sel: &'static str = format!("calendar-horizon-chip-{}", week_goal.id).leak();
    let week_day_sel: &'static str = format!("week-allday-{today}").leak();
    let chip_pos = cx.debug_bounds(week_chip_sel).expect("week goal chip").center();
    let col_pos = cx.debug_bounds(week_day_sel).expect("week day column").center();
    cx.simulate_mouse_move(chip_pos, None, modifiers);
    cx.simulate_mouse_down(chip_pos, MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(chip_pos.x + px(6.), chip_pos.y), Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_move(col_pos, Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_up(col_pos, MouseButton::Left, modifiers);
    cx.run_until_parked();

    let scheduled_week = store.task(week_goal.id).unwrap().unwrap();
    assert_eq!(scheduled_week.due_date, Some(today));
    assert_eq!(scheduled_week.horizon, None);
    assert!(cx.debug_bounds("calendar-week-goals-banner").is_none());
}

#[gpui::test]
fn caldav_event_editor_validates_empty_title_and_invalid_times(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let sub_id = store
        .add_caldav_subscription(
            "Work",
            "http://127.0.0.1:9/dav/work/",
            Some("#3B82F6"),
            fern_core::ical::CalendarProvider::CalDav,
            "alice",
            "pw",
            true,
        )
        .unwrap();
    cx.update(|cx| {
        theme::init(cx);
        crate::subscriptions::init(store.clone(), cx);
    });
    let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
    cx.run_until_parked();

    let day = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
    cx.update(|window, cx| {
        view.update(cx, |cal, cx| {
            cal.open_new_event_editor(day, window, cx);

            // Empty title must set ed.error and keep editor open.
            cal.save_event_editor(window, cx);
            let ed = cal.event_editor.as_ref().expect("editor stays open on empty title");
            assert!(ed.error.is_some());

            // Invalid start time must set ed.error instead of converting to all-day.
            ed.title.update(cx, |inp, cx| inp.set_text("Late sync", cx));
            ed.start_time.update(cx, |inp, cx| inp.set_text("25:99", cx));
            cal.save_event_editor(window, cx);
            let ed = cal.event_editor.as_ref().expect("editor stays open on bad start time");
            assert!(ed.error.is_some());

            // Empty start time when !all_day must also set ed.error.
            ed.start_time.update(cx, |inp, cx| inp.set_text("", cx));
            cal.save_event_editor(window, cx);
            let ed = cal.event_editor.as_ref().expect("editor stays open on empty start time");
            assert!(ed.error.is_some());

            // Invalid end time must set ed.error instead of silently falling back.
            ed.start_time.update(cx, |inp, cx| inp.set_text("10:30 PM", cx));
            ed.end_time.update(cx, |inp, cx| inp.set_text("not-a-time", cx));
            cal.save_event_editor(window, cx);
            let ed = cal.event_editor.as_ref().expect("editor stays open on bad end time");
            assert!(ed.error.is_some());
            assert!(store.calendar_events(sub_id).unwrap().is_empty());

            // Valid 12h start/end strings save cleanly as a timed event.
            ed.end_time.update(cx, |inp, cx| inp.set_text("11:15 PM", cx));
            cal.save_event_editor(window, cx);
            assert!(cal.event_editor.is_none());
        });
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        let bars = view.read(cx).events_on(day, cx);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].title, "Late sync");
        assert_eq!(bars[0].start_time, NaiveTime::from_hms_opt(22, 30, 0));
        assert_eq!(bars[0].end_time, NaiveTime::from_hms_opt(23, 15, 0));
    });
}

#[gpui::test]
fn calendar_modal_blocks_view_and_navigation_shortcuts(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    store
        .add_caldav_subscription(
            "Team",
            "http://127.0.0.1:9/dav/cal/",
            None,
            fern_core::ical::CalendarProvider::CalDav,
            "alice",
            "pw",
            true,
        )
        .unwrap();
    cx.update(|cx| {
        theme::init(cx);
        crate::subscriptions::init(store.clone(), cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut cal = CalendarView::new(store.clone(), window, cx);
        cal.month = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        cal.anchor = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        cal.reload(cx);
        window.focus(&cal.focus_handle, cx);
        cal
    });
    cx.simulate_resize(size(px(1000.), px(700.)));
    cx.run_until_parked();

    // 1. Options modal blocks view/navigation actions.
    cx.update(|_, cx| view.update(cx, |cal, cx| cal.open_options(Page::Main, cx)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.dispatch_action(Box::new(DayView), cx);
        window.dispatch_action(Box::new(WeekView), cx);
        window.dispatch_action(Box::new(NextPeriod), cx);
        window.dispatch_action(Box::new(CalendarToday), cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let cal = view.read(cx);
        assert_eq!(cal.mode, CalendarMode::Month);
        assert_eq!(cal.month, NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).options_dialog.is_none()));

    // 2. Event editor modal blocks view/navigation actions even when focus leaves TextInput.
    cx.update(|window, cx| {
        view.update(cx, |cal, cx| {
            cal.open_new_event_editor(NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(), window, cx);
            window.focus(&cal.focus_handle, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.dispatch_action(Box::new(AgendaView), cx);
        window.dispatch_action(Box::new(PreviousPeriod), cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let cal = view.read(cx);
        assert_eq!(cal.mode, CalendarMode::Month);
        assert_eq!(cal.month, NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).event_editor.is_none()));
}

#[gpui::test]
fn calendar_options_subpages_have_working_back_buttons(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(theme::init);
    let (view, cx) = cx.add_window_view(|_, cx| CalendarOptionsDialog::new(store, Page::Color, cx));
    cx.simulate_resize(size(px(700.), px(600.)));
    cx.run_until_parked();

    let back_pos = cx
        .debug_bounds("calendar-options-back")
        .expect("back button on Color page")
        .center();
    cx.simulate_click(back_pos, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar-options-back").is_none());

    let style_pos = cx.debug_bounds("calendar-style").expect("style row on Main page").center();
    cx.simulate_click(style_pos, Modifiers::default());
    cx.run_until_parked();
    let back_pos = cx
        .debug_bounds("calendar-options-back")
        .expect("back button on Style page")
        .center();
    cx.simulate_click(back_pos, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("calendar-options-back").is_none());
    let _ = view;
}

#[gpui::test]
fn bucket_range_matches_per_day_queries_across_repeats_spans_and_events(cx: &mut TestAppContext) {
    use fern_core::{Frequency, Recurrence};
    let store = Rc::new(Store::open_in_memory().unwrap());
    let month = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let d = |day| NaiveDate::from_ymd_opt(2026, 11, day).unwrap();

    let single = store.add_task("Single").unwrap();
    store.set_due(single.id, Some(d(5)), None).unwrap();

    let span = store.add_task("Multi-day").unwrap();
    store.set_due(span.id, Some(d(10)), None).unwrap();
    store.set_due_end(span.id, Some((d(12), None))).unwrap();

    let repeat = store.add_task("Every 3 days").unwrap();
    let rep_start = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
    store.set_due(repeat.id, Some(rep_start), None).unwrap();
    store
        .set_repeat(repeat.id, Some(Recurrence::new(Frequency::Daily, 3)), rep_start)
        .unwrap();

    store
        .add_subscription_with_feed(
            "Meetings",
            "https://example.com/cal.ics",
            None,
            concat!(
                "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:wk-1\nSUMMARY:Standup\n",
                "DTSTART:20261102T090000\nDTEND:20261102T093000\n",
                "RRULE:FREQ=WEEKLY;COUNT=4\nEND:VEVENT\nEND:VCALENDAR"
            ),
        )
        .unwrap();

    cx.update(|cx| {
        theme::init(cx);
        crate::subscriptions::init(store.clone(), cx);
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut cal = CalendarView::new(store.clone(), window, cx);
        cal.month = month;
        cal.reload(cx);
        cal
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        let cal = view.read(cx);
        let weeks = cal.month_grid_weeks(cx);
        let first = weeks[0][0];
        let last = weeks[weeks.len() - 1][6];
        let today = d(15);
        let buckets = cal.bucket_range(first, last, today, cx);

        for week in weeks {
            for day in week {
                let expected_tasks: Vec<_> = cal.tasks_on(day).into_iter().map(|t| t.id).collect();
                let actual_tasks: Vec<_> = buckets
                    .tasks
                    .get(&day)
                    .map(|list| list.iter().map(|t| t.id).collect())
                    .unwrap_or_default();
                assert_eq!(actual_tasks, expected_tasks, "tasks mismatch on {day}");

                let expected_events: Vec<_> =
                    cal.events_on(day, cx).into_iter().map(|e| (e.key, e.start)).collect();
                let actual_events: Vec<_> = buckets
                    .events
                    .get(&day)
                    .map(|list| list.iter().map(|e| (e.key.clone(), e.start)).collect())
                    .unwrap_or_default();
                assert_eq!(actual_events, expected_events, "events mismatch on {day}");
            }
        }
    });
}

#[gpui::test]
fn year_to_agenda_restores_anchor_month_and_shift_advances_agenda(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let future_year = today.year() + 1;
    let nov_day = NaiveDate::from_ymd_opt(future_year, 11, 18).unwrap();
    let nov_month = NaiveDate::from_ymd_opt(future_year, 11, 1).unwrap();
    let jan_day = NaiveDate::from_ymd_opt(future_year, 1, 10).unwrap();
    let jan_task = store.add_task("Jan item").unwrap();
    store.set_due(jan_task.id, Some(jan_day), None).unwrap();
    let nov_task = store.add_task("Nov item").unwrap();
    store.set_due(nov_task.id, Some(nov_day), None).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        desktop_common::ActivePreferences::update(cx, |p| p.horizons_enabled = true);
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut cal = CalendarView::new(store.clone(), window, cx);
        cal.anchor = nov_day;
        cal.month = nov_month;
        cal.set_mode(CalendarMode::Year, cx);
        cal
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        let cal = view.read(cx);
        assert_eq!(cal.month, NaiveDate::from_ymd_opt(future_year, 1, 1).unwrap());
    });

    cx.update(|_, cx| {
        view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Agenda, cx));
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        let cal = view.read(cx);
        assert_eq!(cal.month, nov_month);
        assert_eq!(cal.toolbar_month(cx), nov_month);
        assert_eq!(calendar::month_start(cal.agenda_start()), nov_month);
        assert_eq!(cal.agenda.first().map(|(d, _)| *d), Some(nov_day));
    });

    // When `month` sits before `today`, `toolbar_month` matches `agenda_start()` and `shift(1)` advances from it.
    let current_month = calendar::month_start(today);
    let next_month = calendar::add_months(current_month, 1);
    cx.update(|_, cx| {
        view.update(cx, |cal, cx| {
            cal.month = NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap();
            cal.selected_day = None;
            cal.reload(cx);
            assert_eq!(cal.toolbar_month(cx), current_month);
            cal.shift(1, cx);
            assert_eq!(cal.month, next_month);
            assert_eq!(cal.agenda_start(), next_month);
            assert_eq!(cal.toolbar_month(cx), next_month);
        });
    });
}

#[gpui::test]
fn reload_skips_countdowns_when_disabled_and_loads_when_enabled(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let countdown = store
        .add_countdown("Launch", today, fern_core::CountdownKind::Countdown)
        .unwrap();
    let mut options = CalendarOptions {
        show_countdowns: false,
        ..CalendarOptions::default()
    };
    store.save_calendar_options(&options).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        desktop_common::ActivePreferences::update(cx, |p| p.horizons_enabled = true);
    });
    let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));

    cx.update(|_, cx| {
        view.update(cx, |cal, cx| {
            assert!(cal.countdowns.is_empty());

            cal.set_mode(CalendarMode::Agenda, cx);
            assert!(cal.countdowns.is_empty());

            options.show_countdowns = true;
            store.save_calendar_options(&options).unwrap();
            cal.reload(cx);
            assert_eq!(cal.countdowns.len(), 1);
            assert_eq!(cal.countdowns[0].id, countdown.id);

            cal.set_mode(CalendarMode::Month, cx);
            assert_eq!(cal.countdowns.len(), 1);
            assert_eq!(cal.countdowns[0].id, countdown.id);

            options.show_countdowns = false;
            store.save_calendar_options(&options).unwrap();
            cal.reload(cx);
            assert!(cal.countdowns.is_empty());
        });
    });
}
