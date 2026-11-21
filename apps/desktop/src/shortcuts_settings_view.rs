use super::*;
use desktop_common::keymap::{self, ResolvedShortcut, SHORTCUT_SPECS, ShortcutSection};
use gpui::KeyDownEvent;
use ui::{Button, ButtonVariant, ConfirmDialog};

/// Which shortcut slot in Settings → Shortcuts is currently capturing a key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecordingTarget {
    Action(usize),
    #[cfg(windows)]
    Global(crate::hotkeys::Hotkey),
}

const PILL_WIDTH: f32 = 168.;
const PILL_HEIGHT: f32 = 28.;

impl SettingsWindow {
    pub(super) fn open_user_keymap(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move { keymap::ensure_user_keymap_file() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |_, cx| match result {
                Ok(path) => cx.open_with_system(&path),
                Err(err) => {
                    let message = format!("{err:#}");
                    eprintln!("keymap: {message}");
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
                        read_errors.iter().for_each(|err| eprintln!("keymap: {err}"));
                        keymap::set_errors(read_errors, cx);
                    }
                    Err(err) => {
                        eprintln!("failed to reload the keymap: {err:#}");
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
        let default_source = match gpui::AssetSource::load(&assets::Assets, keymap::DEFAULT_KEYMAP_PATH) {
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
                            errors.iter().for_each(|err| eprintln!("keymap: {err}"));
                            keymap::set_errors(errors, cx);
                        }
                        Err(err) => {
                            eprintln!("failed to apply shortcut: {err:#}");
                            keymap::set_errors(vec![err.to_string()], cx);
                        }
                    }
                    cx.refresh_windows();
                }
                Err(err) => {
                    let message = format!("{err:#}");
                    eprintln!("keymap: {message}");
                    keymap::set_errors(vec![message], cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn reset_all_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.recording_shortcut = None;
        self.confirm_reset_shortcuts = false;
        crate::hotkeys::reset_defaults(cx);
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
        let Some(target) = self.recording_shortcut else {
            return;
        };
        cx.stop_propagation();

        let ks = &event.keystroke;
        let unmodified = !ks.modifiers.control && !ks.modifiers.alt && !ks.modifiers.shift && !ks.modifiers.platform;
        if unmodified {
            match ks.key.as_str() {
                "escape" => {
                    self.recording_shortcut = None;
                    cx.notify();
                    return;
                }
                "backspace" | "delete" => {
                    self.recording_shortcut = None;
                    match target {
                        RecordingTarget::Action(idx) => self.apply_action_shortcut(idx, None, cx),
                        #[cfg(windows)]
                        RecordingTarget::Global(hotkey) => crate::hotkeys::set(hotkey, None, cx),
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
                    self.recording_shortcut = None;
                    cx.notify();
                    return;
                };
                if let Some(config_key) = keymap::record_keystroke(ks, spec.allow_unmodified) {
                    self.recording_shortcut = None;
                    self.apply_action_shortcut(idx, Some(config_key), cx);
                    cx.notify();
                }
            }
            #[cfg(windows)]
            RecordingTarget::Global(hotkey) => {
                if let Some(binding) = crate::hotkeys::Binding::from_keystroke(ks) {
                    self.recording_shortcut = None;
                    crate::hotkeys::set(hotkey, Some(binding), cx);
                    cx.notify();
                }
            }
        }
    }

    /// Scrollable content + sticky bottom bar for `Page::Shortcuts`.
    pub(super) fn render_shortcuts_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors().clone();
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
                    el.child(div().text_size(px(12.)).text_color(colors.text_muted).child(hint))
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
                    .children(keymap_errors.into_iter().map(|err| {
                        div().text_size(px(12.)).text_color(error_color).child(err)
                    })),
            );
        }

        // 0: General
        scroll_body = scroll_body.child(render_section(ShortcutSection::General, 0, cx));

        // 1: Global Actions (Windows)
        #[cfg(windows)]
        {
            scroll_body = scroll_body.child(
                v_flex()
                    .child(section_header(1))
                    .child(group_card().children(self.render_global_shortcut_rows(cx))),
            );
        }

        // 2: New Task
        scroll_body = scroll_body.child(render_section(ShortcutSection::NewTask, 2, cx));

        // 3: Quick Add Task syntax (read-only reference)
        scroll_body = scroll_body.child(
            v_flex()
                .child(section_header(3))
                .child(group_card().children(strings.quick_add_help.into_iter().map(|(what, example)| {
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
                }))),
        );

        // 4: Edit Task
        scroll_body = scroll_body.child(render_section(ShortcutSection::EditTask, 4, cx));
        // 5: Navigation
        scroll_body = scroll_body.child(render_section(ShortcutSection::Navigation, 5, cx));
        // 6: Switch Views
        scroll_body = scroll_body.child(render_section(ShortcutSection::SwitchView, 6, cx));
        // 7: Focus
        scroll_body = scroll_body.child(render_section(ShortcutSection::Focus, 7, cx));
        // 8: Sticky Note
        scroll_body = scroll_body.child(render_section(ShortcutSection::StickyNote, 8, cx));

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
                                this.recording_shortcut = None;
                                this.confirm_reset_shortcuts = true;
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
                    .child(VerticalScrollbar::for_handle("settings-page-scroll", self.page_scroll.clone())),
            )
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
        let colors = theme.colors().clone();
        let idx = item.index;
        let label = strings.shortcut_labels[idx];
        let recording = self.recording_shortcut == Some(RecordingTarget::Action(idx));
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
            conflict_tip,
            cx.listener(move |this, _, window, cx| {
                this.recording_shortcut = if this.recording_shortcut == Some(RecordingTarget::Action(idx)) {
                    None
                } else {
                    Some(RecordingTarget::Action(idx))
                };
                window.focus(&this.focus_handle, cx);
                cx.notify();
            }),
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.recording_shortcut = None;
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
                                .justify_center()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(colors.border.opacity(0.6))
                                .child(
                                    Label::new(format!("{first} {} {second}", strings.shortcut_then))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        )
                        .child(Label::new(strings.shortcut_or).size(LabelSize::Small).color(Color::Muted))
                    })
                    .child(pill),
            )
            .into_any_element()
    }

    #[cfg(windows)]
    fn render_global_shortcut_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let all = crate::hotkeys::bindings(cx);
        all.iter()
            .map(|(hotkey, binding, taken)| {
                let hotkey = *hotkey;
                let idx = hotkey.index();
                let recording = self.recording_shortcut == Some(RecordingTarget::Global(hotkey));
                let has_binding = binding.is_some();
                let duplicates: Vec<&str> = binding
                    .as_ref()
                    .map(|b| {
                        all.iter()
                            .filter(|(other, other_b, _)| *other != hotkey && other_b.as_ref() == Some(b))
                            .map(|(other, _, _)| other.label(strings))
                            .collect()
                    })
                    .unwrap_or_default();
                let conflict_tip = if !duplicates.is_empty() {
                    Some(strings.shortcut_conflict.replace("{other}", &duplicates.join(", ")))
                } else if *taken {
                    Some(strings.shortcut_taken.to_owned())
                } else {
                    None
                };
                let has_conflict = conflict_tip.is_some() && !recording;
                let pill_text = if recording {
                    strings.press_shortcut.to_owned()
                } else {
                    binding
                        .as_ref()
                        .map_or_else(|| strings.shortcut_set.to_owned(), |b| b.label())
                };
                let group_name = SharedString::from(format!("shortcut-pill-global-{idx}"));

                let pill = self.render_editable_pill(
                    SharedString::from(format!("global-shortcut-{idx}")),
                    group_name,
                    pill_text,
                    recording,
                    has_binding,
                    has_conflict,
                    conflict_tip,
                    cx.listener(move |this, _, window, cx| {
                        this.recording_shortcut = if this.recording_shortcut == Some(RecordingTarget::Global(hotkey)) {
                            None
                        } else {
                            Some(RecordingTarget::Global(hotkey))
                        };
                        window.focus(&this.focus_handle, cx);
                        cx.notify();
                    }),
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.recording_shortcut = None;
                        crate::hotkeys::set(hotkey, None, cx);
                        cx.notify();
                    }),
                    cx,
                );

                h_flex()
                    .h(px(ROW_HEIGHT))
                    .px_4()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(Label::new(hotkey.label(strings)).truncate()))
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

        let border_color = if recording {
            colors.accent
        } else if has_conflict {
            error_color
        } else if has_binding {
            colors.border
        } else {
            colors.border.opacity(0.75)
        };
        let text_color = if recording {
            Color::Accent
        } else if has_conflict {
            Color::Error
        } else if has_binding {
            Color::Default
        } else {
            Color::Muted
        };
        let hover_border = if recording {
            colors.accent
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
                el.bg(colors.accent.opacity(0.10)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.recording_shortcut = None;
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

    pub(super) fn render_reset_shortcuts_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.confirm_reset_shortcuts {
            return None;
        }
        let strings = i18n::t(cx);
        Some(
            ConfirmDialog::new("confirm-reset-shortcuts", strings.shortcut_reset_confirm_title)
                .description(strings.shortcut_reset_confirm_body)
                .cancel_label(strings.cancel)
                .confirm_label(strings.shortcut_reset_all)
                .destructive(true)
                .on_cancel(cx.listener(|this, _, _, cx| {
                    this.confirm_reset_shortcuts = false;
                    cx.notify();
                }))
                .on_confirm(cx.listener(|this, _, _, cx| {
                    this.reset_all_shortcuts(cx);
                }))
                .into_any_element(),
        )
    }
}
