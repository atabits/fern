//! Fern Cloud zero-knowledge account, real-time sync dashboard, storage quota, connected devices,
//! and vault security controls (`Page::Cloud`) plus compact status summary card for `Page::Sync`.

use desktop_common::sync::{CloudSyncSnapshot, LiveCloudSnapshot, RealtimeStatus};
use desktop_common::text_input::{Mode, Submitted, TextInput};
use fern_core::cloud::{
    AccountProfile, DEFAULT_CLOUD_URL, DeviceSessionInfo, MIN_PASSWORD_LEN, normalize_cloud_url,
    password_strength_score, validate_email,
};
use fern_i18n as i18n;
use gpui::{AnyElement, ClipboardItem, Div, Entity, Focusable as _, FontWeight, Stateful};
use ui::prelude::*;

use super::backups_diagnostics::format_bytes;
use super::{Page, SettingsEvent, SettingsWindow};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CloudAuthTab {
    #[default]
    SignIn,
    Register,
    Recover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CloudSecurityAction {
    ChangePassword,
    RegenerateRecovery,
    DeleteAccount,
}

pub(super) struct CloudSettingsState {
    pub auth_tab: CloudAuthTab,
    pub show_password: bool,
    pub email: Entity<TextInput>,
    pub password: Entity<TextInput>,
    pub confirm_password: Entity<TextInput>,
    pub display_name: Entity<TextInput>,
    pub recovery_key_input: Entity<TextInput>,
    pub current_password: Entity<TextInput>,
    pub new_password: Entity<TextInput>,
    pub new_password_confirm: Entity<TextInput>,
    pub busy: bool,
    pub status_message: Option<(String, bool)>,
    pub pending_recovery_key: Option<String>,
    pub copied_recovery: bool,
    pub account_profile: Option<AccountProfile>,
    pub devices: Vec<DeviceSessionInfo>,
    pub devices_loading: bool,
    pub security_action: Option<CloudSecurityAction>,
}

fn cloud_server_url() -> String {
    std::env::var("FERN_CLOUD_URL")
        .ok()
        .map(|url| normalize_cloud_url(&url))
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_CLOUD_URL.to_owned())
}

impl CloudSettingsState {
    pub(super) fn new(cx: &mut Context<SettingsWindow>) -> Self {
        let strings = i18n::t(cx);
        let mk_input = |placeholder: &'static str,
                        masked: bool,
                        cx: &mut Context<SettingsWindow>| {
            cx.new(|cx| {
                let mut input = TextInput::new(Mode::SingleLine, placeholder, cx);
                if masked {
                    input = input.masked();
                }
                input
            })
        };
        let email = mk_input(strings.cloud_email_placeholder, false, cx);
        let password = mk_input(strings.cloud_enter_password, true, cx);
        let confirm_password = mk_input(strings.cloud_confirm_password, true, cx);
        let display_name = mk_input(strings.cloud_display_name_placeholder, false, cx);
        let recovery_key_input = mk_input(strings.cloud_recovery_key_placeholder, false, cx);
        let current_password = mk_input(strings.cloud_current_password, true, cx);
        let new_password = mk_input(strings.cloud_password_placeholder, true, cx);
        let new_password_confirm = mk_input(strings.cloud_confirm_password, true, cx);

        cx.subscribe(&password, |this: &mut SettingsWindow, _, _: &Submitted, cx| {
            if this.cloud.auth_tab == CloudAuthTab::SignIn {
                this.submit_cloud_auth(cx);
            }
        })
        .detach();
        cx.subscribe(
            &confirm_password,
            |this: &mut SettingsWindow, _, _: &Submitted, cx| {
                this.submit_cloud_auth(cx);
            },
        )
        .detach();

        Self {
            auth_tab: CloudAuthTab::SignIn,
            show_password: false,
            email,
            password,
            confirm_password,
            display_name,
            recovery_key_input,
            current_password,
            new_password,
            new_password_confirm,
            busy: false,
            status_message: None,
            pending_recovery_key: None,
            copied_recovery: false,
            account_profile: None,
            devices: Vec::new(),
            devices_loading: false,
            security_action: None,
        }
    }

    pub(super) fn refresh_placeholders(
        &self,
        strings: &'static i18n::Strings,
        cx: &mut Context<SettingsWindow>,
    ) {
        let pw_placeholder = if self.auth_tab == CloudAuthTab::SignIn {
            strings.cloud_enter_password
        } else {
            strings.cloud_password_placeholder
        };
        self.email
            .update(cx, |i, cx| i.set_placeholder(strings.cloud_email_placeholder, cx));
        self.password
            .update(cx, |i, cx| i.set_placeholder(pw_placeholder, cx));
        self.confirm_password
            .update(cx, |i, cx| i.set_placeholder(strings.cloud_confirm_password, cx));
        self.display_name
            .update(cx, |i, cx| i.set_placeholder(strings.cloud_display_name_placeholder, cx));
        self.recovery_key_input
            .update(cx, |i, cx| i.set_placeholder(strings.cloud_recovery_key_placeholder, cx));
        self.current_password
            .update(cx, |i, cx| i.set_placeholder(strings.cloud_current_password, cx));
        self.new_password
            .update(cx, |i, cx| i.set_placeholder(strings.cloud_password_placeholder, cx));
        self.new_password_confirm
            .update(cx, |i, cx| i.set_placeholder(strings.cloud_confirm_password, cx));
    }

    fn set_auth_tab(&mut self, tab: CloudAuthTab, cx: &mut Context<SettingsWindow>) {
        self.auth_tab = tab;
        self.status_message = None;
        let strings = i18n::t(cx);
        let pw_placeholder = if tab == CloudAuthTab::SignIn {
            strings.cloud_enter_password
        } else {
            strings.cloud_password_placeholder
        };
        self.password
            .update(cx, |i, cx| i.set_placeholder(pw_placeholder, cx));
    }

    fn toggle_password_visibility(&mut self, cx: &mut Context<SettingsWindow>) {
        self.show_password = !self.show_password;
        let masked = !self.show_password;
        self.password.update(cx, |i, cx| i.set_masked(masked, cx));
        self.confirm_password
            .update(cx, |i, cx| i.set_masked(masked, cx));
    }

    fn clear_sensitive_inputs(&self, cx: &mut Context<SettingsWindow>) {
        for input in [
            &self.password,
            &self.confirm_password,
            &self.recovery_key_input,
            &self.current_password,
            &self.new_password,
            &self.new_password_confirm,
        ] {
            input.update(cx, |i, cx| i.set_text(String::new(), cx));
        }
    }
}

impl SettingsWindow {
    pub(super) fn load_cloud_details_if_signed_in(&mut self, cx: &mut Context<Self>) {
        if self.cloud.devices_loading {
            return;
        }
        let signed_in = self.store.cloud_meta().ok().flatten().is_some();
        if !signed_in {
            return;
        }
        self.cloud.devices_loading = true;
        cx.notify();
        let bg = cx.background_spawn(async move {
            desktop_sync::cloud::fetch_account_and_devices_current()
        });
        cx.spawn(async move |this, cx| {
            let result = bg.await;
            let _ = this.update(cx, |this, cx| {
                this.cloud.devices_loading = false;
                match result {
                    Ok(Some((session, account, devices))) => {
                        this.cloud.account_profile = Some(account.clone());
                        this.cloud.devices = devices;
                        this.update_live_cloud_snapshot(&session, Some(&account), cx);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        this.cloud.status_message = Some((
                            desktop_common::diagnostics::redact_sensitive(&format!("{err:#}")),
                            true,
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn update_live_cloud_snapshot(
        &self,
        session: &fern_core::cloud::CloudSessionState,
        account: Option<&AccountProfile>,
        cx: &mut Context<Self>,
    ) {
        let prev_rt = cx
            .try_global::<LiveCloudSnapshot>()
            .map(|g| g.0.realtime)
            .unwrap_or_default();
        let meta = session.meta();
        let mut snap = CloudSyncSnapshot::from_meta(&meta);
        snap.realtime = prev_rt;
        if let Some(acc) = account {
            snap.storage_used_bytes = acc.storage_used_bytes;
            snap.storage_quota_bytes = acc.storage_quota_bytes;
            snap.synced_rows_count = acc.synced_rows_count;
            snap.max_rows_quota = acc.max_rows_quota;
            snap.device_count = acc.device_count;
        }
        cx.set_global(LiveCloudSnapshot(snap));
    }

    pub(super) fn submit_cloud_auth(&mut self, cx: &mut Context<Self>) {
        if self.cloud.busy {
            return;
        }
        let strings = i18n::t(cx);
        let tab = self.cloud.auth_tab;
        let server_url = cloud_server_url();
        let email_raw = self.cloud.email.read(cx).text().to_owned();
        let email = match validate_email(&email_raw) {
            Ok(e) => e,
            Err(err) => {
                self.cloud.status_message = Some((err.to_string(), true));
                cx.notify();
                return;
            }
        };
        let password = self.cloud.password.read(cx).text().to_owned();
        let confirm = self.cloud.confirm_password.read(cx).text().to_owned();
        let display_name = self.cloud.display_name.read(cx).text().to_owned();
        let recovery_input = self.cloud.recovery_key_input.read(cx).text().to_owned();

        if matches!(tab, CloudAuthTab::Register | CloudAuthTab::Recover) {
            if password.chars().count() < MIN_PASSWORD_LEN {
                self.cloud.status_message = Some((strings.cloud_password_too_short.to_owned(), true));
                cx.notify();
                return;
            }
            if password != confirm {
                self.cloud.status_message = Some((strings.cloud_passwords_mismatch.to_owned(), true));
                cx.notify();
                return;
            }
        } else if password.is_empty() {
            self.cloud.status_message = Some((strings.cloud_enter_password.to_owned(), true));
            cx.notify();
            return;
        }

        let device_id = match self.store.sync_device() {
            Ok(id) => id,
            Err(err) => {
                self.cloud.status_message = Some((format!("{err:#}"), true));
                cx.notify();
                return;
            }
        };
        let device_name = desktop_sync::cloud::default_device_name();

        self.cloud.busy = true;
        self.cloud.status_message = None;
        cx.notify();

        let bg = cx.background_spawn(async move {
            match tab {
                CloudAuthTab::SignIn => {
                    let (session, account) = desktop_sync::cloud::login_and_store(
                        &server_url,
                        &email,
                        &password,
                        device_id,
                        device_name,
                    )?;
                    Ok::<_, anyhow::Error>((session, account, None))
                }
                CloudAuthTab::Register => {
                    let (session, account, recovery) = desktop_sync::cloud::register_and_store(
                        &server_url,
                        &email,
                        &display_name,
                        &password,
                        device_id,
                        device_name,
                        None,
                    )?;
                    Ok((
                        session,
                        account,
                        Some(recovery.to_formatted_string().to_string()),
                    ))
                }
                CloudAuthTab::Recover => {
                    let (session, account, new_recovery) = desktop_sync::cloud::recover_and_store(
                        &server_url,
                        &email,
                        &recovery_input,
                        &password,
                        device_id,
                        device_name,
                    )?;
                    Ok((
                        session,
                        account,
                        Some(new_recovery.to_formatted_string().to_string()),
                    ))
                }
            }
        });

        cx.spawn(async move |this, cx| {
            let result = bg.await;
            let _ = this.update(cx, |this, cx| {
                this.cloud.busy = false;
                match result {
                    Ok((session, account, recovery_key)) => {
                        let meta = session.meta();
                        let switched_user = this
                            .store
                            .cloud_meta()
                            .ok()
                            .flatten()
                            .is_some_and(|prev| prev.user_id != meta.user_id);
                        let prep_res = if switched_user {
                            this.store.clear_synced_user_data()
                        } else {
                            Ok(())
                        };
                        if let Err(err) = prep_res.and_then(|()| this.store.set_cloud_meta(Some(&meta))) {
                            this.cloud.status_message = Some((format!("{err:#}"), true));
                        } else {
                            this.cloud.clear_sensitive_inputs(cx);
                            this.cloud.pending_recovery_key = recovery_key;
                            this.cloud.copied_recovery = false;
                            this.cloud.account_profile = Some(account.clone());
                            this.cloud.status_message = None;
                            this.update_live_cloud_snapshot(&session, Some(&account), cx);
                            cx.emit(SettingsEvent::Changed);
                            this.load_cloud_details_if_signed_in(cx);
                        }
                    }
                    Err(err) => {
                        this.cloud.status_message = Some((
                            desktop_common::diagnostics::redact_sensitive(&format!("{err:#}")),
                            true,
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn sign_out_cloud(&mut self, cx: &mut Context<Self>) {
        if self.cloud.busy {
            return;
        }
        self.cloud.busy = true;
        self.cloud.status_message = None;
        cx.notify();

        let bg = cx.background_spawn(async move { desktop_sync::cloud::logout_and_clear() });
        cx.spawn(async move |this, cx| {
            let res = bg.await;
            let _ = this.update(cx, |this, cx| {
                this.cloud.busy = false;
                let _ = this.store.set_cloud_meta(None);
                this.cloud.account_profile = None;
                this.cloud.devices.clear();
                this.cloud.pending_recovery_key = None;
                this.cloud.security_action = None;
                cx.set_global(LiveCloudSnapshot(CloudSyncSnapshot::default()));
                if let Err(err) = res {
                    this.cloud.status_message = Some((format!("{err:#}"), true));
                }
                cx.emit(SettingsEvent::Changed);
                cx.notify();
            });
        })
        .detach();
    }

    fn revoke_cloud_device(&mut self, session_id: String, cx: &mut Context<Self>) {
        if self.cloud.busy {
            return;
        }
        self.cloud.busy = true;
        cx.notify();
        let bg = cx.background_spawn(async move {
            desktop_sync::cloud::revoke_device_current(&session_id)
        });
        cx.spawn(async move |this, cx| {
            let res = bg.await;
            let _ = this.update(cx, |this, cx| {
                this.cloud.busy = false;
                match res {
                    Ok(devices) => {
                        this.cloud.devices = devices;
                        this.cloud.status_message = None;
                    }
                    Err(err) => {
                        this.cloud.status_message = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn revoke_other_cloud_devices(&mut self, cx: &mut Context<Self>) {
        if self.cloud.busy {
            return;
        }
        self.cloud.busy = true;
        cx.notify();
        let bg = cx.background_spawn(async move {
            desktop_sync::cloud::revoke_other_devices_current()
        });
        cx.spawn(async move |this, cx| {
            let res = bg.await;
            let _ = this.update(cx, |this, cx| {
                this.cloud.busy = false;
                match res {
                    Ok(devices) => {
                        this.cloud.devices = devices;
                        this.cloud.status_message = None;
                    }
                    Err(err) => {
                        this.cloud.status_message = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn submit_cloud_security_action(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self.cloud.security_action else {
            return;
        };
        if self.cloud.busy {
            return;
        }
        let strings = i18n::t(cx);
        let current = self.cloud.current_password.read(cx).text().to_owned();
        let new_pw = self.cloud.new_password.read(cx).text().to_owned();
        let new_pw_confirm = self.cloud.new_password_confirm.read(cx).text().to_owned();

        if current.is_empty() {
            self.cloud.status_message = Some((strings.cloud_current_password.to_owned(), true));
            cx.notify();
            return;
        }
        if action == CloudSecurityAction::ChangePassword {
            if new_pw.chars().count() < MIN_PASSWORD_LEN {
                self.cloud.status_message = Some((strings.cloud_password_too_short.to_owned(), true));
                cx.notify();
                return;
            }
            if new_pw != new_pw_confirm {
                self.cloud.status_message = Some((strings.cloud_passwords_mismatch.to_owned(), true));
                cx.notify();
                return;
            }
        }

        self.cloud.busy = true;
        self.cloud.status_message = None;
        cx.notify();

        let bg = cx.background_spawn(async move {
            match action {
                CloudSecurityAction::ChangePassword => {
                    let new_rec =
                        desktop_sync::cloud::change_password_current(&current, &new_pw)?;
                    Ok::<_, anyhow::Error>((
                        Some(new_rec.to_formatted_string().to_string()),
                        false,
                    ))
                }
                CloudSecurityAction::RegenerateRecovery => {
                    let new_rec =
                        desktop_sync::cloud::regenerate_recovery_key_current(&current)?;
                    Ok((Some(new_rec.to_formatted_string().to_string()), false))
                }
                CloudSecurityAction::DeleteAccount => {
                    desktop_sync::cloud::delete_account_current(&current)?;
                    Ok((None, true))
                }
            }
        });

        cx.spawn(async move |this, cx| {
            let res = bg.await;
            let _ = this.update(cx, |this, cx| {
                this.cloud.busy = false;
                let strings = i18n::t(cx);
                match res {
                    Ok((maybe_key, deleted)) => {
                        this.cloud.clear_sensitive_inputs(cx);
                        this.cloud.security_action = None;
                        if deleted {
                            let _ = this.store.set_cloud_meta(None);
                            this.cloud.account_profile = None;
                            this.cloud.devices.clear();
                            this.cloud.pending_recovery_key = None;
                            cx.set_global(LiveCloudSnapshot(CloudSyncSnapshot::default()));
                            this.cloud.status_message =
                                Some((strings.cloud_account_deleted.to_owned(), false));
                            cx.emit(SettingsEvent::Changed);
                        } else {
                            this.cloud.pending_recovery_key = maybe_key;
                            this.cloud.copied_recovery = false;
                            let msg = match action {
                                CloudSecurityAction::ChangePassword => strings.cloud_password_updated,
                                _ => strings.cloud_recovery_updated,
                            };
                            this.cloud.status_message = Some((msg.to_owned(), false));
                            this.load_cloud_details_if_signed_in(cx);
                        }
                    }
                    Err(err) => {
                        this.cloud.status_message = Some((
                            desktop_common::diagnostics::redact_sensitive(&format!("{err:#}")),
                            true,
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Compact 1-row banner rendered at the top of `Page::Sync` (`Data & Sync`), linking to `Page::Cloud`
    /// without duplicating the full auth form alongside self-hosted `fern-server` fields.
    pub(super) fn render_cloud_summary_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let status = theme.status();
        let meta = self.store.cloud_meta().ok().flatten();
        let live_snap = cx
            .try_global::<LiveCloudSnapshot>()
            .map(|g| g.0.clone())
            .unwrap_or_default();

        let (subtitle, badge, cta) = if let Some(meta) = meta {
            let rt_chip = match live_snap.realtime {
                RealtimeStatus::Live => Some((strings.cloud_live_badge, status.success)),
                RealtimeStatus::Connecting => Some((strings.cloud_connecting_badge, status.warning)),
                RealtimeStatus::Disconnected => Some((strings.cloud_e2ee_badge, status.success)),
                RealtimeStatus::SessionExpired => Some((strings.cloud_offline_badge, status.error)),
            };
            (meta.email, rt_chip, strings.cloud_manage_account)
        } else {
            (
                strings.cloud_hero_subtitle.to_owned(),
                Some((strings.cloud_e2ee_badge, status.success)),
                strings.cloud_sign_in,
            )
        };

        h_flex()
            .p_3p5()
            .gap_3()
            .items_center()
            .rounded(px(10.))
            .bg(colors.surface_background)
            .border_1()
            .border_color(colors.border_variant)
            .child(
                div()
                    .size(px(36.))
                    .flex_none()
                    .rounded(px(9.))
                    .bg(colors.accent.opacity(0.14))
                    .border_1()
                    .border_color(colors.accent.opacity(0.28))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::RefreshCw)
                            .size(IconSize::Small)
                            .color(Color::Accent),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Label::new(strings.cloud_title)
                                    .size(LabelSize::Small)
                                    .weight(FontWeight::SEMIBOLD),
                            )
                            .when_some(badge, |row, (text, color)| {
                                row.child(Chip::new(text, color))
                            }),
                    )
                    .child(
                        Label::new(subtitle)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted)
                            .truncate(),
                    ),
            )
            .child(
                Button::new("sync-open-cloud-page", cta)
                    .variant(ButtonVariant::Secondary)
                    .size(ButtonSize::Sm)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_page(Page::Cloud, cx);
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_cloud_page(
        &self,
        body: Stateful<Div>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let status = theme.status();
        let meta = self.store.cloud_meta().ok().flatten();
        let live_snap = cx
            .try_global::<LiveCloudSnapshot>()
            .map(|g| g.0.clone())
            .unwrap_or_default();

        // Reusable stacked vertical form field with crisp 13px typography & focus ring.
        let v_field = |id: &'static str,
                       label: &'static str,
                       input: Entity<TextInput>,
                       show_eye: bool,
                       cx: &mut Context<Self>| {
            let focused = input.focus_handle(cx).is_focused(window);
            let show_pw = self.cloud.show_password;
            v_flex()
                .gap_1()
                .child(
                    Label::new(label)
                        .size(LabelSize::XSmall)
                        .weight(FontWeight::MEDIUM)
                        .color(Color::Muted),
                )
                .child(
                    h_flex()
                        .debug_selector(move || id.to_owned())
                        .w_full()
                        .h(px(34.))
                        .px_3()
                        .gap_2()
                        .items_center()
                        .rounded(px(8.))
                        .bg(colors.background)
                        .border_1()
                        .border_color(if focused { colors.accent } else { colors.border })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_size(px(13.))
                                .child(input),
                        )
                        .when(show_eye, |row| {
                            row.child(
                                div()
                                    .id("cloud-toggle-pw-eye")
                                    .size(px(24.))
                                    .flex_none()
                                    .rounded(px(5.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors.element_hover))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud.toggle_password_visibility(cx);
                                        cx.notify();
                                    }))
                                    .child(
                                        Icon::new(if show_pw {
                                            IconName::EyeOff
                                        } else {
                                            IconName::Eye
                                        })
                                        .size(IconSize::XSmall)
                                        .color(Color::Muted),
                                    ),
                            )
                        }),
                )
        };

        let recovery_banner = self.cloud.pending_recovery_key.as_ref().map(|key| {
            let key_for_copy = key.clone();
            v_flex()
                .p_4()
                .gap_2p5()
                .rounded(px(10.))
                .bg(status.warning.opacity(0.10))
                .border_1()
                .border_color(status.warning.opacity(0.40))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::CircleAlert)
                                .size(IconSize::Small)
                                .color(Color::Custom(status.warning)),
                        )
                        .child(
                            Label::new(strings.cloud_recovery_key_title)
                                .weight(FontWeight::SEMIBOLD),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_muted)
                        .child(strings.cloud_recovery_key_body),
                )
                .child(
                    div()
                        .px_3()
                        .py_2p5()
                        .rounded(px(7.))
                        .bg(colors.background)
                        .border_1()
                        .border_color(colors.border)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(key.clone()),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new(
                                "cloud-copy-recovery",
                                if self.cloud.copied_recovery {
                                    strings.cloud_recovery_key_copied
                                } else {
                                    strings.cloud_copy_recovery_key
                                },
                            )
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .icon(if self.cloud.copied_recovery {
                                IconName::Check
                            } else {
                                IconName::Copy
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    key_for_copy.clone(),
                                ));
                                this.cloud.copied_recovery = true;
                                cx.notify();
                            })),
                        )
                        .child(
                            Button::new("cloud-dismiss-recovery", strings.cloud_recovery_key_saved)
                                .variant(ButtonVariant::Primary)
                                .size(ButtonSize::Sm)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cloud.pending_recovery_key = None;
                                    this.cloud.copied_recovery = false;
                                    cx.notify();
                                })),
                        ),
                )
        });

        let status_banner = self.cloud.status_message.as_ref().map(|(msg, is_err)| {
            let tint = if *is_err { status.error } else { status.success };
            h_flex()
                .px_3()
                .py_2p5()
                .gap_2()
                .items_center()
                .rounded(px(8.))
                .bg(tint.opacity(0.11))
                .border_1()
                .border_color(tint.opacity(0.32))
                .child(
                    Icon::new(if *is_err {
                        IconName::CircleAlert
                    } else {
                        IconName::CircleCheck
                    })
                    .size(IconSize::XSmall)
                    .color(Color::Custom(tint)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(tint)
                        .child(msg.clone()),
                )
        });

        if let Some(meta) = meta {
            // ==================== SIGNED-IN ACCOUNT DASHBOARD ====================
            let rt_label_color = match live_snap.realtime {
                RealtimeStatus::Live => (strings.cloud_live_badge, status.success),
                RealtimeStatus::Connecting => (strings.cloud_connecting_badge, status.warning),
                RealtimeStatus::Disconnected => (strings.cloud_offline_badge, colors.text_muted),
                RealtimeStatus::SessionExpired => (strings.cloud_offline_badge, status.error),
            };
            let display_title = if meta.display_name.trim().is_empty() {
                meta.email.split('@').next().unwrap_or("Fern").to_owned()
            } else {
                meta.display_name.clone()
            };
            let used = self
                .cloud
                .account_profile
                .as_ref()
                .map(|a| a.storage_used_bytes)
                .unwrap_or(live_snap.storage_used_bytes);
            let quota = self
                .cloud
                .account_profile
                .as_ref()
                .map(|a| a.storage_quota_bytes)
                .unwrap_or(live_snap.storage_quota_bytes)
                .max(1);
            let rows_count = self
                .cloud
                .account_profile
                .as_ref()
                .map(|a| a.synced_rows_count)
                .unwrap_or(live_snap.synced_rows_count);
            let pct = ((used as f64 / quota as f64).clamp(0.0, 1.0) * 100.0) as f32;
            let has_other_devices = self.cloud.devices.iter().any(|d| !d.is_current);

            let sec_form = self.cloud.security_action.map(|action| {
                let is_delete = action == CloudSecurityAction::DeleteAccount;
                let action_label = match action {
                    CloudSecurityAction::ChangePassword => strings.cloud_change_password,
                    CloudSecurityAction::RegenerateRecovery => strings.cloud_regenerate_recovery,
                    CloudSecurityAction::DeleteAccount => strings.cloud_delete_account,
                };
                v_flex()
                    .p_4()
                    .gap_3()
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .when(is_delete, |el| {
                        el.child(
                            div()
                                .text_size(px(12.))
                                .text_color(status.error)
                                .child(strings.cloud_delete_account_hint),
                        )
                    })
                    .child(v_field(
                        "cloud-sec-current-pw",
                        strings.cloud_current_password,
                        self.cloud.current_password.clone(),
                        false,
                        cx,
                    ))
                    .when(action == CloudSecurityAction::ChangePassword, |el| {
                        el.child(v_field(
                            "cloud-sec-new-pw",
                            strings.cloud_new_password,
                            self.cloud.new_password.clone(),
                            false,
                            cx,
                        ))
                        .child(v_field(
                            "cloud-sec-confirm-pw",
                            strings.cloud_confirm_password,
                            self.cloud.new_password_confirm.clone(),
                            false,
                            cx,
                        ))
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("cloud-sec-cancel", strings.cancel)
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::Sm)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloud.security_action = None;
                                        this.cloud.clear_sensitive_inputs(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new(
                                    "cloud-sec-submit",
                                    if self.cloud.busy {
                                        strings.cloud_working
                                    } else {
                                        action_label
                                    },
                                )
                                .variant(if is_delete {
                                    ButtonVariant::Destructive
                                } else {
                                    ButtonVariant::Primary
                                })
                                .size(ButtonSize::Sm)
                                .disabled(self.cloud.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.submit_cloud_security_action(cx);
                                })),
                            ),
                    )
            });

            let sec_row = |id: &'static str,
                           label: &'static str,
                           icon: IconName,
                           action: CloudSecurityAction,
                           danger: bool| {
                let active = self.cloud.security_action == Some(action);
                h_flex()
                    .id(id)
                    .h(px(42.))
                    .px_4()
                    .gap_2p5()
                    .items_center()
                    .cursor_pointer()
                    .when(active, |el| el.bg(colors.element_selected))
                    .hover(|s| s.bg(colors.element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.cloud.security_action = if this.cloud.security_action == Some(action) {
                            None
                        } else {
                            Some(action)
                        };
                        cx.notify();
                    }))
                    .child(Icon::new(icon).size(IconSize::Small).color(if danger {
                        Color::Custom(status.error)
                    } else {
                        Color::Muted
                    }))
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(label)
                                .size(LabelSize::Small)
                                .color(if danger {
                                    Color::Custom(status.error)
                                } else {
                                    Color::Default
                                }),
                        ),
                    )
                    .child(
                        Icon::new(if active {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(IconSize::XSmall)
                        .color(Color::Muted),
                    )
            };

            return body
                .child(
                    v_flex()
                        .w_full()
                        .max_w(px(540.))
                        .mx_auto()
                        .gap_4()
                        .children(recovery_banner)
                        // Profile Hero Card
                        .child(
                            v_flex()
                                .p_5()
                                .gap_4()
                                .rounded(px(12.))
                                .bg(colors.surface_background)
                                .border_1()
                                .border_color(colors.border_variant)
                                .child(
                                    h_flex()
                                        .gap_3p5()
                                        .items_center()
                                        .child(
                                            div()
                                                .relative()
                                                .size(px(48.))
                                                .flex_none()
                                                .rounded(px(12.))
                                                .bg(colors.accent.opacity(0.16))
                                                .border_1()
                                                .border_color(colors.accent.opacity(0.35))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(17.))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(colors.accent)
                                                .child(meta.initials())
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .bottom(px(-2.))
                                                        .right(px(-2.))
                                                        .size(px(11.))
                                                        .rounded_full()
                                                        .bg(rt_label_color.1)
                                                        .border_2()
                                                        .border_color(colors.surface_background),
                                                ),
                                        )
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .gap_1()
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_center()
                                                        .flex_wrap()
                                                        .child(
                                                            div()
                                                                .text_size(px(16.))
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .child(display_title),
                                                        )
                                                        .child(Chip::new(
                                                            strings.cloud_e2ee_badge,
                                                            status.success,
                                                        ))
                                                        .child(Chip::new(
                                                            rt_label_color.0,
                                                            rt_label_color.1,
                                                        )),
                                                )
                                                .child(
                                                    Label::new(meta.email)
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Muted)
                                                        .truncate(),
                                                ),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .flex_wrap()
                                        .child(
                                            Button::new("cloud-sync-now", strings.cloud_sync_now)
                                                .variant(ButtonVariant::Primary)
                                                .size(ButtonSize::Sm)
                                                .icon(IconName::RefreshCw)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    (this.host.sync_now)(cx);
                                                    this.load_cloud_details_if_signed_in(cx);
                                                })),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            Button::new("cloud-sign-out", strings.cloud_sign_out)
                                                .variant(ButtonVariant::Ghost)
                                                .size(ButtonSize::Sm)
                                                .disabled(self.cloud.busy)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.sign_out_cloud(cx);
                                                })),
                                        ),
                                ),
                        )
                        // Storage & Vault Stats Card
                        .child(
                            v_flex()
                                .p_4()
                                .gap_2p5()
                                .rounded(px(10.))
                                .bg(colors.surface_background)
                                .border_1()
                                .border_color(colors.border_variant)
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            Label::new(strings.cloud_storage_usage)
                                                .size(LabelSize::Small)
                                                .weight(FontWeight::MEDIUM),
                                        )
                                        .child(
                                            Label::new(format!(
                                                "{} / {}",
                                                format_bytes(used),
                                                format_bytes(quota)
                                            ))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                        ),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(6.))
                                        .rounded_full()
                                        .bg(colors.background)
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .h_full()
                                                .w(gpui::relative(pct / 100.0))
                                                .rounded_full()
                                                .bg(if pct > 90.0 {
                                                    status.error
                                                } else {
                                                    colors.accent
                                                }),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .child(
                                            Label::new(format!(
                                                "{}: {rows_count}",
                                                strings.cloud_synced_items
                                            ))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                        )
                                        .child(
                                            Label::new(format!(
                                                "{}: {}",
                                                strings.cloud_devices_title,
                                                self.cloud.devices.len().max(1)
                                            ))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                        ),
                                ),
                        )
                        // Connected Devices Card
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .justify_between()
                                        .px_0p5()
                                        .child(
                                            Label::new(strings.cloud_devices_title)
                                                .size(LabelSize::Small)
                                                .weight(FontWeight::MEDIUM),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .when(has_other_devices, |row| {
                                                    row.child(
                                                        Button::new(
                                                            "cloud-revoke-others",
                                                            strings.cloud_revoke_others,
                                                        )
                                                        .variant(ButtonVariant::Ghost)
                                                        .size(ButtonSize::Sm)
                                                        .disabled(self.cloud.busy)
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.revoke_other_cloud_devices(cx);
                                                        })),
                                                    )
                                                })
                                                .child(
                                                    Button::new(
                                                        "cloud-refresh-devices",
                                                        strings.cloud_refresh_devices,
                                                    )
                                                    .variant(ButtonVariant::Ghost)
                                                    .size(ButtonSize::Sm)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.load_cloud_details_if_signed_in(cx);
                                                    })),
                                                ),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .rounded(px(10.))
                                        .overflow_hidden()
                                        .bg(colors.surface_background)
                                        .border_1()
                                        .border_color(colors.border_variant)
                                        .children(self.cloud.devices.iter().enumerate().map(
                                            |(idx, dev)| {
                                                let sid = dev.session_id.clone();
                                                h_flex()
                                                    .min_h(px(46.))
                                                    .px_4()
                                                    .py_2()
                                                    .gap_3()
                                                    .items_center()
                                                    .when(idx > 0, |row| {
                                                        row.border_t_1()
                                                            .border_color(colors.border_variant)
                                                    })
                                                    .child(
                                                        v_flex()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .gap_0p5()
                                                            .child(
                                                                h_flex()
                                                                    .gap_2()
                                                                    .items_center()
                                                                    .child(
                                                                        Label::new(
                                                                            dev.device_name.clone(),
                                                                        )
                                                                        .size(LabelSize::Small)
                                                                        .weight(FontWeight::MEDIUM)
                                                                        .truncate(),
                                                                    )
                                                                    .when(dev.is_current, |row| {
                                                                        row.child(Chip::new(
                                                                            strings
                                                                                .cloud_this_device,
                                                                            colors.accent,
                                                                        ))
                                                                    }),
                                                            )
                                                            .child(
                                                                Label::new(format!(
                                                                    "{} · {}",
                                                                    dev.platform, dev.ip_hint
                                                                ))
                                                                .size(LabelSize::XSmall)
                                                                .color(Color::Muted),
                                                            ),
                                                    )
                                                    .when(!dev.is_current, |row| {
                                                        row.child(
                                                            Button::new(
                                                                SharedString::from(format!(
                                                                    "cloud-revoke-{idx}"
                                                                )),
                                                                strings.cloud_revoke_device,
                                                            )
                                                            .variant(ButtonVariant::Ghost)
                                                            .size(ButtonSize::Sm)
                                                            .disabled(self.cloud.busy)
                                                            .on_click(cx.listener(
                                                                move |this, _, _, cx| {
                                                                    this.revoke_cloud_device(
                                                                        sid.clone(),
                                                                        cx,
                                                                    );
                                                                },
                                                            )),
                                                        )
                                                    })
                                            },
                                        )),
                                ),
                        )
                        // Security & Recovery Card
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    Label::new(strings.cloud_security_title)
                                        .size(LabelSize::Small)
                                        .weight(FontWeight::MEDIUM),
                                )
                                .child(
                                    v_flex()
                                        .rounded(px(10.))
                                        .overflow_hidden()
                                        .bg(colors.surface_background)
                                        .border_1()
                                        .border_color(colors.border_variant)
                                        .child(sec_row(
                                            "cloud-change-pw-btn",
                                            strings.cloud_change_password,
                                            IconName::Settings2,
                                            CloudSecurityAction::ChangePassword,
                                            false,
                                        ))
                                        .child(div().border_t_1().border_color(colors.border_variant).child(sec_row(
                                            "cloud-regen-rec-btn",
                                            strings.cloud_regenerate_recovery,
                                            IconName::RotateCcw,
                                            CloudSecurityAction::RegenerateRecovery,
                                            false,
                                        )))
                                        .child(div().border_t_1().border_color(colors.border_variant).child(sec_row(
                                            "cloud-delete-acct-btn",
                                            strings.cloud_delete_account,
                                            IconName::Trash,
                                            CloudSecurityAction::DeleteAccount,
                                            true,
                                        )))
                                        .children(sec_form),
                                ),
                        )
                        .children(status_banner),
                )
                .into_any_element();
        }

        // ==================== SIGNED-OUT AUTH EXPERIENCE ====================
        let tab = self.cloud.auth_tab;
        let is_recover = tab == CloudAuthTab::Recover;
        let pw_text = self.cloud.password.read(cx).text();
        let show_strength =
            matches!(tab, CloudAuthTab::Register | CloudAuthTab::Recover) && !pw_text.is_empty();
        let strength = if show_strength {
            password_strength_score(pw_text)
        } else {
            0
        };
        let strength_color = match strength {
            0..=1 => status.error,
            2 => status.warning,
            _ => status.success,
        };

        let seg_btn = |id: &'static str, label: &'static str, target: CloudAuthTab| {
            let active = tab == target;
            div()
                .id(id)
                .flex_1()
                .h(px(30.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(colors.background)
                        .border_1()
                        .border_color(colors.border)
                })
                .when(!active, |el| el.hover(|s| s.bg(colors.element_hover)))
                .child(
                    Label::new(label)
                        .size(LabelSize::Small)
                        .weight(if active {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .color(if active { Color::Default } else { Color::Muted }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.cloud.set_auth_tab(target, cx);
                    cx.notify();
                }))
        };

        let submit_label = if self.cloud.busy {
            strings.cloud_working
        } else {
            match tab {
                CloudAuthTab::SignIn => strings.cloud_sign_in,
                CloudAuthTab::Register => strings.cloud_create_account,
                CloudAuthTab::Recover => strings.cloud_recover_account,
            }
        };

        body.child(
            h_flex().pb_1().child(
                Button::new("cloud-back-to-sync", strings.data_and_sync)
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(IconName::ChevronLeft)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_page(Page::Sync, cx);
                    })),
            ),
        )
        .child(
            v_flex()
                .w_full()
                .max_w(px(430.))
                .mx_auto()
                .pb_2()
                .gap_3p5()
                // Centered Product Hero
                .child(
                    v_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .size(px(44.))
                                .rounded(px(12.))
                                .bg(colors.accent.opacity(0.14))
                                .border_1()
                                .border_color(colors.accent.opacity(0.30))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::RefreshCw)
                                        .size(IconSize::Custom(px(20.)))
                                        .color(Color::Accent),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(19.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(strings.cloud_title),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(colors.text_muted)
                                .text_center()
                                .child(strings.cloud_hero_subtitle),
                        ),
                )
                .children(recovery_banner)
                // Segmented Control (Sign In | Create Account) OR Recover Back Header
                .child(if !is_recover {
                    h_flex()
                        .w_full()
                        .p(px(3.))
                        .gap_1()
                        .rounded(px(9.))
                        .bg(colors.surface_background)
                        .border_1()
                        .border_color(colors.border_variant)
                        .child(seg_btn(
                            "cloud-tab-signin",
                            strings.cloud_sign_in,
                            CloudAuthTab::SignIn,
                        ))
                        .child(seg_btn(
                            "cloud-tab-register",
                            strings.cloud_create_account,
                            CloudAuthTab::Register,
                        ))
                        .into_any_element()
                } else {
                    h_flex()
                        .items_center()
                        .justify_between()
                        .child(
                            Button::new("cloud-back-signin", strings.cloud_back_to_sign_in)
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .icon(IconName::ChevronLeft)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cloud.set_auth_tab(CloudAuthTab::SignIn, cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Label::new(strings.cloud_recover_account)
                                .size(LabelSize::Small)
                                .weight(FontWeight::MEDIUM),
                        )
                        .into_any_element()
                })
                // Main Auth Card
                .child(
                    v_flex()
                        .w_full()
                        .px_5()
                        .py_4()
                        .gap_3()
                        .rounded(px(12.))
                        .bg(colors.surface_background)
                        .border_1()
                        .border_color(colors.border_variant)
                        .child(v_field(
                            "cloud-email-input",
                            strings.cloud_email,
                            self.cloud.email.clone(),
                            false,
                            cx,
                        ))
                        .when(tab == CloudAuthTab::Register, |card| {
                            card.child(v_field(
                                "cloud-display-name-input",
                                strings.cloud_display_name,
                                self.cloud.display_name.clone(),
                                false,
                                cx,
                            ))
                        })
                        .when(is_recover, |card| {
                            card.child(v_field(
                                "cloud-recovery-key-input",
                                strings.cloud_recovery_key,
                                self.cloud.recovery_key_input.clone(),
                                false,
                                cx,
                            ))
                        })
                        .child(
                            v_flex()
                                .gap_1p5()
                                .child(v_field(
                                    "cloud-password-input",
                                    if is_recover {
                                        strings.cloud_new_password
                                    } else {
                                        strings.cloud_password
                                    },
                                    self.cloud.password.clone(),
                                    true,
                                    cx,
                                ))
                                .when(show_strength, |box_el| {
                                    box_el.child(
                                        h_flex().gap_1().pt_0p5().children((0..4u8).map(|idx| {
                                            div()
                                                .flex_1()
                                                .h(px(3.))
                                                .rounded_full()
                                                .bg(if idx < strength {
                                                    strength_color
                                                } else {
                                                    colors.border
                                                })
                                        })),
                                    )
                                })
                                .when(tab == CloudAuthTab::SignIn, |box_el| {
                                    box_el.child(
                                        h_flex().justify_end().child(
                                            div()
                                                .id("cloud-tab-recover")
                                                .cursor_pointer()
                                                .hover(|s| s.opacity(0.8))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cloud
                                                        .set_auth_tab(CloudAuthTab::Recover, cx);
                                                    cx.notify();
                                                }))
                                                .child(
                                                    Label::new(strings.cloud_forgot_password)
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Muted),
                                                ),
                                        ),
                                    )
                                }),
                        )
                        .when(matches!(tab, CloudAuthTab::Register | CloudAuthTab::Recover), |card| {
                            card.child(v_field(
                                "cloud-confirm-password-input",
                                strings.cloud_confirm_password,
                                self.cloud.confirm_password.clone(),
                                false,
                                cx,
                            ))
                        })
                        .children(status_banner)
                        .child(
                            div().pt_1().child(
                                Button::new("cloud-auth-submit", submit_label)
                                    .variant(ButtonVariant::Primary)
                                    .size(ButtonSize::Md)
                                    .full_width()
                                    .disabled(self.cloud.busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_cloud_auth(cx);
                                    })),
                            ),
                        ),
                )
                // Clean Minimalist Trust Footer
                .child(
                    h_flex()
                        .justify_center()
                        .items_center()
                        .gap_2p5()
                        .flex_wrap()
                        .child(
                            h_flex()
                                .id("cloud-e2ee-trust-pill")
                                .gap_1p5()
                                .items_center()
                                .tooltip(Tooltip::text(strings.cloud_e2ee_note))
                                .child(
                                    div()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(status.success),
                                )
                                .child(
                                    Label::new(strings.cloud_e2ee_badge)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        ),
                ),
        )
        .into_any_element()
    }
}
