//! The calendar: tasks placed on their due dates in a month grid or a week with a time axis,
//! matched to TickTick's Calendar section.

use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{CalendarOptions, ColorBy, Id, Priority, Store, Task};
use gpui::{EventEmitter, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use crate::actions::{AgendaView, DayView, MonthView, NewTask, ToggleSidebar, WeekView, YearView};
use crate::calendar::{self, week_axis};
use crate::drag::DraggedTask;
use crate::i18n;
use crate::calendar_options_view::{CalendarOptionsDialog, OptionsEvent, Page};
use crate::task_detail::{StartFocus, TaskDetail, TaskUpdated};
use crate::task_rows;

/// How many tasks a day cell shows before collapsing the rest into "+N".
const TASKS_PER_DAY: usize = 5;

/// The user clicked a task in the calendar.

/// The week-number column of the month grid.
const WEEK_NUMBER_WIDTH: f32 = 32.;
pub struct OpenTask(pub Id);

/// A task's right-click menu asked for one of the details' pickers (`due`, `tags`,
/// `link-parent`): the task opens in the list with that picker.
pub struct OpenTaskPicker(pub Id, pub &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarMode {
    /// One day on the hour axis, as the original's Day view.
    Day,
    Week,
    Month,
    /// The original's Multi-Day: this many days from the anchor on the hour axis.
    MultiDay(u8),
    /// The original's Agenda: the coming days with something due, as a list.
    Agenda,
    /// The original's Year: the twelve months as small calendars.
    Year,
    /// The original's Multi-Week: this many weeks from the anchor's, laid out like the month.
    MultiWeek(u8),
}

/// The Multi-Week counts the menu's stepper goes through.
const MULTI_WEEKS: std::ops::RangeInclusive<u8> = 2..=6;

impl CalendarMode {
    /// Whether the days are drawn on the hour axis (all but Month and Agenda).
    pub fn is_timed(self) -> bool {
        !matches!(self, CalendarMode::Month | CalendarMode::Agenda | CalendarMode::Year | CalendarMode::MultiWeek(_))
    }
}

pub struct CalendarView {
    store: Rc<Store>,
    mode: CalendarMode,
    /// The shown month (first day) in month mode.
    month: NaiveDate,
    /// A day of the shown week in week mode.
    anchor: NaiveDate,
    mode_menu_open: bool,
    focus_handle: gpui::FocusHandle,
    /// The mode menu's Multi-Day side menu, while hovered open.
    multi_day_menu: bool,
    /// The weeks Multi-Week shows, set with the menu's stepper; and whether its row is hovered,
    /// which shows the stepper, as in the original.
    multi_weeks: u8,
    multi_week_hovered: bool,
    tasks: Vec<Task>,
    /// The Agenda's days, from today (or the shown month's first day, when later) a year on.
    agenda: Vec<(NaiveDate, Vec<fern_core::agenda::AgendaItem>)>,
    /// The countdowns the Agenda lists, and the one whose card is open (its date and id).
    countdowns: Vec<fern_core::Countdown>,
    open_countdown: Option<(NaiveDate, Id)>,
    /// A task's right-click menu: the task and where the click was, and its open side menu.
    context_menu: Option<(Id, gpui::Point<Pixels>)>,
    context_side: Option<u8>,
    /// The Year view's day card, opened by clicking a day.
    year_day: Option<NaiveDate>,
    /// The task card a click on a task opens beside it, as in the original: the task, the day
    /// it was clicked on, and the details it shows.
    card: Option<(Id, NaiveDate)>,
    detail: gpui::Entity<TaskDetail>,
    /// View Options, the header's "…" menu and the dialog it opens.
    options: CalendarOptions,
    more_menu: bool,
    options_dialog: Option<gpui::Entity<CalendarOptionsDialog>>,
    /// The "Arrange tasks" panel: the open tasks without a date, grouped by the tab's choice
    /// (lists, tags or priority), to drag onto a day.
    arrange: Option<usize>,
    undated: Vec<Task>,
    /// The colors tasks take with Color By List or Tag.
    list_colors: std::collections::HashMap<Id, gpui::Hsla>,
    tag_colors: std::collections::HashMap<String, gpui::Hsla>,
    list_names: Vec<(Id, String)>,
    /// With Show Habit, the habits due today; with Show Focus Records, the focus sessions.
    habits_today: Vec<fern_core::Habit>,
    focus_sessions: Vec<fern_core::FocusSession>,
}

/// The original's task card: 402 px wide, 301 px tall.
const CARD_WIDTH: f32 = 402.;
const CARD_HEIGHT: f32 = 301.;

/// How far ahead the Agenda reaches.
const AGENDA_DAYS: i64 = 366;

impl EventEmitter<OpenTask> for CalendarView {}
impl EventEmitter<OpenTaskPicker> for CalendarView {}
impl EventEmitter<crate::task_detail::StartFocus> for CalendarView {}

impl gpui::Focusable for CalendarView {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl CalendarView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let today = Local::now().date_naive();
        let detail = cx.new(|cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.set_card();
            detail
        });
        cx.subscribe(&detail, |this, _, _: &TaskUpdated, cx| this.card_updated(cx)).detach();
        cx.subscribe(&detail, |_, _, StartFocus(id, title, stopwatch), cx| cx.emit(StartFocus(*id, title.clone(), *stopwatch))).detach();
        // A subtask clicked in the card opens in the card.
        cx.subscribe(&detail, |this, _, crate::task_detail::OpenTask(id), cx| {
            if let Some((_, day)) = this.card {
                this.open_card(*id, day, cx);
            }
        })
        .detach();
        let mut this = Self {
            store,
            mode: CalendarMode::Month,
            month: calendar::month_start(today),
            anchor: today,
            mode_menu_open: false,
            focus_handle: cx.focus_handle(),
            multi_day_menu: false,
            multi_weeks: 5,
            multi_week_hovered: false,
            tasks: Vec::new(),
            agenda: Vec::new(),
            countdowns: Vec::new(),
            open_countdown: None,
            context_menu: None,
            year_day: None,
            context_side: None,
            card: None,
            detail,
            options: CalendarOptions::default(),
            more_menu: false,
            options_dialog: None,
            arrange: None,
            undated: Vec::new(),
            list_colors: Default::default(),
            tag_colors: Default::default(),
            list_names: Vec::new(),
            habits_today: Vec::new(),
            focus_sessions: Vec::new(),
        };
        this.reload(cx);
        this
    }

    /// The days on the hour axis: the anchor's week, the anchor, or several days from it.
    fn shown_days(&self) -> Vec<NaiveDate> {
        match self.mode {
            CalendarMode::Day => vec![self.anchor],
            CalendarMode::MultiDay(n) => (0..n as i64).map(|i| self.anchor + chrono::Duration::days(i)).collect(),
            _ => calendar::week_of(self.anchor).into_iter().filter(|d| self.options.shows_day(*d)).collect(),
        }
    }

    /// The tasks a day shows, as View Options say.
    fn tasks_on(&self, date: NaiveDate) -> Vec<&Task> {
        self.tasks.iter().filter(|t| self.options.shows_task(t, date)).collect()
    }

    /// A task's color, by its list, first tag or priority as View Options → Color By says.
    fn task_tint(&self, task: &Task, theme: &theme::Theme) -> gpui::Hsla {
        let accent = theme.colors().accent;
        match self.options.color_by {
            ColorBy::List => task.list_id.and_then(|id| self.list_colors.get(&id).copied()).unwrap_or(accent),
            ColorBy::Tag => task.tags.first().and_then(|t| self.tag_colors.get(&t.to_lowercase()).copied()).unwrap_or(accent),
            ColorBy::Priority => task_rows::priority_color(task.priority, theme).unwrap_or(accent),
        }
    }

    /// The extra items of a day, as View Options say: today's habits (as the original's calendar,
    /// on today only), the day's focus records, and its countdowns.
    fn day_extras(&self, date: NaiveDate, cx: &App) -> Vec<AnyElement> {
        let theme = cx.theme();
        let today = Local::now().date_naive();
        let bar = |tint: gpui::Hsla| h_flex().h(px(18.)).flex_none().gap_1().px_1().rounded(px(3.)).bg(tint.opacity(0.28));
        let mut items: Vec<AnyElement> = Vec::new();
        if date == today {
            for habit in &self.habits_today {
                let tint = match self.options.habit_color {
                    fern_core::HabitColor::Theme => theme.colors().accent,
                    fern_core::HabitColor::Fixed => crate::calendar_options_view::palette(self.options.habit_fixed),
                    fern_core::HabitColor::Habit => crate::lists_panel::parse_color(habit.color.as_deref()).unwrap_or(theme.colors().accent),
                };
                items.push(
                    bar(tint)
                        .child(Icon::new(IconName::FernHabit).size(IconSize::XSmall).color(Color::Muted))
                        .child(Label::new(habit.name.clone()).size(LabelSize::XSmall).truncate())
                        .into_any_element(),
                );
            }
        }
        // A focus record: an outlined bar with a timer, the task and the time it started.
        for session in self.options.focus_on(&self.focus_sessions, date) {
            let start = chrono::DateTime::from_timestamp_millis(session.started_at).map(|t| t.with_timezone(&Local).time());
            items.push(
                h_flex()
                    .h(px(18.))
                    .flex_none()
                    .gap_1()
                    .px_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme.colors().accent.opacity(0.4))
                    .child(Icon::new(IconName::Timer).size(IconSize::XSmall).color(Color::Muted))
                    .child(div().flex_1().min_w_0().child(Label::new(session.title.clone()).size(LabelSize::XSmall).color(Color::Muted).truncate()))
                    .when_some(start, |el, start| el.child(Label::new(calendar::format_time(start)).size(LabelSize::XSmall).color(Color::Muted)))
                    .into_any_element(),
            );
        }
        for countdown in self.countdowns.iter().filter(|c| !c.archived && self.options.shows_countdown(c, date)) {
            let tint = match self.options.countdown_color {
                fern_core::CountdownColor::Fixed => crate::calendar_options_view::palette(self.options.countdown_fixed),
                fern_core::CountdownColor::Countdown => crate::lists_panel::parse_color(countdown.color.as_deref()).unwrap_or(theme.status().warning),
            };
            items.push(
                bar(tint)
                    .child(div().size(px(6.)).flex_none().rounded_full().bg(tint))
                    .child(Label::new(countdown.name.clone()).size(LabelSize::XSmall).truncate())
                    .into_any_element(),
            );
        }
        items
    }

    fn open_options(&mut self, page: Page, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let dialog = cx.new(|_| CalendarOptionsDialog::new(store, page));
        cx.subscribe(&dialog, |this, _, event: &OptionsEvent, cx| {
            match event {
                OptionsEvent::Changed(options) => this.options = *options,
                OptionsEvent::Closed => this.options_dialog = None,
            }
            this.reload(cx);
        })
        .detach();
        self.options_dialog = Some(dialog);
        self.more_menu = false;
        cx.notify();
    }

    /// The header's "…" menu, as the original's: View Options and Arrange Tasks.
    fn render_more_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let row = |id: &'static str, icon: IconName, label: &'static str| {
            ListItem::new(id).height(px(34.)).start_slot(Icon::new(icon).color(Color::Muted)).child(Label::new(label))
        };
        popover(cx)
            .w(px(184.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.more_menu = false;
                cx.notify();
            }))
            .child(row("calendar-view-options", IconName::Settings2, strings.view_options).on_click(cx.listener(|this, _, _, cx| this.open_options(Page::Main, cx))))
            .child(row("calendar-arrange", IconName::ListChecks, strings.arrange_tasks).on_click(cx.listener(|this, _, _, cx| {
                this.more_menu = false;
                this.arrange = Some(this.arrange.unwrap_or(0));
                this.reload(cx);
            })))
            .into_any_element()
    }

    /// The original's "Arrange tasks" panel at the calendar's right: the open tasks without a
    /// date under Lists, Tags or Priority tabs, dragged onto a day to plan them.
    fn render_arrange(&self, tab: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let mut groups: Vec<(String, Vec<&Task>)> = Vec::new();
        match tab {
            0 => {
                let mut keys: Vec<(Option<Id>, String)> = vec![(None, strings.inbox.to_owned())];
                keys.extend(self.list_names.iter().map(|(id, name)| (Some(*id), name.clone())));
                for (id, name) in keys {
                    let tasks: Vec<&Task> = self.undated.iter().filter(|t| t.list_id == id).collect();
                    if !tasks.is_empty() {
                        groups.push((name, tasks));
                    }
                }
            }
            1 => {
                let mut names: Vec<String> = self.undated.iter().flat_map(|t| t.tags.iter().cloned()).collect();
                names.sort_by_key(|n| n.to_lowercase());
                names.dedup_by_key(|n| n.to_lowercase());
                for name in names {
                    let tasks: Vec<&Task> = self.undated.iter().filter(|t| t.tags.iter().any(|g| g.eq_ignore_ascii_case(&name))).collect();
                    groups.push((format!("#{name}"), tasks));
                }
                let untagged: Vec<&Task> = self.undated.iter().filter(|t| t.tags.is_empty()).collect();
                if !untagged.is_empty() {
                    groups.push((strings.group_no_tags.to_owned(), untagged));
                }
            }
            _ => {
                for (priority, label) in [Priority::High, Priority::Medium, Priority::Low, Priority::None]
                    .into_iter()
                    .zip([strings.priority_high, strings.priority_medium, strings.priority_low, strings.priority_none])
                {
                    let tasks: Vec<&Task> = self.undated.iter().filter(|t| t.priority == priority).collect();
                    if !tasks.is_empty() {
                        groups.push((label.to_owned(), tasks));
                    }
                }
            }
        }
        let tabs = h_flex().p(px(3.)).rounded(px(8.)).bg(theme.colors().surface_background).children(strings.arrange_tabs.iter().enumerate().map(|(i, label)| {
            div()
                .id(SharedString::from(format!("arrange-tab-{i}")))
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(i == tab, |el| el.bg(theme.colors().element_selected))
                .child(Label::new(*label).size(LabelSize::Small).color(if i == tab { Color::Default } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.arrange = Some(i);
                    cx.notify();
                }))
        }));
        let mut body: Vec<AnyElement> = Vec::new();
        for (name, tasks) in groups {
            body.push(
                h_flex()
                    .pt_2()
                    .pb_1()
                    .gap_1()
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    .child(Label::new(name).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(tasks.len().to_string()).size(LabelSize::XSmall).color(Color::Muted))
                    .into_any_element(),
            );
            body.extend(tasks.into_iter().map(|t| self.task_bar(t, None, cx).h(px(20.)).into_any_element()));
        }
        if body.is_empty() {
            body.push(div().pt_4().child(Label::new(strings.arrange_empty).size(LabelSize::Small).color(Color::Muted)).into_any_element());
        }
        v_flex()
            .id("calendar-arrange")
            .w(px(240.))
            .flex_none()
            .h_full()
            .px_3()
            .pt_3()
            .gap_2()
            .border_l_1()
            .border_color(theme.colors().border_variant)
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.arrange_tasks_title).weight(FontWeight::SEMIBOLD)))
                    .child(IconButton::new("arrange-close", IconName::X).size(px(22.)).icon_size(IconSize::Small).on_click(cx.listener(|this, _, _, cx| {
                        this.arrange = None;
                        cx.notify();
                    }))),
            )
            .child(tabs)
            .child(v_flex().id("arrange-tasks").flex_1().min_h_0().overflow_y_scroll().gap(px(3.)).children(body))
            .into_any_element()
    }

    /// Opens a task's card beside it, the task as clicked on `day`.
    fn open_card(&mut self, id: Id, day: NaiveDate, cx: &mut Context<Self>) {
        let Ok(Some(task)) = self.store.task(id) else { return };
        let lists = self.store.lists().unwrap_or_default();
        self.detail.update(cx, |detail, cx| {
            detail.set_lists(lists);
            detail.show(Some(task), cx);
        });
        self.card = Some((id, day));
        cx.notify();
    }

    fn close_card(&mut self, cx: &mut Context<Self>) {
        if self.card.take().is_some() {
            self.detail.update(cx, |detail, cx| detail.show(None, cx));
            cx.notify();
        }
    }

    /// The card's task changed: the calendar shows the change, and the card closes when the task
    /// is gone.
    fn card_updated(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        let Some((id, _)) = self.card else { return };
        match self.store.task(id) {
            Ok(Some(task)) if !task.is_deleted() => self.detail.update(cx, |detail, cx| detail.refresh(Some(task), cx)),
            _ => self.close_card(cx),
        }
    }

    /// The header's "…" menu and what it opens, for the debug-build dev state
    /// (`calendar-more=menu|options|color|color-by|style|arrange|arrange-tags|extras`).
    #[cfg(debug_assertions)]
    pub fn dev_more(&mut self, what: &str, cx: &mut Context<Self>) {
        match what {
            "menu" => self.more_menu = true,
            "options" => self.open_options(Page::Main, cx),
            "color" => self.open_options(Page::Color, cx),
            "color-by" => {
                self.open_options(Page::Color, cx);
                if let Some(dialog) = &self.options_dialog {
                    dialog.update(cx, |d, _| d.dev_color_menu());
                }
            }
            "style" => self.open_options(Page::Style, cx),
            "color-habit" | "color-countdown" => {
                self.open_options(Page::Color, cx);
                let tab = if what == "color-habit" { 1 } else { 2 };
                if let Some(dialog) = &self.options_dialog {
                    dialog.update(cx, |d, _| d.dev_color_tab(tab));
                }
            }
            "arrange" => self.arrange = Some(0),
            "arrange-tags" => self.arrange = Some(1),
            // Show Habit and Show Focus Records turned on.
            "extras" => {
                let options = CalendarOptions { show_habits: true, show_focus: true, ..self.store.calendar_options().unwrap_or_default() };
                self.store.save_calendar_options(&options).ok();
            }
            _ => {}
        }
        self.reload(cx);
    }

    /// Opens a task's card from the debug-build dev state (`calendar-card=<title part>`).
    #[cfg(debug_assertions)]
    pub fn dev_card(&mut self, needle: &str, cx: &mut Context<Self>) {
        if let Some(task) = self.tasks.iter().find(|t| t.title.contains(needle)) {
            let day = task.due_date.unwrap_or_else(|| Local::now().date_naive());
            self.open_card(task.id, day, cx);
        }
    }

    /// The card a clicked task opens: its details, framed, closed by a click elsewhere or Escape.
    fn render_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        div()
            .id("calendar-task-card")
            .occlude()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(theme.colors().border)
            .bg(theme.colors().background)
            .shadow_lg()
            // The details' own popovers lie outside the card; a click in them keeps it open.
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if !this.detail.read(cx).has_popup() {
                    this.close_card(cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && !this.detail.read(cx).has_popup() {
                    this.close_card(cx);
                }
            }))
            .child(self.detail.clone())
            .into_any_element()
    }

    /// Re-reads the tasks of the shown days.
    fn open_context(&mut self, id: Id, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        self.context_menu = Some((id, position));
        self.context_side = None;
        cx.notify();
    }

    /// A choice in a task's right-click menu (see `task_menu`): edits apply here; the pickers
    /// open the task in the list.
    fn context_action(&mut self, action: crate::task_menu::MenuAction, cx: &mut Context<Self>) {
        use crate::task_menu::MenuAction;
        let Some((id, _)) = self.context_menu else { return };
        if let MenuAction::Side(side) = action {
            self.context_side = Some(side);
            cx.notify();
            return;
        }
        self.context_menu = None;
        self.context_side = None;
        let Some(task) = self.store.task(id).ok().flatten() else {
            cx.notify();
            return;
        };
        if let Some(result) = crate::task_menu::apply(&self.store, &task, action, Local::now().date_naive()) {
            if let Err(err) = result {
                eprintln!("failed to change the task: {err:#}");
            }
            self.reload(cx);
            return;
        }
        match action {
            MenuAction::Due(None) => cx.emit(OpenTaskPicker(id, "due")),
            MenuAction::LinkParent => cx.emit(OpenTaskPicker(id, "link-parent")),
            MenuAction::Tags => cx.emit(OpenTaskPicker(id, "tags")),
            MenuAction::AddSubtask => cx.emit(OpenTaskPicker(id, "subtask")),
            MenuAction::Focus(stopwatch) => cx.emit(crate::task_detail::StartFocus(id, task.title.clone(), stopwatch)),
            MenuAction::CopyLink => cx.write_to_clipboard(gpui::ClipboardItem::new_string(fern_core::link::task_link(id))),
            MenuAction::Delete => {
                if let Err(err) = self.store.delete_task(id) {
                    eprintln!("failed to delete the task: {err:#}");
                }
                self.reload(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The task's right-click menu, as the original's calendar has it (no Add Subtask or Pin).
    fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, position) = self.context_menu?;
        let task = self.store.task(id).ok().flatten()?;
        let lists = self.store.lists().unwrap_or_default();
        let menu = crate::task_menu::render(
            &task,
            &lists,
            crate::task_menu::MenuKind::Calendar,
            self.context_side,
            i18n::t(cx),
            cx,
            Rc::new(|this: &mut Self, action, _, cx| this.context_action(action, cx)),
        );
        Some(crate::task_menu::place(position, menu))
    }

    /// Opens a task's right-click menu from the debug-build dev state (`calendar-context=<title part>`).
    #[cfg(debug_assertions)]
    pub fn dev_context(&mut self, needle: &str, cx: &mut Context<Self>) {
        if let Some(task) = self.tasks.iter().find(|t| t.title.contains(needle)) {
            self.open_context(task.id, gpui::point(px(600.), px(300.)), cx);
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.options = self.store.calendar_options().unwrap_or_default();
        let lists = self.store.lists().unwrap_or_default();
        self.list_colors = lists.iter().filter_map(|l| crate::lists_panel::parse_color(l.color.as_deref()).map(|c| (l.id, c))).collect();
        self.list_names = lists.iter().filter(|l| !l.archived).map(|l| (l.id, l.name.clone())).collect();
        self.tag_colors = self
            .store
            .tags()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| crate::lists_panel::parse_color(t.color.as_deref()).map(|c| (t.name.to_lowercase(), c)))
            .collect();
        if self.mode == CalendarMode::Agenda {
            let from = self.agenda_start();
            self.countdowns = self.store.countdowns().unwrap_or_default();
            match self.store.tasks() {
                Ok(tasks) => {
                    let countdowns: &[fern_core::Countdown] = if self.options.show_countdowns { &self.countdowns } else { &[] };
                    self.agenda = fern_core::agenda::agenda(&tasks, countdowns, from, from + chrono::Duration::days(AGENDA_DAYS))
                }
                Err(err) => eprintln!("failed to load the agenda: {err:#}"),
            }
            cx.notify();
            return;
        }
        let (start, end) = match self.mode {
            CalendarMode::MultiWeek(n) => {
                let weeks = calendar::weeks_from(self.anchor, n as usize);
                (weeks[0][0], weeks[weeks.len() - 1][6])
            }
            // The original's Year marks no tasks on the days, but a day's card lists them.
            CalendarMode::Year => {
                let year = self.month.year();
                (NaiveDate::from_ymd_opt(year, 1, 1).unwrap_or(self.month), NaiveDate::from_ymd_opt(year, 12, 31).unwrap_or(self.month))
            }
            CalendarMode::Month => {
                let weeks = calendar::month_weeks(self.month);
                (weeks[0][0], weeks[weeks.len() - 1][6])
            }
            _ => {
                let days = self.shown_days();
                (days[0], days[days.len() - 1])
            }
        };
        match self.store.tasks_due_between(start, end) {
            Ok(tasks) => self.tasks = tasks,
            Err(err) => eprintln!("failed to load calendar tasks: {err:#}"),
        }
        // Repeating tasks due earlier come back on the shown days ("Show All Repeat Cycle"),
        // and the Arrange panel lists the undated ones.
        let all = self.store.tasks().unwrap_or_default();
        if self.options.show_repeats {
            let shown: std::collections::HashSet<Id> = self.tasks.iter().map(|t| t.id).collect();
            self.tasks.extend(
                all.iter().filter(|t| t.repeat.is_some() && !t.is_closed() && !t.is_deleted() && t.due_date.is_some_and(|d| d < start) && !shown.contains(&t.id)).cloned(),
            );
        }
        self.undated = all.into_iter().filter(|t| t.due_date.is_none() && !t.is_closed() && !t.is_deleted() && t.parent_id.is_none()).collect();
        self.countdowns = self.store.countdowns().unwrap_or_default();
        let today = Local::now().date_naive();
        self.habits_today = if self.options.show_habits {
            self.store
                .habits()
                .unwrap_or_default()
                .into_iter()
                .filter(|h| !h.archived && h.is_due(today, &self.store.checkins(h.id).unwrap_or_default()))
                .collect()
        } else {
            Vec::new()
        };
        self.focus_sessions = if self.options.show_focus { self.store.focus_sessions().unwrap_or_default() } else { Vec::new() };
        cx.notify();
    }

    /// The Agenda starts today in the current month, and on the first day of a later one.
    fn agenda_start(&self) -> NaiveDate {
        Local::now().date_naive().max(self.month)
    }

    fn shift(&mut self, delta: i32, cx: &mut Context<Self>) {
        match self.mode {
            CalendarMode::Month | CalendarMode::Agenda => self.month = calendar::add_months(self.month, delta),
            CalendarMode::Year => self.month = calendar::add_months(self.month, 12 * delta),
            mode => {
                let step = match mode {
                    CalendarMode::Day => 1,
                    CalendarMode::MultiDay(n) => n as i64,
                    CalendarMode::MultiWeek(n) => 7 * n as i64,
                    _ => 7,
                };
                self.anchor += chrono::Duration::days(step * delta as i64);
                self.month = calendar::month_start(self.anchor);
            }
        }
        self.card = None;
        self.reload(cx);
    }

    pub fn set_mode(&mut self, mode: CalendarMode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.mode_menu_open = false;
        if (mode.is_timed() || matches!(mode, CalendarMode::MultiWeek(_))) && calendar::month_start(self.anchor) != self.month {
            self.anchor = self.month;
        }
        self.card = None;
        self.reload(cx);
    }

    /// Opens a countdown's card in the Agenda, for the debug-build dev state.
    #[cfg(debug_assertions)]
    pub fn dev_open_countdown(&mut self, name: &str, cx: &mut Context<Self>) {
        self.open_countdown = self
            .agenda
            .iter()
            .find_map(|(date, items)| items.iter().find(|i| i.countdown && i.title.contains(name)).map(|i| (*date, i.task)));
        cx.notify();
    }

    /// The mode menu with its Multi-Day side menu open, for the debug-build dev state.
    #[cfg(debug_assertions)]
    pub fn dev_year_day(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        self.year_day = Some(day);
        cx.notify();
    }

    pub fn dev_open_menu(&mut self, cx: &mut Context<Self>) {
        self.mode_menu_open = true;
        self.multi_day_menu = true;
        cx.notify();
    }

    /// Moves a dropped task to `date`, keeping its time of day.
    fn drop_on_day(&mut self, id: Id, date: NaiveDate, cx: &mut Context<Self>) {
        let time = self.store.task(id).ok().flatten().and_then(|t| t.due_time);
        if let Err(err) = self.store.set_due(id, Some(date), time) {
            eprintln!("failed to move the task: {err:#}");
        }
        self.reload(cx);
    }

    fn go_to_today(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        self.month = calendar::month_start(today);
        self.anchor = today;
        self.reload(cx);
    }

    fn render_day(&self, date: NaiveDate, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tasks = self.tasks_on(date);
        let extras = self.day_extras(date, cx);
        let hidden = (tasks.len() + extras.len()).saturating_sub(TASKS_PER_DAY);
        let room = TASKS_PER_DAY.saturating_sub(tasks.len());
        // The first day of a month carries the month name, as in the original: "Oct 1".
        let number = if date.day() == 1 {
            let month = strings.months_short[date.month0() as usize];
            if strings.day_before_month { format!("1 {month}") } else { format!("{month} 1") }
        } else {
            date.day().to_string()
        };

        let accent = theme.colors().accent;
        v_flex()
            .id(SharedString::from(format!("calendar-day-{date}")))
            .flex_1()
            .min_w_0()
            .h_full()
            .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.12)))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on_day(dragged.id, date, cx)))
            .gap(px(2.))
            .px(px(2.))
            .pt_1()
            .border_r_1()
            .border_b_1()
            .border_color(theme.colors().border_variant)
            .child(
                h_flex().h(px(26.)).px_1().child(
                    div()
                        .min_w(px(22.))
                        .h(px(22.))
                        .px_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .when(date == today, |el| el.bg(theme.colors().accent))
                        // As in the original, the days of the months around are muted.
                        .child(Label::new(number).weight(FontWeight::SEMIBOLD).color(
                            if self.mode == CalendarMode::Month && date.month() != self.month.month() && date != today {
                                Color::Muted
                            } else {
                                Color::Default
                            },
                        )),
                ),
            )
            .children(tasks.iter().take(TASKS_PER_DAY).map(|task| self.task_bar(task, Some(date), cx)))
            // Habits, focus records and countdowns, as in the original's month.
            .children(extras.into_iter().take(room))
            .when(hidden > 0, |el| {
                el.child(div().px_1().child(Label::new(format!("+{hidden}")).size(LabelSize::XSmall).color(Color::Muted)))
            })
            .into_any_element()
    }

    /// A task's bar on `day`; `None` in the Year view's day card, where a click opens the task in
    /// the list.
    fn task_bar(&self, task: &Task, day: Option<NaiveDate>, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme().clone();
        let id = task.id;
        let done = task.is_completed();
        let tint = self.task_tint(task, &theme);
        let modern = self.options.modern;
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        h_flex()
            .id(SharedString::from(format!("calendar-task-{id}")))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .h(px(18.))
            .flex_none()
            .gap_1()
            .px_1()
            .rounded(px(3.))
            .cursor_pointer()
            // Style → Item Style: Classic fills the bar, Modern tints it lighter with a colored edge.
            .bg(tint.opacity(match (done, modern) {
                (true, _) => 0.12,
                (false, true) => 0.16,
                (false, false) => 0.28,
            }))
            .when(modern, |el| el.border_l_2().border_color(tint))
            .hover(|s| s.bg(tint.opacity(0.4)))
            .child(
                div()
                    .size(px(10.))
                    .flex_none()
                    .rounded(px(2.))
                    .border_1()
                    .border_color(theme.colors().text_muted)
                    .when(done, |el| el.bg(theme.colors().text_muted)),
            )
            .child(
                Label::new(task.title.clone())
                    .size(LabelSize::XSmall)
                    .color(if done { Color::Muted } else { Color::Default })
                    .strikethrough(done)
                    .truncate(),
            )
            .on_click(cx.listener(move |this, _, _, cx| match day {
                Some(day) => this.open_card(id, day, cx),
                None => cx.emit(OpenTask(id)),
            }))
            .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| this.open_context(id, event.position, cx)))
            // As in the original, the card opens on the bar's left, or on its right in the first
            // days of the week, where there is no room.
            .when_some(day.filter(|d| self.card == Some((id, *d))), |el, day| {
                let column = calendar::week_of(day).iter().position(|d| *d == day).unwrap_or(0);
                let placement = if column < 3 { Placement::RightOf } else { Placement::LeftOf };
                el.relative().child(PopoverAttachment::new(placement, self.render_card(cx)))
            })
    }

    fn render_week(&self, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let week = self.shown_days();
        let gutter = px(56.);
        let line = theme.colors().border_variant;

        // All-day strip: day numbers and tasks without a time.
        let all_day = h_flex()
            .flex_none()
            .items_start()
            .min_h(px(64.))
            .border_b_1()
            .border_color(line)
            .child(div().w(gutter).flex_none())
            .children(week.iter().copied().map(|date| {
                let accent = theme.colors().accent;
                let tasks: Vec<AnyElement> = self
                    .tasks
                    .iter()
                    .filter(|t| self.options.shows_task(t, date) && t.due_time.is_none())
                    .map(|t| self.task_bar(t, Some(date), cx).into_any_element())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .chain(self.day_extras(date, cx))
                    .collect();
                v_flex()
                    .id(SharedString::from(format!("week-allday-{date}")))
                    .flex_1()
                    .min_w_0()
                    .self_stretch()
                    .gap(px(2.))
                    .px(px(2.))
                    .pb_1()
                    .border_l_1()
                    .border_color(line)
                    .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.12)))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on_day(dragged.id, date, cx)))
                    .child(
                        h_flex().h(px(26.)).px_1().child(
                            div()
                                .min_w(px(22.))
                                .h(px(22.))
                                .px_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .when(date == today, |el| el.bg(theme.colors().accent))
                                .child(Label::new(date.day().to_string()).weight(FontWeight::SEMIBOLD)),
                        ),
                    )
                    .children(tasks)
            }));

        // Time axis labels and hour lines.
        let hour_label = |hour: u32| -> SharedString {
            if hour == 12 { strings.noon.into() } else { calendar::format_hour(hour).into() }
        };
        let now_color = theme.status().error;
        let mut axis = v_flex().w(gutter).flex_none().child(
            div()
                .h(px(week_axis::FOLDED_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.colors().panel_background)
                .flex_col()
                .child(Label::new(calendar::format_hour(0)).size(LabelSize::XSmall).color(Color::Muted))
                .child(Label::new(format!("– {}", calendar::format_hour(7))).size(LabelSize::XSmall).color(Color::Muted)),
        );
        for hour in week_axis::FIRST_HOUR..week_axis::LAST_HOUR {
            axis = axis.child(
                div()
                    .h(px(week_axis::HOUR_HEIGHT))
                    .px_1()
                    .flex()
                    .justify_end()
                    .child(Label::new(hour_label(hour)).size(LabelSize::XSmall).color(Color::Muted)),
            );
        }
        axis = axis.child(
            div()
                .h(px(week_axis::FOLDED_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.colors().panel_background)
                .flex_col()
                .child(Label::new(calendar::format_hour(21)).size(LabelSize::XSmall).color(Color::Muted))
                .child(Label::new(format!("– {}", calendar::format_hour(0))).size(LabelSize::XSmall).color(Color::Muted)),
        );

        let columns = week.iter().copied().map(|date| {
            let accent = theme.colors().accent;
            let timed: Vec<AnyElement> = self
                .tasks
                .iter()
                .filter(|t| self.options.show_completed || !t.is_completed())
                .filter_map(|t| calendar::timed_segment(t, date).map(|segment| (t, segment)))
                .map(|(t, (from, to))| {
                    // A span is drawn as a block from its start to its end; a point in time as a bar.
                    let height = (week_axis::offset(to) - week_axis::offset(from)).max(18.);
                    div()
                        .absolute()
                        .top(px(week_axis::offset(from)))
                        .left(px(2.))
                        .right(px(2.))
                        .h(px(height))
                        .child(self.task_bar(t, Some(date), cx).h(px(height)).items_start().pt(px(1.)))
                        .into_any_element()
                })
                .collect();
            let mut hour_lines = Vec::new();
            for hour in week_axis::FIRST_HOUR..=week_axis::LAST_HOUR {
                let time = chrono::NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or_default();
                hour_lines.push(
                    div().absolute().top(px(week_axis::offset(time))).left_0().right_0().h(px(1.)).bg(line).into_any_element(),
                );
            }
            div()
                .id(SharedString::from(format!("week-day-{date}")))
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(week_axis::total_height()))
                .border_l_1()
                .border_color(line)
                .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.08)))
                .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on_day(dragged.id, date, cx)))
                .children(hour_lines)
                .children(timed)
                // The original's red line at the current time, with a dot where it starts.
                .when(date == today, |el| {
                    let top = week_axis::offset(Local::now().time());
                    el.child(div().absolute().top(px(top - 1.)).left_0().right_0().h(px(2.)).bg(now_color)).child(
                        div().absolute().top(px(top - 4.)).left(px(-4.)).size(px(8.)).rounded_full().bg(now_color),
                    )
                })
        });

        v_flex()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(line)
            .child(all_day)
            .child(
                div()
                    .id("week-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        h_flex()
                            .items_start()
                            .child(
                                // The current time in red on the axis, as in the original.
                                div().relative().child(axis).when(week.contains(&today), |el| {
                                    let now = Local::now().time();
                                    el.child(
                                        div()
                                            .absolute()
                                            .top(px(week_axis::offset(now) - 7.))
                                            .right(px(4.))
                                            .px_0p5()
                                            .bg(theme.colors().background)
                                            .child(
                                                Label::new(calendar::format_time(now))
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Custom(now_color)),
                                            ),
                                    )
                                }),
                            )
                            .children(columns),
                    ),
            )
            .into_any_element()
    }

    /// The original's Agenda: a row per day — the day, "All Day" or the time, a circle and a
    /// card per task — with a heading where a new month starts, and today always shown.
    fn render_agenda(&self, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let start = self.agenda_start();
        let mut days = self.agenda.clone();
        if start == today && days.first().is_none_or(|(date, _)| *date != today) {
            days.insert(0, (today, Vec::new()));
        }
        let mut month = calendar::month_start(start);
        let mut rows: Vec<AnyElement> = Vec::new();
        for (date, items) in days {
            if calendar::month_start(date) != month {
                month = calendar::month_start(date);
                rows.push(
                    div()
                        .pt(px(28.))
                        .pb(px(10.))
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .child(calendar::month_title(month, strings))
                        .into_any_element(),
                );
            }
            let is_today = date == today;
            // The original's day label: the number semibold, the weekday beside it on its baseline
            // (the boxes' bottoms, less the number's deeper descent).
            let day = h_flex()
                .w(px(120.))
                .flex_none()
                .gap_2()
                .items_end()
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if is_today { theme.colors().accent } else { theme.colors().text })
                        .child(date.day().to_string()),
                )
                .child(
                    div().pb(px(4.)).child(
                        Label::new(strings.weekdays_short[date.weekday().num_days_from_monday() as usize])
                            .size(LabelSize::XSmall)
                            .weight(FontWeight::MEDIUM)
                            .color(if is_today { Color::Accent } else { Color::Muted }),
                    ),
                );
            let entries: Vec<AnyElement> = if items.is_empty() {
                vec![
                    h_flex()
                        .gap_3()
                        .child(div().w(px(72.)).flex_none().child(Label::new(strings.range_all_day).size(LabelSize::Small).color(Color::Muted)))
                        .child(div().size(px(10.)).flex_none().rounded_full().bg(theme.colors().element_hover))
                        // The original's banner: a light sky with a sun, drawn for Fern rather than
                        // copied, the words dark on it.
                        .child(
                            h_flex()
                                .flex_1()
                                .h(px(64.))
                                .px_5()
                                .rounded(px(8.))
                                .bg(gpui::linear_gradient(
                                    90.,
                                    gpui::linear_color_stop(gpui::rgb(0xdbe9f7), 0.),
                                    gpui::linear_color_stop(gpui::rgb(0x9fc7ee), 1.),
                                ))
                                .child(div().flex_1().text_color(gpui::rgb(0x334155)).child(Label::new(strings.no_tasks_today).color(Color::Custom(gpui::rgb(0x334155).into()))))
                                .child(
                                    div()
                                        .size(px(44.))
                                        .mr(px(80.))
                                        .rounded_full()
                                        .bg(gpui::linear_gradient(
                                            180.,
                                            gpui::linear_color_stop(gpui::rgb(0xfde68a), 0.),
                                            gpui::linear_color_stop(gpui::rgb(0xf59e0b), 1.),
                                        )),
                                ),
                        )
                        .into_any_element(),
                ]
            } else {
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let id = item.task;
                        // Countdowns, as in the original: an orange card and a filled dot instead of
                        // the task's circle; a click shows the countdown's card.
                        let countdown = item.countdown;
                        let item_color = item.color.clone();
                        let tint = if countdown {
                            theme.status().warning
                        } else {
                            task_rows::priority_color(item.priority, &theme).unwrap_or(theme.colors().accent)
                        };
                        let when = item.time.map_or(strings.range_all_day.to_owned(), calendar::format_time);
                        h_flex()
                            .gap_3()
                            .child(div().w(px(72.)).flex_none().child(Label::new(when).size(LabelSize::Small).color(Color::Muted)))
                            // As in the original: a countdown's round icon, a repeating task's repeat
                            // mark, or a task's circle.
                            .child(div().w(px(16.)).flex_none().flex().justify_center().child(if countdown {
                                div()
                                    .size(px(16.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(crate::countdown_view::countdown_color(item_color.as_deref(), cx).opacity(0.5))
                                    .text_size(px(9.))
                                    .child(match item.icon.clone() {
                                        Some(icon) => icon.into_any_element(),
                                        None => Icon::new(crate::countdown_view::kind_icon(item.kind.unwrap_or_default()))
                                            .size(IconSize::XSmall)
                                            .color(Color::Default)
                                            .into_any_element(),
                                    })
                                    .into_any_element()
                            } else if item.repeats {
                                Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Muted).into_any_element()
                            } else {
                                div().size(px(10.)).rounded_full().border_1().border_color(theme.colors().text_muted).into_any_element()
                            }))
                            .child(
                                h_flex()
                                    .id(SharedString::from(format!("agenda-{date}-{i}")))
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(40.))
                                    .pl_3()
                                    .pr_3()
                                    .gap_2()
                                    .rounded(px(6.))
                                    .border_l_3()
                                    .border_color(tint)
                                    .bg(tint.opacity(0.12))
                                    .hover(|s| s.bg(tint.opacity(0.2)))
                                    .cursor_pointer()
                                    .child(div().flex_1().min_w_0().child(Label::new(item.title).truncate()))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if countdown {
                                            this.open_countdown = Some((date, id));
                                            cx.notify();
                                        } else {
                                            cx.emit(OpenTask(id))
                                        }
                                    }))
                                    .when(!countdown, |el| {
                                        el.on_mouse_down(
                                            gpui::MouseButton::Right,
                                            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| this.open_context(id, event.position, cx)),
                                        )
                                    })
                                    .when_some(
                                        self.open_countdown
                                            .filter(|open| *open == (date, id))
                                            .and_then(|_| self.countdowns.iter().find(|c| c.id == id)),
                                        |el, shown| {
                                            el.relative().child(PopoverAttachment::new(
                                                Placement::BelowRight,
                                                popover(cx)
                                                    .w(px(220.))
                                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                                        this.open_countdown = None;
                                                        cx.notify();
                                                    }))
                                                    .child(crate::countdown_view::render_summary(shown, date, cx))
                                                    .into_any_element(),
                                            ))
                                        },
                                    ),
                            )
                            .into_any_element()
                    })
                    .collect()
            };
            rows.push(
                h_flex()
                    .py(px(30.))
                    .items_start()
                    .child(div().pt(px(6.)).child(day))
                    .child(v_flex().flex_1().gap_2().children(entries))
                    .into_any_element(),
            );
        }
        v_flex()
            .id("agenda")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pl(px(185.))
            .pr(px(60.))
            .pb_6()
            // The original's agenda: 185 px in from the left, up to 1030 px wide, each day 100 px
            // apart.
            .child(v_flex().pt(px(14.)).max_w(px(1030.)).children(rows))
            .into_any_element()
    }

    fn render_mode_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let items = [
            (CalendarMode::Day, strings.day, "D/1"),
            (CalendarMode::Week, strings.week, "W/2"),
            (CalendarMode::Month, strings.month, "M/3"),
            (CalendarMode::Year, strings.year, "Y/4"),
            (CalendarMode::Agenda, strings.agenda, "A/5"),
        ];

        popover(cx)
            .w(px(170.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.mode_menu_open = false;
                cx.notify();
            }))
            .children(items.into_iter().map(|(mode, label, hint)| {
                let selected = self.mode == mode;
                ListItem::new(label)
                    .start_slot(div().w(px(14.)).when(selected, |el| el.child(Label::new("✓").color(Color::Accent))))
                    .child(Label::new(label).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }))
                    .end_slot(Label::new(hint).size(LabelSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
            }))
            .child(div().py_1().child(Divider::horizontal()))
            // The original's Multi-Day, with the number of days in a side menu.
            .child(
                SubmenuItem::new("multi-day", strings.multi_day)
                    .open(self.multi_day_menu)
                    .placement(Placement::LeftSide)
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered && !this.multi_day_menu {
                            this.multi_day_menu = true;
                            cx.notify();
                        }
                    }))
                    .items((2..=6u8).map(|n| {
                        let selected = self.mode == CalendarMode::MultiDay(n);
                        ListItem::new(SharedString::from(format!("multi-day-{n}")))
                            .child(Label::new(strings.n_days.replace("{n}", &n.to_string())).color(if selected { Color::Accent } else { Color::Default }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.multi_day_menu = false;
                                this.set_mode(CalendarMode::MultiDay(n), cx)
                            }))
                    }))

            )
            .child(self.render_multi_week_item(cx))
            .into_any_element()
    }

    /// The menu's Multi-Week row: the week count, and while hovered a "− n +" stepper, as in the
    /// original. Clicking the row shows that many weeks.
    fn render_multi_week_item(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let n = self.multi_weeks;
        let selected = matches!(self.mode, CalendarMode::MultiWeek(_));
        let step = |id: &'static str, icon: IconName, delta: i8| {
            IconButton::new(id, icon).size(px(20.)).icon_size(IconSize::XSmall).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                let n = (this.multi_weeks as i8 + delta).clamp(*MULTI_WEEKS.start() as i8, *MULTI_WEEKS.end() as i8) as u8;
                this.multi_weeks = n;
                if matches!(this.mode, CalendarMode::MultiWeek(_)) {
                    this.mode = CalendarMode::MultiWeek(n);
                    this.reload(cx);
                }
                cx.notify();
            }))
        };
        h_flex()
            .id("multi-week")
            .h(px(32.))
            .mx_1()
            .px_2()
            .gap_2()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.colors().element_hover))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.multi_week_hovered = *hovered;
                cx.notify();
            }))
            .child(div().w(px(14.)).when(selected, |el| el.child(Label::new("✓").color(Color::Accent))))
            .child(
                div().flex_1().child(
                    Label::new(strings.multi_week).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }),
                ),
            )
            .map(|el| {
                if self.multi_week_hovered {
                    el.child(step("multi-week-fewer", IconName::Minus, -1))
                        .child(Label::new(n.to_string()).size(LabelSize::Small))
                        .child(step("multi-week-more", IconName::Plus, 1))
                } else {
                    el.child(Label::new(strings.n_weeks.replace("{n}", &n.to_string())).size(LabelSize::XSmall).color(Color::Muted))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| this.set_mode(CalendarMode::MultiWeek(this.multi_weeks), cx)))
            .into_any_element()
    }

    /// The original's Year: the twelve months in three columns, today marked; a click on a day
    /// opens it in the Day view.
    /// The Year view, as the original's: the months as small calendars filling the view, four
    /// columns when it is wide and three otherwise; a click on a day opens its card beside it.
    fn render_year(&self, today: NaiveDate, width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let year = self.month.year();
        let columns = if width >= px(1400.) { 4 } else { 3 };
        let open = self.year_day;
        let months: Vec<AnyElement> = (1..=12u32)
            .map(|m| {
                let month = NaiveDate::from_ymd_opt(year, m, 1).expect("every month has a first day");
                let days = calendar::month_grid(month);
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().pb_3().text_size(px(18.)).font_weight(FontWeight::MEDIUM).child(strings.months[month.month0() as usize]))
                    .child(h_flex().h(px(30.)).children(calendar::weekday_labels(&strings.weekdays_narrow).map(|day| {
                        div().flex_1().flex().justify_center().child(Label::new(day).size(LabelSize::Small).color(Color::Muted))
                    })))
                    .children(days.chunks(7).map(|week| {
                        h_flex().flex_1().min_h(px(22.)).children(week.iter().map(|&date| {
                            let outside = date.month() != month.month();
                            let is_open = open == Some(date) && !outside;
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .id(SharedString::from(format!("year-{month}-{date}")))
                                        .relative()
                                        .size(px(26.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .cursor_pointer()
                                        .when(date == today && !outside, |el| el.bg(theme.colors().accent))
                                        .when(is_open && date != today, |el| el.bg(theme.colors().element_selected))
                                        // Style → Heatmap: a day tinted by how many items it has.
                                        .when(self.options.heatmap && !outside && date != today && !is_open, |el| {
                                            let count = self.tasks_on(date).len();
                                            el.when(count > 0, |el| el.rounded(px(4.)).bg(theme.colors().accent.opacity((0.2 + 0.15 * count as f32).min(0.8))))
                                        })
                                        .hover(|s| s.bg(theme.colors().element_hover))
                                        .child(Label::new(date.day().to_string()).color(if outside { Color::Muted } else { Color::Default }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.year_day = if this.year_day == Some(date) { None } else { Some(date) };
                                            cx.notify();
                                        }))
                                        .when(is_open, |el| el.child(PopoverAttachment::new(Placement::RightTop, self.render_year_card(date, cx)))),
                                )
                        }))
                    }))
                    .into_any_element()
            })
            .collect();
        let mut months = months.into_iter().peekable();
        let mut rows = Vec::new();
        while months.peek().is_some() {
            rows.push(h_flex().flex_1().min_h(px(240.)).items_stretch().gap(px(60.)).children(months.by_ref().take(columns)));
        }
        v_flex()
            .id("year")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(60.))
            .pt_2()
            .pb_4()
            .gap(px(24.))
            .children(rows)
            .into_any_element()
    }

    /// The original's day card in the Year view: the date, then the day's tasks or "No tasks
    /// scheduled."; a task opens as from the month.
    fn render_year_card(&self, date: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let tasks: Vec<Task> = self.tasks_on(date).into_iter().cloned().collect();
        let bars: Vec<_> = tasks.iter().map(|task| self.task_bar(task, None, cx).h(px(22.))).collect();
        popover(cx)
            .w(px(328.))
            .p_3()
            .gap_1p5()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.year_day = None;
                cx.notify();
            }))
            .child(Label::new(calendar::format_date(date, strings)).weight(FontWeight::SEMIBOLD))
            .when(bars.is_empty(), |el| el.child(Label::new(strings.no_tasks_scheduled).size(LabelSize::Small).color(Color::Muted)))
            .children(bars)
            .into_any_element()
    }
}

impl Render for CalendarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let shown_weeks = match self.mode {
            CalendarMode::MultiWeek(n) => calendar::weeks_from(self.anchor, n as usize),
            _ => calendar::month_weeks(self.month),
        };
        let weeks: Vec<AnyElement> = shown_weeks
            .into_iter()
            .map(|week| {
                h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h_0()
                    // Settings → Show Week Numbers: "W42" before each week, as in the original.
                    .when(calendar::week_numbers(), |el| {
                        el.child(
                            div()
                                .w(px(WEEK_NUMBER_WIDTH))
                                .flex_none()
                                .pt_1()
                                .flex()
                                .justify_center()
                                .child(Label::new(format!("W{}", calendar::week_number(&week))).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    })
                    .children(week.into_iter().filter(|d| self.options.shows_day(*d)).map(|date| self.render_day(date, today, cx)))
                    .into_any_element()
            })
            .collect();
        let bordered = |id: &'static str| {
            // The original's header buttons: 30 px boxes with text at the body size.
            h_flex()
                .id(id)
                .h(px(30.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .bg(theme.colors().surface_background)
        };

        let arrange = self.arrange.map(|tab| self.render_arrange(tab, cx));
        let calendar = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .h(px(50.))
                    .flex_none()
                    .pl(px(16.))
                    .pr(px(12.))
                    .gap_2()
                    .child(
                        IconButton::new("calendar-toggle-sidebar", IconName::PanelLeft)
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx)),
                    )
                    .child(
                        div().flex_1().child(
                            Label::new(match self.mode {
                                // The original's Year shows just the year.
                                CalendarMode::Year => self.month.year().to_string(),
                                mode => calendar::month_title(
                                    match mode {
                                        CalendarMode::Month | CalendarMode::Agenda => self.month,
                                        CalendarMode::Week | CalendarMode::MultiWeek(_) => calendar::week_of(self.anchor)[0],
                                        _ => self.anchor,
                                    },
                                    strings,
                                ),
                            })
                                .size(LabelSize::Large)
                                .weight(FontWeight::SEMIBOLD),
                        ),
                    )
                    .when(self.mode == CalendarMode::Week && calendar::week_numbers(), |el| {
                        el.child(
                            Label::new(format!("W{}", calendar::week_number(&calendar::week_of(self.anchor))))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .child(
                        bordered("calendar-add-frame").child(
                            IconButton::new("calendar-add", IconName::Plus)
                                .size(px(28.))
                                .icon_size(IconSize::Small)
                                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewTask), cx)),
                        ),
                    )
                    .child(
                        div()
                            .relative()
                            .child(
                                bordered("calendar-mode")
                                    .px_3()
                                    .gap_1()
                                    .cursor_pointer()
                                    .child(
                                        Label::new(match self.mode {
                                            CalendarMode::Week => strings.week,
                                            CalendarMode::Day => strings.day,
                                            CalendarMode::MultiDay(_) => strings.multi_day,
                                            CalendarMode::Month => strings.month,
                                            CalendarMode::Agenda => strings.agenda,
                                            CalendarMode::Year => strings.year,
                                            CalendarMode::MultiWeek(_) => strings.multi_week,
                                        }),
                                    )
                                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mode_menu_open = true;
                                        cx.notify();
                                    })),
                            )
                            .when(self.mode_menu_open, |el| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, self.render_mode_menu(cx)))
                            }),
                    )
                    .child(
                        bordered("calendar-nav")
                            .child(
                                IconButton::new("calendar-prev", IconName::ChevronLeft)
                                    .size(px(28.))
                                    .icon_size(IconSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| this.shift(-1, cx))),
                            )
                            .child(
                                div()
                                    .id("calendar-today")
                                    .h_full()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .border_x_1()
                                    .border_color(theme.colors().border)
                                    .cursor_pointer()
                                    .child(Label::new(strings.today))
                                    .on_click(cx.listener(|this, _, _, cx| this.go_to_today(cx))),
                            )
                            .child(
                                IconButton::new("calendar-next", IconName::ChevronRight)
                                    .size(px(28.))
                                    .icon_size(IconSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| this.shift(1, cx))),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .child(IconButton::new("calendar-more", IconName::Ellipsis).on_click(cx.listener(|this, _, _, cx| {
                                this.more_menu = !this.more_menu;
                                cx.notify();
                            })))
                            .when(self.more_menu, |el| el.child(PopoverAttachment::new(Placement::BelowRight, self.render_more_menu(cx)))),
                    ),
            )
            .when(!matches!(self.mode, CalendarMode::Agenda | CalendarMode::Year), |el| el.child(
                h_flex()
                    .flex_none()
                    .h(px(22.))
                    .when(self.mode.is_timed(), |el| el.child(div().w(px(56.)).flex_none()))
                    .when(matches!(self.mode, CalendarMode::Month | CalendarMode::MultiWeek(_)) && calendar::week_numbers(), |el| {
                        el.child(div().w(px(WEEK_NUMBER_WIDTH)).flex_none())
                    })
                    // The days' weekday names: the week's, or those of the shown days.
                    .children(match self.mode {
                        CalendarMode::Month | CalendarMode::Week | CalendarMode::MultiWeek(_) => {
                            let shown = calendar::week_of(self.anchor).map(|d| self.options.shows_day(d));
                            crate::calendar::weekday_labels(&strings.weekdays_short).into_iter().zip(shown).filter(|(_, on)| *on).map(|(l, _)| l).collect::<Vec<_>>()
                        }
                        _ => self
                            .shown_days()
                            .iter()
                            .map(|d| strings.weekdays_short[d.weekday().num_days_from_monday() as usize])
                            .collect(),
                    }
                    .into_iter()
                    .map(|day| {
                        div()
                            .flex_1()
                            .flex()
                            .justify_center()
                            .child(Label::new(day).size(LabelSize::Small).color(Color::Muted))
                    })),
            ))
            .map(|el| match self.mode {
                CalendarMode::Agenda => el.child(self.render_agenda(today, cx)),
                CalendarMode::Year => el.child(self.render_year(today, window.viewport_size().width, cx)),
                CalendarMode::Month | CalendarMode::MultiWeek(_) => el.child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .border_t_1()
                        .border_color(theme.colors().border_variant)
                        .children(weeks),
                ),
                _ => el.child(self.render_week(today, cx)),
            });
        h_flex()
            .relative()
            .track_focus(&self.focus_handle)
            .key_context("Calendar")
            // The view menu's keys, as in the original: D/1, W/2, M/3.
            .on_action(cx.listener(|this, _: &DayView, _, cx| this.set_mode(CalendarMode::Day, cx)))
            .on_action(cx.listener(|this, _: &WeekView, _, cx| this.set_mode(CalendarMode::Week, cx)))
            .on_action(cx.listener(|this, _: &MonthView, _, cx| this.set_mode(CalendarMode::Month, cx)))
            .on_action(cx.listener(|this, _: &AgendaView, _, cx| this.set_mode(CalendarMode::Agenda, cx)))
            .on_action(cx.listener(|this, _: &YearView, _, cx| this.set_mode(CalendarMode::Year, cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.options_dialog.take().is_some() {
                    cx.notify();
                }
            }))
            .size_full()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(calendar)
            .children(arrange)
            .children(self.options_dialog.clone())
            .children(self.render_context_menu(cx))
    }
}
