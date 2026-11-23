use super::*;
use crate::list_dialog::DialogEvent;
use crate::tag_dialog::TagEvent;

impl TodoView {
    /// A kanban card: the task with its tags and date, and its subtasks under it.
    /// Opens a board card's details beside it, as the original's board does.
    pub(super) fn open_card(&mut self, id: Id, cx: &mut Context<Self>) {
        let Ok(Some(task)) = self.store.task(id) else { return };
        let lists = self.lists.clone();
        self.card_detail.update(cx, |detail, cx| {
            detail.set_lists(lists);
            detail.show(Some(task), cx);
        });
        self.card = Some(id);
        cx.notify();
    }

    pub(super) fn close_card(&mut self, cx: &mut Context<Self>) {
        if self.card.take().is_some() {
            self.card_detail.update(cx, |detail, cx| detail.show(None, cx));
            cx.notify();
        }
    }

    pub(super) fn dismiss_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.card_detail.update(cx, |detail, cx| detail.dismiss_popup(window, cx)) {
            self.close_card(cx);
            window.focus(&self.list_focus, cx);
        }
    }

    /// The card's task changed: the board shows it, and the card closes when the task is gone.
    pub(super) fn card_updated(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        let Some(id) = self.card else { return };
        match self.store.task(id) {
            Ok(Some(task)) if !task.is_deleted() => self.card_detail.update(cx, |detail, cx| detail.refresh(Some(task), cx)),
            _ => self.close_card(cx),
        }
    }

    /// The board's task card: the details, framed, closed by a click elsewhere or Escape.
    pub(super) fn render_card_details(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        div()
            .id("board-task-card")
            .occlude()
            .w(px(402.))
            .h(px(440.))
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(theme.colors().border)
            .bg(theme.colors().background)
            .shadow_lg()
            // The details' own popovers lie outside the card; a click in them keeps it open.
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if !this.card_detail.read(cx).has_popup() {
                    this.close_card(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DismissTransient, window, cx| {
                this.dismiss_card(window, cx);
            }))
            .child(self.card_detail.clone())
            .into_any_element()
    }

    /// A group header: its label and count; a list section also gets a "…" menu with Rename and
    /// Delete, and is replaced by a field while it is renamed.
    pub(super) fn render_group_header(
        &self,
        entry_ix: usize,
        label: String,
        count: usize,
        drop: Option<&DropTarget>,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        // Matched by name: the overdue group is the one header of a date view with this title.
        let overdue = drop.is_none() && label == strings.group_overdue;
        let section = match drop {
            Some(DropTarget::Section(Some(id))) => Some(*id),
            _ => None,
        };
        let renaming = section.and_then(|id| self.section_rename.as_ref().filter(|(s, _)| *s == id)).map(|(_, input)| input.clone());
        let group: SharedString = match section {
            Some(id) => format!("group-header-sec-{id}").into(),
            None => format!("group-header-{entry_ix}-{label}").into(),
        };
        h_flex()
            .id(group.clone())
            .group(group.clone())
            .h(px(height))
            .gap_2()
            .map(|el| match renaming {
                Some(input) => el.child(
                    h_flex().flex_1().min_w_0().h(px(28.)).px_2().rounded(px(6.)).bg(cx.theme().colors().surface_background).child(input),
                ),
                None => el
                    .child(Label::new(label).weight(FontWeight::BOLD).truncate())
                    .child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)),
            })
            .when_some(section.filter(|_| !self.is_current_view_archived_list()), |el, id| {
                el.child(div().flex_1()).child(
                    div()
                        .relative()
                        .opacity(if self.section_menu == Some(id) || crate::touch::active() { 1. } else { 0. })
                        .group_hover(group, |s| s.opacity(1.))
                        .child(
                            IconButton::new(SharedString::from(format!("section-more-{id}")), IconName::Ellipsis)
                                .size(px(22.))
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text(strings.more))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.section_menu = Some(id);
                                    cx.notify();
                                })),
                        )
                        .when(self.section_menu == Some(id), |el| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowRight,
                                popover(cx)
                                    .min_w(px(200.))
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.section_menu = None;
                                        cx.notify();
                                    }))
                                    .child(
                                        ListItem::new("rename-section")
                                            .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                                            .child(Label::new(strings.rename_section))
                                            .on_click(cx.listener(move |this, _, window, cx| this.start_rename_section(id, window, cx))),
                                    )
                                    .child(
                                        ListItem::new("delete-section")
                                            .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                            .child(Label::new(strings.delete_section).color(Color::Error))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.section_menu = None;
                                                let result = this.store.delete_list_section(id);
                                                this.check(result);
                                                this.reload(cx);
                                            })),
                                    ),
                            ))
                        }),
                )
            })
            // The overdue group's "Postpone" moves its tasks to a later day, as the original's.
            .when(overdue, |el| {
                el.child(div().flex_1()).child(
                    div()
                        .relative()
                        .child(
                            div()
                                .id("postpone")
                                .cursor_pointer()
                                .child(Label::new(strings.postpone).size(LabelSize::Small).color(Color::Accent))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.postpone_menu = true;
                                    cx.notify();
                                })),
                        )
                        .when(self.postpone_menu, |el| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowRight,
                                popover(cx)
                                    .min_w(px(180.))
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.postpone_menu = false;
                                        cx.notify();
                                    }))
                                    .children(fern_core::DefaultDate::ALL.iter().zip(strings.default_dates).map(|(&choice, name)| {
                                        ListItem::new(SharedString::from(format!("postpone-{name}")))
                                            .child(Label::new(name))
                                            .on_click(cx.listener(move |this, _, _, cx| this.postpone_overdue(Some(choice.date(Self::today())), cx)))
                                    }))
                                    .child(Divider::horizontal())
                                    .child(
                                        ListItem::new("postpone-clear")
                                            .start_slot(Icon::new(IconName::CalendarX).size(IconSize::XSmall).color(Color::Muted))
                                            .child(Label::new(strings.clear).color(Color::Muted))
                                            .on_click(cx.listener(|this, _, _, cx| this.postpone_overdue(None, cx))),
                                    ),
                            ))
                        }),
                )
            })
            .into_any_element()
    }

    /// Moves every overdue task in view to `day` (keeping its time), or clears their due dates when `None`.
    pub(super) fn postpone_overdue(&mut self, day: Option<NaiveDate>, cx: &mut Context<Self>) {
        self.postpone_menu = false;
        let today = Self::today();
        let overdue: Vec<_> = self
            .tasks
            .iter()
            .filter(|t| !t.is_closed() && t.due_date.is_some_and(|d| d < today))
            .map(|t| (t.id, t.due_time))
            .collect();
        for (id, time) in overdue {
            let result = self.store.set_due(id, day, day.and(time));
            self.check(result);
        }
        crate::data_version::data_changed(cx.entity_id(), cx);
        self.reload(cx);
    }

    pub(super) fn start_rename_section(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_section_rename(cx);
        self.section_menu = None;
        self.column_menu = None;
        let strings = i18n::t(cx);
        let name = self.sections.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.section_name, cx);
            input.set_text(name, cx);
            input
        });
        cx.subscribe(&input, move |this, _, Submitted(name), cx| {
            let result = this.store.rename_list_section(id, name);
            this.check(result);
            this.section_rename = None;
            this.reload(cx);
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.section_rename = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.section_rename = Some((id, input));
        cx.notify();
    }

    pub(super) fn start_new_section(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_current_view_archived_list() {
            return;
        }
        let _ = self.store.set_setting(&format!("kanban:hide_new_section:{}", self.view_key()), "0");
        self.hide_new_section = false;
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.section_name, cx));
        cx.subscribe(&input, |this, _, Submitted(name), cx| {
            if let Some(list) = this.section_owner() {
                let result = this.store.insert_list_section(list, name, this.new_section_index.take());
                this.check(result);
            }
            this.new_section = None;
            this.reload(cx);
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.new_section = None;
            this.new_section_index = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.new_section = Some(input);
        self.header_menu_open = false;
        cx.notify();
    }

    pub(super) fn commit_new_section(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.new_section.take() else { return };
        let index = self.new_section_index.take();
        if self.is_current_view_archived_list() {
            cx.notify();
            return;
        }
        let name = input.read(cx).text().trim().to_owned();
        if !name.is_empty()
            && let Some(list) = self.section_owner()
        {
            let result = self.store.insert_list_section(list, &name, index);
            self.check(result);
            self.reload(cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn commit_section_rename(&mut self, cx: &mut Context<Self>) {
        let Some((section, input)) = self.section_rename.take() else { return };
        let name = input.read(cx).text().trim().to_owned();
        if !name.is_empty() {
            let result = self.store.rename_list_section(section, &name);
            self.check(result);
            self.reload(cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn commit_column_add(&mut self, cx: &mut Context<Self>) {
        let Some((_, target, input)) = self.column_add.take() else { return };
        let text = input.read(cx).text().to_owned();
        if !text.trim().is_empty()
            && let Some(task) = self.add_task(&text, cx)
        {
            self.drop_on_column(task.id, target, cx);
            return;
        }
        cx.notify();
    }

    pub(super) fn open_sort_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sort_menu.is_none() {
            self.sort_menu_return_focus = window.focused(cx);
            window.focus(&self.sort_menu_focus, cx);
        }
        self.sort_menu = Some(None);
        cx.notify();
    }

    pub(super) fn close_sort_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sort_menu = None;
        let previous = self.sort_menu_return_focus.take();
        if self.sort_menu_focus.is_focused(window) {
            if let Some(previous) = previous {
                window.focus(&previous, cx);
            } else {
                window.blur(cx);
            }
        }
        cx.notify();
    }

    pub fn dismiss_transient(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.cancel_panel_drag(window, cx) || self.cancel_task_drag(window, cx) {
            return true;
        }
        if self.view == View::Summary
            && self.summary.update(cx, |summary, cx| summary.dismiss_transient(cx))
        {
            return true;
        }
        if self.view == View::Horizons
            && self.horizons.update(cx, |horizons, cx| horizons.dismiss_transient(window, cx))
        {
            return true;
        }
        if self.sort_menu.is_some() {
            self.close_sort_menu(window, cx);
            return true;
        }
        if self.add_tags.take().is_some() {
            if self.add_tag_input.focus_handle(cx).is_focused(window) {
                window.focus(&self.input.focus_handle(cx), cx);
            }
            cx.notify();
            return true;
        }

        if std::mem::take(&mut self.timeline_tz_menu) {
            if self.timeline_tz_input.focus_handle(cx).is_focused(window) {
                window.focus(&self.list_focus, cx);
            }
            cx.notify();
            return true;
        }

        // Close active confirmations and nested/modal dialogs one layer at a time before menus.
        if std::mem::take(&mut self.confirm_empty_trash)
            || self.confirm_delete.take().is_some()
            || self.batch_add_pending.take().is_some()
        {
            cx.notify();
            return true;
        }
        if self.folder_dialog.take().is_some() {
            cx.notify();
            return true;
        }
        if let Some(dialog) = &self.list_dialog
            && dialog.update(cx, |d, cx| d.dismiss_nested(cx))
        {
            return true;
        }
        if self.filter_edit.as_ref().is_some_and(|edit| edit.preview) {
            self.set_filter_preview(false, cx);
            return true;
        }
        if self.list_dialog.take().is_some()
            || self.tag_dialog.take().is_some()
            || self.merge_dialog.take().is_some()
            || self.filter_edit.take().is_some()
            || self.view_options.take().is_some()
        {
            cx.notify();
            return true;
        }

        // These menus don't move keyboard focus when opened from the list.
        let dismissed_menu = self.context_menu.take().is_some()
            || self.habit_menu.take().is_some()
            || self.list_menu.take().is_some()
            || self.closed_menu.take().is_some()
            || self.batch_menu.take().is_some()
            || self.column_menu.take().is_some()
            || self.section_menu.take().is_some()
            || self.suggestions_open.take().is_some()
            || std::mem::take(&mut self.header_menu_open)
            || std::mem::take(&mut self.postpone_menu)
            || std::mem::take(&mut self.scale_menu_open)
            || std::mem::take(&mut self.templates_open)
            || std::mem::take(&mut self.input_box_open)
            || std::mem::take(&mut self.add_menu_open)
            || self.add_date_month.take().is_some();
        if dismissed_menu {
            self.context_side = None;
            self.add_list_side = false;
            self.add_date_row = None;
            self.add_range = None;
            cx.notify();
            return true;
        }

        if self.title_edit.is_some() {
            self.finish_title_edit(false, cx);
            window.focus(&self.list_focus, cx);
            return true;
        }
        if let Some((_, _, input)) = self.column_add.take() {
            if input.focus_handle(cx).is_focused(window) {
                window.focus(&self.list_focus, cx);
            }
            cx.notify();
            return true;
        }
        if let Some(input) = self.new_section.take() {
            self.new_section_index = None;
            if input.focus_handle(cx).is_focused(window) {
                window.focus(&self.list_focus, cx);
            }
            cx.notify();
            return true;
        }
        if self.section_rename.take().is_some() {
            window.focus(&self.list_focus, cx);
            cx.notify();
            return true;
        }
        if self.card.is_some() {
            self.dismiss_card(window, cx);
            return true;
        }
        if !self.multi_selected.is_empty() {
            self.clear_multi_selection(cx);
            return true;
        }
        if self.detail.read(cx).has_popup()
            && self.detail.update(cx, |detail, cx| detail.dismiss_popup(window, cx))
        {
            return true;
        }
        if self.uses_popup_details() && (self.selected.is_some() || self.list_activity.is_some()) {
            self.selected = None;
            self.list_activity = None;
            window.focus(&self.list_focus, cx);
            cx.notify();
            return true;
        }
        false
    }

    pub(super) fn start_new_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_lists = true;
        self.open_list_dialog(None, None, window, cx);
    }

    /// Opens the original's Add List dialog (in a folder when given), or Edit List for `list`.
    pub(super) fn open_list_dialog(&mut self, list: Option<Id>, folder: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        self.list_menu = None;
        self.header_menu_open = false;
        let dialog = cx.new(|cx| ListDialog::new(self.store.clone(), list, folder, window, cx));
        cx.subscribe_in(&dialog, window, |this, _, event: &DialogEvent, window, cx| {
            this.list_dialog = None;
            window.focus(&this.list_focus, cx);
            if let DialogEvent::Saved(id) = event {
                crate::data_version::data_changed(cx.entity_id(), cx);
                if this.view != View::List(*id) {
                    this.set_view(View::List(*id), cx);
                    return;
                }
            }
            let was_board = this.uses_board_layout();
            this.reload(cx);
            if was_board != this.uses_board_layout() {
                this.sync_layout_selection(cx);
            }
        })
        .detach();
        self.list_dialog = Some(dialog);
        cx.notify();
    }

    /// The New Folder dialog, or Edit Folder for `folder`.
    pub(super) fn open_folder_dialog(&mut self, folder: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        self.list_menu = None;
        let dialog = cx.new(|cx| FolderDialog::new(self.store.clone(), folder, window, cx));
        cx.subscribe_in(&dialog, window, |this, _, event: &DialogEvent, window, cx| {
            this.folder_dialog = None;
            window.focus(&this.list_focus, cx);
            if matches!(event, DialogEvent::Saved(_)) {
                crate::data_version::data_changed(cx.entity_id(), cx);
            }
            this.reload(cx);
        })
        .detach();
        self.folder_dialog = Some(dialog);
        cx.notify();
    }

    /// The original's Add Tags (under `parent` for Add Sub-tag), or Edit Tag for `tag`.
    pub(super) fn open_tag_dialog(&mut self, tag: Option<String>, parent: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.list_menu = None;
        let editing = tag.clone();
        let dialog = cx.new(|cx| TagDialog::new(self.store.clone(), tag, parent, window, cx));
        cx.subscribe_in(&dialog, window, move |this, _, event: &TagEvent, window, cx| {
            if let TagEvent::Saved(name) = event
                && editing.as_ref().is_some_and(|old| this.view == View::Tag(old.clone()))
            {
                this.view = View::Tag(name.clone());
            }
            this.tag_dialog = None;
            window.focus(&this.list_focus, cx);
            this.reload(cx);
        })
        .detach();
        self.tag_dialog = Some(dialog);
        cx.notify();
    }

    pub(super) fn open_merge_dialog(&mut self, tag: String, window: &mut Window, cx: &mut Context<Self>) {
        self.list_menu = None;
        let from = tag.clone();
        let dialog = cx.new(|_| MergeDialog::new(self.store.clone(), tag));
        cx.subscribe_in(&dialog, window, move |this, _, event: &TagEvent, window, cx| {
            if let TagEvent::Saved(into) = event
                && this.view == View::Tag(from.clone())
            {
                this.view = View::Tag(into.clone());
            }
            this.merge_dialog = None;
            window.focus(&this.list_focus, cx);
            this.reload(cx);
        })
        .detach();
        self.merge_dialog = Some(dialog);
        cx.notify();
    }

    pub(super) fn open_context_menu(
        &mut self,
        id: Id,
        position: gpui::Point<Pixels>,
        select_task: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.list_focus, cx);
        if select_task {
            self.select(id, cx);
        }
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned().or_else(|| self.store.task(id).ok().flatten()) else {
            return;
        };
        // As in the original: archived-list tasks have no context menu; Trash has Restore / Delete Forever.
        if !task.is_deleted() && self.is_task_read_only(&task) {
            return;
        }
        let nesting = task_menu::TaskNesting::of(&self.store, id);
        self.context_menu = Some((task, position, nesting));
        self.context_side = None;
        cx.notify();
    }

    pub(super) fn render_empty_state(&self, strings: &'static Strings, _theme: &theme::Theme, _px_fallback: Pixels) -> AnyElement {
        let can_add = !self.is_read_only_view() && !matches!(self.view, View::Day(_) | View::Summary | View::Horizons);
        let input = self.input.clone();
        let with_cta = |state: EmptyState| -> EmptyState {
            if can_add && (!self.kanban || self.timeline) {
                state.action("empty-add-task", strings.add_task, move |_, window, cx| {
                    window.focus(&input.focus_handle(cx), cx);
                })
            } else {
                state
            }
        };
        match &self.view {
            View::Day(_) => EmptyState::new(IconName::CalendarCheck, strings.free_day_hint)
                .title(strings.free_day)
                .into_any_element(),
            View::Tomorrow => with_cta(
                EmptyState::new(IconName::Map, strings.plan_ahead)
                    .title(strings.no_tasks_tomorrow),
            )
            .into_any_element(),
            View::Completed => EmptyState::new(IconName::SquareCheck, strings.no_completed_tasks).into_any_element(),
            View::WontDo => EmptyState::new(IconName::SquareX, strings.no_wont_do).into_any_element(),
            View::Trash => EmptyState::new(IconName::Trash, strings.trash_is_empty).into_any_element(),
            View::Search(query) if query.trim().is_empty() => {
                EmptyState::new(IconName::Search, strings.search_hint).into_any_element()
            }
            View::Search(_) => EmptyState::new(IconName::Search, strings.nothing_found).into_any_element(),
            other => {
                let (icon, title) = match other {
                    View::Inbox => (IconName::Inbox, strings.no_tasks),
                    View::Today => (IconName::Sun, strings.no_tasks_today),
                    View::Next7Days => (IconName::CalendarDays, strings.no_tasks),
                    View::Tag(_) => (IconName::Tag, strings.no_tasks),
                    View::Filter(_) => (IconName::Layers, strings.no_tasks),
                    _ => (IconName::ListChecks, strings.no_tasks),
                };
                if can_add {
                    with_cta(EmptyState::new(icon, strings.empty_tasks_hint).title(title)).into_any_element()
                } else {
                    EmptyState::new(icon, title).into_any_element()
                }
            }
        }
    }

    pub(super) fn restore(&mut self, id: Id, cx: &mut Context<Self>) {
        let result = self.store.restore_task(id);
        self.check(result);
        self.reload(cx);
    }

    pub(super) fn request_empty_trash(&mut self, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        if self.tasks.is_empty() {
            return;
        }
        self.confirm_empty_trash = true;
        cx.notify();
    }

    pub(super) fn empty_trash(&mut self, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        self.confirm_empty_trash = false;
        let result = self.store.empty_trash();
        self.check(result);
        self.reload(cx);
    }

    /// Asks for confirmation before permanently deleting a single trashed task, as in the original.
    pub(super) fn request_purge_task(&mut self, id: Id, cx: &mut Context<Self>) {
        let title = self
            .tasks
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.title.clone())
            .or_else(|| self.store.task(id).ok().flatten().map(|t| t.title))
            .unwrap_or_default();
        self.confirm_delete = Some(ConfirmDeleteTarget::PurgeTask(id, title));
        cx.notify();
    }

    pub(super) fn execute_confirmed_delete(&mut self, target: ConfirmDeleteTarget, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        match target {
            ConfirmDeleteTarget::List(id, _) => {
                let result = self.store.delete_list(id);
                self.check(result);
                if self.view == View::List(id) {
                    crate::data_version::data_changed(cx.entity_id(), cx);
                    self.set_view(View::Inbox, cx);
                    return;
                }
            }
            ConfirmDeleteTarget::Tag(tag) => {
                let result = self.store.delete_tag(&tag);
                self.check(result);
                if self.view == View::Tag(tag) {
                    crate::data_version::data_changed(cx.entity_id(), cx);
                    self.set_view(View::Inbox, cx);
                    return;
                }
            }
            ConfirmDeleteTarget::Filter(id, _) => {
                let result = self.store.delete_filter(id);
                if self.check(result).is_some() && self.view == View::Filter(id) {
                    crate::data_version::data_changed(cx.entity_id(), cx);
                    self.set_view(View::Inbox, cx);
                    return;
                }
            }
            ConfirmDeleteTarget::Section(id, _) => {
                let result = self.store.delete_list_section(id);
                self.check(result);
            }
            ConfirmDeleteTarget::PurgeTask(id, _) => {
                let result = self.store.purge_task(id);
                self.check(result);
                if self.card == Some(id) {
                    self.close_card(cx);
                }
            }
            ConfirmDeleteTarget::BatchPurge(_) => {
                self.run_batch(true, cx, |store, ids| store.batch_purge(ids));
                return;
            }
        }
        crate::data_version::data_changed(cx.entity_id(), cx);
        self.reload(cx);
    }

    pub(super) fn confirm_batch_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lines) = self.batch_add_pending.take() else {
            return;
        };
        let mut shared_draft = self.add_draft.clone();
        shared_draft.files.clear();
        for line in lines {
            self.add_draft = shared_draft.clone();
            self.submit_new_task(&line, window, cx);
        }
        self.add_draft = Default::default();
        cx.notify();
    }

    pub(super) fn paste_batch_as_one(&mut self, cx: &mut Context<Self>) {
        let Some(lines) = self.batch_add_pending.take() else {
            return;
        };
        let joined = lines.join(" ");
        self.input.update(cx, |input, cx| {
            let existing = input.text().to_owned();
            let combined = if existing.is_empty() {
                joined
            } else {
                format!("{existing} {joined}")
            };
            input.set_text(combined, cx);
        });
        self.update_quick_add_hint(cx);
        cx.notify();
    }

    pub(super) fn render_section(
        &self,
        section: Section,
        count: usize,
        collapsed: bool,
        strings: &Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = cx.theme().colors().accent;
        let drop_target = match section {
            Section::Tasks => DropTarget::Kind(Kind::Task),
            Section::Notes => DropTarget::Kind(Kind::Note),
            Section::Completed => DropTarget::Complete,
        };
        h_flex()
            .id(SharedString::from(format!("group-{section:?}")))
            .h(px(36.))
            // As the original's, the arrow sits 7 px in from the rows' edge.
            .pl(px(7.))
            .gap_1()
            .rounded(px(6.))
            .cursor_pointer()
            .when(self.view != View::Trash && !self.is_current_view_archived_list(), |el| {
                el.drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.10)))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                        cx.stop_propagation();
                        this.task_drop = None;
                        this.task_drag_origin = None;
                        this.drop_on_column(dragged.id, drop_target.clone(), cx);
                    }))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
            .child(chevron(collapsed))
            .child(Label::new(section.title(strings)).weight(FontWeight::BOLD))
            .child(div().pl_1().child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)))
            .into_any_element()
    }

    pub(super) fn render_detail_popup(
        &self,
        detail_width: f32,
        theme: &theme::Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let popup_height = (window.viewport_size().height * 0.8)
            .min((window.viewport_size().height - px(32.)).max(px(200.)));
        Modal::new("detail-popup")
            .width(px(detail_width))
            .height(popup_height)
            .padding(px(0.))
            .gap(px(0.))
            .overflow_hidden()
            .on_dismiss(cx.listener(|this, _, window, cx| {
                this.selected = None;
                this.list_activity = None;
                window.focus(&this.list_focus, cx);
                cx.notify();
            }))
            .child(
                div()
                    .size_full()
                    .bg(theme.colors().background)
                    .map(|el| match self.list_activity.clone() {
                        Some(entries) => el.child(self.render_list_activity(&entries, false, cx)),
                        None => el.child(self.detail.clone()),
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_undo_toast(
        &self,
        label: String,
        strings: &'static Strings,
        theme: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .absolute()
            .bottom(px(24.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                h_flex()
                    .id("undo-toast")
                    .h(px(40.))
                    .px_4()
                    .gap_4()
                    .rounded(px(8.))
                    .bg(theme.colors().elevated_surface_background)
                    .border_1()
                    .border_color(theme.colors().border)
                    .shadow_md()
                    .child(Label::new(label).size(LabelSize::Small).truncate())
                    .child(
                        div()
                            .id("undo-button")
                            .cursor_pointer()
                            .child(Label::new(strings.undo).size(LabelSize::Small).color(Color::Accent))
                            .on_click(cx.listener(|this, _, _, cx| this.apply_undo(cx))),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_confirm_empty_trash(
        &self,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .absolute()
            .inset_0()
            .when(cfg!(test), |el| {
                el.debug_selector(|| "confirm-empty-trash-modal".to_owned())
            })
            .child(
                ConfirmDialog::new("confirm-empty-trash", strings.confirm_empty_trash)
                    .width(px(340.))
                    .description(strings.confirm_empty_trash_hint)
                    .cancel_label(strings.cancel)
                    .confirm_label(strings.empty_trash)
                    .destructive(true)
                    .on_cancel(cx.listener(|this, _, _, cx| {
                        this.confirm_empty_trash = false;
                        cx.notify();
                    }))
                    .on_confirm(cx.listener(|this, _, _, cx| this.empty_trash(cx))),
            )
            .into_any_element()
    }

    pub(super) fn render_confirm_delete(
        &self,
        target: ConfirmDeleteTarget,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, hint) = match &target {
            ConfirmDeleteTarget::List(_, name) => (
                format!("{} \"{name}\"?", strings.delete),
                strings.confirm_delete_list_hint,
            ),
            ConfirmDeleteTarget::Tag(tag) => (
                format!("{} \"#{tag}\"?", strings.delete),
                strings.confirm_delete_tag_hint,
            ),
            ConfirmDeleteTarget::Filter(_, name) => (
                format!("{} \"{name}\"?", strings.delete_filter),
                strings.confirm_delete_filter_hint,
            ),
            ConfirmDeleteTarget::Section(_, name) => (
                format!("{} \"{name}\"?", strings.delete),
                strings.confirm_delete_section_hint,
            ),
            ConfirmDeleteTarget::PurgeTask(_, title) => (
                if title.trim().is_empty() {
                    format!("{}?", strings.delete_forever)
                } else {
                    format!("{} \"{title}\"?", strings.delete_forever)
                },
                strings.confirm_delete_forever_single_hint,
            ),
            ConfirmDeleteTarget::BatchPurge(count) => (
                format!(
                    "{} ({})?",
                    strings.delete_forever,
                    strings.batch_selected.replace("{n}", &count.to_string())
                ),
                strings.confirm_delete_forever_hint,
            ),
        };
        div()
            .absolute()
            .inset_0()
            .when(cfg!(test), |el| {
                el.debug_selector(|| "confirm-delete-modal".to_owned())
            })
            .child(
                ConfirmDialog::new("confirm-delete", title)
                    .width(px(340.))
                    .description(hint)
                    .cancel_label(strings.cancel)
                    .confirm_label(strings.delete)
                    .destructive(true)
                    .on_cancel(cx.listener(|this, _, _, cx| {
                        this.confirm_delete = None;
                        cx.notify();
                    }))
                    .on_confirm(cx.listener(move |this, _, _, cx| {
                        this.execute_confirmed_delete(target.clone(), cx);
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_batch_add_modal(
        &self,
        lines: Vec<String>,
        strings: &'static Strings,
        theme: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = lines.len();
        let preview: Vec<String> = lines.iter().take(5).cloned().collect();
        let extra = count.saturating_sub(preview.len());
        let desc = strings.batch_add_tasks_desc.replace("{n}", &count.to_string());
        let add_label = format!("{} ({count})", strings.add);
        Modal::new("batch-add-backdrop")
            .width(px(380.))
            .padding(px(16.))
            .on_dismiss(cx.listener(|this, _, _, cx| {
                this.batch_add_pending = None;
                cx.notify();
            }))
            .child(Label::new(strings.batch_add_tasks).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
            .child(Label::new(desc).size(LabelSize::Small).color(Color::Muted))
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .rounded(px(6.))
                    .bg(theme.colors().surface_background)
                    .border_1()
                    .border_color(theme.colors().border_variant)
                    .children(preview.into_iter().map(|line| {
                        h_flex()
                            .gap_1p5()
                            .child(div().w(px(4.)).h(px(4.)).rounded_full().bg(theme.colors().accent))
                            .child(Label::new(line).size(LabelSize::XSmall).truncate())
                    }))
                    .when(extra > 0, |list| {
                        list.child(
                            Label::new(format!("+{extra}"))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    }),
            )
            .child(
                h_flex()
                    .pt_1()
                    .gap_2()
                    .justify_end()
                    .child(
                        div()
                            .when(cfg!(test), |el| el.debug_selector(|| "batch-add-cancel".to_owned()))
                            .child(
                                Button::new("batch-add-cancel", strings.cancel)
                                    .variant(ButtonVariant::Secondary)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.batch_add_pending = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .when(cfg!(test), |el| el.debug_selector(|| "batch-add-as-one".to_owned()))
                            .child(
                                Button::new("batch-add-as-one", strings.batch_add_as_one)
                                    .variant(ButtonVariant::Secondary)
                                    .on_click(cx.listener(|this, _, _, cx| this.paste_batch_as_one(cx))),
                            ),
                    )
                    .child(
                        div()
                            .when(cfg!(test), |el| el.debug_selector(|| "batch-add-confirm".to_owned()))
                            .child(
                                Button::new("batch-add-confirm", add_label)
                                    .variant(ButtonVariant::Primary)
                                    .on_click(cx.listener(|this, _, window, cx| this.confirm_batch_add(window, cx))),
                            ),
                    ),
            )
            .into_any_element()
    }
}
