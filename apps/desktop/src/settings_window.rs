//! The Settings window from the account menu, modeled on the original's: pages on the left,
//! grouped rows on the right, values picked from small dropdowns.

use std::rc::Rc;

use chrono::Datelike as _;
use fern_core::reminder::{ALL_DAY_OFFSETS, TIMED_OFFSETS};
use fern_core::{DURATIONS, DefaultDate, Priority, format_minutes, DateFormat, Preferences, SmartList, Store, ThemeMode, Visibility, WEEK_STARTS};
use gpui::{EventEmitter, FocusHandle, Focusable, FontWeight, WindowAppearance};
use theme::Theme;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::actions::DismissSettings;
use crate::calendar;
use crate::i18n::{self, CurrentLocale, Locale};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    SmartList,
    #[default]
    DateTime,
    Appearance,
    More,
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
    TimeFormat,
    DateFormat,
    WeekStart,
    Language,
    DefaultList,
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
    menu: Option<Menu>,
    focus_handle: FocusHandle,
}

impl EventEmitter<SettingsEvent> for SettingsWindow {}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Installs the theme and the first day of the week from `preferences`.
pub fn apply(preferences: &Preferences, window: &Window, cx: &mut App) {
    let dark = match preferences.theme {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
    };
    let theme = if dark { Theme::dark() } else { Theme::light() };
    if theme.name != cx.theme().name {
        theme::set(theme, cx);
    }
    calendar::set_week_start(preferences.week_start);
    calendar::set_hour12(preferences.hour12);
    calendar::set_date_format(preferences.date_format);
    calendar::set_week_numbers(preferences.week_numbers);
    cx.refresh_windows();
}

impl SettingsWindow {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let preferences = store.preferences().unwrap_or_default();
        Self { store, preferences, page: Page::default(), menu: None, focus_handle: cx.focus_handle() }
    }

    pub fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        self.menu = None;
        cx.notify();
    }

    fn set_menu(&mut self, menu: Option<Menu>, cx: &mut Context<Self>) {
        self.menu = menu;
        cx.notify();
    }

    fn update(&mut self, window: &mut Window, cx: &mut Context<Self>, change: impl FnOnce(&mut Preferences)) {
        change(&mut self.preferences);
        self.menu = None;
        if let Err(err) = self.store.save_preferences(&self.preferences) {
            eprintln!("failed to save the settings: {err:#}");
        }
        apply(&self.preferences, window, cx);
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
        self.page = match page {
            "smart-list" => Page::SmartList,
            "appearance" => Page::Appearance,
            "more" => Page::More,
            "about" => Page::About,
            _ => Page::DateTime,
        };
        self.menu = match menu {
            "week" => Some(Menu::WeekStart),
            "time" => Some(Menu::TimeFormat),
            "date" => Some(Menu::DateFormat),
            "today" => Some(Menu::Visibility(SmartList::Today)),
            "tags" => Some(Menu::Visibility(SmartList::Tags)),
            "language" => Some(Menu::Language),
            "list" => Some(Menu::DefaultList),
            "duration" => Some(Menu::DefaultDuration),
            "reminder" => Some(Menu::TimedReminder),
            _ => None,
        };
        cx.notify();
    }

    /// Task Default's first group: date, duration and reminders of new tasks.
    fn render_task_defaults(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let defaults = self.preferences.task_defaults.clone();
        let pick = |change: Box<dyn Fn(&mut fern_core::TaskDefaults)>| -> Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> {
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
                let pick: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> =
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
                let pick: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> =
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
            ListItem::new(id).height(px(34.)).toggle_state(selected).start_slot(Icon::new(icon)).child(Label::new(label))
        };
        let page = |id, icon, index: usize, target: Page| {
            item(id, icon, strings.settings_pages[index], self.page == target)
                .on_click(cx.listener(move |this, _, _, cx| this.set_page(target, cx)))
        };
        v_flex()
            .flex_none()
            .w(px(200.))
            .h_full()
            .p_2()
            .gap_0p5()
            .bg(cx.theme().colors().panel_background)
            .border_r_1()
            .border_color(cx.theme().colors().border)
            .child(div().px_2().pt_2().pb_3().child(Label::new(strings.settings).size(LabelSize::Large).weight(FontWeight::SEMIBOLD)))
            .child(page("settings-smart-list", IconName::Funnel, 0, Page::SmartList))
            .child(page("settings-date-time", IconName::Clock, 1, Page::DateTime))
            .child(page("settings-appearance", IconName::PaintRoller, 2, Page::Appearance))
            .child(page("settings-more", IconName::ToggleRight, 3, Page::More))
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
        options: Vec<(SharedString, bool, Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)>)>,
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
            .h(px(36.))
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

    fn render_page(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let group = || v_flex().py_1p5().rounded(px(8.)).bg(colors.surface_background);
        let title = |text: &'static str| div().pb_3().child(Label::new(text).weight(FontWeight::SEMIBOLD));
        let body = v_flex().id("settings-page").flex_1().min_w_0().h_full().overflow_y_scroll().bg(colors.background).pt(px(48.)).px_6().pb_6().gap_4();
        match self.page {
            Page::SmartList => {
                let row = |this: &Self, list: SmartList, cx: &mut Context<Self>| {
                    let (icon, label) = smart_list_label(list, strings);
                    let current = this.preferences.visibility(list);
                    let options = list
                        .options()
                        .iter()
                        .map(|&visibility| {
                            let pick: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> = Box::new(
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
                        let pick: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> =
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
                        let pick: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> =
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
                        let pick: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> =
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
                            .h(px(36.))
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
            Page::Appearance => {
                let mode = self.preferences.theme;
                let follow = mode == ThemeMode::System;
                let dark_now = !cx.theme().is_light();
                let swatch = |id: &'static str, label: &'static str, dark: bool| {
                    let selected = dark == dark_now;
                    let (fill, dots) = if dark { (Theme::dark(), true) } else { (Theme::light(), false) };
                    v_flex()
                        .id(id)
                        .items_center()
                        .gap_1p5()
                        .cursor_pointer()
                        .child(
                            div()
                                .relative()
                                .size(px(48.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(colors.border)
                                .bg(fill.colors().background)
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(dots, |el| {
                                    el.child(h_flex().gap(px(3.)).children((0..3).map(|_| {
                                        div().size(px(5.)).rounded_full().bg(colors.accent)
                                    })))
                                })
                                .when(selected, |el| {
                                    el.child(
                                        div()
                                            .absolute()
                                            .right(px(-5.))
                                            .bottom(px(-5.))
                                            .size(px(16.))
                                            .rounded_full()
                                            .bg(colors.accent)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Custom(gpui::white()))),
                                    )
                                }),
                        )
                        .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.update(window, cx, |p| p.theme = if dark { ThemeMode::Dark } else { ThemeMode::Light })
                        }))
                };
                let system_dark = matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
                body.child(
                    group().child(
                        h_flex()
                            .id("follow-system")
                            .h(px(36.))
                            .px_4()
                            .cursor_pointer()
                            .child(div().flex_1().child(Label::new(strings.follow_system_dark_mode)))
                            .child(Switch::new().toggle_state(follow))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.update(window, cx, |p| {
                                    p.theme = match (follow, system_dark) {
                                        (false, _) => ThemeMode::System,
                                        (true, true) => ThemeMode::Dark,
                                        (true, false) => ThemeMode::Light,
                                    }
                                })
                            })),
                    ),
                )
                .child(
                    v_flex()
                        .gap_3()
                        .child(Label::new(strings.color_series).weight(FontWeight::SEMIBOLD))
                        .child(
                            h_flex()
                                .gap_4()
                                .child(swatch("theme-light", strings.theme_names[0], false))
                                .child(swatch("theme-dark", strings.theme_names[1], true)),
                        ),
                )
                .into_any_element()
            }
            Page::More => {
                let current = i18n::locale(cx);
                let languages = Locale::ALL
                    .into_iter()
                    .map(|locale| {
                        let pick: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> =
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
                let mut list_options: Vec<(SharedString, bool, Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)>)> =
                    vec![(
                        strings.inbox.into(),
                        default_list.is_none(),
                        Box::new(|this, window, cx| this.update(window, cx, |p| p.default_list = None)),
                    )];
                list_options.extend(lists.iter().map(|list| {
                    let id = list.id;
                    let pick: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> =
                        Box::new(move |this, window, cx| this.update(window, cx, |p| p.default_list = Some(id)));
                    (SharedString::from(list.name.clone()), default_list == Some(id), pick)
                }));
                body.child(group().child(self.select_row(
                    None,
                    strings.language,
                    current.native_name().into(),
                    Menu::Language,
                    languages,
                    cx,
                )))
                .child({
                    let r = self.preferences.recognition;
                    // A switch row with a hint, and the checkbox under it while the switch is on.
                    let section = |id: &'static str,
                                   label: &'static str,
                                   hint: &'static str,
                                   on: bool,
                                   toggle: fn(&mut fern_core::Recognition),
                                   check: Option<(&'static str, bool, fn(&mut fern_core::Recognition))>,
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
                                        .child(Checkbox::new(SharedString::from(format!("{id}-check"))).toggle_state(checked))
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
                        ),
                )
                .into_any_element()
            }
            Page::About => {
                let row = |label: &'static str, value: &'static str| {
                    h_flex()
                        .h(px(36.))
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
                        .child(row(strings.version, env!("CARGO_PKG_VERSION")))
                        .child(row(strings.license, "AGPL-3.0-or-later")),
                )
                .into_any_element()
            }
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        h_flex()
            .id("settings-window")
            .track_focus(&self.focus_handle)
            .key_context("Settings")
            .on_action(cx.listener(|_, _: &DismissSettings, _, cx| cx.emit(SettingsEvent::Dismissed)))
            .w(px(800.))
            .h(px(680.))
            .relative()
            .rounded(px(12.))
            .overflow_hidden()
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .text_color(colors.text)
            .child(self.render_nav(cx))
            .child(self.render_page(window, cx))
            .child(
                div().absolute().top_3().right_3().child(
                    IconButton::new("close-settings", IconName::X)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Dismissed))),
                ),
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
