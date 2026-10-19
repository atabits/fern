//! Times the store's reads that the views make, on a large made-up database: a few thousand
//! tasks with tags and checklists, habits with a year of check-ins and focus records.
//! `cargo run --release -p fern_core --example bench_store -- [tasks] [path to keep the database at]`

use std::time::Instant;

use chrono::{Duration, Local};
use fern_core::{ListDraft, Store, View};

fn main() -> anyhow::Result<()> {
    let count: usize = std::env::args().nth(1).and_then(|n| n.parse().ok()).unwrap_or(5000);
    let keep = std::env::args().nth(2).map(std::path::PathBuf::from);
    let path = keep.clone().unwrap_or_else(|| std::env::temp_dir().join(format!("fern-bench-{}.db", std::process::id())));
    let store = Store::open(&path)?;
    let today = Local::now().date_naive();

    let start = Instant::now();
    (|| -> anyhow::Result<()> {
        let lists: Vec<_> = (0..20)
            .map(|i| store.add_list(&ListDraft { name: format!("List {i}"), ..Default::default() }))
            .collect::<anyhow::Result<_>>()?;
        for i in 0..count {
            let task = store.add_task(&format!("Task number {i}"))?;
            store.set_list(task.id, Some(lists[i % lists.len()].id))?;
            store.set_due(task.id, Some(today + Duration::days((i % 60) as i64 - 30)), None)?;
            store.set_tags(task.id, &[format!("tag{}", i % 15)])?;
            if i % 5 == 0 {
                for j in 0..4 {
                    store.add_checklist_item(task.id, &format!("Step {j}"), None)?;
                }
            }
            if i % 3 == 0 {
                store.set_completed(task.id, true)?;
            }
        }
        for h in 0..10 {
            let habit = store.add_habit(&format!("Habit {h}"))?;
            for d in 0..365 {
                if d % 3 != 0 {
                    store.toggle_checkin(habit.id, today - Duration::days(d))?;
                }
            }
        }
        for s in 0..2000 {
            let at = (Local::now() - Duration::hours(s * 3)).timestamp_millis();
            store.add_focus_session(None, "Focus", at, at + 25 * 60_000)?;
        }
        Ok(())
    })()?;
    println!("seeded {count} tasks in {:?}", start.elapsed());

    fn time<T>(name: &str, f: impl Fn() -> anyhow::Result<T>) -> anyhow::Result<()> {
        f()?;
        let runs = 20;
        let start = Instant::now();
        for _ in 0..runs {
            f()?;
        }
        println!("{name:>24}: {:>8.2?}", start.elapsed() / runs);
        Ok(())
    }
    time("tasks", || store.tasks())?;
    time("tasks_in Today", || store.tasks_in(&View::Today, today))?;
    time("tasks_in Next 7 Days", || store.tasks_in(&View::Next7Days, today))?;
    time("tasks_in All", || store.tasks_in(&View::All, today))?;
    time("lists", || store.lists())?;
    time("tags", || store.tags())?;
    time("habits", || store.habits())?;
    let habit = store.habits()?[0].id;
    time("checkins", || store.checkins(habit))?;
    time("focus_sessions", || store.focus_sessions())?;
    time("setting", || store.setting("x"))?;
    drop(store);
    if keep.is_some() {
        return Ok(());
    }
    for suffix in ["", "-wal", "-shm"] {
        std::fs::remove_file(format!("{}{suffix}", path.display())).ok();
    }
    Ok(())
}
