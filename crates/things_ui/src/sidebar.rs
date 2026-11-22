//! Calm Things 3-style left sidebar: smart views, Areas, Projects with `ui::Donut`, and task DnD drop targets.

use std::collections::{HashMap, HashSet};

use desktop_common::{colors, layout};
use fern_core::things::{ProjectProgress, ThingsCounts, ThingsViewKind};
use fern_core::{Id, List, ListFolder};
use fern_i18n::Strings;
use gpui::{AnyElement, Context, FontWeight, Hsla, Pixels, ScrollHandle, Window, div, px, transparent_black};
use task_ui::drag::DraggedTask;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Divider, Donut, Icon, IconButton, IconName, IconSize, Label,
    LabelSize, ListItem, Placement, PopoverAttachment, popover, v_scroll,
};

pub const SIDEBAR_WIDTH: f32 = 228.;

/// Target when dropping a dragged task onto a sidebar item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropDestination {
    Inbox,
    Today,
    Upcoming,
    Anytime,
    Someday,
    Project(Id),
}

/// Actions emitted by the Things sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarAction {
    Select(ThingsViewKind),
    ToggleArea(Id),
    DropTask {
        task_id: Id,
        target: DropDestination,
    },
    ToggleAddMenu,
    CloseAddMenu,
    NewProject(Option<Id>),
    NewArea,
}

pub struct SidebarProps<'a> {
    pub active: &'a ThingsViewKind,
    pub counts: ThingsCounts,
    pub lists: &'a [List],
    pub folders: &'a [ListFolder],
    pub progress: &'a HashMap<Id, ProjectProgress>,
    pub collapsed_areas: &'a HashSet<Id>,
    pub add_menu_open: bool,
    pub strings: &'static Strings,
    pub scroll_handle: &'a ScrollHandle,
}

/// Builds a circular `ui::Donut` progress ring for a project.
pub(crate) fn project_donut(
    progress: ProjectProgress,
    completed_project: bool,
    accent: Hsla,
    size: Pixels,
    thickness: Pixels,
) -> impl IntoElement {
    let segments: Vec<(f32, Hsla)> = if completed_project || (progress.total > 0 && progress.completed >= progress.total) {
        vec![(1.0, accent)]
    } else if progress.completed > 0 && progress.total > progress.completed {
        let done = progress.completed as f32;
        let remaining = (progress.total - progress.completed) as f32;
        vec![(done, accent), (remaining, transparent_black())]
    } else {
        Vec::new()
    };
    Donut::new(segments).size(size).thickness(thickness)
}

pub fn render_sidebar<V: 'static>(
    props: SidebarProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, SidebarAction, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let theme = cx.theme().clone();
    let theme_colors = theme.colors().clone();
    let status = theme.status().clone();
    let strings = props.strings;
    let active = *props.active;
    let accent = theme_colors.accent;

    let smart_row = |id: &'static str,
                     selector: &'static str,
                     view: ThingsViewKind,
                     label: &'static str,
                     icon: IconName,
                     icon_color: Hsla,
                     count: usize,
                     drop_target: Option<DropDestination>,
                     cx: &mut Context<V>| {
        let selected = active == view;
        let on_select = on_action.clone();
        let on_drop = on_action.clone();
        let row = ListItem::new(id)
            .height(px(32.))
            .hover_color(theme_colors.list_hover)
            .selected_color(theme_colors.list_selected)
            .toggle_state(selected)
            .start_slot(Icon::new(icon).size(IconSize::Small).color(icon_color))
            .child(
                Label::new(label)
                    .size(LabelSize::Small)
                    .weight(if selected { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                    .color(theme_colors.list_text)
                    .truncate(),
            )
            .when(count > 0, |item| {
                item.end_slot(
                    Label::new(count.to_string())
                        .size(LabelSize::XSmall)
                        .color(theme_colors.list_count),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                on_select(this, SidebarAction::Select(view), window, cx);
            }));

        div()
            .id(SharedString::from(format!("wrap-{id}")))
            .when(cfg!(test), |el| el.debug_selector(move || selector.to_owned()))
            .rounded(px(6.))
            .when_some(drop_target, |el, target| {
                el.drag_over::<DraggedTask>(move |style, _, _, _| {
                    style.bg(accent.opacity(0.16)).border_1().border_color(accent.opacity(0.55))
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedTask, window, cx| {
                    on_drop(
                        this,
                        SidebarAction::DropTask {
                            task_id: dragged.id,
                            target,
                        },
                        window,
                        cx,
                    );
                }))
            })
            .child(row)
            .into_any_element()
    };

    let project_row = |list: &List, indent: bool, cx: &mut Context<V>| {
        let id = list.id;
        let view = ThingsViewKind::Project(id);
        let selected = active == view;
        let prog = props.progress.get(&id).copied().unwrap_or_default();
        let remaining = prog.total.saturating_sub(prog.completed);
        let ring_color = colors::parse_color(list.color.as_deref()).unwrap_or(accent);
        let on_select = on_action.clone();
        let on_drop = on_action.clone();
        let row = ListItem::new(SharedString::from(format!("things-project-{id}")))
            .height(px(30.))
            .hover_color(theme_colors.list_hover)
            .selected_color(theme_colors.list_selected)
            .toggle_state(selected)
            .start_slot(project_donut(prog, list.completed_at.is_some(), ring_color, px(15.), px(2.3)))
            .child(
                Label::new(list.name.clone())
                    .size(LabelSize::Small)
                    .weight(if selected { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                    .color(if list.completed_at.is_some() {
                        theme_colors.list_text_muted
                    } else {
                        theme_colors.list_text
                    })
                    .truncate(),
            )
            .when(remaining > 0, |item| {
                item.end_slot(
                    Label::new(remaining.to_string())
                        .size(LabelSize::XSmall)
                        .color(theme_colors.list_count),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                on_select(this, SidebarAction::Select(view), window, cx);
            }));

        div()
            .id(SharedString::from(format!("things-drop-project-{id}")))
            .when(cfg!(test), |el| el.debug_selector(move || format!("things-project-{id}")))
            .when(indent, |el| el.pl_3())
            .rounded(px(6.))
            .drag_over::<DraggedTask>(move |style, _, _, _| {
                style.bg(accent.opacity(0.16)).border_1().border_color(accent.opacity(0.55))
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, window, cx| {
                on_drop(
                    this,
                    SidebarAction::DropTask {
                        task_id: dragged.id,
                        target: DropDestination::Project(id),
                    },
                    window,
                    cx,
                );
            }))
            .child(row)
            .into_any_element()
    };

    let active_lists: Vec<&List> = props.lists.iter().filter(|l| !l.archived).collect();
    let standalone_projects: Vec<&List> = active_lists.iter().copied().filter(|l| l.folder.is_none()).collect();

    let mut areas_and_projects: Vec<AnyElement> = Vec::new();
    for proj in &standalone_projects {
        areas_and_projects.push(project_row(proj, false, cx));
    }

    for folder in props.folders {
        let folder_id = folder.id;
        let collapsed = props.collapsed_areas.contains(&folder_id);
        let view = ThingsViewKind::Area(folder_id);
        let selected = active == view;
        let on_select = on_action.clone();
        let on_toggle = on_action.clone();
        let on_new_proj = on_action.clone();
        let area_children: Vec<&List> = active_lists
            .iter()
            .copied()
            .filter(|l| l.folder == Some(folder_id))
            .collect();

        let area_header = ListItem::new(SharedString::from(format!("things-area-{folder_id}")))
            .height(px(30.))
            .hover_color(theme_colors.list_hover)
            .selected_color(theme_colors.list_selected)
            .toggle_state(selected)
            .start_slot(
                h_flex()
                    .gap_1()
                    .child(
                        IconButton::new(
                            SharedString::from(format!("things-area-toggle-{folder_id}")),
                            if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            },
                        )
                        .size(px(18.))
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            on_toggle(this, SidebarAction::ToggleArea(folder_id), window, cx);
                        })),
                    )
                    .child(Icon::new(IconName::Folder).size(IconSize::Small).color(Color::Icon)),
            )
            .child(
                Label::new(folder.name.clone())
                    .size(LabelSize::Small)
                    .weight(FontWeight::MEDIUM)
                    .color(theme_colors.list_text)
                    .truncate(),
            )
            .end_slot(
                IconButton::new(
                    SharedString::from(format!("things-area-add-{folder_id}")),
                    IconName::Plus,
                )
                .size(px(20.))
                .icon_size(IconSize::XSmall)
                .icon_color(Color::Muted)
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    on_new_proj(this, SidebarAction::NewProject(Some(folder_id)), window, cx);
                })),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                on_select(this, SidebarAction::Select(view), window, cx);
            }));

        areas_and_projects.push(
            div()
                .when(cfg!(test), |el| el.debug_selector(move || format!("things-area-{folder_id}")))
                .child(area_header)
                .into_any_element(),
        );
        if !collapsed {
            for proj in area_children {
                areas_and_projects.push(project_row(proj, true, cx));
            }
        }
    }

    let toggle_menu = on_action.clone();
    let close_menu = on_action.clone();
    let new_project = on_action.clone();
    let new_area = on_action.clone();

    v_flex()
        .w(px(SIDEBAR_WIDTH))
        .flex_none()
        .h_full()
        .bg(theme_colors.lists_background)
        .border_r_1()
        .border_color(theme_colors.lists_border)
        .pt(layout::header_inset())
        .child(
            v_scroll(
                "things-sidebar-scroll",
                v_flex()
                    .id("things-sidebar-items")
                    .px_2p5()
                    .py_2()
                    .gap_0p5()
                    .child(smart_row(
                        "things-nav-inbox",
                        "things-nav-inbox",
                        ThingsViewKind::Inbox,
                        strings.things_inbox,
                        IconName::Inbox,
                        status.info,
                        props.counts.inbox,
                        Some(DropDestination::Inbox),
                        cx,
                    ))
                    .child(div().h(px(6.)))
                    .child(smart_row(
                        "things-nav-today",
                        "things-nav-today",
                        ThingsViewKind::Today,
                        strings.things_today,
                        IconName::Sun,
                        status.warning,
                        props.counts.today,
                        Some(DropDestination::Today),
                        cx,
                    ))
                    .child(smart_row(
                        "things-nav-upcoming",
                        "things-nav-upcoming",
                        ThingsViewKind::Upcoming,
                        strings.things_upcoming,
                        IconName::CalendarDays,
                        status.error,
                        props.counts.upcoming,
                        Some(DropDestination::Upcoming),
                        cx,
                    ))
                    .child(smart_row(
                        "things-nav-anytime",
                        "things-nav-anytime",
                        ThingsViewKind::Anytime,
                        strings.things_anytime,
                        IconName::Layers,
                        accent,
                        props.counts.anytime,
                        Some(DropDestination::Anytime),
                        cx,
                    ))
                    .child(smart_row(
                        "things-nav-someday",
                        "things-nav-someday",
                        ThingsViewKind::Someday,
                        strings.things_someday,
                        IconName::Archive,
                        theme_colors.icon_muted,
                        props.counts.someday,
                        Some(DropDestination::Someday),
                        cx,
                    ))
                    .child(div().h(px(6.)))
                    .child(smart_row(
                        "things-nav-logbook",
                        "things-nav-logbook",
                        ThingsViewKind::Logbook,
                        strings.things_logbook,
                        IconName::CircleCheck,
                        status.success,
                        props.counts.logbook,
                        None,
                        cx,
                    ))
                    .when(!areas_and_projects.is_empty(), |col| {
                        col.child(div().my_2().child(Divider::horizontal()))
                            .child(
                                div().px_2().pb_1().child(
                                    Label::new(format!("{} · {}", strings.things_areas, strings.things_projects))
                                        .size(LabelSize::XSmall)
                                        .weight(FontWeight::SEMIBOLD)
                                        .color(Color::Muted),
                                ),
                            )
                            .children(areas_and_projects)
                    }),
            )
            .track_scroll(props.scroll_handle)
            .flex_1()
            .min_h_0(),
        )
        .child(
            div()
                .relative()
                .flex_none()
                .px_2p5()
                .py_2()
                .border_t_1()
                .border_color(theme_colors.border_variant)
                .child(
                    Button::new("things-sidebar-add", strings.things_new_project)
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .icon(IconName::Plus)
                        .full_width()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            toggle_menu(this, SidebarAction::ToggleAddMenu, window, cx);
                        })),
                )
                .when(props.add_menu_open, |el| {
                    el.child(PopoverAttachment::new(
                        Placement::AboveLeft,
                        popover(cx)
                            .id("things-sidebar-add-menu")
                            .min_w(px(184.))
                            .on_mouse_down_out(cx.listener(move |this, _, window, cx| {
                                close_menu(this, SidebarAction::CloseAddMenu, window, cx);
                            }))
                            .child(
                                ListItem::new("things-add-project")
                                    .height(px(30.))
                                    .start_slot(Icon::new(IconName::CircleDot).size(IconSize::Small).color(Color::Accent))
                                    .child(Label::new(strings.things_new_project).size(LabelSize::Small))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        new_project(this, SidebarAction::NewProject(None), window, cx);
                                    })),
                            )
                            .child(
                                ListItem::new("things-add-area")
                                    .height(px(30.))
                                    .start_slot(Icon::new(IconName::Folder).size(IconSize::Small).color(Color::Icon))
                                    .child(Label::new(strings.things_new_area).size(LabelSize::Small))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        new_area(this, SidebarAction::NewArea, window, cx);
                                    })),
                            ),
                    ))
                }),
        )
        .into_any_element()
}
