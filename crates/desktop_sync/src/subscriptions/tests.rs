use super::*;

const FEED: &str = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:new-year\nSUMMARY:New Year\nDTSTART;VALUE=DATE:20260101\nEND:VEVENT\nEND:VCALENDAR";

#[test]
fn subscribed_tzid_and_utc_events_keep_the_same_time_after_reload() -> Result<()> {
    // The reported Windows/Moscow case, fetched through the same path as a URL subscription.
    let feed = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
        "BEGIN:VEVENT\r\nUID:NY\r\nSUMMARY:NY\r\n",
        "DTSTART;TZID=America/New_York:20261115T090000\r\n",
        "DTEND;TZID=America/New_York:20261115T100000\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:UTC\r\nSUMMARY:UTC\r\n",
        "DTSTART:20261115T140000Z\r\nDTEND:20261115T150000Z\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n",
    );
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}/calendar.ics", server.server_addr());
    let worker = std::thread::spawn(move || {
        let request = server.recv_timeout(Duration::from_secs(10)).unwrap().expect("feed requested");
        request.respond(tiny_http::Response::from_string(feed)).unwrap();
    });
    let fetched = fetch(&url);
    worker.join().unwrap();
    let fetched = fetched?;
    let store = Rc::new(Store::open_in_memory()?);
    let id = store.add_subscription("Synthetic calendar", &url, None)?;
    store.save_subscription_feed(id, &url, Ok(&fetched))?;
    let mut feeds = Feeds::new(store.clone());
    let day = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
    let moscow = chrono::FixedOffset::east_opt(3 * 3600).unwrap();
    for offline in [false, true] {
        if offline {
            store.save_subscription_feed(id, &url, Err("offline"))?;
            feeds.load();
        }
        let occurrences = feeds.occurrences_in_timezone(day, day, &moscow);
        assert_eq!(occurrences.len(), 2);
        assert_eq!(occurrences[0].times, occurrences[1].times);
        for occurrence in occurrences {
            assert_eq!((occurrence.times.start, occurrence.times.end), (day, day));
            assert_eq!(occurrence.times.start_time, chrono::NaiveTime::from_hms_opt(17, 0, 0));
            assert_eq!(occurrence.times.end_time, chrono::NaiveTime::from_hms_opt(18, 0, 0));
        }
        assert_eq!(store.subscriptions()?[0].feed, feed, "source data must remain intact");
    }
    // A new display timezone must not require fetching or reparsing the feed.
    let utc = feeds.occurrences_in_timezone(day, day, &chrono::Utc);
    assert_eq!(utc[0].times.start_time, chrono::NaiveTime::from_hms_opt(14, 0, 0));
    assert_eq!(utc[0].times, utc[1].times);
    Ok(())
}

#[test]
fn unsupported_timezone_is_reported_without_discarding_valid_events() -> Result<()> {
    let unknown = FEED.replace("END:VCALENDAR", concat!(
        "BEGIN:VEVENT\nUID:unknown\nSUMMARY:Unsupported zone\n",
        "DTSTART;TZID=Unknown/Timezone:20260101T090000\nEND:VEVENT\nEND:VCALENDAR",
    ));
    let store = Rc::new(Store::open_in_memory()?);
    let url = "https://example.com/synthetic.ics";
    let id = store.add_subscription_with_feed("Mixed calendar", url, None, &unknown)?;
    let mut feeds = Feeds::new(store.clone());
    let calendar = &feeds.calendars()[0];
    assert_eq!(calendar.events.len(), 1);
    assert!(calendar.subscription.error.as_ref().unwrap().contains("Unknown/Timezone"));
    assert!(store.subscriptions()?[0].error.is_none(), "parse warnings are separate from fetch errors");
    assert_eq!(store.subscriptions()?[0].feed, unknown);

    store.save_subscription_feed(id, url, Err("offline"))?;
    feeds.load();
    let error = feeds.calendars()[0].subscription.error.as_ref().unwrap();
    assert!(error.contains("offline") && error.contains("Unknown/Timezone"));

    store.save_subscription_feed(id, url, Ok(FEED))?;
    feeds.load();
    assert!(feeds.calendars()[0].subscription.error.is_none());
    assert_eq!(feeds.calendars()[0].events.len(), 1);
    Ok(())
}

#[test]
fn holiday_source_changes_only_after_a_successful_read() -> Result<()> {
    let store = Rc::new(Store::open_in_memory()?);
    let google = ChinaHolidaySource::Google;
    let apple = ChinaHolidaySource::Apple;
    let id = store.add_subscription_with_feed("My holidays", google.url(), Some("#22C55E"), FEED)?;
    store.update_subscription(id, "My holidays", google.url(), Some("#22C55E"), true)?;
    let mut feeds = Feeds::new(store.clone());
    let request = Id::now_v7();
    feeds.reading.insert(id, request);
    let replacement = FEED.replace("20260101", "20270101");

    feeds.finish_china_holiday_change(
        Some((id, google.url())),
        apple,
        "Holidays",
        Err(anyhow::anyhow!("feed unavailable")),
    );
    assert_eq!(feeds.china_holiday_status(), ChinaHolidayStatus::Failed);
    assert!(feeds.is_reading(id));
    assert_eq!(feeds.calendars()[0].subscription.url, google.url());
    assert_eq!(feeds.calendars()[0].subscription.feed, FEED);

    feeds.finish_china_holiday_change(Some((id, google.url())), apple, "Holidays", Ok(replacement.clone()));
    assert_eq!(feeds.china_holiday_status(), ChinaHolidayStatus::Idle);
    assert!(!feeds.is_reading(id));
    assert_eq!(feeds.calendars().len(), 1);
    let changed = &feeds.calendars()[0].subscription;
    assert_eq!(changed.id, id);
    assert_eq!(changed.name, "My holidays");
    assert_eq!(changed.color.as_deref(), Some("#22C55E"));
    assert!(changed.hidden);
    assert_eq!(changed.url, apple.url());
    assert_eq!(changed.feed, replacement);
    assert_eq!(feeds.calendars()[0].events.len(), 1);

    // A refresh started before the switch may finish after it.
    assert!(!store.save_subscription_feed(id, google.url(), Ok(FEED))?);
    assert_eq!(store.subscriptions()?[0].feed, replacement);
    Ok(())
}

#[test]
fn a_second_holiday_source_cannot_be_added_after_one_is_selected() -> Result<()> {
    let store = Rc::new(Store::open_in_memory()?);
    store.add_subscription("Personal", "https://example.com/private.ics", None)?;
    let mut feeds = Feeds::new(store);

    feeds.finish_china_holiday_change(None, ChinaHolidaySource::Apple, "Holidays", Ok(FEED.to_owned()));
    feeds.finish_china_holiday_change(None, ChinaHolidaySource::Google, "Holidays", Ok(FEED.to_owned()));

    assert_eq!(feeds.calendars().len(), 2);
    assert_eq!(feeds.calendars()[0].subscription.name, "Personal");
    assert_eq!(feeds.calendars()[1].subscription.url, ChinaHolidaySource::Apple.url());
    Ok(())
}

#[test]
fn caldav_two_way_sync_pulls_puts_and_deletes_events() -> Result<()> {
    use std::sync::{Arc, Mutex};

    let store = Rc::new(Store::open_in_memory()?);
    let events_state: Arc<Mutex<HashMap<String, (String, String)>>> = Arc::new(Mutex::new(HashMap::from([(
        "/dav/cal/remote-1.ics".to_owned(),
        (
            "\"e-1\"".to_owned(),
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:remote-1\r\nSUMMARY:Remote Event\r\nDTSTART:20261110T090000\r\nDTEND:20261110T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".to_owned(),
        ),
    )])));
    let auth_seen = Arc::new(Mutex::new(Vec::<String>::new()));

    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_ip().unwrap();
    let base_url = format!("http://{addr}");

    let srv_state = Arc::clone(&events_state);
    let srv_auth = Arc::clone(&auth_seen);
    let handle = std::thread::spawn(move || {
        for mut req in server.incoming_requests().take(6) {
            if let Some(auth) = req.headers().iter().find(|h| h.field.equiv("Authorization")) {
                srv_auth.lock().unwrap().push(auth.value.to_string());
            }
            let path = req.url().to_owned();
            let method = req.method().as_str().to_owned();
            let mut body = String::new();
            let _ = req.as_reader().read_to_string(&mut body);

            match (method.as_str(), path.as_str()) {
                ("PROPFIND", "/dav/") => {
                    let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:response><d:href>/dav/</d:href><d:propstat><d:prop><c:calendar-home-set><d:href>/dav/home/</d:href></c:calendar-home-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
                    let resp = tiny_http::Response::from_string(xml).with_status_code(207);
                    let _ = req.respond(resp);
                }
                ("PROPFIND", "/dav/home/") => {
                    let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:response><d:href>/dav/cal/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>Team</d:displayname></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
                    let resp = tiny_http::Response::from_string(xml).with_status_code(207);
                    let _ = req.respond(resp);
                }
                ("REPORT", "/dav/cal/") => {
                    let guard = srv_state.lock().unwrap();
                    let mut xml = String::from(r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">"#);
                    for (href, (etag, ics)) in guard.iter() {
                        xml.push_str(&format!(
                            "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:getetag>{etag}</d:getetag><c:calendar-data><![CDATA[{ics}]]></c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
                        ));
                    }
                    xml.push_str("</d:multistatus>");
                    let _ = req.respond(tiny_http::Response::from_string(xml).with_status_code(207));
                }
                ("PUT", p) => {
                    srv_state.lock().unwrap().insert(p.to_owned(), ("\"e-2\"".to_owned(), body));
                    let resp = tiny_http::Response::from_string("")
                        .with_status_code(201)
                        .with_header(tiny_http::Header::from_bytes(&b"ETag"[..], &b"\"e-2\""[..]).unwrap());
                    let _ = req.respond(resp);
                }
                ("DELETE", p) => {
                    srv_state.lock().unwrap().remove(p);
                    let _ = req.respond(tiny_http::Response::from_string("").with_status_code(204));
                }
                _ => {
                    let _ = req.respond(tiny_http::Response::from_string("").with_status_code(404));
                }
            }
        }
    });

    let discovered = discover_caldav_calendars(&format!("{base_url}/dav/"), "alice", "secret")?;
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].name, "Team");
    assert!(discovered[0].writable);

    let sub_id = store.add_caldav_subscription(
        &discovered[0].name,
        &discovered[0].url,
        Some("#3B82F6"),
        CalendarProvider::CalDav,
        "alice",
        "secret",
        true,
    )?;
    let sub = store.subscriptions()?.into_iter().find(|s| s.id == sub_id).unwrap();

    // 1. Initial pull fetches remote-1.
    let out1 = sync_caldav_remote(&sub, &[]);
    assert!(out1.errors.is_empty(), "{:?}", out1.errors);
    store.replace_caldav_snapshot(sub_id, &sub.url, out1.remote.as_deref().unwrap(), None)?;
    assert_eq!(ical::parse(&store.subscriptions()?[0].feed).len(), 1);

    // 2. Local upsert + remote delete via two-way sync.
    let draft = EventDraft {
        uid: "local-1".into(),
        title: "Created Here".into(),
        location: "Room 1".into(),
        notes: String::new(),
        start_date: NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        start_time: chrono::NaiveTime::from_hms_opt(11, 0, 0),
        end_date: NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        end_time: chrono::NaiveTime::from_hms_opt(12, 0, 0),
        all_day: false,
        timezone: None,
    };
    let local_ics = ical::serialize_event(&draft);
    store.upsert_local_calendar_event(sub_id, "local-1", &caldav::event_href(&sub.url, "local-1"), &local_ics)?;
    store.mark_calendar_event_deleted(sub_id, "remote-1")?;

    let local = store.calendar_events(sub_id)?;
    let out2 = sync_caldav_remote(&sub, &local);
    assert!(out2.errors.is_empty(), "{:?}", out2.errors);
    for (uid, href, etag) in &out2.upserts_pushed {
        store.mark_caldav_event_clean(sub_id, uid, href, etag.as_deref())?;
    }
    for uid in &out2.deletes_pushed {
        store.remove_caldav_event(sub_id, uid)?;
    }
    store.replace_caldav_snapshot(sub_id, &sub.url, out2.remote.as_deref().unwrap(), None)?;

    let final_events = ical::parse(&store.subscriptions()?[0].feed);
    assert_eq!(final_events.len(), 1);
    assert_eq!(final_events[0].uid, "local-1");
    assert!(auth_seen.lock().unwrap().iter().any(|h| h.starts_with("Basic ")));
    let _ = handle.join();
    Ok(())
}

#[test]
fn caldav_partial_push_preserves_succeeded_etag_when_later_op_fails() -> Result<()> {
    let store = Rc::new(Store::open_in_memory()?);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_ip().unwrap();
    let cal_url = format!("http://{addr}/dav/cal/");

    let handle = std::thread::spawn(move || {
        for req in server.incoming_requests().take(3) {
            let path = req.url().to_owned();
            let method = req.method().as_str().to_owned();
            match (method.as_str(), path.as_str()) {
                ("PUT", "/dav/cal/ok-event.ics") => {
                    let resp = tiny_http::Response::from_string("")
                        .with_status_code(201)
                        .with_header(tiny_http::Header::from_bytes(&b"ETag"[..], &b"\"etag-new\""[..]).unwrap());
                    let _ = req.respond(resp);
                }
                ("PUT", "/dav/cal/fail-event.ics") => {
                    let _ = req.respond(tiny_http::Response::from_string("err").with_status_code(500));
                }
                ("REPORT", "/dav/cal/") => {
                    let _ = req.respond(tiny_http::Response::from_string("err").with_status_code(500));
                }
                _ => {
                    let _ = req.respond(tiny_http::Response::from_string("").with_status_code(404));
                }
            }
        }
    });

    let sub_id = store.add_caldav_subscription("Work", &cal_url, None, CalendarProvider::CalDav, "u", "p", true)?;
    let sub = store.subscriptions()?.into_iter().find(|s| s.id == sub_id).unwrap();
    let make_ics = |uid: &str| {
        format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{uid}\r\nDTSTART:20261110T090000\r\nDTEND:20261110T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    };
    store.upsert_local_calendar_event(sub_id, "ok-event", &caldav::event_href(&cal_url, "ok-event"), &make_ics("ok-event"))?;
    store.upsert_local_calendar_event(sub_id, "fail-event", &caldav::event_href(&cal_url, "fail-event"), &make_ics("fail-event"))?;

    let local = store.calendar_events(sub_id)?;
    let outcome = sync_caldav_remote(&sub, &local);
    assert_eq!(outcome.upserts_pushed.len(), 1);
    assert_eq!(outcome.upserts_pushed[0].0, "ok-event");
    assert_eq!(outcome.upserts_pushed[0].2.as_deref(), Some("\"etag-new\""));
    assert!(outcome.remote.is_none());
    assert_eq!(outcome.errors.len(), 2);

    for (uid, href, etag) in &outcome.upserts_pushed {
        store.mark_caldav_event_clean(sub_id, uid, href, etag.as_deref())?;
    }
    let after = store.calendar_events(sub_id)?;
    let ok_ev = after.iter().find(|e| e.uid == "ok-event").unwrap();
    let fail_ev = after.iter().find(|e| e.uid == "fail-event").unwrap();
    assert_eq!(ok_ev.dirty, EventDirty::Clean);
    assert_eq!(ok_ev.etag.as_deref(), Some("\"etag-new\""));
    assert_eq!(fail_ev.dirty, EventDirty::Upsert);

    let _ = handle.join();
    Ok(())
}

#[test]
fn caldav_412_precondition_failed_recovers_via_etag_refresh() -> Result<()> {
    let store = Rc::new(Store::open_in_memory()?);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_ip().unwrap();
    let cal_url = format!("http://{addr}/dav/cal/");

    let handle = std::thread::spawn(move || {
        for req in server.incoming_requests().take(4) {
            let method = req.method().as_str().to_owned();
            let path = req.url().to_owned();
            let if_match = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("If-Match"))
                .map(|h| h.value.to_string());
            match (method.as_str(), path.as_str()) {
                ("PUT", "/dav/cal/conflict.ics") if if_match.as_deref() == Some("\"stale\"") => {
                    let _ = req.respond(tiny_http::Response::from_string("").with_status_code(412));
                }
                ("GET", "/dav/cal/conflict.ics") => {
                    let resp = tiny_http::Response::from_string(FEED)
                        .with_status_code(200)
                        .with_header(tiny_http::Header::from_bytes(&b"ETag"[..], &b"\"fresh\""[..]).unwrap());
                    let _ = req.respond(resp);
                }
                ("PUT", "/dav/cal/conflict.ics") if if_match.as_deref() == Some("\"fresh\"") => {
                    let resp = tiny_http::Response::from_string("")
                        .with_status_code(204)
                        .with_header(tiny_http::Header::from_bytes(&b"ETag"[..], &b"\"updated\""[..]).unwrap());
                    let _ = req.respond(resp);
                }
                ("REPORT", "/dav/cal/") => {
                    let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"></d:multistatus>"#;
                    let _ = req.respond(tiny_http::Response::from_string(xml).with_status_code(207));
                }
                _ => {
                    let _ = req.respond(tiny_http::Response::from_string("").with_status_code(500));
                }
            }
        }
    });

    let sub_id = store.add_caldav_subscription("Work", &cal_url, None, CalendarProvider::CalDav, "u", "p", true)?;
    let sub = store.subscriptions()?.into_iter().find(|s| s.id == sub_id).unwrap();
    let ev = StoredEvent {
        subscription_id: sub_id,
        uid: "conflict".into(),
        href: format!("{cal_url}conflict.ics"),
        etag: Some("\"stale\"".into()),
        ics: FEED.into(),
        dirty: EventDirty::Upsert,
    };
    let outcome = sync_caldav_remote(&sub, &[ev]);
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(outcome.upserts_pushed.len(), 1);
    assert_eq!(outcome.upserts_pushed[0].2.as_deref(), Some("\"updated\""));
    let _ = handle.join();
    Ok(())
}
