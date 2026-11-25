use desktop_common::i18n;
use desktop_common::keymap::{self, ResolvedShortcut, SHORTCUT_SPECS, ShortcutSection};
use gpui::{FontWeight, KeyDownEvent, MouseButton};
use ui::prelude::*;
use ui::VerticalScrollbar;

use super::{ActiveConfirm, ROW_HEIGHT, SettingsWindow};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecordingTarget {
    Action(usize),
    Global(usize),
}

const PILL_WIDTH: f32 = 168.;
const PILL_HEIGHT: f32 = 28.;

fn replace_platform_modifier(text: &str, platform_key: &str) -> SharedString {
    if platform_key == "Win" {
        text.to_owned().into()
    } else {
        text.replace("Win", platform_key).into()
    }
}

fn platform_shortcut_text(text: &str) -> SharedString {
    let platform_key = if cfg!(target_os = "macos") {
        "Cmd"
    } else if cfg!(target_os = "linux") {
        "Super"
    } else {
        "Win"
    };
    replace_platform_modifier(text, platform_key)
}

impl SettingsWindow {
    pub(super) fn open_user_keymap(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move { keymap::ensure_user_keymap_file() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |_, cx| match result {
                Ok(path) => cx.open_with_system(&path),
                Err(err) => {
                    let message = format!("{err:#}");
                    desktop_common::logging::log_error("keymap", &message);
                    keymap::set_errors(vec![message], cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn reload_keymap(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move {
            let mut read_errors = Vec::new();
            let user_keymap = match keymap::read_user_keymap_file() {
                Ok(contents) => contents,
                Err(err) => {
                    read_errors.push(format!("{}: {err}", keymap::USER_KEYMAP_FILE));
                    None
                }
            };
            (user_keymap, read_errors)
        });
        cx.spawn(async move |this, cx| {
            let (user_keymap, mut read_errors) = task.await;
            this.update(cx, |_, cx| {
                cx.clear_key_bindings();
                match keymap::load_with_user(&assets::Assets, user_keymap.as_deref(), cx) {
                    Ok(mut errors) => {
                        read_errors.append(&mut errors);
                        read_errors
                            .iter()
                            .for_each(|err| desktop_common::logging::log_warn("keymap", err));
                        keymap::set_errors(read_errors, cx);
                    }
                    Err(err) => {
                        desktop_common::logging::log_error(
                            "keymap",
                            format!("failed to reload the keymap: {err:#}"),
                        );
                        read_errors.push(err.to_string());
                        keymap::set_errors(read_errors, cx);
                    }
                }
                cx.refresh_windows();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn apply_action_shortcut(
        &mut self,
        spec_index: usize,
        new_key: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(spec) = SHORTCUT_SPECS.get(spec_index).copied() else {
            return;
        };
        let default_source =
            match gpui::AssetSource::load(&assets::Assets, keymap::DEFAULT_KEYMAP_PATH) {
                Ok(Some(bytes)) => match String::from_utf8(bytes.into_owned()) {
                    Ok(text) => text,
                    Err(err) => {
                        keymap::set_errors(vec![format!("{err}")], cx);
                        cx.notify();
                        return;
                    }
                },
                Ok(None) => {
                    keymap::set_errors(vec![format!("{} is missing", keymap::DEFAULT_KEYMAP_PATH)], cx);
                    cx.notify();
                    return;
                }
                Err(err) => {
                    keymap::set_errors(vec![format!("{err:#}")], cx);
                    cx.notify();
                    return;
                }
            };

        let action = spec.action;
        let task = cx.background_spawn(async move {
            keymap::write_action_shortcut_to_file(&default_source, action, new_key.as_deref())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |_, cx| match result {
                Ok(updated_source) => {
                    cx.clear_key_bindings();
                    match keymap::load_with_user(&assets::Assets, Some(&updated_source), cx) {
                        Ok(errors) => {
                            errors
                                .iter()
                                .for_each(|err| desktop_common::logging::log_warn("keymap", err));
                            keymap::set_errors(errors, cx);
                        }
                        Err(err) => {
                            desktop_common::logging::log_error(
                                "keymap",
                                format!("failed to apply shortcut: {err:#}"),
                            );
                            keymap::set_errors(vec![err.to_string()], cx);
                        }
                    }
                    cx.refresh_windows();
                }
                Err(err) => {
                    let message = format!("{err:#}");
                    desktop_common::logging::log_error("keymap", &message);
                    keymap::set_errors(vec![message], cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn reset_all_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.shortcuts.recording = None;
        self.shortcuts.rejected = false;
        self.confirm = None;
        (self.host.reset_global_hotkeys)(cx);
        let task = cx.background_spawn(async move { keymap::reset_user_keymap_file() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |_, cx| {
                let mut errors = Vec::new();
                if let Err(err) = result {
                    errors.push(format!("{err:#}"));
                }
                cx.clear_key_bindings();
                match keymap::load_default(&assets::Assets, cx) {
                    Ok(mut load_errors) => errors.append(&mut load_errors),
                    Err(err) => errors.push(err.to_string()),
                }
                keymap::set_errors(errors, cx);
                cx.refresh_windows();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn handle_shortcut_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.shortcuts.recording else {
            return;
        };
        cx.stop_propagation();

        let ks = &event.keystroke;
        let unmodified =
            !ks.modifiers.control && !ks.modifiers.alt && !ks.modifiers.shift && !ks.modifiers.platform;
        if unmodified {
            match ks.key.as_str() {
                "escape" => {
                    self.shortcuts.recording = None;
                    self.shortcuts.rejected = false;
                    cx.notify();
                    return;
                }
                "backspace" | "delete" => {
                    self.shortcuts.recording = None;
                    self.shortcuts.rejected = false;
                    match target {
                        RecordingTarget::Action(idx) => self.apply_action_shortcut(idx, None, cx),
                        RecordingTarget::Global(idx) => {
                            let _ = (self.host.set_global_hotkey)(idx, None, cx);
                        }
                    }
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }

        match target {
            RecordingTarget::Action(idx) => {
                let Some(spec) = SHORTCUT_SPECS.get(idx) else {
                    self.shortcuts.recording = None;
                    self.shortcuts.rejected = false;
                    cx.notify();
                    return;
                };
                match keymap::classify_keystroke(ks, spec.allow_unmodified) {
                    keymap::RecordOutcome::IgnoreModifier => {}
                    keymap::RecordOutcome::Accepted(config_key) => {
                        self.shortcuts.recording = None;
                        self.shortcuts.rejected = false;
                        self.apply_action_shortcut(idx, Some(config_key), cx);
                        cx.notify();
                    }
                    keymap::RecordOutcome::Rejected => {
                        self.shortcuts.rejected = true;
                        cx.notify();
                    }
                }
            }
            RecordingTarget::Global(idx) => {
                if keymap::is_bare_modifier(ks) {
                    return;
                }
                if (self.host.set_global_hotkey)(idx, Some(ks), cx) {
                    self.shortcuts.recording = None;
                    self.shortcuts.rejected = false;
                    cx.notify();
                } else {
                    self.shortcuts.rejected = true;
                    cx.notify();
                }
            }
        }
    }

    pub(super) fn render_shortcuts_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let error_color = theme.status().error;
        let resolved: Vec<ResolvedShortcut> = keymap::cached_resolved_shortcuts(cx).to_vec();
        let keymap_errors = keymap::errors(cx).to_vec();

        let group_card = || v_flex().py_1p5().rounded(px(8.)).bg(colors.surface_background);
        let section_header = |group_idx: usize| {
            let title = strings.shortcut_groups[group_idx];
            let hint = strings.shortcut_group_hints[group_idx];
            v_flex()
                .pb_2p5()
                .gap_1()
                .child(Label::new(title).weight(FontWeight::SEMIBOLD))
                .when_some(hint, |el, hint| {
                    el.child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child(platform_shortcut_text(hint)),
                    )
                })
        };

        let render_section = |section: ShortcutSection, group_idx: usize, cx: &mut Context<Self>| -> AnyElement {
            let rows: Vec<AnyElement> = resolved
                .iter()
                .filter(|item| item.spec.section == section)
                .map(|item| self.render_action_shortcut_row(item, cx))
                .collect();
            v_flex()
                .child(section_header(group_idx))
                .child(group_card().children(rows))
                .into_any_element()
        };

        let mut scroll_body = v_flex()
            .id("settings-page")
            .debug_selector(|| "settings-shortcuts-page".to_owned())
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.page_scroll)
            .pt_1()
            .px_6()
            .pb_6()
            .gap_5();

        if !keymap_errors.is_empty() {
            scroll_body = scroll_body.child(
                v_flex()
                    .debug_selector(|| "shortcuts-keymap-errors".to_owned())
                    .p_3()
                    .gap_1()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(error_color.opacity(0.45))
                    .bg(error_color.opacity(0.08))
                    .child(
                        Label::new(strings.keymap_errors)
                            .size(LabelSize::Small)
                            .weight(FontWeight::SEMIBOLD)
                            .color(Color::Error),
                    )
                    .children(
                        keymap_errors
                            .into_iter()
                            .map(|err| div().text_size(px(12.)).text_color(error_color).child(err)),
                    ),
            );
        }

        scroll_body = scroll_body.child(render_section(ShortcutSection::General, 0, cx));

        scroll_body = scroll_body.child(
            v_flex()
                .child(section_header(1))
                .when(desktop_common::is_wayland_session(), |el| {
                    el.child(
                        div()
                            .mb_2()
                            .px_3()
                            .py_2()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(colors.border.opacity(0.6))
                            .bg(colors.surface_background)
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child(strings.wayland_hotkeys_hint),
                    )
                })
                .child(group_card().children(self.render_global_shortcut_rows(cx))),
        );

        scroll_body = scroll_body.child(render_section(ShortcutSection::NewTask, 2, cx));

        scroll_body = scroll_body.child(
            v_flex().child(section_header(3)).child(
                group_card().children(strings.quick_add_help.into_iter().map(|(what, example)| {
                    h_flex()
                        .h(px(ROW_HEIGHT))
                        .px_4()
                        .gap_3()
                        .child(div().flex_1().min_w_0().child(Label::new(what).truncate()))
                        .child(
                            div()
                                .min_w(px(PILL_WIDTH))
                                .h(px(PILL_HEIGHT))
                                .px_2p5()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(colors.border.opacity(0.65))
                                .child(Label::new(example).size(LabelSize::Small).color(Color::Muted)),
                        )
                })),
            ),
        );

        scroll_body = scroll_body.child(render_section(ShortcutSection::EditTask, 4, cx));
        scroll_body = scroll_body.child(render_section(ShortcutSection::Navigation, 5, cx));
        scroll_body = scroll_body.child(render_section(ShortcutSection::SwitchView, 6, cx));
        scroll_body = scroll_body.child(render_section(ShortcutSection::Focus, 7, cx));
        scroll_body = scroll_body.child(render_section(ShortcutSection::StickyNote, 8, cx));

        let global_list = (self.host.global_shortcuts)(strings, cx);
        let recording_banner = self.shortcuts.recording.map(|target| {
            let (name, rule_idx) = match target {
                RecordingTarget::Action(idx) => {
                    let allow_unmodified =
                        SHORTCUT_SPECS.get(idx).is_some_and(|s| s.allow_unmodified);
                    (strings.shortcut_labels[idx], usize::from(allow_unmodified))
                }
                RecordingTarget::Global(idx) => {
                    let label = global_list.get(idx).map_or("", |g| g.label);
                    (label, 2)
                }
            };
            let rejected = self.shortcuts.rejected;
            let message = platform_shortcut_text(if rejected {
                strings.shortcut_record_rejected[rule_idx]
            } else {
                strings.shortcut_record_hints[rule_idx]
            });
            let accent_or_error = if rejected { error_color } else { colors.accent };
            h_flex()
                .flex_none()
                .debug_selector(|| "shortcuts-recording-hint".to_owned())
                .px_6()
                .py_2()
                .gap_2()
                .items_center()
                .border_t_1()
                .border_color(accent_or_error.opacity(0.35))
                .bg(accent_or_error.opacity(0.08))
                .child(
                    Icon::new(if rejected { IconName::Info } else { IconName::Keyboard })
                        .size(IconSize::Small)
                        .color(if rejected { Color::Error } else { Color::Accent }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(if rejected { error_color } else { colors.text })
                        .child(format!("{name}: {message}")),
                )
        });

        let footer = h_flex()
            .flex_none()
            .h(px(54.))
            .px_6()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(colors.border.opacity(0.6))
            .bg(colors.background)
            .child(
                Button::new("edit-keymap", strings.edit_keymap)
                    .variant(ButtonVariant::Secondary)
                    .icon(IconName::FileText)
                    .on_click(cx.listener(|this, _, _, cx| this.open_user_keymap(cx))),
            )
            .child(
                IconButton::new("reload-keymap", IconName::RefreshCw)
                    .icon_color(Color::Muted)
                    .tooltip(Tooltip::text(strings.reload_keymap))
                    .on_click(cx.listener(|this, _, _, cx| this.reload_keymap(cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .debug_selector(|| "shortcuts-reset-all".to_owned())
                    .child(
                        Button::new("reset-shortcuts", strings.shortcut_reset_all)
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.shortcuts.recording = None;
                                this.shortcuts.rejected = false;
                                this.confirm = Some(ActiveConfirm::ResetShortcuts);
                                cx.notify();
                            })),
                    ),
            );

        v_flex()
            .flex_1()
            .min_h_0()
            .size_full()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(scroll_body)
                    .child(VerticalScrollbar::for_handle(
                        "settings-page-scroll",
                        self.page_scroll.clone(),
                    )),
            )
            .children(recording_banner)
            .child(footer)
            .into_any_element()
    }

    fn render_action_shortcut_row(
        &self,
        item: &ResolvedShortcut,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let idx = item.index;
        let label = strings.shortcut_labels[idx];
        let recording = self.shortcuts.recording == Some(RecordingTarget::Action(idx));
        let direct = item.display_direct();
        let has_binding = direct.is_some();
        let conflict_tip = (!item.conflicts.is_empty()).then(|| {
            let names: Vec<&str> = item
                .conflicts
                .iter()
                .filter_map(|&other| strings.shortcut_labels.get(other).copied())
                .collect();
            strings.shortcut_conflict.replace("{other}", &names.join(", "))
        });
        let has_conflict = conflict_tip.is_some() && !recording;
        let rule_tip = strings.shortcut_rule_tips[usize::from(item.spec.allow_unmodified)];
        let pill_tooltip =
            conflict_tip.or_else(|| (!recording).then(|| platform_shortcut_text(rule_tip).to_string()));
        let pill_text = if recording {
            strings.press_shortcut.to_owned()
        } else {
            direct.unwrap_or_else(|| strings.shortcut_set.to_owned())
        };
        let group_name = SharedString::from(format!("shortcut-pill-action-{idx}"));

        let pill = self.render_editable_pill(
            SharedString::from(format!("shortcut-action-{idx}")),
            group_name,
            pill_text,
            recording,
            has_binding,
            has_conflict,
            pill_tooltip,
            cx.listener(move |this, _, window, cx| {
                this.shortcuts.rejected = false;
                this.shortcuts.recording =
                    if this.shortcuts.recording == Some(RecordingTarget::Action(idx)) {
                        None
                    } else {
                        Some(RecordingTarget::Action(idx))
                    };
                window.focus(&this.focus_handle, cx);
                cx.notify();
            }),
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.shortcuts.recording = None;
                this.shortcuts.rejected = false;
                this.apply_action_shortcut(idx, None, cx);
                cx.notify();
            }),
            cx,
        );

        h_flex()
            .h(px(ROW_HEIGHT))
            .px_4()
            .gap_3()
            .child(div().flex_1().min_w_0().child(Label::new(label).truncate()))
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .when_some(item.fixed_chord_parts.as_ref(), |el, (first, second)| {
                        el.child(
                            div()
                                .h(px(PILL_HEIGHT))
                                .px_2p5()
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(colors.border.opacity(0.6))
                                .child(
                                    Label::new(format!("{first} {second}"))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        )
                    })
                    .child(pill),
            )
            .into_any_element()
    }

    fn render_global_shortcut_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let all = (self.host.global_shortcuts)(strings, cx);
        all.iter()
            .enumerate()
            .map(|(idx, entry)| {
                let recording = self.shortcuts.recording == Some(RecordingTarget::Global(idx));
                let has_binding = entry.binding.is_some();
                let duplicates: Vec<&str> = entry
                    .binding
                    .as_deref()
                    .map(|b| {
                        all.iter()
                            .enumerate()
                            .filter(|(other_i, other)| {
                                *other_i != idx && other.binding.as_deref() == Some(b)
                            })
                            .map(|(_, other)| other.label)
                            .collect()
                    })
                    .unwrap_or_default();
                let conflict_tip = if !duplicates.is_empty() {
                    Some(
                        strings
                            .shortcut_conflict
                            .replace("{other}", &duplicates.join(", ")),
                    )
                } else if entry.taken {
                    Some(strings.shortcut_taken.to_owned())
                } else {
                    None
                };
                let has_conflict = conflict_tip.is_some() && !recording;
                let pill_tooltip = conflict_tip.or_else(|| {
                    (!recording)
                        .then(|| platform_shortcut_text(strings.shortcut_rule_tips[2]).to_string())
                });
                let pill_text = if recording {
                    strings.press_shortcut.to_owned()
                } else {
                    entry
                        .binding
                        .clone()
                        .unwrap_or_else(|| strings.shortcut_set.to_owned())
                };
                let group_name = SharedString::from(format!("shortcut-pill-global-{idx}"));

                let pill = self.render_editable_pill(
                    SharedString::from(format!("global-shortcut-{idx}")),
                    group_name,
                    pill_text,
                    recording,
                    has_binding,
                    has_conflict,
                    pill_tooltip,
                    cx.listener(move |this, _, window, cx| {
                        this.shortcuts.rejected = false;
                        this.shortcuts.recording =
                            if this.shortcuts.recording == Some(RecordingTarget::Global(idx)) {
                                None
                            } else {
                                Some(RecordingTarget::Global(idx))
                            };
                        window.focus(&this.focus_handle, cx);
                        cx.notify();
                    }),
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.shortcuts.recording = None;
                        this.shortcuts.rejected = false;
                        let _ = (this.host.set_global_hotkey)(idx, None, cx);
                        cx.notify();
                    }),
                    cx,
                );

                h_flex()
                    .h(px(ROW_HEIGHT))
                    .px_4()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(Label::new(entry.label).truncate()))
                    .child(pill)
                    .into_any_element()
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_editable_pill(
        &self,
        id: SharedString,
        group_name: SharedString,
        text: String,
        recording: bool,
        has_binding: bool,
        has_conflict: bool,
        tooltip: Option<String>,
        on_start: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        on_clear: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme();
        let colors = theme.colors();
        let error_color = theme.status().error;

        let border_color = if recording && self.shortcuts.rejected {
            error_color
        } else if recording {
            colors.accent
        } else if has_conflict {
            error_color
        } else if has_binding {
            colors.border
        } else {
            colors.border.opacity(0.75)
        };
        let text_color = if recording && self.shortcuts.rejected {
            Color::Error
        } else if recording {
            Color::Accent
        } else if has_conflict {
            Color::Error
        } else if has_binding {
            Color::Default
        } else {
            Color::Muted
        };
        let hover_border = if recording {
            border_color
        } else if has_conflict {
            error_color
        } else {
            colors.text_muted.opacity(0.6)
        };
        let clear_id = SharedString::from(format!("{id}-clear"));
        let debug_id = id.clone();

        div()
            .id(id)
            .debug_selector(move || debug_id.to_string())
            .group(group_name.clone())
            .relative()
            .w(px(PILL_WIDTH))
            .h(px(PILL_HEIGHT))
            .px_2()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .border_1()
            .border_color(border_color)
            .when(recording, |el| {
                let fill = if self.shortcuts.rejected {
                    error_color
                } else {
                    colors.accent
                };
                el.bg(fill.opacity(0.10))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.shortcuts.recording = None;
                        this.shortcuts.rejected = false;
                        cx.notify();
                    }))
            })
            .when(!recording, |el| el.hover(move |s| s.border_color(hover_border)))
            .cursor_pointer()
            .when_some(tooltip, |el, tip| el.tooltip(Tooltip::text(tip)))
            .on_click(on_start)
            .child(
                div()
                    .max_w_full()
                    .when(has_binding && !recording, |el| el.px_3())
                    .child(Label::new(text).size(LabelSize::Small).color(text_color).truncate()),
            )
            .when(has_binding && !recording, |el| {
                let hover_bg = colors.element_hover;
                el.child(
                    div()
                        .id(clear_id)
                        .absolute()
                        .right(px(4.))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .opacity(0.)
                        .group_hover(group_name, |s| s.opacity(1.))
                        .hover(move |s| s.bg(hover_bg))
                        .tooltip(Tooltip::text(strings.clear))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(on_clear)
                        .child(Icon::new(IconName::X).size(IconSize::XSmall).color(Color::Muted)),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_shortcut_text_substitutes_platform_modifier_across_locales() {
        for locale in [i18n::Locale::En, i18n::Locale::Ru, i18n::Locale::ZhCn] {
            let strings = locale.strings();
            for text in strings
                .shortcut_rule_tips
                .iter()
                .chain(strings.shortcut_record_hints.iter())
                .chain(strings.shortcut_record_rejected.iter())
                .copied()
                .chain(strings.shortcut_group_hints.iter().flatten().copied())
            {
                if text.contains("Win") {
                    let mac = replace_platform_modifier(text, "Cmd");
                    assert!(mac.contains("Cmd") && !mac.contains("Win"), "{locale:?}: {mac}");
                    let linux = replace_platform_modifier(text, "Super");
                    assert!(
                        linux.contains("Super") && !linux.contains("Win"),
                        "{locale:?}: {linux}"
                    );
                    let win = replace_platform_modifier(text, "Win");
                    assert_eq!(win.as_ref(), text);
                }
            }
        }
        let sample = "Ctrl, Alt, or Win + key";
        let expected = if cfg!(target_os = "macos") {
            "Ctrl, Alt, or Cmd + key"
        } else if cfg!(target_os = "linux") {
            "Ctrl, Alt, or Super + key"
        } else {
            sample
        };
        assert_eq!(platform_shortcut_text(sample).as_ref(), expected);
    }
}
