use chrono::{NaiveDate, NaiveTime, TimeZone};
use chrono_tz::{Asia::Shanghai, America::New_York, Europe::Moscow, Pacific::Honolulu, UTC};

use super::{Event, Occurrence, parse_with_warnings};

fn d(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn t(hour: u32, minute: u32) -> Option<NaiveTime> {
    NaiveTime::from_hms_opt(hour, minute, 0)
}

fn one(properties: &str) -> Event {
    let parsed = parse_with_warnings(&format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:test\n{properties}\nEND:VEVENT\nEND:VCALENDAR"));
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.events.len(), 1);
    parsed.events.into_iter().next().unwrap()
}

fn starts<T: TimeZone>(event: &Event, from: NaiveDate, to: NaiveDate, zone: &T) -> Vec<(NaiveDate, Option<NaiveTime>)> {
    event.occurrences_in_timezone(from, to, zone).into_iter().map(|at| (at.start, at.start_time)).collect()
}

#[test]
fn new_york_and_utc_user_fixture_show_the_same_moscow_time() {
    let parsed = parse_with_warnings("BEGIN:VCALENDAR
BEGIN:VEVENT
UID:new-york
DTSTART;TZID=America/New_York:20261115T090000
DTEND;TZID=America/New_York:20261115T100000
END:VEVENT
BEGIN:VEVENT
UID:utc
DTSTART:20261115T140000Z
DTEND:20261115T150000Z
END:VEVENT
END:VCALENDAR");
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.events.len(), 2);
    for event in parsed.events {
        assert_eq!(event.occurrences_in_timezone(d(2026, 11, 15), d(2026, 11, 15), &Moscow), [Occurrence {
            start: d(2026, 11, 15), start_time: t(17, 0),
            end: d(2026, 11, 15), end_time: t(18, 0),
        }]);
    }
}

#[test]
fn source_zones_can_shift_the_day_and_year_in_both_directions() {
    let east = one("DTSTART;TZID=Asia/Shanghai:20270101T003000\nDTEND;TZID=Asia/Shanghai:20270101T013000");
    let west = one("DTSTART;TZID=America/Los_Angeles:20261231T233000\nDURATION:PT1H");
    assert_eq!(starts(&east, d(2026, 12, 31), d(2026, 12, 31), &Moscow), [(d(2026, 12, 31), t(19, 30))]);
    assert!(starts(&east, d(2027, 1, 1), d(2027, 1, 1), &Moscow).is_empty());
    assert_eq!(starts(&west, d(2027, 1, 1), d(2027, 1, 1), &Moscow), [(d(2027, 1, 1), t(10, 30))]);
    assert_eq!(starts(&east, d(2026, 12, 31), d(2026, 12, 31), &Honolulu), [(d(2026, 12, 31), t(6, 30))]);
}

#[test]
fn dates_and_floating_times_keep_their_civil_values_in_every_zone() {
    let date = one("DTSTART;VALUE=DATE:20261231\nDTEND;VALUE=DATE:20270102\nRRULE:FREQ=YEARLY;COUNT=2");
    let floating = one("DTSTART:20261231T233000\nDURATION:PT1H");
    for zone in [Moscow, New_York, Shanghai, Honolulu, UTC] {
        assert_eq!(date.occurrences_in_timezone(d(2027, 1, 1), d(2027, 1, 1), &zone), [Occurrence {
            start: d(2026, 12, 31), start_time: None, end: d(2027, 1, 1), end_time: None,
        }]);
        assert_eq!(floating.occurrences_in_timezone(d(2027, 1, 1), d(2027, 1, 1), &zone), [Occurrence {
            start: d(2026, 12, 31), start_time: t(23, 30), end: d(2027, 1, 1), end_time: t(0, 30),
        }]);
    }
    // X-WR-TIMEZONE is calendar metadata, not permission to reinterpret a floating value.
    let parsed = parse_with_warnings("BEGIN:VCALENDAR\nX-WR-TIMEZONE:America/New_York\nBEGIN:VEVENT\nDTSTART:20261115T090000\nEND:VEVENT\nEND:VCALENDAR");
    assert_eq!(starts(&parsed.events[0], d(2026, 11, 15), d(2026, 11, 15), &Moscow), [(d(2026, 11, 15), t(9, 0))]);
}

#[test]
fn weekly_days_are_expanded_before_timezone_conversion() {
    let event = one("DTSTART:20261116T230000Z\nRRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=3");
    assert_eq!(starts(&event, d(2026, 11, 1), d(2026, 12, 5), &Moscow), [
        (d(2026, 11, 17), t(2, 0)), (d(2026, 11, 24), t(2, 0)), (d(2026, 12, 1), t(2, 0)),
    ]);
    let event = one("DTSTART;TZID=Asia/Shanghai:20261116T003000\nRRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=3");
    assert_eq!(starts(&event, d(2026, 11, 1), d(2026, 12, 1), &Moscow), [
        (d(2026, 11, 15), t(19, 30)), (d(2026, 11, 22), t(19, 30)), (d(2026, 11, 29), t(19, 30)),
    ]);
}

#[test]
fn every_occurrence_uses_source_and_destination_dst_rules() {
    let source = one("DTSTART;TZID=America/New_York:20261025T090000\nDURATION:PT1H\nRRULE:FREQ=WEEKLY;COUNT=3");
    assert_eq!(starts(&source, d(2026, 10, 1), d(2026, 11, 30), &Moscow), [
        (d(2026, 10, 25), t(16, 0)), (d(2026, 11, 1), t(17, 0)), (d(2026, 11, 8), t(17, 0)),
    ]);
    let utc = one("DTSTART:20261025T090000Z\nDURATION:PT1H\nRRULE:FREQ=WEEKLY;COUNT=3");
    assert_eq!(starts(&utc, d(2026, 10, 1), d(2026, 11, 30), &New_York), [
        (d(2026, 10, 25), t(5, 0)), (d(2026, 11, 1), t(4, 0)), (d(2026, 11, 8), t(4, 0)),
    ]);
}

#[test]
fn until_compares_the_exact_instant_inclusively() {
    let before = one("DTSTART:20261117T070000Z\nRRULE:FREQ=DAILY;UNTIL=20261118T065959Z");
    let inclusive = one("DTSTART;TZID=Asia/Shanghai:20261117T150000\nRRULE:FREQ=DAILY;UNTIL=20261118T070000Z");
    assert_eq!(starts(&before, d(2026, 11, 1), d(2026, 11, 30), &Moscow), [(d(2026, 11, 17), t(10, 0))]);
    assert_eq!(starts(&inclusive, d(2026, 11, 1), d(2026, 11, 30), &Moscow), [
        (d(2026, 11, 17), t(10, 0)), (d(2026, 11, 18), t(10, 0)),
    ]);
}

#[test]
fn exclusions_match_the_full_original_start_and_keep_count_semantics() {
    let different_time = one("DTSTART:20261117T090000Z\nRRULE:FREQ=DAILY;COUNT=2\nEXDATE:20261117T100000Z");
    assert_eq!(starts(&different_time, d(2026, 11, 1), d(2026, 11, 30), &Moscow).len(), 2);
    let excluded = one("DTSTART;TZID=America/New_York:20261115T090000\nRRULE:FREQ=WEEKLY;COUNT=3\nEXDATE:20261122T140000Z");
    assert_eq!(starts(&excluded, d(2026, 11, 1), d(2026, 12, 30), &Moscow), [
        (d(2026, 11, 15), t(17, 0)), (d(2026, 11, 29), t(17, 0)),
    ]);
}

#[test]
fn moved_and_cancelled_instances_replace_the_original_instant_across_zones() {
    let parsed = parse_with_warnings("BEGIN:VCALENDAR
BEGIN:VEVENT
UID:series
DTSTART;TZID=America/New_York:20261115T090000
RRULE:FREQ=WEEKLY;COUNT=3
END:VEVENT
BEGIN:VEVENT
UID:series
RECURRENCE-ID:20261122T140000Z
DTSTART;TZID=Asia/Shanghai:20261123T233000
DURATION:PT1H
END:VEVENT
BEGIN:VEVENT
UID:series
RECURRENCE-ID;TZID=America/New_York:20261129T090000
STATUS:CANCELLED
END:VEVENT
END:VCALENDAR");
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.events.len(), 2);
    assert_eq!(starts(&parsed.events[0], d(2026, 11, 1), d(2026, 12, 1), &Moscow), [(d(2026, 11, 15), t(17, 0))]);
    assert_eq!(starts(&parsed.events[1], d(2026, 11, 1), d(2026, 12, 1), &Moscow), [(d(2026, 11, 23), t(18, 30))]);
}

#[test]
fn explicit_gaps_use_the_previous_offset_but_generated_gaps_do_not_count() {
    let explicit = one("DTSTART;TZID=America/New_York:20260308T023000");
    assert_eq!(starts(&explicit, d(2026, 3, 8), d(2026, 3, 8), &UTC), [(d(2026, 3, 8), t(7, 30))]);
    let repeated = one("DTSTART;TZID=America/New_York:20260301T023000\nRRULE:FREQ=WEEKLY;COUNT=3");
    assert_eq!(starts(&repeated, d(2026, 3, 1), d(2026, 3, 31), &UTC), [
        (d(2026, 3, 1), t(7, 30)), (d(2026, 3, 15), t(6, 30)), (d(2026, 3, 22), t(6, 30)),
    ]);
    let fold = one("DTSTART;TZID=America/New_York:20261101T013000\nRRULE:FREQ=WEEKLY;COUNT=2");
    assert_eq!(starts(&fold, d(2026, 11, 1), d(2026, 11, 30), &UTC), [
        (d(2026, 11, 1), t(5, 30)), (d(2026, 11, 8), t(6, 30)),
    ]);
}

#[test]
fn dtend_and_duration_keep_exact_and_nominal_dst_durations_separate() {
    let properties = "DTSTART;TZID=America/New_York:20260307T120000\nRRULE:FREQ=DAILY;COUNT=2";
    let nominal = one(&format!("{properties}\nDURATION:P1D"));
    let exact = one(&format!("{properties}\nDURATION:PT24H"));
    let end = one(&format!("{properties}\nDTEND;TZID=America/New_York:20260308T120000"));
    let range = |event: &Event| event.occurrences_in_timezone(d(2026, 3, 7), d(2026, 3, 9), &New_York);
    let nominal = range(&nominal);
    let exact = range(&exact);
    let end = range(&end);
    assert_eq!((nominal[0].end_time, nominal[1].end_time), (t(12, 0), t(12, 0)));
    assert_eq!((exact[0].end_time, exact[1].end_time), (t(13, 0), t(12, 0)));
    assert_eq!((end[0].end_time, end[1].end_time), (t(12, 0), t(11, 0)));
}

#[test]
fn midnight_is_exclusive_after_converting_both_endpoints() {
    let event = one("DTSTART;TZID=America/New_York:20261115T233000\nDTEND;TZID=America/New_York:20261116T000000");
    assert_eq!(event.occurrences_in_timezone(d(2026, 11, 16), d(2026, 11, 16), &Moscow), [Occurrence {
        start: d(2026, 11, 16), start_time: t(7, 30), end: d(2026, 11, 16), end_time: t(8, 0),
    }]);
    let local = event.occurrences_in_timezone(d(2026, 11, 15), d(2026, 11, 15), &New_York);
    assert_eq!((local[0].end, local[0].end_time), (d(2026, 11, 15), t(23, 59)));
    assert!(event.occurrences_in_timezone(d(2026, 11, 16), d(2026, 11, 16), &New_York).is_empty());
}

#[test]
fn quoted_iana_aliases_work_and_unknown_zones_are_reported() {
    let parsed = parse_with_warnings("BEGIN:VCALENDAR
BEGIN:VTIMEZONE
TZID:urn:vendor:office
X-LIC-LOCATION:America/New_York
END:VTIMEZONE
BEGIN:VEVENT
DTSTART;VALUE=DATE-TIME;TZID=\"urn:vendor:office\":20261115T090000
END:VEVENT
BEGIN:VEVENT
DTSTART;TZID=\"America/New_York\":20261115T090000
END:VEVENT
BEGIN:VEVENT
DTSTART;TZID=Unknown/Zone:20261115T090000
END:VEVENT
END:VCALENDAR");
    assert_eq!(parsed.events.len(), 2);
    assert_eq!(parsed.warnings, ["Unsupported calendar time zone: Unknown/Zone"]);
    for event in parsed.events {
        assert_eq!(starts(&event, d(2026, 11, 15), d(2026, 11, 15), &Moscow), [(d(2026, 11, 15), t(17, 0))]);
    }
}
