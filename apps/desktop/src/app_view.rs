use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use desktop_common::plugin_bus::PluginBus;
use fern_core::{RailItemId, Store, link::DeepLink};
use gpui::Focusable as _;
use plugin_ui::{PluginHost, PluginView, render_plugin_overlays};
use things_ui::{ThingsEvent, ThingsView};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment};

mod alerts;
mod chrome;
mod plugins;
mod sync_loop;

use crate::actions::{
    CloseWindow, DismissTransient, GoToHorizons, GoToSummary, MinimizeWindow, NewTask,
    OpenSettings, PrintDetail, PrintList, Quit, Search, ShowAll, ShowCompleted, ShowInbox,
    ShowNext7Days, ShowShortcuts, ShowToday, ShowTomorrow, ShowTrash, ShowWontDo, StartStop,
    SyncNow, TogglePause, ToggleSidebar,
};
use crate::calendar_view::{self, CalendarView};
use crate::countdown_view::CountdownView;
use crate::focus_view::FocusView;
use crate::habit_view::HabitView;
use crate::i18n;
use crate::life_clock_view::{LifeClockEvent, LifeClockPaletteMode, LifeClockView};
use crate::matrix_view::{self, MatrixView};
use crate::release_notes::{self, ReleaseNotes};
use crate::reminders::{self, Notice, Notices};
use crate::search_modal::{SearchEvent, SearchModal};
use crate::settings_window::{self, SettingsEvent, SettingsWindow};
use crate::sidebar::{self, Section};
use crate::title_bar::{self, ShowReleaseNotes, TitleBar};
use crate::todo_view::TodoView;

/// Handles Escape for an active plugin modal: closes any open `Select` popover inside a `View`
/// modal first, otherwise dismisses the modal via `plugin_ui::dismiss_active_plugin_modal`
/// (resolving pending `Confirm`/`Prompt` promises, unmounting `"__modal__"` handlers, and
/// bumping `modal_generation`).
pub(crate) fn dismiss_plugin_modal(cx: &mut App) -> bool {
    plugin_ui::handle_plugin_modal_escape(cx)
}

fn first_enabled_task_section(features: fern_core::Features) -> Section {
    if features.tasks {
        Section::Tasks
    } else if features.things {
        Section::Things
    } else {
        Section::Tasks
    }
}

trait WorkspaceSection {
    fn set_section_active(&self, active: bool, cx: &mut App);
    fn sync_section_external(&self, cx: &mut App);
    fn focus_section(&self, window: &mut Window, cx: &mut App);
    fn render_section(&self) -> AnyElement;
}

macro_rules! impl_workspace_section {
    ($($view:ty),+ $(,)?) => {
        $(
            impl WorkspaceSection for gpui::Entity<$view> {
                fn set_section_active(&self, active: bool, cx: &mut App) {
                    self.update(cx, |view, cx| view.set_active(active, cx));
                }

                fn sync_section_external(&self, cx: &mut App) {
                    self.update(cx, |view, cx| view.sync_external(cx));
                }

                fn focus_section(&self, window: &mut Window, cx: &mut App) {
                    window.focus(&self.focus_handle(cx), cx);
                }

                fn render_section(&self) -> AnyElement {
                    self.clone().into_any_element()
                }
            }
        )+
    };
}

impl_workspace_section!(
    TodoView,
    ThingsView,
    CalendarView,
    MatrixView,
    CountdownView,
    LifeClockView,
    HabitView,
    FocusView,
);

pub struct AppView {
    pub(crate) store: Rc<Store>,
    section: Section,
    pub(crate) todo: gpui::Entity<TodoView>,
    pub(crate) things: gpui::Entity<ThingsView>,
    pub(crate) calendar: gpui::Entity<CalendarView>,
    pub(crate) matrix: gpui::Entity<MatrixView>,
    pub(crate) countdown: gpui::Entity<CountdownView>,
    pub(crate) life_clock: gpui::Entity<LifeClockView>,
    pub(crate) life_clock_rail_hidden: bool,
    pub(crate) habits: gpui::Entity<HabitView>,
    pub(crate) focus: gpui::Entity<FocusView>,
    plugin_views: HashMap<(String, String), gpui::Entity<PluginView>>,
    window_handle: Option<gpui::WindowHandle<Self>>,
    title_bar: gpui::Entity<TitleBar>,
    pub(crate) search: Option<gpui::Entity<SearchModal>>,
    pub(crate) account_menu_open: bool,
    notices: Notices,
    notices_open: bool,
    active_alerts: Vec<Notice>,
    sync: crate::sync::SyncStatus,
    sync_task: Option<gpui::Task<()>>,
    sync_generation: u64,
    cloud_sse_stop: Option<Arc<AtomicBool>>,
    cloud_sse_task: Option<gpui::Task<()>>,
    cloud_push_debounce: Option<gpui::Task<()>>,
    /// Cached `store.sync_mode() != SyncMode::Off` so `render` never queries SQLite every frame.
    pub(crate) has_sync_config: bool,
    /// Cached `tray::always_on_top` so `render_account_menu` never queries SQLite every frame.
    always_on_top: bool,
    /// Cached tray tooltip derived from `focus.running_clock()` so unchanged notifications
    /// skip redundant Win32 `Shell_NotifyIconW` calls and full `AppView` re-renders.
    last_tray_tooltip: String,
    /// Settings → Features: the sections shown in the sidebar.
    pub(crate) features: fern_core::Features,
    /// Full rail item order including plugin sidebar views.
    rail_items: Vec<RailItemId>,
    /// Scroll handle for the left navigation rail (needed so drag slot calculation accounts for scroll).
    rail_scroll: gpui::ScrollHandle,
    /// The rail icon currently being dragged, and the order before the drag started (for Escape).
    dragging_rail: Option<Section>,
    rail_drag_origin: Option<Vec<RailItemId>>,
    pub(crate) settings: Option<gpui::WindowHandle<SettingsWindow>>,
    update_notice: Option<Rc<ReleaseNotes>>,
    /// The Statistics page opened from the account menu, which covers the whole window.
    statistics_open: bool,
    /// The sidebar icon under the pointer, whose name shows beside it.
    nav_hovered: Option<Section>,
    _modal_escape_sub: gpui::Subscription,
    last_window_state: Option<desktop_common::window_state::MainWindowState>,
    window_bounds_save_task: Option<gpui::Task<()>>,
    #[cfg(test)]
    minimized_count: usize,
}

impl AppView {
    pub fn try_quit(main_window: gpui::WindowHandle<Self>, cx: &mut App) -> bool {
        let can_quit = main_window
            .update(cx, |view, window, cx| {
                let ok = view.prepare_to_leave_calendar(window, cx);
                if !ok {
                    crate::tray::restore_main_window();
                    window.activate_window();
                } else {
                    view.flush_window_state();
                    view.unmount_all_plugin_views(cx);
                    view.close_settings(cx);
                }
                ok
            })
            .unwrap_or(true);
        if !can_quit {
            return false;
        }
        crate::shutdown::AppShutdown::run_once(cx);
        cx.quit();
        true
    }

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
                            view.shutdown_for_main_window_close(cx);
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
                Self::try_quit(main_window, cx);
            });
        });
    }

    #[cfg(test)]
    pub fn new(store: Rc<Store>, first_start: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial_prefs = store.preferences().unwrap_or_default();
        Self::new_with_preferences(store, initial_prefs, first_start, window, cx)
    }

    pub fn new_with_preferences(
        store: Rc<Store>,
        initial_prefs: fern_core::Preferences,
        first_start: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
                view.shutdown_for_main_window_close(cx);
                true
            }).unwrap_or(true)
        });
        desktop_common::ActivePreferences::sync_from_preferences(
            &initial_prefs,
            desktop_common::sound::enabled(&store),
            cx,
        );
        let store_features = initial_prefs.features;
        let rail_items = initial_prefs.rail_items.clone();
        let initial_section = first_enabled_task_section(store_features);
        let todo = cx.new(|cx| {
            if initial_section == Section::Tasks {
                TodoView::new(store.clone(), window, cx)
            } else {
                TodoView::new_inactive(store.clone(), window, cx)
            }
        });
        let things = cx.new(|cx| {
            if initial_section == Section::Things {
                ThingsView::new(store.clone(), window, cx)
            } else {
                ThingsView::new_inactive(store.clone(), window, cx)
            }
        });
        cx.subscribe_in(&things, window, |this, _, event: &ThingsEvent, window, cx| match event {
            ThingsEvent::StartFocus {
                task_id,
                title,
                stopwatch,
            } => {
                let (target_id, title, stopwatch) = (*task_id, title.clone(), *stopwatch);
                if this.select(Section::Pomodoro, window, cx) {
                    this.focus
                        .update(cx, |focus, cx| focus.start_for(Some(target_id), title, stopwatch, cx));
                }
            }
            ThingsEvent::OpenSearch => {
                this.open_search(window, cx);
            }
        })
        .detach();
        let calendar = cx.new(|cx| CalendarView::new_inactive(store.clone(), window, cx));
        // A task clicked in the calendar opens in the task list with its details.
        cx.subscribe_in(&calendar, window, |this, _, calendar_view::OpenTask(id), window, cx| {
            this.open_task(*id, window, cx);
        })
        .detach();
        // A picker asked for in the calendar's right-click menu opens with the task in the list.
        cx.subscribe_in(&calendar, window, |this, _, calendar_view::OpenTaskPicker(id, what), window, cx| {
            let (id, what) = (*id, *what);
            if !this.features.tasks && this.features.things {
                this.open_task(id, window, cx);
                return;
            }
            if !this.features.tasks || !this.select(Section::Tasks, window, cx) {
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
        let matrix = cx.new(|cx| MatrixView::new_inactive(store.clone(), cx));
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
        let countdown = cx.new(|cx| CountdownView::new_inactive(store.clone(), cx));
        let life_clock = cx.new(|cx| LifeClockView::new_inactive(store.clone(), cx));
        cx.subscribe(&life_clock, |this, _, event: &LifeClockEvent, cx| match event {
            LifeClockEvent::ToggleSidebar => {
                this.toggle_life_clock_rail(cx);
            }
            LifeClockEvent::PaletteChanged(_) => {
                this.sync_title_bar_appearance(cx);
            }
        })
        .detach();
        let habits = cx.new(|cx| HabitView::new_inactive(store.clone(), cx));
        // "Start Focus" in a habit's menu starts a Pomodoro or a stopwatch linked to it.
        cx.subscribe_in(&habits, window, |this, _, event: &crate::habit_view::StartFocus, window, cx| {
            let (habit_id, title, stopwatch) = (event.habit_id, event.title.clone(), event.stopwatch);
            if this.select(Section::Pomodoro, window, cx) {
                this.focus.update(cx, |focus, cx| focus.start_for_habit(habit_id, title, stopwatch, cx));
            }
        })
        .detach();
        let focus = cx.new(|cx| FocusView::new_inactive(store.clone(), cx));
        cx.observe(&focus, |this, focus, cx| {
            let tip = match focus.read(cx).running_clock() {
                Some((clock, true)) => format!("Fern — {clock} (Paused)"),
                Some((clock, false)) => format!("Fern — {clock}"),
                None => "Fern".to_owned(),
            };
            if this.last_tray_tooltip != tip {
                crate::tray::set_tooltip(&tip);
                this.last_tray_tooltip = tip;
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&focus, window, |this, _, crate::focus_view::StatsClosed, window, cx| {
            this.statistics_open = false;
            this.sync_active_section(cx);
            if this.section == Section::LifeClock {
                window.focus(&this.life_clock.focus_handle(cx), cx);
            }
            this.sync_title_bar_appearance(cx);
            cx.notify();
        })
        .detach();
        let window_handle = window.window_handle().downcast::<Self>();
        let has_sync_config =
            !matches!(store.sync_mode(), Ok(fern_core::sync::SyncMode::Off) | Err(_));
        let always_on_top = crate::tray::TrayPrefs::current(&store, cx).always_on_top;
        // Development builds can share the user database and must leave its update notice intact.
        let update_notice = if cfg!(all(debug_assertions, not(test))) {
            None
        } else {
            release_notes::take_update_notice(&store, first_start)
                .unwrap_or_else(|err| {
                    desktop_common::logging::log_error("app", format!("failed to read the update notification: {err:#}"));
                    None
                })
                .map(Rc::new)
        };
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
        cx.observe_global::<PluginBus>(|this, cx| {
            this.on_plugin_bus_changed(cx);
        })
        .detach();
        cx.observe_global::<i18n::CurrentLocale>(|_, cx| {
            let code = cx.global::<i18n::CurrentLocale>().0.code().to_owned();
            PluginHost::set_appearance(Some(code), None, cx);
        })
        .detach();
        cx.observe_global::<desktop_common::data_version::DataVersion>(|this, cx| {
            this.schedule_cloud_push_debounce(cx);
        })
        .detach();
        cx.observe_global::<desktop_common::sync::LiveCloudSnapshot>(|_, cx| {
            cx.notify();
        })
        .detach();
        let _modal_escape_sub = cx.intercept_keystrokes(|event, _window, cx| {
            if event.keystroke.key == "escape"
                && !event.keystroke.modifiers.modified()
                && dismiss_plugin_modal(cx)
            {
                cx.stop_propagation();
            }
        });
        // Mutated only by the debug-build dev state below.
        #[cfg_attr(not(debug_assertions), allow(unused_mut))]
        let mut this = Self {
            title_bar,
            store,
            section: initial_section,
            todo,
            things,
            calendar,
            matrix,
            countdown,
            life_clock,
            life_clock_rail_hidden: false,
            habits,
            focus,
            plugin_views: HashMap::new(),
            window_handle,
            search: None,
            account_menu_open: false,
            notices: Notices::default(),
            notices_open: false,
            active_alerts: Vec::new(),
            sync: crate::sync::SyncStatus::Off,
            sync_task: None,
            sync_generation: 0,
            cloud_sse_stop: None,
            cloud_sse_task: None,
            cloud_push_debounce: None,
            has_sync_config,
            always_on_top,
            last_tray_tooltip: "Fern".to_owned(),
            features: store_features,
            rail_items,
            rail_scroll: gpui::ScrollHandle::new(),
            dragging_rail: None,
            rail_drag_origin: None,
            settings: None,
            update_notice,
            statistics_open: false,
            nav_hovered: None,
            _modal_escape_sub,
            last_window_state: Some(desktop_common::window_state::capture(window)),
            window_bounds_save_task: None,
            #[cfg(test)]
            minimized_count: 0,
        };
        cx.observe_window_bounds(window, |this, window, cx| {
            this.schedule_save_window_state(window, cx);
        })
        .detach();
        reminders::start(this.store.clone(), window.window_handle(), cx);
        cx.set_global(crate::LiveSyncStatus(this.sync.clone()));
        this.start_sync(cx);
        settings_window::apply(&initial_prefs, window, cx);
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
        this.sync_active_section(cx);
        this.sync_title_bar_appearance(cx);
        this
    }

    fn schedule_save_window_state(&mut self, window: &Window, cx: &mut Context<Self>) {
        if desktop_common::is_dev_state() {
            return;
        }
        let state = desktop_common::window_state::capture(window);
        self.last_window_state = Some(state.clone());
        let store = self.store.clone();
        self.window_bounds_save_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            desktop_common::window_state::save(&store, &state);
        }));
    }

    fn flush_window_state(&mut self) {
        self.window_bounds_save_task = None;
        if let Some(state) = &self.last_window_state {
            desktop_common::window_state::save(&self.store, state);
        }
    }

    fn shutdown_for_main_window_close(&mut self, cx: &mut Context<Self>) {
        self.stop_cloud_realtime(cx);
        self.flush_window_state();
        self.unmount_all_plugin_views(cx);
        self.close_settings(cx);
        PluginHost::shutdown(cx);
    }

    pub fn prepare_for_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.prepare_to_leave_calendar(window, cx);
        self.shutdown_for_main_window_close(cx);
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

    fn builtin_sections(&self) -> [(Section, &dyn WorkspaceSection); 8] {
        [
            (Section::Tasks, &self.todo),
            (Section::Things, &self.things),
            (Section::Calendar, &self.calendar),
            (Section::Matrix, &self.matrix),
            (Section::Countdown, &self.countdown),
            (Section::LifeClock, &self.life_clock),
            (Section::Habits, &self.habits),
            (Section::Pomodoro, &self.focus),
        ]
    }

    fn builtin_section(&self, section: &Section) -> Option<&dyn WorkspaceSection> {
        let normalized = if matches!(section, Section::Search) {
            &Section::Tasks
        } else {
            section
        };
        self.builtin_sections()
            .into_iter()
            .find_map(|(sec, widget)| (sec == *normalized).then_some(widget))
    }

    fn sync_active_section(&mut self, cx: &mut Context<Self>) {
        let stats = self.statistics_open;
        let current = if matches!(self.section, Section::Search) {
            Section::Tasks
        } else {
            self.section.clone()
        };
        for (sec, widget) in self.builtin_sections() {
            let active = if sec == Section::Pomodoro {
                stats || current == Section::Pomodoro
            } else {
                !stats && current == sec
            };
            widget.set_section_active(active, cx);
        }
    }

    fn sync_external_all(&self, cx: &mut Context<Self>) {
        for (_, widget) in self.builtin_sections() {
            widget.sync_section_external(cx);
        }
    }

    pub(crate) fn sync_title_bar_appearance(&self, cx: &mut Context<Self>) {
        let appearance = (self.section == Section::LifeClock
            && !self.statistics_open
            && self.life_clock.read(cx).palette_mode() == LifeClockPaletteMode::Classic)
            .then_some(theme::Appearance::Light);
        self.title_bar
            .update(cx, |bar, cx| bar.set_caption_appearance(appearance, cx));
    }

    pub(crate) fn set_life_clock_rail_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        self.life_clock_rail_hidden = hidden;
        self.life_clock
            .update(cx, |view, cx| view.set_sidebar_hidden(hidden, cx));
        cx.notify();
    }

    fn toggle_life_clock_rail(&mut self, cx: &mut Context<Self>) {
        let hidden = !self.life_clock_rail_hidden;
        self.set_life_clock_rail_hidden(hidden, cx);
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
        if self.section == Section::LifeClock && section != Section::LifeClock {
            self.life_clock_rail_hidden = false;
            self.life_clock
                .update(cx, |view, cx| view.set_sidebar_hidden(false, cx));
        }
        if self.section != section
            && let Section::Plugin {
                ref plugin_id,
                ref view_id,
            } = self.section
        {
            let key = (plugin_id.to_string(), view_id.to_string());
            if let Some(prev) = self.plugin_views.remove(&key) {
                prev.update(cx, |view, cx| view.unmount(cx));
            }
        }
        self.todo.update(cx, |todo, cx| todo.stop_search(cx));
        let plugin_target = match &section {
            Section::Plugin { plugin_id, view_id } => Some((plugin_id.clone(), view_id.clone())),
            _ => None,
        };
        self.section = section;
        self.sync_active_section(cx);
        if let Some((plugin_id, view_id)) = plugin_target {
            let view = self.ensure_plugin_view(plugin_id.as_ref(), view_id.as_ref(), window, cx);
            window.focus(&view.focus_handle(cx), cx);
        } else if !matches!(self.section, Section::Tasks | Section::Search)
            && let Some(widget) = self.builtin_section(&self.section)
        {
            widget.focus_section(window, cx);
        }
        self.sync_title_bar_appearance(cx);
        cx.notify();
        true
    }

    fn close_search(&mut self, cx: &mut Context<Self>) -> bool {
        let closed = self.search.take().is_some();
        if closed {
            SearchModal::clear_plugin_search_hits(cx);
        }
        closed
    }

    pub(crate) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search = cx.new(|cx| SearchModal::new(self.store.clone(), cx));
        cx.subscribe_in(&search, window, |this, _, event, window, cx| {
            this.close_search(cx);
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
                    if this.features.tasks {
                        if this.select(Section::Tasks, window, cx) {
                            this.todo.update(cx, |todo, cx| todo.show_search_results(query, cx));
                        }
                    } else if this.features.things {
                        let _ = this.select(Section::Things, window, cx);
                    }
                }
                SearchEvent::OpenView(view) => {
                    let view = view.clone();
                    this.show_view(view, window, cx);
                }
                SearchEvent::CreateTask(query) => {
                    let query = query.clone();
                    let target = if (this.section == Section::Things && this.features.things)
                        || (!this.features.tasks && this.features.things)
                    {
                        Section::Things
                    } else {
                        Section::Tasks
                    };
                    if desktop_common::plugin_bus::PluginHooksGlobal::global(cx)
                        .is_some_and(|hooks| hooks.is_active())
                        && let Some(win) = this.window_handle
                    {
                        let (lists, preferences) = this.quick_add_context();
                        crate::quick_add_window::run_quick_add_plugins(
                            win,
                            vec![query],
                            None,
                            None,
                            None,
                            lists,
                            preferences,
                            Some(target),
                            cx,
                        );
                    } else {
                        this.add_quick_task_with(&query, None, None, None, cx);
                        this.select(target, window, cx);
                    }
                }
                SearchEvent::RunPluginCommand {
                    plugin_id,
                    command_id,
                    arg,
                } => {
                    PluginHost::run_command_async(
                        plugin_id.clone(),
                        command_id.clone(),
                        arg.clone(),
                        cx,
                    );
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
        self.things.update(cx, |things, cx| things.sync_external(cx));
    }

    /// Creates a task from a plugin-modified `beforeTaskCreate` draft.
    pub fn add_plugin_task_draft(
        &mut self,
        draft: &fern_core::TaskDraft,
        top: bool,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.store.create_task(draft.clone(), top) {
            desktop_common::logging::log_error("plugin", format!("failed to create plugin-modified task: {err:#}"));
            return;
        }
        crate::sticky_note::data_changed(cx.entity_id(), cx);
        self.reload_all(cx);
    }

    /// Lists and user preferences (recognition + task defaults) for the Quick Add window.
    pub fn quick_add_context(&self) -> (Vec<fern_core::List>, fern_core::Preferences) {
        let lists = self.store.lists().unwrap_or_default().into_iter().filter(|l| !l.archived).collect();
        let preferences = self.store.preferences().unwrap_or_default();
        (lists, preferences)
    }

    /// The add field of the task list, focused: New Task.
    pub fn quick_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let use_things = (self.section == Section::Things && self.features.things)
            || (!self.features.tasks && self.features.things);
        if use_things {
            self.select(Section::Things, window, cx);
        } else if self.features.tasks && self.select(Section::Tasks, window, cx) {
            self.todo.update(cx, |todo, cx| todo.focus_new_task(window, cx));
        }
    }

    fn search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Search, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        if self.section == Section::LifeClock {
            self.toggle_life_clock_rail(cx);
        } else {
            self.todo.update(cx, |todo, cx| todo.toggle_lists(cx));
        }
    }

    fn show_view(&mut self, view: fern_core::View, window: &mut Window, cx: &mut Context<Self>) {
        if self.features.tasks {
            let target = if view == fern_core::View::Horizons && !self.features.horizons {
                fern_core::View::Today
            } else {
                view
            };
            if self.select(Section::Tasks, window, cx) {
                self.todo.update(cx, |todo, cx| todo.select_view(target, window, cx));
            }
        } else if self.features.things {
            let things_view = match view {
                fern_core::View::Inbox => Some(things_ui::ThingsViewKind::Inbox),
                fern_core::View::Today => Some(things_ui::ThingsViewKind::Today),
                fern_core::View::Tomorrow
                | fern_core::View::Next7Days
                | fern_core::View::Day(_) => Some(things_ui::ThingsViewKind::Upcoming),
                fern_core::View::Completed | fern_core::View::WontDo => {
                    Some(things_ui::ThingsViewKind::Logbook)
                }
                fern_core::View::List(id) => Some(things_ui::ThingsViewKind::Project(id)),
                fern_core::View::All
                | fern_core::View::Tag(_)
                | fern_core::View::Search(_)
                | fern_core::View::Filter(_)
                | fern_core::View::Horizons
                | fern_core::View::Summary => Some(things_ui::ThingsViewKind::Anytime),
                fern_core::View::Trash => None,
            };
            if let Some(target_view) = things_view
                && self.select(Section::Things, window, cx)
            {
                self.things
                    .update(cx, |things, cx| things.select_view(target_view, window, cx));
            }
        }
    }

    pub(crate) fn open_statistics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.account_menu_open = false;
        self.statistics_open = true;
        self.sync_active_section(cx);
        self.focus.update(cx, |focus, cx| focus.open_stats_overview(cx));
        window.focus(&self.focus.focus_handle(cx), cx);
        self.sync_title_bar_appearance(cx);
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(settings) = self.settings.take() {
            let _ = settings.update(cx, |settings, window, cx| settings.close_now(window, cx));
        }
    }

    fn apply_settings_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Sync preferences and feature flags before reloading views so every view sees them live.
        let prefs = self.store.preferences().unwrap_or_default();
        desktop_common::ActivePreferences::sync_from_preferences(
            &prefs,
            desktop_common::sound::enabled(&self.store),
            cx,
        );
        self.features = prefs.features;
        self.rail_items = prefs.rail_items;
        // The default list, smart list visibility, mini calendar, week start and horizons flag show up in the views.
        self.sync_external_all(cx);
        self.has_sync_config =
            !matches!(self.store.sync_mode(), Ok(fern_core::sync::SyncMode::Off) | Err(_));
        self.sync_generation = self.sync_generation.wrapping_add(1);
        self.refresh_cloud_snapshot_from_store(cx);
        if self.has_sync_config {
            self.start_sync(cx);
        } else {
            self.sync_task = None;
            self.stop_cloud_realtime(cx);
            if self.sync != crate::sync::SyncStatus::Off {
                self.set_sync_status(crate::sync::SyncStatus::Off, cx);
            }
        }
        if !self.section.enabled(self.features) {
            let fallback = first_enabled_task_section(self.features);
            self.select(fallback, window, cx);
        }
        self.sync_title_bar_appearance(cx);
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
            self.close_search(cx);
        }
        if let Some(settings) = self.settings {
            let reused = settings.update(cx, |settings, settings_window, cx| {
                if settings.is_closing() {
                    return false;
                }
                if let Some(notes) = notes.clone() {
                    settings.show_release_notes(notes, settings_window, cx);
                } else {
                    if let Some(page) = initial_page {
                        settings.set_page(page, cx);
                    }
                    settings_window.focus(&settings.focus_handle(cx), cx);
                }
                settings_window.activate_window();
                true
            });
            if matches!(reused, Ok(true)) {
                cx.notify();
                return;
            }
            self.settings = None;
        }
        let weak_for_sync = cx.weak_entity();
        let sync_now_cb: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| {
            let _ = weak_for_sync.update(cx, |this, cx| this.sync_now(cx));
        });
        let handle = match settings_window::open(
            self.store.clone(),
            notes,
            initial_page,
            sync_now_cb,
            window,
            cx,
        ) {
            Ok(handle) => handle,
            Err(err) => {
                desktop_common::logging::log_error("settings", format!("failed to open Settings: {err:#}"));
                return;
            }
        };
        if let Ok(settings) = handle.update(cx, |_, _, cx| cx.entity()) {
            cx.subscribe_in(&settings, window, move |this, _, event, window, cx| match event {
                SettingsEvent::Changed => this.apply_settings_changes(window, cx),
                SettingsEvent::Dismissed => {
                    if this.settings == Some(handle) {
                        this.settings = None;
                        cx.notify();
                    }
                }
            })
            .detach();
        }
        self.settings = Some(handle);
        cx.notify();
    }

    /// Opens a task from a `fern://task/…` link or notice, routing to Things when Things is active
    /// or Classic Tasks is turned off.
    pub fn open_task(&mut self, task: fern_core::Id, window: &mut Window, cx: &mut Context<Self>) {
        let use_things = (self.section == Section::Things && self.features.things)
            || (!self.features.tasks && self.features.things);
        if use_things {
            if self.select(Section::Things, window, cx) {
                self.things.update(cx, |things, cx| things.open_task(task, window, cx));
            }
        } else if self.features.tasks && self.select(Section::Tasks, window, cx) {
            self.todo.update(cx, |todo, cx| {
                todo.open_task_in_all(task, cx);
            });
        }
    }

    /// Dispatches any parsed `fern://` deep link (`task` or `plugin`).
    pub fn open_deep_link(&mut self, link: DeepLink, window: &mut Window, cx: &mut Context<Self>) {
        match link {
            DeepLink::Task(id) => self.open_task(id, window, cx),
            DeepLink::Plugin {
                plugin_id,
                command,
                params,
            } => {
                let arg = if params.is_empty() {
                    None
                } else if params.len() == 1 && params[0].0 == "arg" {
                    Some(params[0].1.clone())
                } else {
                    let map: serde_json::Map<String, serde_json::Value> = params
                        .into_iter()
                        .map(|(k, v)| (k, serde_json::Value::String(v)))
                        .collect();
                    Some(serde_json::Value::Object(map).to_string())
                };
                PluginHost::run_command_async(plugin_id, command, arg, cx);
            }
        }
    }

    fn dismiss_any_transient(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if dismiss_plugin_modal(cx) {
            return true;
        }
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
        if self.close_search(cx) {
            cx.notify();
            return true;
        }
        if self.statistics_open {
            if self.focus.update(cx, |focus, cx| focus.dismiss_transient(cx)) {
                return true;
            }
            self.statistics_open = false;
            self.focus.update(cx, |focus, cx| focus.close_stats(cx));
            self.sync_active_section(cx);
            self.sync_title_bar_appearance(cx);
            cx.notify();
            return true;
        }
        match &self.section {
            Section::Tasks | Section::Search => {
                self.todo.update(cx, |todo, cx| todo.dismiss_transient(window, cx))
            }
            Section::Things => {
                self.things.update(cx, |things, cx| things.dismiss_transient(window, cx))
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
            Section::LifeClock => {
                if self.life_clock.update(cx, |view, cx| view.dismiss_transient(cx)) {
                    true
                } else if self.life_clock_rail_hidden {
                    self.set_life_clock_rail_hidden(false, cx);
                    true
                } else {
                    false
                }
            }
            Section::Plugin { .. } => false,
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
            self.rail_items = origin;
        }
        let this = cx.entity().downgrade();
        let on_select = move |section, window: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.select(section, window, cx)).ok();
        };
        let hover_target = cx.entity().downgrade();
        let on_nav_hover = move |section: Section, hovered: bool, cx: &mut App| {
            hover_target
                .update(cx, |this, cx| {
                    let now = if hovered {
                        Some(section)
                    } else {
                        this.nav_hovered.take().filter(|s| s != &section)
                    };
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
        let avatar_badge = cx
            .try_global::<desktop_common::sync::LiveCloudSnapshot>()
            .map(|g| &g.0)
            .filter(|s| s.signed_in)
            .map(|snap| {
                let status = theme.status();
                let dot = if matches!(self.sync, crate::sync::SyncStatus::Failed(_)) {
                    Some(status.error)
                } else {
                    match snap.realtime {
                        desktop_common::sync::RealtimeStatus::Live => Some(status.success),
                        desktop_common::sync::RealtimeStatus::Connecting => Some(status.warning),
                        desktop_common::sync::RealtimeStatus::Disconnected => None,
                        desktop_common::sync::RealtimeStatus::SessionExpired => Some(status.error),
                    }
                };
                sidebar::AvatarBadge {
                    initials: Some(snap.initials.clone()),
                    status_dot: dot,
                    tooltip: Some(if snap.display_name.trim().is_empty() {
                        snap.email.clone()
                    } else {
                        format!("{} ({})", snap.display_name, snap.email)
                    }),
                }
            })
            .unwrap_or_default();
        let account_menu = self.account_menu_open.then(|| {
            PopoverAttachment::new(Placement::RightTop, self.render_account_menu(cx)).into_any_element()
        });
        let features = self.features;
        let sync_button = self.has_sync_config.then(|| sidebar::SyncButton {
            tooltip: self.sync_tooltip(i18n::t(cx), cx),
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

        let main_panel = if !self.statistics_open {
            let focus_clock = self.focus.read(cx).running_clock();
            let plugin_view_entity = if let Section::Plugin {
                ref plugin_id,
                ref view_id,
            } = self.section
            {
                self.plugin_views
                    .get(&(plugin_id.to_string(), view_id.to_string()))
                    .cloned()
            } else {
                None
            };
            let hide_rail = self.section == Section::LifeClock && self.life_clock_rail_hidden;
            let rail = (!hide_rail).then(|| {
                sidebar::render(
                    &self.section,
                    self.nav_hovered.as_ref(),
                    &self.rail_items,
                    self.dragging_rail.as_ref(),
                    &self.rail_scroll,
                    on_nav_hover,
                    on_select,
                    on_start_rail_drag,
                    on_avatar,
                    avatar_badge,
                    account_menu,
                    bell,
                    sync_button,
                    focus_clock,
                    features,
                    window,
                    cx,
                )
            });
            let body = if let Some(widget) = self.builtin_section(&self.section) {
                div().flex_1().min_w_0().h_full().child(widget.render_section())
            } else {
                let mut el = div().flex_1().min_w_0().h_full();
                if let Some(view) = plugin_view_entity {
                    el = el.child(view);
                }
                el
            };
            Some((rail, body))
        } else {
            None
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
            .on_action(cx.listener(|this, _: &GoToHorizons, window, cx| {
                if this.features.tasks
                    && this.features.horizons
                    && desktop_common::calendar::horizons_enabled(cx)
                {
                    this.show_view(fern_core::View::Horizons, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &GoToSummary, window, cx| {
                if this.features.tasks {
                    this.show_view(fern_core::View::Summary, window, cx);
                }
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
            .when_some(main_panel, |el, (rail, body)| el.children(rail).child(body))
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
            .children(self.render_active_alerts(window, cx))
            .child(render_plugin_overlays(window, cx))
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
mod tests;
