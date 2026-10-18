use std::rc::Rc;

use fern_core::Store;
use gpui::Focusable as _;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::actions::{
    NewTask, OpenSettings, Search, ShowInbox, ShowNext7Days, ShowShortcuts, ShowToday, ShowTomorrow, ToggleSidebar,
};
use crate::calendar_view::{self, CalendarView};
use crate::countdown_view::CountdownView;
use crate::focus_view::FocusView;
use crate::habit_view::HabitView;
use crate::i18n;
use crate::matrix_view::{self, MatrixView};
use crate::reminders::{self, Notice, Notices, Target};
use crate::search_modal::{SearchEvent, SearchModal};
use crate::settings_window::{self, SettingsEvent, SettingsWindow};
use crate::sidebar::{self, Section};
use crate::todo_view::TodoView;

pub struct AppView {
    store: Rc<Store>,
    section: Section,
    todo: gpui::Entity<TodoView>,
    calendar: gpui::Entity<CalendarView>,
    matrix: gpui::Entity<MatrixView>,
    countdown: gpui::Entity<CountdownView>,
    habits: gpui::Entity<HabitView>,
    focus: gpui::Entity<FocusView>,
    search: Option<gpui::Entity<SearchModal>>,
    account_menu_open: bool,
    notices: Notices,
    notices_open: bool,
    /// The "Shortcuts" panel on the right, while open.
    shortcuts_open: bool,
    settings: Option<gpui::Entity<SettingsWindow>>,
}

impl AppView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let todo = cx.new(|cx| TodoView::new(store.clone(), window, cx));
        let calendar = cx.new(|cx| CalendarView::new(store.clone(), cx));
        // A task clicked in the calendar opens in the task list with its details.
        cx.subscribe_in(&calendar, window, |this, _, calendar_view::OpenTask(id), window, cx| {
            let id = *id;
            this.select(Section::Tasks, window, cx);
            this.todo.update(cx, |todo, cx| {
                todo.set_view(fern_core::View::All, cx);
                todo.select(id, cx);
            });
        })
        .detach();
        // "Start Focus" in a task's menu starts a Pomodoro for it in the Focus section.
        cx.subscribe_in(&todo, window, |this, _, crate::task_detail::StartFocus(id, title), window, cx| {
            let target = Some((*id, title.clone()));
            this.select(Section::Pomodoro, window, cx);
            this.focus.update(cx, |focus, cx| focus.start(target, cx));
        })
        .detach();
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        cx.subscribe_in(&matrix, window, |this, _, matrix_view::OpenTask(id), window, cx| {
            let id = *id;
            this.select(Section::Tasks, window, cx);
            this.todo.update(cx, |todo, cx| {
                todo.set_view(fern_core::View::All, cx);
                todo.select(id, cx);
            });
        })
        .detach();
        let countdown = cx.new(|cx| CountdownView::new(store.clone(), cx));
        let habits = cx.new(|cx| HabitView::new(store.clone(), cx));
        let focus = cx.new(|cx| FocusView::new(store.clone(), cx));
        // Mutated only by the debug-build dev state below.
        #[cfg_attr(not(debug_assertions), allow(unused_mut))]
        let mut this = Self {
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
            shortcuts_open: false,
            settings: None,
        };
        reminders::start(this.store.clone(), cx);
        settings_window::apply(&this.store.preferences().unwrap_or_default(), window, cx);
        // "Follow System Dark Mode" tracks the system's appearance while the app runs.
        cx.observe_window_appearance(window, |this, window, cx| {
            let preferences = this.store.preferences().unwrap_or_default();
            if preferences.theme == fern_core::ThemeMode::System {
                settings_window::apply(&preferences, window, cx);
            }
        })
        .detach();
        #[cfg(debug_assertions)]
        if let Ok(state) = std::env::var("FERN_DEV_STATE") {
            if state.contains("section=calendar") {
                this.select(Section::Calendar, window, cx);
            }
            if state.contains("calendar=week") {
                this.calendar.update(cx, |c, cx| c.set_mode(crate::calendar_view::CalendarMode::Week, cx));
            }
            if state.contains("section=matrix") {
                this.select(Section::Matrix, window, cx);
            }
            // `countdown=<name>><YYYY-MM-DD>><countdown|anniversary|holiday>`, repeatable.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("countdown=")) {
                let parts: Vec<&str> = spec.split('>').collect();
                if let [name, date, kind] = parts[..]
                    && let Ok(date) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                {
                    let kind = fern_core::CountdownKind::parse(kind);
                    this.countdown.update(cx, |view, cx| view.dev_add(name, date, kind, cx));
                }
            }
            // `habit=<name>><days ago,…>`, repeatable.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("habit=")) {
                let (name, days) = spec.split_once('>').unwrap_or((spec, ""));
                let days: Vec<i64> = days.split(',').filter_map(|d| d.parse().ok()).collect();
                this.habits.update(cx, |view, cx| view.dev_add(name, &days, cx));
            }
            // `habit-select=<name>` opens a habit's details panel; `habit-do=<command>` (see
            // `HabitView::dev_apply`) acts on the selected habit. They run in order.
            for part in state.split(';') {
                if let Some(name) = part.strip_prefix("habit-select=") {
                    this.habits.update(cx, |view, cx| view.dev_select(name, cx));
                } else if let Some(command) = part.strip_prefix("habit-do=") {
                    this.habits.update(cx, |view, cx| view.dev_apply(command, window, cx));
                }
            }
            // `focus=<title>><minutes ago>><minutes>`, repeatable.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("focus=")) {
                let parts: Vec<&str> = spec.split('>').collect();
                if let [title, ago, minutes] = parts[..]
                    && let (Ok(ago), Ok(minutes)) = (ago.parse(), minutes.parse())
                {
                    this.focus.update(cx, |view, cx| view.dev_record(title, ago, minutes, cx));
                }
            }
            if let Some(menu) = state.split(';').find_map(|part| part.strip_prefix("focus-menu=")) {
                this.focus.update(cx, |view, cx| view.dev_menu(menu, cx));
            }
            // `focus-task=<title part>`: what "Start Focus" in a task's menu does.
            if let Some(needle) = state.split(';').find_map(|part| part.strip_prefix("focus-task="))
                && let Some(task) = this.store.tasks().unwrap_or_default().into_iter().find(|t| t.title.contains(needle))
            {
                this.select(Section::Pomodoro, window, cx);
                this.focus.update(cx, |focus, cx| focus.start(Some((task.id, task.title)), cx));
            }
            if state.contains("section=focus") {
                this.select(Section::Pomodoro, window, cx);
            }
            if state.contains("section=habits") {
                this.select(Section::Habits, window, cx);
            }
            if state.contains("section=countdown") {
                this.select(Section::Countdown, window, cx);
            }
            // `notice=<task title part>><hours ago>`: a fired reminder for the bell; `notices=open`.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("notice=")) {
                let (needle, ago) = spec.split_once('>').unwrap_or((spec, "0"));
                if let Some(task) = this.store.tasks().unwrap_or_default().into_iter().find(|t| t.title.contains(needle)) {
                    let at = chrono::Local::now().naive_local() - chrono::Duration::hours(ago.parse().unwrap_or(0));
                    this.add_notices(vec![Notice { target: Target::Task(task.id), title: task.title, at }], cx);
                }
            }
            // `theme=light|dark|system`, saved like a pick in the Settings.
            if let Some(mode) = state.split(';').find_map(|part| part.strip_prefix("theme=")).and_then(fern_core::ThemeMode::from_code) {
                let preferences = fern_core::Preferences { theme: mode, ..this.store.preferences().unwrap_or_default() };
                this.store.save_preferences(&preferences).ok();
                settings_window::apply(&preferences, window, cx);
            }
            // `settings=<page>[><menu>]` opens the Settings window (see `SettingsWindow::dev_open`).
            if let Some(spec) = state.split(';').find_map(|part| part.strip_prefix("settings=")) {
                this.open_settings(window, cx);
                if let Some(settings) = &this.settings {
                    settings.update(cx, |settings, cx| settings.dev_open(spec, cx));
                }
            }
            if state.contains("shortcuts=open") {
                this.shortcuts_open = true;
            }
            if state.contains("notices=open") {
                this.set_notices_open(true, cx);
            }
            if let Some(query) = state.split(';').find_map(|part| part.strip_prefix("search-popover=")) {
                this.open_search(window, cx);
                if let Some(search) = &this.search {
                    search.update(cx, |search, cx| search.set_query(query, cx));
                }
            }
        }
        this
    }

    fn select(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        // Search is a popover over the current screen, as in the original.
        if section == Section::Search {
            self.open_search(window, cx);
            return;
        }
        self.todo.update(cx, |todo, cx| todo.stop_search(cx));
        if section == Section::Calendar {
            self.calendar.update(cx, |calendar, cx| calendar.reload(cx));
        }
        if section == Section::Matrix {
            self.matrix.update(cx, |matrix, cx| matrix.reload(cx));
        }
        if section == Section::Pomodoro {
            self.focus.update(cx, |focus, cx| focus.reload(cx));
        }
        if section == Section::Habits {
            self.habits.update(cx, |habits, cx| habits.reload(cx));
        }
        if section == Section::Countdown {
            self.countdown.update(cx, |countdown, cx| countdown.reload(cx));
        }
        self.section = section;
        cx.notify();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search = cx.new(|cx| SearchModal::new(self.store.clone(), cx));
        cx.subscribe_in(&search, window, |this, _, event, window, cx| {
            this.search = None;
            match event {
                SearchEvent::OpenTask(id) => {
                    let id = *id;
                    this.select(Section::Tasks, window, cx);
                    this.todo.update(cx, |todo, cx| {
                        todo.set_view(fern_core::View::All, cx);
                        todo.select(id, cx);
                    });
                }
                SearchEvent::OpenView(fern_core::View::Search(query)) => {
                    let query = query.clone();
                    this.select(Section::Tasks, window, cx);
                    this.todo.update(cx, |todo, cx| todo.show_search_results(query, cx));
                }
                SearchEvent::OpenView(view) => {
                    let view = view.clone();
                    this.select(Section::Tasks, window, cx);
                    this.todo.update(cx, |todo, cx| todo.set_view(view, cx));
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
        self.select(Section::Tasks, window, cx);
        self.todo.update(cx, |todo, cx| todo.focus_new_task(window, cx));
    }

    fn search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Search, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.todo.update(cx, |todo, cx| todo.toggle_lists(cx));
    }

    fn show_view(&mut self, view: fern_core::View, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Tasks, window, cx);
        self.todo.update(cx, |todo, cx| todo.set_view(view, cx));
    }

    fn set_account_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        self.account_menu_open = open;
        cx.notify();
    }

    fn render_account_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        popover(cx)
            .w(px(180.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_account_menu(false, cx)))
            .child(
                ListItem::new("account-settings")
                    .start_slot(Icon::new(IconName::Settings))
                    .child(Label::new(strings.settings))
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
            .into_any_element()
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.account_menu_open = false;
        let settings = cx.new(|cx| SettingsWindow::new(self.store.clone(), cx));
        cx.subscribe_in(&settings, window, |this, _, event, window, cx| {
            this.settings = None;
            if let SettingsEvent::ShowShortcuts = event {
                this.shortcuts_open = true;
            }
            // The default list and the week start show up in the views.
            this.todo.update(cx, |todo, cx| todo.reload(cx));
            window.focus(&this.todo.focus_handle(cx), cx);
            cx.notify();
        })
        .detach();
        window.focus(&settings.focus_handle(cx), cx);
        self.settings = Some(settings);
        cx.notify();
    }
}

impl AppView {
    /// Opens a task from a `fern://task/…` link, in All with its details.
    pub fn open_task(&mut self, task: fern_core::Id, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Tasks, window, cx);
        self.todo.update(cx, |todo, cx| {
            todo.set_view(fern_core::View::All, cx);
            todo.select(task, cx);
        });
    }

    fn toggle_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.shortcuts_open = !self.shortcuts_open;
        cx.notify();
    }

    /// The original's Shortcuts panel: the keymap's bindings by group, and quick add syntax.
    fn render_shortcuts(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let list = std::str::from_utf8(
            &gpui::AssetSource::load(&assets::Assets, crate::keymap::DEFAULT_KEYMAP_PATH).ok().flatten().unwrap_or_default(),
        )
        .ok()
        .and_then(|source| crate::keymap::shortcut_list(source).ok())
        .unwrap_or_default();
        let chip = |text: String| {
            div()
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(theme.colors().border)
                .child(Label::new(text).size(LabelSize::XSmall).color(Color::Muted))
        };
        let row = |label: &'static str, keys: Vec<String>| {
            h_flex()
                .h(px(32.))
                .child(div().flex_1().child(Label::new(label).size(LabelSize::Small)))
                .child(h_flex().gap_1().children(keys.into_iter().map(chip)))
        };
        let heading = |text: &'static str| div().pt_3().pb_1().child(Label::new(text).weight(gpui::FontWeight::SEMIBOLD));
        let mut body = v_flex().child(heading(strings.shortcut_groups[0]));
        for (i, (action, keys)) in list.into_iter().enumerate() {
            if action.starts_with("task_list::") && i > 0 && list_group_start(i) {
                body = body.child(heading(strings.shortcut_groups[1]));
            }
            if action == "task_list::PinTask" {
                body = body.child(heading(strings.shortcut_groups[3]));
            }
            if !keys.is_empty() {
                body = body.child(row(strings.shortcut_labels[i], keys));
            }
        }
        body = body.child(heading(strings.shortcut_groups[2]));
        for (what, example) in strings.quick_add_help {
            body = body.child(row(what, vec![example.to_owned()]));
        }
        v_flex()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(380.))
            .bg(theme.colors().elevated_surface_background)
            .border_l_1()
            .border_color(theme.colors().border)
            .shadow_lg()
            .child(
                h_flex()
                    .h(px(48.))
                    .px_4()
                    .child(div().flex_1().child(Label::new(strings.shortcuts).size(LabelSize::Large)))
                    .child(IconButton::new("close-shortcuts", IconName::X).on_click(cx.listener(|this, _, _, cx| this.toggle_shortcuts(cx)))),
            )
            .child(v_flex().id("shortcut-list").flex_1().min_h_0().overflow_y_scroll().px_4().pb_4().child(body))
            .into_any_element()
    }

    pub fn add_notices(&mut self, notices: Vec<Notice>, cx: &mut Context<Self>) {
        if !notices.is_empty() {
            self.notices.push(notices);
            cx.notify();
        }
    }

    /// Opening the list marks the notifications read, which clears the badge.
    fn set_notices_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.notices_open = open;
        if open {
            self.notices.mark_read();
        }
        cx.notify();
    }

    fn open_notice(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.set_notices_open(false, cx);
        match target {
            Target::Task(task) => {
                self.select(Section::Tasks, window, cx);
                self.todo.update(cx, |todo, cx| {
                    todo.set_view(fern_core::View::All, cx);
                    todo.select(task, cx);
                });
            }
            Target::Habit(habit) => {
                self.select(Section::Habits, window, cx);
                self.habits.update(cx, |habits, cx| habits.select(habit, cx));
            }
        }
    }

    fn render_notices(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let today = chrono::Local::now().date_naive();
        popover(cx)
            .w(px(300.))
            .max_h(px(420.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_notices_open(false, cx)))
            .child(div().px_2().py_1().child(Label::new(strings.notifications).weight(gpui::FontWeight::SEMIBOLD)))
            .when(self.notices.items.is_empty(), |el| {
                el.child(div().px_2().py_3().child(Label::new(strings.no_notifications).color(Color::Muted)))
            })
            .child(
                v_flex().id("notices").overflow_y_scroll().children(self.notices.items.iter().enumerate().map(|(i, notice)| {
                    let target = notice.target;
                    let (when, _) = crate::task_rows::due_label(notice.at.date(), Some(notice.at.time()), today, strings);
                    ListItem::new(SharedString::from(format!("notice-{i}")))
                        .height(px(44.))
                        .start_slot(Icon::new(IconName::AlarmClock).color(Color::Accent))
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(Label::new(notice.title.clone()).truncate())
                                .child(Label::new(when).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| this.open_notice(target, window, cx)))
                })),
            )
            .into_any_element()
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let this = cx.entity().downgrade();
        let on_select = move |section, window: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.select(section, window, cx)).ok();
        };
        let on_avatar = cx.listener(|this, _, _, cx| this.set_account_menu(true, cx));
        let account_menu = self.account_menu_open.then(|| {
            PopoverAttachment::new(Placement::RightTop, self.render_account_menu(cx)).into_any_element()
        });
        let bell = sidebar::Bell {
            unread: self.notices.unread,
            on_click: Box::new(cx.listener(|this, _, _, cx| {
                let open = !this.notices_open;
                this.set_notices_open(open, cx)
            })),
            menu: self
                .notices_open
                .then(|| PopoverAttachment::new(Placement::RightBottom, self.render_notices(cx)).into_any_element()),
        };

        h_flex()
            .relative()
            .key_context("Workspace")
            .on_action(cx.listener(Self::new_task))
            .on_action(cx.listener(Self::search))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(|this, _: &ShowShortcuts, _, cx| this.toggle_shortcuts(cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
            .on_action(cx.listener(|this, _: &ShowInbox, window, cx| this.show_view(fern_core::View::Inbox, window, cx)))
            .on_action(cx.listener(|this, _: &ShowToday, window, cx| this.show_view(fern_core::View::Today, window, cx)))
            .on_action(cx.listener(|this, _: &ShowTomorrow, window, cx| {
                this.show_view(fern_core::View::Tomorrow, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowNext7Days, window, cx| {
                this.show_view(fern_core::View::Next7Days, window, cx)
            }))
            .size_full()
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(sidebar::render(self.section, on_select, on_avatar, account_menu, bell, cx))
            .child(match self.section {
                Section::Tasks | Section::Search => div().flex_1().h_full().child(self.todo.clone()),
                Section::Calendar => div().flex_1().h_full().child(self.calendar.clone()),
                Section::Matrix => div().flex_1().h_full().child(self.matrix.clone()),
                Section::Countdown => div().flex_1().h_full().child(self.countdown.clone()),
                Section::Habits => div().flex_1().h_full().child(self.habits.clone()),
                Section::Pomodoro => div().flex_1().h_full().child(self.focus.clone()),

            })
            .when(self.shortcuts_open, |el| el.child(self.render_shortcuts(cx)))
            .when_some(self.settings.clone(), |el, settings| {
                el.child(
                    div()
                        .id("settings-backdrop")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(gpui::black().opacity(0.35))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(settings),
                )
            })
            .when_some(self.search.clone(), |el, search| {
                el.child(div().absolute().top(px(160.)).left_0().right_0().flex().justify_center().child(search))
            })
    }
}

/// Where the task list's shortcuts start in `keymap::LISTED_ACTIONS`.
fn list_group_start(index: usize) -> bool {
    crate::keymap::LISTED_ACTIONS.iter().position(|a| a.starts_with("task_list::")) == Some(index)
}
