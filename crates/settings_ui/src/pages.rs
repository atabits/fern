use std::rc::Rc;

use chrono::Datelike as _;
use desktop_common::i18n::{self, Locale};
use desktop_common::text_input::TextInput;
use desktop_common::{calendar, paths, release_notes};
use fern_core::reminder::{ALL_DAY_OFFSETS, TIMED_OFFSETS};
use fern_core::{
    DURATIONS, DateFormat, DefaultDate, Priority, SmartList, Visibility, WEEK_STARTS,
    format_minutes,
};
use gpui::{Div, Entity, FontWeight, Hsla, Stateful};
use ui::prelude::*;
use ui::{ConfirmDialog, Placement, PopoverAttachment, VerticalScrollbar, popover, v_scroll};

use super::{ActiveConfirm, Menu, Page, ROW_HEIGHT, SettingsWindow};

type SelectPick = Box<dyn Fn(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>)>;
type SelectOption = (SharedString, bool, SelectPick);

fn page_group(surface: Hsla) -> Div {
    v_flex().py_1p5().rounded(px(8.)).bg(surface)
}

fn page_title(text: &'static str) -> Div {
    div()
        .pb_3()
        .child(Label::new(text).weight(FontWeight::MEDIUM))
}

impl SettingsWindow {
    pub(super) fn render_active_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm.as_ref()?;
        let strings = i18n::t(cx);
        match confirm {
            ActiveConfirm::RestoreBackup(path) => {
                let filename = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("backup.db");
                let description = strings.restore_backup_confirm_body.replace("{file}", filename);
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .debug_selector(|| "confirm-restore-backup".to_owned())
                        .child(
                            ConfirmDialog::new(
                                "confirm-restore-backup",
                                strings.restore_backup_confirm_title,
                                strings.cancel,
                                strings.restore_action,
                            )
                            .description(description)
                            .destructive(true)
                            .on_cancel(cx.listener(|this, _, _, cx| {
                                this.confirm = None;
                                cx.notify();
                            }))
                            .on_confirm(cx.listener(|this, _, _, cx| {
                                this.confirm_restore(cx);
                            })),
                        )
                        .into_any_element(),
                )
            }
            ActiveConfirm::ResetShortcuts => Some(
                ConfirmDialog::new(
                    "confirm-reset-shortcuts",
                    strings.shortcut_reset_confirm_title,
                    strings.cancel,
                    strings.shortcut_reset_all,
                )
                .description(strings.shortcut_reset_confirm_body)
                .destructive(true)
                .on_cancel(cx.listener(|this, _, _, cx| {
                    this.confirm = None;
                    cx.notify();
                }))
                .on_confirm(cx.listener(|this, _, _, cx| {
                    this.reset_all_shortcuts(cx);
                }))
                .into_any_element(),
            ),
            ActiveConfirm::SwitchTheme(_) => Some(
                div()
                    .absolute()
                    .inset_0()
                    .when(cfg!(test), |el| {
                        el.debug_selector(|| "confirm-switch-theme".to_owned())
                    })
                    .child(
                        ConfirmDialog::new(
                            "confirm-switch-theme",
                            strings.switch_theme,
                            strings.cancel,
                            strings.theme_always,
                        )
                        .width(px(480.))
                        .description(strings.theme_override_hint)
                        .secondary_action(
                            strings.theme_once,
                            cx.listener(|this, _, window, cx| {
                                this.confirm_theme_switch(false, window, cx);
                            }),
                        )
                        .on_cancel(cx.listener(|this, _, _, cx| {
                            this.confirm = None;
                            cx.notify();
                        }))
                        .on_confirm(cx.listener(|this, _, window, cx| {
                            this.confirm_theme_switch(true, window, cx);
                        })),
                    )
                    .into_any_element(),
            ),
            ActiveConfirm::DeleteSubscription { title, .. } => Some(
                div()
                    .absolute()
                    .inset_0()
                    .when(cfg!(test), |el| {
                        el.debug_selector(|| "confirm-delete-subscription".to_owned())
                    })
                    .child(
                        ConfirmDialog::new(
                            "confirm-delete-subscription",
                            format!("{} \"{title}\"?", strings.subscription_delete),
                            strings.cancel,
                            strings.subscription_delete,
                        )
                        .destructive(true)
                        .on_cancel(cx.listener(|this, _, _, cx| {
                            this.confirm = None;
                            cx.notify();
                        }))
                        .on_confirm(cx.listener(|this, _, _, cx| {
                            this.confirm_delete_subscription(cx);
                        })),
                    )
                    .into_any_element(),
            ),
        }
    }

    fn render_task_defaults(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let defaults = self.preferences.task_defaults.clone();
        let pick = |change: Box<dyn Fn(&mut fern_core::TaskDefaults)>| -> SelectPick {
            Box::new(move |this, window, cx| this.update(window, cx, |p| change(&mut p.task_defaults)))
        };
        let date_name = |date: Option<DefaultDate>| match date {
            None => strings.none_option,
            Some(date) => {
                strings.default_dates[DefaultDate::ALL.iter().position(|d| *d == date).unwrap_or(0)]
            }
        };
        let dates = std::iter::once(None)
            .chain(DefaultDate::ALL.into_iter().map(Some))
            .map(|date| {
                (
                    SharedString::from(date_name(date)),
                    date == defaults.date,
                    pick(Box::new(move |t| t.date = date)),
                )
            })
            .collect();
        let duration_name = |minutes: i64| {
            DURATIONS
                .iter()
                .position(|d| *d == minutes)
                .map_or_else(|| format_minutes(minutes), |i| strings.durations[i].to_owned())
        };
        let durations = DURATIONS
            .into_iter()
            .map(|minutes| {
                (
                    SharedString::from(duration_name(minutes)),
                    minutes == defaults.duration,
                    pick(Box::new(move |t| t.duration = minutes)),
                )
            })
            .collect();
        let reminder_options = |offsets: [i64; 5],
                                names: [&'static str; 5],
                                current: Option<i64>,
                                set: fn(&mut fern_core::TaskDefaults, Option<i64>)| {
            std::iter::once((None, strings.none_option))
                .chain(offsets.into_iter().zip(names).map(|(o, n)| (Some(o), n)))
                .map(|(offset, name)| {
                    (
                        SharedString::from(name),
                        offset == current,
                        pick(Box::new(move |t| set(t, offset))),
                    )
                })
                .collect::<Vec<_>>()
        };
        let reminder_name = |offsets: [i64; 5], names: [&'static str; 5], current: Option<i64>| {
            current
                .and_then(|o| offsets.iter().position(|x| *x == o))
                .map_or(strings.none_option, |i| names[i])
        };
        page_group(cx.theme().colors().surface_background)
            .child(self.select_row(
                None,
                strings.default_date,
                date_name(defaults.date).into(),
                Menu::DefaultDate,
                dates,
                cx,
            ))
            .child(self.select_row(
                None,
                strings.default_duration,
                duration_name(defaults.duration).into(),
                Menu::DefaultDuration,
                durations,
                cx,
            ))
            .child(self.select_row(
                None,
                strings.default_timed_reminder,
                reminder_name(TIMED_OFFSETS, strings.reminders_timed, defaults.timed_reminder)
                    .into(),
                Menu::TimedReminder,
                reminder_options(
                    TIMED_OFFSETS,
                    strings.reminders_timed,
                    defaults.timed_reminder,
                    |t, o| t.timed_reminder = o,
                ),
                cx,
            ))
            .child(self.select_row(
                None,
                strings.default_all_day_reminder,
                reminder_name(
                    ALL_DAY_OFFSETS,
                    strings.reminders_all_day,
                    defaults.all_day_reminder,
                )
                .into(),
                Menu::AllDayReminder,
                reminder_options(
                    ALL_DAY_OFFSETS,
                    strings.reminders_all_day,
                    defaults.all_day_reminder,
                    |t, o| t.all_day_reminder = o,
                ),
                cx,
            ))
    }

    fn render_priority_and_tag(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let defaults = self.preferences.task_defaults.clone();
        let priorities = [
            (Priority::None, strings.priority_none),
            (Priority::Low, strings.priority_low),
            (Priority::Medium, strings.priority_medium),
            (Priority::High, strings.priority_high),
        ];
        let priority_name = priorities
            .iter()
            .find(|(p, _)| *p == defaults.priority)
            .map_or(strings.priority_none, |(_, n)| n);
        let priority_options = priorities
            .into_iter()
            .map(|(priority, name)| {
                let pick: SelectPick = Box::new(move |this, window, cx| {
                    this.update(window, cx, |p| p.task_defaults.priority = priority)
                });
                (SharedString::from(name), priority == defaults.priority, pick)
            })
            .collect();
        let tag_options = std::iter::once(None)
            .chain(self.cached_tags.iter().map(|t| Some(t.name.clone())))
            .map(|tag| {
                let selected = tag == defaults.tag;
                let label = SharedString::from(
                    tag.clone()
                        .unwrap_or_else(|| strings.none_option.to_owned()),
                );
                let pick: SelectPick = Box::new(move |this, window, cx| {
                    this.update(window, cx, |p| p.task_defaults.tag = tag.clone())
                });
                (label, selected, pick)
            })
            .collect();
        vec![
            self.select_row(
                None,
                strings.default_priority,
                (*priority_name).into(),
                Menu::DefaultPriority,
                priority_options,
                cx,
            ),
            self.select_row(
                None,
                strings.default_tag,
                defaults
                    .tag
                    .clone()
                    .unwrap_or_else(|| strings.none_option.to_owned())
                    .into(),
                Menu::DefaultTag,
                tag_options,
                cx,
            ),
        ]
    }

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
                    .when(*selected, |item| {
                        item.end_slot(
                            Icon::new(IconName::Check)
                                .color(Color::Accent)
                                .size(IconSize::Small),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        (options[i].2)(this, window, cx)
                    }))
            });
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .when(cfg!(test), |el| {
                        el.debug_selector(move || format!("settings-select-popover-{menu:?}"))
                    })
                    .min_w(px(180.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_menu(None, cx)))
                    .child(
                        v_scroll(
                            SharedString::from(format!("settings-select-scroll-{menu:?}")),
                            v_flex()
                                .id(SharedString::from(format!("settings-select-list-{menu:?}")))
                                .children(items),
                        )
                        .max_h(px(260.))
                        .compact(),
                    ),
            )
        });
        let hover_bg = cx.theme().colors().element_hover;
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
                    .px_2()
                    .py_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(open, |el| el.bg(hover_bg))
                    .hover(move |s| s.bg(hover_bg))
                    .child(Label::new(value).color(Color::Muted))
                    .child(
                        Icon::new(IconName::ChevronsUpDown)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_menu((!open).then_some(menu), cx)
                    }))
                    .children(dropdown),
            )
            .into_any_element()
    }

    pub(super) fn render_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let body = v_flex()
            .id("settings-page")
            .debug_selector(|| "settings-page".to_owned())
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.page_scroll)
            .pt_1()
            .px_6()
            .pb_6()
            .gap_4();
        let page = match self.page {
            Page::SmartList => self.render_smart_list_page(body, cx),
            Page::DateTime => self.render_date_time_page(body, cx),
            Page::Appearance => body.child(self.render_appearance(cx)).into_any_element(),
            Page::More => self.render_more_page(body, cx),
            Page::Notifications => self.render_notifications_page(body, cx),
            Page::Features => self.render_features_page(body, cx),
            Page::Sync => self.render_sync_page(body, cx),
            Page::Integrations => body.children(self.render_integrations(cx)).into_any_element(),
            Page::Plugins => {
                return div()
                    .id("settings-page")
                    .debug_selector(|| "settings-page".to_owned())
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.host.plugin_settings_view.clone())
                    .into_any_element();
            }
            Page::Sticky => body.child(self.render_sticky(cx)).into_any_element(),
            Page::Shortcuts => return self.render_shortcuts_page(cx),
            Page::About => self.render_about_page(body, cx),
        };
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(page)
            .child(VerticalScrollbar::for_handle(
                "settings-page-scroll",
                self.page_scroll.clone(),
            ))
            .into_any_element()
    }

    fn render_smart_list_page(&self, body: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let surface = cx.theme().colors().surface_background;
        let row = |this: &Self, list: SmartList, cx: &mut Context<Self>| {
            let (icon, label) = smart_list_label(list, strings);
            let current = this.preferences.visibility(list);
            let options = list
                .options()
                .iter()
                .map(|&visibility| {
                    let pick: SelectPick = Box::new(move |this, window, cx| {
                        this.update(window, cx, |p| p.set_visibility(list, visibility))
                    });
                    (
                        visibility_label(list, visibility, strings).into(),
                        visibility == current,
                        pick,
                    )
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
        let [
            all,
            today,
            tomorrow,
            next7,
            inbox,
            horizons,
            summary,
            tags,
            filters,
            completed,
            wont_do,
            trash,
        ] = SmartList::ALL;
        let mut top_lists = vec![all, today, tomorrow, next7, inbox];
        if self.preferences.features.horizons {
            top_lists.push(horizons);
        }
        top_lists.push(summary);
        body.child(page_title(strings.settings_pages[0]))
            .child(page_group(surface).children(top_lists.into_iter().map(|l| row(self, l, cx))))
            .child(page_group(surface).children([tags, filters].map(|l| row(self, l, cx))))
            .child(
                page_group(surface)
                    .children([completed, wont_do, trash].map(|l| row(self, l, cx))),
            )
            .into_any_element()
    }

    fn render_date_time_page(&self, body: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let surface = cx.theme().colors().surface_background;
        let week_start = self.preferences.week_start;
        let options = WEEK_STARTS
            .into_iter()
            .zip(strings.week_start_names)
            .map(|(day, name)| {
                let pick: SelectPick = Box::new(move |this, window, cx| {
                    this.update(window, cx, |p| p.week_start = day)
                });
                (SharedString::from(name), day == week_start, pick)
            })
            .collect();
        let current = WEEK_STARTS
            .iter()
            .position(|d| *d == week_start)
            .unwrap_or(0);
        let hour12 = self.preferences.hour12;
        let week_numbers = self.preferences.week_numbers;
        let countdown_dates = self.preferences.countdown_dates;
        let sample = chrono::NaiveDate::from_ymd_opt(chrono::Local::now().year(), 12, 31)
            .expect("December 31 exists");
        let current_format = self.preferences.date_format;
        let date_formats = DateFormat::ALL
            .into_iter()
            .map(|format| {
                let pick: SelectPick = Box::new(move |this, window, cx| {
                    this.update(window, cx, |p| p.date_format = Some(format))
                });
                let label = calendar::format_date_as(sample, Some(format), false, true);
                let selected = current_format == Some(format)
                    || (current_format.is_none()
                        && label == calendar::format_date(sample, strings, cx));
                (SharedString::from(label), selected, pick)
            })
            .collect();
        let formats = [false, true]
            .into_iter()
            .map(|twelve| {
                let pick: SelectPick = Box::new(move |this, window, cx| {
                    this.update(window, cx, |p| p.hour12 = twelve)
                });
                (
                    SharedString::from(strings.time_formats[twelve as usize]),
                    twelve == hour12,
                    pick,
                )
            })
            .collect();
        body.child(
            v_flex()
                .child(page_title(strings.settings_pages[1]))
                .child(
                    page_group(surface)
                        .child(self.select_row(
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
                            calendar::format_date(sample, strings, cx).into(),
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
                        )),
                ),
        )
        .child(
            page_group(surface)
                .child(
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
                )
                .child(
                    h_flex()
                        .id("countdown-dates")
                        .h(px(ROW_HEIGHT))
                        .px_4()
                        .cursor_pointer()
                        .child(div().flex_1().child(Label::new(strings.show_countdown_mode)))
                        .child(Switch::new().toggle_state(countdown_dates))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.update(window, cx, |p| p.countdown_dates = !countdown_dates)
                        })),
                ),
        )
        .into_any_element()
    }

    fn render_more_page(&self, body: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let current = i18n::locale(cx);
        let languages = Locale::available()
            .map(|locale| {
                let pick: SelectPick = Box::new(move |this, _, cx| this.set_locale(locale, cx));
                (
                    SharedString::from(locale.native_name()),
                    locale == current,
                    pick,
                )
            })
            .collect();
        let lists = &self.cached_lists;
        let default_list = self.preferences.default_list;
        let list_name = default_list
            .and_then(|id| lists.iter().find(|l| l.id == id))
            .map(|l| SharedString::from(l.name.clone()))
            .unwrap_or(strings.inbox.into());
        let mut list_options: Vec<SelectOption> = vec![(
            strings.inbox.into(),
            default_list.is_none(),
            Box::new(|this, window, cx| this.update(window, cx, |p| p.default_list = None)),
        )];
        list_options.extend(lists.iter().filter(|l| !l.archived).map(|list| {
            let id = list.id;
            let pick: SelectPick = Box::new(move |this, window, cx| {
                this.update(window, cx, |p| p.default_list = Some(id))
            });
            (
                SharedString::from(list.name.clone()),
                default_list == Some(id),
                pick,
            )
        }));
        let position_row =
            |label, menu, bottom, set: fn(&mut fern_core::TaskDefaults, bool), cx: &mut Context<Self>| {
                let top_label = strings.add_to_top;
                let bottom_label = strings.add_to_bottom;
                let options = [(false, top_label), (true, bottom_label)]
                    .into_iter()
                    .map(|(choice, name)| {
                        let pick: SelectPick = Box::new(move |this, window, cx| {
                            this.update(window, cx, |p| set(&mut p.task_defaults, choice))
                        });
                        (name.into(), choice == bottom, pick)
                    })
                    .collect();
                let value = if bottom { bottom_label } else { top_label };
                self.select_row(None, label, value.into(), menu, options, cx)
            };
        let defaults = &self.preferences.task_defaults;
        body.child(
            page_group(colors.surface_background)
                .debug_selector(|| "settings-language".to_owned())
                .child(self.select_row(
                    None,
                    strings.language,
                    current.native_name().into(),
                    Menu::Language,
                    languages,
                    cx,
                )),
        )
        .child({
            let r = self.preferences.recognition;
            type RecognitionCheck = (&'static str, bool, fn(&mut fern_core::Recognition));
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.update(window, cx, |p| toggle(&mut p.recognition))
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.text_muted)
                            .child(hint),
                    )
                    .when_some(check.filter(|_| on), |el, (text, checked, flip)| {
                        el.child(
                            h_flex()
                                .id(SharedString::from(format!("{id}-remove")))
                                .gap_2()
                                .pt_0p5()
                                .cursor_pointer()
                                .child(
                                    Checkbox::new(SharedString::from(format!("{id}-check")))
                                        .accent()
                                        .toggle_state(checked),
                                )
                                .child(Label::new(text).size(LabelSize::Small))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.update(window, cx, |p| flip(&mut p.recognition))
                                })),
                        )
                    })
            };
            v_flex().child(page_title(strings.smart_recognition)).child(
                page_group(colors.surface_background)
                    .child(section(
                        "recognize-dates",
                        strings.date_recognition,
                        strings.date_recognition_hint,
                        r.dates,
                        |r| r.dates = !r.dates,
                        Some((
                            strings.remove_date_text,
                            r.remove_date_text,
                            |r| r.remove_date_text = !r.remove_date_text,
                        )),
                        cx,
                    ))
                    .child(section(
                        "recognize-tags",
                        strings.tag_recognition,
                        strings.tag_recognition_hint,
                        r.tags,
                        |r| r.tags = !r.tags,
                        Some((
                            strings.remove_tag_text,
                            r.remove_tag_text,
                            |r| r.remove_tag_text = !r.remove_tag_text,
                        )),
                        cx,
                    )),
            )
        })
        .child({
            let defaults_modified = self.preferences.task_defaults
                != fern_core::TaskDefaults::default()
                || self.preferences.default_list.is_some();
            v_flex()
                .child(
                    h_flex()
                        .items_center()
                        .child(div().flex_1().child(page_title(strings.task_default)))
                        .when(defaults_modified, |el| {
                            el.child(
                                div()
                                    .id("reset-task-defaults")
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors.element_hover))
                                    .child(
                                        Label::new(strings.habit_reset)
                                            .size(LabelSize::XSmall)
                                            .color(Color::Accent),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.update(window, cx, |p| {
                                            p.task_defaults = fern_core::TaskDefaults::default();
                                            p.default_list = None;
                                        });
                                    })),
                            )
                        }),
                )
                .child(self.render_task_defaults(cx))
                .child(div().h_3())
                .child(
                    page_group(colors.surface_background)
                        .children(self.render_priority_and_tag(cx))
                        .child(self.select_row(
                            None,
                            strings.default_list,
                            list_name,
                            Menu::DefaultList,
                            list_options,
                            cx,
                        )),
                )
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
                .child(page_group(colors.surface_background).children([
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
                ]))
        })
        .child({
            let on = self.preferences.mini_calendar;
            v_flex().child(page_title(strings.mini_calendar)).child(
                page_group(colors.surface_background).child(
                    h_flex()
                        .id("mini-calendar")
                        .h(px(ROW_HEIGHT))
                        .px_4()
                        .cursor_pointer()
                        .child(div().flex_1().child(Label::new(strings.show_mini_calendar)))
                        .child(Switch::new().toggle_state(on))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.update(window, cx, |p| p.mini_calendar = !on)
                        })),
                ),
            )
        })
        .into_any_element()
    }

    fn render_notifications_page(&self, body: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let n = self.preferences.notifications;
        let values = [n.daily.is_some(), n.desktop, n.constant];
        let toggles: [fn(&mut fern_core::Notifications); 3] = [
            |n| {
                n.daily = if n.daily.is_some() {
                    None
                } else {
                    chrono::NaiveTime::from_hms_opt(9, 0, 0)
                }
            },
            |n| n.desktop = !n.desktop,
            |n| n.constant = !n.constant,
        ];
        let times: Vec<SelectOption> = (5..=23u32)
            .filter_map(|h| chrono::NaiveTime::from_hms_opt(h, 0, 0))
            .map(|time| {
                let option: SelectOption = (
                    calendar::format_time(time, strings, cx).into(),
                    n.daily == Some(time),
                    Box::new(move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                        this.update(window, cx, |p| p.notifications.daily = Some(time))
                    }),
                );
                option
            })
            .collect();
        let mut daily_time = n.daily.map(|t| {
            self.select_row(
                None,
                strings.daily_time,
                calendar::format_time(t, strings, cx).into(),
                Menu::DailyTime,
                times,
                cx,
            )
        });
        let body = body.child(page_title(strings.notifications_page)).child(
            page_group(colors.surface_background).children(
                strings
                    .notification_items
                    .into_iter()
                    .zip(values)
                    .zip(toggles)
                    .enumerate()
                    .map(|(i, (((name, hint), on), toggle))| {
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
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.update(window, cx, |p| toggle(&mut p.notifications))
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(colors.text_muted)
                                    .child(hint),
                            )
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
            ),
        );
        let sound_enabled = self.sound_enabled;
        let body = body.child(
            page_group(colors.surface_background).child(
                h_flex()
                    .id("completion-sound")
                    .h(px(ROW_HEIGHT))
                    .px_4()
                    .cursor_pointer()
                    .child(div().flex_1().child(Label::new(strings.completion_sound)))
                    .child(Switch::new().toggle_state(sound_enabled))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let next = !this.sound_enabled;
                        if let Err(err) = desktop_common::sound::set_enabled(&this.store, next, cx) {
                            desktop_common::logging::log_error(
                                "settings",
                                format!("failed to save the completion sound setting: {err:#}"),
                            );
                        } else {
                            this.sound_enabled = next;
                            if next {
                                desktop_common::sound::play_complete(cx);
                            }
                        }
                        cx.notify();
                    })),
            ),
        );
        let enabled = self.tray.minimize_to_tray;
        let topmost = self.tray.always_on_top;
        let is_wayland = paths::is_wayland_session();
        body.child(
            page_group(colors.surface_background)
                .child(
                    h_flex()
                        .id("minimize-to-tray")
                        .h(px(ROW_HEIGHT))
                        .px_4()
                        .cursor_pointer()
                        .child(div().flex_1().child(Label::new(strings.minimize_to_tray)))
                        .child(Switch::new().toggle_state(enabled))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let next = !this.tray.minimize_to_tray;
                            if let Err(err) = (this.host.set_minimize_to_tray)(&this.store, next, cx) {
                                desktop_common::logging::log_error(
                                    "settings",
                                    format!("failed to save the notification area setting: {err:#}"),
                                );
                            } else {
                                this.tray.minimize_to_tray = next;
                            }
                            cx.notify();
                        })),
                )
                .when(!is_wayland, |group| {
                    group.child(
                        h_flex()
                            .id("always-on-top")
                            .h(px(ROW_HEIGHT))
                            .px_4()
                            .cursor_pointer()
                            .child(div().flex_1().child(Label::new(strings.tray_always_on_top)))
                            .child(Switch::new().toggle_state(topmost))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next = !this.tray.always_on_top;
                                if let Err(err) = (this.host.set_always_on_top)(&this.store, next, cx) {
                                    desktop_common::logging::log_error(
                                        "settings",
                                        format!("failed to save always_on_top setting: {err:#}"),
                                    );
                                } else {
                                    this.tray.always_on_top = next;
                                }
                                cx.notify();
                            })),
                    )
                })
                .when(is_wayland, |group| {
                    group.child(
                        div()
                            .px_4()
                            .py_2p5()
                            .border_t_1()
                            .border_color(colors.border_variant.opacity(0.6))
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child(strings.wayland_window_controls_hint),
                    )
                }),
        )
        .into_any_element()
    }

    fn render_features_page(&self, body: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let features = self.preferences.features;
        let warning_color = theme.status().warning;
        let frontend_row = |id: &'static str,
                            debug_id: &'static str,
                            (name, hint): (&'static str, &'static str),
                            on: bool,
                            locked: bool,
                            beta: bool,
                            toggle: fn(&mut fern_core::Features)| {
            v_flex()
                .px_4()
                .py_1p5()
                .gap_0p5()
                .child(
                    h_flex()
                        .id(id)
                        .debug_selector(move || debug_id.to_owned())
                        .when(!locked, |el| el.cursor_pointer())
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .child(Label::new(name))
                                .when(beta, |row| row.child(Chip::new(strings.beta_badge, warning_color))),
                        )
                        .child(
                            div()
                                .when(locked, |el| el.opacity(0.6))
                                .child(Switch::new().toggle_state(on)),
                        )
                        .when(!locked, |el| {
                            el.on_click(cx.listener(move |this, _, window, cx| {
                                this.update(window, cx, |p| {
                                    toggle(&mut p.features);
                                    p.features.sanitize();
                                });
                            }))
                        }),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .child(hint),
                )
                .when(locked, |el| {
                    el.child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.text_muted)
                            .child(strings.feature_last_frontend_hint),
                    )
                })
        };
        let values = [
            features.calendar,
            features.matrix,
            features.habits,
            features.focus,
            features.countdown,
            features.life_clock,
            features.horizons,
        ];
        let toggles: [fn(&mut fern_core::Features); 7] = [
            |f| f.calendar = !f.calendar,
            |f| f.matrix = !f.matrix,
            |f| f.habits = !f.habits,
            |f| f.focus = !f.focus,
            |f| f.countdown = !f.countdown,
            |f| f.life_clock = !f.life_clock,
            |f| f.horizons = !f.horizons,
        ];
        body.child(page_title(strings.features))
            .child(
                page_group(colors.surface_background).children(
                    strings
                        .feature_items
                        .into_iter()
                        .zip(values)
                        .zip(toggles)
                        .enumerate()
                        .map(|(i, (((name, hint), on), toggle))| {
                            v_flex()
                                .px_4()
                                .py_1p5()
                                .gap_0p5()
                                .child(
                                    h_flex()
                                        .id(SharedString::from(format!("feature-{i}")))
                                        .debug_selector(move || format!("settings-feature-{i}"))
                                        .cursor_pointer()
                                        .child(div().flex_1().child(Label::new(name)))
                                        .child(Switch::new().toggle_state(on))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.update(window, cx, |p| toggle(&mut p.features))
                                        })),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(colors.text_muted)
                                        .child(hint),
                                )
                        }),
                ),
            )
            .child(
                page_group(colors.surface_background)
                    .child(frontend_row(
                        "feature-tasks",
                        "settings-feature-tasks",
                        strings.feature_tasks_item,
                        features.tasks,
                        features.tasks && !features.things,
                        false,
                        |f| f.tasks = !f.tasks,
                    ))
                    .child(frontend_row(
                        "feature-things",
                        "settings-feature-things",
                        strings.feature_things_item,
                        features.things,
                        features.things && !features.tasks,
                        true,
                        |f| f.things = !f.things,
                    )),
            )
            .into_any_element()
    }

    fn render_sync_page(&self, body: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let field = |id: &'static str, label: &'static str, input: Entity<TextInput>| {
            h_flex()
                .h(px(40.))
                .px_4()
                .gap_4()
                .child(div().w(px(110.)).flex_none().child(Label::new(label).truncate()))
                .child(
                    div()
                        .debug_selector(move || id.to_owned())
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
        let folder = paths::data_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        body.child(page_title(strings.data_on_device))
            .child(
                page_group(colors.surface_background).child(
                    h_flex()
                        .h(px(ROW_HEIGHT))
                        .px_4()
                        .gap_4()
                        .child(Label::new(strings.data_folder))
                        .child(
                            div()
                                .id("data-folder-path")
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .tooltip(Tooltip::text(folder.clone()))
                                .child(Label::new(folder).color(Color::Muted).truncate()),
                        )
                        .child(
                            Button::new("open-data-folder", strings.open_folder)
                                .variant(ButtonVariant::Secondary)
                                .size(ButtonSize::Sm)
                                .on_click(|_, _, cx| {
                                    if let Ok(dir) = paths::data_dir() {
                                        cx.open_with_system(&dir);
                                    }
                                }),
                        ),
                ),
            )
            .child(
                div()
                    .pt_2()
                    .pb_4()
                    .px_1()
                    .text_size(px(12.))
                    .text_color(colors.text_muted)
                    .child(strings.data_hint),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(page_title(strings.sync_optional))
                    .child(
                        page_group(colors.surface_background)
                            .child(field("sync-server-input", strings.sync_server, self.sync.server.clone()))
                            .child(field("sync-token-input", strings.sync_token, self.sync.token.clone()))
                            .child(field(
                                "sync-passphrase-input",
                                strings.sync_passphrase,
                                self.sync.passphrase.clone(),
                            )),
                    )
                    .child(
                        div()
                            .pt_1()
                            .px_1()
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child(strings.sync_passphrase_hint),
                    )
                    .child(
                        div()
                            .px_1()
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child(strings.sync_hint),
                    ),
            )
            .into_any_element()
    }

    fn render_about_page(&self, body: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
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
                .child(
                    Label::new("Fern")
                        .size(LabelSize::Large)
                        .weight(FontWeight::SEMIBOLD),
                )
                .child(Label::new(strings.about_tagline).color(Color::Muted)),
        )
        .child(
            page_group(colors.surface_background)
                .child(row(strings.version, release_notes::CURRENT_VERSION))
                .children(self.installed_notes.as_ref().map(|notes| {
                    let title = strings
                        .whats_new_in_version
                        .replace("{version}", &notes.version);
                    let notes = notes.clone();
                    div().px_2().child(
                        ui::ListItem::new("installed-release-notes")
                            .height(px(ROW_HEIGHT))
                            .child(div().flex_1().child(Label::new(title)))
                            .end_slot(
                                Icon::new(IconName::ChevronRight)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.show_release_notes(notes.clone(), window, cx)
                            })),
                    )
                }))
                .child(row(strings.license, "AGPL-3.0-or-later")),
        )
        .children(self.render_updates(cx))
        .child(
            v_flex()
                .child(page_title(strings.backup_restore))
                .child(
                    page_group(colors.surface_background).child(
                        h_flex()
                            .min_h(px(ROW_HEIGHT))
                            .py_1p5()
                            .px_4()
                            .gap_x_4()
                            .gap_y_1p5()
                            .flex_wrap()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(110.))
                                    .child(Label::new(strings.backup_restore)),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_none()
                                    .flex_wrap()
                                    .child(
                                        Button::new("generate-backup", strings.generate_backup)
                                            .variant(ButtonVariant::Secondary)
                                            .size(ButtonSize::Sm)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.generate_backup(cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("import-backup", strings.import_backups)
                                            .variant(ButtonVariant::Secondary)
                                            .size(ButtonSize::Sm)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.import_backup(cx)
                                            })),
                                    ),
                            ),
                    ),
                )
                .children(self.backups.message.clone().map(|message| {
                    div()
                        .pt_1p5()
                        .px_1()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .child(message)
                })),
        )
        .child(self.render_auto_backups(cx))
        .child(self.render_diagnostics(cx))
        .into_any_element()
    }
}

fn smart_list_label(list: SmartList, strings: &i18n::Strings) -> (IconName, &'static str) {
    match list {
        SmartList::All => (IconName::Layers, strings.all),
        SmartList::Today => (IconName::CalendarCheck, strings.today),
        SmartList::Tomorrow => (IconName::Sunrise, strings.tomorrow),
        SmartList::Next7Days => (IconName::CalendarRange, strings.next_7_days_short),
        SmartList::Inbox => (IconName::Inbox, strings.inbox),
        SmartList::Horizons => (IconName::FernHorizons, strings.section_horizons),
        SmartList::Summary => (IconName::FileText, strings.summary),
        SmartList::Tags => (IconName::Tag, strings.tags),
        SmartList::Filters => (IconName::Funnel, strings.filters),
        SmartList::Completed => (IconName::SquareCheckBig, strings.completed),
        SmartList::WontDo => (IconName::SquareX, strings.wont_do),
        SmartList::Trash => (IconName::Trash, strings.trash),
    }
}

fn visibility_label(
    list: SmartList,
    visibility: Visibility,
    strings: &i18n::Strings,
) -> &'static str {
    match visibility {
        Visibility::Show => strings.visibility_names[0],
        Visibility::IfNotEmpty if list == SmartList::Tags => strings.visibility_names[3],
        Visibility::IfNotEmpty => strings.visibility_names[1],
        Visibility::Hide => strings.visibility_names[2],
    }
}
