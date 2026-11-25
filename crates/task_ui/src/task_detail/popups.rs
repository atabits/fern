use std::cell::Cell;
use std::rc::Rc;

use fern_core::{Id, Kind, Priority, Task};
use gpui::Focusable;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use crate::i18n::{self, Strings};
use crate::task_rows;

use super::{
    EstimateDraft, Popup, StartFocus, TaskDetail, Undoable, check_mark, list_matches, same_tag,
};

impl TaskDetail {
    /// The panel's "…" menu, a subset of the original's: the entries Fern supports so far.
    /// A "…" menu entry; hovering it closes the Start Focus side menu.
    fn close_side_menu_on_hover(&self, item: ListItem, cx: &mut Context<Self>) -> impl IntoElement {
        let selector = format!("task-menu-{}", item.id());
        div().id(item.id()).debug_selector(move || selector).child(item).on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if *hovered && this.focus_submenu {
                this.focus_submenu = false;
                cx.notify();
            }
        }))
    }

    pub(super) fn render_more_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        self.render_more_popup_in(None, strings, cx)
    }

    pub(super) fn render_more_popup_in(
        &self,
        max_height: Option<Pixels>,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.session.draft.is_some() { return self.render_draft_menu(strings, cx); }
        let Some(task) = self.session.task.as_ref() else { return div().into_any_element() };
        let wont_do = task.is_wont_do();
        let is_note = task.kind == Kind::Note;
        let submenu_bounds = Rc::new(Cell::new(None::<gpui::Bounds<Pixels>>));
        let outside_submenu = submenu_bounds.clone();
        let item = |id: &'static str, icon: IconName, label: &'static str| {
            ListItem::new(id).height(px(34.)).start_slot(Icon::new(icon).color(Color::Icon)).child(Label::new(label))
        };
        let content = v_flex()
            .id("task-more-popup-items")
            .when(self.sticky_style.is_some(), |menu| {
                menu.when(!is_note, |menu| {
                    let label = if task.is_completed() { strings.reopen } else { strings.completed };
                    menu.child(item("sticky-complete", IconName::SquareCheck, label)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_popup(cx);
                            this.toggle_completed(cx);
                        })))
                })
                .child(item("sticky-checklist", IconName::ListChecks, strings.checklist)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_popup(cx);
                        this.start_checklist(cx);
                    })))
                .child(item("sticky-priority", IconName::Flag, strings.summary_priority)
                    .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Priority, cx))))
                .child(item("sticky-move", IconName::Inbox, strings.move_to)
                    .on_click(cx.listener(|this, _, window, cx| this.open_popup_from_ui(Popup::List, window, cx))))
                .child(item("sticky-format", IconName::Baseline, strings.text_format)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_popup(cx);
                        this.format_bar = !this.format_bar;
                        this.format_headings = false;
                    })))
                .child(item("sticky-comments", IconName::MessageSquareText, strings.comment_tip)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_popup(cx);
                        this.comment_open = !this.comment_open;
                        if this.comment_open {
                            window.focus(&this.comment_input.focus_handle(cx), cx);
                        } else {
                            this.comment_emoji = false;
                        }
                    })))
            })
            .when(self.can_add_subtask(), |el| {
                el.child(self.close_side_menu_on_hover(item("more-subtask", IconName::CornerDownRight, strings.add_subtask_item).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.close_popup(cx);
                        this.subtask_open = true;
                        window.focus(&this.subtask_input.focus_handle(cx), cx);
                    },
                )), cx))
            })
            .when(!is_note, |el| {
                el.child(self.close_side_menu_on_hover(item("more-link-parent", IconName::ListTree, strings.link_parent).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.open_popup_from_ui(Popup::LinkParent, window, cx);
                        let search = this.ensure_link_search(cx);
                        window.focus(&search.focus_handle(cx), cx);
                    },
                )), cx))
            })
            .when(!is_note && task.parent_id.is_some(), |el| {
                el.child(self.close_side_menu_on_hover(item("more-unlink-parent", IconName::X, strings.unlink_parent).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.close_popup(cx);
                        this.unlink_current_parent(cx);
                    },
                )), cx))
            })
            .child(self.close_side_menu_on_hover({
                let pinned = task.pinned_at.is_some();
                item("more-pin", if pinned { IconName::PinOff } else { IconName::Pin }, if pinned { strings.unpin } else { strings.pin })
                    .on_click(cx.listener(move |this, _, _, cx| this.menu_action(cx, move |store, id| store.set_pinned(id, !pinned))))
            }, cx))
            .when(!is_note, |el| {
                let active = task.is_in_progress();
                el.child(self.close_side_menu_on_hover(
                    item("more-in-progress", IconName::Hourglass, if active { strings.clear_in_progress } else { strings.mark_in_progress })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_in_progress(cx))),
                    cx,
                ))
            })
            .child(self.close_side_menu_on_hover(
                item("more-wont-do", if wont_do { IconName::RotateCcw } else { IconName::SquareX }, if wont_do {
                    strings.reopen
                } else {
                    strings.mark_wont_do
                })
                .on_click(cx.listener(|this, _, _, cx| this.toggle_wont_do(cx))),
            cx,
            ))
            .child(self.close_side_menu_on_hover(item("more-location", IconName::MapPin, strings.add_location).on_click(cx.listener(|this, _, window, cx| {
                this.close_popup(cx);
                this.location_open = true;
                window.focus(&this.location_input.focus_handle(cx), cx);
                cx.notify();
            })), cx))
            .child(self.close_side_menu_on_hover(item("more-tags", IconName::Tag, strings.tags).on_click(cx.listener(|this, _, window, cx| {
                this.open_tags(Popup::Tags, window, cx);
            })), cx))
            .child(self.close_side_menu_on_hover(
                item("more-attachment", IconName::Paperclip, strings.upload_attachment)
                    .on_click(cx.listener(|this, _, _, cx| this.upload_attachment(cx))),
                cx,
            ))
            .child(
                SubmenuItem::new("more-focus", strings.start_focus)
                    .icon(IconName::CircleDot)
                    .height(px(34.))
                    .open(self.focus_submenu)
                    .track_bounds(submenu_bounds)
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered && !this.focus_submenu {
                            this.focus_submenu = true;
                            cx.notify();
                        }
                    }))
                    .items([(false, strings.start_pomo), (true, strings.start_stopwatch)].map(|(stopwatch, label)| {
                        ListItem::new(SharedString::from(format!("more-focus-{stopwatch}")))
                            .height(px(34.))
                            .child(Label::new(label))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.close_popup(cx);
                                if let Some(task) = this.session.task.as_ref() {
                                    cx.emit(StartFocus(task.id, task.title.clone(), stopwatch));
                                }
                            }))
                    }))
                    .items([ListItem::new("more-focus-estimate")
                        .height(px(34.))
                        .child(div().debug_selector(|| "task-focus-estimate".into()).child(Label::new(strings.estimation)))
                        .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Estimate, cx)))]),
            )
            .child(div().py_1().child(Divider::horizontal()))
            .child(self.close_side_menu_on_hover(item("more-activities", IconName::ListTree, strings.task_activities).on_click(cx.listener(
                |this, _, _, cx| {
                    this.close_popup(cx);
                    this.open_activity(cx);
                },
            )), cx))
            .child(self.close_side_menu_on_hover(item("more-template", IconName::LayoutTemplate, strings.save_as_template).on_click(cx.listener(
                |this, _, _, cx| this.menu_action(cx, |store, id| store.save_template(id).map(|_| ())),
            )), cx))
            .child(self.close_side_menu_on_hover(item("more-duplicate", IconName::Copy, strings.duplicate).on_click(cx.listener(
                |this, _, _, cx| this.menu_action(cx, |store, id| store.duplicate_task(id).map(|_| ())),
            )), cx))
            .child(self.close_side_menu_on_hover(item("more-copy-link", IconName::Link, strings.copy_link).on_click(cx.listener(|this, _, _, cx| {
                this.close_popup(cx);
                if let Some(task) = this.session.task.as_ref() {
                    this.copy_to_clipboard(fern_core::link::task_link(task.id), cx);
                }
            })), cx))
            .child(self.close_side_menu_on_hover(item("more-copy-markdown", IconName::Code, strings.copy_markdown).on_click(cx.listener(|this, _, _, cx| {
                this.close_popup(cx);
                if let Some(task) = this.session.task.as_ref() {
                    let checklist = this.session.store.checklist(task.id).unwrap_or_default();
                    let text = fern_core::markdown::format_task_markdown(task, &checklist, &this.session.subtasks);
                    this.copy_to_clipboard(text, cx);
                }
            })), cx))
            .child(self.close_side_menu_on_hover(item("more-copy-plain", IconName::FileText, strings.copy_plain_text).on_click(cx.listener(|this, _, _, cx| {
                this.close_popup(cx);
                if let Some(task) = this.session.task.as_ref() {
                    let checklist = this.session.store.checklist(task.id).unwrap_or_default();
                    let text = fern_core::markdown::format_task_plain_text(task, &checklist, &this.session.subtasks);
                    this.copy_to_clipboard(text, cx);
                }
            })), cx))
            .when(is_note || (task.parent_id.is_none() && self.session.subtasks.is_empty()), |el| {
                el.child(self.close_side_menu_on_hover(
                    item(
                        "more-convert",
                        if is_note { IconName::SquareCheck } else { IconName::FileText },
                        if is_note { strings.convert_to_task } else { strings.convert_to_note },
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_kind(cx))),
                cx,
                ))
            })
            .child(div().py_1().child(Divider::horizontal()))
            .child(self.close_side_menu_on_hover(item("more-print", IconName::Printer, strings.print).on_click(cx.listener(|this, _, _, cx| {
                this.close_popup(cx);
                this.print_current(cx);
            })), cx))
            .child(div().my_1().child(Divider::horizontal()))
            .child(self.close_side_menu_on_hover(
                ListItem::new("more-delete")
                    .height(px(34.))
                    .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                    .child(Label::new(strings.delete).color(Color::Error))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let task = this.session.task.as_ref().map(|t| (t.id, t.title.clone()));
                        this.menu_action(cx, |store, id| store.delete_task(id));
                        if let Some((id, title)) = task {
                            let label = i18n::t(cx).undo_deleted.replace("{title}", &title);
                            cx.emit(Undoable(crate::undo::UndoAction::Restore(id), label));
                        }
                    })),
                cx,
            ));
        // The original's menu: 178 px wide (wider when a label needs it), a row every 34 px.
        popover(cx)
            .debug_selector(|| "task-more-popup".into())
            .min_w(px(178.))
            .on_mouse_down_out(cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                if !outside_submenu.get().is_some_and(|bounds| bounds.contains(&event.position)) {
                    this.close_popup(cx);
                }
            }))
            .map(|el| match max_height {
                Some(h) => el
                    .p_0()
                    .child(v_scroll("sticky-more-scroll-view", content.p_1()).max_h(h).compact()),
                None => el.child(content),
            })
            .into_any_element()
    }

    /// The original's Estimation popover: Estimated Pomo or Estimated Duration, with Cancel / OK.
    pub(super) fn render_estimate(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let draft = self.session.estimate_draft;
        let theme = cx.theme().clone();
        let kind = |minutes: bool, label: &'static str| {
            div()
                .id(if minutes { "estimate-duration" } else { "estimate-pomo" })
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(draft.minutes == minutes, |el| el.bg(theme.colors().element_selected))
                .child(Label::new(label).size(LabelSize::Small).color(if draft.minutes == minutes { Color::Default } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.session.estimate_draft.minutes != minutes {
                        this.session.estimate_draft = EstimateDraft { minutes, amount: 0 };
                        cx.notify();
                    }
                }))
        };
        let step = |id: &'static str, icon: IconName, up: bool| {
            IconButton::new(id, icon).size(px(24.)).icon_size(IconSize::Small).on_click(cx.listener(move |this, _, _, cx| {
                this.session.estimate_draft.step(up);
                cx.notify();
            }))
        };
        let value = if draft.minutes { strings.format_minutes(draft.amount as i64) } else { draft.amount.to_string() };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |l| l.color(Color::OnAccent)))
        };
        popover(cx)
            .debug_selector(|| "task-estimate-popup".into())
            .w(px(280.))
            .p_3()
            .gap_3()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(
                h_flex()
                    .p_0p5()
                    .rounded(px(8.))
                    .bg(theme.colors().surface_background)
                    .child(kind(false, strings.estimated_pomo))
                    .child(kind(true, strings.estimated_duration)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(step("estimate-less", IconName::Minus, false))
                    .child(
                        div()
                            .min_w(px(64.))
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .child(Label::new(value)),
                    )
                    .child(step("estimate-more", IconName::Plus, true))
                    .when(!draft.minutes, |el| el.child(Label::new(strings.pomo_unit).color(Color::Muted))),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .children(
                        (if draft.minutes { [30_u32, 60, 120, 240] } else { [1, 2, 4, 6] })
                            .into_iter()
                            .map(|preset| {
                                let selected = draft.amount == preset;
                                let label = if draft.minutes {
                                    strings.format_minutes(preset as i64)
                                } else {
                                    preset.to_string()
                                };
                                div()
                                    .id(SharedString::from(format!("estimate-preset-{preset}")))
                                    .flex_1()
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(5.))
                                    .cursor_pointer()
                                    .bg(if selected {
                                        theme.colors().accent.opacity(0.18)
                                    } else {
                                        theme.colors().element_hover
                                    })
                                    .hover(|s| s.bg(theme.colors().accent.opacity(0.24)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.session.estimate_draft.amount = preset;
                                        cx.notify();
                                    }))
                                    .child(
                                        Label::new(label)
                                            .size(LabelSize::XSmall)
                                            .color(if selected { Color::Accent } else { Color::Muted }),
                                    )
                            }),
                    )
                    .when(draft.amount > 0, |el| {
                        el.child(
                            div()
                                .id("estimate-clear")
                                .px_2()
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.session.estimate_draft.amount = 0;
                                    cx.notify();
                                }))
                                .child(Label::new(strings.clear).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(button("estimate-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| this.close_popup(cx))))
                    .child(button("estimate-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| {
                        let estimate = this.session.estimate_draft.estimate();
                        this.menu_action(cx, move |store, id| store.set_estimate(id, estimate));
                    }))),
            )
            .into_any_element()
    }

    pub(super) fn render_link_parent(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.session.task.is_none() {
            return div().into_any_element();
        }
        let query = self
            .link_search
            .as_ref()
            .map(|input| input.read(cx).text().trim().to_lowercase())
            .unwrap_or_default();
        let list_name = |list_id: Option<Id>| -> SharedString {
            list_id
                .and_then(|id| self.session.lists.iter().find(|l| l.id == id))
                .map_or(strings.inbox.into(), |l| l.name.clone().into())
        };
        let candidates: Vec<&Task> = self
            .session
            .link_parent_candidates
            .iter()
            .filter(|t| query.is_empty() || t.title.to_lowercase().contains(&query))
            .take(30)
            .collect();
        let linked_parent = query.is_empty().then(|| self.session.parent.clone()).flatten();
        popover(cx)
            .w(px(340.))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.dismiss_popup(window, cx);
            }))
            .child(
                h_flex()
                    .h(px(34.))
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.colors().border)
                    .child(Icon::new(IconName::Search).color(Color::Muted))
                    .when_some(self.link_search.clone(), |el, input| el.child(div().flex_1().child(input))),
            )
            .when_some(linked_parent, |el, parent| {
                let parent_title: SharedString = if parent.title.is_empty() {
                    strings.untitled.into()
                } else {
                    parent.title.clone().into()
                };
                let parent_list = list_name(parent.list_id);
                el.child(
                    v_flex()
                        .pt_1()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .child(Label::new(strings.linked_parent).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                        .child(
                            h_flex()
                                .id("link-parent-current")
                                .h(px(32.))
                                .px_2()
                                .gap_2()
                                .items_center()
                                .child(Icon::new(IconName::Circle).size(IconSize::Small).color(Color::Accent))
                                .child(div().flex_1().min_w_0().child(Label::new(parent_title).truncate()))
                                .child(Label::new(parent_list).size(LabelSize::XSmall).color(Color::Muted))
                                .child(
                                    div()
                                        .debug_selector(|| "link-parent-unlink".to_owned())
                                        .child(
                                            IconButton::new("link-parent-unlink-btn", IconName::X)
                                                .size(px(24.))
                                                .icon_size(IconSize::XSmall)
                                                .icon_color(Color::Muted)
                                                .tooltip(Tooltip::text(strings.unlink_parent))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    cx.stop_propagation();
                                                    this.dismiss_popup(window, cx);
                                                    this.unlink_current_parent(cx);
                                                })),
                                        ),
                                ),
                        )
                        .child(div().my_1().child(Divider::horizontal()))
                        .child(
                            div()
                                .px_2()
                                .pb_1()
                                .child(Label::new(strings.link_other_parent).size(LabelSize::XSmall).color(Color::Muted)),
                        ),
                )
            })
            .child(
                v_scroll(
                    "link-parent-list-scroll",
                    v_flex().id("link-parent-list").children(candidates.into_iter().map(|candidate| {
                        let parent = candidate.id;
                        let list = list_name(candidate.list_id);
                        ListItem::new(SharedString::from(format!("link-{parent}")))
                            .start_slot(Icon::new(IconName::Circle).size(IconSize::Small).color(Color::Muted))
                            .child(Label::new(candidate.title.clone()).truncate())
                            .end_slot(Label::new(list).size(LabelSize::XSmall).color(Color::Muted))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.dismiss_popup(window, cx);
                                this.link_current_parent(parent, cx);
                            }))
                    })),
                )
                .max_h(px(320.)),
            )
            .into_any_element()
    }

    pub(super) fn render_priority_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.session.task.as_ref().map(|t| t.priority).unwrap_or_default();
        let items = [
            (Priority::High, "priority-high", strings.priority_high),
            (Priority::Medium, "priority-medium", strings.priority_medium),
            (Priority::Low, "priority-low", strings.priority_low),
            (Priority::None, "priority-none", strings.priority_none),
        ];
        popover(cx)
            .min_w(px(190.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .children(items.into_iter().map(|(priority, id, label)| {
                let color = task_rows::priority_color(priority, &theme).unwrap_or(theme.colors().checkbox_border);
                ListItem::new(id)
                    .start_slot(Icon::new(IconName::Flag).size(IconSize::Large).color(color))
                    .child(Label::new(label))
                    .when(priority == current, |item| item.end_slot(check_mark()))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_priority(priority, cx)))
            }))
            .into_any_element()
    }

    /// The bottom bar's list menu, as the original's (200 × 240, opening upwards): a search
    /// field, then the Inbox and the lists, the task's own in the accent color; a list with
    /// sections has a chevron, and hovering it shows its sections beside the menu, the task's
    /// section ticked. A click on a list or a section moves the task there.
    pub(super) fn render_list_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.session.task.as_ref().map(|t| (t.list_id, t.section_id));
        let query = self
            .list_search
            .as_ref()
            .map(|input| input.read(cx).text().to_owned())
            .unwrap_or_default();
        let submenu_bounds = Rc::new(Cell::new(None::<gpui::Bounds<Pixels>>));
        let outside_submenu = submenu_bounds.clone();
        let rows: Vec<(Option<Id>, SharedString, IconName)> = std::iter::once((None, SharedString::new_static(strings.inbox), IconName::Inbox))
            .chain(self.session.lists.iter().map(|l| (Some(l.id), SharedString::from(l.name.clone()), IconName::List)))
            .filter(|(_, name, _)| list_matches(name, &query))
            .collect();
        popover(cx)
            .debug_selector(|| "task-list-popup".into())
            .w(px(200.))
            .h(px(240.))
            .p_0()
            .on_mouse_down_out(cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                if !outside_submenu.get().is_some_and(|bounds| bounds.contains(&event.position)) {
                    this.dismiss_popup(window, cx);
                }
            }))
            .child(
                h_flex()
                    .h(px(39.))
                    .flex_none()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(Icon::new(IconName::Search).color(Color::Muted))
                    .when_some(self.list_search.clone(), |el, input| el.child(div().flex_1().child(input))),
            )
            .child(
                v_scroll(
                    "list-menu-rows-scroll",
                    v_flex().id("list-menu-rows").p_1().children(rows.into_iter().map(|(list, name, icon)| {
                let selected = current.is_some_and(|(l, _)| l == list);
                let color = if selected { Color::Accent } else { Color::Default };
                let sections = self
                    .session
                    .list_sections
                    .get(&list)
                    .cloned()
                    .unwrap_or_default();
                let open = self.list_submenu == Some(list) && !sections.is_empty();
                let has_sections = !sections.is_empty();
                let row_id = format!("move-to-{}", list.map_or("inbox".to_owned(), |id| id.to_string()));
                div()
                    .id(SharedString::from(row_id.clone()))
                    .debug_selector(move || row_id)
                    .relative()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.list_submenu != Some(list) {
                            this.list_submenu = Some(list);
                            cx.notify();
                        }
                    }))
                    .child(
                        ListItem::new("row")
                            .height(px(33.))
                            .toggle_state(open)
                            .start_slot(Icon::new(icon).color(color))
                            .child(Label::new(name).color(color).truncate())
                            .when(has_sections, |item| item.end_slot(Icon::new(if self.sticky_style.is_some() { IconName::ChevronLeft } else { IconName::ChevronRight }).size(IconSize::Small).color(Color::Muted)))
                            .on_click(cx.listener(move |this, _, window, cx| this.set_list(list, window, cx))),
                    )
                    .when(open, |el| {
                        let current_section = current.and_then(|(l, s)| if l == list { s } else { None });
                        let submenu_bounds = submenu_bounds.clone();
                        el.child(PopoverAttachment::new(
                            if self.sticky_style.is_some() { Placement::LeftSide } else { Placement::RightSide },
                            popover(cx).debug_selector(|| "task-list-sections".into()).relative().min_w(px(162.))
                            .child(gpui::canvas(move |area, _, _| submenu_bounds.set(Some(area)), |_, _, _, _| {})
                                .absolute().inset_0())
                            .children(sections.into_iter().map(|section| {
                                let section_id = section.id;
                                let ticked = current_section == Some(section_id);
                                let color = if ticked { Color::Accent } else { Color::Default };
                                ListItem::new(SharedString::from(format!("move-to-section-{section_id}")))
                                    .height(px(34.))
                                    .start_slot(Icon::new(IconName::PanelBottom).color(color))
                                    .child(Label::new(section.name).color(color).truncate())
                                    .when(ticked, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if !this.edit_draft(cx, |task, draft| {
                                            task.list_id = list; task.section_id = Some(section_id); draft.list_changed = true;
                                        }) { this.save(cx, |store, task| {
                                            store.set_list(task, list)?;
                                            store.set_section(task, Some(section_id))
                                        }); }
                                        this.dismiss_popup(window, cx);
                                    }))
                            })),
                        ))
                    })
            })),
                )
                .flex_1()
                .min_h_0()
                .compact(),
            )
            .into_any_element()
    }

    pub(super) fn render_tags_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let strings = i18n::t(cx);
        let Some(draft) = &self.session.tag_draft else { return div().into_any_element() };
        let query = self.tag_query(cx).to_owned();
        let folded = query.to_lowercase();
        let mut names: Vec<&str> = self.session.all_tags.iter().map(|tag| tag.name.as_str()).collect();
        for name in &draft.picked {
            if !names.iter().any(|known| same_tag(known, name)) {
                names.push(name);
            }
        }
        let offer_new = !query.is_empty() && !names.iter().any(|name| same_tag(name, &query));
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div().id(id).flex_1().h(px(28.)).flex().items_center().justify_center()
                .rounded(px(6.)).cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().ghost_element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        popover(cx)
            .debug_selector(|| "task-tag-picker".to_owned())
            .w(px(240.))
            .gap_1()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.dismiss_popup(window, cx);
            }))
            .child(
                h_flex()
                    .h(px(30.))
                    .px_2()
                    .gap_2()
                    .rounded(px(4.))
                    .bg(theme.colors().background)
                    .child(Icon::new(IconName::Search).size(IconSize::Small).color(Color::Muted))
                    .child(div().flex_1().min_w_0().child(self.tag_input.clone())),
            )
            .child(
                v_scroll(
                    "task-tag-options-scroll",
                    v_flex()
                        .id("task-tag-options")
                        .children(names.into_iter().filter(|name| name.to_lowercase().contains(&folded)).map(|name| {
                            let name = name.to_owned();
                            let checked = draft.picked.iter().any(|picked| same_tag(picked, &name));
                            ListItem::new(SharedString::from(format!("tag-option-{name}")))
                                .start_slot(Icon::new(IconName::Tag).size(IconSize::Small).color(self.tag_color(&name, cx)))
                                .child(Label::new(name.clone()).truncate())
                                .when(checked, |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_picked_tag(name.clone(), cx)))
                        }))
                        .when(offer_new, |el| el.child(ListItem::new("task-tag-create")
                            .start_slot(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Accent))
                            .child(Label::new(format!("{}: {query}", strings.new_tag)).truncate())
                            .on_click(cx.listener(|this, _, window, cx| this.pick_typed_tag(window, cx))))),
                )
                .max_h(px(200.))
                .compact(),
            )
            .when(draft.save_failed, |el| el.child(Label::new(strings.task_save_failed).color(Color::Error)))
            .child(h_flex().gap_2().p_1()
                .child(button("task-tags-cancel", strings.cancel, false).on_click(cx.listener(|this, _, window, cx| {
                    this.dismiss_popup(window, cx);
                })))
                .child(button("task-tags-ok", strings.ok, true).debug_selector(|| "task-tags-ok".to_owned())
                    .on_click(cx.listener(|this, _, window, cx| this.apply_tags(window, cx)))))
            .into_any_element()
    }
}
