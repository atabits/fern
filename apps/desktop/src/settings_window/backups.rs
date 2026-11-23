use std::path::{Path, PathBuf};

use fern_core::Store;
use fern_core::import::ImportSummary;
use ui::prelude::*;

use crate::i18n;
use crate::release_notes;

use super::{SettingsEvent, SettingsWindow};

fn write_temp_backup(store: &Store) -> anyhow::Result<PathBuf> {
    let dir = crate::scratch_dir()?;
    let temp = dir.join(format!("fern-backup-{}.db", std::process::id()));
    let attachments = crate::attachments_dir().ok();
    store.backup_to(&temp, attachments.as_deref())?;
    Ok(temp)
}

fn read_and_remove_backup(temp: &Path) -> anyhow::Result<Vec<u8>> {
    let bytes = std::fs::read(temp);
    std::fs::remove_file(temp).ok();
    Ok(bytes?)
}

pub(super) fn format_ticktick_import(
    imported: anyhow::Result<ImportSummary>,
    strings: &i18n::Strings,
) -> String {
    match imported {
        Ok(summary) => strings
            .imported_from_ticktick
            .replace("{tasks}", &summary.tasks.to_string())
            .replace("{lists}", &summary.lists.to_string()),
        Err(err) => format!("{err:#}"),
    }
}

fn build_diagnostic_report(
    store: &Store,
    locale: &str,
    db_path: Option<&Path>,
    backups_dir: Option<&Path>,
    sync_status: &crate::sync::SyncStatus,
    db_health: Option<&desktop_common::diagnostics::DbHealth>,
) -> String {
    let input = desktop_common::diagnostics::DiagnosticInput {
        app_version: release_notes::CURRENT_VERSION,
        locale,
        db_path,
        backups_dir,
        sync_status,
        db_health,
    };
    desktop_common::diagnostics::build_diagnostic_report(store, &input)
}

impl SettingsWindow {
    /// "Generate Backup": all data in one file, saved where the user chooses.
    /// Runs SQLite backup + attachment bundling off the UI thread when backed by an on-disk DB.
    pub(super) fn generate_backup(&mut self, cx: &mut Context<Self>) {
        let read = if let Some(db_path) = cx.try_global::<crate::DbPath>().map(|p| p.0.clone()) {
            cx.background_spawn(async move {
                let bg_store = Store::open(&db_path)?;
                let temp = write_temp_backup(&bg_store)?;
                read_and_remove_backup(&temp)
            })
        } else {
            let temp = match write_temp_backup(&self.store) {
                Ok(temp) => temp,
                Err(err) => {
                    self.backup_message = Some(err.to_string());
                    cx.notify();
                    return;
                }
            };
            cx.background_spawn(async move { read_and_remove_backup(&temp) })
        };
        cx.spawn(async move |this, cx| {
            let bytes = read.await;
            this.update(cx, |this, cx| match bytes {
                Ok(bytes) => {
                    let name = format!("fern-backup-{}.db", chrono::Local::now().format("%Y-%m-%d"));
                    crate::export::save_as(name, bytes, cx, |this, result, cx| {
                        this.backup_message = crate::export::outcome(result, i18n::t(cx));
                        this.refresh_auto_backups(cx);
                        cx.notify();
                    });
                }
                Err(err) => {
                    this.backup_message = Some(err.to_string());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn finish_ticktick_import(
        &mut self,
        imported: anyhow::Result<ImportSummary>,
        cx: &mut Context<Self>,
    ) {
        self.backup_message = Some(format_ticktick_import(imported, i18n::t(cx)));
        self.refresh_cached_collections(cx);
        cx.emit(SettingsEvent::Changed);
        crate::sticky_note::data_changed(cx.entity_id(), cx);
        cx.notify();
    }

    /// "Import Backups": a Fern backup is checked now and restored on the next start; a TickTick
    /// backup (its CSV file) is parsed and imported off the UI thread.
    pub(super) fn import_backup(&mut self, cx: &mut Context<Self>) {
        let db_path = cx.try_global::<crate::DbPath>().map(|p| p.0.clone());
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Some(path) = chosen
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths| paths.into_iter().next())
            else {
                return;
            };
            if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("csv")) {
                if let Some(db_path) = db_path {
                    let imported = cx
                        .background_spawn(async move {
                            let text = std::fs::read_to_string(&path)?;
                            let bg_store = Store::open(&db_path)?;
                            bg_store.import_ticktick(&text)
                        })
                        .await;
                    this.update(cx, |this, cx| this.finish_ticktick_import(imported, cx))
                        .ok();
                    return;
                }
                let text = cx.background_spawn(async move { std::fs::read_to_string(&path) }).await;
                this.update(cx, |this, cx| {
                    let imported = text
                        .map_err(anyhow::Error::from)
                        .and_then(|text| this.store.import_ticktick(&text));
                    this.finish_ticktick_import(imported, cx);
                })
                .ok();
                return;
            }
            this.update(cx, |this, cx| {
                this.pending_restore = Some(path);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn refresh_auto_backups(&mut self, cx: &mut Context<Self>) {
        #[cfg(test)]
        let dir = self.backups_dir_override.clone();
        #[cfg(not(test))]
        let dir = crate::backups_dir().ok();
        let Some(dir) = dir else {
            self.auto_backups.clear();
            cx.notify();
            return;
        };
        let task = cx.background_spawn(async move { fern_core::store::list_backups(&dir) });
        self.auto_backups_task = Some(cx.spawn(async move |this, cx| {
            let backups = task.await;
            this.update(cx, |this, cx| {
                this.auto_backups = backups;
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn confirm_restore(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.pending_restore.take() else {
            return;
        };
        cx.notify();
        self.stage_restore(path, cx);
    }

    fn stage_restore(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        #[cfg(test)]
        let target_override = self.restore_target_override.clone();
        let outcome = cx.background_spawn(async move {
            fern_core::Store::check_backup(&path)?;
            #[cfg(test)]
            let target = match target_override {
                Some(target) => target,
                None => crate::pending_restore_path()?,
            };
            #[cfg(not(test))]
            let target = crate::pending_restore_path()?;
            std::fs::copy(&path, &target)?;
            Ok::<(), anyhow::Error>(())
        });
        cx.spawn(async move |this, cx| {
            let outcome = outcome.await;
            this.update(cx, |this, cx| {
                let strings = i18n::t(cx);
                this.backup_message = Some(match outcome {
                    Ok(()) => {
                        desktop_common::diagnostics::log_event(
                            "db",
                            "staged backup restore for next launch",
                        );
                        strings.restore_on_restart.to_owned()
                    }
                    Err(err) => {
                        desktop_common::diagnostics::log_event(
                            "db",
                            format!("failed to stage backup restore: {err:#}"),
                        );
                        format!("{err}")
                    }
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn save_diagnostic_log(&mut self, cx: &mut Context<Self>) {
        let default_dir = crate::export::default_dir();
        let filename = format!(
            "fern-diagnostics-{}.txt",
            chrono::Local::now().format("%Y-%m-%d")
        );
        let chosen = cx.prompt_for_new_path(&default_dir, Some(&filename));
        let db_path = cx.try_global::<crate::DbPath>().map(|p| p.0.clone());
        #[cfg(test)]
        let backups_dir = self.backups_dir_override.clone();
        #[cfg(not(test))]
        let backups_dir = crate::backups_dir().ok();
        let db_health = cx.try_global::<desktop_common::diagnostics::DbHealth>().cloned();
        let sync_status = cx
            .try_global::<crate::LiveSyncStatus>()
            .map(|s| s.0.clone())
            .unwrap_or_default();
        let locale_code = i18n::locale(cx).code().to_owned();

        cx.spawn(async move |this, cx| {
            let Some(path) = chosen.await.ok().and_then(Result::ok).flatten() else {
                return;
            };
            let result: anyhow::Result<PathBuf> = if let Some(db_path) = db_path {
                cx.background_spawn(async move {
                    let bg_store = Store::open(&db_path)?;
                    let report = build_diagnostic_report(
                        &bg_store,
                        &locale_code,
                        Some(&db_path),
                        backups_dir.as_deref(),
                        &sync_status,
                        db_health.as_ref(),
                    );
                    std::fs::write(&path, report.as_bytes())?;
                    Ok(path)
                })
                .await
            } else {
                let report = match this.update(cx, |this, _| {
                    build_diagnostic_report(
                        &this.store,
                        &locale_code,
                        None,
                        backups_dir.as_deref(),
                        &sync_status,
                        db_health.as_ref(),
                    )
                }) {
                    Ok(report) => report,
                    Err(_) => return,
                };
                cx.background_spawn(async move {
                    std::fs::write(&path, report.as_bytes())?;
                    Ok(path)
                })
                .await
            };
            this.update(cx, |this, cx| {
                let strings = i18n::t(cx);
                match result {
                    Ok(saved_path) => {
                        this.diagnostic_message = Some((
                            strings
                                .diagnostic_log_saved
                                .replace("{path}", &saved_path.display().to_string()),
                            false,
                        ));
                    }
                    Err(err) => {
                        desktop_common::diagnostics::log_event(
                            "diagnostics",
                            format!("failed to save diagnostic log: {err:#}"),
                        );
                        let redacted =
                            desktop_common::diagnostics::redact_sensitive(&format!("{err:#}"));
                        this.diagnostic_message = Some((
                            strings.diagnostic_log_failed.replace("{error}", &redacted),
                            true,
                        ));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
