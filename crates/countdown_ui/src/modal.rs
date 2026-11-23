use std::path::PathBuf;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::countdown::{DatePart, ShowInList, days_in_month, with_date_part};
use fern_core::{CountdownKind, DateFormat, Frequency, Id, Recurrence};
use gpui::{Entity, Focusable as _, FontWeight, ScrollHandle, Subscription};
use ui::prelude::*;
use ui::{Button, ButtonVariant, Modal, Placement, PopoverAttachment, popover};

use desktop_common::data_version::data_changed;
use desktop_common::paths::attachments_dir;
use desktop_common::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};
use desktop_common::{calendar, colors as lists_panel};
use fern_i18n::{self as i18n, Strings};

use crate::{CountdownView, Filter, PendingFocus, kind_icon, kind_label};

/// The "new countdown" popover's state.
pub(crate) struct Draft {
    pub(crate) name: Entity<TextInput>,
    pub(crate) kind: CountdownKind,
    pub(crate) date: NaiveDate,
    pub(crate) date_inputs: DateInputs,
    /// The icon (an emoji) and its color, as the original's icon picker sets them.
    pub(crate) icon: Option<String>,
    pub(crate) color: Option<String>,
    /// The original's Repeat for the date.
    pub(crate) repeat: Option<Recurrence>,
    /// The countdown being edited; `None` for a new one.
    editing: Option<Id>,
    /// A birthday's "Show Age".
    show_age: bool,
    /// The row whose dropdown is open, or the icon picker.
    pub(crate) open: Option<DraftRow>,
    /// The original's Reminder: days before the date, at 09:00.
    pub(crate) reminders: Vec<u32>,
    /// Its Custom reminders (days before, time), and the Custom form while open.
    pub(crate) custom_reminders: Vec<(u32, chrono::NaiveTime)>,
    pub(crate) custom: Option<CustomReminder>,
    pub(crate) constant_reminder: bool,
    pub(crate) show_in_list: ShowInList,
    pub(crate) plus_one_day: bool,
    pub(crate) count_up: bool,
    /// The card's picture: the saved one's name, or a new file picked, or nothing.
    background: Option<String>,
    new_background: Option<PathBuf>,
}

/// The Reminder's Custom form: a Day or Week unit, how many in advance, and the time.
pub(crate) struct CustomReminder {
    pub(crate) weeks: bool,
    pub(crate) count: Entity<TextInput>,
    pub(crate) time: Entity<TextInput>,
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
    pub(crate) scroll: ScrollHandle,
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

    pub(crate) fn label(self, strings: &Strings) -> &'static str {
        match self {
            Self::Year => strings.year,
            Self::Month => strings.month,
            Self::Day => strings.day,
        }
    }

    pub(crate) fn value(self, date: NaiveDate) -> i32 {
        match self {
            Self::Year => date.year(),
            Self::Month => date.month() as i32,
            Self::Day => date.day() as i32,
        }
    }

    fn text(self, date: NaiveDate) -> String {
        self.value(date).to_string()
    }

    pub(crate) fn options(self, date: NaiveDate) -> std::ops::RangeInclusive<i32> {
        match self {
            Self::Year => 1910..=2099,
            Self::Month => 1..=12,
            Self::Day => 1..=days_in_month(date.year(), date.month()) as i32,
        }
    }

    pub(crate) fn with_value(self, date: NaiveDate, value: i32) -> NaiveDate {
        with_date_part(
            date,
            match self {
                Self::Year => DatePart::Year(value),
                Self::Month => DatePart::Month(value as u32),
                Self::Day => DatePart::Day(value as u32),
            },
        )
    }

    pub(crate) fn parse(self, date: NaiveDate, text: &str) -> Option<NaiveDate> {
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

    pub(crate) fn date_from_fields(&self, cx: &App) -> Option<NaiveDate> {
        let mut date = self.date;
        for field in DateField::ALL {
            let text = self.date_inputs.fields[field as usize].read(cx).text();
            if text != field.text(self.date) {
                date = field.parse(date, text)?;
            }
        }
        Some(date)
    }

    pub(crate) fn date_in_popover(&self) -> bool {
        matches!(self.kind, CountdownKind::Birthday | CountdownKind::Holiday)
    }

    pub(crate) fn close_date_part(&mut self) {
        self.open = self.date_in_popover().then_some(DraftRow::Date);
    }

    pub(crate) fn dismiss_popover(&mut self) -> bool {
        self.custom.take().is_some() || self.open.take().is_some()
    }
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
                        this.pending_focus = None;
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

    pub(crate) fn open_date_part(draft: &mut Draft, field: DateField) {
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

    pub(crate) fn close_draft(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.draft = None;
        if let Some(window) = window {
            self.pending_focus = None;
            window.focus(&self.focus_handle, cx);
        } else {
            self.pending_focus = Some(PendingFocus::View);
        }
        cx.notify();
    }

    pub fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let name = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.countdown_name, cx));
        cx.subscribe_in(&name, window, |this, _, _: &Submitted, window, cx| {
            this.save_draft(cx);
            this.apply_pending_focus(window, cx);
        })
        .detach();
        cx.subscribe_in(&name, window, |this, _, _: &Cancelled, window, cx| {
            if let Some(draft) = this.draft.as_mut()
                && draft.dismiss_popover()
            {
                this.pending_focus = None;
                window.focus(&draft.name.focus_handle(cx), cx);
                cx.notify();
                return;
            }
            this.close_draft(Some(window), cx);
        })
        .detach();
        self.pending_focus = None;
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

    pub(crate) fn close_custom_reminder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = self.draft.as_mut() {
            draft.custom = None;
            self.pending_focus = None;
            window.focus(&draft.name.focus_handle(cx), cx);
        }
        cx.notify();
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
            cx.subscribe_in(input, window, |this, _, _: &Submitted, window, cx| {
                this.confirm_custom_reminder(cx);
                this.apply_pending_focus(window, cx);
            })
            .detach();
            cx.subscribe_in(input, window, |this, _, _: &Cancelled, window, cx| {
                this.close_custom_reminder(window, cx);
            })
            .detach();
        }
        self.pending_focus = None;
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
            self.pending_focus = Some(PendingFocus::DraftName);
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
        self.pending_focus = Some(PendingFocus::View);
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

    pub(crate) fn draft_listener<E>(
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_draft(Some(window), cx);
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_draft(Some(window), cx);
                            })),
                    )
                    .child(
                        div()
                            .debug_selector(|| "countdown-save".to_owned())
                            .child(
                                Button::new("countdown-save", strings.save)
                                    .variant(ButtonVariant::Primary)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_draft(cx);
                                        this.apply_pending_focus(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
