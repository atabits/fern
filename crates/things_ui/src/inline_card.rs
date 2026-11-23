//! Signature Things 3 inline-expanding task card (compact collapsed row + elevated editing card).

use std::collections::HashMap;

use chrono::NaiveDate;
use desktop_common::{calendar, completion_mark};
use desktop_common::text_input::TextInput;
use fern_core::things::ThingsViewKind;
use fern_core::{Id, List, ListFolder, ScheduleBucket, Tag, Task};
use fern_i18n::Strings;
use gpui::{AnyElement, Context, Entity, FontWeight, Hsla, Subscription, Window, div, px};
use task_ui::drag::DraggedTask;
use task_ui::{lists_panel, task_rows};
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Checkbox, Chip, Divider, Icon, IconButton, IconName,
    IconSize, Label, LabelSize, ListItem, Placement, PopoverAttachment, Tooltip, popover,
    v_scroll,
};

use crate::bottom_bar;
use crate::magic_plus::DraggedMagicPlus;
use crate::when_picker;

/// Which popover is anchored to the currently expanded inline card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardPopover {
    When,
    Deadline,
    Tags,
    Move,
}

/// One editable checklist item inside an expanded inline card.
pub struct ChecklistRowState {
    pub id: Id,
    pub completed: bool,
    pub title_text: String,
    pub input: Entity<TextInput>,
    pub _subscriptions: Vec<Subscription>,
}

/// Persistent entity state for the single expanded inline card.
pub struct ExpandedCardState {
    pub task_id: Id,
    pub title: Entity<TextInput>,
    pub notes: Entity<TextInput>,
    pub checklist_rows: Vec<ChecklistRowState>,
    pub new_checklist_item: Entity<TextInput>,
    pub show_checklist: bool,
    pub tag_input: Entity<TextInput>,
    pub active_popover: Option<CardPopover>,
    /// True when the task was freshly inserted empty via `+ New To-Do` / Magic Plus
    /// and has not yet been mutated or given content.
    pub created_empty: bool,
    pub _subscriptions: Vec<Subscription>,
}

/// Actions emitted by collapsed rows or the expanded inline card.
#[derive(Clone, Debug)]
pub enum CardAction {
    Select(Id),
    Expand(Id),
    Collapse,
    ToggleComplete(Id),
    Delete(Id),
    StartFocus(Id, String, bool),
    ToggleChecklist,
    ToggleChecklistItem(Id, bool),
    DeleteChecklistItem(Id),
    ToggleTag(String),
    RemoveTag(String),
    OpenTagDialog,
    TogglePopover(CardPopover),
    ClosePopover,
    MoveTo(Option<Id>),
    MagicDropAfter(Id),
    ReorderAfter { dragged_id: Id, target_id: Id },
}

pub struct CollapsedRowProps<'a> {
    pub task: &'a Task,
    pub view: ThingsViewKind,
    pub selected: bool,
    pub today: NaiveDate,
    pub list_names: &'a HashMap<Id, String>,
    pub tag_colors: &'a HashMap<String, Hsla>,
    pub strings: &'static Strings,
}

/// Renders a compact collapsed task row with checkbox, title, and subtle metadata chips.
pub fn render_collapsed_row<V: 'static>(
    props: CollapsedRowProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, CardAction, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let theme = cx.theme().clone();
    let colors = theme.colors().clone();
    let status = theme.status().clone();
    let task = props.task;
    let id = task.id;
    let strings = props.strings;
    let closed = task.is_closed();
    let display_title: SharedString = if task.title.trim().is_empty() {
        strings.untitled.into()
    } else {
        task.title.clone().into()
    };

    let priority_tint = task_rows::priority_color(task.priority, &theme);
    let check_act = on_action.clone();
    let row_act = on_action.clone();
    let expand_act = on_action.clone();
    let magic_drop_act = on_action.clone();
    let reorder_drop_act = on_action;

    let evening_chip = (task.schedule_bucket == ScheduleBucket::Evening && !closed).then(|| {
        h_flex()
            .gap_1()
            .px_1p5()
            .h(px(18.))
            .rounded(px(5.))
            .bg(colors.accent.opacity(0.10))
            .child(Icon::new(IconName::Moon).size(IconSize::XSmall).color(Color::Accent))
            .child(Label::new(strings.things_today_evening).size(LabelSize::XSmall).color(Color::Accent))
    });

    let when_chip = if closed
        || matches!(
            props.view,
            ThingsViewKind::Inbox | ThingsViewKind::Today | ThingsViewKind::Someday
        )
        || task.schedule_bucket == ScheduleBucket::Evening
    {
        None
    } else {
        when_picker::format_when_label(task, props.today, strings).map(|label| {
            h_flex()
                .gap_1()
                .px_1p5()
                .h(px(18.))
                .rounded(px(5.))
                .bg(colors.element_hover)
                .child(Icon::new(IconName::CalendarDays).size(IconSize::XSmall).color(Color::Muted))
                .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted))
        })
    };

    let horizon_chip = (calendar::horizons_enabled() && task.due_date.is_none())
        .then(|| {
            task.horizon.map(|h| {
                let label = calendar::format_horizon_badge(h, strings);
                let past = h.is_past_relative_to(props.today);
                let (fg_hsla, bg_hsla, _) = task_rows::horizon_badge_style(past, closed, &theme);
                let fg = if past && !closed { Color::Custom(fg_hsla) } else { Color::Muted };
                let bg = if past && !closed { bg_hsla } else { colors.element_hover };
                h_flex()
                    .when(cfg!(test), |el| {
                        el.debug_selector(move || format!("things-horizon-chip-{id}"))
                    })
                    .gap_1()
                    .px_1p5()
                    .h(px(18.))
                    .rounded(px(5.))
                    .bg(bg)
                    .child(
                        Icon::new(IconName::FernHorizons)
                            .size(IconSize::XSmall)
                            .color(fg),
                    )
                    .child(Label::new(label).size(LabelSize::XSmall).color(fg))
            })
        })
        .flatten();

    let due_chip = task.due_date.map(|due| {
        let (label, overdue) = task_rows::due_label(due, task.due_time, props.today, strings);
        let overdue = overdue && !closed;
        let fg = if overdue { Color::Error } else { Color::Muted };
        let bg = if overdue {
            status.error.opacity(0.12)
        } else {
            colors.element_hover
        };
        h_flex()
            .gap_1()
            .px_1p5()
            .h(px(18.))
            .rounded(px(5.))
            .bg(bg)
            .child(Icon::new(IconName::Flag).size(IconSize::XSmall).color(fg))
            .child(Label::new(label).size(LabelSize::XSmall).color(fg))
    });

    let project_chip = if matches!(props.view, ThingsViewKind::Project(_)) {
        None
    } else {
        task.list_id
            .and_then(|lid| props.list_names.get(&lid))
            .map(|name| {
                div()
                    .when(cfg!(test), |el| {
                        el.debug_selector(move || format!("things-project-chip-{id}"))
                    })
                    .max_w(px(120.))
                    .overflow_hidden()
                    .px_1p5()
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .bg(colors.element_hover)
                    .child(
                        Label::new(name.clone())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted)
                            .truncate(),
                    )
            })
    };

    let drag_payload = DraggedTask {
        id,
        title: display_title.clone(),
    };
    let allow_row_drop = props.view != ThingsViewKind::Logbook && !closed;
    let group_name = SharedString::from(format!("things-row-group-{id}"));

    div()
        .id(SharedString::from(format!("things-row-{id}")))
        .when(cfg!(test), |el| el.debug_selector(move || format!("things-row-{id}")))
        .group(group_name.clone())
        .min_h(px(36.))
        .px_3()
        .py_1p5()
        .rounded(px(8.))
        .flex()
        .items_center()
        .gap_2p5()
        .cursor_pointer()
        .when(props.selected, |el| el.bg(colors.element_selected))
        .when(!props.selected, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
        .when(allow_row_drop, |el| {
            el.drag_over::<DraggedMagicPlus>(move |style, _, _, cx| {
                style.border_b_2().border_color(cx.theme().colors().accent)
            })
            .on_drop(cx.listener(move |this, _: &DraggedMagicPlus, window, cx| {
                magic_drop_act(this, CardAction::MagicDropAfter(id), window, cx);
            }))
            .drag_over::<DraggedTask>(move |style, _, _, cx| {
                style
                    .border_b_2()
                    .border_color(cx.theme().colors().accent.opacity(0.75))
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, window, cx| {
                if dragged.id != id {
                    reorder_drop_act(
                        this,
                        CardAction::ReorderAfter {
                            dragged_id: dragged.id,
                            target_id: id,
                        },
                        window,
                        cx,
                    );
                }
            }))
        })
        .on_drag(drag_payload, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
        .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
            cx.stop_propagation();
            if event.click_count() >= 2 {
                row_act(this, CardAction::Expand(id), window, cx);
            } else {
                row_act(this, CardAction::Select(id), window, cx);
            }
        }))
        .child(
            div()
                .when(cfg!(test), |el| el.debug_selector(move || format!("things-check-{id}")))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Checkbox::new(SharedString::from(format!("things-cb-{id}")))
                        .size(px(16.))
                        .toggle_state(task.is_completed())
                        .dismissed(task.is_wont_do())
                        .tint(priority_tint)
                        .animate_completion(completion_mark::started(id, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            check_act(this, CardAction::ToggleComplete(id), window, cx);
                        })),
                ),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .child(
                    div().min_w_0().child(
                        Label::new(display_title)
                            .size(LabelSize::Default)
                            .strikethrough(closed)
                            .color(if closed || task.title.trim().is_empty() {
                                Color::Muted
                            } else {
                                Color::Default
                            })
                            .truncate(),
                    ),
                )
                .when(!task.notes.trim().is_empty(), |el| {
                    el.child(Icon::new(IconName::FileText).size(IconSize::XSmall).color(Color::IconMuted))
                })
                .when_some(task.checklist, |el, (done, total)| {
                    el.child(
                        h_flex()
                            .gap_0p5()
                            .child(Icon::new(IconName::ListChecks).size(IconSize::XSmall).color(Color::IconMuted))
                            .child(
                                Label::new(format!("{done}/{total}"))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                })
                .children(task.tags.iter().take(2).map(|tag| {
                    let color = props
                        .tag_colors
                        .get(&tag.to_lowercase())
                        .copied()
                        .unwrap_or(colors.text_muted);
                    Chip::new(tag.clone(), color)
                })),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_1p5()
                .children(evening_chip)
                .children(when_chip)
                .children(horizon_chip)
                .children(due_chip)
                .children(project_chip)
                .child(
                    div()
                        .invisible()
                        .group_hover(group_name, |s| s.visible())
                        .child(
                            IconButton::new(SharedString::from(format!("things-expand-btn-{id}")), IconName::ChevronDown)
                                .size(px(22.))
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text(strings.show_details))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    expand_act(this, CardAction::Expand(id), window, cx);
                                })),
                        ),
                ),
        )
        .into_any_element()
}

pub struct ExpandedCardProps<'a> {
    pub task: &'a Task,
    pub state: &'a ExpandedCardState,
    pub today: NaiveDate,
    pub lists: &'a [List],
    pub folders: &'a [ListFolder],
    pub all_tags: &'a [Tag],
    pub tag_colors: &'a HashMap<String, Hsla>,
    pub strings: &'static Strings,
    pub when_popover: Option<AnyElement>,
    pub deadline_popover: Option<AnyElement>,
}

/// Renders the signature elevated inline-expanding card for `props.task`.
pub fn render_expanded_card<V: 'static>(
    props: ExpandedCardProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, CardAction, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let theme = cx.theme().clone();
    let colors = theme.colors().clone();
    let status = theme.status().clone();
    let task = props.task;
    let id = task.id;
    let state = props.state;
    let strings = props.strings;
    let closed = task.is_closed();
    let priority_tint = task_rows::priority_color(task.priority, &theme);

    let act_check = on_action.clone();
    let act_collapse = on_action.clone();
    let act_checklist_toggle = on_action.clone();
    let act_when = on_action.clone();
    let act_deadline = on_action.clone();
    let act_tags = on_action.clone();
    let act_move = on_action.clone();
    let act_focus = on_action.clone();
    let act_done_btn = on_action.clone();
    let act_delete = on_action.clone();
    let act_close_pop = on_action.clone();
    let act_move_pick = on_action.clone();

    let horizon_when = (calendar::horizons_enabled() && task.due_date.is_none())
        .then_some(task.horizon)
        .flatten();
    let when_label = when_picker::format_when_label(task, props.today, strings)
        .or_else(|| horizon_when.map(|h| calendar::format_horizon_badge(h, strings)))
        .unwrap_or_else(|| strings.things_when.to_owned());
    let has_when = task.start_date.is_some()
        || task.schedule_bucket != ScheduleBucket::None
        || horizon_when.is_some();
    let when_icon = match task.schedule_bucket {
        ScheduleBucket::Evening => IconName::Moon,
        ScheduleBucket::Someday => IconName::Archive,
        ScheduleBucket::None => {
            if task.start_date == Some(props.today) {
                IconName::Sun
            } else if task.start_date.is_none() && horizon_when.is_some() {
                IconName::FernHorizons
            } else {
                IconName::CalendarDays
            }
        }
    };

    let (deadline_label, deadline_overdue) = match task.due_date {
        Some(d) => {
            let (lbl, over) = task_rows::due_label(d, task.due_time, props.today, strings);
            (lbl, over && !closed)
        }
        None => (strings.things_deadline.to_owned(), false),
    };

    let project_label = task
        .list_id
        .and_then(|lid| props.lists.iter().find(|l| l.id == lid))
        .map(|l| l.name.clone())
        .unwrap_or_else(|| strings.things_inbox.to_owned());

    let checklist_visible = state.show_checklist || !state.checklist_rows.is_empty();
    let checklist_elements: Vec<AnyElement> = state
        .checklist_rows
        .iter()
        .map(|row| {
            let item_id = row.id;
            let next_done = !row.completed;
            let toggle_act = on_action.clone();
            let delete_act = on_action.clone();
            h_flex()
                .gap_2()
                .py_0p5()
                .child(
                    Checkbox::new(SharedString::from(format!("things-cl-{item_id}")))
                        .size(px(14.))
                        .toggle_state(row.completed)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            toggle_act(this, CardAction::ToggleChecklistItem(item_id, next_done), window, cx);
                        })),
                )
                .child(div().flex_1().min_w_0().text_size(px(13.)).child(row.input.clone()))
                .child(
                    IconButton::new(SharedString::from(format!("things-cl-del-{item_id}")), IconName::X)
                        .size(px(20.))
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text(strings.delete))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            delete_act(this, CardAction::DeleteChecklistItem(item_id), window, cx);
                        })),
                )
                .into_any_element()
        })
        .collect();

    let tags_popover = (state.active_popover == Some(CardPopover::Tags)).then(|| {
        let query = state.tag_input.read(cx).text().trim().to_lowercase();
        let close_pop = act_close_pop.clone();
        let open_dialog = on_action.clone();
        let tag_items: Vec<AnyElement> = props
            .all_tags
            .iter()
            .filter(|t| query.is_empty() || t.name.to_lowercase().contains(&query))
            .map(|tag| {
                let name = tag.name.clone();
                let name_lower = name.to_lowercase();
                let assigned = task.tags.iter().any(|t| t.to_lowercase() == name_lower);
                let dot = lists_panel::color(tag.color.as_deref(), cx);
                let toggle_act = on_action.clone();
                ListItem::new(SharedString::from(format!("card-tag-opt-{name}")))
                    .height(px(28.))
                    .toggle_state(assigned)
                    .start_slot(div().size(px(8.)).rounded_full().bg(dot))
                    .child(
                        Label::new(name.clone())
                            .size(LabelSize::Small)
                            .color(if assigned { Color::Accent } else { Color::Default })
                            .truncate(),
                    )
                    .when(assigned, |el| {
                        el.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        toggle_act(this, CardAction::ToggleTag(name.clone()), window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        popover(cx)
            .id("things-card-tags-popover")
            .when(cfg!(test), |el| {
                el.debug_selector(|| "things-card-tags-popover".to_owned())
            })
            .w(px(228.))
            .p_2()
            .gap_1p5()
            .on_mouse_down_out(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                close_pop(this, CardAction::ClosePopover, window, cx);
            }))
            .child(
                h_flex()
                    .h(px(28.))
                    .px_2()
                    .gap_1p5()
                    .rounded(px(6.))
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.border_variant)
                    .child(Icon::new(IconName::Tag).size(IconSize::XSmall).color(Color::Muted))
                    .child(div().flex_1().min_w_0().child(state.tag_input.clone())),
            )
            .when(!tag_items.is_empty(), |el| {
                el.child(
                    v_scroll(
                        "things-card-tags-scroll",
                        v_flex().id("things-card-tags-list").gap_0p5().children(tag_items),
                    )
                    .max_h(px(180.))
                    .compact(),
                )
            })
            .child(Divider::horizontal())
            .child(
                Button::new("things-card-new-tag-dialog", strings.add_tags)
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(IconName::Plus)
                    .full_width()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        open_dialog(this, CardAction::OpenTagDialog, window, cx);
                    })),
            )
            .into_any_element()
    });

    let move_popover = (state.active_popover == Some(CardPopover::Move)).then(|| {
        let close_pop = act_close_pop.clone();
        bottom_bar::render_move_popover(
            task.list_id,
            props.lists,
            props.folders,
            strings,
            cx,
            move |this, list_id, window, cx| {
                act_move_pick(this, CardAction::MoveTo(list_id), window, cx);
            },
            move |this, window, cx| {
                close_pop(this, CardAction::ClosePopover, window, cx);
            },
        )
    });

    #[derive(Clone, Copy)]
    enum PillTone {
        Muted,
        Accent,
        Warning,
        Error,
    }

    let pill_button = |id: SharedString,
                       selector: Option<&'static str>,
                       icon: IconName,
                       label: String,
                       tone: PillTone| {
        let text_color = match tone {
            PillTone::Error => Color::Error,
            PillTone::Warning => Color::Warning,
            PillTone::Accent => Color::Accent,
            PillTone::Muted => Color::Muted,
        };
        h_flex()
            .id(id)
            .when_some(selector, |el, sel| {
                el.when(cfg!(test), |el| el.debug_selector(move || sel.to_owned()))
            })
            .max_w(px(160.))
            .h(px(26.))
            .px_2()
            .gap_1()
            .rounded(px(6.))
            .cursor_pointer()
            .when(matches!(tone, PillTone::Error), |el| el.bg(status.error.opacity(0.10)))
            .when(matches!(tone, PillTone::Warning), |el| el.bg(status.warning.opacity(0.12)))
            .when(matches!(tone, PillTone::Accent), |el| el.bg(colors.accent.opacity(0.10)))
            .when(matches!(tone, PillTone::Muted), |el| el.hover(|s| s.bg(colors.element_hover)))
            .child(
                div()
                    .flex_none()
                    .child(Icon::new(icon).size(IconSize::XSmall).color(text_color)),
            )
            .child(
                Label::new(label)
                    .size(LabelSize::XSmall)
                    .color(text_color)
                    .truncate(),
            )
    };

    let when_past_horizon = !closed
        && task.start_date.is_none()
        && task.schedule_bucket == ScheduleBucket::None
        && horizon_when.is_some_and(|h| h.is_past_relative_to(props.today));
    let when_tone = if when_past_horizon {
        PillTone::Warning
    } else if has_when || state.active_popover == Some(CardPopover::When) {
        PillTone::Accent
    } else {
        PillTone::Muted
    };
    let deadline_tone = if deadline_overdue {
        PillTone::Error
    } else if task.due_date.is_some() || state.active_popover == Some(CardPopover::Deadline) {
        PillTone::Accent
    } else {
        PillTone::Muted
    };
    let move_tone = if state.active_popover == Some(CardPopover::Move) {
        PillTone::Accent
    } else {
        PillTone::Muted
    };

    let focus_title = if task.title.trim().is_empty() {
        strings.untitled.to_owned()
    } else {
        task.title.clone()
    };

    v_flex()
        .id(SharedString::from(format!("things-expanded-{id}")))
        .when(cfg!(test), |el| el.debug_selector(move || format!("things-expanded-{id}")))
        .my_2()
        .px_4()
        .py_3()
        .gap_2p5()
        .rounded(px(10.))
        .bg(colors.elevated_surface_background)
        .border_1()
        .border_color(colors.border)
        .shadow_lg()
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(
            h_flex()
                .items_start()
                .gap_2p5()
                .child(
                    div()
                        .pt(px(3.))
                        .when(cfg!(test), |el| el.debug_selector(move || format!("things-check-{id}")))
                        .child(
                            Checkbox::new(SharedString::from(format!("things-exp-cb-{id}")))
                                .size(px(16.))
                                .toggle_state(task.is_completed())
                                .dismissed(task.is_wont_do())
                                .tint(priority_tint)
                                .animate_completion(completion_mark::started(id, cx))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    act_check(this, CardAction::ToggleComplete(id), window, cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .when(cfg!(test), |el| el.debug_selector(move || format!("things-card-title-{id}")))
                        .flex_1()
                        .min_w_0()
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(state.title.clone()),
                )
                .child(
                    IconButton::new(SharedString::from(format!("things-collapse-{id}")), IconName::ChevronUp)
                        .size(px(24.))
                        .icon_size(IconSize::Small)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text(strings.habit_collapse))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            act_collapse(this, CardAction::Collapse, window, cx);
                        })),
                ),
        )
        .child(
            div()
                .pl(px(26.))
                .min_h(px(40.))
                .text_size(px(13.))
                .child(state.notes.clone()),
        )
        .when(checklist_visible, |el| {
            el.child(
                v_flex()
                    .pl(px(26.))
                    .gap_1()
                    .children(checklist_elements)
                    .child(
                        h_flex()
                            .gap_2()
                            .py_0p5()
                            .child(Icon::new(IconName::Plus).size(IconSize::XSmall).color(Color::Muted))
                            .child(div().flex_1().min_w_0().text_size(px(13.)).child(state.new_checklist_item.clone())),
                    ),
            )
        })
        .when(!task.tags.is_empty(), |el| {
            el.child(
                h_flex()
                    .pl(px(26.))
                    .flex_wrap()
                    .gap_1p5()
                    .children(task.tags.iter().map(|tag| {
                        let name = tag.clone();
                        let color = props
                            .tag_colors
                            .get(&tag.to_lowercase())
                            .copied()
                            .unwrap_or(colors.accent);
                        let rem = on_action.clone();
                        Chip::new(name.clone(), color).end_slot(
                            div()
                                .id(SharedString::from(format!("things-tag-rm-{id}-{name}")))
                                .cursor_pointer()
                                .child(Icon::new(IconName::X).size(IconSize::XSmall).color(Color::Muted))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    rem(this, CardAction::RemoveTag(name.clone()), window, cx);
                                })),
                        )
                    })),
            )
        })
        .child(Divider::horizontal())
        .child(
            h_flex()
                .justify_between()
                .gap_2()
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .flex_wrap()
                        .gap_1()
                        .child(
                            div()
                                .relative()
                                .child(
                                    pill_button(
                                        SharedString::from(format!("things-card-when-{id}")),
                                        Some("things-card-when"),
                                        when_icon,
                                        when_label,
                                        when_tone,
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        act_when(this, CardAction::TogglePopover(CardPopover::When), window, cx);
                                    })),
                                )
                                .when_some(props.when_popover, |el, pop| {
                                    el.child(PopoverAttachment::new(Placement::BelowLeft, pop))
                                }),
                        )
                        .child(
                            div()
                                .relative()
                                .child(
                                    pill_button(
                                        SharedString::from(format!("things-card-deadline-{id}")),
                                        Some("things-card-deadline"),
                                        IconName::Flag,
                                        deadline_label,
                                        deadline_tone,
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        act_deadline(this, CardAction::TogglePopover(CardPopover::Deadline), window, cx);
                                    })),
                                )
                                .when_some(props.deadline_popover, |el, pop| {
                                    el.child(PopoverAttachment::new(Placement::BelowLeft, pop))
                                }),
                        )
                        .child(
                            div()
                                .relative()
                                .child(
                                    IconButton::new(SharedString::from(format!("things-card-tags-{id}")), IconName::Tag)
                                        .size(px(26.))
                                        .icon_size(IconSize::Small)
                                        .toggle_state(state.active_popover == Some(CardPopover::Tags))
                                        .tooltip(Tooltip::text(strings.tags))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            act_tags(this, CardAction::TogglePopover(CardPopover::Tags), window, cx);
                                        })),
                                )
                                .when_some(tags_popover, |el, pop| {
                                    el.child(PopoverAttachment::new(Placement::BelowLeft, pop))
                                }),
                        )
                        .child(
                            IconButton::new(SharedString::from(format!("things-card-cl-{id}")), IconName::ListChecks)
                                .size(px(26.))
                                .icon_size(IconSize::Small)
                                .toggle_state(checklist_visible)
                                .tooltip(Tooltip::text(strings.checklist))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    act_checklist_toggle(this, CardAction::ToggleChecklist, window, cx);
                                })),
                        )
                        .child(
                            div()
                                .relative()
                                .child(
                                    pill_button(
                                        SharedString::from(format!("things-card-move-{id}")),
                                        Some("things-card-move"),
                                        IconName::Folder,
                                        project_label,
                                        move_tone,
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        act_move(this, CardAction::TogglePopover(CardPopover::Move), window, cx);
                                    })),
                                )
                                .when_some(move_popover, |el, pop| {
                                    el.child(PopoverAttachment::new(Placement::BelowLeft, pop))
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .when(cfg!(test), |el| {
                            el.debug_selector(|| "things-card-footer-actions".to_owned())
                        })
                        .flex_none()
                        .gap_1()
                        .child(
                            IconButton::new(SharedString::from(format!("things-card-focus-{id}")), IconName::Timer)
                                .size(px(26.))
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text(strings.start_focus))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    act_focus(
                                        this,
                                        CardAction::StartFocus(id, focus_title.clone(), false),
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            IconButton::new(
                                SharedString::from(format!("things-card-done-{id}")),
                                if closed { IconName::RotateCcw } else { IconName::Check },
                            )
                            .size(px(26.))
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text(if closed { strings.reopen } else { strings.mark_done }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                act_done_btn(this, CardAction::ToggleComplete(id), window, cx);
                            })),
                        )
                        .child(
                            IconButton::new(SharedString::from(format!("things-card-del-{id}")), IconName::Trash)
                                .size(px(26.))
                                .icon_size(IconSize::Small)
                                .icon_color(Color::Muted)
                                .hover_icon_color(Color::Error)
                                .tooltip(Tooltip::text(strings.delete))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    act_delete(this, CardAction::Delete(id), window, cx);
                                })),
                        ),
                ),
        )
        .into_any_element()
}
