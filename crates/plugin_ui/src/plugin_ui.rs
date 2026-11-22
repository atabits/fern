//! GPUI host, declarative view renderer, modal/toast overlays, and plugin settings UI for Fern.

pub mod host;
pub mod modal;
pub mod settings_view;
pub mod view;

pub use host::{PluginHost, PluginHostConfig, collect_plugin_themes};
pub use modal::{PluginOverlayView, render_plugin_overlays};
pub use settings_view::{PluginSettingsTab, PluginSettingsView};
pub use view::{PluginView, PluginViewEvent, UiTreeState};

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use desktop_common::plugin_bus::{PluginBus, PluginNavAction};
    use fern_core::Store;
    use fern_core::plugin_store::UserScript;
    use fern_i18n::{CurrentLocale, Locale};
    use gpui::{
        AppContext as _, Focusable as _, Modifiers, ParentElement as _, Render, Styled as _,
        TestAppContext, div, point, px, size,
    };
    use plugins::ui_ast::{
        HandlerId, UiAlign, UiButtonSize, UiButtonVariant, UiJustify, UiNode, UiSelectOption,
        UiTextSize, UiTreePayload, UiWeight,
    };

    use super::*;

    fn setup_host(cx: &mut TestAppContext) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        let cache_dir = plugins_dir.join(".registry-cache");
        let vault_path = tmp.path().join("secrets.vault");
        let key_path = tmp.path().join("secrets.key");
        std::fs::create_dir_all(&plugins_dir).unwrap();

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(CurrentLocale(Locale::En));
            PluginHost::init(
                PluginHostConfig {
                    plugins_dir,
                    registry_cache_dir: cache_dir,
                    db_path: None,
                    secrets_vault_path: vault_path,
                    secrets_key_path: key_path,
                    safe_mode: false,
                    locale: "en".to_owned(),
                    theme_appearance: "light".to_owned(),
                    hot_reload: false,
                },
                cx,
            )
            .unwrap();
        });
        tmp
    }

    fn sample_ui_tree(input_value: &str, revision: u64) -> UiTreePayload {
        UiTreePayload {
            plugin_id: "com.test.ui".to_owned(),
            view_id: "main".to_owned(),
            root: UiNode::VStack {
                gap: Some(8.0),
                padding: Some(12.0),
                align: None,
                children: vec![
                    UiNode::Heading {
                        text: "Plugin Dashboard".to_owned(),
                        level: 2,
                    },
                    UiNode::HStack {
                        gap: Some(8.0),
                        padding: None,
                        align: None,
                        justify: None,
                        wrap: false,
                        children: vec![
                            UiNode::Badge {
                                label: "Beta".to_owned(),
                                color: Some("accent".to_owned()),
                            },
                            UiNode::Svg {
                                content: r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="currentColor"/></svg>"#.to_owned(),
                                width: 16.0,
                                height: 16.0,
                            },
                        ],
                    },
                    UiNode::Card {
                        title: Some("Stats".to_owned()),
                        subtitle: Some("Summary".to_owned()),
                        padding: None,
                        children: vec![
                            UiNode::Progress {
                                value: 0.5,
                                label: Some("Halfway".to_owned()),
                                color: None,
                            },
                            UiNode::Donut {
                                value: 0.75,
                                label: Some("75%".to_owned()),
                                size: Some(48.0),
                                color: None,
                            },
                            UiNode::Markdown {
                                content: "**Bold** and *italic*".to_owned(),
                            },
                            UiNode::KeyValue {
                                label: "Status".to_owned(),
                                value: "Ready".to_owned(),
                                muted: false,
                            },
                            UiNode::Text {
                                text: "Typed text".to_owned(),
                                muted: true,
                                weight: Some(UiWeight::Medium),
                                color: None,
                                size: Some(UiTextSize::Sm),
                            },
                        ],
                    },
                    UiNode::Input {
                        key: "draft".to_owned(),
                        value: input_value.to_owned(),
                        revision,
                        placeholder: Some("Type here".to_owned()),
                        disabled: false,
                        on_change: Some(HandlerId(1)),
                        on_submit: Some(HandlerId(2)),
                    },
                    UiNode::SegmentedTabs {
                        key: Some("mode".to_owned()),
                        value: "a".to_owned(),
                        options: vec![
                            UiSelectOption {
                                label: "A".to_owned(),
                                value: "a".to_owned(),
                            },
                            UiSelectOption {
                                label: "B".to_owned(),
                                value: "b".to_owned(),
                            },
                        ],
                        on_change: Some(HandlerId(3)),
                    },
                    UiNode::Button {
                        label: "Run".to_owned(),
                        variant: UiButtonVariant::Primary,
                        size: UiButtonSize::Sm,
                        icon: Some("play".to_owned()),
                        disabled: false,
                        loading: false,
                        on_click: Some(HandlerId(4)),
                    },
                ],
            },
        }
    }

    #[gpui::test]
    fn ui_node_rendering_preserves_text_input_focus_across_same_revision_rerender(
        cx: &mut TestAppContext,
    ) {
        let _tmp = setup_host(cx);
        cx.update(|cx| {
            PluginHost::set_view_tree("com.test.ui", "main", sample_ui_tree("seed", 1), cx);
        });

        let (view, cx) = cx.add_window_view(|_, cx| PluginView::new("com.test.ui", "main", cx));
        cx.simulate_resize(size(px(800.), px(640.)));
        cx.run_until_parked();

        // Focus the rendered TextInput and type into it.
        cx.update(|window, cx| {
            let input = view.read(cx).input_entity("draft").unwrap().clone();
            let fh = input.read(cx).focus_handle(cx);
            window.focus(&fh, cx);
        });
        cx.simulate_input(" edited");
        cx.run_until_parked();

        cx.update(|window, cx| {
            let input = view.read(cx).input_entity("draft").unwrap();
            assert_eq!(input.read(cx).text(), "seed edited");
            assert!(input.read(cx).focus_handle(cx).is_focused(window));
        });

        // Re-render with the SAME revision (1) and a stale value: must NOT clobber typed text or focus.
        cx.update(|_, cx| {
            PluginHost::set_view_tree("com.test.ui", "main", sample_ui_tree("stale", 1), cx);
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            let input = view.read(cx).input_entity("draft").unwrap();
            assert_eq!(input.read(cx).text(), "seed edited");
            assert!(input.read(cx).focus_handle(cx).is_focused(window));
        });

        // Re-render with incremented revision (2): updates text cleanly.
        cx.update(|_, cx| {
            PluginHost::set_view_tree("com.test.ui", "main", sample_ui_tree("reset", 2), cx);
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            let input = view.read(cx).input_entity("draft").unwrap();
            assert_eq!(input.read(cx).text(), "reset");
        });

        cx.update(|_, cx| PluginHost::shutdown(cx));
    }

    #[gpui::test]
    fn error_fallback_card_renders_and_opens_logs(cx: &mut TestAppContext) {
        let _tmp = setup_host(cx);
        cx.update(|cx| {
            PluginHost::set_view_error(
                "com.test.broken",
                "main",
                "ReferenceError: x is not defined".to_owned(),
                cx,
            );
        });

        let (view, cx) =
            cx.add_window_view(|_, cx| PluginView::new("com.test.broken", "main", cx));
        let open_logs_hits = Rc::new(Cell::new(0usize));
        let hits_c = Rc::clone(&open_logs_hits);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, ev, _| {
                if matches!(ev, PluginViewEvent::OpenLogs { .. }) {
                    hits_c.set(hits_c.get() + 1);
                }
            })
            .detach();
        });

        cx.simulate_resize(size(px(720.), px(520.)));
        cx.run_until_parked();

        assert!(cx.debug_bounds("plugin-view-error-fallback").is_some());
        let actions = cx.debug_bounds("plugin-view-error-actions").unwrap();
        // Click the right-hand "Open Logs" button inside the centered actions row.
        let logs_point = point(actions.right() - px(36.), actions.center().y);
        cx.simulate_click(logs_point, Modifiers::default());
        cx.run_until_parked();

        assert_eq!(open_logs_hits.get(), 1);
        cx.update(|_, cx| {
            let navs = PluginBus::update(cx, |bus| bus.drain_nav());
            assert_eq!(navs, vec![PluginNavAction::OpenSettingsPlugins]);
            assert_eq!(
                PluginHost::take_requested_settings_plugin(cx).as_deref(),
                Some("com.test.broken")
            );
        });

        cx.update(|_, cx| PluginHost::shutdown(cx));
    }

    struct SettingsWithOverlays {
        settings: gpui::Entity<PluginSettingsView>,
    }

    impl Render for SettingsWithOverlays {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            div()
                .relative()
                .size_full()
                .child(self.settings.clone())
                .child(render_plugin_overlays(window, cx))
        }
    }

    #[gpui::test]
    fn plugin_settings_view_smoke_construction_and_tab_switching(cx: &mut TestAppContext) {
        let _tmp = setup_host(cx);
        let store = Rc::new(Store::open_in_memory().unwrap());
        store
            .save_user_script(&UserScript {
                id: "s1".to_owned(),
                name: "Test Script".to_owned(),
                enabled: true,
                trigger_kind: "manual".to_owned(),
                trigger_arg: String::new(),
                permissions: r#"["tasks:read"]"#.to_owned(),
                code: "return fern.tasks.list().length;".to_owned(),
                updated_at: 100,
            })
            .unwrap();

        let (wrapper, cx) = cx.add_window_view(|_, cx| SettingsWithOverlays {
            settings: cx.new(|cx| PluginSettingsView::new(Rc::clone(&store), cx)),
        });
        cx.simulate_resize(size(px(900.), px(680.)));
        cx.run_until_parked();

        assert!(cx.debug_bounds("plugin-settings-view").is_some());
        let settings = cx.update(|_, cx| wrapper.read(cx).settings.clone());

        cx.update(|_, cx| {
            assert_eq!(settings.read(cx).active_tab(), PluginSettingsTab::Installed);
            settings.update(cx, |view, cx| view.set_tab(PluginSettingsTab::Scripts, cx));
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(settings.read(cx).active_tab(), PluginSettingsTab::Scripts);
            settings.update(cx, |view, cx| view.set_tab(PluginSettingsTab::Browse, cx));
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(settings.read(cx).active_tab(), PluginSettingsTab::Browse);
        });

        // Opening a plugin's own embedded screen inside Settings → Plugins switches cleanly and returns.
        cx.update(|_, cx| {
            PluginHost::set_view_tree(
                "com.test.page",
                "sync",
                sample_ui_tree("hello", 1),
                cx,
            );
            settings.update(cx, |view, cx| {
                view.open_plugin_page(
                    "com.test.page".to_owned(),
                    "Test Page Plugin".to_owned(),
                    "sync".to_owned(),
                    cx,
                );
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("plugin-settings-active-page").is_some());
        cx.update(|_, cx| {
            assert_eq!(
                settings.read(cx).active_plugin_page_id(),
                Some(("com.test.page", "sync"))
            );
            settings.update(cx, |view, cx| view.close_plugin_page(cx));
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("plugin-settings-active-page").is_none());

        cx.update(|_, cx| PluginHost::shutdown(cx));
    }

    #[gpui::test]
    fn hstack_with_vstack_and_badge_does_not_collapse_text_width(cx: &mut TestAppContext) {
        let _tmp = setup_host(cx);
        let payload = UiTreePayload {
            plugin_id: "com.test.header".to_owned(),
            view_id: "sync".to_owned(),
            root: UiNode::VStack {
                gap: Some(16.0),
                padding: Some(24.0),
                align: None,
                children: vec![
                    UiNode::HStack {
                        gap: None,
                        padding: None,
                        align: Some(UiAlign::Center),
                        justify: Some(UiJustify::Between),
                        wrap: false,
                        children: vec![
                            UiNode::VStack {
                                gap: Some(4.0),
                                padding: None,
                                align: None,
                                children: vec![
                                    UiNode::Heading {
                                        text: "TickTick".to_owned(),
                                        level: 1,
                                    },
                                    UiNode::Text {
                                        text: "Two-way sync of lists and tasks between Fern and TickTick."
                                            .to_owned(),
                                        muted: true,
                                        weight: None,
                                        color: None,
                                        size: None,
                                    },
                                ],
                            },
                            UiNode::Badge {
                                label: "Ready to sign in".to_owned(),
                                color: Some("#22c55e".to_owned()),
                            },
                        ],
                    },
                    UiNode::HStack {
                        gap: None,
                        padding: None,
                        align: Some(UiAlign::Center),
                        justify: Some(UiJustify::Between),
                        wrap: true,
                        children: vec![
                            UiNode::VStack {
                                gap: Some(2.0),
                                padding: None,
                                align: None,
                                children: vec![
                                    UiNode::Heading {
                                        text: "TickTick".to_owned(),
                                        level: 1,
                                    },
                                    UiNode::Text {
                                        text: "Last sync: 17:42".to_owned(),
                                        muted: true,
                                        weight: None,
                                        color: None,
                                        size: Some(UiTextSize::Sm),
                                    },
                                ],
                            },
                            UiNode::HStack {
                                gap: Some(8.0),
                                padding: None,
                                align: Some(UiAlign::Center),
                                justify: None,
                                wrap: false,
                                children: vec![
                                    UiNode::Badge {
                                        label: "Connected".to_owned(),
                                        color: Some("#22c55e".to_owned()),
                                    },
                                    UiNode::Button {
                                        label: "Sync Now".to_owned(),
                                        variant: UiButtonVariant::Primary,
                                        size: UiButtonSize::Md,
                                        icon: None,
                                        disabled: false,
                                        loading: false,
                                        on_click: Some(HandlerId(1)),
                                    },
                                ],
                            },
                        ],
                    },
                ],
            },
        };
        cx.update(|cx| {
            PluginHost::set_view_tree("com.test.header", "sync", payload, cx);
        });

        let (_view, cx) = cx.add_window_view(|_, cx| PluginView::new("com.test.header", "sync", cx));
        cx.simulate_resize(size(px(800.), px(640.)));
        cx.run_until_parked();

        // node_counter order: 0 = root VStack, 1 = onboarding HStack, 2 = onboarding title VStack,
        // 3 = connected HStack, 4 = connected title VStack, 5 = connected actions HStack.
        let onboarding_row = cx
            .debug_bounds("plugin-hstack-1")
            .expect("onboarding header hstack bounds");
        let onboarding_col = cx
            .debug_bounds("plugin-vstack-2")
            .expect("onboarding title vstack bounds");
        let connected_row = cx
            .debug_bounds("plugin-hstack-3")
            .expect("connected header hstack bounds");
        let connected_col = cx
            .debug_bounds("plugin-vstack-4")
            .expect("connected title vstack bounds");

        assert!(
            onboarding_col.size.width > px(400.),
            "onboarding title column collapsed: {:?}",
            onboarding_col
        );
        assert!(
            onboarding_row.size.height < px(110.),
            "onboarding header wrapped vertically: {:?}",
            onboarding_row
        );
        assert!(
            connected_col.size.width > px(350.),
            "connected title column collapsed: {:?}",
            connected_col
        );
        assert!(
            connected_row.size.height < px(110.),
            "connected header wrapped vertically: {:?}",
            connected_row
        );

        cx.update(|_, cx| PluginHost::shutdown(cx));
    }
}
