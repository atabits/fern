use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BatchMenu {
    List,
    Tags,
}

impl TodoView {
    pub(super) fn is_multi_selecting(&self) -> bool {
        self.multi_selected.len() > 1
    }

    pub(super) fn is_task_selected(&self, id: Id) -> bool {
        if self.is_multi_selecting() {
            self.multi_selected.contains(&id)
        } else {
            self.selected == Some(id)
        }
    }

    pub(super) fn selected_task_ids(&self) -> Vec<Id> {
        if self.is_multi_selecting() {
            self.tasks
                .iter()
                .filter(|task| self.multi_selected.contains(&task.id))
                .map(|task| task.id)
                .collect()
        } else {
            self.active_task_id().into_iter().collect()
        }
    }

    pub(super) fn clear_multi_selection(&mut self, cx: &mut Context<Self>) {
        if self.multi_selected.is_empty() && self.batch_menu.is_none() {
            return;
        }
        self.multi_selected.clear();
        self.selection_anchor = self.selected;
        self.batch_menu = None;
        if let Some(id) = self.selected {
            self.selected = None;
            self.select(id, cx);
        }
        cx.notify();
    }

    pub(super) fn handle_row_click(
        &mut self,
        id: Id,
        modifiers: gpui::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.list_focus, cx);
        let strings = i18n::t(cx);
        if modifiers.shift {
            let ids = self.visible_task_ids(strings);
            let anchor = self.selection_anchor.or(self.selected).unwrap_or(id);
            if let (Some(a), Some(b)) = (ids.iter().position(|x| *x == anchor), ids.iter().position(|x| *x == id)) {
                let (start, end) = if a <= b { (a, b) } else { (b, a) };
                self.multi_selected = ids[start..=end].iter().copied().collect();
            } else {
                self.multi_selected.clear();
                self.multi_selected.insert(id);
            }
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(anchor);
            }
            self.batch_menu = None;
            if self.multi_selected.len() <= 1 {
                self.multi_selected.clear();
                self.selected = None;
                self.select(id, cx);
            } else {
                self.selected = Some(id);
            }
            cx.notify();
            return;
        }

        if modifiers.secondary() {
            if self.multi_selected.is_empty()
                && let Some(prev) = self.selected
                && prev != id
            {
                self.multi_selected.insert(prev);
            }
            if !self.multi_selected.remove(&id) {
                self.multi_selected.insert(id);
            }
            self.selection_anchor = Some(id);
            self.batch_menu = None;
            match self.multi_selected.len() {
                0 => {
                    self.selected = None;
                    self.detail.update(cx, |detail, cx| detail.show(None, cx));
                }
                1 => {
                    if let Some(only) = self.multi_selected.iter().next().copied() {
                        self.multi_selected.clear();
                        self.selected = None;
                        self.select(only, cx);
                    }
                }
                _ => {
                    self.selected = Some(id);
                }
            }
            cx.notify();
            return;
        }

        let had_multi = !self.multi_selected.is_empty();
        self.multi_selected.clear();
        self.selection_anchor = Some(id);
        self.batch_menu = None;
        if had_multi {
            self.selected = None;
        }
        self.select(id, cx);
        cx.notify();
    }

    pub(super) fn select_all_visible(&mut self, _: &SelectAllTasks, window: &mut Window, cx: &mut Context<Self>) {
        if self.view == View::Summary {
            return;
        }
        window.focus(&self.list_focus, cx);
        let ids = self.visible_task_ids(i18n::t(cx));
        self.batch_menu = None;
        match ids.len() {
            0 => self.clear_multi_selection(cx),
            1 => {
                self.multi_selected.clear();
                self.selection_anchor = Some(ids[0]);
                self.select(ids[0], cx);
                cx.notify();
            }
            _ => {
                self.multi_selected = ids.iter().copied().collect();
                self.selection_anchor = Some(ids[0]);
                self.selected = Some(ids[0]);
                cx.notify();
            }
        }
    }

    pub(super) fn extend_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let ids = self.visible_task_ids(i18n::t(cx));
        if ids.is_empty() {
            return;
        }
        let anchor = self.selection_anchor.or(self.active_task_id()).unwrap_or(ids[0]);
        self.selection_anchor = Some(anchor);
        let current = self
            .active_task_id()
            .and_then(|id| ids.iter().position(|x| *x == id))
            .unwrap_or(0);
        let next_ix = (current as isize + step).clamp(0, ids.len() as isize - 1) as usize;
        let anchor_ix = ids.iter().position(|x| *x == anchor).unwrap_or(next_ix);
        let (start, end) = if anchor_ix <= next_ix { (anchor_ix, next_ix) } else { (next_ix, anchor_ix) };
        self.multi_selected = ids[start..=end].iter().copied().collect();
        self.batch_menu = None;
        let focus_id = ids[next_ix];
        if self.multi_selected.len() <= 1 {
            self.multi_selected.clear();
            self.select_in_current_layout(focus_id, cx);
        } else {
            self.selected = Some(focus_id);
            if let Some(ix) = self
                .list_entries
                .iter()
                .position(|e| matches!(e, ListEntry::Row(Row::Task { task, .. }) if task.id == focus_id))
            {
                self.list_state.scroll_to_reveal_item(ix);
            }
        }
        cx.notify();
    }

    pub(super) fn toggle_in_progress_selected(&mut self, _: &ToggleInProgress, _: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected_task_ids();
        if ids.is_empty() {
            return;
        }
        let all_in_progress = ids
            .iter()
            .filter_map(|id| self.tasks.iter().find(|t| t.id == *id))
            .all(Task::is_in_progress);
        self.run_batch(false, cx, move |store, ids| store.batch_set_in_progress(ids, !all_in_progress));
    }

    pub(super) fn run_batch(
        &mut self,
        clear_after: bool,
        cx: &mut Context<Self>,
        op: impl FnOnce(&Store, &[Id]) -> anyhow::Result<()>,
    ) {
        let ids = self.selected_task_ids();
        if ids.is_empty() {
            return;
        }
        let result = op(&self.store, &ids);
        self.check(result);
        self.batch_menu = None;
        if clear_after {
            self.multi_selected.clear();
            self.selection_anchor = None;
        }
        self.reload(cx);
    }

    pub(super) fn render_batch_panel(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ids = self.selected_task_ids();
        let selected_tasks: Vec<&Task> = ids
            .iter()
            .filter_map(|id| self.tasks.iter().find(|t| t.id == *id))
            .collect();
        let all_completed = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.is_completed());
        let all_in_progress = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.is_in_progress());
        let all_wont_do = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.is_wont_do());
        let all_pinned = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.pinned_at.is_some());
        let all_notes = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.kind == Kind::Note);
        let today = Self::today();

        let action_chip = |id: &'static str, icon: IconName, label: &'static str, active: bool, tint: Option<Color>| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .px_2p5()
                .py_1()
                .rounded(px(6.))
                .border_1()
                .border_color(if active { theme.colors().accent } else { theme.colors().border })
                .bg(if active { theme.colors().accent.opacity(0.12) } else { theme.colors().surface_background })
                .hover(|s| s.bg(theme.colors().element_hover))
                .cursor_pointer()
                .child(
                    h_flex()
                        .gap_1p5()
                        .child(Icon::new(icon).size(IconSize::XSmall).color(tint.unwrap_or(if active { Color::Accent } else { Color::IconMuted })))
                        .child(Label::new(label).size(LabelSize::Small).color(if active { Color::Accent } else { Color::Default })),
                )
        };

        let mut body = v_flex().p_4().gap_4();
        if self.view == View::Trash {
            body = body.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        action_chip("batch-restore", IconName::RotateCcw, strings.restore, false, Some(Color::Accent))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_batch(true, cx, |store, ids| store.batch_restore(ids));
                            })),
                    )
                    .child(
                        action_chip("batch-purge", IconName::Trash, strings.delete_forever, false, Some(Color::Error))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_batch(true, cx, |store, ids| store.batch_purge(ids));
                            })),
                    ),
            );
        } else {
            body = body
                // Status & Pin actions.
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            action_chip(
                                "batch-complete",
                                IconName::SquareCheck,
                                if all_completed { strings.batch_reopen } else { strings.batch_complete },
                                all_completed,
                                None,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !all_completed {
                                    desktop_common::sound::play_complete(&this.store);
                                }
                                this.run_batch(false, cx, move |store, ids| store.batch_complete(ids, !all_completed));
                            })),
                        )
                        .child(
                            action_chip(
                                "batch-in-progress",
                                IconName::Hourglass,
                                if all_in_progress { strings.clear_in_progress } else { strings.mark_in_progress },
                                all_in_progress,
                                None,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_batch(false, cx, move |store, ids| store.batch_set_in_progress(ids, !all_in_progress));
                            })),
                        )
                        .child(
                            action_chip(
                                "batch-wont-do",
                                IconName::SquareX,
                                if all_wont_do { strings.reopen } else { strings.mark_wont_do },
                                all_wont_do,
                                None,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_batch(false, cx, move |store, ids| store.batch_set_wont_do(ids, !all_wont_do));
                            })),
                        )
                        .child(
                            action_chip(
                                "batch-pin",
                                IconName::Pin,
                                if all_pinned { strings.unpin } else { strings.pin },
                                all_pinned,
                                None,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_batch(false, cx, move |store, ids| store.batch_set_pinned(ids, !all_pinned));
                            })),
                        )
                        .child(
                            action_chip("batch-merge", IconName::ListChecks, strings.batch_merge, false, None)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let ids = this.selected_task_ids();
                                    let merged = this.check(this.store.batch_merge_tasks(&ids)).flatten();
                                    if let Some(target_id) = merged {
                                        this.multi_selected.clear();
                                        this.batch_menu = None;
                                        this.selected = None;
                                        this.reload(cx);
                                        this.select(target_id, cx);
                                        crate::data_version::data_changed(cx.entity_id(), cx);
                                    }
                                })),
                        )
                        .child(
                            action_chip("batch-duplicate", IconName::Copy, strings.duplicate, false, None)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run_batch(false, cx, |store, ids| store.batch_duplicate(ids).map(|_| ()));
                                })),
                        )
                        .child(
                            action_chip(
                                "batch-convert-kind",
                                if all_notes { IconName::SquareCheck } else { IconName::FileText },
                                if all_notes { strings.convert_to_task } else { strings.convert_to_note },
                                false,
                                None,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let target = if all_notes { Kind::Task } else { Kind::Note };
                                this.run_batch(false, cx, move |store, ids| store.batch_set_kind(ids, target));
                            })),
                        ),
                )
                // Due dates.
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(Label::new(strings.due_date).size(LabelSize::XSmall).color(Color::Muted))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    action_chip("batch-due-today", IconName::Sun, strings.today, false, Some(Color::Accent))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.run_batch(false, cx, move |store, ids| store.batch_set_due(ids, Some(today), None));
                                        })),
                                )
                                .child(
                                    action_chip("batch-due-tomorrow", IconName::Sunrise, strings.tomorrow, false, None)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let date = today + Duration::days(1);
                                            this.run_batch(false, cx, move |store, ids| store.batch_set_due(ids, Some(date), None));
                                        })),
                                )
                                .child(
                                    action_chip("batch-due-week", IconName::CalendarPlus, strings.next_week, false, None)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let date = today + Duration::days(7);
                                            this.run_batch(false, cx, move |store, ids| store.batch_set_due(ids, Some(date), None));
                                        })),
                                )
                                .child(
                                    action_chip("batch-due-clear", IconName::CalendarX, strings.clear, false, None)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.run_batch(false, cx, |store, ids| store.batch_set_due(ids, None, None));
                                        })),
                                ),
                        ),
                )
                // Priorities.
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(Label::new(strings.filter_priority).size(LabelSize::XSmall).color(Color::Muted))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_2()
                                .children([
                                    ("batch-prio-high", Priority::High, strings.priority_high),
                                    ("batch-prio-med", Priority::Medium, strings.priority_medium),
                                    ("batch-prio-low", Priority::Low, strings.priority_low),
                                    ("batch-prio-none", Priority::None, strings.priority_none),
                                ].into_iter().map(|(id, priority, label)| {
                                    let active = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.priority == priority);
                                    let color = task_rows::priority_color(priority, &theme);
                                    div()
                                        .id(id)
                                        .debug_selector(move || id.to_owned())
                                        .px_2p5()
                                        .py_1()
                                        .rounded(px(6.))
                                        .border_1()
                                        .border_color(if active { theme.colors().accent } else { theme.colors().border })
                                        .bg(if active { theme.colors().accent.opacity(0.12) } else { theme.colors().surface_background })
                                        .hover(|s| s.bg(theme.colors().element_hover))
                                        .cursor_pointer()
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .child(
                                                    Icon::new(IconName::Flag)
                                                        .size(IconSize::XSmall)
                                                        .color(color.map_or(Color::Muted, Color::Custom)),
                                                )
                                                .child(Label::new(label).size(LabelSize::Small)),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.run_batch(false, cx, move |store, ids| store.batch_set_priority(ids, priority));
                                        }))
                                })),
                        ),
                )
                // Move to List.
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(
                            div()
                                .id("batch-list-toggle")
                                .px_2()
                                .py_1()
                                .rounded(px(6.))
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .cursor_pointer()
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(Icon::new(IconName::List).size(IconSize::XSmall).color(Color::Muted))
                                                .child(Label::new(strings.move_to).size(LabelSize::Small)),
                                        )
                                        .child(Icon::new(if self.batch_menu == Some(BatchMenu::List) { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::XSmall).color(Color::Muted)),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.batch_menu = if this.batch_menu == Some(BatchMenu::List) { None } else { Some(BatchMenu::List) };
                                    cx.notify();
                                })),
                        )
                        .when(self.batch_menu == Some(BatchMenu::List), |el| {
                            let all_inbox = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.list_id.is_none());
                            el.child(
                                v_flex()
                                    .pl_2()
                                    .gap_0p5()
                                    .child(
                                        ListItem::new("batch-list-inbox")
                                            .height(px(28.))
                                            .start_slot(Icon::new(IconName::Inbox).size(IconSize::XSmall).color(Color::Muted))
                                            .child(Label::new(strings.inbox).size(LabelSize::Small))
                                            .when(all_inbox, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.run_batch(false, cx, |store, ids| store.batch_set_list(ids, None));
                                            })),
                                    )
                                    .children(self.active_lists().into_iter().map(|list| {
                                        let list_id = list.id;
                                        let all_here = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.list_id == Some(list_id));
                                        ListItem::new(SharedString::from(format!("batch-list-{list_id}")))
                                            .height(px(28.))
                                            .start_slot(Icon::new(IconName::List).size(IconSize::XSmall).color(Color::Muted))
                                            .child(Label::new(list.name).size(LabelSize::Small).truncate())
                                            .when(all_here, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.run_batch(false, cx, move |store, ids| store.batch_set_list(ids, Some(list_id)));
                                            }))
                                    })),
                            )
                        }),
                )
                // Tags.
                .when(!self.tags.is_empty(), |el| {
                    el.child(
                        v_flex()
                            .gap_1p5()
                            .child(
                                div()
                                    .id("batch-tags-toggle")
                                    .px_2()
                                    .py_1()
                                    .rounded(px(6.))
                                    .hover(|s| s.bg(theme.colors().element_hover))
                                    .cursor_pointer()
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .child(Icon::new(IconName::Tag).size(IconSize::XSmall).color(Color::Muted))
                                                    .child(Label::new(strings.tags).size(LabelSize::Small)),
                                            )
                                            .child(Icon::new(if self.batch_menu == Some(BatchMenu::Tags) { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::XSmall).color(Color::Muted)),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.batch_menu = if this.batch_menu == Some(BatchMenu::Tags) { None } else { Some(BatchMenu::Tags) };
                                        cx.notify();
                                    })),
                            )
                            .when(self.batch_menu == Some(BatchMenu::Tags), |el| {
                                el.child(
                                    v_flex()
                                        .pl_2()
                                        .gap_0p5()
                                        .children(self.tags.iter().map(|tag| {
                                            let name = tag.name.clone();
                                            let all_have = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.tags.iter().any(|existing| existing == &name));
                                            ListItem::new(SharedString::from(format!("batch-tag-{name}")))
                                                .height(px(28.))
                                                .start_slot(Icon::new(IconName::Tag).size(IconSize::XSmall).color(Color::Muted))
                                                .child(Label::new(name.clone()).size(LabelSize::Small).truncate())
                                                .when(all_have, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    let tag_name = name.clone();
                                                    this.run_batch(false, cx, move |store, ids| {
                                                        if all_have {
                                                            store.batch_remove_tag(ids, &tag_name)
                                                        } else {
                                                            store.batch_add_tag(ids, &tag_name)
                                                        }
                                                    });
                                                }))
                                        })),
                                )
                            }),
                    )
                });
        }

        v_flex()
            .id("batch-edit-panel")
            .debug_selector(|| "batch-edit-panel".to_owned())
            .size_full()
            .bg(theme.colors().background)
            .border_l_1()
            .border_color(theme.colors().border)
            .child(
                h_flex()
                    .mt(crate::title_bar::header_inset())
                    .h(px(48.))
                    .px_5()
                    .border_b_1()
                    .border_color(theme.colors().border)
                    .child(
                        div()
                            .flex_1()
                            .child(Label::new(strings.batch_selected.replace("{n}", &ids.len().to_string())).size(LabelSize::Large)),
                    )
                    .child(
                        div()
                            .id("batch-clear-tip")
                            .debug_selector(|| "batch-clear".to_owned())
                            .tooltip(Tooltip::text(strings.clear_selection))
                            .child(
                                IconButton::new("batch-clear", IconName::X)
                                    .on_click(cx.listener(|this, _, _, cx| this.clear_multi_selection(cx))),
                            ),
                    ),
            )
            .child(v_scroll("batch-panel-scroll", body.id("batch-panel-body")).flex_1().min_h_0().compact())
            .when(self.view != View::Trash, |el| {
                el.child(
                    h_flex()
                        .h(px(48.))
                        .px_4()
                        .border_t_1()
                        .border_color(theme.colors().border)
                        .justify_end()
                        .child(
                            div()
                                .id("batch-delete")
                                .px_3()
                                .py_1()
                                .rounded(px(6.))
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .cursor_pointer()
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .child(Icon::new(IconName::Trash).size(IconSize::XSmall).color(Color::Error))
                                        .child(Label::new(strings.delete).size(LabelSize::Small).color(Color::Error)),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run_batch(true, cx, |store, ids| store.batch_delete(ids));
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_batch_bar(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ids = self.selected_task_ids();
        let selected_tasks: Vec<&Task> = ids
            .iter()
            .filter_map(|id| self.tasks.iter().find(|t| t.id == *id))
            .collect();
        let all_completed = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.is_completed());
        let all_in_progress = !selected_tasks.is_empty() && selected_tasks.iter().all(|t| t.is_in_progress());
        let today = Self::today();
        let bar_btn = |id: &'static str, icon: IconName, tip: &'static str, tint: Color| {
            div()
                .id(id)
                .tooltip(Tooltip::text(tip))
                .child(IconButton::new(SharedString::from(format!("{id}-btn")), icon).icon_size(IconSize::Small).icon_color(tint))
        };

        div()
            .id("batch-edit-bar")
            .debug_selector(|| "batch-edit-bar".to_owned())
            .h(px(42.))
            .px_3()
            .rounded(px(10.))
            .bg(theme.colors().elevated_surface_background)
            .border_1()
            .border_color(theme.colors().border)
            .shadow_md()
            .flex()
            .items_center()
            .gap_2()
            .child(Label::new(strings.batch_selected.replace("{n}", &ids.len().to_string())).size(LabelSize::Small).color(Color::Accent))
            .when(self.view == View::Trash, |el| {
                el.child(
                    bar_btn("batch-bar-restore", IconName::RotateCcw, strings.restore, Color::Accent)
                        .on_click(cx.listener(|this, _, _, cx| this.run_batch(true, cx, |store, ids| store.batch_restore(ids)))),
                )
                .child(
                    bar_btn("batch-bar-purge", IconName::Trash, strings.delete_forever, Color::Error)
                        .on_click(cx.listener(|this, _, _, cx| this.run_batch(true, cx, |store, ids| store.batch_purge(ids)))),
                )
            })
            .when(self.view != View::Trash, |el| {
                el.child(
                    bar_btn(
                        "batch-bar-complete",
                        IconName::SquareCheck,
                        if all_completed { strings.batch_reopen } else { strings.batch_complete },
                        Color::Icon,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !all_completed {
                            desktop_common::sound::play_complete(&this.store);
                        }
                        this.run_batch(false, cx, move |store, ids| store.batch_complete(ids, !all_completed));
                    })),
                )
                .child(
                    bar_btn(
                        "batch-bar-progress",
                        IconName::Hourglass,
                        if all_in_progress { strings.clear_in_progress } else { strings.mark_in_progress },
                        if all_in_progress { Color::Accent } else { Color::Icon },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.run_batch(false, cx, move |store, ids| store.batch_set_in_progress(ids, !all_in_progress));
                    })),
                )
                .child(
                    bar_btn("batch-bar-today", IconName::Sun, strings.today, Color::Icon)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.run_batch(false, cx, move |store, ids| store.batch_set_due(ids, Some(today), None));
                        })),
                )
                .child(
                    bar_btn(
                        "batch-bar-high",
                        IconName::Flag,
                        strings.priority_high,
                        task_rows::priority_color(Priority::High, &theme).map_or(Color::Icon, Color::Custom),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.run_batch(false, cx, |store, ids| store.batch_set_priority(ids, Priority::High));
                    })),
                )
                .child(
                    bar_btn("batch-bar-delete", IconName::Trash, strings.delete, Color::Error)
                        .on_click(cx.listener(|this, _, _, cx| this.run_batch(true, cx, |store, ids| store.batch_delete(ids)))),
                )
            })
            .child(
                bar_btn("batch-bar-clear", IconName::X, strings.clear_selection, Color::Muted)
                    .on_click(cx.listener(|this, _, _, cx| this.clear_multi_selection(cx))),
            )
            .into_any_element()
    }
}
