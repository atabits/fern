use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::Local;
use fern_core::{Id, Kind, Priority, Task};
use gpui::{Entity, FocusHandle, Focusable, WeakFocusHandle};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use crate::calendar;
use crate::due_picker::{self, DueDraft};
use crate::i18n::{self, Strings};
use crate::lists_panel;
use crate::task_rows;
use crate::text_input::TextInput;

use super::{
    EstimateDraft, Popup, PopupWindow, StartFocus, TagDraft, TaskDetail, TaskUpdated, Undoable,
    check_mark, list_matches, same_tag,
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
                        window.focus(&this.link_search.focus_handle(cx), cx);
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
        let query = self.link_search.read(cx).text().trim().to_lowercase();
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
                    .child(div().flex_1().child(self.link_search.clone())),
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
        let query = self.list_search.read(cx).text().to_owned();
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
                    .child(div().flex_1().child(self.list_search.clone())),
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

    /// Whether one of the details' popovers is open, which a click outside the card closes first.
    pub fn has_popup(&self) -> bool {
        self.session.popup.is_some()
    }

    pub fn sticky_popup_return_focus(&self, window: &Window, cx: &App) -> WeakFocusHandle {
        self.editor_window.popup_return_focus.clone().unwrap_or_else(|| {
            window
                .focused(cx)
                .filter(|focus| self.is_editor_focus(focus, cx))
                .unwrap_or_else(|| self.title.focus_handle(cx))
                .downgrade()
        })
    }

    pub fn sticky_popup_inputs(&self, cx: &App) -> (Option<FocusHandle>, [Entity<TextInput>; 2]) {
        let input = match self.session.popup {
            Some(Popup::Tags | Popup::InlineTags) => Some(&self.tag_input),
            Some(Popup::List) => Some(&self.list_search),
            Some(Popup::LinkParent) => Some(&self.link_search),
            _ => None,
        };
        (
            input.map(|input| input.focus_handle(cx)),
            [self.tag_input.clone(), self.time_input.clone()],
        )
    }

    pub fn bind_popup_window(&self, window: &Window, cx: &mut Context<Self>) -> PopupWindow {
        PopupWindow::new(cx.weak_entity(), &self.tag_input, &self.time_input, window, cx)
    }

    /// The same menus and drafts, hosted outside the small native note window.
    pub fn render_sticky_popup(&mut self, available: gpui::Size<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        match self.session.popup {
            Some(Popup::Due) => self.render_due_popup_in(available, strings, cx),
            Some(Popup::More) => self.render_more_popup_in(Some(available.height), strings, cx),
            Some(Popup::Priority) => self.render_priority_popup(strings, cx),
            Some(Popup::List) => self.render_list_popup(strings, cx),
            Some(Popup::Tags | Popup::InlineTags) => self.render_tags_popup(cx),
            Some(Popup::Templates) => self.render_draft_templates(cx),
            Some(Popup::Estimate) => self.render_estimate(strings, cx),
            Some(Popup::LinkParent) => self.render_link_parent(strings, cx),
            None => div().into_any_element(),
        }
    }

    /// Reserve the side-menu area before it opens. Resizing a native popup on hover
    /// replaces its render surface and causes a visible flash, even for a single frame.
    pub fn sticky_submenu_space(&self) -> Pixels {
        match self.session.popup {
            Some(Popup::More) => px(200.),
            Some(Popup::List) => px(180.),
            _ => px(0.),
        }
    }

    fn link_parent_candidates(&self) -> Vec<Task> {
        let Some(task) = self.session.task.as_ref() else {
            return Vec::new();
        };
        let all_tasks = self.session.store.tasks().unwrap_or_default();
        let tree = fern_core::TaskTree::build(&all_tasks);
        let task_depth = tree.depth(task.id);
        let descendants = tree.descendants(task.id);
        let subtree_height = descendants
            .iter()
            .map(|desc| tree.depth(desc.id).saturating_sub(task_depth))
            .max()
            .unwrap_or(0);
        let excluded: HashSet<Id> = descendants
            .iter()
            .map(|desc| desc.id)
            .chain([task.id])
            .collect();
        let depth_by_id: HashMap<Id, usize> = all_tasks
            .iter()
            .map(|t| (t.id, tree.depth(t.id)))
            .collect();
        drop(descendants);
        drop(tree);
        all_tasks
            .into_iter()
            .filter(|t| {
                let cand_depth = depth_by_id.get(&t.id).copied().unwrap_or(0);
                !t.is_closed()
                    && !t.is_deleted()
                    && t.kind != Kind::Note
                    && !excluded.contains(&t.id)
                    && task.parent_id != Some(t.id)
                    && cand_depth + 1 + subtree_height <= fern_core::MAX_SUBTASK_DEPTH
            })
            .collect()
    }

    pub(super) fn open_popup(&mut self, popup: Popup, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.focus_submenu = false;
        self.editor_window.popup_return_focus = None;
        self.session.tag_draft = None;
        match popup {
            Popup::Due => {
                self.session.preferences = self.session.store.preferences().unwrap_or_default();
                let today = Local::now().date_naive();
                self.tz_input.update(cx, |input, cx| input.set_text("", cx));
                self.session.due_draft = self.session.task.as_ref().map(|task| self.due_preview(task, today));
            }
            Popup::Tags | Popup::InlineTags => {
                self.session.all_tags = self.session.store.tags().unwrap_or_default();
                let original = self.task_tags();
                self.session.tag_draft = Some(TagDraft {
                    picked: original.clone(),
                    original,
                    save_failed: false,
                });
                self.tag_input.update(cx, |input, cx| input.set_text("", cx));
            }
            Popup::List => {
                self.list_search.update(cx, |input, cx| input.set_text("", cx));
                self.list_submenu = None;
                self.session.list_sections.clear();
                for list_id in std::iter::once(None).chain(self.session.lists.iter().map(|l| Some(l.id))) {
                    let sections = self.session.store.list_sections(list_id).unwrap_or_default();
                    self.session.list_sections.insert(list_id, sections);
                }
            }
            Popup::Templates => {
                self.session.draft_templates = self.session.store.templates().unwrap_or_default();
            }
            Popup::Priority | Popup::More => {}
            Popup::LinkParent => {
                self.link_search.update(cx, |input, cx| input.set_text("", cx));
                self.session.link_parent_candidates = self.link_parent_candidates();
            }
            Popup::Estimate => {
                self.session.estimate_draft =
                    EstimateDraft::from(self.session.task.as_ref().and_then(|t| t.estimate));
            }
        }
        self.session.popup = Some(popup);
        cx.notify();
    }

    pub(super) fn open_popup_from_ui(&mut self, popup: Popup, window: &Window, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let return_focus = if self.popup_input_focused(window, cx) {
            self.editor_window.popup_return_focus.clone()
        } else {
            window.focused(cx).map(|focus| focus.downgrade())
        };
        self.open_popup(popup, cx);
        self.editor_window.popup_return_focus = return_focus;
    }

    pub(super) fn open_tags(&mut self, popup: Popup, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.open_popup_from_ui(popup, window, cx);
        window.focus(&self.tag_input.focus_handle(cx), cx);
    }

    /// Settings → Task Default → Default Duration, in minutes.
    pub(super) fn default_duration(&self) -> i64 {
        self.session.preferences.task_defaults.duration
    }

    /// The details' own pickers, for the task list's right-click menu: `subtask` focuses the Add
    /// Subtask field, `due`, `tags` and `link-parent` open their popovers.
    pub fn start(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.task.is_none() || self.is_read_only() {
            return;
        }
        match what {
            "subtask" if self.can_add_subtask() => {
                self.subtask_open = true;
                window.focus(&self.subtask_input.focus_handle(cx), cx);
            }
            "due" => self.open_popup_from_ui(Popup::Due, window, cx),
            "tags" => self.open_tags(Popup::Tags, window, cx),
            "link-parent" if self.can_link_parent() => {
                self.open_popup_from_ui(Popup::LinkParent, window, cx);
                window.focus(&self.link_search.focus_handle(cx), cx);
            }
            _ => {}
        }
    }

    /// Opens the due-date picker (the "Set date" shortcut).
    pub fn open_due(&mut self, cx: &mut Context<Self>) {
        if self.session.task.is_some() && !self.is_read_only() {
            self.open_popup(Popup::Due, cx);
        }
    }

    #[cfg(debug_assertions)]
    pub fn dev_open_popup(&mut self, popup: &str, cx: &mut Context<Self>) {
        use fern_core::{Frequency, Recurrence};
        use crate::due_picker::{CustomReminder, DueMenu};

        let popup = match popup {
            "due" => Popup::Due,
            "priority" => Popup::Priority,
            "tags" => Popup::Tags,
            "list" => Popup::List,
            // The list menu with the task's list's sections open beside it.
            "list-sections" => {
                self.open_popup(Popup::List, cx);
                self.list_submenu = Some(self.session.task.as_ref().and_then(|t| t.list_id));
                return;
            }
            "more" => Popup::More,
            // The "…" menu with its Start Focus side menu open.
            "more-focus" => {
                self.open_popup(Popup::More, cx);
                self.focus_submenu = true;
                return;
            }
            "link-parent" => Popup::LinkParent,
            "estimate" => Popup::Estimate,
            "checklist" => {
                self.start_checklist(cx);
                return;
            }
            // The due-date popover with its time menu or the custom repeat editor open.
            "due-reminder" | "due-reminder-custom" => {
                self.open_popup(Popup::Due, cx);
                let custom = popup == "due-reminder-custom";
                self.update_draft(cx, |d| {
                    d.menu = Some(if custom {
                        DueMenu::CustomReminder(CustomReminder::new(d.time.is_some()))
                    } else {
                        DueMenu::Reminder
                    })
                });
                return;
            }
            "activity" => {
                self.open_activity(cx);
                return;
            }
            "due-duration" => {
                self.open_popup(Popup::Due, cx);
                let duration = self.default_duration();
                self.update_draft(cx, |d| d.start_duration(Local::now().naive_local(), duration));
                return;
            }
            // The custom repeat's monthly "Each" with the 1st, 15th and last day picked.
            "due-custom-monthly" => {
                self.open_popup(Popup::Due, cx);
                self.update_draft(cx, |d| {
                    let mut rule = Recurrence::new(Frequency::Monthly, 1);
                    rule.month_days = 1 | 1 << 14 | fern_core::recurrence::LAST_DAY;
                    d.menu = Some(DueMenu::Custom(rule));
                });
                return;
            }
            // The custom repeat's monthly "Workday", daily "Skip Weekends" and "By Completion Date".
            "due-custom-workday" | "due-custom-skip" | "due-custom-completion" | "due-custom-specific" => {
                self.open_popup(Popup::Due, cx);
                self.update_draft(cx, |d| {
                    let rule = match popup {
                        "due-custom-workday" => Recurrence {
                            nth_workday: Some(1),
                            ..Recurrence::new(Frequency::Monthly, 1)
                        },
                        "due-custom-skip" => Recurrence {
                            skip_weekends: true,
                            ..Recurrence::new(Frequency::Daily, 1)
                        },
                        "due-custom-specific" => {
                            let today = Local::now().date_naive();
                            Recurrence {
                                dates: vec![today, today + chrono::Duration::days(13)],
                                ..Recurrence::new(Frequency::Daily, 1)
                            }
                        }
                        _ => Recurrence {
                            from_completion: true,
                            ..Recurrence::new(Frequency::Weekly, 1)
                        },
                    };
                    d.menu = Some(DueMenu::Custom(rule));
                });
                return;
            }
            "due-time" | "due-custom" => {
                self.open_popup(Popup::Due, cx);
                let today = Local::now().date_naive();
                let custom = popup == "due-custom";
                self.update_draft(cx, |d| {
                    d.menu = Some(if custom {
                        DueMenu::Custom(due_picker::custom_start(d.repeat.clone(), d.date.unwrap_or(today)))
                    } else {
                        DueMenu::Time
                    })
                });
                return;
            }
            _ => return,
        };
        self.open_popup(popup, cx);
    }

    pub(super) fn close_popup(&mut self, cx: &mut Context<Self>) {
        self.session.popup = None;
        self.editor_window.popup_return_focus = None;
        self.session.tag_draft = None;
        self.focus_submenu = false;
        cx.notify();
    }

    fn popup_input_focused(&self, window: &Window, cx: &App) -> bool {
        match self.session.popup {
            Some(Popup::Tags | Popup::InlineTags) => self.tag_input.focus_handle(cx).is_focused(window),
            Some(Popup::LinkParent) => self.link_search.focus_handle(cx).is_focused(window),
            Some(Popup::Due) => {
                self.time_input.focus_handle(cx).is_focused(window)
                    || self.tz_input.focus_handle(cx).is_focused(window)
            }
            Some(Popup::List) => self.list_search.focus_handle(cx).is_focused(window),
            _ => false,
        }
    }

    /// Dismisses a popover even if focus is still in the task list or calendar.
    pub fn dismiss_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.session.popup.is_none() {
            return false;
        }
        if self.popup_input_focused(window, cx) {
            let focus = self
                .editor_window
                .popup_return_focus
                .take()
                .and_then(|focus| focus.upgrade())
                .unwrap_or_else(|| self.title.focus_handle(cx));
            window.focus(&focus, cx);
        }
        self.close_popup(cx);
        true
    }

    pub(super) fn update_draft(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut DueDraft)) {
        if let Some(draft) = self.session.due_draft.as_mut() {
            let was_timed = draft.time.is_some();
            let had_date = draft.date.is_some();
            change(draft);
            let timed = draft.time.is_some();
            if !draft.reminders_changed
                && let Some(task_draft) = &self.session.draft
                && (was_timed != timed || had_date != draft.date.is_some())
            {
                draft.reminders = task_draft
                    .default_reminder(timed)
                    .filter(|_| draft.date.is_some())
                    .into_iter()
                    .collect();
            }
            cx.notify();
        }
    }

    /// "OK": saves the draft's date, time, timezone, reminders, and repeat.
    pub(super) fn apply_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = self.session.due_draft.take() {
            let today = Local::now().date_naive();
            let changed = self.session.task.as_ref().is_some_and(|task| {
                let before = self.due_preview(task, today);
                draft.date != before.date
                    || draft.time != before.time
                    || draft.end() != before.end()
                    || draft.repeat != before.repeat
                    || draft.timezone != before.timezone
                    || draft.is_floating != before.is_floating
                    || draft.constant_reminder != before.constant_reminder
                    || draft.horizon != before.horizon
            });
            let reminders: Vec<i64> = if draft.date.is_some() {
                draft.reminders.clone()
            } else {
                Vec::new()
            };
            let constant = draft.constant_reminder && !reminders.is_empty();
            let target_horizon = if draft.date.is_some() { None } else { draft.horizon };
            if !self.edit_draft(cx, |task, state| {
                if changed {
                    task.due_date = draft.date;
                    task.due_time = draft.date.and(draft.time);
                    task.end_date = draft.end().map(|(date, _)| date);
                    task.end_time = draft.end().and_then(|(_, time)| time);
                    task.repeat = draft.repeat.clone();
                    task.timezone = draft.timezone.clone();
                    task.is_floating = draft.is_floating;
                    task.horizon = target_horizon;
                    state.date_changed = true;
                }
                task.reminders = reminders.clone();
                task.constant_reminder = constant;
                state.reminders_changed = draft.reminders_changed;
            }) {
                self.save(cx, |store, id| {
                    if draft.date.is_some() {
                        store.set_due(id, draft.date, draft.time)?;
                        store.set_due_end(id, draft.end())?;
                        store.set_timezone(id, draft.timezone.as_deref(), draft.is_floating)?;
                        store.set_reminders(id, &reminders)?;
                        store.set_task_constant_reminder(id, constant)?;
                        store.set_repeat(id, draft.repeat, today)
                    } else {
                        store.set_due(id, None, None)?;
                        store.set_due_end(id, None)?;
                        store.set_timezone(id, None, false)?;
                        store.set_reminders(id, &[])?;
                        store.set_task_constant_reminder(id, false)?;
                        store.set_horizon(id, target_horizon)?;
                        store.set_repeat(id, draft.repeat, today)
                    }
                });
            } else {
                self.refresh_draft_title(cx);
            }
        }
        self.dismiss_popup(window, cx);
    }

    /// "Clear": removes the due date, time, timezone, reminders, horizon, and repeat.
    pub(super) fn clear_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.due_draft = None;
        let today = Local::now().date_naive();
        if !self.edit_draft(cx, |task, state| {
            task.due_date = None;
            task.due_time = None;
            task.end_date = None;
            task.end_time = None;
            task.timezone = None;
            task.is_floating = false;
            task.constant_reminder = false;
            task.horizon = None;
            task.reminders.clear();
            task.repeat = None;
            state.date_changed = true;
            state.reminders_changed = false;
        }) {
            self.save(cx, |store, id| {
                store.set_due(id, None, None)?;
                store.set_due_end(id, None)?;
                store.set_timezone(id, None, false)?;
                store.set_reminders(id, &[])?;
                store.set_task_constant_reminder(id, false)?;
                store.set_horizon(id, None)?;
                store.set_repeat(id, None, today)
            });
        } else {
            self.refresh_draft_title(cx);
        }
        self.dismiss_popup(window, cx);
    }

    pub(super) fn set_priority(&mut self, priority: Priority, cx: &mut Context<Self>) {
        if !self.edit_draft(cx, |task, draft| {
            task.priority = priority;
            draft.priority_changed = true;
        }) {
            self.save(cx, |store, id| store.set_priority(id, priority));
        }
        self.close_popup(cx);
    }

    pub(super) fn set_list(&mut self, list: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.edit_draft(cx, |task, draft| {
            task.list_id = list;
            task.section_id = None;
            draft.list_changed = true;
        }) {
            self.save(cx, |store, id| store.set_list(id, list));
        }
        self.dismiss_popup(window, cx);
    }

    pub(super) fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.update_draft(cx, |d| d.month = calendar::add_months(d.month, delta));
    }

    pub(super) fn task_tags(&self) -> Vec<String> {
        self.session.task.as_ref().map(|t| t.tags.clone()).unwrap_or_default()
    }

    fn tag_query<'a>(&self, cx: &'a App) -> &'a str {
        self.tag_input.read(cx).text().trim().trim_start_matches('#').trim()
    }

    pub fn pick_typed_time(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(time) = due_picker::parse_time(text) {
            self.update_draft(cx, |d| d.pick_time(Some(time), Local::now().date_naive()));
            self.time_input.update(cx, |input, cx| input.clear(window, cx));
        }
    }

    pub fn pick_typed_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.tag_query(cx).to_owned();
        if query.is_empty() {
            return;
        }
        let name = self
            .session
            .all_tags
            .iter()
            .find(|tag| same_tag(&tag.name, &query))
            .map_or(query, |tag| tag.name.clone());
        if let Some(draft) = &mut self.session.tag_draft {
            if !draft.picked.iter().any(|picked| same_tag(picked, &name)) {
                draft.picked.push(name);
            }
            draft.save_failed = false;
            self.tag_input.update(cx, |input, cx| input.clear(window, cx));
            cx.notify();
        }
    }

    fn toggle_picked_tag(&mut self, name: String, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.session.tag_draft {
            if let Some(index) = draft.picked.iter().position(|picked| same_tag(picked, &name)) {
                draft.picked.remove(index);
            } else {
                draft.picked.push(name);
            }
            draft.save_failed = false;
            cx.notify();
        }
    }

    pub(super) fn apply_tags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.draft.is_some() {
            if let Some(tags) = self.session.tag_draft.as_ref().map(|draft| draft.picked.clone()) {
                self.edit_draft(cx, |task, _| task.tags = tags);
            }
            self.dismiss_popup(window, cx);
            return;
        }
        let (Some(id), Some(draft)) = (self.task_id(), &mut self.session.tag_draft) else { return };
        let contains = |tags: &[String], name: &str| tags.iter().any(|tag| same_tag(tag, name));
        let save = (|| -> anyhow::Result<bool> {
            let Some(mut task) = self.session.store.task(id)?.filter(|task| !task.is_deleted()) else {
                anyhow::bail!("cannot save tags for a missing or deleted task");
            };
            // Apply the user's changes to the current tags, which may have synced since opening.
            let before = task.tags.clone();
            task.tags.retain(|tag| !contains(&draft.original, tag) || contains(&draft.picked, tag));
            for tag in &draft.picked {
                if !contains(&draft.original, tag) && !contains(&task.tags, tag) {
                    task.tags.push(tag.clone());
                }
            }
            if task.tags == before {
                return Ok(false);
            }
            self.session.store.set_tags(id, &task.tags)?;
            Ok(true)
        })();
        match save {
            Ok(changed) => {
                self.dismiss_popup(window, cx);
                if changed {
                    cx.emit(TaskUpdated);
                }
            }
            Err(error) => {
                eprintln!("failed to save task tags: {error:#}");
                draft.save_failed = true;
                cx.notify();
            }
        }
    }

    pub(super) fn tag_color(&self, name: &str, cx: &App) -> Hsla {
        let color = self
            .session
            .all_tags
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.color.as_deref());
        lists_panel::color(color, cx)
    }
}
