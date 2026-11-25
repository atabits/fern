use std::path::PathBuf;

use fern_core::plugin_store::PluginState;
use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement, PathPromptOptions, Styled, div, px,
};
use icons::IconName;
use plugins::registry::RegistryClient;
use plugins::{Permission, PermissionApprovalStatus, PluginStatus, RegistrySource};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{Button, ButtonVariant, Color, ConfirmDialog, Icon, IconSize, Label, LabelSize, Modal};
use uuid::Uuid;

use super::PluginSettingsView;
use crate::host::PluginHost;

#[derive(Clone)]
pub(super) struct PendingZipOverwrite {
    pub(super) zip_path: PathBuf,
    pub(super) existing_state: Option<PluginState>,
}

#[derive(Clone)]
pub(super) struct PermissionPromptState {
    pub(super) plugin_id: String,
    pub(super) plugin_name: String,
    pub(super) version: String,
    pub(super) added_permissions: Vec<Permission>,
    pub(super) is_first_install: bool,
    pub(super) pending_zip_overwrite: Option<PendingZipOverwrite>,
}

impl PluginSettingsView {
    pub(super) fn install_from_zip_dialog(&mut self, cx: &mut Context<Self>) {
        if PluginHost::global(cx).is_none() {
            return;
        }
        let chosen = cx.prompt_for_paths(PathPromptOptions {
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
            let _ = this.update(cx, |this, cx| {
                this.install_from_zip_path(path, cx);
            });
        })
        .detach();
    }

    /// Validates a local `.zip` archive in an isolated staging directory before touching
    /// `plugins_dir/<id>`. Any overwrite or manifest hash change requires explicit re-approval
    /// before swapping files or running code.
    pub(crate) fn install_from_zip_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some((client, plugins_dir)) = PluginHost::global(cx)
            .map(|h| (h.registry_client().clone(), h.plugins_dir().clone()))
        else {
            return;
        };
        let mgr = PluginHost::global(cx).and_then(|h| h.manager().cloned());
        let path_for_inspect = path.clone();
        let plugins_dir_inspect = plugins_dir.clone();

        cx.spawn(async move |this, cx| {
            // Stage-inspect the zip in an isolated temp dir so we know its manifest ID, hash,
            // and permissions BEFORE overwriting `plugins_dir/<id>`.
            let preview_res = cx
                .background_spawn(async move {
                    std::fs::create_dir_all(&plugins_dir_inspect)?;
                    let inspect_dir = plugins_dir_inspect
                        .join(format!(".zip-inspect-{}", Uuid::now_v7()));
                    std::fs::create_dir_all(&inspect_dir)?;
                    struct InspectCleanup(PathBuf);
                    impl Drop for InspectCleanup {
                        fn drop(&mut self) {
                            let _ = std::fs::remove_dir_all(&self.0);
                        }
                    }
                    let _cleanup = InspectCleanup(inspect_dir.clone());
                    let staging_client = RegistryClient::new(&inspect_dir, &inspect_dir);
                    staging_client.install_from_zip_file(&path_for_inspect, None)
                })
                .await;

            let preview = match preview_res {
                Ok(out) => out,
                Err(err) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status_banner = Some((format!("{err:#}"), true));
                        cx.notify();
                    });
                    return;
                }
            };

            let decision = this.update(cx, |this, cx| {
                let existing_state = this.store.plugin_state(&preview.manifest.id).ok().flatten();
                let target_exists = plugins_dir.join(&preview.manifest.id).exists();
                let is_overwrite = target_exists || existing_state.is_some();
                let hash_changed = existing_state
                    .as_ref()
                    .is_some_and(|st| st.manifest_hash != preview.manifest_hash);
                let diff_status = preview
                    .manifest
                    .check_permission_diff(&preview.manifest_hash, existing_state.as_ref());

                if is_overwrite || hash_changed {
                    // Never overwrite or run unapproved local-ZIP code over an existing plugin
                    // before the user explicitly confirms and re-approves permissions.
                    let shown_permissions = match &diff_status {
                        PermissionApprovalStatus::NeedsApproval { diff, .. }
                            if !diff.added.is_empty() =>
                        {
                            diff.added.clone()
                        }
                        _ => preview.manifest.permissions.clone(),
                    };
                    this.pending_permission_modal = Some(PermissionPromptState {
                        plugin_id: preview.manifest.id.clone(),
                        plugin_name: preview.manifest.name.clone(),
                        version: preview.manifest.version.clone(),
                        added_permissions: shown_permissions,
                        is_first_install: false,
                        pending_zip_overwrite: Some(PendingZipOverwrite {
                            zip_path: path.clone(),
                            existing_state,
                        }),
                    });
                    cx.notify();
                    return false;
                }

                // Fresh install: guarantee plugin starts disabled until approved/enabled.
                let _ = this.store.set_plugin_enabled(&preview.manifest.id, false);
                true
            });

            if !matches!(decision, Ok(true)) {
                return;
            }

            let install_res = cx
                .background_spawn(async move {
                    let out = client.install_from_zip_file(&path, None)?;
                    let infos = if let Some(mgr) = mgr {
                        Some(mgr.reload_all()?)
                    } else {
                        None
                    };
                    Ok::<_, anyhow::Error>((out, infos))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match install_res {
                    Ok((out, infos)) => {
                        this.status_banner = Some((
                            format!("{} v{}", out.manifest.name, out.manifest.version),
                            false,
                        ));
                        let reloaded_status = infos
                            .as_ref()
                            .and_then(|list| list.iter().find(|p| p.manifest.id == out.manifest.id))
                            .map(|p| &p.status);
                        match reloaded_status {
                            Some(PluginStatus::NeedsApproval {
                                added_permissions,
                                is_first_install,
                            }) => {
                                this.pending_permission_modal = Some(PermissionPromptState {
                                    plugin_id: out.manifest.id,
                                    plugin_name: out.manifest.name,
                                    version: out.manifest.version,
                                    added_permissions: added_permissions.clone(),
                                    is_first_install: *is_first_install,
                                    pending_zip_overwrite: None,
                                });
                            }
                            None => {
                                if let PermissionApprovalStatus::NeedsApproval {
                                    diff,
                                    is_first_install,
                                } = out.approval_status
                                {
                                    this.pending_permission_modal = Some(PermissionPromptState {
                                        plugin_id: out.manifest.id,
                                        plugin_name: out.manifest.name,
                                        version: out.manifest.version,
                                        added_permissions: diff.added,
                                        is_first_install,
                                        pending_zip_overwrite: None,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(err) => {
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn approve_pending_permissions(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_permission_modal.take() else {
            return;
        };
        if let Some(zip_overwrite) = pending.pending_zip_overwrite {
            let Some(client) = PluginHost::global(cx).map(|h| h.registry_client().clone()) else {
                cx.notify();
                return;
            };
            let mgr = PluginHost::global(cx).and_then(|h| h.manager().cloned());
            let pid = pending.plugin_id;
            if self
                .active_plugin_page
                .as_ref()
                .is_some_and(|p| p.plugin_id == pid)
            {
                self.close_plugin_page(cx);
            }
            // Disable in DB before swapping files so overwritten code never auto-activates if interrupted.
            let _ = self.store.set_plugin_enabled(&pid, false);
            let _ = self.store.set_plugin_permissions(&pid, "[]", "");
            let pid_bg = pid.clone();
            let task = cx.background_spawn(async move {
                if let Some(mgr) = &mgr {
                    let _ = mgr.set_plugin_enabled(&pid_bg, false);
                }
                let out = client.install_from_zip_file(
                    &zip_overwrite.zip_path,
                    zip_overwrite.existing_state.as_ref(),
                )?;
                if let Some(mgr) = &mgr {
                    let _ = mgr.reload_all()?;
                    mgr.approve_permissions(&pid_bg)?;
                }
                Ok::<_, anyhow::Error>(out)
            });
            cx.spawn(async move |this, cx| {
                let res = task.await;
                let _ = this.update(cx, |this, cx| {
                    match res {
                        Ok(out) => {
                            this.clear_plugin_ui_caches(&pid);
                            this.status_banner = Some((
                                format!("{} v{}", out.manifest.name, out.manifest.version),
                                false,
                            ));
                        }
                        Err(err) => {
                            this.status_banner = Some((format!("{err:#}"), true));
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
            cx.notify();
            return;
        }

        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            cx.notify();
            return;
        };
        let pid = pending.plugin_id;
        let task = cx.background_spawn(async move { mgr.approve_permissions(&pid) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(err) = res {
                    this.status_banner = Some((format!("{err:#}"), true));
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn uninstall_plugin(&mut self, plugin_id: String, cx: &mut Context<Self>) {
        let Some(client) = PluginHost::global(cx).map(|h| h.registry_client().clone()) else {
            return;
        };
        if self
            .active_plugin_page
            .as_ref()
            .is_some_and(|p| p.plugin_id == plugin_id)
        {
            self.close_plugin_page(cx);
        }
        let mgr = PluginHost::global(cx).and_then(|h| h.manager().cloned());
        let secrets_paths = PluginHost::global(cx).map(PluginHost::secrets_paths);
        let pid = plugin_id.clone();
        let task = cx.background_spawn(async move {
            if let Some(mgr) = &mgr {
                mgr.purge_plugin_data(&pid)?;
            } else if let Some((vault_path, key_path)) = secrets_paths
                && vault_path.exists()
            {
                let vault = plugins::SecretsVault::open(&vault_path, &key_path)?;
                vault.delete_plugin(&pid)?;
            }
            client.uninstall_plugin(&pid)?;
            if let Some(mgr) = &mgr {
                let _ = mgr.reload_all()?;
            }
            Ok::<_, anyhow::Error>(())
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        // Also ensures DB cleanup when uninstalling in Safe Mode (`mgr` is None).
                        let _ = this.store.delete_plugin_all_data(&plugin_id);
                        this.clear_plugin_ui_caches(&plugin_id);
                        if this.expanded_settings.as_deref() == Some(&plugin_id) {
                            this.expanded_settings = None;
                        }
                        if this.expanded_logs.as_deref() == Some(&plugin_id) {
                            this.expanded_logs = None;
                        }
                        this.plugin_logs.remove(&plugin_id);
                    }
                    Err(err) => {
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn add_registry_url(&mut self, cx: &mut Context<Self>) {
        let raw = self.registry_url_input.read(cx).text().trim().to_owned();
        if raw.is_empty() {
            return;
        }
        let lower = raw.to_ascii_lowercase();
        if !(lower.starts_with("https://") || lower.starts_with("http://")) {
            self.status_banner = Some(("URL must start with https:// or http://".to_owned(), true));
            cx.notify();
            return;
        }
        if !self.registries.iter().any(|s| s.url == raw) {
            self.registries.push(RegistrySource {
                url: raw,
                name: None,
                enabled: true,
            });
            self.browse_generation = self.browse_generation.wrapping_add(1);
            self.save_registries();
        }
        self.registry_url_input
            .update(cx, |inp, cx| inp.set_text("", cx));
        self.status_banner = None;
        self.refresh_registries(cx);
    }

    pub(super) fn render_permission_modal(
        &self,
        pending: &PermissionPromptState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();

        let perms_list = if pending.added_permissions.is_empty() {
            v_flex().child(
                Label::new(strings.plugins_permissions_changed)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
        } else {
            v_flex()
                .w_full()
                .gap_1p5()
                .children(pending.added_permissions.iter().map(|perm| {
                    h_flex()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Check)
                                .size(IconSize::XSmall)
                                .color(Color::Accent),
                        )
                        .child(Label::new(perm.summary()).size(LabelSize::Small))
                }))
        };

        Modal::new("plugin-permissions-modal")
            .width(px(440.))
            .on_dismiss(cx.listener(|this, _, _, cx| {
                this.pending_permission_modal = None;
                cx.notify();
            }))
            .child(
                Label::new(strings.plugins_permissions_approve)
                    .size(LabelSize::Default)
                    .weight(FontWeight::SEMIBOLD),
            )
            .child(
                Label::new(format!(
                    "{} ({} v{})",
                    pending.plugin_name, pending.plugin_id, pending.version
                ))
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .when(!pending.is_first_install, |m| {
                m.child(
                    Label::new(strings.plugins_permissions_changed)
                        .size(LabelSize::Small)
                        .color(Color::Warning),
                )
            })
            .child(
                div()
                    .w_full()
                    .p_3()
                    .rounded(px(6.))
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.border_variant)
                    .child(perms_list),
            )
            .child(
                h_flex()
                    .w_full()
                    .pt_1()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("perm-modal-cancel", strings.cancel)
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pending_permission_modal = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("perm-modal-approve", strings.plugins_permissions_approve)
                            .variant(ButtonVariant::Primary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.approve_pending_permissions(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_delete_script_modal(
        &self,
        name: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        div()
            .absolute()
            .inset_0()
            .when(cfg!(test), |el| {
                el.debug_selector(|| "plugin-script-delete-modal".to_owned())
            })
            .child(
                ConfirmDialog::new("plugin-script-delete", strings.confirm_delete_script, strings.cancel, strings.delete)
                    .width(px(340.))
                    .description(name.to_owned())
                    .destructive(true)
                    .on_cancel(cx.listener(|this, _, _, cx| {
                        this.confirm_delete_script = None;
                        cx.notify();
                    }))
                    .on_confirm(cx.listener(|this, _, _, cx| {
                        this.confirm_delete_script = None;
                        this.delete_current_script(cx);
                    })),
            )
            .into_any_element()
    }
}
