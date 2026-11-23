use std::path::PathBuf;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::countdown::{DatePart, ShowInList, days_in_month, with_date_part};
use fern_core::{CountdownKind, DateFormat, Frequency, Id, Recurrence};
use gpui::{Entity, Focusable as _, FontWeight, ScrollHandle, Subscription};
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Modal, Placement, PopoverAttachment, SegmentedTab,
    SegmentedTabs, popover,
};

use desktop_common::data_version::data_changed;
use desktop_common::paths::attachments_dir;
use desktop_common::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};
use desktop_common::{calendar, colors as lists_panel};
use fern_i18n::{self as i18n, Strings};

use crate::{CountdownView, Filter, format_date, kind_icon, kind_label};

/// Emoji offered for countdowns; the original's illustrated icon set is its own artwork.
const COUNTDOWN_EMOJI: [&str; 14] = [
    "🎂", "🎉", "❤️", "💍", "✈️", "🎄", "🎓", "⭐", "🏆", "🎯", "🚩", "🎮", "👶", "🏠",
];

/// The "new countdown" popover's state.
pub(crate) struct Draft {
    pub(crate) name: Entity<TextInput>,
    kind: CountdownKind,
    pub(crate) date: NaiveDate,
    pub(crate) date_inputs: DateInputs,
    /// The icon (an emoji) and its color, as the original's icon picker sets them.
    icon: Option<String>,
    color: Option<String>,
    /// The original's Repeat for the date.
    pub(crate) repeat: Option<Recurrence>,
    /// The countdown being edited; `None` for a new one.
    editing: Option<Id>,
    /// A birthday's "Show Age".
    show_age: bool,
    /// The row whose dropdown is open, or the icon picker.
    pub(crate) open: Option<DraftRow>,
    /// The original's Reminder: days before the date, at 09:00.
    reminders: Vec<u32>,
    /// Its Custom reminders (days before, time), and the Custom form while open.
    custom_reminders: Vec<(u32, chrono::NaiveTime)>,
    pub(crate) custom: Option<CustomReminder>,
    constant_reminder: bool,
    show_in_list: ShowInList,
    plus_one_day: bool,
    count_up: bool,
    /// The card's picture: the saved one's name, or a new file picked, or nothing.
    background: Option<String>,
    new_background: Option<PathBuf>,
}

/// The Reminder's Custom form: a Day or Week unit, how many in advance, and the time.
pub(crate) struct CustomReminder {
    pub(crate) weeks: bool,
    pub(crate) count: Entity<TextInput>,
    time: Entity<TextInput>,
}

/// The countdown dialog's fields that open a dropdown, as in the original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DraftRow {
    Icon,
    Presets,
    Date,
    DatePart(DateField),
    Reminder,
    Repeat,
    Type,
    CalcMode,
    CountingMode,
    Show,
}

/// The direct date inputs used by all four countdown types.
pub(crate) struct DateInputs {
    pub(crate) fields: [Entity<TextInput>; 3],
    scroll: ScrollHandle,
    _focus_subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DateField {
    Year,
    Month,
    Day,
}

impl DateField {
    const ALL: [Self; 3] = [Self::Year, Self::Month, Self::Day];

    pub(crate) fn ordered(strings: &Strings) -> [Self; 3] {
        match calendar::effective_date_format(strings) {
            DateFormat::MonthDayYear => [Self::Month, Self::Day, Self::Year],
            DateFormat::DayMonthYear => [Self::Day, Self::Month, Self::Year],
            DateFormat::YearMonthDay | DateFormat::IsoYearMonthDay => Self::ALL,
        }
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Year => "countdown-date-year",
            Self::Month => "countdown-date-month",
            Self::Day => "countdown-date-day",
        }
    }

    fn label(self, strings: &Strings) -> &'static str {
        match self {
            Self::Year => strings.year,
            Self::Month => strings.month,
            Self::Day => strings.day,
        }
    }

    fn value(self, date: NaiveDate) -> i32 {
        match self {
            Self::Year => date.year(),
            Self::Month => date.month() as i32,
            Self::Day => date.day() as i32,
        }
    }

    fn text(self, date: NaiveDate) -> String {
        self.value(date).to_string()
    }

    fn options(self, date: NaiveDate) -> std::ops::RangeInclusive<i32> {
        match self {
            Self::Year => 1910..=2099,
            Self::Month => 1..=12,
            Self::Day => 1..=days_in_month(date.year(), date.month()) as i32,
        }
    }

    fn with_value(self, date: NaiveDate, value: i32) -> NaiveDate {
        with_date_part(
            date,
            match self {
                Self::Year => DatePart::Year(value),
                Self::Month => DatePart::Month(value as u32),
                Self::Day => DatePart::Day(value as u32),
            },
        )
    }

    fn parse(self, date: NaiveDate, text: &str) -> Option<NaiveDate> {
        let text = text.trim();
        if text == self.text(date) {
            return Some(date);
        }
        if !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let mut value = text.parse::<i32>().ok()?;
        if self == Self::Year && text.len() == 2 {
            let pivot = Local::now().year().rem_euclid(100) + 10;
            value += if value <= pivot { 2000 } else { 1900 };
        }
        let valid = match self {
            Self::Year => matches!(text.len(), 2 | 4) && (1..=9999).contains(&value),
            Self::Month | Self::Day => text.len() <= 2 && self.options(date).contains(&value),
        };
        valid.then(|| self.with_value(date, value))
    }
}

impl Draft {
    pub(crate) fn set_date(&mut self, date: NaiveDate, cx: &mut Context<CountdownView>) {
        self.date = date;
        for field in DateField::ALL {
            let input = &self.date_inputs.fields[field as usize];
            let text = field.text(date);
            if input.read(cx).text() != text {
                input.update(cx, |input, cx| input.set_text(text, cx));
            }
        }
    }

    fn date_from_fields(&self, cx: &App) -> Option<NaiveDate> {
        let mut date = self.date;
        for field in DateField::ALL {
            let text = self.date_inputs.fields[field as usize].read(cx).text();
            if text != field.text(self.date) {
                date = field.parse(date, text)?;
            }
        }
        Some(date)
    }

    fn date_in_popover(&self) -> bool {
        matches!(self.kind, CountdownKind::Birthday | CountdownKind::Holiday)
    }

    fn close_date_part(&mut self) {
        self.open = self.date_in_popover().then_some(DraftRow::Date);
    }

    pub(crate) fn dismiss_popover(&mut self) -> bool {
        self.custom.take().is_some() || self.open.take().is_some()
    }
}

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

fn render_form_row(label: &'static str, content: AnyElement) -> impl IntoElement {
    h_flex()
        .min_h(px(32.))
        .gap_2()
        .child(div().w(px(80.)).flex_none().child(Label::new(label)))
        .child(div().flex_1().min_w_0().child(content))
}

impl CountdownView {
    fn date_inputs(date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) -> DateInputs {
        let strings = i18n::t(cx);
        let mut focus_subscriptions = Vec::new();
        let fields = DateField::ALL.map(|field| {
            let input = cx.new(|cx| {
                let mut input = TextInput::new(Mode::SingleLine, field.label(strings), cx);
                input.set_text(field.text(date), cx);
                input
            });
            cx.subscribe(&input, |_, _, _: &Changed, cx| cx.notify()).detach();
            cx.subscribe_in(&input, window, move |this, _, _: &Submitted, window, cx| {
                let Some(draft) = this.draft.as_mut() else { return };
                if !Self::commit_date_fields(draft, cx) {
                    return;
                }
                draft.close_date_part();
                let order = DateField::ordered(i18n::t(cx));
                if let Some(next) = order.windows(2).find(|pair| pair[0] == field).map(|pair| pair[1]) {
                    window.focus(&draft.date_inputs.fields[next as usize].focus_handle(cx), cx);
                }
                cx.notify();
            })
            .detach();
            cx.subscribe_in(&input, window, move |this, _, _: &Cancelled, window, cx| {
                let Some(draft) = this.draft.as_mut() else { return };
                draft.set_date(draft.date, cx);
                match draft.open {
                    Some(DraftRow::DatePart(_)) => draft.close_date_part(),
                    _ => {
                        draft.open = None;
                        window.focus(&draft.name.focus_handle(cx), cx);
                    }
                }
                cx.notify();
            })
            .detach();
            focus_subscriptions.push(cx.on_focus(&input.focus_handle(cx), window, |_, _, cx| cx.notify()));
            focus_subscriptions.push(cx.on_blur(&input.focus_handle(cx), window, |this, _, cx| {
                if let Some(draft) = this.draft.as_mut() {
                    // Keep invalid input visible until it is corrected or explicitly cancelled.
                    Self::commit_date_fields(draft, cx);
                    cx.notify();
                }
            }));
            input
        });
        DateInputs {
            fields,
            scroll: ScrollHandle::new(),
            _focus_subscriptions: focus_subscriptions,
        }
    }

    fn commit_date_fields(draft: &mut Draft, cx: &mut Context<Self>) -> bool {
        let Some(date) = draft.date_from_fields(cx) else { return false };
        draft.set_date(date, cx);
        true
    }

    fn open_date_part(draft: &mut Draft, field: DateField) {
        draft.open = Some(DraftRow::DatePart(field));
        let options = field.options(draft.date);
        let value = field.value(draft.date);
        let selected = value.clamp(*options.start(), *options.end()) - *options.start()
            + i32::from(value > *options.end());
        draft.date_inputs.scroll.set_offset(gpui::point(px(0.), px(0.)));
        draft
            .date_inputs
            .scroll
            .scroll_to_top_of_item(selected.saturating_sub(3).max(0) as usize);
    }

    pub fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let name = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.countdown_name, cx));
        cx.subscribe(&name, |this, _, _: &Submitted, cx| this.save_draft(cx)).detach();
        cx.subscribe(&name, |this, _, _: &Cancelled, cx| {
            if let Some(draft) = this.draft.as_mut()
                && draft.dismiss_popover()
            {
                cx.notify();
                return;
            }
            this.draft = None;
            cx.notify();
        })
        .detach();
        window.focus(&name.focus_handle(cx), cx);
        let today = Local::now().date_naive();
        let kind = match self.filter {
            Filter::Kind(kind) => kind,
            Filter::All => CountdownKind::Countdown,
        };
        let date_inputs = Self::date_inputs(today, window, cx);
        self.draft = Some(Draft {
            name,
            kind,
            date: today,
            date_inputs,
            icon: None,
            color: None,
            repeat: None,
            editing: None,
            show_age: false,
            open: None,
            reminders: fern_core::countdown::DEFAULT_REMINDERS.to_vec(),
            custom_reminders: Vec::new(),
            custom: None,
            constant_reminder: false,
            show_in_list: Default::default(),
            plus_one_day: false,
            count_up: false,
            background: None,
            new_background: None,
        });
        cx.notify();
    }

    /// A kind from the "+" menu: a new countdown of that kind.
    pub fn start_new_kind(&mut self, kind: CountdownKind, window: &mut Window, cx: &mut Context<Self>) {
        self.add_menu = false;
        self.start_new(window, cx);
        if let Some(draft) = self.draft.as_mut() {
            draft.kind = kind;
            // As in the original, a new birthday or holiday repeats yearly.
            if matches!(kind, CountdownKind::Birthday | CountdownKind::Holiday) {
                draft.repeat = Some(Recurrence::new(Frequency::Yearly, 1));
            }
        }
        cx.notify();
    }

    /// "Edit" in a card's "…" menu: the countdown's popover, filled in.
    pub fn start_edit(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            self.reload(cx);
        }
        let Some(countdown) = self.countdowns.iter().find(|c| c.id == id).cloned() else {
            return;
        };
        self.card_menu = None;
        self.start_new(window, cx);
        if let Some(draft) = self.draft.as_mut() {
            draft.name.update(cx, |input, cx| input.set_text(countdown.name.clone(), cx));
            draft.kind = countdown.kind;
            draft.set_date(countdown.date, cx);
            draft.icon = countdown.icon;
            draft.color = countdown.color;
            draft.repeat = countdown.repeat;
            draft.show_age = countdown.show_age;
            draft.reminders = countdown.reminders.clone();
            draft.custom_reminders = countdown.custom_reminders.clone();
            draft.constant_reminder = countdown.constant_reminder;
            draft.show_in_list = countdown.show_in_list;
            draft.plus_one_day = countdown.plus_one_day;
            draft.count_up = countdown.count_up;
            draft.background = countdown.background.clone();
            draft.editing = Some(id);
        }
        cx.notify();
    }

    /// The Background row's "Choose a picture…": the system's file dialog.
    fn choose_background(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Some(path) = chosen
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths| paths.into_iter().next())
            else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(draft) = this.draft.as_mut() {
                    draft.background = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    draft.new_background = Some(path);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Reminder's Custom: its form, starting as the original's at 1 day and 09:00.
    pub fn open_custom_reminder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = |text: &str, cx: &mut Context<Self>| {
            let text = text.to_owned();
            cx.new(|cx| {
                let mut input = TextInput::new(Mode::SingleLine, "", cx);
                input.set_text(text, cx);
                input
            })
        };
        let count = field("1", cx);
        let time = field(&calendar::format_time(fern_core::countdown::reminder_time(), i18n::t(cx)), cx);
        for input in [&count, &time] {
            cx.subscribe(input, |_, _, _: &Changed, cx| cx.notify()).detach();
            cx.subscribe(input, |this, _, _: &Submitted, cx| this.confirm_custom_reminder(cx))
                .detach();
            cx.subscribe(input, |this, _, _: &Cancelled, cx| {
                if let Some(draft) = this.draft.as_mut() {
                    draft.custom = None;
                }
                cx.notify();
            })
            .detach();
        }
        window.focus(&count.focus_handle(cx), cx);
        if let Some(draft) = self.draft.as_mut() {
            draft.custom = Some(CustomReminder {
                weeks: false,
                count,
                time,
            });
        }
        cx.notify();
    }

    /// The Custom form's OK: adds the reminder when the count and time read.
    pub(crate) fn confirm_custom_reminder(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_mut() else { return };
        let Some(custom) = draft.custom.as_ref() else { return };
        let count: Option<u32> = custom.count.read(cx).text().trim().parse().ok();
        let time = calendar::parse_time(custom.time.read(cx).text());
        if let (Some(count), Some(time)) = (count, time) {
            let days = if custom.weeks { count.saturating_mul(7) } else { count };
            if !draft.custom_reminders.contains(&(days, time)) {
                draft.custom_reminders.push((days, time));
                draft.custom_reminders.sort();
            }
            draft.custom = None;
        }
        cx.notify();
    }

    pub(crate) fn save_draft(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_ref() else { return };
        let name = draft.name.read(cx).text().trim().to_owned();
        if name.is_empty() {
            cx.notify();
            return;
        }
        let Some(date) = draft.date_from_fields(cx) else {
            if let Some(draft) = self.draft.as_mut() {
                if draft.date_in_popover() {
                    draft.open = Some(DraftRow::Date);
                }
                cx.notify();
            }
            return;
        };
        let Some(draft) = self.draft.take() else { return };
        let mut mutated = false;
        let added = match draft.editing {
            Some(id) => self
                .store
                .update_countdown(id, &name, date, draft.kind, draft.repeat.as_ref())
                .map(|()| {
                    mutated = true;
                    id
                }),
            None => self.store.add_countdown(&name, date, draft.kind).and_then(|countdown| {
                mutated = true;
                self.store
                    .update_countdown(countdown.id, &name, date, draft.kind, draft.repeat.as_ref())
                    .map(|()| countdown.id)
            }),
        }
        .and_then(|id| {
            self.store
                .set_countdown_look(id, draft.icon.as_deref(), draft.color.as_deref())?;
            self.store
                .set_countdown_show_age(id, draft.kind == CountdownKind::Birthday && draft.show_age)?;
            self.store.set_countdown_reminders(
                id,
                &draft.reminders,
                &draft.custom_reminders,
                draft.constant_reminder,
            )?;
            // A new picture is copied in; one taken off is cleared.
            if draft.new_background.is_some() || draft.background.is_none() {
                let dir = attachments_dir()?;
                self.store
                    .set_countdown_background(id, draft.new_background.as_deref(), &dir)?;
            }
            self.store.set_countdown_display(
                id,
                draft.show_in_list,
                draft.plus_one_day,
                draft.kind == CountdownKind::Birthday && draft.count_up,
            )
        });
        if let Err(err) = added {
            eprintln!("failed to add the countdown: {err:#}");
        }
        if mutated {
            data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    fn draft_listener<E>(
        &self,
        cx: &Context<Self>,
        f: impl Fn(&mut Draft) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _, _, cx| {
            if let Some(draft) = this.draft.as_mut() {
                f(draft);
            }
            cx.notify();
        })
    }

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
        let fields = h_flex().gap_2().w_full().children(DateField::ordered(strings).map(|field| {
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

    fn render_date_editor(&self, draft: &Draft, window: &Window, cx: &Context<Self>) -> Div {
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
                    .child(div().flex_1().min_w_0().child(Label::new(format_date(draft.date, strings)).truncate()))
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
                                let field = DateField::ordered(i18n::t(cx))[0];
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

    fn render_repeat_menu(&self, draft: &Draft, cx: &Context<Self>) -> (String, AnyElement) {
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

    fn render_type_menu(&self, kind: CountdownKind, cx: &Context<Self>) -> AnyElement {
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
                                .on_click(self.draft_listener(cx, |d| d.custom = None)),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            Button::new("countdown-custom-ok", strings.ok)
                                .variant(ButtonVariant::Primary)
                                .size(ButtonSize::Sm)
                                .full_width()
                                .on_click(cx.listener(|this, _, _, cx| this.confirm_custom_reminder(cx))),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_reminder_menu(&self, draft: &Draft, cx: &Context<Self>) -> (String, AnyElement) {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let custom_label = |days: u32, time: chrono::NaiveTime| {
            format!(
                "{} ({})",
                strings.countdown_custom_label.replace("{n}", &days.to_string()),
                calendar::format_time(time, strings)
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

        let time = calendar::format_time(fern_core::countdown::reminder_time(), strings);
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

    fn render_calc_menu(&self, plus_one: bool, cx: &Context<Self>) -> (String, AnyElement) {
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

    fn render_counting_menu(&self, count_up: bool, cx: &Context<Self>) -> AnyElement {
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

    fn render_show_menu(&self, show: ShowInList, cx: &Context<Self>) -> (String, AnyElement) {
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

    fn render_icon_menu(&self, draft: &Draft, cx: &Context<Self>) -> AnyElement {
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

    fn render_presets_popover(&self, kind: CountdownKind, cx: &Context<Self>) -> PopoverAttachment {
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

    fn render_draft_dropdown(
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

    /// The original's add / edit dialog: the icon and name on top, then a label on the left and
    /// the value in a field with a ▾ for Date, Repeat and Type, "Show Age" for a birthday, and
    /// below a divider Cancel and Save.
    pub(crate) fn render_draft(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = self.draft.as_ref() else {
            return div().into_any_element();
        };
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let (kind, open) = (draft.kind, draft.open);

        let (repeat_text, repeat_menu) = self.render_repeat_menu(draft, cx);
        let type_menu = self.render_type_menu(kind, cx);
        let (reminder_text, reminder_menu) = self.render_reminder_menu(draft, cx);
        let (calc_text, calc_menu) = self.render_calc_menu(draft.plus_one_day, cx);
        let counting_menu = self.render_counting_menu(draft.count_up, cx);
        let (show_text, show_menu) = self.render_show_menu(draft.show_in_list, cx);
        let icon_menu = self.render_icon_menu(draft, cx);

        let date_row = render_form_row(
            strings.filter_date,
            self.render_date_editor(draft, window, cx).into_any_element(),
        );
        let reminder_row = render_form_row(
            strings.reminder,
            self.render_draft_dropdown(DraftRow::Reminder, open, reminder_text, reminder_menu, cx),
        );
        let repeat_row = render_form_row(
            strings.repeat,
            self.render_draft_dropdown(DraftRow::Repeat, open, repeat_text, repeat_menu, cx),
        );
        // As in the original, a birthday has Counting Mode where the others have Day Calculation Mode.
        // A holiday has neither, as in the original.
        let calc_row = match kind {
            CountdownKind::Birthday => Some(render_form_row(
                strings.countdown_counting_mode,
                self.render_draft_dropdown(
                    DraftRow::CountingMode,
                    open,
                    strings.countdown_counting_options[draft.count_up as usize].to_owned(),
                    counting_menu,
                    cx,
                ),
            )),
            CountdownKind::Holiday => None,
            _ => Some(render_form_row(
                strings.countdown_calc_mode,
                self.render_draft_dropdown(DraftRow::CalcMode, open, calc_text, calc_menu, cx),
            )),
        };
        let show_row = render_form_row(
            strings.countdown_show,
            self.render_draft_dropdown(DraftRow::Show, open, show_text, show_menu, cx),
        );
        let type_row = render_form_row(
            strings.filter_type,
            self.render_draft_dropdown(
                DraftRow::Type,
                open,
                kind_label(kind, strings).to_owned(),
                type_menu,
                cx,
            ),
        );

        let icon_open = open == Some(DraftRow::Icon);
        let icon = div()
            .relative()
            .child(
                div()
                    .id("countdown-icon")
                    .size(px(35.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(lists_panel::parse_color(draft.color.as_deref())
                        .unwrap_or(theme.colors().accent)
                        .opacity(0.35))
                    .child(match &draft.icon {
                        Some(icon) => div().text_size(px(18.)).child(icon.clone()).into_any_element(),
                        None => Icon::new(kind_icon(kind)).color(Color::Default).into_any_element(),
                    })
                    .on_click(self.draft_listener(cx, |d| {
                        d.open = if d.open == Some(DraftRow::Icon) {
                            None
                        } else {
                            Some(DraftRow::Icon)
                        };
                    })),
            )
            .when(icon_open, |el| {
                el.child(PopoverAttachment::new(
                    Placement::BelowLeft,
                    popover(cx)
                        .w(px(300.))
                        .p_3()
                        .on_mouse_down_out(self.draft_listener(cx, |d| d.open = None))
                        .child(icon_menu),
                ))
            });

        let show_age = draft.show_age;
        let presets = (open == Some(DraftRow::Presets)).then(|| self.render_presets_popover(kind, cx));

        Modal::new("countdown-modal")
            .width(px(430.))
            .padding(px(16.))
            .gap(px(8.))
            .child(
                h_flex()
                    .child(
                        div().flex_1().child(
                            Label::new(if draft.editing.is_some() {
                                strings.countdown_edit
                            } else {
                                strings.countdown_new
                            })
                            .weight(FontWeight::BOLD),
                        ),
                    )
                    .child(
                        IconButton::new("countdown-x", IconName::X)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.draft = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(80.)).flex_none().child(icon))
                    .child(
                        h_flex()
                            .flex_1()
                            .h(px(32.))
                            .pl_3()
                            .pr_1()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .child(div().flex_1().child(draft.name.clone()))
                            // The original's bookmark: ready-made entries to start from, for the
                            // kinds that have them.
                            .when(!fern_core::countdown::presets(kind).is_empty(), |el| {
                                el.child(
                                    div()
                                        .relative()
                                        .child(
                                            IconButton::new("countdown-presets", IconName::Bookmark)
                                                .icon_color(Color::Muted)
                                                .tooltip(Tooltip::text(strings.templates))
                                                .on_click(self.draft_listener(cx, |d| {
                                                    d.open = if d.open == Some(DraftRow::Presets) {
                                                        None
                                                    } else {
                                                        Some(DraftRow::Presets)
                                                    };
                                                })),
                                        )
                                        .children(presets),
                                )
                            }),
                    ),
            )
            .child(date_row)
            .child(reminder_row)
            .child(repeat_row)
            .child(type_row)
            .children(calc_row)
            .child(show_row)
            // The card's picture: the file's name with "×" to take it off, or the chooser.
            .child(render_form_row(
                strings.countdown_background,
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id("countdown-background")
                            .flex_1()
                            .min_w_0()
                            .h(px(32.))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .cursor_pointer()
                            .child(
                                Label::new(
                                    draft
                                        .background
                                        .clone()
                                        .unwrap_or_else(|| strings.countdown_choose_image.to_owned()),
                                )
                                .truncate()
                                .color(if draft.background.is_some() {
                                    Color::Default
                                } else {
                                    Color::Muted
                                }),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.choose_background(cx))),
                    )
                    .when(draft.background.is_some(), |el| {
                        el.child(
                            IconButton::new("countdown-background-clear", IconName::X)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text(strings.clear))
                                .on_click(self.draft_listener(cx, |d| {
                                    d.background = None;
                                    d.new_background = None;
                                })),
                        )
                    })
                    .into_any_element(),
            ))
            .child(div().mt_1().h(px(1.)).bg(theme.colors().border))
            // The original's "Show Age", for a birthday.
            .when(kind == CountdownKind::Birthday, |el| {
                el.child(
                    h_flex()
                        .id("countdown-show-age")
                        .gap_2()
                        .cursor_pointer()
                        .child(Switch::new().toggle_state(show_age))
                        .child(Label::new(strings.countdown_show_age))
                        .on_click(self.draft_listener(cx, |d| d.show_age = !d.show_age)),
                )
            })
            .child(
                h_flex()
                    .pt_2()
                    .gap(px(10.))
                    .justify_end()
                    .child(
                        Button::new("countdown-cancel", strings.cancel)
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.draft = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .debug_selector(|| "countdown-save".to_owned())
                            .child(
                                Button::new("countdown-save", strings.save)
                                    .variant(ButtonVariant::Primary)
                                    .on_click(cx.listener(|this, _, _, cx| this.save_draft(cx))),
                            ),
                    ),
            )
            .into_any_element()
    }
}
