//! Fills a database with a lived-in demo for the README's screenshots: lists with emoji,
//! tags, a week of timed and all-day tasks, habits with a history, focus records and
//! countdowns. Everything is made up.
//! `cargo run -p fern_core --example seed_showcase -- /path/to/tasks.db`

use chrono::{Datelike, Duration, Local, NaiveTime};
use fern_core::countdown::CountdownKind;
use fern_core::{Kind, ListDraft, Priority, Recurrence, Store};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: seed_showcase <path to database>");
    let store = Store::open(&path)?;
    let today = Local::now().date_naive();
    let at = |h: u32, m: u32| NaiveTime::from_hms_opt(h, m, 0);
    let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();

    let personal = store.add_list(&ListDraft { name: "Personal".into(), icon: Some("🏡".into()), color: Some("#22C55E".into()), ..Default::default() })?;
    let work = store.add_list(&ListDraft { name: "Work".into(), icon: Some("💼".into()), color: Some("#6366F1".into()), ..Default::default() })?;
    let reading = store.add_list(&ListDraft { name: "Reading list".into(), icon: Some("📚".into()), color: Some("#F59E0B".into()), ..Default::default() })?;
    let trip = store.add_list(&ListDraft { name: "Lisbon trip".into(), icon: Some("✈️".into()), color: Some("#06B6D4".into()), ..Default::default() })?;

    let task = |title: &str, list: Option<fern_core::Id>| -> anyhow::Result<fern_core::Task> {
        let task = store.add_task(title)?;
        store.set_list(task.id, list)?;
        Ok(task)
    };

    // Inbox
    let t = task("Call the bank about the new card", None)?;
    store.set_due(t.id, Some(today), at(10, 30))?;
    store.set_priority(t.id, Priority::High)?;
    let t = task("Pick up the dry cleaning", None)?;
    store.set_due(t.id, Some(today + Duration::days(1)), None)?;
    let t = task("Renew the passport", None)?;
    store.set_due(t.id, Some(today + Duration::days(12)), None)?;
    store.set_tags(t.id, &tags(&["errands"]))?;
    let t = task("Ideas for the garden", None)?;
    store.set_kind(t.id, Kind::Note)?;
    store.set_notes(t.id, "1. Herbs by the kitchen window\n2. **Tomatoes** along the fence\n3. A bench under the old apple tree")?;

    // Work
    let t = task("Prepare the quarterly review", Some(work.id))?;
    store.set_due(t.id, Some(today), at(15, 0))?;
    store.set_due_end(t.id, Some((today, at(16, 30))))?;
    store.set_priority(t.id, Priority::High)?;
    store.set_tags(t.id, &tags(&["meeting"]))?;
    store.set_notes(t.id, "Keep it to **ten minutes**: where we are, what changed and what's next.\n\n- Revenue and churn\n- The new onboarding\n- Hiring plan")?;
    for (item, done) in [("Collect the numbers", true), ("Draft the slides", true), ("Rehearse", false)] {
        let item = store.add_checklist_item(t.id, item, None)?;
        store.set_checklist_item_completed(item.id, done)?;
    }
    let t = task("Review the design proposal", Some(work.id))?;
    store.set_due(t.id, Some(today + Duration::days(1)), at(11, 0))?;
    store.set_tags(t.id, &tags(&["design"]))?;
    let t = task("1:1 with Maya", Some(work.id))?;
    store.set_due(t.id, Some(today + Duration::days(2)), at(14, 0))?;
    store.set_repeat(t.id, Some(Recurrence::new(fern_core::Frequency::Weekly, 1)), today)?;
    store.set_tags(t.id, &tags(&["meeting"]))?;
    let t = task("Write the release notes", Some(work.id))?;
    store.set_due(t.id, Some(today + Duration::days(3)), None)?;
    store.set_priority(t.id, Priority::Medium)?;
    store.set_tags(t.id, &tags(&["writing"]))?;
    let t = task("Update the onboarding guide", Some(work.id))?;
    store.set_tags(t.id, &tags(&["writing"]))?;
    let done = task("Send the invoice", Some(work.id))?;
    let board = |name: &str| store.add_list_section(Some(work.id), name);
    let (todo, doing) = (board("To do")?, board("In progress")?);
    for (needle, section) in [("onboarding", &todo), ("release notes", &todo), ("quarterly", &doing), ("design proposal", &doing), ("Maya", &doing)] {
        if let Some(t) = store.tasks()?.into_iter().find(|t| t.title.contains(needle)) {
            store.set_section(t.id, Some(section.id))?;
        }
    }
    store.set_completed(done.id, true)?;

    // Personal
    let t = task("Morning run in the park", Some(personal.id))?;
    store.set_due(t.id, Some(today), at(8, 0))?;
    store.set_due_end(t.id, Some((today, at(9, 0))))?;
    store.set_tags(t.id, &tags(&["health"]))?;
    let t = task("Book a dentist appointment", Some(personal.id))?;
    store.set_due(t.id, Some(today - Duration::days(1)), None)?;
    store.set_tags(t.id, &tags(&["health"]))?;
    let t = task("Dinner with Sam and Alex", Some(personal.id))?;
    store.set_due(t.id, Some(today + Duration::days(4)), at(19, 30))?;
    let t = task("Water the plants", Some(personal.id))?;
    store.set_due(t.id, Some(today + Duration::days(2)), None)?;
    store.set_repeat(t.id, Some(Recurrence::new(fern_core::Frequency::Daily, 3)), today)?;
    let t = task("Fix the bike's brakes", Some(personal.id))?;
    store.set_priority(t.id, Priority::Low)?;
    for title in ["Buy brake pads", "Watch a repair video"] {
        let child = store.add_task(title)?;
        store.set_list(child.id, Some(personal.id))?;
        store.set_parent(child.id, Some(t.id))?;
    }

    // Reading list
    for (title, done) in [("The Pragmatic Programmer", true), ("Designing Data-Intensive Applications", false), ("A Philosophy of Software Design", false)] {
        let t = task(title, Some(reading.id))?;
        if done {
            store.set_completed(t.id, true)?;
        }
    }

    // Lisbon trip
    let t = task("Book the flights", Some(trip.id))?;
    store.set_due(t.id, Some(today + Duration::days(5)), None)?;
    store.set_priority(t.id, Priority::High)?;
    let t = task("Find a place to stay in Alfama", Some(trip.id))?;
    store.set_due(t.id, Some(today + Duration::days(6)), None)?;
    let t = task("Make a list of cafés", Some(trip.id))?;
    store.set_tags(t.id, &tags(&["food"]))?;

    // A week of meetings and plans around today, so the calendar is busy; the past ones are done.
    let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    for (day, start, end, title, list, tag) in [
        (0, (9, 30), (10, 30), "Team planning", work.id, "meeting"),
        (0, (13, 0), (14, 0), "Lunch with Jordan", personal.id, ""),
        (1, (10, 0), (12, 0), "Design workshop", work.id, "design"),
        (1, (18, 0), (19, 0), "Yoga class", personal.id, "health"),
        (2, (9, 0), (9, 30), "Stand-up", work.id, "meeting"),
        (2, (14, 0), (16, 0), "Write the blog post", work.id, "writing"),
        (3, (11, 0), (12, 0), "Interview a candidate", work.id, "meeting"),
        (3, (16, 0), (17, 0), "Groceries for the week", personal.id, "errands"),
        (4, (10, 0), (11, 30), "Product demo", work.id, "meeting"),
        (4, (19, 0), (21, 0), "Movie night", personal.id, ""),
        (5, (10, 0), (12, 0), "Farmers market", personal.id, "food"),
        (6, (11, 0), (12, 0), "Call Mom", personal.id, ""),
        (6, (17, 0), (18, 0), "Plan next week", personal.id, ""),
    ] {
        let date = monday + Duration::days(day);
        let t = task(title, Some(list))?;
        store.set_due(t.id, Some(date), at(start.0, start.1))?;
        store.set_due_end(t.id, Some((date, at(end.0, end.1))))?;
        if !tag.is_empty() {
            store.set_tags(t.id, &tags(&[tag]))?;
        }
        if date < today {
            store.set_completed(t.id, true)?;
        }
    }

    for (tag, color) in [("health", "#22C55E"), ("meeting", "#6366F1"), ("design", "#EC4899"), ("writing", "#F59E0B"), ("errands", "#0EA5E9"), ("food", "#EF4444")] {
        store.set_tag_color(tag, Some(color))?;
    }

    // Habits with a few weeks of check-ins.
    for (name, icon, color, skip) in [("Drink water", "💧", "#38BDF8", 5), ("Read 20 pages", "📖", "#F59E0B", 3), ("Meditate", "🧘", "#A78BFA", 4), ("Walk 8,000 steps", "👟", "#34D399", 6)] {
        let habit = store.add_habit(name)?;
        store.set_habit_look(habit.id, Some(icon), Some(color))?;
        for ago in 0..28 {
            if ago % skip != 0 || (ago == 0 && skip % 2 == 1) {
                store.toggle_checkin(habit.id, today - Duration::days(ago))?;
            }
        }
    }

    // Focus records over the past week.
    for (ago, hour, minutes, title) in [(0, 9, 50, "Prepare the quarterly review"), (0, 14, 25, "Write the release notes"), (1, 10, 75, "Review the design proposal"), (2, 9, 50, "Update the onboarding guide"), (3, 16, 25, "Morning run in the park"), (5, 10, 50, "Book the flights"), (4, 11, 100, "Prepare the quarterly review"), (6, 15, 50, "Write the release notes")] {
        let day = today - Duration::days(ago);
        let start = day.and_hms_opt(hour, 0, 0).and_then(|t| t.and_local_timezone(Local).single()).expect("a valid local time");
        let start = start.timestamp_millis();
        let task_id = store.tasks()?.into_iter().find(|t| t.title == title).map(|t| t.id);
        store.add_focus_session_as(task_id, title, start, start + minutes * 60_000, false)?;
    }

    // Countdowns.
    let trip_day = store.add_countdown("Lisbon", today + Duration::days(18), CountdownKind::Countdown)?;
    store.set_countdown_look(trip_day.id, Some("✈️"), Some("#06B6D4"))?;
    let new_year = chrono::NaiveDate::from_ymd_opt(today.year() + 1, 1, 1).expect("New Year's Day");
    let holiday = store.add_countdown("New Year", new_year, CountdownKind::Holiday)?;
    store.set_countdown_look(holiday.id, Some("🎆"), Some("#F43F5E"))?;
    let together = store.add_countdown("Together", today - Duration::days(1_204), CountdownKind::Anniversary)?;
    store.set_countdown_look(together.id, Some("💛"), Some("#F59E0B"))?;
    let birthday = store.add_countdown("Sam's birthday", (today + Duration::days(47)).with_year(today.year() - 30).unwrap_or(today), CountdownKind::Birthday)?;
    store.set_countdown_look(birthday.id, Some("🎂"), Some("#EC4899"))?;
    let marathon = store.add_countdown("Half marathon", today + Duration::days(96), CountdownKind::Countdown)?;
    store.set_countdown_look(marathon.id, Some("🏃"), Some("#22C55E"))?;
    Ok(())
}
