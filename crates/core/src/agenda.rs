//! The calendar's Agenda: the days ahead that have something due, with repeating tasks shown on
//! each of their coming dates, as in the original, and countdowns on theirs.

use chrono::{Datelike, NaiveDate, NaiveTime};

use crate::countdown::{Countdown, CountdownKind};
use crate::model::{Id, Priority, Task};

/// One entry of a day in the agenda.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgendaItem {
    pub task: Id,
    pub title: String,
    pub time: Option<NaiveTime>,
    pub priority: Priority,
    /// A repeating task: its first date is the task itself, later ones are the dates to come.
    pub repeats: bool,
    /// A countdown rather than a task; `task` is then the countdown's id.
    pub countdown: bool,
}

/// The days from `from` through `until` with open tasks due or countdowns, in order; on a day,
/// all-day tasks come first, then countdowns, then timed tasks by time. Completed, Won't Do and
/// deleted tasks are left out. Anniversaries and holidays come back every year; a countdown is
/// listed on its date only.
pub fn agenda(
    tasks: &[Task],
    countdowns: &[Countdown],
    from: NaiveDate,
    until: NaiveDate,
) -> Vec<(NaiveDate, Vec<AgendaItem>)> {
    let mut entries: Vec<(NaiveDate, AgendaItem)> = Vec::new();
    for task in tasks.iter().filter(|t| !t.is_closed() && !t.is_deleted()) {
        let Some(mut date) = task.due_date else { continue };
        let item = AgendaItem {
            task: task.id,
            title: task.title.clone(),
            time: task.due_time,
            priority: task.priority,
            repeats: task.repeat.is_some(),
            countdown: false,
        };
        loop {
            if date > until {
                break;
            }
            if date >= from {
                entries.push((date, item.clone()));
            }
            match &task.repeat {
                Some(repeat) => {
                    let next = repeat.next_after(date);
                    if next <= date {
                        break;
                    }
                    date = next;
                }
                None => break,
            }
        }
    }
    for countdown in countdowns {
        let item = AgendaItem {
            task: countdown.id,
            title: countdown.name.clone(),
            time: None,
            priority: Priority::None,
            repeats: false,
            countdown: true,
        };
        let yearly = countdown.kind != CountdownKind::Countdown;
        let years = if yearly { from.year().max(countdown.date.year())..=until.year() } else { countdown.date.year()..=countdown.date.year() };
        for year in years {
            // Feb 29 falls on Feb 28 in other years.
            let Some(date) = countdown.date.with_year(year).or_else(|| NaiveDate::from_ymd_opt(year, 2, 28)) else { continue };
            if date >= from && date <= until {
                entries.push((date, item.clone()));
            }
        }
    }
    entries.sort_by(|(a, x), (b, y)| a.cmp(b).then(x.time.is_some().cmp(&y.time.is_some())).then(x.time.cmp(&y.time)));
    let mut days: Vec<(NaiveDate, Vec<AgendaItem>)> = Vec::new();
    for (date, item) in entries {
        match days.last_mut() {
            Some((day, items)) if *day == date => items.push(item),
            _ => days.push((date, vec![item])),
        }
    }
    days
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recurrence::{Frequency, Recurrence};
    use crate::store::Store;

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).unwrap()
    }

    #[test]
    fn days_ahead_with_repeats() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let rent = store.add_task("Pay rent")?;
        store.set_due(rent.id, Some(d(10, 13)), None)?;
        store.set_repeat(rent.id, Some(Recurrence::new(Frequency::Monthly, 1)), d(10, 18))?;
        let call = store.add_task("Call")?;
        store.set_due(call.id, Some(d(10, 20)), NaiveTime::from_hms_opt(15, 0, 0))?;
        let plants = store.add_task("Water the plants")?;
        store.set_due(plants.id, Some(d(10, 20)), None)?;
        let done = store.add_task("Done already")?;
        store.set_due(done.id, Some(d(10, 21)), None)?;
        store.set_completed(done.id, true)?;
        let past = store.add_task("Overdue")?;
        store.set_due(past.id, Some(d(10, 1)), None)?;

        let days = agenda(&store.tasks()?, &[], d(10, 18), d(12, 31));
        let dates: Vec<NaiveDate> = days.iter().map(|(date, _)| *date).collect();
        assert_eq!(dates, [d(10, 20), d(11, 13), d(12, 13)], "past and completed tasks are left out");
        let titles: Vec<&str> = days[0].1.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Water the plants", "Call"], "all-day first, then by time");
        assert!(days[1].1[0].repeats);
        Ok(())
    }

    #[test]
    fn countdowns_on_their_dates() {
        let countdown = |name: &str, date, kind| Countdown { id: uuid::Uuid::now_v7(), name: name.into(), date, kind };
        let countdowns = [
            countdown("Release", d(11, 19), CountdownKind::Countdown),
            countdown("New Year", NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), CountdownKind::Holiday),
            countdown("Long gone", d(1, 5), CountdownKind::Countdown),
        ];
        let until = NaiveDate::from_ymd_opt(2027, 10, 18).unwrap();
        let days = agenda(&[], &countdowns, d(10, 18), until);
        let listed: Vec<(NaiveDate, &str)> = days.iter().map(|(date, items)| (*date, items[0].title.as_str())).collect();
        assert_eq!(listed, [(d(11, 19), "Release"), (NaiveDate::from_ymd_opt(2027, 1, 1).unwrap(), "New Year")]);
        assert!(days[0].1[0].countdown);
    }
}
