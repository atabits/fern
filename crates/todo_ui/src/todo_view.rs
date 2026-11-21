use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use fern_core::{ColorBy, Id, Kind, List, ListFolder, Priority, Store, Tag, Task, View, ViewCounts};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone as _};
use gpui::{ElementId, Entity, FocusHandle, Focusable, FontWeight, ListAlignment, ListState, MouseButton, Pixels, Point, list};
use ui::prelude::*;

use crate::drag::{
    DraggedColumn, DraggedFilter, DraggedFolder, DraggedList, DraggedParentTag,
    DraggedSidebarSection, DraggedSmartList, DraggedTag, DraggedTask, ResizeEdge, ResizedTask,
};
use ui::{HorizontalScrollbar, Placement, PopoverAttachment, SubmenuItem, VerticalScrollbar, popover};

use crate::actions::{
    AddSubtask, ClearDate, DeleteTask, DismissTransient, ExtendSelectionNext, ExtendSelectionPrevious, PinTask,
    PriorityHigh, PriorityLow, PriorityMedium, PriorityNone, SelectAllTasks, SelectNext, SelectPrevious, SetDate,
    SetNextWeek, SetToday, SetTomorrow, ToggleAllSubtasks, ToggleComplete, ToggleInProgress,
};
use crate::i18n::{self, CurrentLocale, Strings};
use crate::list_dialog::{DialogEvent, FolderDialog, ListDialog};
use crate::tag_dialog::{MergeDialog, TagDialog, TagEvent};
use crate::lists_panel;
use crate::resize::{self, DraggedEdge, Panel};
use crate::task_detail::{OpenTask, StartFocus, TaskDetail, TaskUpdated};
use crate::task_menu;
use crate::task_rows::{self, ClosedRange, DropTarget, GroupBy, Row, Section, SortBy};
use crate::text_input::{Cancelled, Changed, Mode, MultiLinePasted, Submitted, TextInput};

/// The details' width by default and the narrowest its edge drags to; the task list keeps at
/// least `LIST_MIN_WIDTH`, which also bounds how wide the details get.
const DETAIL_WIDTH: f32 = 419.;
const DETAIL_MIN_WIDTH: f32 = 340.;
const LIST_MIN_WIDTH: f32 = 360.;
const ROW_HEIGHT: f32 = 40.;
/// Below this width the window is narrow, for View Options' Task Detail Layout.
const NARROW_WINDOW: f32 = 1200.;
/// Space left of the checkbox for the subtask chevron.
const GUTTER: f32 = 20.;
const INDENT: f32 = 24.;
/// Left padding before a root kanban card's checkbox (chevron sits inside this gutter).
const KANBAN_GUTTER: f32 = 16.;


impl gpui::EventEmitter<StartFocus> for TodoView {}

/// Emitted when the user opens a habit from Today / Next 7 Days.
pub struct OpenHabit(pub Id);

impl gpui::EventEmitter<OpenHabit> for TodoView {}

/// Emitted when the user starts a focus session directly from a habit row in Today / Next 7 Days.
pub struct StartHabitFocus {
    pub habit_id: Id,
    pub title: String,
    pub stopwatch: bool,
}

impl gpui::EventEmitter<StartHabitFocus> for TodoView {}

mod batch;
mod filters;
mod kanban;
mod menus;
mod panel_dnd;
mod quick_add;
mod timeline;
#[cfg(all(test, debug_assertions))]
mod tests;

use batch::BatchMenu;
use filters::FilterEdit;
use kanban::{BoardCard, KanbanColRows};
use menus::{ClosedMenu, ListMenuTarget, SortMenu};
#[cfg(debug_assertions)]
use menus::RangeEnd;
use panel_dnd::{DragExpandTarget, PanelDragState};
use quick_add::AddDraft;

/// The saved task stays in its original group briefly while its checkbox settles.
/// Only presentation is held; the store and the details already have the committed state.
struct CompletingTask {
    before: Task,
    index: usize,
    view: View,
    undo: crate::undo::UndoAction,
    _timer: gpui::Task<()>,
}


#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ConfirmDeleteTarget {
    List(Id, String),
    Tag(String),
    Filter(Id, String),
    Section(Id, String),
    BatchPurge(usize),
}

pub struct TodoView {
    store: Rc<Store>,
    view: View,
    tasks: Vec<Task>,
    completing: HashMap<Id, CompletingTask>,
    counts: ViewCounts,
    lists: Vec<List>,
    tags: Vec<Tag>,
    show_lists: bool,
    /// The lists column's and the details' widths, as their edges were dragged.
    lists_width: f32,
    detail_width: f32,
    narrow_viewport: bool,
    /// The edge being dragged, if any.
    resizing: Option<Panel>,
    /// Saves the widths once the drag has rested a moment.
    save_widths: Option<gpui::Task<()>>,
    collapsed_tasks: HashSet<Id>,
    collapsed_sections: HashSet<Section>,
    /// Day and section groups folded by their header's arrow, by their title.
    collapsed_days: HashSet<String>,
    input: Entity<TextInput>,
    /// Focus of the task list itself, for keyboard navigation (the `TaskList` key context).
    list_focus: FocusHandle,
    /// The task list, drawn as Zed's lists: only the rows in view are built. `list_entries` are
    /// what it showed last, to tell it which rows changed.
    list_state: ListState,
    list_entries: Vec<ListEntry>,
    /// Cached `Row` slice extracted from `list_entries` so `move_task_drag` does not clone every
    /// task on every mouse pixel.
    list_rows: Vec<Row>,
    /// Window bounds of currently painted `list_entries` rows, keyed by entry index.
    list_entry_bounds: Rc<RefCell<HashMap<usize, gpui::Bounds<Pixels>>>>,
    /// Active task-list drag origin: `(dragged_task_id, start_mouse_x)`.
    task_drag_origin: Option<(Id, Pixels)>,
    /// Live drop plan while dragging a task over `#tasks`.
    task_drop: Option<task_rows::TaskDropPlan>,
    tag_colors: HashMap<String, Hsla>,
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
    drag_expand: Option<(DragExpandTarget, gpui::Task<()>)>,
    lists_scroll: gpui::ScrollHandle,
    lists_rows: Rc<RefCell<Vec<lists_panel::PanelChild>>>,
    panel_drag: Option<PanelDragState>,
    search_input: Entity<TextInput>,
    header_menu_open: bool,
    confirm_empty_trash: bool,
    confirm_delete: Option<ConfirmDeleteTarget>,
    /// Lines pasted into Quick Add awaiting confirmation for Batch Add Tasks.
    batch_add_pending: Option<Vec<String>>,
    /// Filters of the Completed and Won't Do views; `None` lists means all lists.
    closed_range: ClosedRange,
    closed_lists: Option<Vec<Option<Id>>>,
    closed_menu: Option<ClosedMenu>,
    /// The header's sort menu: `Some(None)` when open, with the expanded submenu otherwise.
    sort_menu: Option<Option<SortMenu>>,
    sort_menu_focus: FocusHandle,
    sort_menu_return_focus: Option<FocusHandle>,
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
    /// Global kanban display options (from View Options, persisted in store settings).
    kanban_column_size: u8,
    kanban_column_color: bool,
    kanban_quick_add: bool,
    kanban_workflow: bool,
    kanban_quick_adds: HashMap<String, (DropTarget, Entity<TextInput>)>,
    timeline_secondary_tz: Option<String>,
    timeline_tz_menu: bool,
    timeline_tz_input: Entity<TextInput>,
    timeline_color_by: ColorBy,
    timeline_show_weekends: bool,
    kanban_section_colors: HashMap<Id, String>,
    kanban_folded_sub: HashSet<String>,
    kanban_scroll: gpui::ScrollHandle,
    /// Horizontal pan-to-scroll on empty kanban space: `(start_pointer_x, start_scroll_offset_x)`.
    kanban_pan: Option<(Pixels, Pixels)>,
    /// Window bounds of currently painted kanban task rows and columns, for drag-and-drop.
    kanban_row_bounds: Rc<RefCell<HashMap<Id, gpui::Bounds<Pixels>>>>,
    kanban_col_bounds: Rc<RefCell<HashMap<String, gpui::Bounds<Pixels>>>>,
    /// Cached per-column visible rows from the last kanban render frame, for drag planning.
    kanban_drag_rows: Vec<KanbanColRows>,
    /// Target column key highlighted during a cross-column task drag.
    kanban_hover_col: Option<String>,
    /// The Timeline's groups folded up, by name.
    timeline_folded: HashSet<String>,
    /// Today's "Suggested Tasks" panel (the header's light bulb), and whether its Long Unfinished
    /// list is shown in full.
    suggestions_open: Option<bool>,
    /// The header menu's View Options dialog and its Show Date by dropdown, while open.
    view_options: Option<bool>,
    section_rename: Option<(Id, Entity<TextInput>)>,
    title_edit: Option<(Id, Entity<TextInput>, gpui::Subscription)>,
    /// The "Undo" toast after completing or deleting a task, with its timer.
    undo: Option<(crate::undo::UndoAction, String, gpui::Task<()>)>,
    /// The last undone action, available for Redo (Ctrl+Shift+Z / Ctrl+Y).
    redo: Option<(crate::undo::UndoAction, String)>,
    /// Saved filters with their task counts, for the lists column.
    filters: Vec<(fern_core::SmartFilter, usize)>,
    /// Settings → Smart List, read on reload.
    preferences: fern_core::Preferences,
    /// The date quick add recognized in the field so far, shown at its end.
    quick_add_due: Option<(NaiveDate, Option<chrono::NaiveTime>)>,
    quick_add_repeat: bool,
    /// The mini calendar: expanded to a month, which month, and the days with open tasks.
    mini_expanded: bool,
    mini_month: NaiveDate,
    busy_days: HashSet<NaiveDate>,
    /// Today and Next 7 Days: the habits due today, whether each is checked in and its amount;
    /// empty elsewhere or when the habit settings keep them out.
    today_habits: Vec<(fern_core::Habit, bool, f64)>,
    /// Right-click context menu on a habit row in Today / Next 7 Days.
    habit_menu: Option<(Id, gpui::Point<Pixels>)>,
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
    /// The add field's date picker on its Duration tab: the start and end being picked.
    add_range: Option<crate::due_picker::DueDraft>,
    add_date_row: Option<u8>,
    /// The section whose "…" menu is open.
    section_menu: Option<Id>,
    /// The overdue group's "Postpone" menu is open.
    postpone_menu: bool,
    detail: Entity<TaskDetail>,
    /// The board's task card, opened beside a clicked card as the original's, and its task.
    card_detail: Entity<TaskDetail>,
    card: Option<Id>,
    selected: Option<Id>,
    multi_selected: HashSet<Id>,
    selection_anchor: Option<Id>,
    batch_menu: Option<BatchMenu>,
    /// A task row's right-click menu: the task, pointer window coordinates, and precomputed nesting.
    context_menu: Option<(Id, gpui::Point<Pixels>, task_menu::TaskNesting)>,
    /// Its side menu open: 0 Move to, 1 Start Focus.
    context_side: Option<u8>,
    error: Option<String>,
    add_attachment_error: Option<String>,
}

impl TodoView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let summary = cx.new(|_| crate::summary_view::SummaryView::new(store.clone()));
        let mini_expanded = store.setting("mini_expanded").ok().flatten().as_deref() == Some("1");
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_task, cx).intercept_multiline_paste());
        cx.subscribe_in(&input, window, |this, _, Submitted(text), window, cx| this.submit_new_task(text, window, cx))
            .detach();
        cx.subscribe(&input, |this, _, MultiLinePasted(lines), cx| {
            this.batch_add_pending = Some(lines.clone());
            cx.notify();
        })
        .detach();
        // As the user types, the recognized date, tags and list are highlighted and the date is
        // shown at the end of the field, as in the original.
        cx.subscribe(&input, |this, _, _: &Changed, cx| this.update_quick_add_hint(cx)).detach();
        window.focus(&input.focus_handle(cx), cx);
        // Escape in the quick-add field hands the keyboard to the list.
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            if !this.dismiss_transient(window, cx) {
                window.focus(&this.list_focus, cx);
            }
        })
        .detach();

        let add_tag_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.type_in_tag, cx));
        cx.subscribe(&add_tag_input, |_, _, _: &Changed, cx| cx.notify()).detach();
        cx.subscribe_in(&add_tag_input, window, |this, _, _: &Cancelled, window, cx| {
            this.dismiss_transient(window, cx);
        })
        .detach();
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
            this.update_search_query(query, cx);
        })
        .detach();
        let timeline_tz_input = cx.new(|cx| TextInput::new(Mode::SingleLine, "", cx).without_cancel());
        cx.subscribe(&timeline_tz_input, |_, _, _: &Changed, cx| cx.notify()).detach();
        cx.observe_window_bounds(window, |this, window, cx| this.sync_viewport(window, cx))
            .detach();

        // A change made in a sticky note shows here.
        cx.observe_global::<crate::data_version::DataVersion>(|this, cx| {
            if crate::data_version::changed_elsewhere(cx.entity_id(), cx) {
                this.reload(cx);
            }
        })
        .detach();

        // Placeholders live inside the inputs, so they are updated when the language changes.
        cx.observe_global::<CurrentLocale>(|this, cx| {
            let strings = i18n::t(cx);
            this.sync_input_placeholder(cx);
            this.search_input.update(cx, |input, cx| input.set_placeholder(strings.search_placeholder, cx));
            for (_, input) in this.kanban_quick_adds.values() {
                input.update(cx, |input, cx| input.set_placeholder(strings.add_task, cx));
            }
        })
        .detach();

        cx.observe_keystrokes(|this, event, window, cx| {
            if event.keystroke.key == "escape" {
                let cancelled = this.cancel_panel_drag(window, cx) || this.cancel_task_drag(window, cx);
                if cancelled {
                    cx.notify();
                }
            }
        })
        .detach();

        let detail = cx.new(|cx| TaskDetail::new(store.clone(), window, cx));
        cx.subscribe(&detail, |this, _, _: &TaskUpdated, cx| this.reload(cx)).detach();
        cx.subscribe(&detail, |this, _, OpenTask(id), cx| this.select(*id, cx)).detach();
        cx.subscribe(&detail, |_, _, StartFocus(id, title, stopwatch), cx| cx.emit(StartFocus(*id, title.clone(), *stopwatch))).detach();
        cx.subscribe(&detail, |this, _, crate::task_detail::Undoable(action, label), cx| {
            this.offer_undo(action.clone(), label.clone(), cx);
        })
        .detach();

        let card_detail = cx.new(|cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.set_card(false);
            detail
        });
        cx.subscribe(&card_detail, |this, _, _: &TaskUpdated, cx| this.card_updated(cx)).detach();
        // A subtask or parent clicked in the card opens in the card.
        cx.subscribe(&card_detail, |this, _, OpenTask(id), cx| this.open_card(*id, cx)).detach();
        cx.subscribe(&card_detail, |_, _, StartFocus(id, title, stopwatch), cx| cx.emit(StartFocus(*id, title.clone(), *stopwatch))).detach();
        cx.subscribe(&card_detail, |this, _, crate::task_detail::Undoable(action, label), cx| {
            this.offer_undo(action.clone(), label.clone(), cx);
        })
        .detach();

        let width = |key: &str, default: f32| store.setting(key).ok().flatten().and_then(|value| value.parse().ok()).unwrap_or(default);
        let lists_width = width("lists_width", lists_panel::WIDTH).clamp(lists_panel::MIN_WIDTH, lists_panel::MAX_WIDTH);
        let detail_width = width("detail_width", DETAIL_WIDTH).max(DETAIL_MIN_WIDTH);
        let narrow_viewport = window.viewport_size().width < px(NARROW_WINDOW);
        let mut this = Self {
            store,
            view: View::Inbox,
            tasks: Vec::new(),
            completing: HashMap::new(),
            counts: ViewCounts::default(),
            lists: Vec::new(),
            tags: Vec::new(),
            show_lists: true,
            lists_width,
            detail_width,
            narrow_viewport,
            resizing: None,
            save_widths: None,
            collapsed_tasks: HashSet::new(),
            // As in TickTick, completed tasks sit collapsed at the bottom of the list.
            collapsed_sections: HashSet::from([Section::Completed]),
            collapsed_days: HashSet::new(),
            input,
            list_focus: cx.focus_handle(),
            list_state: Self::new_list_state(),
            list_entries: Vec::new(),
            list_rows: Vec::new(),
            list_entry_bounds: Rc::new(RefCell::new(HashMap::new())),
            task_drag_origin: None,
            task_drop: None,
            tag_colors: HashMap::new(),
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
            drag_expand: None,
            lists_scroll: gpui::ScrollHandle::new(),
            lists_rows: Rc::new(RefCell::new(Vec::new())),
            panel_drag: None,
            search_input,
            header_menu_open: false,
            confirm_empty_trash: false,
            confirm_delete: None,
            batch_add_pending: None,
            closed_range: ClosedRange::All,
            closed_lists: None,
            closed_menu: None,
            sort_menu: None,
            sort_menu_focus: cx.focus_handle(),
            sort_menu_return_focus: None,
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
            kanban_column_size: 1,
            kanban_column_color: false,
            kanban_quick_add: false,
            kanban_workflow: false,
            kanban_quick_adds: HashMap::new(),
            timeline_secondary_tz: None,
            timeline_tz_menu: false,
            timeline_tz_input,
            timeline_color_by: ColorBy::List,
            timeline_show_weekends: true,
            kanban_section_colors: HashMap::new(),
            kanban_folded_sub: HashSet::new(),
            kanban_scroll: gpui::ScrollHandle::new(),
            kanban_pan: None,
            kanban_row_bounds: Rc::new(RefCell::new(HashMap::new())),
            kanban_col_bounds: Rc::new(RefCell::new(HashMap::new())),
            kanban_drag_rows: Vec::new(),
            kanban_hover_col: None,
            timeline_folded: HashSet::new(),
            view_options: None,
            suggestions_open: None,
            section_rename: None,
            title_edit: None,
            section_menu: None,
            postpone_menu: false,
            templates_open: false,
            add_menu_open: false,
            add_list_side: false,
            add_draft: AddDraft::default(),
            add_tags: None,
            add_tag_input,
            input_box_open: false,
            add_date_month: None,
            add_range: None,
            add_date_row: None,
            list_activity: None,
            filters: Vec::new(),
            preferences: Default::default(),
            quick_add_due: None,
            quick_add_repeat: false,
            mini_expanded,
            mini_month: crate::calendar::month_start(Self::today()),
            busy_days: HashSet::new(),
            today_habits: Vec::new(),
            habit_menu: None,
            summary,
            filter_edit: None,
            undo: None,
            redo: None,
            detail,
            card_detail,
            card: None,
            selected: None,
            multi_selected: HashSet::new(),
            selection_anchor: None,
            batch_menu: None,
            context_menu: None,
            context_side: None,
            error: None,
            add_attachment_error: None,
        };
        this.reload_inner(false, false, cx);
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
                    self.add_task(text, cx);
                    self.reload(cx);
                }
                Some(("list", name)) => {
                    let result = self.store.create_list(name);
                    if let Some(list) = self.check(result) {
                        self.set_view(View::List(list.id), cx);
                    } else {
                        self.reload(cx);
                    }
                }
                Some(("view", "trash")) => self.set_view(View::Trash, cx),
                Some(("view", "wontdo")) => self.set_view(View::WontDo, cx),
                Some(("view", "today")) => self.set_view(View::Today, cx),
                Some(("view", "summary")) => self.set_view(View::Summary, cx),
                Some(("view", "next7")) => self.set_view(View::Next7Days, cx),
                Some(("view", "completed")) => self.set_view(View::Completed, cx),
                Some(("view", name)) if name.starts_with('#') => self.set_view(View::Tag(name[1..].to_owned()), cx),
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
                Some(("list-section", spec)) => {
                    // `list-section=<name>` adds a section to the current list; `list-section=<name>><task>` also files a task.
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
                    let columns = self.board_columns(strings);
                    if let Some((needle, column)) = spec.split_once('>')
                        && let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id)
                        && let Some(target) = column.parse::<usize>().ok().and_then(|c| columns.get(c)).and_then(|col| col.drop.clone())
                    {
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
                // The add field's picker on its Duration tab, from the next full hour.
                Some(("menu", "add-duration")) => {
                    self.add_date_month = Some(crate::calendar::month_start(Self::today()));
                    let mut range = crate::due_picker::DueDraft { month: crate::calendar::month_start(Self::today()), ..Default::default() };
                    range.start_duration(Local::now().naive_local(), self.preferences.task_defaults.duration);
                    self.add_range = Some(range);
                }
                Some(("menu", "add-date")) => {
                    self.add_draft.date = Some(Self::today() + chrono::Duration::days(1));
                    self.add_date_month = Some(crate::calendar::month_start(Self::today()));
                }
                Some(("menu", "detailed-input")) => {
                    self.save_preferences(|p| p.detailed_input = true, cx);
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
                        self.open_context_menu(task.id, gpui::point(px(420.), px(200.)), true, window, cx);
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
                    if mode == "chain"
                        && let Some(edit) = self.filter_edit.as_mut()
                    {
                        edit.draft.rules = fern_core::filter::examples()[2].rules.clone();
                        edit.inputs = Self::filter_inputs(&edit.draft, cx);
                        edit.chain_menu = Some(1);
                    }
                }
                Some(("filter", spec)) => {
                    // `filter=<name>><tag>`: a saved filter of one tag, opened.
                    let (name, tag) = spec.split_once('>').unwrap_or((spec, ""));
                    let names = if tag.is_empty() { Vec::new() } else { vec![tag.to_owned()] };
                    let filter = fern_core::SmartFilter {
                        name: name.to_owned(),
                        rules: fern_core::FilterRules { conditions: vec![fern_core::Condition::new(fern_core::ConditionValue::Tags { names, untagged: false })], ..Default::default() },
                        ..fern_core::SmartFilter::new()
                    };
                    if self.store.save_filter(&filter).is_ok() {
                        self.set_view(View::Filter(filter.id), cx);
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
                Some(("group", key)) => self.set_sort(None, Some(GroupBy::from_key(key)), window, cx),
                Some(("sort", key)) => self.set_sort(Some(SortBy::from_key(key)), None, window, cx),
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

    fn uses_board_layout(&self) -> bool {
        (self.kanban || self.timeline)
            && !matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_))
    }

    fn uses_popup_details(&self) -> bool {
        self.preferences.popup_details && self.narrow_viewport
    }

    fn sync_detail_inset(&mut self, cx: &mut Context<Self>) {
        let top_inset = !self.uses_popup_details();
        self.detail.update(cx, |detail, cx| detail.set_top_inset(top_inset, cx));
    }

    fn sync_viewport(&mut self, window: &Window, cx: &mut Context<Self>) {
        let narrow = window.viewport_size().width < px(NARROW_WINDOW);
        if self.narrow_viewport != narrow {
            self.narrow_viewport = narrow;
            self.sync_detail_inset(cx);
            cx.notify();
        }
    }

    fn default_task_for_view(&self, cx: &App) -> Option<Task> {
        if self.view == View::Summary || self.uses_board_layout() || self.uses_popup_details() {
            return None;
        }
        self.rows(i18n::t(cx)).into_iter().find_map(|row| match row {
            Row::Task { task, .. } => Some(task),
            Row::Section { .. } | Row::Day { .. } => None,
        })
    }

    fn sync_layout_selection(&mut self, cx: &mut Context<Self>) {
        let selected = self.default_task_for_view(cx);
        self.selected = selected.as_ref().map(|t| t.id);
        let lists = self.active_lists();
        self.detail.update(cx, |detail, cx| {
            detail.set_lists(lists);
            detail.show(selected, cx);
        });
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.reload_inner(false, true, cx);
    }

    pub(super) fn reload_inner(&mut self, mut switched: bool, notify_notes: bool, cx: &mut Context<Self>) {
        // Sticky notes show data mutations, not view navigation.
        if notify_notes {
            crate::data_version::data_changed(cx.entity_id(), cx);
        }
        self.preferences = self.store.preferences().unwrap_or_default();
        self.sync_detail_inset(cx);
        // Loaded once: the views, counts and filters below are all selections of it.
        let all_tasks = self.store.tasks().unwrap_or_default();
        let today = Self::today();
        let week_start = self.preferences.week_start;
        if let Some(filters) = self.check(self.store.filters()) {
            self.filters = filters
                .into_iter()
                .map(|filter| {
                    let count = all_tasks.iter().filter(|t| filter.matches(t, today, week_start)).count();
                    (filter, count)
                })
                .collect();
            if let View::Filter(id) = self.view
                && !self.filters.iter().any(|(filter, _)| filter.id == id)
            {
                self.view = View::Inbox;
                switched = true;
            }
        }
        if let Some(lists) = self.check(self.store.lists()) {
            self.lists = lists;
        }
        // The current list may have been deleted.
        if let View::List(id) = self.view
            && !self.lists.iter().any(|l| l.id == id)
        {
            self.view = View::Inbox;
            switched = true;
        }
        self.sync_input_placeholder(cx);
        self.completing.retain(|id, pending| {
            pending.view == self.view && !cx.reduce_motion()
                && all_tasks.iter().any(|task| {
                    task.id == *id && !task.is_deleted() && !task.is_wont_do()
                        && task.list_id == pending.before.list_id
                        && match &pending.undo {
                            crate::undo::UndoAction::Reopen { .. } => task.is_completed(),
                            crate::undo::UndoAction::ReopenRepeat { copy, .. } => all_tasks.iter()
                                .any(|copy_task| copy_task.id == *copy && copy_task.is_completed() && !copy_task.is_deleted()),
                            _ => false,
                        }
                })
        });
        self.busy_days = all_tasks
            .iter()
            .filter(|t| !t.is_closed() && !t.is_deleted())
            .filter_map(|t| t.due_date)
            .collect();
        self.show_details = self.view_flag("show_details", false);
        self.kanban = self.view_flag("kanban", false);
        self.timeline = self.view_flag("timeline", false);
        self.kanban_workflow = self.view_flag("kanban_workflow", false);
        self.kanban_column_size = match self.store.setting("kanban:column_size").ok().flatten().as_deref() {
            Some("s") => 0,
            Some("l") => 2,
            _ => 1,
        };
        self.kanban_column_color = self.store.setting("kanban:column_color").ok().flatten().is_some_and(|v| v == "1");
        self.kanban_quick_add = self.store.setting("kanban:quick_add").ok().flatten().is_some_and(|v| v == "1");
        self.timeline_secondary_tz = self
            .store
            .setting("timeline_secondary_tz")
            .ok()
            .flatten()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty() && s.parse::<chrono_tz::Tz>().is_ok());
        self.timeline_color_by = self
            .store
            .setting("timeline_color_by")
            .ok()
            .flatten()
            .and_then(|c| ColorBy::from_code(&c))
            .unwrap_or(ColorBy::List);
        self.timeline_show_weekends = self
            .store
            .setting("timeline_show_weekends")
            .ok()
            .flatten()
            .as_deref()
            != Some("0");
        self.timeline_scale = self
            .store
            .setting(&format!("timeline_scale:{}", self.view_key()))
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .filter(|s| *s < 3)
            .unwrap_or(0);
        self.sections = match self.section_owner() {
            Some(list) => self.store.list_sections(list).unwrap_or_default(),
            None => Vec::new(),
        };
        self.kanban_section_colors = self
            .sections
            .iter()
            .filter_map(|s| {
                self.store
                    .setting(&format!("kanban:section_color:{}", s.id))
                    .ok()
                    .flatten()
                    .filter(|c| !c.is_empty())
                    .map(|c| (s.id, c))
            })
            .collect();
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
        if let Some(counts) = self.check(self.store.counts_from(&all_tasks, today)) {
            self.counts = counts;
        }
        let show_completed = matches!(self.view, View::Today | View::Tomorrow | View::Next7Days)
            && self.view_flag("show_completed", true);
        if let Some(tasks) = self.check(self.store.tasks_in_from_with_today_completed(all_tasks, &self.view, today, show_completed)) {
            self.tasks = tasks;
        }
        self.ensure_kanban_sections(i18n::t(cx));
        if let Some(tags) = self.check(self.store.tags()) {
            self.tags = tags;
        }
        self.reload_today_habits();
        let lists = self.active_lists();
        if switched {
            self.reset_view_transients(cx);
            self.list_state = Self::new_list_state();
            self.list_entries.clear();
            self.list_rows.clear();
            let selected = self.default_task_for_view(cx);
            self.selected = selected.as_ref().map(|t| t.id);
            self.detail.update(cx, |detail, cx| {
                detail.set_lists(lists);
                detail.show(selected, cx);
            });
        } else {
            self.multi_selected.retain(|id| self.tasks.iter().any(|t| t.id == *id));
            if self.multi_selected.len() <= 1 {
                self.multi_selected.clear();
                self.batch_menu = None;
            }
            // Keep completed tasks open after they leave a view like Today. A deleted task is only
            // shown in Trash; otherwise the detail panel returns to its empty state.
            let selected = self.selected.and_then(|id| self.store.task(id).ok().flatten())
                .filter(|task| task.is_deleted() == (self.view == View::Trash));
            self.selected = selected.as_ref().map(|t| t.id);
            self.detail.update(cx, |detail, cx| {
                detail.set_lists(lists);
                detail.refresh(selected, cx);
            });
        }
        cx.notify();
    }

    /// Today and Next 7 Days list the habits due today, unless turned off in the habit settings.
    fn reload_today_habits(&mut self) {
        self.today_habits.clear();
        if !matches!(self.view, View::Today | View::Next7Days)
            || self.store.setting(crate::habit_view::IN_TODAY_SETTING).ok().flatten().as_deref() == Some("0")
        {
            return;
        }
        let today = Self::today();
        let week_start = crate::calendar::week_start();
        let sections = self.store.habit_sections().unwrap_or_default();
        let section_order = fern_core::HabitSection::ordered(&sections);
        let all_records = self.store.all_habit_checkins().unwrap_or_default();
        let mut items: Vec<(usize, i64, fern_core::Habit, bool, f64)> = Vec::new();
        for habit in self.store.habits().unwrap_or_default().into_iter().filter(|h| !h.archived) {
            let recs = all_records.get(&habit.id).map(Vec::as_slice).unwrap_or(&[]);
            if habit.is_active_on(today, recs, week_start) {
                let today_rec = recs.iter().find(|c| c.date == today);
                let amount = today_rec.map_or(0.0, |c| c.value);
                let checked = today_rec.is_some_and(|c| c.is_done());
                let sec_idx = section_order.iter().position(|s| *s == habit.section).unwrap_or(section_order.len());
                items.push((sec_idx, habit.sort_order, habit, checked, amount));
            }
        }
        items.sort_by_key(|(sec_idx, sort_order, _, _, _)| (*sec_idx, *sort_order));
        self.today_habits = items.into_iter().map(|(_, _, h, c, a)| (h, c, a)).collect();
    }


    /// Task ids in the order they are shown, for keyboard navigation.
    fn visible_task_ids(&self, strings: &Strings) -> Vec<Id> {
        if self.kanban && !self.timeline && !matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_)) {
            let mut ids = Vec::new();
            let push_cards = |cards: &[BoardCard], ids: &mut Vec<Id>| {
                ids.extend(self.kanban_visible_rows(cards).into_iter().map(|r| r.task.id));
            };
            for col in self.board_columns(strings) {
                if !col.pinned_cards.is_empty() {
                    if !self.kanban_folded_sub.contains(&format!("{}:pinned", col.key)) {
                        push_cards(&col.pinned_cards, &mut ids);
                    }
                    if !col.open_cards.is_empty() && !self.kanban_folded_sub.contains(&format!("{}:unpinned", col.key)) {
                        push_cards(&col.open_cards, &mut ids);
                    }
                } else {
                    push_cards(&col.open_cards, &mut ids);
                }
                if !col.completed_cards.is_empty() && !self.kanban_folded_sub.contains(&format!("{}:completed", col.key)) {
                    push_cards(&col.completed_cards, &mut ids);
                }
            }
            return ids;
        }
        self.rows(strings)
            .into_iter()
            .filter_map(|row| match row {
                Row::Task { task, .. } => Some(task.id),
                Row::Section { .. } | Row::Day { .. } => None,
            })
            .collect()
    }

    /// The rows of the current view: closed tasks by day, the trash flat, everything else grouped.
    /// Tells the list which entries changed since it last drew: the ones between the unchanged
    /// start and end get measured again, and the scroll position stays put. Habits and
    /// countdowns are read elsewhere, so they are always measured again.
    fn new_list_state() -> ListState {
        ListState::new(0, ListAlignment::Top, px(400.)).measure_all()
    }

    fn sync_list(&mut self, entries: Vec<ListEntry>) {
        let old = &self.list_entries;
        let start = old.iter().zip(&entries).take_while(|(a, b)| a == b).count();
        let end = old[start..].iter().rev().zip(entries[start..].iter().rev()).take_while(|(a, b)| a == b).count();
        if start < old.len() || old.len() != entries.len() {
            let inserted = entries.len() - start - end;
            self.list_state.splice(start..old.len() - end, inserted);
            if inserted > 0 {
                self.list_state.remeasure_items(start..start + inserted);
            }
        }
        for (ix, entry) in entries.iter().enumerate() {
            if !matches!(entry, ListEntry::Row(_)) {
                self.list_state.remeasure_items(ix..ix + 1);
            }
        }
        self.list_rows = entries
            .iter()
            .filter_map(|e| match e {
                ListEntry::Row(row) => Some(row.clone()),
                _ => None,
            })
            .collect();
        self.list_entries = entries;
    }

    fn render_entry(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        // Each entry spans the list's width, as in the column the list replaced.
        let entry_bounds = self.list_entry_bounds.clone();
        div()
            .relative()
            .w_full()
            .px(px(14.))
            .child(
                gpui::canvas(move |area, _, _| {
                    entry_bounds.borrow_mut().insert(ix, area);
                }, |_, _, _, _| {})
                .absolute()
                .size_full(),
            )
            .child(self.render_entry_content(ix, cx))
            .into_any_element()
    }

    fn render_entry_content(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let empty = || div().into_any_element();
        match self.list_entries.get(ix).cloned() {
            Some(ListEntry::NewSection) => match self.new_section.clone() {
                Some(input) => h_flex()
                    .h(px(32.))
                    .mx(px(GUTTER))
                    .px_2()
                    .rounded(px(6.))
                    .bg(cx.theme().colors().surface_background)
                    .child(input)
                    .into_any_element(),
                None => empty(),
            },
            Some(ListEntry::Habits) => self.render_habits(strings, cx).unwrap_or_else(empty),
            Some(ListEntry::Countdowns) => self.render_countdowns(strings, cx).unwrap_or_else(empty),
            Some(ListEntry::Row(Row::Section { section, count, collapsed })) => {
                self.render_section(section, count, collapsed, strings, cx)
            }
            // As the original's, a group folds by its header, the arrow in the gutter.
            Some(ListEntry::Row(Row::Day { label, count, drop })) => {
                let collapsed = self.collapsed_days.contains(&label);
                let key = label.clone();
                let accent = cx.theme().colors().accent;
                let drop_on_header = drop.clone();
                h_flex()
                    .id(SharedString::from(format!("day-{label}")))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when_some(drop_on_header, |el, target| {
                        el.drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.10)))
                            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                                cx.stop_propagation();
                                this.task_drop = None;
                                this.task_drag_origin = None;
                                this.drop_on_column(dragged.id, target.clone(), cx);
                            }))
                    })
                    .child(div().w(px(GUTTER)).flex_none().pl(px(4.)).child(chevron(collapsed)))
                    .child(div().flex_1().min_w_0().child(self.render_group_header(label, count, drop.as_ref(), 36., cx)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.section_rename.is_none() {
                            this.toggle_day(key.clone(), cx);
                        }
                    }))
                    .into_any_element()
            }
            Some(ListEntry::Row(Row::Task { task, depth, has_children, collapsed })) => {
                let tag_colors = std::mem::take(&mut self.tag_colors);
                let row = self.render_task(&task, depth, has_children, collapsed, &tag_colors, Self::today(), strings, cx);
                self.tag_colors = tag_colors;
                row
            }
            None => empty(),
        }
    }

    fn rows(&self, strings: &Strings) -> Vec<Row> {
        task_rows::fold_days(self.rows_with(strings, &self.collapsed_tasks, &self.collapsed_sections), &self.collapsed_days)
    }

    fn tasks_for_rows(&self) -> Vec<Task> {
        let mut tasks = self.tasks.clone();
        let mut pending: Vec<_> = self.completing.values().collect();
        pending.sort_by_key(|pending| pending.index);
        for pending in pending {
            if let crate::undo::UndoAction::ReopenRepeat { copy, .. } = &pending.undo {
                tasks.retain(|task| task.id != *copy);
            }
            if let Some(task) = tasks.iter_mut().find(|task| task.id == pending.before.id) {
                *task = pending.before.clone();
            } else {
                tasks.insert(pending.index.min(tasks.len()), pending.before.clone());
            }
        }
        tasks
    }

    /// Rows with the given collapsed tasks and groups; the kanban board shows everything.
    fn rows_with(&self, strings: &Strings, collapsed_tasks: &HashSet<Id>, collapsed_sections: &HashSet<Section>) -> Vec<Row> {
        let tasks = self.tasks_for_rows();
        let closed = |closed_at: fn(&Task) -> Option<i64>| {
            let today = Self::today();
            let tasks: Vec<Task> = tasks
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
                &tasks,
                collapsed_tasks,
                collapsed_sections,
                self.view != View::Trash,
            ),
            _ => {
                let (sort, group) = self.sort_settings();
                let mut tasks = tasks;
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
    /// Opens a board card's details beside it, as the original's board does.
    fn open_card(&mut self, id: Id, cx: &mut Context<Self>) {
        let Ok(Some(task)) = self.store.task(id) else { return };
        let lists = self.lists.clone();
        self.card_detail.update(cx, |detail, cx| {
            detail.set_lists(lists);
            detail.show(Some(task), cx);
        });
        self.card = Some(id);
        cx.notify();
    }

    fn close_card(&mut self, cx: &mut Context<Self>) {
        if self.card.take().is_some() {
            self.card_detail.update(cx, |detail, cx| detail.show(None, cx));
            cx.notify();
        }
    }

    fn dismiss_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.card_detail.update(cx, |detail, cx| detail.dismiss_popup(window, cx)) {
            self.close_card(cx);
            window.focus(&self.list_focus, cx);
        }
    }

    /// The card's task changed: the board shows it, and the card closes when the task is gone.
    fn card_updated(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        let Some(id) = self.card else { return };
        match self.store.task(id) {
            Ok(Some(task)) if !task.is_deleted() => self.card_detail.update(cx, |detail, cx| detail.refresh(Some(task), cx)),
            _ => self.close_card(cx),
        }
    }

    /// The board's task card: the details, framed, closed by a click elsewhere or Escape.
    fn render_card_details(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        div()
            .id("board-task-card")
            .occlude()
            .w(px(402.))
            .h(px(440.))
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(theme.colors().border)
            .bg(theme.colors().background)
            .shadow_lg()
            // The details' own popovers lie outside the card; a click in them keeps it open.
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if !this.card_detail.read(cx).has_popup() {
                    this.close_card(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DismissTransient, window, cx| {
                this.dismiss_card(window, cx);
            }))
            .child(self.card_detail.clone())
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
        // Matched by name: the overdue group is the one header of a date view with this title.
        let overdue = drop.is_none() && label == strings.group_overdue;
        let section = match drop {
            Some(DropTarget::Section(Some(id))) => Some(*id),
            _ => None,
        };
        let renaming = section.and_then(|id| self.section_rename.as_ref().filter(|(s, _)| *s == id)).map(|(_, input)| input.clone());
        let group = SharedString::from(format!("group-header-{label}"));
        h_flex()
            .id(group.clone())
            .group(group.clone())
            .h(px(height))
            .gap_2()
            .map(|el| match renaming {
                Some(input) => el.child(
                    h_flex().flex_1().min_w_0().h(px(28.)).px_2().rounded(px(6.)).bg(cx.theme().colors().surface_background).child(input),
                ),
                None => el
                    .child(Label::new(label).weight(FontWeight::BOLD).truncate())
                    .child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)),
            })
            .when_some(section, |el, id| {
                el.child(div().flex_1()).child(
                    div()
                        .relative()
                        .opacity(if self.section_menu == Some(id) || crate::touch::active() { 1. } else { 0. })
                        .group_hover(group, |s| s.opacity(1.))
                        .child(
                            IconButton::new(SharedString::from(format!("section-more-{id}")), IconName::Ellipsis)
                                .size(px(22.))
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text(strings.more))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.section_menu = Some(id);
                                    cx.notify();
                                })),
                        )
                        .when(self.section_menu == Some(id), |el| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowRight,
                                popover(cx)
                                    .min_w(px(200.))
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
            // The overdue group's "Postpone" moves its tasks to a later day, as the original's.
            .when(overdue, |el| {
                el.child(div().flex_1()).child(
                    div()
                        .relative()
                        .child(
                            div()
                                .id("postpone")
                                .cursor_pointer()
                                .child(Label::new(strings.postpone).size(LabelSize::Small).color(Color::Accent))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.postpone_menu = true;
                                    cx.notify();
                                })),
                        )
                        .when(self.postpone_menu, |el| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowRight,
                                popover(cx)
                                    .min_w(px(180.))
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.postpone_menu = false;
                                        cx.notify();
                                    }))
                                    .children(fern_core::DefaultDate::ALL.iter().zip(strings.default_dates).map(|(&choice, name)| {
                                        ListItem::new(SharedString::from(format!("postpone-{name}")))
                                            .child(Label::new(name))
                                            .on_click(cx.listener(move |this, _, _, cx| this.postpone_overdue(Some(choice.date(Self::today())), cx)))
                                    }))
                                    .child(Divider::horizontal())
                                    .child(
                                        ListItem::new("postpone-clear")
                                            .start_slot(Icon::new(IconName::CalendarX).size(IconSize::XSmall).color(Color::Muted))
                                            .child(Label::new(strings.clear).color(Color::Muted))
                                            .on_click(cx.listener(|this, _, _, cx| this.postpone_overdue(None, cx))),
                                    ),
                            ))
                        }),
                )
            })
            .into_any_element()
    }

    /// Moves every overdue task in view to `day` (keeping its time), or clears their due dates when `None`.
    pub(super) fn postpone_overdue(&mut self, day: Option<NaiveDate>, cx: &mut Context<Self>) {
        self.postpone_menu = false;
        let today = Self::today();
        let overdue: Vec<_> = self
            .tasks
            .iter()
            .filter(|t| !t.is_closed() && t.due_date.is_some_and(|d| d < today))
            .map(|t| (t.id, t.due_time))
            .collect();
        for (id, time) in overdue {
            let result = self.store.set_due(id, day, day.and(time));
            self.check(result);
        }
        crate::data_version::data_changed(cx.entity_id(), cx);
        self.reload(cx);
    }

    fn start_rename_section(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_section_rename(cx);
        self.section_menu = None;
        self.column_menu = None;
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
        self.section_rename = Some((id, input));
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


    /// Whose sections the view shows: a list's, the Inbox's (`Some(None)`), or none.
    fn section_owner(&self) -> Option<Option<Id>> {
        match self.view {
            View::List(id) => Some(Some(id)),
            View::Inbox => Some(None),
            _ => None,
        }
    }

    fn start_new_section(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.store.set_setting(&format!("kanban:hide_new_section:{}", self.view_key()), "0");
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
            this.new_section_index = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.new_section = Some(input);
        self.header_menu_open = false;
        cx.notify();
    }

    fn commit_new_section(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.new_section.take() else { return };
        let index = self.new_section_index.take();
        let name = input.read(cx).text().trim().to_owned();
        if !name.is_empty()
            && let Some(list) = self.section_owner()
        {
            let result = self.store.insert_list_section(list, &name, index);
            self.check(result);
            self.reload(cx);
        } else {
            cx.notify();
        }
    }

    fn commit_section_rename(&mut self, cx: &mut Context<Self>) {
        let Some((section, input)) = self.section_rename.take() else { return };
        let name = input.read(cx).text().trim().to_owned();
        if !name.is_empty() {
            let result = self.store.rename_list_section(section, &name);
            self.check(result);
            self.reload(cx);
        } else {
            cx.notify();
        }
    }

    fn commit_column_add(&mut self, cx: &mut Context<Self>) {
        let Some((_, target, input)) = self.column_add.take() else { return };
        let text = input.read(cx).text().to_owned();
        if !text.trim().is_empty()
            && let Some(task) = self.add_task(&text, cx)
        {
            self.drop_on_column(task.id, target, cx);
            return;
        }
        cx.notify();
    }


    fn save_preferences(&mut self, change: impl FnOnce(&mut fern_core::Preferences), cx: &mut Context<Self>) {
        let result = self.store.update_preferences(change);
        if let Some(preferences) = self.check(result) {
            self.preferences = preferences;
            self.sync_detail_inset(cx);
        }
        cx.notify();
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
        self.kanban_workflow = self.view_flag("kanban_workflow", false);
        self.header_menu_open = false;
        if name == "show_completed" && matches!(self.view, View::Today | View::Tomorrow | View::Next7Days) {
            if value {
                self.collapsed_sections.remove(&Section::Completed);
            }
            self.reload(cx);
        } else {
            cx.notify();
        }
    }

    /// The header menu's View: list, kanban or timeline, one at a time.
    fn set_layout(&mut self, kanban: bool, timeline: bool, cx: &mut Context<Self>) {
        let was_board = self.uses_board_layout();
        self.completing.clear();
        self.close_card(cx);
        let key = self.view_key();
        for (name, on) in [("kanban", kanban), ("timeline", timeline)] {
            let result = self.store.set_setting(&format!("{name}:{key}"), if on { "1" } else { "0" });
            self.check(result);
        }
        self.kanban = kanban;
        self.timeline = timeline;
        self.timeline_from = Self::today();
        self.header_menu_open = false;
        if kanban && !timeline {
            let strings = i18n::t(cx);
            self.ensure_kanban_sections(strings);
        }
        if was_board != self.uses_board_layout() {
            self.sync_layout_selection(cx);
        }
        cx.notify();
    }


    fn sort_settings(&self) -> (SortBy, GroupBy) {
        let key = self.view_key();
        let get = |what: &str| self.store.setting(&format!("{what}:{key}")).ok().flatten().unwrap_or_default();
        // A tag's view is grouped by list until chosen otherwise, as the original's.
        let group = match get("group") {
            key if key.is_empty() && matches!(self.view, View::Tag(_)) => GroupBy::List,
            // The original's board of a smart list has a column per list.
            key if key.is_empty() && self.kanban && self.view.shows_list_names() => GroupBy::List,
            key => GroupBy::from_key(&key),
        };
        let group = if self.kanban && !self.timeline && group == GroupBy::None {
            if self.view.shows_list_names() { GroupBy::List } else { GroupBy::Custom }
        } else {
            group
        };
        (SortBy::from_key(&get("sort")), group)
    }

    fn set_sort(&mut self, sort: Option<SortBy>, group: Option<GroupBy>, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.view_key();
        let result = sort
            .map(|s| self.store.set_setting(&format!("sort:{key}"), s.key()))
            .into_iter()
            .chain(group.map(|g| self.store.set_setting(&format!("group:{key}"), g.key())))
            .collect::<anyhow::Result<Vec<()>>>();
        self.check(result);
        if group == Some(GroupBy::Custom) {
            let strings = i18n::t(cx);
            self.ensure_kanban_sections(strings);
        }
        self.close_sort_menu(window, cx);
    }

    fn open_sort_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sort_menu.is_none() {
            self.sort_menu_return_focus = window.focused(cx);
            window.focus(&self.sort_menu_focus, cx);
        }
        self.sort_menu = Some(None);
        cx.notify();
    }

    fn close_sort_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sort_menu = None;
        let previous = self.sort_menu_return_focus.take();
        if self.sort_menu_focus.is_focused(window) {
            if let Some(previous) = previous {
                window.focus(&previous, cx);
            } else {
                window.blur(cx);
            }
        }
        cx.notify();
    }

    pub fn dismiss_transient(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.cancel_panel_drag(window, cx) || self.cancel_task_drag(window, cx) {
            return true;
        }
        if self.view == View::Summary
            && self.summary.update(cx, |summary, cx| summary.dismiss_transient(cx))
        {
            return true;
        }
        if self.sort_menu.is_some() {
            self.close_sort_menu(window, cx);
            return true;
        }
        if self.add_tags.take().is_some() {
            if self.add_tag_input.focus_handle(cx).is_focused(window) {
                window.focus(&self.input.focus_handle(cx), cx);
            }
            cx.notify();
            return true;
        }

        // These menus don't move keyboard focus when opened from the list.
        let dismissed_menu = [
            self.context_menu.take().is_some(),
            self.habit_menu.take().is_some(),
            self.list_menu.take().is_some(),
            self.closed_menu.take().is_some(),
            self.batch_menu.take().is_some(),
            self.column_menu.take().is_some(),
            self.section_menu.take().is_some(),
            self.view_options.take().is_some(),
            self.filter_edit.take().is_some(),
            self.list_dialog.take().is_some(),
            self.folder_dialog.take().is_some(),
            self.tag_dialog.take().is_some(),
            self.merge_dialog.take().is_some(),
            self.suggestions_open.take().is_some(),
            std::mem::take(&mut self.header_menu_open),
            std::mem::take(&mut self.confirm_empty_trash),
            self.confirm_delete.take().is_some(),
            self.batch_add_pending.take().is_some(),
            std::mem::take(&mut self.postpone_menu),
            std::mem::take(&mut self.scale_menu_open),
            std::mem::take(&mut self.templates_open),
            std::mem::take(&mut self.input_box_open),
            std::mem::take(&mut self.add_menu_open),
            self.add_date_month.take().is_some(),
        ].into_iter().any(|was_open| was_open);
        if dismissed_menu {
            self.context_side = None;
            self.add_list_side = false;
            self.add_date_row = None;
            self.add_range = None;
            cx.notify();
            return true;
        }

        if self.title_edit.is_some() {
            self.finish_title_edit(false, cx);
            window.focus(&self.list_focus, cx);
            return true;
        }
        if self.section_rename.take().is_some() {
            window.focus(&self.list_focus, cx);
            cx.notify();
            return true;
        }
        if self.card.is_some() {
            self.dismiss_card(window, cx);
            return true;
        }
        if !self.multi_selected.is_empty() {
            self.clear_multi_selection(cx);
            return true;
        }
        self.detail.read(cx).has_popup()
            && self.detail.update(cx, |detail, cx| detail.dismiss_popup(window, cx))
    }


    fn active_task_id(&self) -> Option<Id> {
        self.selected.or(self.card)
    }

    fn select_in_current_layout(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.kanban && !self.timeline && self.selected.is_none() {
            self.open_card(id, cx);
        } else {
            self.select(id, cx);
        }
    }

    fn move_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let ids = self.visible_task_ids(i18n::t(cx));
        if ids.is_empty() {
            return;
        }
        self.multi_selected.clear();
        self.batch_menu = None;
        let next = match self.active_task_id().and_then(|id| ids.iter().position(|i| *i == id)) {
            Some(index) => (index as isize + step).clamp(0, ids.len() as isize - 1) as usize,
            None if step > 0 => 0,
            None => ids.len() - 1,
        };
        let id = ids[next];
        self.selection_anchor = Some(id);
        self.select_in_current_layout(id, cx);
        if let Some(ix) = self.list_entries.iter().position(|e| matches!(e, ListEntry::Row(Row::Task { task, .. }) if task.id == id)) {
            self.list_state.scroll_to_reveal_item(ix);
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    fn toggle_selected(&mut self, _: &ToggleComplete, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_multi_selecting() {
            let ids = self.selected_task_ids();
            let all_completed = !ids.is_empty()
                && ids
                    .iter()
                    .filter_map(|id| self.tasks.iter().find(|t| t.id == *id))
                    .all(Task::is_completed);
            self.run_batch(false, cx, move |store, ids| store.batch_complete(ids, !all_completed));
            return;
        }
        if let Some(id) = self.active_task_id() {
            if self.completing.contains_key(&id) {
                self.toggle(id, false, cx);
            } else if let Some(task) = self.tasks.iter().find(|task| task.id == id) {
                self.toggle(id, !task.is_completed(), cx);
            }
        }
    }

    /// Applies a change to the selected task, for the task edit shortcuts.
    fn edit_selected(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&Store, &Task) -> anyhow::Result<()>) {
        let Some(task) = self.active_task_id().and_then(|id| self.tasks.iter().find(|t| t.id == id)).cloned() else { return };
        let result = change(&self.store, &task);
        self.check(result);
        self.reload(cx);
        self.select_in_current_layout(task.id, cx);
    }

    fn pin_selected(&mut self, _: &PinTask, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_multi_selecting() {
            let ids = self.selected_task_ids();
            let all_pinned = !ids.is_empty()
                && ids
                    .iter()
                    .filter_map(|id| self.tasks.iter().find(|t| t.id == *id))
                    .all(|t| t.pinned_at.is_some());
            self.run_batch(false, cx, move |store, ids| store.batch_set_pinned(ids, !all_pinned));
            return;
        }
        self.edit_selected(cx, |store, task| store.set_pinned(task.id, task.pinned_at.is_none()));
    }

    fn date_selected(&mut self, _: &SetDate, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_some() {
            self.detail.update(cx, |detail, cx| detail.open_due(cx));
        } else if self.card.is_some() {
            self.card_detail.update(cx, |detail, cx| detail.open_due(cx));
        }
    }

    fn set_selected_due(&mut self, days: Option<i64>, cx: &mut Context<Self>) {
        let today = Self::today();
        if self.is_multi_selecting() {
            let date = days.map(|d| today + Duration::days(d));
            self.run_batch(false, cx, move |store, ids| match date {
                Some(d) => store.batch_move_dates(ids, d),
                None => store.batch_set_due(ids, None, None),
            });
            return;
        }
        self.edit_selected(cx, move |store, task| {
            let date = days.map(|d| today + Duration::days(d));
            store.set_due(task.id, date, date.and(task.due_time))
        });
    }

    fn set_selected_priority(&mut self, priority: Priority, cx: &mut Context<Self>) {
        if self.is_multi_selecting() {
            self.run_batch(false, cx, move |store, ids| store.batch_set_priority(ids, priority));
            return;
        }
        self.edit_selected(cx, move |store, task| store.set_priority(task.id, priority));
    }

    fn delete_selected(&mut self, _: &DeleteTask, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_multi_selecting() {
            self.run_batch(true, cx, |store, ids| store.batch_delete(ids));
            return;
        }
        let Some(id) = self.active_task_id() else { return };
        // Keep a selection so repeated presses keep working down the list.
        let ids = self.visible_task_ids(i18n::t(cx));
        let neighbor = ids
            .iter()
            .position(|i| *i == id)
            .and_then(|index| ids.get(index + 1).or_else(|| index.checked_sub(1).and_then(|i| ids.get(i))))
            .copied();
        self.delete(id, cx);
        if let Some(neighbor) = neighbor {
            self.select_in_current_layout(neighbor, cx);
        }
    }

    fn reset_view_transients(&mut self, cx: &mut Context<Self>) {
        self.multi_selected.clear();
        self.selection_anchor = None;
        self.batch_menu = None;
        self.header_menu_open = false;
        self.list_activity = None;
        self.close_card(cx);
        self.context_menu = None;
        self.context_side = None;
        self.closed_menu = None;
        self.sort_menu = None;
        self.sort_menu_return_focus = None;
        self.scale_menu_open = false;
        self.arrange_open = false;
        self.suggestions_open = None;
        self.section_menu = None;
        self.postpone_menu = false;
        self.column_menu = None;
        self.column_add = None;
        self.kanban_folded_sub.clear();
        self.new_section = None;
        self.new_section_index = None;
        self.section_rename = None;
        self.title_edit = None;
        self.kanban_pan = None;
        self.kanban_quick_adds.clear();
        self.clear_task_drag_state();
        self.completing.clear();
    }

    pub fn select_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_viewport(window, cx);
        let switching = self.view != view;
        self.set_view(view, cx);
        if switching && self.view != View::Summary {
            window.focus(&self.list_focus, cx);
        }
    }

    pub fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        self.list_activity = None;
        if self.view != view {
            self.finish_title_edit(true, cx);
            self.reset_view_transients(cx);
            self.view = view;
            self.reload_inner(true, false, cx);
        }
    }

    /// Switches to `View::All` and selects `id` directly without loading the first row's details twice.
    pub fn open_task_in_all(&mut self, id: Id, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        self.list_activity = None;
        if self.view != View::All {
            self.finish_title_edit(true, cx);
            self.reset_view_transients(cx);
            self.view = View::All;
            self.list_state = Self::new_list_state();
            self.list_entries.clear();
            self.selected = Some(id);
            self.reload_inner(false, false, cx);
        } else {
            self.select(id, cx);
        }
    }

    fn update_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        if !matches!(self.view, View::Search(_)) {
            self.set_view(View::Search(query), cx);
            return;
        }
        if self.view == View::Search(query.clone()) {
            return;
        }
        self.finish_title_edit(true, cx);
        self.view = View::Search(query);
        if let Some(tasks) = self.check(self.store.tasks_in(&self.view, Self::today())) {
            self.tasks = tasks;
        }
        self.list_state = Self::new_list_state();
        self.list_entries.clear();
        self.sync_layout_selection(cx);
        cx.notify();
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
            this.list_dialog = None;
            window.focus(&this.list_focus, cx);
            if let DialogEvent::Saved(id) = event {
                crate::data_version::data_changed(cx.entity_id(), cx);
                if this.view != View::List(*id) {
                    this.set_view(View::List(*id), cx);
                    return;
                }
            }
            let was_board = this.uses_board_layout();
            this.reload(cx);
            if was_board != this.uses_board_layout() {
                this.sync_layout_selection(cx);
            }
        })
        .detach();
        self.list_dialog = Some(dialog);
        cx.notify();
    }

    /// The New Folder dialog, or Edit Folder for `folder`.
    fn open_folder_dialog(&mut self, folder: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        self.list_menu = None;
        let dialog = cx.new(|cx| FolderDialog::new(self.store.clone(), folder, window, cx));
        cx.subscribe_in(&dialog, window, |this, _, event: &DialogEvent, window, cx| {
            this.folder_dialog = None;
            window.focus(&this.list_focus, cx);
            if matches!(event, DialogEvent::Saved(_)) {
                crate::data_version::data_changed(cx.entity_id(), cx);
            }
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


    fn clear_task_drag_state(&mut self) -> bool {
        self.task_drop.take().is_some()
            | self.task_drag_origin.take().is_some()
            | self.kanban_hover_col.take().is_some()
    }

    fn cancel_task_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let had_state = self.clear_task_drag_state();
        if had_state || cx.has_active_drag() {
            cx.stop_active_drag(window);
            cx.notify();
            return true;
        }
        false
    }

    fn open_context_menu(
        &mut self,
        id: Id,
        position: gpui::Point<Pixels>,
        select_task: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.list_focus, cx);
        if select_task {
            self.select(id, cx);
        }
        let nesting = task_menu::TaskNesting::of(&self.store, id);
        self.context_menu = Some((id, position, nesting));
        self.context_side = None;
        cx.notify();
    }

    fn render_drop_indicator(
        plan: &task_rows::TaskDropPlan,
        gutter: f32,
        accent: Hsla,
        dot_bg: Hsla,
    ) -> AnyElement {
        let left = px(plan.target_level as f32 * INDENT + gutter);
        div()
            .absolute()
            .left(left)
            .right_0()
            .when(!plan.after, |s| s.top(px(-1.)))
            .when(plan.after, |s| s.bottom(px(-1.)))
            .h(px(2.))
            .bg(accent)
            .child(
                div()
                    .absolute()
                    .left(px(-3.))
                    .top(px(-2.))
                    .size(px(6.))
                    .rounded_full()
                    .border_2()
                    .border_color(accent)
                    .bg(dot_bg),
            )
            .into_any_element()
    }

    fn bind_task_list_actions<E: InteractiveElement>(&self, el: E, cx: &mut Context<Self>) -> E {
        el.key_context("TaskList")
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_all_visible))
            .on_action(cx.listener(|this, _: &ExtendSelectionNext, _, cx| this.extend_selection(1, cx)))
            .on_action(cx.listener(|this, _: &ExtendSelectionPrevious, _, cx| this.extend_selection(-1, cx)))
            .on_action(cx.listener(Self::toggle_selected))
            .on_action(cx.listener(Self::toggle_in_progress_selected))
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
            .on_action(cx.listener(Self::toggle_all_subtasks))
            .on_action(cx.listener(Self::add_subtask_selected))
            .on_action(cx.listener(|this, _: &crate::actions::Undo, _, cx| this.apply_undo(cx)))
            .on_action(cx.listener(|this, _: &crate::actions::Redo, _, cx| this.apply_redo(cx)))
    }

    fn render_empty_state(&self, strings: &'static Strings, theme: &theme::Theme, px_fallback: Pixels) -> AnyElement {
        match &self.view {
            View::Day(_) => v_flex()
                .pt(px(160.))
                .items_center()
                .gap_1()
                .child(Icon::new(IconName::CalendarCheck).size(IconSize::Medium).color(Color::Muted))
                .child(Label::new(strings.free_day))
                .child(Label::new(strings.free_day_hint).size(LabelSize::Small).color(Color::Muted))
                .into_any_element(),
            View::Tomorrow => v_flex()
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
                .child(Label::new(strings.plan_ahead).size(LabelSize::Small).color(Color::Muted))
                .into_any_element(),
            other => {
                let (icon, message) = match other {
                    View::Inbox => (IconName::Inbox, strings.no_tasks),
                    View::Today => (IconName::Sun, strings.no_tasks),
                    View::Next7Days => (IconName::CalendarDays, strings.no_tasks),
                    View::Completed => (IconName::SquareCheck, strings.no_completed_tasks),
                    View::WontDo => (IconName::SquareX, strings.no_wont_do),
                    View::Trash => (IconName::Trash, strings.trash_is_empty),
                    View::Search(query) if query.trim().is_empty() => (IconName::Search, strings.search_hint),
                    View::Search(_) => (IconName::Search, strings.nothing_found),
                    View::Tag(_) => (IconName::Tag, strings.no_tasks),
                    View::Filter(_) => (IconName::Layers, strings.no_tasks),
                    _ => (IconName::ListChecks, strings.no_tasks),
                };
                v_flex()
                    .w_full()
                    .px(px_fallback)
                    .pt(px(120.))
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(56.))
                            .rounded_full()
                            .bg(theme.colors().surface_background)
                            .border_1()
                            .border_color(theme.colors().border_variant)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(icon).size(IconSize::Large).color(Color::Muted)),
                    )
                    .child(Label::new(message).size(LabelSize::Small).color(Color::Muted))
                    .into_any_element()
            }
        }
    }


    fn toggle_task_collapsed(&mut self, id: Id, cx: &mut Context<Self>) {
        if !self.collapsed_tasks.remove(&id) {
            self.collapsed_tasks.insert(id);
        }
        cx.notify();
    }

    fn toggle_day(&mut self, label: String, cx: &mut Context<Self>) {
        if !self.collapsed_days.remove(&label) {
            self.collapsed_days.insert(label);
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

    /// Follows a dragged panel edge. `event.bounds` is this view's.
    fn resize_panel(&mut self, event: &gpui::DragMoveEvent<DraggedEdge>, _: &mut Window, cx: &mut Context<Self>) {
        let panel = event.drag(cx).0;
        let bounds = event.bounds;
        let x = event.event.position.x;
        let prev = (self.resizing, self.show_lists, self.lists_width, self.detail_width);
        self.resizing = Some(panel);
        match panel {
            Panel::Lists => {
                let width = f32::from(x - bounds.left());
                // Dragged almost shut, the column hides; dragged back out, it shows again.
                self.show_lists = width >= lists_panel::COLLAPSE_BELOW;
                if self.show_lists {
                    self.lists_width = width.clamp(lists_panel::MIN_WIDTH, lists_panel::MAX_WIDTH);
                }
            }
            Panel::Detail => {
                let width = f32::from(bounds.right() - x);
                self.detail_width = width.clamp(DETAIL_MIN_WIDTH, self.max_detail_width(f32::from(bounds.size.width)).max(DETAIL_MIN_WIDTH));
            }
        }
        let widths_changed = (self.show_lists, self.lists_width, self.detail_width) != (prev.1, prev.2, prev.3);
        if widths_changed {
            self.schedule_save_widths(cx);
        }
        if widths_changed || self.resizing != prev.0 {
            cx.notify();
        }
    }

    /// The widest the details may be in a view `view_width` wide, leaving the task list its room.
    fn max_detail_width(&self, view_width: f32) -> f32 {
        let lists = if self.show_lists { self.lists_width } else { 0. };
        view_width - lists - LIST_MIN_WIDTH
    }

    /// A double click on an edge: the panel's default width.
    fn reset_panel_width(&mut self, panel: Panel, cx: &mut Context<Self>) {
        match panel {
            Panel::Lists => self.lists_width = lists_panel::WIDTH,
            Panel::Detail => self.detail_width = DETAIL_WIDTH,
        }
        self.schedule_save_widths(cx);
        cx.notify();
    }

    fn schedule_save_widths(&mut self, cx: &mut Context<Self>) {
        self.save_widths = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
            this.update(cx, |this, _| {
                for (key, width) in [("lists_width", this.lists_width), ("detail_width", this.detail_width)] {
                    if let Err(err) = this.store.set_setting(key, &width.round().to_string()) {
                        eprintln!("failed to save the panel width: {err:#}");
                    }
                }
            })
            .ok();
        }));
    }

    /// An edge's handle; `panel` takes its width from where the edge is dropped.
    fn resize_handle(&self, panel: Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let dragged = self.resizing == Some(panel) && cx.has_active_drag();
        resize::handle(
            panel,
            dragged,
            move |_, cx| {
                this.update(cx, |this, cx| this.reset_panel_width(panel, cx)).ok();
            },
            cx,
        )
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
        self.multi_selected.clear();
        self.batch_menu = None;
        self.selection_anchor = Some(id);
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
        if let Some(pending) = self.completing.get(&id) {
            // A second click cancels this completion, including a repeating occurrence.
            // Repeated complete commands must not advance the recurrence again.
            if completed { return; }
            let action = pending.undo.clone();
            if self.check(action.apply(&self.store)).is_some() {
                self.completing.remove(&id);
                if self.undo.as_ref().is_some_and(|(undo, _, _)| *undo == action) {
                    self.undo = None;
                }
            }
            self.reload(cx);
            return;
        }
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
                    crate::completion_mark::record(id, cx);
                    desktop_common::sound::play_complete(&self.store);
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
        let completed = match &action {
            crate::undo::UndoAction::Reopen { id, .. } | crate::undo::UndoAction::ReopenRepeat { id, .. } => Some(*id),
            _ => None,
        };
        if !cx.reduce_motion()
            && let Some(id) = completed
            && let Some((index, before)) = self.tasks_for_rows().into_iter().enumerate()
                .find(|(_, task)| task.id == id && !task.is_closed())
        {
            let timer = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(crate::completion_mark::SETTLE_DURATION).await;
                this.update(cx, |this, cx| {
                    this.completing.remove(&id);
                    cx.notify();
                }).ok();
            });
            self.completing.insert(id, CompletingTask {
                before, index, view: self.view.clone(), undo: action.clone(), _timer: timer,
            });
        }
        let expected = action.clone();
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
            this.update(cx, |this, cx| {
                if this.undo.as_ref().is_some_and(|(a, _, _)| *a == expected) {
                    this.undo = None;
                    cx.notify();
                }
            })
            .ok();
        });
        self.redo = None;
        self.undo = Some((action, label, timer));
        cx.notify();
    }

    fn apply_undo(&mut self, cx: &mut Context<Self>) {
        if let Some((action, label, _)) = self.undo.take() {
            let result = action.apply(&self.store);
            if self.check(result).is_some() {
                self.completing.retain(|_, pending| pending.undo != action);
                self.redo = Some((action, label));
            }
            self.reload(cx);
        }
    }

    fn apply_redo(&mut self, cx: &mut Context<Self>) {
        if let Some((action, label)) = self.redo.take() {
            let result = action.redo(&self.store);
            if let Some(Some(new_undo)) = self.check(result) {
                self.offer_undo(new_undo, label, cx);
            }
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
        self.clear_task_drag_state();
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

    /// Drops a dragged task right before `target`, inheriting `target`'s parent (used by dev-state `move=A>B`).
    #[cfg(debug_assertions)]
    fn drop_task(&mut self, dragged: Id, target: Id, cx: &mut Context<Self>) {
        let parent = self.store.task(target).ok().flatten().and_then(|t| t.parent_id);
        let result = self.store.move_task_in_tree(dragged, parent, Some((target, false)));
        self.check(result);
        self.reload(cx);
    }

    /// Whether the active list view renders subtasks nested under their parents.
    fn allows_tree_nesting(&self) -> bool {
        !matches!(self.view, View::Completed | View::WontDo | View::Trash | View::Next7Days)
            && self.sort_settings().1 == GroupBy::Custom
    }

    fn move_task_drag(&mut self, event: &gpui::DragMoveEvent<DraggedTask>, _: &mut Window, cx: &mut Context<Self>) {
        if self.view == View::Trash || self.kanban || self.timeline {
            return;
        }
        let pos = event.event.position;
        if !event.bounds.contains(&pos) {
            if self.task_drop.take().is_some() {
                cx.notify();
            }
            return;
        }
        let dragged_id = event.drag(cx).id;
        let start_x = match self.task_drag_origin {
            Some((id, x)) if id == dragged_id => x,
            _ => {
                self.task_drag_origin = Some((dragged_id, pos.x));
                pos.x
            }
        };
        let allow_nesting = self.allows_tree_nesting();
        let bounds_map = self.list_entry_bounds.borrow();

        let mut hit: Option<(usize, gpui::Bounds<Pixels>)> = None;
        let mut bottommost_row_y: Option<Pixels> = None;
        for (&ix, &bounds) in bounds_map.iter() {
            if !matches!(self.list_entries.get(ix), Some(ListEntry::Row(_))) {
                continue;
            }
            bottommost_row_y = Some(bottommost_row_y.map_or(bounds.bottom(), |b| b.max(bounds.bottom())));
            if pos.y >= bounds.top() && pos.y <= bounds.bottom() {
                hit = Some((ix, bounds));
            }
        }
        drop(bounds_map);

        let plan = if let Some((ix, bounds)) = hit {
            match self.list_entries.get(ix) {
                Some(ListEntry::Row(Row::Task { task, .. })) => {
                    let mid_y = bounds.top() + bounds.size.height / 2.;
                    let after = pos.y >= mid_y;
                    let dx = f32::from(pos.x - start_x);
                    // Subtract `render_entry`'s 14 px horizontal padding so `rel_x = 0` is the row's left edge.
                    let rel_x = f32::from(pos.x - bounds.left()) - 14.0;
                    task_rows::plan_task_drop(
                        &self.list_rows,
                        dragged_id,
                        task.id,
                        after,
                        dx,
                        rel_x,
                        INDENT,
                        GUTTER,
                        allow_nesting,
                    )
                }
                _ => None,
            }
        } else if bottommost_row_y.is_some_and(|bottom| pos.y > bottom) {
            task_rows::plan_task_drop_at_end(&self.list_rows, dragged_id)
        } else {
            None
        };

        if self.task_drop != plan {
            self.task_drop = plan;
            cx.notify();
        }
    }

    fn drop_task_in_list(&mut self, dragged: &DraggedTask, _: &mut Window, cx: &mut Context<Self>) {
        let plan = self.task_drop.take();
        self.task_drag_origin = None;
        if let Some(plan) = plan {
            cx.stop_propagation();
            self.apply_task_drop(dragged.id, plan, cx);
        } else {
            cx.notify();
        }
    }

    fn apply_task_drop(&mut self, dragged: Id, plan: task_rows::TaskDropPlan, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<()> {
            let Some(dragged_task) = self.store.task(dragged)? else { return Ok(()) };
            if let Some(group_drop) = plan.group_drop {
                match group_drop {
                    DropTarget::Section(sec) => {
                        if dragged_task.is_completed() {
                            self.store.set_completed(dragged, false)?;
                        }
                        if dragged_task.section_id != sec {
                            self.store.set_section(dragged, sec)?;
                        }
                    }
                    DropTarget::Todo => {
                        if dragged_task.is_completed() {
                            self.store.set_completed(dragged, false)?;
                        }
                        if dragged_task.is_in_progress() {
                            self.store.set_in_progress(dragged, false)?;
                        }
                    }
                    DropTarget::InProgress => {
                        if dragged_task.is_completed() {
                            self.store.set_completed(dragged, false)?;
                        }
                        if !dragged_task.is_in_progress() {
                            self.store.set_in_progress(dragged, true)?;
                        }
                    }
                    DropTarget::Complete => {
                        if !dragged_task.is_completed() {
                            self.store.set_completed(dragged, true)?;
                        }
                    }
                    DropTarget::Kind(kind) => {
                        if dragged_task.is_completed() {
                            self.store.set_completed(dragged, false)?;
                        }
                        if dragged_task.kind != kind {
                            self.store.set_kind(dragged, kind)?;
                        }
                    }
                    other => {
                        if dragged_task.is_completed() {
                            self.store.set_completed(dragged, false)?;
                        }
                        self.apply_column_value(dragged, other)?;
                    }
                }
            } else if dragged_task.is_completed()
                && self.store.task(plan.target_id)?.is_some_and(|t| !t.is_completed())
            {
                self.store.set_completed(dragged, false)?;
            }
            self.store.move_task_in_tree(dragged, plan.new_parent, plan.order_target)?;
            Ok(())
        })();
        if self.check(result).is_some()
            && let Some(parent_id) = plan.expand_parent
        {
            self.collapsed_tasks.remove(&parent_id);
        }
        self.reload(cx);
    }

    /// Parents in the current view that have at least one visible subtask.
    fn parent_task_ids(&self, strings: &Strings) -> Vec<Id> {
        self.rows_with(strings, &HashSet::new(), &HashSet::new())
            .into_iter()
            .filter_map(|row| match row {
                Row::Task { task, has_children: true, .. } => Some(task.id),
                _ => None,
            })
            .collect()
    }

    fn toggle_all_subtasks_now(&mut self, cx: &mut Context<Self>) {
        let parents = self.parent_task_ids(i18n::t(cx));
        if parents.is_empty() {
            return;
        }
        let any_expanded = parents.iter().any(|id| !self.collapsed_tasks.contains(id));
        if any_expanded {
            self.collapsed_tasks.extend(parents);
        } else {
            for id in parents {
                self.collapsed_tasks.remove(&id);
            }
        }
        cx.notify();
    }

    fn toggle_all_subtasks(&mut self, _: &ToggleAllSubtasks, _: &mut Window, cx: &mut Context<Self>) {
        if self.title_edit.is_some() {
            return;
        }
        self.toggle_all_subtasks_now(cx);
    }

    fn add_subtask_selected(&mut self, _: &AddSubtask, window: &mut Window, cx: &mut Context<Self>) {
        if self.title_edit.is_some() {
            return;
        }
        if let Some(id) = self.selected {
            self.start_detail(id, "subtask", window, cx);
        }
    }

    fn restore(&mut self, id: Id, cx: &mut Context<Self>) {
        let result = self.store.restore_task(id);
        self.check(result);
        self.reload(cx);
    }

    fn request_empty_trash(&mut self, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        if self.tasks.is_empty() {
            return;
        }
        self.confirm_empty_trash = true;
        cx.notify();
    }

    fn empty_trash(&mut self, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        self.confirm_empty_trash = false;
        let result = self.store.empty_trash();
        self.check(result);
        self.reload(cx);
    }

    fn execute_confirmed_delete(&mut self, target: ConfirmDeleteTarget, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        match target {
            ConfirmDeleteTarget::List(id, _) => {
                let result = self.store.delete_list(id);
                self.check(result);
                if self.view == View::List(id) {
                    crate::data_version::data_changed(cx.entity_id(), cx);
                    self.set_view(View::Inbox, cx);
                    return;
                }
            }
            ConfirmDeleteTarget::Tag(tag) => {
                let result = self.store.delete_tag(&tag);
                self.check(result);
                if self.view == View::Tag(tag) {
                    crate::data_version::data_changed(cx.entity_id(), cx);
                    self.set_view(View::Inbox, cx);
                    return;
                }
            }
            ConfirmDeleteTarget::Filter(id, _) => {
                let result = self.store.delete_filter(id);
                if self.check(result).is_some() && self.view == View::Filter(id) {
                    crate::data_version::data_changed(cx.entity_id(), cx);
                    self.set_view(View::Inbox, cx);
                    return;
                }
            }
            ConfirmDeleteTarget::Section(id, _) => {
                let result = self.store.delete_list_section(id);
                self.check(result);
            }
            ConfirmDeleteTarget::BatchPurge(_) => {
                self.run_batch(true, cx, |store, ids| store.batch_purge(ids));
                return;
            }
        }
        crate::data_version::data_changed(cx.entity_id(), cx);
        self.reload(cx);
    }

    fn confirm_batch_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lines) = self.batch_add_pending.take() else {
            return;
        };
        let mut shared_draft = self.add_draft.clone();
        shared_draft.files.clear();
        for line in lines {
            self.add_draft = shared_draft.clone();
            self.submit_new_task(&line, window, cx);
        }
        self.add_draft = Default::default();
        cx.notify();
    }

    fn paste_batch_as_one(&mut self, cx: &mut Context<Self>) {
        let Some(lines) = self.batch_add_pending.take() else {
            return;
        };
        let joined = lines.join(" ");
        self.input.update(cx, |input, cx| {
            let existing = input.text().to_owned();
            let combined = if existing.is_empty() {
                joined
            } else {
                format!("{existing} {joined}")
            };
            input.set_text(combined, cx);
        });
        self.update_quick_add_hint(cx);
        cx.notify();
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
        let accent = cx.theme().colors().accent;
        let drop_target = match section {
            Section::Tasks => DropTarget::Kind(Kind::Task),
            Section::Notes => DropTarget::Kind(Kind::Note),
            Section::Completed => DropTarget::Complete,
        };
        h_flex()
            .id(SharedString::from(format!("group-{section:?}")))
            .h(px(36.))
            // As the original's, the arrow sits 7 px in from the rows' edge.
            .pl(px(7.))
            .gap_1()
            .rounded(px(6.))
            .cursor_pointer()
            .when(self.view != View::Trash, |el| {
                el.drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.10)))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                        cx.stop_propagation();
                        this.task_drop = None;
                        this.task_drag_origin = None;
                        this.drop_on_column(dragged.id, drop_target.clone(), cx);
                    }))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
            .child(chevron(collapsed))
            .child(Label::new(section.title(strings)).weight(FontWeight::BOLD))
            .child(div().pl_1().child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)))
            .into_any_element()
    }

    fn start_title_edit(&mut self, id: Id, click: Option<gpui::Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_title_edit(true, cx);
        let Some(task) = self.tasks.iter().find(|task| task.id == id && !task.is_deleted()) else { return };
        let title = task.title.clone();
        if !self.uses_board_layout() {
            self.select(id, cx);
        }
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "", cx);
            input.set_text(title, cx);
            if let Some(position) = click {
                input.place_cursor_at(position, cx);
            }
            input
        });
        cx.subscribe_in(&input, window, |this, _, _: &Submitted, window, cx| {
            this.finish_title_edit(true, cx);
            window.focus(&this.list_focus, cx);
        }).detach();
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            this.finish_title_edit(false, cx);
            window.focus(&this.list_focus, cx);
        }).detach();
        let focus = input.focus_handle(cx);
        let blur = cx.on_blur(&focus, window, |this, _, cx| this.finish_title_edit(true, cx));
        if click.is_some() {
            input.update(cx, |input, cx| input.open_touch_keyboard(window, cx));
        }
        self.title_edit = Some((id, input, blur));
        window.focus(&focus, cx);
        cx.notify();
    }

    fn finish_title_edit(&mut self, save: bool, cx: &mut Context<Self>) {
        let Some((id, input, _blur)) = self.title_edit.take() else { return };
        let title = input.read(cx).text().trim();
        if save && !title.is_empty() && self.tasks.iter().any(|task| task.id == id && task.title != title) {
            let result = self.store.set_title(id, title);
            if self.check(result).is_some() {
                self.reload(cx);
            }
        }
        cx.notify();
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
        let title_input = self.title_edit.as_ref().filter(|(editing, _, _)| *editing == id).map(|(_, input, _)| input.clone());
        let done = task.is_completed() || self.completing.contains_key(&id);
        let details = self.show_details && !task.notes.trim().is_empty();
        let wont_do = task.is_wont_do();
        let closed_view = matches!(self.view, View::Completed | View::WontDo);
        let selected = self.is_task_selected(id);
        let group = format!("task-{id}");

        let marker = if task.kind == Kind::Note {
            Icon::new(IconName::FileText).into_any_element()
        } else {
            Checkbox::new("marker")
                .toggle_state(done)
                .animate_completion(crate::completion_mark::started(id, cx))
                .in_progress(task.is_in_progress())
                .dismissed(wont_do)
                .lines(task.checklist.is_some())
                .tint(task_rows::priority_color(task.priority, &theme))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if cx.has_active_drag() {
                        return;
                    }
                    cx.stop_propagation();
                    this.toggle(id, !done && !wont_do, cx);
                }))
                .into_any_element()
        };
        // A finger gets a wider target around the box, which does the same.
        let marker = if crate::touch::active() && task.kind != Kind::Note {
            div()
                .id("marker-target")
                .p(px(8.))
                .m(px(-8.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if cx.has_active_drag() {
                        return;
                    }
                    cx.stop_propagation();
                    this.toggle(id, !done && !wont_do, cx);
                }))
                .child(marker)
                .into_any_element()
        } else {
            marker
        };

        let due = task.due_date.map(|date| {
            if matches!(self.view, View::Completed | View::WontDo) {
                (task_rows::closed_due_label(date, today, strings), false)
            } else {
                task_rows::row_due_label(date, task.due_time, today, strings, self.preferences.countdown_dates)
            }
        });

        let draggable = self.view != View::Trash && title_input.is_none();
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let accent = theme.colors().accent;
        let dragging_active = cx.has_active_drag();
        let is_being_dragged = dragging_active && self.task_drag_origin.is_some_and(|(d, _)| d == id);
        let drop_indicator = dragging_active
            .then(|| self.task_drop.as_ref().filter(|plan| plan.target_id == id).cloned())
            .flatten();

        // The separator starts at the checkbox; the gutter to its left holds the chevron.
        div()
            .id(ElementId::Uuid(id))
            .debug_selector(move || format!("task-row-{id}"))
            .group(group.clone())
            .relative()
            .when(draggable, |el| {
                let view = cx.entity().downgrade();
                el.on_drag(dragged, move |dragged, cursor_offset, window, cx| {
                    let dragged_id = dragged.id;
                    let fallback_x = window.mouse_position().x;
                    view.update(cx, |this, cx| {
                        let start_x = this
                            .list_entries
                            .iter()
                            .position(|e| matches!(e, ListEntry::Row(Row::Task { task, .. }) if task.id == dragged_id))
                            .and_then(|ix| this.list_entry_bounds.borrow().get(&ix).copied())
                            .map_or(fallback_x, |area| area.left() + px(14.) + cursor_offset.x);
                        this.task_drag_origin = Some((dragged_id, start_x));
                        this.task_drop = None;
                        cx.notify();
                    })
                    .ok();
                    cx.new(|_| dragged.clone())
                })
            })
            .when(is_being_dragged, |el| el.opacity(0.45))
            .when_some(drop_indicator, |el, plan| {
                el.child(Self::render_drop_indicator(&plan, GUTTER, accent, theme.colors().background))
            })
            .h(px(if details { ROW_HEIGHT + 14. } else { ROW_HEIGHT }))
            .flex()
            .pl(px(depth as f32 * INDENT))
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.colors().ghost_element_selected))
            .when(!selected, |el| el.hover(|s| s.bg(theme.colors().ghost_element_hover)))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                if cx.has_active_drag()
                    || this.title_edit.as_ref().is_some_and(|(editing, _, _)| *editing == id)
                {
                    return;
                }
                this.handle_row_click(id, event.modifiers(), window, cx);
            }))
            // The original's right-click menu, at the pointer; the task is selected too.
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                this.open_context_menu(id, event.position, true, window, cx);
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
                            if cx.has_active_drag() {
                                return;
                            }
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
                    // Reserve the menu button's width and gap even when it is hidden.
                    .pr(px(36.))
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(marker)
                    .when(task.pinned_at.is_some() && !closed_view, |el| {
                        el.child(Icon::new(IconName::Pin).size(IconSize::XSmall).color(Color::Accent))
                    })
                    .child(
                        v_flex().flex_1().min_w_0().debug_selector(|| format!("task-title-field-{id}")).child(if let Some(input) = title_input {
                            input.into_any_element()
                        } else {
                            div()
                                .id("task-title")
                                .max_w_full()
                                .min_w_0()
                                .self_start()
                                .child(Label::new(if task.title.is_empty() {
                                    SharedString::new_static(strings.untitled)
                                } else {
                                    task.title.clone().into()
                                })
                                // As the original: closed tasks are muted, not struck through.
                                .color(if done || wont_do || task.title.is_empty() { Color::Muted } else { Color::Default })
                                .truncate())
                                .debug_selector(|| format!("task-title-{id}"))
                                .when(!task.is_deleted(), |el| {
                                    el.cursor(gpui::CursorStyle::IBeam).on_click(cx.listener(
                                        move |this, event: &gpui::ClickEvent, window, cx| {
                                            if cx.has_active_drag() {
                                                return;
                                            }
                                            cx.stop_propagation();
                                            if event.modifiers().shift || event.modifiers().secondary() {
                                                this.handle_row_click(id, event.modifiers(), window, cx);
                                                return;
                                            }
                                            this.start_title_edit(id, Some(event.position()), window, cx);
                                        },
                                    ))
                                })
                                .into_any_element()
                        })
                        // "Show Details": the first line of the notes under the title.
                        .when_some(details.then(|| task.notes.lines().find(|l| !l.trim().is_empty())).flatten(), |el, line| {
                            el.child(Label::new(line.trim().to_owned()).size(LabelSize::XSmall).color(Color::Muted).truncate())
                        }),
                    )
                    // A closed task's tags are muted with it, as the original's.
                    .children(task.tags.iter().map(|tag| {
                        let color = tag_colors.get(tag).copied().unwrap_or(theme.colors().text_muted);
                        div().flex_none().when(closed_view, |el| el.opacity(0.45)).child(Chip::new(tag.clone(), color))
                    }))
                    // As the original's, a view across lists names each task's list after its tags.
                    .when(self.view.shows_list_names(), |el| {
                        let list = task
                            .list_id
                            .and_then(|id| self.lists.iter().find(|l| l.id == id))
                            .map_or(SharedString::new_static(strings.inbox), |l| l.name.clone().into());
                        el.child(div().flex_none().child(Label::new(list).size(LabelSize::Small).color(Color::Muted)))
                    })
                    .when_some(task.location.as_deref().filter(|s| !s.trim().is_empty()), |el, loc| {
                        el.child(
                            h_flex()
                                .flex_none()
                                .max_w(px(120.))
                                .gap_1()
                                .child(Icon::new(IconName::MapPin).size(IconSize::XSmall).color(Color::Muted))
                                .child(Label::new(loc.trim().to_owned()).size(LabelSize::XSmall).color(Color::Muted).truncate()),
                        )
                    })
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
                    .when_some(due, |el, (label, overdue)| {
                        el.when(!task.reminders.is_empty() && !closed_view, |el| {
                            el.child(
                                Icon::new(IconName::AlarmClock)
                                    .size(IconSize::XSmall)
                                    .color(if task.constant_reminder { Color::Accent } else { Color::Muted }),
                            )
                        })
                        .when(task.repeat.is_some(), |el| {
                            el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                        })
                        .when(!closed_view, |el| {
                            el.when_some(
                                crate::due_picker::short_timezone_label(task.timezone.as_deref(), task.is_floating, strings),
                                |el, tz| el.child(Label::new(tz).size(LabelSize::XSmall).color(Color::Muted)),
                            )
                        })
                        .child({
                            let due_label = Label::new(label)
                                .size(LabelSize::Small)
                                .color(if closed_view {
                                    Color::Muted
                                } else if overdue && !done {
                                    Color::Error
                                } else {
                                    Color::Accent
                                });
                            if closed_view {
                                due_label.into_any_element()
                            } else {
                                let due_tip = match (self.preferences.countdown_dates, task.due_date) {
                                    (true, Some(d)) => format!("{} · {}", crate::calendar::format_date(d, strings), strings.show_countdown_mode),
                                    _ => strings.show_countdown_mode.to_owned(),
                                };
                                div()
                                    .id(SharedString::from(format!("due-toggle-{id}")))
                                    .px_1()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.colors().element_hover))
                                    .tooltip(Tooltip::text(due_tip))
                                    .child(due_label)
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.save_preferences(|p| p.countdown_dates = !p.countdown_dates, cx);
                                    }))
                                    .into_any_element()
                            }
                        })
                    })
                    // As the original's, a note with text shows a small page at the row's end.
                    .when(task.kind == fern_core::Kind::Note && !task.notes.trim().is_empty(), |el| {
                        el.child(Icon::new(IconName::FileText).size(IconSize::XSmall).color(Color::Muted))
                    })
                    // And a task with subtasks a small tree + progress count, open or folded.
                    .when(has_children, |el| {
                        let subs: Vec<&Task> = self
                            .tasks
                            .iter()
                            .filter(|t| t.parent_id == Some(id) && !t.is_deleted() && !t.is_wont_do())
                            .collect();
                        let done_subs = subs.iter().filter(|t| t.is_completed()).count();
                        let total_subs = subs.len();
                        el.child(
                            h_flex()
                                .gap_0p5()
                                .child(Icon::new(IconName::ListTree).size(IconSize::XSmall).color(Color::Muted))
                                .when(total_subs > 0, |row| {
                                    row.child(
                                        Label::new(format!("{done_subs}/{total_subs}"))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                }),
                        )
                    })
                    .when(self.view == View::Trash, |el| {
                        el.child(
                            h_flex()
                                .id(SharedString::from(format!("trash-actions-{id}")))
                                .flex_none()
                                .gap_1()
                                .opacity(if crate::touch::active() { 1. } else { 0. })
                                .group_hover(group.clone(), |s| s.opacity(1.))
                                .child(
                                    IconButton::new(SharedString::from(format!("restore-button-{id}")), IconName::RotateCcw)
                                        .size(px(20.))
                                        .icon_size(IconSize::Small)
                                        .icon_color(Color::Muted)
                                        .hover_icon_color(Color::Icon)
                                        .tooltip(Tooltip::text(strings.restore))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.restore(id, cx);
                                        })),
                                )
                                .child(
                                    IconButton::new(SharedString::from(format!("purge-button-{id}")), IconName::Trash)
                                        .size(px(20.))
                                        .icon_size(IconSize::Small)
                                        .icon_color(Color::Muted)
                                        .hover_icon_color(theme.status().error)
                                        .tooltip(Tooltip::text(strings.delete))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.delete(id, cx);
                                        })),
                                ),
                        )
                    }),
            )
            // Keep the menu button inside the row, including in Trash and while editing.
            .child({
                let more_group = SharedString::from(format!("row-more-{id}"));
                div()
                    .id("row-more")
                    .group(more_group.clone())
                    .absolute()
                    .right(px(6.))
                    .top_0()
                    .w(px(20.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(0.)
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .child(
                        Icon::new(IconName::Ellipsis)
                            .size(IconSize::XSmall)
                            .color(Color::Muted)
                            .group_hover_color(more_group, Color::Icon),
                    )
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_context_menu(id, event.position, true, window, cx);
                    }))
            })
            // And the drag handle left of a hovered row, as the original's.
            .when(draggable, |el| {
                let drag_group = SharedString::from(format!("row-drag-{id}"));
                el.child(
                    div()
                        .group(drag_group.clone())
                        .absolute()
                        .left(px(-13.))
                        .top_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .child(
                            Icon::new(IconName::Menu)
                                .size(IconSize::XSmall)
                                .color(Color::Muted)
                                .group_hover_color(drag_group, Color::Icon),
                        ),
                )
            })
            .into_any_element()
    }

    /// As in the original, Today and Next 7 Days list the habits due today, unless turned off in
    /// the habit settings. Checking one checks the habit in (one unit for amount goals).
    fn render_habits(&self, strings: &Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let today = Self::today();
        let habits = self.today_habits.clone();
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
                            task_rows::relative_day(today, strings.todays_habit, strings)
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
                    let (target, unit) = habit.goal.as_ref().map_or((1.0, String::new()), |g| (g.amount, g.unit.clone()));
                    let progress_text = format!(
                        "{} / {} {}",
                        fern_core::habit::format_amount(amount),
                        fern_core::habit::format_amount(target),
                        unit
                    )
                    .trim()
                    .to_owned();
                    h_flex()
                        .id(SharedString::from(format!("habit-row-{id}")))
                        .h(px(ROW_HEIGHT))
                        .pl(px(GUTTER))
                        .pr_3()
                        .gap(px(10.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.colors().ghost_element_hover))
                        .child(
                            div()
                                .id(SharedString::from(format!("habit-check-{id}")))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    let result = if has_goal {
                                        this.store.step_checkin(id, today).map(|_| ())
                                    } else {
                                        this.store.toggle_checkin(id, today).map(|_| ())
                                    };
                                    this.check(result);
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload_today_habits();
                                    cx.notify();
                                }))
                                .child(crate::habit_view::avatar(&habit, px(20.), cx)),
                        )
                        .child(
                            div().flex_1().min_w_0().child(
                                Label::new(habit.name.clone())
                                    .color(if checked { Color::Muted } else { Color::Default })
                                    .truncate(),
                            ),
                        )
                        .child(Label::new(progress_text).size(LabelSize::Small).color(Color::Muted))
                        .child(Label::new(strings.today).size(LabelSize::Small).color(Color::Accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let result = if has_goal {
                                this.store.step_checkin(id, today).map(|_| ())
                            } else {
                                this.store.toggle_checkin(id, today).map(|_| ())
                            };
                            this.check(result);
                            desktop_common::data_version::data_changed(cx.entity_id(), cx);
                            this.reload_today_habits();
                            cx.notify();
                        }))
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.habit_menu = Some((id, ev.position));
                                cx.notify();
                            }),
                        )
                        .border_b_1()
                        .border_color(theme.colors().border_variant)
                }))
                .into_any_element(),
        )
    }

    fn render_habit_context_menu(&self, strings: &Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, pos) = self.habit_menu?;
        let habit = self.today_habits.iter().find(|(h, _, _)| h.id == id)?.0.clone();
        let today = Self::today();
        let name = habit.name;
        let name_sw = name.clone();
        Some(
            div()
                .id("todo-habit-ctx")
                .absolute()
                .left(pos.x)
                .top(pos.y)
                .occlude()
                .child(
                    popover(cx)
                        .min_w(px(176.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.habit_menu = None;
                            cx.notify();
                        }))
                        .child(
                            ListItem::new("todo-habit-open")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::FernHabit).color(Color::Muted))
                                .child(Label::new(strings.section_habits))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    cx.emit(OpenHabit(id));
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("todo-habit-pomo")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Timer).color(Color::Muted))
                                .child(Label::new(strings.habit_start_pomo))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    cx.emit(StartHabitFocus { habit_id: id, title: name.clone(), stopwatch: false });
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("todo-habit-sw")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Clock).color(Color::Muted))
                                .child(Label::new(strings.habit_start_stopwatch))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    cx.emit(StartHabitFocus { habit_id: id, title: name_sw.clone(), stopwatch: true });
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("todo-habit-skip")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Minus).color(Color::Muted))
                                .child(Label::new(strings.habit_skip))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    let res = this.store.skip_habit_date(id, today);
                                    this.check(res);
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload_today_habits();
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("todo-habit-undone")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Ban).color(Color::Muted))
                                .child(Label::new(strings.habit_unachieved))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    let res = this.store.mark_checkin_undone(id, today);
                                    this.check(res);
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload_today_habits();
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("todo-habit-reset")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::RotateCcw).color(Color::Muted))
                                .child(Label::new(strings.habit_reset))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.habit_menu = None;
                                    let res = this.store.reset_checkin(id, today, true);
                                    this.check(res);
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload_today_habits();
                                    cx.notify();
                                })),
                        ),
                )
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
                        .child(div().flex_1().min_w_0().child(Label::new(countdown.name).truncate()))
                        .child(Label::new(when).size(LabelSize::Small).color(Color::Accent))
                        .border_b_1()
                        .border_color(theme.colors().border_variant)
                }))
                .into_any_element(),
        )
    }

    /// The details' own picker for a task, for the right-click menu and the calendar's.
    pub fn start_detail(&mut self, id: Id, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.kanban && !self.timeline && self.selected.is_none() {
            self.open_card(id, cx);
            self.card_detail.update(cx, |detail, cx| detail.start(what, window, cx));
        } else {
            self.select(id, cx);
            self.detail.update(cx, |detail, cx| detail.start(what, window, cx));
        }
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
        if !cx.has_active_drag() {
            if self.panel_drag.is_some() {
                self.restore_panel_drag_snapshot();
            }
            self.clear_task_drag_state();
        }
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
        let popup_details = self.uses_popup_details();
        // The dragged width, narrowed while the window leaves the task list too little room.
        let view_width = f32::from(window.viewport_size().width) - crate::sidebar::WIDTH;
        let detail_width = self.detail_width.min(self.max_detail_width(view_width)).max(DETAIL_MIN_WIDTH);
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
        let board: Option<AnyElement> = board
            .or_else(|| kanban_view.then(|| self.render_kanban_board(&tag_colors, today, strings, window, cx)));
        let rows = if kanban_view { Vec::new() } else { self.rows(strings) };
        let rows_empty = rows.is_empty();
        if !kanban_view {
            self.tag_colors = tag_colors;
            let mut entries = Vec::with_capacity(rows.len() + 3);
            if self.new_section.is_some() {
                entries.push(ListEntry::NewSection);
            }
            // In Next 7 Days today's habits come first, as the original's.
            if self.view == View::Next7Days {
                entries.push(ListEntry::Habits);
            }
            entries.extend(rows.into_iter().map(ListEntry::Row));
            if self.view != View::Next7Days {
                entries.push(ListEntry::Habits);
            }
            entries.push(ListEntry::Countdowns);
            self.sync_list(entries);
        }

        let list = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            
            .child(
                h_flex()
                    .mt(crate::title_bar::header_inset())
                    .h(px(60.))
                    // The title sits 4 px above the row's middle, as the original's.
                    .pb(px(8.))
                    .flex_none()
                    .gap_2()
                    .pl(px(24.))
                    .pr(px(16.))
                    .child(
                        IconButton::new("toggle-sidebar", IconName::PanelLeft)
                            .tooltip(Tooltip::text(strings.shortcut_labels[2]))
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
                            div().debug_selector(|| "empty-trash-button".to_owned()).child(
                                IconButton::new("empty-trash-button", IconName::BrushCleaning)
                                    .tooltip(Tooltip::text(strings.empty_trash))
                                    .on_click(cx.listener(|this, _, _, cx| this.request_empty_trash(cx))),
                            ),
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
                    .when(kanban_only, |el| {
                        let workflow_on = self.kanban_workflow;
                        el.child(
                            h_flex()
                                .id("kanban-workflow-toggle")
                                .h(px(26.))
                                .px_2()
                                .gap_1()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .when(workflow_on, |el| el.bg(theme.colors().element_selected))
                                .hover(|s| s.bg(theme.colors().ghost_element_hover))
                                .child(
                                    Icon::new(IconName::Kanban)
                                        .size(IconSize::XSmall)
                                        .color(if workflow_on { Color::Accent } else { Color::Muted }),
                                )
                                .child(
                                    Label::new(strings.kanban_workflow)
                                        .size(LabelSize::Small)
                                        .color(if workflow_on { Color::Accent } else { Color::Muted }),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_kanban_workflow(cx))),
                        )
                    })
                    .when(self.view != View::Trash && !closed_header, |el| {
                        el.child(
                            div()
                                .relative()
                                .debug_selector(|| "task-sort-menu-trigger".to_owned())
                                .child(IconButton::new("sort", IconName::ArrowDownUp).tip(strings.group_and_sort, ui::TipSide::Below).on_click(cx.listener(|this, _, window, cx| this.open_sort_menu(window, cx))))
                                .when_some(self.sort_menu, |el, expanded| {
                                    el.child(PopoverAttachment::new(Placement::BelowRight, self.render_sort_menu(expanded, strings, cx)))
                                }),
                        )
                    })
                    .when(self.view != View::Trash && !closed_header, |el| el.child(
                        div()
                            .relative()
                            .child(IconButton::new("more", IconName::Ellipsis).tip(strings.more, ui::TipSide::Below).on_click(cx.listener(
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
                    .child(div().flex_1().min_w_0().debug_selector(|| "quick-add-input".to_owned()).child(self.input.clone()))
                    .children(self.shown_add_due().map(|(date, time)| {
                        let (label, _) = task_rows::due_label(date, time, Self::today(), strings);
                        // A picked duration shows its span, as the original's "Today, 22:00 - 23:00".
                        let label = match self.add_draft.end.filter(|_| self.quick_add_due.is_none()) {
                            Some((end, end_time)) => crate::due_picker::range_label(date, time, end, end_time, strings),
                            None => label,
                        };
                        let has_repeat = self.quick_add_repeat || self.add_draft.repeat.is_some();
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .child(Icon::new(IconName::CalendarDays).size(IconSize::Small).color(Color::Accent))
                            .when(has_repeat, |el| {
                                el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                            })
                            .child(Label::new(label).size(LabelSize::Small).color(Color::Accent))
                    }))
                    .when(self.shown_add_due().is_none() && (self.quick_add_repeat || self.add_draft.repeat.is_some()), |el| {
                        el.child(Icon::new(IconName::Repeat).size(IconSize::Small).color(Color::Accent))
                    })
                    .child(
                        div()
                            .relative()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                h_flex().child(self.add_date_button(Placement::BelowCenter, strings, cx)).child(
                                // The original's "▾": priority, list, template and note for the
                                // next task; its flag takes the chosen priority's color.
                                IconButton::new("add-options", IconName::ChevronDown)
                                    .size(px(24.))
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text(strings.more))
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
                self.bind_task_list_actions(
                    div().id("tasks").track_focus(&self.list_focus),
                    cx,
                )
                    .on_drag_move(cx.listener(Self::move_task_drag))
                    .on_drop(cx.listener(Self::drop_task_in_list))
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child({
                        let bounds = self.list_entry_bounds.clone();
                        gpui::canvas(move |_, _, _| bounds.borrow_mut().clear(), |_, _, _, _| {})
                            .absolute()
                            .size_0()
                    })
                    .when(rows_empty, |el| el.child(self.render_empty_state(strings, &theme, px(12.))))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(
                                list(self.list_state.clone(), cx.processor(|this, ix, _, cx| this.render_entry(ix, cx)))
                                    .size_full()
                                    .pb_4(),
                            )
                            .child(VerticalScrollbar::for_list("task-list-scroll", self.list_state.clone())),
                    ),
            ))
            .when_some(self.error.clone().or_else(|| self.add_attachment_error.clone()), |el, error| {
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
        let on_select = move |view, window: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.select_view(view, window, cx)).ok();
        };
        let on_add_list = cx.listener(|this, _, window, cx| this.start_new_list(window, cx));
        let this = cx.entity().downgrade();
        let on_drop_task = move |view, id, _: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.drop_on_view(view, id, cx)).ok();
        };

        h_flex()
            .relative()
            .size_full()
            .key_context("Tasks")
            .items_start()
            .on_action(cx.listener(|this, _: &DismissTransient, window, cx| {
                if !this.dismiss_transient(window, cx) {
                    cx.propagate();
                }
            }))
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .text_size(px(14.))
            .on_drag_move(cx.listener(Self::resize_panel))
            .on_drag_move(cx.listener(Self::move_smart_list_drag))
            .on_drag_move(cx.listener(Self::move_sidebar_section_drag))
            .on_drag_move(cx.listener(Self::move_list_drag))
            .on_drag_move(cx.listener(Self::move_folder_drag))
            .on_drag_move(cx.listener(Self::move_tag_drag))
            .on_drag_move(cx.listener(Self::move_parent_tag_drag))
            .on_drag_move(cx.listener(Self::move_filter_drag))
            .capture_any_mouse_up(cx.listener(Self::finish_panel_drag_up))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.clear_task_drag_state() {
                    cx.notify();
                }
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, event, window, cx| {
                if this.clear_task_drag_state() {
                    cx.notify();
                }
                this.finish_panel_drag_up(event, window, cx);
            }))
            .when(self.show_lists, |el| {
                let active_drag = self.panel_drag.as_ref().map(|d| &d.item);
                let highlight_target = self.panel_drag.as_ref().and_then(|d| d.highlight.as_ref());
                el.child(div().relative().flex_none().h_full().child(lists_panel::render(
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
                        width: self.lists_width,
                        scroll_handle: &self.lists_scroll,
                        rows_out: &self.lists_rows,
                        active_drag,
                        highlight_target,
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
                )).child(self.resize_handle(Panel::Lists, cx)))
            })
            .map(|el| {
                if self.view == View::Summary {
                    return el.child(div().flex_1().min_w_0().h_full().child(self.summary.clone()));
                }
                let multi = self.is_multi_selecting();
                // As in the original, the Timeline takes the whole width until a task is opened.
                if (timeline_view || kanban_only) && (self.selected.is_none() || multi) && self.list_activity.is_none() {
                    return el.child(list);
                }
                // View Options' Pop-up: in a narrow window an opened task shows over the list.
                if popup_details {
                    if multi || (self.selected.is_none() && self.list_activity.is_none()) {
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
                                    .w(px(detail_width))
                                    .h(gpui::relative(0.8))
                                    .rounded(px(10.))
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(theme.colors().border)
                                    .bg(theme.colors().background)
                                    .shadow_lg()
                                    .on_click(|_, _, cx| cx.stop_propagation())
                                    .map(|el| match self.list_activity.clone() {
                                        Some(entries) => el.child(self.render_list_activity(&entries, false, cx)),
                                        None => el.child(self.detail.clone()),
                                    }),
                            ),
                    );
                }
                el.child(list).child(
                    div()
                        .relative()
                        .flex_none()
                        .w(px(detail_width))
                        .h_full()
                        .map(|el| match self.list_activity.clone() {
                            Some(entries) => el.child(self.render_list_activity(&entries, true, cx)),
                            None if multi => el.child(self.render_batch_panel(strings, cx)),
                            None => el.child(self.detail.clone()),
                        })
                        .child(self.resize_handle(Panel::Detail, cx)),
                )
            })
            .when(
                self.view != View::Summary
                    && self.is_multi_selecting()
                    && (popup_details || ((timeline_view || kanban_only) && self.list_activity.is_none())),
                |el| {
                    let bottom = if self.undo.is_some() { px(72.) } else { px(24.) };
                    el.child(
                        div()
                            .absolute()
                            .bottom(bottom)
                            .left_0()
                            .right_0()
                            .flex()
                            .justify_center()
                            .child(self.render_batch_bar(strings, cx)),
                    )
                },
            )
            .when_some(self.filter_edit.as_ref().map(|edit| self.render_filter_edit(edit, window, cx)), |el, dialog| el.child(dialog))
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
            .children(self.render_habit_context_menu(strings, cx))
            .children(self.render_list_menu(strings, cx))
            .children(self.view_options.map(|menu| self.render_view_options(menu, strings, cx)))
            .children(self.suggestions_open.filter(|_| self.view == View::Today).map(|all| self.render_suggestions(all, strings, cx)))
            .children(self.list_dialog.clone())
            .children(self.folder_dialog.clone())
            .children(self.tag_dialog.clone())
            .children(self.merge_dialog.clone())
            .when(self.confirm_empty_trash, |el| {
                el.child(
                    div()
                        .id("confirm-empty-trash-backdrop")
                        .debug_selector(|| "confirm-empty-trash-modal".to_owned())
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::black().opacity(0.35))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.confirm_empty_trash = false;
                            cx.notify();
                        }))
                        .child(
                            v_flex()
                                .id("confirm-empty-trash-card")
                                .w(px(340.))
                                .p_5()
                                .gap_3()
                                .rounded(px(10.))
                                .bg(theme.colors().elevated_surface_background)
                                .border_1()
                                .border_color(theme.colors().border)
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(Label::new(strings.confirm_empty_trash).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                                .child(Label::new(strings.confirm_empty_trash_hint).size(LabelSize::Small).color(Color::Muted))
                                .child(
                                    h_flex()
                                        .pt_2()
                                        .gap_2()
                                        .justify_end()
                                        .child(
                                            div()
                                                .id("empty-trash-cancel")
                                                .debug_selector(|| "empty-trash-cancel".to_owned())
                                                .px_3()
                                                .h(px(30.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .border_1()
                                                .border_color(theme.colors().border)
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.colors().element_hover))
                                                .child(Label::new(strings.cancel).size(LabelSize::Small))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.confirm_empty_trash = false;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id("empty-trash-confirm")
                                                .debug_selector(|| "empty-trash-confirm".to_owned())
                                                .px_3()
                                                .h(px(30.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .bg(theme.status().error)
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.status().error.opacity(0.9)))
                                                .child(Label::new(strings.empty_trash).size(LabelSize::Small).color(Color::OnAccent))
                                                .on_click(cx.listener(|this, _, _, cx| this.empty_trash(cx))),
                                        ),
                                ),
                        ),
                )
            })
            .when_some(self.confirm_delete.clone(), |el, target| {
                let (title, hint) = match &target {
                    ConfirmDeleteTarget::List(_, name) => (
                        format!("{} \"{name}\"?", strings.delete),
                        strings.confirm_delete_list_hint,
                    ),
                    ConfirmDeleteTarget::Tag(tag) => (
                        format!("{} \"#{tag}\"?", strings.delete),
                        strings.confirm_delete_tag_hint,
                    ),
                    ConfirmDeleteTarget::Filter(_, name) => (
                        format!("{} \"{name}\"?", strings.delete_filter),
                        strings.confirm_delete_filter_hint,
                    ),
                    ConfirmDeleteTarget::Section(_, name) => (
                        format!("{} \"{name}\"?", strings.delete),
                        strings.confirm_delete_section_hint,
                    ),
                    ConfirmDeleteTarget::BatchPurge(count) => (
                        format!(
                            "{} ({})?",
                            strings.delete_forever,
                            strings.batch_selected.replace("{n}", &count.to_string())
                        ),
                        strings.confirm_delete_forever_hint,
                    ),
                };
                el.child(
                    div()
                        .id("confirm-delete-backdrop")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::black().opacity(0.35))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.confirm_delete = None;
                            cx.notify();
                        }))
                        .child(
                            v_flex()
                                .id("confirm-delete-card")
                                .w(px(340.))
                                .p_5()
                                .gap_3()
                                .rounded(px(10.))
                                .bg(theme.colors().elevated_surface_background)
                                .border_1()
                                .border_color(theme.colors().border)
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(Label::new(title).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                                .child(Label::new(hint).size(LabelSize::Small).color(Color::Muted))
                                .child(
                                    h_flex()
                                        .pt_2()
                                        .gap_2()
                                        .justify_end()
                                        .child(
                                            div()
                                                .id("confirm-delete-cancel")
                                                .px_3()
                                                .h(px(30.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .border_1()
                                                .border_color(theme.colors().border)
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.colors().element_hover))
                                                .child(Label::new(strings.cancel).size(LabelSize::Small))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.confirm_delete = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id("confirm-delete-ok")
                                                .px_3()
                                                .h(px(30.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .bg(theme.status().error)
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.status().error.opacity(0.9)))
                                                .child(Label::new(strings.delete).size(LabelSize::Small).color(Color::OnAccent))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.execute_confirmed_delete(target.clone(), cx);
                                                })),
                                        ),
                                ),
                        ),
                )
            })
            .when_some(self.batch_add_pending.clone(), |el, lines| {
                let count = lines.len();
                let preview: Vec<String> = lines.iter().take(5).cloned().collect();
                let extra = count.saturating_sub(preview.len());
                let desc = strings.batch_add_tasks_desc.replace("{n}", &count.to_string());
                let add_label = format!("{} ({count})", strings.add);
                el.child(
                    div()
                        .id("batch-add-backdrop")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::black().opacity(0.45))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.batch_add_pending = None;
                                cx.notify();
                            }),
                        )
                        .child(
                            v_flex()
                                .id("batch-add-modal")
                                .w(px(380.))
                                .p_4()
                                .gap_3()
                                .rounded(px(10.))
                                .bg(theme.colors().elevated_surface_background)
                                .border_1()
                                .border_color(theme.colors().border)
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(Label::new(strings.batch_add_tasks).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                                .child(Label::new(desc).size(LabelSize::Small).color(Color::Muted))
                                .child(
                                    v_flex()
                                        .p_2()
                                        .gap_1()
                                        .rounded(px(6.))
                                        .bg(theme.colors().surface_background)
                                        .border_1()
                                        .border_color(theme.colors().border_variant)
                                        .children(preview.into_iter().map(|line| {
                                            h_flex()
                                                .gap_1p5()
                                                .child(div().w(px(4.)).h(px(4.)).rounded_full().bg(theme.colors().accent))
                                                .child(Label::new(line).size(LabelSize::XSmall).truncate())
                                        }))
                                        .when(extra > 0, |list| {
                                            list.child(
                                                Label::new(format!("+{extra}"))
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Muted),
                                            )
                                        }),
                                )
                                .child(
                                    h_flex()
                                        .pt_1()
                                        .gap_2()
                                        .justify_end()
                                        .child(
                                            div()
                                                .id("batch-add-cancel")
                                                .debug_selector(|| "batch-add-cancel".to_owned())
                                                .px_3()
                                                .h(px(30.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .border_1()
                                                .border_color(theme.colors().border)
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.colors().element_hover))
                                                .child(Label::new(strings.cancel).size(LabelSize::Small))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.batch_add_pending = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id("batch-add-as-one")
                                                .debug_selector(|| "batch-add-as-one".to_owned())
                                                .px_3()
                                                .h(px(30.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .border_1()
                                                .border_color(theme.colors().border)
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.colors().element_hover))
                                                .child(Label::new(strings.batch_add_as_one).size(LabelSize::Small))
                                                .on_click(cx.listener(|this, _, _, cx| this.paste_batch_as_one(cx))),
                                        )
                                        .child(
                                            div()
                                                .id("batch-add-confirm")
                                                .debug_selector(|| "batch-add-confirm".to_owned())
                                                .px_3()
                                                .h(px(30.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .bg(theme.colors().accent)
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.colors().accent.opacity(0.9)))
                                                .child(Label::new(add_label).size(LabelSize::Small).color(Color::OnAccent))
                                                .on_click(cx.listener(|this, _, window, cx| this.confirm_batch_add(window, cx))),
                                        ),
                                ),
                        ),
                )
            })
    }
}


#[derive(Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
enum ListEntry {
    /// The field for a new section's name.
    NewSection,
    /// Today's habits.
    Habits,
    /// The countdowns shown in Today.
    Countdowns,
    Row(Row),
}

