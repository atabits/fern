use desktop_common::i18n;
use desktop_common::release_notes::ReleaseNotes;
use gpui::FontWeight;
use task_ui::task_detail;
use ui::prelude::*;
use ui::v_scroll;

use super::{ROW_HEIGHT, SettingsWindow, UpdateUiStatus};

impl SettingsWindow {
    /// About -> Updates: automatic checks, current status, and update action.
    pub(super) fn render_updates(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snap = (self.host.updater_snapshot)(cx);
        if !snap.supported {
            return None;
        }
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let version = |text: &str, version: &str| text.replace("{version}", version);
        let (message, action) = match &snap.status {
            UpdateUiStatus::Idle => (None, Some(strings.check_for_updates.to_owned())),
            UpdateUiStatus::Checking => (Some(strings.update_checking.to_owned()), None),
            UpdateUiStatus::UpToDate => (
                Some(strings.update_up_to_date.to_owned()),
                Some(strings.check_for_updates.to_owned()),
            ),
            UpdateUiStatus::Downloading(v) => {
                (Some(version(strings.update_downloading, v)), None)
            }
            UpdateUiStatus::Ready(notes) => (
                Some(version(strings.update_ready, &notes.version)),
                Some(strings.restart_to_update.to_owned()),
            ),
            UpdateUiStatus::Available(notes) => (
                Some(version(strings.update_available, &notes.version)),
                Some(version(strings.update_to, &notes.version)),
            ),
            UpdateUiStatus::Failed(error) => (
                Some(strings.update_failed.replace("{error}", error)),
                Some(strings.check_for_updates.to_owned()),
            ),
        };
        let notes = match snap.status {
            UpdateUiStatus::Ready(notes) | UpdateUiStatus::Available(notes) => Some(notes),
            _ => None,
        };
        let install = notes.is_some();
        let automatic = snap.automatic;
        let set_auto = self.host.set_auto_update.clone();
        let check_updates = self.host.check_for_updates.clone();
        let restart_update = self.host.restart_to_update.clone();
        Some(
            v_flex()
                .child(div().pb_3().child(Label::new(strings.updates).weight(FontWeight::MEDIUM)))
                .child(
                    v_flex()
                        .py_1p5()
                        .rounded(px(8.))
                        .bg(colors.surface_background)
                        .child(
                            h_flex()
                                .id("auto-update")
                                .h(px(ROW_HEIGHT))
                                .px_4()
                                .cursor_pointer()
                                .child(div().flex_1().child(Label::new(strings.check_updates_automatically)))
                                .child(Switch::new().toggle_state(automatic))
                                .on_click(move |_, _, cx| set_auto(!automatic, cx)),
                        )
                        .child(
                            h_flex()
                                .min_h(px(ROW_HEIGHT))
                                .py_2()
                                .px_4()
                                .gap_x_3()
                                .gap_y_1()
                                .flex_wrap()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(110.))
                                        .text_size(px(13.))
                                        .text_color(if install {
                                            colors.text
                                        } else {
                                            colors.text_muted
                                        })
                                        .child(
                                            message
                                                .unwrap_or_else(|| format!("Fern {}", env!("CARGO_PKG_VERSION"))),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .flex_none()
                                        .flex_wrap()
                                        .children(notes.map(|notes| {
                                            div()
                                                .debug_selector(|| "update-notes".to_owned())
                                                .child(
                                                    ui::Button::new("update-notes", strings.whats_new)
                                                        .variant(ui::ButtonVariant::Secondary)
                                                        .size(ui::ButtonSize::Sm)
                                                        .on_click(cx.listener(move |this, _, window, cx| {
                                                            this.show_release_notes(
                                                                notes.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                        })),
                                                )
                                        }))
                                        .children(action.map(|action| {
                                            let check = check_updates.clone();
                                            let restart = restart_update.clone();
                                            div()
                                                .debug_selector(|| "update-action".to_owned())
                                                .child(
                                                    ui::Button::new("update-action", action)
                                                        .variant(if install {
                                                            ui::ButtonVariant::Primary
                                                        } else {
                                                            ui::ButtonVariant::Secondary
                                                        })
                                                        .size(ui::ButtonSize::Sm)
                                                        .on_click(move |_, _, cx| {
                                                            if install {
                                                                restart(cx);
                                                            } else {
                                                                check(cx);
                                                            }
                                                        }),
                                                )
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_release_notes(&self, notes: &ReleaseNotes, cx: &App) -> AnyElement {
        let strings = i18n::t(cx);
        let title = strings
            .whats_new_in_version
            .replace("{version}", &notes.version);
        let url = notes.url.clone();
        v_scroll(
            SharedString::from(format!("release-notes-scroll-{}", notes.version)),
            v_flex()
                .id(SharedString::from(format!("release-notes-page-{}", notes.version)))
                .debug_selector(|| "release-notes-page".to_owned())
                .px_6()
                .pt_2()
                .pb_6()
                .gap_4()
                .child(Label::new(title).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                .child(if notes.markdown.is_empty() {
                    div()
                        .text_size(px(14.))
                        .child(strings.release_notes_missing)
                        .into_any_element()
                } else {
                    task_detail::render_release_markdown(&notes.markdown, cx)
                })
                .child(
                    ui::Button::new("release-notes-github", strings.release_on_github)
                        .variant(ui::ButtonVariant::Secondary)
                        .size(ui::ButtonSize::Sm)
                        .icon(IconName::SquareArrowOutUpRight)
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                ),
        )
        .flex_1()
        .min_h_0()
        .into_any_element()
    }
}
