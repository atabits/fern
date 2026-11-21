use std::rc::Rc;

use fern_core::Store;
use gpui::Focusable as _;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::actions::{
    CloseWindow, DismissTransient, MinimizeWindow, NewTask, OpenSettings, PrintDetail, PrintList,
    Quit, Search, ShowAll, ShowCompleted, ShowInbox, ShowNext7Days, ShowShortcuts, ShowToday,
    ShowTomorrow, ShowTrash, ShowWontDo, StartStop, SyncNow, TogglePause, ToggleSidebar,
};
use crate::calendar_view::{self, CalendarView};
use crate::countdown_view::CountdownView;
use crate::focus_view::FocusView;
use crate::habit_view::HabitView;
use crate::i18n;
use crate::matrix_view::{self, MatrixView};
use crate::release_notes::{self, ReleaseNotes, action_button};
use crate::reminders::{self, Notice, NoticeAction, Notices, Reply, Target};
use crate::search_modal::{SearchEvent, SearchModal};
use crate::settings_window::{self, SettingsEvent, SettingsWindow};
use crate::sidebar::{self, Section};
use crate::title_bar::{self, ShowReleaseNotes, TitleBar};
use crate::todo_view::TodoView;

fn reorder_enabled_rail(
    original: &[fern_core::RailSection],
    features: fern_core::Features,
    from: usize,
    to: usize,
) -> Vec<fern_core::RailSection> {
    let mut enabled: Vec<fern_core::RailSection> =
        original.iter().copied().filter(|&s| Section::from(s).enabled(features)).collect();
    if from >= enabled.len() || to >= enabled.len() || from == to {
        return original.to_vec();
    }
    let moved = enabled.remove(from);
    enabled.insert(to, moved);
    let mut next_enabled = enabled.into_iter();
    original
        .iter()
        .copied()
        .map(|s| if Section::from(s).enabled(features) { next_enabled.next().unwrap_or(s) } else { s })
        .collect()
}

pub struct AppView {
    pub(crate) store: Rc<Store>,
    section: Section,
    pub(crate) todo: gpui::Entity<TodoView>,
    pub(crate) calendar: gpui::Entity<CalendarView>,
    pub(crate) matrix: gpui::Entity<MatrixView>,
    pub(crate) countdown: gpui::Entity<CountdownView>,
    pub(crate) habits: gpui::Entity<HabitView>,
    pub(crate) focus: gpui::Entity<FocusView>,
    title_bar: gpui::Entity<TitleBar>,
    pub(crate) search: Option<gpui::Entity<SearchModal>>,
    pub(crate) account_menu_open: bool,
    notices: Notices,
    notices_open: bool,
    active_alerts: Vec<Notice>,
    sync: crate::sync::SyncStatus,
    /// Cached `store.sync_config().is_some()` so `render` never queries SQLite every frame.
    pub(crate) has_sync_config: bool,
    /// Cached `tray::always_on_top` so `render_account_menu` never queries SQLite every frame.
    always_on_top: bool,
    /// Settings → Features: the sections shown in the sidebar.
    pub(crate) features: fern_core::Features,
    /// Order of icons in the narrow left navigation rail.
    rail_order: Vec<fern_core::RailSection>,
    /// Scroll handle for the left navigation rail (needed so drag slot calculation accounts for scroll).
    rail_scroll: gpui::ScrollHandle,
    /// The rail icon currently being dragged, and the order before the drag started (for Escape).
    dragging_rail: Option<Section>,
    rail_drag_origin: Option<Vec<fern_core::RailSection>>,
    pub(crate) settings: Option<gpui::WindowHandle<SettingsWindow>>,
    update_notice: Option<Rc<ReleaseNotes>>,
    /// The Statistics page opened from the account menu, which covers the whole window.
    statistics_open: bool,
    /// The sidebar icon under the pointer, whose name shows beside it.
    nav_hovered: Option<Section>,
    #[cfg(test)]
    minimized_count: usize,
}

impl AppView {
    pub fn register_window_actions(main_window: gpui::WindowHandle<Self>, cx: &mut App) {
        // Action dispatch may be using the active window. Wait until GPUI has put it back.
        cx.on_action(move |_: &CloseWindow, cx| {
            let target = cx.active_window().unwrap_or_else(|| main_window.into());
            cx.defer(move |cx| {
                if target == main_window.into() {
                    let _ = main_window.update(cx, |view, window, cx| {
                        if crate::tray::hide_to_tray() {
                            return;
                        }
                        if view.prepare_to_leave_calendar(window, cx) {
                            view.close_settings(cx);
                            window.remove_window();
                        }
                    });
                } else if let Some(settings) = target.downcast::<SettingsWindow>() {
                    let _ = settings.update(cx, |settings, window, cx| settings.close(window, cx));
                } else if let Some(sticky) = target.downcast::<crate::sticky_note::StickyNote>() {
                    // StickyNote handles CloseWindow itself when focused; if Ctrl+W bubbles here
                    // while unfocused, forget it before removing its window across every OS.
                    let _ = sticky.update(cx, |note, window, cx| note.close_and_forget(window, cx));
                } else {
                    let _ = target.update(cx, |_, window, _| window.remove_window());
                }
            });
        });
        cx.on_action(move |_: &Quit, cx| {
            cx.defer(move |cx| {
                let can_quit = main_window.update(cx, |view, window, cx| {
                    let can_quit = view.prepare_to_leave_calendar(window, cx);
                    if !can_quit {
                        window.activate_window();
                    }
                    can_quit
                }).unwrap_or(true);
                if can_quit {
                    crate::hotkeys::shutdown(cx);
                    crate::tray::prepare_quit();
                    #[cfg(windows)]
                    crate::single_instance::release();
                    cx.quit();
                }
            });
        });
    }

    pub fn new(store: Rc<Store>, first_start: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app_view = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            #[cfg(not(windows))]
            if crate::tray::hide_to_tray() {
                return false;
            }
            app_view.update(cx, |view, cx| {
                if !view.prepare_to_leave_calendar(window, cx) {
                    return false;
                }
                view.close_settings(cx);
                true
            }).unwrap_or(true)
        });
        let todo = cx.new(|cx| TodoView::new(store.clone(), window, cx));
        let calendar = cx.new(|cx| CalendarView::new(store.clone(), window, cx));
        // A task clicked in the calendar opens in the task list with its details.
        cx.subscribe_in(&calendar, window, |this, _, calendar_view::OpenTask(id), window, cx| {
            this.open_task(*id, window, cx);
        })
        .detach();
        // A picker asked for in the calendar's right-click menu opens with the task in the list.
        cx.subscribe_in(&calendar, window, |this, _, calendar_view::OpenTaskPicker(id, what), window, cx| {
            let (id, what) = (*id, *what);
            if !this.select(Section::Tasks, window, cx) {
                return;
            }
            this.todo.update(cx, |todo, cx| {
                todo.open_task_in_all(id, cx);
                todo.start_detail(id, what, window, cx);
            });
        })
        .detach();
        cx.subscribe_in(&calendar, window, |this, _, crate::task_detail::StartFocus(id, title, stopwatch), window, cx| {
            let (target, stopwatch) = ((*id, title.clone()), *stopwatch);
            if this.select(Section::Pomodoro, window, cx) {
                this.focus.update(cx, |focus, cx| focus.start_for(Some(target.0), target.1, stopwatch, cx));
            }
        })
        .detach();
        cx.subscribe_in(&calendar, window, |this, _, calendar_view::OpenHabit(id), window, cx| {
            let id = *id;
            if this.select(Section::Habits, window, cx) {
                this.habits.update(cx, |habits, cx| habits.select(id, cx));
            }
        })
        .detach();
        // "Start Focus" in a task's menu starts a Pomodoro for it in the Focus section.
        cx.subscribe_in(&todo, window, |this, _, crate::task_detail::StartFocus(id, title, stopwatch), window, cx| {
            let (target, stopwatch) = ((*id, title.clone()), *stopwatch);
            if this.select(Section::Pomodoro, window, cx) {
                this.focus.update(cx, |focus, cx| focus.start_for(Some(target.0), target.1, stopwatch, cx));
            }
        })
        .detach();
        cx.subscribe_in(&todo, window, |this, _, crate::todo_view::OpenHabit(id), window, cx| {
            let id = *id;
            if this.select(Section::Habits, window, cx) {
                this.habits.update(cx, |habits, cx| habits.select(id, cx));
            }
        })
        .detach();
        cx.subscribe_in(&todo, window, |this, _, event: &crate::todo_view::StartHabitFocus, window, cx| {
            let (habit_id, title, stopwatch) = (event.habit_id, event.title.clone(), event.stopwatch);
            if this.select(Section::Pomodoro, window, cx) {
                this.focus.update(cx, |focus, cx| focus.start_for_habit(Some(habit_id), title, stopwatch, cx));
            }
        })
        .detach();
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        cx.subscribe_in(&matrix, window, |this, _, matrix_view::OpenTask(id), window, cx| {
            this.open_task(*id, window, cx);
        })
        .detach();
        cx.subscribe_in(&matrix, window, |this, _, crate::task_detail::StartFocus(id, title, stopwatch), window, cx| {
            let (target, stopwatch) = ((*id, title.clone()), *stopwatch);
            if this.select(Section::Pomodoro, window, cx) {
                this.focus.update(cx, |focus, cx| focus.start_for(Some(target.0), target.1, stopwatch, cx));
            }
        })
        .detach();
        let countdown = cx.new(|cx| CountdownView::new(store.clone(), cx));
        let habits = cx.new(|cx| HabitView::new(store.clone(), cx));
        // "Start Focus" in a habit's menu starts a Pomodoro or a stopwatch linked to it.
        cx.subscribe_in(&habits, window, |this, _, event: &crate::habit_view::StartFocus, window, cx| {
            let (habit_id, title, stopwatch) = (event.habit_id, event.title.clone(), event.stopwatch);
            if this.select(Section::Pomodoro, window, cx) {
                this.focus.update(cx, |focus, cx| focus.start_for_habit(habit_id, title, stopwatch, cx));
            }
        })
        .detach();
        let focus = cx.new(|cx| FocusView::new(store.clone(), cx));
        cx.observe(&focus, |_, focus, cx| {
            let tip = match focus.read(cx).running_clock() {
                Some((clock, true)) => format!("Fern — {clock} (Paused)"),
                Some((clock, false)) => format!("Fern — {clock}"),
                None => "Fern".to_owned(),
            };
            crate::tray::set_tooltip(&tip);
            cx.notify();
        })
        .detach();
        cx.subscribe(&focus, |this, _, crate::focus_view::StatsClosed, cx| {
            this.statistics_open = false;
            cx.notify();
        })
        .detach();
        let initial_prefs = store.preferences().unwrap_or_default();
        let store_features = initial_prefs.features;
        let rail_order = initial_prefs.rail_order;
        let has_sync_config = store.sync_config().ok().flatten().is_some();
        let always_on_top = crate::tray::TrayPrefs::current(&store, cx).always_on_top;
        let update_notice = release_notes::take_update_notice(&store, first_start).unwrap_or_else(|err| {
            eprintln!("failed to read the update notification: {err:#}");
            None
        }).map(Rc::new);
        let title_bar = cx.new(TitleBar::new);
        cx.subscribe_in(&title_bar, window, |this, _, ShowReleaseNotes(notes), window, cx| {
            this.open_settings(Some(notes.clone()), window, cx);
        })
        .detach();
        cx.observe_global::<crate::tray::TrayPrefs>(|this, cx| {
            this.always_on_top = cx.global::<crate::tray::TrayPrefs>().always_on_top;
            cx.notify();
        })
        .detach();
        // Mutated only by the debug-build dev state below.
        #[cfg_attr(not(debug_assertions), allow(unused_mut))]
        let mut this = Self {
            title_bar,
            store,
            section: Section::Tasks,
            todo,
            calendar,
            matrix,
            countdown,
            habits,
            focus,
            search: None,
            account_menu_open: false,
            notices: Notices::default(),
            notices_open: false,
            active_alerts: Vec::new(),
            sync: crate::sync::SyncStatus::Off,
            has_sync_config,
            always_on_top,
            features: store_features,
            rail_order,
            rail_scroll: gpui::ScrollHandle::new(),
            dragging_rail: None,
            rail_drag_origin: None,
            settings: None,
            update_notice,
            statistics_open: false,
            nav_hovered: None,
            #[cfg(test)]
            minimized_count: 0,
        };
        reminders::start(this.store.clone(), window.window_handle(), cx);
        this.start_sync(cx);
        settings_window::apply(&this.store.preferences().unwrap_or_default(), window, cx);
        // "Follow System Dark Mode" tracks the system's appearance while the app runs.
        cx.observe_window_appearance(window, |this, window, cx| {
            let preferences = this.store.preferences().unwrap_or_default();
            if preferences.theme.mode == fern_core::ThemeMode::System {
                settings_window::apply(&preferences, window, cx);
            }
        })
        .detach();
        #[cfg(debug_assertions)]
        crate::dev_state::apply(&mut this, window, cx);
        this
    }

    pub fn prepare_for_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.prepare_to_leave_calendar(window, cx);
        self.close_settings(cx);
    }

    fn prepare_to_leave_calendar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.section != Section::Calendar
            || self.calendar.update(cx, |calendar, cx| calendar.finish_task_before_navigation(cx))
        {
            return true;
        }
        window.focus(&self.calendar.focus_handle(cx), cx);
        false
    }

    pub(crate) fn select(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Search is a popover over the current screen, as in the original.
        if section == Section::Search {
            self.open_search(window, cx);
            return true;
        }
        if section != Section::Calendar && !self.prepare_to_leave_calendar(window, cx) {
            return false;
        }
        self.todo.update(cx, |todo, cx| todo.stop_search(cx));
        // Other sections change tasks and check habits in.
        if section == Section::Tasks && self.section != Section::Tasks {
            self.todo.update(cx, |todo, cx| todo.reload(cx));
        }
        if section == Section::Calendar {
            self.calendar.update(cx, |calendar, cx| calendar.reload(cx));
            window.focus(&self.calendar.focus_handle(cx), cx);
        }
        if section == Section::Matrix {
            self.matrix.update(cx, |matrix, cx| matrix.reload(cx));
            window.focus(&self.matrix.focus_handle(cx), cx);
        }
        if section == Section::Pomodoro {
            self.focus.update(cx, |focus, cx| focus.reload(cx));
            window.focus(&self.focus.focus_handle(cx), cx);
        }
        if section == Section::Habits {
            self.habits.update(cx, |habits, cx| habits.reload(cx));
            window.focus(&self.habits.focus_handle(cx), cx);
        }
        if section == Section::Countdown {
            self.countdown.update(cx, |countdown, cx| countdown.reload(cx));
            window.focus(&self.countdown.focus_handle(cx), cx);
        }
        self.section = section;
        cx.notify();
        true
    }

    fn start_rail_drag(&mut self, section: Section, cx: &mut Context<Self>) {
        self.rail_drag_origin = Some(self.rail_order.clone());
        self.dragging_rail = Some(section);
        self.nav_hovered = None;
        cx.notify();
    }

    fn cancel_rail_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.dragging_rail.take().is_some() {
            cx.stop_active_drag(window);
            if let Some(origin) = self.rail_drag_origin.take() {
                self.rail_order = origin;
            }
            self.nav_hovered = None;
            cx.notify();
            return true;
        }
        false
    }

    fn finish_rail_drag_up(
        &mut self,
        event: &gpui::MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != gpui::MouseButton::Left || self.dragging_rail.take().is_none() {
            return;
        }
        cx.stop_active_drag(window);
        let origin = self.rail_drag_origin.take();
        if origin.as_ref() != Some(&self.rail_order) {
            let order = self.rail_order.clone();
            if let Ok(prefs) = self.store.update_preferences(|p| p.rail_order = order) {
                self.rail_order = prefs.rail_order;
            }
        }
        self.nav_hovered = None;
        cx.notify();
    }

    fn move_rail_drag(
        &mut self,
        event: &gpui::DragMoveEvent<crate::drag::DraggedRailSection>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dragged = event.drag(cx).section;
        if self.dragging_rail.is_none() {
            self.rail_drag_origin = Some(self.rail_order.clone());
        }
        self.dragging_rail = Some(Section::from(dragged));
        let enabled_count = self
            .rail_order
            .iter()
            .filter(|&&s| Section::from(s).enabled(self.features))
            .count();
        if enabled_count == 0 {
            return;
        }
        let top_y = title_bar::height() + px(sidebar::SECTIONS_TOP_OFFSET);
        let content_y = event.event.position.y - top_y - self.rail_scroll.offset().y;
        let slot = ((f32::from(content_y) / sidebar::SLOT_HEIGHT).floor() as isize)
            .clamp(0, (enabled_count - 1) as isize) as usize;
        let current = self
            .rail_order
            .iter()
            .copied()
            .filter(|&s| Section::from(s).enabled(self.features))
            .position(|s| s == dragged)
            .unwrap_or(0);
        if current != slot {
            self.rail_order = reorder_enabled_rail(&self.rail_order, self.features, current, slot);
            self.nav_hovered = None;
            cx.notify();
        }
    }

    pub(crate) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search = cx.new(|cx| SearchModal::new(self.store.clone(), cx));
        cx.subscribe_in(&search, window, |this, _, event, window, cx| {
            this.search = None;
            match event {
                SearchEvent::OpenTask(id) => {
                    this.open_task(*id, window, cx);
                }
                SearchEvent::OpenHabit(id) => {
                    let id = *id;
                    if this.select(Section::Habits, window, cx) {
                        this.habits.update(cx, |habits, cx| habits.select(id, cx));
                    }
                }
                SearchEvent::OpenView(fern_core::View::Search(query)) => {
                    let query = query.clone();
                    if this.select(Section::Tasks, window, cx) {
                        this.todo.update(cx, |todo, cx| todo.show_search_results(query, cx));
                    }
                }
                SearchEvent::OpenView(view) => {
                    let view = view.clone();
                    this.show_view(view, window, cx);
                }
                SearchEvent::CreateTask(query) => {
                    let query = query.clone();
                    this.add_quick_task_with(&query, None, None, None, cx);
                    this.select(Section::Tasks, window, cx);
                }
                SearchEvent::Dismissed => {}
            }
            cx.notify();
        })
        .detach();
        window.focus(&search.focus_handle(cx), cx);
        self.search = Some(search);
        cx.notify();
    }

    fn new_task(&mut self, _: &NewTask, window: &mut Window, cx: &mut Context<Self>) {
        self.quick_add(window, cx);
    }

    /// A task from the Quick Add window.
    pub fn add_quick_task_with(
        &mut self,
        text: &str,
        date: Option<chrono::NaiveDate>,
        priority: Option<fern_core::Priority>,
        list: Option<Option<fern_core::Id>>,
        cx: &mut Context<Self>,
    ) {
        self.todo.update(cx, |todo, cx| todo.add_quick_with(text, date, priority, list, cx));
    }

    /// Lists and quick-add recognition preference for the Quick Add window.
    pub fn quick_add_context(&self) -> (Vec<fern_core::List>, fern_core::Recognition) {
        let lists = self.store.lists().unwrap_or_default().into_iter().filter(|l| !l.archived).collect();
        let recognition = self.store.preferences().map(|p| p.recognition).unwrap_or_default();
        (lists, recognition)
    }

    /// The add field of the task list, focused: New Task.
    pub fn quick_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.select(Section::Tasks, window, cx) {
            self.todo.update(cx, |todo, cx| todo.focus_new_task(window, cx));
        }
    }

    fn search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Search, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.todo.update(cx, |todo, cx| todo.toggle_lists(cx));
    }

    fn show_view(&mut self, view: fern_core::View, window: &mut Window, cx: &mut Context<Self>) {
        if self.select(Section::Tasks, window, cx) {
            self.todo.update(cx, |todo, cx| todo.select_view(view, window, cx));
        }
    }

    fn set_account_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        self.account_menu_open = open;
        cx.notify();
    }

    fn render_account_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let topmost = self.always_on_top;
        popover(cx)
            .min_w(px(196.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_account_menu(false, cx)))
            .child(
                ListItem::new("account-settings")
                    .start_slot(Icon::new(IconName::Settings).color(Color::Icon))
                    .child(Label::new(strings.settings))
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(None, window, cx))),
            )
            .child(
                ListItem::new("account-statistics")
                    .start_slot(Icon::new(IconName::ChartNoAxesColumn).color(Color::Icon))
                    .child(Label::new(strings.statistics))
                    .on_click(cx.listener(|this, _, _, cx| this.open_statistics(cx))),
            )
            .child(
                ListItem::new("account-shortcuts")
                    .start_slot(Icon::new(IconName::Keyboard).color(Color::Icon))
                    .child(Label::new(strings.shortcuts))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings_page(settings_window::Page::Shortcuts, window, cx);
                    })),
            )
            .child(div().my_1().child(Divider::horizontal()))
            .child(
                ListItem::new("account-always-on-top")
                    .start_slot(Icon::new(if topmost { IconName::PinOff } else { IconName::Pin }).color(if topmost { Color::Accent } else { Color::Icon }))
                    .child(Label::new(strings.tray_always_on_top).color(if topmost { Color::Accent } else { Color::Default }))
                    .when(topmost, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.account_menu_open = false;
                        if let Err(err) = crate::tray::set_always_on_top(&this.store, !topmost, cx) {
                            eprintln!("failed to toggle always_on_top: {err:#}");
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub(crate) fn open_statistics(&mut self, cx: &mut Context<Self>) {
        self.account_menu_open = false;
        self.statistics_open = true;
        self.focus.update(cx, |focus, cx| focus.open_stats_overview(cx));
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(settings) = self.settings.take() {
            let _ = settings.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn apply_settings_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The default list, smart list visibility, mini calendar and week start show up in the views.
        self.todo.update(cx, |todo, cx| todo.reload(cx));
        self.calendar.update(cx, |calendar, cx| calendar.reload(cx));
        // A section turned off in Features closes.
        let prefs = self.store.preferences().unwrap_or_default();
        self.features = prefs.features;
        self.rail_order = prefs.rail_order;
        self.has_sync_config = self.store.sync_config().ok().flatten().is_some();
        if !self.section.enabled(self.features) {
            self.select(Section::Tasks, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn open_settings(&mut self, notes: Option<Rc<ReleaseNotes>>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_at(notes, None, window, cx);
    }

    pub(crate) fn open_settings_page(
        &mut self,
        page: settings_window::Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_at(None, Some(page), window, cx);
    }

    fn open_settings_at(
        &mut self,
        notes: Option<Rc<ReleaseNotes>>,
        initial_page: Option<settings_window::Page>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.account_menu_open = false;
        if notes.is_some() {
            self.search = None;
        }
        if let Some(settings) = self.settings {
            let existing = settings.update(cx, |settings, settings_window, cx| {
                if let Some(notes) = notes.clone() {
                    settings.show_release_notes(notes, settings_window, cx);
                } else {
                    if let Some(page) = initial_page {
                        settings.set_page(page, cx);
                    }
                    settings_window.focus(&settings.focus_handle(cx), cx);
                }
                settings_window.activate_window();
            });
            if existing.is_ok() {
                cx.notify();
                return;
            }
            self.settings = None;
        }
        let handle = match settings_window::open(self.store.clone(), notes, initial_page, window, cx) {
            Ok(handle) => handle,
            Err(err) => {
                eprintln!("failed to open Settings: {err:#}");
                return;
            }
        };
        if let Ok(settings) = handle.update(cx, |_, _, cx| cx.entity()) {
            cx.subscribe_in(&settings, window, |this, _, event, window, cx| match event {
                SettingsEvent::Changed => this.apply_settings_changes(window, cx),
                SettingsEvent::Dismissed => {
                    this.settings = None;
                    cx.notify();
                }
            })
            .detach();
        }
        self.settings = Some(handle);
        cx.notify();
    }

    fn render_update_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notes = self.update_notice.as_ref()?;
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let message = strings.update_installed.replace("{version}", &notes.version);
        Some(
            popover(cx)
                .id("update-notice")
                .debug_selector(|| "update-notice".to_owned())
                .w(px(296.))
                .p_2()
                .gap_1()
                .child(
                    h_flex().items_start().gap_2()
                        .child(div().flex_none().pl_1().pt(px(5.)).child(
                            Icon::new(IconName::CircleCheck).size(IconSize::Small).color(Color::Accent),
                        ))
                        .child(div().flex_1().min_w_0().pt_1().line_height(px(20.)).child(
                            Label::new(message).weight(gpui::FontWeight::MEDIUM),
                        ))
                        .child(
                            action_button("dismiss-update-notice", cx)
                                .debug_selector(|| "dismiss-update-notice".to_owned())
                                .size(px(26.))
                                .min_h(px(26.))
                                .px_0()
                                .justify_center()
                                .tooltip(Tooltip::text(strings.close))
                                .child(Icon::new(IconName::X).size(IconSize::Small).color(Color::Muted))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_notice = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    h_flex().pl(px(26.)).pb_1().child(
                        action_button("updated-whats-new", cx)
                            .debug_selector(|| "updated-whats-new".to_owned())
                            .border_color(colors.border)
                            .child(Label::new(strings.whats_new).size(LabelSize::Small).weight(gpui::FontWeight::MEDIUM))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(notes) = this.update_notice.take() {
                                    this.open_settings(Some(notes), window, cx);
                                }
                            })),
                    ),
                )
                .into_any_element(),
        )
    }
}

impl AppView {
    /// Opens a task from a `fern://task/…` link, in All with its details.
    pub fn open_task(&mut self, task: fern_core::Id, window: &mut Window, cx: &mut Context<Self>) {
        if self.select(Section::Tasks, window, cx) {
            self.todo.update(cx, |todo, cx| {
                todo.open_task_in_all(task, cx);
            });
        }
    }

    fn dismiss_any_transient(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.cancel_rail_drag(window, cx) {
            return true;
        }
        if self.account_menu_open {
            self.set_account_menu(false, cx);
            return true;
        }
        if self.notices_open {
            self.set_notices_open(false, cx);
            return true;
        }
        if self.search.take().is_some() {
            cx.notify();
            return true;
        }
        if self.statistics_open {
            if self.focus.update(cx, |focus, cx| focus.dismiss_transient(cx)) {
                return true;
            }
            self.statistics_open = false;
            self.focus.update(cx, |focus, cx| focus.close_stats(cx));
            cx.notify();
            return true;
        }
        match self.section {
            Section::Tasks | Section::Search => {
                self.todo.update(cx, |todo, cx| todo.dismiss_transient(window, cx))
            }
            Section::Calendar => {
                self.calendar.update(cx, |cal, cx| cal.try_dismiss_transient(window, cx))
            }
            Section::Matrix => {
                self.matrix.update(cx, |matrix, cx| matrix.try_dismiss_transient(window, cx))
            }
            Section::Habits => self.habits.update(cx, |habits, cx| habits.dismiss_transient(cx)),
            Section::Pomodoro => self.focus.update(cx, |focus, cx| focus.dismiss_transient(cx)),
            Section::Countdown => {
                self.countdown.update(cx, |countdown, cx| countdown.dismiss_transient(cx))
            }
        }
    }

    fn dismiss_transient(&mut self, _: &DismissTransient, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dismiss_any_transient(window, cx) {
            cx.propagate();
        }
    }

    fn minimize_window(&mut self, _: &MinimizeWindow, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismiss_any_transient(window, cx) {
            return;
        }
        #[cfg(test)]
        {
            let _ = window;
            self.minimized_count += 1;
            cx.notify();
        }
        #[cfg(not(test))]
        if !crate::tray::hide_to_tray_if_enabled() {
            window.minimize_window();
        }
    }

    /// Syncs on start, whenever there are local changes to send, and every few minutes to fetch
    /// other devices' changes. Screenshots of the debug build do not sync.
    fn start_sync(&mut self, cx: &mut Context<Self>) {
        if std::env::var_os("FERN_DEV_STATE").is_some() {
            return;
        }
        let db_path = cx.try_global::<crate::DbPath>().map(|p| p.0.clone());
        cx.spawn(async move |this, cx| {
            let mut since_pull = std::time::Duration::ZERO;
            let mut first = true;
            loop {
                let due = if first || since_pull >= crate::sync::PULL_EVERY {
                    if this.upgrade().is_none() {
                        break;
                    }
                    true
                } else if let Some(path) = db_path.clone() {
                    if this.upgrade().is_none() {
                        break;
                    }
                    cx.background_executor()
                        .spawn(async move {
                            Store::open(&path)
                                .ok()
                                .and_then(|s| s.has_unsent_changes().ok())
                                .unwrap_or(false)
                        })
                        .await
                } else {
                    match this.update(cx, |this, _| this.store.has_unsent_changes().unwrap_or(false)) {
                        Ok(has) => has,
                        Err(_) => break,
                    }
                };
                if due {
                    since_pull = std::time::Duration::ZERO;
                    if this.update(cx, |this, cx| this.sync_now(cx)).is_err() {
                        break;
                    }
                }
                first = false;
                cx.background_executor().timer(crate::sync::PUSH_CHECK_EVERY).await;
                since_pull += crate::sync::PUSH_CHECK_EVERY;
            }
        })
        .detach();
    }

    /// One sync with the server from Settings → Sync: sends the local changes, applies the others
    /// and refreshes the views.
    pub fn sync_now(&mut self, cx: &mut Context<Self>) {
        if self.sync == crate::sync::SyncStatus::Running {
            return;
        }
        let config = match self.store.sync_config() {
            Ok(Some(config)) => config,
            Ok(None) => {
                self.sync = crate::sync::SyncStatus::Off;
                cx.notify();
                return;
            }
            Err(err) => {
                self.sync = crate::sync::SyncStatus::Failed(format!("{err:#}"));
                cx.notify();
                return;
            }
        };
        let request = match self.store.sync_request() {
            Ok(request) => request,
            Err(err) => {
                self.sync = crate::sync::SyncStatus::Failed(format!("{err:#}"));
                cx.notify();
                return;
            }
        };
        self.sync = crate::sync::SyncStatus::Running;
        cx.notify();
        let body = request.body.clone();
        let sent_config = config.clone();
        let answer = cx.background_spawn(async move { crate::sync::post(&sent_config, &body) });
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| {
                if this.store.sync_config().ok().flatten().as_ref() != Some(&config) {
                    this.sync = crate::sync::SyncStatus::Off;
                    cx.notify();
                    return;
                }
                match answer.and_then(|answer| this.store.finish_sync(&request, &answer)) {
                    Ok(received) => {
                        if received > 0 {
                            this.reload_all(cx);
                        }
                        this.sync_files(config, cx);
                    }
                    Err(err) => this.complete_sync(Err(err), cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// After a sync, sends the attached files the server lacks and fetches the missing ones.
    /// Statting attachments (`files_to_sync`) and HTTP transfers both run off the UI thread.
    fn sync_files(&mut self, config: fern_core::sync::SyncConfig, cx: &mut Context<Self>) {
        let dir = match crate::attachments_dir() {
            Ok(dir) => dir,
            Err(err) => {
                self.complete_sync(Err(err), cx);
                return;
            }
        };
        let transfer_config = config.clone();
        let transfer = if let Some(db_path) = cx.try_global::<crate::DbPath>().map(|p| p.0.clone()) {
            cx.background_spawn(async move {
                let bg_store = Store::open(&db_path)?;
                let (upload, download) = bg_store.files_to_sync(&dir)?;
                if upload.is_empty() && download.is_empty() {
                    return Ok(None);
                }
                Ok(Some(crate::sync::transfer_files(&transfer_config, &dir, &upload, &download)))
            })
        } else {
            let (upload, download) = match self.store.files_to_sync(&dir) {
                Ok(files) => files,
                Err(err) => {
                    self.complete_sync(Err(err), cx);
                    return;
                }
            };
            if upload.is_empty() && download.is_empty() {
                self.complete_sync(Ok(()), cx);
                return;
            }
            cx.background_spawn(async move {
                Ok(Some(crate::sync::transfer_files(&transfer_config, &dir, &upload, &download)))
            })
        };
        cx.spawn(async move |this, cx| {
            let outcome = transfer.await;
            this.update(cx, |this, cx| {
                let outcome = match outcome {
                    Ok(Some(outcome)) => outcome,
                    Ok(None) => {
                        this.complete_sync(Ok(()), cx);
                        return;
                    }
                    Err(err) => {
                        this.complete_sync(Err(err), cx);
                        return;
                    }
                };
                if outcome.downloaded {
                    this.reload_all(cx);
                }
                if this.store.sync_config().ok().flatten().as_ref() != Some(&config) {
                    this.sync = crate::sync::SyncStatus::Off;
                    cx.notify();
                    return;
                }
                let result = this.store.mark_files_synced(&outcome.synced).and_then(|()| {
                    if outcome.failed.is_empty() {
                        return Ok(());
                    }
                    let strings = i18n::t(cx);
                    let errors = outcome.failed.iter().map(|(name, error)| {
                        let display_name = name.get(..36)
                            .filter(|id| fern_core::Id::parse_str(id).is_ok())
                            .and_then(|_| name.get(36..))
                            .and_then(|suffix| suffix.strip_prefix('-'))
                            .unwrap_or(name);
                        strings.sync_file_failed
                            .replace("{file}", display_name)
                            .replace("{error}", &crate::sync::attachment_error(error, strings))
                    }).collect::<Vec<_>>().join("\n");
                    Err(anyhow::anyhow!(errors))
                });
                this.complete_sync(result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn complete_sync(&mut self, result: anyhow::Result<()>, cx: &mut Context<Self>) {
        self.sync = match result {
            Ok(()) => crate::sync::SyncStatus::Done(chrono::Local::now().naive_local()),
            Err(err) => crate::sync::SyncStatus::Failed(format!("{err:#}")),
        };
        cx.notify();
    }

    /// Re-reads every view, after changes came from another device.
    fn reload_all(&mut self, cx: &mut Context<Self>) {
        let prefs = self.store.preferences().unwrap_or_default();
        self.features = prefs.features;
        self.rail_order = prefs.rail_order;
        self.has_sync_config = self.store.sync_config().ok().flatten().is_some();
        crate::calendar::set_week_start(prefs.week_start);
        crate::calendar::set_hour12(prefs.hour12);
        crate::calendar::set_date_format(prefs.date_format);
        crate::calendar::set_week_numbers(prefs.week_numbers);
        self.todo.update(cx, |view, cx| view.reload(cx));
        self.calendar.update(cx, |view, cx| view.reload(cx));
        self.matrix.update(cx, |view, cx| view.reload(cx));
        self.countdown.update(cx, |view, cx| view.reload(cx));
        self.habits.update(cx, |view, cx| view.reload(cx));
        self.focus.update(cx, |view, cx| view.reload(cx));
        cx.notify();
    }

    /// The sync button's tooltip: "Sync" and how the last sync went.
    fn sync_tooltip(&self, strings: &'static i18n::Strings) -> String {
        let status = match &self.sync {
            crate::sync::SyncStatus::Off if self.has_sync_config => strings.sync_never.to_owned(),
            crate::sync::SyncStatus::Off => strings.sync_off.to_owned(),
            crate::sync::SyncStatus::Running => strings.sync_running.to_owned(),
            crate::sync::SyncStatus::Done(at) => strings.sync_done.replace("{time}", &crate::calendar::format_time(at.time(), strings)),
            crate::sync::SyncStatus::Failed(error) => strings.sync_failed.replace("{error}", error),
        };
        format!("{}\n{status}", strings.sync)
    }

    pub fn add_notices(&mut self, notices: Vec<Notice>, cx: &mut Context<Self>) {
        if !notices.is_empty() {
            self.notices.push(notices);
            cx.notify();
        }
    }

    pub fn push_active_alerts(&mut self, mut notices: Vec<Notice>, cx: &mut Context<Self>) {
        if notices.is_empty() {
            return;
        }
        notices.sort_by_key(|n| std::cmp::Reverse(n.at));
        for notice in notices.into_iter().rev() {
            self.active_alerts.retain(|existing| existing.target != notice.target);
            self.active_alerts.insert(0, notice);
        }
        self.active_alerts.truncate(5);
        cx.notify();
    }

    pub fn has_active_alert(&self, target: Target) -> bool {
        self.active_alerts.iter().any(|n| n.target == target)
    }

    /// Opening the list marks the notifications read, which clears the badge.
    pub(crate) fn set_notices_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.notices_open = open;
        if open {
            self.notices.mark_read();
        }
        cx.notify();
    }

    fn open_notice(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.active_alerts.retain(|n| n.target != target);
        self.set_notices_open(false, cx);
        match target {
            Target::Task(task) => self.open_task(task, window, cx),
            Target::Habit(habit) => {
                if self.select(Section::Habits, window, cx) {
                    self.habits.update(cx, |habits, cx| habits.select(habit, cx));
                }
            }
            Target::Countdown(_) => {
                self.select(Section::Countdown, window, cx);
            }
        }
    }

    /// A click on a reminder, in the bell, floating banner, or desktop notification.
    pub fn answer_notice(&mut self, notice: &Notice, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        self.active_alerts.retain(|n| n.target != notice.target);
        let Reply::Action(action) = reply else {
            window.activate_window();
            self.open_notice(notice.target, window, cx);
            return;
        };
        let now = chrono::Local::now().naive_local();
        let snooze_minutes = match action {
            NoticeAction::Snooze5m => Some(5),
            NoticeAction::Snooze | NoticeAction::Snooze15m => Some(reminders::SNOOZE_MINUTES),
            NoticeAction::Snooze1h => Some(60),
            _ => None,
        };
        let mut touched_tasks = false;
        let mut touched_habits = false;
        let done = match notice.target {
            Target::Task(id) => {
                touched_tasks = true;
                if let Some(mins) = snooze_minutes {
                    self.store.snooze_task(id, now + chrono::Duration::minutes(mins))
                } else {
                    match action {
                        NoticeAction::Complete => {
                            let res = self.store.complete(id, true).map(|_| ());
                            if res.is_ok() {
                                desktop_common::sound::play_complete(&self.store);
                            }
                            res
                        }
                        NoticeAction::Dismiss => self.store.clear_task_snooze(id),
                        _ => Ok(()),
                    }
                }
            }
            Target::Habit(habit) => {
                touched_habits = true;
                if let Some(mins) = snooze_minutes {
                    self.store.snooze_habit(habit, now + chrono::Duration::minutes(mins))
                } else {
                    match action {
                        NoticeAction::CheckIn => {
                            let today = now.date();
                            if self.store.checkins(habit).unwrap_or_default().contains(&today) {
                                Ok(())
                            } else {
                                let res = self.store.toggle_checkin(habit, today).map(|_| ());
                                if res.is_ok() {
                                    desktop_common::sound::play_complete(&self.store);
                                }
                                res
                            }
                        }
                        NoticeAction::PlusOne => {
                            let step = self
                                .store
                                .habits()
                                .unwrap_or_default()
                                .into_iter()
                                .find(|h| h.id == habit)
                                .and_then(|h| h.goal)
                                .map_or(1.0, |g| g.step_amount());
                            self.store.add_checkin_amount(habit, now.date(), step).map(|_| ())
                        }
                        NoticeAction::Record => {
                            window.activate_window();
                            self.set_notices_open(false, cx);
                            if !self.select(Section::Habits, window, cx) {
                                return;
                            }
                            self.habits.update(cx, |habits, cx| habits.record(habit, window, cx));
                            Ok(())
                        }
                        _ => Ok(()),
                    }
                }
            }
            Target::Countdown(_) => Ok(()),
        };
        if let Err(err) = done {
            eprintln!("failed to answer the reminder: {err:#}");
        }
        self.notices.items.retain(|n| n != notice);
        desktop_common::data_version::data_changed(cx.entity_id(), cx);
        if touched_tasks {
            self.todo.update(cx, |todo, cx| todo.reload(cx));
            self.calendar.update(cx, |calendar, cx| calendar.reload(cx));
            self.matrix.update(cx, |matrix, cx| matrix.reload(cx));
        }
        if touched_habits {
            self.habits.update(cx, |habits, cx| habits.reload(cx));
        }
        cx.notify();
    }

    fn render_active_alerts(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.active_alerts.is_empty() {
            return None;
        }
        let strings = i18n::t(cx);
        let today = chrono::Local::now().date_naive();
        let theme = cx.theme().clone();
        Some(
            v_flex()
                .id("active-reminder-alerts")
                .debug_selector(|| "active-reminder-alerts".to_owned())
                .absolute()
                .bottom(px(16.))
                .right(px(16.))
                .w(px(320.))
                .gap_2()
                .children(self.active_alerts.iter().enumerate().map(|(i, notice)| {
                    let target = notice.target;
                    let focus_title = notice.title.clone();
                    let (when, _) = crate::task_rows::due_label(notice.at.date(), Some(notice.at.time()), today, strings);
                    let actions = notice.actions();
                    let dismiss_notice = notice.clone();
                    let focus_notice = notice.clone();
                    let can_focus = matches!(target, Target::Task(_) | Target::Habit(_));
                    v_flex()
                        .id(SharedString::from(format!("alert-{i}")))
                        .p_3()
                        .gap_2()
                        .rounded(px(10.))
                        .bg(theme.colors().elevated_surface_background)
                        .border_1()
                        .border_color(if notice.constant { theme.colors().accent } else { theme.colors().border })
                        .shadow_md()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(Icon::new(IconName::AlarmClock).size(IconSize::Small).color(Color::Accent))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("alert-{i}-open")))
                                        .flex_1()
                                        .min_w_0()
                                        .cursor_pointer()
                                        .tooltip(Tooltip::text(notice.title.clone()))
                                        .child(Label::new(notice.title.clone()).truncate())
                                        .on_click(cx.listener(move |this, _, window, cx| this.open_notice(target, window, cx))),
                                )
                                .when(can_focus, |el| {
                                    el.child(
                                        IconButton::new(SharedString::from(format!("alert-{i}-focus")), IconName::CircleDot)
                                            .size(px(22.))
                                            .icon_size(IconSize::XSmall)
                                            .icon_color(Color::Muted)
                                            .hover_icon_color(Color::Accent)
                                            .tooltip(Tooltip::text(strings.start_pomo))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.active_alerts.retain(|n| n.target != focus_notice.target);
                                                this.notices.items.retain(|n| n != &focus_notice);
                                                match target {
                                                    Target::Task(id) => {
                                                        this.focus.update(cx, |focus, cx| {
                                                            focus.start_for(Some(id), focus_title.clone(), false, cx);
                                                        });
                                                    }
                                                    Target::Habit(id) => {
                                                        this.focus.update(cx, |focus, cx| {
                                                            focus.start_for_habit(Some(id), focus_title.clone(), false, cx);
                                                        });
                                                    }
                                                    Target::Countdown(_) => {}
                                                }
                                                let _ = this.select(Section::Pomodoro, window, cx);
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(
                                    div()
                                        .id(SharedString::from(format!("alert-{i}-dismiss")))
                                        .debug_selector(move || format!("alert-{i}-dismiss"))
                                        .child(
                                            IconButton::new(SharedString::from(format!("alert-{i}-dismiss-btn")), IconName::X)
                                                .size(px(22.))
                                                .icon_size(IconSize::XSmall)
                                                .icon_color(Color::Muted)
                                                .hover_icon_color(Color::Icon)
                                                .tooltip(Tooltip::text(strings.dismiss))
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    cx.stop_propagation();
                                                    this.answer_notice(&dismiss_notice, Reply::Action(NoticeAction::Dismiss), window, cx);
                                                })),
                                        ),
                                ),
                        )
                        .child(Label::new(when).size(LabelSize::XSmall).color(Color::Muted))
                        .when(!actions.is_empty(), |el| {
                            el.child(
                                h_flex().flex_wrap().gap_1().children(actions.iter().map(|&action| {
                                    let notice = notice.clone();
                                    let key = action.key();
                                    let primary = matches!(
                                        action,
                                        NoticeAction::Complete | NoticeAction::CheckIn | NoticeAction::PlusOne
                                    );
                                    div()
                                        .id(SharedString::from(format!("alert-{i}-{key}")))
                                        .debug_selector(move || format!("alert-{i}-{key}"))
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .when(primary, |btn| {
                                            btn.bg(theme.colors().accent)
                                                .hover(|s| s.opacity(0.9))
                                                .child(Label::new(action.label(strings)).size(LabelSize::Small).color(Color::OnAccent))
                                        })
                                        .when(!primary, |btn| {
                                            btn.border_1()
                                                .border_color(theme.colors().border)
                                                .hover(|s| s.bg(theme.colors().element_hover))
                                                .child(Label::new(action.label(strings)).size(LabelSize::Small))
                                        })
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.answer_notice(&notice, Reply::Action(action), window, cx);
                                        }))
                                })),
                            )
                        })
                }))
                .into_any_element(),
        )
    }

    fn render_notices(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let today = chrono::Local::now().date_naive();
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(300.))
            .max_h(px(420.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_notices_open(false, cx)))
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .items_center()
                    .child(div().flex_1().child(Label::new(strings.notifications).weight(gpui::FontWeight::SEMIBOLD)))
                    .when(!self.notices.items.is_empty(), |el| {
                        el.child(
                            div()
                                .id("notices-clear-all")
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.notices.items.clear();
                                    this.notices.unread = 0;
                                    cx.notify();
                                }))
                                .child(Label::new(strings.clear).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    }),
            )
            .when(self.notices.items.is_empty(), |el| {
                el.child(
                    v_flex()
                        .py_6()
                        .px_3()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Bell).size(IconSize::Medium).color(Color::Muted))
                        .child(Label::new(strings.no_notifications).size(LabelSize::Small).color(Color::Muted)),
                )
            })
            .child(
                v_scroll(
                    "notices-scroll",
                    v_flex().id("notices").children(self.notices.items.iter().enumerate().map(|(i, notice)| {
                        let target = notice.target;
                        let (when, _) = crate::task_rows::due_label(notice.at.date(), Some(notice.at.time()), today, strings);
                        let actions = notice.actions();
                        let buttons = actions.iter().map(|&action| {
                            let notice = notice.clone();
                            div()
                                .id(SharedString::from(format!("notice-{i}-{}", action.key())))
                                .px_2()
                                .py_0p5()
                                .rounded(px(4.))
                                .border_1().border_color(theme.colors().border)
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .cursor_pointer()
                                .child(Label::new(action.label(strings)).size(LabelSize::Small))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.answer_notice(&notice, Reply::Action(action), window, cx);
                                }))
                        });
                        let item_height = if actions.is_empty() {
                            44.
                        } else if actions.len() > 3 {
                            92.
                        } else {
                            72.
                        };
                        ListItem::new(SharedString::from(format!("notice-{i}")))
                            .height(px(item_height))
                            .start_slot(Icon::new(IconName::AlarmClock).color(Color::Accent))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .py_1()
                                    .gap_0p5()
                                    .child(Label::new(notice.title.clone()).truncate())
                                    .child(Label::new(when).size(LabelSize::XSmall).color(Color::Muted))
                                    .child(h_flex().flex_wrap().gap_1().children(buttons)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| this.open_notice(target, window, cx)))
                    })),
                )
                .min_h_0()
                .compact(),
            )
            .into_any_element()
    }

    /// Starts a Pomodoro (`stopwatch == false`) or stopwatch (`stopwatch == true`) from the tray
    /// without opening the main window, and opens the floating mini clock.
    pub fn start_focus_from_tray(&mut self, stopwatch: bool, cx: &mut Context<Self>) {
        self.focus.update(cx, |focus, cx| focus.start_mode(stopwatch, cx));
        crate::focus_view::focus_mini::open(self.focus.clone(), cx);
    }

    /// Opens or closes the floating focus mini window (global shortcut).
    pub fn toggle_focus_mini(&mut self, cx: &mut Context<Self>) {
        crate::focus_view::focus_mini::toggle(self.focus.clone(), cx);
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::touch::begin_frame(window);
        let theme = cx.theme().clone();
        if !cx.has_active_drag() && self.dragging_rail.take().is_some()
            && let Some(origin) = self.rail_drag_origin.take()
        {
            self.rail_order = origin;
        }
        let this = cx.entity().downgrade();
        let on_select = move |section, window: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.select(section, window, cx)).ok();
        };
        let hover_target = cx.entity().downgrade();
        let on_nav_hover = move |section: Section, hovered: bool, cx: &mut App| {
            hover_target
                .update(cx, |this, cx| {
                    let now = if hovered { Some(section) } else { this.nav_hovered.filter(|s| *s != section) };
                    if this.nav_hovered != now {
                        this.nav_hovered = now;
                        cx.notify();
                    }
                })
                .ok();
        };
        let on_start_rail_drag = {
            let this = cx.entity().downgrade();
            move |section, cx: &mut App| {
                this.update(cx, |this, cx| this.start_rail_drag(section, cx)).ok();
            }
        };
        let on_avatar = cx.listener(|this, _, _, cx| this.set_account_menu(!this.account_menu_open, cx));
        let account_menu = self.account_menu_open.then(|| {
            PopoverAttachment::new(Placement::RightTop, self.render_account_menu(cx)).into_any_element()
        });
        let features = self.features;
        let sync_button = self.has_sync_config.then(|| sidebar::SyncButton {
            tooltip: self.sync_tooltip(i18n::t(cx)),
            failed: matches!(self.sync, crate::sync::SyncStatus::Failed(_)),
            on_click: Box::new(cx.listener(|this, _, _, cx| this.sync_now(cx))),
        });
        let bell_panel = if self.notices_open {
            Some(self.render_notices(cx))
        } else if self.settings.is_none() && self.search.is_none() && !self.account_menu_open {
            self.render_update_notice(cx)
        } else {
            None
        };
        let bell = sidebar::Bell {
            unread: self.notices.unread,
            on_click: Box::new(cx.listener(|this, _, _, cx| {
                this.update_notice = None;
                let open = !this.notices_open;
                this.set_notices_open(open, cx)
            })),
            menu: bell_panel.map(|panel| PopoverAttachment::new(Placement::RightBottom, panel).into_any_element()),
        };

        let content = h_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .key_context("Workspace")
            .on_action(cx.listener(Self::minimize_window))
            .on_action(cx.listener(Self::new_task))
            .on_action(cx.listener(Self::search))
            .on_action(cx.listener(|this, _: &SyncNow, _, cx| this.sync_now(cx)))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(|this, _: &ShowShortcuts, window, cx| {
                this.open_settings_page(settings_window::Page::Shortcuts, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(None, window, cx)))
            .on_action(cx.listener(|this, _: &ShowAll, window, cx| this.show_view(fern_core::View::All, window, cx)))
            .on_action(cx.listener(|this, _: &ShowInbox, window, cx| this.show_view(fern_core::View::Inbox, window, cx)))
            .on_action(cx.listener(|this, _: &ShowToday, window, cx| this.show_view(fern_core::View::Today, window, cx)))
            .on_action(cx.listener(|this, _: &ShowTomorrow, window, cx| {
                this.show_view(fern_core::View::Tomorrow, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowNext7Days, window, cx| {
                this.show_view(fern_core::View::Next7Days, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowCompleted, window, cx| {
                this.show_view(fern_core::View::Completed, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowWontDo, window, cx| {
                this.show_view(fern_core::View::WontDo, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowTrash, window, cx| {
                this.show_view(fern_core::View::Trash, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PrintList, _, cx| {
                if matches!(this.section, Section::Tasks | Section::Search) {
                    this.todo.update(cx, |todo, cx| todo.print_list(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &PrintDetail, _, cx| {
                if matches!(this.section, Section::Tasks | Section::Search) {
                    this.todo.update(cx, |todo, cx| todo.print_selected_detail(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &TogglePause, _, cx| {
                this.focus.update(cx, |focus, cx| focus.toggle_pause_or_start(cx));
            }))
            .on_action(cx.listener(|this, _: &StartStop, window, cx| {
                this.focus.update(cx, |focus, cx| focus.start_or_end(window, cx));
            }))
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .when(self.statistics_open, |el| el.child(div().size_full().child(self.focus.clone())))
            .when(!self.statistics_open, |el| {
                let focus_clock = self.focus.read(cx).running_clock();
                el.child(sidebar::render(
                    self.section,
                    self.nav_hovered,
                    &self.rail_order,
                    self.dragging_rail,
                    &self.rail_scroll,
                    on_nav_hover,
                    on_select,
                    on_start_rail_drag,
                    on_avatar,
                    account_menu,
                    bell,
                    sync_button,
                    focus_clock,
                    features,
                    cx,
                ))
                .child(match self.section {
                    Section::Tasks | Section::Search => div().flex_1().min_w_0().h_full().child(self.todo.clone()),
                    Section::Calendar => div().flex_1().min_w_0().h_full().child(self.calendar.clone()),
                    Section::Matrix => div().flex_1().min_w_0().h_full().child(self.matrix.clone()),
                    Section::Countdown => div().flex_1().min_w_0().h_full().child(self.countdown.clone()),
                    Section::Habits => div().flex_1().min_w_0().h_full().child(self.habits.clone()),
                    Section::Pomodoro => div().flex_1().min_w_0().h_full().child(self.focus.clone()),
                })
            })
            .when_some(self.search.clone(), |el, search| {
                el.child(div().absolute().top(px(160.)).left_0().right_0().flex().justify_center().child(search))
            });
        let root = v_flex()
            .relative()
            .size_full()
            .on_action(cx.listener(Self::dismiss_transient))
            .on_drag_move(cx.listener(Self::move_rail_drag))
            .capture_any_mouse_up(cx.listener(Self::finish_rail_drag_up))
            .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(Self::finish_rail_drag_up))
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(content)
            .child(self.title_bar.clone())
            .children(self.render_active_alerts(cx))
            // A ring under a finger held long enough: lifting it opens the context menu.
            .when_some(crate::touch::held(), |el, at| {
                let accent = theme.colors().accent;
                el.child(
                    div()
                        .absolute()
                        .left(at.x - px(24.))
                        .top(at.y - px(24.))
                        .size(px(48.))
                        .rounded_full()
                        .border_2()
                        .border_color(accent)
                        .bg(accent.opacity(0.15)),
                )
            });
        title_bar::window_frame(root, window, cx)
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[gpui::test]
    fn update_notification_opens_the_installed_notes_without_an_updater(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        ReleaseNotes::new(release_notes::CURRENT_VERSION, "https://example.com/fern", "- Cached release changes.")
            .cache_for_restart(&store)
            .unwrap();
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));

        cx.simulate_resize(gpui::size(px(900.), px(400.)));
        let bell = cx.debug_bounds("notification-bell").unwrap();
        let notice = cx.debug_bounds("update-notice").unwrap();
        let button = cx.debug_bounds("updated-whats-new").unwrap();
        assert!(notice.left() >= bell.right());
        assert_eq!(notice.bottom(), bell.bottom());
        assert!(notice.top() >= title_bar::height() && notice.bottom() <= px(400.));
        assert!(cx.update(|_, cx| app.read(cx).settings.is_none()));
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        let settings_window = cx.update(|_, cx| {
            assert!(app.read(cx).update_notice.is_none());
            app.read(cx).settings.unwrap()
        });
        let mut settings_cx = gpui::VisualTestContext::from_window(settings_window.into(), cx);
        assert!(settings_cx.debug_bounds("release-notes-page").is_some());
    }

    #[gpui::test]
    fn escape_dismisses_focused_ui_and_keeps_the_main_view(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, true, window, cx));

        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(!app.read(cx).todo.focus_handle(cx).is_focused(window)));

        let settings_window = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_settings(None, window, cx);
                app.settings.unwrap()
            })
        });
        let mut settings_cx = gpui::VisualTestContext::from_window(settings_window.into(), cx);
        settings_cx.simulate_keystrokes("escape");
        cx.update(|_, cx| {
            assert!(app.read(cx).settings.is_none());
            assert_eq!(cx.windows().len(), 1);
        });
        cx.simulate_keystrokes("escape escape");
        cx.update(|_, cx| {
            assert_eq!(app.read(cx).section, Section::Tasks);
            assert!(app.read(cx).settings.is_none());
        });
    }

    #[gpui::test]
    fn user_escape_minimize_respects_text_inputs_and_transient_ui(cx: &mut gpui::TestAppContext) {
        let user_keymap = r#"[{"context": "Workspace && !TextInput", "bindings": {"escape": "fern::MinimizeWindow"}}]"#;
        cx.update(|cx| {
            theme::init(cx);
            let errors = crate::keymap::load_with_user(&assets::Assets, Some(user_keymap), cx).unwrap();
            assert!(errors.is_empty(), "keymap errors: {errors:?}");
        });
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, true, window, cx));

        // 1. Focus is initially in quick-add TextInput: Esc blurs to task list without minimizing.
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            assert!(!app.read(cx).todo.focus_handle(cx).is_focused(window));
            assert_eq!(app.read(cx).minimized_count, 0);
        });

        // 2. When a transient overlay (notices popover) is open, Esc closes it instead of minimizing.
        cx.update(|_, cx| app.update(cx, |app, cx| app.set_notices_open(true, cx)));
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| {
            assert!(!app.read(cx).notices_open);
            assert_eq!(app.read(cx).minimized_count, 0);
        });

        // 3. When no TextInput or transient UI is active, Esc triggers MinimizeWindow.
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| assert_eq!(app.read(cx).minimized_count, 1));
    }

    #[gpui::test]
    fn switching_modules_finishes_the_calendar_draft_and_keeps_the_destination_focus(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (app, cx) = cx.add_window_view(|window, cx| {
            let mut app = AppView::new(store.clone(), true, window, cx);
            app.select(Section::Calendar, window, cx);
            app
        });
        cx.simulate_resize(gpui::size(px(1200.), px(650.)));
        let modifiers = gpui::Modifiers::default();
        let add = cx.debug_bounds("calendar-add-frame").unwrap().center();
        cx.simulate_click(add, modifiers);
        cx.simulate_input("Save before switching modules");
        let date = cx.debug_bounds("task-detail-due-date").unwrap().center();
        cx.simulate_click(date, modifiers);
        assert!(cx.debug_bounds("due-popup-ok").is_some());

        cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Matrix, window, cx))));

        assert!(cx.debug_bounds("calendar-task-editor").is_none());
        assert!(cx.debug_bounds("matrix-header-menu-trigger").is_some());
        cx.update(|window, cx| assert!(app.read(cx).matrix.focus_handle(cx).is_focused(window)));
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Save before switching modules");
    }

    #[gpui::test]
    fn window_close_and_quit_finish_the_calendar_draft(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (_, cx) = cx.add_window_view(|window, cx| {
            let mut app = AppView::new(store.clone(), true, window, cx);
            app.select(Section::Calendar, window, cx);
            app
        });
        cx.update(|window, cx| AppView::register_window_actions(window.window_handle().downcast().unwrap(), cx));
        cx.simulate_resize(gpui::size(px(1200.), px(650.)));

        for (index, (title, action)) in [
            ("Native close", None),
            ("Quit action", Some(Box::new(Quit) as Box<dyn gpui::Action>)),
            ("Close action", Some(Box::new(CloseWindow) as Box<dyn gpui::Action>)),
        ].into_iter().enumerate() {
            let add = cx.debug_bounds("calendar-add-frame").unwrap().center();
            cx.simulate_click(add, gpui::Modifiers::default());
            cx.simulate_input(title);
            assert_eq!(store.tasks().unwrap().len(), index);

            if let Some(action) = action {
                cx.update(|window, cx| window.dispatch_action(action, cx));
            } else {
                assert!(cx.simulate_close());
            }

            let tasks = store.tasks().unwrap();
            assert_eq!(tasks.len(), index + 1);
            assert!(tasks.iter().any(|task| task.title == title));
        }
    }

    #[gpui::test]
    fn settings_scroll_below_the_header_in_a_short_window(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase, point, size};

        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, true, window, cx));
        let settings_window = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_settings(None, window, cx);
                app.settings.unwrap()
            })
        });
        let mut settings_cx = gpui::VisualTestContext::from_window(settings_window.into(), cx);
        settings_cx.simulate_resize(size(px(900.), px(400.)));
        settings_window.update(&mut settings_cx, |settings, _, cx| {
            settings.set_page(settings_window::Page::More, cx);
        }).unwrap();
        let header = settings_cx.debug_bounds("settings-header").unwrap();
        let page = settings_cx.debug_bounds("settings-page").unwrap();
        let language = settings_cx.debug_bounds("settings-language").unwrap();
        assert_eq!(header.top(), px(0.));
        assert_eq!(page.top(), header.bottom());
        assert!(page.bottom() <= px(400.));
        assert!(language.top() >= page.top());

        let position = page.center();
        let modifiers = Modifiers::default();
        settings_cx.simulate_mouse_move(position, None, modifiers);
        settings_cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(-160.))),
            modifiers,
            touch_phase: TouchPhase::Moved,
        });
        assert!(settings_cx.debug_bounds("settings-language").unwrap().top() < language.top());
        assert_eq!(settings_cx.debug_bounds("settings-header"), Some(header));

        for page in [settings_window::Page::SmartList, settings_window::Page::More] {
            settings_window.update(&mut settings_cx, |settings, _, cx| settings.set_page(page, cx)).unwrap();
        }
        assert_eq!(settings_cx.debug_bounds("settings-language"), Some(language));
    }

    #[gpui::test]
    fn settings_window_is_separate_single_instance_and_updates_main_window_live(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store.clone(), true, window, cx));

        let first = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_settings(None, window, cx);
                app.settings.unwrap()
            })
        });
        cx.update(|_, cx| assert_eq!(cx.windows().len(), 2));
        assert!(cx.debug_bounds("settings-header").is_none(), "settings must not render inside the main window");

        // Opening settings again reuses the existing window instead of spawning a second one.
        let second = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_settings(None, window, cx);
                app.settings.unwrap()
            })
        });
        assert_eq!(first, second);
        cx.update(|_, cx| assert_eq!(cx.windows().len(), 2));

        // Changing a feature in the separate Settings window updates the main window live while
        // keeping Settings open.
        let mut settings_cx = gpui::VisualTestContext::from_window(first.into(), cx);
        first.update(&mut settings_cx, |settings, _, cx| {
            settings.set_page(settings_window::Page::Features, cx);
        }).unwrap();
        cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Calendar, window, cx))));
        let calendar_toggle = settings_cx.debug_bounds("settings-feature-0").unwrap();
        settings_cx.simulate_click(calendar_toggle.center(), gpui::Modifiers::default());
        cx.update(|_, cx| {
            assert!(!app.read(cx).features.calendar);
            assert_eq!(app.read(cx).section, Section::Tasks);
            assert!(app.read(cx).settings.is_some());
            assert_eq!(cx.windows().len(), 2);
        });

        // Clicking Shortcuts in Settings switches to Page::Shortcuts inside the Settings window.
        let shortcuts_item = settings_cx.debug_bounds("settings-shortcuts").unwrap();
        settings_cx.simulate_click(shortcuts_item.center(), gpui::Modifiers::default());
        cx.update(|_, cx| {
            assert!(app.read(cx).settings.is_some());
            assert_eq!(cx.windows().len(), 2);
            let page = first.read_with(cx, |settings, _| settings.current_page()).unwrap();
            assert_eq!(page, settings_window::Page::Shortcuts);
        });
        assert!(settings_cx.debug_bounds("settings-shortcuts-page").is_some());

        // Closing Settings via native close clears AppView::settings.
        assert!(settings_cx.simulate_close());
        // `simulate_close` only runs `on_window_should_close`; mirror the OS removing the window.
        first.update(&mut settings_cx, |_, window, _| window.remove_window()).unwrap();
        cx.update(|_, cx| {
            assert!(app.read(cx).settings.is_none());
            assert_eq!(cx.windows().len(), 1);
        });

        // ShowShortcuts opens Settings directly on Page::Shortcuts.
        let shortcuts_win = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_settings_page(settings_window::Page::Shortcuts, window, cx);
                app.settings.unwrap()
            })
        });
        cx.update(|_, cx| {
            let page = shortcuts_win.read_with(cx, |settings, _| settings.current_page()).unwrap();
            assert_eq!(page, settings_window::Page::Shortcuts);
        });
        let mut reopened_cx = gpui::VisualTestContext::from_window(shortcuts_win.into(), cx);
        assert!(reopened_cx.simulate_close());
        shortcuts_win.update(&mut reopened_cx, |_, window, _| window.remove_window()).unwrap();
        cx.update(|_, cx| {
            assert!(app.read(cx).settings.is_none());
            assert_eq!(cx.windows().len(), 1);
        });

        // Reopen Settings and verify closing the main window also closes the Settings window.
        cx.update(|window, cx| app.update(cx, |app, cx| app.open_settings(None, window, cx)));
        cx.update(|_, cx| {
            assert!(app.read(cx).settings.is_some());
            assert_eq!(cx.windows().len(), 2);
        });
        assert!(cx.simulate_close());
        cx.update(|_, cx| {
            assert!(app.read(cx).settings.is_none());
            assert_eq!(cx.windows().len(), 1);
        });
    }

    #[gpui::test]
    fn title_bar_overlays_full_height_columns_above_header_controls(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Check merged title bar").unwrap();
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, true, window, cx));
        cx.simulate_resize(gpui::size(px(1200.), px(700.)));
        cx.update(|_, cx| {
            let todo = app.read(cx).todo.clone();
            todo.update(cx, |todo, cx| todo.select(task.id, cx));
        });

        let bar = cx.debug_bounds("title-bar").unwrap();
        let sidebar = cx.debug_bounds("sidebar").unwrap();
        let lists = cx.debug_bounds("lists-column").unwrap();
        let sort = cx.debug_bounds("task-sort-menu-trigger").unwrap();
        let priority = cx.debug_bounds("task-detail-priority").unwrap();
        assert_eq!(bar.top(), px(0.));
        assert_eq!(bar.bottom(), title_bar::height());
        assert_eq!(sidebar.top(), px(0.));
        assert_eq!(sidebar.bottom(), px(700.));
        assert_eq!(lists.top(), px(0.));
        assert_eq!(lists.bottom(), px(700.));
        assert!(sort.top() >= bar.bottom());
        assert!(priority.top() >= bar.bottom());

        cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Calendar, window, cx))));
        let more = cx.debug_bounds("calendar-more-button").unwrap();
        assert!(more.top() >= bar.bottom());

        cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Matrix, window, cx))));
        let matrix_more = cx.debug_bounds("matrix-header-menu-trigger").unwrap();
        assert!(matrix_more.top() >= bar.bottom());
    }

    #[gpui::test]
    fn sidebar_rail_drag_reorders_navigation_icons(cx: &mut gpui::TestAppContext) {
        use fern_core::RailSection;
        use gpui::{Modifiers, MouseButton, point, size};

        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store.clone(), true, window, cx));
        cx.simulate_resize(size(px(1200.), px(700.)));
        cx.run_until_parked();

        let modifiers = Modifiers::default();

        // Normal click switches sections.
        let habits = cx.debug_bounds("section-habits").unwrap();
        cx.simulate_click(habits.center(), modifiers);
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(app.read(cx).section, Section::Habits));

        // Drag Calendar up onto Tasks through multiple incremental steps: icons reorder LIVE while
        // dragging, never stick after the initial threshold step, and releasing even far outside
        // the narrow 50 px rail still commits without switching sections.
        let cal = cx.debug_bounds("section-calendar").unwrap();
        let tasks = cx.debug_bounds("section-tasks").unwrap();
        cx.simulate_mouse_move(cal.center(), None, modifiers);
        cx.simulate_mouse_down(cal.center(), MouseButton::Left, modifiers);
        for dy in [5., 12., 25.] {
            cx.simulate_mouse_move(
                point(cal.center().x, cal.center().y - px(dy)),
                Some(MouseButton::Left),
                modifiers,
            );
            cx.run_until_parked();
        }
        let lifted_over_tasks = point(tasks.center().x + px(120.), tasks.center().y);
        cx.simulate_mouse_move(lifted_over_tasks, Some(MouseButton::Left), modifiers);
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("section-calendar").unwrap().top() < cx.debug_bounds("section-tasks").unwrap().top(),
            "neighbors slide live before MouseUp"
        );
        cx.simulate_mouse_up(lifted_over_tasks, MouseButton::Left, modifiers);
        cx.run_until_parked();

        cx.update(|_, cx| {
            assert_eq!(app.read(cx).section, Section::Habits, "dragging does not switch active section");
            assert_eq!(app.read(cx).rail_order[0], RailSection::Calendar);
            assert_eq!(app.read(cx).rail_order[1], RailSection::Tasks);
        });
        assert_eq!(store.preferences().unwrap().rail_order[0], RailSection::Calendar);

        // Escape cancels an in-progress drag and restores the previous order.
        let cal = cx.debug_bounds("section-calendar").unwrap();
        cx.simulate_mouse_move(cal.center(), None, modifiers);
        cx.simulate_mouse_down(cal.center(), MouseButton::Left, modifiers);
        cx.simulate_mouse_move(point(cal.center().x, cal.center().y + px(8.)), Some(MouseButton::Left), modifiers);
        cx.run_until_parked();
        cx.simulate_mouse_move(point(cal.center().x, cal.center().y + px(120.)), Some(MouseButton::Left), modifiers);
        cx.run_until_parked();
        cx.update(|_, cx| assert_ne!(app.read(cx).rail_order[0], RailSection::Calendar));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(app.read(cx).dragging_rail.is_none());
            assert_eq!(app.read(cx).rail_order[0], RailSection::Calendar);
        });
    }

    #[gpui::test]
    fn active_alert_banner_snoozes_and_completes_task(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let t1 = store.add_task("Call bank").unwrap();
        let t2 = store.add_task("Pay rent").unwrap();
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store.clone(), true, window, cx));
        cx.simulate_resize(gpui::size(px(1200.), px(700.)));
        cx.run_until_parked();

        let at = chrono::Local::now().naive_local();
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.push_active_alerts(
                    vec![Notice { target: Target::Task(t1.id), title: t1.title.clone(), at, constant: true, has_goal: false }],
                    cx,
                );
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("active-reminder-alerts").is_some());

        // Click "Snooze 5m" on active alert card -> sets snoozed_until and removes card.
        let snooze_5m = cx.debug_bounds("alert-0-snooze-5m").unwrap().center();
        cx.simulate_click(snooze_5m, gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(store.task(t1.id).unwrap().unwrap().snoozed_until.is_some());
        assert!(cx.debug_bounds("active-reminder-alerts").is_none());

        // Push second alert -> click "Done" -> marks task completed and removes card.
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.push_active_alerts(
                    vec![Notice { target: Target::Task(t2.id), title: t2.title.clone(), at, constant: false, has_goal: false }],
                    cx,
                );
            });
        });
        cx.run_until_parked();
        let complete = cx.debug_bounds("alert-0-complete").unwrap().center();
        cx.simulate_click(complete, gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(store.task(t2.id).unwrap().unwrap().is_completed());
        assert!(cx.debug_bounds("active-reminder-alerts").is_none());
    }

    #[gpui::test]
    fn tray_focus_starts_pomo_and_stopwatch_without_switching_section(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
        cx.simulate_resize(gpui::size(px(1200.), px(700.)));
        cx.run_until_parked();

        // Starting Pomo from tray starts a countdown timer while keeping the main section unchanged.
        cx.update(|_, cx| app.update(cx, |app, cx| app.start_focus_from_tray(false, cx)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = app.read(cx);
            assert_eq!(view.section, Section::Tasks);
            let now = chrono::Local::now().timestamp_millis();
            let (_, _, running, paused) = view.focus.read(cx).clock_state(now, i18n::t(cx));
            assert!(running);
            assert!(!paused);
        });

        // Starting Stopwatch switches the timer to stopwatch mode and runs it.
        cx.update(|_, cx| app.update(cx, |app, cx| app.start_focus_from_tray(true, cx)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = app.read(cx);
            assert_eq!(view.section, Section::Tasks);
            let now = chrono::Local::now().timestamp_millis();
            let (clock, _, running, paused) = view.focus.read(cx).clock_state(now, i18n::t(cx));
            assert!(running);
            assert!(!paused);
            assert!(clock.starts_with("00:"), "expected stopwatch starting at 00:.., got {clock}");
        });
    }
}
