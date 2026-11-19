use fern_core::{ThemeMode, ThemeSelection};
use gpui::{FontWeight, Global, PromptLevel, WeakEntity, WindowAppearance};
use theme::{DARK_PRESETS, LIGHT_PRESETS, Theme, ThemePreset};
use ui::prelude::*;

use super::{ROW_HEIGHT, SettingsWindow};
use crate::i18n;

/// An explicit "just this time" choice expires on restart or the next system appearance change.
#[derive(Clone, Copy)]
struct TemporaryTheme {
    system_dark: bool,
    preset: &'static ThemePreset,
}

impl Global for TemporaryTheme {}

fn system_dark(window: &Window) -> bool {
    matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

fn remembered(selection: &ThemeSelection, dark: bool) -> &'static ThemePreset {
    let id = if dark { &selection.dark } else { &selection.light };
    ThemePreset::resolve(dark, id.as_deref())
}

fn clear_temporary(cx: &mut App) {
    if cx.has_global::<TemporaryTheme>() {
        cx.remove_global::<TemporaryTheme>();
    }
}

fn night_swatches(
    night: &'static ThemePreset,
    show_tooltips: bool,
    pick: impl Fn(&'static ThemePreset, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> ui::HorizontalStack {
    let colors = cx.theme().colors();
    h_flex().flex_wrap().gap_2().children(DARK_PRESETS.iter().zip(i18n::t(cx).dark_theme_names).map(|(preset, name)| {
        let selected = preset.id == night.id;
        let pick = pick.clone();
        div().id(preset.id).tab_index(0).aria_label(name)
            .role(gpui::Role::RadioButton).aria_toggled(selected.into())
            .size(px(30.)).flex_none().rounded_full().border_2().p(px(2.))
            .border_color(if selected { preset.swatch_color() } else { gpui::transparent_black() })
            .cursor_pointer().hover(|s| s.bg(colors.element_hover))
            .focus_visible(|s| s.border_color(colors.text))
            .when(show_tooltips, |el| el.tooltip(Tooltip::text(name)))
            .child(div().size_full().rounded_full().bg(preset.swatch()))
            .on_click(move |_, window, cx| { cx.stop_propagation(); pick(preset, window, cx) })
    }))
}

/// GPUI keeps a hoverable tooltip open while the pointer crosses into the palette.
struct NightPalette(WeakEntity<SettingsWindow>);

impl Render for NightPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(settings) = self.0.upgrade() else { return div() };
        let active = cx.theme().preset;
        let night = if active.is_dark() { active } else { remembered(&settings.read(cx).preferences.theme, true) };
        ui::popover(cx).w(px(252.)).child(night_swatches(night, false, move |preset, window, cx| {
            settings.update(cx, |settings, cx| settings.pick_theme(preset, window, cx));
        }, cx))
    }
}

pub(super) fn apply_theme(selection: &ThemeSelection, window: &Window, cx: &mut App) {
    let dark = system_dark(window);
    let preset = match cx.try_global::<TemporaryTheme>() {
        Some(choice) if selection.mode == ThemeMode::System && choice.system_dark == dark => choice.preset,
        _ => {
            clear_temporary(cx);
            remembered(selection, selection.mode.is_dark(dark))
        }
    };
    if cx.theme().preset.id != preset.id {
        theme::set(Theme::from_preset(preset), cx);
        cx.refresh_windows();
    }
}

impl SettingsWindow {
    fn save_theme(&mut self, selection: ThemeSelection, temporary: Option<TemporaryTheme>, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.store.save_theme_selection(&selection) {
            let strings = i18n::t(cx);
            let _ = window.prompt(PromptLevel::Warning, strings.theme_save_failed, Some(&error.to_string()), &[strings.ok], cx);
            return;
        }
        self.preferences.theme = selection;
        if let Some(choice) = temporary { cx.set_global(choice) } else { clear_temporary(cx) }
        apply_theme(&self.preferences.theme, window, cx);
        cx.refresh_windows();
    }

    fn pick_theme(&mut self, preset: &'static ThemePreset, window: &mut Window, cx: &mut Context<Self>) {
        if cx.theme().preset.id == preset.id {
            return;
        }
        let system_dark = system_dark(window);
        if self.preferences.theme.mode == ThemeMode::System
            && preset.is_dark() != system_dark
            && cx.theme().preset.is_dark() == system_dark
        {
            let strings = i18n::t(cx);
            let answers = [strings.theme_once, strings.theme_always, strings.cancel];
            let response = window.prompt(PromptLevel::Info, strings.switch_theme, Some(strings.theme_override_hint), &answers, cx);
            cx.spawn_in(window, async move |this, cx| {
                let Ok(choice @ (0 | 1)) = response.await else { return };
                this.update_in(cx, |this, window, cx| {
                    this.use_theme(preset, choice == 1, window, cx);
                }).ok();
            }).detach();
        } else {
            self.use_theme(preset, false, window, cx);
        }
    }

    fn use_theme(&mut self, preset: &'static ThemePreset, fixed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut selection = self.preferences.theme.clone();
        let dark = preset.is_dark();
        let system_dark = system_dark(window);
        if !fixed && selection.mode == ThemeMode::System && dark != system_dark {
            // "Just this time" leaves the saved opposite-appearance palette intact.
            cx.set_global(TemporaryTheme { system_dark, preset });
            apply_theme(&self.preferences.theme, window, cx);
            cx.refresh_windows();
            return;
        }
        if dark { selection.dark = Some(preset.id.into()) } else { selection.light = Some(preset.id.into()) }
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
                let active = cx.theme().preset;
                if active.is_dark() {
                    selection.dark = Some(active.id.into());
                    ThemeMode::Dark
                } else {
                    selection.light = Some(active.id.into());
                    ThemeMode::Light
                }
            }
        };
        self.save_theme(selection, None, window, cx);
    }

    fn pick_night_theme(&mut self, preset: &'static ThemePreset, window: &mut Window, cx: &mut Context<Self>) {
        if self.preferences.theme.mode == ThemeMode::System && !system_dark(window) && cx.theme().is_light() {
            // Configure the next night in daylight without switching the workspace.
            let mut selection = self.preferences.theme.clone();
            selection.dark = Some(preset.id.into());
            self.save_theme(selection, None, window, cx);
        } else {
            self.use_theme(preset, false, window, cx);
        }
    }

    pub(super) fn render_appearance(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        let active = cx.theme().preset;
        let night = if active.is_dark() { active } else { remembered(&self.preferences.theme, true) };
        let follow = self.preferences.theme.mode == ThemeMode::System;
        let owner = cx.weak_entity();
        let tile = |preset: &'static ThemePreset, name: &'static str| {
            let dark = preset.is_dark();
            let selected = if dark { active.is_dark() } else { active.id == preset.id };
            v_flex()
                .id(if dark { "theme-night" } else { preset.id }).tab_index(0)
                .role(gpui::Role::RadioButton).aria_toggled(selected.into()).aria_label(name)
                .w(px(72.)).flex_none().items_center().py_1().gap_1p5()
                .rounded(px(8.)).border_1().border_color(gpui::transparent_black())
                .cursor_pointer().hover(|s| s.bg(colors.ghost_element_hover))
                .focus_visible(|s| s.border_color(colors.accent))
                .when(dark, |el| {
                    let owner = owner.clone();
                    el.hoverable_tooltip(move |_, cx| cx.new(|_| NightPalette(owner.clone())).into())
                })
                .child(
                    div().relative().size(px(52.)).rounded(px(8.)).border_1().border_color(colors.border)
                        .bg(if dark { gpui::black().into() } else { preset.swatch() })
                        .flex().items_center().justify_center()
                        .when(dark, |el| el.child(h_flex().gap(px(5.)).children(
                            (0..3).map(|_| div().size(px(7.)).rounded_full().bg(night.swatch())),
                        )))
                        .when(selected, |el| el.child(
                            div().absolute().right(px(-6.)).bottom(px(-6.)).size(px(20.)).rounded_full()
                                .border_2().border_color(colors.background)
                                .flex().items_center().justify_center().bg(colors.accent)
                                .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::IconOnAccent)),
                        )),
                )
                .child(Label::new(name).size(LabelSize::Small).color(if selected { Color::Default } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, window, cx| this.pick_theme(preset, window, cx)))
        };
        v_flex().gap_4()
            .child(
                h_flex().id("follow-system").tab_index(0).aria_label(strings.follow_system_dark_mode)
                    .role(gpui::Role::Switch).aria_toggled(follow.into())
                    .min_h(px(ROW_HEIGHT)).px_4().gap_2().rounded(px(8.))
                    .bg(colors.surface_background).border_1().border_color(gpui::transparent_black())
                    .cursor_pointer().focus_visible(|s| s.border_color(colors.accent))
                    .child(div().flex_1().child(Label::new(strings.follow_system_dark_mode)))
                    .child(Switch::new().toggle_state(follow))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_system_theme(window, cx))),
            )
            .child(v_flex().gap_2()
                .child(Label::new(strings.color_series).weight(FontWeight::SEMIBOLD))
                .child(h_flex().flex_wrap().gap_1()
                    .children(LIGHT_PRESETS.iter().zip(strings.light_theme_names).map(|(preset, name)| tile(preset, name)))
                    .child(tile(night, strings.dark_theme))))
            .when(follow || active.is_dark(), |el| el.child(v_flex().gap_2()
                .child(Label::new(strings.night_theme).weight(FontWeight::SEMIBOLD))
                .child(night_swatches(night, true, move |preset, window, cx| {
                    owner.update(cx, |settings, cx| settings.pick_night_theme(preset, window, cx)).ok();
                }, cx))))
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use fern_core::{Store, ThemeMode, ThemeSelection};

    use super::*;

    #[gpui::test]
    fn temporary_override_preserves_saved_palettes_and_locks_on_system_toggle(cx: &mut gpui::TestAppContext) {
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
        cx.update(|_, cx| assert_eq!(cx.theme().preset.id, "matcha"));

        // Configuring the night theme in daylight saves it without switching the active light theme.
        let darkpink = ThemePreset::resolve(true, Some("darkpink"));
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.pick_night_theme(darkpink, window, cx));
        });
        assert_eq!(store.preferences().unwrap().theme.dark.as_deref(), Some("darkpink"));
        cx.update(|_, cx| assert_eq!(cx.theme().preset.id, "matcha"));

        // "Just this time" applies darkpurple temporarily without clobbering saved darkpink.
        let darkpurple = ThemePreset::resolve(true, Some("darkpurple"));
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.use_theme(darkpurple, false, window, cx));
        });
        assert_eq!(store.preferences().unwrap().theme.dark.as_deref(), Some("darkpink"));
        assert_eq!(store.preferences().unwrap().theme.mode, ThemeMode::System);
        cx.update(|_, cx| assert_eq!(cx.theme().preset.id, "darkpurple"));

        // Picking another dark preset while already temporarily dark switches immediately without prompting.
        let darkgreen = ThemePreset::resolve(true, Some("darkgreen"));
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.pick_theme(darkgreen, window, cx));
        });
        assert_eq!(store.preferences().unwrap().theme.dark.as_deref(), Some("darkpink"));
        cx.update(|_, cx| assert_eq!(cx.theme().preset.id, "darkgreen"));

        // Turning off "Follow System Dark Mode" keeps the active temporary theme and persists it.
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.toggle_system_theme(window, cx));
        });
        let saved = store.preferences().unwrap().theme;
        assert_eq!(saved.mode, ThemeMode::Dark);
        assert_eq!(saved.dark.as_deref(), Some("darkgreen"));
        cx.update(|_, cx| assert_eq!(cx.theme().preset.id, "darkgreen"));

        // Turning "Follow System Dark Mode" back on restores the saved light palette.
        cx.update(|window, cx| {
            settings.update(cx, |settings, cx| settings.toggle_system_theme(window, cx));
        });
        assert_eq!(store.preferences().unwrap().theme.mode, ThemeMode::System);
        cx.update(|_, cx| assert_eq!(cx.theme().preset.id, "matcha"));
    }
}
