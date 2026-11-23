use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::model::Id;

use super::{Store, now_ms, parse_uuid};

impl Store {
    /// The subscribed calendars, in the order they were added.
    pub fn subscriptions(&self) -> Result<Vec<crate::ical::Subscription>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, url, color, hidden, feed, fetched_at, error, kind, provider, username, password, writable, sync_token
             FROM calendar_subscriptions ORDER BY created_at, rowid",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let kind_str: String = row.get(8)?;
                let provider_str: String = row.get(9)?;
                Ok(crate::ical::Subscription {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    url: row.get(2)?,
                    color: row.get(3)?,
                    hidden: row.get(4)?,
                    feed: row.get(5)?,
                    fetched_at: row.get(6)?,
                    error: row.get(7)?,
                    kind: crate::ical::CalendarKind::from_code(&kind_str),
                    provider: crate::ical::CalendarProvider::from_code(&provider_str),
                    username: row.get(10)?,
                    password: row.get(11)?,
                    writable: row.get::<_, i64>(12)? != 0,
                    sync_token: row.get(13)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn add_subscription(&self, name: &str, url: &str, color: Option<&str>) -> Result<Id> {
        self.insert_subscription(name, url, color, None)
    }

    pub fn add_subscription_with_feed(&self, name: &str, url: &str, color: Option<&str>, feed: &str) -> Result<Id> {
        self.insert_subscription(name, url, color, Some(feed))
    }

    fn insert_subscription(&self, name: &str, url: &str, color: Option<&str>, feed: Option<&str>) -> Result<Id> {
        let url = url.trim();
        anyhow::ensure!(!url.is_empty(), "a subscription needs an address");
        let id = Uuid::now_v7();
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO calendar_subscriptions (id, name, url, color, created_at, feed, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id.to_string(), name.trim(), url, color, now, feed.unwrap_or_default(), feed.map(|_| now)],
        )?;
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_caldav_subscription(
        &self,
        name: &str,
        url: &str,
        color: Option<&str>,
        provider: crate::ical::CalendarProvider,
        username: &str,
        password: &str,
        writable: bool,
    ) -> Result<Id> {
        let url = url.trim();
        anyhow::ensure!(!url.is_empty(), "a CalDAV calendar needs an address");
        let id = Uuid::now_v7();
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO calendar_subscriptions (id, name, url, color, created_at, feed, fetched_at, kind, provider, username, password, writable)
             VALUES (?1, ?2, ?3, ?4, ?5, '', NULL, 'caldav', ?6, ?7, ?8, ?9)",
            params![
                id.to_string(),
                name.trim(),
                url,
                color,
                now,
                provider.code(),
                username.trim(),
                password,
                i64::from(writable),
            ],
        )?;
        Ok(id)
    }

    /// Changes a subscription; a new address drops the feed read from the old one.
    pub fn update_subscription(&self, id: Id, name: &str, url: &str, color: Option<&str>, hidden: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE calendar_subscriptions SET name = ?2, color = ?4, hidden = ?5,
                 feed = CASE WHEN url = ?3 THEN feed ELSE '' END,
                 fetched_at = CASE WHEN url = ?3 THEN fetched_at END,
                 url = ?3
             WHERE id = ?1",
            params![id.to_string(), name.trim(), url.trim(), color, hidden],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_caldav_subscription(
        &self,
        id: Id,
        name: &str,
        url: &str,
        color: Option<&str>,
        hidden: bool,
        provider: crate::ical::CalendarProvider,
        username: &str,
        password: &str,
        writable: bool,
    ) -> Result<()> {
        let url = url.trim();
        let prev_url: Option<String> = self
            .conn
            .query_row("SELECT url FROM calendar_subscriptions WHERE id = ?1", params![id.to_string()], |r| r.get(0))
            .optional()?;
        let url_changed = prev_url.as_deref() != Some(url);
        self.in_tx(|| {
            self.conn.execute(
                "UPDATE calendar_subscriptions
                 SET name = ?2, color = ?4, hidden = ?5, kind = 'caldav',
                     provider = ?6, username = ?7, password = ?8, writable = ?9,
                     feed = CASE WHEN url = ?3 THEN feed ELSE '' END,
                     fetched_at = CASE WHEN url = ?3 THEN fetched_at ELSE NULL END,
                     sync_token = CASE WHEN url = ?3 THEN sync_token ELSE NULL END,
                     error = CASE WHEN url = ?3 THEN error ELSE NULL END,
                     url = ?3
                 WHERE id = ?1",
                params![
                    id.to_string(),
                    name.trim(),
                    url,
                    color,
                    hidden,
                    provider.code(),
                    username.trim(),
                    password,
                    i64::from(writable),
                ],
            )?;
            if url_changed {
                self.conn.execute("DELETE FROM calendar_events WHERE subscription_id = ?1", params![id.to_string()])?;
            }
            Ok(())
        })
    }

    pub fn delete_subscription(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM calendar_subscriptions WHERE id = ?1", params![id.to_string()])?;
        Ok(())
    }

    /// Keeps the subscription's identity and preferences while replacing its address and feed together.
    pub fn replace_subscription_source(&self, id: Id, previous_url: &str, url: &str, feed: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE calendar_subscriptions SET url = ?3, feed = ?4, fetched_at = ?5, error = NULL
             WHERE id = ?1 AND url = ?2 AND NOT EXISTS (
                 SELECT 1 FROM calendar_subscriptions WHERE url = ?3 AND id != ?1
             )",
            params![id.to_string(), previous_url, url, feed, now_ms()],
        )?;
        Ok(changed == 1)
    }

    /// Keeps what was read from a subscription's address: the feed, or the error that kept it
    /// (the last feed read then stays).
    pub fn save_subscription_feed(&self, id: Id, url: &str, feed: Result<&str, &str>) -> Result<bool> {
        let changed = match feed {
            Ok(feed) => self.conn.execute(
                "UPDATE calendar_subscriptions SET feed = ?3, fetched_at = ?4, error = NULL WHERE id = ?1 AND url = ?2",
                params![id.to_string(), url, feed, now_ms()],
            )?,
            Err(error) => self.conn.execute(
                "UPDATE calendar_subscriptions SET error = ?3 WHERE id = ?1 AND url = ?2",
                params![id.to_string(), url, error],
            )?,
        };
        Ok(changed == 1)
    }

    /// Cached CalDAV events for a subscription, including locally queued deletions.
    pub fn calendar_events(&self, subscription_id: Id) -> Result<Vec<crate::ical::StoredEvent>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT subscription_id, uid, href, etag, ics, dirty FROM calendar_events
             WHERE subscription_id = ?1 ORDER BY updated_at, uid",
        )?;
        let rows = stmt
            .query_map(params![subscription_id.to_string()], |row| {
                Ok(crate::ical::StoredEvent {
                    subscription_id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    uid: row.get(1)?,
                    href: row.get(2)?,
                    etag: row.get(3)?,
                    ics: row.get(4)?,
                    dirty: crate::ical::EventDirty::from_i64(row.get(5)?),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Queues or updates a local CalDAV event edit (`dirty = Upsert`) and refreshes the combined feed.
    pub fn upsert_local_calendar_event(&self, subscription_id: Id, uid: &str, href: &str, ics: &str) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute(
                "INSERT INTO calendar_events (subscription_id, uid, href, etag, ics, dirty, updated_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, 1, ?5)
                 ON CONFLICT(subscription_id, uid) DO UPDATE SET
                     href = CASE WHEN excluded.href != '' THEN excluded.href ELSE calendar_events.href END,
                     ics = excluded.ics,
                     dirty = 1,
                     updated_at = excluded.updated_at",
                params![subscription_id.to_string(), uid, href, ics, now_ms()],
            )?;
            rebuild_caldav_feed(&self.conn, subscription_id)?;
            Ok(())
        })
    }

    /// Marks a CalDAV event for remote deletion (or drops it immediately if never pushed).
    pub fn mark_calendar_event_deleted(&self, subscription_id: Id, uid: &str) -> Result<bool> {
        self.in_tx(|| {
            let existing: Option<(Option<String>, i64)> = self
                .conn
                .query_row(
                    "SELECT etag, dirty FROM calendar_events WHERE subscription_id = ?1 AND uid = ?2",
                    params![subscription_id.to_string(), uid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((etag, dirty)) = existing else {
                return Ok(false);
            };
            if etag.is_none() && dirty == crate::ical::EventDirty::Upsert.to_i64() {
                self.conn.execute(
                    "DELETE FROM calendar_events WHERE subscription_id = ?1 AND uid = ?2",
                    params![subscription_id.to_string(), uid],
                )?;
            } else {
                self.conn.execute(
                    "UPDATE calendar_events SET dirty = 2, updated_at = ?3 WHERE subscription_id = ?1 AND uid = ?2",
                    params![subscription_id.to_string(), uid, now_ms()],
                )?;
            }
            rebuild_caldav_feed(&self.conn, subscription_id)?;
            Ok(true)
        })
    }

    /// Marks a dirty local event as synced cleanly after `PUT`.
    pub fn mark_caldav_event_clean(&self, subscription_id: Id, uid: &str, href: &str, etag: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE calendar_events SET href = ?3, etag = ?4, dirty = 0, updated_at = ?5
             WHERE subscription_id = ?1 AND uid = ?2 AND dirty = 1",
            params![subscription_id.to_string(), uid, href, etag, now_ms()],
        )?;
        Ok(())
    }

    /// Removes a deleted event record after remote `DELETE` succeeds.
    pub fn remove_caldav_event(&self, subscription_id: Id, uid: &str) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute(
                "DELETE FROM calendar_events WHERE subscription_id = ?1 AND uid = ?2 AND dirty = 2",
                params![subscription_id.to_string(), uid],
            )?;
            rebuild_caldav_feed(&self.conn, subscription_id)?;
            Ok(())
        })
    }

    /// Replaces clean remote CalDAV events from a remote fetch while keeping any still-dirty local edits.
    pub fn replace_caldav_snapshot(
        &self,
        subscription_id: Id,
        url: &str,
        remote: &[crate::ical::StoredEvent],
        sync_token: Option<&str>,
    ) -> Result<bool> {
        self.in_tx(|| {
            let still_matches: Option<String> = self
                .conn
                .query_row(
                    "SELECT url FROM calendar_subscriptions WHERE id = ?1",
                    params![subscription_id.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            if still_matches.as_deref() != Some(url) {
                return Ok(false);
            }

            let now = now_ms();
            self.conn.execute("DELETE FROM calendar_events WHERE subscription_id = ?1 AND dirty = 0", params![subscription_id.to_string()])?;
            for ev in remote {
                self.conn.execute(
                    "INSERT INTO calendar_events (subscription_id, uid, href, etag, ics, dirty, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)
                     ON CONFLICT(subscription_id, uid) DO UPDATE SET
                         href = excluded.href,
                         etag = excluded.etag,
                         ics = excluded.ics,
                         dirty = 0,
                         updated_at = excluded.updated_at
                     WHERE calendar_events.dirty = 0",
                    params![subscription_id.to_string(), ev.uid, ev.href, ev.etag, ev.ics, now],
                )?;
            }
            rebuild_caldav_feed(&self.conn, subscription_id)?;
            self.conn.execute(
                "UPDATE calendar_subscriptions SET fetched_at = ?2, error = NULL, sync_token = ?3 WHERE id = ?1 AND url = ?4",
                params![subscription_id.to_string(), now, sync_token, url],
            )?;
            Ok(true)
        })
    }
}

fn rebuild_caldav_feed(tx: &Connection, subscription_id: Id) -> Result<()> {
    let mut stmt = tx.prepare(
        "SELECT ics FROM calendar_events WHERE subscription_id = ?1 AND dirty != 2 ORDER BY uid",
    )?;
    let entries: Vec<String> = stmt.query_map(params![subscription_id.to_string()], |r| r.get(0))?.collect::<Result<_, _>>()?;
    drop(stmt);

    let mut combined = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Fern//Calendar//EN\r\n");
    for ics in &entries {
        let normalized = ics.replace("\r\n", "\n");
        let mut inside_component = 0usize;
        for line in normalized.lines() {
            let upper = line.to_ascii_uppercase();
            if upper == "BEGIN:VCALENDAR" || upper == "END:VCALENDAR" {
                continue;
            }
            if upper.starts_with("BEGIN:") {
                inside_component += 1;
            }
            if inside_component > 0 {
                combined.push_str(line);
                combined.push_str("\r\n");
            }
            if upper.starts_with("END:") {
                inside_component = inside_component.saturating_sub(1);
            }
        }
    }
    combined.push_str("END:VCALENDAR\r\n");

    tx.execute(
        "UPDATE calendar_subscriptions SET feed = ?2 WHERE id = ?1",
        params![subscription_id.to_string(), combined],
    )?;
    Ok(())
}
