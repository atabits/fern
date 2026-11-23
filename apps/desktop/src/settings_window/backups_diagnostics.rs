use gpui::FontWeight;
use ui::prelude::*;

use crate::calendar;
use crate::i18n;
use crate::release_notes::action_button;

use super::{ROW_HEIGHT, SettingsWindow};

impl SettingsWindow {
    pub(super) fn render_restore_backup_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let path = self.pending_restore.as_ref()?;
        let strings = i18n::t(cx);
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("backup.db");
        let description = strings.restore_backup_confirm_body.replace("{file}", filename);
        Some(
            div()
                .absolute()
                .inset_0()
                .debug_selector(|| "confirm-restore-backup".to_owned())
                .child(
                    ConfirmDialog::new(
                        "confirm-restore-backup",
                        strings.restore_backup_confirm_title,
                    )
                    .description(description)
                    .cancel_label(strings.cancel)
                    .confirm_label(strings.restore_action)
                    .destructive(true)
                    .on_cancel(cx.listener(|this, _, _, cx| {
                        this.pending_restore = None;
                        cx.notify();
                    }))
                    .on_confirm(cx.listener(|this, _, _, cx| {
                        this.confirm_restore(cx);
                    })),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_auto_backups(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let group = v_flex().py_1p5().rounded(px(8.)).bg(colors.surface_background);
        let content = if self.auto_backups.is_empty() {
            group.child(
                h_flex()
                    .debug_selector(|| "settings-auto-backups-empty".to_owned())
                    .h(px(ROW_HEIGHT))
                    .px_4()
                    .child(
                        Label::new(strings.auto_backups_empty)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
        } else {
            group.children(self.auto_backups.iter().enumerate().map(|(idx, entry)| {
                let primary = entry
                    .date
                    .map(|date| calendar::format_date(date, strings))
                    .unwrap_or_else(|| entry.filename.clone());
                let size_label = format_bytes(entry.size_bytes);
                let filename = entry.filename.clone();
                let path = entry.path.clone();
                h_flex()
                    .id(SharedString::from(format!("auto-backup-{idx}")))
                    .debug_selector(move || format!("auto-backup-{idx}"))
                    .h(px(ROW_HEIGHT))
                    .px_4()
                    .gap_3()
                    .child(
                        h_flex()
                            .id(SharedString::from(format!("auto-backup-info-{idx}")))
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .tooltip(Tooltip::text(filename))
                            .child(Label::new(primary).truncate())
                            .child(
                                Label::new(size_label)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("auto-backup-restore-{idx}")))
                            .debug_selector(move || format!("auto-backup-restore-{idx}"))
                            .px_2()
                            .py_1()
                            .rounded(px(4.))
                            .hover(|s| s.bg(colors.element_hover))
                            .cursor_pointer()
                            .child(Label::new(strings.restore_action).color(Color::Accent))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pending_restore = Some(path.clone());
                                cx.notify();
                            })),
                    )
            }))
        };
        v_flex()
            .debug_selector(|| "settings-auto-backups".to_owned())
            .child(div().pb_3().child(Label::new(strings.auto_backups).weight(FontWeight::MEDIUM)))
            .child(content)
            .into_any_element()
    }

    pub(super) fn render_db_integrity_row(&self, cx: &Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let warning = cx.theme().status().warning;
        let db_health = cx.try_global::<desktop_common::diagnostics::DbHealth>();
        let healthy = db_health.is_none_or(|health| health.healthy);
        let status_text = if healthy {
            format!("{} (v{})", strings.db_integrity_ok, self.schema_version)
        } else {
            format!("{} (v{})", strings.db_integrity_issue, self.schema_version)
        };
        let detail = db_health.filter(|health| !health.healthy).and_then(|health| {
            if let Some(backup) = &health.recovered_corrupt_backup {
                let file = backup
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("tasks.db.corrupt");
                Some(strings.db_corrupt_recovered.replace("{file}", file))
            } else if !health.messages.is_empty() {
                Some(
                    health
                        .messages
                        .iter()
                        .map(|m| desktop_common::diagnostics::redact_sensitive(m))
                        .collect::<Vec<_>>()
                        .join("; "),
                )
            } else {
                None
            }
        });
        v_flex()
            .debug_selector(|| "settings-db-integrity".to_owned())
            .when(healthy, |el| el.px_4().py_2())
            .when(!healthy, |el| {
                el.mx_2()
                    .my_1()
                    .px_2p5()
                    .py_2()
                    .rounded(px(6.))
                    .bg(warning.opacity(0.10))
                    .border_1()
                    .border_color(warning.opacity(0.35))
            })
            .gap_1()
            .child(
                h_flex()
                    .min_h(px(24.))
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(Label::new(strings.db_integrity).truncate()))
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1p5()
                            .items_center()
                            .child(
                                Icon::new(if healthy {
                                    IconName::CircleCheck
                                } else {
                                    IconName::CircleAlert
                                })
                                .size(IconSize::Small)
                                .color(if healthy { Color::Success } else { Color::Warning }),
                            )
                            .child(
                                Label::new(status_text)
                                    .size(LabelSize::Small)
                                    .color(if healthy { Color::Muted } else { Color::Warning }),
                            ),
                    ),
            )
            .when_some(detail, |el, detail| {
                el.child(
                    div()
                        .debug_selector(|| "settings-db-integrity-detail".to_owned())
                        .text_size(px(11.))
                        .text_color(warning)
                        .child(detail),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_diagnostics(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors().clone();
        v_flex()
            .child(div().pb_3().child(Label::new(strings.diagnostics).weight(FontWeight::MEDIUM)))
            .child(
                v_flex()
                    .py_1p5()
                    .rounded(px(8.))
                    .bg(colors.surface_background)
                    .child(self.render_db_integrity_row(cx))
                    .child(
                        h_flex()
                            .min_h(px(ROW_HEIGHT))
                            .px_4()
                            .py_1p5()
                            .child(
                                action_button("save-diagnostic-log", cx)
                                    .debug_selector(|| "save-diagnostic-log".to_owned())
                                    .border_color(colors.border)
                                    .child(
                                        Icon::new(IconName::FileText)
                                            .size(IconSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        Label::new(strings.save_diagnostic_log)
                                            .size(LabelSize::Small)
                                            .weight(FontWeight::MEDIUM),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save_diagnostic_log(cx);
                                    })),
                            ),
                    ),
            )
            .children(self.diagnostic_message.as_ref().map(|(message, is_error)| {
                div()
                    .pt_1p5()
                    .px_1()
                    .text_size(px(11.))
                    .text_color(if *is_error { theme.status().error } else { colors.text_muted })
                    .child(message.clone())
            }))
            .into_any_element()
    }
}

/// Formats a byte count into a compact human-readable string (e.g. `124 KB`, `1.4 MB`).
pub(super) fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if bytes >= 1024 {
        format!("{} KB", (bytes + 512) / 1024)
    } else {
        format!("{bytes} B")
    }
}
