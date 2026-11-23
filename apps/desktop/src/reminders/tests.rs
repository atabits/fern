use chrono::NaiveDate;

use super::platform::{MAX_TOAST_WAITERS, WaiterSlot, encode_reg_sz};
use super::*;

fn notice(title: &str, hour: u32) -> Notice {
    let at = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap().and_hms_opt(hour, 0, 0).unwrap();
    Notice { target: Target::Task(uuid::Uuid::now_v7()), title: title.into(), at, constant: false, has_goal: false }
}

#[test]
fn daily_notification_text() {
    let titles: Vec<String> = (1..=7).map(|i| format!("Task {i}")).collect();
    let (title, body) = daily_text(&titles, &i18n::EN);
    assert_eq!(title, "Today: 7 tasks");
    assert_eq!(body.lines().count(), 6, "five tasks and an ellipsis");
    let (_, empty) = daily_text(&[], &i18n::EN);
    assert_eq!(empty, i18n::EN.daily_empty);
}

#[test]
fn newest_first_and_unread_count() {
    let mut notices = Notices::default();
    notices.push(vec![notice("a", 9), notice("b", 10)]);
    notices.mark_read();
    notices.push(vec![notice("c", 11)]);
    let titles: Vec<&str> = notices.items.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, ["c", "b", "a"]);
    assert_eq!(notices.unread, 1);
}

#[test]
fn missed_reminders_are_found_between_checks() {
    let store = Store::open_in_memory().unwrap();
    let task = store.add_task("water the plants").unwrap();
    let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    store.set_due(task.id, Some(day), None).unwrap();
    store.set_reminders(task.id, &[0]).unwrap();
    let at = |h| day.and_hms_opt(h, 0, 0).unwrap();
    assert_eq!(fired(&store, at(8), at(10)).len(), 1);
    assert!(fired(&store, at(9), at(10)).is_empty());

    let habit = store.add_habit("Read").unwrap();
    store.set_habit_reminders(habit.id, &[chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap()]).unwrap();
    let notices = fired(&store, at(9), at(10));
    assert_eq!(notices.iter().map(|n| n.target).collect::<Vec<_>>(), [Target::Habit(habit.id)]);
    assert!(!notices[0].has_goal);
    assert_eq!(notices[0].actions(), [NoticeAction::Snooze, NoticeAction::CheckIn]);
    store.toggle_checkin(habit.id, day).unwrap();
    assert!(fired(&store, at(9), at(10)).is_empty(), "a habit done today is not reminded");
}

#[test]
fn habit_and_task_reminder_actions_and_snooze() {
    let store = Store::open_in_memory().unwrap();
    let task = store.add_task("water the plants").unwrap();
    assert_eq!(actions(&store, Target::Task(task.id)), NoticeAction::for_task());
    let read = store.add_habit("Read").unwrap();
    assert_eq!(actions(&store, Target::Habit(read.id)), [NoticeAction::Snooze, NoticeAction::CheckIn]);
    let water = store.add_habit("Water").unwrap();
    store.update_habit(water.id, "Water", Some(&fern_core::Goal::new(8.0, "glasses"))).unwrap();
    assert_eq!(
        actions(&store, Target::Habit(water.id)),
        [NoticeAction::Snooze, NoticeAction::Record, NoticeAction::PlusOne]
    );
    for action in actions(&store, Target::Habit(water.id))
        .iter()
        .chain(actions(&store, Target::Task(task.id)))
        .chain(NoticeAction::for_countdown())
    {
        assert_eq!(NoticeAction::from_key(action.key()), Some(*action));
    }

    // A snoozed habit reminder and a snoozed task reminder both fire again when their time comes.
    let at = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap().and_hms_opt(9, 0, 0).unwrap();
    store.snooze_habit(read.id, at + chrono::Duration::minutes(SNOOZE_MINUTES)).unwrap();
    store.snooze_habit(water.id, at + chrono::Duration::minutes(SNOOZE_MINUTES)).unwrap();
    store.snooze_task(task.id, at + chrono::Duration::minutes(5)).unwrap();
    store.set_task_constant_reminder(task.id, true).unwrap();
    let notices = fired(&store, at, at + chrono::Duration::minutes(20));
    assert!(notices.iter().any(|n| n.target == Target::Task(task.id) && n.constant));
    assert!(notices.iter().any(|n| n.target == Target::Habit(read.id) && !n.has_goal));
    let water_notice = notices.iter().find(|n| n.target == Target::Habit(water.id)).unwrap();
    assert!(water_notice.has_goal);
    assert_eq!(water_notice.actions(), [NoticeAction::Snooze, NoticeAction::Record, NoticeAction::PlusOne]);
}

#[test]
fn waiter_slots_are_bounded_and_released_on_drop() {
    let mut held = Vec::new();
    for _ in 0..MAX_TOAST_WAITERS {
        held.push(WaiterSlot::try_acquire().expect("slot should be available"));
    }
    assert!(WaiterSlot::try_acquire().is_none(), "exceeding MAX_TOAST_WAITERS must fail");
    held.pop();
    assert!(WaiterSlot::try_acquire().is_some(), "dropping a slot must free it");
}

#[test]
fn windows_toast_xml_escapes_text_and_adds_reminder_scenario() {
    let normal = build_toast_xml("Call <Bob> & Co", "At \"5\"", &[("snooze-5m", "5m"), ("complete", "Done")], false);
    assert!(normal.contains("Call &lt;Bob&gt; &amp; Co"));
    assert!(normal.contains("At &quot;5&quot;"));
    assert!(!normal.contains("scenario=\"reminder\""));
    assert!(normal.contains(r#"<action content="5m" arguments="snooze-5m" activationType="foreground"/>"#));

    let urgent = build_toast_xml("Pay rent", "Now", &[("complete", "Done")], true);
    assert!(urgent.contains(r#"scenario="reminder""#));
}

#[test]
fn on_reminder_payload_reflects_global_constant_override() {
    let mut notices = vec![notice("Standup", 9)];
    assert!(!notices[0].constant);
    apply_constant_override(&mut notices, false);
    assert_eq!(notice_to_json(&notices[0])["constant"], serde_json::json!(false));
    apply_constant_override(&mut notices, true);
    assert!(notices[0].constant);
    assert_eq!(notice_to_json(&notices[0])["constant"], serde_json::json!(true));
}

#[test]
fn idle_poll_skips_checked_at_write_until_interval_or_event() {
    let store = Store::open_in_memory().unwrap();
    let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    let t0 = day.and_hms_opt(9, 0, 0).unwrap();
    store.set_setting(CHECKED_AT, &t0.format(STAMP_FORMAT).to_string()).unwrap();

    // 20 seconds later with no reminders: skips writing CHECKED_AT.
    let t1 = t0 + chrono::Duration::seconds(20);
    let idle_short = poll_once(&store, t0, t0, t1);
    assert!(!idle_short.persisted_checked_at);
    assert_eq!(
        store.setting(CHECKED_AT).unwrap().as_deref(),
        Some("2026-10-18T09:00:00")
    );

    // 9 minutes 59 seconds later: still skips idle write.
    let t2 = t0 + chrono::Duration::seconds(599);
    let idle_just_before = poll_once(&store, t1, t0, t2);
    assert!(!idle_just_before.persisted_checked_at);
    assert_eq!(
        store.setting(CHECKED_AT).unwrap().as_deref(),
        Some("2026-10-18T09:00:00")
    );

    // >= 10 minutes later: persists CHECKED_AT.
    let t3 = t0 + chrono::Duration::minutes(10);
    let idle_after_10m = poll_once(&store, t2, t0, t3);
    assert!(idle_after_10m.persisted_checked_at);
    assert_eq!(
        store.setting(CHECKED_AT).unwrap().as_deref(),
        Some("2026-10-18T09:10:00")
    );

    // Firing notice persists CHECKED_AT immediately even 60 seconds after last_persisted.
    let task = store.add_task("Call bank").unwrap();
    store
        .set_due(
            task.id,
            Some(day),
            Some(chrono::NaiveTime::from_hms_opt(9, 11, 0).unwrap()),
        )
        .unwrap();
    store.set_reminders(task.id, &[0]).unwrap();
    let t4 = t3 + chrono::Duration::seconds(60);
    let with_notice = poll_once(&store, t3, t3, t4);
    assert!(!with_notice.notices.is_empty());
    assert!(with_notice.persisted_checked_at);
    assert_eq!(
        store.setting(CHECKED_AT).unwrap().as_deref(),
        Some("2026-10-18T09:11:00")
    );
}

#[test]
fn habit_repeat_guard_stops_when_checked_in_skipped_archived_or_deleted() {
    let store = Store::open_in_memory().unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();

    let h1 = store.add_habit("Drink water").unwrap();
    assert!(is_target_still_open(&store, Target::Habit(h1.id), today));
    store.toggle_checkin(h1.id, today).unwrap();
    assert!(!is_target_still_open(&store, Target::Habit(h1.id), today));
    // Unchecking re-opens it, skipping closes it.
    store.toggle_checkin(h1.id, today).unwrap();
    assert!(is_target_still_open(&store, Target::Habit(h1.id), today));
    store.skip_habit_date(h1.id, today).unwrap();
    assert!(!is_target_still_open(&store, Target::Habit(h1.id), today));

    let h2 = store.add_habit("Stretch").unwrap();
    assert!(is_target_still_open(&store, Target::Habit(h2.id), today));
    store.set_habit_archived(h2.id, true).unwrap();
    assert!(!is_target_still_open(&store, Target::Habit(h2.id), today));

    let h3 = store.add_habit("Journal").unwrap();
    assert!(is_target_still_open(&store, Target::Habit(h3.id), today));
    store.delete_habit(h3.id).unwrap();
    assert!(!is_target_still_open(&store, Target::Habit(h3.id), today));
}

#[test]
fn reg_sz_encodes_nul_terminated_utf16() {
    let encoded = encode_reg_sz("Fern");
    let (chunks, remainder) = encoded.as_chunks::<2>();
    assert!(remainder.is_empty());
    let units: Vec<u16> = chunks
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect();
    assert_eq!(units, [b'F' as u16, b'e' as u16, b'r' as u16, b'n' as u16, 0]);
}
