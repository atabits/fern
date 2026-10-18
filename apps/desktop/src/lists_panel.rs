//! The lists column between the narrow sidebar and the task list, modeled on TickTick's.

use std::collections::HashSet;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Id, List, ListFolder, Preferences, SmartList, Tag, View, ViewCounts};
use gpui::{ClickEvent, MouseButton, Pixels, Point, rgb};
use ui::prelude::*;

use crate::calendar;
use crate::drag::{DraggedList, DraggedTask};
use crate::i18n::Strings;

pub const WIDTH: f32 = 212.;

/// Colors offered for lists, as stored in the database.
pub const LIST_COLORS: [&str; 8] =
    ["#EF4444", "#F97316", "#EAB308", "#22C55E", "#06B6D4", "#3B82F6", "#A855F7", "#EC4899"];

/// The title of a view for the list header.
pub fn title(view: &View, lists: &[List], filters: &[fern_core::SmartFilter], strings: &Strings) -> SharedString {
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
        View::Summary => strings.summary.into(),
        View::Day(day) => calendar::format_date(*day, strings).into(),
    }
}

/// A tag or list color parsed from `#RRGGBB`.
pub fn parse_color(value: Option<&str>) -> Option<Hsla> {
    value
        .and_then(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok())
        .map(|c| rgb(c).into())
}

/// A tag or list color, or the muted text color when unset.
pub fn color(value: Option<&str>, cx: &App) -> Hsla {
    parse_color(value).unwrap_or(cx.theme().colors().text_muted)
}

pub struct Props<'a> {
    pub active: &'a View,
    pub counts: &'a ViewCounts,
    pub lists: &'a [List],
    pub tags: &'a [Tag],
    pub strings: &'static Strings,
    /// The folders of lists, and the ones folded up.
    pub folders: &'a [ListFolder],
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
    /// A list dropped after `after` (first when `None`), into `folder`.
    DropList { list: Id, after: Option<Id>, folder: Option<Id> },
    /// A list dropped on a folder's row: into it, after its lists.
    DropListInFolder { list: Id, folder: Id },
}

pub fn render(
    props: Props,
    on_select: impl Fn(View, &mut Window, &mut App) + Clone + 'static,
    on_panel: impl Fn(PanelEvent, &mut Window, &mut App) + Clone + 'static,
    on_add_list: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_drop_task: impl Fn(View, fern_core::Id, &mut Window, &mut App) + Clone + 'static,
    on_add_filter: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_mini: impl Fn(MiniAction, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    let Props { active, counts, lists, tags, strings, folders, collapsed_folders, archived_open, collapsed_tags, tags_collapsed, filters, preferences, mini } = props;
    let shows = |list: SmartList, count: usize| preferences.visibility(list).shows(count);
    let colors = cx.theme().colors();

    let item = |id: String,
                view: View,
                label: SharedString,
                icon: AnyElement,
                count: Option<usize>,
                dot: Option<Hsla>,
                menu: Option<SmartList>| {
        let on_select = on_select.clone();
        let on_panel = on_panel.clone();
        let group = SharedString::from(format!("row-{id}"));
        let on_drop_task = on_drop_task.clone();
        let drop_view = view.clone();
        let accent = colors.accent;
        // Every entry is also a drop target for dragged tasks.
        let item = ListItem::new(SharedString::from(id.clone()))
            .height(px(38.))
            .toggle_state(&view == active)
            .start_slot(icon)
            .child(Label::new(label).truncate())
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
                                .when(menu.is_some(), |el| el.group_hover(group.clone(), |s| s.invisible()))
                                .child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)),
                        )
                    })
                    // As the original's, a hovered smart list shows "…" where its count is.
                    .when_some(menu, |el, list| {
                        el.child(
                            div()
                                .id("smart-more")
                                .absolute()
                                .right(px(10.))
                                .invisible()
                                .group_hover(group.clone(), |s| s.visible())
                                .child(Icon::new(IconName::Ellipsis).size(IconSize::Small).color(Color::Muted))
                                .on_mouse_down(gpui::MouseButton::Left, move |event: &gpui::MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    on_panel(PanelEvent::SmartMenu(list, event.position), window, cx)
                                }),
                        )
                    }),
            )
            .on_click(move |_, window, cx| on_select(view.clone(), window, cx));
        div()
            .id(SharedString::from(format!("drop-{id}")))
            .group(group)
            .relative()
            .rounded(px(6.))
            .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.18)))
            .on_drop(move |dragged: &DraggedTask, window, cx| on_drop_task(drop_view.clone(), dragged.id, window, cx))
            .child(item)
    };
    let smart = |view: View, label: &'static str, icon: IconName, count: Option<usize>| {
        // Every smart list but the Inbox can be hidden from its "…", as in the original.
        let menu = match view {
            View::All => Some(SmartList::All),
            View::Today => Some(SmartList::Today),
            View::Tomorrow => Some(SmartList::Tomorrow),
            View::Next7Days => Some(SmartList::Next7Days),
            View::Summary => Some(SmartList::Summary),
            _ => None,
        };
        item(format!("view-{label}"), view, label.into(), Icon::new(icon).into_any_element(), count, None, menu)
    };
    // As in the original: a row every 38 px, sections apart with a 12 px gap around their line,
    // and a section's "+" shown on hover.
    let separator = || div().my_3().mx_2().child(Divider::horizontal());
    let heading = |text: &'static str| {
        h_flex()
            .id(SharedString::from(format!("heading-{text}")))
            .group(SharedString::from(format!("heading-{text}")))
            .h(px(36.))
            .pt_1()
            .px_2()
            .child(div().flex_1().child(Label::new(text).size(LabelSize::Small).color(Color::Muted)))
    };
    let on_hover_only = |text: &'static str, button: IconButton| {
        div().invisible().group_hover(SharedString::from(format!("heading-{text}")), |s| s.visible()).child(button)
    };

    // The lists in their order; a folder sits where its first list is (empty ones last), its
    // lists indented under it unless it is folded up, as in the original.
    let list_entry = |list: &List, indent: bool, muted: bool| -> AnyElement {
        let id = list.id;
        let entry = item(
            format!("list-{id}"),
            View::List(id),
            list.name.clone().into(),
            match &list.icon {
                Some(icon) => div().w(px(16.)).flex().justify_center().text_size(px(14.)).child(icon.clone()).into_any_element(),
                None => Icon::new(IconName::List).into_any_element(),
            },
            Some(list.open_count),
            parse_color(list.color.as_deref()),
            None,
        );
        let on_panel = on_panel.clone();
        let on_drop = on_panel.clone();
        let folder = list.folder;
        let accent = colors.accent;
        let dragged = DraggedList { id, name: list.name.clone().into() };
        div()
            .id(SharedString::from(format!("list-entry-{id}")))
            .when(indent, |el| el.pl(px(14.)))
            .when(muted, |el| el.opacity(0.6))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .drag_over::<DraggedList>(move |style, _, _, _| style.border_b_2().border_color(accent))
            .on_drop(move |dragged: &DraggedList, window, cx| {
                if dragged.id != id {
                    on_drop(PanelEvent::DropList { list: dragged.id, after: Some(id), folder }, window, cx)
                }
            })
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                cx.stop_propagation();
                on_panel(PanelEvent::ListMenu(id, event.position), window, cx)
            })
            .child(entry)
            .into_any_element()
    };
    let folder_entry = |folder: &ListFolder, inside: &[&List]| -> AnyElement {
        let id = folder.id;
        let collapsed = collapsed_folders.contains(&id);
        let count: usize = inside.iter().map(|l| l.open_count).sum();
        let on_panel = on_panel.clone();
        let on_menu = on_panel.clone();
        let on_drop = on_panel.clone();
        let accent = colors.accent;
        div()
            .id(SharedString::from(format!("folder-{id}")))
            .rounded(px(6.))
            .drag_over::<DraggedList>(move |style, _, _, _| style.bg(accent.opacity(0.18)))
            .on_drop(move |dragged: &DraggedList, window, cx| on_drop(PanelEvent::DropListInFolder { list: dragged.id, folder: id }, window, cx))
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                cx.stop_propagation();
                on_menu(PanelEvent::FolderMenu(id, event.position), window, cx)
            })
            .child(
                ListItem::new(SharedString::from(format!("folder-item-{id}")))
                    .height(px(36.))
                    .start_slot(Icon::new(if collapsed { IconName::Folder } else { IconName::FolderOpen }))
                    .child(Label::new(folder.name.clone()).truncate())
                    .end_slot(
                        h_flex()
                            .gap_1()
                            .when(collapsed && count > 0, |el| el.child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)))
                            .child(Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall).color(Color::Muted)),
                    )
                    .on_click(move |_, window, cx| on_panel(PanelEvent::ToggleFolder(id), window, cx)),
            )
            .into_any_element()
    };
    let active_lists: Vec<&List> = lists.iter().filter(|l| !l.archived).collect();
    let mut list_entries: Vec<AnyElement> = Vec::new();
    let mut shown_folders: HashSet<Id> = HashSet::new();
    for list in &active_lists {
        match list.folder.and_then(|f| folders.iter().find(|x| x.id == f)) {
            None => list_entries.push(list_entry(list, false, false)),
            Some(folder) if shown_folders.insert(folder.id) => {
                let inside: Vec<&List> = active_lists.iter().copied().filter(|l| l.folder == Some(folder.id)).collect();
                list_entries.push(folder_entry(folder, &inside));
                if !collapsed_folders.contains(&folder.id) {
                    list_entries.extend(inside.iter().map(|l| list_entry(l, true, false)));
                }
            }
            Some(_) => {}
        }
    }
    for folder in folders.iter().filter(|f| !shown_folders.contains(&f.id)) {
        list_entries.push(folder_entry(folder, &[]));
    }
    let archived: Vec<&List> = lists.iter().filter(|l| l.archived).collect();
    if !archived.is_empty() {
        let on_panel = on_panel.clone();
        list_entries.push(
            ListItem::new("archived-lists")
                .height(px(36.))
                .start_slot(Icon::new(IconName::Archive))
                .child(Label::new(strings.archived_lists))
                .end_slot(Icon::new(if archived_open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::XSmall).color(Color::Muted))
                .on_click(move |_, window, cx| on_panel(PanelEvent::ToggleArchived, window, cx))
                .into_any_element(),
        );
        if archived_open {
            list_entries.extend(archived.iter().map(|l| list_entry(l, true, true)));
        }
    }

    // Tags as in the original: pinned ones first, sub-tags indented under their parent (which
    // folds them up), a right click for the tag's menu; the heading shows "…" and "+" on hover.
    let family_count = |tag: &Tag| tag.open_count + tags.iter().filter(|t| t.parent.as_ref() == Some(&tag.name)).map(|t| t.open_count).sum::<usize>();
    let tag_entry = |tag: &Tag, indent: bool, fold: Option<bool>| -> AnyElement {
        let name = tag.name.clone();
        let on_menu = on_panel.clone();
        let on_fold = on_panel.clone();
        let count = if fold == Some(true) { family_count(tag) } else { tag.open_count };
        let menu_name = name.clone();
        let fold_name = name.clone();
        div()
            .id(SharedString::from(format!("tag-entry-{name}")))
            .relative()
            .when(indent, |el| el.pl(px(14.)))
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                cx.stop_propagation();
                on_menu(PanelEvent::TagMenu(menu_name.clone(), event.position), window, cx)
            })
            .child(item(
                format!("tag-{name}"),
                View::Tag(name.clone()),
                name.clone().into(),
                Icon::new(IconName::Tag).into_any_element(),
                Some(count),
                Some(color(tag.color.as_deref(), cx)),
                None,
            ))
            .when_some(fold, |el, folded| {
                // A parent's arrow at the left edge, as the original's folders of tags have.
                el.child(
                    div()
                        .id(SharedString::from(format!("tag-fold-{name}")))
                        .absolute()
                        .left(px(-6.))
                        .top(px(11.))
                        .cursor_pointer()
                        .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(move |_, window, cx| on_fold(PanelEvent::ToggleTag(fold_name.clone()), window, cx)),
                )
            })
            .into_any_element()
    };
    let mut ordered: Vec<&Tag> = tags.iter().filter(|t| t.parent.is_none() || !tags.iter().any(|p| Some(&p.name) == t.parent.as_ref())).collect();
    ordered.sort_by_key(|t| !t.pinned);
    let mut tag_entries: Vec<AnyElement> = Vec::new();
    if !tags_collapsed {
        for tag in ordered {
            let children: Vec<&Tag> = tags.iter().filter(|t| t.parent.as_ref() == Some(&tag.name)).collect();
            if !shows(SmartList::Tags, family_count(tag)) {
                continue;
            }
            let folded = collapsed_tags.contains(&tag.name);
            tag_entries.push(tag_entry(tag, false, (!children.is_empty()).then_some(folded)));
            if !folded {
                tag_entries.extend(children.into_iter().filter(|t| shows(SmartList::Tags, t.open_count)).map(|t| tag_entry(t, true, None)));
            }
        }
    }
    let any_tags = tags.iter().any(|t| shows(SmartList::Tags, t.open_count));
    let tags_heading = {
        let (on_add, on_menu, on_fold) = (on_panel.clone(), on_panel.clone(), on_panel.clone());
        let hover = |el: Stateful<Div>| el.invisible().group_hover("tags-heading", |s| s.visible());
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
                    .absolute()
                    .left(px(-4.))
                    .top(px(7.))
                    .child(Icon::new(if tags_collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall).color(Color::Muted)),
            ))
            .child(div().flex_1().child(Label::new(strings.tags).size(LabelSize::Small).color(Color::Muted)))
            .child(hover(
                div()
                    .id("tags-menu")
                    .child(Icon::new(IconName::Ellipsis).size(IconSize::Small).color(Color::Muted))
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
                        .tooltip(Tooltip::text(strings.add_tags))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_add(PanelEvent::AddTag, window, cx)
                        }),
                ),
            )
            .into_any_element()
    };
    let mut tags_block = any_tags.then_some((tags_heading, tag_entries));
    let lists_column = v_flex()
        .id("lists-panel")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px_2()
        .py_2()
        .when(shows(SmartList::Inbox, counts.inbox), |el| {
            el.child(smart(View::Inbox, strings.inbox, IconName::Inbox, Some(counts.inbox)))
        })
        .when(shows(SmartList::All, counts.all), |el| {
            el.child(smart(View::All, strings.all, IconName::Layers, Some(counts.all)))
        })
        .when(shows(SmartList::Today, counts.today), |el| {
            el.child(smart(View::Today, strings.today, IconName::CalendarCheck, Some(counts.today)))
        })
        .when(shows(SmartList::Tomorrow, counts.tomorrow), |el| {
            el.child(smart(View::Tomorrow, strings.tomorrow, IconName::Sunrise, Some(counts.tomorrow)))
        })
        .when(shows(SmartList::Next7Days, counts.next_7_days), |el| {
            el.child(smart(
                View::Next7Days,
                strings.next_7_days_short,
                IconName::CalendarRange,
                Some(counts.next_7_days),
            ))
        })
        .when(shows(SmartList::Summary, 1), |el| el.child(smart(View::Summary, strings.summary, IconName::FileText, None)))
        .child(separator())
        // As the original's: without lists, Tags come before the Lists hint.
        .when(lists.is_empty(), |el| match tags_block.take() {
            Some((heading, entries)) => el.child(heading).children(entries),
            None => el,
        })
        .child({
            // Dropping a list on the heading puts it first, out of any folder.
            let on_panel = on_panel.clone();
            let accent = colors.accent;
            div()
                .id("lists-heading")
                .rounded(px(6.))
                .drag_over::<DraggedList>(move |style, _, _, _| style.bg(accent.opacity(0.18)))
                .on_drop(move |dragged: &DraggedList, window, cx| {
                    on_panel(PanelEvent::DropList { list: dragged.id, after: None, folder: None }, window, cx)
                })
                .child(
                    heading(strings.lists).child(on_hover_only(
                        strings.lists,
                        IconButton::new("add-list", IconName::Plus)
                            .size(px(20.))
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text(strings.add_list))
                            .on_click(on_add_list),
                    )),
                )
        })
        // As in the original, a hint until there is a list; Lists, Tags and Filters follow one
        // another without lines between them.
        .when(lists.is_empty(), |el| {
            el.child(
                div()
                    .mx_1()
                    .mb_1()
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(px(6.))
                    .bg(colors.surface_background)
                    .child(Label::new(strings.lists_hint).size(LabelSize::Small).color(Color::Muted)),
            )
        })
        .children(list_entries)
        .when_some(tags_block.take(), |el, (heading, entries)| el.child(heading).children(entries))
        // Filters, as in the original: a hint until there is one.
        .when(shows(SmartList::Filters, 1), |el| el.child(
            heading(strings.filters).child(on_hover_only(
                strings.filters,
                IconButton::new("add-filter", IconName::Plus)
                    .size(px(20.))
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text(strings.add_filter))
                    .on_click(on_add_filter),
            )),
        ))
        .when(filters.is_empty() && shows(SmartList::Filters, 1), |el| {
            el.child(
                div()
                    .mx_1()
                    .mb_1()
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(px(6.))
                    .bg(colors.surface_background)
                    .child(Label::new(strings.filters_hint).size(LabelSize::Small).color(Color::Muted)),
            )
        })
        .children(filters.iter().filter(|_| shows(SmartList::Filters, 1)).map(|(filter, count)| {
            item(
                format!("filter-{}", filter.id),
                View::Filter(filter.id),
                filter.name.clone().into(),
                Icon::new(IconName::Funnel).into_any_element(),
                Some(*count),
                None,
                None,
            )
        }))
        .when(shows(SmartList::Filters, 1), |el| el.child(separator()))
        .when(shows(SmartList::Completed, 1), |el| {
            el.child(smart(View::Completed, strings.completed, IconName::SquareCheckBig, None))
        })
        .when(shows(SmartList::WontDo, 1), |el| el.child(smart(View::WontDo, strings.wont_do, IconName::SquareX, None)))
        .when(shows(SmartList::Trash, 1), |el| el.child(smart(View::Trash, strings.trash, IconName::Trash, None)));

    v_flex()
        .flex_none()
        .w(px(WIDTH))
        .pt(px(crate::TOP_INSET))
        .h_full()
        .bg(colors.panel_background)
        .border_r_1()
        .border_color(colors.border)
        .child(lists_column)
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
        let busy = mini.expanded && mini.busy.contains(&date);
        let on_select = on_select.clone();
        // Columns of equal width that may be narrower than the circle (the column is), so the
        // letters stay over the numbers.
        div().flex_1().min_w_0().flex().justify_center().child(
            v_flex()
                .id(SharedString::from(format!("mini-day-{date}")))
                .flex_none()
                .size(px(26.))
                .cursor_pointer()
                .hover(|s| s.bg(colors.ghost_element_hover))
                .when(selected, |el| el.bg(colors.accent))
                .on_click(move |_, window, cx| on_select(View::Day(date), window, cx))
                .items_center()
                .justify_center()
                .rounded_full()
                .when(is_today && !selected, |el| el.bg(colors.element_hover))
                .child(
                    Label::new(date.day().to_string())
                        .size(LabelSize::Small)
                        .color(if selected {
                            Color::Custom(gpui::white())
                        } else if is_today {
                            Color::Accent
                        } else if faded {
                            Color::Muted
                        } else {
                            Color::Default
                        }),
                )
                .child(div().size(px(3.)).rounded_full().when(busy, |el| el.bg(colors.accent))),
        )
    };
    let weekdays = h_flex().children(crate::calendar::weekday_labels(&strings.weekdays_narrow).iter().map(|day| {
        div().flex_1().min_w_0().flex().justify_center().child(Label::new(*day).size(LabelSize::Small).color(Color::Muted))
    }));
    let toggle = on_mini.clone();
    // The hover tab on the top border: "^" expands, "v" collapses.
    let tab = div()
        .absolute()
        .top(px(-9.))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .invisible()
        .group_hover("mini-calendar", |s| s.visible())
        .child(
            div()
                .id("mini-calendar-toggle")
                .w(px(40.))
                .h(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .bg(colors.element_hover)
                .cursor_pointer()
                .child(Icon::new(if mini.expanded { IconName::ChevronDown } else { IconName::ChevronUp }).size(IconSize::XSmall))
                .on_click(move |_, window, cx| toggle(MiniAction::Toggle, window, cx)),
        );
    let body = if mini.expanded {
        let arrows = |id: &'static str, icon: IconName, action: MiniAction| {
            let on_mini = on_mini.clone();
            IconButton::new(id, icon).size(px(20.)).icon_size(IconSize::XSmall).on_click(move |_, window, cx| on_mini(action, window, cx))
        };
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(calendar::month_title(mini.month, strings)).size(LabelSize::Small)))
                    .child(arrows("mini-back", IconName::ChevronLeft, MiniAction::Shift(-1)))
                    .child(arrows("mini-this-month", IconName::Circle, MiniAction::ThisMonth))
                    .child(arrows("mini-forward", IconName::ChevronRight, MiniAction::Shift(1))),
            )
            .child(weekdays)
            .children(calendar::month_grid(mini.month).chunks(7).map(|week| {
                h_flex().children(week.iter().map(|&date| day_cell(date, date.month() != mini.month.month())))
            }))
    } else {
        v_flex().gap_2().child(weekdays).child(h_flex().children(calendar::week_of(today).map(|date| day_cell(date, false))))
    };
    v_flex()
        .id("mini-calendar")
        .group("mini-calendar")
        .relative()
        .flex_none()
        .py_2()
        .px_2()
        .border_t_1()
        .border_color(colors.border)
        .child(tab)
        .child(body)
}
