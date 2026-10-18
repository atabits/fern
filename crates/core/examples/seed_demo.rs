//! Fills a database with generic demo tasks for UI checks and screenshots.
//! `cargo run -p fern_core --example seed_demo -- /path/to/tasks.db`

use fern_core::{Kind, Priority, Store};
use chrono::{Duration, Local};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: seed_demo <path to database>");
    let store = Store::open(&path)?;
    let today = Local::now().date_naive();
    let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();

    let ideas = store.add_task("Blog post ideas")?;
    store.set_kind(ideas.id, Kind::Note)?;
    store.set_notes(ideas.id, "1. Keyboard shortcuts worth learning\n2. A week without notifications")?;

    let checkup = store.add_task("Book a dental checkup")?;
    store.set_tags(checkup.id, &tags(&["health"]))?;

    let rent = store.add_task("Pay rent")?;
    store.set_tags(rent.id, &tags(&["home"]))?;
    store.set_due(rent.id, Some(today + Duration::days(26)), None)?;

    let garage = store.add_task("Clear out the garage")?;
    store.set_tags(garage.id, &tags(&["home"]))?;
    for title in ["Sort the boxes", "Donate old clothes"] {
        let child = store.add_task(title)?;
        store.set_parent(child.id, Some(garage.id))?;
    }

    let release = store.add_task("Prepare the release notes")?;
    store.set_tags(release.id, &tags(&["work", "writing"]))?;
    store.set_priority(release.id, Priority::Medium)?;

    let plants = store.add_task("Water the plants")?;
    store.set_due(plants.id, Some(today + Duration::days(6)), None)?;

    let trip = store.add_task("Plan the weekend trip")?;
    let tickets = store.add_task("Compare train tickets")?;
    store.set_parent(tickets.id, Some(trip.id))?;

    let backup = store.add_task("Back up the laptop")?;
    store.set_due(backup.id, Some(today), None)?;
    store.set_priority(backup.id, Priority::High)?;

    let library = store.add_task("Return library books")?;
    store.set_due(library.id, Some(today + Duration::days(1)), None)?;

    store.add_task("Read a chapter of a book")?;
    let recycling = store.add_task("Take out the recycling")?;
    store.set_due(recycling.id, Some(today - Duration::days(1)), None)?;

    let done = store.add_task("Renew the gym membership")?;
    store.set_completed(done.id, true)?;

    store.set_tag_color("health", Some("#22C55E"))?;
    store.set_tag_color("home", Some("#3B82F6"))?;
    store.set_tag_color("work", Some("#A855F7"))?;
    store.set_tag_color("writing", Some("#F59E0B"))?;
    Ok(())
}
