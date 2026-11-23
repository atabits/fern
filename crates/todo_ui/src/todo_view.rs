use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use fern_core::{ColorBy, Id, Kind, List, ListFolder, Priority, Store, Tag, Task, View, ViewCounts};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone as _};
use gpui::{ElementId, Entity, FocusHandle, Focusable, FontWeight, ListState, MouseButton, Pixels, Point};
use ui::prelude::*;

use crate::drag::{
    DraggedColumn, DraggedFilter, DraggedFolder, DraggedList, DraggedParentTag,
    DraggedSidebarSection, DraggedSmartList, DraggedTag, DraggedTask, ResizeEdge, ResizedTask,
};
use ui::{HorizontalScrollbar, Placement, PopoverAttachment, SubmenuItem, popover};

use crate::actions::{DismissTransient, SelectAllTasks, ToggleInProgress};
use crate::i18n::{self, CurrentLocale, Strings};
use crate::list_dialog::{FolderDialog, ListDialog};
use crate::tag_dialog::{MergeDialog, TagDialog};
use crate::lists_panel;
use crate::resize::Panel;
use crate::task_detail::{OpenTask, StartFocus, TaskDetail, TaskTextEdited, TaskTextField, TaskUpdated};
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

mod actions;
mod batch;
#[cfg(debug_assertions)]
mod dev_state;
mod dialogs;
mod filters;
mod kanban;
mod kanban_cards;
mod kanban_dnd;
mod layout_settings;
mod list_render;
mod menus;
mod panel_dnd;
mod quick_add;
mod quick_add_popovers;
mod timeline;
mod today_widgets;
#[cfg(all(test, debug_assertions))]
mod tests;

use batch::BatchMenu;
use filters::FilterEdit;
use kanban::{BoardCard, KanbanColRows};
use list_render::{ListEntry, RowsCacheKey, chevron};
use menus::{ClosedMenu, ListMenuTarget, SortMenu};
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
    Template(Id, String),
    PurgeTask(Id, String),
    BatchPurge(usize),
}

pub struct TodoView {
    window_handle: gpui::AnyWindowHandle,
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
    list_rows: Rc<Vec<Row>>,
    rows_version: u64,
    cached_rows: RefCell<Option<(RowsCacheKey, Rc<Vec<Row>>)>>,
    kanban_cache: RefCell<Option<(kanban::KanbanCacheKey, Rc<Vec<kanban::BoardCol>>)>>,
    synced_list_key: Option<(RowsCacheKey, bool, bool)>,
    /// Window bounds of currently painted `list_entries` rows, keyed by entry index.
    list_entry_bounds: Rc<RefCell<HashMap<usize, gpui::Bounds<Pixels>>>>,
    /// Active task-list drag origin: `(dragged_task_id, start_mouse_x)`.
    task_drag_origin: Option<(Id, Pixels)>,
    /// Live drop plan while dragging a task over `#tasks`.
    task_drop: Option<task_rows::TaskDropPlan>,
    list_colors: HashMap<Id, Hsla>,
    tag_colors: Rc<HashMap<String, Hsla>>,
    tag_colors_ci: HashMap<String, Hsla>,
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
    /// Cached per-view flags & sort/group settings (refreshed in `reload_view_settings` so
    /// `render()` and `move_task_drag()` perform zero SQLite queries).
    show_completed: bool,
    sort_descending: bool,
    hide_new_section: bool,
    sort_by: SortBy,
    group_by: GroupBy,
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
    kanban_quick_add_subs: HashMap<String, gpui::Subscription>,
    kanban_col_scrolls: HashMap<String, gpui::ScrollHandle>,
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
    cached_suggestions: fern_core::suggest::Suggestions,
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
    quick_add_horizon: Option<fern_core::Horizon>,
    quick_add_repeat: bool,
    /// The mini calendar: expanded to a month, which month, and the days with open tasks.
    mini_expanded: bool,
    mini_month: NaiveDate,
    busy_days: HashSet<NaiveDate>,
    /// Today and Next 7 Days: the habits due today, whether each is checked in and its amount;
    /// empty elsewhere or when the habit settings keep them out.
    today_habits: Rc<Vec<(fern_core::Habit, bool, f64)>>,
    /// Today's countdowns shown in the Today list (cached on reload; empty outside Today).
    today_countdowns: Vec<fern_core::Countdown>,
    /// Cached subtask progress `(done, total)` per parent task ID, computed in O(N) on reload.
    subtask_progress: HashMap<Id, (usize, usize)>,
    /// Cached templates list for the quick-add templates popover.
    cached_templates: Vec<fern_core::Template>,
    /// Right-click context menu on a habit row in Today / Next 7 Days.
    habit_menu: Option<(Id, gpui::Point<Pixels>)>,
    /// The Summary smart list's report.
    summary: Entity<crate::summary_view::SummaryView>,
    /// The embedded TimeStripe-inspired Horizons planning view.
    horizons: Entity<crate::horizons_view::HorizonsView>,
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
    /// A task row's right-click menu: the cached task, pointer window coordinates, and precomputed nesting.
    context_menu: Option<(Task, gpui::Point<Pixels>, task_menu::TaskNesting)>,
    /// Its side menu open: 0 Move to, 1 Start Focus.
    context_side: Option<u8>,
    error: Option<String>,
    add_attachment_error: Option<String>,
    pending_complete: HashSet<Id>,
    submitting_task: bool,
    text_edit_sync_task: Option<gpui::Task<()>>,
    active: bool,
    dirty: bool,
}

impl TodoView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, window, true, cx)
    }

    pub fn new_inactive(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, window, false, cx)
    }

    fn new_with_active(
        store: Rc<Store>,
        window: &mut Window,
        initial_active: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let strings = i18n::t(cx);
        let summary = cx.new(|cx| crate::summary_view::SummaryView::new(store.clone(), cx));
        let horizons = cx.new(|cx| crate::horizons_view::HorizonsView::new(store.clone(), window, cx));
        cx.subscribe_in(&horizons, window, |_this, _, event: &crate::horizons_view::HorizonsEvent, window, cx| {
            match event {
                crate::horizons_view::HorizonsEvent::StartFocus { task_id, title, stopwatch } => {
                    cx.emit(StartFocus(*task_id, title.clone(), *stopwatch));
                }
                crate::horizons_view::HorizonsEvent::OpenSearch => {
                    window.dispatch_action(Box::new(crate::actions::Search), cx);
                }
            }
        })
        .detach();
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
        if initial_active {
            window.focus(&input.focus_handle(cx), cx);
        }
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

        // A change made in a sticky note or another view shows here without re-broadcasting DataVersion.
        cx.observe_global::<crate::data_version::DataVersion>(|this, cx| {
            if crate::data_version::changed_elsewhere(cx.entity_id(), cx) {
                if this.active {
                    this.reload_inner(false, false, cx);
                } else {
                    this.dirty = true;
                }
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
        cx.subscribe(&detail, |this, _, edit: &TaskTextEdited, cx| {
            this.apply_task_text_edit(edit, false, cx);
        })
        .detach();
        cx.subscribe(&detail, |this, _, OpenTask(id), cx| this.select(*id, cx)).detach();
        cx.subscribe(&detail, |_, _, StartFocus(id, title, stopwatch), cx| cx.emit(StartFocus(*id, title.clone(), *stopwatch))).detach();
        cx.subscribe(&detail, |this, _, crate::task_detail::Undoable(action, label), cx| {
            this.offer_undo(action.clone(), label.clone(), cx);
        })
        .detach();
        cx.subscribe(&detail, |this, _, crate::task_detail::RequestDeleteForever(id), cx| {
            this.request_purge_task(*id, cx);
        })
        .detach();

        let card_detail = cx.new(|cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.set_card(false);
            detail
        });
        cx.subscribe(&card_detail, |this, _, _: &TaskUpdated, cx| this.card_updated(cx)).detach();
        cx.subscribe(&card_detail, |this, _, edit: &TaskTextEdited, cx| {
            this.apply_task_text_edit(edit, true, cx);
        })
        .detach();
        // A subtask or parent clicked in the card opens in the card.
        cx.subscribe(&card_detail, |this, _, OpenTask(id), cx| this.open_card(*id, cx)).detach();
        cx.subscribe(&card_detail, |_, _, StartFocus(id, title, stopwatch), cx| cx.emit(StartFocus(*id, title.clone(), *stopwatch))).detach();
        cx.subscribe(&card_detail, |this, _, crate::task_detail::Undoable(action, label), cx| {
            this.offer_undo(action.clone(), label.clone(), cx);
        })
        .detach();
        cx.subscribe(&card_detail, |this, _, crate::task_detail::RequestDeleteForever(id), cx| {
            this.request_purge_task(*id, cx);
        })
        .detach();

        let width = |key: &str, default: f32| store.setting(key).ok().flatten().and_then(|value| value.parse().ok()).unwrap_or(default);
        let lists_width = width("lists_width", lists_panel::WIDTH).clamp(lists_panel::MIN_WIDTH, lists_panel::MAX_WIDTH);
        let detail_width = width("detail_width", DETAIL_WIDTH).max(DETAIL_MIN_WIDTH);
        let narrow_viewport = window.viewport_size().width < px(NARROW_WINDOW);
        let window_handle = window.window_handle();
        let mut this = Self {
            window_handle,
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
            list_rows: Rc::new(Vec::new()),
            rows_version: 0,
            cached_rows: RefCell::new(None),
            kanban_cache: RefCell::new(None),
            synced_list_key: None,
            list_entry_bounds: Rc::new(RefCell::new(HashMap::new())),
            task_drag_origin: None,
            task_drop: None,
            list_colors: HashMap::new(),
            tag_colors: Rc::new(HashMap::new()),
            tag_colors_ci: HashMap::new(),
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
            show_completed: true,
            sort_descending: false,
            hide_new_section: false,
            sort_by: SortBy::Custom,
            group_by: GroupBy::Custom,
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
            kanban_quick_add_subs: HashMap::new(),
            kanban_col_scrolls: HashMap::new(),
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
            cached_suggestions: fern_core::suggest::Suggestions::default(),
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
            quick_add_horizon: None,
            quick_add_repeat: false,
            mini_expanded,
            mini_month: crate::calendar::month_start(Self::today()),
            busy_days: HashSet::new(),
            today_habits: Rc::new(Vec::new()),
            today_countdowns: Vec::new(),
            subtask_progress: HashMap::new(),
            cached_templates: Vec::new(),
            habit_menu: None,
            summary,
            horizons,
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
            pending_complete: HashSet::new(),
            submitting_task: false,
            text_edit_sync_task: None,
            active: initial_active,
            dirty: !initial_active,
        };
        if initial_active {
            this.reload_inner(false, false, cx);
            #[cfg(debug_assertions)]
            this.apply_dev_state(window, cx);
        } else {
            this.sync_subview_active(cx);
        }
        this
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
        if matches!(self.view, View::Summary | View::Horizons) || self.uses_board_layout() || self.uses_popup_details() {
            return None;
        }
        self.rows_rc(i18n::t(cx)).iter().find_map(|row| match row {
            Row::Task { task, .. } => Some(task.clone()),
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

    fn sync_subview_active(&self, cx: &mut Context<Self>) {
        let horizons_active = self.active && self.view == View::Horizons;
        self.horizons.update(cx, |horizons, cx| horizons.set_active(horizons_active, cx));
        let summary_active = self.active && matches!(self.view, View::Summary);
        self.summary.update(cx, |summary, cx| summary.set_active(summary_active, cx));
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if active && self.dirty {
            self.dirty = false;
            self.reload_inner(false, false, cx);
        } else {
            self.sync_subview_active(cx);
        }
    }

    pub fn reload_external(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        self.reload_inner(false, false, cx);
        self.horizons.update(cx, |horizons, cx| horizons.sync_external(cx));
    }

    pub fn sync_external(&mut self, cx: &mut Context<Self>) {
        if self.active {
            self.reload_external(cx);
        } else {
            self.dirty = true;
            self.horizons.update(cx, |horizons, cx| horizons.sync_external(cx));
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.reload_inner(false, true, cx);
    }

    pub(super) fn reload_inner(&mut self, mut switched: bool, notify_notes: bool, cx: &mut Context<Self>) {
        self.dirty = false;
        // Sticky notes show data mutations, not view navigation.
        if notify_notes {
            self.text_edit_sync_task = None;
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
            self.refresh_list_colors();
        }
        // The current list may have been deleted.
        if let View::List(id) = self.view
            && !self.lists.iter().any(|l| l.id == id)
        {
            self.view = View::Inbox;
            switched = true;
        }
        // If Horizons was turned off in Settings -> Features, leave View::Horizons cleanly.
        if !crate::calendar::horizons_enabled() && self.view == View::Horizons {
            self.view = View::Today;
            switched = true;
        }
        self.sync_subview_active(cx);
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
        self.reload_view_settings();
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
            .filter(|s| !s.is_empty() && crate::calendar::is_valid_timezone(s));
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
        if self.suggestions_open.is_some() && self.view == View::Today {
            self.cached_suggestions = fern_core::suggest::suggestions(&all_tasks, today);
        }
        if let Some(edit) = self.filter_edit.as_mut() {
            edit.all_tasks = all_tasks.clone();
        }
        if self.templates_open {
            self.cached_templates = self.store.templates().unwrap_or_default();
        }
        if let Some(counts) = self.check(self.store.counts_from(&all_tasks, today)) {
            self.counts = counts;
        }
        let show_completed = matches!(self.view, View::Today | View::Tomorrow | View::Next7Days)
            && self.show_completed;
        if let Some(tasks) = self.check(self.store.tasks_in_from_with_today_completed(all_tasks, &self.view, today, show_completed)) {
            self.tasks = tasks;
        }
        self.ensure_kanban_sections(i18n::t(cx), cx);
        self.refresh_subtask_progress();
        if let Some(tags) = self.check(self.store.tags()) {
            self.tags = tags;
            self.refresh_tag_colors();
        }
        self.reload_today_habits();
        self.reload_today_countdowns();
        self.invalidate_rows();
        let lists = self.active_lists();
        if switched {
            self.reset_view_transients(cx);
            self.list_state = Self::new_list_state();
            self.list_entries.clear();
            self.list_rows = Rc::new(Vec::new());
            self.synced_list_key = None;
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
        self.sync_kanban_column_state(cx);
        cx.notify();
    }

    pub(super) fn refresh_list_colors(&mut self) {
        self.list_colors = self
            .lists
            .iter()
            .filter_map(|l| lists_panel::parse_color(l.color.as_deref()).map(|c| (l.id, c)))
            .collect();
    }

    pub(super) fn refresh_tag_colors(&mut self) {
        let mut exact = HashMap::with_capacity(self.tags.len());
        let mut ci = HashMap::with_capacity(self.tags.len());
        for tag in &self.tags {
            if let Some(color) = lists_panel::parse_color(tag.color.as_deref()) {
                exact.insert(tag.name.clone(), color);
                ci.insert(tag.name.to_lowercase(), color);
            }
        }
        self.tag_colors = Rc::new(exact);
        self.tag_colors_ci = ci;
    }

    pub(super) fn tag_color_for(&self, name: &str) -> Option<Hsla> {
        self.tag_colors
            .get(name)
            .or_else(|| self.tag_colors_ci.get(&name.to_lowercase()))
            .copied()
    }

    fn apply_task_text_edit(&mut self, edit: &TaskTextEdited, is_card: bool, cx: &mut Context<Self>) {
        if matches!(self.view, View::Search(_) | View::Filter(_))
            || self.sort_settings().0 == SortBy::Title
            || self.dirty
        {
            if is_card {
                self.card_updated(cx);
            } else {
                self.reload(cx);
            }
            return;
        }
        let patch = |task: &mut Task| match &edit.field {
            TaskTextField::Title(title) => task.title = title.clone(),
            TaskTextField::Notes(notes) => task.notes = notes.clone(),
            TaskTextField::Location(location) => task.location = location.clone(),
        };
        for task in self.tasks.iter_mut().filter(|t| t.id == edit.id) {
            patch(task);
        }
        if let Some(pending) = self.completing.get_mut(&edit.id) {
            patch(&mut pending.before);
        }
        self.invalidate_rows();
        cx.notify();
        self.text_edit_sync_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.text_edit_sync_task = None;
                crate::data_version::data_changed(cx.entity_id(), cx);
            });
        }));
    }

    fn refresh_subtask_progress(&mut self) {
        self.subtask_progress.clear();
        let tree = fern_core::TaskTree::build(&self.tasks);
        for task in &self.tasks {
            if !tree.has_children(task.id) {
                continue;
            }
            let mut done = 0usize;
            let mut total = 0usize;
            for child in tree.children(task.id) {
                if child.is_deleted() || child.is_wont_do() {
                    continue;
                }
                total += 1;
                if child.is_completed() {
                    done += 1;
                }
            }
            self.subtask_progress.insert(task.id, (done, total));
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

    fn save_preferences(&mut self, change: impl FnOnce(&mut fern_core::Preferences), cx: &mut Context<Self>) {
        let result = self.store.update_preferences(change);
        if let Some(preferences) = self.check(result) {
            self.preferences = preferences;
            self.sync_detail_inset(cx);
            crate::data_version::data_changed(cx.entity_id(), cx);
        }
        cx.notify();
    }


    /// The lists not archived, for choosing a list.
    fn active_lists(&self) -> Vec<List> {
        self.lists.iter().filter(|l| !l.archived).cloned().collect()
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

    pub(super) fn is_list_archived(&self, list_id: Id) -> bool {
        self.lists.iter().any(|l| l.id == list_id && l.archived)
    }

    pub(super) fn is_current_view_archived_list(&self) -> bool {
        matches!(self.view, View::List(id) if self.is_list_archived(id))
    }

    /// Whether a task is read-only (in the trash or in an archived list), matching the original.
    pub(super) fn is_task_read_only(&self, task: &Task) -> bool {
        task.is_deleted() || task.list_id.is_some_and(|id| self.is_list_archived(id))
    }

    pub(super) fn is_task_id_read_only(&self, id: Id) -> bool {
        if let Some(task) = self.tasks.iter().find(|t| t.id == id) {
            return self.is_task_read_only(task);
        }
        self.store.is_task_read_only(id).unwrap_or(false)
    }

    /// Views where new tasks cannot be added.
    fn is_read_only_view(&self) -> bool {
        matches!(self.view, View::Completed | View::WontDo | View::Trash | View::Search(_))
            || self.is_current_view_archived_list()
    }
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
        let (rows_empty, board) = if timeline_view {
            (true, Some(self.render_timeline(strings, cx)))
        } else if kanban_view {
            let tag_colors = Rc::clone(&self.tag_colors);
            (
                true,
                Some(self.render_kanban_board(&tag_colors, today, strings, window, cx)),
            )
        } else {
            (self.sync_list_entries(strings), None)
        };

        let list = self.render_list_pane(
            board,
            kanban_view,
            kanban_only,
            closed_header,
            add_focused,
            input_focus,
            rows_empty,
            strings,
            &theme,
            cx,
        );

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
                if self.view == View::Horizons {
                    return el.child(div().flex_1().min_w_0().h_full().child(self.horizons.clone()));
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
                    return el.child(list).child(self.render_detail_popup(detail_width, &theme, window, cx));
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
                !matches!(self.view, View::Summary | View::Horizons)
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
                el.child(self.render_undo_toast(label, strings, &theme, cx))
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
            .when(self.confirm_empty_trash, |el| el.child(self.render_confirm_empty_trash(strings, cx)))
            .when_some(self.confirm_delete.clone(), |el, target| {
                el.child(self.render_confirm_delete(target, strings, cx))
            })
            .when_some(self.batch_add_pending.clone(), |el, lines| {
                el.child(self.render_batch_add_modal(lines, strings, &theme, cx))
            })
    }
}

