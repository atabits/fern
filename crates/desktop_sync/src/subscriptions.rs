//! Subscribed calendars, the original's "URL" calendars: their feeds are read from their
//! addresses now and then, kept in the store for offline use, and their events laid out for the
//! calendar.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use chrono::{Local, NaiveDate, TimeZone};
use fern_core::caldav;
#[cfg(not(target_arch = "wasm32"))]
use fern_core::ical::EventDirty;
use fern_core::ical::{self, CalendarKind, CalendarProvider, Event, EventDraft, StoredEvent, Subscription};
use fern_core::{Id, Store};
use gpui::{App, AppContext as _, Context, Entity, Global};

/// Not right at startup, so that the first frame does not wait on the network.
const FIRST_READ_AFTER: Duration = Duration::from_secs(5);
const READ_EVERY: Duration = Duration::from_secs(30 * 60);
/// The largest feed read, so that a wrong address cannot fill the memory.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FEED: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChinaHolidaySource {
    Google,
    Apple,
}

impl ChinaHolidaySource {
    pub const ALL: [Self; 2] = [Self::Google, Self::Apple];

    pub const fn url(self) -> &'static str {
        match self {
            Self::Google => fern_i18n::CHINA_HOLIDAY_GOOGLE_URL,
            Self::Apple => "https://calendars.icloud.com/holidays/cn_zh.ics",
        }
    }

    pub fn from_url(url: &str) -> Option<Self> {
        let url = url.trim();
        let address = url.strip_prefix("https://").or_else(|| url.strip_prefix("webcal://"))?;
        Self::ALL.into_iter().find(|source| &source.url()["https://".len()..] == address)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Google => "Google Calendar",
            Self::Apple => "Apple iCloud",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChinaHolidayStatus {
    #[default]
    Idle,
    Reading,
    Failed,
}

/// A subscribed calendar with the events of its last feed.
pub struct Calendar {
    pub subscription: Subscription,
    pub events: Vec<Event>,
}

/// An event on one of the days asked for: the day its occurrence starts.
pub struct Occurrence<'a> {
    pub event: &'a Event,
    pub calendar: &'a Subscription,
    pub times: ical::Occurrence,
}

pub struct Feeds {
    store: Rc<Store>,
    calendars: Vec<Calendar>,
    /// The active read request for each subscription.
    reading: HashMap<Id, Id>,
    /// Monotonic counter so stale background `.ics` parses never overwrite newer ones.
    load_generation: u64,
    china_holiday_status: ChinaHolidayStatus,
}

struct GlobalFeeds(Entity<Feeds>);

impl Global for GlobalFeeds {}

pub fn init(store: Rc<Store>, cx: &mut App) {
    let feeds = cx.new(|cx| {
        let mut feeds = Feeds::new_empty(store);
        feeds.reload_async(cx);
        feeds.start(cx);
        feeds
    });
    cx.set_global(GlobalFeeds(feeds));
}

pub fn feeds(cx: &App) -> Option<Entity<Feeds>> {
    cx.try_global::<GlobalFeeds>().map(|global| global.0.clone())
}

fn parse_calendars(subscriptions: Vec<Subscription>) -> Vec<Calendar> {
    subscriptions
        .into_iter()
        .map(|mut subscription| {
            let parsed = ical::parse_with_warnings(&subscription.feed);
            if !parsed.warnings.is_empty() {
                let warning = parsed.warnings.join("; ");
                subscription.error = Some(match subscription.error {
                    Some(error) => format!("{error}; {warning}"),
                    None => warning,
                });
            }
            Calendar { events: parsed.events, subscription }
        })
        .collect()
}

impl Feeds {
    fn new_empty(store: Rc<Store>) -> Self {
        Self {
            store,
            calendars: Vec::new(),
            reading: HashMap::new(),
            load_generation: 0,
            china_holiday_status: ChinaHolidayStatus::Idle,
        }
    }

    #[cfg(test)]
    fn new(store: Rc<Store>) -> Self {
        let mut feeds = Self::new_empty(store);
        feeds.load();
        feeds
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIRST_READ_AFTER).await;
            loop {
                if this.update(cx, |this, cx| this.read_all(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(READ_EVERY).await;
            }
        })
        .detach();
    }

    /// Synchronously loads and parses feeds (used only by synchronous unit tests).
    #[cfg(test)]
    fn load(&mut self) {
        let subscriptions = self.store.subscriptions().unwrap_or_default();
        self.calendars = parse_calendars(subscriptions);
    }

    /// Reconciles subscription metadata immediately so UI lists stay responsive, then offloads
    /// `.ics` feed parsing to the background executor so the UI thread is never blocked.
    fn reload_async(&mut self, cx: &mut Context<Self>) {
        let subscriptions = self.store.subscriptions().unwrap_or_default();
        let mut existing: HashMap<Id, Vec<Event>> = self
            .calendars
            .drain(..)
            .map(|c| (c.subscription.id, c.events))
            .collect();
        self.calendars = subscriptions
            .iter()
            .map(|sub| Calendar {
                events: existing.remove(&sub.id).unwrap_or_default(),
                subscription: sub.clone(),
            })
            .collect();

        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_executor()
                .spawn(async move { parse_calendars(subscriptions) })
                .await;
            this.update(cx, |this, cx| {
                if this.load_generation == generation {
                    this.calendars = parsed;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn calendars(&self) -> &[Calendar] {
        &self.calendars
    }

    pub fn is_reading(&self, id: Id) -> bool {
        self.reading.contains_key(&id)
    }

    pub fn china_holiday_status(&self) -> ChinaHolidayStatus {
        self.china_holiday_status
    }

    /// The events of the shown calendars that cover any day from `from` to `to`.
    pub fn occurrences(&self, from: NaiveDate, to: NaiveDate) -> Vec<Occurrence<'_>> {
        self.occurrences_in_timezone(from, to, &Local)
    }

    fn occurrences_in_timezone<Tz: TimeZone>(&self, from: NaiveDate, to: NaiveDate, timezone: &Tz) -> Vec<Occurrence<'_>> {
        self.calendars
            .iter()
            .filter(|calendar| !calendar.subscription.hidden)
            .flat_map(|calendar| {
                calendar.events.iter().flat_map(move |event| {
                    event.occurrences_in_timezone(from, to, timezone).into_iter()
                        .map(move |times| Occurrence { event, calendar: &calendar.subscription, times })
                })
            })
            .collect()
    }

    pub fn read_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<Id> = self.calendars.iter().map(|calendar| calendar.subscription.id).collect();
        for id in ids {
            self.read(id, cx);
        }
    }

    /// Reads a subscription's feed (ICS pull or CalDAV two-way push/pull) off the UI thread.
    pub fn read(&mut self, id: Id, cx: &mut Context<Self>) {
        let Some(sub) = self
            .calendars
            .iter()
            .find(|c| c.subscription.id == id)
            .map(|c| c.subscription.clone())
            .or_else(|| {
                self.store
                    .subscriptions()
                    .ok()
                    .and_then(|subs| subs.into_iter().find(|s| s.id == id))
            })
        else {
            return;
        };
        if self.is_reading(id) {
            return;
        }
        let request = Id::now_v7();
        self.reading.insert(id, request);
        cx.notify();

        if sub.kind == CalendarKind::CalDav {
            let local_events = self.store.calendar_events(id).unwrap_or_default();
            let url = sub.url.clone();
            cx.spawn(async move |this, cx| {
                let outcome = cx
                    .background_executor()
                    .spawn(async move { sync_caldav_remote(&sub, &local_events) })
                    .await;
                this.update(cx, |this, cx| {
                    if this.reading.get(&id) != Some(&request) {
                        return;
                    }
                    this.reading.remove(&id);
                    // Always persist every push/delete that succeeded remotely before recording
                    // any partial error, so subsequent syncs never retry with a stale ETag.
                    for (uid, href, etag) in &outcome.upserts_pushed {
                        if let Err(err) = this.store.mark_caldav_event_clean(id, uid, href, etag.as_deref()) {
                            eprintln!("failed to mark CalDAV event clean: {err:#}");
                        }
                    }
                    for uid in &outcome.deletes_pushed {
                        if let Err(err) = this.store.remove_caldav_event(id, uid) {
                            eprintln!("failed to remove deleted CalDAV event: {err:#}");
                        }
                    }
                    if let Some(remote) = &outcome.remote
                        && let Err(err) = this.store.replace_caldav_snapshot(id, &url, remote, outcome.sync_token.as_deref())
                    {
                        eprintln!("failed to save CalDAV snapshot: {err:#}");
                    }
                    if !outcome.errors.is_empty() {
                        let msg = outcome.errors.join("; ");
                        if let Err(save_err) = this.store.save_subscription_feed(id, &url, Err(&msg)) {
                            eprintln!("failed to save CalDAV sync error: {save_err:#}");
                        }
                    }
                    this.reload_async(cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
            return;
        }

        let url = sub.url;
        cx.spawn(async move |this, cx| {
            let read_url = url.clone();
            let feed = cx.background_executor().spawn(async move { fetch(&read_url) }).await;
            this.update(cx, |this, cx| {
                if this.reading.get(&id) != Some(&request) {
                    return;
                }
                this.reading.remove(&id);
                let error = feed.as_ref().err().map(|err| format!("{err:#}"));
                if let Err(err) = this.store.save_subscription_feed(id, &url, feed.as_deref().map_err(|_| error.as_deref().unwrap_or_default())) {
                    eprintln!("failed to keep the calendar feed: {err:#}");
                }
                this.reload_async(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn select_china_holiday_source(
        &mut self,
        current: Option<Id>,
        source: ChinaHolidaySource,
        name: &'static str,
        cx: &mut Context<Self>,
    ) {
        if self.china_holiday_status == ChinaHolidayStatus::Reading {
            return;
        }
        let mut previous = None;
        for calendar in &self.calendars {
            let subscription = &calendar.subscription;
            if let Some(existing) = ChinaHolidaySource::from_url(&subscription.url) {
                if current.is_none() || existing == source {
                    return;
                }
                if current == Some(subscription.id) {
                    previous = Some((subscription.id, subscription.url.clone()));
                }
            }
        }
        if current.is_some() && previous.is_none() {
            return;
        }
        self.china_holiday_status = ChinaHolidayStatus::Reading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move {
                let feed = fetch(source.url())?;
                anyhow::ensure!(!ical::parse(&feed).is_empty(), "the holiday feed has no events");
                Ok(feed)
            }).await;
            this.update(cx, |this, cx| {
                this.apply_china_holiday_change(previous.as_ref().map(|(id, url)| (*id, url.as_str())), source, name, result);
                this.reload_async(cx);
                cx.notify();
            }).ok();
        }).detach();
    }

    fn apply_china_holiday_change(
        &mut self,
        previous: Option<(Id, &str)>,
        source: ChinaHolidaySource,
        name: &str,
        result: Result<String>,
    ) {
        let saved = result.and_then(|feed| {
            let subscriptions = self.store.subscriptions()?;
            let has_provider = subscriptions.iter().any(|subscription| {
                ChinaHolidaySource::from_url(&subscription.url).is_some_and(|existing| {
                    previous.is_none() || existing == source
                })
            });
            anyhow::ensure!(!has_provider, "the holiday calendar is already subscribed");
            if let Some((id, url)) = previous {
                anyhow::ensure!(
                    self.store.replace_subscription_source(id, url, source.url(), &feed)?,
                    "the subscription changed while the holiday feed was loading",
                );
                self.reading.remove(&id);
            } else {
                self.store.add_subscription_with_feed(name, source.url(), Some("#22C55E"), &feed)?;
            }
            Ok(())
        });
        self.china_holiday_status = match saved {
            Ok(()) => ChinaHolidayStatus::Idle,
            Err(err) => {
                eprintln!("failed to select the holiday calendar: {err:#}");
                ChinaHolidayStatus::Failed
            }
        };
    }

    #[cfg(test)]
    fn finish_china_holiday_change(
        &mut self,
        previous: Option<(Id, &str)>,
        source: ChinaHolidaySource,
        name: &str,
        result: Result<String>,
    ) {
        self.apply_china_holiday_change(previous, source, name, result);
        self.load();
    }

    pub fn add(&mut self, name: &str, url: &str, color: Option<&str>, cx: &mut Context<Self>) {
        match self.store.add_subscription(name, url, color) {
            Ok(id) => {
                self.reload_async(cx);
                self.read(id, cx);
            }
            Err(err) => eprintln!("failed to subscribe to the calendar: {err:#}"),
        }
        cx.notify();
    }

    pub fn update_subscription(&mut self, id: Id, name: &str, url: &str, color: Option<&str>, hidden: bool, cx: &mut Context<Self>) {
        let moved = self.calendars.iter().find(|c| c.subscription.id == id).is_some_and(|c| c.subscription.url.trim() != url.trim());
        if let Err(err) = self.store.update_subscription(id, name, url, color, hidden) {
            eprintln!("failed to change the subscription: {err:#}");
        }
        self.reload_async(cx);
        if moved {
            self.reading.remove(&id);
            self.read(id, cx);
        }
        cx.notify();
    }

    pub fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.delete_subscription(id) {
            eprintln!("failed to unsubscribe: {err:#}");
        } else {
            self.reading.remove(&id);
        }
        self.reload_async(cx);
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_caldav(
        &mut self,
        name: &str,
        url: &str,
        color: Option<&str>,
        provider: CalendarProvider,
        username: &str,
        password: &str,
        writable: bool,
        cx: &mut Context<Self>,
    ) {
        match self.store.add_caldav_subscription(name, url, color, provider, username, password, writable) {
            Ok(id) => {
                self.reload_async(cx);
                self.read(id, cx);
            }
            Err(err) => eprintln!("failed to add CalDAV calendar: {err:#}"),
        }
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_caldav(
        &mut self,
        id: Id,
        name: &str,
        url: &str,
        color: Option<&str>,
        hidden: bool,
        provider: CalendarProvider,
        username: &str,
        password: &str,
        writable: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = self.calendars.iter().find(|c| c.subscription.id == id).is_some_and(|c| {
            c.subscription.url.trim() != url.trim() || c.subscription.username != username.trim() || c.subscription.password != password
        });
        if let Err(err) = self.store.update_caldav_subscription(id, name, url, color, hidden, provider, username, password, writable) {
            eprintln!("failed to update CalDAV calendar: {err:#}");
        }
        self.reload_async(cx);
        if changed {
            self.reading.remove(&id);
            self.read(id, cx);
        }
        cx.notify();
    }

    /// Enabled, visible CalDAV calendars with write permission enabled.
    pub fn writable_calendars(&self) -> Vec<&Subscription> {
        self.calendars
            .iter()
            .filter(|c| !c.subscription.hidden && c.subscription.is_caldav() && c.subscription.writable)
            .map(|c| &c.subscription)
            .collect()
    }

    pub fn stored_event(&self, subscription_id: Id, uid: &str) -> Option<StoredEvent> {
        self.store
            .calendar_events(subscription_id)
            .ok()?
            .into_iter()
            .find(|ev| ev.uid == uid)
    }

    /// Creates or updates a writable CalDAV event locally and pushes it in the background.
    pub fn save_event(&mut self, subscription_id: Id, draft: &EventDraft, cx: &mut Context<Self>) -> Result<()> {
        let cal_url = self
            .calendars
            .iter()
            .find(|c| c.subscription.id == subscription_id)
            .map(|c| c.subscription.url.clone())
            .context("calendar subscription not found")?;
        let existing_href = self
            .stored_event(subscription_id, &draft.uid)
            .map(|e| e.href)
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| caldav::event_href(&cal_url, &draft.uid));
        let ics = ical::serialize_event(draft);
        self.store.upsert_local_calendar_event(subscription_id, &draft.uid, &existing_href, &ics)?;
        self.reload_async(cx);
        cx.notify();
        self.reading.remove(&subscription_id);
        self.read(subscription_id, cx);
        Ok(())
    }

    /// Marks a writable CalDAV event for remote deletion and syncs immediately.
    pub fn delete_event(&mut self, subscription_id: Id, uid: &str, cx: &mut Context<Self>) -> Result<()> {
        self.store.mark_calendar_event_deleted(subscription_id, uid)?;
        self.reload_async(cx);
        cx.notify();
        self.reading.remove(&subscription_id);
        self.read(subscription_id, cx);
        Ok(())
    }
}

/// A CalDAV calendar found on a server via `PROPFIND` discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredCalendar {
    pub url: String,
    pub name: String,
    pub writable: bool,
}

struct CalDavSyncOutcome {
    upserts_pushed: Vec<(String, String, Option<String>)>,
    deletes_pushed: Vec<String>,
    remote: Option<Vec<StoredEvent>>,
    sync_token: Option<String>,
    errors: Vec<String>,
}

#[cfg(not(target_arch = "wasm32"))]
fn dav_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .allow_non_standard_methods(true)
        .build()
        .into()
}

/// Discovers CalDAV calendars under `url` via `PROPFIND` principal -> calendar-home-set -> calendars.
#[cfg(not(target_arch = "wasm32"))]
pub fn discover_caldav_calendars(url: &str, username: &str, password: &str) -> Result<Vec<DiscoveredCalendar>> {
    let base_url = normalize_https_url(url);
    anyhow::ensure!(!base_url.is_empty(), "a CalDAV URL is required");
    let auth = caldav::auth_header(username, password);
    let agent = dav_agent();

    let (status, _, body) = dav_request(
        &agent,
        "PROPFIND",
        &base_url,
        auth.as_deref(),
        Some("0"),
        Some("application/xml; charset=utf-8"),
        None,
        caldav::propfind_discovery_body(),
    )?;
    anyhow::ensure!(status == 207 || status == 200, "CalDAV discovery failed with HTTP {status}");
    let root_resources = caldav::parse_multistatus(&body);
    let direct: Vec<DiscoveredCalendar> = root_resources
        .iter()
        .filter(|r| r.is_calendar)
        .map(|r| DiscoveredCalendar {
            url: caldav::resolve_href(&base_url, &r.href),
            name: r.display_name.clone().unwrap_or_else(|| "Calendar".to_owned()),
            writable: !r.read_only,
        })
        .collect();
    if !direct.is_empty() {
        return Ok(direct);
    }

    let mut home_url = root_resources
        .iter()
        .find_map(|r| r.calendar_home_set.as_deref())
        .map(|href| caldav::resolve_href(&base_url, href));

    if home_url.is_none()
        && let Some(principal_href) = root_resources.iter().find_map(|r| r.current_user_principal.as_deref())
    {
        let principal_url = caldav::resolve_href(&base_url, principal_href);
        let (p_status, _, p_body) = dav_request(
            &agent,
            "PROPFIND",
            &principal_url,
            auth.as_deref(),
            Some("0"),
            Some("application/xml; charset=utf-8"),
            None,
            caldav::propfind_home_set_body(),
        )?;
        if p_status == 207 || p_status == 200 {
            let p_res = caldav::parse_multistatus(&p_body);
            home_url = p_res
                .iter()
                .find_map(|r| r.calendar_home_set.as_deref())
                .map(|href| caldav::resolve_href(&principal_url, href));
        }
    }

    let target_home = home_url.unwrap_or_else(|| base_url.clone());
    let (h_status, _, h_body) = dav_request(
        &agent,
        "PROPFIND",
        &target_home,
        auth.as_deref(),
        Some("1"),
        Some("application/xml; charset=utf-8"),
        None,
        caldav::propfind_calendars_body(),
    )?;
    anyhow::ensure!(h_status == 207 || h_status == 200, "listing CalDAV calendars failed with HTTP {h_status}");
    let listed: Vec<DiscoveredCalendar> = caldav::parse_multistatus(&h_body)
        .into_iter()
        .filter(|r| r.is_calendar)
        .map(|r| DiscoveredCalendar {
            url: caldav::resolve_href(&target_home, &r.href),
            name: r.display_name.unwrap_or_else(|| "Calendar".to_owned()),
            writable: !r.read_only,
        })
        .collect();
    anyhow::ensure!(!listed.is_empty(), "no CalDAV calendars were found at {target_home}");
    Ok(listed)
}

#[cfg(not(target_arch = "wasm32"))]
fn push_caldav_upsert(
    agent: &ureq::Agent,
    base_url: &str,
    auth: Option<&str>,
    ev: &StoredEvent,
) -> Result<(String, String, Option<String>)> {
    let target = if ev.href.is_empty() {
        caldav::event_href(base_url, &ev.uid)
    } else {
        caldav::resolve_href(base_url, &ev.href)
    };
    let (status, etag, _body) = dav_request(
        agent,
        "PUT",
        &target,
        auth,
        None,
        Some("text/calendar; charset=utf-8"),
        ev.etag.as_deref(),
        &ev.ics,
    )?;
    if (200..300).contains(&status) {
        return Ok((ev.uid.clone(), target, etag.or_else(|| ev.etag.clone())));
    }
    if status == 412 {
        // Precondition Failed: refresh the server's current ETag (or detect remote deletion)
        // and retry once so a stale ETag never permanently wedges two-way sync.
        let (g_status, fresh_etag, _g_body) = dav_request(agent, "GET", &target, auth, None, None, None, "")?;
        let retry_if_match = if g_status == 404 || g_status == 410 {
            None
        } else if (200..300).contains(&g_status) {
            fresh_etag.as_deref()
        } else {
            anyhow::bail!("CalDAV PUT {target} returned HTTP 412 and refresh GET returned HTTP {g_status}");
        };
        let (r_status, r_etag, _r_body) = dav_request(
            agent,
            "PUT",
            &target,
            auth,
            None,
            Some("text/calendar; charset=utf-8"),
            retry_if_match,
            &ev.ics,
        )?;
        anyhow::ensure!(
            (200..300).contains(&r_status),
            "CalDAV PUT {target} failed after 412 retry with HTTP {r_status}"
        );
        return Ok((ev.uid.clone(), target, r_etag.or(fresh_etag)));
    }
    anyhow::bail!("CalDAV PUT {target} returned HTTP {status}");
}

#[cfg(not(target_arch = "wasm32"))]
fn push_caldav_delete(
    agent: &ureq::Agent,
    base_url: &str,
    auth: Option<&str>,
    ev: &StoredEvent,
) -> Result<()> {
    if ev.href.is_empty() {
        return Ok(());
    }
    let target = caldav::resolve_href(base_url, &ev.href);
    let (status, _, _) = dav_request(agent, "DELETE", &target, auth, None, None, ev.etag.as_deref(), "")?;
    if (200..300).contains(&status) || status == 404 || status == 410 {
        return Ok(());
    }
    if status == 412 {
        let (g_status, fresh_etag, _) = dav_request(agent, "GET", &target, auth, None, None, None, "")?;
        if g_status == 404 || g_status == 410 {
            return Ok(());
        }
        anyhow::ensure!(
            (200..300).contains(&g_status),
            "CalDAV DELETE {target} returned HTTP 412 and refresh GET returned HTTP {g_status}"
        );
        let (r_status, _, _) = dav_request(agent, "DELETE", &target, auth, None, None, fresh_etag.as_deref(), "")?;
        anyhow::ensure!(
            (200..300).contains(&r_status) || r_status == 404 || r_status == 410,
            "CalDAV DELETE {target} failed after 412 retry with HTTP {r_status}"
        );
        return Ok(());
    }
    anyhow::bail!("CalDAV DELETE {target} returned HTTP {status}");
}

#[cfg(not(target_arch = "wasm32"))]
fn pull_caldav_remote(
    agent: &ureq::Agent,
    sub: &Subscription,
    base_url: &str,
    auth: Option<&str>,
    local_events: &[StoredEvent],
) -> Result<Vec<StoredEvent>> {
    let (status, _, body) = dav_request(
        agent,
        "REPORT",
        base_url,
        auth,
        Some("1"),
        Some("application/xml; charset=utf-8"),
        None,
        caldav::calendar_query_body(),
    )?;

    let mut remote = Vec::new();
    if status == 207 || status == 200 {
        for res in caldav::parse_multistatus(&body) {
            if res.is_collection || res.is_calendar {
                continue;
            }
            let resolved_href = caldav::resolve_href(base_url, &res.href);
            let data = if let Some(ics) = res.calendar_data {
                ics
            } else if res.href.ends_with(".ics") {
                let (g_status, _, g_body) = dav_request(agent, "GET", &resolved_href, auth, None, None, None, "")?;
                if !(200..300).contains(&g_status) {
                    continue;
                }
                g_body
            } else {
                continue;
            };
            if let Some(uid) = first_event_uid(&data, &resolved_href) {
                remote.push(StoredEvent {
                    subscription_id: sub.id,
                    uid,
                    href: resolved_href,
                    etag: res.etag,
                    ics: data,
                    dirty: EventDirty::Clean,
                });
            }
        }
    } else if status == 405 || status == 501 || status == 400 {
        // Fallback: PROPFIND Depth:1 + GET for servers without REPORT calendar-query.
        let (p_status, _, p_body) = dav_request(
            agent,
            "PROPFIND",
            base_url,
            auth,
            Some("1"),
            Some("application/xml; charset=utf-8"),
            None,
            caldav::propfind_etags_body(),
        )?;
        anyhow::ensure!(p_status == 207 || p_status == 200, "CalDAV PROPFIND {base_url} returned HTTP {p_status}");
        let clean_by_href: HashMap<&str, &StoredEvent> = local_events
            .iter()
            .filter(|e| e.dirty == EventDirty::Clean)
            .map(|e| (e.href.as_str(), e))
            .collect();
        for res in caldav::parse_multistatus(&p_body) {
            if res.is_collection || !res.href.ends_with(".ics") {
                continue;
            }
            let resolved_href = caldav::resolve_href(base_url, &res.href);
            let cached_same = clean_by_href.get(resolved_href.as_str()).copied().filter(|e| {
                e.etag
                    .as_deref()
                    .zip(res.etag.as_deref())
                    .is_some_and(|(a, b)| caldav::normalize_etag(a) == caldav::normalize_etag(b))
            });
            let (etag, data) = if let Some(cached) = cached_same {
                (cached.etag.clone(), cached.ics.clone())
            } else {
                let (g_status, g_etag, g_body) = dav_request(agent, "GET", &resolved_href, auth, None, None, None, "")?;
                if !(200..300).contains(&g_status) {
                    continue;
                }
                (g_etag.or(res.etag), g_body)
            };
            if let Some(uid) = first_event_uid(&data, &resolved_href) {
                remote.push(StoredEvent {
                    subscription_id: sub.id,
                    uid,
                    href: resolved_href,
                    etag,
                    ics: data,
                    dirty: EventDirty::Clean,
                });
            }
        }
    } else {
        anyhow::bail!("CalDAV REPORT {base_url} returned HTTP {status}");
    }

    Ok(remote)
}

#[cfg(not(target_arch = "wasm32"))]
fn sync_caldav_remote(sub: &Subscription, local_events: &[StoredEvent]) -> CalDavSyncOutcome {
    let base_url = normalize_https_url(&sub.url);
    let auth = caldav::auth_header(&sub.username, &sub.password);
    let agent = dav_agent();
    let mut upserts_pushed = Vec::new();
    let mut deletes_pushed = Vec::new();
    let mut errors = Vec::new();

    if sub.writable {
        for ev in local_events {
            match ev.dirty {
                EventDirty::Clean => {}
                EventDirty::Upsert => match push_caldav_upsert(&agent, &base_url, auth.as_deref(), ev) {
                    Ok(pushed) => upserts_pushed.push(pushed),
                    Err(err) => errors.push(format!("{err:#}")),
                },
                EventDirty::Delete => match push_caldav_delete(&agent, &base_url, auth.as_deref(), ev) {
                    Ok(()) => deletes_pushed.push(ev.uid.clone()),
                    Err(err) => errors.push(format!("{err:#}")),
                },
            }
        }
    }

    let remote = match pull_caldav_remote(&agent, sub, &base_url, auth.as_deref(), local_events) {
        Ok(events) => Some(events),
        Err(err) => {
            errors.push(format!("{err:#}"));
            None
        }
    };

    CalDavSyncOutcome { upserts_pushed, deletes_pushed, remote, sync_token: None, errors }
}

#[cfg(not(target_arch = "wasm32"))]
fn first_event_uid(ics: &str, fallback_href: &str) -> Option<String> {
    let parsed = ical::parse(ics);
    if let Some(ev) = parsed.first()
        && !ev.uid.is_empty()
    {
        return Some(ev.uid.clone());
    }
    fallback_href
        .rsplit('/')
        .next()
        .map(|s| s.trim_end_matches(".ics"))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(not(target_arch = "wasm32"))]
fn normalize_https_url(url: &str) -> String {
    let url = url.trim();
    match url.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_owned(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn dav_request(
    agent: &ureq::Agent,
    method: &str,
    url: &str,
    auth: Option<&str>,
    depth: Option<&str>,
    content_type: Option<&str>,
    if_match: Option<&str>,
    body: &str,
) -> Result<(u16, Option<String>, String)> {
    let mut builder = ureq::http::Request::builder()
        .method(method)
        .uri(url)
        .header("User-Agent", &format!("fern/{}", env!("CARGO_PKG_VERSION")));
    if let Some(auth) = auth {
        builder = builder.header("Authorization", auth);
    }
    if let Some(depth) = depth {
        builder = builder.header("Depth", depth);
    }
    if let Some(ct) = content_type {
        builder = builder.header("Content-Type", ct);
    }
    if let Some(etag) = if_match {
        builder = builder.header("If-Match", etag);
    }
    let req = builder.body(body.to_owned()).with_context(|| format!("invalid CalDAV request for {url}"))?;
    let mut response = agent.run(req).with_context(|| format!("CalDAV {method} {url} failed"))?;
    let status = response.status().as_u16();
    let etag = response.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_owned);
    let text = response.body_mut().with_config().limit(MAX_FEED).read_to_string()?;
    Ok((status, etag, text))
}

/// Reads a feed; `webcal://` addresses are read over HTTPS, as calendar apps do.
#[cfg(not(target_arch = "wasm32"))]
fn fetch(url: &str) -> Result<String> {
    let url = url.trim();
    let url = match url.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_owned(),
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut response = agent.get(&url)
        .header("User-Agent", &format!("fern/{}", env!("CARGO_PKG_VERSION")))
        .call()
        .with_context(|| format!("cannot read {url}"))?;
    let text = response.body_mut().with_config().limit(MAX_FEED).read_to_string()?;
    anyhow::ensure!(text.contains("BEGIN:VCALENDAR"), "the address gives no calendar");
    Ok(text)
}

#[cfg(target_arch = "wasm32")]
pub fn discover_caldav_calendars(_url: &str, _username: &str, _password: &str) -> Result<Vec<DiscoveredCalendar>> {
    anyhow::bail!("CalDAV discovery is unavailable in the browser build")
}

#[cfg(target_arch = "wasm32")]
fn sync_caldav_remote(_sub: &Subscription, _local_events: &[StoredEvent]) -> CalDavSyncOutcome {
    CalDavSyncOutcome {
        upserts_pushed: Vec::new(),
        deletes_pushed: Vec::new(),
        remote: None,
        sync_token: None,
        errors: vec!["CalDAV sync is unavailable in the browser build".to_owned()],
    }
}

#[cfg(target_arch = "wasm32")]
fn fetch(_url: &str) -> Result<String> {
    anyhow::bail!("remote iCal feeds are unavailable in the browser build")
}

#[cfg(test)]
mod tests;
