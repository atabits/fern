//! Window-local input bindings. GPUI's `subscribe_in` follows the subscriber's
//! last rendered window, which is unsuitable for a session read by two surfaces.
//! These subscriptions keep an explicit window handle and a weak session handle.

use super::*;
use gpui::{AnyWindowHandle, WeakEntity};

#[cfg(all(test, debug_assertions))]
mod tests;

fn subscribe<E: 'static>(
    window: AnyWindowHandle,
    detail: WeakEntity<TaskDetail>,
    input: &Entity<TextInput>,
    mut on_event: impl FnMut(&mut TaskDetail, &Entity<TextInput>, &E, &mut Window, &mut Context<TaskDetail>) + 'static,
    cx: &mut App,
) -> Subscription
where
    TextInput: EventEmitter<E>,
{
    cx.subscribe(input, move |input, event, cx| {
        // A closed owner deliberately has no fallback window. Neither a popup
        // redraw nor another note can redirect this field's keyboard action.
        window.update(cx, |_, window, cx| {
            detail.update(cx, |detail, cx| on_event(detail, &input, event, window, cx)).ok();
        }).ok();
    })
}

pub(super) struct EditorWindow {
    window: AnyWindowHandle,
    pub pending_focus: Option<FocusTarget>,
    pub popup_return_focus: Option<WeakFocusHandle>,
    pub inline_popup: Option<PopupWindow>,
    _subscriptions: Vec<Subscription>,
}

impl EditorWindow {
    pub fn take_pending_focus(&mut self, window: &Window) -> Option<FocusTarget> {
        if window.window_handle() == self.window { self.pending_focus.take() } else { None }
    }

    pub fn new(
        title: &Entity<TextInput>,
        new_item: &Entity<TextInput>,
        subtask: &Entity<TextInput>,
        comment: &Entity<TextInput>,
        window: &Window,
        cx: &mut Context<TaskDetail>,
    ) -> Self {
        let window = window.window_handle();
        let detail = cx.weak_entity();
        let subscriptions = vec![
            subscribe(window, detail.clone(), title, |this, _, _: &Submitted, window, cx| {
                if this.is_read_only() {
                    return;
                }
                if this.session.draft.is_some() {
                    match this.session.popup {
                        Some(Popup::Due) => this.apply_due(window, cx),
                        None => { this.finish_draft(cx); }
                        Some(_) => {}
                    }
                } else {
                    window.focus(&this.notes.focus_handle(cx), cx);
                }
            }, cx),
            subscribe(window, detail.clone(), new_item, |this, input, Submitted(text), window, cx| {
                if this.is_read_only() {
                    return;
                }
                if !text.trim().is_empty() && this.add_checklist_item(text, None, cx).is_some() {
                    input.update(cx, |input, cx| input.clear(window, cx));
                    this.sync_checklist(cx);
                }
            }, cx),
            subscribe(window, detail.clone(), subtask, |this, input, Submitted(text), window, cx| {
                if this.is_read_only() {
                    return;
                }
                if !text.trim().is_empty() {
                    this.save(cx, |store, id| store.add_subtask(id, text).map(|_| ()));
                    input.update(cx, |input, cx| input.clear(window, cx));
                    this.sync_subtasks();
                }
            }, cx),
            subscribe(window, detail.clone(), comment, |this, input, Submitted(text), window, cx| {
                if this.is_read_only() {
                    return;
                }
                if let Some(task) = this.task_id() && !text.trim().is_empty() {
                    match this.session.store.add_comment(task, text) {
                        Ok(_) => {
                            input.update(cx, |input, cx| input.clear(window, cx));
                            cx.emit(TaskUpdated);
                        }
                        Err(err) => eprintln!("failed to add the comment: {err:#}"),
                    }
                    this.sync_comments();
                    cx.notify();
                }
            }, cx),
            subscribe(window, detail, comment, |this, _, _: &Cancelled, window, cx| {
                if this.comment_emoji {
                    this.comment_emoji = false;
                } else {
                    this.comment_open = false;
                    window.focus(&this.title.focus_handle(cx), cx);
                }
                cx.notify();
            }, cx),
        ];
        Self { window, pending_focus: None, popup_return_focus: None, inline_popup: None, _subscriptions: subscriptions }
    }

    pub fn checklist(&self, input: &Entity<TextInput>, id: Id, cx: &mut Context<TaskDetail>) -> Subscription {
        subscribe(self.window, cx.weak_entity(), input, move |this, _, _: &Submitted, _, cx| {
            if this.is_read_only() {
                return;
            }
            if let Some(created) = this.add_checklist_item("", Some(id), cx) {
                this.editor_window.pending_focus = Some(FocusTarget::Item(created.id));
                this.sync_checklist(cx);
            }
        }, cx)
    }
}

/// The same picker inputs can be hosted inline or by a separate native window.
/// Exactly one host owns these bindings; moving the surface retains the inputs
/// and drafts, while dropping its bindings disconnects the old window.
pub struct PopupWindow {
    _subscriptions: Vec<Subscription>,
}

impl PopupWindow {
    pub(super) fn new(
        detail: WeakEntity<TaskDetail>,
        tags: &Entity<TextInput>,
        time: &Entity<TextInput>,
        window: &Window,
        cx: &mut App,
    ) -> Self {
        let window = window.window_handle();
        Self { _subscriptions: vec![
            subscribe(window, detail.clone(), tags, |this, _, _: &Submitted, window, cx| {
                this.pick_typed_tag(window, cx);
            }, cx),
            subscribe(window, detail, time, |this, _, Submitted(text), window, cx| {
                this.pick_typed_time(text, window, cx);
            }, cx),
        ] }
    }
}
