//! GPUI host, declarative view renderer, modal/toast overlays, and plugin settings UI for Fern.

pub mod host;
pub mod modal;
pub mod settings_view;
pub mod view;

pub use host::{PluginHost, PluginHostConfig, collect_plugin_themes};
pub use modal::{
    PluginOverlayView, dismiss_active_plugin_modal, handle_plugin_modal_escape,
    render_plugin_overlays,
};
pub use plugins::{HookDecision, Permission, PluginInfo, PluginManager, PluginStatus};
pub mod api {
    pub use plugins::api::{parse_task_draft, task_to_json};
}
pub use settings_view::{PluginSettingsTab, PluginSettingsView};
pub use view::{PluginView, PluginViewEvent, UiTreeState, rasterize_svg, safe_usvg_options};

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use desktop_common::plugin_bus::{PluginBus, PluginNavAction};
    use fern_core::Store;
    use fern_core::plugin_store::UserScript;
    use fern_i18n::{CurrentLocale, Locale};
    use gpui::{
        AppContext as _, Focusable as _, IntoElement as _, Modifiers, ParentElement as _, Render,
        Styled as _, TestAppContext, VisualTestContext, div, point, px, size,
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

    #[gpui::test]
    fn nested_hstack_and_scroll_layout_do_not_collapse(cx: &mut TestAppContext) {
        let _tmp = setup_host(cx);
        let payload = UiTreePayload {
            plugin_id: "com.test.layout".to_owned(),
            view_id: "main".to_owned(),
            root: UiNode::VStack {
                gap: Some(12.0),
                padding: Some(16.0),
                align: None,
                children: vec![
                    // Nested auto-sized HStack containing Text, VStack, Card, Input, Button
                    UiNode::HStack {
                        gap: Some(8.0),
                        padding: None,
                        align: Some(UiAlign::Center),
                        justify: Some(UiJustify::Between),
                        wrap: false,
                        children: vec![
                            UiNode::VStack {
                                gap: Some(2.0),
                                padding: None,
                                align: None,
                                children: vec![UiNode::Heading {
                                    text: "Header".to_owned(),
                                    level: 2,
                                }],
                            },
                            UiNode::HStack {
                                gap: Some(8.0),
                                padding: None,
                                align: Some(UiAlign::Center),
                                justify: None,
                                wrap: false,
                                children: vec![
                                    UiNode::Text {
                                        text: "3 synced".to_owned(),
                                        muted: true,
                                        weight: None,
                                        color: None,
                                        size: None,
                                    },
                                    UiNode::VStack {
                                        gap: Some(2.0),
                                        padding: None,
                                        align: None,
                                        children: vec![UiNode::Text {
                                            text: "Nested Col".to_owned(),
                                            muted: false,
                                            weight: None,
                                            color: None,
                                            size: None,
                                        }],
                                    },
                                    UiNode::Card {
                                        title: None,
                                        subtitle: None,
                                        padding: Some(6.0),
                                        children: vec![UiNode::Text {
                                            text: "Mini".to_owned(),
                                            muted: false,
                                            weight: None,
                                            color: None,
                                            size: None,
                                        }],
                                    },
                                    UiNode::Input {
                                        key: "nested_in".to_owned(),
                                        value: "abc".to_owned(),
                                        revision: 1,
                                        placeholder: None,
                                        disabled: false,
                                        on_change: None,
                                        on_submit: None,
                                    },
                                ],
                            },
                        ],
                    },
                    // HStack justify Start: Text must NOT stretch to crush or push Badge to right edge
                    UiNode::HStack {
                        gap: Some(8.0),
                        padding: None,
                        align: Some(UiAlign::Center),
                        justify: Some(UiJustify::Start),
                        wrap: false,
                        children: vec![
                            UiNode::Text {
                                text: "Label:".to_owned(),
                                muted: false,
                                weight: None,
                                color: None,
                                size: None,
                            },
                            UiNode::Badge {
                                label: "OK".to_owned(),
                                color: None,
                            },
                        ],
                    },
                    // Scroll without max_height must auto-grow instead of collapsing to 0px
                    UiNode::Scroll {
                        key: "unbounded".to_owned(),
                        max_height: None,
                        child: None,
                        children: vec![
                            UiNode::Text {
                                text: "Scroll row 1".to_owned(),
                                muted: false,
                                weight: None,
                                color: None,
                                size: None,
                            },
                            UiNode::Text {
                                text: "Scroll row 2".to_owned(),
                                muted: false,
                                weight: None,
                                color: None,
                                size: None,
                            },
                            UiNode::Text {
                                text: "Scroll row 3".to_owned(),
                                muted: false,
                                weight: None,
                                color: None,
                                size: None,
                            },
                        ],
                    },
                    // Scroll with explicit max_height remains bounded
                    UiNode::Scroll {
                        key: "bounded".to_owned(),
                        max_height: Some(80.0),
                        child: None,
                        children: (0..8)
                            .map(|i| UiNode::Text {
                                text: format!("Bounded row {i}"),
                                muted: false,
                                weight: None,
                                color: None,
                                size: None,
                            })
                            .collect(),
                    },
                ],
            },
        };

        cx.update(|cx| {
            PluginHost::set_view_tree("com.test.layout", "main", payload, cx);
        });
        let (_view, cx) = cx.add_window_view(|_, cx| PluginView::new("com.test.layout", "main", cx));
        cx.simulate_resize(size(px(860.), px(680.)));
        cx.run_until_parked();

        // Text("3 synced") is test_text_counter 0, Text("Nested Col") is 1, Text("Mini") is 2,
        // Text("Label:") is 3.
        let synced_text = cx.debug_bounds("plugin-text-0").expect("3 synced text bounds");
        let nested_col = cx.debug_bounds("plugin-vstack-4").expect("nested vstack bounds");
        let nested_card = cx.debug_bounds("plugin-card-0").expect("nested card bounds");
        let nested_input = cx
            .debug_bounds("plugin-input-nested_in")
            .expect("nested input bounds");
        let start_label = cx.debug_bounds("plugin-text-3").expect("Label: text bounds");
        let unbounded_scroll = cx
            .debug_bounds("plugin-scroll-unbounded")
            .expect("unbounded scroll bounds");
        let bounded_scroll = cx
            .debug_bounds("plugin-scroll-bounded")
            .expect("bounded scroll bounds");

        assert!(
            synced_text.size.width > px(20.),
            "nested HStack Text collapsed: {:?}",
            synced_text
        );
        assert!(
            nested_col.size.width > px(20.),
            "nested HStack VStack collapsed: {:?}",
            nested_col
        );
        assert!(
            nested_card.size.width > px(20.),
            "nested HStack Card collapsed: {:?}",
            nested_card
        );
        assert!(
            nested_input.size.width >= px(100.),
            "nested HStack Input collapsed: {:?}",
            nested_input
        );
        assert!(
            start_label.size.width > px(15.) && start_label.size.width < px(200.),
            "justify:Start Text stretched unexpectedly: {:?}",
            start_label
        );
        assert!(
            unbounded_scroll.size.height > px(40.),
            "UiNode::Scroll without max_height collapsed to 0px: {:?}",
            unbounded_scroll
        );
        assert!(
            bounded_scroll.size.height > px(20.) && bounded_scroll.size.height <= px(85.),
            "UiNode::Scroll with max_height=80 did not bound properly: {:?}",
            bounded_scroll
        );

        cx.update(|_, cx| PluginHost::shutdown(cx));
    }

    #[test]
    fn safe_usvg_options_blocks_image_href_and_data_resolution() {
        let opts = crate::view::safe_usvg_options();
        assert!(
            (opts.image_href_resolver.resolve_string)("/etc/passwd", &opts).is_none(),
            "resolve_string must be a no-op returning None"
        );
        assert!(
            (opts.image_href_resolver.resolve_data)("image/svg+xml", Arc::new(Vec::new()), &opts)
                .is_none(),
            "resolve_data must be a no-op returning None"
        );

        // Even with a prefixed SVG image tag referencing a real local file, usvg must not load it.
        let tmp = tempfile::tempdir().unwrap();
        let dummy = tmp.path().join("secret.png");
        std::fs::write(&dummy, b"not-a-real-image").unwrap();
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:s="http://www.w3.org/2000/svg" xmlns:x="http://www.w3.org/1999/xlink" viewBox="0 0 16 16"><s:image x:href="{}" width="16" height="16"/></svg>"#,
            dummy.display()
        );
        let tree = resvg::usvg::Tree::from_str(&svg, &opts).unwrap();
        assert!(
            tree.root().children().is_empty(),
            "external image node should not resolve with no-op ImageHrefResolver"
        );
    }

    fn write_test_plugin(dir: &std::path::Path, manifest_json: &str, entry_js: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest_json).unwrap();
        std::fs::write(dir.join("index.js"), entry_js).unwrap();
    }

    #[gpui::test]
    fn plugin_view_reopen_remounts_and_handles_first_click(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        let cache_dir = plugins_dir.join(".registry-cache");
        let db_path = tmp.path().join("tasks.db");
        let vault_path = tmp.path().join("secrets.vault");
        let key_path = tmp.path().join("secrets.key");
        let _seed_store = Store::open(&db_path).unwrap();

        let plugin_dir = plugins_dir.join("com.test.reopen");
        write_test_plugin(
            &plugin_dir,
            r#"{
                "id": "com.test.reopen",
                "name": "Reopen Test",
                "version": "1.0.0",
                "api_version": 1,
                "entry": "index.js",
                "permissions": [],
                "activation_events": ["*"],
                "contributions": {
                    "pages": [{ "id": "main", "title": "Main" }]
                }
            }"#,
            r#"
            import fern from "fern";
            import ui from "fern:ui";
            let mounts = 0;
            let clicks = 0;
            export function activate() {
                fern.views.register("main", {
                    mount() { mounts += 1; },
                    render() {
                        return ui.vstack({
                            children: [
                                ui.text(`mounts:${mounts},clicks:${clicks}`),
                                ui.button({
                                    label: "Hit",
                                    onClick: () => { clicks += 1; }
                                })
                            ]
                        });
                    }
                });
            }
            "#,
        );

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(CurrentLocale(Locale::En));
            PluginHost::init(
                PluginHostConfig {
                    plugins_dir: plugins_dir.clone(),
                    registry_cache_dir: cache_dir,
                    db_path: Some(db_path.clone()),
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
            let mgr = PluginHost::global(cx).unwrap().manager().unwrap().clone();
            mgr.approve_permissions("com.test.reopen").unwrap();
            PluginHost::drain_updates_for_test(cx);
        });
        cx.run_until_parked();

        let store = Rc::new(Store::open(&db_path).unwrap());
        let (settings, cx) = cx.add_window_view(|_, cx| PluginSettingsView::new(Rc::clone(&store), cx));
        cx.simulate_resize(size(px(800.), px(640.)));
        cx.run_until_parked();

        // First open -> mount() runs once.
        cx.update(|_, cx| {
            settings.update(cx, |v, cx| {
                v.open_plugin_page(
                    "com.test.reopen".to_owned(),
                    "Reopen Test".to_owned(),
                    "main".to_owned(),
                    cx,
                );
            });
        });
        cx.run_until_parked();

        let tree1 = cx
            .update(|_, cx| {
                PluginHost::global(cx)
                    .unwrap()
                    .view_tree("com.test.reopen", "main")
                    .cloned()
            })
            .expect("first open tree");
        let text1 = serde_json::to_string(&tree1.root).unwrap();
        assert!(text1.contains("mounts:1,clicks:0"), "unexpected first tree: {text1}");

        // Back (close) -> cached tree is evicted immediately.
        cx.update(|_, cx| {
            settings.update(cx, |v, cx| v.close_plugin_page(cx));
        });
        cx.update(|_, cx| {
            assert!(
                PluginHost::global(cx)
                    .unwrap()
                    .view_tree("com.test.reopen", "main")
                    .is_none(),
                "unmount must evict cached view tree"
            );
        });

        // Reopen immediately -> unmount & mount run in FIFO order, mount() runs a 2nd time,
        // and the very first button click works!
        cx.update(|_, cx| {
            settings.update(cx, |v, cx| {
                v.open_plugin_page(
                    "com.test.reopen".to_owned(),
                    "Reopen Test".to_owned(),
                    "main".to_owned(),
                    cx,
                );
            });
        });
        cx.run_until_parked();

        let tree2 = cx
            .update(|_, cx| {
                PluginHost::global(cx)
                    .unwrap()
                    .view_tree("com.test.reopen", "main")
                    .cloned()
            })
            .expect("reopened tree");
        let text2 = serde_json::to_string(&tree2.root).unwrap();
        assert!(
            text2.contains("mounts:2,clicks:0"),
            "reopen must re-run mount(): {text2}"
        );

        // Extract the button's HandlerId from the reopened tree and dispatch a click immediately.
        let click_hid = match &tree2.root {
            UiNode::VStack { children, .. } => match &children[1] {
                UiNode::Button { on_click, .. } => on_click.expect("button handler id"),
                other => panic!("expected button, got {other:?}"),
            },
            other => panic!("expected vstack, got {other:?}"),
        };
        cx.update(|_, cx| {
            PluginHost::dispatch_ui_event_async(
                "com.test.reopen".to_owned(),
                plugins::ui_ast::UiEvent::click("main", click_hid),
                cx,
            );
        });
        cx.run_until_parked();

        let tree3 = cx
            .update(|_, cx| {
                PluginHost::global(cx)
                    .unwrap()
                    .view_tree("com.test.reopen", "main")
                    .cloned()
            })
            .expect("post-click tree");
        let text3 = serde_json::to_string(&tree3.root).unwrap();
        assert!(
            text3.contains("mounts:2,clicks:1"),
            "first click after reopen was dropped: {text3}"
        );

        cx.update(|_, cx| PluginHost::shutdown(cx));
    }

    #[gpui::test]
    fn displaced_confirm_resolves_and_escape_dismisses_modal_and_surfaces_view_error(
        cx: &mut TestAppContext,
    ) {
        let _tmp = setup_host(cx);

        struct OverlayOnlyView;
        impl Render for OverlayOnlyView {
            fn render(
                &mut self,
                window: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                div()
                    .relative()
                    .size_full()
                    .child(render_plugin_overlays(window, cx))
            }
        }

        let (_view, cx) = cx.add_window_view(|_, _| OverlayOnlyView);
        cx.simulate_resize(size(px(640.), px(480.)));
        cx.run_until_parked();

        // Open Confirm #1, then displace it with Confirm #2 via apply_ui_effect.
        cx.update(|_, cx| {
            crate::host::apply_ui_effect(
                plugins::PluginUiEffect::Confirm {
                    request_id: 101,
                    plugin_id: "com.test.a".to_owned(),
                    title: "First".to_owned(),
                    body: "Body 1".to_owned(),
                },
                cx,
            );
            crate::host::apply_ui_effect(
                plugins::PluginUiEffect::Confirm {
                    request_id: 102,
                    plugin_id: "com.test.b".to_owned(),
                    title: "Second".to_owned(),
                    body: "Body 2".to_owned(),
                },
                cx,
            );
        });
        cx.run_until_parked();

        // Escape dismisses Confirm #2 cleanly.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(
                PluginBus::global(cx).unwrap().active_modal.is_none(),
                "Escape must dismiss active Confirm modal"
            );
        });

        // Open View modal + set a "__modal__" dispatch error -> modal displays error banner,
        // and Escape closes it and clears the error.
        cx.update(|_, cx| {
            crate::host::apply_ui_effect(
                plugins::PluginUiEffect::OpenModal {
                    plugin_id: "com.test.modal".to_owned(),
                    view_id: "__modal__".to_owned(),
                    title: "Modal View".to_owned(),
                    tree: UiNode::Text {
                        text: "Inside modal".to_owned(),
                        muted: false,
                        weight: None,
                        color: None,
                        size: None,
                    },
                },
                cx,
            );
            PluginHost::set_view_error(
                "com.test.modal",
                "__modal__",
                "Handler exploded".to_owned(),
                cx,
            );
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("plugin-view-modal-error").is_some(),
            "View modal must surface __modal__ handler errors"
        );

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(PluginBus::global(cx).unwrap().active_modal.is_none());
            assert!(
                PluginHost::global(cx)
                    .unwrap()
                    .view_error("com.test.modal", "__modal__")
                    .is_none(),
                "closing View modal must clear __modal__ error"
            );
        });

        cx.update(|_, cx| PluginHost::shutdown(cx));
    }

    #[gpui::test]
    fn overlays_render_only_in_active_window_without_duplication(cx: &mut TestAppContext) {
        let _tmp = setup_host(cx);

        struct OverlayWindowView;
        impl Render for OverlayWindowView {
            fn render(
                &mut self,
                window: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                div()
                    .relative()
                    .size_full()
                    .child(render_plugin_overlays(window, cx))
            }
        }

        let w1_handle = cx.add_window(|_, _| OverlayWindowView);
        let w2_handle = cx.add_window(|_, _| OverlayWindowView);
        let mut w1 = VisualTestContext::from_window(w1_handle.into(), cx);
        let mut w2 = VisualTestContext::from_window(w2_handle.into(), cx);
        w1.simulate_resize(size(px(640.), px(480.)));
        w2.simulate_resize(size(px(640.), px(480.)));
        w1.run_until_parked();
        w2.run_until_parked();

        w2.update(|window, cx| {
            window.activate_window();
            crate::host::apply_ui_effect(
                plugins::PluginUiEffect::Confirm {
                    request_id: 201,
                    plugin_id: "com.test.multi".to_owned(),
                    title: "Multi-Window Confirm".to_owned(),
                    body: "Only in active window".to_owned(),
                },
                cx,
            );
        });
        w1.run_until_parked();
        w2.run_until_parked();

        let in_w1 = w1.debug_bounds("plugin-overlay-modal").is_some();
        let in_w2 = w2.debug_bounds("plugin-overlay-modal").is_some();
        assert!(
            in_w2 && !in_w1,
            "overlay must render only in active window (w1={in_w1}, w2={in_w2})"
        );

        w2.update(|_, cx| PluginHost::shutdown(cx));
    }

    #[gpui::test]
    fn safe_mode_toggle_unregisters_plugin_themes_and_recovers_on_spawn_error(
        cx: &mut TestAppContext,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        let cache_dir = plugins_dir.join(".registry-cache");
        let db_path = tmp.path().join("tasks.db");
        let vault_path = tmp.path().join("secrets.vault");
        let key_path = tmp.path().join("secrets.key");
        let _seed_store = Store::open(&db_path).unwrap();

        write_test_plugin(
            &plugins_dir.join("com.test.theme"),
            r##"{
                "id": "com.test.theme",
                "name": "Theme Plugin",
                "version": "1.0.0",
                "api_version": 1,
                "entry": "index.js",
                "permissions": [],
                "contributions": {
                    "themes": [{
                        "id": "plugin-neon-test",
                        "name": "Plugin Neon Test",
                        "appearance": "dark",
                        "colors": { "accent": "#ff00aa" }
                    }]
                }
            }"##,
            "export function activate() {}",
        );

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(CurrentLocale(Locale::En));
            PluginHost::init(
                PluginHostConfig {
                    plugins_dir: plugins_dir.clone(),
                    registry_cache_dir: cache_dir.clone(),
                    db_path: Some(db_path.clone()),
                    secrets_vault_path: vault_path.clone(),
                    secrets_key_path: key_path.clone(),
                    safe_mode: false,
                    locale: "en".to_owned(),
                    theme_appearance: "light".to_owned(),
                    hot_reload: false,
                },
                cx,
            )
            .unwrap();
            let mgr = PluginHost::global(cx).unwrap().manager().unwrap().clone();
            mgr.approve_permissions("com.test.theme").unwrap();
            PluginHost::drain_updates_for_test(cx);
        });
        cx.run_until_parked();

        cx.update(|cx| {
            let has_theme = theme::ThemeRegistry::global(cx)
                .custom_themes()
                .iter()
                .any(|t| t.id() == "plugin-neon-test");
            assert!(has_theme, "enabled plugin theme should be registered");

            // Toggle Safe Mode ON -> plugin theme must be cleanly unregistered!
            PluginHost::set_safe_mode(true, cx).unwrap();
            let has_theme_safe = theme::ThemeRegistry::global(cx)
                .custom_themes()
                .iter()
                .any(|t| t.id() == "plugin-neon-test");
            assert!(
                !has_theme_safe,
                "plugin theme leaked into ThemeRegistry after entering Safe Mode"
            );

            // Attempt init with a bad db_path (a directory) -> spawn fails, but PluginHost
            // must be restored in Safe Mode rather than leaving the global removed.
            let bad_db_dir = tmp.path().join("not-a-db-file");
            std::fs::create_dir_all(&bad_db_dir).unwrap();
            let res = PluginHost::init(
                PluginHostConfig {
                    plugins_dir: plugins_dir.clone(),
                    registry_cache_dir: cache_dir.clone(),
                    db_path: Some(bad_db_dir),
                    secrets_vault_path: vault_path,
                    secrets_key_path: key_path,
                    safe_mode: false,
                    locale: "en".to_owned(),
                    theme_appearance: "light".to_owned(),
                    hot_reload: false,
                },
                cx,
            );
            assert!(res.is_err(), "spawning with directory db_path should fail");
            let host = PluginHost::global(cx)
                .expect("PluginHost global must be restored after spawn failure");
            assert!(host.is_safe_mode(), "fallback PluginHost must be in safe mode");
            PluginHost::shutdown(cx);
        });
    }

    #[gpui::test]
    fn search_async_clears_immediately_on_empty_query_and_plugin_view_surfaces_reload_error(
        cx: &mut TestAppContext,
    ) {
        let _tmp = setup_host(cx);
        cx.update(|cx| {
            PluginBus::update(cx, |bus| {
                bus.search_hits.push(desktop_common::plugin_bus::PluginSearchHit {
                    id: "h1".to_owned(),
                    plugin_id: "com.test.s".to_owned(),
                    provider_id: "p".to_owned(),
                    title: "Stale Hit".to_owned(),
                    subtitle: None,
                    badge: None,
                    task_id: None,
                    command_id: None,
                    command_arg: None,
                });
            });
            PluginHost::search_async("   ".to_owned(), cx);
            assert!(
                PluginBus::global(cx).unwrap().search_hits.is_empty(),
                "empty query must clear search_hits immediately"
            );

            // Plugin-level reload error under (plugin_id, "") must surface in PluginView.
            PluginHost::set_view_error(
                "com.test.reloaderr",
                "",
                "manifest parse error".to_owned(),
                cx,
            );
        });

        let (_view, cx) =
            cx.add_window_view(|_, cx| PluginView::new("com.test.reloaderr", "main", cx));
        cx.simulate_resize(size(px(640.), px(480.)));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("plugin-view-error-fallback").is_some(),
            "PluginView must surface plugin-level reload error under (plugin_id, \"\")"
        );

        cx.update(|_, cx| PluginHost::shutdown(cx));
    }

    fn crc32_ieee(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn build_stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for &(name, data) in files {
            let offset = out.len() as u32;
            let crc = crc32_ieee(data);
            let name_bytes = name.as_bytes();
            let size = data.len() as u32;

            // Local file header
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // store
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(data);

            // Central directory header
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name_bytes);
        }
        let cd_offset = out.len() as u32;
        let cd_size = central.len() as u32;
        out.extend_from_slice(&central);
        // End of central directory
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[gpui::test]
    fn zip_overwrite_requires_approval_and_uninstall_purges_state_and_escape_works(
        cx: &mut TestAppContext,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        let cache_dir = plugins_dir.join(".registry-cache");
        let db_path = tmp.path().join("tasks.db");
        let vault_path = tmp.path().join("secrets.vault");
        let key_path = tmp.path().join("secrets.key");
        let seed_store = Store::open(&db_path).unwrap();

        let plugin_id = "com.test.victim";
        let victim_dir = plugins_dir.join(plugin_id);
        write_test_plugin(
            &victim_dir,
            r#"{
                "id": "com.test.victim",
                "name": "Victim Plugin",
                "version": "1.0.0",
                "api_version": 1,
                "entry": "index.js",
                "permissions": ["tasks:read"]
            }"#,
            "// original-trusted-code",
        );
        seed_store
            .plugin_kv_set(plugin_id, "k", "secret-val")
            .unwrap();
        seed_store
            .plugin_meta_set(plugin_id, "task", "t1", "m", "v")
            .unwrap();

        let vault_path_check = vault_path.clone();
        let key_path_check = key_path.clone();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(CurrentLocale(Locale::En));
            PluginHost::init(
                PluginHostConfig {
                    plugins_dir: plugins_dir.clone(),
                    registry_cache_dir: cache_dir,
                    db_path: Some(db_path.clone()),
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
            let mgr = PluginHost::global(cx).unwrap().manager().unwrap().clone();
            mgr.approve_permissions(plugin_id).unwrap();
            mgr.set_secret(plugin_id, "api_token", "top-secret-token")
                .unwrap();
            PluginHost::drain_updates_for_test(cx);
        });
        cx.run_until_parked();

        // Build an untrusted replacement .zip claiming the SAME plugin_id & permissions.
        let evil_zip_path = tmp.path().join("evil.zip");
        let zip_bytes = build_stored_zip(&[
            (
                "manifest.json",
                br#"{
                    "id": "com.test.victim",
                    "name": "Victim Plugin",
                    "version": "1.0.1",
                    "api_version": 1,
                    "entry": "index.js",
                    "permissions": ["tasks:read"]
                }"#,
            ),
            ("index.js", b"// malicious-overwrite-code"),
        ]);
        std::fs::write(&evil_zip_path, zip_bytes).unwrap();

        let store = Rc::new(Store::open(&db_path).unwrap());
        let (settings, cx) =
            cx.add_window_view(|_, cx| PluginSettingsView::new(Rc::clone(&store), cx));
        cx.simulate_resize(size(px(800.), px(640.)));
        cx.run_until_parked();

        // Trigger local ZIP install over existing plugin.
        cx.update(|_, cx| {
            settings.update(cx, |view, cx| {
                view.install_from_zip_path(evil_zip_path.clone(), cx);
            });
        });
        cx.run_until_parked();

        // MUST prompt for re-approval and MUST NOT have overwritten disk files yet!
        cx.update(|_, cx| {
            assert!(
                settings.read(cx).has_pending_permission_modal(),
                "local ZIP overwrite must require explicit re-approval modal"
            );
        });
        let on_disk = std::fs::read_to_string(victim_dir.join("index.js")).unwrap();
        assert_eq!(
            on_disk, "// original-trusted-code",
            "local ZIP overwrite must NOT overwrite plugins_dir/<id> before approval"
        );

        // Escape cancels the pending permission modal (handle_escape returns true).
        cx.update(|_, cx| {
            let handled = settings.update(cx, |view, cx| view.handle_escape(cx));
            assert!(handled, "handle_escape should close pending permission modal");
            assert!(!settings.read(cx).has_pending_permission_modal());
            let handled_again = settings.update(cx, |view, cx| view.handle_escape(cx));
            assert!(
                !handled_again,
                "handle_escape should return false when nothing transient is open"
            );
        });

        // Uninstall must remove files, delete DB plugin_state/KV/meta, and clear UI input caches.
        cx.update(|_, cx| {
            settings.update(cx, |view, cx| {
                view.uninstall_plugin(plugin_id.to_owned(), cx);
            });
        });
        cx.run_until_parked();

        assert!(!victim_dir.exists(), "uninstall must remove plugin dir");
        assert!(
            store.plugin_state(plugin_id).unwrap().is_none(),
            "uninstall must delete plugin_state row"
        );
        assert!(
            store.plugin_kv_get(plugin_id, "k").unwrap().is_none(),
            "uninstall must clear plugin_kv"
        );
        assert!(
            store
                .plugin_meta_get(plugin_id, "task", "t1", "m")
                .unwrap()
                .is_none(),
            "uninstall must clear plugin_entity_meta"
        );
        let vault = plugins::SecretsVault::open(&vault_path_check, &key_path_check).unwrap();
        assert!(
            vault.get(plugin_id, "api_token").unwrap().is_none(),
            "uninstall must revoke plugin secrets in SecretsVault"
        );
        cx.update(|_, cx| {
            assert!(
                !settings.read(cx).has_cached_inputs_for(plugin_id),
                "uninstall must clear cached setting/secret inputs"
            );
            PluginHost::shutdown(cx);
        });
    }

    #[gpui::test]
    fn input_slot_drop_cancels_pending_debounce(cx: &mut TestAppContext) {
        let _tmp = setup_host(cx);
        let events = Rc::new(Cell::new(0usize));
        let events_c = Rc::clone(&events);

        struct InputHostView {
            state: UiTreeState,
            show: bool,
            events: Rc<Cell<usize>>,
        }
        impl Render for InputHostView {
            fn render(
                &mut self,
                _window: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                if !self.show {
                    self.state.clear();
                    return div().into_any_element();
                }
                let payload = UiTreePayload {
                    plugin_id: "com.test.deb".to_owned(),
                    view_id: "main".to_owned(),
                    root: UiNode::Input {
                        key: "q".to_owned(),
                        value: String::new(),
                        revision: 1,
                        placeholder: Some("Search".to_owned()),
                        disabled: false,
                        on_change: Some(HandlerId(1)),
                        on_submit: None,
                    },
                };
                let events = Rc::clone(&self.events);
                let sink: crate::view::UiEventSink = Rc::new(move |_, _| {
                    events.set(events.get() + 1);
                });
                let notify: crate::view::UiNotifySink = Rc::new(|_| {});
                self.state.render_tree(&payload, sink, notify, cx)
            }
        }

        let (view, cx) = cx.add_window_view(|_, _| InputHostView {
            state: UiTreeState::new(),
            show: true,
            events: events_c,
        });
        cx.simulate_resize(size(px(400.), px(200.)));
        cx.run_until_parked();

        // Focus and type to start the 150ms debounce timer, then immediately unmount/clear.
        cx.update(|window, cx| {
            let inp = view.read(cx).state.input_entity("q").unwrap().clone();
            window.focus(&inp.read(cx).focus_handle(cx), cx);
        });
        cx.simulate_input("hello");
        cx.update(|_, cx| {
            view.update(cx, |this, cx| {
                this.show = false;
                this.state.clear();
                cx.notify();
            });
        });
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(300));
        cx.run_until_parked();

        assert_eq!(
            events.get(),
            0,
            "dropping InputSlot must cancel pending debounce without firing on_change"
        );
        cx.update(|_, cx| PluginHost::shutdown(cx));
    }

    #[gpui::test]
    fn concurrent_plugin_settings_and_secret_placeholder_update_cleanly(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        let cache_dir = plugins_dir.join(".registry-cache");
        let db_path = tmp.path().join("tasks.db");
        let vault_path = tmp.path().join("secrets.vault");
        let key_path = tmp.path().join("secrets.key");
        let _seed_store = Store::open(&db_path).unwrap();

        let plugin_id = "com.test.settings";
        write_test_plugin(
            &plugins_dir.join(plugin_id),
            r#"{
                "id": "com.test.settings",
                "name": "Settings Plugin",
                "version": "1.0.0",
                "api_version": 1,
                "entry": "index.js",
                "permissions": [],
                "settings": [
                    { "key": "flag_a", "label": "Flag A", "type": "boolean", "default": false },
                    { "key": "flag_b", "label": "Flag B", "type": "boolean", "default": false },
                    { "key": "count", "label": "Count", "type": "number", "default": 5, "step": 1 },
                    { "key": "token", "label": "Token", "type": "secret", "placeholder": "Enter token" }
                ]
            }"#,
            "export function activate() {}",
        );

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(CurrentLocale(Locale::En));
            PluginHost::init(
                PluginHostConfig {
                    plugins_dir: plugins_dir.clone(),
                    registry_cache_dir: cache_dir,
                    db_path: Some(db_path.clone()),
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
            let mgr = PluginHost::global(cx).unwrap().manager().unwrap().clone();
            mgr.approve_permissions(plugin_id).unwrap();
            PluginHost::drain_updates_for_test(cx);
        });
        cx.run_until_parked();

        let store = Rc::new(Store::open(&db_path).unwrap());
        let (settings, cx) =
            cx.add_window_view(|_, cx| PluginSettingsView::new(Rc::clone(&store), cx));
        cx.simulate_resize(size(px(900.), px(720.)));
        cx.run_until_parked();

        // Fire two setting updates back-to-back before the worker responds; both must persist!
        cx.update(|_, cx| {
            settings.update(cx, |view, cx| {
                view.update_plugin_setting_value_for_test(
                    plugin_id,
                    "flag_a",
                    serde_json::json!(true),
                    cx,
                );
                view.update_plugin_setting_value_for_test(
                    plugin_id,
                    "flag_b",
                    serde_json::json!(true),
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| PluginHost::drain_updates_for_test(cx));
        cx.run_until_parked();

        cx.update(|_, cx| {
            let info = PluginHost::global(cx).unwrap().plugin(plugin_id).unwrap();
            assert_eq!(info.settings_values.get("flag_a"), Some(&serde_json::json!(true)));
            assert_eq!(info.settings_values.get("flag_b"), Some(&serde_json::json!(true)));
            PluginHost::shutdown(cx);
        });
    }
}
