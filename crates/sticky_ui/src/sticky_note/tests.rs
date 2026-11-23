use super::*;
#[cfg(debug_assertions)]
use gpui::Focusable as _;
#[cfg(debug_assertions)]
use gpui::Modifiers;
use gpui::TestAppContext;

fn new_note(store: Rc<Store>, id: Id, window: &mut Window, cx: &mut Context<StickyNote>) -> StickyNote {
    let bounds = window.window_bounds().get_bounds();
    let note = NoteWindow::capture(id, false, bounds);
    let settings = StickySettings::current(&store, cx);
    StickyNote::new(store, note, bounds, settings, window, cx)
}

#[gpui::test]
fn native_title_follows_renames_and_navigation_in_its_own_note(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let first = store.add_task("First").unwrap().id;
    let second = store.add_task("Second").unwrap().id;
    let child = store.add_subtask(first, "Child").unwrap().id;
    let (first_window, second_window) = cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        open(store.clone(), first, cx);
        open(store.clone(), second, cx);
        let handles = &cx.global::<OpenNotes>().0;
        (handles[0], handles[1])
    });
    let mut first_cx = gpui::VisualTestContext::from_window(first_window.into(), cx);
    let mut second_cx = gpui::VisualTestContext::from_window(second_window.into(), cx);
    assert_eq!(first_cx.window_title().as_deref(), Some("First"));

    store.set_title(first, " New\r\ntitle ").unwrap();
    first_window.update(cx, |note, _, cx| {
        note.detail.update(cx, |_, cx| cx.emit(TaskUpdated));
    }).unwrap();
    assert_eq!(first_cx.window_title().as_deref(), Some("New title"));

    store.set_title(first, "").unwrap();
    second_window.update(cx, |_, _, cx| data_changed(cx.entity_id(), cx)).unwrap();
    assert_eq!(first_cx.window_title().as_deref(), Some("Untitled"));
    cx.update(|cx| cx.set_global(i18n::CurrentLocale(i18n::Locale::Ru)));
    assert_eq!(first_cx.window_title().as_deref(), Some(i18n::Locale::Ru.strings().untitled));

    first_window.update(cx, |note, window, cx| note.navigate(child, window, cx)).unwrap();
    assert_eq!(first_cx.window_title().as_deref(), Some("Child"));
    assert_eq!(second_cx.window_title().as_deref(), Some("Second"));
}

#[gpui::test]
#[cfg(debug_assertions)]
fn compact_note_reopens_at_one_line_and_restores_its_previous_height(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let title = "A long note title that needs to fit on a single line when it is collapsed";
    let task = store.add_task(title).unwrap().id;
    cx.update(theme::init);
    let (note, cx) = cx.add_window_view(|window, cx| {
        new_note(store.clone(), task, window, cx)
    });
    cx.simulate_resize(size(px(324.), px(318.)));

    let title_bar = cx.debug_bounds("sticky-title-bar").unwrap();
    let title_press = |click_count| gpui::MouseDownEvent {
        button: MouseButton::Left,
        position: title_bar.center(),
        modifiers: Modifiers::default(),
        click_count,
        first_mouse: false,
    };
    // Two quick double-clicks, followed by another pair after a pause.
    for (click_count, compact) in [(1, false), (2, true), (3, true), (4, false), (1, false), (2, true)] {
        cx.simulate_event(title_press(click_count));
        cx.update(|window, cx| {
            assert_eq!(note.read(cx).compact, compact, "click {click_count}");
            let height = if compact { px(COMPACT_HEIGHT) } else { px(318.) };
            assert_eq!(window.bounds().size, size(px(324.), height));
        });
    }
    assert!(cx.debug_bounds("task-detail-body").is_none());
    for height in [COMPACT_HEIGHT, COMPACT_HEIGHT + 10.] {
        cx.simulate_resize(size(px(324.), px(height)));
        for selector in ["sticky-compact-title", "sticky-title-controls"] {
            let bounds = cx.debug_bounds(selector).unwrap();
            assert!(bounds.size.height <= px(COMPACT_HEIGHT));
            assert!((bounds.center().y - px(height / 2.)).abs() <= px(1.), "{selector}: {bounds:?}");
        }
    }
    let saved = sticky_windows::load(&store).pop().unwrap();
    assert!(saved.compact);
    assert_eq!(saved.width, 324.);
    assert_eq!(saved.height, 318.);

    let palette = cx.update(|window, cx| {
        note.update(cx, |note, cx| {
            note.toggle_palette(window, cx);
            note.palette.unwrap()
        })
    });
    let mut palette_cx = gpui::VisualTestContext::from_window(palette.into(), cx);
    let swatch = palette_cx.debug_bounds("sticky-palette-color-1").unwrap().center();
    palette_cx.simulate_click(swatch, Modifiers::default());
    cx.update(|_, cx| {
        assert!(note.read(cx).compact);
        assert_eq!(note.read(cx).color, Some(1));
    });
    assert_eq!(sticky_windows::load(&store)[0].color, Some(1));

    cx.update(|_, cx| {
        open(store.clone(), task, cx);
        let reopened = *cx.global::<OpenNotes>().0.last().unwrap();
        reopened.update(cx, |note, window, _| {
            assert!(note.compact);
            assert_eq!(window.viewport_size().height, px(COMPACT_HEIGHT));
            window.remove_window();
        }).unwrap();
    });

    cx.simulate_event(title_press(1));
    cx.simulate_event(title_press(2));
    cx.update(|_, cx| {
        assert!(!note.read(cx).compact);
        assert_eq!(note.read(cx).expanded_height, px(318.));
    });
    assert!(cx.debug_bounds("task-detail-body").is_some());
    assert!(!sticky_windows::load(&store)[0].compact);
}

#[gpui::test]
#[cfg(debug_assertions)]
fn small_note_opens_a_full_date_picker_and_saves_the_selected_day(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Small note").unwrap();
    let original = chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
    store.set_due(task.id, Some(original), None).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let (note, cx) = cx.add_window_view(|window, cx| {
        new_note(store.clone(), task.id, window, cx)
    });
    let viewport = size(px(240.), px(200.));
    cx.simulate_resize(viewport);
    cx.run_until_parked();
    let date = cx.debug_bounds("task-detail-due-date").unwrap().center();
    let modifiers = Modifiers::default();
    cx.simulate_mouse_move(date, None, modifiers);
    cx.simulate_click(date, modifiers);
    assert!(cx.debug_bounds("due-popup-ok").is_none(), "the picker is outside the note");
    let popup = cx.update(|_, cx| note.read(cx).detail_popup.expect("native date picker is open"));
    cx.simulate_resize(viewport); // Windows sends unchanged bounds when a child popup opens.
    cx.update(|_, cx| assert_eq!(note.read(cx).detail_popup, Some(popup)));
    let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
    let picked = chrono::NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
    let last_day = popup_cx.debug_bounds("due-day-2026-12-01").unwrap();
    let confirm = popup_cx.debug_bounds("due-popup-ok").unwrap();
    popup_cx.update(|window, _| {
        assert!(last_day.bottom() < confirm.top());
        assert!(confirm.bottom() <= window.viewport_size().height);
        assert!(window.viewport_size().height > viewport.height);
    });
    popup_cx.simulate_click(last_day.center(), modifiers);
    assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(original), "the picker retains a draft until OK");
    let confirm = popup_cx.debug_bounds("due-popup-ok").unwrap();
    popup_cx.simulate_click(confirm.center(), modifiers);
    assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(picked));
    cx.update(|window, cx| {
        assert_eq!(window.viewport_size(), viewport, "opening a picker does not resize the note");
        assert!(note.read(cx).detail_popup.is_none());
    });
}

#[gpui::test]
#[cfg(debug_assertions)]
fn losing_popup_activation_keeps_the_note_open(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Keep this note").unwrap();
    cx.update(theme::init);
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
    let more = cx.debug_bounds("task-detail-more").unwrap().center();
    cx.simulate_click(more, Modifiers::default());
    let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
    let popup_id = cx.update(|_, cx| popup.update(cx, |_, _, cx| cx.entity_id()).unwrap());
    let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
    popup_cx.update(|window, _| window.activate_window());
    // The note reads its popup during rendering to check whether it still exists.
    let owner = cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(cx); window.window_handle() });
    cx.cx.update(|cx| {
        assert_eq!(cx.with_window(popup_id, |window, _| window.window_handle()), Some(owner));
    });
    popup_cx.deactivate_window();
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(cx.windows().len(), 1, "only the transient menu should close");
        assert!(note.read(cx).detail_popup.is_none());
        assert_eq!(note.read(cx).detail.read(cx).task_id(), Some(task.id));
    });
    assert!(sticky_windows::load(&store).iter().any(|saved| saved.task == task.id && saved.open));
}

#[gpui::test]
#[cfg(debug_assertions)]
fn menu_state_observer_closes_the_popup_after_the_owner_renders_last(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Keep the owner").unwrap();
    cx.update(theme::init);
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store, task.id, window, cx));
    let more = cx.debug_bounds("task-detail-more").unwrap().center();
    cx.simulate_click(more, Modifiers::default());
    let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
        let detail = note.read(cx).detail.clone();
        detail.update(cx, |detail, cx| { detail.dismiss_popup(window, cx); });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert_eq!(cx.windows().len(), 1);
        assert!(popup.read(cx).is_err(), "the menu window must be removed");
        assert!(note.read(cx).detail_popup.is_none());
        assert_eq!(window.root::<StickyNote>().unwrap(), Some(note.clone()));
    });
}

#[gpui::test]
#[cfg(debug_assertions)]
fn small_note_menu_reaches_the_last_action_and_escape_restores_focus(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Small note").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store, task.id, window, cx));
    cx.simulate_resize(size(px(240.), px(200.)));
    let title = cx.debug_bounds("task-detail-title").unwrap().center();
    cx.simulate_click(title, Modifiers::default());
    let focused = cx.update(|window, cx| window.focused(cx).unwrap());
    let more = cx.debug_bounds("task-detail-more").unwrap().center();
    cx.simulate_click(more, Modifiers::default());
    let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
    let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
    popup_cx.run_until_parked();
    let resized = popup_cx.update(|window, _| window.window_bounds().get_bounds().size);
    popup_cx.simulate_resize(resized); // TestWindow does not emit the platform's resize callback.
    let last = popup_cx.debug_bounds("task-menu-more-delete").unwrap();
    popup_cx.update(|window, _| assert!(last.bottom() <= window.viewport_size().height, "last={last:?}, viewport={:?}", window.viewport_size()));
    popup_cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        assert!(note.read(cx).detail_popup.is_none());
        assert!(focused.is_focused(window));
    });
}

#[gpui::test]
#[cfg(debug_assertions)]
fn sticky_submenus_keep_window_geometry_and_main_menu_paint_stable(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Submenu regression").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::Ru));
    });
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store, task.id, window, cx));
    cx.simulate_resize(size(px(240.), px(200.)));
    let more = cx.debug_bounds("task-detail-more").unwrap().center();
    cx.simulate_click(more, Modifiers::default());
    let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
    let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
    popup_cx.run_until_parked();
    let original_window = popup_cx.update(|window, _| window.window_bounds().get_bounds());
    popup_cx.simulate_resize(original_window.size);
    let original_menu = popup_cx.debug_bounds("task-more-popup").unwrap();
    assert!(popup_cx.debug_bounds("submenu-content-more-focus").is_none());
    let trigger = popup_cx.debug_bounds("submenu-more-focus").unwrap().center();
    popup_cx.simulate_mouse_move(trigger, None, Modifiers::default());
    popup_cx.run_until_parked();
    assert!(popup_cx.debug_bounds("submenu-content-more-focus").is_some());
    let expanded_menu = popup_cx.debug_bounds("task-more-popup").unwrap();
    assert_eq!(expanded_menu, original_menu);
    popup_cx.update(|window, _| {
        assert_eq!(window.window_bounds().get_bounds(), original_window, "opening a submenu must not resize or move its native window");
        assert!(expanded_menu.right() <= window.viewport_size().width, "menu={expanded_menu:?}, viewport={:?}", window.viewport_size());
        let menu = expanded_menu.scale(window.scale_factor());
        let quads = window.painted_quads();
        let surface = quads.iter().find(|quad| quad.bounds == menu).expect("main menu surface is painted");
        assert_eq!(surface.bounds.intersect(&surface.content_mask.bounds), surface.bounds, "the main menu must not be clipped after a submenu opens");
    });
    let neighbor = popup_cx.debug_bounds("task-menu-more-attachment").unwrap().center();
    popup_cx.simulate_mouse_move(neighbor, None, Modifiers::default());
    popup_cx.run_until_parked();
    assert!(popup_cx.debug_bounds("submenu-content-more-focus").is_none());
    assert_eq!(popup_cx.debug_bounds("task-more-popup").unwrap(), original_menu);
    popup_cx.update(|window, _| assert_eq!(window.window_bounds().get_bounds(), original_window, "closing a submenu must not resize or move its native window"));

    popup_cx.simulate_mouse_move(trigger, None, Modifiers::default());
    let estimate = popup_cx.debug_bounds("task-focus-estimate").unwrap().center();
    popup_cx.simulate_click(estimate, Modifiers::default());
    let (popup, anchor) = cx.update(|window, cx| {
        let note = note.read(cx);
        (note.detail_popup.unwrap(), note.popup_anchor(window, cx))
    });
    let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
    assert!(popup_cx.debug_bounds("task-estimate-popup").is_some(), "clicking the submenu must execute its action before dismissal");
    assert!(popup_cx.debug_bounds("task-more-popup").is_none());
    popup_cx.update(|window, cx| {
        let bounds = window.bounds();
        assert_ne!(bounds.size, original_window.size);
        let placed = anchor.bounds(bounds.size);
        assert!(
            placed.top() >= anchor.screen.top() && placed.bottom() <= anchor.screen.bottom(),
            "placed={placed:?}, screen={:?}",
            anchor.screen
        );
        if anchor.point.y - bounds.size.height >= anchor.screen.top() {
            assert_eq!(placed.bottom(), anchor.point.y);
        } else {
            assert_eq!(placed.top(), anchor.point.y);
        }
        if cfg!(windows) {
            assert_eq!(bounds, window.pixel_snap_bounds(placed), "a different menu size must remain attached to the note");
        } else {
            // TestPlatform accepts AnchoredPopup but TestWindow::resize only
            // updates its size. The real Wayland backend reruns the positioner;
            // its compositor configure event has no equivalent in TestWindow.
            assert_eq!(bounds.origin, original_window.origin);
        }
        assert_eq!(cx.windows().len(), 2, "repositioning must retire the previous popup");
    });
}

#[gpui::test]
#[cfg(debug_assertions)]
fn sticky_list_sections_keep_the_native_menu_geometry_stable(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("List submenu regression").unwrap();
    let section = store.add_list_section(None, "Inbox section").unwrap();
    cx.update(theme::init);
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
    cx.simulate_resize(size(px(240.), px(200.)));
    cx.update(|_, cx| {
        note.read(cx).detail.clone().update(cx, |detail, cx| detail.dev_open_popup("list", cx));
    });
    let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
    let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
    popup_cx.run_until_parked();
    let original_window = popup_cx.update(|window, _| window.window_bounds().get_bounds());
    popup_cx.simulate_resize(original_window.size);
    let original_menu = popup_cx.debug_bounds("task-list-popup").unwrap();
    assert!(popup_cx.debug_bounds("task-list-sections").is_none());

    let inbox = popup_cx.debug_bounds("move-to-inbox").unwrap().center();
    popup_cx.simulate_mouse_move(inbox, None, Modifiers::default());
    popup_cx.run_until_parked();
    let sections = popup_cx.debug_bounds("task-list-sections").unwrap();
    assert!(sections.right() <= original_menu.left());
    assert!(sections.left() >= px(0.));
    assert_eq!(popup_cx.debug_bounds("task-list-popup").unwrap(), original_menu);
    popup_cx.update(|window, _| assert_eq!(window.window_bounds().get_bounds(), original_window));

    popup_cx.update(|_, cx| {
        note.read(cx).detail.clone().update(cx, |detail, cx| detail.dev_open_popup("list", cx));
    });
    popup_cx.run_until_parked();
    assert!(popup_cx.debug_bounds("task-list-sections").is_none());
    assert_eq!(popup_cx.debug_bounds("task-list-popup").unwrap(), original_menu);
    popup_cx.update(|window, _| assert_eq!(window.window_bounds().get_bounds(), original_window));

    popup_cx.simulate_mouse_move(point(px(1.), px(1.)), None, Modifiers::default());
    popup_cx.simulate_mouse_move(inbox, None, Modifiers::default());
    let section_row = popup_cx.debug_bounds("task-list-sections").unwrap().center();
    popup_cx.simulate_click(section_row, Modifiers::default());
    assert_eq!(store.task(task.id).unwrap().unwrap().section_id, Some(section.id));
    cx.update(|_, cx| assert!(note.read(cx).detail_popup.is_none()));
}

#[gpui::test]
#[cfg(debug_assertions)]
fn clicking_note_editors_dismisses_native_popups_without_losing_the_click(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Editable title").unwrap();
    store.set_notes(task.id, "Editable notes").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
    cx.simulate_resize(size(px(400.), px(800.)));

    for editor in ["task-detail-title", "task-notes-preview", "task-notes-editor"] {
        let target = cx.debug_bounds(editor).unwrap().center();
        let more = cx.debug_bounds("task-detail-more").unwrap().center();
        cx.simulate_click(more, Modifiers::default());
        cx.update(|_, cx| assert!(note.read(cx).detail_popup.is_some()));
        cx.simulate_click(target, Modifiers::default());
        cx.update(|window, cx| {
            let note = note.read(cx);
            assert!(note.detail_popup.is_none(), "clicking {editor} must dismiss the menu");
            let focused = window.focused(cx).expect("the click still focuses the editor");
            assert!(note.detail.read(cx).is_editor_focus(&focused, cx));
        });
    }

    let titlebar = cx.debug_bounds("sticky-title-bar").unwrap();
    let color_button = point(titlebar.right() + px(10.), titlebar.center().y);
    cx.simulate_click(color_button, Modifiers::default());
    cx.update(|_, cx| assert!(note.read(cx).palette.is_some()));
    cx.simulate_click(color_button, Modifiers::default());
    cx.update(|_, cx| assert!(note.read(cx).palette.is_none(), "a second click on Color must close its palette"));
    cx.simulate_click(color_button, Modifiers::default());
    let title = cx.debug_bounds("task-detail-title").unwrap().center();
    cx.simulate_click(title, Modifiers::default());
    cx.update(|_, cx| assert!(note.read(cx).palette.is_none()));
    cx.simulate_keystrokes("enter");
    cx.simulate_input(" after dismissal");
    assert!(store.task(task.id).unwrap().unwrap().notes.ends_with(" after dismissal"));
}

#[gpui::test]
#[cfg(debug_assertions)]
fn sticky_tag_picker_submits_in_its_own_window_and_closes_on_real_resize(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Note with tags").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
    cx.simulate_resize(size(px(240.), px(200.)));
    cx.update(|window, cx| note.update(cx, |note, cx| {
        note.detail.update(cx, |detail, cx| detail.start("tags", window, cx));
    }));
    // Follow the currently active surface between keystrokes: backends without
    // a move API recreate it when the filtered menu's measured size changes.
    for character in "sticky tag".chars() {
        let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
        cx.dispatch_keystroke(popup.into(), gpui::Keystroke::parse(&character.to_string()).unwrap());
    }
    let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
    let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
    popup_cx.update(|window, cx| {
        let (_, [tags, _]) = note.read(cx).detail.read(cx).sticky_popup_inputs(cx);
        assert_eq!(tags.read(cx).text(), "sticky tag", "resizing must retain the input entity and its draft");
        assert!(tags.focus_handle(cx).is_focused(window), "resizing must preserve keyboard focus");
        assert_eq!(cx.windows().len(), 2);
    });
    popup_cx.simulate_keystrokes("enter");
    assert!(store.task(task.id).unwrap().unwrap().tags.is_empty());
    let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
    let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
    let confirm = popup_cx.debug_bounds("task-tags-ok").unwrap();
    popup_cx.simulate_click(confirm.center(), Modifiers::default());
    assert_eq!(store.task(task.id).unwrap().unwrap().tags, ["sticky tag"]);
    let more = cx.debug_bounds("task-detail-more").unwrap().center();
    cx.simulate_click(more, Modifiers::default());
    cx.simulate_resize(size(px(280.), px(230.)));
    cx.update(|_, cx| {
        assert!(note.read(cx).detail_popup.is_none());
        assert!(!note.read(cx).detail.read(cx).has_popup());
    });
}

#[gpui::test]
fn compact_note_tooltip_has_its_own_window_and_preserves_keyboard_focus(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Compact note").unwrap();
    cx.update(theme::init);
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store, task.id, window, cx));
    let lifetime = cx.update(|window, cx| {
        note.update(cx, |note, cx| note.toggle_compact(window, cx));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let tooltip = popup::tooltip("Color")(window, cx);
        assert!(focus.is_focused(window));
        tooltip
    });
    cx.draw(point(px(0.), px(0.)), size(px(0.), px(0.)), |_, _| lifetime.clone().into_any_element());
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(cx.windows().len(), 2));
    cx.update(|_, _| drop(lifetime));
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(cx.windows().len(), 1);
        assert!(note.read(cx).compact);
    });
}

#[gpui::test]
fn navigating_to_an_open_note_keeps_each_window_associated_with_its_task(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        let store = Rc::new(Store::open_in_memory().unwrap());
        let first = store.add_task("First").unwrap().id;
        let second = store.add_task("Second").unwrap().id;
        open(store.clone(), first, cx);
        open(store.clone(), second, cx);
        let handle = cx.global::<OpenNotes>().0[0];
        handle.update(cx, |note, window, cx| {
            note.navigate(second, window, cx);
            assert_eq!(note.detail.read(cx).task_id(), Some(first));
            note.forget(cx);
            window.remove_window();
        }).unwrap();
        let notes = sticky_windows::load(&store);
        assert!(notes.iter().any(|note| note.task == second && note.open));
    });
}

#[gpui::test]
fn navigating_to_a_closed_note_applies_its_saved_color(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let first = store.add_task("First").unwrap().id;
        let second = store.add_task("Second").unwrap().id;
        open(store.clone(), first, cx);
        open(store, second, cx);
        let [first_handle, second_handle] = cx.global::<OpenNotes>().0[..] else { panic!("expected two notes") };
        second_handle.update(cx, |note, window, cx| {
            note.color = Some(10);
            note.remember(window, cx);
            note.forget(cx);
            window.remove_window();
        }).unwrap();
        first_handle.update(cx, |note, window, cx| {
            note.navigate(second, window, cx);
            assert_eq!(note.detail.read(cx).task_id(), Some(second));
            assert_eq!(note.style().color, 10);
        }).unwrap();
    });
}

#[gpui::test]
fn reopening_restores_one_window_and_close_stays_closed(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Saved note").unwrap();
        open(store.clone(), task.id, cx);
        open(store.clone(), task.id, cx);
        assert_eq!(cx.windows().len(), 1, "opening the same task activates its existing window");
        let handle = cx.global::<OpenNotes>().0[0];
        handle.update(cx, |note, window, cx| {
            note.pinned = true;
            note.color = Some(10);
            note.remember(window, cx);
            window.remove_window(); // Simulate app shutdown without explicitly closing a note.
        }).unwrap();
        restore(store.clone(), cx);
        let handle = *cx.global::<OpenNotes>().0.last().unwrap();
        handle.update(cx, |note, window, cx| {
            assert!(note.pinned);
            assert_eq!(note.style().color, 10);
            note.forget(cx);
            window.remove_window();
        }).unwrap();
        assert!(!sticky_windows::load(&store)[0].open);
        let count = cx.windows().len();
        restore(store.clone(), cx);
        assert_eq!(cx.windows().len(), count);
        open(store.clone(), task.id, cx);
        store.delete_task(task.id).unwrap();
        for handle in cx.global::<OpenNotes>().0.clone() {
            handle.update(cx, |_, window, _| window.remove_window()).ok();
        }
        restore(store.clone(), cx);
        assert!(!sticky_windows::load(&store)[0].open, "deleted tasks must not reopen");
    });
}

#[gpui::test]
#[cfg(debug_assertions)]
fn external_notes_and_checklist_changes_sync_into_open_note_editors(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Sync fields").unwrap();
    store.set_notes(task.id, "Initial notes").unwrap();
    let item = store.add_checklist_item(task.id, "Initial item", None).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
    cx.simulate_resize(size(px(360.), px(600.)));

    store.set_notes(task.id, "Updated elsewhere").unwrap();
    store.set_checklist_item_title(item.id, "Renamed elsewhere").unwrap();
    store.add_comment(task.id, "External comment").unwrap();
    let other = cx.update(|_, cx| cx.new(|_| ()).entity_id());
    cx.update(|_, cx| data_changed(other, cx));
    cx.run_until_parked();

    // Clicking into notes and typing appends to the updated text instead of overwriting it.
    let preview = cx.debug_bounds("task-notes-preview").unwrap().center();
    cx.simulate_click(preview, Modifiers::default());
    cx.simulate_input("!");
    assert_eq!(store.task(task.id).unwrap().unwrap().notes, "Updated elsewhere!");

    // Clicking into the checklist row and typing appends to the renamed item title.
    let selector: &'static str = format!("checklist-item-{}", item.id).leak();
    let row = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_click(row, Modifiers::default());
    cx.simulate_input("?");
    assert_eq!(store.checklist(task.id).unwrap()[0].title, "Renamed elsewhere?");
    let _ = note;
}

#[gpui::test]
fn close_and_forget_marks_note_closed_and_removes_window(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Close and forget").unwrap();
        open(store.clone(), task.id, cx);
        assert_eq!(cx.windows().len(), 1);
        let saved = desktop_common::sticky_windows::load(&store);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].task, task.id);
        assert!(saved[0].open);

        let handle = cx.global::<OpenNotes>().0[0];
        handle
            .update(cx, |note, window, cx| note.close_and_forget(window, cx))
            .unwrap();

        assert!(cx.windows().is_empty(), "close_and_forget must remove the sticky window");
        let saved = desktop_common::sticky_windows::load(&store);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].task, task.id);
        assert!(!saved[0].open, "close_and_forget must mark the sticky note closed");
    });
}

#[gpui::test]
fn bounds_changed_debounces_persistence_and_flushes_on_pin_and_forget(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Debounced bounds").unwrap();
    cx.update(theme::init);
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
    let initial_width = sticky_windows::load(&store)[0].width;

    cx.simulate_resize(size(px(300.), px(250.)));
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    cx.simulate_resize(size(px(350.), px(280.)));
    cx.executor().advance_clock(std::time::Duration::from_millis(150));
    cx.run_until_parked();
    assert_eq!(
        sticky_windows::load(&store)[0].width,
        initial_width,
        "rapid resizes must stay debounced until 250ms after the last change"
    );

    cx.executor().advance_clock(std::time::Duration::from_millis(120));
    cx.run_until_parked();
    let saved = sticky_windows::load(&store);
    assert_eq!(saved[0].width, 350.);
    assert_eq!(saved[0].height, 280.);
    note.read_with(cx, |n, _| assert!(n.save_bounds_task.is_none()));

    cx.simulate_resize(size(px(380.), px(300.)));
    cx.update(|window, cx| note.update(cx, |n, cx| n.toggle_pin(window, cx)));
    let saved = sticky_windows::load(&store);
    assert!(saved[0].pinned);
    assert_eq!(saved[0].width, 380.);
    note.read_with(cx, |n, _| assert!(n.save_bounds_task.is_none()));

    cx.simulate_resize(size(px(400.), px(310.)));
    cx.update(|_, cx| note.update(cx, |n, cx| n.forget(cx)));
    let saved = sticky_windows::load(&store);
    assert!(!saved[0].open);
    assert_eq!(saved[0].width, 400.);
    note.read_with(cx, |n, _| assert!(n.save_bounds_task.is_none()));
}

#[gpui::test]
#[cfg(debug_assertions)]
fn pinned_note_palette_check_and_escape_dismiss_transients(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Sticky UX").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
    cx.simulate_resize(size(px(260.), px(240.)));

    // Pinning keeps sticky-pin state active and visible.
    cx.update(|window, cx| note.update(cx, |n, cx| n.toggle_pin(window, cx)));
    cx.update(|_, cx| assert!(note.read(cx).pinned));

    // StickyPalette shows check mark on the selected swatch and closes on Escape from note or palette.
    let palette = cx.update(|window, cx| {
        note.update(cx, |n, cx| {
            n.color = Some(10);
            n.toggle_palette(window, cx);
            n.palette.unwrap()
        })
    });
    let mut palette_cx = gpui::VisualTestContext::from_window(palette.into(), cx);
    let check_bounds = palette_cx.debug_bounds("sticky-palette-check").expect("selected swatch renders check icon");
    let selected_bounds = palette_cx.debug_bounds("sticky-palette-color-10").unwrap();
    assert!(selected_bounds.contains(&check_bounds.center()));

    // Escape dispatched on StickyNote (either at root focus or propagated from TaskDetail)
    // dismisses an open palette cleanly.
    cx.update(|window, cx| {
        let focus = note.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(note.read(cx).palette.is_none()));

    let title = cx.debug_bounds("task-detail-title").unwrap().center();
    cx.simulate_click(title, Modifiers::default());
    cx.update(|window, cx| {
        note.update(cx, |n, cx| n.toggle_palette(window, cx));
        assert!(note.read(cx).palette.is_some());
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(note.read(cx).palette.is_none()));

    // Reopening palette and pressing Escape inside the palette also clears note.palette.
    let palette = cx.update(|window, cx| {
        note.update(cx, |n, cx| {
            n.toggle_palette(window, cx);
            n.palette.unwrap()
        })
    });
    let mut palette_cx = gpui::VisualTestContext::from_window(palette.into(), cx);
    palette_cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(note.read(cx).palette.is_none()));
}
