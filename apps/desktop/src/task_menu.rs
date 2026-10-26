//! A task's right-click menu, as the original's, shared by the task list and the calendar: the
//! Date and Priority rows of icons, then the task's actions. The views decide what the actions
//! that open pickers do; the edits to the task itself are applied here.

use std::rc::Rc;

use chrono::{Duration, NaiveDate};
use fern_core::{Id, Kind, List, Priority, Store, Task};
use ui::prelude::*;
use ui::{Divider, SubmenuItem, popover};

use crate::i18n::Strings;
use crate::task_rows;

/// What a choice in the menu asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Due in this many days; `None` is "a date of one's own", the date picker.
    Due(Option<i64>),
    /// The original's "Skip the Recurrence" for a repeating task, and "Clear" for a dated one.
    SkipRecurrence,
    ClearDate,
    Priority(Priority),
    AddSubtask,
    LinkParent,
    Pin,
    WontDo,
    Move(Option<Id>),
    Tags,
    /// Start Focus, with the stopwatch or not.
    Focus(bool),
    Duplicate,
    CopyLink,
    Convert,
    Delete,
    /// A side menu opened (0 Move to, 1 Start Focus) by hovering its item.
    Side(u8),
    Close,
}

/// The menu's items that the calendar's has not: the original's list menu also has Add Subtask
/// and Pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    List,
    Calendar,
}

/// Applies an action that only edits the task; `None` for the ones the view handles.
pub fn apply(store: &Store, task: &Task, action: MenuAction, today: NaiveDate) -> Option<anyhow::Result<()>> {
    let id = task.id;
    Some(match action {
        MenuAction::Due(Some(days)) => store.set_due(id, Some(today + Duration::days(days)), None),
        MenuAction::SkipRecurrence => store.skip_recurrence(id),
        MenuAction::ClearDate => store.set_due(id, None, None),
        MenuAction::Priority(priority) => store.set_priority(id, priority),
        MenuAction::Pin => store.set_pinned(id, task.pinned_at.is_none()),
        MenuAction::WontDo => store.set_wont_do(id, !task.is_wont_do()),
        MenuAction::Move(list) => store.set_list(id, list),
        MenuAction::Duplicate => store.duplicate_task(id).map(|_| ()),
        MenuAction::Convert => store.set_kind(id, if task.kind == Kind::Note { Kind::Task } else { Kind::Note }),
        _ => return None,
    })
}

/// The menu's content (the view places it): 196 px wide with a row every 34 px, as the
/// original's.
pub fn render<V: 'static>(
    task: &Task,
    lists: &[List],
    kind: MenuKind,
    side: Option<u8>,
    strings: &'static Strings,
    cx: &mut Context<V>,
    act: Act<V>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let heading = |label: &'static str| div().px_2().pt_1().child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted));
    let icon_button = |id: SharedString, icon: IconName, color: Color, selected: bool| {
        div()
            .id(id)
            .size(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.colors().element_selected))
            .hover(|s| s.bg(theme.colors().element_hover))
            .child(Icon::new(icon).color(color))
    };
    // As the original's: Today, Tomorrow, Next Week, then Skip the Recurrence for a repeating
    // task, Custom (the date popover), and Clear for a dated one; each with its tooltip.
    let dated = task.due_date.is_some();
    let dates = [
        Some((IconName::Sun, strings.today, MenuAction::Due(Some(0)))),
        Some((IconName::Sunrise, strings.tomorrow, MenuAction::Due(Some(1)))),
        Some((IconName::CalendarPlus, strings.next_week, MenuAction::Due(Some(7)))),
        (dated && task.repeat.is_some()).then_some((IconName::SquareArrowRight, strings.skip_recurrence, MenuAction::SkipRecurrence)),
        Some((IconName::CalendarDays, strings.custom_date, MenuAction::Due(None))),
        dated.then_some((IconName::CalendarX, strings.clear, MenuAction::ClearDate)),
    ];
    let date_row = h_flex().px_1().justify_between().children(dates.into_iter().flatten().enumerate().map(|(i, (icon, tip, action))| {
        icon_button(SharedString::from(format!("task-menu-date-{i}")), icon, Color::Muted, false)
            .tooltip(ui::Tooltip::text(tip))
            .on_click(on_click(&act, action, cx))
    }));
    let priority_row = h_flex().px_1().justify_between().children([Priority::High, Priority::Medium, Priority::Low, Priority::None].map(|priority| {
        let color = task_rows::priority_color(priority, &theme).map_or(Color::Muted, Color::Custom);
        icon_button(SharedString::from(format!("task-menu-priority-{}", priority.to_i64())), IconName::Flag, color, task.priority == priority)
            .on_click(on_click(&act, MenuAction::Priority(priority), cx))
    }));
    let item = |key: &'static str, icon: IconName, label: &'static str, action: MenuAction, cx: &mut Context<V>| {
        ListItem::new(key).height(px(34.)).start_slot(Icon::new(icon).color(Color::Muted)).child(Label::new(label)).on_click(on_click(&act, action, cx))
    };
    let pinned = task.pinned_at.is_some();
    let note = task.kind == Kind::Note;
    let move_items: Vec<ListItem> = std::iter::once((None, SharedString::new_static(strings.inbox)))
        .chain(lists.iter().map(|l| (Some(l.id), SharedString::from(l.name.clone()))))
        .map(|(list, name)| {
            ListItem::new(SharedString::from(format!("task-menu-move-{list:?}")))
                .child(Label::new(name).color(if task.list_id == list { Color::Accent } else { Color::Default }))
                .on_click(on_click(&act, MenuAction::Move(list), cx))
        })
        .collect();
    let focus_items: Vec<ListItem> = [(false, strings.start_pomo), (true, strings.start_stopwatch)]
        .into_iter()
        .map(|(stopwatch, label)| {
            ListItem::new(SharedString::from(format!("task-menu-focus-{stopwatch}"))).child(Label::new(label)).on_click(on_click(&act, MenuAction::Focus(stopwatch), cx))
        })
        .collect();
    let close = {
        let act = act.clone();
        cx.listener(move |this: &mut V, _: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<V>| act(this, MenuAction::Close, window, cx))
    };
    popover(cx)
        .min_w(px(196.))
        .on_mouse_down_out(close)
        .child(heading(strings.filter_date))
        .child(date_row)
        .child(heading(strings.filter_priority))
        .child(priority_row)
        .child(div().py_1().child(Divider::horizontal()))
        .when(kind == MenuKind::List, |el| el.child(item("task-menu-subtask", IconName::CornerDownRight, strings.add_subtask_item, MenuAction::AddSubtask, cx)))
        .child(item("task-menu-parent", IconName::ListTree, strings.link_parent, MenuAction::LinkParent, cx))
        .when(kind == MenuKind::List, |el| {
            el.child(item(
                "task-menu-pin",
                if pinned { IconName::PinOff } else { IconName::Pin },
                if pinned { strings.unpin } else { strings.pin },
                MenuAction::Pin,
                cx,
            ))
        })
        .child(item("task-menu-wont-do", IconName::SquareX, strings.mark_wont_do, MenuAction::WontDo, cx))
        .child(
            SubmenuItem::new("task-menu-move", strings.move_to)
                .icon(IconName::Inbox)
                .height(px(34.))
                .open(side == Some(0))
                .on_hover(on_hover(&act, 0, side, cx))
                .items(move_items),
        )
        .child(item("task-menu-tags", IconName::Tag, strings.tags, MenuAction::Tags, cx))
        .child(div().py_1().child(Divider::horizontal()))
        .child(
            SubmenuItem::new("task-menu-focus", strings.start_focus)
                .icon(IconName::CircleDot)
                .height(px(34.))
                .open(side == Some(1))
                .on_hover(on_hover(&act, 1, side, cx))
                .items(focus_items),
        )
        .child(div().py_1().child(Divider::horizontal()))
        .child(item("task-menu-duplicate", IconName::Copy, strings.duplicate, MenuAction::Duplicate, cx))
        .child(item("task-menu-link", IconName::Link, strings.copy_link, MenuAction::CopyLink, cx))
        .child(item("task-menu-convert", IconName::FileText, if note { strings.convert_to_task } else { strings.convert_to_note }, MenuAction::Convert, cx))
        .child(item("task-menu-delete", IconName::Trash, strings.delete, MenuAction::Delete, cx))
        .into_any_element()
}

type Act<V> = Rc<dyn Fn(&mut V, MenuAction, &mut Window, &mut Context<V>)>;

fn on_click<V: 'static>(act: &Act<V>, action: MenuAction, cx: &Context<V>) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static + use<V> {
    let (act, view) = (act.clone(), cx.entity().downgrade());
    move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
        view.update(cx, |this, cx| act(this, action, window, cx)).ok();
    }
}

fn on_hover<V: 'static>(act: &Act<V>, index: u8, side: Option<u8>, cx: &Context<V>) -> impl Fn(&bool, &mut Window, &mut App) + 'static + use<V> {
    let (act, view) = (act.clone(), cx.entity().downgrade());
    move |hovered: &bool, window: &mut Window, cx: &mut App| {
        if *hovered && side != Some(index) {
            view.update(cx, |this, cx| act(this, MenuAction::Side(index), window, cx)).ok();
        }
    }
}

/// Places the menu at a point of the window, kept inside it.
pub fn place(position: gpui::Point<Pixels>, menu: AnyElement) -> AnyElement {
    gpui::deferred(gpui::anchored().position(position).snap_to_window_with_margin(px(8.)).child(menu))
        .with_priority(1)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_apply_to_the_task() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let task = store.add_task("Water the plants")?;
        let reload = |store: &Store| store.task(task.id).unwrap().unwrap();

        apply(&store, &task, MenuAction::Due(Some(7)), today).unwrap()?;
        assert_eq!(reload(&store).due_date, Some(today + Duration::days(7)), "next week");
        apply(&store, &task, MenuAction::Priority(Priority::High), today).unwrap()?;
        assert_eq!(reload(&store).priority, Priority::High);
        apply(&store, &reload(&store), MenuAction::Pin, today).unwrap()?;
        assert!(reload(&store).pinned_at.is_some());
        apply(&store, &reload(&store), MenuAction::Convert, today).unwrap()?;
        assert_eq!(reload(&store).kind, Kind::Note);
        apply(&store, &reload(&store), MenuAction::Duplicate, today).unwrap()?;
        assert_eq!(store.tasks()?.iter().filter(|t| t.title == "Water the plants").count(), 2);
        assert!(apply(&store, &task, MenuAction::Due(None), today).is_none(), "the date picker is the view's");
        apply(&store, &reload(&store), MenuAction::ClearDate, today).unwrap()?;
        assert_eq!(reload(&store).due_date, None, "Clear");
        assert!(apply(&store, &task, MenuAction::Delete, today).is_none(), "deleting is the view's, with its Undo");
        Ok(())
    }
}
