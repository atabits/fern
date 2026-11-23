use std::sync::Arc;

use fern_core::{ThemeMode, ThemeSelection};
use gpui::{FontWeight, PromptLevel, SharedString, WeakEntity};
use theme::{Theme, ThemeRegistry};
use ui::prelude::*;

use super::{ROW_HEIGHT, SettingsWindow};
use crate::user_themes::{
    self, TemporaryTheme, apply_theme, clear_temporary, remembered, set_temporary, system_dark,
};
use crate::i18n;

fn night_swatches(
    night: &Theme,
    show_tooltips: bool,
    pick: impl Fn(Arc<Theme>, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> ui::HorizontalStack {
    let colors = cx.theme().colors();
    let registry = ThemeRegistry::global(cx);
    let dark_names = i18n::t(cx).dark_theme_names;
    let bundled = registry
        .bundled_dark()
        .iter()
        .enumerate()
        .map(|(idx, theme)| {
            let name = if theme.is_custom() {
                SharedString::from(theme.name().to_owned())
            } else {
                SharedString::from(dark_names.get(idx).copied().unwrap_or_else(|| theme.name()).to_owned())
            };
            (theme.clone(), name)
        });
    let custom = registry.custom_dark().iter().map(|theme| {
        let name = SharedString::from(theme.name().to_owned());
        (theme.clone(), name)
    });

    h_flex().flex_wrap().gap_2().children(bundled.chain(custom).map(|(theme, name)| {
        let selected = theme.id() == night.id();
        let pick = pick.clone();
        let id = SharedString::from(format!("night-swatch-{}", theme.id()));
        div()
            .id(id)
            .tab_index(0)
            .aria_label(name.clone())
            .role(gpui::Role::RadioButton)
            .aria_toggled(selected.into())
            .size(px(30.))
            .flex_none()
            .rounded_full()
            .border_2()
            .p(px(2.))
            .border_color(if selected { theme.swatch_color() } else { gpui::transparent_black() })
            .cursor_pointer()
            .hover(|s| s.bg(colors.element_hover))
            .focus_visible(|s| s.border_color(colors.text))
            .when(show_tooltips, |el| el.tooltip(Tooltip::text(name)))
            .child(div().size_full().rounded_full().bg(theme.swatch()))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                pick(theme.clone(), window, cx)
            })
    }))
}

/// GPUI keeps a hoverable tooltip open while the pointer crosses into the palette.
struct NightPalette(WeakEntity<SettingsWindow>);

impl Render for NightPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(settings) = self.0.upgrade() else { return div() };
        let active = cx.theme().clone();
        let remembered_dark = remembered(&settings.read(cx).preferences.theme, true, cx);
        let night = if active.is_dark() { active } else { remembered_dark };
        ui::popover(cx).w(px(252.)).child(night_swatches(
            &night,
            false,
            move |theme, window, cx| {
                settings.update(cx, |settings, cx| settings.pick_theme(theme.clone(), window, cx));
            },
            cx,
        ))
    }
}

impl SettingsWindow {
    fn save_theme(
        &mut self,
        selection: ThemeSelection,
        temporary: Option<TemporaryTheme>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.store.save_theme_selection(&selection) {
            let strings = i18n::t(cx);
            let prompt = window.prompt(
                PromptLevel::Warning,
                strings.theme_save_failed,
                Some(&error.to_string()),
                &[strings.ok],
                cx,
            );
            cx.background_executor()
                .spawn(async move {
                    let _ = prompt.await;
                })
                .detach();
            return;
        }
        self.preferences.theme = selection;
        if let Some(choice) = temporary {
            set_temporary(choice, cx);
        } else {
            clear_temporary(cx);
        }
        apply_theme(&self.preferences.theme, window, cx);
        cx.notify();
    }

    fn pick_theme(&mut self, theme: Arc<Theme>, window: &mut Window, cx: &mut Context<Self>) {
        if cx.theme().id() == theme.id() {
            return;
        }
        let system_dark = system_dark(window);
        if self.preferences.theme.mode == ThemeMode::System
            && theme.is_dark() != system_dark
            && cx.theme().is_dark() == system_dark
        {
            let strings = i18n::t(cx);
            let answers = [strings.theme_once, strings.theme_always, strings.cancel];
            let response = window.prompt(
                PromptLevel::Info,
                strings.switch_theme,
                Some(strings.theme_override_hint),
                &answers,
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                let Ok(choice @ (0 | 1)) = response.await else { return };
                this.update_in(cx, |this, window, cx| {
                    this.use_theme(theme, choice == 1, window, cx);
                })
                .ok();
            })
            .detach();
        } else {
            self.use_theme(theme, false, window, cx);
        }
    }

    fn use_theme(
        &mut self,
        theme: Arc<Theme>,
        fixed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut selection = self.preferences.theme.clone();
        let dark = theme.is_dark();
        let system_dark = system_dark(window);
        if !fixed && selection.mode == ThemeMode::System && dark != system_dark {
            // "Just this time" leaves the saved opposite-appearance palette intact.
            set_temporary(
                TemporaryTheme {
                    system_dark,
                    dark,
                    theme_id: theme.id_shared(),
                },
                cx,
            );
            apply_theme(&self.preferences.theme, window, cx);
            cx.notify();
            return;
        }
        if dark {
            selection.dark = Some(theme.id().into());
        } else {
            selection.light = Some(theme.id().into());
        }
        if fixed || selection.mode != ThemeMode::System {
            selection.mode = if dark { ThemeMode::Dark } else { ThemeMode::Light };
        }
        self.save_theme(selection, None, window, cx);
    }

    fn toggle_system_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut selection = self.preferences.theme.clone();
        selection.mode = match selection.mode {
            ThemeMode::Light | ThemeMode::Dark => ThemeMode::System,
            ThemeMode::System => {
                let active = cx.theme();
                if active.is_dark() {
                    selection.dark = Some(active.id().into());
                    ThemeMode::Dark
                } else {
                    selection.light = Some(active.id().into());
                    ThemeMode::Light
                }
            }
        };
        self.save_theme(selection, None, window, cx);
    }

    fn pick_night_theme(&mut self, theme: Arc<Theme>, window: &mut Window, cx: &mut Context<Self>) {
        if self.preferences.theme.mode == ThemeMode::System
            && !system_dark(window)
            && cx.theme().is_light()
        {
            // Configure the next night in daylight without switching the workspace.
            let mut selection = self.preferences.theme.clone();
            selection.dark = Some(theme.id().into());
            self.save_theme(selection, None, window, cx);
        } else {
            self.use_theme(theme, false, window, cx);
        }
    }

    pub(super) fn render_appearance(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let registry = ThemeRegistry::global(cx);
        let bundled_light = registry.bundled_light().to_vec();
        let custom_themes = registry.custom_themes().to_vec();
        let theme_errors: Vec<SharedString> =
            registry.errors().iter().cloned().map(SharedString::from).collect();
        let active = cx.theme().clone();
        let is_bundled_dark = active.is_dark() && registry.is_bundled_dark_id(active.id());
        let night = if active.is_dark() {
            active.clone()
        } else {
            remembered(&self.preferences.theme, true, cx)
        };
        let dark_tile_theme = if is_bundled_dark {
            active.clone()
        } else if let Some(saved_dark) =
            self.preferences.theme.dark.as_deref().filter(|id| registry.is_bundled_dark_id(id))
        {
            registry.resolve(true, Some(saved_dark))
        } else {
            registry.resolve(true, None)
        };
        let follow = self.preferences.theme.mode == ThemeMode::System;
        let colors = active.colors();
        let owner = cx.weak_entity();
        let night_dots = dark_tile_theme.swatch();

        let tile = |id: SharedString,
                    theme: Arc<Theme>,
                    label: SharedString,
                    selected: bool,
                    dark_group_tile: bool,
                    tooltip: Option<SharedString>| {
            let swatch_bg = if dark_group_tile { gpui::black().into() } else { theme.swatch() };
            v_flex()
                .id(id)
                .tab_index(0)
                .role(gpui::Role::RadioButton)
                .aria_toggled(selected.into())
                .aria_label(label.clone())
                .w(px(88.))
                .flex_none()
                .items_center()
                .py_1()
                .gap_1p5()
                .rounded(px(8.))
                .border_1()
                .border_color(gpui::transparent_black())
                .cursor_pointer()
                .hover(|s| s.bg(colors.ghost_element_hover))
                .focus_visible(|s| s.border_color(colors.accent))
                .when(dark_group_tile, |el| {
                    let owner = owner.clone();
                    el.hoverable_tooltip(move |_, cx| cx.new(|_| NightPalette(owner.clone())).into())
                })
                .when_some(tooltip, |el, text| el.tooltip(Tooltip::text(text)))
                .child(
                    div()
                        .relative()
                        .size(px(52.))
                        .child(
                            div()
                                .size_full()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(colors.border)
                                .bg(swatch_bg)
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(dark_group_tile, |el| {
                                    el.child(h_flex().gap(px(5.)).children(
                                        (0..3).map(|_| div().size(px(7.)).rounded_full().bg(night_dots)),
                                    ))
                                }),
                        )
                        .when(selected, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .right(px(-6.))
                                    .bottom(px(-6.))
                                    .size(px(20.))
                                    .rounded_full()
                                    .bg(colors.background)
                                    .p(px(2.))
                                    .child(
                                        div()
                                            .size_full()
                                            .rounded_full()
                                            .bg(colors.accent)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                Icon::new(IconName::Check)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::IconOnAccent),
                                            ),
                                    ),
                            )
                        }),
                )
                .child(
                    div().w_full().overflow_hidden().child(
                        Label::new(label)
                            .size(LabelSize::Small)
                            .centered()
                            .truncate()
                            .color(if selected { Color::Default } else { Color::Muted }),
                    ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pick_theme(theme.clone(), window, cx)
                }))
        };

        let night_owner = owner.clone();
        let night_picker = move |cx: &App| {
            let night_owner = night_owner.clone();
            night_swatches(
                &night,
                true,
                move |theme, window, cx| {
                    night_owner
                        .update(cx, |settings, cx| settings.pick_night_theme(theme, window, cx))
                        .ok();
                },
                cx,
            )
        };

        let action_link = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .tab_index(0)
                .px_1p5()
                .py_0p5()
                .rounded(px(4.))
                .border_1()
                .border_color(gpui::transparent_black())
                .cursor_pointer()
                .hover(|s| s.bg(colors.ghost_element_hover))
                .focus_visible(|s| s.border_color(colors.accent))
                .child(Label::new(label).size(LabelSize::Small).color(Color::Accent))
        };

        let custom_section = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(Label::new(strings.custom_themes).weight(FontWeight::SEMIBOLD))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                action_link("create-theme-template", strings.create_theme_template)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        user_themes::create_and_open_sample_async(
                                            this.store.clone(),
                                            window.window_handle(),
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                action_link("open-themes-folder", strings.open_themes_folder)
                                    .on_click(|_, _, cx| user_themes::open_themes_dir_async(cx)),
                            )
                            .child(
                                action_link("reload-themes", strings.reload_themes)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        user_themes::reload_and_apply_async(
                                            this.store.clone(),
                                            window.window_handle(),
                                            cx,
                                        );
                                    })),
                            ),
                    ),
            )
            .child(if custom_themes.is_empty() {
                div()
                    .rounded(px(8.))
                    .bg(colors.surface_background)
                    .px_4()
                    .py_3()
                    .text_size(px(12.))
                    .text_color(colors.text_muted)
                    .child(strings.custom_themes_hint)
                    .into_any_element()
            } else {
                h_flex()
                    .flex_wrap()
                    .items_start()
                    .gap_x_1()
                    .gap_y_2()
                    .children(custom_themes.into_iter().map(|theme| {
                        let id = SharedString::from(format!("custom-theme-{}", theme.id()));
                        let selected = active.id() == theme.id();
                        let badge = if theme.is_dark() {
                            strings.theme_dark_badge
                        } else {
                            strings.theme_light_badge
                        };
                        let tooltip = match theme.author() {
                            Some(author) => format!("{} — {author} ({badge})", theme.name()),
                            None => format!("{} ({badge})", theme.name()),
                        };
                        let label = SharedString::from(theme.name().to_owned());
                        tile(id, theme, label, selected, false, Some(tooltip.into()))
                    }))
                    .into_any_element()
            })
            .when(!theme_errors.is_empty(), |el| {
                let error_color = active.status().error;
                el.child(
                    v_flex()
                        .rounded(px(8.))
                        .bg(colors.surface_background)
                        .border_1()
                        .border_color(error_color.opacity(0.4))
                        .px_4()
                        .py_2()
                        .gap_1()
                        .children(theme_errors.into_iter().map(|err| {
                            div().text_size(px(12.)).text_color(error_color).child(err)
                        })),
                )
            });

        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .rounded(px(8.))
                    .bg(colors.surface_background)
                    .child(
                        h_flex()
                            .id("follow-system")
                            .tab_index(0)
                            .aria_label(strings.follow_system_dark_mode)
                            .role(gpui::Role::Switch)
                            .aria_toggled(follow.into())
                            .min_h(px(ROW_HEIGHT))
                            .px_4()
                            .gap_2()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(gpui::transparent_black())
                            .cursor_pointer()
                            .focus_visible(|s| s.border_color(colors.accent))
                            .child(div().flex_1().child(Label::new(strings.follow_system_dark_mode)))
                            .child(Switch::new().toggle_state(follow))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_system_theme(window, cx)
                            })),
                    )
                    .when(follow, |el| {
                        el.child(div().px_4().child(Divider::horizontal())).child(
                            h_flex()
                                .min_h(px(ROW_HEIGHT))
                                .px_4()
                                .py_2()
                                .gap_3()
                                .child(div().flex_1().child(Label::new(strings.night_theme)))
                                .child(night_picker(cx)),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(Label::new(strings.color_series).weight(FontWeight::SEMIBOLD))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .items_start()
                            .gap_x_1()
                            .gap_y_2()
                            .children(bundled_light.into_iter().enumerate().map(
                                |(idx, theme)| {
                                    let selected = active.id() == theme.id();
                                    let label: SharedString = if theme.is_custom() {
                                        theme.name().to_owned().into()
                                    } else {
                                        strings
                                            .light_theme_names
                                            .get(idx)
                                            .copied()
                                            .unwrap_or_else(|| theme.name())
                                            .to_owned()
                                            .into()
                                    };
                                    tile(theme.id_shared(), theme, label, selected, false, None)
                                },
                            ))
                            .child(tile(
                                "theme-night".into(),
                                dark_tile_theme,
                                strings.dark_theme.into(),
                                is_bundled_dark,
                                true,
                                None,
                            )),
                    ),
            )
            .when(!follow && active.is_dark(), |el| {
                el.child(
                    v_flex()
                        .gap_2()
                        .child(Label::new(strings.night_theme).weight(FontWeight::SEMIBOLD))
                        .child(night_picker(cx)),
                )
            })
            .child(custom_section)
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use fern_core::{Store, ThemeMode, ThemeSelection};

    use super::*;

    #[gpui::test]
    fn temporary_override_preserves_saved_palettes_and_locks_on_system_toggle(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let initial = ThemeSelection {
            mode: ThemeMode::System,
            light: Some("matcha".into()),
            dark: Some("darkcyan".into()),
        };
        store.save_theme_selection(&initial).unwrap();
        let (settings, cx) = cx.add_window_view(|window, cx| {
            let settings = SettingsWindow::new(store.clone(), cx);
            apply_theme(&settings.preferences.theme, window, cx);
            settings
        });

        let dark_system = cx.update(|window, _| system_dark(window));
        assert!(!dark_system, "headless test window defaults to light appearance");
        cx.update(|_, cx| assert_eq!(cx.theme().id(), "matcha"));

        // Configuring the night theme in daylight saves it without switching the active light theme.
        let darkpink = cx.update(|_, cx| ThemeRegistry::global(cx).resolve(true, Some("darkpink")));
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.pick_night_theme(darkpink, window, cx));
        });
        assert_eq!(store.preferences().unwrap().theme.dark.as_deref(), Some("darkpink"));
        cx.update(|_, cx| assert_eq!(cx.theme().id(), "matcha"));

        // "Just this time" applies darkpurple temporarily without clobbering saved darkpink.
        let darkpurple =
            cx.update(|_, cx| ThemeRegistry::global(cx).resolve(true, Some("darkpurple")));
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.use_theme(darkpurple, false, window, cx));
        });
        assert_eq!(store.preferences().unwrap().theme.dark.as_deref(), Some("darkpink"));
        assert_eq!(store.preferences().unwrap().theme.mode, ThemeMode::System);
        cx.update(|_, cx| assert_eq!(cx.theme().id(), "darkpurple"));

        // Picking another dark preset while already temporarily dark switches immediately without prompting.
        let darkgreen =
            cx.update(|_, cx| ThemeRegistry::global(cx).resolve(true, Some("darkgreen")));
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.pick_theme(darkgreen, window, cx));
        });
        assert_eq!(store.preferences().unwrap().theme.dark.as_deref(), Some("darkpink"));
        cx.update(|_, cx| assert_eq!(cx.theme().id(), "darkgreen"));

        // Turning off "Follow System Dark Mode" keeps the active temporary theme and persists it.
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.toggle_system_theme(window, cx));
        });
        let saved = store.preferences().unwrap().theme;
        assert_eq!(saved.mode, ThemeMode::Dark);
        assert_eq!(saved.dark.as_deref(), Some("darkgreen"));
        cx.update(|_, cx| assert_eq!(cx.theme().id(), "darkgreen"));

        // Turning "Follow System Dark Mode" back on restores the saved light palette.
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.toggle_system_theme(window, cx));
        });
        assert_eq!(store.preferences().unwrap().theme.mode, ThemeMode::System);
        cx.update(|_, cx| assert_eq!(cx.theme().id(), "matcha"));
    }

    #[gpui::test]
    fn custom_theme_selection_live_reload_and_fallback(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let v1_json = r##"{
            "name": "Ocean",
            "themes": [
                {
                    "id": "ocean",
                    "name": "Ocean",
                    "appearance": "dark",
                    "style": { "background": "#0f172a", "accent": "#38bdf8" }
                }
            ]
        }"##;
        let (themes_v1, warnings) = theme::load_theme_family_from_str(v1_json).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        cx.update(|cx| theme::reload_user_themes(themes_v1, Vec::new(), cx));

        let store = Rc::new(Store::open_in_memory().unwrap());
        let (settings, cx) = cx.add_window_view(|window, cx| {
            let settings = SettingsWindow::new(store.clone(), cx);
            apply_theme(&settings.preferences.theme, window, cx);
            settings
        });

        let ocean = cx.update(|_, cx| ThemeRegistry::global(cx).resolve(true, Some("ocean")));
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.use_theme(ocean, true, window, cx));
        });
        assert_eq!(store.preferences().unwrap().theme.mode, ThemeMode::Dark);
        assert_eq!(store.preferences().unwrap().theme.dark.as_deref(), Some("ocean"));
        cx.update(|_, cx| {
            assert_eq!(cx.theme().id(), "ocean");
            assert_eq!(cx.theme().colors().accent, theme::try_parse_color("#38bdf8").unwrap());
        });

        // In-place live-reload of the active custom theme updates colors even when its ID stays unchanged.
        let v2_json = r##"{
            "name": "Ocean",
            "themes": [
                {
                    "id": "ocean",
                    "name": "Ocean",
                    "appearance": "dark",
                    "style": { "background": "#0f172a", "accent": "#f97316" }
                }
            ]
        }"##;
        let (themes_v2, warnings) = theme::load_theme_family_from_str(v2_json).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        cx.update(|window, cx| {
            theme::reload_user_themes(themes_v2, Vec::new(), cx);
            apply_theme(&store.preferences().unwrap().theme, window, cx);
        });
        cx.update(|_, cx| {
            assert_eq!(cx.theme().id(), "ocean");
            assert_eq!(cx.theme().colors().accent, theme::try_parse_color("#f97316").unwrap());
        });

        // Removing the custom theme file falls back gracefully to the default dark preset ("night").
        cx.update(|window, cx| {
            theme::reload_user_themes(Vec::new(), Vec::new(), cx);
            apply_theme(&store.preferences().unwrap().theme, window, cx);
        });
        cx.update(|_, cx| assert_eq!(cx.theme().id(), "night"));
    }
}
