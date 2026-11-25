use fern_core::Store;
use ui::prelude::*;

use crate::i18n;

use super::AppView;

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

impl AppView {
    /// Syncs on start, whenever there are local changes to send, and every few minutes to fetch
    /// other devices' changes. Screenshots of the debug build do not sync.
    pub(super) fn start_sync(&mut self, cx: &mut Context<Self>) {
        if desktop_common::is_dev_state() || !self.has_sync_config {
            self.sync_task = None;
            return;
        }
        let db_path = cx.try_global::<crate::DbPath>().map(|p| p.0.clone());
        self.sync_task = Some(cx.spawn(async move |this, cx| {
            let mut bg_store: Option<Store> = None;
            let mut since_pull = std::time::Duration::ZERO;
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
                                Some(s) => match s.sync_config() {
                                    Ok(None) => SyncPoll::NotConfigured,
                                    Ok(Some(_)) => {
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
                    match this.update(cx, |this, _| match this.store.sync_config() {
                        Ok(None) => SyncPoll::NotConfigured,
                        Ok(Some(_)) => {
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
                            this.sync_task = None;
                        });
                        break;
                    }
                    SyncPoll::Due(true) => {
                        since_pull = std::time::Duration::ZERO;
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

    /// One sync with the server from Settings → Sync: sends the local changes, applies the others
    /// and refreshes the views.
    pub fn sync_now(&mut self, cx: &mut Context<Self>) {
        if self.sync == crate::sync::SyncStatus::Running {
            return;
        }
        if let Some(db_path) = desktop_common::DbPath::get(cx) {
            if !self.has_sync_config {
                if self.store.sync_config().ok().flatten().is_none() {
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
                    config: fern_core::sync::SyncConfig,
                    outcome: fern_core::sync::SyncOutcome,
                },
            }
            let started_gen = self.sync_generation;
            self.set_sync_status(crate::sync::SyncStatus::Running, cx);
            let bg_sync = cx.background_spawn(async move {
                let bg_store = Store::open(&db_path)?;
                let Some(config) = bg_store.sync_config()? else {
                    return Ok(BgSyncOutcome::Off);
                };
                let request = bg_store.sync_request()?;
                let answer = crate::sync::post(&config, &request.body)?;
                if bg_store.sync_config().ok().flatten().as_ref() != Some(&config) {
                    return Ok(BgSyncOutcome::Off);
                }
                let outcome = bg_store.finish_sync_detailed(&request, &answer)?;
                Ok(BgSyncOutcome::Done { config, outcome })
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
                            this.set_sync_status(crate::sync::SyncStatus::Off, cx);
                        }
                        Ok(BgSyncOutcome::Done { config, outcome }) => {
                            if outcome.conflicts_preserved > 0 {
                                desktop_common::diagnostics::log_event(
                                    "sync",
                                    format!("preserved {} offline note conflict(s)", outcome.conflicts_preserved),
                                );
                            }
                            if outcome.applied > 0 {
                                this.reload_all(cx);
                            }
                            this.sync_files(config, cx);
                        }
                        Err(err) => this.complete_sync(Err(err), cx),
                    }
                })
                .ok();
            })
            .detach();
            return;
        }

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
                                format!("preserved {} offline note conflict(s)", outcome.conflicts_preserved),
                            );
                        }
                        if outcome.applied > 0 {
                            this.reload_all(cx);
                        }
                        this.sync_files(config, cx);
                    }
                    Err(err) => this.complete_sync(Err(err), cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// After a sync, sends the attached files the server lacks and fetches the missing ones.
    /// Statting attachments (`files_to_sync`) and HTTP transfers both run off the UI thread.
    fn sync_files(&mut self, config: fern_core::sync::SyncConfig, cx: &mut Context<Self>) {
        let dir = match crate::attachments_dir() {
            Ok(dir) => dir,
            Err(err) => {
                self.complete_sync(Err(err), cx);
                return;
            }
        };
        let transfer_config = config.clone();
        let transfer = if let Some(db_path) = desktop_common::DbPath::get(cx) {
            desktop_common::DbRunner::run(Some(db_path), &self.store, cx, move |bg_store| {
                let (upload, download) = bg_store.files_to_sync(&dir)?;
                if upload.is_empty() && download.is_empty() {
                    return Ok(None);
                }
                Ok(Some(crate::sync::transfer_files(&transfer_config, &dir, &upload, &download)))
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
                Ok(Some(crate::sync::transfer_files(&transfer_config, &dir, &upload, &download)))
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
                if this.store.sync_config().ok().flatten().as_ref() != Some(&config) {
                    this.set_sync_status(crate::sync::SyncStatus::Off, cx);
                    return;
                }
                let result = this.store.mark_files_synced(&outcome.synced).and_then(|()| {
                    if outcome.failed.is_empty() {
                        return Ok(());
                    }
                    let strings = i18n::t(cx);
                    let errors = outcome.failed.iter().map(|(name, error)| {
                        let display_name = name.get(..36)
                            .filter(|id| fern_core::Id::parse_str(id).is_ok())
                            .and_then(|_| name.get(36..))
                            .and_then(|suffix| suffix.strip_prefix('-'))
                            .unwrap_or(name);
                        let detail = if desktop_common::sync::is_incompatible_protocol(error).is_some()
                            || desktop_common::sync::is_e2ee_error(error).is_some()
                        {
                            format_sync_error(error, strings)
                        } else {
                            desktop_common::diagnostics::redact_sensitive(&crate::sync::attachment_error(error, strings))
                        };
                        strings.sync_file_failed
                            .replace("{file}", display_name)
                            .replace("{error}", &detail)
                    }).collect::<Vec<_>>().join("\n");
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
        self.has_sync_config = self.store.sync_config().ok().flatten().is_some();
        if !self.has_sync_config {
            self.sync_task = None;
            if self.sync != crate::sync::SyncStatus::Off {
                self.set_sync_status(crate::sync::SyncStatus::Off, cx);
            }
        }
        self.sync_external_all(cx);
        self.sync_title_bar_appearance(cx);
        cx.notify();
    }

    /// The sync button's tooltip: "Sync" and how the last sync went.
    pub(super) fn sync_tooltip(&self, strings: &'static i18n::Strings, cx: &App) -> String {
        let status = match &self.sync {
            crate::sync::SyncStatus::Off if self.has_sync_config => strings.sync_never.to_owned(),
            crate::sync::SyncStatus::Off => strings.sync_off.to_owned(),
            crate::sync::SyncStatus::Running => strings.sync_running.to_owned(),
            crate::sync::SyncStatus::Done(at) => strings.sync_done.replace("{time}", &crate::calendar::format_time(at.time(), strings, cx)),
            crate::sync::SyncStatus::Failed(error) => strings.sync_failed.replace("{error}", error),
        };
        format!("{}\n{status}", strings.sync)
    }
}
