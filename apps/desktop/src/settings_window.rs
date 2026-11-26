//! Thin desktop bridge for [`settings_ui`], injecting host capabilities (`plugin_ui`, system
//! tray, global hotkeys, and updater) without creating sibling crate dependencies.

use std::rc::Rc;

use fern_core::{Preferences, Store};
use gpui::{App, AppContext as _, Entity, Window, WindowHandle};

use crate::hotkeys::{self, Binding, Hotkey};
use crate::release_notes::ReleaseNotes;
use crate::tray::{self, TrayPrefs};
use crate::updater::{self, Status as UpdaterStatus};

pub use settings_ui::{
    Page, SettingsEvent, SettingsHost, SettingsWindow, TrayState, UpdateSnapshot, UpdateUiStatus,
};

fn build_host_with_plugins(
    store: Rc<Store>,
    sync_now: Rc<dyn Fn(&mut App)>,
    cx: &mut App,
) -> (SettingsHost, Entity<plugin_ui::PluginSettingsView>) {
    let plugin_settings = cx.new(|cx| plugin_ui::PluginSettingsView::new(store.clone(), cx));
    let ps_close = plugin_settings.clone();
    let ps_escape = plugin_settings.clone();
    let ps_logs = plugin_settings.clone();
    let host = SettingsHost {
        plugin_settings_view: plugin_settings.clone().into(),
        close_plugin_page: Rc::new(move |cx| {
            ps_close.update(cx, |view, cx| view.close_plugin_page(cx));
        }),
        handle_plugin_escape: Rc::new(move |cx| {
            ps_escape.update(cx, |view, cx| view.handle_escape(cx))
        }),
        dismiss_plugin_modal: Rc::new(crate::app_view::dismiss_plugin_modal),
        render_plugin_overlays: Rc::new(plugin_ui::render_plugin_overlays),
        open_plugin_logs: Rc::new(move |plugin_id, cx| {
            let pid = plugin_id.to_owned();
            ps_logs.update(cx, |view, cx| view.open_logs_for(&pid, cx));
        }),
        on_appearance_changed: Rc::new(|id, appearance, cx| {
            plugin_ui::PluginHost::set_appearance(id, appearance, cx);
        }),
        tray_state: Rc::new(|store, cx| {
            let prefs = TrayPrefs::current(store, cx);
            TrayState {
                minimize_to_tray: prefs.minimize_to_tray,
                always_on_top: prefs.always_on_top,
            }
        }),
        set_minimize_to_tray: Rc::new(tray::set_enabled),
        set_always_on_top: Rc::new(tray::set_always_on_top),
        observe_tray: Rc::new(|cx| {
            Some(cx.observe_global::<TrayPrefs>(|this, cx| {
                let prefs = *cx.global::<TrayPrefs>();
                this.update_tray_state(
                    TrayState {
                        minimize_to_tray: prefs.minimize_to_tray,
                        always_on_top: prefs.always_on_top,
                    },
                    cx,
                );
            }))
        }),
        global_shortcuts: Rc::new(|strings, cx| {
            hotkeys::bindings(cx)
                .into_iter()
                .map(|(hotkey, binding, taken)| settings_ui::GlobalShortcutEntry {
                    id: hotkey.setting_key(),
                    label: hotkey.label(strings),
                    binding: binding.as_ref().map(Binding::label),
                    taken,
                })
                .collect()
        }),
        set_global_hotkey: Rc::new(|index, keystroke, cx| {
            let Some(&hotkey) = Hotkey::ALL.get(index) else {
                return false;
            };
            let Some(ks) = keystroke else {
                hotkeys::set(hotkey, None, cx);
                return true;
            };
            let Some(binding) = Binding::from_keystroke(ks) else {
                return false;
            };
            hotkeys::set(hotkey, Some(binding), cx);
            true
        }),
        reset_global_hotkeys: Rc::new(hotkeys::reset_defaults),
        updater_snapshot: Rc::new(|cx| {
            let Some(updater) = updater::updater(cx) else {
                return UpdateSnapshot::default();
            };
            let u = updater.read(cx);
            let status = match u.status() {
                UpdaterStatus::Idle => UpdateUiStatus::Idle,
                UpdaterStatus::Checking => UpdateUiStatus::Checking,
                UpdaterStatus::UpToDate => UpdateUiStatus::UpToDate,
                UpdaterStatus::Downloading(v) => UpdateUiStatus::Downloading(v.clone()),
                UpdaterStatus::Ready(notes) => UpdateUiStatus::Ready(notes.clone()),
                UpdaterStatus::Available(notes) => UpdateUiStatus::Available(notes.clone()),
                UpdaterStatus::Failed(err) => UpdateUiStatus::Failed(err.clone()),
            };
            UpdateSnapshot {
                supported: u.supported(),
                automatic: u.automatic(),
                status,
            }
        }),
        set_auto_update: Rc::new(|on, cx| {
            if let Some(updater) = updater::updater(cx) {
                updater.update(cx, |u, cx| u.set_automatic(on, cx));
            }
        }),
        check_for_updates: Rc::new(|cx| {
            if let Some(updater) = updater::updater(cx) {
                updater.update(cx, |u, cx| u.check(cx));
            }
        }),
        restart_to_update: Rc::new(|cx| {
            if let Some(updater) = updater::updater(cx) {
                updater.update(cx, |u, cx| u.restart_to_update(cx));
            }
        }),
        observe_updater: Rc::new(|cx| {
            updater::updater(cx).map(|updater| cx.observe(&updater, |_, _, cx| cx.notify()))
        }),
        sync_now,
    };
    (host, plugin_settings)
}

/// Opens Settings in its own desktop window centered on `parent`'s display.
pub fn open(
    store: Rc<Store>,
    notes: Option<Rc<ReleaseNotes>>,
    initial_page: Option<Page>,
    sync_now: Rc<dyn Fn(&mut App)>,
    parent: &Window,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<SettingsWindow>> {
    let (host, _) = build_host_with_plugins(store.clone(), sync_now, cx);
    settings_ui::open(store, host, notes, initial_page, parent, cx)
}

/// Installs the theme and active preferences from `preferences` and updates plugin appearance.
pub fn apply(preferences: &Preferences, window: &Window, cx: &mut App) {
    settings_ui::apply(
        preferences,
        &|id, appearance, cx| plugin_ui::PluginHost::set_appearance(id, appearance, cx),
        window,
        cx,
    );
}

#[cfg(test)]
mod tests {
    use gpui::{Focusable as _, px};

    use super::*;

    #[gpui::test]
    fn settings_window_mounts_plugin_overlays_and_escape_dismisses_modal_and_page_first(
        app_cx: &mut gpui::TestAppContext,
    ) {
        use desktop_common::plugin_bus::{PluginBus, PluginModalKind, PluginModalSpec};

        let store = Rc::new(Store::open_in_memory().unwrap());
        app_cx.update(|cx| {
            theme::init(cx);
            assert!(
                crate::keymap::load_default(&assets::Assets, cx)
                    .unwrap()
                    .is_empty()
            );
            cx.set_global(crate::i18n::CurrentLocale(crate::i18n::Locale::En));
        });
        let plugin_settings_holder = Rc::new(std::cell::RefCell::new(None));
        let holder = Rc::clone(&plugin_settings_holder);
        let (settings, cx) = app_cx.add_window_view(|window, cx| {
            let (host, plugin_settings) = build_host_with_plugins(store.clone(), Rc::new(|_| {}), cx);
            *holder.borrow_mut() = Some(plugin_settings);
            let mut settings = SettingsWindow::new(store, host, cx);
            settings.set_page(Page::Plugins, cx);
            window.focus(&settings.focus_handle(cx), cx);
            settings
        });
        let plugin_settings = plugin_settings_holder.borrow_mut().take().unwrap();
        cx.simulate_resize(gpui::size(px(900.), px(720.)));

        // Open an embedded plugin page AND a plugin confirm modal.
        cx.update(|_, cx| {
            plugin_settings.update(cx, |ps, cx| {
                ps.open_plugin_page("demo".into(), "Demo".into(), "main".into(), cx);
            });
            PluginBus::update(cx, |bus| {
                bus.open_modal(PluginModalSpec {
                    plugin_id: "demo".into(),
                    view_id: "main".into(),
                    title: "Confirm sync".into(),
                    kind: PluginModalKind::Confirm {
                        body: "Proceed?".into(),
                        request_id: 42,
                    },
                });
            });
        });
        cx.run_until_parked();

        // First Escape closes the plugin modal without closing the embedded page or Settings.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(cx.global::<PluginBus>().active_modal.is_none());
            assert_eq!(
                plugin_settings.read(cx).active_plugin_page_id(),
                Some(("demo", "main"))
            );
            assert!(!settings.read(cx).is_closing());
        });

        // Second Escape closes the embedded plugin page without closing Settings.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(plugin_settings.read(cx).active_plugin_page_id(), None);
            assert!(!settings.read(cx).is_closing());
        });

        // Third Escape closes and removes the Settings window.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        app_cx.update(|cx| {
            assert!(cx.windows().is_empty(), "third Escape must close SettingsWindow");
        });
    }
}
