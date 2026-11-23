use gpui::FontWeight;
use ui::prelude::*;

use crate::i18n;
use crate::release_notes::{ReleaseNotes, action_button};

use super::{ROW_HEIGHT, SettingsWindow};

impl SettingsWindow {
    /// About → Updates: the automatic checks, how the last one went and what to do next.
    pub(super) fn render_updates(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::updater::Status;
        let updater = crate::updater::updater(cx).filter(|updater| updater.read(cx).supported())?;
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let (status, automatic) = {
            let updater = updater.read(cx);
            (updater.status().clone(), updater.automatic())
        };
        let version = |text: &str, version: &str| text.replace("{version}", version);
        let (message, action) = match &status {
            Status::Idle => (None, Some(strings.check_for_updates.to_owned())),
            Status::Checking => (Some(strings.update_checking.to_owned()), None),
            Status::UpToDate => (Some(strings.update_up_to_date.to_owned()), Some(strings.check_for_updates.to_owned())),
            Status::Downloading(v) => (Some(version(strings.update_downloading, v)), None),
            Status::Ready(notes) => (Some(version(strings.update_ready, &notes.version)), Some(strings.restart_to_update.to_owned())),
            Status::Available(notes) => (Some(version(strings.update_available, &notes.version)), Some(version(strings.update_to, &notes.version))),
            Status::Failed(error) => (Some(strings.update_failed.replace("{error}", error)), Some(strings.check_for_updates.to_owned())),
        };
        let notes = match status {
            Status::Ready(notes) | Status::Available(notes) => Some(notes),
            _ => None,
        };
        let install = notes.is_some();
        let switch_updater = updater.clone();
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
                                .on_click(move |_, _, cx| switch_updater.update(cx, |updater, cx| updater.set_automatic(!automatic, cx))),
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
                                        .text_color(if install { colors.text } else { colors.text_muted })
                                        .child(message.unwrap_or_else(|| format!("Fern {}", env!("CARGO_PKG_VERSION")))),
                                )
                                .child(h_flex().gap_2().flex_none().flex_wrap()
                                    .children(notes.map(|notes| {
                                        action_button("update-notes", cx)
                                            .debug_selector(|| "update-notes".to_owned())
                                            .border_color(colors.border)
                                            .child(Label::new(strings.whats_new).size(LabelSize::Small))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.show_release_notes(notes.clone(), window, cx);
                                            }))
                                    }))
                                    .children(action.map(|action| {
                                        action_button("update-action", cx)
                                            .debug_selector(|| "update-action".to_owned())
                                            .when(install, |el| el.bg(colors.accent.opacity(0.12)).border_color(colors.accent.opacity(0.35)))
                                            .child(Label::new(action).size(LabelSize::Small).weight(FontWeight::MEDIUM))
                                            .on_click(move |_, _, cx| {
                                                updater.update(cx, |updater, cx| {
                                                    if install {
                                                        updater.restart_to_update(cx)
                                                    } else {
                                                        updater.check(cx)
                                                    }
                                                });
                                            })
                                    })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_release_notes(&self, notes: &ReleaseNotes, cx: &App) -> AnyElement {
        let strings = i18n::t(cx);
        let title = strings.whats_new_in_version.replace("{version}", &notes.version);
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
                    div().text_size(px(14.)).child(strings.release_notes_missing).into_any_element()
                } else {
                    crate::task_detail::render_release_markdown(&notes.markdown, cx)
                })
                .child(
                    action_button("release-notes-github", cx)
                        .self_start()
                        .border_color(cx.theme().colors().border)
                        .child(Label::new(strings.release_on_github).size(LabelSize::Small))
                        .child(Icon::new(IconName::SquareArrowOutUpRight).size(IconSize::Small).color(Color::Muted))
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                ),
        )
        .flex_1()
        .min_h_0()
        .into_any_element()
    }
}
