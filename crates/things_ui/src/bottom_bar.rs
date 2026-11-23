//! Calm Things 3 bottom action bar (`+ New To-Do`, `When`, `Move`, `Quick Find`) and Magic Plus.

use desktop_common::colors;
use fern_core::{Id, List, ListFolder, Task};
use fern_i18n::Strings;
use gpui::{AnyElement, Context, FontWeight, Window, div, px};
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Disableable, Divider, Icon, IconName, IconSize, Label,
    LabelSize, ListItem, Placement, PopoverAttachment, popover, v_scroll,
};

use crate::magic_plus;

/// Actions emitted by the bottom bar and shared move-to-project menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BottomBarAction {
    NewTodo,
    ToggleWhen,
    ToggleMove,
    QuickFind,
    MoveTo(Option<Id>),
    ClosePopover,
}

pub struct BottomBarProps<'a> {
    pub selected_task: Option<&'a Task>,
    pub lists: &'a [List],
    pub folders: &'a [ListFolder],
    pub strings: &'static Strings,
    pub when_popover: Option<AnyElement>,
    pub move_open: bool,
}

/// Renders a clean Move-to-Project popover shared between the bottom bar and expanded inline cards.
pub fn render_move_popover<V: 'static>(
    current_list: Option<Id>,
    lists: &[List],
    folders: &[ListFolder],
    strings: &'static Strings,
    cx: &mut Context<V>,
    on_pick: impl Fn(&mut V, Option<Id>, &mut Window, &mut Context<V>) + Clone + 'static,
    on_close: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let theme_colors = cx.theme().colors().clone();
    let active_lists: Vec<&List> = lists.iter().filter(|l| !l.archived).collect();

    let project_item = |list: &List, indented: bool, cx: &mut Context<V>| {
        let id = list.id;
        let selected = current_list == Some(id);
        let dot_color = colors::parse_color(list.color.as_deref()).unwrap_or(theme_colors.accent);
        let pick = on_pick.clone();
        div()
            .when(indented, |el| el.pl_3())
            .child(
                ListItem::new(SharedString::from(format!("move-proj-{id}")))
                    .height(px(30.))
                    .toggle_state(selected)
                    .start_slot(div().size(px(8.)).rounded_full().bg(dot_color))
                    .child(
                        Label::new(list.name.clone())
                            .size(LabelSize::Small)
                            .color(if selected { Color::Accent } else { Color::Default })
                            .truncate(),
                    )
                    .when(selected, |el| {
                        el.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        pick(this, Some(id), window, cx);
                    })),
            )
            .into_any_element()
    };

    let mut rows: Vec<AnyElement> = Vec::new();
    let inbox_selected = current_list.is_none();
    let pick_inbox = on_pick.clone();
    rows.push(
        ListItem::new("move-inbox")
            .height(px(30.))
            .toggle_state(inbox_selected)
            .start_slot(Icon::new(IconName::Inbox).size(IconSize::Small).color(Color::Info))
            .child(
                Label::new(strings.things_inbox)
                    .size(LabelSize::Small)
                    .color(if inbox_selected { Color::Accent } else { Color::Default }),
            )
            .when(inbox_selected, |el| {
                el.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                pick_inbox(this, None, window, cx);
            }))
            .into_any_element(),
    );

    let standalone: Vec<&List> = active_lists.iter().copied().filter(|l| l.folder.is_none()).collect();
    let folder_groups: Vec<(&ListFolder, Vec<&List>)> = folders
        .iter()
        .filter_map(|folder| {
            let children: Vec<&List> = active_lists
                .iter()
                .copied()
                .filter(|l| l.folder == Some(folder.id))
                .collect();
            (!children.is_empty()).then_some((folder, children))
        })
        .collect();
    if !standalone.is_empty() || !folder_groups.is_empty() {
        rows.push(div().my_1().child(Divider::horizontal()).into_any_element());
    }
    for list in standalone {
        rows.push(project_item(list, false, cx));
    }
    for (folder, children) in folder_groups {
        rows.push(
            div()
                .px_2()
                .pt_1p5()
                .pb_0p5()
                .child(
                    Label::new(folder.name.clone())
                        .size(LabelSize::XSmall)
                        .weight(FontWeight::SEMIBOLD)
                        .color(Color::Muted)
                        .truncate(),
                )
                .into_any_element(),
        );
        for list in children {
            rows.push(project_item(list, true, cx));
        }
    }

    popover(cx)
        .id("things-move-popover")
        .when(cfg!(test), |el| el.debug_selector(|| "things-move-popover".to_owned()))
        .w(px(228.))
        .p_1p5()
        .on_mouse_down_out(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            on_close(this, window, cx);
        }))
        .child(
            div().px_2().py_1().child(
                Label::new(strings.move_to)
                    .size(LabelSize::XSmall)
                    .weight(FontWeight::SEMIBOLD)
                    .color(Color::Muted),
            ),
        )
        .child(
            v_scroll(
                "things-move-scroll",
                v_flex().id("things-move-list").gap_0p5().children(rows),
            )
            .max_h(px(260.))
            .compact(),
        )
        .into_any_element()
}

/// Renders the calm bottom bar with centered actions and draggable Magic Plus button.
pub fn render_bottom_bar<V: 'static>(
    props: BottomBarProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, BottomBarAction, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let colors = cx.theme().colors().clone();
    let strings = props.strings;
    let has_selected = props.selected_task.is_some();
    let current_list = props.selected_task.and_then(|t| t.list_id);

    let act_new = on_action.clone();
    let act_when = on_action.clone();
    let act_move = on_action.clone();
    let act_find = on_action.clone();
    let act_magic = on_action.clone();
    let act_pick_move = on_action.clone();
    let act_close_move = on_action;

    h_flex()
        .flex_none()
        .h(px(48.))
        .px_5()
        .border_t_1()
        .border_color(colors.border_variant)
        .bg(colors.background)
        .justify_between()
        .child(div().w(px(34.)).flex_none())
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("things-bar-new", strings.things_new_todo)
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .icon(IconName::Plus)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            act_new(this, BottomBarAction::NewTodo, window, cx);
                        })),
                )
                .child(
                    div()
                        .relative()
                        .when(cfg!(test), |el| el.debug_selector(|| "things-bar-when".to_owned()))
                        .child(
                            Button::new("things-bar-when", strings.things_when)
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .icon(IconName::CalendarDays)
                                .disabled(!has_selected)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    act_when(this, BottomBarAction::ToggleWhen, window, cx);
                                })),
                        )
                        .when_some(props.when_popover, |el, pop| {
                            el.child(PopoverAttachment::new(Placement::AboveLeft, pop))
                        }),
                )
                .child(
                    div()
                        .relative()
                        .when(cfg!(test), |el| el.debug_selector(|| "things-bar-move".to_owned()))
                        .child(
                            Button::new("things-bar-move", strings.things_move)
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .icon(IconName::SquareArrowRight)
                                .disabled(!has_selected)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    act_move(this, BottomBarAction::ToggleMove, window, cx);
                                })),
                        )
                        .when(props.move_open && has_selected, |el| {
                            let move_pop = render_move_popover(
                                current_list,
                                props.lists,
                                props.folders,
                                strings,
                                cx,
                                move |this, list_id, window, cx| {
                                    act_pick_move(this, BottomBarAction::MoveTo(list_id), window, cx);
                                },
                                move |this, window, cx| {
                                    act_close_move(this, BottomBarAction::ClosePopover, window, cx);
                                },
                            );
                            el.child(PopoverAttachment::new(Placement::AboveLeft, move_pop))
                        }),
                )
                .child(
                    Button::new("things-bar-find", strings.section_search)
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .icon(IconName::Search)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            act_find(this, BottomBarAction::QuickFind, window, cx);
                        })),
                ),
        )
        .child(magic_plus::render_magic_plus(
            strings,
            cx,
            move |this, _, window, cx| {
                act_magic(this, BottomBarAction::NewTodo, window, cx);
            },
        ))
        .into_any_element()
}
