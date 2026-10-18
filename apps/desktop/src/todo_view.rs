use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use fern_core::{Id, Kind, List, Priority, Store, Tag, Task, View, ViewCounts};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone as _};
use gpui::{ElementId, Entity, FocusHandle, Focusable, FontWeight, MouseButton};
use ui::prelude::*;

use crate::drag::DraggedTask;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use crate::actions::{
    ClearDate, DeleteTask, PinTask, PriorityHigh, PriorityLow, PriorityMedium, PriorityNone, SelectNext, SelectPrevious,
    SetDate, SetNextWeek, SetToday, SetTomorrow, ToggleComplete,
};
use crate::i18n::{self, CurrentLocale, Strings};
use crate::lists_panel::{self, LIST_COLORS};
use crate::task_detail::{OpenTask, StartFocus, TaskDetail, TaskUpdated};
use crate::task_rows::{self, ClosedRange, DropTarget, GroupBy, Row, Section, SortBy};
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

/// The share of a task row, from the left, where a dropped task goes before it; the rest of the
/// row nests it.
const NEST_ZONE: f32 = 0.6;
const DETAIL_WIDTH: f32 = 400.;
const ROW_HEIGHT: f32 = 40.;
/// Space left of the checkbox for the subtask chevron.
const GUTTER: f32 = 20.;
const INDENT: f32 = 24.;

/// An open filter dropdown of the Completed and Won't Do views. The lists filter is edited as a
/// draft until "OK", as in the original.
#[derive(Clone, Debug, PartialEq)]
enum ClosedMenu {
    Dates,
    /// "Other Date": the range being picked and the calendar shown for one of its ends.
    OtherDate { start: Option<NaiveDate>, end: Option<NaiveDate>, picking: Option<RangeEnd>, month: NaiveDate },
    Lists(Option<Vec<Option<Id>>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortMenu {
    Group,
    Sort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RangeEnd {
    Start,
    End,
}

impl gpui::EventEmitter<StartFocus> for TodoView {}

/// The filter dialog: the filter being edited and its text fields.
struct FilterEdit {
    draft: fern_core::SmartFilter,
    is_new: bool,
    /// The "Advanced" tab: conditions combined with "all" or "any".
    advanced: bool,
    name: Entity<TextInput>,
    keywords: Entity<TextInput>,
}

/// What the add field's "▾" menu set for the next task.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct AddDraft {
    priority: Option<Priority>,
    /// `Some(None)` is the Inbox.
    list: Option<Option<Id>>,
    note: bool,
    tags: Vec<String>,
    /// The date picked with the field's calendar button, with its time, reminders and repeat;
    /// a typed date wins.
    date: Option<NaiveDate>,
    time: Option<chrono::NaiveTime>,
    /// `None` keeps the default reminder from the settings.
    reminders: Option<Vec<i64>>,
    repeat: Option<fern_core::Recurrence>,
    /// Files picked with the menu's Attachment, attached once the task is added.
    files: Vec<std::path::PathBuf>,
}

pub struct TodoView {
    store: Rc<Store>,
    view: View,
    tasks: Vec<Task>,
    counts: ViewCounts,
    lists: Vec<List>,
    tags: Vec<Tag>,
    show_lists: bool,
    collapsed_tasks: HashSet<Id>,
    collapsed_sections: HashSet<Section>,
    input: Entity<TextInput>,
    /// Focus of the task list itself, for keyboard navigation (the `TaskList` key context).
    list_focus: FocusHandle,
    new_list: Entity<TextInput>,
    creating_list: bool,
    rename_input: Entity<TextInput>,
    renaming_list: bool,
    search_input: Entity<TextInput>,
    header_menu_open: bool,
    /// Filters of the Completed and Won't Do views; `None` lists means all lists.
    closed_range: ClosedRange,
    closed_lists: Option<Vec<Option<Id>>>,
    closed_menu: Option<ClosedMenu>,
    /// The header's sort menu: `Some(None)` when open, with the expanded submenu otherwise.
    sort_menu: Option<Option<SortMenu>>,
    /// The current list's "Show Details", read when the list loads.
    show_details: bool,
    /// The current list's layout: cards in columns instead of rows (the menu's View).
    kanban: bool,
    /// The original's Timeline view, and the first day it shows.
    timeline: bool,
    timeline_from: NaiveDate,
    /// The current list's sections.
    sections: Vec<fern_core::ListSection>,
    /// The "Add Section" field, while it is shown.
    new_section: Option<Entity<TextInput>>,
    /// The "Undo" toast after completing or deleting a task, with its timer.
    undo: Option<(crate::undo::UndoAction, String, gpui::Task<()>)>,
    /// Saved filters with their task counts, for the lists column.
    filters: Vec<(fern_core::SmartFilter, usize)>,
    /// Settings → Smart List, read on reload.
    preferences: fern_core::Preferences,
    /// The date quick add recognized in the field so far, shown at its end.
    quick_add_due: Option<(NaiveDate, Option<chrono::NaiveTime>)>,
    /// The mini calendar: expanded to a month, which month, and the days with open tasks.
    mini_expanded: bool,
    mini_month: NaiveDate,
    busy_days: HashSet<NaiveDate>,
    /// The Summary smart list's report.
    summary: Entity<crate::summary_view::SummaryView>,
    /// The "Add Filter" / "Edit Filter" dialog, while it is open.
    filter_edit: Option<FilterEdit>,
    /// The "List Activities" panel, shown in place of the details while open.
    list_activity: Option<Vec<(fern_core::Activity, Id, String)>>,
    /// The "Add task" field's template picker, while it is open.
    templates_open: bool,
    /// The add field's "▾" menu, its list side menu, and what it set for the next task.
    add_menu_open: bool,
    add_list_side: bool,
    add_draft: AddDraft,
    /// The "▾" menu's Tags picker while open, with the tags ticked so far, and its search field.
    add_tags: Option<Vec<String>>,
    add_tag_input: Entity<TextInput>,
    /// The "▾" menu's "Input Box Setting" chooser, while open.
    input_box_open: bool,
    /// The field's date picker, while open, with the month it shows, and its expanded row
    /// (0 Time, 1 Reminder, 2 Repeat).
    add_date_month: Option<NaiveDate>,
    add_date_row: Option<u8>,
    /// The section whose "…" menu is open.
    section_menu: Option<Id>,
    /// A section being renamed, with its field.
    renaming_section: Option<(Id, Entity<TextInput>)>,
    detail: Entity<TaskDetail>,
    selected: Option<Id>,
    error: Option<String>,
}

impl TodoView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let summary = cx.new(|_| crate::summary_view::SummaryView::new(store.clone()));
        let mini_expanded = store.setting("mini_expanded").ok().flatten().as_deref() == Some("1");
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_task, cx));
        cx.subscribe_in(&input, window, |this, _, Submitted(text), window, cx| this.submit_new_task(text, window, cx))
            .detach();
        // As the user types, the recognized date, tags and list are highlighted and the date is
        // shown at the end of the field, as in the original.
        cx.subscribe(&input, |this, _, _: &Changed, cx| this.update_quick_add_hint(cx)).detach();
        window.focus(&input.focus_handle(cx), cx);
        // Escape in the quick-add field hands the keyboard to the list.
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            window.focus(&this.list_focus, cx);
        })
        .detach();

        let new_list = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.new_list, cx));
        cx.subscribe_in(&new_list, window, |this, input, Submitted(name), window, cx| {
            this.create_list(name, cx);
            input.update(cx, |input, cx| input.clear(window, cx));
        })
        .detach();
        cx.subscribe(&new_list, |this, _, _: &Cancelled, cx| {
            this.creating_list = false;
            cx.notify();
        })
        .detach();

        let rename_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.new_list, cx));
        cx.subscribe(&rename_input, |this, _, Submitted(name), cx| this.rename_current_list(name, cx))
            .detach();
        cx.subscribe(&rename_input, |this, _, _: &Cancelled, cx| {
            this.renaming_list = false;
            cx.notify();
        })
        .detach();

        let add_tag_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.type_in_tag, cx));
        cx.subscribe(&add_tag_input, |_, _, _: &Changed, cx| cx.notify()).detach();
        // Enter ticks the typed tag, a new one too.
        cx.subscribe(&add_tag_input, |this, input, Submitted(text), cx| {
            let tag = text.trim().trim_start_matches('#').to_owned();
            if let Some(picked) = this.add_tags.as_mut()
                && !tag.is_empty()
                && !picked.iter().any(|t| t.to_lowercase() == tag.to_lowercase())
            {
                picked.push(tag);
            }
            input.update(cx, |input, cx| input.set_text("", cx));
            cx.notify();
        })
        .detach();
        let search_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.search_placeholder, cx));
        cx.subscribe(&search_input, |this, input, _: &Changed, cx| {
            let query = input.read(cx).text().to_owned();
            this.view = View::Search(query);
            this.reload(cx);
        })
        .detach();

        // Placeholders live inside the inputs, so they are updated when the language changes.
        cx.observe_global::<CurrentLocale>(|this, cx| {
            let strings = i18n::t(cx);
            this.sync_input_placeholder(cx);
            this.new_list.update(cx, |input, cx| input.set_placeholder(strings.new_list, cx));
            this.search_input.update(cx, |input, cx| input.set_placeholder(strings.search_placeholder, cx));
        })
        .detach();

        let detail = cx.new(|cx| TaskDetail::new(store.clone(), window, cx));
        cx.subscribe(&detail, |this, _, _: &TaskUpdated, cx| this.reload(cx)).detach();
        cx.subscribe(&detail, |this, _, OpenTask(id), cx| this.select(*id, cx)).detach();
        cx.subscribe(&detail, |_, _, StartFocus(id, title, stopwatch), cx| cx.emit(StartFocus(*id, title.clone(), *stopwatch))).detach();
        cx.subscribe(&detail, |this, _, crate::task_detail::Undoable(action, label), cx| {
            this.offer_undo(*action, label.clone(), cx);
        })
        .detach();

        let mut this = Self {
            store,
            view: View::Inbox,
            tasks: Vec::new(),
            counts: ViewCounts::default(),
            lists: Vec::new(),
            tags: Vec::new(),
            show_lists: true,
            collapsed_tasks: HashSet::new(),
            // As in TickTick, completed tasks sit collapsed at the bottom of the list.
            collapsed_sections: HashSet::from([Section::Completed]),
            input,
            list_focus: cx.focus_handle(),
            new_list,
            creating_list: false,
            rename_input,
            renaming_list: false,
            search_input,
            header_menu_open: false,
            closed_range: ClosedRange::All,
            closed_lists: None,
            closed_menu: None,
            sort_menu: None,
            show_details: false,
            kanban: false,
            timeline: false,
            timeline_from: Self::today(),
            sections: Vec::new(),
            new_section: None,
            section_menu: None,
            templates_open: false,
            add_menu_open: false,
            add_list_side: false,
            add_draft: AddDraft::default(),
            add_tags: None,
            add_tag_input,
            input_box_open: false,
            add_date_month: None,
            add_date_row: None,
            list_activity: None,
            filters: Vec::new(),
            preferences: Default::default(),
            quick_add_due: None,
            mini_expanded,
            mini_month: crate::calendar::month_start(Self::today()),
            busy_days: HashSet::new(),
            summary,
            filter_edit: None,
            undo: None,
            renaming_section: None,
            detail,
            selected: None,
            error: None,
        };
        this.reload(cx);
        #[cfg(debug_assertions)]
        this.apply_dev_state(window, cx);
        this
    }

    /// Debug builds only: puts the UI into a given state without clicks, for screenshots.
    /// `FERN_DEV_STATE="add=<quick-add text>;list=<new list>;view=<list name>;select=<title part>;
    /// popup=due|priority|tags|list;menu=header;new-list=1"`
    #[cfg(debug_assertions)]
    fn apply_dev_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(state) = std::env::var("FERN_DEV_STATE") else { return };
        for part in state.split(';') {
            match part.split_once('=') {
                Some(("add", text)) => {
                    let result = self.add_task(text);
                    self.check(result);
                    self.reload(cx);
                }
                Some(("list", name)) => self.create_list(name, cx),
                Some(("view", "trash")) => self.set_view(View::Trash, cx),
                Some(("view", "wontdo")) => self.set_view(View::WontDo, cx),
                Some(("view", "today")) => self.set_view(View::Today, cx),
                Some(("view", "summary")) => self.set_view(View::Summary, cx),
                // `type=<text>`: text in the quick-add field, as if typed (not added).
                Some(("type", text)) => {
                    self.input.update(cx, |input, cx| input.set_text(text.to_owned(), cx));
                    self.update_quick_add_hint(cx);
                }
                // `mini=month`: the mini calendar expanded to the month.
                Some(("mini", "month")) => self.mini_expanded = true,
                // `view=day+N`: the mini calendar's day N days from today.
                Some(("view", day)) if day.starts_with("day") => {
                    let offset: i64 = day.trim_start_matches("day").trim_start_matches('+').parse().unwrap_or(0);
                    self.set_view(View::Day(Self::today() + Duration::days(offset)), cx);
                }
                // `summary=<period|status|grouping|fields>` opens a Summary dropdown.
                Some(("summary", menu)) => self.summary.update(cx, |summary, cx| summary.dev_open(menu, window, cx)),
                Some(("drop", spec)) => {
                    // `drop=<title part>><today|tomorrow|inbox|completed|trash|list:Name|tag:Name>`:
                    // the same path as dropping a task on the lists column.
                    let all = self.store.tasks().unwrap_or_default();
                    if let Some((needle, target)) = spec.split_once('>')
                        && let Some(id) = all.iter().find(|t| t.title.contains(needle)).map(|t| t.id)
                    {
                        let view = match target {
                            "today" => Some(View::Today),
                            "tomorrow" => Some(View::Tomorrow),
                            "inbox" => Some(View::Inbox),
                            "completed" => Some(View::Completed),
                            "wontdo" => Some(View::WontDo),
                            "trash" => Some(View::Trash),
                            other => other
                                .strip_prefix("tag:")
                                .map(|t| View::Tag(t.to_owned()))
                                .or_else(|| {
                                    let name = other.strip_prefix("list:")?;
                                    self.lists.iter().find(|l| l.name == name).map(|l| View::List(l.id))
                                }),
                        };
                        if let Some(view) = view {
                            self.drop_on_view(view, id, cx);
                        }
                    }
                }
                Some(("move", spec)) => {
                    // `move=<dragged title part>><target title part>`: same path as a mouse drop.
                    let find = |needle: &str| self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id);
                    if let Some((dragged, target)) = spec.split_once('>')
                        && let (Some(dragged), Some(target)) = (find(dragged), find(target))
                    {
                        self.drop_task(dragged, target, cx);
                    }
                }
                Some(("span", spec)) => {
                    // `span=<title part>><days from today> [HH:MM]><days from today> [HH:MM]`.
                    let at = |part: &str| {
                        let mut words = part.split_whitespace();
                        let days: i64 = words.next()?.parse().ok()?;
                        let time = words.next().and_then(crate::due_picker::parse_time);
                        Some((Self::today() + Duration::days(days), time))
                    };
                    let parts: Vec<&str> = spec.split('>').collect();
                    if let [needle, start, end] = parts[..]
                        && let (Some((start, start_time)), Some(end)) = (at(start), at(end))
                        && let Some(id) = self.store.tasks().unwrap_or_default().iter().find(|t| t.title.contains(needle)).map(|t| t.id)
                    {
                        self.store.set_due(id, Some(start), start_time).ok();
                        self.store.set_due_end(id, Some(end)).ok();
                        self.reload(cx);
                    }
                }
                Some(("search", query)) => {
                    let query = query.to_owned();
                    self.search_input.update(cx, |input, cx| input.set_text(query, cx));
                    self.start_search(window, cx);
                }
                Some(("view", name)) => {
                    if let Some(id) = self.lists.iter().find(|l| l.name == name).map(|l| l.id) {
                        self.set_view(View::List(id), cx);
                    }
                }
                Some(("section", spec)) => {
                    // `section=<name>` adds a section to the current list; `section=<name>><task>` also files a task.
                    let (name, task) = spec.split_once('>').map_or((spec, None), |(n, t)| (n, Some(t)));
                    if let View::List(list) = self.view {
                        let existing = self.store.list_sections(list).unwrap_or_default().into_iter().find(|s| s.name == name);
                        let section = existing.or_else(|| self.store.add_list_section(list, name).ok());
                        if let (Some(section), Some(task)) = (section, task)
                            && let Some(id) = self.tasks.iter().find(|t| t.title.contains(task)).map(|t| t.id)
                        {
                            self.store.set_section(id, Some(section.id)).ok();
                        }
                        self.reload(cx);
                    }
                }
                Some(("kdrop", spec)) => {
                    // `kdrop=<task title part>><column number>`: drop a card on a kanban column.
                    let strings = i18n::t(cx);
                    let columns = task_rows::kanban_columns(self.rows_with(strings, &HashSet::new(), &HashSet::new()), strings);
                    if let Some((needle, column)) = spec.split_once('>')
                        && let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id)
                        && let Some((_, Some(target), _)) = column.parse::<usize>().ok().and_then(|c| columns.get(c))
                    {
                        let target = target.clone();
                        self.drop_on_column(id, target, cx);
                    }
                }
                Some(("section-menu", name)) => {
                    self.section_menu = self.sections.iter().find(|s| s.name == name).map(|s| s.id);
                }
                Some(("template", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        self.store.save_template(id).ok();
                    }
                }
                Some(("menu", "templates")) => self.templates_open = true,
                Some(("menu", "input-box")) => self.input_box_open = true,
                Some(("menu", "timeline")) => self.set_layout(false, true, cx),
                // `menu=add-date-repeat`: the picker with a time and a weekly repeat, Repeat open.
                Some(("menu", "add-date-repeat")) => {
                    self.add_draft.date = Some(Self::today() + chrono::Duration::days(1));
                    self.add_draft.time = chrono::NaiveTime::from_hms_opt(9, 0, 0);
                    self.add_draft.repeat = Some(fern_core::Recurrence::new(fern_core::Frequency::Weekly, 1));
                    self.add_date_month = Some(crate::calendar::month_start(Self::today()));
                    self.add_date_row = Some(2);
                }
                // `menu=add-date`: the field's date picker open, with tomorrow picked.
                Some(("menu", "add-date")) => {
                    self.add_draft.date = Some(Self::today() + chrono::Duration::days(1));
                    self.add_date_month = Some(crate::calendar::month_start(Self::today()));
                }
                Some(("menu", "detailed-input")) => {
                    self.preferences.detailed_input = true;
                    self.store.save_preferences(&self.preferences).ok();
                    self.sync_input_placeholder(cx);
                }
                Some(("menu", "add-tags")) => {
                    self.add_tags = Some(vec!["home".to_owned()]);
                }
                Some(("menu", "add")) => {
                    self.add_menu_open = true;
                    self.add_list_side = true;
                }
                Some(("menu", "activity")) => self.open_list_activity(cx),
                Some(("filter-new", mode)) => {
                    self.open_filter_edit(None, window, cx);
                    if let Some(edit) = self.filter_edit.as_mut() {
                        edit.advanced = mode == "advanced";
                    }
                }
                Some(("filter", spec)) => {
                    // `filter=<name>><tag>`: a saved filter of one tag, opened.
                    let (name, tag) = spec.split_once('>').unwrap_or((spec, ""));
                    let filter = fern_core::SmartFilter {
                        name: name.to_owned(),
                        tags: (!tag.is_empty()).then(|| vec![tag.to_owned()]).unwrap_or_default(),
                        ..fern_core::SmartFilter::new()
                    };
                    if self.store.save_filter(&filter).is_ok() {
                        self.view = View::Filter(filter.id);
                        self.reload(cx);
                    }
                }
                Some(("flag", name)) => {
                    let default = name == "show_completed";
                    self.toggle_view_flag(name, default, cx);
                }
                Some(("pin", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        let result = self.store.set_pinned(id, true);
                        self.check(result);
                        self.reload(cx);
                    }
                }
                // `estimate=<title part>>p3|m90`: an Estimation, as Start Focus → Estimation sets it.
                Some(("estimate", spec)) => {
                    let all = self.store.tasks().unwrap_or_default();
                    if let Some((needle, value)) = spec.split_once('>')
                        && let Some(task) = all.iter().find(|t| t.title.contains(needle))
                    {
                        let estimate = fern_core::Estimate::from_storage(&value.replacen(['p', 'm'], &format!("{}:", &value[..1]), 1));
                        self.store.set_estimate(task.id, estimate).ok();
                        self.reload(cx);
                    }
                }
                Some(("select", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        self.select(id, cx);
                    }
                }
                Some(("popup", popup)) => {
                    let popup = popup.to_owned();
                    self.detail.update(cx, |detail, cx| detail.dev_open_popup(&popup, cx));
                }
                Some(("menu", "header")) => self.header_menu_open = true,
                Some(("menu", "sort")) => self.sort_menu = Some(Some(SortMenu::Sort)),
                Some(("menu", "group")) => self.sort_menu = Some(Some(SortMenu::Group)),
                Some(("group", key)) => self.set_sort(None, Some(GroupBy::from_key(key)), cx),
                Some(("sort", key)) => self.set_sort(Some(SortBy::from_key(key)), None, cx),
                Some(("menu", "dates")) => self.closed_menu = Some(ClosedMenu::Dates),
                Some(("menu", "other-date")) => {
                    self.open_other_date(cx);
                    self.update_other_date(cx, |start, _, picking, _| {
                        *start = Some(Self::today() - Duration::days(7));
                        *picking = Some(RangeEnd::End);
                    });
                }
                Some(("menu", "lists")) => self.closed_menu = Some(ClosedMenu::Lists(self.closed_lists.clone())),
                Some(("range", "this-week")) => self.closed_range = ClosedRange::ThisWeek,
                Some(("complete", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        self.toggle(id, true, cx);
                    }
                }
                Some(("delete", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        self.delete(id, cx);
                    }
                }
                Some(("rename", _)) => self.start_rename(window, cx),
                Some(("new-list", _)) => self.start_new_list(window, cx),
                _ => {}
            }
        }
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    /// The mini calendar's tab and arrows; whether it is expanded is remembered.
    fn mini_action(&mut self, action: lists_panel::MiniAction, cx: &mut Context<Self>) {
        match action {
            lists_panel::MiniAction::Toggle => {
                self.mini_expanded = !self.mini_expanded;
                let value = if self.mini_expanded { "1" } else { "0" };
                let result = self.store.set_setting("mini_expanded", value);
                self.check(result);
            }
            lists_panel::MiniAction::Shift(delta) => self.mini_month = crate::calendar::add_months(self.mini_month, delta),
            lists_panel::MiniAction::ThisMonth => self.mini_month = crate::calendar::month_start(Self::today()),
        }
        cx.notify();
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.preferences = self.store.preferences().unwrap_or_default();
        self.sync_input_placeholder(cx);
        self.busy_days = self
            .store
            .tasks()
            .unwrap_or_default()
            .iter()
            .filter(|t| !t.is_closed() && !t.is_deleted())
            .filter_map(|t| t.due_date)
            .collect();
        self.show_details = self.view_flag("show_details", false);
        self.kanban = self.view_flag("kanban", false);
        self.timeline = self.view_flag("timeline", false);
        let today = Self::today();
        self.filters = self
            .store
            .filters()
            .unwrap_or_default()
            .into_iter()
            .map(|f| {
                let count = self.store.tasks_in(&View::Filter(f.id), today).map_or(0, |t| t.len());
                (f, count)
            })
            .collect();
        self.sections = match self.view {
            View::List(list) => self.store.list_sections(list).unwrap_or_default(),
            _ => Vec::new(),
        };
        let today = Self::today();
        if let Some(lists) = self.check(self.store.lists()) {
            self.lists = lists;
        }
        // The current list may have been deleted.
        if let View::List(id) = self.view
            && !self.lists.iter().any(|l| l.id == id)
        {
            self.view = View::Inbox;
        }
        if let Some(tasks) = self.check(self.store.tasks_in(&self.view, today)) {
            self.tasks = tasks;
        }
        if let Some(counts) = self.check(self.store.counts(today)) {
            self.counts = counts;
        }
        if let Some(tags) = self.check(self.store.tags()) {
            self.tags = tags;
        }
        // The details panel keeps showing a task as long as it exists, even when it has left
        // the current view (for example, after being completed in "Today").
        let selected = self.selected.and_then(|id| self.store.task(id).ok().flatten());
        self.selected = selected.as_ref().map(|t| t.id);
        let lists = self.lists.clone();
        self.detail.update(cx, |detail, cx| {
            detail.set_lists(lists);
            detail.refresh(selected, cx);
        });
        cx.notify();
    }

    /// Creates a task from the quick-add line: due date, time, tags, priority and list come from
    /// the text ("tomorrow 6pm #home !1 ^work"); anything missing comes from the current view —
    /// a due date in Today/Tomorrow, the tag in a tag view, the list in a list view.
    /// A task from a template, placed like a typed one: in the current list, tag or day.
    fn add_from_template(&mut self, template: Id, cx: &mut Context<Self>) {
        self.templates_open = false;
        let today = Self::today();
        let result = (|| -> anyhow::Result<Task> {
            let task = self.store.add_from_template(template)?;
            match &self.view {
                View::Today | View::Next7Days => self.store.set_due(task.id, Some(today), None)?,
                View::Tomorrow => self.store.set_due(task.id, Some(today + Duration::days(1)), None)?,
                View::List(list) => self.store.set_list(task.id, Some(*list))?,
                View::Tag(tag) if !task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) => {
                    let mut tags = task.tags.clone();
                    tags.push(tag.clone());
                    self.store.set_tags(task.id, &tags)?;
                }
                _ => {}
            }
            Ok(task)
        })();
        if let Some(task) = self.check(result) {
            self.reload(cx);
            self.select(task.id, cx);
        }
    }

    fn render_templates(&self, strings: &Strings, cx: &mut Context<Self>) -> AnyElement {
        let templates = self.store.templates().unwrap_or_default();
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(280.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.templates_open = false;
                cx.notify();
            }))
            .child(div().px_2().py_1().child(Label::new(strings.templates).weight(FontWeight::SEMIBOLD)))
            .when(templates.is_empty(), |el| {
                el.child(div().px_2().py_2().child(Label::new(strings.no_templates).size(LabelSize::Small).color(Color::Muted)))
            })
            .children(templates.into_iter().map(|template| {
                let id = template.id;
                let group = SharedString::from(format!("template-{id}"));
                let tags = (!template.tags.is_empty())
                    .then(|| template.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "));
                let checklist = template.checklist.len();
                h_flex()
                    .id(group.clone())
                    .group(group.clone())
                    .h(px(34.))
                    .px_2()
                    .gap_2()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(Icon::new(IconName::LayoutTemplate).size(IconSize::Small).color(Color::Muted))
                    .child(div().flex_1().min_w_0().child(Label::new(template.title.clone()).truncate()))
                    .children(tags.map(|t| Label::new(t).size(LabelSize::XSmall).color(Color::Muted)))
                    .when(checklist > 0, |el| {
                        el.child(Icon::new(IconName::ListChecks).size(IconSize::XSmall).color(Color::Muted))
                            .child(Label::new(checklist.to_string()).size(LabelSize::XSmall).color(Color::Muted))
                    })
                    .child(
                        div().opacity(0.).group_hover(group, |s| s.opacity(1.)).child(
                            IconButton::new(SharedString::from(format!("delete-template-{id}")), IconName::X)
                                .size(px(20.))
                                .icon_size(IconSize::XSmall)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    let result = this.store.delete_template(id);
                                    this.check(result);
                                    cx.notify();
                                })),
                        ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.add_from_template(id, cx)))
            }))
            .into_any_element()
    }

    fn add_task(&self, input: &str) -> anyhow::Result<Task> {
        let today = Self::today();
        let list_names: Vec<String> = self.lists.iter().map(|l| l.name.clone()).collect();
        let parsed = fern_core::parse_quick_add_with(input, today, &list_names, self.preferences.recognition);
        let title = if parsed.title.is_empty() { input.trim() } else { parsed.title.as_str() };
        let task = self.store.add_task_at(title, !self.preferences.task_defaults.add_to_bottom)?;

        let view_due = match &self.view {
            View::Today | View::Next7Days => Some(today),
            View::Tomorrow => Some(today + Duration::days(1)),
            View::Day(day) => Some(*day),
            _ => None,
        };
        // Settings → More → Task Default fills in what neither the text nor the view gives.
        let defaults = &self.preferences.task_defaults;
        // The date picker's time and reminders go with its date, not with a typed one.
        let from_picker = parsed.due_date.is_none() && self.add_draft.date.is_some();
        let due_time = if from_picker { self.add_draft.time } else { parsed.due_time };
        if let Some(date) = parsed.due_date.or(self.add_draft.date).or(view_due).or(defaults.date.map(|d| d.date(today))) {
            self.store.set_due(task.id, Some(date), due_time)?;
            match self.add_draft.reminders.as_ref().filter(|_| from_picker) {
                Some(reminders) => self.store.set_reminders(task.id, reminders)?,
                None => {
                    let reminder = if due_time.is_some() { defaults.timed_reminder } else { defaults.all_day_reminder };
                    if let Some(offset) = reminder {
                        self.store.set_reminders(task.id, &[offset])?;
                    }
                }
            }
        }

        let mut tags = parsed.tags;
        if let Some(tag) = &defaults.tag
            && !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase())
        {
            tags.push(tag.clone());
        }
        if let View::Tag(tag) = &self.view
            && !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase())
        {
            tags.push(tag.clone());
        }
        for tag in &self.add_draft.tags {
            if !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
                tags.push(tag.clone());
            }
        }
        if !tags.is_empty() {
            self.store.set_tags(task.id, &tags)?;
        }
        if let Some(repeat) = parsed.repeat.or_else(|| self.add_draft.repeat.clone().filter(|_| self.add_draft.date.is_some())) {
            self.store.set_repeat(task.id, Some(repeat), today)?;
        }
        let draft_priority = self.add_draft.priority.filter(|p| *p != fern_core::Priority::None);
        if let Some(priority) = parsed
            .priority
            .or(draft_priority)
            .or((defaults.priority != fern_core::Priority::None).then_some(defaults.priority))
        {
            self.store.set_priority(task.id, priority)?;
        }

        let parsed_list =
            parsed.list.and_then(|name| self.lists.iter().find(|l| l.name == name)).map(|l| l.id);
        let view_list = match self.view {
            View::List(id) => Some(id),
            _ => None,
        };
        // Outside the Inbox and lists, new tasks go to the default list from the Settings.
        let default_list = match self.view {
            View::Inbox | View::List(_) => None,
            _ => self.store.preferences().ok().and_then(|p| p.default_list),
        };
        // The "▾" menu's list goes before the view's and the default one.
        let draft_list = self.add_draft.list;
        if let Some(Some(list)) = parsed_list.map(Some).or(draft_list).or(view_list.map(Some)).or(default_list.map(Some)) {
            self.store.set_list(task.id, Some(list))?;
        }
        if self.add_draft.note {
            self.store.set_kind(task.id, fern_core::Kind::Note)?;
        }
        if !self.add_draft.files.is_empty() {
            let dir = crate::attachments_dir()?;
            for file in &self.add_draft.files {
                self.store.add_attachment(task.id, file, &dir)?;
            }
        }
        Ok(task)
    }

    /// Highlights what quick add recognizes in the field and shows the recognized date.
    fn update_quick_add_hint(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text().to_owned();
        let list_names: Vec<String> = self.lists.iter().map(|l| l.name.clone()).collect();
        let parsed = fern_core::parse_quick_add_with(&text, Self::today(), &list_names, self.preferences.recognition);
        let spans = parsed.spans.clone();
        self.input.update(cx, |input, cx| input.set_highlights(spans, cx));
        self.quick_add_due = parsed.due_date.map(|date| (date, parsed.due_time));
        cx.notify();
    }

    /// The add field's placeholder: the Detailed box asks "What would you like to do?".
    fn sync_input_placeholder(&mut self, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let placeholder = if self.preferences.detailed_input { strings.input_box_placeholder } else { strings.add_task };
        self.input.update(cx, |input, cx| input.set_placeholder(placeholder, cx));
    }

    /// Adds the typed task, as Enter in the add field or its "Add" button does.
    fn submit_new_task(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if text.trim().is_empty() {
            return;
        }
        let result = self.add_task(text);
        if self.check(result).is_some() {
            self.input.update(cx, |input, cx| input.clear(window, cx));
            self.quick_add_due = None;
            self.add_draft = AddDraft::default();
            self.reload(cx);
        }
    }

    /// The popovers the add field's buttons open: the "▾" menu, its Tags and Input Box Setting,
    /// and the templates.
    fn add_field_popovers(&self, el: Div, strings: &'static Strings, cx: &mut Context<Self>) -> Div {
        el.when(self.add_menu_open, |el| {
            el.child(PopoverAttachment::new(Placement::BelowRight, self.render_add_menu(strings, cx)))
        })
        .when_some(self.add_tags.clone(), |el, picked| {
            el.child(PopoverAttachment::new(Placement::BelowRight, self.render_add_tags(&picked, strings, cx)))
        })
        .when(self.input_box_open, |el| {
            el.child(PopoverAttachment::new(Placement::BelowRight, self.render_input_box_setting(strings, cx)))
        })
        .when(self.templates_open, |el| {
            el.child(PopoverAttachment::new(Placement::BelowRight, self.render_templates(strings, cx)))
        })
    }

    /// The date the field shows: the typed one, or the one picked with its calendar button.
    fn shown_add_due(&self) -> Option<(NaiveDate, Option<chrono::NaiveTime>)> {
        self.quick_add_due.or(self.add_draft.date.map(|date| (date, self.add_draft.time)))
    }

    /// The field's calendar button, as in the original: it opens a date picker for the next task,
    /// placed under the button.
    fn add_date_button(&self, placement: Placement, strings: &'static Strings, cx: &mut Context<Self>) -> Div {
        let picked = self.add_draft.date.is_some();
        let button = IconButton::new("add-date", IconName::CalendarDays)
            .size(px(24.))
            .icon_size(IconSize::Small)
            .icon_color(if picked { Color::Accent } else { Color::Muted })
            .on_click(cx.listener(|this, _, _, cx| {
                this.add_date_month = match this.add_date_month {
                    Some(_) => None,
                    None => Some(crate::calendar::month_start(this.add_draft.date.unwrap_or_else(Self::today))),
                };
                cx.notify();
            }));
        div().relative().child(button).when_some(self.add_date_month, |el, month| {
            el.child(PopoverAttachment::new(placement, self.render_add_date(month, strings, cx)))
        })
    }

    /// The date picker's Time, Reminder and Repeat rows, as in the task's date popover: a click
    /// opens the row's choices under it.
    fn render_add_date_rows(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let draft = &self.add_draft;
        let date = draft.date.unwrap_or_else(Self::today);
        let has_time = draft.time.is_some();
        let open = self.add_date_row;
        let row = |i: u8, icon: IconName, label: String, set: bool| {
            h_flex()
                .id(SharedString::from(format!("add-date-row-{i}")))
                .h(px(30.))
                .px_1()
                .gap_2()
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(Icon::new(icon).size(IconSize::Small).color(if set { Color::Accent } else { Color::Muted }))
                .child(div().flex_1().min_w_0().child(Label::new(label).size(LabelSize::Small).color(if set { Color::Accent } else { Color::Default }).truncate()))
                .child(Icon::new(if open == Some(i) { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::XSmall).color(Color::Muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.add_date_row = if this.add_date_row == Some(i) { None } else { Some(i) };
                    cx.notify();
                }))
        };
        let choice = |id: SharedString, label: String, selected: bool| {
            h_flex()
                .id(id)
                .h(px(26.))
                .px_2()
                .gap_2()
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(div().flex_1().child(Label::new(label).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default })))
                .when(selected, |el| el.child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
        };
        let time_label = draft.time.map_or(strings.time.to_owned(), crate::calendar::format_time);
        let reminder_label = draft
            .reminders
            .as_ref()
            .and_then(|r| crate::due_picker::reminder_label(r, has_time, strings))
            .unwrap_or_else(|| strings.reminder.to_owned());
        let repeat_label =
            draft.repeat.as_ref().map_or(strings.repeat.to_owned(), |r| crate::due_picker::repeat_label(r, strings));
        let mut rows = v_flex().gap_0p5();
        rows = rows.child(row(0, IconName::Clock, time_label, has_time));
        if open == Some(0) {
            let times = std::iter::once(None).chain(crate::due_picker::time_options().into_iter().map(Some));
            rows = rows.child(v_flex().id("add-date-times").max_h(px(150.)).overflow_y_scroll().children(times.map(|time| {
                let label = time.map_or(strings.no_time.to_owned(), crate::calendar::format_time);
                choice(SharedString::from(format!("add-time-{label}")), label, draft.time == time).on_click(cx.listener(move |this, _, _, cx| {
                    this.add_draft.time = time;
                    this.add_draft.date.get_or_insert_with(Self::today);
                    this.add_draft.reminders = None;
                    this.add_date_row = None;
                    cx.notify();
                }))
            })));
        }
        rows = rows.child(row(1, IconName::AlarmClock, reminder_label, draft.reminders.as_ref().is_some_and(|r| !r.is_empty())));
        if open == Some(1) {
            let chosen = draft.reminders.clone().unwrap_or_default();
            rows = rows.child(v_flex().children(crate::due_picker::reminder_options(has_time, strings).into_iter().map(|(offset, label, hint)| {
                let selected = chosen.contains(&offset);
                let label = if hint.is_empty() { label.to_owned() } else { format!("{label} ({hint})") };
                choice(SharedString::from(format!("add-reminder-{offset}")), label, selected).on_click(cx.listener(move |this, _, _, cx| {
                    let reminders = this.add_draft.reminders.get_or_insert_with(Vec::new);
                    if let Some(i) = reminders.iter().position(|r| *r == offset) {
                        reminders.remove(i);
                    } else {
                        reminders.push(offset);
                    }
                    this.add_draft.date.get_or_insert_with(Self::today);
                    cx.notify();
                }))
            })));
        }
        rows = rows.child(row(2, IconName::Repeat, repeat_label, draft.repeat.is_some()));
        if open == Some(2) {
            let options = crate::due_picker::repeat_options(date, strings, i18n::locale(cx));
            rows = rows.child(
                v_flex()
                    .child(choice("add-repeat-none".into(), strings.none_option.to_owned(), draft.repeat.is_none()).on_click(cx.listener(|this, _, _, cx| {
                        this.add_draft.repeat = None;
                        this.add_date_row = None;
                        cx.notify();
                    })))
                    .children(options.into_iter().enumerate().map(|(i, (rule, label, hint))| {
                        let selected = draft.repeat.as_ref() == Some(&rule);
                        let label = if hint.is_empty() { label } else { format!("{label} ({hint})") };
                        choice(SharedString::from(format!("add-repeat-{i}")), label, selected).on_click(cx.listener(move |this, _, _, cx| {
                            this.add_draft.repeat = Some(rule.clone());
                            this.add_draft.date.get_or_insert_with(Self::today);
                            this.add_date_row = None;
                            cx.notify();
                        }))
                    })),
            );
        }
        rows.into_any_element()
    }

    /// The add field's date picker: the original's quick picks (today, tomorrow, next week, next
    /// month) and the month; a pick applies to the next task added. Clear drops it.
    fn render_add_date(&self, month: NaiveDate, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let today = Self::today();
        let picked = self.add_draft.date;
        // A pick stays in the picker, as in the original; OK closes it and Clear drops it all.
        let pick = |date: Option<NaiveDate>| {
            move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.add_draft.date = date;
                if date.is_none() {
                    this.add_draft.time = None;
                    this.add_draft.reminders = None;
                    this.add_draft.repeat = None;
                    this.add_date_month = None;
                }
                cx.notify();
            }
        };
        let [d_today, d_tomorrow, d_week, d_month] = crate::due_picker::quick_dates(today);
        let quick = |id: &'static str, icon: IconName, tip: &'static str, date: NaiveDate| {
            div()
                .id(id)
                .flex_1()
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .tooltip(Tooltip::text(tip))
                .child(Icon::new(icon).size(IconSize::Large))
                .on_click(cx.listener(pick(Some(date))))
        };
        let shift = |delta: i32| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let Some(month) = this.add_date_month.as_mut() {
                    *month = crate::calendar::add_months(*month, delta);
                }
                cx.notify();
            })
        };
        let days = crate::calendar::month_grid(month);
        let weeks: Vec<_> = days
            .chunks(7)
            .map(|week| {
                h_flex().children(week.iter().map(|&date| {
                    let selected = picked == Some(date);
                    let in_month = date.month() == month.month();
                    div().flex_1().flex().justify_center().child(
                        div()
                            .id(SharedString::from(format!("add-day-{date}")))
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .when(selected, |el| el.bg(theme.colors().accent))
                            .when(!selected && date == today, |el| el.bg(theme.colors().accent.opacity(0.18)))
                            .when(!selected, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                            .child(Label::new(date.day().to_string()).size(LabelSize::Small).color(
                                if selected || in_month { Color::Default } else { Color::Muted },
                            ))
                            .on_click(cx.listener(pick(Some(date)))),
                    )
                }))
            })
            .collect();
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
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };
        popover(cx)
            .w(px(260.))
            .p_2()
            .gap_1()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.add_date_month = None;
                cx.notify();
            }))
            .child(
                h_flex()
                    .child(quick("add-date-today", IconName::Sun, strings.today, d_today))
                    .child(quick("add-date-tomorrow", IconName::Sunrise, strings.tomorrow, d_tomorrow))
                    .child(quick("add-date-week", IconName::CalendarPlus, strings.next_week, d_week))
                    .child(quick("add-date-month", IconName::Moon, strings.next_month, d_month)),
            )
            .child(
                h_flex()
                    .px_1()
                    .child(div().flex_1().child(Label::new(crate::calendar::month_title(month, strings)).size(LabelSize::Small)))
                    .child(IconButton::new("add-date-prev", IconName::ChevronLeft).size(px(22.)).icon_size(IconSize::XSmall).on_click(shift(-1)))
                    .child(IconButton::new("add-date-next", IconName::ChevronRight).size(px(22.)).icon_size(IconSize::XSmall).on_click(shift(1))),
            )
            .child(h_flex().children(crate::calendar::weekday_labels(&strings.weekdays_narrow).map(|day| {
                div().flex_1().flex().justify_center().child(Label::new(day).size(LabelSize::XSmall).color(Color::Muted))
            })))
            .children(weeks)
            .child(div().py_1().child(Divider::horizontal()))
            .child(self.render_add_date_rows(strings, cx))
            .child(
                h_flex()
                    .pt_1()
                    .gap_2()
                    .child(button("add-date-clear", strings.clear, false).on_click(cx.listener(pick(None))))
                    .child(button("add-date-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| {
                        this.add_date_month = None;
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }

    /// The original's "Input Box Setting": a preview of the Simple and the Detailed add field;
    /// a click picks one.
    fn render_input_box_setting(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let detailed = self.preferences.detailed_input;
        let line = |w: f32| div().w(px(w)).h(px(6.)).rounded(px(3.)).bg(theme.colors().text_muted.opacity(0.4));
        let simple_preview = h_flex()
            .h(px(28.))
            .px_2()
            .gap_1()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.colors().accent)
            .child(Icon::new(IconName::Plus).size(IconSize::XSmall).color(Color::Muted))
            .child(line(70.))
            .child(div().flex_1())
            .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted));
        let detailed_preview = v_flex()
            .p_1p5()
            .gap_1p5()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.colors().accent)
            .child(line(90.))
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(IconName::Flag).size(IconSize::XSmall).color(Color::Muted))
                    .child(Icon::new(IconName::Inbox).size(IconSize::XSmall).color(Color::Muted))
                    .child(Icon::new(IconName::Ellipsis).size(IconSize::XSmall).color(Color::Muted))
                    .child(div().flex_1())
                    .child(div().w(px(22.)).h(px(12.)).rounded(px(3.)).bg(theme.colors().accent)),
            );
        let option = |id: &'static str, preview: AnyElement, label: &'static str, value: bool| {
            let selected = detailed == value;
            v_flex()
                .id(id)
                .flex_1()
                .gap_2()
                .items_center()
                .cursor_pointer()
                .child(
                    div()
                        .w_full()
                        .h(px(76.))
                        .px_2()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .rounded(px(6.))
                        .bg(theme.colors().background)
                        .child(preview),
                )
                .child(
                    h_flex()
                        .gap_1p5()
                        .child(
                            div()
                                .size(px(14.))
                                .rounded_full()
                                .border_1()
                                .border_color(if selected { theme.colors().accent } else { theme.colors().text_muted })
                                .when(selected, |el| el.bg(theme.colors().accent)),
                        )
                        .child(Label::new(label).size(LabelSize::Small)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.preferences.detailed_input = value;
                    this.sync_input_placeholder(cx);
                    if let Err(err) = this.store.save_preferences(&this.preferences) {
                        eprintln!("failed to save the input box setting: {err:#}");
                    }
                    cx.notify();
                }))
        };
        popover(cx)
            .w(px(380.))
            .p_3()
            .gap_3()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.input_box_open = false;
                cx.notify();
            }))
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.input_box_setting).weight(FontWeight::SEMIBOLD)))
                    .child(IconButton::new("input-box-close", IconName::X).size(px(22.)).icon_size(IconSize::Small).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.input_box_open = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(option("input-box-simple", simple_preview.into_any_element(), strings.input_box_simple, false))
                    .child(option("input-box-detailed", detailed_preview.into_any_element(), strings.input_box_detailed, true)),
            )
            .into_any_element()
    }

    /// The original's "Detailed" add field: the text on top, a toolbar under it — priority, list
    /// and "…" open the options menu — and an "Add" button.
    fn render_detailed_add(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let input_focus = self.input.read(cx).focus_handle(cx);
        let date_button = self.add_date_button(Placement::BelowLeft, strings, cx);
        let open_menu = |side: bool| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.add_menu_open = !this.add_menu_open;
                this.add_list_side = side;
                cx.notify();
            })
        };
        let priority_color = self
            .add_draft
            .priority
            .and_then(|p| task_rows::priority_color(p, &theme))
            .map_or(Color::Muted, Color::Custom);
        v_flex()
            .mx_2()
            .mb_2()
            .flex_none()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().border)
            .bg(theme.colors().surface_background)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| window.focus(&input_focus, cx))
            .child(h_flex().gap_2().child(div().flex_1().min_w_0().child(self.input.clone())).children(
                self.shown_add_due().map(|(date, time)| {
                    let (label, _) = task_rows::due_label(date, time, Self::today(), strings);
                    Label::new(label).size(LabelSize::Small).color(Color::Accent)
                }),
            ))
            .child(
                h_flex()
                    .gap_1()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(date_button)
                    .child(IconButton::new("add-priority", IconName::Flag).size(px(24.)).icon_size(IconSize::Small).icon_color(priority_color).on_click(open_menu(false)))
                    .child(IconButton::new("add-list", IconName::Inbox).size(px(24.)).icon_size(IconSize::Small).on_click(open_menu(true)))
                    .child(self.add_field_popovers(
                        div().relative().child(IconButton::new("add-more", IconName::Ellipsis).size(px(24.)).icon_size(IconSize::Small).on_click(open_menu(false))),
                        strings,
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("add-submit")
                            .h(px(24.))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .bg(theme.colors().accent)
                            .child(Label::new(strings.add).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, window, cx| {
                                let text = this.input.read(cx).text().to_owned();
                                this.submit_new_task(&text, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The add field's "▾" menu, as in the original.
    fn render_add_menu(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let draft = self.add_draft.clone();
        let flags = [Priority::High, Priority::Medium, Priority::Low, Priority::None].map(|priority| {
            let selected = draft.priority == Some(priority);
            let color = task_rows::priority_color(priority, &theme).unwrap_or(theme.colors().text_muted);
            div()
                .id(SharedString::from(format!("add-priority-{}", priority.to_i64())))
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.colors().element_selected))
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(Icon::new(IconName::Flag).color(Color::Custom(color)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.add_draft.priority = if selected { None } else { Some(priority) };
                    cx.notify();
                }))
        });
        let list_name = match draft.list {
            Some(Some(id)) => self.lists.iter().find(|l| l.id == id).map_or(strings.inbox.to_owned(), |l| l.name.clone()),
            _ => strings.inbox.to_owned(),
        };
        let list_items = std::iter::once((None, strings.inbox.to_owned()))
            .chain(self.lists.iter().map(|l| (Some(l.id), l.name.clone())))
            .map(|(id, name)| {
                let selected = draft.list.unwrap_or(None) == id && (draft.list.is_some() || id.is_none());
                ListItem::new(SharedString::from(format!("add-list-{}", id.map_or("inbox".to_owned(), |i| i.to_string()))))
                    .child(Label::new(name).color(if selected { Color::Accent } else { Color::Default }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.add_draft.list = Some(id);
                        this.add_menu_open = false;
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();
        popover(cx)
            .w(px(240.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.add_menu_open = false;
                cx.notify();
            }))
            .child(div().px_2().pt_1().child(Label::new(strings.filter_priority).size(LabelSize::XSmall).color(Color::Muted)))
            .child(h_flex().px_1().py_1().justify_between().children(flags))
            .child(div().py_1().child(Divider::horizontal()))
            .child(
                SubmenuItem::new("add-list", list_name)
                    .icon(IconName::Inbox)
                    .open(self.add_list_side)
                    .placement(Placement::LeftSide)
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered && !this.add_list_side {
                            this.add_list_side = true;
                            cx.notify();
                        }
                    }))
                    .items(list_items),
            )
            .child(
                ListItem::new("add-tags")
                    .start_slot(Icon::new(IconName::Tag).color(Color::Muted))
                    .child(Label::new(strings.tags))
                    .when(!draft.tags.is_empty(), |item| {
                        item.end_slot(Label::new(draft.tags.len().to_string()).size(LabelSize::XSmall).color(Color::Accent))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_menu_open = false;
                        this.add_tags = Some(this.add_draft.tags.clone());
                        this.add_tag_input.update(cx, |input, cx| input.set_text("", cx));
                        let focus = this.add_tag_input.read(cx).focus_handle(cx);
                        window.focus(&focus, cx);
                        cx.notify();
                    })),
            )
            .child(
                ListItem::new("add-attachment")
                    .start_slot(Icon::new(IconName::Paperclip).color(Color::Muted))
                    .child(Label::new(strings.attachment))
                    .when(!draft.files.is_empty(), |item| {
                        item.end_slot(Label::new(draft.files.len().to_string()).size(LabelSize::XSmall).color(Color::Accent))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_menu_open = false;
                        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
                            files: true,
                            directories: false,
                            multiple: true,
                            prompt: None,
                        });
                        cx.spawn(async move |this, cx| {
                            let Some(paths) = chosen.await.ok().and_then(Result::ok).flatten() else { return };
                            this.update(cx, |this, cx| {
                                this.add_draft.files.extend(paths);
                                cx.notify();
                            })
                            .ok();
                        })
                        .detach();
                        cx.notify();
                    })),
            )
            .child(
                ListItem::new("add-from-template")
                    .start_slot(Icon::new(IconName::LayoutTemplate).color(Color::Muted))
                    .child(Label::new(strings.add_from_template))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_menu_open = false;
                        this.templates_open = true;
                        cx.notify();
                    })),
            )
            .child(
                ListItem::new("add-as-note")
                    .start_slot(Icon::new(IconName::FileText).color(Color::Muted))
                    .child(Label::new(if draft.note { strings.convert_to_task } else { strings.convert_to_note }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_draft.note = !this.add_draft.note;
                        this.add_menu_open = false;
                        cx.notify();
                    })),
            )
            .child(div().py_1().child(Divider::horizontal()))
            .child(
                ListItem::new("add-input-box")
                    .start_slot(Icon::new(IconName::Settings).color(Color::Muted))
                    .child(Label::new(strings.input_box_setting))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_menu_open = false;
                        this.input_box_open = true;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// The "▾" menu's Tags, as in the original: a search field over the tags, ticked by a click
    /// (Enter ticks a typed one, new tags too), then Cancel or OK; they go on the next task added.
    fn render_add_tags(&self, picked: &[String], strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let query = self.add_tag_input.read(cx).text().trim().to_lowercase();
        let mut names: Vec<String> = self.tags.iter().map(|t| t.name.clone()).collect();
        for tag in picked {
            if !names.iter().any(|n| n.to_lowercase() == tag.to_lowercase()) {
                names.push(tag.clone());
            }
        }
        let rows = names.into_iter().filter(|name| name.to_lowercase().contains(&query)).map(|name| {
            let ticked = picked.iter().any(|t| t.to_lowercase() == name.to_lowercase());
            ListItem::new(SharedString::from(format!("add-tag-{name}")))
                .start_slot(Icon::new(IconName::Tag).color(Color::Muted))
                .child(Label::new(name.clone()))
                .when(ticked, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(picked) = this.add_tags.as_mut() {
                        if let Some(i) = picked.iter().position(|t| t.to_lowercase() == name.to_lowercase()) {
                            picked.remove(i);
                        } else {
                            picked.push(name.clone());
                        }
                    }
                    cx.notify();
                }))
        });
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
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };
        popover(cx)
            .w(px(240.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.add_tags = None;
                cx.notify();
            }))
            .child(
                h_flex()
                    .px_2()
                    .pb_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(Icon::new(IconName::Search).size(IconSize::Small).color(Color::Muted))
                    .child(div().flex_1().child(self.add_tag_input.clone())),
            )
            .child(v_flex().id("add-tag-list").max_h(px(240.)).overflow_y_scroll().py_1().children(rows))
            .child(
                h_flex()
                    .p_2()
                    .gap_2()
                    .child(button("add-tags-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| {
                        this.add_tags = None;
                        cx.notify();
                    })))
                    .child(button("add-tags-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(picked) = this.add_tags.take() {
                            this.add_draft.tags = picked;
                        }
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }

    /// Focuses the quick-add field.
    pub fn focus_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_read_only_view() {
            self.set_view(View::Inbox, cx);
        }
        window.focus(&self.input.focus_handle(cx), cx);
    }

    /// Task ids in the order they are shown, for keyboard navigation.
    fn visible_task_ids(&self, strings: &Strings) -> Vec<Id> {
        self.rows(strings)
            .into_iter()
            .filter_map(|row| match row {
                Row::Task { task, .. } => Some(task.id),
                Row::Section { .. } | Row::Day { .. } => None,
            })
            .collect()
    }

    /// The rows of the current view: closed tasks by day, the trash flat, everything else grouped.
    fn rows(&self, strings: &Strings) -> Vec<Row> {
        self.rows_with(strings, &self.collapsed_tasks, &self.collapsed_sections)
    }

    /// Rows with the given collapsed tasks and groups; the kanban board shows everything.
    fn rows_with(&self, strings: &Strings, collapsed_tasks: &HashSet<Id>, collapsed_sections: &HashSet<Section>) -> Vec<Row> {
        let closed = |closed_at: fn(&Task) -> Option<i64>| {
            let today = Self::today();
            let tasks: Vec<Task> = self
                .tasks
                .iter()
                .filter(|t| {
                    let day = closed_at(t).and_then(task_rows::local_day).unwrap_or(today);
                    self.closed_range.contains(day, today)
                        && self.closed_lists.as_ref().is_none_or(|lists| lists.contains(&t.list_id))
                })
                .cloned()
                .collect();
            task_rows::rows_by_day(&tasks, closed_at, today, strings)
        };
        match self.view {
            View::Completed => closed(|t| t.completed_at),
            View::WontDo => closed(|t| t.wont_do_at),
            View::Trash | View::Search(_) => task_rows::build_rows(
                &self.tasks,
                collapsed_tasks,
                collapsed_sections,
                self.view != View::Trash,
            ),
            _ => {
                let (sort, group) = self.sort_settings();
                let mut tasks = self.tasks.clone();
                if !self.view_flag("show_completed", true) {
                    tasks.retain(|t| !t.is_completed());
                }
                task_rows::sort_tasks(&mut tasks, sort);
                task_rows::pinned_first(&mut tasks);
                match group {
                    GroupBy::Custom if !self.sections.is_empty() => {
                        task_rows::section_rows(&tasks, &self.sections, collapsed_tasks, strings)
                    }
                    GroupBy::Custom => {
                        task_rows::build_rows(&tasks, collapsed_tasks, collapsed_sections, true)
                    }
                    group => task_rows::grouped_rows(&tasks, group, Self::today(), self.preferences.task_defaults.overdue_last, strings),
                }
            }
        }
    }

    /// The settings key of the current view's sort and grouping: each list keeps its own.
    fn view_key(&self) -> String {
        match &self.view {
            View::List(id) => format!("list:{id}"),
            View::Tag(tag) => format!("tag:{tag}"),
            other => format!("{other:?}").to_lowercase(),
        }
    }

    /// A kanban card: the task with its tags and date, and its subtasks under it.
    fn render_card(
        &self,
        task: &Task,
        subtasks: &[Task],
        tag_colors: &HashMap<String, Hsla>,
        today: NaiveDate,
        strings: &Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.selected == Some(task.id);
        let line = |task: &Task, id: SharedString, cx: &mut Context<Self>| {
            let task_id = task.id;
            let done = task.is_completed();
            h_flex()
                .items_start()
                .gap_2()
                .child(
                    Checkbox::new(id)
                        .toggle_state(done)
                        .tint(task_rows::priority_color(task.priority, &theme))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle(task_id, !done, cx);
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .text_color(if done { theme.colors().text_muted } else { theme.colors().text })
                        .child(if task.title.is_empty() { strings.untitled.to_owned() } else { task.title.clone() }),
                )
        };
        let id = task.id;
        let due = task.due_date.map(|date| task_rows::due_label(date, task.due_time, today, strings));
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        v_flex()
            .id(ElementId::Uuid(id))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .p_2()
            .gap_1p5()
            .rounded(px(8.))
            .border_1()
            .border_color(if selected { theme.colors().accent } else { theme.colors().border_variant })
            .bg(theme.colors().surface_background)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .child(line(task, SharedString::from(format!("card-check-{id}")), cx))
            .when(!task.tags.is_empty() || due.is_some(), |el| {
                el.child(
                    h_flex()
                        .pl(px(24.))
                        .gap_1()
                        .flex_wrap()
                        .children(task.tags.iter().map(|tag| {
                            Chip::new(tag.clone(), tag_colors.get(tag).copied().unwrap_or(theme.colors().text_muted))
                        }))
                        .when_some(due, |el, (label, overdue)| {
                            el.child(Label::new(label).size(LabelSize::XSmall).color(if overdue { Color::Error } else { Color::Accent }))
                        }),
                )
            })
            .children(subtasks.iter().map(|sub| {
                div().pl(px(16.)).child(line(sub, SharedString::from(format!("card-check-{}", sub.id)), cx))
            }))
            .into_any_element()
    }

    /// A group header: its label and count; a list section also gets a "…" menu with Rename and
    /// Delete, and is replaced by a field while it is renamed.
    fn render_group_header(
        &self,
        label: String,
        count: usize,
        drop: Option<&DropTarget>,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let section = match drop {
            Some(DropTarget::Section(Some(id))) => Some(*id),
            _ => None,
        };
        let renaming = section.and_then(|id| self.renaming_section.as_ref().filter(|(s, _)| *s == id)).map(|(_, input)| input.clone());
        let group = SharedString::from(format!("group-header-{label}"));
        h_flex()
            .id(group.clone())
            .group(group.clone())
            .h(px(height))
            .gap_2()
            .map(|el| match renaming {
                Some(input) => el.child(
                    h_flex().flex_1().h(px(28.)).px_2().rounded(px(6.)).bg(cx.theme().colors().surface_background).child(input),
                ),
                None => el
                    .child(Label::new(label).weight(FontWeight::BOLD))
                    .child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)),
            })
            .when_some(section, |el, id| {
                el.child(div().flex_1()).child(
                    div()
                        .relative()
                        .opacity(if self.section_menu == Some(id) { 1. } else { 0. })
                        .group_hover(group, |s| s.opacity(1.))
                        .child(
                            IconButton::new(SharedString::from(format!("section-more-{id}")), IconName::Ellipsis)
                                .size(px(22.))
                                .icon_size(IconSize::Small)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.section_menu = Some(id);
                                    cx.notify();
                                })),
                        )
                        .when(self.section_menu == Some(id), |el| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowRight,
                                popover(cx)
                                    .w(px(200.))
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
            .into_any_element()
    }

    fn start_rename_section(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
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
            this.renaming_section = None;
            this.reload(cx);
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.renaming_section = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.section_menu = None;
        self.renaming_section = Some((id, input));
        cx.notify();
    }

    fn print_list(&mut self, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        let strings = i18n::t(cx);
        let title = self.view_title(strings);
        let html = crate::print::list_html(&title, &self.tasks, Self::today(), strings);
        if let Err(err) = crate::print::open("list", &html) {
            self.error = Some(err.to_string());
        }
        cx.notify();
    }

    fn view_title(&self, strings: &Strings) -> SharedString {
        let filters: Vec<fern_core::SmartFilter> = self.filters.iter().map(|(f, _)| f.clone()).collect();
        lists_panel::title(&self.view, &self.lists, &filters, strings)
    }

    /// Opens the filter dialog for a new filter, or for the current one.
    fn open_filter_edit(&mut self, existing: Option<fern_core::SmartFilter>, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let is_new = existing.is_none();
        let draft = existing.unwrap_or_else(fern_core::SmartFilter::new);
        let field = |placeholder: &'static str, text: String, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new(Mode::SingleLine, placeholder, cx);
                input.set_text(text, cx);
                input
            })
        };
        let name = field(strings.filter_name, draft.name.clone(), cx);
        let keywords = field(strings.filter_keywords, draft.keywords.clone(), cx);
        window.focus(&name.focus_handle(cx), cx);
        self.header_menu_open = false;
        let advanced = draft.match_any;
        self.filter_edit = Some(FilterEdit { draft, is_new, advanced, name, keywords });
        cx.notify();
    }

    fn update_filter_draft(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut fern_core::SmartFilter)) {
        if let Some(edit) = self.filter_edit.as_mut() {
            change(&mut edit.draft);
        }
        cx.notify();
    }

    fn save_filter(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.filter_edit.as_ref() else { return };
        let mut filter = edit.draft.clone();
        filter.name = edit.name.read(cx).text().trim().to_owned();
        filter.keywords = edit.keywords.read(cx).text().trim().to_owned();
        if filter.name.is_empty() {
            return;
        }
        let result = self.store.save_filter(&filter);
        if self.check(result).is_some() {
            self.filter_edit = None;
            self.view = View::Filter(filter.id);
            self.reload(cx);
        }
    }

    fn delete_current_filter(&mut self, cx: &mut Context<Self>) {
        if let View::Filter(id) = self.view {
            let result = self.store.delete_filter(id);
            self.check(result);
            self.header_menu_open = false;
            self.view = View::Inbox;
            self.reload(cx);
        }
    }

    /// The original's "Add Filter" dialog: name, lists, tags, date, priority, keywords, type.
    fn render_filter_edit(&self, edit: &FilterEdit, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let draft = &edit.draft;
        let chip = |id: String, label: SharedString, on: bool| {
            div()
                .id(SharedString::from(id))
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(12.))
                .cursor_pointer()
                .when(on, |el| el.bg(theme.colors().accent.opacity(0.25)))
                .when(!on, |el| el.bg(theme.colors().element_hover))
                .child(Label::new(label).size(LabelSize::XSmall).color(if on { Color::Accent } else { Color::Default }))
        };
        let row = |label: &'static str, content: AnyElement| {
            h_flex()
                .items_start()
                .gap_3()
                .child(div().w(px(72.)).pt_1().child(Label::new(label).size(LabelSize::Small)))
                .child(div().flex_1().min_w_0().child(content))
        };
        let field = |input: Entity<TextInput>| {
            h_flex().h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().background).child(input)
        };
        // Lists: "All" or any of the inbox and lists.
        let list_options: Vec<(Option<Id>, SharedString)> = std::iter::once((None, SharedString::new_static(strings.inbox)))
            .chain(self.lists.iter().map(|l| (Some(l.id), l.name.clone().into())))
            .collect();
        let lists = h_flex()
            .flex_wrap()
            .gap_1()
            .child(chip("filter-lists-all".into(), strings.filter_all.into(), draft.lists.is_empty()).on_click(
                cx.listener(|this, _, _, cx| this.update_filter_draft(cx, |d| d.lists.clear())),
            ))
            .children(list_options.into_iter().map(|(list, name)| {
                let on = draft.lists.contains(&list);
                chip(format!("filter-list-{list:?}"), name, on).on_click(cx.listener(move |this, _, _, cx| {
                    this.update_filter_draft(cx, |d| {
                        if let Some(at) = d.lists.iter().position(|l| *l == list) {
                            d.lists.remove(at);
                        } else {
                            d.lists.push(list);
                        }
                    })
                }))
            }));
        let tags = h_flex()
            .flex_wrap()
            .gap_1()
            .child(chip("filter-tags-all".into(), strings.filter_all.into(), draft.tags.is_empty()).on_click(
                cx.listener(|this, _, _, cx| this.update_filter_draft(cx, |d| d.tags.clear())),
            ))
            .children(self.tags.iter().map(|tag| {
                let name = tag.name.clone();
                let on = draft.tags.iter().any(|t| t.to_lowercase() == name.to_lowercase());
                chip(format!("filter-tag-{name}"), format!("#{name}").into(), on).on_click(cx.listener(move |this, _, _, cx| {
                    let name = name.clone();
                    this.update_filter_draft(cx, |d| {
                        if let Some(at) = d.tags.iter().position(|t| t.to_lowercase() == name.to_lowercase()) {
                            d.tags.remove(at);
                        } else {
                            d.tags.push(name);
                        }
                    })
                }))
            }));
        let dates = h_flex().flex_wrap().gap_1().children(fern_core::DateRule::ALL.into_iter().enumerate().map(|(i, rule)| {
            let label = if i == 0 { strings.filter_all } else { strings.filter_dates[i - 1] };
            chip(format!("filter-date-{}", rule.key()), label.into(), draft.date == rule)
                .on_click(cx.listener(move |this, _, _, cx| this.update_filter_draft(cx, |d| d.date = rule)))
        }));
        let priorities = h_flex()
            .flex_wrap()
            .gap_1()
            .child(chip("filter-priority-all".into(), strings.filter_all.into(), draft.priorities.is_empty()).on_click(
                cx.listener(|this, _, _, cx| this.update_filter_draft(cx, |d| d.priorities.clear())),
            ))
            .children(
                [
                    (Priority::High, strings.priority_high),
                    (Priority::Medium, strings.priority_medium),
                    (Priority::Low, strings.priority_low),
                    (Priority::None, strings.priority_none),
                ]
                .into_iter()
                .map(|(priority, label)| {
                    chip(format!("filter-priority-{}", priority.to_i64()), label.into(), draft.priorities.contains(&priority))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_filter_draft(cx, |d| {
                                if let Some(at) = d.priorities.iter().position(|p| *p == priority) {
                                    d.priorities.remove(at);
                                } else {
                                    d.priorities.push(priority);
                                }
                            })
                        }))
                }),
            );
        let kinds = h_flex().gap_1().children([(None, strings.filter_all), (Some(Kind::Task), strings.filter_kinds[0]), (Some(Kind::Note), strings.filter_kinds[1])].into_iter().map(
            |(kind, label)| {
                chip(format!("filter-kind-{kind:?}"), label.into(), draft.kind == kind)
                    .on_click(cx.listener(move |this, _, _, cx| this.update_filter_draft(cx, |d| d.kind = kind)))
            },
        ));
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px_4()
                .h(px(30.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };
        let is_new = edit.is_new;
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.35))
            .child(
                v_flex()
                    .id("filter-dialog")
                    .w(px(480.))
                    .p_4()
                    .gap_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.colors().border)
                    .bg(theme.colors().elevated_surface_background)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Label::new(if is_new { strings.add_filter } else { strings.edit_filter }).weight(FontWeight::SEMIBOLD))
                    .child(h_flex().justify_center().gap_1().children([false, true].map(|advanced| {
                        chip(format!("filter-mode-{advanced}"), strings.filter_modes[advanced as usize].into(), edit.advanced == advanced)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(edit) = this.filter_edit.as_mut() {
                                    edit.advanced = advanced;
                                    if !advanced {
                                        edit.draft.match_any = false;
                                    }
                                }
                                cx.notify();
                            }))
                    })))
                    .child(field(edit.name.clone()))
                    .when(edit.advanced, |el| {
                        el.child(row(
                            strings.filter_match,
                            h_flex()
                                .gap_1()
                                .children([false, true].map(|any| {
                                    chip(format!("filter-match-{any}"), strings.filter_match_options[any as usize].into(), draft.match_any == any)
                                        .on_click(cx.listener(move |this, _, _, cx| this.update_filter_draft(cx, |d| d.match_any = any)))
                                }))
                                .into_any_element(),
                        ))
                    })
                    .child(row(strings.filter_lists, lists.into_any_element()))
                    .child(row(strings.tags, tags.into_any_element()))
                    .child(row(strings.filter_date, dates.into_any_element()))
                    .child(row(strings.filter_priority, priorities.into_any_element()))
                    .child(row(strings.filter_include, field(edit.keywords.clone()).into_any_element()))
                    .child(row(strings.filter_type, kinds.into_any_element()))
                    .child(
                        h_flex()
                            .gap_2()
                            .pt_2()
                            .child(div().flex_1())
                            .child(button("filter-close", strings.close, false).on_click(cx.listener(|this, _, _, cx| {
                                this.filter_edit = None;
                                cx.notify();
                            })))
                            .child(button("filter-save", strings.save, true).on_click(cx.listener(|this, _, _, cx| this.save_filter(cx)))),
                    ),
            )
            .into_any_element()
    }

    fn open_list_activity(&mut self, cx: &mut Context<Self>) {
        let list = match self.view {
            View::List(id) => Some(id),
            _ => None,
        };
        self.list_activity = Some(self.store.list_activity(list, 300).unwrap_or_default());
        self.header_menu_open = false;
        cx.notify();
    }

    /// The original's List Activities: the list's task events by day, newest first, each with
    /// its task, which a click opens.
    fn render_list_activity(&self, entries: &[(fern_core::Activity, Id, String)], cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Self::today();
        let mut items: Vec<AnyElement> = Vec::new();
        let mut current_day = None;
        for (activity, task, title) in entries {
            let Some(when) = Local.timestamp_millis_opt(activity.at).single() else { continue };
            let day = when.date_naive();
            if current_day != Some(day) {
                current_day = Some(day);
                items.push(
                    div()
                        .pt_3()
                        .pb_1()
                        .child(Label::new(task_rows::day_header(day, today, strings)).size(LabelSize::XSmall).color(Color::Muted))
                        .into_any_element(),
                );
            }
            let (icon, color) = match activity.kind.as_str() {
                "created" => (IconName::Plus, theme.colors().accent),
                "completed" => (IconName::Check, theme.status().success),
                "deleted" => (IconName::Trash, theme.status().error),
                _ => (IconName::Pencil, theme.colors().text_muted),
            };
            let task = *task;
            items.push(
                h_flex()
                    .items_start()
                    .gap_2()
                    .py_1()
                    .child(
                        div()
                            .mt(px(2.))
                            .size(px(18.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(color)
                            .child(Icon::new(icon).size(IconSize::XSmall).color(Color::Custom(theme.colors().background))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(Label::new(crate::task_detail::activity_text(&activity.kind, strings).to_owned()).size(LabelSize::Small).color(Color::Muted))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("activity-{task}-{}", activity.at)))
                                            .min_w_0()
                                            .cursor_pointer()
                                            .child(Label::new(title.clone()).size(LabelSize::Small).color(Color::Accent).truncate())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.list_activity = None;
                                                this.select(task, cx);
                                            })),
                                    ),
                            )
                            .child(Label::new(format!("{} {}", crate::calendar::format_day_month(when.date_naive(), strings), crate::calendar::format_time(when))).size(LabelSize::XSmall).color(Color::Muted)),
                    )
                    .into_any_element(),
            );
        }
        v_flex()
            .size_full()
            .bg(theme.colors().background)
            .border_l_1()
            .border_color(theme.colors().border)
            .child(
                h_flex()
                    .h(px(48.))
                    .px_5()
                    .border_b_1()
                    .border_color(theme.colors().border)
                    .child(div().flex_1().child(Label::new(strings.list_activities).size(LabelSize::Large)))
                    .child(IconButton::new("close-list-activity", IconName::X).on_click(cx.listener(|this, _, _, cx| {
                        this.list_activity = None;
                        cx.notify();
                    }))),
            )
            .child(
                v_flex()
                    .id("list-activity")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .pb_4()
                    .when(entries.is_empty(), |el| el.child(div().pt_4().child(Label::new(strings.stats_no_data).color(Color::Muted))))
                    .children(items),
            )
            .into_any_element()
    }

    fn start_new_section(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.section_name, cx));
        cx.subscribe(&input, |this, _, Submitted(name), cx| {
            if let View::List(list) = this.view {
                let result = this.store.add_list_section(list, name);
                this.check(result);
            }
            this.new_section = None;
            this.reload(cx);
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.new_section = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.new_section = Some(input);
        self.header_menu_open = false;
        cx.notify();
    }

    /// A card dropped on a kanban column takes the column's value: its section, kind, date,
    /// priority or tag, or completion. Leaving the Completed column reopens the task.
    fn drop_on_column(&mut self, id: Id, target: DropTarget, cx: &mut Context<Self>) {
        let Ok(Some(task)) = self.store.task(id) else { return };
        let store = &self.store;
        let result = (|| -> anyhow::Result<()> {
            if task.is_completed() && target != DropTarget::Complete {
                store.set_completed(id, false)?;
            }
            match target {
                DropTarget::Section(section) => store.set_section(id, section),
                DropTarget::Kind(kind) => store.set_kind(id, kind),
                DropTarget::Complete => store.set_completed(id, true),
                DropTarget::Due(date) => store.set_due(id, date, date.and(task.due_time)),
                DropTarget::Priority(priority) => store.set_priority(id, priority),
                DropTarget::Tag(Some(tag)) => {
                    let mut tags: Vec<String> = task.tags.iter().skip(1).filter(|t| **t != tag).cloned().collect();
                    tags.insert(0, tag);
                    store.set_tags(id, &tags)
                }
                DropTarget::Tag(None) => store.set_tags(id, &[]),
            }
        })();
        self.check(result);
        self.reload(cx);
    }

    /// A per-list switch of the header's "…" menu ("Show Completed", "Show Details").
    fn view_flag(&self, name: &str, default: bool) -> bool {
        let key = format!("{name}:{}", self.view_key());
        self.store.setting(&key).ok().flatten().map_or(default, |v| v == "1")
    }

    fn toggle_view_flag(&mut self, name: &str, default: bool, cx: &mut Context<Self>) {
        let key = format!("{name}:{}", self.view_key());
        let value = !self.view_flag(name, default);
        let result = self.store.set_setting(&key, if value { "1" } else { "0" });
        self.check(result);
        self.show_details = self.view_flag("show_details", false);
        self.kanban = self.view_flag("kanban", false);
        self.timeline = self.view_flag("timeline", false);
        self.header_menu_open = false;
        cx.notify();
    }

    /// The header menu's View: list, kanban or timeline, one at a time.
    fn set_layout(&mut self, kanban: bool, timeline: bool, cx: &mut Context<Self>) {
        let key = self.view_key();
        for (name, on) in [("kanban", kanban), ("timeline", timeline)] {
            let result = self.store.set_setting(&format!("{name}:{key}"), if on { "1" } else { "0" });
            self.check(result);
        }
        self.kanban = kanban;
        self.timeline = timeline;
        self.timeline_from = Self::today();
        self.header_menu_open = false;
        cx.notify();
    }

    /// The original's Timeline: the groups as rows, the coming days as columns (weekends shaded),
    /// and each dated task as a bar from its date to the end of its duration; a click opens it.
    fn render_timeline(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        const DAYS: usize = 10;
        const LABEL_WIDTH: f32 = 150.;
        let theme = cx.theme().clone();
        let today = Self::today();
        let from = self.timeline_from;
        let days: Vec<NaiveDate> = (0..DAYS as i64).map(|i| from + chrono::Duration::days(i)).collect();
        let groups = task_rows::kanban_columns(self.rows_with(strings, &HashSet::new(), &HashSet::new()), strings);
        let weekend = |date: &NaiveDate| matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        let shift = |delta: i64| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.timeline_from += chrono::Duration::days(delta);
                cx.notify();
            })
        };
        // The day columns behind each row, so that weekends stay shaded all the way down; a bar
        // dropped on one moves its task there, keeping its duration.
        let accent = theme.colors().accent;
        let backdrop = |row: usize, height: Pixels, cx: &mut Context<Self>| {
            h_flex().absolute().top_0().left_0().w_full().h(height).children(days.iter().map(|&date| {
                div()
                    .id(SharedString::from(format!("timeline-day-{row}-{date}")))
                    .flex_1()
                    .h_full()
                    .border_l_1()
                    .border_color(theme.colors().border_variant)
                    .when(weekend(&date), |el| el.bg(theme.colors().element_hover.opacity(0.5)))
                    .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.15)))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                        let result = this.store.move_dates(dragged.id, date);
                        this.check(result);
                        this.reload(cx);
                    }))
            }))
        };
        let header = v_flex()
            .flex_none()
            .child(
                h_flex()
                    .h(px(30.))
                    .px_3()
                    .child(div().flex_1().child(Label::new(strings.months_short[from.month0() as usize]).size(LabelSize::Small).color(Color::Muted)))
                    .child(
                        div()
                            .id("timeline-today")
                            .px_2()
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .rounded(px(11.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .cursor_pointer()
                            .child(Label::new(strings.today).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.timeline_from = Self::today();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .h(px(28.))
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(
                        h_flex()
                            .w(px(LABEL_WIDTH))
                            .flex_none()
                            .px_1()
                            .child(IconButton::new("timeline-prev", IconName::ChevronLeft).size(px(20.)).icon_size(IconSize::XSmall).on_click(shift(-7))),
                    )
                    .child(h_flex().flex_1().children(days.iter().map(|date| {
                        let is_today = *date == today;
                        div()
                            .flex_1()
                            .h_full()
                            .px_2()
                            .flex()
                            .items_center()
                            .when(weekend(date), |el| el.bg(theme.colors().element_hover.opacity(0.5)))
                            .child(Label::new(date.day().to_string()).size(LabelSize::Small).color(if is_today { Color::Accent } else { Color::Default }))
                    })))
                    .child(IconButton::new("timeline-next", IconName::ChevronRight).size(px(20.)).icon_size(IconSize::XSmall).on_click(shift(7))),
            );
        let rows = groups.into_iter().enumerate().map(|(i, (label, _, cards))| {
            let bars: Vec<(Task, usize, usize)> = cards
                .into_iter()
                .flat_map(|(task, subtasks)| std::iter::once(task).chain(subtasks))
                .filter_map(|task| fern_core::timeline::bar(&task, from, DAYS).map(|(first, span)| (task, first, span)))
                .collect();
            let height = px(16. + 32. * bars.len().max(1) as f32);
            h_flex()
                .h(height)
                .border_b_1()
                .border_color(theme.colors().border_variant)
                .child(
                    div()
                        .w(px(LABEL_WIDTH))
                        .flex_none()
                        .h_full()
                        .px_3()
                        .pt_2()
                        .child(Label::new(label).size(LabelSize::Small).weight(FontWeight::SEMIBOLD)),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .mr(px(20.))
                        .relative()
                        .child(backdrop(i, height, cx))
                        .children(bars.into_iter().enumerate().map(|(n, (task, first, span))| {
                            let id = task.id;
                            let tint = task_rows::priority_color(task.priority, &theme).unwrap_or(theme.colors().accent);
                            let dragged = DraggedTask { id, title: task.title.clone().into() };
                            div()
                                .id(SharedString::from(format!("timeline-{i}-{id}")))
                                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                                .absolute()
                                .top(px(8. + 32. * n as f32))
                                .left(gpui::relative(first as f32 / DAYS as f32))
                                .w(gpui::relative(span as f32 / DAYS as f32))
                                .h(px(26.))
                                .px_1()
                                .child(
                                    div()
                                        .size_full()
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.))
                                        .bg(tint.opacity(if task.is_completed() { 0.3 } else { 0.75 }))
                                        .cursor_pointer()
                                        .child(Label::new(task.title.clone()).size(LabelSize::Small).truncate()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                        })),
                )
        });
        v_flex().id("timeline").flex_1().min_h_0().overflow_y_scroll().child(header).children(rows).into_any_element()
    }

    fn sort_settings(&self) -> (SortBy, GroupBy) {
        let key = self.view_key();
        let get = |what: &str| self.store.setting(&format!("{what}:{key}")).ok().flatten().unwrap_or_default();
        (SortBy::from_key(&get("sort")), GroupBy::from_key(&get("group")))
    }

    fn set_sort(&mut self, sort: Option<SortBy>, group: Option<GroupBy>, cx: &mut Context<Self>) {
        let key = self.view_key();
        let result = sort
            .map(|s| self.store.set_setting(&format!("sort:{key}"), s.key()))
            .into_iter()
            .chain(group.map(|g| self.store.set_setting(&format!("group:{key}"), g.key())))
            .collect::<anyhow::Result<Vec<()>>>();
        self.check(result);
        self.sort_menu = None;
        cx.notify();
    }

    /// The original's sort menu: "Group by" and "Sort by", each opening its options in place.
    fn render_sort_menu(&self, expanded: Option<SortMenu>, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let (sort, group) = self.sort_settings();
        let row = |id: &'static str, icon: IconName, label: &'static str, value: &'static str, which: SortMenu| {
            ListItem::new(id)
                .start_slot(Icon::new(icon).color(Color::Muted))
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(div().flex_1().child(Label::new(label)))
                        .child(Label::new(value).color(Color::Muted))
                        .child(
                            Icon::new(if expanded == Some(which) { IconName::ChevronDown } else { IconName::ChevronRight })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sort_menu = Some(if expanded == Some(which) { None } else { Some(which) });
                    cx.notify();
                }))
        };
        let option = |id: String, label: &'static str, selected: bool| {
            ListItem::new(SharedString::from(id))
                .child(div().pl_6().child(Label::new(label).color(if selected { Color::Accent } else { Color::Default })))
                .when(selected, |item| item.end_slot(Label::new("✓").color(Color::Accent)))
        };
        popover(cx)
            .w(px(250.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.sort_menu = None;
                cx.notify();
            }))
            .child(row("group-by", IconName::Rows3, strings.group_by, group.label(strings), SortMenu::Group))
            .when(expanded == Some(SortMenu::Group), |el| {
                el.children(GroupBy::ALL.into_iter().map(|g| {
                    option(format!("group-{}", g.key()), g.label(strings), g == group)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_sort(None, Some(g), cx)))
                }))
            })
            .child(row("sort-by", IconName::ArrowDownUp, strings.sort_by, sort.label(strings), SortMenu::Sort))
            .when(expanded == Some(SortMenu::Sort), |el| {
                el.children(SortBy::ALL.into_iter().map(|s| {
                    option(format!("sort-{}", s.key()), s.label(strings), s == sort)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_sort(Some(s), None, cx)))
                }))
            })
            .into_any_element()
    }

    fn move_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let ids = self.visible_task_ids(i18n::t(cx));
        if ids.is_empty() {
            return;
        }
        let next = match self.selected.and_then(|id| ids.iter().position(|i| *i == id)) {
            Some(index) => (index as isize + step).clamp(0, ids.len() as isize - 1) as usize,
            None if step > 0 => 0,
            None => ids.len() - 1,
        };
        self.select(ids[next], cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    fn toggle_selected(&mut self, _: &ToggleComplete, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(task) = self.selected.and_then(|id| self.tasks.iter().find(|t| t.id == id)) {
            let (id, done) = (task.id, task.is_completed());
            self.toggle(id, !done, cx);
        }
    }

    /// Applies a change to the selected task, for the task edit shortcuts.
    fn edit_selected(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&Store, &Task) -> anyhow::Result<()>) {
        let Some(task) = self.selected.and_then(|id| self.tasks.iter().find(|t| t.id == id)).cloned() else { return };
        let result = change(&self.store, &task);
        self.check(result);
        self.reload(cx);
        self.select(task.id, cx);
    }

    fn pin_selected(&mut self, _: &PinTask, _: &mut Window, cx: &mut Context<Self>) {
        self.edit_selected(cx, |store, task| store.set_pinned(task.id, task.pinned_at.is_none()));
    }

    fn date_selected(&mut self, _: &SetDate, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_some() {
            self.detail.update(cx, |detail, cx| detail.open_due(cx));
        }
    }

    fn set_selected_due(&mut self, days: Option<i64>, cx: &mut Context<Self>) {
        let today = Self::today();
        self.edit_selected(cx, move |store, task| {
            let date = days.map(|d| today + Duration::days(d));
            store.set_due(task.id, date, date.and(task.due_time))
        });
    }

    fn set_selected_priority(&mut self, priority: Priority, cx: &mut Context<Self>) {
        self.edit_selected(cx, move |store, task| store.set_priority(task.id, priority));
    }

    fn delete_selected(&mut self, _: &DeleteTask, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        // Keep a selection so repeated presses keep working down the list.
        let ids = self.visible_task_ids(i18n::t(cx));
        let neighbor = ids
            .iter()
            .position(|i| *i == id)
            .and_then(|index| ids.get(index + 1).or_else(|| index.checked_sub(1).and_then(|i| ids.get(i))))
            .copied();
        self.delete(id, cx);
        if let Some(neighbor) = neighbor {
            self.select(neighbor, cx);
        }
    }

    pub fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        self.list_activity = None;
        self.renaming_list = false;
        if self.view != view {
            self.view = view;
            self.reload(cx);
        }
    }

    /// Switches the list to search results and focuses the search field.
    #[cfg(debug_assertions)]
    pub fn start_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).text().to_owned();
        self.set_view(View::Search(query), cx);
        window.focus(&self.search_input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Shows the full results for `query` in the list (the popover's "View more").
    pub fn show_search_results(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_input.update(cx, |input, cx| input.set_text(query.clone(), cx));
        self.set_view(View::Search(query), cx);
    }

    /// Leaves search results for the Inbox.
    pub fn stop_search(&mut self, cx: &mut Context<Self>) {
        if matches!(self.view, View::Search(_)) {
            self.set_view(View::Inbox, cx);
        }
    }

    fn start_new_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.creating_list = true;
        self.show_lists = true;
        window.focus(&self.new_list.focus_handle(cx), cx);
        cx.notify();
    }

    fn create_list(&mut self, name: &str, cx: &mut Context<Self>) {
        self.creating_list = false;
        if name.trim().is_empty() {
            cx.notify();
            return;
        }
        let result = self.store.create_list(name);
        if let Some(list) = self.check(result) {
            self.view = View::List(list.id);
        }
        self.reload(cx);
    }

    fn current_list(&self) -> Option<&List> {
        match self.view {
            View::List(id) => self.lists.iter().find(|l| l.id == id),
            _ => None,
        }
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.current_list().map(|l| l.name.clone()) else { return };
        self.header_menu_open = false;
        self.renaming_list = true;
        self.rename_input.update(cx, |input, cx| input.set_text(name, cx));
        window.focus(&self.rename_input.focus_handle(cx), cx);
        cx.notify();
    }

    fn rename_current_list(&mut self, name: &str, cx: &mut Context<Self>) {
        self.renaming_list = false;
        if let View::List(id) = self.view
            && !name.trim().is_empty()
        {
            let result = self.store.rename_list(id, name);
            self.check(result);
        }
        self.reload(cx);
    }

    fn set_current_list_color(&mut self, color: Option<&str>, cx: &mut Context<Self>) {
        if let View::List(id) = self.view {
            let result = self.store.set_list_color(id, color);
            self.check(result);
        }
        self.header_menu_open = false;
        self.reload(cx);
    }

    fn delete_current_list(&mut self, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        if let View::List(id) = self.view {
            let result = self.store.delete_list(id);
            self.check(result);
            self.view = View::Inbox;
        }
        self.reload(cx);
    }

    fn toggle_task_collapsed(&mut self, id: Id, cx: &mut Context<Self>) {
        if !self.collapsed_tasks.remove(&id) {
            self.collapsed_tasks.insert(id);
        }
        cx.notify();
    }

    fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if !self.collapsed_sections.remove(&section) {
            self.collapsed_sections.insert(section);
        }
        cx.notify();
    }

    pub fn toggle_lists(&mut self, cx: &mut Context<Self>) {
        self.show_lists = !self.show_lists;
        cx.notify();
    }

    /// Database errors are shown in a line at the bottom of the window instead of crashing.
    fn check<T>(&mut self, result: anyhow::Result<T>) -> Option<T> {
        match result {
            Ok(value) => {
                self.error = None;
                Some(value)
            }
            Err(err) => {
                self.error = Some(format!("{err:#}"));
                None
            }
        }
    }

    pub fn select(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.selected == Some(id) {
            return;
        }
        self.selected = Some(id);
        // The task may be outside the current view, for example a subtask opened from the panel.
        let task = self
            .tasks
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .or_else(|| self.store.task(id).ok().flatten());
        self.detail.update(cx, |detail, cx| detail.show(task, cx));
        cx.notify();
    }

    fn toggle(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        // Unchecking a "Won't Do" task reopens it, as in the original.
        let wont_do = self.tasks.iter().any(|t| t.id == id && t.is_wont_do());
        let task = self.tasks.iter().find(|t| t.id == id).cloned();
        match task {
            // Completing offers Undo, also for a repeating task, which moves to its next date.
            Some(task) if completed && !wont_do => {
                let result = crate::undo::complete(&self.store, &task);
                if let Some(action) = self.check(result) {
                    let label = i18n::t(cx).undo_completed.replace("{title}", &task.title);
                    self.offer_undo(action, label, cx);
                }
            }
            _ => {
                let result =
                    if wont_do { self.store.set_wont_do(id, false) } else { self.store.set_completed(id, completed) };
                self.check(result);
            }
        }
        self.reload(cx);
    }

    /// Shows the "Undo" toast for a few seconds.
    fn offer_undo(&mut self, action: crate::undo::UndoAction, label: String, cx: &mut Context<Self>) {
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
            this.update(cx, |this, cx| {
                if this.undo.as_ref().is_some_and(|(a, _, _)| *a == action) {
                    this.undo = None;
                    cx.notify();
                }
            })
            .ok();
        });
        self.undo = Some((action, label, timer));
        cx.notify();
    }

    fn apply_undo(&mut self, cx: &mut Context<Self>) {
        if let Some((action, _, _)) = self.undo.take() {
            let result = action.apply(&self.store);
            self.check(result);
            self.reload(cx);
        }
    }

    /// Moves a task to the trash, or deletes it for good when it is already there.
    fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        let in_trash = self.view == View::Trash;
        let title = self.tasks.iter().find(|t| t.id == id).map(|t| t.title.clone()).unwrap_or_default();
        let result = if in_trash { self.store.purge_task(id) } else { self.store.delete_task(id) };
        if self.check(result).is_some() && !in_trash {
            let label = i18n::t(cx).undo_deleted.replace("{title}", &title);
            self.offer_undo(crate::undo::UndoAction::Restore(id), label, cx);
        }
        self.reload(cx);
    }

    /// A task dropped on an entry of the lists column takes that entry's meaning: a list moves it,
    /// a tag tags it, Today/Tomorrow set its date, Completed completes it, Trash deletes it.
    fn drop_on_view(&mut self, view: View, id: Id, cx: &mut Context<Self>) {
        let today = Self::today();
        let time = self.store.task(id).ok().flatten().and_then(|t| t.due_time);
        let result = match &view {
            View::Inbox => self.store.set_list(id, None),
            View::List(list) => self.store.set_list(id, Some(*list)),
            View::Tag(tag) => match self.store.task(id) {
                Ok(Some(task)) if !task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) => {
                    let mut tags = task.tags;
                    tags.push(tag.clone());
                    self.store.set_tags(id, &tags)
                }
                other => other.map(|_| ()),
            },
            View::Today | View::Next7Days => self.store.set_due(id, Some(today), time),
            View::Tomorrow => self.store.set_due(id, Some(today + Duration::days(1)), time),
            View::Completed => self.store.set_completed(id, true),
            View::WontDo => self.store.set_wont_do(id, true),
            View::Trash => self.store.delete_task(id),
            View::Day(day) => self.store.set_due(id, Some(*day), time),
            View::All | View::Search(_) | View::Filter(_) | View::Summary => Ok(()),
        };
        self.check(result);
        self.reload(cx);
    }

    /// Drops a dragged task onto the right part of `parent`'s row: it becomes a subtask there,
    /// in the parent's list. A task cannot go inside itself or its own subtasks.
    fn nest_task(&mut self, dragged: Id, parent: Id, cx: &mut Context<Self>) {
        if dragged == parent {
            return;
        }
        let result = self.store.link_parent(dragged, parent);
        self.check(result);
        self.reload(cx);
    }

    /// Drops a dragged task right before `target`.
    fn drop_task(&mut self, dragged: Id, target: Id, cx: &mut Context<Self>) {
        let result = self.store.move_task(dragged, target, false);
        self.check(result);
        self.reload(cx);
    }

    fn restore(&mut self, id: Id, cx: &mut Context<Self>) {
        let result = self.store.restore_task(id);
        self.check(result);
        self.reload(cx);
    }

    fn empty_trash(&mut self, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        let result = self.store.empty_trash();
        self.check(result);
        self.reload(cx);
    }

    /// Views where new tasks cannot be added.
    fn is_read_only_view(&self) -> bool {
        matches!(self.view, View::Completed | View::WontDo | View::Trash | View::Search(_))
    }

    fn render_section(
        &self,
        section: Section,
        count: usize,
        collapsed: bool,
        strings: &Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .id(SharedString::from(format!("group-{section:?}")))
            .h(px(36.))
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
            .child(chevron(collapsed))
            .child(Label::new(section.title(strings)).weight(FontWeight::BOLD))
            .child(div().pl_1().child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_task(
        &self,
        task: &Task,
        depth: usize,
        has_children: bool,
        collapsed: bool,
        tag_colors: &HashMap<String, Hsla>,
        today: NaiveDate,
        strings: &Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let id = task.id;
        let done = task.is_completed();
        let details = self.show_details && !task.notes.trim().is_empty();
        let wont_do = task.is_wont_do();
        let closed_view = matches!(self.view, View::Completed | View::WontDo);
        let selected = self.selected == Some(id);
        let group = format!("task-{id}");

        let marker = if task.kind == Kind::Note {
            Icon::new(IconName::FileText).into_any_element()
        } else {
            Checkbox::new("marker")
                .toggle_state(done)
                .dismissed(wont_do)
                .lines(task.checklist.is_some())
                .tint(task_rows::priority_color(task.priority, &theme))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle(id, !done && !wont_do, cx);
                }))
                .into_any_element()
        };

        let due = task.due_date.map(|date| task_rows::due_label(date, task.due_time, today, strings));

        let draggable = self.view != View::Trash;
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let accent = theme.colors().accent;

        // The separator starts at the checkbox; the gutter to its left holds the chevron.
        div()
            .id(ElementId::Uuid(id))
            .group(group.clone())
            .when(draggable, |el| {
                // Two drop zones, as in the original: on the left a dragged task goes before this
                // one; on the right it becomes this task's subtask.
                el.on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(gpui::relative(NEST_ZONE))
                            .drag_over::<DraggedTask>(move |style, _, _, _| style.border_t_2().border_color(accent))
                            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_task(dragged.id, id, cx))),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .h_full()
                            .w(gpui::relative(1. - NEST_ZONE))
                            .rounded(px(6.))
                            .drag_over::<DraggedTask>(move |style, _, _, _| {
                                style.border_1().border_color(accent).bg(accent.opacity(0.08))
                            })
                            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.nest_task(dragged.id, id, cx))),
                    )
            })
            .h(px(if details { ROW_HEIGHT + 14. } else { ROW_HEIGHT }))
            .flex()
            .pl(px(depth as f32 * INDENT))
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.colors().ghost_element_selected))
            .when(!selected, |el| el.hover(|s| s.bg(theme.colors().ghost_element_hover)))
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.list_focus, cx);
                this.select(id, cx);
            }))
            .child(
                div()
                    .id("toggle-children")
                    .w(px(GUTTER))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(has_children, |el| {
                        el.child(chevron(collapsed)).on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_task_collapsed(id, cx);
                        }))
                    }),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(px(10.))
                    .pr_3()
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(marker)
                    .when(task.pinned_at.is_some() && !closed_view, |el| {
                        el.child(Icon::new(IconName::Pin).size(IconSize::XSmall).color(Color::Accent))
                    })
                    .child(
                        v_flex().flex_1().min_w_0().child(
                            Label::new(if task.title.is_empty() {
                                SharedString::new_static(strings.untitled)
                            } else {
                                task.title.clone().into()
                            })
                            .color(if done || wont_do || task.title.is_empty() { Color::Muted } else { Color::Default })
                            .strikethrough(done)
                            .truncate(),
                        )
                        // "Show Details": the first line of the notes under the title.
                        .when_some(details.then(|| task.notes.lines().find(|l| !l.trim().is_empty())).flatten(), |el, line| {
                            el.child(Label::new(line.trim().to_owned()).size(LabelSize::XSmall).color(Color::Muted).truncate())
                        }),
                    )
                    .children(task.tags.iter().map(|tag| {
                        let color = tag_colors.get(tag).copied().unwrap_or(theme.colors().text_muted);
                        Chip::new(tag.clone(), color)
                    }))
                    .when_some(task.checklist, |el, (done_items, total)| {
                        el.child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .child(Icon::new(IconName::ListChecks).size(IconSize::XSmall))
                                .child(
                                    Label::new(format!("{done_items}/{total}"))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        )
                    })
                    // Closed tasks show their list, as in the original's Completed and Won't Do views.
                    .when(closed_view, |el| {
                        let list = task
                            .list_id
                            .and_then(|id| self.lists.iter().find(|l| l.id == id))
                            .map_or(SharedString::new_static(strings.inbox), |l| l.name.clone().into());
                        el.child(Label::new(list).size(LabelSize::Small).color(Color::Muted))
                    })
                    .when_some(due, |el, (label, overdue)| {
                        el.when(!task.reminders.is_empty() && !closed_view, |el| {
                            el.child(Icon::new(IconName::AlarmClock).size(IconSize::XSmall).color(Color::Muted))
                        })
                        .when(task.repeat.is_some(), |el| {
                            el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                        })
                        .child(
                            Label::new(label)
                                .size(LabelSize::Small)
                                .color(if closed_view {
                                    Color::Muted
                                } else if overdue && !done {
                                    Color::Error
                                } else {
                                    Color::Accent
                                }),
                        )
                    })
                    .when(self.view == View::Trash, |el| {
                        el.child(
                            div()
                                .id("restore")
                                .flex_none()
                                .opacity(0.)
                                .group_hover(group.clone(), |s| s.opacity(1.))
                                .child(
                                    IconButton::new("restore-button", IconName::RotateCcw)
                                        .size(px(20.))
                                        .icon_size(IconSize::Small)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.restore(id, cx);
                                        })),
                                ),
                        )
                    })
                    .child(
                        div()
                            .id("delete")
                            .flex_none()
                            .px_1()
                            .text_color(theme.colors().text_muted)
                            .opacity(0.)
                            .group_hover(group, |s| s.opacity(1.))
                            .hover(|s| s.text_color(theme.status().error))
                            .child("✕")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.delete(id, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// As in the original, Today and Next 7 Days list the habits due today, unless turned off in
    /// the habit settings. Checking one checks the habit in (one unit for amount goals).
    fn render_habits(&self, strings: &Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !matches!(self.view, View::Today | View::Next7Days)
            || self.store.setting(crate::habit_view::IN_TODAY_SETTING).ok().flatten().as_deref() == Some("0")
        {
            return None;
        }
        let theme = cx.theme().clone();
        let today = Self::today();
        let habits: Vec<(fern_core::Habit, bool)> = self
            .store
            .habits()
            .unwrap_or_default()
            .into_iter()
            .filter(|h| !h.archived)
            .filter_map(|h| {
                let done = self.store.checkins(h.id).unwrap_or_default();
                h.is_due(today, &done).then(|| {
                    let checked = done.contains(&today);
                    (h, checked)
                })
            })
            .collect();
        if habits.is_empty() {
            return None;
        }
        let count = habits.len();
        Some(
            v_flex()
                .child(
                    h_flex()
                        .h(px(36.))
                        .gap_1()
                        .pl(px(GUTTER))
                        .child(Label::new(strings.section_habits).weight(FontWeight::BOLD))
                        .child(div().pl_1().child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted))),
                )
                .children(habits.into_iter().map(|(habit, checked)| {
                    let id = habit.id;
                    let has_goal = habit.goal.is_some();
                    h_flex()
                        .h(px(ROW_HEIGHT))
                        .pl(px(GUTTER))
                        .pr_3()
                        .gap(px(10.))
                        .child(
                            Checkbox::new(SharedString::from(format!("habit-check-{id}")))
                                .toggle_state(checked)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let result = if has_goal {
                                        this.store.step_checkin(id, today).map(|_| ())
                                    } else {
                                        this.store.toggle_checkin(id, today).map(|_| ())
                                    };
                                    this.check(result);
                                    cx.notify();
                                })),
                        )
                        .child(crate::habit_view::avatar(&habit, px(22.), cx))
                        .child(
                            div().flex_1().min_w_0().child(
                                Label::new(habit.name.clone())
                                    .color(if checked { Color::Muted } else { Color::Default })
                                    .strikethrough(checked)
                                    .truncate(),
                            ),
                        )
                        .border_b_1()
                        .border_color(theme.colors().border_variant)
                }))
                .into_any_element(),
        )
    }

    fn set_closed_menu(&mut self, menu: Option<ClosedMenu>, cx: &mut Context<Self>) {
        self.closed_menu = menu;
        cx.notify();
    }

    fn open_other_date(&mut self, cx: &mut Context<Self>) {
        let (start, end) = match self.closed_range {
            ClosedRange::Custom(start, end) => (Some(start), Some(end)),
            _ => (None, None),
        };
        let month = crate::calendar::month_start(start.unwrap_or_else(Self::today));
        self.set_closed_menu(Some(ClosedMenu::OtherDate { start, end, picking: None, month }), cx);
    }

    fn update_other_date(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Option<NaiveDate>, &mut Option<NaiveDate>, &mut Option<RangeEnd>, &mut NaiveDate),
    ) {
        if let Some(ClosedMenu::OtherDate { start, end, picking, month }) = self.closed_menu.as_mut() {
            change(start, end, picking, month);
        }
        cx.notify();
    }

    /// The "Other Date" popover: Start and End fields, a calendar for the one being picked.
    fn render_other_date(
        &self,
        start: Option<NaiveDate>,
        end: Option<NaiveDate>,
        picking: Option<RangeEnd>,
        month: NaiveDate,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let today = Self::today();
        let field = |which: RangeEnd, value: Option<NaiveDate>, placeholder: &'static str| {
            let label: SharedString =
                value.map_or(placeholder.into(), |date| task_rows::short_date(date, strings).into());
            h_flex()
                .id(if which == RangeEnd::Start { "range-start" } else { "range-end" })
                .h(px(30.))
                .px_2()
                .rounded(px(6.))
                .border_1()
                .border_color(if picking == Some(which) { theme.colors().accent } else { theme.colors().border })
                .cursor_pointer()
                .child(Label::new(label).color(if value.is_some() { Color::Default } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_other_date(cx, |_, _, picking, _| *picking = Some(which))
                }))
        };
        let calendar = picking.map(|which| {
            let selected = if which == RangeEnd::Start { start } else { end };
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .child(div().flex_1().child(Label::new(crate::calendar::month_title(month, strings)).weight(FontWeight::SEMIBOLD)))
                        .child(IconButton::new("range-prev", IconName::ChevronLeft).size(px(22.)).on_click(cx.listener(
                            |this, _, _, cx| this.update_other_date(cx, |_, _, _, month| *month = crate::calendar::add_months(*month, -1)),
                        )))
                        .child(IconButton::new("range-next", IconName::ChevronRight).size(px(22.)).on_click(cx.listener(
                            |this, _, _, cx| this.update_other_date(cx, |_, _, _, month| *month = crate::calendar::add_months(*month, 1)),
                        ))),
                )
                .child(h_flex().children(crate::calendar::weekday_labels(&strings.weekdays_narrow).iter().map(|day| {
                    div().flex_1().flex().justify_center().child(Label::new(*day).size(LabelSize::XSmall).color(Color::Muted))
                })))
                .children(crate::calendar::month_grid(month).chunks(7).map(|week| {
                    h_flex().children(week.iter().map(|&date| {
                        let is_selected = selected == Some(date);
                        div()
                            .id(SharedString::from(format!("range-day-{date}")))
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .when(is_selected, |el| el.bg(theme.colors().accent))
                            .when(!is_selected, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                            .child(
                                Label::new(date.day().to_string()).size(LabelSize::Small).color(if is_selected {
                                    Color::Default
                                } else if date == today {
                                    Color::Accent
                                } else if date.month() != month.month() {
                                    Color::Muted
                                } else {
                                    Color::Default
                                }),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_other_date(cx, |start, end, picking, _| {
                                    match which {
                                        RangeEnd::Start => *start = Some(date),
                                        RangeEnd::End => *end = Some(date),
                                    }
                                    *picking = None;
                                })
                            }))
                    }))
                }))
        });
        let (start_calendar, end_calendar) =
            if picking == Some(RangeEnd::Start) { (calendar, None) } else { (None, calendar) };
        let ready = start.is_some() && end.is_some();
        popover(cx)
            .w(px(240.))
            .p_2()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_closed_menu(None, cx)))
            .child(field(RangeEnd::Start, start, strings.range_start))
            .children(start_calendar)
            .child(field(RangeEnd::End, end, strings.range_end))
            .children(end_calendar)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id("range-cancel")
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .cursor_pointer()
                            .child(Label::new(strings.cancel).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| this.set_closed_menu(None, cx))),
                    )
                    .child(
                        div()
                            .id("range-confirm")
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(theme.colors().accent)
                            .when(!ready, |el| el.opacity(0.5))
                            .when(ready, |el| el.cursor_pointer())
                            .child(Label::new(strings.confirm).size(LabelSize::Small))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let (Some(start), Some(end)) = (start, end) {
                                    this.closed_range = ClosedRange::custom(start, end);
                                    this.set_closed_menu(None, cx);
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    /// Toggles a list in the lists-filter draft; "All Lists" is `None`.
    fn toggle_closed_list(&mut self, list: Option<Option<Id>>, cx: &mut Context<Self>) {
        if let Some(ClosedMenu::Lists(draft)) = self.closed_menu.as_mut() {
            *draft = match (list, draft.take()) {
                (None, _) => None,
                (Some(list), None) => Some(vec![list]),
                (Some(list), Some(mut lists)) => {
                    if let Some(at) = lists.iter().position(|l| *l == list) {
                        lists.remove(at);
                    } else {
                        lists.push(list);
                    }
                    (!lists.is_empty()).then_some(lists)
                }
            };
        }
        cx.notify();
    }

    /// The "All Dates" / "All Lists" filter buttons of the Completed and Won't Do views.
    fn render_closed_filters(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let list_name = |list: Option<Id>| -> SharedString {
            list.and_then(|id| self.lists.iter().find(|l| l.id == id))
                .map_or(SharedString::new_static(strings.inbox), |l| l.name.clone().into())
        };
        let lists_label: SharedString = match &self.closed_lists {
            None => strings.all_lists.into(),
            Some(lists) if lists.len() == 1 => list_name(lists[0]),
            Some(lists) => format!("{} ({})", strings.all_lists, lists.len()).into(),
        };
        let open = self.closed_menu.clone();
        let filter_button = |id: &'static str, label: SharedString, active: bool| {
            h_flex()
                .id(id)
                .h(px(24.))
                .px_2()
                .gap_1()
                .rounded(px(5.))
                .border_1()
                .border_color(if active { theme.colors().accent } else { theme.colors().border })
                .cursor_pointer()
                .child(Label::new(label).size(LabelSize::XSmall))
                .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
        };
        let check = || Label::new("✓").color(Color::Accent);

        let dates_menu = popover(cx)
            .w(px(180.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_closed_menu(None, cx)))
            .children(ClosedRange::PRESETS.into_iter().map(|range| {
                let selected = range == self.closed_range;
                ListItem::new(SharedString::from(format!("closed-range-{range:?}")))
                    .child(Label::new(range.label(strings)).color(if selected { Color::Accent } else { Color::Default }))
                    .when(selected, |item| item.end_slot(check()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.closed_range = range;
                        this.set_closed_menu(None, cx);
                    }))
            }))
            .child({
                let custom = matches!(self.closed_range, ClosedRange::Custom(..));
                ListItem::new("closed-range-other")
                    .child(Label::new(strings.other_date).color(if custom { Color::Accent } else { Color::Default }))
                    .when(custom, |item| item.end_slot(check()))
                    .on_click(cx.listener(|this, _, _, cx| this.open_other_date(cx)))
            });
        let other_menu = match &open {
            Some(ClosedMenu::OtherDate { start, end, picking, month }) => {
                Some(self.render_other_date(*start, *end, *picking, *month, strings, cx))
            }
            _ => None,
        };

        let draft = match &open {
            Some(ClosedMenu::Lists(draft)) => draft.clone(),
            _ => None,
        };
        let entries: Vec<(Option<Option<Id>>, SharedString, IconName)> = std::iter::once((None, strings.all_lists.into(), IconName::Layers))
            .chain(std::iter::once((Some(None), strings.inbox.into(), IconName::Inbox)))
            .chain(self.lists.iter().map(|l| (Some(Some(l.id)), l.name.clone().into(), IconName::List)))
            .collect();
        let lists_menu = popover(cx)
            .w(px(240.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_closed_menu(None, cx)))
            .children(entries.into_iter().map(|(list, label, icon)| {
                let selected = match (&list, &draft) {
                    (None, None) => true,
                    (Some(list), Some(lists)) => lists.contains(list),
                    _ => false,
                };
                let id = match list {
                    None => "closed-list-all".to_owned(),
                    Some(None) => "closed-list-inbox".to_owned(),
                    Some(Some(id)) => format!("closed-list-{id}"),
                };
                ListItem::new(SharedString::from(id))
                    .start_slot(Icon::new(icon).color(if selected { Color::Accent } else { Color::Muted }))
                    .child(Label::new(label).color(if selected { Color::Accent } else { Color::Default }))
                    .when(selected, |item| item.end_slot(check()))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_closed_list(list, cx)))
            }))
            .child(div().py_1().child(Divider::horizontal()))
            .child(
                h_flex()
                    .gap_2()
                    .p_1()
                    .child(
                        div()
                            .id("closed-lists-cancel")
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .cursor_pointer()
                            .child(Label::new(strings.cancel).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| this.set_closed_menu(None, cx))),
                    )
                    .child(
                        div()
                            .id("closed-lists-ok")
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(theme.colors().accent)
                            .cursor_pointer()
                            .child(Label::new(strings.ok).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(ClosedMenu::Lists(draft)) = this.closed_menu.take() {
                                    this.closed_lists = draft;
                                }
                                cx.notify();
                            })),
                    ),
            );

        h_flex()
            .h(px(30.))
            .flex_none()
            .px(px(24.))
            .gap_2()
            .child(
                div()
                    .relative()
                    .child(
                        filter_button(
                            "closed-dates",
                            self.closed_range.label(strings).into(),
                            matches!(open, Some(ClosedMenu::Dates | ClosedMenu::OtherDate { .. })),
                        )
                            .on_click(cx.listener(|this, _, _, cx| this.set_closed_menu(Some(ClosedMenu::Dates), cx))),
                    )
                    .when(open == Some(ClosedMenu::Dates), |el| {
                        el.child(PopoverAttachment::new(Placement::BelowLeft, dates_menu))
                    })
                    .when_some(other_menu, |el, menu| el.child(PopoverAttachment::new(Placement::BelowLeft, menu))),
            )
            .child(
                div()
                    .relative()
                    .child(
                        filter_button("closed-lists", lists_label, matches!(open, Some(ClosedMenu::Lists(_))))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let draft = this.closed_lists.clone();
                                this.set_closed_menu(Some(ClosedMenu::Lists(draft)), cx)
                            })),
                    )
                    .when(matches!(open, Some(ClosedMenu::Lists(_))), |el| {
                        el.child(PopoverAttachment::new(Placement::BelowLeft, lists_menu))
                    }),
            )
            .into_any_element()
    }

    fn render_header_menu(&self, strings: &Strings, cx: &mut Context<Self>) -> AnyElement {
        let in_list = matches!(self.view, View::List(_));
        let in_trash = self.view == View::Trash;
        popover(cx)
            .w(px(236.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.header_menu_open = false;
                cx.notify();
            }))
            .when(in_list, |el| {
                let current = self.current_list().and_then(|l| l.color.clone());
                el.child(
                    ListItem::new("rename-list")
                        .start_slot(Icon::new(IconName::Pencil))
                        .child(Label::new(strings.rename_list))
                        .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx))),
                )
                .child(div().px_2().pt_1().child(Label::new(strings.color).size(LabelSize::Small).color(Color::Muted)))
                .child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .flex_wrap()
                        .gap_1p5()
                        .child(
                            color_swatch("color-none", None, current.is_none(), cx)
                                .on_click(cx.listener(|this, _, _, cx| this.set_current_list_color(None, cx))),
                        )
                        .children(LIST_COLORS.into_iter().map(|color| {
                            color_swatch(color, Some(color), current.as_deref() == Some(color), cx).on_click(
                                cx.listener(move |this, _, _, cx| this.set_current_list_color(Some(color), cx)),
                            )
                        })),
                )
                .child(div().my_1().child(Divider::horizontal()))
                .child(
                    ListItem::new("delete-list")
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(Label::new(strings.delete_list).color(Color::Error))
                        .on_click(cx.listener(|this, _, _, cx| this.delete_current_list(cx))),
                )
            })
            .when_some(match self.view {
                View::Filter(id) => self.filters.iter().find(|(f, _)| f.id == id).map(|(f, _)| f.clone()),
                _ => None,
            }, |el, filter| {
                el.child(
                    ListItem::new("edit-filter")
                        .start_slot(Icon::new(IconName::Pencil))
                        .child(Label::new(strings.edit_filter))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_filter_edit(Some(filter.clone()), window, cx))),
                )
                .child(
                    ListItem::new("delete-filter")
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(Label::new(strings.delete_filter).color(Color::Error))
                        .on_click(cx.listener(|this, _, _, cx| this.delete_current_filter(cx))),
                )
                .child(div().my_1().child(Divider::horizontal()))
            })
            .when(in_trash, |el| {
                el.child(
                    ListItem::new("empty-trash")
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(Label::new(strings.empty_trash).color(Color::Error))
                        .on_click(cx.listener(|this, _, _, cx| this.empty_trash(cx))),
                )
            })
            .when(!in_trash && !matches!(self.view, View::Completed | View::WontDo | View::Search(_)), |el| {
                let completed = self.view_flag("show_completed", true);
                let details = self.view_flag("show_details", false);
                let kanban = self.kanban;
                let timeline = self.timeline && !kanban;
                let theme = cx.theme().clone();
                let layout = |id: &'static str, icon: IconName, on: bool| {
                    div()
                        .id(id)
                        .flex_1()
                        .h(px(30.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(on, |el| el.bg(theme.colors().element_selected))
                        .hover(|s| s.bg(theme.colors().element_hover))
                        .child(Icon::new(icon).color(if on { Color::Accent } else { Color::Muted }))
                };
                el.when(in_list, |el| el.child(div().my_1().child(Divider::horizontal())))
                    .child(div().px_2().pt_1().child(Label::new(strings.view_label).size(LabelSize::Small).color(Color::Muted)))
                    .child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_1()
                            .child(layout("layout-list", IconName::List, !kanban && !timeline).on_click(cx.listener(|this, _, _, cx| this.set_layout(false, false, cx))))
                            .child(layout("layout-kanban", IconName::Kanban, kanban).on_click(cx.listener(|this, _, _, cx| this.set_layout(true, false, cx))))
                            .child(layout("layout-timeline", IconName::ChartGantt, timeline).on_click(cx.listener(|this, _, _, cx| this.set_layout(false, true, cx)))),
                    )
                    .child(div().my_1().child(Divider::horizontal()))
                    .child(
                        ListItem::new("toggle-completed")
                            .start_slot(Icon::new(IconName::SquareCheck).color(Color::Muted))
                            .child(Label::new(if completed { strings.hide_completed } else { strings.show_completed }))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_view_flag("show_completed", true, cx))),
                    )
                    .child(
                        ListItem::new("toggle-details")
                            .start_slot(Icon::new(IconName::FileText).color(Color::Muted))
                            .child(Label::new(if details { strings.hide_details } else { strings.show_details }))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_view_flag("show_details", false, cx))),
                    )
                    .child(div().my_1().child(Divider::horizontal()))
                    .when(in_list, |el| {
                        el.child(
                            ListItem::new("add-section")
                                .start_slot(Icon::new(IconName::Plus).color(Color::Muted))
                                .child(Label::new(strings.add_section))
                                .on_click(cx.listener(|this, _, window, cx| this.start_new_section(window, cx))),
                        )
                    })
                    .when(in_list || self.view == View::Inbox, |el| {
                        el.child(
                            ListItem::new("list-activities")
                                .start_slot(Icon::new(IconName::ListTree).color(Color::Muted))
                                .child(Label::new(strings.list_activities))
                                .on_click(cx.listener(|this, _, _, cx| this.open_list_activity(cx))),
                        )
                    })
                    .child(
                        ListItem::new("print-list")
                            .start_slot(Icon::new(IconName::Printer).color(Color::Muted))
                            .child(Label::new(strings.print))
                            .on_click(cx.listener(|this, _, _, cx| this.print_list(cx))),
                    )
            })
            .when(!in_list && !in_trash && matches!(self.view, View::Completed | View::WontDo | View::Search(_)), |el| {
                el.child(div().px_2().py_1().child(Label::new(strings.no_actions).color(Color::Muted)))
            })
            .into_any_element()
    }
}

/// A round color option; `None` is "no color", drawn as an outlined circle.
fn color_swatch(id: &'static str, color: Option<&str>, selected: bool, cx: &App) -> Stateful<Div> {
    let colors = cx.theme().colors();
    let ring = if selected { colors.text } else { gpui::transparent_black() };
    div()
        .id(id)
        .size(px(18.))
        .rounded_full()
        .cursor_pointer()
        .map(|el| match lists_panel::parse_color(color) {
            Some(fill) => el.bg(fill).border_2().border_color(ring),
            None => el.border_1().border_color(if selected { colors.text } else { colors.text_muted }),
        })
}

fn chevron(collapsed: bool) -> Icon {
    Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall)
}

impl Focusable for TodoView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for TodoView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let input_focus = self.input.focus_handle(cx);
        let today = Self::today();
        let tag_colors: HashMap<String, Hsla> = self
            .tags
            .iter()
            .map(|t| (t.name.clone(), lists_panel::color(t.color.as_deref(), cx)))
            .collect();
        let timeline_view = self.timeline
            && !self.kanban
            && !matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_));
        let kanban_view = (self.kanban || timeline_view)
            && !matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_));
        let board: Option<AnyElement> = if timeline_view {
            Some(self.render_timeline(strings, cx))
        } else {
            None
        };
        let board: Option<AnyElement> = board.or_else(|| kanban_view.then(|| {
            let columns = task_rows::kanban_columns(self.rows_with(strings, &HashSet::new(), &HashSet::new()), strings);
            let theme = cx.theme().clone();
            h_flex()
                .id("kanban")
                .flex_1()
                .items_start()
                .gap_3()
                .px_4()
                .pb_4()
                .overflow_x_scroll()
                .children(columns.into_iter().enumerate().map(|(i, (title, drop, cards))| {
                    let drop_for_header = drop.clone();
                    let count = cards.len();
                    let accent = theme.colors().accent;
                    v_flex()
                        .id(SharedString::from(format!("kanban-column-{i}")))
                        .rounded(px(8.))
                        .when_some(drop, |el, drop| {
                            el.drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.08)))
                                .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                                    this.drop_on_column(dragged.id, drop.clone(), cx)
                                }))
                        })
                        .w(px(270.))
                        .flex_none()
                        .max_h_full()
                        .gap_2()
                        .child(self.render_group_header(
                            if title.is_empty() { strings.group_tasks.to_owned() } else { title },
                            count,
                            drop_for_header.as_ref(),
                            30.,
                            cx,
                        ))
                        .child(
                            v_flex()
                                .id(SharedString::from(format!("kanban-cards-{i}")))
                                .gap_2()
                                .overflow_y_scroll()
                                .children(cards.iter().map(|(task, subtasks)| {
                                    self.render_card(task, subtasks, &tag_colors, today, strings, cx)
                                })),
                        )
                        .when(count == 0, |el| {
                            el.child(div().h(px(40.)).rounded(px(8.)).border_1().border_color(theme.colors().border_variant))
                        })
                }))
                .when(matches!(self.view, View::List(_)), |el| {
                    el.child(
                        v_flex().w(px(220.)).flex_none().pt_1().map(|el| match self.new_section.clone() {
                            Some(input) => el.child(
                                h_flex().h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().surface_background).child(input),
                            ),
                            None => el.child(
                                h_flex()
                                    .id("kanban-new-section")
                                    .gap_1()
                                    .cursor_pointer()
                                    .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Accent))
                                    .child(Label::new(strings.add_section).color(Color::Accent))
                                    .on_click(cx.listener(|this, _, window, cx| this.start_new_section(window, cx))),
                            ),
                        }),
                    )
                })
                .into_any_element()
        }));
        let rows = if kanban_view { Vec::new() } else { self.rows(strings) };
        let rows: Vec<_> = rows
            .iter()
            .map(|row| match row {
                Row::Section { section, count, collapsed } => {
                    self.render_section(*section, *count, *collapsed, strings, cx)
                }
                Row::Day { label, count, drop } => {
                    div().pl(px(GUTTER)).child(self.render_group_header(label.clone(), *count, drop.as_ref(), 36., cx)).into_any_element()
                }
                Row::Task { task, depth, has_children, collapsed } => self.render_task(
                    task,
                    *depth,
                    *has_children,
                    *collapsed,
                    &tag_colors,
                    today,
                    strings,
                    cx,
                ),
            })
            .collect();

        let list = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .h(px(60.))
                    .flex_none()
                    .gap_2()
                    .pl(px(24.))
                    .pr(px(16.))
                    .child(
                        IconButton::new("toggle-sidebar", IconName::PanelLeft)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_lists(cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(20.))
                            .font_weight(FontWeight::BOLD)
                            .map(|el| {
                                if matches!(self.view, View::Search(_)) {
                                    el.child(
                                        h_flex()
                                            .gap_2()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::NORMAL)
                                            .child(Icon::new(IconName::Search).size(IconSize::Large))
                                            .child(self.search_input.clone()),
                                    )
                                } else if self.renaming_list {
                                    el.child(self.rename_input.clone())
                                } else {
                                    el.truncate().child(self.view_title(strings))
                                }
                            }),
                    )
                    .when(self.view == View::Trash, |el| {
                        el.child(
                            IconButton::new("empty-trash-button", IconName::BrushCleaning)
                                .on_click(cx.listener(|this, _, _, cx| this.empty_trash(cx))),
                        )
                    })
                    .when(self.view != View::Trash, |el| {
                        el.child(
                            div()
                                .relative()
                                .child(IconButton::new("sort", IconName::ArrowDownUp).on_click(cx.listener(|this, _, _, cx| {
                                    this.sort_menu = Some(None);
                                    cx.notify();
                                })))
                                .when_some(self.sort_menu, |el, expanded| {
                                    el.child(PopoverAttachment::new(Placement::BelowRight, self.render_sort_menu(expanded, strings, cx)))
                                }),
                        )
                    })
                    .when(self.view != View::Trash, |el| el.child(
                        div()
                            .relative()
                            .child(IconButton::new("more", IconName::Ellipsis).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.header_menu_open = true;
                                    cx.notify();
                                },
                            )))
                            .when(self.header_menu_open, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    self.render_header_menu(strings, cx),
                                ))
                            }),
                    )),
            )
            .when(matches!(self.view, View::Completed | View::WontDo), |el| {
                el.child(self.render_closed_filters(strings, cx))
            })
            .when(!self.is_read_only_view() && self.preferences.detailed_input, |el| el.child(self.render_detailed_add(strings, cx)))
            .when(!self.is_read_only_view() && !self.preferences.detailed_input, |el| el.child(
                h_flex()
                    .mx_2()
                    .mb_2()
                    .h(px(38.))
                    .flex_none()
                    .gap_2()
                    .px_3()
                    .rounded(px(6.))
                    .bg(theme.colors().surface_background)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&input_focus, cx);
                    })
                    .child(Icon::new(IconName::Plus))
                    .child(div().flex_1().min_w_0().child(self.input.clone()))
                    .children(self.shown_add_due().map(|(date, time)| {
                        let (label, _) = task_rows::due_label(date, time, Self::today(), strings);
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .child(Icon::new(IconName::CalendarDays).size(IconSize::Small).color(Color::Accent))
                            .child(Label::new(label).size(LabelSize::Small).color(Color::Accent))
                    }))
                    .child(
                        div()
                            .relative()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                h_flex().child(self.add_date_button(Placement::BelowRight, strings, cx)).child(
                                // The original's "▾": priority, list, template and note for the
                                // next task; its flag takes the chosen priority's color.
                                IconButton::new("add-options", IconName::ChevronDown)
                                    .size(px(24.))
                                    .icon_size(IconSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.add_menu_open = !this.add_menu_open;
                                        this.add_list_side = false;
                                        cx.notify();
                                    })),
                            ))
                            .map(|el| self.add_field_popovers(el, strings, cx)),
                    ),
            ))
            .children(board)
            .when(!kanban_view, |el| el.child(
                div()
                    .id("tasks")
                    .track_focus(&self.list_focus)
                    .key_context("TaskList")
                    .on_action(cx.listener(Self::select_next))
                    .on_action(cx.listener(Self::select_previous))
                    .on_action(cx.listener(Self::toggle_selected))
                    .on_action(cx.listener(Self::delete_selected))
                    .on_action(cx.listener(Self::pin_selected))
                    .on_action(cx.listener(Self::date_selected))
                    .on_action(cx.listener(|this, _: &ClearDate, _, cx| this.set_selected_due(None, cx)))
                    .on_action(cx.listener(|this, _: &SetToday, _, cx| this.set_selected_due(Some(0), cx)))
                    .on_action(cx.listener(|this, _: &SetTomorrow, _, cx| this.set_selected_due(Some(1), cx)))
                    .on_action(cx.listener(|this, _: &SetNextWeek, _, cx| this.set_selected_due(Some(7), cx)))
                    .on_action(cx.listener(|this, _: &PriorityNone, _, cx| this.set_selected_priority(Priority::None, cx)))
                    .on_action(cx.listener(|this, _: &PriorityLow, _, cx| this.set_selected_priority(Priority::Low, cx)))
                    .on_action(cx.listener(|this, _: &PriorityMedium, _, cx| this.set_selected_priority(Priority::Medium, cx)))
                    .on_action(cx.listener(|this, _: &PriorityHigh, _, cx| this.set_selected_priority(Priority::High, cx)))
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_4()
                    // A free day in the mini calendar, as in the original.
                    .when(rows.is_empty() && matches!(self.view, View::Day(_)), |el| {
                        el.child(
                            v_flex()
                                .pt(px(160.))
                                .items_center()
                                .gap_1()
                                .child(Icon::new(IconName::CalendarCheck).size(IconSize::Medium).color(Color::Muted))
                                .child(Label::new(strings.free_day))
                                .child(Label::new(strings.free_day_hint).size(LabelSize::Small).color(Color::Muted)),
                        )
                    })
                    .when(rows.is_empty() && !matches!(self.view, View::Day(_)), |el| {
                        el.child(div().px_3().pt_8().child(
                            Label::new(match &self.view {
                                View::Completed => strings.no_completed_tasks,
                                View::WontDo => strings.no_wont_do,
                                View::Trash => strings.trash_is_empty,
                                View::Search(query) if query.trim().is_empty() => strings.search_hint,
                                View::Search(_) => strings.nothing_found,
                                _ => strings.no_tasks,
                            })
                            .color(Color::Muted),
                        ))
                    })
                    .when_some(self.new_section.clone().filter(|_| !kanban_view), |el, input| {
                        el.child(
                            h_flex()
                                .h(px(32.))
                                .mx(px(GUTTER))
                                .px_2()
                                .rounded(px(6.))
                                .bg(cx.theme().colors().surface_background)
                                .child(input),
                        )
                    })
                    .children(rows)
                    .children(self.render_habits(strings, cx)),
            ))
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .px_6()
                        .py_2()
                        .border_t_1()
                        .border_color(theme.colors().border)
                        .child(Label::new(error).color(Color::Error)),
                )
            });

        let this = cx.entity().downgrade();
        let on_select = move |view, _: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.set_view(view, cx)).ok();
        };
        let on_add_list = cx.listener(|this, _, window, cx| this.start_new_list(window, cx));
        let this = cx.entity().downgrade();
        let on_drop_task = move |view, id, _: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.drop_on_view(view, id, cx)).ok();
        };

        h_flex()
            .relative()
            .size_full()
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .text_size(px(14.))
            .when(self.show_lists, |el| {
                el.child(lists_panel::render(
                    lists_panel::Props {
                        filters: &self.filters,
                        preferences: &self.preferences,
                        active: &self.view,
                        counts: &self.counts,
                        lists: &self.lists,
                        tags: &self.tags,
                        strings,
                        new_list_input: self.creating_list.then(|| self.new_list.clone().into_any_element()),
                        mini: lists_panel::MiniCalendar { expanded: self.mini_expanded, month: self.mini_month, busy: &self.busy_days },
                    },
                    on_select,
                    on_add_list,
                    on_drop_task,
                    cx.listener(|this, _, window, cx| this.open_filter_edit(None, window, cx)),
                    {
                        let this = cx.entity().downgrade();
                        move |action, _: &mut Window, cx: &mut App| {
                            this.update(cx, |this, cx| this.mini_action(action, cx)).ok();
                        }
                    },
                    cx,
                ))
            })
            .map(|el| {
                if self.view == View::Summary {
                    return el.child(div().flex_1().min_w_0().h_full().child(self.summary.clone()));
                }
                // As in the original, the Timeline takes the whole width until a task is opened.
                if timeline_view && self.selected.is_none() && self.list_activity.is_none() {
                    return el.child(list);
                }
                el.child(list).child(div().flex_none().w(px(DETAIL_WIDTH)).h_full().map(|el| match self.list_activity.clone() {
                    Some(entries) => el.child(self.render_list_activity(&entries, cx)),
                    None => el.child(self.detail.clone()),
                }))
            })
            .when_some(self.filter_edit.as_ref().map(|edit| self.render_filter_edit(edit, cx)), |el, dialog| el.child(dialog))
            .when_some(self.undo.as_ref().map(|(_, label, _)| label.clone()), |el, label| {
                el.child(
                    div().absolute().bottom(px(24.)).left_0().right_0().flex().justify_center().child(
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
                    ),
                )
            })
    }
}
