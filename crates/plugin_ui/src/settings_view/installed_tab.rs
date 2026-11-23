use std::sync::Arc;

use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px,
};
use icons::IconName;
use plugins::{PluginInfo, PluginStatus};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Chip, Color, Divider, EmptyState, Icon, IconButton,
    IconSize, Label, LabelSize, Switch, Tooltip,
};

use super::helpers::{format_bytes, render_log_entries_box};
use super::modals::PermissionPromptState;
use super::{ActivePluginPage, PluginSettingsTab, PluginSettingsView};
use crate::host::PluginHost;

impl PluginSettingsView {
    pub(super) fn render_installed_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();

        let safe_mode_active = PluginHost::global(cx).is_some_and(|h| h.is_safe_mode());
        let plugins: Arc<Vec<PluginInfo>> = PluginHost::global(cx)
            .map(PluginHost::plugins_arc)
            .unwrap_or_default();

        let safe_mode_card = v_flex()
            .overflow_hidden()
            .rounded(px(8.))
            .bg(colors.surface_background)
            .border_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .id("plugins-safe-mode-toggle")
                    .px_4()
                    .py_3()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.ghost_element_hover))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_safe_mode_pref(cx)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(Label::new(strings.plugins_safe_mode).weight(FontWeight::MEDIUM))
                            .child(
                                Label::new(strings.plugins_safe_mode_hint)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(Switch::new().toggle_state(self.safe_mode_pref)),
            )
            .when(safe_mode_active, |col| {
                col.child(Divider::horizontal()).child(
                    h_flex()
                        .px_4()
                        .py_2p5()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::CircleAlert)
                                .size(IconSize::Small)
                                .color(Color::Warning),
                        )
                        .child(
                            Label::new(strings.plugins_safe_mode_active)
                                .size(LabelSize::Small)
                                .color(Color::Warning),
                        ),
                )
            });

        let toolbar = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("plugins-new-template", strings.plugins_new_template)
                            .variant(ButtonVariant::Primary)
                            .size(ButtonSize::Sm)
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, _, cx| this.create_sample_plugin(cx))),
                    )
                    .child(
                        Button::new("plugins-install-zip", strings.plugins_install_zip)
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .icon(IconName::FolderOpen)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.install_from_zip_dialog(cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        IconButton::new("plugins-open-folder", IconName::Folder)
                            .size(px(30.))
                            .icon_size(IconSize::Small)
                            .icon_color(Color::IconMuted)
                            .hover_icon_color(Color::Icon)
                            .tooltip(Tooltip::text(strings.plugins_open_folder))
                            .on_click(cx.listener(|this, _, _, cx| this.open_plugins_folder(cx))),
                    )
                    .child(
                        IconButton::new("plugins-reload-all", IconName::RefreshCw)
                            .size(px(30.))
                            .icon_size(IconSize::Small)
                            .icon_color(Color::IconMuted)
                            .hover_icon_color(Color::Icon)
                            .tooltip(Tooltip::text(strings.plugins_reload_all))
                            .disabled(safe_mode_active)
                            .when(!safe_mode_active, |btn| {
                                btn.on_click(|_, _, cx| PluginHost::reload_all_async(cx))
                            }),
                    ),
            );

        let cards: Vec<AnyElement> = if plugins.is_empty() {
            vec![
                div()
                    .w_full()
                    .py_10()
                    .child(EmptyState::new(IconName::Layers, strings.plugins_empty))
                    .into_any_element(),
            ]
        } else {
            plugins
                .iter()
                .map(|info| self.render_plugin_card(info, &theme, cx))
                .collect()
        };

        v_flex()
            .w_full()
            .gap_4()
            .child(safe_mode_card)
            .child(toolbar)
            .children(cards)
            .into_any_element()
    }

    fn render_plugin_card(
        &mut self,
        info: &PluginInfo,
        theme: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = theme.colors();
        let status_colors = theme.status();
        let pid: SharedString = info.manifest.id.clone().into();

        let (status_label, status_color) = match &info.status {
            PluginStatus::Active => (strings.plugin_status_active, status_colors.success),
            PluginStatus::Idle => (strings.plugin_status_idle, status_colors.info),
            PluginStatus::Disabled => (strings.plugins_disabled, colors.text_muted),
            PluginStatus::NeedsApproval { .. } => {
                (strings.plugins_permissions_approve, status_colors.warning)
            }
            PluginStatus::CircuitBroken { .. } => {
                (strings.plugin_status_broken, status_colors.error)
            }
            PluginStatus::Error(_) => (strings.plugin_status_error, status_colors.error),
        };

        let metrics_text = strings
            .plugins_metrics
            .replace("{mem}", &format_bytes(info.metrics.memory_bytes))
            .replace("{ms}", &info.metrics.last_exec_ms.to_string());

        let primary_view = info
            .manifest
            .contributions
            .primary_view()
            .map(|(vid, _)| vid);
        let can_open_page = primary_view.is_some()
            && info.enabled
            && !matches!(
                info.status,
                PluginStatus::NeedsApproval { .. }
                    | PluginStatus::CircuitBroken { .. }
                    | PluginStatus::Disabled
            );
        let has_settings = !info.manifest.settings.is_empty();
        let settings_open = self.expanded_settings.as_deref() == Some(pid.as_ref());
        let logs_open = self.expanded_logs.as_deref() == Some(pid.as_ref());

        let pid_toggle = pid.clone();
        let pid_reload = pid.clone();
        let pid_settings = pid.clone();
        let pid_logs = pid.clone();
        let pid_uninstall = pid.clone();

        let mut card = v_flex()
            .w_full()
            .p_4()
            .gap_3()
            .rounded(px(8.))
            .bg(colors.surface_background)
            .border_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_wrap()
                                    .child(
                                        Label::new(info.manifest.name.clone())
                                            .weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(
                                        Label::new(format!("v{}", info.manifest.version))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .when(
                                        info.manifest.version.to_ascii_lowercase().contains("beta"),
                                        |row| {
                                            row.child(Chip::new(
                                                strings.beta_badge,
                                                theme.status().warning,
                                            ))
                                        },
                                    )
                                    .child(Chip::new(status_label, status_color)),
                            )
                            .when(!info.manifest.description.is_empty(), |col| {
                                col.child(
                                    Label::new(info.manifest.description.clone())
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                            })
                            .child(
                                Label::new(format!(
                                    "{} · {}",
                                    info.manifest.id, metrics_text
                                ))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                            ),
                    )
                    .child(
                        h_flex().gap_2().child(
                            div()
                                .id(ElementId::Name(SharedString::from(format!(
                                    "plugin-enable-{pid_toggle}"
                                ))))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_plugin_enabled_by_id(pid_toggle.as_ref(), cx);
                                }))
                                .child(Switch::new().toggle_state(info.enabled)),
                        ),
                    ),
            );

        if matches!(info.status, PluginStatus::NeedsApproval { .. }) {
            let pid_approve = pid.clone();
            card = card.child(
                h_flex()
                    .p_2p5()
                    .gap_2()
                    .justify_between()
                    .rounded(px(6.))
                    .bg(status_colors.warning.opacity(0.12))
                    .child(
                        Label::new(strings.plugins_permissions_changed)
                            .size(LabelSize::Small)
                            .color(Color::Warning),
                    )
                    .child(
                        Button::new(
                            ElementId::Name(SharedString::from(format!(
                                "plugin-approve-{pid}"
                            ))),
                            strings.plugins_permissions_approve,
                        )
                        .variant(ButtonVariant::Primary)
                        .size(ButtonSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(prompt_state) = PluginHost::global(cx)
                                .and_then(|h| h.plugin(pid_approve.as_ref()))
                                .and_then(|info| {
                                    if let PluginStatus::NeedsApproval {
                                        added_permissions,
                                        is_first_install,
                                    } = &info.status
                                    {
                                        Some(PermissionPromptState {
                                            plugin_id: info.manifest.id.clone(),
                                            plugin_name: info.manifest.name.clone(),
                                            version: info.manifest.version.clone(),
                                            added_permissions: added_permissions.clone(),
                                            is_first_install: *is_first_install,
                                            pending_zip_overwrite: None,
                                        })
                                    } else {
                                        None
                                    }
                                })
                            {
                                this.pending_permission_modal = Some(prompt_state);
                                cx.notify();
                            }
                        })),
                    ),
            );
        }

        if let PluginStatus::CircuitBroken { last_error } = &info.status {
            let pid_reset = pid.clone();
            card = card.child(
                h_flex()
                    .p_2p5()
                    .gap_2()
                    .justify_between()
                    .rounded(px(6.))
                    .bg(status_colors.error.opacity(0.12))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                Label::new(strings.plugins_circuit_broken)
                                    .size(LabelSize::Small)
                                    .color(Color::Error),
                            )
                            .child(
                                Label::new(last_error.clone())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .truncate(),
                            ),
                    )
                    .child(
                        Button::new(
                            ElementId::Name(SharedString::from(format!(
                                "plugin-reset-{pid_reset}"
                            ))),
                            strings.plugins_reset_enable,
                        )
                        .variant(ButtonVariant::Primary)
                        .size(ButtonSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reset_and_enable_plugin(pid_reset.as_ref().to_owned(), cx);
                        })),
                    ),
            );
        } else if let PluginStatus::Error(err) = &info.status {
            card = card.child(
                Label::new(err.clone())
                    .size(LabelSize::XSmall)
                    .color(Color::Error),
            );
        }

        if !info.manifest.permissions.is_empty() {
            card = card.child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        Label::new(format!("{}:", strings.plugins_permissions))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .children(info.manifest.permissions.iter().map(|perm| {
                        Chip::new(perm.summary(), colors.accent)
                    })),
            );
        }

        card = card.child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .when_some(primary_view.filter(|_| can_open_page), |row, view_id| {
                    let pid_open = pid.clone();
                    let name_open: SharedString = info.manifest.name.clone().into();
                    let view_id_open: SharedString = view_id.to_owned().into();
                    row.child(
                        Button::new(
                            ElementId::Name(SharedString::from(format!(
                                "plugin-open-{pid_open}"
                            ))),
                            strings.plugins_open,
                        )
                        .variant(ButtonVariant::Secondary)
                        .size(ButtonSize::Sm)
                        .icon(IconName::SquareArrowOutUpRight)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_plugin_page(
                                pid_open.as_ref().to_owned(),
                                name_open.as_ref().to_owned(),
                                view_id_open.as_ref().to_owned(),
                                cx,
                            );
                        })),
                    )
                })
                .child(
                    Button::new(
                        ElementId::Name(SharedString::from(format!(
                            "plugin-reload-{pid_reload}"
                        ))),
                        strings.plugins_reload,
                    )
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(IconName::RefreshCw)
                    .on_click(move |_, _, cx| {
                        PluginHost::reload_plugin_async(pid_reload.as_ref().to_owned(), cx);
                    }),
                )
                .when(has_settings, |row| {
                    row.child(
                        Button::new(
                            ElementId::Name(SharedString::from(format!(
                                "plugin-settings-btn-{pid_settings}"
                            ))),
                            strings.plugins_settings,
                        )
                        .variant(if settings_open {
                            ButtonVariant::Secondary
                        } else {
                            ButtonVariant::Ghost
                        })
                        .size(ButtonSize::Sm)
                        .icon(IconName::Settings2)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.expanded_settings.as_deref() == Some(pid_settings.as_ref()) {
                                this.expanded_settings = None;
                            } else {
                                this.expanded_settings = Some(pid_settings.as_ref().to_owned());
                            }
                            cx.notify();
                        })),
                    )
                })
                .child(
                    Button::new(
                        ElementId::Name(SharedString::from(format!(
                            "plugin-logs-btn-{pid_logs}"
                        ))),
                        strings.plugins_logs,
                    )
                    .variant(if logs_open {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Ghost
                    })
                    .size(ButtonSize::Sm)
                    .icon(IconName::FileText)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.expanded_logs.as_deref() == Some(pid_logs.as_ref()) {
                            this.expanded_logs = None;
                        } else {
                            let pid_owned = pid_logs.as_ref().to_owned();
                            this.expanded_logs = Some(pid_owned.clone());
                            this.fetch_plugin_logs_async(pid_owned, cx);
                        }
                        cx.notify();
                    })),
                )
                .child(
                    Button::new(
                        ElementId::Name(SharedString::from(format!(
                            "plugin-uninstall-{pid_uninstall}"
                        ))),
                        strings.plugins_uninstall,
                    )
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(IconName::Trash)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.uninstall_plugin(pid_uninstall.as_ref().to_owned(), cx);
                    })),
                ),
        );

        if settings_open && has_settings {
            let form = self.render_plugin_settings_form(info, cx);
            card = card.child(Divider::horizontal()).child(form);
        }

        if logs_open {
            let logs_panel = self.render_plugin_logs_panel(pid.as_ref(), theme, cx);
            card = card.child(Divider::horizontal()).child(logs_panel);
        }

        card.into_any_element()
    }

    fn render_plugin_logs_panel(
        &self,
        plugin_id: &str,
        theme: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = theme.colors();
        let logs = self
            .plugin_logs
            .get(plugin_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let pid_refresh: SharedString = plugin_id.to_owned().into();
        let pid_clear = pid_refresh.clone();

        v_flex()
            .w_full()
            .gap_2()
            .pt_1()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        Label::new(strings.plugins_logs)
                            .size(LabelSize::Small)
                            .weight(FontWeight::MEDIUM),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "logs-refresh-{plugin_id}"
                                    ))),
                                    IconName::RefreshCw,
                                )
                                .size(px(24.))
                                .icon_size(IconSize::XSmall)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.fetch_plugin_logs_async(
                                        pid_refresh.as_ref().to_owned(),
                                        cx,
                                    );
                                })),
                            )
                            .child(
                                Button::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "logs-clear-{plugin_id}"
                                    ))),
                                    strings.plugins_clear_logs,
                                )
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.clear_plugin_logs_async(
                                        pid_clear.as_ref().to_owned(),
                                        cx,
                                    );
                                })),
                            ),
                    ),
            )
            .child(render_log_entries_box(
                logs,
                strings.plugin_no_logs,
                colors,
                theme.status(),
            ))
            .into_any_element()
    }

    pub(super) fn render_active_plugin_page(
        &self,
        page: &ActivePluginPage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        let plugin_info = PluginHost::global(cx).and_then(|h| h.plugin(&page.plugin_id));
        let has_settings = plugin_info
            .as_ref()
            .is_some_and(|p| !p.manifest.settings.is_empty());
        let is_beta = plugin_info
            .as_ref()
            .is_some_and(|p| p.manifest.version.to_ascii_lowercase().contains("beta"));
        let warning_color = cx.theme().status().warning;
        let pid: SharedString = page.plugin_id.clone().into();
        let pid_settings = pid.clone();
        let pid_logs = pid.clone();
        let pid_reload = pid;

        v_flex()
            .size_full()
            .min_h_0()
            .when(cfg!(test), |el| {
                el.debug_selector(|| "plugin-settings-active-page".to_owned())
            })
            .child(
                h_flex()
                    .w_full()
                    .px_6()
                    .pt_1()
                    .pb_2p5()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                Button::new("plugin-page-back", strings.plugins_back)
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::Sm)
                                    .icon(IconName::ChevronLeft)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_plugin_page(cx);
                                    })),
                            )
                            .child(
                                Label::new(page.plugin_name.clone())
                                    .size(LabelSize::Default)
                                    .weight(FontWeight::SEMIBOLD)
                                    .truncate(),
                            )
                            .when(is_beta, |row| {
                                row.child(Chip::new(strings.beta_badge, warning_color))
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .when(has_settings, |row| {
                                row.child(
                                    Button::new("plugin-page-settings", strings.plugins_settings)
                                        .variant(ButtonVariant::Ghost)
                                        .size(ButtonSize::Sm)
                                        .icon(IconName::Settings2)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.close_plugin_page(cx);
                                            this.tab = PluginSettingsTab::Installed;
                                            this.expanded_settings =
                                                Some(pid_settings.as_ref().to_owned());
                                            this.expanded_logs = None;
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                Button::new("plugin-page-logs", strings.plugins_logs)
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::Sm)
                                    .icon(IconName::FileText)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_logs_for(pid_logs.as_ref(), cx);
                                    })),
                            )
                            .child(
                                IconButton::new("plugin-page-reload", IconName::RefreshCw)
                                    .size(px(28.))
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::IconMuted)
                                    .hover_icon_color(Color::Icon)
                                    .tooltip(Tooltip::text(strings.plugins_reload))
                                    .on_click(move |_, _, cx| {
                                        PluginHost::reload_plugin_async(
                                            pid_reload.as_ref().to_owned(),
                                            cx,
                                        );
                                    }),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().w_full().child(page.view.clone()))
            .into_any_element()
    }
}
