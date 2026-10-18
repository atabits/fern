use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use fern_core::{Id, Kind, List, ListFolder, Priority, Store, Tag, Task, View, ViewCounts};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone as _};
use gpui::{ElementId, Entity, FocusHandle, Focusable, FontWeight, MouseButton, Pixels, Point};
use ui::prelude::*;

use crate::drag::DraggedTask;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use crate::actions::{
    ClearDate, DeleteTask, PinTask, PriorityHigh, PriorityLow, PriorityMedium, PriorityNone, SelectNext, SelectPrevious,
    SetDate, SetNextWeek, SetToday, SetTomorrow, ToggleComplete,
};
use crate::i18n::{self, CurrentLocale, Strings};
use crate::list_dialog::{DialogEvent, FolderDialog, ListDialog};
use crate::tag_dialog::{MergeDialog, TagDialog, TagEvent};
use crate::lists_panel;
use crate::task_detail::{OpenTask, StartFocus, TaskDetail, TaskUpdated};
use crate::task_menu;
use crate::task_rows::{self, ClosedRange, DropTarget, GroupBy, Row, Section, SortBy};
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

/// The share of a task row, from the left, where a dropped task goes before it; the rest of the
/// row nests it.
const NEST_ZONE: f32 = 0.6;
const DETAIL_WIDTH: f32 = 400.;
const ROW_HEIGHT: f32 = 40.;
/// Below this width the window is narrow, for View Options' Task Detail Layout.
const NARROW_WINDOW: f32 = 1200.;
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
    Order,
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
    /// The "Advanced" tab: a chain of conditions with AND and OR between them.
    advanced: bool,
    name: Entity<TextInput>,
    keywords: Entity<TextInput>,
    /// The chain condition whose menu is open (field, NOT, remove).
    chain_menu: Option<usize>,
    /// "Preview": the tasks the filter would show, in place of the conditions.
    preview: bool,
    /// The Advanced tab's "Examples" menu, while open.
    examples_open: bool,
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
    /// The Add List / Edit List dialog and the folder dialog, while open.
    list_dialog: Option<Entity<ListDialog>>,
    folder_dialog: Option<Entity<FolderDialog>>,
    folders: Vec<ListFolder>,
    collapsed_folders: HashSet<Id>,
    archived_open: bool,
    /// A right-clicked list, folder or tag in the lists column, with the point its menu opens at.
    list_menu: Option<(ListMenuTarget, Point<Pixels>)>,
    /// The tag dialogs, folded parent tags and whether the Tags section is folded.
    tag_dialog: Option<Entity<TagDialog>>,
    merge_dialog: Option<Entity<MergeDialog>>,
    collapsed_tags: HashSet<String>,
    tags_collapsed: bool,
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
    /// The Timeline's scale: 0 Day, 1 Week, 2 Month; and whether its menu is open.
    timeline_scale: usize,
    scale_menu_open: bool,
    /// The Timeline's "Arrange Tasks" panel: the undated tasks, to drag onto days.
    arrange_open: bool,
    /// The current list's sections.
    sections: Vec<fern_core::ListSection>,
    /// The "Add Section" field, while it is shown.
    new_section: Option<Entity<TextInput>>,
    /// Where the new section goes among the list's sections (the column menu's Add Section to
    /// Left / Right); `None` puts it last.
    new_section_index: Option<usize>,
    /// The kanban: a column's "+" field (its index and value), its "…" menu (the section and
    /// whether Move to's side menu is open), and a section being renamed.
    column_add: Option<(usize, DropTarget, Entity<TextInput>)>,
    column_menu: Option<(Id, bool)>,
    /// The Timeline's groups folded up, by name.
    timeline_folded: HashSet<String>,
    /// Today's "Suggested Tasks" panel (the header's light bulb), and whether its Long Unfinished
    /// list is shown in full.
    suggestions_open: Option<bool>,
    /// The header menu's View Options dialog and its Show Date by dropdown, while open.
    view_options: Option<bool>,
    section_rename: Option<(Id, Entity<TextInput>)>,
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
    /// A task row's right-click menu: the task and where the click was (window coordinates).
    context_menu: Option<(Id, gpui::Point<Pixels>)>,
    /// Its side menu open: 0 Move to, 1 Start Focus.
    context_side: Option<u8>,
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
            list_dialog: None,
            folder_dialog: None,
            folders: Vec::new(),
            collapsed_folders: HashSet::new(),
            archived_open: false,
            list_menu: None,
            tag_dialog: None,
            merge_dialog: None,
            collapsed_tags: HashSet::new(),
            tags_collapsed: false,
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
            timeline_scale: 0,
            scale_menu_open: false,
            arrange_open: false,
            sections: Vec::new(),
            new_section: None,
            new_section_index: None,
            column_add: None,
            column_menu: None,
            timeline_folded: HashSet::new(),
            view_options: None,
            suggestions_open: None,
            section_rename: None,
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
            context_menu: None,
            context_side: None,
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
                Some(("list", name)) => {
                    let result = self.store.create_list(name);
                    if let Some(list) = self.check(result) {
                        self.view = View::List(list.id);
                    }
                    self.reload(cx);
                }
                Some(("view", "trash")) => self.set_view(View::Trash, cx),
                Some(("view", "wontdo")) => self.set_view(View::WontDo, cx),
                Some(("view", "today")) => self.set_view(View::Today, cx),
                Some(("view", "summary")) => self.set_view(View::Summary, cx),
                Some(("view", "next7")) => self.set_view(View::Next7Days, cx),
                Some(("view", "completed")) => self.set_view(View::Completed, cx),
                Some(("view", "tomorrow")) => self.set_view(View::Tomorrow, cx),
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
                    if let Some(list) = self.section_owner() {
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
                Some(("column-menu", name)) => {
                    self.column_menu = self.sections.iter().find(|s| s.name == name).map(|s| (s.id, false));
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
                Some(("menu", "kanban")) => self.set_layout(true, false, cx),
                Some(("menu", "view-options")) => self.view_options = Some(false),
                Some(("menu", "suggestions")) => self.suggestions_open = Some(false),
                Some(("menu", "view-options-date")) => self.view_options = Some(true),
                Some(("show-date", "countdown")) => self.save_preferences(|p| p.countdown_dates = true, cx),
                Some(("menu", "arrange")) => {
                    self.set_layout(false, true, cx);
                    self.arrange_open = true;
                }
                Some(("menu", "timeline-week")) => {
                    self.set_layout(false, true, cx);
                    self.timeline_scale = 1;
                }
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
                // `context=<title part>`: that task's right-click menu, opened beside its row.
                Some(("context", needle)) => {
                    if let Some(task) = self.tasks.iter().find(|t| t.title.contains(needle)).cloned() {
                        self.select(task.id, cx);
                        self.context_menu = Some((task.id, gpui::point(px(420.), px(200.))));
                    }
                }
                // `filter-new=normal|advanced`, `filter-new=chain` (an Advanced chain with a
                // condition's menu open), `filter-new=preview` or `filter-new=examples`.
                Some(("filter-new", mode)) => {
                    self.open_filter_edit(None, window, cx);
                    if mode != "normal" {
                        self.set_filter_advanced(true, cx);
                    }
                    if let Some(edit) = self.filter_edit.as_mut() {
                        edit.preview = mode == "preview";
                        edit.examples_open = mode == "examples";
                    }
                    if mode == "chain" {
                        if let Some(edit) = self.filter_edit.as_mut() {
                            edit.draft.chain = fern_core::filter::chain_from_storage("lists and:tags! or:date");
                            edit.chain_menu = Some(1);
                        }
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
                Some(("menu", "sort-root")) => self.sort_menu = Some(None),
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
                Some(("new-list", _)) => self.start_new_list(window, cx),
                // `detail-bar=format|format-headings|comment|comment-emoji|comment:<text>`.
                Some(("detail-bar", what)) => self.detail.update(cx, |detail, cx| detail.dev_bar(what, window, cx)),
                // `list-dialog=new|<list name>[>icon|folder|type|smart|new-folder|kanban|timeline]`.
                Some(("list-dialog", spec)) => {
                    let (which, menu) = spec.split_once('>').unwrap_or((spec, ""));
                    let list = self.lists.iter().find(|l| l.name == which).map(|l| l.id);
                    self.open_list_dialog(list, None, window, cx);
                    if let Some(dialog) = &self.list_dialog {
                        dialog.update(cx, |dialog, cx| dialog.dev_open(menu, window, cx));
                    }
                }
                // `folder=<name>><list>,<list>`: a folder with these lists in it (dev data only).
                Some(("folder", spec)) => {
                    let (name, lists) = spec.split_once('>').unwrap_or((spec, ""));
                    if let Ok(folder) = self.store.add_list_folder(name) {
                        for list in self.lists.clone().iter().filter(|l| lists.split(',').any(|n| n == l.name)) {
                            let draft = fern_core::ListDraft { folder: Some(folder.id), ..fern_core::ListDraft::of(list) };
                            let result = self.store.update_list(list.id, &draft);
                            self.check(result);
                        }
                    }
                    self.reload(cx);
                }
                // `list-icon=<list>><emoji>`, `archive-list=<list>`: dev data only.
                Some(("list-icon", spec)) => {
                    let (name, icon) = spec.split_once('>').unwrap_or((spec, ""));
                    if let Some(list) = self.lists.iter().find(|l| l.name == name).cloned() {
                        let result = self.store.update_list(list.id, &fern_core::ListDraft { icon: Some(icon.to_owned()), ..fern_core::ListDraft::of(&list) });
                        self.check(result);
                    }
                    self.reload(cx);
                }
                Some(("archive-list", name)) => {
                    if let Some(id) = self.lists.iter().find(|l| l.name == name).map(|l| l.id) {
                        let result = self.store.set_list_archived(id, true);
                        self.check(result);
                        self.archived_open = true;
                    }
                    self.reload(cx);
                }
                // `tag-dialog=new|<tag>[>parent]`, `merge-dialog=<tag>[>open]`, `tag-menu=<tag>`,
                // `tags-menu`, `sub-tag=<tag>><parent>` (dev data only).
                Some(("tag-dialog", spec)) => {
                    let (which, menu) = spec.split_once('>').unwrap_or((spec, ""));
                    let tag = (which != "new").then(|| which.to_owned());
                    self.open_tag_dialog(tag, None, window, cx);
                    if menu == "parent"
                        && let Some(dialog) = &self.tag_dialog
                    {
                        dialog.update(cx, |dialog, cx| dialog.dev_open_parent(cx));
                    }
                }
                Some(("merge-dialog", spec)) => {
                    let (tag, menu) = spec.split_once('>').unwrap_or((spec, ""));
                    self.open_merge_dialog(tag.to_owned(), window, cx);
                    if menu == "open"
                        && let Some(dialog) = &self.merge_dialog
                    {
                        dialog.update(cx, |dialog, cx| dialog.dev_open(cx));
                    }
                }
                Some(("tag-menu", tag)) => self.list_menu = Some((ListMenuTarget::Tag(tag.to_owned()), gpui::point(px(150.), px(420.)))),
                Some(("tags-menu", _)) => self.list_menu = Some((ListMenuTarget::TagsHeading, gpui::point(px(170.), px(330.)))),
                Some(("sub-tag", spec)) => {
                    let (tag, parent) = spec.split_once('>').unwrap_or((spec, ""));
                    let result = self.store.update_tag(tag, tag, None, Some(parent));
                    self.check(result);
                    self.reload(cx);
                }
                // `list-menu=<list>` and `folder-menu=<folder>` open their right-click menus.
                Some(("list-menu", name)) => {
                    if let Some(id) = self.lists.iter().find(|l| l.name == name).map(|l| l.id) {
                        self.list_menu = Some((ListMenuTarget::List(id), gpui::point(px(180.), px(260.))));
                    }
                }
                Some(("folder-menu", name)) => {
                    if let Some(id) = self.folders.iter().find(|f| f.name == name).map(|f| f.id) {
                        self.list_menu = Some((ListMenuTarget::Folder(id), gpui::point(px(180.), px(260.))));
                    }
                }
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
        self.timeline_scale = self
            .store
            .setting(&format!("timeline_scale:{}", self.view_key()))
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .filter(|s| *s < 3)
            .unwrap_or(0);
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
        self.sections = match self.section_owner() {
            Some(list) => self.store.list_sections(list).unwrap_or_default(),
            None => Vec::new(),
        };
        let today = Self::today();
        if let Some(lists) = self.check(self.store.lists()) {
            self.lists = lists;
        }
        self.folders = self.store.list_folders().unwrap_or_default();
        self.collapsed_folders = self
            .store
            .setting("collapsed_folders")
            .ok()
            .flatten()
            .unwrap_or_default()
            .split(',')
            .filter_map(|id| Id::parse_str(id).ok())
            .collect();
        self.collapsed_tags = self.store.setting("collapsed_tags").ok().flatten().unwrap_or_default().lines().map(str::to_owned).collect();
        self.tags_collapsed = self.store.setting("tags_collapsed").ok().flatten().is_some_and(|v| v == "1");
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
        let lists = self.active_lists();
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
        // In a note list (the original's List Type), new entries are notes.
        let note_list = self.store.task(task.id)?.and_then(|t| t.list_id).is_some_and(|id| self.lists.iter().any(|l| l.id == id && l.kind == fern_core::Kind::Note));
        if self.add_draft.note || note_list {
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
        // In the smart lists, as the original: 'Add task to "Inbox"', naming where new tasks go.
        let smart = matches!(self.view, View::All | View::Today | View::Tomorrow | View::Next7Days);
        let placeholder: SharedString = if self.preferences.detailed_input {
            strings.input_box_placeholder.into()
        } else if smart {
            let list = self.preferences.default_list.and_then(|id| self.lists.iter().find(|l| l.id == id)).map_or(strings.inbox.to_owned(), |l| l.name.clone());
            strings.add_task_to.replace("{list}", &list).into()
        } else {
            strings.add_task.into()
        };
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
            .mx(px(20.))
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
            .chain(self.lists.iter().filter(|l| !l.archived).map(|l| (Some(l.id), l.name.clone())))
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
                task_rows::order_tasks(&mut tasks, sort, self.view_flag("descending", false));
                task_rows::pinned_first(&mut tasks);
                match group {
                    // As the original's Next 7 Days: a group for each day.
                    GroupBy::Custom if self.view == View::Next7Days => {
                        task_rows::next_days_rows(&tasks, Self::today(), crate::calendar::week_start(), strings)
                    }
                    GroupBy::Custom if !self.sections.is_empty() => {
                        task_rows::section_rows(&tasks, &self.sections, collapsed_tasks, strings)
                    }
                    GroupBy::Custom => {
                        task_rows::build_rows(&tasks, collapsed_tasks, collapsed_sections, true)
                    }
                    group => {
                        let lists: Vec<(Id, String)> = self.lists.iter().map(|l| (l.id, l.name.clone())).collect();
                        task_rows::grouped_rows_in(&tasks, group, Self::today(), self.preferences.task_defaults.overdue_last, &lists, strings)
                    }
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
                // A note shows the note icon, as the original's cards, not a box.
                .map(|el| if task.kind == Kind::Note {
                    el.child(Icon::new(IconName::FileText).size(IconSize::Small).color(Color::Muted))
                } else {
                    el.child(
                        Checkbox::new(id)
                            .toggle_state(done)
                            .tint(task_rows::priority_color(task.priority, &theme))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle(task_id, !done, cx);
                            })),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(14.))
                        .text_color(if done { theme.colors().text_muted } else { theme.colors().text })
                        .child(if task.title.is_empty() { strings.untitled.to_owned() } else { task.title.clone() }),
                )
        };
        let id = task.id;
        let due = task.due_date.map(|date| task_rows::row_due_label(date, task.due_time, today, strings, self.preferences.countdown_dates));
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
        let advanced = draft.match_any || !draft.chain.is_empty();
        self.filter_edit = Some(FilterEdit { draft, is_new, advanced, name, keywords, chain_menu: None, preview: false, examples_open: false });
        cx.notify();
    }

    fn update_filter_draft(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut fern_core::SmartFilter)) {
        if let Some(edit) = self.filter_edit.as_mut() {
            change(&mut edit.draft);
        }
        cx.notify();
    }

    /// The dialog's Normal / Advanced tabs. Advanced starts its chain from the set conditions,
    /// joined as "all" or "any" were; Normal drops the chain.
    fn set_filter_advanced(&mut self, advanced: bool, cx: &mut Context<Self>) {
        let Some(edit) = self.filter_edit.as_mut() else { return };
        edit.advanced = advanced;
        edit.chain_menu = None;
        let draft = &mut edit.draft;
        if !advanced {
            draft.match_any = false;
            draft.chain.clear();
        } else if draft.chain.is_empty() {
            use fern_core::{Field, Join, Link};
            let set = [
                (Field::Lists, !draft.lists.is_empty()),
                (Field::Tags, !draft.tags.is_empty()),
                (Field::Date, draft.date != fern_core::DateRule::All),
                (Field::Priority, !draft.priorities.is_empty()),
                (Field::Include, !draft.keywords.trim().is_empty()),
                (Field::Type, draft.kind.is_some()),
            ];
            let join = if draft.match_any { Join::Or } else { Join::And };
            draft.chain = set.into_iter().filter(|(_, on)| *on).map(|(field, _)| Link::new(join, field)).collect();
            if draft.chain.is_empty() {
                draft.chain = vec![Link::new(Join::And, Field::Lists)];
            }
        }
        cx.notify();
    }

    fn update_chain(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Vec<fern_core::Link>, &mut Option<usize>)) {
        if let Some(edit) = self.filter_edit.as_mut() {
            change(&mut edit.draft.chain, &mut edit.chain_menu);
        }
        cx.notify();
    }

    /// The Advanced chain, as the original lays it out: condition, Logic, condition … and "+".
    /// A condition's chip opens its menu: the field, NOT and remove; Logic switches AND and OR.
    fn render_filter_chain(&self, edit: &FilterEdit, cx: &mut Context<Self>) -> AnyElement {
        use fern_core::{Field, Join, Link};
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let field_name = |field: Field| match field {
            Field::Lists => strings.filter_lists,
            Field::Tags => strings.tags,
            Field::Date => strings.filter_date,
            Field::Priority => strings.filter_priority,
            Field::Include => strings.filter_include,
            Field::Type => strings.filter_type,
        };
        let cell = |id: SharedString, dashed: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px_3()
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .when(dashed, |el| el.border_dashed())
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
        };
        let mut row = h_flex().flex_wrap().gap_2();
        for (i, link) in edit.draft.chain.iter().copied().enumerate() {
            if i > 0 {
                row = row.child(
                    cell(SharedString::from(format!("filter-logic-{i}")), true)
                        .child(Label::new(strings.filter_logic[(link.join == Join::Or) as usize]).size(LabelSize::Small).color(Color::Accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_chain(cx, |chain, _| {
                                chain[i].join = if chain[i].join == Join::And { Join::Or } else { Join::And };
                            })
                        })),
                );
            }
            let label = if link.not { format!("{} {}", strings.filter_not, field_name(link.field)) } else { field_name(link.field).to_owned() };
            let menu = (edit.chain_menu == Some(i)).then(|| {
                PopoverAttachment::new(
                    Placement::BelowLeft,
                    popover(cx)
                        .w(px(180.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update_chain(cx, |_, menu| *menu = None)))
                        .children(Field::ALL.into_iter().map(|field| {
                            let selected = field == link.field;
                            ListItem::new(SharedString::from(format!("filter-chain-{i}-{}", field.key())))
                                .child(Label::new(field_name(field)).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }))
                                .when(selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_chain(cx, |chain, menu| {
                                        chain[i].field = field;
                                        *menu = None;
                                    })
                                }))
                        }))
                        .child(div().h(px(1.)).my_1().bg(theme.colors().border_variant))
                        .child(
                            ListItem::new(SharedString::from(format!("filter-chain-{i}-not")))
                                .child(Label::new(strings.filter_not).size(LabelSize::Small))
                                .when(link.not, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                                .on_click(cx.listener(move |this, _, _, cx| this.update_chain(cx, |chain, _| chain[i].not = !chain[i].not))),
                        )
                        .when(edit.draft.chain.len() > 1, |el| {
                            el.child(
                                ListItem::new(SharedString::from(format!("filter-chain-{i}-remove")))
                                    .start_slot(Icon::new(IconName::Trash).size(IconSize::Small).color(Color::Error))
                                    .child(Label::new(strings.delete).size(LabelSize::Small).color(Color::Error))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_chain(cx, |chain, menu| {
                                            chain.remove(i);
                                            *menu = None;
                                        })
                                    })),
                            )
                        })
                        .into_any_element(),
                )
            });
            row = row.child(
                div()
                    .relative()
                    .child(
                        cell(SharedString::from(format!("filter-cond-{i}")), false)
                            .child(Label::new(label).size(LabelSize::Small))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_chain(cx, |_, menu| *menu = if *menu == Some(i) { None } else { Some(i) })
                            })),
                    )
                    .children(menu),
            );
        }
        row.child(
            cell("filter-chain-add".into(), false)
                .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Muted))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.update_chain(cx, |chain, _| {
                        // The next condition not in the chain yet, or Lists again.
                        let field = Field::ALL.into_iter().find(|f| !chain.iter().any(|l| l.field == *f)).unwrap_or(Field::Lists);
                        chain.push(Link::new(Join::And, field));
                    })
                })),
        )
        .into_any_element()
    }

    fn set_filter_preview(&mut self, preview: bool, cx: &mut Context<Self>) {
        if let Some(edit) = self.filter_edit.as_mut() {
            edit.preview = preview;
            edit.examples_open = false;
            edit.chain_menu = None;
        }
        cx.notify();
    }

    /// "Examples" in the Advanced tab: ready-made chains, applied with a click (the name stays).
    fn render_filter_examples(&self, open: bool, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let menu = open.then(|| {
            PopoverAttachment::new(
                Placement::AboveRight,
                popover(cx)
                    .w(px(260.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if let Some(edit) = this.filter_edit.as_mut() {
                            edit.examples_open = false;
                        }
                        cx.notify();
                    }))
                    .children(fern_core::filter::examples().into_iter().enumerate().map(|(i, example)| {
                        ListItem::new(("filter-example", i))
                            .child(Label::new(strings.filter_example_names[i]).size(LabelSize::Small))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(edit) = this.filter_edit.as_mut() {
                                    let (id, name) = (edit.draft.id, edit.draft.name.clone());
                                    edit.draft = fern_core::SmartFilter { id, name, ..example.clone() };
                                    edit.examples_open = false;
                                    let keywords = edit.keywords.clone();
                                    let text = edit.draft.keywords.clone();
                                    keywords.update(cx, |input, cx| input.set_text(text, cx));
                                }
                                cx.notify();
                            }))
                    }))
                    .into_any_element(),
            )
        });
        div()
            .relative()
            .child(
                div()
                    .id("filter-examples")
                    .px_2()
                    .cursor_pointer()
                    .child(Label::new(strings.filter_examples).size(LabelSize::Small).color(Color::Muted))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(edit) = this.filter_edit.as_mut() {
                            edit.examples_open = !edit.examples_open;
                        }
                        cx.notify();
                    })),
            )
            .children(menu)
            .into_any_element()
    }

    /// The original's "Preview": the open tasks the filter would show, by list with a count,
    /// then Cancel (back to the conditions) or Save.
    fn render_filter_preview(&self, edit: &FilterEdit, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Self::today();
        let mut draft = edit.draft.clone();
        draft.keywords = edit.keywords.read(cx).text().to_owned();
        let tasks = self.store.tasks().unwrap_or_default();
        let shown = draft.preview(&tasks, today);
        let lists: Vec<(Option<Id>, SharedString)> = std::iter::once((None, SharedString::new_static(strings.inbox)))
            .chain(self.lists.iter().map(|l| (Some(l.id), l.name.clone().into())))
            .collect();
        let mut body = v_flex().id("filter-preview-tasks").max_h(px(420.)).overflow_y_scroll().gap_0p5();
        if shown.is_empty() {
            body = body.child(div().py_4().child(Label::new(strings.filter_preview_empty).color(Color::Muted)));
        }
        for (list, name) in lists {
            let in_list: Vec<&&Task> = shown.iter().filter(|t| t.list_id == list).collect();
            if in_list.is_empty() {
                continue;
            }
            body = body.child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    .child(Label::new(name).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(in_list.len().to_string()).size(LabelSize::XSmall).color(Color::Muted)),
            );
            for task in in_list {
                let due = task.due_date.map(|date| crate::task_rows::due_label(date, task.due_time, today, strings));
                body = body.child(
                    h_flex()
                        .h(px(34.))
                        .pl_4()
                        .gap_2()
                        .border_b_1()
                        .border_color(theme.colors().border_variant)
                        .child(
                            Icon::new(if task.kind == Kind::Note { IconName::FileText } else { IconName::Square })
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .child(div().flex_1().min_w_0().child(Label::new(task.title.clone()).size(LabelSize::Small).truncate()))
                        .children(task.tags.iter().map(|tag| Label::new(tag.clone()).size(LabelSize::XSmall).color(Color::Muted)))
                        .children(due.map(|(label, overdue)| {
                            Label::new(label).size(LabelSize::XSmall).color(if overdue { Color::Error } else { Color::Accent })
                        })),
                );
            }
        }
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
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.35))
            .child(
                v_flex()
                    .id("filter-preview-dialog")
                    .w(px(560.))
                    .p_4()
                    .gap_2()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.colors().border)
                    .bg(theme.colors().elevated_surface_background)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(h_flex().justify_center().child(Label::new(strings.filter_preview).weight(FontWeight::SEMIBOLD)))
                    .child(body)
                    .child(
                        h_flex()
                            .gap_2()
                            .pt_2()
                            .justify_end()
                            .child(button("filter-preview-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| this.set_filter_preview(false, cx))))
                            .child(button("filter-preview-save", strings.save, true).on_click(cx.listener(|this, _, _, cx| this.save_filter(cx)))),
                    ),
            )
            .into_any_element()
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
        if edit.preview {
            return self.render_filter_preview(edit, cx);
        }
        let chain = edit.advanced.then(|| self.render_filter_chain(edit, cx));
        let examples = edit.advanced.then(|| self.render_filter_examples(edit.examples_open, cx));
        let shows = |field: fern_core::Field| !edit.advanced || edit.draft.chain.iter().any(|l| l.field == field);
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
                            .on_click(cx.listener(move |this, _, _, cx| this.set_filter_advanced(advanced, cx)))
                    })))
                    .child(field(edit.name.clone()))
                    .children(chain)
                    // In the Advanced tab only the chain's conditions are set.
                    .when(shows(fern_core::Field::Lists), |el| el.child(row(strings.filter_lists, lists.into_any_element())))
                    .when(shows(fern_core::Field::Tags), |el| el.child(row(strings.tags, tags.into_any_element())))
                    .when(shows(fern_core::Field::Date), |el| el.child(row(strings.filter_date, dates.into_any_element())))
                    .when(shows(fern_core::Field::Priority), |el| el.child(row(strings.filter_priority, priorities.into_any_element())))
                    .when(shows(fern_core::Field::Include), |el| el.child(row(strings.filter_include, field(edit.keywords.clone()).into_any_element())))
                    .when(shows(fern_core::Field::Type), |el| el.child(row(strings.filter_type, kinds.into_any_element())))
                    .child(
                        h_flex()
                            .gap_2()
                            .pt_2()
                            .child(
                                div()
                                    .id("filter-preview")
                                    .cursor_pointer()
                                    .child(Label::new(strings.filter_preview).size(LabelSize::Small).color(Color::Accent))
                                    .on_click(cx.listener(|this, _, _, cx| this.set_filter_preview(true, cx))),
                            )
                            .child(div().flex_1())
                            .children(examples)
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

    /// Whose sections the view shows: a list's, the Inbox's (`Some(None)`), or none.
    fn section_owner(&self) -> Option<Option<Id>> {
        match self.view {
            View::List(id) => Some(Some(id)),
            View::Inbox => Some(None),
            _ => None,
        }
    }

    fn start_new_section(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.section_name, cx));
        cx.subscribe(&input, |this, _, Submitted(name), cx| {
            if let Some(list) = this.section_owner() {
                let result = this.store.insert_list_section(list, name, this.new_section_index.take());
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

    /// The original's Suggested Tasks: Recently Added and Long Unfinished, each with "+" to plan
    /// the task for today; four of the long unfinished ones until "More".
    fn render_suggestions(&self, all: bool, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let today = Self::today();
        let found = fern_core::suggest::suggestions(&self.store.tasks().unwrap_or_default(), today);
        let row = |task: &Task, note: String, cx: &mut Context<Self>| {
            let id = task.id;
            h_flex()
                .id(SharedString::from(format!("suggestion-{id}")))
                .py_1p5()
                .gap_2()
                .items_start()
                .border_b_1()
                .border_color(theme.colors().border_variant)
                .child(div().pt(px(3.)).child(Checkbox::new(SharedString::from(format!("suggestion-box-{id}"))).size(px(14.))))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(Label::new(task.title.clone()).truncate())
                        .child(Label::new(note).size(LabelSize::XSmall).color(Color::Muted)),
                )
                .child(
                    IconButton::new(SharedString::from(format!("suggestion-add-{id}")), IconName::Plus)
                        .icon_color(Color::Accent)
                        .tooltip(Tooltip::text(strings.add_to_today))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let result = this.store.set_due(id, Some(Self::today()), None);
                            this.check(result);
                            this.reload(cx);
                        })),
                )
                .into_any_element()
        };
        let recent: Vec<AnyElement> = found
            .recent
            .iter()
            .map(|task| {
                let when = fern_core::suggest::created_day(task)
                    .zip(chrono::DateTime::from_timestamp_millis(task.created_at))
                    .map(|(day, at)| task_rows::due_label(day, Some(at.with_timezone(&Local).time()), today, strings).0)
                    .unwrap_or_default();
                row(task, strings.added_on.replace("{when}", &when), cx)
            })
            .collect();
        let shown = if all { found.long_unfinished.len() } else { found.long_unfinished.len().min(4) };
        let long: Vec<AnyElement> = found.long_unfinished[..shown]
            .iter()
            .map(|task| {
                let days = fern_core::suggest::created_day(task).map_or(0, |d| (today - d).num_days());
                row(task, strings.created_days_ago.replace("{n}", &days.to_string()), cx)
            })
            .collect();
        let more = !all && found.long_unfinished.len() > 4;
        let heading = |text: &'static str| Label::new(text).size(LabelSize::Small).weight(FontWeight::SEMIBOLD);
        div()
            .absolute()
            .right(px(12.))
            .bottom(px(12.))
            .child(
                v_flex()
                    .id("suggested-tasks")
                    .w(px(370.))
                    .max_h(px(560.))
                    .overflow_y_scroll()
                    .p_4()
                    .gap_2()
                    .rounded(px(10.))
                    .bg(theme.colors().elevated_surface_background)
                    .border_1()
                    .border_color(theme.colors().border)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .child(div().flex_1().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(strings.suggested_tasks))
                            .child(IconButton::new("suggestions-x", IconName::X).icon_color(Color::Muted).on_click(cx.listener(|this, _, _, cx| {
                                this.suggestions_open = None;
                                cx.notify();
                            }))),
                    )
                    .when(recent.is_empty() && long.is_empty(), |el| el.child(Label::new(strings.no_suggestions).color(Color::Muted)))
                    .when(!recent.is_empty(), |el| el.child(heading(strings.recently_added)).children(recent))
                    .when(!long.is_empty(), |el| {
                        el.child(
                            h_flex()
                                .pt_2()
                                .child(div().flex_1().child(heading(strings.long_unfinished)))
                                .when(more, |el| {
                                    el.child(
                                        h_flex()
                                            .id("suggestions-more")
                                            .cursor_pointer()
                                            .child(Label::new(strings.more).size(LabelSize::Small).color(Color::Muted))
                                            .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Muted))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.suggestions_open = Some(true);
                                                cx.notify();
                                            })),
                                    )
                                }),
                        )
                        .children(long)
                    }),
            )
            .into_any_element()
    }

    fn save_preferences(&mut self, change: impl FnOnce(&mut fern_core::Preferences), cx: &mut Context<Self>) {
        change(&mut self.preferences);
        let result = self.store.save_preferences(&self.preferences);
        self.check(result);
        cx.notify();
    }

    /// The original's View Options: Show Date by (Task Time or Countdown) and the Task Detail
    /// Layout for a narrow window (Side peek or Pop-up), each drawn as the original's sketches.
    fn render_view_options(&self, menu_open: bool, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let countdown = self.preferences.countdown_dates;
        let popup = self.preferences.popup_details;
        let choice = |id: &'static str, label: &'static str, on: bool| {
            ListItem::new(id)
                .height(px(34.))
                .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
                .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
        };
        let menu = menu_open.then(|| {
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .w(px(196.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.view_options = Some(false);
                        cx.notify();
                    }))
                    .child(choice("show-date-time", strings.task_time, !countdown).on_click(cx.listener(|this, _, _, cx| {
                        this.view_options = Some(false);
                        this.save_preferences(|p| p.countdown_dates = false, cx);
                    })))
                    .child(choice("show-date-countdown", strings.date_countdown, countdown).on_click(cx.listener(|this, _, _, cx| {
                        this.view_options = Some(false);
                        this.save_preferences(|p| p.countdown_dates = true, cx);
                    }))),
            )
        });
        // A small sketch of the window: the details beside the list, or over it.
        let sketch = |over: bool| {
            let line = |w: f32| div().w(px(w)).h(px(3.)).rounded(px(1.)).bg(theme.colors().text_muted.opacity(0.3));
            div()
                .relative()
                .w(px(140.))
                .h(px(92.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .bg(theme.colors().background)
                .child(div().absolute().left(px(6.)).top(px(8.)).child(v_flex().gap_1().children([40., 30., 36., 28.].map(line))))
                .child(if over {
                    div()
                        .absolute()
                        .left(px(40.))
                        .top(px(18.))
                        .w(px(80.))
                        .h(px(58.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.colors().border)
                        .bg(theme.colors().elevated_surface_background)
                        .p_1p5()
                        .child(v_flex().gap_1().children([50., 60., 40.].map(line)))
                } else {
                    div()
                        .absolute()
                        .right_0()
                        .top_0()
                        .w(px(64.))
                        .h_full()
                        .border_l_1()
                        .border_color(theme.colors().border)
                        .p_1p5()
                        .child(v_flex().gap_1().children([40., 50., 30.].map(line)))
                })
        };
        let layout = |id: &'static str, label: &'static str, over: bool, cx: &mut Context<Self>| {
            let on = popup == over;
            v_flex()
                .id(id)
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(sketch(over))
                .child(
                    h_flex()
                        .gap_1p5()
                        .child(
                            div()
                                .size(px(16.))
                                .rounded_full()
                                .border_1()
                                .border_color(if on { theme.colors().accent } else { theme.colors().border })
                                .when(on, |el| el.bg(theme.colors().accent).flex().items_center().justify_center().child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Custom(gpui::white())))),
                        )
                        .child(Label::new(label)),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.save_preferences(|p| p.popup_details = over, cx)))
        };
        let section = || v_flex().p_4().gap_1().rounded(px(8.)).bg(theme.colors().surface_background);
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.35))
            .occlude()
            .child(
                v_flex()
                    .id("view-options")
                    .w(px(440.))
                    .p_5()
                    .gap_3()
                    .rounded(px(10.))
                    .bg(theme.colors().elevated_surface_background)
                    .border_1()
                    .border_color(theme.colors().border)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .child(div().flex_1().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(strings.view_options))
                            .child(IconButton::new("view-options-x", IconName::X).icon_color(Color::Muted).on_click(cx.listener(|this, _, _, cx| {
                                this.view_options = None;
                                cx.notify();
                            }))),
                    )
                    .child(
                        h_flex()
                            .h(px(48.))
                            .px_4()
                            .rounded(px(8.))
                            .bg(theme.colors().surface_background)
                            .child(div().flex_1().child(Label::new(strings.show_date_by)))
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        h_flex()
                                            .id("show-date-by")
                                            .gap_1()
                                            .cursor_pointer()
                                            .child(Label::new(if countdown { strings.date_countdown } else { strings.task_time }).color(Color::Muted))
                                            .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.view_options = Some(!this.view_options.unwrap_or(false));
                                                cx.notify();
                                            })),
                                    )
                                    .children(menu),
                            ),
                    )
                    .child(
                        section()
                            .child(Label::new(strings.task_detail_layout))
                            .child(Label::new(strings.task_detail_layout_hint).size(LabelSize::Small).color(Color::Muted))
                            .child(
                                h_flex()
                                    .pt_3()
                                    .justify_around()
                                    .child(layout("layout-side", strings.side_peek, false, cx))
                                    .child(layout("layout-popup", strings.pop_up, true, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// A kanban column's "+": a field at the column's top; the task it adds takes the column's
    /// value, as one dropped there.
    fn start_column_add(&mut self, column: usize, target: DropTarget, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_task, cx));
        cx.subscribe_in(&input, window, move |this, input, Submitted(text), window, cx| {
            if !text.trim().is_empty() {
                let result = this.add_task(text);
                if let Some(task) = this.check(result)
                    && let Some((_, target, _)) = this.column_add.clone()
                {
                    this.drop_on_column(task.id, target, cx);
                }
                input.update(cx, |input, cx| input.clear(window, cx));
                this.reload(cx);
            }
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.column_add = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.column_add = Some((column, target, input));
        cx.notify();
    }

    /// A column's header, as the original's: the name and count, then "+" and, for a section,
    /// "…" with Rename, Add Section to Left / Right, Move to › and Delete.
    fn render_column_header(
        &self,
        column: usize,
        title: String,
        count: usize,
        target: Option<DropTarget>,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let section = match target {
            Some(DropTarget::Section(Some(id))) => Some(id),
            _ => None,
        };
        let renaming = self.section_rename.as_ref().filter(|(id, _)| Some(*id) == section).map(|(_, input)| input.clone());
        let menu_open = self.column_menu.is_some_and(|(id, _)| Some(id) == section);
        h_flex()
            .h(px(30.))
            .gap_1()
            .map(|el| match renaming {
                Some(input) => el.child(div().flex_1().child(input)),
                None => el
                    .child(Label::new(title).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted))
                    .child(div().flex_1()),
            })
            .when_some(target.filter(|t| !matches!(t, DropTarget::Complete)), |el, target| {
                el.child(
                    IconButton::new(SharedString::from(format!("kanban-add-{column}")), IconName::Plus)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text(strings.add_task))
                        .on_click(cx.listener(move |this, _, window, cx| this.start_column_add(column, target.clone(), window, cx))),
                )
            })
            .when_some(section, |el, id| {
                el.child(
                    div()
                        .relative()
                        .child(
                            IconButton::new(SharedString::from(format!("kanban-menu-{column}")), IconName::Ellipsis)
                                .icon_color(Color::Muted)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.column_menu = if this.column_menu.is_some_and(|(open, _)| open == id) { None } else { Some((id, false)) };
                                    cx.notify();
                                })),
                        )
                        .when(menu_open, |el| el.child(PopoverAttachment::new(Placement::BelowRight, self.render_column_menu(id, column, strings, cx)))),
                )
            })
            .into_any_element()
    }

    fn render_column_menu(&self, section: Id, column: usize, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let item = |key: &'static str, icon: IconName, label: &'static str| {
            ListItem::new(key).height(px(34.)).start_slot(Icon::new(icon).color(Color::Muted)).child(Label::new(label))
        };
        // The section's place among the list's sections, for Add Section to Left / Right.
        let index = self.sections.iter().position(|s| s.id == section).unwrap_or(column);
        let side = self.column_menu.is_some_and(|(_, side)| side);
        // Move to: the Inbox and the lists, but not where the section already is.
        let owner = self.section_owner();
        let lists: Vec<ListItem> = std::iter::once((None, SharedString::new_static(strings.inbox), IconName::Inbox))
            .chain(self.active_lists().into_iter().map(|l| (Some(l.id), SharedString::from(l.name), IconName::List)))
            .filter(|(to, _, _)| owner != Some(*to))
            .map(|(to, name, icon)| {
                ListItem::new(SharedString::from(format!("section-move-{to:?}")))
                    .start_slot(Icon::new(icon).color(Color::Muted))
                    .child(Label::new(name))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.column_menu = None;
                        let result = this.store.move_list_section(section, to);
                        this.check(result);
                        this.reload(cx);
                    }))
            })
            .collect();
        let view = cx.entity().downgrade();
        popover(cx)
            .w(px(220.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.column_menu = None;
                cx.notify();
            }))
            .child(item("section-rename", IconName::Pencil, strings.rename_section).on_click(cx.listener(move |this, _, window, cx| {
                this.column_menu = None;
                let strings = i18n::t(cx);
                let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.section_name, cx));
                let name = this.sections.iter().find(|s| s.id == section).map(|s| s.name.clone()).unwrap_or_default();
                input.update(cx, |input, cx| input.set_text(name, cx));
                cx.subscribe(&input, move |this, _, Submitted(name), cx| {
                    let result = this.store.rename_list_section(section, name);
                    this.check(result);
                    this.section_rename = None;
                    this.reload(cx);
                })
                .detach();
                cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
                    this.section_rename = None;
                    cx.notify();
                })
                .detach();
                window.focus(&input.focus_handle(cx), cx);
                this.section_rename = Some((section, input));
                cx.notify();
            })))
            .child(item("section-left", IconName::BetweenVerticalStart, strings.add_section_left).on_click(cx.listener(move |this, _, window, cx| {
                this.column_menu = None;
                this.start_new_section(window, cx);
                this.new_section_index = Some(index);
            })))
            .child(item("section-right", IconName::BetweenVerticalEnd, strings.add_section_right).on_click(cx.listener(move |this, _, window, cx| {
                this.column_menu = None;
                this.start_new_section(window, cx);
                this.new_section_index = Some(index + 1);
            })))
            .child(
                SubmenuItem::new("section-move", strings.move_to)
                    .icon(IconName::Inbox)
                    .height(px(34.))
                    .open(side)
                    .on_hover(move |hovered: &bool, _, cx: &mut App| {
                        if *hovered && !side {
                            view.update(cx, |this, cx| {
                                this.column_menu = Some((section, true));
                                cx.notify();
                            })
                            .ok();
                        }
                    })
                    .items(lists),
            )
            .child(item("section-delete", IconName::Trash, strings.delete).on_click(cx.listener(move |this, _, _, cx| {
                this.column_menu = None;
                let result = this.store.delete_list_section(section);
                this.check(result);
                this.reload(cx);
            })))
            .into_any_element()
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
                DropTarget::List(list) => store.set_list(id, list),
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

    /// The Timeline's scale button, "Day ▾", and its menu: Day, Week, Month.
    fn render_scale_button(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let labels = [strings.day, strings.week, strings.month];
        div()
            .relative()
            .ml_2()
            .child(
                h_flex()
                    .id("timeline-scale")
                    .px_2()
                    .h(px(22.))
                    .gap_1()
                    .rounded(px(11.))
                    .border_1()
                    .border_color(theme.colors().border)
                    .cursor_pointer()
                    .child(Label::new(labels[self.timeline_scale]).size(LabelSize::Small))
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.scale_menu_open = !this.scale_menu_open;
                        cx.notify();
                    })),
            )
            .when(self.scale_menu_open, |el| {
                el.child(PopoverAttachment::new(
                    Placement::BelowRight,
                    popover(cx)
                        .w(px(150.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.scale_menu_open = false;
                            cx.notify();
                        }))
                        .children(labels.into_iter().enumerate().map(|(i, label)| {
                            let selected = i == self.timeline_scale;
                            ListItem::new(SharedString::from(format!("timeline-scale-{i}")))
                                .child(Label::new(label).color(if selected { Color::Accent } else { Color::Default }))
                                .when(selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.timeline_scale = i;
                                    this.scale_menu_open = false;
                                    let result = this.store.set_setting(&format!("timeline_scale:{}", this.view_key()), &i.to_string());
                                    this.check(result);
                                    cx.notify();
                                }))
                        }))
                        .into_any_element(),
                ))
            })
            .into_any_element()
    }

    /// The original's Timeline: the groups as rows, the coming days as columns (weekends shaded),
    /// and each dated task as a bar from its date to the end of its duration; a click opens it.
    fn render_timeline(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        // The original's scales: Day shows ten days, Week three weeks, Month six; the arrows move
        // by a week, two weeks or a month.
        let scale = self.timeline_scale;
        let days_shown = [10usize, 22, 45][scale];
        let step = [7i64, 14, 30][scale];
        const LABEL_WIDTH: f32 = 108.;
        let theme = cx.theme().clone();
        let today = Self::today();
        let from = self.timeline_from;
        let today_line = theme.status().error;
        let days: Vec<NaiveDate> = (0..days_shown as i64).map(|i| from + chrono::Duration::days(i)).collect();
        let groups = task_rows::kanban_columns(self.rows_with(strings, &HashSet::new(), &HashSet::new()), strings);
        let scale_button = self.render_scale_button(strings, cx);
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
                    .when(weekend(&date), |el| el.bg(theme.colors().element_hover.opacity(0.5)))
                    // Week and Month mark today with a red line, as the original does.
                    .when(scale > 0 && date == today, |el| el.border_l_2().border_color(today_line))
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
                    .child(div().w(px(LABEL_WIDTH - 12.)).flex_none().child(Label::new(strings.months_short[from.month0() as usize]).size(LabelSize::Small).color(Color::Muted)))
                    // A month's name above its first day, for the scales that cross months.
                    .child(h_flex().flex_1().mr(px(8.)).children(days.iter().map(|date| {
                        div().flex_1().when(scale > 0 && date.day() == 1 && *date != from, |el| {
                            el.child(Label::new(strings.months_short[date.month0() as usize]).size(LabelSize::Small).color(Color::Muted))
                        })
                    })))
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
                    )
                    .child(scale_button),
            )
            .child(
                h_flex()
                    .h(px(34.))
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(
                        h_flex()
                            .w(px(LABEL_WIDTH))
                            .flex_none()
                            .px_1()
                            .child(IconButton::new("timeline-prev", IconName::ChevronLeft).size(px(20.)).icon_size(IconSize::XSmall).on_click(shift(-step))),
                    )
                    .child(h_flex().flex_1().children(days.iter().map(|date| {
                        let is_today = *date == today;
                        // Month shows every seventh day's number, so that they fit.
                        let labelled = scale < 2 || (date.day() - 1) % 7 == 0;
                        // As in the original: the numbers centered over their days, semibold, and
                        // the weekend's muted.
                        let color = match (is_today, scale) {
                            (true, 0) => Color::Accent,
                            (true, _) => Color::Custom(today_line),
                            _ if weekend(date) => Color::Muted,
                            _ => Color::Default,
                        };
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(weekend(date), |el| el.bg(theme.colors().element_hover.opacity(0.5)))
                            .when(labelled || is_today, |el| el.child(Label::new(date.day().to_string()).weight(FontWeight::SEMIBOLD).color(color)))
                    })))
                    .child(IconButton::new("timeline-next", IconName::ChevronRight).size(px(20.)).icon_size(IconSize::XSmall).on_click(shift(step))),
            );
        let rows = groups.into_iter().enumerate().map(|(i, (label, _, cards))| {
            let bars: Vec<(Task, usize, usize)> = cards
                .into_iter()
                .flat_map(|(task, subtasks)| std::iter::once(task).chain(subtasks))
                .filter_map(|task| fern_core::timeline::bar(&task, from, days_shown).map(|(first, span)| (task, first, span)))
                .collect();
            // A group folds up with the arrow before its name, as in the original.
            let folded = self.timeline_folded.contains(&label);
            let bars = if folded { Vec::new() } else { bars };
            let height = px(if folded { 40. } else { 20. + 40. * bars.len().max(1) as f32 });
            let fold_label = label.clone();
            h_flex()
                .h(height)
                .border_b_1()
                .border_color(theme.colors().border_variant)
                .child(
                    h_flex()
                        .id(SharedString::from(format!("timeline-group-{i}")))
                        .w(px(LABEL_WIDTH))
                        .flex_none()
                        .h_full()
                        .items_start()
                        .gap_1()
                        .pl_2()
                        .pt(px(14.))
                        .cursor_pointer()
                        .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall).color(Color::Muted))
                        .child(Label::new(label).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.timeline_folded.remove(&fold_label) {
                                this.timeline_folded.insert(fold_label.clone());
                            }
                            cx.notify();
                        })),
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
                                .top(px(8. + 40. * n as f32))
                                .left(gpui::relative(first as f32 / days_shown as f32))
                                .w(gpui::relative(span as f32 / days_shown as f32))
                                .h(px(32.))
                                .px_0p5()
                                .child(
                                    div()
                                        .size_full()
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded(px(8.))
                                        .bg(tint.opacity(if task.is_completed() { 0.3 } else { 0.75 }))
                                        .cursor_pointer()
                                        .child(Label::new(task.title.clone()).truncate()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                        })),
                )
        }).collect::<Vec<_>>();
        // Under the groups the weekends stay shaded down to the bottom, as in the original.
        let filler = h_flex()
            .flex_1()
            .min_h(px(200.))
            .child(div().w(px(LABEL_WIDTH)).flex_none())
            .child(div().flex_1().h_full().mr(px(20.)).relative().overflow_hidden().child(backdrop(usize::MAX, px(4000.), cx)));
        let timeline = v_flex().id("timeline").flex_1().min_w_0().h_full().overflow_y_scroll().child(header).children(rows).child(filler);
        h_flex()
            .flex_1()
            .min_h_0()
            .items_start()
            .child(timeline)
            .when(self.arrange_open, |el| el.child(self.render_arrange(strings, cx)))
            .into_any_element()
    }

    /// The original's "Arrange Tasks": the list's undated open tasks by group, as bars in their
    /// first tag's color, to drag onto a day of the Timeline.
    fn render_arrange(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let groups = task_rows::kanban_columns(self.rows_with(strings, &HashSet::new(), &HashSet::new()), strings);
        let tag_color = |task: &Task| {
            task.tags
                .first()
                .and_then(|name| self.tags.iter().find(|t| t.name.to_lowercase() == name.to_lowercase()))
                .and_then(|t| lists_panel::parse_color(t.color.as_deref()))
                .unwrap_or(theme.colors().accent)
        };
        let bar = |task: Task, depth: usize, cx: &mut Context<Self>| {
            let id = task.id;
            let color = tag_color(&task);
            let dragged = DraggedTask { id, title: task.title.clone().into() };
            div()
                .id(SharedString::from(format!("arrange-{id}")))
                .ml(px(depth as f32 * 14.))
                .h(px(22.))
                .px_1p5()
                .flex()
                .items_center()
                .rounded(px(3.))
                .bg(color.opacity(0.7))
                .cursor_grab()
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                .gap_1()
                .when(task.kind == Kind::Note, |el| el.child(Icon::new(IconName::FileText).size(IconSize::XSmall)))
                .child(Label::new(task.title).size(LabelSize::XSmall).truncate())
                .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                .into_any_element()
        };
        let mut items: Vec<AnyElement> = Vec::new();
        for (label, _, cards) in groups {
            let undated: Vec<(Task, Vec<Task>)> = cards
                .into_iter()
                .filter(|(task, _)| !task.is_closed())
                .map(|(task, subtasks)| (task, subtasks.into_iter().filter(|t| t.due_date.is_none() && !t.is_closed()).collect::<Vec<_>>()))
                .filter(|(task, subtasks)| task.due_date.is_none() || !subtasks.is_empty())
                .collect();
            let count: usize = undated.iter().map(|(t, s)| usize::from(t.due_date.is_none()) + s.len()).sum();
            if count == 0 {
                continue;
            }
            items.push(
                h_flex()
                    .pt_2()
                    .pb_1()
                    .gap_1()
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    .child(Label::new(label).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(count.to_string()).size(LabelSize::XSmall).color(Color::Muted))
                    .into_any_element(),
            );
            for (task, subtasks) in undated {
                if task.due_date.is_none() {
                    items.push(bar(task, 0, cx));
                }
                for subtask in subtasks {
                    items.push(bar(subtask, 1, cx));
                }
            }
        }
        v_flex()
            .id("arrange-tasks")
            .w(px(220.))
            .flex_none()
            .h_full()
            .px_2()
            .pt_2()
            .gap_1()
            .border_l_1()
            .border_color(theme.colors().border_variant)
            .overflow_y_scroll()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.arrange_tasks).weight(FontWeight::SEMIBOLD)))
                    .child(IconButton::new("arrange-close", IconName::X).size(px(22.)).icon_size(IconSize::Small).on_click(cx.listener(|this, _, _, cx| {
                        this.arrange_open = false;
                        cx.notify();
                    }))),
            )
            .children(items)
            .into_any_element()
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
        let descending = self.view_flag("descending", false);
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
            // The original's Order: Ascending or Descending, kept per list; with the manual
            // (Custom) sort, as there, no Order.
            .when(sort != SortBy::Custom, |el| el.child(row("order-by", IconName::ArrowDownUp, strings.order, strings.order_directions[descending as usize], SortMenu::Order)))
            .when(sort != SortBy::Custom && expanded == Some(SortMenu::Order), |el| {
                el.children([false, true].map(|value| {
                    option(format!("order-{value}"), strings.order_directions[value as usize], value == descending).on_click(cx.listener(move |this, _, _, cx| {
                        if this.view_flag("descending", false) != value {
                            this.toggle_view_flag("descending", false, cx);
                        }
                        this.sort_menu = None;
                        cx.notify();
                    }))
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
        self.show_lists = true;
        self.open_list_dialog(None, None, window, cx);
    }

    /// Opens the original's Add List dialog (in a folder when given), or Edit List for `list`.
    fn open_list_dialog(&mut self, list: Option<Id>, folder: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        self.list_menu = None;
        self.header_menu_open = false;
        let dialog = cx.new(|cx| ListDialog::new(self.store.clone(), list, folder, window, cx));
        cx.subscribe_in(&dialog, window, |this, _, event: &DialogEvent, window, cx| {
            if let DialogEvent::Saved(id) = event {
                this.view = View::List(*id);
            }
            this.list_dialog = None;
            window.focus(&this.list_focus, cx);
            this.reload(cx);
        })
        .detach();
        self.list_dialog = Some(dialog);
        cx.notify();
    }

    /// The New Folder dialog, or Edit Folder for `folder`.
    fn open_folder_dialog(&mut self, folder: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        self.list_menu = None;
        let dialog = cx.new(|cx| FolderDialog::new(self.store.clone(), folder, window, cx));
        cx.subscribe_in(&dialog, window, |this, _, _: &DialogEvent, window, cx| {
            this.folder_dialog = None;
            window.focus(&this.list_focus, cx);
            this.reload(cx);
        })
        .detach();
        self.folder_dialog = Some(dialog);
        cx.notify();
    }

    /// The lists not archived, for choosing a list.
    fn active_lists(&self) -> Vec<List> {
        self.lists.iter().filter(|l| !l.archived).cloned().collect()
    }

    /// The original's Add Tags (under `parent` for Add Sub-tag), or Edit Tag for `tag`.
    fn open_tag_dialog(&mut self, tag: Option<String>, parent: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.list_menu = None;
        let editing = tag.clone();
        let dialog = cx.new(|cx| TagDialog::new(self.store.clone(), tag, parent, window, cx));
        cx.subscribe_in(&dialog, window, move |this, _, event: &TagEvent, window, cx| {
            if let TagEvent::Saved(name) = event
                && editing.as_ref().is_some_and(|old| this.view == View::Tag(old.clone()))
            {
                this.view = View::Tag(name.clone());
            }
            this.tag_dialog = None;
            window.focus(&this.list_focus, cx);
            this.reload(cx);
        })
        .detach();
        self.tag_dialog = Some(dialog);
        cx.notify();
    }

    fn open_merge_dialog(&mut self, tag: String, window: &mut Window, cx: &mut Context<Self>) {
        self.list_menu = None;
        let from = tag.clone();
        let dialog = cx.new(|_| MergeDialog::new(self.store.clone(), tag));
        cx.subscribe_in(&dialog, window, move |this, _, event: &TagEvent, window, cx| {
            if let TagEvent::Saved(into) = event
                && this.view == View::Tag(from.clone())
            {
                this.view = View::Tag(into.clone());
            }
            this.merge_dialog = None;
            window.focus(&this.list_focus, cx);
            this.reload(cx);
        })
        .detach();
        self.merge_dialog = Some(dialog);
        cx.notify();
    }

    fn panel_event(&mut self, event: lists_panel::PanelEvent, window: &mut Window, cx: &mut Context<Self>) {
        use lists_panel::PanelEvent;
        match event {
            PanelEvent::ToggleFolder(id) => {
                if !self.collapsed_folders.remove(&id) {
                    self.collapsed_folders.insert(id);
                }
                let value: Vec<String> = self.collapsed_folders.iter().map(Id::to_string).collect();
                let result = self.store.set_setting("collapsed_folders", &value.join(","));
                self.check(result);
            }
            PanelEvent::ToggleArchived => self.archived_open = !self.archived_open,
            PanelEvent::AddTag => return self.open_tag_dialog(None, None, window, cx),
            PanelEvent::TagsMenu(at) => self.list_menu = Some((ListMenuTarget::TagsHeading, at)),
            PanelEvent::ToggleTags => {
                self.tags_collapsed = !self.tags_collapsed;
                let result = self.store.set_setting("tags_collapsed", if self.tags_collapsed { "1" } else { "0" });
                self.check(result);
            }
            PanelEvent::ToggleTag(tag) => {
                if !self.collapsed_tags.remove(&tag) {
                    self.collapsed_tags.insert(tag);
                }
                let value: Vec<&str> = self.collapsed_tags.iter().map(String::as_str).collect();
                let result = self.store.set_setting("collapsed_tags", &value.join("\n"));
                self.check(result);
            }
            PanelEvent::TagMenu(tag, at) => self.list_menu = Some((ListMenuTarget::Tag(tag), at)),
            PanelEvent::ListMenu(id, at) => self.list_menu = Some((ListMenuTarget::List(id), at)),
            PanelEvent::FolderMenu(id, at) => self.list_menu = Some((ListMenuTarget::Folder(id), at)),
            PanelEvent::DropList { list, after, folder } => {
                let result = self.store.place_list(list, after, folder);
                self.check(result);
            }
            PanelEvent::DropListInFolder { list, folder } => {
                // After the folder's last list; an empty folder takes it where it is.
                let order: Vec<&List> = self.lists.iter().filter(|l| l.id != list).collect();
                let after = order.iter().rev().find(|l| l.folder == Some(folder)).map(|l| l.id).or_else(|| {
                    let at = self.lists.iter().position(|l| l.id == list)?;
                    self.lists[..at].last().map(|l| l.id)
                });
                let result = self.store.place_list(list, after, Some(folder));
                self.check(result);
            }
        }
        self.reload(cx);
    }

    fn list_action(&mut self, action: ListAction, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.list_menu.take().map(|(t, _)| t);
        let current = match self.view {
            View::List(id) => Some(id),
            _ => None,
        };
        let list = match target {
            Some(ListMenuTarget::List(id)) => Some(id),
            _ => current,
        };
        match (action, target) {
            (ListAction::Edit, Some(ListMenuTarget::Tag(tag))) => return self.open_tag_dialog(Some(tag), None, window, cx),
            (ListAction::AddSubtag, Some(ListMenuTarget::Tag(tag))) => return self.open_tag_dialog(None, Some(tag), window, cx),
            (ListAction::Merge, Some(ListMenuTarget::Tag(tag))) => return self.open_merge_dialog(tag, window, cx),
            (ListAction::Pin, Some(ListMenuTarget::Tag(tag))) => {
                let pinned = self.tags.iter().any(|t| t.name == tag && t.pinned);
                let result = self.store.set_tag_pinned(&tag, !pinned);
                self.check(result);
            }
            (ListAction::Delete, Some(ListMenuTarget::Tag(tag))) => {
                let result = self.store.delete_tag(&tag);
                self.check(result);
                if self.view == View::Tag(tag) {
                    self.view = View::Inbox;
                }
            }
            (ListAction::ShowTags(visibility), _) => {
                self.preferences.set_visibility(fern_core::SmartList::Tags, visibility);
                let result = self.store.save_preferences(&self.preferences);
                self.check(result);
            }
            (ListAction::Edit, Some(ListMenuTarget::Folder(folder))) => return self.open_folder_dialog(Some(folder), window, cx),
            (ListAction::AddList, Some(ListMenuTarget::Folder(folder))) => return self.open_list_dialog(None, Some(folder), window, cx),
            (ListAction::Ungroup, Some(ListMenuTarget::Folder(folder))) => {
                let result = self.store.ungroup_list_folder(folder);
                self.check(result);
            }
            (ListAction::Edit, _) => return self.open_list_dialog(list, None, window, cx),
            (ListAction::Duplicate, _) => {
                if let Some(id) = list {
                    let result = self.store.duplicate_list(id, i18n::t(cx).list_copy_suffix);
                    if let Some(copy) = self.check(result) {
                        self.view = View::List(copy.id);
                    }
                }
            }
            (ListAction::Archive, _) => {
                if let Some(list) = list.and_then(|id| self.lists.iter().find(|l| l.id == id)) {
                    let (id, archived) = (list.id, !list.archived);
                    let result = self.store.set_list_archived(id, archived);
                    self.check(result);
                    self.archived_open |= archived;
                }
            }
            (ListAction::Delete, _) => {
                if let Some(id) = list {
                    let result = self.store.delete_list(id);
                    self.check(result);
                    if current == Some(id) {
                        self.view = View::Inbox;
                    }
                }
            }
            _ => {}
        }
        self.header_menu_open = false;
        self.reload(cx);
    }

    /// The menu of a list (Edit, Duplicate, Archive or Unarchive, Delete) or of a folder (Edit,
    /// Add List, Ungroup), from the list header's "…" or a right click in the lists column.
    fn render_list_items(&self, target: ListMenuTarget, strings: &'static Strings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let item = |id: &'static str, icon: IconName, label: &'static str, action: ListAction, cx: &mut Context<Self>| {
            ListItem::new(id)
                .height(px(34.))
                .start_slot(Icon::new(icon).color(if action == ListAction::Delete { Color::Error } else { Color::Muted }))
                .child(Label::new(label).color(if action == ListAction::Delete { Color::Error } else { Color::Default }))
                .on_click(cx.listener(move |this, _, window, cx| this.list_action(action, window, cx)))
                .into_any_element()
        };
        match target {
            ListMenuTarget::List(id) => {
                let archived = self.lists.iter().any(|l| l.id == id && l.archived);
                vec![
                    item("list-menu-edit", IconName::Pencil, strings.edit, ListAction::Edit, cx),
                    item("list-menu-duplicate", IconName::Copy, strings.duplicate, ListAction::Duplicate, cx),
                    item(
                        "list-menu-archive",
                        if archived { IconName::ArchiveRestore } else { IconName::Archive },
                        if archived { strings.unarchive_list } else { strings.archive_list },
                        ListAction::Archive,
                        cx,
                    ),
                    div().my_1().child(Divider::horizontal()).into_any_element(),
                    item("list-menu-delete", IconName::Trash, strings.delete, ListAction::Delete, cx),
                ]
            }
            ListMenuTarget::Tag(tag) => {
                // As in the original: words only. "Move to Shared Tags" needs shared lists.
                let tag = self.tags.iter().find(|t| t.name == tag);
                let pinned = tag.is_some_and(|t| t.pinned);
                let top = tag.is_some_and(|t| t.parent.is_none());
                let text = |id: &'static str, label: &'static str, action: ListAction, cx: &mut Context<Self>| {
                    ListItem::new(id)
                        .height(px(34.))
                        .child(Label::new(label))
                        .on_click(cx.listener(move |this, _, window, cx| this.list_action(action, window, cx)))
                        .into_any_element()
                };
                let mut items = vec![
                    text("tag-menu-edit", strings.edit, ListAction::Edit, cx),
                    text("tag-menu-pin", if pinned { strings.unpin } else { strings.pin }, ListAction::Pin, cx),
                ];
                if top {
                    items.push(text("tag-menu-subtag", strings.add_subtag, ListAction::AddSubtag, cx));
                }
                items.push(text("tag-menu-merge", strings.merge_tags, ListAction::Merge, cx));
                items.push(text("tag-menu-delete", strings.delete, ListAction::Delete, cx));
                items
            }
            ListMenuTarget::TagsHeading => {
                use fern_core::Visibility;
                let current = self.preferences.visibility(fern_core::SmartList::Tags);
                [(Visibility::IfNotEmpty, strings.visibility_names[1]), (Visibility::Show, strings.visibility_names[0]), (Visibility::Hide, strings.visibility_names[2])]
                    .into_iter()
                    .map(|(visibility, label)| {
                        let on = current == visibility;
                        ListItem::new(SharedString::from(format!("tags-show-{label}")))
                            .height(px(34.))
                            .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
                            .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                            .on_click(cx.listener(move |this, _, window, cx| this.list_action(ListAction::ShowTags(visibility), window, cx)))
                            .into_any_element()
                    })
                    .collect()
            }
            ListMenuTarget::Folder(_) => vec![
                item("folder-menu-edit", IconName::Pencil, strings.edit, ListAction::Edit, cx),
                item("folder-menu-add", IconName::Plus, strings.add_list, ListAction::AddList, cx),
                item("folder-menu-ungroup", IconName::FolderOpen, strings.ungroup, ListAction::Ungroup, cx),
            ],
        }
    }

    fn render_list_menu(&self, strings: &'static Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (target, at) = self.list_menu.clone()?;
        // The tag menu is the original's 163 px; the others as the task menu.
        let width = if matches!(target, ListMenuTarget::Tag(_) | ListMenuTarget::TagsHeading) { 163. } else { 196. };
        let items = self.render_list_items(target, strings, cx);
        let menu = popover(cx)
            .w(px(width))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.list_menu = None;
                cx.notify();
            }))
            .children(items)
            .into_any_element();
        Some(task_menu::place(at, menu))
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

        let due = task.due_date.map(|date| {
            if matches!(self.view, View::Completed | View::WontDo) {
                (task_rows::closed_due_label(date, today, strings), false)
            } else {
                task_rows::row_due_label(date, task.due_time, today, strings, self.preferences.countdown_dates)
            }
        });

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
            // The original's right-click menu, at the pointer; the task is selected too.
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                window.focus(&this.list_focus, cx);
                this.select(id, cx);
                this.context_menu = Some((id, event.position));
                this.context_side = None;
                cx.notify();
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
                            // As the original: closed tasks are muted, not struck through.
                            .color(if done || wont_do || task.title.is_empty() { Color::Muted } else { Color::Default })
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
                    // As the original: "…" on a hovered row opens the task's menu there.
                    .child(
                        div()
                            .id("row-more")
                            .flex_none()
                            .opacity(0.)
                            .group_hover(group.clone(), |s| s.opacity(1.))
                            .child(Icon::new(IconName::Ellipsis).size(IconSize::Small).color(Color::Muted))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                window.focus(&this.list_focus, cx);
                                this.select(id, cx);
                                this.context_menu = Some((id, event.position));
                                this.context_side = None;
                                cx.notify();
                            })),
                    ),
            )
            // And the drag handle left of a hovered row, as the original's.
            .when(draggable, |el| {
                el.child(
                    div()
                        .absolute()
                        .left(px(-13.))
                        .top_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .child(Icon::new(IconName::Menu).size(IconSize::XSmall).color(Color::Muted)),
                )
            })
            .into_any_element()
    }

    /// As in the original, Today and Next 7 Days list the habits due today, unless turned off in
    /// the habit settings. Checking one checks the habit in (one unit for amount goals).
    /// The kanban's Habit column in Today and Next 7 Days, as the original's: a card per habit
    /// due today with its icon and name and "Today 0 / 2 unit" (or "0 / 1" without a goal); a
    /// click checks it in or adds a step.
    fn render_habit_column(&self, strings: &'static Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !matches!(self.view, View::Today | View::Next7Days)
            || self.store.setting(crate::habit_view::IN_TODAY_SETTING).ok().flatten().as_deref() == Some("0")
        {
            return None;
        }
        let theme = cx.theme().clone();
        let today = Self::today();
        let habits: Vec<(fern_core::Habit, u32)> = self
            .store
            .habits()
            .unwrap_or_default()
            .into_iter()
            .filter(|h| !h.archived)
            .filter_map(|h| {
                let done = self.store.checkins(h.id).unwrap_or_default();
                let amount = self.store.checkin_records(h.id).unwrap_or_default().into_iter().find(|c| c.date == today).map_or(0, |c| c.value);
                h.is_due(today, &done).then_some((h, amount))
            })
            .collect();
        if habits.is_empty() {
            return None;
        }
        let count = habits.len();
        let cards: Vec<AnyElement> = habits
            .into_iter()
            .map(|(habit, amount)| {
                let id = habit.id;
                let has_goal = habit.goal.is_some();
                let (target, unit) = habit.goal.as_ref().map_or((1, String::new()), |g| (g.amount, g.unit.clone()));
                v_flex()
                    .id(SharedString::from(format!("habit-card-{id}")))
                    .p_2()
                    .gap_1()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.colors().border_variant)
                    .bg(theme.colors().surface_background)
                    .cursor_pointer()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(crate::habit_view::avatar(&habit, px(20.), cx))
                            .child(div().flex_1().min_w_0().child(Label::new(habit.name.clone()).truncate())),
                    )
                    .child(
                        h_flex()
                            .pl(px(28.))
                            .gap_1()
                            .child(Label::new(strings.today).size(LabelSize::Small).color(Color::Accent))
                            .child(Label::new(format!("{amount} / {target} {unit}").trim().to_owned()).size(LabelSize::Small).color(Color::Muted)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let result = if has_goal {
                            this.store.step_checkin(id, today).map(|_| ())
                        } else {
                            this.store.toggle_checkin(id, today).map(|_| ())
                        };
                        this.check(result);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        Some(
            v_flex()
                .w(px(258.))
                .flex_none()
                .gap_2()
                .child(
                    h_flex()
                        .h(px(30.))
                        .gap_1()
                        .child(Label::new(strings.section_habits).weight(FontWeight::SEMIBOLD))
                        .child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)),
                )
                .children(cards)
                .into_any_element(),
        )
    }

    fn render_habits(&self, strings: &Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !matches!(self.view, View::Today | View::Next7Days)
            || self.store.setting(crate::habit_view::IN_TODAY_SETTING).ok().flatten().as_deref() == Some("0")
        {
            return None;
        }
        let theme = cx.theme().clone();
        let today = Self::today();
        let habits: Vec<(fern_core::Habit, bool, u32)> = self
            .store
            .habits()
            .unwrap_or_default()
            .into_iter()
            .filter(|h| !h.archived)
            .filter_map(|h| {
                let done = self.store.checkins(h.id).unwrap_or_default();
                let amount = self.store.checkin_records(h.id).unwrap_or_default().into_iter().find(|c| c.date == today).map_or(0, |c| c.value);
                h.is_due(today, &done).then(|| {
                    let checked = done.contains(&today);
                    (h, checked, amount)
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
                        // Next 7 Days names today's habits by the day, as the original: "Sunday, Today's Habit".
                        .child(Label::new(if self.view == View::Next7Days {
                            format!("{}, {}", strings.weekdays_full[today.weekday().num_days_from_monday() as usize], strings.todays_habit)
                        } else {
                            strings.section_habits.to_owned()
                        }).weight(FontWeight::BOLD))
                        .child(div().pl_1().child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted))),
                )
                // As the original's rows: the icon and name, then "0 / 2 unit" and "Today" on
                // the right, no box; a click checks the habit in or adds a step.
                .children(habits.into_iter().map(|(habit, checked, amount)| {
                    let id = habit.id;
                    let has_goal = habit.goal.is_some();
                    let (target, unit) = habit.goal.as_ref().map_or((1, String::new()), |g| (g.amount, g.unit.clone()));
                    h_flex()
                        .id(SharedString::from(format!("habit-row-{id}")))
                        .h(px(ROW_HEIGHT))
                        .pl(px(GUTTER))
                        .pr_3()
                        .gap(px(10.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.colors().ghost_element_hover))
                        .child(crate::habit_view::avatar(&habit, px(20.), cx))
                        .child(
                            div().flex_1().min_w_0().child(
                                Label::new(habit.name.clone())
                                    .color(if checked { Color::Muted } else { Color::Default })
                                    .truncate(),
                            ),
                        )
                        .child(Label::new(format!("{amount} / {target} {unit}").trim().to_owned()).size(LabelSize::Small).color(Color::Muted))
                        .child(Label::new(strings.today).size(LabelSize::Small).color(Color::Accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let result = if has_goal {
                                this.store.step_checkin(id, today).map(|_| ())
                            } else {
                                this.store.toggle_checkin(id, today).map(|_| ())
                            };
                            this.check(result);
                            cx.notify();
                        }))
                        .border_b_1()
                        .border_color(theme.colors().border_variant)
                }))
                .into_any_element(),
        )
    }

    /// The countdowns whose Show puts them in Today (on the day by default, as in the original),
    /// each with its icon, name and the days left.
    fn render_countdowns(&self, strings: &Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.view != View::Today {
            return None;
        }
        let theme = cx.theme().clone();
        let today = Self::today();
        let countdowns: Vec<fern_core::Countdown> = self
            .store
            .countdowns()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| !c.archived && c.show_in_list.shows(c.next_date(today), today))
            .collect();
        if countdowns.is_empty() {
            return None;
        }
        let count = countdowns.len();
        Some(
            v_flex()
                .child(
                    h_flex()
                        .h(px(36.))
                        .gap_1()
                        .pl(px(GUTTER))
                        .child(Label::new(strings.section_countdown).weight(FontWeight::BOLD))
                        .child(div().pl_1().child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted))),
                )
                .children(countdowns.into_iter().map(|countdown| {
                    let date = countdown.next_date(today);
                    let days = countdown.span(today).total_days;
                    let when = if date == today { strings.today.to_owned() } else { format!("{days} {}", strings.habit_days) };
                    h_flex()
                        .h(px(ROW_HEIGHT))
                        .pl(px(GUTTER))
                        .pr_3()
                        .gap(px(10.))
                        .child(match countdown.icon.clone() {
                            Some(icon) => div().w(px(16.)).child(icon).into_any_element(),
                            None => Icon::new(crate::countdown_view::kind_icon(countdown.kind)).color(Color::Accent).into_any_element(),
                        })
                        .child(div().flex_1().min_w_0().child(Label::new(countdown.name.clone()).truncate()))
                        .child(Label::new(when).size(LabelSize::Small).color(Color::Accent))
                        .border_b_1()
                        .border_color(theme.colors().border_variant)
                }))
                .into_any_element(),
        )
    }

    /// The details' own picker for a task, for the right-click menu and the calendar's.
    pub fn start_detail(&mut self, id: Id, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.select(id, cx);
        self.detail.update(cx, |detail, cx| detail.start(what, window, cx));
    }

    /// A right-click menu choice (see `task_menu`).
    fn context_action(&mut self, action: task_menu::MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        use task_menu::MenuAction;
        let Some((id, _)) = self.context_menu else { return };
        if let MenuAction::Side(side) = action {
            self.context_side = Some(side);
            cx.notify();
            return;
        }
        self.context_menu = None;
        self.context_side = None;
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned().or_else(|| self.store.task(id).ok().flatten()) else {
            cx.notify();
            return;
        };
        if let Some(result) = task_menu::apply(&self.store, &task, action, Self::today()) {
            self.check(result);
            self.reload(cx);
            return;
        }
        match action {
            MenuAction::Due(None) => self.start_detail(id, "due", window, cx),
            MenuAction::AddSubtask => self.start_detail(id, "subtask", window, cx),
            MenuAction::LinkParent => self.start_detail(id, "link-parent", window, cx),
            MenuAction::Tags => self.start_detail(id, "tags", window, cx),
            MenuAction::Focus(stopwatch) => cx.emit(StartFocus(id, task.title.clone(), stopwatch)),
            MenuAction::CopyLink => cx.write_to_clipboard(gpui::ClipboardItem::new_string(fern_core::link::task_link(id))),
            // Deleting goes the list's usual way, with its Undo.
            MenuAction::Delete => self.delete(id, cx),
            _ => {}
        }
        cx.notify();
    }

    /// The task's right-click menu, as the original's, at the pointer.
    fn render_context_menu(&self, strings: &'static Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, position) = self.context_menu?;
        let task = self.tasks.iter().find(|t| t.id == id).cloned().or_else(|| self.store.task(id).ok().flatten())?;
        let menu = task_menu::render(
            &task,
            &self.active_lists(),
            task_menu::MenuKind::List,
            self.context_side,
            strings,
            cx,
            std::rc::Rc::new(|this: &mut Self, action, window, cx| this.context_action(action, window, cx)),
        );
        Some(task_menu::place(position, menu))
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

    fn render_header_menu(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let in_list = matches!(self.view, View::List(_));
        let in_trash = self.view == View::Trash;
        popover(cx)
            .w(px(236.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.header_menu_open = false;
                cx.notify();
            }))
            .when_some(match self.view {
                View::List(id) => Some(id),
                _ => None,
            }, |el, id| el.children(self.render_list_items(ListMenuTarget::List(id), strings, cx)).child(div().my_1().child(Divider::horizontal())))
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
                    .when(timeline, |el| {
                        el.child(
                            ListItem::new("arrange-tasks")
                                .start_slot(Icon::new(IconName::CalendarPlus).color(Color::Muted))
                                .child(Label::new(strings.arrange_tasks))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.arrange_open = true;
                                    this.header_menu_open = false;
                                    cx.notify();
                                })),
                        )
                    })
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
                    .child(
                        ListItem::new("view-options")
                            .start_slot(Icon::new(IconName::Settings2).color(Color::Muted))
                            .child(Label::new(strings.view_options))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.header_menu_open = false;
                                this.view_options = Some(false);
                                cx.notify();
                            })),
                    )
                    .child(div().my_1().child(Divider::horizontal()))
                    .when(self.section_owner().is_some(), |el| {
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
fn chevron(collapsed: bool) -> Icon {
    Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall)
}

impl Focusable for TodoView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for TodoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let closed_header = matches!(self.view, View::Completed | View::WontDo);
        let add_focused = self.input.focus_handle(cx).is_focused(window);
        let input_focus = self.input.focus_handle(cx);
        let today = Self::today();
        let tag_colors: HashMap<String, Hsla> = self
            .tags
            .iter()
            .map(|t| (t.name.clone(), lists_panel::color(t.color.as_deref(), cx)))
            .collect();
        // The original's "too narrow" window: the details then follow View Options' layout.
        let popup_details = self.preferences.popup_details && window.viewport_size().width < px(NARROW_WINDOW);
        let timeline_view = self.timeline
            && !self.kanban
            && !matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_));
        let kanban_view = (self.kanban || timeline_view)
            && !matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_));
        // The original's kanban and timeline have no add field on top: a kanban column has its
        // own "+".
        let kanban_only = kanban_view && !timeline_view;
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
                .gap(px(24.))
                .px_4()
                .pt_1()
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
                        .w(px(258.))
                        .flex_none()
                        .max_h_full()
                        .gap_2()
                        .child(self.render_column_header(
                            i,
                            if title.is_empty() { strings.group_tasks.to_owned() } else { title },
                            count,
                            drop_for_header,
                            strings,
                            cx,
                        ))
                        .children(self.column_add.as_ref().filter(|(at, _, _)| *at == i).map(|(_, _, input)| {
                            h_flex()
                                .h(px(36.))
                                .px_3()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(theme.colors().accent)
                                .bg(theme.colors().surface_background)
                                .child(input.clone())
                        }))
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
                .children(self.render_habit_column(strings, cx))
                .when(self.section_owner().is_some(), |el| {
                    el.child(
                        v_flex().w(px(220.)).flex_none().pt(px(6.)).map(|el| match self.new_section.clone() {
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
            .pt(px(crate::TOP_INSET))
            .child(
                h_flex()
                    .h(px(60.))
                    // The title sits 4 px above the row's middle, as the original's.
                    .pb(px(8.))
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
                    // The original's light bulb in Today: Suggested Tasks.
                    .when(self.view == View::Today, |el| {
                        el.child(
                            IconButton::new("suggestions", IconName::Lightbulb)
                                .toggle_state(self.suggestions_open.is_some())
                                .tooltip(Tooltip::text(strings.suggested_tasks))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.suggestions_open = if this.suggestions_open.is_some() { None } else { Some(false) };
                                    cx.notify();
                                })),
                        )
                    })
                    // The Completed and Won't Do views have only Print, as the original's.
                    .when(closed_header, |el| el.child(IconButton::new("print-closed", IconName::Printer).tooltip(Tooltip::text(strings.print)).on_click(cx.listener(|this, _, _, cx| this.print_list(cx)))))
                    .when(self.view != View::Trash && !closed_header, |el| {
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
                    .when(self.view != View::Trash && !closed_header, |el| el.child(
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
            .when(!self.is_read_only_view() && !kanban_view && self.preferences.detailed_input, |el| el.child(self.render_detailed_add(strings, cx)))
            .when(!self.is_read_only_view() && !kanban_view && !self.preferences.detailed_input, |el| el.child(
                // As in the original: 20 px in from the column's edge, lined up with the rows,
                // and outlined in the accent while focused.
                h_flex()
                    .mx(px(20.))
                    .mb_2()
                    .h(px(38.))
                    .flex_none()
                    .gap_2()
                    .px_3()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if add_focused { theme.colors().accent } else { gpui::transparent_black() })
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
                    .px(px(14.))
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
                    // An empty Tomorrow, as the original's: a map in a soft patch and its words.
                    .when(rows.is_empty() && self.view == View::Tomorrow, |el| {
                        el.child(
                            v_flex()
                                .pt(px(160.))
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .size(px(96.))
                                        .mb_2()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .bg(theme.colors().element_hover)
                                        .child(Icon::new(IconName::Map).size(IconSize::XLarge).color(Color::Accent)),
                                )
                                .child(Label::new(strings.no_tasks_tomorrow))
                                .child(Label::new(strings.plan_ahead).size(LabelSize::Small).color(Color::Muted)),
                        )
                    })
                    .when(rows.is_empty() && !matches!(self.view, View::Day(_) | View::Tomorrow), |el| {
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
                    // In Next 7 Days today's habits come first, as the original's.
                    .when(self.view == View::Next7Days, |el| el.children(self.render_habits(strings, cx)))
                    .children(rows)
                    .when(self.view != View::Next7Days, |el| el.children(self.render_habits(strings, cx)))
                    .children(self.render_countdowns(strings, cx)),
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
                        folders: &self.folders,
                        collapsed_folders: &self.collapsed_folders,
                        archived_open: self.archived_open,
                        collapsed_tags: &self.collapsed_tags,
                        tags_collapsed: self.tags_collapsed,
                        mini: lists_panel::MiniCalendar { expanded: self.mini_expanded, month: self.mini_month, busy: &self.busy_days },
                    },
                    on_select,
                    {
                        let this = cx.entity().downgrade();
                        move |event, window: &mut Window, cx: &mut App| {
                            this.update(cx, |this, cx| this.panel_event(event, window, cx)).ok();
                        }
                    },
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
                    return el.child(div().flex_1().min_w_0().h_full().pt(px(crate::TOP_INSET)).child(self.summary.clone()));
                }
                // As in the original, the Timeline takes the whole width until a task is opened.
                if (timeline_view || kanban_only) && self.selected.is_none() && self.list_activity.is_none() {
                    return el.child(list);
                }
                // View Options' Pop-up: in a narrow window an opened task shows over the list.
                if popup_details {
                    if self.selected.is_none() && self.list_activity.is_none() {
                        return el.child(list);
                    }
                    return el.child(list).child(
                        div()
                            .id("detail-popup")
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(gpui::black().opacity(0.35))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected = None;
                                this.list_activity = None;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .id("detail-popup-card")
                                    .w(px(DETAIL_WIDTH))
                                    .h(gpui::relative(0.8))
                                    .rounded(px(10.))
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(theme.colors().border)
                                    .bg(theme.colors().background)
                                    .shadow_lg()
                                    .on_click(|_, _, cx| cx.stop_propagation())
                                    .map(|el| match self.list_activity.clone() {
                                        Some(entries) => el.child(self.render_list_activity(&entries, cx)),
                                        None => el.child(self.detail.clone()),
                                    }),
                            ),
                    );
                }
                el.child(list).child(div().flex_none().w(px(DETAIL_WIDTH)).h_full().pt(px(crate::TOP_INSET)).map(|el| match self.list_activity.clone() {
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
            .children(self.render_context_menu(strings, cx))
            .children(self.render_list_menu(strings, cx))
            .children(self.view_options.map(|menu| self.render_view_options(menu, strings, cx)))
            .children(self.suggestions_open.filter(|_| self.view == View::Today).map(|all| self.render_suggestions(all, strings, cx)))
            .children(self.list_dialog.clone())
            .children(self.folder_dialog.clone())
            .children(self.tag_dialog.clone())
            .children(self.merge_dialog.clone())
    }
}

/// What a list menu acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ListMenuTarget {
    List(Id),
    Folder(Id),
    Tag(String),
    /// The Tags heading's "…": whether tags show.
    TagsHeading,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListAction {
    Edit,
    Duplicate,
    Archive,
    Delete,
    AddList,
    Ungroup,
    Pin,
    AddSubtag,
    Merge,
    ShowTags(fern_core::Visibility),
}
