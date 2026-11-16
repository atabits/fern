use super::*;
use gpui::{TestAppContext, VisualTestContext, size};

// Hosts the production popup builder and bindings without native activation or
// automatic dismissal, so tests can deliberately alternate the two render orders.
struct PopupHost {
    detail: Entity<TaskDetail>,
    focus: FocusHandle,
    _inputs: PopupWindow,
}

impl Render for PopupHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.detail.update(cx, |detail, cx| detail.render_sticky_popup(window.viewport_size(), cx));
        div().track_focus(&self.focus).child(content)
    }
}

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
}

fn editor(store: Rc<Store>, task: Task, cx: &mut TestAppContext) -> (Entity<TaskDetail>, VisualTestContext) {
    let (detail, owner) = cx.add_window_view(|window, cx| {
        let mut detail = TaskDetail::new(store, window, cx);
        detail.set_sticky_style(Default::default(), cx);
        detail.show(Some(task), cx);
        detail
    });
    owner.simulate_resize(size(px(480.), px(900.)));
    let window = owner.update(|window, _| window.window_handle());
    (detail, VisualTestContext::from_window(window, cx))
}

fn popup(detail: &Entity<TaskDetail>, cx: &mut TestAppContext) -> (Entity<PopupHost>, VisualTestContext) {
    let (popup, window_cx) = cx.add_window_view(|window, cx| {
        let inputs = detail.update(cx, |detail, cx| detail.bind_popup_window(window, cx));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.observe(detail, |_, _, cx| cx.notify()).detach();
        PopupHost { detail: detail.clone(), focus, _inputs: inputs }
    });
    let window = window_cx.update(|window, _| window.window_handle());
    (popup, VisualTestContext::from_window(window, cx))
}

fn draw_last(detail: &Entity<TaskDetail>, cx: &mut VisualTestContext) {
    cx.run_until_parked();
    let handle = cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
        window.window_handle()
    });
    cx.cx.update(|cx| {
        assert_eq!(cx.with_window(detail.entity_id(), |window, _| window.window_handle()), Some(handle),
            "the test must put GPUI's implicit session window on the other surface");
    });
}

#[gpui::test]
fn editor_submissions_keep_the_owner_after_popup_render(cx: &mut TestAppContext) {
    init(cx);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Owner").unwrap();
    let (detail, mut owner) = editor(store.clone(), task.clone(), cx);
    let (popup, mut popup_cx) = popup(&detail, cx);

    owner.update(|window, cx| window.focus(&detail.read(cx).title.focus_handle(cx), cx));
    draw_last(&detail, &mut popup_cx);
    owner.simulate_keystrokes("enter");
    owner.update(|window, cx| assert!(detail.read(cx).notes.focus_handle(cx).is_focused(window)));
    popup_cx.update(|window, cx| assert!(popup.read(cx).focus.is_focused(window)));

    owner.update(|window, cx| detail.update(cx, |detail, cx| {
        detail.show_checklist = true;
        detail.new_item.update(cx, |input, cx| input.set_text("First item", cx));
        window.focus(&detail.new_item.focus_handle(cx), cx);
        cx.notify();
    }));
    draw_last(&detail, &mut popup_cx);
    owner.simulate_keystrokes("enter");
    assert_eq!(store.checklist(task.id).unwrap().len(), 1);
    owner.update(|window, cx| {
        let detail = detail.read(cx);
        assert!(detail.new_item.read(cx).text().is_empty());
        assert!(detail.new_item.focus_handle(cx).is_focused(window));
        window.focus(&detail.checklist[0].input.focus_handle(cx), cx);
    });
    draw_last(&detail, &mut popup_cx);
    owner.simulate_keystrokes("enter");
    assert_eq!(store.checklist(task.id).unwrap().len(), 2);
    owner.update(|window, cx| assert!(detail.read(cx).checklist[1].input.focus_handle(cx).is_focused(window)));

    owner.update(|window, cx| detail.update(cx, |detail, cx| {
        detail.subtask_open = true;
        detail.subtask_input.update(cx, |input, cx| input.set_text("Child", cx));
        window.focus(&detail.subtask_input.focus_handle(cx), cx);
        cx.notify();
    }));
    draw_last(&detail, &mut popup_cx);
    owner.simulate_keystrokes("enter");
    assert_eq!(store.subtasks(task.id).unwrap().len(), 1);
    owner.update(|window, cx| {
        let detail = detail.read(cx);
        assert!(detail.subtask_input.read(cx).text().is_empty());
        assert!(detail.subtask_input.focus_handle(cx).is_focused(window));
    });

    owner.update(|window, cx| detail.update(cx, |detail, cx| {
        detail.comment_open = true;
        detail.comment_input.update(cx, |input, cx| input.set_text("Comment", cx));
        window.focus(&detail.comment_input.focus_handle(cx), cx);
        cx.notify();
    }));
    draw_last(&detail, &mut popup_cx);
    owner.simulate_keystrokes("enter");
    assert_eq!(store.comments(task.id).unwrap().len(), 1);
    owner.update(|window, cx| {
        let detail = detail.read(cx);
        assert!(detail.comment_input.read(cx).text().is_empty());
        assert!(detail.comment_input.focus_handle(cx).is_focused(window));
    });
    draw_last(&detail, &mut popup_cx);
    owner.simulate_keystrokes("escape");
    owner.update(|window, cx| {
        assert!(!detail.read(cx).comment_open);
        assert!(detail.read(cx).title.focus_handle(cx).is_focused(window));
    });
    popup_cx.update(|window, cx| assert!(popup.read(cx).focus.is_focused(window)));
}

#[gpui::test]
fn popup_submissions_keep_their_window_after_owner_render_and_replacement(cx: &mut TestAppContext) {
    init(cx);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Popup").unwrap();
    let (detail, mut owner) = editor(store, task, cx);
    let (old_popup, mut old_popup_cx) = popup(&detail, cx);
    owner.update(|window, cx| window.focus(&detail.read(cx).title.focus_handle(cx), cx));
    old_popup_cx.update(|window, cx| detail.update(cx, |detail, cx| {
        detail.open_tags(Popup::Tags, window, cx);
        detail.tag_input.update(cx, |input, cx| input.set_text("first", cx));
    }));
    draw_last(&detail, &mut owner);
    old_popup_cx.simulate_keystrokes("enter");
    old_popup_cx.update(|window, cx| {
        let detail = detail.read(cx);
        assert_eq!(detail.session.tag_draft.as_ref().unwrap().picked, ["first"]);
        assert!(detail.tag_input.read(cx).text().is_empty());
        assert!(detail.tag_input.focus_handle(cx).is_focused(window));
    });
    owner.update(|window, cx| assert!(detail.read(cx).title.focus_handle(cx).is_focused(window)));

    // Keep the old view alive after closing its window. Even retained bindings
    // must not run actions against the replacement or the editor window.
    old_popup_cx.update(|window, _| window.remove_window());
    let old_weak = old_popup.downgrade();
    let (replacement, mut popup_cx) = popup(&detail, cx);
    popup_cx.update(|window, cx| detail.update(cx, |detail, cx| {
        detail.open_popup(Popup::Due, cx);
        detail.session.due_draft.as_mut().unwrap().menu = Some(DueMenu::Time);
        detail.time_input.update(cx, |input, cx| input.set_text("14:35", cx));
        window.focus(&detail.time_input.focus_handle(cx), cx);
    }));
    draw_last(&detail, &mut owner);
    popup_cx.simulate_keystrokes("enter");
    popup_cx.update(|window, cx| {
        let detail = detail.read(cx);
        assert_eq!(detail.session.due_draft.as_ref().unwrap().time, chrono::NaiveTime::from_hms_opt(14, 35, 0));
        assert!(detail.time_input.read(cx).text().is_empty());
        assert!(detail.time_input.focus_handle(cx).is_focused(window));
    });
    owner.update(|window, cx| assert!(detail.read(cx).title.focus_handle(cx).is_focused(window)));
    drop(old_popup);
    cx.update(|_| {});
    cx.run_until_parked();
    assert!(old_weak.upgrade().is_none(), "closed popup bindings must not retain their host");
    let replacement_weak = replacement.downgrade();
    popup_cx.update(|window, _| window.remove_window());
    drop(replacement);
    cx.update(|_| {});
    cx.run_until_parked();
    assert!(replacement_weak.upgrade().is_none());
}

#[gpui::test]
fn closed_editor_cannot_reroute_submissions_to_its_popup_or_another_note(cx: &mut TestAppContext) {
    init(cx);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let first_task = store.add_task("First").unwrap();
    let second_task = store.add_task("Second").unwrap();
    let (first, mut first_cx) = editor(store.clone(), first_task.clone(), cx);
    let (popup, mut popup_cx) = popup(&first, cx);
    let (second, mut second_cx) = editor(store.clone(), second_task.clone(), cx);
    first_cx.update(|window, _| window.remove_window());
    draw_last(&first, &mut popup_cx);
    cx.update(|cx| {
        let input = first.read(cx).new_item.clone();
        input.update(cx, |_, cx| cx.emit(Submitted("Must not be saved".into())));
        let title = first.read(cx).title.clone();
        title.update(cx, |_, cx| cx.emit(Submitted("First".into())));
    });
    assert!(store.checklist(first_task.id).unwrap().is_empty());
    popup_cx.update(|window, cx| assert!(popup.read(cx).focus.is_focused(window)));
    second_cx.update(|window, cx| second.update(cx, |detail, cx| {
        detail.show_checklist = true;
        detail.new_item.update(cx, |input, cx| input.set_text("Only second", cx));
        window.focus(&detail.new_item.focus_handle(cx), cx);
        cx.notify();
    }));
    draw_last(&first, &mut popup_cx);
    second_cx.simulate_keystrokes("enter");
    assert!(store.checklist(first_task.id).unwrap().is_empty());
    assert_eq!(store.checklist(second_task.id).unwrap().len(), 1);

    let weak = first.downgrade();
    popup_cx.update(|window, _| window.remove_window());
    drop(popup);
    drop(first);
    cx.update(|_| {});
    cx.run_until_parked();
    assert!(weak.upgrade().is_none(), "bindings must not form a cycle with the editing session");
}
