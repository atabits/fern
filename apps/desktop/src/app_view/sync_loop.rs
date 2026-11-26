use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use desktop_common::sync::{CloudSyncSnapshot, LiveCloudSnapshot, RealtimeStatus};
use fern_core::Store;
use fern_core::cloud::CloudSessionState;
use fern_core::sync::SyncMode;
use futures::StreamExt as _;
use ui::prelude::*;

use crate::i18n;

use super::AppView;

const CLOUD_PUSH_DEBOUNCE: Duration = Duration::from_secs(2);

pub(super) fn format_sync_error(err: &anyhow::Error, strings: &i18n::Strings) -> String {
    if let Some(mismatch) = desktop_common::sync::is_incompatible_protocol(err) {
        return strings
            .sync_incompatible_protocol
            .replace("{client}", &mismatch.client.to_string())
            .replace("{server}", &mismatch.server.to_string());
    }
    if let Some(e2ee) = desktop_common::sync::is_e2ee_error(err) {
        return match e2ee {
            fern_core::e2ee::E2eeError::PassphraseRequired => {
                strings.sync_e2ee_passphrase_required.to_owned()
            }
            fern_core::e2ee::E2eeError::WrongPassphrase => {
                strings.sync_e2ee_wrong_passphrase.to_owned()
            }
            fern_core::e2ee::E2eeError::Malformed(_) => {
                desktop_common::diagnostics::redact_sensitive(&format!("{err:#}"))
            }
        };
    }
    desktop_common::diagnostics::redact_sensitive(&format!("{err:#}"))
}

enum SyncTarget {
    Cloud(Box<CloudSessionState>),
    SelfHosted(fern_core::sync::SyncConfig),
}

enum SseSignal {
    Status(RealtimeStatus),
    RemoteChange(fern_core::cloud::SyncEventNotification),
    SessionExpired(String),
}

/// Computes `base_secs + uniform_random(0..base_secs)` in milliseconds using CSPRNG bytes from
/// UUIDv7 so reconnecting clients never synchronize into a thundering herd (Zed pattern).
fn jittered_backoff_ms(base_secs: u64) -> u64 {
    let base_ms = base_secs.saturating_mul(1000).max(500);
    let id = fern_core::Id::now_v7();
    let b = id.as_bytes();
    let rnd = u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]);
    base_ms.saturating_add(rnd % base_ms)
}

impl AppView {
    pub(super) fn refresh_cloud_snapshot_from_store(&self, cx: &mut Context<Self>) {
        let prev = cx
            .try_global::<LiveCloudSnapshot>()
            .map(|g| g.0.clone())
            .unwrap_or_default();
        let next = match self.store.cloud_meta().ok().flatten() {
            Some(meta) => {
                let mut snap = CloudSyncSnapshot::from_meta(&meta);
                if prev.signed_in && prev.user_id == snap.user_id {
                    snap.realtime = prev.realtime;
                    snap.storage_used_bytes = prev.storage_used_bytes;
                    snap.storage_quota_bytes = prev.storage_quota_bytes;
                    snap.synced_rows_count = prev.synced_rows_count;
                    snap.max_rows_quota = prev.max_rows_quota;
                    snap.device_count = prev.device_count;
                }
                snap
            }
            None => CloudSyncSnapshot::default(),
        };
        cx.set_global(LiveCloudSnapshot(next));
    }

    pub(super) fn set_cloud_realtime_status(&mut self, realtime: RealtimeStatus, cx: &mut Context<Self>) {
        let mut snap = cx
            .try_global::<LiveCloudSnapshot>()
            .map(|g| g.0.clone())
            .unwrap_or_default();
        if snap.realtime != realtime {
            snap.realtime = realtime;
            cx.set_global(LiveCloudSnapshot(snap));
            cx.notify();
        }
    }

    pub(super) fn schedule_cloud_push_debounce(&mut self, cx: &mut Context<Self>) {
        if desktop_common::is_dev_state() {
            return;
        }
        let is_cloud = cx
            .try_global::<LiveCloudSnapshot>()
            .is_some_and(|g| g.0.signed_in);
        if !is_cloud {
            return;
        }
        self.cloud_push_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CLOUD_PUSH_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                if this.store.has_unsent_changes().unwrap_or(false) {
                    this.sync_now(cx);
                }
            });
        }));
    }

    pub(super) fn stop_cloud_realtime(&mut self, cx: &mut Context<Self>) {
        if let Some(flag) = self.cloud_sse_stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.cloud_sse_task = None;
        self.set_cloud_realtime_status(RealtimeStatus::Disconnected, cx);
    }

    /// Handles permanent cloud revocation (`401` on refresh or SSE `event: revoked`): stops the
    /// SSE reconnect loop (mirroring Zed's `request_sign_out()`), clears the dead local vault,
    /// and surfaces a clear `SessionExpired` status instead of hammering the server forever.
    pub(super) fn handle_cloud_session_expired(&mut self, message: String, cx: &mut Context<Self>) {
        if let Some(flag) = self.cloud_sse_stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.cloud_sse_task = None;
        self.set_cloud_realtime_status(RealtimeStatus::SessionExpired, cx);
        self.set_sync_status(crate::sync::SyncStatus::Failed(message), cx);
        cx.background_spawn(async move {
            let _ = desktop_sync::cloud::clear_local_session();
        })
        .detach();
    }

    /// Maintains a lightweight Server-Sent Events push stream while signed into Fern Cloud so
    /// edits made on Web or another Desktop appear in sub-second latency.
    pub(super) fn start_cloud_realtime(&mut self, cx: &mut Context<Self>) {
        self.stop_cloud_realtime(cx);
        if desktop_common::is_dev_state() {
            return;
        }
        let Some(meta) = self.store.cloud_meta().ok().flatten() else {
            return;
        };
        let my_device_id = self.store.sync_device().unwrap_or_default();
        let stop_flag = Arc::new(AtomicBool::new(false));
        self.cloud_sse_stop = Some(Arc::clone(&stop_flag));
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<SseSignal>();

        let thread_stop = Arc::clone(&stop_flag);
        let expected_user_id = meta.user_id;
        std::thread::Builder::new()
            .name("fern-cloud-sse".into())
            .spawn(move || {
                let mut backoff_secs = 2u64;
                while !thread_stop.load(Ordering::Relaxed) {
                    let _ = tx.unbounded_send(SseSignal::Status(RealtimeStatus::Connecting));
                    let ticket_res =
                        desktop_sync::cloud::request_events_ticket_current(&expected_user_id);

                    let mut rate_limit_wait: Option<u64> = None;
                    match ticket_res {
                        Ok((server_url, ticket)) => {
                            let connected_tx = tx.clone();
                            let event_tx = tx.clone();
                            let became_live = AtomicBool::new(false);
                            let stream_res = desktop_sync::cloud::stream_events_once(
                                &server_url,
                                &ticket,
                                &thread_stop,
                                || {
                                    became_live.store(true, Ordering::Relaxed);
                                    let _ = connected_tx
                                        .unbounded_send(SseSignal::Status(RealtimeStatus::Live));
                                },
                                |notif| {
                                    let _ = event_tx.unbounded_send(SseSignal::RemoteChange(notif));
                                },
                            );
                            if became_live.load(Ordering::Relaxed) {
                                backoff_secs = 2;
                            }
                            if let Err(err) = stream_res {
                                if desktop_common::sync::is_session_expired(&err) {
                                    let _ = desktop_sync::cloud::clear_local_session();
                                    let _ = tx.unbounded_send(SseSignal::SessionExpired(
                                        desktop_common::diagnostics::redact_sensitive(&format!(
                                            "{err:#}"
                                        )),
                                    ));
                                    break;
                                }
                                rate_limit_wait = desktop_common::sync::rate_limit_retry_after(&err);
                            }
                        }
                        Err(err) => {
                            if desktop_common::sync::is_session_expired(&err) {
                                let _ = desktop_sync::cloud::clear_local_session();
                                let _ = tx.unbounded_send(SseSignal::SessionExpired(
                                    desktop_common::diagnostics::redact_sensitive(&format!(
                                        "{err:#}"
                                    )),
                                ));
                                break;
                            }
                            rate_limit_wait = desktop_common::sync::rate_limit_retry_after(&err);
                            desktop_common::diagnostics::log_event(
                                "cloud",
                                desktop_common::diagnostics::redact_sensitive(&format!(
                                    "realtime stream connect failed: {err:#}"
                                )),
                            );
                        }
                    }

                    if thread_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = tx.unbounded_send(SseSignal::Status(RealtimeStatus::Disconnected));
                    let wait_ms = match rate_limit_wait {
                        Some(secs) => secs.max(backoff_secs).saturating_mul(1000),
                        None => jittered_backoff_ms(backoff_secs),
                    };
                    let ticks = (wait_ms / 200).max(1);
                    for _ in 0..ticks {
                        if thread_stop.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    backoff_secs = (backoff_secs * 2).min(30);
                }
            })
            .ok();

        self.cloud_sse_task = Some(cx.spawn(async move |this, cx| {
            while let Some(signal) = rx.next().await {
                let alive = this
                    .update(cx, |this, cx| match signal {
                        SseSignal::Status(status) => this.set_cloud_realtime_status(status, cx),
                        SseSignal::RemoteChange(notif) => {
                            if notif.from_device != my_device_id {
                                let seen = this.store.sync_seen_position().unwrap_or(0);
                                if notif.position > seen {
                                    this.sync_now(cx);
                                }
                            }
                        }
                        SseSignal::SessionExpired(msg) => {
                            this.handle_cloud_session_expired(msg, cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    /// Syncs on start, whenever there are local changes to send, and every few minutes to fetch
    /// other devices' changes. Screenshots of the debug build do not sync.
    pub(super) fn start_sync(&mut self, cx: &mut Context<Self>) {
        self.refresh_cloud_snapshot_from_store(cx);
        if desktop_common::is_dev_state() || !self.has_sync_config {
            self.sync_task = None;
            self.stop_cloud_realtime(cx);
            return;
        }
        if self.store.cloud_meta().ok().flatten().is_some() {
            self.start_cloud_realtime(cx);
        } else {
            self.stop_cloud_realtime(cx);
        }
        let db_path = cx.try_global::<crate::DbPath>().map(|p| p.0.clone());
        self.sync_task = Some(cx.spawn(async move |this, cx| {
            let mut bg_store: Option<Store> = None;
            let mut since_pull = Duration::ZERO;
            let mut first = true;
            loop {
                if this.upgrade().is_none() {
                    break;
                }
                enum SyncPoll {
                    NotConfigured,
                    Due(bool),
                }
                let force_due = first || since_pull >= crate::sync::PULL_EVERY;
                let poll = if let Some(path) = db_path.clone() {
                    let taken_store = bg_store.take();
                    let (returned_store, poll) = cx
                        .background_executor()
                        .spawn(async move {
                            let bg = taken_store.or_else(|| Store::open(&path).ok());
                            let poll = match bg.as_ref() {
                                Some(s) => match s.sync_mode() {
                                    Ok(SyncMode::Off) => SyncPoll::NotConfigured,
                                    Ok(SyncMode::Cloud(_) | SyncMode::SelfHosted(_)) => {
                                        let due = force_due || s.has_unsent_changes().unwrap_or(false);
                                        SyncPoll::Due(due)
                                    }
                                    Err(_) => SyncPoll::Due(force_due),
                                },
                                None => SyncPoll::Due(force_due),
                            };
                            (bg, poll)
                        })
                        .await;
                    bg_store = returned_store;
                    poll
                } else {
                    match this.update(cx, |this, _| match this.store.sync_mode() {
                        Ok(SyncMode::Off) => SyncPoll::NotConfigured,
                        Ok(SyncMode::Cloud(_) | SyncMode::SelfHosted(_)) => {
                            SyncPoll::Due(force_due || this.store.has_unsent_changes().unwrap_or(false))
                        }
                        Err(_) => SyncPoll::Due(force_due),
                    }) {
                        Ok(poll) => poll,
                        Err(_) => break,
                    }
                };
                match poll {
                    SyncPoll::NotConfigured => {
                        let _ = this.update(cx, |this, cx| {
                            this.has_sync_config = false;
                            if this.sync != crate::sync::SyncStatus::Off {
                                this.set_sync_status(crate::sync::SyncStatus::Off, cx);
                            }
                            this.stop_cloud_realtime(cx);
                            this.sync_task = None;
                        });
                        break;
                    }
                    SyncPoll::Due(true) => {
                        since_pull = Duration::ZERO;
                        if this.update(cx, |this, cx| this.sync_now(cx)).is_err() {
                            break;
                        }
                    }
                    SyncPoll::Due(false) => {}
                }
                first = false;
                cx.background_executor().timer(crate::sync::PUSH_CHECK_EVERY).await;
                since_pull += crate::sync::PUSH_CHECK_EVERY;
            }
        }));
    }

    /// One sync with Fern Cloud or self-hosted `fern-server`: sends local changes, applies
    /// remote changes, transfers E2EE attachments, and refreshes active views.
    pub fn sync_now(&mut self, cx: &mut Context<Self>) {
        if self.sync == crate::sync::SyncStatus::Running {
            return;
        }
        if let Some(db_path) = desktop_common::DbPath::get(cx) {
            if !self.has_sync_config {
                let configured = !matches!(self.store.sync_mode(), Ok(SyncMode::Off) | Err(_));
                if !configured {
                    if self.sync != crate::sync::SyncStatus::Off {
                        self.set_sync_status(crate::sync::SyncStatus::Off, cx);
                    }
                    return;
                }
                self.has_sync_config = true;
            }
            enum BgSyncOutcome {
                Off,
                Done {
                    target: SyncTarget,
                    outcome: fern_core::sync::SyncOutcome,
                },
            }
            let started_gen = self.sync_generation;
            self.set_sync_status(crate::sync::SyncStatus::Running, cx);
            let bg_sync = cx.background_spawn(async move {
                let bg_store = Store::open(&db_path)?;
                match bg_store.sync_mode()? {
                    SyncMode::Off => Ok(BgSyncOutcome::Off),
                    SyncMode::Cloud(meta) => {
                        let session = desktop_sync::cloud::load_current_session(&meta.user_id)?;
                        let e2ee_key = session.e2ee_key()?;
                        let request = bg_store.sync_request_with_key(Some(&e2ee_key))?;
                        let (answer, latest_session) =
                            desktop_sync::cloud::post_sync_current(&meta.user_id, &request.body)?;
                        if bg_store.cloud_meta().ok().flatten().as_ref() != Some(&meta) {
                            return Ok(BgSyncOutcome::Off);
                        }
                        let outcome = bg_store.finish_sync_detailed_with_key(
                            &request,
                            &answer,
                            Some(&e2ee_key),
                        )?;
                        Ok(BgSyncOutcome::Done {
                            target: SyncTarget::Cloud(Box::new(latest_session)),
                            outcome,
                        })
                    }
                    SyncMode::SelfHosted(config) => {
                        let request = bg_store.sync_request()?;
                        let answer = crate::sync::post(&config, &request.body)?;
                        if bg_store.sync_config().ok().flatten().as_ref() != Some(&config) {
                            return Ok(BgSyncOutcome::Off);
                        }
                        let outcome = bg_store.finish_sync_detailed(&request, &answer)?;
                        Ok(BgSyncOutcome::Done {
                            target: SyncTarget::SelfHosted(config),
                            outcome,
                        })
                    }
                }
            });
            cx.spawn(async move |this, cx| {
                let result: anyhow::Result<BgSyncOutcome> = bg_sync.await;
                this.update(cx, |this, cx| {
                    if this.sync_generation != started_gen {
                        this.set_sync_status(crate::sync::SyncStatus::Off, cx);
                        if this.has_sync_config {
                            this.sync_now(cx);
                        }
                        return;
                    }
                    if !this.has_sync_config {
                        this.set_sync_status(crate::sync::SyncStatus::Off, cx);
                        return;
                    }
                    match result {
                        Ok(BgSyncOutcome::Off) => {
                            this.has_sync_config = false;
                            this.sync_task = None;
                            this.stop_cloud_realtime(cx);
                            this.set_sync_status(crate::sync::SyncStatus::Off, cx);
                        }
                        Ok(BgSyncOutcome::Done { target, outcome }) => {
                            if outcome.conflicts_preserved > 0 {
                                desktop_common::diagnostics::log_event(
                                    "sync",
                                    format!(
                                        "preserved {} offline note conflict(s)",
                                        outcome.conflicts_preserved
                                    ),
                                );
                            }
                            if outcome.applied > 0 {
                                this.reload_all(cx);
                            }
                            this.sync_files(target, cx);
                        }
                        Err(err) => {
                            if desktop_common::sync::is_session_expired(&err) {
                                let msg = format_sync_error(&err, i18n::t(cx));
                                this.handle_cloud_session_expired(msg, cx);
                            } else {
                                this.complete_sync(Err(err), cx);
                            }
                        }
                    }
                })
                .ok();
            })
            .detach();
            return;
        }

        // In-memory store path (used by unit/GPUI tests).
        let config = match self.store.sync_config() {
            Ok(Some(config)) => config,
            Ok(None) => {
                self.set_sync_status(crate::sync::SyncStatus::Off, cx);
                return;
            }
            Err(err) => {
                self.complete_sync(Err(err), cx);
                return;
            }
        };
        let request = match self.store.sync_request() {
            Ok(request) => request,
            Err(err) => {
                self.complete_sync(Err(err), cx);
                return;
            }
        };
        self.set_sync_status(crate::sync::SyncStatus::Running, cx);
        let body = request.body.clone();
        let sent_config = config.clone();
        let answer = cx.background_spawn(async move { crate::sync::post(&sent_config, &body) });
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| {
                if this.store.sync_config().ok().flatten().as_ref() != Some(&config) {
                    this.set_sync_status(crate::sync::SyncStatus::Off, cx);
                    return;
                }
                match answer.and_then(|answer| this.store.finish_sync_detailed(&request, &answer)) {
                    Ok(outcome) => {
                        if outcome.conflicts_preserved > 0 {
                            desktop_common::diagnostics::log_event(
                                "sync",
                                format!(
                                    "preserved {} offline note conflict(s)",
                                    outcome.conflicts_preserved
                                ),
                            );
                        }
                        if outcome.applied > 0 {
                            this.reload_all(cx);
                        }
                        this.sync_files(SyncTarget::SelfHosted(config), cx);
                    }
                    Err(err) => this.complete_sync(Err(err), cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// After a sync, sends attached files the server lacks and fetches missing ones.
    fn sync_files(&mut self, target: SyncTarget, cx: &mut Context<Self>) {
        let dir = match crate::attachments_dir() {
            Ok(dir) => dir,
            Err(err) => {
                self.complete_sync(Err(err), cx);
                return;
            }
        };
        let check_mode = match &target {
            SyncTarget::Cloud(s) => SyncMode::Cloud(s.meta()),
            SyncTarget::SelfHosted(c) => SyncMode::SelfHosted(c.clone()),
        };
        let transfer = if let Some(db_path) = desktop_common::DbPath::get(cx) {
            desktop_common::DbRunner::run(Some(db_path), &self.store, cx, move |bg_store| {
                let (upload, download) = bg_store.files_to_sync(&dir)?;
                if upload.is_empty() && download.is_empty() {
                    return Ok(None);
                }
                match target {
                    SyncTarget::Cloud(session) => {
                        let outcome = desktop_sync::cloud::transfer_files_current(
                            &session.user_id,
                            &dir,
                            &upload,
                            &download,
                        );
                        Ok(Some(outcome))
                    }
                    SyncTarget::SelfHosted(config) => {
                        Ok(Some(crate::sync::transfer_files(&config, &dir, &upload, &download)))
                    }
                }
            })
        } else {
            let (upload, download) = match self.store.files_to_sync(&dir) {
                Ok(files) => files,
                Err(err) => {
                    self.complete_sync(Err(err), cx);
                    return;
                }
            };
            if upload.is_empty() && download.is_empty() {
                self.complete_sync(Ok(()), cx);
                return;
            }
            cx.background_spawn(async move {
                match target {
                    SyncTarget::Cloud(session) => {
                        let outcome = desktop_sync::cloud::transfer_files_current(
                            &session.user_id,
                            &dir,
                            &upload,
                            &download,
                        );
                        Ok(Some(outcome))
                    }
                    SyncTarget::SelfHosted(config) => {
                        Ok(Some(crate::sync::transfer_files(&config, &dir, &upload, &download)))
                    }
                }
            })
        };
        cx.spawn(async move |this, cx| {
            let outcome = transfer.await;
            this.update(cx, |this, cx| {
                let outcome = match outcome {
                    Ok(Some(outcome)) => outcome,
                    Ok(None) => {
                        this.complete_sync(Ok(()), cx);
                        return;
                    }
                    Err(err) => {
                        this.complete_sync(Err(err), cx);
                        return;
                    }
                };
                if outcome.downloaded {
                    this.reload_all(cx);
                }
                let still_matches = match (&check_mode, this.store.sync_mode().ok()) {
                    (SyncMode::Cloud(a), Some(SyncMode::Cloud(b))) => a.user_id == b.user_id && a.server_url == b.server_url,
                    (SyncMode::SelfHosted(a), Some(SyncMode::SelfHosted(b))) => a == &b,
                    _ => false,
                };
                if !still_matches {
                    this.set_sync_status(crate::sync::SyncStatus::Off, cx);
                    return;
                }
                let result = this.store.mark_files_synced(&outcome.synced).and_then(|()| {
                    if outcome.failed.is_empty() {
                        return Ok(());
                    }
                    let strings = i18n::t(cx);
                    let errors = outcome
                        .failed
                        .iter()
                        .map(|(name, error)| {
                            let display_name = name
                                .get(..36)
                                .filter(|id| fern_core::Id::parse_str(id).is_ok())
                                .and_then(|_| name.get(36..))
                                .and_then(|suffix| suffix.strip_prefix('-'))
                                .unwrap_or(name);
                            let detail = if desktop_common::sync::is_incompatible_protocol(error)
                                .is_some()
                                || desktop_common::sync::is_e2ee_error(error).is_some()
                            {
                                format_sync_error(error, strings)
                            } else {
                                desktop_common::diagnostics::redact_sensitive(
                                    &crate::sync::attachment_error(error, strings),
                                )
                            };
                            strings
                                .sync_file_failed
                                .replace("{file}", display_name)
                                .replace("{error}", &detail)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Err(anyhow::anyhow!(errors))
                });
                this.complete_sync(result, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn set_sync_status(&mut self, status: crate::sync::SyncStatus, cx: &mut Context<Self>) {
        self.sync = status.clone();
        cx.set_global(crate::LiveSyncStatus(status));
        cx.notify();
    }

    pub(super) fn complete_sync(&mut self, result: anyhow::Result<()>, cx: &mut Context<Self>) {
        let status = match result {
            Ok(()) => crate::sync::SyncStatus::Done(chrono::Local::now().naive_local()),
            Err(err) => {
                desktop_common::diagnostics::log_event(
                    "sync",
                    desktop_common::diagnostics::redact_sensitive(&format!("sync failed: {err:#}")),
                );
                crate::sync::SyncStatus::Failed(format_sync_error(&err, i18n::t(cx)))
            }
        };
        self.set_sync_status(status, cx);
    }

    /// Re-reads every view, after changes came from another device.
    pub(super) fn reload_all(&mut self, cx: &mut Context<Self>) {
        let prefs = self.store.preferences().unwrap_or_default();
        desktop_common::ActivePreferences::sync_from_preferences(
            &prefs,
            desktop_common::sound::enabled(&self.store),
            cx,
        );
        self.features = prefs.features;
        self.rail_items = prefs.rail_items;
        self.has_sync_config = !matches!(self.store.sync_mode(), Ok(SyncMode::Off) | Err(_));
        self.refresh_cloud_snapshot_from_store(cx);
        if !self.has_sync_config {
            self.sync_task = None;
            self.stop_cloud_realtime(cx);
            if self.sync != crate::sync::SyncStatus::Off {
                self.set_sync_status(crate::sync::SyncStatus::Off, cx);
            }
        }
        self.sync_external_all(cx);
        self.sync_title_bar_appearance(cx);
        cx.notify();
    }

    /// The sync button's tooltip: "Sync" (or "Fern Cloud (E2EE)") and how the last sync went.
    pub(super) fn sync_tooltip(&self, strings: &'static i18n::Strings, cx: &App) -> String {
        let status = match &self.sync {
            crate::sync::SyncStatus::Off if self.has_sync_config => strings.sync_never.to_owned(),
            crate::sync::SyncStatus::Off => strings.sync_off.to_owned(),
            crate::sync::SyncStatus::Running => strings.sync_running.to_owned(),
            crate::sync::SyncStatus::Done(at) => strings
                .sync_done
                .replace("{time}", &crate::calendar::format_time(at.time(), strings, cx)),
            crate::sync::SyncStatus::Failed(error) => {
                strings.sync_failed.replace("{error}", error)
            }
        };
        if let Some(cloud) = cx
            .try_global::<LiveCloudSnapshot>()
            .map(|g| &g.0)
            .filter(|s| s.signed_in)
        {
            let rt = match cloud.realtime {
                RealtimeStatus::Live => strings.cloud_live_badge,
                RealtimeStatus::Connecting => strings.cloud_connecting_badge,
                RealtimeStatus::Disconnected => strings.cloud_offline_badge,
                RealtimeStatus::SessionExpired => strings.cloud_offline_badge,
            };
            format!("{} ({}) · {rt}\n{status}", strings.cloud_title, strings.cloud_e2ee_badge)
        } else {
            format!("{}\n{status}", strings.sync)
        }
    }
}
