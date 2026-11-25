use fern_core::countdown::ShowInList;
use fern_core::{CountdownKind, Frequency, Recurrence};
use gpui::{Entity, Focusable as _};
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Placement, PopoverAttachment, SegmentedTab,
    SegmentedTabs, popover,
};

use desktop_common::text_input::TextInput;
use desktop_common::{calendar, colors as lists_panel};
use fern_i18n as i18n;

use crate::modal::{CustomReminder, DateField, Draft, DraftRow};
use crate::{CountdownView, format_date, kind_icon, kind_label};

/// Emoji offered for countdowns; the original's illustrated icon set is its own artwork.
const COUNTDOWN_EMOJI: [&str; 14] = [
    "🎂", "🎉", "❤️", "💍", "✈️", "🎄", "🎓", "⭐", "🏆", "🎯", "🚩", "🎮", "👶", "🏠",
];

fn draft_choice(id: SharedString, label: &'static str, on: bool) -> ListItem {
    ListItem::new(id)
        .height(px(34.))
        .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
        .when(on, |item| {
            item.end_slot(
                Icon::new(IconName::Check)
                    .size(IconSize::XSmall)
                    .color(Color::Accent),
            )
        })
}

impl CountdownView {
    fn render_date_options(&self, draft: &Draft, field: DateField, cx: &Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let options = field.options(draft.date);
        let selected = field.value(draft.date);
        let earlier = (selected < *options.start()).then_some(selected);
        let later = (selected > *options.end()).then_some(selected);
        v_scroll(
            ("countdown-date-options-scroll", field as usize),
            v_flex()
                .id(("countdown-date-options", field as usize))
                .debug_selector(|| "countdown-date-options".to_owned())
                .children(earlier.into_iter().chain(options).chain(later).enumerate().map(|(i, value)| {
                    let on = value == selected;
                    let label = if field == DateField::Month {
                        strings.months_short[value as usize - 1].to_owned()
                    } else {
                        value.to_string()
                    };
                    ListItem::new((field.id(), i))
                        .height(px(34.))
                        .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
                        .when(on, |item| {
                            item.end_slot(
                                Icon::new(IconName::Check)
                                    .size(IconSize::XSmall)
                                    .color(Color::Accent),
                            )
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(draft) = this.draft.as_mut() {
                                draft.set_date(field.with_value(draft.date, value), cx);
                                draft.close_date_part();
                                window.focus(&draft.date_inputs.fields[field as usize].focus_handle(cx), cx);
                                cx.notify();
                            }
                        }))
                })),
        )
        .track_scroll(&draft.date_inputs.scroll)
        .max_h(px(272.))
        .w_full()
        .compact()
        .into_any_element()
    }

    fn render_date_fields(&self, draft: &Draft, window: &Window, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let strings = i18n::t(cx);
        let active = match draft.open {
            Some(DraftRow::DatePart(field)) => Some(field),
            _ => None,
        };
        let in_popover = draft.date_in_popover();
        let fields = h_flex().gap_2().w_full().children(DateField::ordered(strings, cx).map(|field| {
            let input = &draft.date_inputs.fields[field as usize];
            let is_open = active == Some(field);
            let invalid = field.parse(draft.date, input.read(cx).text()).is_none();
            let focused = input.focus_handle(cx).is_focused(window);
            let border = if invalid {
                cx.theme().status().error
            } else if is_open || focused {
                colors.accent
            } else {
                colors.border
            };
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .id(field.id())
                        .debug_selector(move || field.id().to_owned())
                        .h(px(32.))
                        .pl_2()
                        .pr_1()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(border)
                        .child(div().flex_1().min_w_0().child(input.clone()))
                        .child(
                            div()
                                .debug_selector(move || format!("{}-toggle", field.id()))
                                .child(
                                    IconButton::new(
                                        ("countdown-date-options-toggle", field as usize),
                                        IconName::ChevronDown,
                                    )
                                    .size(px(20.))
                                    .icon_size(IconSize::XSmall)
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text(field.label(strings)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(draft) = this.draft.as_mut() {
                                            if is_open {
                                                draft.close_date_part();
                                            } else {
                                                Self::open_date_part(draft, field);
                                            }
                                            window.focus(
                                                &draft.date_inputs.fields[field as usize].focus_handle(cx),
                                                cx,
                                            );
                                            cx.notify();
                                        }
                                    })),
                                ),
                        ),
                )
                .when(is_open && !in_popover, |el| {
                    el.child(PopoverAttachment::new(
                        Placement::BelowLeft,
                        popover(cx)
                            .w(px(110.))
                            .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                                if let Some(draft) = this.draft.as_mut()
                                    && draft.open == Some(DraftRow::DatePart(field))
                                {
                                    draft.close_date_part();
                                    cx.notify();
                                }
                            }))
                            .child(self.render_date_options(draft, field, cx)),
                    ))
                })
        }));

        v_flex()
            .gap_1()
            .child(fields)
            .when_some(active.filter(|_| in_popover), |el, field| {
                el.child(self.render_date_options(draft, field, cx))
            })
    }

    pub(crate) fn render_date_editor(&self, draft: &Draft, window: &Window, cx: &Context<Self>) -> Div {
        if !draft.date_in_popover() {
            return self.render_date_fields(draft, window, cx);
        }
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        let open = matches!(draft.open, Some(DraftRow::Date | DraftRow::DatePart(_)));
        div()
            .relative()
            .child(
                h_flex()
                    .id("countdown-date-popup-trigger")
                    .debug_selector(|| "countdown-date-popup-trigger".to_owned())
                    .h(px(32.))
                    .px_3()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if open { colors.accent } else { colors.border })
                    .cursor_pointer()
                    .child(div().flex_1().min_w_0().child(Label::new(format_date(draft.date, strings, cx)).truncate()))
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(draft) = this.draft.as_mut() {
                            draft.open = (!open).then_some(DraftRow::Date);
                            let target = if open {
                                draft.name.focus_handle(cx)
                            } else {
                                let field = DateField::ordered(i18n::t(cx), cx)[0];
                                draft.date_inputs.fields[field as usize].focus_handle(cx)
                            };
                            window.focus(&target, cx);
                            cx.notify();
                        }
                    })),
            )
            .when(open, |el| {
                el.child(PopoverAttachment::new(
                    Placement::BelowLeft,
                    popover(cx)
                        .w(px(294.))
                        .p_2()
                        .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                            if let Some(draft) = this.draft.as_mut()
                                && matches!(draft.open, Some(DraftRow::Date | DraftRow::DatePart(_)))
                            {
                                let Some(date) = draft.date_from_fields(cx) else { return };
                                draft.set_date(date, cx);
                                draft.open = None;
                                window.focus(&draft.name.focus_handle(cx), cx);
                                cx.notify();
                            }
                        }))
                        .child(self.render_date_fields(draft, window, cx)),
                ))
            })
    }

    pub(crate) fn render_repeat_menu(&self, draft: &Draft, cx: &Context<Self>) -> (String, AnyElement) {
        let strings = i18n::t(cx);
        let repeats = [
            (None, strings.none_option),
            (Some(Frequency::Daily), strings.repeat_daily),
            (Some(Frequency::Weekly), strings.repeat_weekly),
            (Some(Frequency::Monthly), strings.repeat_monthly),
            (Some(Frequency::Yearly), strings.repeat_yearly),
        ];
        let current = draft.repeat.as_ref().map(|r| r.frequency);
        let repeat_text = repeats
            .iter()
            .find(|(f, _)| *f == current)
            .map_or(strings.none_option, |(_, l)| *l)
            .to_owned();
        let menu = v_flex()
            .children(repeats.into_iter().map(|(frequency, label)| {
                draft_choice(
                    SharedString::from(format!("countdown-repeat-{label}")),
                    label,
                    current == frequency,
                )
                .on_click(self.draft_listener(cx, move |d| {
                    d.repeat = frequency.map(|f| Recurrence::new(f, 1));
                    d.open = None;
                }))
            }))
            .into_any_element();
        (repeat_text, menu)
    }

    pub(crate) fn render_type_menu(&self, kind: CountdownKind, cx: &Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        v_flex()
            .children(
                [
                    CountdownKind::Countdown,
                    CountdownKind::Anniversary,
                    CountdownKind::Birthday,
                    CountdownKind::Holiday,
                ]
                .map(|option| {
                    draft_choice(
                        SharedString::from(format!("countdown-kind-{}", option.as_str())),
                        kind_label(option, strings),
                        option == kind,
                    )
                    .start_slot(Icon::new(kind_icon(option)).color(Color::Muted))
                    .on_click(self.draft_listener(cx, move |d| {
                        d.kind = option;
                        d.open = None;
                    }))
                }),
            )
            .into_any_element()
    }

    fn render_custom_reminder_form(&self, custom: &CustomReminder, cx: &Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let weeks = custom.weeks;
        let count: u32 = custom.count.read(cx).text().trim().parse().unwrap_or(0);
        let days = if weeks { count.saturating_mul(7) } else { count };
        let field = |input: Entity<TextInput>| {
            h_flex()
                .w(px(100.))
                .h(px(30.))
                .px_2()
                .rounded(px(6.))
                .bg(theme.colors().element_hover)
                .child(input)
        };
        v_flex()
            .gap_2()
            .p_1()
            .child(
                SegmentedTabs::new().full_width().tabs(
                    strings.countdown_custom_units.iter().enumerate().map(|(i, label)| {
                        SegmentedTab::new(("countdown-custom-unit", i), *label)
                            .toggle_state(weeks == (i == 1))
                            .on_click(self.draft_listener(cx, move |d| {
                                if let Some(custom) = d.custom.as_mut() {
                                    custom.weeks = i == 1;
                                }
                            }))
                    }),
                ),
            )
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.countdown_custom_advance[weeks as usize])))
                    .child(field(custom.count.clone())),
            )
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.countdown_custom_at)))
                    .child(field(custom.time.clone())),
            )
            .child(
                Label::new(strings.countdown_custom_ahead.replace("{n}", &days.to_string()))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Button::new("countdown-custom-cancel", strings.cancel)
                                .variant(ButtonVariant::Secondary)
                                .size(ButtonSize::Sm)
                                .full_width()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_custom_reminder(window, cx);
                                })),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            Button::new("countdown-custom-ok", strings.ok)
                                .variant(ButtonVariant::Primary)
                                .size(ButtonSize::Sm)
                                .full_width()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_custom_reminder(cx);
                                    this.apply_pending_focus(window, cx);
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_reminder_menu(&self, draft: &Draft, cx: &Context<Self>) -> (String, AnyElement) {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let custom_label = |days: u32, time: chrono::NaiveTime| {
            format!(
                "{} ({})",
                strings.countdown_custom_label.replace("{n}", &days.to_string()),
                calendar::format_time(time, strings, cx)
            )
        };
        let reminder_text = if draft.reminders.is_empty() && draft.custom_reminders.is_empty() {
            strings.none_option.to_owned()
        } else {
            fern_core::countdown::REMINDER_DAYS
                .iter()
                .zip(strings.countdown_reminder_days)
                .filter(|(days, _)| draft.reminders.contains(days))
                .map(|(_, label)| label.to_owned())
                .chain(draft.custom_reminders.iter().map(|&(days, time)| custom_label(days, time)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(custom) = draft.custom.as_ref() {
            return (reminder_text, self.render_custom_reminder_form(custom, cx));
        }

        let time = calendar::format_time(fern_core::countdown::reminder_time(), strings, cx);
        let constant = draft.constant_reminder;
        let menu = v_flex()
            .child(
                draft_choice(
                    "countdown-reminder-none".into(),
                    strings.none_option,
                    draft.reminders.is_empty() && draft.custom_reminders.is_empty(),
                )
                .on_click(self.draft_listener(cx, |d| {
                    d.reminders.clear();
                    d.custom_reminders.clear();
                })),
            )
            .children(
                fern_core::countdown::REMINDER_DAYS
                    .into_iter()
                    .zip(strings.countdown_reminder_days)
                    .map(|(days, label)| {
                        let on = draft.reminders.contains(&days);
                        ListItem::new(SharedString::from(format!("countdown-reminder-{days}")))
                            .height(px(34.))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
                                    .child(Label::new(format!("({time})")).color(if on { Color::Accent } else { Color::Muted })),
                            )
                            .when(on, |item| {
                                item.end_slot(
                                    Icon::new(IconName::Check)
                                        .size(IconSize::XSmall)
                                        .color(Color::Accent),
                                )
                            })
                            .on_click(self.draft_listener(cx, move |d| {
                                if let Some(at) = d.reminders.iter().position(|r| *r == days) {
                                    d.reminders.remove(at);
                                } else {
                                    d.reminders.push(days);
                                    d.reminders.sort();
                                }
                            }))
                    }),
            )
            // Custom reminders, each unticked with a click, then the original's Custom.
            .children(draft.custom_reminders.iter().map(|&(days, time)| {
                ListItem::new(SharedString::from(format!("countdown-custom-{days}-{time}")))
                    .height(px(34.))
                    .child(Label::new(custom_label(days, time)).color(Color::Accent))
                    .end_slot(
                        Icon::new(IconName::Check)
                            .size(IconSize::XSmall)
                            .color(Color::Accent),
                    )
                    .on_click(self.draft_listener(cx, move |d| {
                        d.custom_reminders.retain(|r| *r != (days, time))
                    }))
            }))
            .child(
                ListItem::new("countdown-custom")
                    .height(px(34.))
                    .child(Label::new(strings.countdown_custom))
                    .on_click(cx.listener(|this, _, window, cx| this.open_custom_reminder(window, cx))),
            )
            .child(div().my_1().h(px(1.)).bg(theme.colors().border_variant))
            .child(
                h_flex()
                    .id("countdown-constant")
                    .h(px(34.))
                    .px_2()
                    .cursor_pointer()
                    .child(div().flex_1().child(Label::new(strings.habit_constant_reminder)))
                    .child(Switch::new().toggle_state(constant))
                    .on_click(self.draft_listener(cx, |d| d.constant_reminder = !d.constant_reminder)),
            )
            .child(
                h_flex().pt_1().justify_end().child(
                    Button::new("countdown-reminder-ok", strings.ok)
                        .variant(ButtonVariant::Primary)
                        .size(ButtonSize::Sm)
                        .on_click(self.draft_listener(cx, |d| d.open = None)),
                ),
            )
            .into_any_element();
        (reminder_text, menu)
    }

    pub(crate) fn render_calc_menu(&self, plus_one: bool, cx: &Context<Self>) -> (String, AnyElement) {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let calc_text = strings.countdown_calc_options[plus_one as usize].0.to_owned();
        let menu = v_flex()
            .children(
                strings
                    .countdown_calc_options
                    .iter()
                    .enumerate()
                    .map(|(i, (label, about))| {
                        let on = plus_one == (i == 1);
                        h_flex()
                            .id(("countdown-calc", i))
                            .px_2()
                            .py_1p5()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.colors().element_hover))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .child(Label::new(*label).color(if on { Color::Accent } else { Color::Default }))
                                    .child(Label::new(*about).size(LabelSize::XSmall).color(Color::Muted)),
                            )
                            .when(on, |el| {
                                el.child(
                                    Icon::new(IconName::Check)
                                        .size(IconSize::XSmall)
                                        .color(Color::Accent),
                                )
                            })
                            .on_click(self.draft_listener(cx, move |d| {
                                d.plus_one_day = i == 1;
                                d.open = None;
                            }))
                    }),
            )
            .into_any_element();
        (calc_text, menu)
    }

    pub(crate) fn render_counting_menu(&self, count_up: bool, cx: &Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        v_flex()
            .children(
                strings
                    .countdown_counting_options
                    .iter()
                    .enumerate()
                    .map(|(i, label)| {
                        draft_choice(
                            SharedString::from(format!("countdown-counting-{i}")),
                            label,
                            count_up == (i == 1),
                        )
                        .on_click(self.draft_listener(cx, move |d| {
                            d.count_up = i == 1;
                            d.open = None;
                        }))
                    }),
            )
            .into_any_element()
    }

    pub(crate) fn render_show_menu(&self, show: ShowInList, cx: &Context<Self>) -> (String, AnyElement) {
        let strings = i18n::t(cx);
        let show_text = strings.countdown_show_options[show as usize].to_owned();
        let menu = v_flex()
            .child(
                div().px_2().py_1().child(
                    Label::new(strings.countdown_show_heading)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
            .children(ShowInList::ALL.into_iter().map(|option| {
                draft_choice(
                    SharedString::from(format!("countdown-show-{}", option.key())),
                    strings.countdown_show_options[option as usize],
                    option == show,
                )
                .on_click(self.draft_listener(cx, move |d| {
                    d.show_in_list = option;
                    d.open = None;
                }))
            }))
            .into_any_element();
        (show_text, menu)
    }

    pub(crate) fn render_icon_menu(&self, draft: &Draft, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .gap_2()
            .child(div().flex().flex_wrap().gap_0p5().children(COUNTDOWN_EMOJI.iter().map(|emoji| {
                let on = draft.icon.as_deref() == Some(*emoji);
                div()
                    .id(SharedString::from(format!("countdown-emoji-{emoji}")))
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(17.))
                    .when(on, |el| el.bg(theme.colors().element_selected))
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(*emoji)
                    .on_click(self.draft_listener(cx, move |d| {
                        d.icon = (!on).then(|| (*emoji).to_owned())
                    }))
            })))
            .child(
                h_flex().gap_1p5().children(
                    std::iter::once(None)
                        .chain(lists_panel::LIST_COLORS.iter().map(|c| Some(*c)))
                        .map(|color| {
                            let on = draft.color.as_deref() == color;
                            let fill = lists_panel::parse_color(color).unwrap_or(theme.colors().accent);
                            div()
                                .id(SharedString::from(format!(
                                    "countdown-color-{}",
                                    color.unwrap_or("none")
                                )))
                                .size(px(20.))
                                .rounded_full()
                                .cursor_pointer()
                                .bg(fill.opacity(if color.is_none() { 0.35 } else { 1. }))
                                .when(on, |el| el.border_2().border_color(theme.colors().text))
                                .on_click(self.draft_listener(cx, move |d| {
                                    d.color = color.map(str::to_owned)
                                }))
                        }),
                ),
            )
            .into_any_element()
    }

    pub(crate) fn render_presets_popover(&self, kind: CountdownKind, cx: &Context<Self>) -> PopoverAttachment {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let names = if kind == CountdownKind::Anniversary {
            strings.countdown_anniversary_presets
        } else {
            strings.countdown_presets
        };
        PopoverAttachment::new(
            Placement::BelowRight,
            popover(cx)
                .min_w(px(260.))
                .on_mouse_down_out(self.draft_listener(cx, |d| d.open = None))
                .children(
                    fern_core::countdown::presets(kind)
                        .iter()
                        .zip(names)
                        .enumerate()
                        .map(|(i, (preset, name))| {
                            let preset = *preset;
                            ListItem::new(("countdown-preset", i))
                                .height(px(34.))
                                .start_slot(
                                    div()
                                        .size(px(24.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .bg(lists_panel::parse_color(Some(preset.color))
                                            .unwrap_or(theme.colors().accent)
                                            .opacity(0.45))
                                        .text_size(px(13.))
                                        .child(preset.icon),
                                )
                                .child(Label::new(name))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(draft) = this.draft.as_mut() {
                                        draft.icon = Some(preset.icon.to_owned());
                                        draft.color = Some(preset.color.to_owned());
                                        draft.open = None;
                                        draft.name.update(cx, |input, cx| input.set_text(name, cx));
                                    }
                                    cx.notify();
                                }))
                        }),
                ),
        )
    }

    pub(crate) fn render_draft_dropdown(
        &self,
        which: DraftRow,
        open: Option<DraftRow>,
        text: String,
        menu: AnyElement,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let on = open == Some(which);
        div()
            .relative()
            .child(
                h_flex()
                    .id(SharedString::from(format!("countdown-row-{which:?}")))
                    .h(px(32.))
                    .px_3()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if on { theme.colors().accent } else { theme.colors().border })
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(div().flex_1().min_w_0().child(Label::new(text).truncate()))
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .on_click(self.draft_listener(cx, move |d| {
                        d.open = if d.open == Some(which) { None } else { Some(which) };
                    })),
            )
            .when(on, |el| {
                el.child(PopoverAttachment::new(
                    Placement::BelowLeft,
                    popover(cx)
                        .w(px(294.))
                        .p_2()
                        .on_mouse_down_out(self.draft_listener(cx, |d| d.open = None))
                        .child(menu),
                ))
            })
            .into_any_element()
    }
}
