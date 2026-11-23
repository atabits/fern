use chrono::Local;
use fern_core::{ChecklistItem, Id, Kind};
use gpui::MouseButton;
use ui::prelude::*;

use crate::i18n::{self, Strings};
use crate::task_rows;
use crate::text_input::{Changed, Mode, TextInput};

use super::{ChecklistRow, FocusTarget, OpenTask, StickyPaletteTokens, TaskDetail, sticky_checkbox};

impl TaskDetail {
    pub(super) fn sync_subtasks(&mut self) {
        self.session.subtasks = match self.task_id() {
            Some(id) => self.session.store.subtasks(id).unwrap_or_default(),
            None => Vec::new(),
        };
        let parent_id = self.session.task.as_ref().and_then(|task| task.parent_id);
        self.session.parent = parent_id.and_then(|id| self.session.store.task(id).ok().flatten());
        self.session.task_depth = self
            .task_id()
            .and_then(|id| self.session.store.task_depth(id).ok())
            .unwrap_or(0);
    }

    fn task_depth(&self) -> usize {
        self.session.task_depth
    }

    pub(super) fn can_add_subtask(&self) -> bool {
        !self.is_read_only()
            && self
                .session
                .task
                .as_ref()
                .is_some_and(|task| task.kind != Kind::Note && !task.is_deleted() && self.task_depth() < fern_core::MAX_SUBTASK_DEPTH)
    }

    pub(super) fn can_link_parent(&self) -> bool {
        !self.is_read_only()
            && self
                .session
                .task
                .as_ref()
                .is_some_and(|task| task.kind != Kind::Note && !task.is_deleted())
    }

    pub(super) fn reload_current_task(&mut self) {
        if let Some(id) = self.task_id()
            && let Ok(Some(fresh)) = self.session.store.task(id)
        {
            self.session.task = Some(fresh);
        }
        self.sync_subtasks();
    }

    pub(super) fn unlink_current_parent(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.save(cx, |store, id| store.unlink_parent(id));
        self.reload_current_task();
        cx.notify();
    }

    pub(super) fn link_current_parent(&mut self, parent: Id, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.save(cx, |store, id| store.link_parent(id, parent));
        self.reload_current_task();
        cx.notify();
    }

    fn unlink_subtask(&mut self, subtask_id: Id, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.save(cx, |store, _| store.unlink_parent(subtask_id));
        self.sync_subtasks();
        cx.notify();
    }

    fn toggle_subtask(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.save(cx, |store, _| store.set_completed(id, completed));
        if completed {
            desktop_common::sound::play_complete(&self.session.store);
        }
        self.reload_current_task();
        cx.notify();
    }

    /// Reloads checklist items, keeping the inputs of items that still exist so that the caret
    /// does not jump while the user types.
    pub(super) fn sync_checklist(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.task_id() else {
            self.checklist.clear();
            return;
        };
        let items = self.session.draft.as_ref().map_or_else(
            || self.session.store.checklist(task).unwrap_or_default(),
            |draft| draft.checklist.clone(),
        );
        let sticky_tokens = self.sticky_tokens();
        let mut old: Vec<ChecklistRow> = std::mem::take(&mut self.checklist);
        for item in items {
            let row = match old.iter().position(|row| row.item.id == item.id) {
                Some(index) => {
                    let mut row = old.swap_remove(index);
                    if row.item.title != item.title && row.input.read(cx).text() != item.title {
                        row.input.update(cx, |input, cx| input.set_text(item.title.clone(), cx));
                    }
                    if row.item.completed != item.completed
                        && let Some(t) = sticky_tokens
                    {
                        let ink = if item.completed { t.muted } else { t.ink };
                        row.input.update(cx, |input, cx| input.set_ink_override(Some(ink), Some(t.muted), Some(t.ink), cx));
                    }
                    row.item = item;
                    row
                }
                None => self.checklist_row(item, cx),
            };
            self.checklist.push(row);
        }
        cx.notify();
    }

    fn checklist_row(&mut self, item: ChecklistItem, cx: &mut Context<Self>) -> ChecklistRow {
        let id = item.id;
        let title = item.title.clone();
        let completed = item.completed;
        let read_only = self.is_read_only();
        let sticky_tokens = self.sticky_tokens();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::Wrapped, "", cx).without_cancel();
            input.set_text(title, cx);
            input.set_read_only(read_only, cx);
            if let Some(t) = sticky_tokens {
                let ink = if completed { t.muted } else { t.ink };
                input.set_ink_override(Some(ink), Some(t.muted), Some(t.ink), cx);
            }
            input
        });
        let subscriptions = [
            cx.subscribe(&input, move |this, input, _: &Changed, cx| {
                if this.is_read_only() {
                    return;
                }
                let text = input.read(cx).text().to_owned();
                if this.edit_draft(cx, |_, draft| {
                    if let Some(item) = draft.checklist.iter_mut().find(|item| item.id == id) {
                        item.title = text.clone();
                    }
                }) {
                    return;
                }
                this.save(cx, |store, _| store.set_checklist_item_title(id, &text));
            }),
            self.editor_window.checklist(&input, id, cx),
        ];
        ChecklistRow {
            item,
            input,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn toggle_checklist_item(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        if !self.edit_draft(cx, |_, draft| {
            if let Some(item) = draft.checklist.iter_mut().find(|item| item.id == id) {
                item.completed = completed;
            }
        }) {
            self.save(cx, |store, _| store.set_checklist_item_completed(id, completed));
        }
        if completed {
            desktop_common::sound::play_complete(&self.session.store);
        }
        self.sync_checklist(cx);
    }

    fn delete_checklist_item(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        if !self.edit_draft(cx, |_, draft| draft.checklist.retain(|item| item.id != id)) {
            self.save(cx, |store, _| store.delete_checklist_item(id));
        }
        self.sync_checklist(cx);
    }

    fn mutate_completed_checklist(
        &mut self,
        cx: &mut Context<Self>,
        edit_draft: impl FnOnce(&mut Vec<ChecklistItem>),
        apply_store: impl FnOnce(&fern_core::Store, Vec<Id>) -> anyhow::Result<()>,
    ) {
        if self.is_read_only() {
            return;
        }
        let done_ids: Vec<Id> = self.checklist.iter().filter(|r| r.item.completed).map(|r| r.item.id).collect();
        if done_ids.is_empty() {
            return;
        }
        if !self.edit_draft(cx, |_, draft| edit_draft(&mut draft.checklist)) {
            self.save(cx, move |store, _| apply_store(store, done_ids));
        }
        self.sync_checklist(cx);
    }

    fn uncheck_all_checklist_items(&mut self, cx: &mut Context<Self>) {
        self.mutate_completed_checklist(
            cx,
            |items| {
                for item in items {
                    item.completed = false;
                }
            },
            |store, done_ids| {
                for id in done_ids {
                    store.set_checklist_item_completed(id, false)?;
                }
                Ok(())
            },
        );
    }

    fn clear_completed_checklist_items(&mut self, cx: &mut Context<Self>) {
        self.mutate_completed_checklist(
            cx,
            |items| items.retain(|item| !item.completed),
            |store, done_ids| {
                for id in done_ids {
                    store.delete_checklist_item(id)?;
                }
                Ok(())
            },
        );
    }

    pub(super) fn start_checklist(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.show_checklist = true;
        self.editor_window.pending_focus = Some(FocusTarget::NewItem);
        cx.notify();
    }

    fn render_progress_bar(
        done: usize,
        total: usize,
        theme: &theme::Theme,
        sticky: Option<StickyPaletteTokens>,
    ) -> AnyElement {
        let ratio = if total == 0 { 0.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) };
        h_flex()
            .gap_2()
            .pb_1()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .h(px(3.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(sticky.map_or(theme.colors().border_variant, |t| t.border))
                    .child(div().h_full().w(gpui::relative(ratio)).rounded_full().bg(sticky.map_or(theme.colors().accent, |t| t.accent))),
            )
            .child(
                Label::new(format!("{done}/{total}"))
                    .size(LabelSize::XSmall)
                    .color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted))),
            )
            .into_any_element()
    }

    pub(super) fn render_checklist(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();
        let read_only = self.is_read_only();
        let total = self.checklist.len();
        let done_count = self.checklist.iter().filter(|r| r.item.completed).count();
        let muted_text = sticky.map_or(theme.colors().text_muted, |t| t.muted);
        let delete_hover = sticky.map_or(theme.status().error, |t| t.error);
        v_flex()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(total > 0, |el| {
                el.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(div().flex_1().min_w_0().child(Self::render_progress_bar(done_count, total, &theme, sticky)))
                        .when(done_count > 0 && !read_only, |bar| {
                            bar.child(
                                IconButton::new("checklist-uncheck-all", IconName::RotateCcw)
                                    .size(px(22.))
                                    .icon_size(IconSize::XSmall)
                                    .icon_color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted)))
                                    .when_some(sticky, |b, t| b.hover_color(t.surface))
                                    .tooltip(Tooltip::text(strings.checklist_uncheck_all))
                                    .on_click(cx.listener(|this, _, _, cx| this.uncheck_all_checklist_items(cx))),
                            )
                            .child(
                                IconButton::new("checklist-clear-completed", IconName::BrushCleaning)
                                    .size(px(22.))
                                    .icon_size(IconSize::XSmall)
                                    .icon_color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted)))
                                    .when_some(sticky, |b, t| b.hover_color(t.surface))
                                    .tooltip(Tooltip::text(strings.checklist_clear_completed))
                                    .on_click(cx.listener(|this, _, _, cx| this.clear_completed_checklist_items(cx))),
                            )
                        }),
                )
            })
            .children(self.checklist.iter().map(|row| {
                let id = row.item.id;
                let done = row.item.completed;
                let row_group = SharedString::from(format!("checklist-item-{id}"));
                let checkbox_id = SharedString::from(format!("item-checkbox-{id}"));
                let checkbox = if let Some(t) = sticky {
                    sticky_checkbox(checkbox_id, done, false, false, None, t)
                        .when(!read_only, |cb| {
                            cb.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_checklist_item(id, !done, cx)
                            }))
                        })
                        .into_any_element()
                } else {
                    Checkbox::new(checkbox_id)
                        .size(px(14.))
                        .toggle_state(done)
                        .when(!read_only, |cb| {
                            cb.on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_checklist_item(id, !done, cx)
                            }))
                        })
                        .into_any_element()
                };
                h_flex()
                    .id(SharedString::from(format!("item-{id}")))
                    .debug_selector(move || format!("checklist-item-{id}"))
                    .group(row_group.clone())
                    .min_h(px(30.))
                    .items_start()
                    .gap_2()
                    .py_1()
                    .child(div().mt(px(3.)).child(checkbox))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(done, |el| el.text_color(muted_text).line_through())
                            .child(row.input.clone()),
                    )
                    .when(!read_only, |el| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("item-delete-{id}")))
                                .flex_none()
                                .px_1()
                                .text_color(muted_text)
                                .opacity(if crate::touch::active() { 1. } else { 0. })
                                .group_hover(row_group, |s| s.opacity(1.))
                                .hover(move |s| s.text_color(delete_hover))
                                .cursor_pointer()
                                .tooltip(Tooltip::text(strings.delete))
                                .child("✕")
                                .on_click(cx.listener(move |this, _, _, cx| this.delete_checklist_item(id, cx))),
                        )
                    })
            }))
            .when(!read_only, |el| {
                el.child(
                    h_flex()
                        .min_h(px(30.))
                        .gap_2()
                        .child(Icon::new(IconName::Plus).size(IconSize::Small).when_some(sticky, |i, t| i.color(t.muted)))
                        .child(div().flex_1().child(self.new_item.clone())),
                )
            })
    }

    pub(super) fn render_subtasks(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();
        let can_add = self.can_add_subtask();
        let can_edit = !self.is_read_only();
        let total = self.session.subtasks.len();
        let done_count = self.session.subtasks.iter().filter(|s| s.is_completed()).count();
        let today = Local::now().date_naive();
        let countdown_dates = self.session.preferences.countdown_dates;
        v_flex()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(total > 0, |el| el.child(Self::render_progress_bar(done_count, total, &theme, sticky)))
            .children(self.session.subtasks.iter().map(|subtask| {
                let id = subtask.id;
                let done = subtask.is_completed();
                let group_name = SharedString::from(format!("subtask-row-{id}"));
                let due = subtask
                    .due_date
                    .map(|date| task_rows::row_due_label(date, subtask.due_time, today, strings, countdown_dates));
                let checkbox_id = SharedString::from(format!("subtask-checkbox-{id}"));
                let checkbox = if let Some(t) = sticky {
                    sticky_checkbox(
                        checkbox_id,
                        done,
                        subtask.is_wont_do(),
                        subtask.is_in_progress(),
                        t.priority_color(subtask.priority),
                        t,
                    )
                    .when(can_edit, |cb| {
                        cb.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_subtask(id, !done, cx);
                        }))
                    })
                    .into_any_element()
                } else {
                    Checkbox::new(checkbox_id)
                        .size(px(14.))
                        .toggle_state(done)
                        .in_progress(subtask.is_in_progress())
                        .dismissed(subtask.is_wont_do())
                        .tint(task_rows::priority_color(subtask.priority, &theme))
                        .when(can_edit, |cb| {
                            cb.on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_subtask(id, !done, cx);
                            }))
                        })
                        .into_any_element()
                };
                div().group(group_name.clone()).child(
                    ListItem::new(SharedString::from(format!("subtask-{id}")))
                        .height(px(30.))
                        .when_some(sticky, |item, t| item.hover_color(t.surface))
                        .start_slot(checkbox)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(done, |el| el.line_through())
                                .child(
                                    Label::new(if subtask.title.is_empty() {
                                        SharedString::new_static(strings.untitled)
                                    } else {
                                        subtask.title.clone().into()
                                    })
                                    .color(if done {
                                        sticky.map_or(Color::Muted, |t| Color::Custom(t.muted))
                                    } else {
                                        sticky.map_or(Color::Default, |t| Color::Custom(t.ink))
                                    })
                                    .truncate(),
                                ),
                        )
                        .end_slot(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .when_some(due, |el, (label, overdue)| {
                                    el.child(
                                        Label::new(label)
                                            .size(LabelSize::XSmall)
                                            .color(if overdue && !done {
                                                sticky.map_or(Color::Error, |t| Color::Custom(t.error))
                                            } else {
                                                sticky.map_or(Color::Muted, |t| Color::Custom(t.muted))
                                            }),
                                    )
                                })
                                .when(can_edit, |el| {
                                    el.child(
                                        div()
                                            .debug_selector(move || format!("subtask-unlink-{id}"))
                                            .opacity(if crate::touch::active() { 1. } else { 0. })
                                            .group_hover(group_name.clone(), |s| s.opacity(1.))
                                            .child(
                                                IconButton::new(SharedString::from(format!("subtask-unlink-btn-{id}")), IconName::X)
                                                    .size(px(22.))
                                                    .icon_size(IconSize::XSmall)
                                                    .icon_color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted)))
                                                    .when_some(sticky, |b, t| b.hover_color(t.surface))
                                                    .tooltip(Tooltip::text(strings.unlink_parent))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.unlink_subtask(id, cx);
                                                    })),
                                            ),
                                    )
                                })
                                .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).when_some(sticky, |i, t| i.color(t.muted))),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenTask(id)))),
                )
            }))
            .when(can_add, |el| {
                el.child(
                    h_flex()
                        .min_h(px(30.))
                        .px_2()
                        .gap_2()
                        .child(Icon::new(IconName::Plus).size(IconSize::Small).when_some(sticky, |i, t| i.color(t.muted)))
                        .child(div().flex_1().child(self.subtask_input.clone())),
                )
            })
    }
}
