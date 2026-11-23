use super::*;
use gpui::TestAppContext;

const DEFAULT_JSON: &str = include_str!("../../../../assets/keymaps/default.json");

/// Formats a captured GPUI `Keystroke` into canonical keymap JSON syntax when suitable for binding.
fn record_keystroke(ks: &Keystroke, allow_unmodified: bool) -> Option<String> {
    match classify_keystroke(ks, allow_unmodified) {
        RecordOutcome::Accepted(key) => Some(key),
        RecordOutcome::IgnoreModifier | RecordOutcome::Rejected => None,
    }
}

#[test]
fn resolved_shortcuts_read_defaults_without_conflicts() {
    let list = resolve_shortcuts_with_user(DEFAULT_JSON, None).unwrap();
    assert_eq!(list.len(), SHORTCUT_SPECS.len());
    assert_eq!(SHORTCUT_SPECS.len(), fern_i18n::Locale::En.strings().shortcut_labels.len());
    let find = |action: &str| list.iter().find(|r| r.spec.action == action).unwrap();

    assert!(!find("fern::NewTask").keys.is_empty());
    assert_eq!(find("fern::DismissTransient").keys, ["Esc"]);
    let mut next = find("task_list::SelectNext").keys.clone();
    next.sort();
    assert_eq!(next, ["J", "↓"]);
    assert_eq!(find("task_list::PinTask").keys, ["Tab, P"]);
    assert_eq!(
        find("fern::ShowToday").fixed_chord_parts,
        Some(("G".to_owned(), "T".to_owned()))
    );
    assert_eq!(
        find("task_list::ViewKanban").fixed_chord_parts,
        Some(("V".to_owned(), "K".to_owned()))
    );
    assert_eq!(find("sticky::TogglePin").keys, ["F12"]);
    for row in &list {
        assert!(
            row.conflicts.is_empty(),
            "default keymap should not have conflicts, got {:?} for {}",
            row.conflicts,
            row.spec.action
        );
    }
    if cfg!(target_os = "macos") {
        assert_eq!(display_keys("secondary-shift-p"), "⇧⌘P");
        assert_eq!(display_keys("ctrl-shift-m"), "⌃⇧M");
        assert_eq!(display_keys("secondary--"), "⌘-");
    } else {
        assert_eq!(display_keys("secondary-shift-p"), "Ctrl+Shift+P");
        assert_eq!(display_keys("ctrl-shift-m"), "Ctrl+Shift+M");
        assert_eq!(display_keys("secondary--"), "Ctrl+-");
    }
}

#[gpui::test]
fn default_keymap_loads_without_errors(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let errors = load_default(&assets::Assets, cx).unwrap();
        assert!(errors.is_empty(), "keymap errors: {errors:?}");
    });
}

#[gpui::test]
fn unknown_actions_are_reported(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let errors = load(r#"[{"bindings": {"ctrl-x": "fern::NoSuchAction"}}]"#, cx).unwrap();
        assert_eq!(errors.len(), 1);
    });
}

#[gpui::test]
fn user_keymap_overrides_unbinds_and_adds_minimize_window(cx: &mut TestAppContext) {
    let user_keymap = r#"[
        {
            "context": "Workspace && !TextInput",
            "bindings": {
                "escape": "fern::MinimizeWindow"
            }
        },
        {
            "context": "TaskList && !TextInput",
            "bindings": {
                "space": null
            }
        },
        {
            "bindings": {
                "secondary-q": null,
                "ctrl-shift-m": "fern::NewTask"
            }
        }
    ]"#;

    cx.update(|cx| {
        let errors = load_with_user(&assets::Assets, Some(user_keymap), cx).unwrap();
        assert!(errors.is_empty(), "user keymap errors: {errors:?}");

        // Broken user JSON reports an error without failing default bindings.
        let broken_errors = load_with_user(&assets::Assets, Some("{not valid json"), cx).unwrap();
        assert_eq!(broken_errors.len(), 1);
        assert!(broken_errors[0].contains(USER_KEYMAP_FILE));
    });

    let list = resolve_shortcuts_with_user(DEFAULT_JSON, Some(user_keymap)).unwrap();
    let find = |action: &str| list.iter().find(|r| r.spec.action == action).unwrap();
    assert_eq!(find("fern::MinimizeWindow").keys, ["Esc"]);
    assert_eq!(find("fern::DismissTransient").keys, ["Esc"]);
    assert!(find("fern::MinimizeWindow").conflicts.is_empty());
    assert!(find("fern::Quit").keys.is_empty());
    assert!(find("fern::NewTask").keys.contains(&display_keys("ctrl-shift-m")));
    assert_eq!(find("task_list::ToggleComplete").keys, ["Tab, M"]);
}

#[test]
fn update_user_keymap_preserves_comments_other_rules_and_fixed_chords() {
    let existing = r#"// Personal header comment
[
  {
    "context": "Workspace && !TextInput",
    "bindings": {
      "escape": "fern::MinimizeWindow"
    }
  }
]
"#;
    // Rebind ShowToday direct slot without touching `g t` or the custom MinimizeWindow rule.
    let updated = update_user_keymap_source(
        DEFAULT_JSON,
        Some(existing),
        "fern::ShowToday",
        Some("ctrl-alt-2"),
    )
    .unwrap();
    assert!(updated.starts_with("// Personal header comment\n"));
    let resolved = resolve_shortcuts_with_user(DEFAULT_JSON, Some(&updated)).unwrap();
    let find = |action: &str| resolved.iter().find(|r| r.spec.action == action).unwrap();
    assert_eq!(find("fern::MinimizeWindow").keys, ["Esc"]);
    assert_eq!(
        find("fern::ShowToday").fixed_chord_parts,
        Some(("G".to_owned(), "T".to_owned()))
    );
    assert_eq!(find("fern::ShowToday").keys, [display_keys("ctrl-alt-2")]);

    // Clearing ShowToday direct slot unbinds default direct key while keeping `g t`.
    let cleared =
        update_user_keymap_source(DEFAULT_JSON, Some(&updated), "fern::ShowToday", None)
            .unwrap();
    let resolved_cleared = resolve_shortcuts_with_user(DEFAULT_JSON, Some(&cleared)).unwrap();
    let today = resolved_cleared
        .iter()
        .find(|r| r.spec.action == "fern::ShowToday")
        .unwrap();
    assert_eq!(today.fixed_chord_parts, Some(("G".to_owned(), "T".to_owned())));
    assert!(today.keys.is_empty());

    // Setting ShowToday back to its platform default removes the Workspace override cleanly.
    let default_key = if cfg!(target_os = "macos") { "cmd-2" } else { "ctrl-2" };
    let restored = update_user_keymap_source(
        DEFAULT_JSON,
        Some(&cleared),
        "fern::ShowToday",
        Some(default_key),
    )
    .unwrap();
    assert!(!restored.contains("ShowToday"));
    assert!(restored.contains("fern::MinimizeWindow"));

    // Broken user JSON is never silently overwritten.
    assert!(
        update_user_keymap_source(DEFAULT_JSON, Some("{broken"), "fern::SyncNow", Some("ctrl-r"))
            .is_err()
    );
}

#[test]
fn conflict_detection_flags_overlapping_contexts_only() {
    let conflicting = r#"[
        {
            "context": "Workspace",
            "bindings": {
                "secondary-s": "fern::PrintList"
            }
        },
        {
            "context": "StickyNote",
            "bindings": {
                "secondary-n": "sticky::TogglePin"
            }
        }
    ]"#;
    // Note: binding secondary-s in Workspace replaces SyncNow in effective resolution; to create
    // a real conflict between two distinct active actions, bind PrintList globally to secondary-n
    // while NewTask is bound in Workspace.
    let overlap = r#"[
        {
            "bindings": {
                "secondary-n": "fern::PrintList"
            }
        },
        {
            "context": "Workspace",
            "bindings": {
                "secondary-n": "fern::NewTask"
            }
        },
        {
            "context": "StickyNote",
            "bindings": {
                "secondary-n": "sticky::TogglePin"
            }
        }
    ]"#;
    let _ = conflicting;
    let list = resolve_shortcuts_with_user(DEFAULT_JSON, Some(overlap)).unwrap();
    let idx = |action: &str| SHORTCUT_SPECS.iter().position(|s| s.action == action).unwrap();
    let print_idx = idx("fern::PrintList");
    let new_task_idx = idx("fern::NewTask");
    let sticky_idx = idx("sticky::TogglePin");
    assert!(list[print_idx].conflicts.contains(&new_task_idx));
    assert!(list[new_task_idx].conflicts.contains(&print_idx));
    // StickyNote overlaps with global `None` (PrintList), but NOT with Workspace (NewTask).
    assert!(!list[sticky_idx].conflicts.contains(&new_task_idx));
}

#[test]
fn record_keystroke_handles_modifiers_function_keys_and_rejections() {
    assert_eq!(
        record_keystroke(&Keystroke::parse("ctrl-shift-p").unwrap(), false).as_deref(),
        Some("ctrl-shift-p")
    );
    assert_eq!(
        record_keystroke(&Keystroke::parse("alt-delete").unwrap(), false).as_deref(),
        Some("alt-delete")
    );
    assert_eq!(
        record_keystroke(&Keystroke::parse("f12").unwrap(), false).as_deref(),
        Some("f12")
    );
    assert_eq!(
        record_keystroke(&Keystroke::parse("shift-enter").unwrap(), false).as_deref(),
        Some("shift-enter")
    );
    // Unmodified Escape / Backspace / Delete are reserved for cancel / clear.
    assert_eq!(record_keystroke(&Keystroke::parse("escape").unwrap(), true), None);
    assert_eq!(record_keystroke(&Keystroke::parse("backspace").unwrap(), true), None);
    assert_eq!(record_keystroke(&Keystroke::parse("delete").unwrap(), true), None);
    // Bare letters rejected in Workspace, accepted in allow_unmodified contexts.
    assert_eq!(
        classify_keystroke(&Keystroke::parse("a").unwrap(), false),
        RecordOutcome::Rejected
    );
    assert_eq!(record_keystroke(&Keystroke::parse("a").unwrap(), true).as_deref(), Some("a"));
    assert_eq!(
        classify_keystroke(&Keystroke::parse("space").unwrap(), false),
        RecordOutcome::Rejected
    );
    assert_eq!(
        record_keystroke(&Keystroke::parse("space").unwrap(), true).as_deref(),
        Some("space")
    );
    assert_eq!(
        classify_keystroke(
            &Keystroke { key: "control".to_owned(), ..Default::default() },
            false
        ),
        RecordOutcome::IgnoreModifier
    );
}

#[test]
fn user_keymap_template_documents_all_shortcut_specs_and_upgrades_empty_legacy_header() {
    for spec in SHORTCUT_SPECS {
        assert!(
            USER_KEYMAP_TEMPLATE.contains(spec.action),
            "USER_KEYMAP_TEMPLATE should document {}",
            spec.action
        );
    }
    let dir = std::env::temp_dir().join(format!("fern-keymap-upgrade-{}", fern_core::Id::now_v7()));
    let path = dir.join(USER_KEYMAP_FILE);
    // Old empty auto-generated template gets upgraded cleanly.
    write_file_creating_parents(&path, "// Fern user keymap — old header\n[\n]\n").unwrap();
    ensure_user_keymap_file_at(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), USER_KEYMAP_TEMPLATE);

    // User-written empty file with custom comment is never overwritten.
    let custom_comment = "// My own notes\n[]\n";
    std::fs::write(&path, custom_comment).unwrap();
    ensure_user_keymap_file_at(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), custom_comment);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ensure_user_keymap_creates_valid_template_without_overwriting() {
    let dir = std::env::temp_dir().join(format!("fern-keymap-test-{}", fern_core::Id::now_v7()));
    let path = dir.join(USER_KEYMAP_FILE);
    ensure_user_keymap_file_at(&path).unwrap();
    let created = std::fs::read_to_string(&path).unwrap();
    assert_eq!(created, USER_KEYMAP_TEMPLATE);
    assert!(resolve_shortcuts_with_user(DEFAULT_JSON, Some(&created)).is_ok());

    let commented = r#"
        // Comment before array
        [
          {
            /* Block comment */
            "context": "Workspace && !TextInput",
            "bindings": {
              "escape": "fern::MinimizeWindow" // Inline comment
            }
          }
        ]
    "#;
    let list = resolve_shortcuts_with_user(DEFAULT_JSON, Some(commented)).unwrap();
    let minimize = list
        .iter()
        .find(|r| r.spec.action == "fern::MinimizeWindow")
        .unwrap();
    assert_eq!(minimize.keys, ["Esc"]);

    let custom = r#"[{"bindings": {"escape": "fern::MinimizeWindow"}}]"#;
    std::fs::write(&path, custom).unwrap();
    ensure_user_keymap_file_at(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn things_view_s_opens_when_without_shadowing_workspace_sync_now(
    cx: &mut TestAppContext,
) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use gpui::{
        Context, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render,
        Window, div,
    };

    struct Harness {
        workspace_focus: FocusHandle,
        things_focus: FocusHandle,
        text_focus: FocusHandle,
        when_hits: Arc<AtomicUsize>,
        sync_hits: Arc<AtomicUsize>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let when_hits = Arc::clone(&self.when_hits);
            let sync_hits = Arc::clone(&self.sync_hits);
            div()
                .key_context("Workspace")
                .track_focus(&self.workspace_focus)
                .on_action(move |_: &crate::actions::SyncNow, _, _| {
                    sync_hits.fetch_add(1, Ordering::SeqCst);
                })
                .child(
                    div()
                        .key_context("ThingsView")
                        .track_focus(&self.things_focus)
                        .on_action(move |_: &crate::actions::OpenWhen, _, _| {
                            when_hits.fetch_add(1, Ordering::SeqCst);
                        })
                        .child(div().key_context("TextInput").track_focus(&self.text_focus)),
                )
        }
    }

    cx.update(|cx| {
        let errors = load_default(&assets::Assets, cx).unwrap();
        assert!(errors.is_empty(), "keymap errors: {errors:?}");
    });

    let when_hits = Arc::new(AtomicUsize::new(0));
    let sync_hits = Arc::new(AtomicUsize::new(0));
    let (view, wcx) = cx.add_window_view(|_, cx| Harness {
        workspace_focus: cx.focus_handle(),
        things_focus: cx.focus_handle(),
        text_focus: cx.focus_handle(),
        when_hits: Arc::clone(&when_hits),
        sync_hits: Arc::clone(&sync_hits),
    });

    // 1. Inside ThingsView (outside TextInput), `s` dispatches things::OpenWhen.
    wcx.update(|window, cx| {
        let handle = view.read(cx).things_focus.clone();
        handle.focus(window, cx);
    });
    wcx.simulate_keystrokes("s");
    assert_eq!(when_hits.load(Ordering::SeqCst), 1);
    assert_eq!(sync_hits.load(Ordering::SeqCst), 0);

    // 2. Inside ThingsView, secondary-s dispatches Workspace SyncNow directly (not shadowed).
    wcx.simulate_keystrokes("secondary-s");
    assert_eq!(when_hits.load(Ordering::SeqCst), 1);
    assert_eq!(sync_hits.load(Ordering::SeqCst), 1);

    // 3. Inside a TextInput within ThingsView, `s` does not trigger things::OpenWhen.
    wcx.update(|window, cx| {
        let handle = view.read(cx).text_focus.clone();
        handle.focus(window, cx);
    });
    wcx.simulate_keystrokes("s");
    assert_eq!(when_hits.load(Ordering::SeqCst), 1);

    // 4. Outside ThingsView (focus on Workspace), `s` does not trigger OpenWhen and secondary-s triggers SyncNow.
    wcx.update(|window, cx| {
        let handle = view.read(cx).workspace_focus.clone();
        handle.focus(window, cx);
    });
    wcx.simulate_keystrokes("s");
    assert_eq!(when_hits.load(Ordering::SeqCst), 1);
    wcx.simulate_keystrokes("secondary-s");
    assert_eq!(sync_hits.load(Ordering::SeqCst), 2);
}
