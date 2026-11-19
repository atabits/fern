//! The Settings window from the account menu, modeled on the original's: pages on the left,
//! grouped rows on the right, values picked from small dropdowns.

use std::rc::Rc;

use chrono::Datelike as _;
use fern_core::reminder::{ALL_DAY_OFFSETS, TIMED_OFFSETS};
use fern_core::{DURATIONS, DefaultDate, Priority, format_minutes, DateFormat, Preferences, SmartList, Store, Visibility, WEEK_STARTS};
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, FontWeight, ScrollHandle, point};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::actions::DismissSettings;
use crate::calendar;
use crate::i18n::{self, CurrentLocale, Locale};
use crate::release_notes::{self, ReleaseNotes, action_button};
use crate::subscriptions::{ChinaHolidaySource, ChinaHolidayStatus};
#[path = "appearance_view.rs"]
mod appearance_view;
#[path = "sticky_settings_view.rs"]
mod sticky_settings_view;

/// A row of a Settings page, and of the sections column, as the original's.
const ROW_HEIGHT: f32 = 40.;
const WINDOW_RADIUS: f32 = 12.;
const WINDOW_BORDER_WIDTH: f32 = 1.;
// GPUI clips overflow to a rectangle, so each panel rounds its own outer corners.
const PANEL_RADIUS: Pixels = px(WINDOW_RADIUS - WINDOW_BORDER_WIDTH);

type SelectPick = Box<dyn Fn(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>)>;
type SelectOption = (SharedString, bool, SelectPick);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    /// The original's Features: the optional sections on or off.
    Features,
    /// Daily notification, desktop reminders and constant reminders.
    Notifications,
    SmartList,
    #[default]
    DateTime,
    Appearance,
    More,
    /// The sync server's address and token.
    Sync,
    /// The original's Integrations: calendars subscribed to by address.
    Integrations,
    Sticky,
    About,
}

/// The open dropdown, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Visibility(SmartList),
    DefaultDate,
    DefaultDuration,
    TimedReminder,
    AllDayReminder,
    DefaultPriority,
    DefaultTag,
    AddTo,
    OverdueAt,
    TimeFormat,
    DateFormat,
    WeekStart,
    Language,
    DefaultList,
    DailyTime,
    ChinaHolidays(Option<fern_core::Id>),
}

pub enum SettingsEvent {
    Dismissed,
    /// The "Shortcuts" page, which is the Shortcuts panel.
    ShowShortcuts,
}

pub struct SettingsWindow {
    store: Rc<Store>,
    preferences: Preferences,
    page: Page,
    page_scroll: ScrollHandle,
    menu: Option<Menu>,
    focus_handle: FocusHandle,
    /// The outcome of the last backup or import.
    backup_message: Option<String>,
    installed_notes: Option<Rc<ReleaseNotes>>,
    release_notes: Option<Rc<ReleaseNotes>>,
    /// Settings → Sync: the server's address and the token, saved as they are typed.
    sync_server: Entity<crate::text_input::TextInput>,
    sync_token: Entity<crate::text_input::TextInput>,
    /// Integrations: the subscription being added (`Some(None)`) or changed, its color, and its
    /// name and address fields.
    subscription_form: Option<(Option<fern_core::Id>, Option<String>)>,
    subscription_name: Entity<crate::text_input::TextInput>,
    subscription_url: Entity<crate::text_input::TextInput>,
    _updates: Option<gpui::Subscription>,
    _feeds: Option<gpui::Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsWindow {}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Installs the theme and the first day of the week from `preferences`.
pub fn apply(preferences: &Preferences, window: &Window, cx: &mut App) {
    appearance_view::apply_theme(&preferences.theme, window, cx);
    calendar::set_week_start(preferences.week_start);
    calendar::set_hour12(preferences.hour12);
    calendar::set_date_format(preferences.date_format);
    calendar::set_week_numbers(preferences.week_numbers);
    cx.refresh_windows();
}

/// Re-applies only the active theme selection (used on live-reload and manual reload).
pub fn apply_theme_only(selection: &fern_core::ThemeSelection, window: &Window, cx: &mut App) {
    appearance_view::apply_theme(selection, window, cx);
}

impl SettingsWindow {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let preferences = store.preferences().unwrap_or_default();
        let config = store.sync_config().ok().flatten().unwrap_or_default();
        let field = |placeholder: &'static str, text: String, masked: bool, cx: &mut Context<Self>| {
            let input = cx.new(|cx| {
                let mut input = crate::text_input::TextInput::new(crate::text_input::Mode::SingleLine, placeholder, cx);
                if masked {
                    input = input.masked();
                }
                input.set_text(text, cx);
                input
            });
            cx.subscribe(&input, |this: &mut Self, _, _: &crate::text_input::Changed, cx| this.save_sync(cx)).detach();
            input
        };
        let sync_server = field("https://fern.example.org", config.server, false, cx);
        let sync_token = field("", config.token, true, cx);
        let updates = crate::updater::updater(cx).map(|updater| cx.observe(&updater, |_, _, cx| cx.notify()));
        let feeds = crate::subscriptions::feeds(cx).map(|feeds| cx.observe(&feeds, |_, _, cx| cx.notify()));
        let plain = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| crate::text_input::TextInput::new(crate::text_input::Mode::SingleLine, placeholder, cx))
        };
        let subscription_name = plain(i18n::t(cx).subscription_name_placeholder, cx);
        let subscription_url = plain("https://…/calendar.ics", cx);
        let installed_notes = release_notes::installed(&store).map(Rc::new);
        Self {
            store,
            preferences,
            page: Page::default(),
            page_scroll: ScrollHandle::new(),
            menu: None,
            focus_handle: cx.focus_handle(),
            backup_message: None,
            installed_notes,
            release_notes: None,
            sync_server,
            sync_token,
            subscription_form: None,
            subscription_name,
            subscription_url,
            _updates: updates,
            _feeds: feeds,
        }
    }

    fn save_sync(&mut self, cx: &mut Context<Self>) {
        let config = fern_core::sync::SyncConfig {
            server: self.sync_server.read(cx).text().to_owned(),
            token: self.sync_token.read(cx).text().to_owned(),
        };
        if let Err(err) = self.store.set_sync_config(&config) {
            eprintln!("failed to save the sync settings: {err:#}");
        }
    }

    pub fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page != page {
            self.page_scroll.set_offset(point(px(0.), px(0.)));
        }
        self.page = page;
        self.menu = None;
        self.release_notes = None;
        cx.notify();
    }

    pub fn show_release_notes(&mut self, notes: Rc<ReleaseNotes>, window: &mut Window, cx: &mut Context<Self>) {
        self.set_page(Page::About, cx);
        self.release_notes = Some(notes);
        window.focus(&self.focus_handle, cx);
    }

    fn back_to_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_page(Page::About, cx);
        window.focus(&self.focus_handle, cx);
    }

    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.menu, Some(Menu::ChinaHolidays(_))) {
            self.set_menu(None, cx);
        } else if self.release_notes.is_some() {
            self.back_to_about(window, cx);
        } else {
            cx.emit(SettingsEvent::Dismissed);
        }
    }

    fn set_menu(&mut self, menu: Option<Menu>, cx: &mut Context<Self>) {
        self.menu = menu;
        cx.notify();
    }

    fn update(&mut self, window: &mut Window, cx: &mut Context<Self>, change: impl FnOnce(&mut Preferences)) {
        self.menu = None;
        match self.store.update_preferences(change) {
            Ok(preferences) => {
                self.preferences = preferences;
                apply(&self.preferences, window, cx);
            }
            Err(err) => {
                eprintln!("failed to save the settings: {err:#}");
                cx.notify();
            }
        }
    }

    /// "Generate Backup": all data in one file, saved where the user chooses.
    fn generate_backup(&mut self, cx: &mut Context<Self>) {
        let contents = crate::scratch_dir().map_err(anyhow::Error::from).and_then(|dir| {
            let temp = dir.join(format!("fern-backup-{}.db", std::process::id()));
            let attachments = crate::attachments_dir().ok();
            let contents = self.store.backup_to(&temp, attachments.as_deref()).and_then(|()| Ok(std::fs::read(&temp)?));
            std::fs::remove_file(&temp).ok();
            contents
        });
        match contents {
            Ok(bytes) => {
                let name = format!("fern-backup-{}.db", chrono::Local::now().format("%Y-%m-%d"));
                crate::export::save_as(name, bytes, cx, |this, result, cx| {
                    this.backup_message = crate::export::outcome(result, i18n::t(cx));
                    cx.notify();
                });
            }
            Err(err) => self.backup_message = Some(err.to_string()),
        }
        cx.notify();
    }

    /// "Import Backups": a Fern backup is checked now and restored on the next start; a TickTick
    /// backup (its CSV file) is added to the tasks here right away.
    fn import_backup(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            let Some(path) = chosen.await.ok().and_then(Result::ok).flatten().and_then(|paths| paths.into_iter().next()) else {
                return;
            };
            if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("csv")) {
                let text = std::fs::read_to_string(&path);
                this.update(cx, |this, cx| {
                    let strings = i18n::t(cx);
                    let imported = text.map_err(anyhow::Error::from).and_then(|text| this.store.import_ticktick(&text));
                    this.backup_message = Some(match imported {
                        Ok(summary) => strings
                            .imported_from_ticktick
                            .replace("{tasks}", &summary.tasks.to_string())
                            .replace("{lists}", &summary.lists.to_string()),
                        Err(err) => format!("{err:#}"),
                    });
                    crate::sticky_note::data_changed(cx.entity_id(), cx);
                    cx.notify();
                })
                .ok();
                return;
            }
            let outcome = fern_core::Store::check_backup(&path)
                .and_then(|()| Ok(std::fs::copy(&path, crate::pending_restore_path()?)?))
                .map(|_| ());
            this.update(cx, |this, cx| {
                let strings = i18n::t(cx);
                this.backup_message = Some(match outcome {
                    Ok(()) => strings.restore_on_restart.to_owned(),
                    Err(err) => format!("{err}"),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_setting("locale", locale.code()) {
            eprintln!("failed to save the language: {err:#}");
        }
        cx.set_global(CurrentLocale(locale));
        self.menu = None;
        cx.refresh_windows();
    }

    /// Opens a dropdown from the debug-build dev state: `settings=<page>[>menu]`.
    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, spec: &str, cx: &mut Context<Self>) {
        let (page, menu) = spec.split_once('>').unwrap_or((spec, ""));
        // `about>import:<path>`: imports a TickTick backup as "Import Backups" does.
        if let Some(path) = menu.strip_prefix("import:") {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let strings = i18n::t(cx);
            self.backup_message = Some(match self.store.import_ticktick(&text) {
                Ok(summary) => strings
                    .imported_from_ticktick
                    .replace("{tasks}", &summary.tasks.to_string())
                    .replace("{lists}", &summary.lists.to_string()),
                Err(err) => format!("{err:#}"),
            });
            crate::sticky_note::data_changed(cx.entity_id(), cx);
        }
        let page = match page {
            "smart-list" => Page::SmartList,
            "appearance" => Page::Appearance,
            "more" => Page::More,
            "about" => Page::About,
            "sync" => Page::Sync,
            "integrations" => Page::Integrations,
            "sticky" => Page::Sticky,
            "features" => Page::Features,
            "notifications" => Page::Notifications,
            _ => Page::DateTime,
        };
        self.set_page(page, cx);
        self.menu = match menu {
            "week" => Some(Menu::WeekStart),
            "time" => Some(Menu::TimeFormat),
            "date" => Some(Menu::DateFormat),
            "today" => Some(Menu::Visibility(SmartList::Today)),
            "tags" => Some(Menu::Visibility(SmartList::Tags)),
            "language" => Some(Menu::Language),
            "list" => Some(Menu::DefaultList),
            "duration" => Some(Menu::DefaultDuration),
            "add-to" => Some(Menu::AddTo),
            "overdue" => Some(Menu::OverdueAt),
            "reminder" => Some(Menu::TimedReminder),
            _ => None,
        };
        cx.notify();
    }

    /// Task Default's first group: date, duration and reminders of new tasks.
    fn render_task_defaults(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let defaults = self.preferences.task_defaults.clone();
        let pick = |change: Box<dyn Fn(&mut fern_core::TaskDefaults)>| -> SelectPick {
            Box::new(move |this, window, cx| this.update(window, cx, |p| change(&mut p.task_defaults)))
        };
        let date_name = |date: Option<DefaultDate>| match date {
            None => strings.none_option,
            Some(date) => strings.default_dates[DefaultDate::ALL.iter().position(|d| *d == date).unwrap_or(0)],
        };
        let dates = std::iter::once(None)
            .chain(DefaultDate::ALL.into_iter().map(Some))
            .map(|date| (SharedString::from(date_name(date)), date == defaults.date, pick(Box::new(move |t| t.date = date))))
            .collect();
        let duration_name = |minutes: i64| {
            DURATIONS.iter().position(|d| *d == minutes).map_or_else(|| format_minutes(minutes), |i| strings.durations[i].to_owned())
        };
        let durations = DURATIONS
            .into_iter()
            .map(|minutes| (SharedString::from(duration_name(minutes)), minutes == defaults.duration, pick(Box::new(move |t| t.duration = minutes))))
            .collect();
        let reminder_options = |offsets: [i64; 5], names: [&'static str; 5], current: Option<i64>, set: fn(&mut fern_core::TaskDefaults, Option<i64>)| {
            std::iter::once((None, strings.none_option))
                .chain(offsets.into_iter().zip(names).map(|(o, n)| (Some(o), n)))
                .map(|(offset, name)| (SharedString::from(name), offset == current, pick(Box::new(move |t| set(t, offset)))))
                .collect::<Vec<_>>()
        };
        let reminder_name = |offsets: [i64; 5], names: [&'static str; 5], current: Option<i64>| {
            current.and_then(|o| offsets.iter().position(|x| *x == o)).map_or(strings.none_option, |i| names[i])
        };
        v_flex()
            .py_1p5()
            .rounded(px(8.))
            .bg(cx.theme().colors().surface_background)
            .child(self.select_row(None, strings.default_date, date_name(defaults.date).into(), Menu::DefaultDate, dates, cx))
            .child(self.select_row(None, strings.default_duration, duration_name(defaults.duration).into(), Menu::DefaultDuration, durations, cx))
            .child(self.select_row(
                None,
                strings.default_timed_reminder,
                reminder_name(TIMED_OFFSETS, strings.reminders_timed, defaults.timed_reminder).into(),
                Menu::TimedReminder,
                reminder_options(TIMED_OFFSETS, strings.reminders_timed, defaults.timed_reminder, |t, o| t.timed_reminder = o),
                cx,
            ))
            .child(self.select_row(
                None,
                strings.default_all_day_reminder,
                reminder_name(ALL_DAY_OFFSETS, strings.reminders_all_day, defaults.all_day_reminder).into(),
                Menu::AllDayReminder,
                reminder_options(ALL_DAY_OFFSETS, strings.reminders_all_day, defaults.all_day_reminder, |t, o| t.all_day_reminder = o),
                cx,
            ))
    }

    /// Task Default's priority and tag rows, above the default list.
    fn render_priority_and_tag(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let defaults = self.preferences.task_defaults.clone();
        let priorities = [
            (Priority::None, strings.priority_none),
            (Priority::Low, strings.priority_low),
            (Priority::Medium, strings.priority_medium),
            (Priority::High, strings.priority_high),
        ];
        let priority_name = priorities.iter().find(|(p, _)| *p == defaults.priority).map_or(strings.priority_none, |(_, n)| n);
        let priority_options = priorities
            .into_iter()
            .map(|(priority, name)| {
                let pick: SelectPick =
                    Box::new(move |this, window, cx| this.update(window, cx, |p| p.task_defaults.priority = priority));
                (SharedString::from(name), priority == defaults.priority, pick)
            })
            .collect();
        let tags = self.store.tags().unwrap_or_default();
        let tag_options = std::iter::once(None)
            .chain(tags.into_iter().map(|t| Some(t.name)))
            .map(|tag| {
                let selected = tag == defaults.tag;
                let label = SharedString::from(tag.clone().unwrap_or_else(|| strings.none_option.to_owned()));
                let pick: SelectPick =
                    Box::new(move |this, window, cx| this.update(window, cx, |p| p.task_defaults.tag = tag.clone()));
                (label, selected, pick)
            })
            .collect();
        vec![
            self.select_row(None, strings.default_priority, (*priority_name).into(), Menu::DefaultPriority, priority_options, cx),
            self.select_row(
                None,
                strings.default_tag,
                defaults.tag.clone().unwrap_or_else(|| strings.none_option.to_owned()).into(),
                Menu::DefaultTag,
                tag_options,
                cx,
            ),
        ]
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let item = |id: &'static str, icon: IconName, label: &'static str, selected: bool| {
            ListItem::new(id)
                .height(px(37.))
                .toggle_state(selected)
                .start_slot(Icon::new(icon).color(Color::Icon))
                .child(Label::new(label).truncate())
        };
        let page = |id, icon, index: usize, target: Page| {
            item(id, icon, strings.settings_pages[index], self.page == target)
                .on_click(cx.listener(move |this, _, _, cx| this.set_page(target, cx)))
        };
        v_flex()
            .id("settings-navigation")
            .flex_none()
            .w(px(240.))
            .h_full()
            .overflow_y_scroll()
            .p_2()
            .gap_0p5()
            .rounded_tl(PANEL_RADIUS)
            .rounded_bl(PANEL_RADIUS)
            .bg(cx.theme().colors().panel_background)
            .border_r_1()
            .border_color(cx.theme().colors().border)
            .child(div().px_2().pt_2().pb_3().child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(strings.settings)))
            .child(
                item("settings-features", IconName::LayoutGrid, strings.features, self.page == Page::Features)
                    .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Features, cx))),
            )
            .child(page("settings-smart-list", IconName::Funnel, 0, Page::SmartList))
            .child(
                item("settings-notifications", IconName::Bell, strings.notifications_page, self.page == Page::Notifications)
                    .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Notifications, cx))),
            )
            .child(page("settings-date-time", IconName::Clock, 1, Page::DateTime))
            .child(page("settings-appearance", IconName::PaintRoller, 2, Page::Appearance))
            .child(page("settings-more", IconName::ToggleRight, 3, Page::More))
            .child(
                item("settings-sticky", IconName::FileText, strings.sticky_notes, self.page == Page::Sticky)
                    .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Sticky, cx))),
            )
            .child(
                item("settings-sync", IconName::RefreshCw, strings.data_and_sync, self.page == Page::Sync)
                    .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Sync, cx))),
            )
            .child(
                item("settings-integrations", IconName::CalendarDays, strings.integrations, self.page == Page::Integrations)
                    .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Integrations, cx))),
            )
            .child(div().h(px(12.)))
            .child(
                item("settings-shortcuts", IconName::Keyboard, strings.settings_pages[4], false)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::ShowShortcuts))),
            )
            .child(div().h(px(12.)))
            .child(page("settings-about", IconName::Info, 5, Page::About))
    }

    /// A row whose value opens a dropdown of `options` (label, selected, what picking it does).
    fn select_row(
        &self,
        icon: Option<IconName>,
        label: &'static str,
        value: SharedString,
        menu: Menu,
        options: Vec<SelectOption>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.menu == Some(menu);
        let dropdown = open.then(|| {
            let options = Rc::new(options);
            let items = (0..options.len()).map(|i| {
                let (label, selected, _) = &options[i];
                let options = options.clone();
                ListItem::new(SharedString::from(format!("option-{i}")))
                    .child(Label::new(label.clone()))
                    .when(*selected, |item| item.end_slot(Icon::new(IconName::Check).color(Color::Accent).size(IconSize::Small)))
                    .on_click(cx.listener(move |this, _, window, cx| (options[i].2)(this, window, cx)))
            });
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .min_w(px(180.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_menu(None, cx)))
                    .children(items),
            )
        });
        h_flex()
            .debug_selector(|| format!("settings-row-{menu:?}"))
            .h(px(ROW_HEIGHT))
            .px_4()
            .gap_2()
            .children(icon.map(|icon| Icon::new(icon).color(Color::Muted)))
            .child(div().flex_1().child(Label::new(label)))
            .child(
                h_flex()
                    .id(SharedString::from(format!("settings-value-{label}")))
                    .relative()
                    .gap_1()
                    .cursor_pointer()
                    .child(Label::new(value).color(Color::Muted))
                    .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_menu((!open).then_some(menu), cx)))
                    .children(dropdown),
            )
            .into_any_element()
    }

    /// About → Updates: the automatic checks, how the last one went and what to do next.
    fn render_updates(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::updater::Status;
        let updater = crate::updater::updater(cx).filter(|updater| updater.read(cx).supported())?;
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let (status, automatic) = {
            let updater = updater.read(cx);
            (updater.status().clone(), updater.automatic())
        };
        let version = |text: &str, version: &str| text.replace("{version}", version);
        let (message, action) = match &status {
            Status::Idle => (None, Some(strings.check_for_updates.to_owned())),
            Status::Checking => (Some(strings.update_checking.to_owned()), None),
            Status::UpToDate => (Some(strings.update_up_to_date.to_owned()), Some(strings.check_for_updates.to_owned())),
            Status::Downloading(v) => (Some(version(strings.update_downloading, v)), None),
            Status::Ready(notes) => (Some(version(strings.update_ready, &notes.version)), Some(strings.restart_to_update.to_owned())),
            Status::Available(notes) => (Some(version(strings.update_available, &notes.version)), Some(version(strings.update_to, &notes.version))),
            Status::Failed(error) => (Some(strings.update_failed.replace("{error}", error)), Some(strings.check_for_updates.to_owned())),
        };
        let notes = match status {
            Status::Ready(notes) | Status::Available(notes) => Some(notes),
            _ => None,
        };
        let install = notes.is_some();
        let switch_updater = updater.clone();
        Some(
            v_flex()
                .child(div().pb_3().child(Label::new(strings.updates).weight(FontWeight::MEDIUM)))
                .child(
                    v_flex()
                        .py_1p5()
                        .rounded(px(8.))
                        .bg(colors.surface_background)
                        .child(
                            h_flex()
                                .id("auto-update")
                                .h(px(ROW_HEIGHT))
                                .px_4()
                                .cursor_pointer()
                                .child(div().flex_1().child(Label::new(strings.check_updates_automatically)))
                                .child(Switch::new().toggle_state(automatic))
                                .on_click(move |_, _, cx| switch_updater.update(cx, |updater, cx| updater.set_automatic(!automatic, cx))),
                        )
                        .child(
                            h_flex()
                                .min_h(px(ROW_HEIGHT))
                                .py_2()
                                .px_4()
                                .gap_x_3()
                                .gap_y_1()
                                .flex_wrap()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(110.))
                                        .text_size(px(13.))
                                        .text_color(if install { colors.text } else { colors.text_muted })
                                        .child(message.unwrap_or_else(|| format!("Fern {}", env!("CARGO_PKG_VERSION")))),
                                )
                                .child(h_flex().gap_2().flex_none().flex_wrap()
                                    .children(notes.map(|notes| {
                                        action_button("update-notes", cx)
                                            .debug_selector(|| "update-notes".to_owned())
                                            .border_color(colors.border)
                                            .child(Label::new(strings.whats_new).size(LabelSize::Small))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.show_release_notes(notes.clone(), window, cx);
                                            }))
                                    }))
                                    .children(action.map(|action| {
                                        action_button("update-action", cx)
                                            .debug_selector(|| "update-action".to_owned())
                                            .when(install, |el| el.bg(colors.accent.opacity(0.12)).border_color(colors.accent.opacity(0.35)))
                                            .child(Label::new(action).size(LabelSize::Small).weight(FontWeight::MEDIUM))
                                            .on_click(move |_, _, cx| {
                                                updater.update(cx, |updater, cx| {
                                                    if install {
                                                        updater.restart_to_update(cx)
                                                    } else {
                                                        updater.check(cx)
                                                    }
                                                });
                                            })
                                    })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Opens the subscription form, empty for a new calendar or filled from `id`'s.
    fn open_subscription_form(&mut self, id: Option<fern_core::Id>, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let current = id.and_then(|id| {
            let feeds = crate::subscriptions::feeds(cx)?;
            let feeds = feeds.read(cx);
            feeds.calendars().iter().find(|c| c.subscription.id == id).map(|c| c.subscription.clone())
        });
        let (name, url, color) = current.map_or((String::new(), String::new(), None), |s| (s.name, s.url, s.color));
        self.subscription_name.update(cx, |input, cx| input.set_text(name, cx));
        self.subscription_url.update(cx, |input, cx| input.set_text(url, cx));
        self.subscription_form = Some((id, color));
        window.focus(&self.subscription_url.focus_handle(cx), cx);
        cx.notify();
    }

    fn save_subscription_form(&mut self, cx: &mut Context<Self>) {
        let Some((id, color)) = self.subscription_form.take() else { return };
        let name = self.subscription_name.read(cx).text().to_owned();
        let url = self.subscription_url.read(cx).text().to_owned();
        if let Some(feeds) = crate::subscriptions::feeds(cx)
            && !url.trim().is_empty()
        {
            feeds.update(cx, |feeds, cx| match id {
                Some(id) => {
                    let hidden = feeds.calendars().iter().find(|c| c.subscription.id == id).is_some_and(|c| c.subscription.hidden);
                    feeds.update_subscription(id, &name, &url, color.as_deref(), hidden, cx);
                }
                None => feeds.add(&name, &url, color.as_deref(), cx),
            });
        }
        cx.notify();
    }

    fn render_china_holiday_source(
        &self,
        current: Option<(fern_core::Id, ChinaHolidaySource)>,
        enabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let hover = cx.theme().colors().ghost_element_hover;
        let subscription = current.map(|(id, _)| id);
        let selected = current.map(|(_, source)| source);
        let menu = Menu::ChinaHolidays(subscription);
        let open = enabled && self.menu == Some(menu);
        let dropdown = open.then(|| {
            PopoverAttachment::new(
                Placement::BelowLeft,
                popover(cx)
                    .min_w(px(156.))
                    .debug_selector(|| "china-holiday-source-menu".to_owned())
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_menu(None, cx)))
                    .children(ChinaHolidaySource::ALL.map(|source| {
                        h_flex()
                            .id(source.label())
                            .debug_selector(move || source.label().to_owned())
                            .tab_index(0)
                            .h(px(32.))
                            .gap_2()
                            .px_2()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|style| style.bg(hover))
                            .focus_visible(|style| style.bg(hover))
                            .child(div().flex_1().child(Label::new(source.label())))
                            .when(selected == Some(source), |el| {
                                el.child(Icon::new(IconName::Check).color(Color::Accent).size(IconSize::Small))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_menu(None, cx);
                                cx.stop_propagation();
                                if selected != Some(source)
                                    && let Some(feeds) = crate::subscriptions::feeds(cx)
                                {
                                    feeds.update(cx, |feeds, cx| {
                                        feeds.select_china_holiday_source(subscription, source, strings.holidays_name, cx);
                                    });
                                }
                            }))
                    })),
            )
        });
        let color = if enabled { Color::Accent } else { Color::Muted };
        h_flex()
            .id(match subscription {
                Some(id) => SharedString::from(format!("china-holiday-source-{id}")),
                None => SharedString::from("subscription-holidays"),
            })
            .debug_selector(|| "china-holiday-source".to_owned())
            .relative()
            .flex_none()
            .gap_1()
            .child(
                Label::new(selected.map_or(strings.add_holidays, ChinaHolidaySource::label))
                    .color(color)
                    .when(selected.is_some(), |label| label.size(LabelSize::Small)),
            )
            .when(enabled, |el| {
                el.cursor_pointer()
                    .tab_index(0)
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(color))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_menu((!open).then_some(menu), cx)))
                    .when(selected.is_some(), |el| el.tooltip(Tooltip::text(strings.subscription_change_source)))
            })
            .children(dropdown)
            .into_any_element()
    }

    /// Integrations, as the original's URL calendars: each subscribed calendar with its color,
    /// whether it is read and when, show or hide, change and unsubscribe; and the form that adds
    /// or changes one (name, address, color).
    fn render_integrations(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let Some(feeds) = crate::subscriptions::feeds(cx) else { return Vec::new() };
        let feeds = feeds.read(cx);
        let chinese = i18n::locale(cx) == Locale::ZhCn;
        let china_status = feeds.china_holiday_status();
        let china_sources = ChinaHolidaySource::ALL.map(|source| {
            feeds.calendars().iter().any(|calendar| {
                ChinaHolidaySource::from_url(&calendar.subscription.url) == Some(source)
            })
        });
        let has_china_holidays = china_sources.into_iter().any(|installed| installed);
        let can_choose_source = china_status != ChinaHolidayStatus::Reading && !china_sources.into_iter().all(|installed| installed);
        let rows: Vec<AnyElement> = feeds
            .calendars()
            .iter()
            .map(|calendar| {
                let subscription = &calendar.subscription;
                let id = subscription.id;
                let hidden = subscription.hidden;
                let china_source = ChinaHolidaySource::from_url(&subscription.url);
                let status = if feeds.is_reading(id) {
                    (strings.subscription_reading.to_owned(), Color::Muted)
                } else if let Some(error) = &subscription.error {
                    (error.clone(), Color::Error)
                } else {
                    (strings.subscription_events.replace("{n}", &calendar.events.len().to_string()), Color::Muted)
                };
                let dot = crate::lists_panel::parse_color(subscription.color.as_deref()).unwrap_or(colors.accent);
                h_flex()
                    .id(SharedString::from(format!("subscription-{id}")))
                    .when_some(subscription.error.as_ref(), |el, error| el.tooltip(Tooltip::text(error.clone())))
                    .min_h(px(ROW_HEIGHT))
                    .px_4()
                    .gap_3()
                    .child(div().size(px(10.)).flex_none().rounded_full().bg(dot))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(Label::new(subscription.title().to_owned()).truncate().color(if hidden { Color::Muted } else { Color::Default }))
                            .child(Label::new(status.0).size(LabelSize::XSmall).color(status.1).truncate()),
                    )
                    .children(china_source.map(|source| self.render_china_holiday_source(Some((id, source)), can_choose_source, cx)))
                    .child(
                        IconButton::new(SharedString::from(format!("subscription-show-{id}")), if hidden { IconName::EyeOff } else { IconName::Eye })
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(if hidden { strings.subscription_show } else { strings.subscription_hide }))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(feeds) = crate::subscriptions::feeds(cx) {
                                    feeds.update(cx, |feeds, cx| {
                                        if let Some(s) = feeds.calendars().iter().find(|c| c.subscription.id == id).map(|c| c.subscription.clone()) {
                                            feeds.update_subscription(id, &s.name, &s.url, s.color.as_deref(), !hidden, cx);
                                        }
                                    });
                                }
                            })),
                    )
                    .child(
                        IconButton::new(SharedString::from(format!("subscription-read-{id}")), IconName::RefreshCw)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.subscription_read_now))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(feeds) = crate::subscriptions::feeds(cx) {
                                    feeds.update(cx, |feeds, cx| feeds.read(id, cx));
                                }
                            })),
                    )
                    .child(
                        IconButton::new(SharedString::from(format!("subscription-edit-{id}")), IconName::Pencil)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.subscription_edit))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_subscription_form(Some(id), window, cx))),
                    )
                    .child(
                        IconButton::new(SharedString::from(format!("subscription-delete-{id}")), IconName::Trash)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.subscription_delete))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(feeds) = crate::subscriptions::feeds(cx) {
                                    feeds.update(cx, |feeds, cx| feeds.delete(id, cx));
                                }
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let group = || v_flex().py_1p5().rounded(px(8.)).bg(colors.surface_background);
        let field = |label: &'static str, input: Entity<crate::text_input::TextInput>| {
            h_flex()
                .h(px(40.))
                .px_4()
                .gap_4()
                .child(div().w(px(80.)).child(Label::new(label)))
                .child(div().flex_1().min_w_0().overflow_hidden().h(px(28.)).px_2().flex().items_center().rounded(px(6.)).bg(colors.background).child(input))
        };
        let form = self.subscription_form.as_ref().map(|(_, color)| {
            let swatches = h_flex().gap_1p5().children(std::iter::once(None).chain(crate::lists_panel::LIST_COLORS.iter().map(|c| Some(*c))).map(|swatch| {
                let selected = color.as_deref() == swatch;
                let fill = crate::lists_panel::parse_color(swatch).unwrap_or(colors.accent);
                div()
                    .id(SharedString::from(format!("subscription-color-{}", swatch.unwrap_or("none"))))
                    .size(px(20.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(fill.opacity(if swatch.is_none() { 0.35 } else { 1. }))
                    .when(selected, |el| el.border_2().border_color(colors.text))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some((_, color)) = &mut this.subscription_form {
                            *color = swatch.map(str::to_owned);
                        }
                        cx.notify();
                    }))
            }));
            group()
                .child(field(strings.subscription_name, self.subscription_name.clone()))
                .child(field("URL", self.subscription_url.clone()))
                .child(h_flex().h(px(40.)).px_4().gap_4().child(div().w(px(80.)).child(Label::new(strings.color))).child(swatches))
                .child(
                    h_flex()
                        .px_4()
                        .py_2()
                        .gap_2()
                        .justify_end()
                        .child(form_button("subscription-cancel", strings.cancel, false, &colors).on_click(cx.listener(|this, _, _, cx| {
                            this.subscription_form = None;
                            cx.notify();
                        })))
                        .child(form_button("subscription-save", strings.ok, true, &colors).on_click(cx.listener(|this, _, _, cx| this.save_subscription_form(cx)))),
                )
                .into_any_element()
        });
        let mut page = vec![
            div().pb_3().child(Label::new(strings.subscribed_calendars).weight(FontWeight::MEDIUM)).into_any_element(),
            div().pb_2().px_1().text_size(px(12.)).text_color(colors.text_muted).child(strings.subscriptions_hint).into_any_element(),
        ];
        if !rows.is_empty() {
            page.push(group().children(rows).into_any_element());
        }
        if chinese || has_china_holidays {
            let message = match china_status {
                ChinaHolidayStatus::Idle => None,
                ChinaHolidayStatus::Reading => Some((strings.subscription_reading, Color::Muted)),
                ChinaHolidayStatus::Failed => Some((strings.holiday_source_failed, Color::Error)),
            };
            if let Some((text, color)) = message {
                page.push(div().px_1().child(Label::new(text).size(LabelSize::Small).color(color)).into_any_element());
            }
        }
        match form {
            Some(form) => page.push(form),
            None => {
                // The original's Holidays: the public holidays of the language's country, a
                // calendar like any other once added.
                let has_holidays = if chinese {
                    has_china_holidays
                } else {
                    feeds.calendars().iter().any(|c| c.subscription.url == strings.holidays_url)
                };
                page.push(
                    h_flex()
                        .px_1()
                        .gap_5()
                        .child(
                            div()
                                .id("subscription-add")
                                .cursor_pointer()
                                .child(Label::new(strings.add_calendar).color(Color::Accent))
                                .on_click(cx.listener(|this, _, window, cx| this.open_subscription_form(None, window, cx))),
                        )
                        .when(!has_holidays, |el| {
                            if chinese {
                                return el.child(self.render_china_holiday_source(None, can_choose_source, cx));
                            }
                            el.child(
                                div()
                                    .id("subscription-holidays")
                                    .cursor_pointer()
                                    .child(Label::new(strings.add_holidays).color(Color::Accent))
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        let strings = i18n::t(cx);
                                        if let Some(feeds) = crate::subscriptions::feeds(cx) {
                                            feeds.update(cx, |feeds, cx| feeds.add(strings.holidays_name, strings.holidays_url, Some("#22C55E"), cx));
                                        }
                                    })),
                            )
                        })
                        .into_any_element(),
                )
            }
        }
        page
    }

    fn render_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let group = || v_flex().py_1p5().rounded(px(8.)).bg(colors.surface_background);
        let title = |text: &'static str| div().pb_3().child(Label::new(text).weight(FontWeight::MEDIUM));
        let body = v_flex()
            .id("settings-page")
            .debug_selector(|| "settings-page".to_owned())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.page_scroll)
            .pt_1()
            .px_6()
            .pb_6()
            .gap_4();
        match self.page {
            Page::SmartList => {
                let row = |this: &Self, list: SmartList, cx: &mut Context<Self>| {
                    let (icon, label) = smart_list_label(list, strings);
                    let current = this.preferences.visibility(list);
                    let options = list
                        .options()
                        .iter()
                        .map(|&visibility| {
                            let pick: SelectPick = Box::new(
                                move |this, window, cx| this.update(window, cx, |p| p.set_visibility(list, visibility)),
                            );
                            (visibility_label(list, visibility, strings).into(), visibility == current, pick)
                        })
                        .collect();
                    this.select_row(
                        Some(icon),
                        label,
                        visibility_label(list, current, strings).into(),
                        Menu::Visibility(list),
                        options,
                        cx,
                    )
                };
                let [all, today, tomorrow, next7, inbox, summary, tags, filters, completed, wont_do, trash] = SmartList::ALL;
                body.child(title(strings.settings_pages[0]))
                    .child(group().children([all, today, tomorrow, next7, inbox, summary].map(|l| row(self, l, cx))))
                    .child(group().children([tags, filters].map(|l| row(self, l, cx))))
                    .child(group().children([completed, wont_do, trash].map(|l| row(self, l, cx))))
                    .into_any_element()
            }
            Page::DateTime => {
                let week_start = self.preferences.week_start;
                let options = WEEK_STARTS
                    .into_iter()
                    .zip(strings.week_start_names)
                    .map(|(day, name)| {
                        let pick: SelectPick =
                            Box::new(move |this, window, cx| this.update(window, cx, |p| p.week_start = day));
                        (SharedString::from(name), day == week_start, pick)
                    })
                    .collect();
                let current = WEEK_STARTS.iter().position(|d| *d == week_start).unwrap_or(0);
                let hour12 = self.preferences.hour12;
                let week_numbers = self.preferences.week_numbers;
                // The original shows each format on the year's last day.
                let sample = chrono::NaiveDate::from_ymd_opt(chrono::Local::now().year(), 12, 31).expect("December 31 exists");
                let current_format = self.preferences.date_format;
                let date_formats = DateFormat::ALL
                    .into_iter()
                    .map(|format| {
                        let pick: SelectPick =
                            Box::new(move |this, window, cx| this.update(window, cx, |p| p.date_format = Some(format)));
                        let label = calendar::format_date_as(sample, Some(format), false, true);
                        let selected = current_format == Some(format)
                            || (current_format.is_none() && label == calendar::format_date(sample, strings));
                        (SharedString::from(label), selected, pick)
                    })
                    .collect();
                let formats = [false, true]
                    .into_iter()
                    .map(|twelve| {
                        let pick: SelectPick =
                            Box::new(move |this, window, cx| this.update(window, cx, |p| p.hour12 = twelve));
                        (SharedString::from(strings.time_formats[twelve as usize]), twelve == hour12, pick)
                    })
                    .collect();
                body.child(
                    v_flex()
                        .child(title(strings.settings_pages[1]))
                        .child(group().child(self.select_row(
                            None,
                            strings.time_format,
                            strings.time_formats[hour12 as usize].into(),
                            Menu::TimeFormat,
                            formats,
                            cx,
                        ))
                        .child(self.select_row(
                            None,
                            strings.date_format,
                            calendar::format_date(sample, strings).into(),
                            Menu::DateFormat,
                            date_formats,
                            cx,
                        ))
                        .child(self.select_row(
                            None,
                            strings.start_week_on,
                            strings.week_start_names[current].into(),
                            Menu::WeekStart,
                            options,
                            cx,
                        ))),
                )
                .child(
                    group().child(
                        h_flex()
                            .id("week-numbers")
                            .h(px(ROW_HEIGHT))
                            .px_4()
                            .cursor_pointer()
                            .child(div().flex_1().child(Label::new(strings.show_week_numbers)))
                            .child(Switch::new().toggle_state(week_numbers))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.update(window, cx, |p| p.week_numbers = !week_numbers)
                            })),
                    ),
                )
                .into_any_element()
            }
            Page::Appearance => body.child(self.render_appearance(cx)).into_any_element(),
            Page::More => {
                let current = i18n::locale(cx);
                let languages = Locale::available()
                    .map(|locale| {
                        let pick: SelectPick =
                            Box::new(move |this, _, cx| this.set_locale(locale, cx));
                        (SharedString::from(locale.native_name()), locale == current, pick)
                    })
                    .collect();
                let lists = self.store.lists().unwrap_or_default();
                let default_list = self.preferences.default_list;
                let list_name = default_list
                    .and_then(|id| lists.iter().find(|l| l.id == id))
                    .map(|l| SharedString::from(l.name.clone()))
                    .unwrap_or(strings.inbox.into());
                let mut list_options: Vec<SelectOption> =
                    vec![(
                        strings.inbox.into(),
                        default_list.is_none(),
                        Box::new(|this, window, cx| this.update(window, cx, |p| p.default_list = None)),
                    )];
                list_options.extend(lists.iter().filter(|l| !l.archived).map(|list| {
                    let id = list.id;
                    let pick: SelectPick =
                        Box::new(move |this, window, cx| this.update(window, cx, |p| p.default_list = Some(id)));
                    (SharedString::from(list.name.clone()), default_list == Some(id), pick)
                }));
                let position_row = |label, menu, bottom, set: fn(&mut fern_core::TaskDefaults, bool), cx: &mut Context<Self>| {
                    let top_label = strings.add_to_top;
                    let bottom_label = strings.add_to_bottom;
                    let options = [(false, top_label), (true, bottom_label)]
                        .into_iter()
                        .map(|(choice, name)| {
                            let pick: SelectPick =
                                Box::new(move |this, window, cx| this.update(window, cx, |p| set(&mut p.task_defaults, choice)));
                            (name.into(), choice == bottom, pick)
                        })
                        .collect();
                    let value = if bottom { bottom_label } else { top_label };
                    self.select_row(None, label, value.into(), menu, options, cx)
                };
                let defaults = &self.preferences.task_defaults;
                body.child(group().debug_selector(|| "settings-language".to_owned()).child(self.select_row(
                    None,
                    strings.language,
                    current.native_name().into(),
                    Menu::Language,
                    languages,
                    cx,
                )))
                .child({
                    let r = self.preferences.recognition;
                    type RecognitionCheck = (&'static str, bool, fn(&mut fern_core::Recognition));
                    // A switch row with a hint, and the checkbox under it while the switch is on.
                    let section = |id: &'static str,
                                   label: &'static str,
                                   hint: &'static str,
                                   on: bool,
                                   toggle: fn(&mut fern_core::Recognition),
                                   check: Option<RecognitionCheck>,
                                   cx: &mut Context<Self>| {
                        v_flex()
                            .px_4()
                            .py_1p5()
                            .gap_1()
                            .child(
                                h_flex()
                                    .id(id)
                                    .cursor_pointer()
                                    .child(div().flex_1().child(Label::new(label)))
                                    .child(Switch::new().toggle_state(on))
                                    .on_click(cx.listener(move |this, _, window, cx| this.update(window, cx, |p| toggle(&mut p.recognition)))),
                            )
                            .child(div().text_size(px(11.)).text_color(colors.text_muted).child(hint))
                            .when_some(check.filter(|_| on), |el, (text, checked, flip)| {
                                el.child(
                                    h_flex()
                                        .id(SharedString::from(format!("{id}-remove")))
                                        .gap_2()
                                        .pt_0p5()
                                        .cursor_pointer()
                                        .child(Checkbox::new(SharedString::from(format!("{id}-check"))).accent().toggle_state(checked))
                                        .child(Label::new(text).size(LabelSize::Small))
                                        .on_click(cx.listener(move |this, _, window, cx| this.update(window, cx, |p| flip(&mut p.recognition)))),
                                )
                            })
                    };
                    v_flex().child(title(strings.smart_recognition)).child(
                        group()
                            .child(section(
                                "recognize-dates",
                                strings.date_recognition,
                                strings.date_recognition_hint,
                                r.dates,
                                |r| r.dates = !r.dates,
                                Some((strings.remove_date_text, r.remove_date_text, |r| r.remove_date_text = !r.remove_date_text)),
                                cx,
                            ))
                            .child(section(
                                "recognize-tags",
                                strings.tag_recognition,
                                strings.tag_recognition_hint,
                                r.tags,
                                |r| r.tags = !r.tags,
                                Some((strings.remove_tag_text, r.remove_tag_text, |r| r.remove_tag_text = !r.remove_tag_text)),
                                cx,
                            )),
                    )
                })
                .child(
                    v_flex()
                        .child(title(strings.task_default))
                        .child(self.render_task_defaults(cx))
                        .child(div().h_3())
                        .child(group().children(self.render_priority_and_tag(cx)).child(self.select_row(
                            None,
                            strings.default_list,
                            list_name,
                            Menu::DefaultList,
                            list_options,
                            cx,
                        )))
                        .child(
                            div()
                                .w_full()
                                .pt_1p5()
                                .px_1()
                                .text_size(px(11.))
                                .text_color(colors.text_muted)
                                .child(strings.default_list_hint),
                        )
                        .child(div().h_3())
                        .child(group().children([
                            position_row(
                                strings.default_add_to,
                                Menu::AddTo,
                                defaults.add_to_bottom,
                                |defaults, bottom| defaults.add_to_bottom = bottom,
                                cx,
                            ),
                            position_row(
                                strings.overdue_section_at,
                                Menu::OverdueAt,
                                defaults.overdue_last,
                                |defaults, bottom| defaults.overdue_last = bottom,
                                cx,
                            ),
                        ])),
                )
                .child({
                    let on = self.preferences.mini_calendar;
                    v_flex().child(title(strings.mini_calendar)).child(
                        group().child(
                            h_flex()
                                .id("mini-calendar")
                                .h(px(ROW_HEIGHT))
                                .px_4()
                                .cursor_pointer()
                                .child(div().flex_1().child(Label::new(strings.show_mini_calendar)))
                                .child(Switch::new().toggle_state(on))
                                .on_click(cx.listener(move |this, _, window, cx| this.update(window, cx, |p| p.mini_calendar = !on))),
                        ),
                    )
                })
                .into_any_element()
            }
            Page::Notifications => {
                let n = self.preferences.notifications;
                let values = [n.daily.is_some(), n.desktop, n.constant];
                let toggles: [fn(&mut fern_core::Notifications); 3] = [
                    |n| n.daily = if n.daily.is_some() { None } else { chrono::NaiveTime::from_hms_opt(9, 0, 0) },
                    |n| n.desktop = !n.desktop,
                    |n| n.constant = !n.constant,
                ];
                let times: Vec<SelectOption> = (5..=23u32)
                    .filter_map(|h| chrono::NaiveTime::from_hms_opt(h, 0, 0))
                    .map(|time| {
                        let option: SelectOption = (
                            calendar::format_time(time, strings).into(),
                            n.daily == Some(time),
                            Box::new(move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                                this.update(window, cx, |p| p.notifications.daily = Some(time))
                            }),
                        );
                        option
                    })
                    .collect();
                let mut daily_time = n.daily.map(|t| self.select_row(None, strings.daily_time, calendar::format_time(t, strings).into(), Menu::DailyTime, times, cx));
                let body = body.child(title(strings.notifications_page)).child(group().children(
                    strings.notification_items.into_iter().zip(values).zip(toggles).enumerate().map(|(i, (((name, hint), on), toggle))| {
                        v_flex()
                            .px_4()
                            .py_1p5()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .id(SharedString::from(format!("notification-{i}")))
                                    .cursor_pointer()
                                    .child(div().flex_1().child(Label::new(name)))
                                    .child(Switch::new().toggle_state(on))
                                    .on_click(cx.listener(move |this, _, window, cx| this.update(window, cx, |p| toggle(&mut p.notifications)))),
                            )
                            .child(div().text_size(px(11.)).text_color(colors.text_muted).child(hint))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .enumerate()
                    .flat_map(|(i, row)| {
                        let extra = if i == 0 { daily_time.take() } else { None };
                        std::iter::once(row).chain(extra)
                    })
                    .collect::<Vec<_>>(),
                ));
                #[cfg(windows)]
                let body = {
                    let enabled = crate::tray::enabled(&self.store);
                    body.child(group().child(
                        h_flex()
                            .id("minimize-to-tray")
                            .h(px(ROW_HEIGHT))
                            .px_4()
                            .gap_2()
                            .cursor_pointer()
                            .child(Checkbox::new("minimize-to-tray-checkbox").accent().toggle_state(enabled))
                            .child(Label::new(strings.minimize_to_tray))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let enabled = crate::tray::enabled(&this.store);
                                if let Err(err) = crate::tray::set_enabled(&this.store, !enabled) {
                                    eprintln!("failed to save the notification area setting: {err:#}");
                                }
                                cx.notify();
                            })),
                    ))
                };
                body.into_any_element()
            }
            Page::Features => {
                let features = self.preferences.features;
                let values = [features.calendar, features.matrix, features.habits, features.focus, features.countdown];
                let toggles: [fn(&mut fern_core::Features); 5] = [
                    |f| f.calendar = !f.calendar,
                    |f| f.matrix = !f.matrix,
                    |f| f.habits = !f.habits,
                    |f| f.focus = !f.focus,
                    |f| f.countdown = !f.countdown,
                ];
                body.child(title(strings.features)).child(group().children(
                    strings.feature_items.into_iter().zip(values).zip(toggles).enumerate().map(|(i, (((name, hint), on), toggle))| {
                        v_flex()
                            .px_4()
                            .py_1p5()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .id(SharedString::from(format!("feature-{i}")))
                                    .cursor_pointer()
                                    .child(div().flex_1().child(Label::new(name)))
                                    .child(Switch::new().toggle_state(on))
                                    .on_click(cx.listener(move |this, _, window, cx| this.update(window, cx, |p| toggle(&mut p.features)))),
                            )
                            .child(div().text_size(px(11.)).text_color(colors.text_muted).child(hint))
                    }),
                ))
                .into_any_element()
            }
            Page::Sync => {
                let field = |label: &'static str, input: Entity<crate::text_input::TextInput>| {
                    h_flex()
                        .h(px(40.))
                        .px_4()
                        .gap_4()
                        .child(div().w(px(80.)).child(Label::new(label)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .h(px(28.))
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .bg(colors.background)
                                .child(input),
                        )
                };
                let folder = crate::data_dir().map(|dir| dir.display().to_string()).unwrap_or_default();
                body.child(title(strings.data_on_device))
                    .child(
                        group().child(
                            h_flex()
                                .h(px(ROW_HEIGHT))
                                .px_4()
                                .gap_4()
                                .child(Label::new(strings.data_folder))
                                .child(div().flex_1().min_w_0().overflow_hidden().child(Label::new(folder).color(Color::Muted).truncate()))
                                .child(
                                    div()
                                        .id("open-data-folder")
                                        .cursor_pointer()
                                        .child(Label::new(strings.open_folder).color(Color::Accent))
                                        .on_click(|_, _, cx| {
                                            if let Ok(dir) = crate::data_dir() {
                                                cx.open_with_system(&dir);
                                            }
                                        }),
                                ),
                        ),
                    )
                    .child(div().pt_2().pb_4().px_1().text_size(px(12.)).text_color(colors.text_muted).child(strings.data_hint))
                    .child(title(strings.sync_optional))
                    .child(
                        group()
                            .child(field(strings.sync_server, self.sync_server.clone()))
                            .child(field(strings.sync_token, self.sync_token.clone())),
                    )
                    .child(div().pt_2().px_1().text_size(px(12.)).text_color(colors.text_muted).child(strings.sync_hint))
                    .into_any_element()
            }
            Page::Integrations => body.children(self.render_integrations(cx)).into_any_element(),
            Page::Sticky => body.child(self.render_sticky(cx)).into_any_element(),
            Page::About => {
                let row = |label: &'static str, value: &'static str| {
                    h_flex()
                        .h(px(ROW_HEIGHT))
                        .px_4()
                        .child(div().flex_1().child(Label::new(label)))
                        .child(Label::new(value).color(Color::Muted))
                };
                body.child(
                    v_flex()
                        .items_center()
                        .gap_1()
                        .py_4()
                        .child(Label::new("Fern").size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                        .child(Label::new(strings.about_tagline).color(Color::Muted)),
                )
                .child(
                    group()
                        .child(row(strings.version, release_notes::CURRENT_VERSION))
                        .children(self.installed_notes.as_ref().map(|notes| {
                            let title = strings.whats_new_in_version.replace("{version}", &notes.version);
                            let notes = notes.clone();
                            action_button("installed-release-notes", cx)
                                .mx_2()
                                .min_h(px(ROW_HEIGHT))
                                .child(div().flex_1().child(Label::new(title)))
                                .child(Icon::new(IconName::ChevronRight).size(IconSize::Small).color(Color::Muted))
                                .on_click(cx.listener(move |this, _, window, cx| this.show_release_notes(notes.clone(), window, cx)))
                        }))
                        .child(row(strings.license, "AGPL-3.0-or-later")),
                )
                .children(self.render_updates(cx))
                .child(
                    v_flex()
                        .child(title(strings.backup_restore))
                        .child(
                            group().child(
                                h_flex()
                                    .h(px(ROW_HEIGHT))
                                    .px_4()
                                    .gap_4()
                                    .child(div().flex_1().child(Label::new(strings.backup_restore)))
                                    .child(
                                        div()
                                            .id("generate-backup")
                                            .cursor_pointer()
                                            .child(Label::new(strings.generate_backup).color(Color::Accent))
                                            .on_click(cx.listener(|this, _, _, cx| this.generate_backup(cx))),
                                    )
                                    .child(
                                        div()
                                            .id("import-backup")
                                            .cursor_pointer()
                                            .child(Label::new(strings.import_backups).color(Color::Accent))
                                            .on_click(cx.listener(|this, _, _, cx| this.import_backup(cx))),
                                    ),
                            ),
                        )
                        .children(self.backup_message.clone().map(|message| {
                            div().pt_1p5().px_1().text_size(px(11.)).text_color(colors.text_muted).child(message)
                        })),
                )
                .into_any_element()
            }
        }
    }

    fn render_release_notes(&self, notes: &ReleaseNotes, cx: &App) -> AnyElement {
        let strings = i18n::t(cx);
        let title = strings.whats_new_in_version.replace("{version}", &notes.version);
        let url = notes.url.clone();
        v_flex()
            .id(SharedString::from(format!("release-notes-page-{}", notes.version)))
            .debug_selector(|| "release-notes-page".to_owned())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_6()
            .pt_2()
            .pb_6()
            .gap_4()
            .child(Label::new(title).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
            .child(if notes.markdown.is_empty() {
                div().text_size(px(14.)).child(strings.release_notes_missing).into_any_element()
            } else {
                crate::task_detail::render_release_markdown(&notes.markdown, cx)
            })
            .child(
                action_button("release-notes-github", cx)
                    .self_start()
                    .border_color(cx.theme().colors().border)
                    .child(Label::new(strings.release_on_github).size(LabelSize::Small))
                    .child(Icon::new(IconName::SquareArrowOutUpRight).size(IconSize::Small).color(Color::Muted))
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .into_any_element()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let notes_open = self.release_notes.is_some();
        let about = i18n::t(cx).settings_pages[5];
        let page = match &self.release_notes {
            Some(notes) => self.render_release_notes(notes, cx),
            None => self.render_page(cx),
        };
        h_flex()
            .id("settings-window")
            .track_focus(&self.focus_handle)
            .tab_group()
            .key_context("Settings")
            .on_action(cx.listener(|this, _: &DismissSettings, window, cx| {
                this.dismiss(window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(this.page, Page::About | Page::Appearance) && event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .w(px(840.))
            .h(px(680.))
            .max_w_full()
            .max_h_full()
            .flex_none()
            .relative()
            .rounded(px(WINDOW_RADIUS))
            .overflow_hidden()
            .bg(colors.elevated_surface_background)
            .border(px(WINDOW_BORDER_WIDTH))
            .border_color(colors.border)
            .shadow_lg()
            .text_color(colors.text)
            .child(self.render_nav(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .rounded_tr(PANEL_RADIUS)
                    .rounded_br(PANEL_RADIUS)
                    .bg(colors.background)
                    .child(
                        h_flex()
                            .debug_selector(|| "settings-header".to_owned())
                            .h(px(44.))
                            .flex_none()
                            .px_3()
                            .when(!notes_open, |el| el.justify_end())
                            .when(notes_open, |el| el.justify_between().child(
                                action_button("release-notes-back", cx)
                                    .child(Icon::new(IconName::ChevronLeft).size(IconSize::Small).color(Color::Muted))
                                    .child(Label::new(about).size(LabelSize::Small))
                                    .on_click(cx.listener(|this, _, window, cx| this.back_to_about(window, cx))),
                            ))
                            .child(IconButton::new("close-settings", IconName::X)
                                .icon_color(Color::Muted)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Dismissed)))),
                    )
                    .child(page),
            )
    }
}

fn smart_list_label(list: SmartList, strings: &i18n::Strings) -> (IconName, &'static str) {
    match list {
        SmartList::All => (IconName::Layers, strings.all),
        SmartList::Today => (IconName::CalendarCheck, strings.today),
        SmartList::Tomorrow => (IconName::Sunrise, strings.tomorrow),
        SmartList::Next7Days => (IconName::CalendarRange, strings.next_7_days_short),
        SmartList::Inbox => (IconName::Inbox, strings.inbox),
        SmartList::Summary => (IconName::FileText, strings.summary),
        SmartList::Tags => (IconName::Tag, strings.tags),
        SmartList::Filters => (IconName::Funnel, strings.filters),
        SmartList::Completed => (IconName::SquareCheckBig, strings.completed),
        SmartList::WontDo => (IconName::SquareX, strings.wont_do),
        SmartList::Trash => (IconName::Trash, strings.trash),
    }
}

fn visibility_label(list: SmartList, visibility: Visibility, strings: &i18n::Strings) -> &'static str {
    match visibility {
        Visibility::Show => strings.visibility_names[0],
        Visibility::IfNotEmpty if list == SmartList::Tags => strings.visibility_names[3],
        Visibility::IfNotEmpty => strings.visibility_names[1],
        Visibility::Hide => strings.visibility_names[2],
    }
}

/// A small dialog button: the accent for the one that confirms, a frame for the other.
fn form_button(id: &'static str, label: &'static str, primary: bool, colors: &theme::ThemeColors) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(80.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .when(primary, |el| el.bg(colors.accent))
        .when(!primary, |el| el.border_1().border_color(colors.border))
        .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[gpui::test]
    fn task_positions_share_a_painted_settings_surface(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (settings, cx) = cx.add_window_view(|_, cx| {
            let mut settings = SettingsWindow::new(store, cx);
            settings.set_page(Page::More, cx);
            settings
        });
        cx.simulate_resize(gpui::size(px(900.), px(720.)));

        for theme in [theme::Theme::light(), theme::Theme::dark()] {
            let surface = theme.colors().surface_background;
            cx.update(|_, cx| {
                theme::set(theme, cx);
                settings.update(cx, |settings, cx| {
                    settings.page_scroll.scroll_to_bottom();
                    cx.notify();
                });
            });
            let add_to = cx.debug_bounds("settings-row-AddTo").unwrap();
            let overdue = cx.debug_bounds("settings-row-OverdueAt").unwrap();
            cx.update(|window, _| {
                let scale = window.scale_factor();
                let positions = [add_to.center().scale(scale), overdue.center().scale(scale)];
                assert!(window.painted_quads().iter().any(|quad| {
                    quad.background.as_solid() == Some(surface)
                        && positions.iter().all(|point| {
                            quad.bounds.contains(point) && quad.content_mask.bounds.contains(point)
                        })
                }), "both task position rows must be painted on the same settings surface");
            });
        }
    }

    #[gpui::test]
    fn china_holidays_offer_both_sources_in_settings(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
            cx.set_global(CurrentLocale(Locale::ZhCn));
            crate::subscriptions::init(store.clone(), cx);
        });
        let (settings, cx) = cx.add_window_view(|window, cx| {
            let mut settings = SettingsWindow::new(store, cx);
            settings.set_page(Page::Integrations, cx);
            window.focus(&settings.focus_handle, cx);
            settings
        });
        cx.simulate_resize(gpui::size(px(900.), px(720.)));

        let trigger = cx.debug_bounds("china-holiday-source").unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::default());
        for source in ChinaHolidaySource::ALL {
            assert!(cx.debug_bounds(source.label()).is_some());
        }
        cx.simulate_keystrokes("escape");
        assert!(cx.debug_bounds("china-holiday-source-menu").is_none());
        cx.update(|_, cx| assert_eq!(settings.read(cx).page, Page::Integrations));
    }
}
