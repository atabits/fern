//! The lists column between the narrow sidebar and the task list, modeled on TickTick's.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{
    Id, List, ListFolder, Preferences, SidebarSection, SmartList, Tag, View, ViewCounts,
};
use gpui::{ClickEvent, MouseButton, Pixels, Point, ScrollHandle};
use ui::prelude::*;

use crate::calendar;
use crate::drag::{
    DraggedFilter, DraggedFolder, DraggedList, DraggedParentTag, DraggedSidebarSection,
    DraggedSmartList, DraggedTag, DraggedTask,
};
use crate::i18n::Strings;

/// The column's width by default and the range its edge drags in; dragged narrower than
/// `COLLAPSE_BELOW`, the column hides, as the sidebar toggle does.
pub const WIDTH: f32 = 212.;
pub const MIN_WIDTH: f32 = 180.;
pub const MAX_WIDTH: f32 = 360.;
pub const COLLAPSE_BELOW: f32 = 120.;

pub use desktop_common::colors::{LIST_COLORS, parse_color};

/// The title of a view for the list header.
pub fn title(view: &View, lists: &[List], filters: &[fern_core::SmartFilter], strings: &Strings, cx: &App) -> SharedString {
    match view {
        View::Inbox => strings.inbox.into(),
        View::All => strings.all.into(),
        View::Today => strings.today.into(),
        View::Tomorrow => strings.tomorrow.into(),
        View::Next7Days => strings.next_7_days.into(),
        View::Completed => strings.completed.into(),
        View::Trash => strings.trash.into(),
        View::WontDo => strings.wont_do.into(),
        View::List(id) => {
            lists.iter().find(|l| l.id == *id).map(|l| l.name.clone()).unwrap_or_default().into()
        }
        View::Tag(name) => name.clone().into(),
        View::Search(_) => strings.section_search.into(),
        View::Filter(id) => filters.iter().find(|f| f.id == *id).map(|f| f.name.clone()).unwrap_or_default().into(),
        View::Horizons => strings.section_horizons.into(),
        View::Summary => strings.summary.into(),
        View::Day(day) => calendar::format_date(*day, strings, cx).into(),
    }
}

/// A tag or list color, or the muted text color when unset.
pub fn color(value: Option<&str>, cx: &App) -> Hsla {
    parse_color(value).unwrap_or(cx.theme().colors().text_muted)
}

/// The sidebar entry currently lifted by the pointer during live-sliding drag-and-drop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActiveDragItem {
    SmartList(SmartList),
    SidebarSection(SidebarSection),
    List(Id),
    Folder(Id),
    Tag(String),
    Filter(Id),
}

/// Row highlighted when hovering the middle zone to group lists into a folder or nest into a
/// folder / parent tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HighlightTarget {
    List(Id),
    Folder(Id),
    Tag(String),
}

/// Descriptor for each direct child pushed into `#lists-panel` (`scroll_handle.bounds_for_item(ix)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelChild {
    SmartList(SmartList),
    Separator,
    SectionHeading(SidebarSection),
    Hint(SidebarSection),
    TopList(Id),
    FolderHeader { id: Id, collapsed: bool, empty: bool },
    SubList { id: Id, folder: Id },
    ArchivedHeader,
    ArchivedList(Id),
    TopTag { name: String, is_parent: bool, collapsed: bool, pinned: bool },
    SubTag { name: String, parent: String },
    Filter(Id),
    BottomSmart(View),
}

/// Precomputed lookup tables and ordering for the lists panel so `render()` avoids rebuilding
/// `HashMap`s, `HashSet`s, and sorted tag vectors on every frame.
#[derive(Default)]
pub struct ListsPanelIndex {
    archived_list_ids: HashSet<Id>,
    folder_by_id: HashMap<Id, usize>,
    lists_by_folder: HashMap<Id, Vec<usize>>,
    active_list_indices: Vec<usize>,
    archived_list_indices: Vec<usize>,
    tag_children: HashMap<String, Vec<usize>>,
    tag_family_counts: HashMap<String, usize>,
    ordered_root_tags: Vec<usize>,
}

impl ListsPanelIndex {
    pub fn build(lists: &[List], folders: &[ListFolder], tags: &[Tag]) -> Self {
        let mut archived_list_ids = HashSet::new();
        let mut lists_by_folder: HashMap<Id, Vec<usize>> = HashMap::new();
        let mut active_list_indices = Vec::with_capacity(lists.len());
        let mut archived_list_indices = Vec::new();
        for (idx, list) in lists.iter().enumerate() {
            if list.archived {
                archived_list_ids.insert(list.id);
                archived_list_indices.push(idx);
            } else {
                if let Some(fid) = list.folder {
                    lists_by_folder.entry(fid).or_default().push(idx);
                }
                active_list_indices.push(idx);
            }
        }
        let folder_by_id: HashMap<Id, usize> =
            folders.iter().enumerate().map(|(idx, f)| (f.id, idx)).collect();

        let tag_names: HashSet<&str> = tags.iter().map(|t| t.name.as_str()).collect();
        let mut tag_children: HashMap<String, Vec<usize>> = HashMap::new();
        for (idx, tag) in tags.iter().enumerate() {
            if let Some(parent) = tag.parent.as_deref() {
                tag_children.entry(parent.to_owned()).or_default().push(idx);
            }
        }
        let tag_family_counts: HashMap<String, usize> = tags
            .iter()
            .map(|tag| {
                let children_sum: usize = tag_children
                    .get(tag.name.as_str())
                    .map(|cs| cs.iter().map(|&idx| tags[idx].open_count).sum())
                    .unwrap_or(0);
                (tag.name.clone(), tag.open_count + children_sum)
            })
            .collect();
        let mut ordered_root_tags: Vec<usize> = tags
            .iter()
            .enumerate()
            .filter(|(_, t)| t.parent.as_deref().is_none_or(|p| !tag_names.contains(p)))
            .map(|(idx, _)| idx)
            .collect();
        ordered_root_tags.sort_by_key(|&idx| !tags[idx].pinned);

        Self {
            archived_list_ids,
            folder_by_id,
            lists_by_folder,
            active_list_indices,
            archived_list_indices,
            tag_children,
            tag_family_counts,
            ordered_root_tags,
        }
    }
}

pub struct Props<'a> {
    pub active: &'a View,
    pub counts: &'a ViewCounts,
    pub lists: &'a [List],
    pub tags: &'a [Tag],
    pub strings: &'static Strings,
    /// The folders of lists, and the ones folded up.
    pub folders: &'a [ListFolder],
    pub index: &'a ListsPanelIndex,
    pub collapsed_folders: &'a HashSet<Id>,
    /// Whether "Archived Lists" is unfolded.
    pub archived_open: bool,
    /// Parent tags folded up, and whether the whole Tags section is.
    pub collapsed_tags: &'a HashSet<String>,
    pub tags_collapsed: bool,
    /// Saved filters with their task counts.
    pub filters: &'a [(fern_core::SmartFilter, usize)],
    /// Which smart lists Settings → Smart List shows.
    pub preferences: &'a Preferences,
    pub mini: MiniCalendar<'a>,
    pub width: f32,
    pub scroll_handle: &'a ScrollHandle,
    pub rows_out: &'a RefCell<Vec<PanelChild>>,
    pub active_drag: Option<&'a ActiveDragItem>,
    pub highlight_target: Option<&'a HighlightTarget>,
}

/// What the lists column asks of the view, besides selecting a view.
#[derive(Clone, Debug)]
pub enum PanelEvent {
    /// The Tags heading: its "+", its "…" menu (at that point) and folding the section.
    AddTag,
    TagsMenu(Point<Pixels>),
    ToggleTags,
    /// A smart list's "…", shown on hover in place of its count, at that point of the window.
    SmartMenu(SmartList, Point<Pixels>),
    /// A parent tag folded up or out, and a right click on a tag.
    ToggleTag(String),
    TagMenu(String, Point<Pixels>),
    ToggleFolder(Id),
    ToggleArchived,
    /// A right click on a list or a folder, at that point of the window.
    ListMenu(Id, Point<Pixels>),
    FolderMenu(Id, Point<Pixels>),
    /// Begins live-sliding a sidebar entry, recording the vertical pointer offset inside its row.
    StartDrag { item: ActiveDragItem, grab_offset_y: f32 },
}

fn vacant_slot(height: f32, indent: bool, selector: String, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    div()
        .w_full()
        .h(px(height))
        .when(indent, |el| el.pl(px(14.)))
        .debug_selector(move || selector)
        .py(px(2.))
        .child(
            div()
                .size_full()
                .rounded(px(6.))
                .bg(colors.list_hover)
                .border_1()
                .border_color(colors.accent.opacity(0.32)),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    props: Props,
    on_select: impl Fn(View, &mut Window, &mut App) + Clone + 'static,
    on_panel: impl Fn(PanelEvent, &mut Window, &mut App) + Clone + 'static,
    on_add_list: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_drop_task: impl Fn(View, fern_core::Id, &mut Window, &mut App) + Clone + 'static,
    on_add_filter: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_mini: impl Fn(MiniAction, &mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let on_add_list = std::rc::Rc::new(on_add_list);
    let on_add_filter = std::rc::Rc::new(on_add_filter);
    let Props {
        active,
        counts,
        lists,
        tags,
        strings,
        folders,
        index,
        collapsed_folders,
        archived_open,
        collapsed_tags,
        tags_collapsed,
        filters,
        preferences,
        mini,
        width,
        scroll_handle,
        rows_out,
        active_drag,
        highlight_target,
    } = props;
    let shows = |list: SmartList, count: usize| preferences.visibility(list).shows(count);
    let colors = cx.theme().colors();
    let accent = colors.accent;
    let active_drag = active_drag.filter(|_| cx.has_active_drag());
    let highlight_target = highlight_target.filter(|_| active_drag.is_some());
    let row_hover = if active_drag.is_some() {
        gpui::transparent_black()
    } else {
        colors.list_hover
    };

    let item = |id: String,
                view: View,
                label: SharedString,
                icon: AnyElement,
                count: Option<usize>,
                dot: Option<Hsla>,
                menu: Option<SmartList>,
                highlighted: bool| {
        let on_select = on_select.clone();
        let on_panel = on_panel.clone();
        let on_secondary = on_panel.clone();
        let group = SharedString::from(format!("row-{id}"));
        let on_drop_task = on_drop_task.clone();
        let drop_view = view.clone();
        let accent = colors.accent;
        // Every entry is also a drop target for dragged tasks.
        let item = ListItem::new(SharedString::from(id.clone()))
            .height(px(38.))
            .hover_color(row_hover)
            .selected_color(if highlighted { gpui::transparent_black() } else { colors.list_selected })
            .toggle_state(&view == active)
            .start_slot(icon)
            .child(Label::new(label).color(colors.list_text).truncate())
            .end_slot(
                h_flex()
                    .gap_2()
                    .when_some(dot, |el, color| el.child(div().size(px(6.)).rounded_full().bg(color)))
                    .when_some(count.filter(|c| *c > 0), |el, count| {
                        el.child(
                            div()
                                .min_w(px(18.))
                                .flex()
                                .justify_end()
                                .when(menu.is_some() && active_drag.is_none(), |el| {
                                    el.group_hover(group.clone(), |s| s.invisible())
                                })
                                .child(Label::new(count.to_string()).size(LabelSize::Small).color(colors.list_count)),
                        )
                    })
                    // As the original's, a hovered smart list shows "…" where its count is.
                    .when_some(menu.filter(|_| active_drag.is_none()), |el, list| {
                        let more_group = SharedString::from(format!("smart-more-{id}"));
                        el.child(
                            div()
                                .id(more_group.clone())
                                .group(more_group.clone())
                                .absolute()
                                .right(px(10.))
                                .invisible()
                                .group_hover(group.clone(), |s| s.visible())
                                .tooltip(Tooltip::text(strings.more))
                                .child(
                                    Icon::new(IconName::Ellipsis)
                                        .size(IconSize::Small)
                                        .color(colors.list_text_muted)
                                        .group_hover_color(more_group, colors.list_text),
                                )
                                .on_mouse_down(gpui::MouseButton::Left, move |event: &gpui::MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    on_panel(PanelEvent::SmartMenu(list, event.position), window, cx)
                                }),
                        )
                    }),
            )
            .on_click(move |_, window, cx| on_select(view.clone(), window, cx));
        let selector_id = id.clone();
        let accepts_task_drop = !matches!(drop_view, View::All | View::Horizons | View::Summary | View::Filter(_) | View::Search(_))
            && !matches!(drop_view, View::List(list_id) if index.archived_list_ids.contains(&list_id));
        div()
            .id(SharedString::from(format!("drop-{id}")))
            .debug_selector(move || format!("lists-item-{selector_id}"))
            .group(group)
            .relative()
            .rounded(px(6.))
            .when(highlighted, |el| el.bg(accent.opacity(0.18)).border_1().border_color(accent))
            .when(accepts_task_drop, |el| {
                el.drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.18)))
                    .on_drop(move |dragged: &DraggedTask, window, cx| on_drop_task(drop_view.clone(), dragged.id, window, cx))
            })
            // The "…" menu on a right click too, and so on a finger's long press.
            .when_some(menu, |el, list| {
                el.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    cx.stop_propagation();
                    on_secondary(PanelEvent::SmartMenu(list, event.position), window, cx)
                })
            })
            .child(item)
    };
    let calendar_day_badge = |text: String, color: Hsla| -> AnyElement {
        v_flex()
            .w(px(16.))
            .h(px(16.))
            .rounded(px(3.5))
            .border_1()
            .border_color(color.opacity(0.85))
            .overflow_hidden()
            .child(div().w_full().h(px(3.)).bg(color.opacity(0.85)))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(8.5))
                    .line_height(px(10.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(color)
                    .child(text),
            )
            .into_any_element()
    };
    let smart = |view: View, label: &'static str, icon: IconName, count: Option<usize>| {
        // Every smart list but the Inbox can be hidden from its "…", as in the original.
        let menu = match view {
            View::All => Some(SmartList::All),
            View::Today => Some(SmartList::Today),
            View::Tomorrow => Some(SmartList::Tomorrow),
            View::Next7Days => Some(SmartList::Next7Days),
            View::Horizons => Some(SmartList::Horizons),
            View::Summary => Some(SmartList::Summary),
            _ => None,
        };
        let leading = match view {
            View::Today => calendar_day_badge(Local::now().date_naive().day().to_string(), colors.list_text),
            View::Next7Days => calendar_day_badge("7".to_owned(), colors.list_text),
            _ => Icon::new(icon).color(colors.list_text).into_any_element(),
        };
        item(
            format!("view-{label}"),
            view,
            label.into(),
            leading,
            count,
            None,
            menu,
            false,
        )
    };
    let draggable_smart = |which: SmartList, view: View, label: &'static str, icon: IconName, count: Option<usize>| -> AnyElement {
        if active_drag == Some(&ActiveDragItem::SmartList(which)) {
            return vacant_slot(38., false, format!("lists-item-view-{label}"), cx);
        }
        let on_start = on_panel.clone();
        let dragged = DraggedSmartList { list: which, label: label.into(), icon };
        smart(view, label, icon, count)
            .on_drag(dragged, move |d, cursor_offset, window, cx| {
                on_start(
                    PanelEvent::StartDrag {
                        item: ActiveDragItem::SmartList(which),
                        grab_offset_y: f32::from(cursor_offset.y).clamp(0., 38.),
                    },
                    window,
                    cx,
                );
                cx.new(|_| d.clone())
            })
            .into_any_element()
    };
    // As in the original: a row every 38 px, sections apart with a 12 px gap around their line,
    // and a section's "+" shown on hover.
    let separator = || div().my_3().mx_2().child(Divider::horizontal()).into_any_element();
    let heading = |text: &'static str| {
        h_flex()
            .id(SharedString::from(format!("heading-{text}")))
            .group(SharedString::from(format!("heading-{text}")))
            .h(px(36.))
            .pt_1()
            .px_2()
            .child(div().flex_1().child(Label::new(text).size(LabelSize::Small).color(colors.list_text_muted)))
    };
    let on_hover_only = |text: &'static str, button: IconButton| {
        div()
            .when(!crate::touch::active() || active_drag.is_some(), |el| el.invisible())
            .when(active_drag.is_none(), |el| el.group_hover(SharedString::from(format!("heading-{text}")), |s| s.visible()))
            .child(button)
    };

    // The lists in their order; a folder sits where its first list is (empty ones last), its
    // lists indented under it unless it is folded up, as in the original.
    let list_entry = |list: &List, indent: bool, muted: bool| -> AnyElement {
        let id = list.id;
        if !muted && active_drag == Some(&ActiveDragItem::List(id)) {
            return vacant_slot(38., indent, format!("lists-item-list-{id}"), cx);
        }
        let highlighted = !muted && highlight_target == Some(&HighlightTarget::List(id));
        let list_color = parse_color(list.color.as_deref());
        let entry = item(
            format!("list-{id}"),
            View::List(id),
            list.name.clone().into(),
            match &list.icon {
                Some(icon) => div().w(px(16.)).flex().justify_center().text_size(px(14.)).child(icon.clone()).into_any_element(),
                None => Icon::new(IconName::List).color(colors.list_text).into_any_element(),
            },
            Some(list.open_count),
            list_color,
            None,
            highlighted,
        );
        let on_menu = on_panel.clone();
        let on_start = on_panel.clone();
        let dragged = DraggedList { id, name: list.name.clone().into(), icon: list.icon.clone(), color: list_color };
        div()
            .id(SharedString::from(format!("list-entry-{id}")))
            .when(indent, |el| el.pl(px(14.)))
            .when(muted, |el| el.opacity(0.6))
            .when(!muted, |el| {
                el.on_drag(dragged, move |dragged, cursor_offset, window, cx| {
                    on_start(
                        PanelEvent::StartDrag {
                            item: ActiveDragItem::List(id),
                            grab_offset_y: f32::from(cursor_offset.y).clamp(0., 38.),
                        },
                        window,
                        cx,
                    );
                    cx.new(|_| dragged.clone())
                })
            })
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                cx.stop_propagation();
                on_menu(PanelEvent::ListMenu(id, event.position), window, cx)
            })
            .child(entry)
            .into_any_element()
    };
    let folder_entry = |folder: &ListFolder, inside_indices: &[usize]| -> AnyElement {
        let id = folder.id;
        if active_drag == Some(&ActiveDragItem::Folder(id)) {
            return vacant_slot(36., false, format!("lists-folder-{id}"), cx);
        }
        let highlighted = highlight_target == Some(&HighlightTarget::Folder(id));
        let collapsed = collapsed_folders.contains(&id);
        let inside_empty = inside_indices.is_empty();
        let count: usize = inside_indices.iter().map(|&idx| lists[idx].open_count).sum();
        let on_toggle = on_panel.clone();
        let on_menu = on_panel.clone();
        let on_start = on_panel.clone();
        let dragged = DraggedFolder { id, name: folder.name.clone().into(), has_lists: !inside_empty };
        div()
            .id(SharedString::from(format!("folder-{id}")))
            .debug_selector(move || format!("lists-folder-{id}"))
            .relative()
            .rounded(px(6.))
            .when(highlighted, |el| el.bg(accent.opacity(0.18)).border_1().border_color(accent))
            .on_drag(dragged, move |dragged, cursor_offset, window, cx| {
                on_start(
                    PanelEvent::StartDrag {
                        item: ActiveDragItem::Folder(id),
                        grab_offset_y: f32::from(cursor_offset.y).clamp(0., 36.),
                    },
                    window,
                    cx,
                );
                cx.new(|_| dragged.clone())
            })
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                cx.stop_propagation();
                on_menu(PanelEvent::FolderMenu(id, event.position), window, cx)
            })
            .child(
                ListItem::new(SharedString::from(format!("folder-item-{id}")))
                    .height(px(36.))
                    .hover_color(row_hover)
                    .selected_color(if highlighted { gpui::transparent_black() } else { colors.list_selected })
                    .start_slot(Icon::new(if collapsed { IconName::Folder } else { IconName::FolderOpen }).color(colors.list_text))
                    .child(Label::new(folder.name.clone()).color(colors.list_text).truncate())
                    .end_slot(
                        h_flex()
                            .gap_1()
                            .when(collapsed && count > 0, |el| el.child(Label::new(count.to_string()).size(LabelSize::Small).color(colors.list_count)))
                            .child(Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall).color(colors.list_text_muted)),
                    )
                    .on_click(move |_, window, cx| on_toggle(PanelEvent::ToggleFolder(id), window, cx)),
            )
            .into_any_element()
    };
    let mut list_entries: Vec<(PanelChild, AnyElement)> = Vec::new();
    let mut shown_folders: HashSet<Id> = HashSet::new();
    for &list_idx in &index.active_list_indices {
        let list = &lists[list_idx];
        match list.folder.and_then(|f| index.folder_by_id.get(&f).copied()) {
            None => list_entries.push((PanelChild::TopList(list.id), list_entry(list, false, false))),
            Some(folder_idx) => {
                let folder = &folders[folder_idx];
                if shown_folders.insert(folder.id) {
                    let inside = index.lists_by_folder.get(&folder.id).map(Vec::as_slice).unwrap_or(&[]);
                    let collapsed = collapsed_folders.contains(&folder.id);
                    let folder_dragged = active_drag == Some(&ActiveDragItem::Folder(folder.id));
                    list_entries.push((
                        PanelChild::FolderHeader { id: folder.id, collapsed: collapsed || folder_dragged, empty: false },
                        folder_entry(folder, inside),
                    ));
                    if !collapsed && !folder_dragged {
                        list_entries.extend(inside.iter().map(|&idx| {
                            let l = &lists[idx];
                            (PanelChild::SubList { id: l.id, folder: folder.id }, list_entry(l, true, false))
                        }));
                    }
                }
            }
        }
    }
    for folder in folders.iter().filter(|f| !shown_folders.contains(&f.id)) {
        let collapsed = collapsed_folders.contains(&folder.id);
        list_entries.push((
            PanelChild::FolderHeader { id: folder.id, collapsed, empty: true },
            folder_entry(folder, &[]),
        ));
    }
    if !index.archived_list_indices.is_empty() {
        let on_panel = on_panel.clone();
        list_entries.push((
            PanelChild::ArchivedHeader,
            ListItem::new("archived-lists")
                .height(px(36.))
                .hover_color(row_hover)
                .selected_color(colors.list_selected)
                .start_slot(Icon::new(IconName::Archive).color(colors.list_text))
                .child(Label::new(strings.archived_lists).color(colors.list_text))
                .end_slot(Icon::new(if archived_open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::XSmall).color(colors.list_text_muted))
                .on_click(move |_, window, cx| on_panel(PanelEvent::ToggleArchived, window, cx))
                .into_any_element(),
        ));
        if archived_open {
            list_entries.extend(index.archived_list_indices.iter().map(|&idx| {
                let l = &lists[idx];
                (PanelChild::ArchivedList(l.id), list_entry(l, true, true))
            }));
        }
    }

    // Tags as in the original: pinned ones first, sub-tags indented under their parent (which
    // folds them up), a right click for the tag's menu; the heading shows "…" and "+" on hover.
    let family_count = |tag: &Tag| index.tag_family_counts.get(tag.name.as_str()).copied().unwrap_or(tag.open_count);
    let tag_entry = |tag: &Tag, indent: bool, fold: Option<bool>| -> AnyElement {
        let name = tag.name.clone();
        if active_drag.is_some_and(|d| matches!(d, ActiveDragItem::Tag(dragged) if dragged == &name)) {
            return vacant_slot(38., indent, format!("lists-item-tag-{name}"), cx);
        }
        let highlighted = highlight_target.is_some_and(|h| matches!(h, HighlightTarget::Tag(target) if target == &name));
        let tag_color = color(tag.color.as_deref(), cx);
        let is_parent = fold.is_some();
        let collapsed = fold == Some(true);
        let on_menu = on_panel.clone();
        let on_fold = on_panel.clone();
        let on_start = on_panel.clone();
        let count = if collapsed { family_count(tag) } else { tag.open_count };
        let menu_name = name.clone();
        let fold_name = name.clone();
        let drag_name = name.clone();
        let row = item(
            format!("tag-{name}"),
            View::Tag(name.clone()),
            name.clone().into(),
            Icon::new(IconName::Tag).color(colors.list_text).into_any_element(),
            Some(count),
            Some(tag_color),
            None,
            highlighted,
        );
        div()
            .id(SharedString::from(format!("tag-entry-{name}")))
            .relative()
            .when(indent, |el| el.pl(px(14.)))
            .map(|el| {
                if is_parent {
                    let dragged = DraggedParentTag { name: name.clone().into(), color: Some(tag_color) };
                    el.on_drag(dragged, move |d, cursor_offset, window, cx| {
                        on_start(
                            PanelEvent::StartDrag {
                                item: ActiveDragItem::Tag(drag_name.clone()),
                                grab_offset_y: f32::from(cursor_offset.y).clamp(0., 38.),
                            },
                            window,
                            cx,
                        );
                        cx.new(|_| d.clone())
                    })
                } else {
                    let dragged = DraggedTag { name: name.clone().into(), color: Some(tag_color) };
                    el.on_drag(dragged, move |d, cursor_offset, window, cx| {
                        on_start(
                            PanelEvent::StartDrag {
                                item: ActiveDragItem::Tag(drag_name.clone()),
                                grab_offset_y: f32::from(cursor_offset.y).clamp(0., 38.),
                            },
                            window,
                            cx,
                        );
                        cx.new(|_| d.clone())
                    })
                }
            })
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                cx.stop_propagation();
                on_menu(PanelEvent::TagMenu(menu_name.clone(), event.position), window, cx)
            })
            .child(row)
            .when_some(fold, |el, folded| {
                // A parent's arrow at the left edge, as the original's folders of tags have.
                let fold_group = SharedString::from(format!("tag-fold-{name}"));
                el.child(
                    div()
                        .id(fold_group.clone())
                        .group(fold_group.clone())
                        .absolute()
                        .left(px(-6.))
                        .top(px(11.))
                        .cursor_pointer()
                        .child(
                            Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown })
                                .size(IconSize::XSmall)
                                .color(colors.list_text_muted)
                                .group_hover_color(fold_group, colors.list_text),
                        )
                        .on_click(move |_, window, cx| on_fold(PanelEvent::ToggleTag(fold_name.clone()), window, cx)),
                )
            })
            .into_any_element()
    };
    let mut tag_entries: Vec<(PanelChild, AnyElement)> = Vec::new();
    if !tags_collapsed {
        for &tag_idx in &index.ordered_root_tags {
            let tag = &tags[tag_idx];
            let children = index.tag_children.get(tag.name.as_str()).map(Vec::as_slice).unwrap_or(&[]);
            let is_dragged_root = active_drag.is_some_and(|d| matches!(d, ActiveDragItem::Tag(n) if n == &tag.name));
            let has_dragged_child = children.iter().any(|&c_idx| {
                let c = &tags[c_idx];
                active_drag.is_some_and(|d| matches!(d, ActiveDragItem::Tag(n) if n == &c.name))
            });
            if !is_dragged_root && !has_dragged_child && !shows(SmartList::Tags, family_count(tag)) {
                continue;
            }
            let folded = collapsed_tags.contains(&tag.name);
            let is_parent = !children.is_empty();
            tag_entries.push((
                PanelChild::TopTag {
                    name: tag.name.clone(),
                    is_parent,
                    collapsed: folded || is_dragged_root,
                    pinned: tag.pinned,
                },
                tag_entry(tag, false, is_parent.then_some(folded)),
            ));
            if !folded && !is_dragged_root {
                for &child_idx in children {
                    let child = &tags[child_idx];
                    let is_dragged_child =
                        active_drag.is_some_and(|d| matches!(d, ActiveDragItem::Tag(n) if n == &child.name));
                    if is_dragged_child || shows(SmartList::Tags, child.open_count) {
                        tag_entries.push((
                            PanelChild::SubTag { name: child.name.clone(), parent: tag.name.clone() },
                            tag_entry(child, true, None),
                        ));
                    }
                }
            }
        }
    }
    let any_tags = tags.iter().any(|t| shows(SmartList::Tags, t.open_count))
        || active_drag.is_some_and(|d| matches!(d, ActiveDragItem::Tag(_)));
    let tags_section_dragged = active_drag == Some(&ActiveDragItem::SidebarSection(SidebarSection::Tags));
    let tags_heading: AnyElement = if tags_section_dragged {
        vacant_slot(36., false, "tags-heading".to_owned(), cx)
    } else {
        let (on_add, on_menu, on_fold, on_start) =
            (on_panel.clone(), on_panel.clone(), on_panel.clone(), on_panel.clone());
        let hover = |el: Stateful<Div>| {
            el.when(!crate::touch::active() || active_drag.is_some(), |el| el.invisible())
                .when(active_drag.is_none(), |el| el.group_hover("tags-heading", |s| s.visible()))
        };
        let dragged_section = DraggedSidebarSection { section: SidebarSection::Tags, label: strings.tags.into() };
        div()
            .id("tags-heading-drop")
            .debug_selector(|| "tags-heading".to_owned())
            .relative()
            .rounded(px(6.))
            .on_drag(dragged_section, move |d, cursor_offset, window, cx| {
                on_start(
                    PanelEvent::StartDrag {
                        item: ActiveDragItem::SidebarSection(SidebarSection::Tags),
                        grab_offset_y: f32::from(cursor_offset.y).clamp(0., 36.),
                    },
                    window,
                    cx,
                );
                cx.new(|_| d.clone())
            })
            .child(
                h_flex()
                    .id("tags-heading")
                    .group("tags-heading")
                    .h(px(36.))
                    .pt_1()
                    .px_2()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(move |_, window, cx| on_fold(PanelEvent::ToggleTags, window, cx))
                    .relative()
                    // The fold arrow shows left of the word on hover, as the original's, without moving it.
                    .child(hover(
                        div()
                            .id("tags-fold")
                            .group("tags-fold")
                            .absolute()
                            .left(px(-4.))
                            .top(px(7.))
                            .child(
                                Icon::new(if tags_collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                                    .size(IconSize::XSmall)
                                    .color(colors.list_text_muted)
                                    .group_hover_color("tags-fold", colors.list_text),
                            ),
                    ))
                    .child(div().flex_1().child(Label::new(strings.tags).size(LabelSize::Small).color(colors.list_text_muted)))
                    .child(hover(
                        div()
                            .id("tags-menu")
                            .group("tags-menu")
                            .tooltip(Tooltip::text(strings.more))
                            .child(
                                Icon::new(IconName::Ellipsis)
                                    .size(IconSize::Small)
                                    .color(colors.list_text_muted)
                                    .group_hover_color("tags-menu", colors.list_text),
                            )
                            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                                cx.stop_propagation();
                                on_menu(PanelEvent::TagsMenu(event.position), window, cx)
                            }),
                    ))
                    .child(
                        hover(div().id("add-tag")).child(
                            IconButton::new("add-tag-button", IconName::Plus)
                                .size(px(20.))
                                .icon_size(IconSize::Small)
                                .icon_color(colors.list_text_muted)
                                .hover_icon_color(colors.list_text)
                                .hover_color(colors.list_hover)
                                .tooltip(Tooltip::text(strings.add_tags))
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    on_add(PanelEvent::AddTag, window, cx)
                                }),
                        ),
                    ),
            )
            .into_any_element()
    };

    let lists_section_dragged = active_drag == Some(&ActiveDragItem::SidebarSection(SidebarSection::Lists));
    let lists_heading: AnyElement = if lists_section_dragged {
        vacant_slot(36., false, "lists-heading".to_owned(), cx)
    } else {
        let on_start = on_panel.clone();
        let dragged_section = DraggedSidebarSection { section: SidebarSection::Lists, label: strings.lists.into() };
        div()
            .id("lists-heading")
            .debug_selector(|| "lists-heading".to_owned())
            .relative()
            .rounded(px(6.))
            .on_drag(dragged_section, move |d, cursor_offset, window, cx| {
                on_start(
                    PanelEvent::StartDrag {
                        item: ActiveDragItem::SidebarSection(SidebarSection::Lists),
                        grab_offset_y: f32::from(cursor_offset.y).clamp(0., 36.),
                    },
                    window,
                    cx,
                );
                cx.new(|_| d.clone())
            })
            .child(
                heading(strings.lists).child(on_hover_only(
                    strings.lists,
                    IconButton::new("add-list", IconName::Plus)
                        .size(px(20.))
                        .icon_size(IconSize::Small)
                        .icon_color(colors.list_text_muted)
                        .hover_icon_color(colors.list_text)
                        .hover_color(colors.list_hover)
                        .tooltip(Tooltip::text(strings.add_list))
                        .on_click({
                            let on_add_list = on_add_list.clone();
                            move |ev, window, cx| on_add_list(ev, window, cx)
                        }),
                )),
            )
            .into_any_element()
    };

    let filters_section_dragged = active_drag == Some(&ActiveDragItem::SidebarSection(SidebarSection::Filters));
    let filters_heading: AnyElement = if filters_section_dragged {
        vacant_slot(36., false, "filters-heading".to_owned(), cx)
    } else {
        let on_start = on_panel.clone();
        let dragged_section = DraggedSidebarSection { section: SidebarSection::Filters, label: strings.filters.into() };
        div()
            .id("filters-heading")
            .debug_selector(|| "filters-heading".to_owned())
            .relative()
            .rounded(px(6.))
            .on_drag(dragged_section, move |d, cursor_offset, window, cx| {
                on_start(
                    PanelEvent::StartDrag {
                        item: ActiveDragItem::SidebarSection(SidebarSection::Filters),
                        grab_offset_y: f32::from(cursor_offset.y).clamp(0., 36.),
                    },
                    window,
                    cx,
                );
                cx.new(|_| d.clone())
            })
            .child(
                heading(strings.filters).child(on_hover_only(
                    strings.filters,
                    IconButton::new("add-filter", IconName::Plus)
                        .size(px(20.))
                        .icon_size(IconSize::Small)
                        .icon_color(colors.list_text_muted)
                        .hover_icon_color(colors.list_text)
                        .hover_color(colors.list_hover)
                        .tooltip(Tooltip::text(strings.add_filter))
                        .on_click({
                            let on_add_filter = on_add_filter.clone();
                            move |ev, window, cx| on_add_filter(ev, window, cx)
                        }),
                )),
            )
            .into_any_element()
    };

    let filter_entries: Vec<(PanelChild, AnyElement)> = filters
        .iter()
        .map(|(filter, count)| {
            let id = filter.id;
            if active_drag == Some(&ActiveDragItem::Filter(id)) {
                return (
                    PanelChild::Filter(id),
                    vacant_slot(38., false, format!("lists-item-filter-{id}"), cx),
                );
            }
            let on_start = on_panel.clone();
            let dragged = DraggedFilter { id, name: filter.name.clone().into() };
            let el = item(
                format!("filter-{id}"),
                View::Filter(id),
                filter.name.clone().into(),
                Icon::new(IconName::Funnel).color(colors.list_text).into_any_element(),
                Some(*count),
                None,
                None,
                false,
            )
            .on_drag(dragged, move |d, cursor_offset, window, cx| {
                on_start(
                    PanelEvent::StartDrag {
                        item: ActiveDragItem::Filter(id),
                        grab_offset_y: f32::from(cursor_offset.y).clamp(0., 38.),
                    },
                    window,
                    cx,
                );
                cx.new(|_| d.clone())
            })
            .into_any_element();
            (PanelChild::Filter(id), el)
        })
        .collect();

    let mut lists_section = Some((lists_heading, list_entries));
    let mut tags_section = any_tags.then_some((tags_heading, tag_entries));
    let mut filters_section = shows(SmartList::Filters, 1).then_some((filters_heading, filter_entries));

    let mut rows = rows_out.borrow_mut();
    rows.clear();
    let mut lists_column = v_flex()
        .id("lists-panel")
        .pl_2()
        // Reserve room for the 6 px resize strip + inset overlay scrollbar so rows stay clear of the thumb.
        .pr(px(16.))
        .pb_2();

    let mut push_row = |col: Stateful<Div>, kind: PanelChild, el: AnyElement| -> Stateful<Div> {
        rows.push(kind);
        col.child(el)
    };

    // Top smart lists in user-chosen order.
    let smart_order: &[SmartList] = if preferences.smart_list_order.is_empty() {
        &SmartList::TOP_DRAGGABLE
    } else {
        &preferences.smart_list_order
    };
    for &which in smart_order {
        let entry = match which {
            SmartList::Inbox if shows(SmartList::Inbox, counts.inbox) => Some(draggable_smart(
                SmartList::Inbox,
                View::Inbox,
                strings.inbox,
                IconName::Inbox,
                Some(counts.inbox),
            )),
            SmartList::All if shows(SmartList::All, counts.all) => Some(draggable_smart(
                SmartList::All,
                View::All,
                strings.all,
                IconName::Layers,
                Some(counts.all),
            )),
            SmartList::Today if shows(SmartList::Today, counts.today) => Some(draggable_smart(
                SmartList::Today,
                View::Today,
                strings.today,
                IconName::CalendarCheck,
                Some(counts.today),
            )),
            SmartList::Tomorrow if shows(SmartList::Tomorrow, counts.tomorrow) => Some(draggable_smart(
                SmartList::Tomorrow,
                View::Tomorrow,
                strings.tomorrow,
                IconName::Sunrise,
                Some(counts.tomorrow),
            )),
            SmartList::Next7Days if shows(SmartList::Next7Days, counts.next_7_days) => Some(draggable_smart(
                SmartList::Next7Days,
                View::Next7Days,
                strings.next_7_days_short,
                IconName::CalendarRange,
                Some(counts.next_7_days),
            )),
            SmartList::Horizons if crate::calendar::horizons_enabled(cx) && shows(SmartList::Horizons, 1) => Some(draggable_smart(
                SmartList::Horizons,
                View::Horizons,
                strings.section_horizons,
                IconName::FernHorizons,
                None,
            )),
            SmartList::Summary if shows(SmartList::Summary, 1) => Some(draggable_smart(
                SmartList::Summary,
                View::Summary,
                strings.summary,
                IconName::FileText,
                None,
            )),
            _ => None,
        };
        if let Some(el) = entry {
            lists_column = push_row(lists_column, PanelChild::SmartList(which), el);
        }
    }

    lists_column = push_row(lists_column, PanelChild::Separator, separator());

    // Middle sections (Lists, Tags, Filters) in user-chosen order.
    let section_order: &[SidebarSection] = if preferences.sidebar_section_order.is_empty() {
        &SidebarSection::ALL
    } else {
        &preferences.sidebar_section_order
    };
    for &section in section_order {
        match section {
            SidebarSection::Lists => {
                if let Some((h, entries)) = lists_section.take() {
                    lists_column = push_row(lists_column, PanelChild::SectionHeading(SidebarSection::Lists), h);
                    if !lists_section_dragged {
                        if lists.is_empty() {
                            let on_add_list = on_add_list.clone();
                            lists_column = push_row(
                                lists_column,
                                PanelChild::Hint(SidebarSection::Lists),
                                div()
                                    .id("lists-empty-hint")
                                    .mx_1()
                                    .mb_1()
                                    .px(px(14.))
                                    .py(px(10.))
                                    .rounded(px(6.))
                                    .bg(colors.ghost_element_hover)
                                    .hover(|s| s.bg(colors.list_hover))
                                    .cursor_pointer()
                                    .tooltip(Tooltip::text(strings.add_list))
                                    .on_click(move |ev, window, cx| on_add_list(ev, window, cx))
                                    .child(Label::new(strings.lists_hint).size(LabelSize::Small).color(colors.list_text_muted))
                                    .into_any_element(),
                            );
                        }
                        for (kind, el) in entries {
                            lists_column = push_row(lists_column, kind, el);
                        }
                    }
                }
            }
            SidebarSection::Tags => {
                if let Some((h, entries)) = tags_section.take() {
                    lists_column = push_row(lists_column, PanelChild::SectionHeading(SidebarSection::Tags), h);
                    if !tags_section_dragged {
                        for (kind, el) in entries {
                            lists_column = push_row(lists_column, kind, el);
                        }
                    }
                }
            }
            SidebarSection::Filters => {
                if let Some((h, entries)) = filters_section.take() {
                    lists_column = push_row(lists_column, PanelChild::SectionHeading(SidebarSection::Filters), h);
                    if !filters_section_dragged {
                        if filters.is_empty() {
                            let on_add_filter = on_add_filter.clone();
                            lists_column = push_row(
                                lists_column,
                                PanelChild::Hint(SidebarSection::Filters),
                                div()
                                    .id("filters-empty-hint")
                                    .mx_1()
                                    .mb_1()
                                    .px(px(14.))
                                    .py(px(10.))
                                    .rounded(px(6.))
                                    .bg(colors.ghost_element_hover)
                                    .hover(|s| s.bg(colors.list_hover))
                                    .cursor_pointer()
                                    .tooltip(Tooltip::text(strings.add_filter))
                                    .on_click(move |ev, window, cx| on_add_filter(ev, window, cx))
                                    .child(Label::new(strings.filters_hint).size(LabelSize::Small).color(colors.list_text_muted))
                                    .into_any_element(),
                            );
                        }
                        for (kind, el) in entries {
                            lists_column = push_row(lists_column, kind, el);
                        }
                    }
                }
            }
        }
    }

    let any_bottom = shows(SmartList::Completed, 1) || shows(SmartList::WontDo, 1) || shows(SmartList::Trash, 1);
    if any_bottom {
        lists_column = push_row(lists_column, PanelChild::Separator, separator());
    }
    if shows(SmartList::Completed, 1) {
        lists_column = push_row(
            lists_column,
            PanelChild::BottomSmart(View::Completed),
            smart(View::Completed, strings.completed, IconName::SquareCheckBig, None).into_any_element(),
        );
    }
    if shows(SmartList::WontDo, 1) {
        lists_column = push_row(
            lists_column,
            PanelChild::BottomSmart(View::WontDo),
            smart(View::WontDo, strings.wont_do, IconName::SquareX, None).into_any_element(),
        );
    }
    if shows(SmartList::Trash, 1) {
        lists_column = push_row(
            lists_column,
            PanelChild::BottomSmart(View::Trash),
            smart(View::Trash, strings.trash, IconName::Trash, None).into_any_element(),
        );
    }
    drop(rows);

    v_flex()
        .debug_selector(|| "lists-column".to_owned())
        .flex_none()
        .w(px(width))
        .h_full()
        .pt(desktop_common::layout::title_bar_height(window, cx))
        .bg(colors.lists_background)
        .border_r_1()
        .border_color(colors.lists_border)
        .child(
            v_scroll("lists-panel-scroll", lists_column)
                .track_scroll(scroll_handle)
                .inset(px(6.))
                .color(colors.list_text)
                .pb_2()
                .flex_1()
                .min_h_0(),
        )
        .when(preferences.mini_calendar, |el| el.child(mini_calendar(active, mini, on_select.clone(), on_mini, strings, cx)))
}

/// What the mini calendar shows: the week, or — expanded with its hover tab — a month.
pub struct MiniCalendar<'a> {
    pub expanded: bool,
    /// The first day of the expanded month.
    pub month: NaiveDate,
    /// Days with open tasks, marked with a dot when expanded.
    pub busy: &'a HashSet<NaiveDate>,
}

/// The mini calendar's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiniAction {
    /// The hover tab: expand to a month or collapse to the week.
    Toggle,
    /// The month arrows.
    Shift(i32),
    /// The circle between them: back to this month.
    ThisMonth,
}

/// The calendar pinned to the bottom of the column, as in TickTick: the current week, or a month
/// with arrows once expanded. Clicking a day opens its tasks (`View::Day`); that day stays marked.
fn mini_calendar(
    active: &View,
    mini: MiniCalendar,
    on_select: impl Fn(View, &mut Window, &mut App) + Clone + 'static,
    on_mini: impl Fn(MiniAction, &mut Window, &mut App) + Clone + 'static,
    strings: &Strings,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    let today = Local::now().date_naive();
    let day_cell = |date: NaiveDate, faded: bool| {
        let is_today = date == today;
        let selected = *active == View::Day(date);
        let filled = selected || (is_today && !matches!(active, View::Day(_)));
        let busy = mini.busy.contains(&date);
        let on_select = on_select.clone();
        // Columns of equal width that may be narrower than the circle (the column is), so the
        // letters stay over the numbers.
        div().flex_1().min_w_0().flex().justify_center().child(
            v_flex()
                .id(SharedString::from(format!("mini-day-{date}")))
                .relative()
                .flex_none()
                .size(px(26.))
                .cursor_pointer()
                .when(!filled, |el| el.hover(|s| s.bg(colors.list_hover)))
                .when(filled, |el| el.bg(colors.accent).hover(|s| s.opacity(0.9)))
                .tooltip(Tooltip::text(calendar::format_date(date, strings, cx)))
                .on_click(move |_, window, cx| on_select(View::Day(date), window, cx))
                .items_center()
                .justify_center()
                .rounded_full()
                .when(is_today && !filled, |el| el.bg(colors.list_hover))
                .child(
                    Label::new(date.day().to_string())
                        .centered()
                        .size(LabelSize::Small)
                        .color(if filled {
                            Color::OnAccent
                        } else if is_today {
                            Color::Accent
                        } else if faded {
                            Color::Custom(colors.list_text_muted)
                        } else {
                            Color::Custom(colors.list_text)
                        }),
                )
                .when(busy, |el| el.child(div().absolute().bottom(px(2.)).size(px(3.)).rounded_full().bg(if filled { colors.icon_on_accent } else { colors.accent }))),
        )
    };
    let weekdays = h_flex().children(crate::calendar::weekday_labels(&strings.weekdays_short, cx).iter().map(|day| {
        div().flex_1().min_w_0().flex().justify_center().child(Label::new(*day).size(LabelSize::XSmall).color(colors.list_text_muted))
    }));
    let toggle = on_mini.clone();
    // The hover tab hanging down from the top border: "^" expands, "v" collapses.
    let tab = div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .when(!crate::touch::active(), |el| el.invisible())
        .group_hover("mini-calendar", |s| s.visible())
        .child(
            div()
                .id("mini-calendar-toggle")
                .group("mini-calendar-toggle")
                .w(px(36.))
                .h(px(12.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_bl(px(4.))
                .rounded_br(px(4.))
                .bg(colors.list_hover)
                .cursor_pointer()
                .tooltip(Tooltip::text(if mini.expanded { strings.habit_collapse } else { strings.habit_expand }))
                .child(
                    Icon::new(if mini.expanded { IconName::ChevronDown } else { IconName::ChevronUp })
                        .color(colors.list_text_muted)
                        .group_hover_color("mini-calendar-toggle", colors.list_text)
                        .size(IconSize::XSmall),
                )
                .on_click(move |_, window, cx| toggle(MiniAction::Toggle, window, cx)),
        );
    let body = if mini.expanded {
        let on_shift = on_mini.clone();
        let on_today = on_mini;
        let mut header_cfg = desktop_common::month_calendar::MonthHeaderConfig::surface(
            mini.month,
            strings,
            "mini-back",
            "mini-forward",
        );
        header_cfg.today_id = Some("mini-this-month");
        header_cfg.today_tooltip = Some(strings.today);
        header_cfg.small_title = true;
        header_cfg.title_weight = gpui::FontWeight::NORMAL;
        header_cfg.sidebar_palette = true;
        header_cfg.arrow_size = px(20.);
        v_flex()
            .gap_1()
            .child(desktop_common::month_calendar::render_month_header(
                header_cfg,
                cx,
                move |delta, window, cx| on_shift(MiniAction::Shift(delta), window, cx),
                Some(Box::new(move |window, cx| {
                    on_today(MiniAction::ThisMonth, window, cx)
                })),
            ))
            .child(weekdays)
            .children(desktop_common::month_calendar::render_month_weeks(
                mini.month,
                cx,
                |date, in_month| day_cell(date, !in_month).into_any_element(),
            ))
    } else {
        v_flex().gap_2().child(weekdays).child(h_flex().children(calendar::week_of(today, cx).map(|date| day_cell(date, false))))
    };
    v_flex()
        .id("mini-calendar")
        .group("mini-calendar")
        .relative()
        .flex_none()
        .bg(colors.lists_background)
        // GPUI paints borders after children. Put the border on the body beneath the tab.
        .child(body.py_2().px_2().border_t_1().border_color(colors.border))
        .child(tab)
}
