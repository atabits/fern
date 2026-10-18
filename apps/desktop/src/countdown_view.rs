//! The Countdown section, matched to TickTick's: filter tabs and a grid of cards showing the time
//! until (or since) a date; clicking the numbers switches between Y/M/D and plain days.

use std::collections::HashSet;
use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Countdown, CountdownKind, Id, Store, span};
use gpui::{Entity, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::calendar;
use crate::i18n::{self, Strings};
use crate::text_input::{Cancelled, Mode, Submitted, TextInput};

const CARD_WIDTH: f32 = 281.;
const CARD_HEIGHT: f32 = 175.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    All,
    Kind(CountdownKind),
}

/// The "new countdown" popover's state.
struct Draft {
    name: Entity<TextInput>,
    kind: CountdownKind,
    date: NaiveDate,
    month: NaiveDate,
    /// The icon (an emoji) and its color, as the original's icon picker sets them.
    icon: Option<String>,
    color: Option<String>,
    /// The original's Repeat for the date.
    repeat: Option<fern_core::Recurrence>,
    /// The countdown being edited; `None` for a new one.
    editing: Option<Id>,
    /// A birthday's "Show Age".
    show_age: bool,
    /// The row whose dropdown is open, or the icon picker.
    open: Option<DraftRow>,
    /// The original's Reminder: days before the date, at 09:00.
    reminders: Vec<u32>,
    constant_reminder: bool,
}

/// The countdown dialog's fields that open a dropdown, as in the original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DraftRow {
    Icon,
    Date,
    Reminder,
    Repeat,
    Type,
}

/// Emoji offered for countdowns; the original's illustrated icon set is its own artwork.
const COUNTDOWN_EMOJI: [&str; 14] = ["🎂", "🎉", "❤️", "💍", "✈️", "🎄", "🎓", "⭐", "🏆", "🎯", "🚩", "🎮", "👶", "🏠"];

pub struct CountdownView {
    store: Rc<Store>,
    countdowns: Vec<Countdown>,
    filter: Filter,
    /// Cards switched to the "days only" format.
    days_only: HashSet<Id>,
    draft: Option<Draft>,
    /// The card whose "…" menu is open.
    card_menu: Option<Id>,
    /// The "+" menu of kinds, as in the original.
    add_menu: bool,
}

fn kind_icon(kind: CountdownKind) -> IconName {
    match kind {
        CountdownKind::Countdown => IconName::Hourglass,
        CountdownKind::Anniversary => IconName::Heart,
        CountdownKind::Birthday => IconName::Cake,
        CountdownKind::Holiday => IconName::Gift,
    }
}

fn kind_label(kind: CountdownKind, strings: &Strings) -> &'static str {
    match kind {
        CountdownKind::Countdown => strings.countdown_kind_countdown,
        CountdownKind::Anniversary => strings.countdown_kind_anniversary,
        CountdownKind::Birthday => strings.countdown_kind_birthday,
        CountdownKind::Holiday => strings.countdown_kind_holiday,
    }
}

fn format_date(date: NaiveDate, strings: &Strings) -> String {
    crate::calendar::format_date(date, strings)
}

/// The color a countdown's dot and icon take: its own, or the accent.
pub fn countdown_color(color: Option<&str>, cx: &App) -> gpui::Hsla {
    crate::lists_panel::parse_color(color).unwrap_or(cx.theme().colors().accent)
}

/// A countdown's round icon: its emoji on its color, or the kind's icon.
fn badge(countdown: &Countdown, color: gpui::Hsla, cx: &App) -> gpui::Div {
    let fill = crate::lists_panel::parse_color(countdown.color.as_deref());
    div()
        .size(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(fill.map_or(color.opacity(0.25), |c| c.opacity(0.6)))
        .map(|el| match &countdown.icon {
            Some(icon) => el.text_size(px(11.)).child(icon.clone()),
            None => el.child(
                Icon::new(kind_icon(countdown.kind))
                    .size(IconSize::XSmall)
                    .color(Color::Custom(if fill.is_some() { cx.theme().colors().text } else { color })),
            ),
        })
}

/// The parts of a countdown card: its color, the "1M 1D" numbers and "Days until …".
struct Face {
    color: gpui::Hsla,
    numbers: gpui::Div,
    subtitle: String,
}

fn face(countdown: &Countdown, today: NaiveDate, days_only: bool, cx: &App) -> Face {
    let strings = i18n::t(cx);
    let theme = cx.theme();
    // A repeating countdown counts to the next time its date comes back.
    let date = countdown.next_date(today);
    let span = span(today, date);
    let color = if span.past { theme.status().warning } else { theme.colors().accent };
    let number = |value: u32, unit: &str| {
        h_flex()
            .items_baseline()
            .child(div().text_size(px(34.)).text_color(color).child(value.to_string()))
            .child(div().pl_0p5().pr_1().text_size(px(13.)).text_color(color).child(unit.to_owned()))
    };
    let [y, m, d] = strings.unit_letters;
    let numbers = if days_only {
        h_flex().child(number(span.total_days, d))
    } else {
        h_flex()
            .when(span.years > 0, |el| el.child(number(span.years, y)))
            .when(span.months > 0, |el| el.child(number(span.months, m)))
            .when(span.days > 0 || (span.years == 0 && span.months == 0), |el| el.child(number(span.days, d)))
    };
    let mut subtitle = format!(
        "{} {}",
        if span.past { strings.countdown_days_since } else { strings.countdown_days_until },
        format_date(date, strings)
    );
    // A birthday with "Show Age" says the age turned on that day.
    if let Some(age) = countdown.age(today).filter(|_| countdown.show_age) {
        subtitle = format!("{} · {subtitle}", strings.countdown_turns.replace("{n}", &age.to_string()));
    }
    Face { color, numbers, subtitle }
}

/// A countdown as the original shows it when clicked in the calendar's Agenda: the icon and
/// name, the time left and "Days until …", counted to `date` (this year's date for yearly ones).
pub fn render_summary(countdown: &Countdown, date: NaiveDate, cx: &App) -> AnyElement {
    let today = Local::now().date_naive();
    let shown = Countdown { date, repeat: None, ..countdown.clone() };
    let Face { color, numbers, subtitle } = face(&shown, today, false, cx);
    v_flex()
        .items_center()
        .gap_1()
        .py_3()
        .px_6()
        .child(
            h_flex()
                .gap_1p5()
                .child(
                    badge(countdown, color, cx),
                )
                .child(Label::new(countdown.name.clone()).weight(FontWeight::SEMIBOLD)),
        )
        .child(numbers)
        .child(Label::new(subtitle).size(LabelSize::XSmall).color(Color::Muted))
        .into_any_element()
}

impl CountdownView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let mut this = Self { store, countdowns: Vec::new(), filter: Filter::All, days_only: HashSet::new(), draft: None, card_menu: None, add_menu: false };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.countdowns = self.store.countdowns().unwrap_or_default();
        cx.notify();
    }

    pub fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let name = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.countdown_name, cx));
        cx.subscribe(&name, |this, _, _: &Submitted, cx| this.save_draft(cx)).detach();
        cx.subscribe(&name, |this, _, _: &Cancelled, cx| {
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
        self.draft = Some(Draft { name, kind, date: today, month: calendar::month_start(today), icon: None, color: None, repeat: None, editing: None, show_age: false, open: None, reminders: fern_core::countdown::DEFAULT_REMINDERS.to_vec(), constant_reminder: false });
        cx.notify();
    }

    /// A kind from the "+" menu: a new countdown of that kind.
    pub fn start_new_kind(&mut self, kind: CountdownKind, window: &mut Window, cx: &mut Context<Self>) {
        self.add_menu = false;
        self.start_new(window, cx);
        if let Some(draft) = self.draft.as_mut() {
            draft.kind = kind;
            // As in the original, a new birthday repeats yearly.
            if kind == CountdownKind::Birthday {
                draft.repeat = Some(fern_core::Recurrence::new(fern_core::Frequency::Yearly, 1));
            }
        }
        cx.notify();
    }

    /// The new countdown's Reminder dropdown, for screenshots.
    #[cfg(debug_assertions)]
    pub fn dev_open_reminder(&mut self, cx: &mut Context<Self>) {
        if let Some(draft) = self.draft.as_mut() {
            draft.open = Some(DraftRow::Reminder);
        }
        cx.notify();
    }

    /// Opens the "+" menu from the debug-build dev state (`countdown-add-menu`).
    #[cfg(debug_assertions)]
    pub fn dev_add_menu(&mut self, cx: &mut Context<Self>) {
        self.add_menu = true;
        cx.notify();
    }

    /// "Edit" in a card's "…" menu: the countdown's popover, filled in.
    pub fn start_edit(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(countdown) = self.countdowns.iter().find(|c| c.id == id).cloned() else { return };
        self.card_menu = None;
        self.start_new(window, cx);
        if let Some(draft) = self.draft.as_mut() {
            draft.name.update(cx, |input, cx| input.set_text(countdown.name.clone(), cx));
            draft.kind = countdown.kind;
            draft.date = countdown.date;
            draft.month = calendar::month_start(countdown.date);
            draft.icon = countdown.icon;
            draft.color = countdown.color;
            draft.repeat = countdown.repeat;
            draft.show_age = countdown.show_age;
            draft.reminders = countdown.reminders.clone();
            draft.constant_reminder = countdown.constant_reminder;
            draft.editing = Some(id);
        }
        cx.notify();
    }

    /// `menu` opens the card's "…" menu, `edit` its edit popover, with a monthly repeat.
    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, name: &str, edit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.countdowns.iter().find(|c| c.name.contains(name)).map(|c| c.id) else { return };
        if edit {
            self.start_edit(id, window, cx);
            if let Some(draft) = self.draft.as_mut() {
                draft.repeat = Some(fern_core::Recurrence::new(fern_core::Frequency::Monthly, 1));
            }
        } else {
            self.card_menu = Some(id);
        }
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub fn dev_add(&mut self, name: &str, date: NaiveDate, kind: CountdownKind, look: Option<(&str, &str)>, cx: &mut Context<Self>) {
        if let Ok(countdown) = self.store.add_countdown(name, date, kind)
            && let Some((icon, color)) = look
        {
            self.store.set_countdown_look(countdown.id, Some(icon), (!color.is_empty()).then_some(color)).ok();
        }
        self.reload(cx);
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.take() else { return };
        let name = draft.name.read(cx).text().to_owned();
        let added = match draft.editing {
            Some(id) => self.store.update_countdown(id, &name, draft.date, draft.kind, draft.repeat.as_ref()).map(|()| id),
            None => self.store.add_countdown(&name, draft.date, draft.kind).and_then(|countdown| {
                self.store.update_countdown(countdown.id, &name, draft.date, draft.kind, draft.repeat.as_ref()).map(|()| countdown.id)
            }),
        }
        .and_then(|id| {
            self.store.set_countdown_look(id, draft.icon.as_deref(), draft.color.as_deref())?;
            self.store.set_countdown_show_age(id, draft.kind == CountdownKind::Birthday && draft.show_age)?;
            self.store.set_countdown_reminders(id, &draft.reminders, draft.constant_reminder)
        });
        if let Err(err) = added {
            eprintln!("failed to add the countdown: {err:#}");
        }
        self.reload(cx);
    }

    fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.delete_countdown(id) {
            eprintln!("failed to delete the countdown: {err:#}");
        }
        self.reload(cx);
    }

    fn toggle_format(&mut self, id: Id, cx: &mut Context<Self>) {
        if !self.days_only.remove(&id) {
            self.days_only.insert(id);
        }
        cx.notify();
    }

    fn render_card(&self, countdown: &Countdown, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let id = countdown.id;
        let group = SharedString::from(format!("countdown-{id}"));
        let Face { color, numbers, subtitle } = face(countdown, today, self.days_only.contains(&id), cx);

        v_flex()
            .id(group.clone())
            .group(group.clone())
            .relative()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .flex_none()
            .items_center()
            .justify_center()
            .gap_1()
            .rounded(px(8.))
            .bg(theme.colors().surface_background)
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        badge(countdown, color, cx),
                    )
                    .child(Label::new(countdown.name.clone()).weight(FontWeight::SEMIBOLD)),
            )
            .child(
                div()
                    .id("countdown-numbers")
                    .cursor_pointer()
                    .tooltip(Tooltip::text(strings.countdown_switch_hint))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_format(id, cx)))
                    .child(numbers),
            )
            .child(Label::new(subtitle).size(LabelSize::XSmall).color(Color::Muted))
            // The original's pin, beside the "…": a pinned card stays first and shows it.
            .child({
                let pinned = countdown.pinned_at.is_some();
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(32.))
                    .opacity(if pinned { 1. } else { 0. })
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .child(
                        IconButton::new("countdown-pin", IconName::Pin)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .icon_color(if pinned { Color::Accent } else { Color::Muted })
                            .tooltip(Tooltip::text(if pinned { strings.unpin } else { strings.pin }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Err(err) = this.store.set_countdown_pinned(id, !pinned) {
                                    eprintln!("failed to pin the countdown: {err:#}");
                                }
                                this.reload(cx);
                            })),
                    )
            })
            .child(
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(6.))
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .when(self.card_menu == Some(id), |el| el.opacity(1.))
                    .child(
                        // The original's "…" on a hovered card: Edit and Delete.
                        IconButton::new("countdown-more", IconName::Ellipsis)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.card_menu = if this.card_menu == Some(id) { None } else { Some(id) };
                                cx.notify();
                            })),
                    )
                    .when(self.card_menu == Some(id), |el| {
                        el.child(PopoverAttachment::new(
                            Placement::BelowRight,
                            popover(cx)
                                .w(px(150.))
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.card_menu = None;
                                    cx.notify();
                                }))
                                .child(
                                    ListItem::new("countdown-edit")
                                        .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                                        .child(Label::new(strings.edit))
                                        .on_click(cx.listener(move |this, _, window, cx| this.start_edit(id, window, cx))),
                                )
                                .child(
                                    ListItem::new("countdown-delete")
                                        .start_slot(Icon::new(IconName::Trash).color(Color::Muted))
                                        .child(Label::new(strings.delete))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.card_menu = None;
                                            this.delete(id, cx)
                                        })),
                                )
                                .into_any_element(),
                        ))
                    }),
            )
            .into_any_element()
    }

    /// The original's add / edit dialog: the icon and name on top, then a label on the left and
    /// the value in a field with a ▾ for Date, Repeat and Type, "Show Age" for a birthday, and
    /// below a divider Cancel and Save.
    fn render_draft(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = self.draft.as_ref() else { return div().into_any_element() };
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let (selected, month, kind, open) = (draft.date, draft.month, draft.kind, draft.open);
        let set = |f: Box<dyn Fn(&mut Draft)>| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let Some(draft) = this.draft.as_mut() {
                    f(draft);
                }
                cx.notify();
            })
        };

        // The Date dropdown: a month to pick the day in.
        let days: Vec<AnyElement> = calendar::month_grid(month)
            .chunks(7)
            .map(|week| {
                h_flex()
                    .children(week.iter().map(|&date| {
                        let is_selected = date == selected;
                        div().flex_1().flex().justify_center().child(
                            div()
                                .id(SharedString::from(format!("countdown-day-{date}")))
                                .size(px(26.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .when(is_selected, |el| el.bg(theme.colors().accent))
                                .when(!is_selected, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                                .child(Label::new(date.day().to_string()).size(LabelSize::Small).color(
                                    if is_selected || date.month() == month.month() {
                                        if date == today && !is_selected { Color::Accent } else { Color::Default }
                                    } else {
                                        Color::Muted
                                    },
                                ))
                                .on_click(set(Box::new(move |d| {
                                    d.date = date;
                                    d.open = None;
                                }))),
                        )
                    }))
                    .into_any_element()
            })
            .collect();
        let shift = |delta: i32| set(Box::new(move |d| d.month = calendar::add_months(d.month, delta)));
        let date_menu = v_flex()
            .gap_1()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(calendar::month_title(month, strings)).size(LabelSize::Small)))
                    .child(IconButton::new("countdown-prev", IconName::ChevronLeft).size(px(22.)).icon_size(IconSize::Small).on_click(shift(-1)))
                    .child(IconButton::new("countdown-next", IconName::ChevronRight).size(px(22.)).icon_size(IconSize::Small).on_click(shift(1))),
            )
            .child(h_flex().children(crate::calendar::weekday_labels(&strings.weekdays_narrow).iter().map(|day| {
                div().flex_1().flex().justify_center().child(Label::new(*day).size(LabelSize::XSmall).color(Color::Muted))
            })))
            .child(v_flex().children(days))
            .into_any_element();

        // The Repeat dropdown: the original's None, Daily, Weekly, Monthly, Yearly.
        use fern_core::Frequency;
        let repeats = [
            (None, strings.none_option),
            (Some(Frequency::Daily), strings.repeat_daily),
            (Some(Frequency::Weekly), strings.repeat_weekly),
            (Some(Frequency::Monthly), strings.repeat_monthly),
            (Some(Frequency::Yearly), strings.repeat_yearly),
        ];
        let current = draft.repeat.as_ref().map(|r| r.frequency);
        let repeat_text = repeats.iter().find(|(f, _)| *f == current).map_or(strings.none_option, |(_, l)| *l);
        let choice = |id: SharedString, label: &'static str, on: bool| {
            ListItem::new(id)
                .height(px(34.))
                .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
                .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
        };
        let repeat_menu = v_flex()
            .children(repeats.into_iter().map(|(frequency, label)| {
                choice(SharedString::from(format!("countdown-repeat-{label}")), label, current == frequency).on_click(set(Box::new(
                    move |d| {
                        d.repeat = frequency.map(|f| fern_core::Recurrence::new(f, 1));
                        d.open = None;
                    },
                )))
            }))
            .into_any_element();
        let type_menu = v_flex()
            .children([CountdownKind::Countdown, CountdownKind::Anniversary, CountdownKind::Birthday, CountdownKind::Holiday].map(|option| {
                choice(SharedString::from(format!("countdown-kind-{}", option.as_str())), kind_label(option, strings), option == kind)
                    .start_slot(Icon::new(kind_icon(option)).color(Color::Muted))
                    .on_click(set(Box::new(move |d| {
                        d.kind = option;
                        d.open = None;
                    })))
            }))
            .into_any_element();

        // The Reminder dropdown: the original's None, the days before at 09:00 (several may be
        // ticked), and Constant Reminder.
        let reminder_text = if draft.reminders.is_empty() {
            strings.none_option.to_owned()
        } else {
            fern_core::countdown::REMINDER_DAYS
                .iter()
                .zip(strings.countdown_reminder_days)
                .filter(|(days, _)| draft.reminders.contains(days))
                .map(|(_, label)| label)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let time = crate::calendar::format_time(fern_core::countdown::reminder_time());
        let constant = draft.constant_reminder;
        let reminder_menu = v_flex()
            .child(choice("countdown-reminder-none".into(), strings.none_option, draft.reminders.is_empty()).on_click(set(Box::new(|d| d.reminders.clear()))))
            .children(fern_core::countdown::REMINDER_DAYS.into_iter().zip(strings.countdown_reminder_days).map(|(days, label)| {
                let on = draft.reminders.contains(&days);
                ListItem::new(SharedString::from(format!("countdown-reminder-{days}")))
                    .height(px(34.))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
                            .child(Label::new(format!("({time})")).color(if on { Color::Accent } else { Color::Muted })),
                    )
                    .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                    .on_click(set(Box::new(move |d| {
                        if let Some(at) = d.reminders.iter().position(|r| *r == days) {
                            d.reminders.remove(at);
                        } else {
                            d.reminders.push(days);
                            d.reminders.sort();
                        }
                    })))
            }))
            .child(div().my_1().h(px(1.)).bg(theme.colors().border_variant))
            .child(
                h_flex()
                    .id("countdown-constant")
                    .h(px(34.))
                    .px_2()
                    .cursor_pointer()
                    .child(div().flex_1().child(Label::new(strings.habit_constant_reminder)))
                    .child(Switch::new().toggle_state(constant))
                    .on_click(set(Box::new(|d| d.constant_reminder = !d.constant_reminder))),
            )
            .child(
                h_flex().pt_1().justify_end().child(
                    div()
                        .id("countdown-reminder-ok")
                        .w(px(100.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .bg(theme.colors().accent)
                        .child(Label::new(strings.ok).size(LabelSize::Small))
                        .on_click(set(Box::new(|d| d.open = None))),
                ),
            )
            .into_any_element();

        // The icon picker under the icon: an emoji and its background color.
        let icon_menu = v_flex()
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
                    .on_click(set(Box::new(move |d| d.icon = (!on).then(|| (*emoji).to_owned()))))
            })))
            .child(h_flex().gap_1p5().children(std::iter::once(None).chain(crate::lists_panel::LIST_COLORS.iter().map(|c| Some(*c))).map(|color| {
                let on = draft.color.as_deref() == color;
                let fill = crate::lists_panel::parse_color(color).unwrap_or(theme.colors().accent);
                div()
                    .id(SharedString::from(format!("countdown-color-{}", color.unwrap_or("none"))))
                    .size(px(20.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(fill.opacity(if color.is_none() { 0.35 } else { 1. }))
                    .when(on, |el| el.border_2().border_color(theme.colors().text))
                    .on_click(set(Box::new(move |d| d.color = color.map(str::to_owned))))
            })))
            .into_any_element();

        let dropdown = |which: DraftRow, text: String, menu: AnyElement, cx: &Context<Self>| -> AnyElement {
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
                        .child(div().flex_1().min_w_0().child(Label::new(text).truncate()))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(draft) = this.draft.as_mut() {
                                draft.open = if draft.open == Some(which) { None } else { Some(which) };
                            }
                            cx.notify();
                        })),
                )
                .when(on, |el| {
                    el.child(PopoverAttachment::new(
                        Placement::BelowLeft,
                        popover(cx)
                            .w(px(294.))
                            .p_2()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                if let Some(draft) = this.draft.as_mut() {
                                    draft.open = None;
                                }
                                cx.notify();
                            }))
                            .child(menu),
                    ))
                })
                .into_any_element()
        };
        let row = |label: &'static str, content: AnyElement| {
            h_flex()
                .min_h(px(32.))
                .gap_2()
                .child(div().w(px(80.)).flex_none().child(Label::new(label)))
                .child(div().flex_1().min_w_0().child(content))
        };
        let date_row = row(strings.filter_date, dropdown(DraftRow::Date, format_date(selected, strings), date_menu, cx));
        let reminder_row = row(strings.reminder, dropdown(DraftRow::Reminder, reminder_text, reminder_menu, cx));
        let repeat_row = row(strings.repeat, dropdown(DraftRow::Repeat, repeat_text.to_owned(), repeat_menu, cx));
        let type_row = row(strings.filter_type, dropdown(DraftRow::Type, kind_label(kind, strings).to_owned(), type_menu, cx));
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
                    .bg(crate::lists_panel::parse_color(draft.color.as_deref()).unwrap_or(theme.colors().accent).opacity(0.35))
                    .child(match &draft.icon {
                        Some(icon) => div().text_size(px(18.)).child(icon.clone()).into_any_element(),
                        None => Icon::new(kind_icon(kind)).color(Color::Default).into_any_element(),
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(draft) = this.draft.as_mut() {
                            draft.open = if draft.open == Some(DraftRow::Icon) { None } else { Some(DraftRow::Icon) };
                        }
                        cx.notify();
                    })),
            )
            .when(icon_open, |el| {
                el.child(PopoverAttachment::new(
                    Placement::BelowLeft,
                    popover(cx)
                        .w(px(300.))
                        .p_3()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            if let Some(draft) = this.draft.as_mut() {
                                draft.open = None;
                            }
                            cx.notify();
                        }))
                        .child(icon_menu),
                ))
            });
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .w(px(100.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };
        let show_age = draft.show_age;

        popover(cx)
            .w(px(430.))
            .p_4()
            .gap_2()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(if draft.editing.is_some() { strings.countdown_edit } else { strings.countdown_new }).weight(FontWeight::BOLD)))
                    .child(IconButton::new("countdown-x", IconName::X).icon_color(Color::Muted).on_click(cx.listener(|this, _, _, cx| {
                        this.draft = None;
                        cx.notify();
                    }))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(80.)).flex_none().child(icon))
                    .child(
                        h_flex()
                            .flex_1()
                            .h(px(32.))
                            .px_3()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .child(draft.name.clone()),
                    ),
            )
            .child(date_row)
            .child(reminder_row)
            .child(repeat_row)
            .child(type_row)
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
                        .on_click(set(Box::new(|d| d.show_age = !d.show_age))),
                )
            })
            .child(
                h_flex()
                    .pt_2()
                    .gap(px(10.))
                    .justify_end()
                    .child(button("countdown-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| {
                        this.draft = None;
                        cx.notify();
                    })))
                    .child(button("countdown-save", strings.save, true).on_click(cx.listener(|this, _, _, cx| this.save_draft(cx)))),
            )
            .into_any_element()
    }

}

impl Render for CountdownView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let filters = [
            (Filter::All, strings.countdown_all),
            (Filter::Kind(CountdownKind::Countdown), strings.countdown_kind_countdown),
            (Filter::Kind(CountdownKind::Anniversary), strings.countdown_kind_anniversary),
            (Filter::Kind(CountdownKind::Holiday), strings.countdown_kind_holiday),
        ];
        let shown: Vec<Countdown> = self
            .countdowns
            .iter()
            .filter(|c| match self.filter {
                Filter::All => true,
                Filter::Kind(kind) => c.kind == kind,
            })
            .cloned()
            .collect();
        let cards: Vec<AnyElement> = shown.iter().map(|c| self.render_card(c, cx)).collect();
        // The add / edit dialog over a dimmed background, as the original shows it.
        let modal = self.draft.is_some().then(|| {
            div()
                .id("countdown-modal")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.35))
                .occlude()
                .child(self.render_draft(cx))
        });

        v_flex()
            .relative()
            .size_full()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(
                h_flex()
                    .h(px(52.))
                    .flex_none()
                    .px(px(22.))
                    .gap_1()
                    .child(Label::new(strings.section_countdown).size(LabelSize::Large).weight(FontWeight::BOLD))
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                    .child(div().flex_1())
                    .child(
                        div()
                            .relative()
                            .child(IconButton::new("countdown-add", IconName::Plus).on_click(cx.listener(|this, _, _, cx| {
                                this.add_menu = !this.add_menu;
                                cx.notify();
                            })))
                            // The original's "+" asks for the kind first: 162 px wide, a row every 34 px.
                            .when(self.add_menu, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .w(px(162.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.add_menu = false;
                                            cx.notify();
                                        }))
                                        .children(
                                            [CountdownKind::Countdown, CountdownKind::Anniversary, CountdownKind::Birthday, CountdownKind::Holiday]
                                                .into_iter()
                                                .map(|kind| {
                                                    ListItem::new(SharedString::from(format!("countdown-add-{}", kind.as_str())))
                                                        .height(px(34.))
                                                        .start_slot(Icon::new(kind_icon(kind)).color(Color::Muted))
                                                        .child(Label::new(kind_label(kind, strings)))
                                                        .on_click(cx.listener(move |this, _, window, cx| this.start_new_kind(kind, window, cx)))
                                                }),
                                        ),
                                ))
                            }),
                    )
                    .child(IconButton::new("countdown-more", IconName::Ellipsis)),
            )
            .child(h_flex().flex_none().px(px(20.)).gap_1().pb_3().children(filters.map(|(filter, label)| {
                let selected = self.filter == filter;
                div()
                    .id(label)
                    .px_2()
                    .py_0p5()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(theme.colors().accent.opacity(0.2)))
                    .child(Label::new(label).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Muted }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = filter;
                        cx.notify();
                    }))
            })))
            .child(
                div()
                    .id("countdown-grid")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(22.))
                    .when(cards.is_empty(), |el| {
                        el.child(div().pt_8().child(Label::new(strings.countdown_empty).color(Color::Muted)))
                    })
                    .child(div().flex().flex_wrap().gap(px(20.)).children(cards)),
            )
            .children(modal)
    }
}
