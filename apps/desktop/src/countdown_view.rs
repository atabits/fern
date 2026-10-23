//! The Countdown section, matched to TickTick's: filter tabs and a grid of cards showing the time
//! until (or since) a date; clicking the numbers switches between Y/M/D and plain days.

use std::collections::HashSet;
use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Countdown, CountdownKind, Id, Store};
use gpui::{Entity, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::calendar;
use crate::i18n::{self, Strings};
use crate::text_input::{Cancelled, Mode, Submitted, TextInput};

const CARD_WIDTH: f32 = 289.;
const CARD_HEIGHT: f32 = 181.;

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
    /// Its Custom reminders (days before, time), and the Custom form while open.
    custom_reminders: Vec<(u32, chrono::NaiveTime)>,
    custom: Option<CustomReminder>,
    constant_reminder: bool,
    show_in_list: fern_core::countdown::ShowInList,
    plus_one_day: bool,
    count_up: bool,
    /// The card's picture: the saved one's name, or a new file picked, or nothing.
    background: Option<String>,
    new_background: Option<std::path::PathBuf>,
}

/// The Reminder's Custom form: a Day or Week unit, how many in advance, and the time.
struct CustomReminder {
    weeks: bool,
    count: Entity<TextInput>,
    time: Entity<TextInput>,
}

/// The countdown dialog's fields that open a dropdown, as in the original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DraftRow {
    Icon,
    Presets,
    Date,
    Month,
    Day,
    Year,
    Reminder,
    Repeat,
    Type,
    CalcMode,
    CountingMode,
    Show,
}

/// Emoji offered for countdowns; the original's illustrated icon set is its own artwork.
/// How far the unit letters sit above the numbers' line box, to share their baseline.
const UNIT_BASELINE: f32 = 9.;

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
    /// The title's "Countdown ▾" menu (Active / Archived) and the header's "…" menu, while open.
    title_menu: bool,
    header_menu: bool,
    /// Listing the archived countdowns.
    show_archived: bool,
}

/// Hides the kind tabs (the header "…" menu's Hide Group).
const HIDE_GROUP_SETTING: &str = "countdown_hide_group";

pub fn kind_icon(kind: CountdownKind) -> IconName {
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
    // The original's 24 px round icon beside the name.
    div()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(fill.map_or(color.opacity(0.25), |c| c.opacity(0.6)))
        .map(|el| match &countdown.icon {
            Some(icon) => el.text_size(px(14.)).child(icon.clone()),
            None => el.child(
                Icon::new(kind_icon(countdown.kind))
                    .size(IconSize::Small)
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
    // A birthday counting up shows the time since the birth.
    let date = if countdown.count_up { countdown.date } else { countdown.next_date(today) };
    let span = countdown.span(today);
    // On a card's picture the numbers are white, as in the original.
    let on_picture = countdown.background.is_some();
    let color = if on_picture {
        gpui::white()
    } else if span.past {
        theme.status().warning
    } else {
        theme.colors().accent
    };
    let number = |value: u32, unit: &str| {
        // As in the original: the numbers in a light serif, the units in the UI font, on the
        // numbers' baseline.
        h_flex()
            .items_end()
            .child(div().text_size(px(46.)).line_height(px(46.)).font_family("IBM Plex Serif").text_color(color).child(value.to_string()))
            .child(div().pl_1().pr_1().mb(px(UNIT_BASELINE)).text_size(px(18.)).line_height(px(18.)).font_weight(FontWeight::MEDIUM).text_color(color).child(unit.to_owned()))
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
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(countdown.name.clone())),
        )
        .child(numbers)
        .child(Label::new(subtitle).size(LabelSize::XSmall).color(Color::Muted))
        .into_any_element()
}

impl CountdownView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let mut this = Self { store, countdowns: Vec::new(), filter: Filter::All, days_only: HashSet::new(), draft: None, card_menu: None, add_menu: false, title_menu: false, header_menu: false, show_archived: false };
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
        self.draft = Some(Draft { name, kind, date: today, month: calendar::month_start(today), icon: None, color: None, repeat: None, editing: None, show_age: false, open: None, reminders: fern_core::countdown::DEFAULT_REMINDERS.to_vec(), custom_reminders: Vec::new(), custom: None, constant_reminder: false, show_in_list: Default::default(), plus_one_day: false, count_up: false, background: None, new_background: None });
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
                draft.repeat = Some(fern_core::Recurrence::new(fern_core::Frequency::Yearly, 1));
            }
        }
        cx.notify();
    }

    /// The Background row's "Choose a picture…": the system's file dialog.
    fn choose_background(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            let Some(path) = chosen.await.ok().and_then(Result::ok).flatten().and_then(|paths| paths.into_iter().next()) else { return };
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
        let time = field(&calendar::format_time(fern_core::countdown::reminder_time()), cx);
        for input in [&count, &time] {
            cx.subscribe(input, |_, _, _: &crate::text_input::Changed, cx| cx.notify()).detach();
            cx.subscribe(input, |this, _, _: &Submitted, cx| this.confirm_custom_reminder(cx)).detach();
        }
        window.focus(&count.focus_handle(cx), cx);
        if let Some(draft) = self.draft.as_mut() {
            draft.custom = Some(CustomReminder { weeks: false, count, time });
        }
        cx.notify();
    }

    /// The Custom form's OK: adds the reminder when the count and time read.
    fn confirm_custom_reminder(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_mut() else { return };
        let Some(custom) = draft.custom.as_ref() else { return };
        let count: Option<u32> = custom.count.read(cx).text().trim().parse().ok();
        let time = crate::due_picker::parse_time(custom.time.read(cx).text());
        if let (Some(count), Some(time)) = (count, time) {
            let days = if custom.weeks { count * 7 } else { count };
            if !draft.custom_reminders.contains(&(days, time)) {
                draft.custom_reminders.push((days, time));
                draft.custom_reminders.sort();
            }
            draft.custom = None;
        }
        cx.notify();
    }

    /// The new countdown's `reminder`, `calc` or `show` dropdown, for screenshots.
    #[cfg(debug_assertions)]
    pub fn dev_open_row(&mut self, row: &str, cx: &mut Context<Self>) {
        if let Some(draft) = self.draft.as_mut() {
            draft.open = Some(match row {
                "calc" => DraftRow::CalcMode,
                "presets" => DraftRow::Presets,
                "counting" => DraftRow::CountingMode,
                "month" => DraftRow::Month,
                "show" => DraftRow::Show,
                _ => DraftRow::Reminder,
            });
        }
        cx.notify();
    }

    /// The header's "Countdown ▾" (or with `more`, its "…") menu, for screenshots.
    #[cfg(debug_assertions)]
    pub fn dev_header_menu(&mut self, more: bool, cx: &mut Context<Self>) {
        self.title_menu = !more;
        self.header_menu = more;
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
            self.store.set_countdown_reminders(id, &draft.reminders, &draft.custom_reminders, draft.constant_reminder)?;
            // A new picture is copied in; one taken off is cleared.
            if draft.new_background.is_some() || draft.background.is_none() {
                let dir = crate::attachments_dir()?;
                self.store.set_countdown_background(id, draft.new_background.as_deref(), &dir)?;
            }
            self.store.set_countdown_display(id, draft.show_in_list, draft.plus_one_day, draft.kind == CountdownKind::Birthday && draft.count_up)
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
        let background: Option<std::path::PathBuf> = countdown
            .background
            .as_ref()
            .and_then(|name| crate::attachments_dir().ok().map(|dir| dir.join(name)))
            .filter(|path| path.exists());
        let has_picture = background.is_some();

        v_flex()
            .id(group.clone())
            .group(group.clone())
            .relative()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .bg(theme.colors().surface_background)
            // The card's picture behind its numbers, darkened so they stay readable.
            .when_some(background, |el, path| {
                el.child(gpui::img(path).absolute().inset_0().size_full().rounded(px(8.)).object_fit(gpui::ObjectFit::Cover))
                    .child(div().absolute().inset_0().rounded(px(8.)).bg(gpui::black().opacity(0.35)))
            })
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        badge(countdown, color, cx),
                    )
                    .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(countdown.name.clone())),
            )
            // The original's spacing: 20 px down to the numbers, 16 px on to the dates.
            .child(
                div()
                    .id("countdown-numbers")
                    .mt(px(20.))
                    .mb(px(16.))
                    .cursor_pointer()
                    .tooltip(Tooltip::text(strings.countdown_switch_hint))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_format(id, cx)))
                    .child(numbers),
            )
            .child(div().text_size(px(12.)).when(has_picture, |el| el.text_color(gpui::white().opacity(0.85))).when(!has_picture, |el| el.text_color(theme.colors().text_muted)).child(subtitle))
            // The original's pin, beside the "…": a pinned card stays first and shows it.
            .child({
                let pinned = countdown.pinned_at.is_some();
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(32.))
                    .opacity(if pinned || crate::touch::active() { 1. } else { 0. })
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
                    .opacity(if crate::touch::active() { 1. } else { 0. })
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
                                .min_w(px(150.))
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
                                .child({
                                    let archived = countdown.archived;
                                    ListItem::new("countdown-archive")
                                        .start_slot(Icon::new(if archived { IconName::ArchiveRestore } else { IconName::Archive }).color(Color::Muted))
                                        .child(Label::new(if archived { strings.habit_unarchive } else { strings.habit_archive }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.card_menu = None;
                                            if let Err(err) = this.store.set_countdown_archived(id, !archived) {
                                                eprintln!("failed to archive the countdown: {err:#}");
                                            }
                                            this.reload(cx);
                                        }))
                                })
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
        let custom_label = |days: u32, time: chrono::NaiveTime| {
            format!("{} ({})", strings.countdown_custom_label.replace("{n}", &days.to_string()), crate::calendar::format_time(time))
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
        let time = crate::calendar::format_time(fern_core::countdown::reminder_time());
        let constant = draft.constant_reminder;
        let custom_form = draft.custom.as_ref().map(|custom| {
            let weeks = custom.weeks;
            let count: u32 = custom.count.read(cx).text().trim().parse().unwrap_or(0);
            let days = if weeks { count * 7 } else { count };
            let field = |input: Entity<TextInput>| {
                h_flex().w(px(100.)).h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().element_hover).child(input)
            };
            let small_button = |id: &'static str, label: &'static str, primary: bool| {
                div()
                    .id(id)
                    .flex_1()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(primary, |el| el.bg(theme.colors().accent))
                    .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                    .child(Label::new(label).size(LabelSize::Small))
            };
            v_flex()
                .gap_2()
                .p_1()
                .child(h_flex().gap_1().children(strings.countdown_custom_units.iter().enumerate().map(|(i, label)| {
                    let on = weeks == (i == 1);
                    div()
                        .id(("countdown-custom-unit", i))
                        .flex_1()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(on, |el| el.bg(theme.colors().element_selected))
                        .when(!on, |el| el.border_1().border_color(theme.colors().border))
                        .child(Label::new(*label).size(LabelSize::Small))
                        .on_click(set(Box::new(move |d| {
                            if let Some(custom) = d.custom.as_mut() {
                                custom.weeks = i == 1;
                            }
                        })))
                })))
                .child(h_flex().child(div().flex_1().child(Label::new(strings.countdown_custom_advance[weeks as usize]))).child(field(custom.count.clone())))
                .child(h_flex().child(div().flex_1().child(Label::new(strings.countdown_custom_at))).child(field(custom.time.clone())))
                .child(Label::new(strings.countdown_custom_ahead.replace("{n}", &days.to_string())).size(LabelSize::Small).color(Color::Muted))
                .child(
                    h_flex()
                        .gap_2()
                        .child(small_button("countdown-custom-cancel", strings.cancel, false).on_click(set(Box::new(|d| d.custom = None))))
                        .child(small_button("countdown-custom-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| this.confirm_custom_reminder(cx)))),
                )
                .into_any_element()
        });
        let reminder_menu = v_flex()
            .child(
                choice("countdown-reminder-none".into(), strings.none_option, draft.reminders.is_empty() && draft.custom_reminders.is_empty()).on_click(
                    set(Box::new(|d| {
                        d.reminders.clear();
                        d.custom_reminders.clear();
                    })),
                ),
            )
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
            // Custom reminders, each unticked with a click, then the original's Custom.
            .children(draft.custom_reminders.iter().map(|&(days, time)| {
                ListItem::new(SharedString::from(format!("countdown-custom-{days}-{time}")))
                    .height(px(34.))
                    .child(Label::new(custom_label(days, time)).color(Color::Accent))
                    .end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
                    .on_click(set(Box::new(move |d| d.custom_reminders.retain(|r| *r != (days, time)))))
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
        // The original's Custom shows its form in place of the list.
        let reminder_menu = custom_form.unwrap_or(reminder_menu);

        // The original's countdown date: month, day and year fields, each a list.
        use fern_core::countdown::{DatePart, days_in_month, with_date_part};
        let part_list = |id: &'static str, items: Vec<(String, DatePart, bool)>| {
            v_flex()
                .id(id)
                .max_h(px(280.))
                .overflow_y_scroll()
                .children(items.into_iter().enumerate().map(|(i, (label, part, on))| {
                    ListItem::new((id, i))
                        .height(px(34.))
                        .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
                        .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                        .on_click(set(Box::new(move |d| {
                            d.date = with_date_part(d.date, part);
                            d.month = calendar::month_start(d.date);
                            d.open = None;
                        })))
                }))
                .into_any_element()
        };
        let month_menu = part_list(
            "countdown-months",
            (1..=12).map(|m| (strings.months_short[m as usize - 1].to_owned(), DatePart::Month(m), m == selected.month())).collect(),
        );
        let day_menu = part_list(
            "countdown-days",
            (1..=days_in_month(selected.year(), selected.month())).map(|d| (d.to_string(), DatePart::Day(d), d == selected.day())).collect(),
        );
        let year_menu = part_list(
            "countdown-years",
            (today.year() - 100..=today.year() + 50).rev().map(|y| (y.to_string(), DatePart::Year(y), y == selected.year())).collect(),
        );

        // Day Calculation Mode: the original's two choices, each with its explanation.
        let plus_one = draft.plus_one_day;
        let calc_text = strings.countdown_calc_options[plus_one as usize].0.to_owned();
        let calc_menu = v_flex()
            .children(strings.countdown_calc_options.iter().enumerate().map(|(i, (label, about))| {
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
                    .when(on, |el| el.child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                    .on_click(set(Box::new(move |d| {
                        d.plus_one_day = i == 1;
                        d.open = None;
                    })))
            }))
            .into_any_element();
        // A birthday's Counting Mode: Countdown or Count Up.
        let count_up = draft.count_up;
        let counting_menu = v_flex()
            .children(strings.countdown_counting_options.iter().enumerate().map(|(i, label)| {
                choice(SharedString::from(format!("countdown-counting-{i}")), label, count_up == (i == 1)).on_click(set(Box::new(move |d| {
                    d.count_up = i == 1;
                    d.open = None;
                })))
            }))
            .into_any_element();
        // Show: when it shows in the Today list.
        use fern_core::countdown::ShowInList;
        let show = draft.show_in_list;
        let show_text = strings.countdown_show_options[show as usize].to_owned();
        let show_menu = v_flex()
            .child(div().px_2().py_1().child(Label::new(strings.countdown_show_heading).size(LabelSize::Small).color(Color::Muted)))
            .children(ShowInList::ALL.into_iter().map(|option| {
                choice(SharedString::from(format!("countdown-show-{}", option.key())), strings.countdown_show_options[option as usize], option == show)
                    .on_click(set(Box::new(move |d| {
                        d.show_in_list = option;
                        d.open = None;
                    })))
            }))
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

        let dropdown_w = |which: DraftRow, text: String, menu: AnyElement, width: f32, cx: &Context<Self>| -> AnyElement {
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
                            .w(px(width))
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
        let dropdown = |which: DraftRow, text: String, menu: AnyElement, cx: &Context<Self>| dropdown_w(which, text, menu, 294., cx);
        let row = |label: &'static str, content: AnyElement| {
            h_flex()
                .min_h(px(32.))
                .gap_2()
                .child(div().w(px(80.)).flex_none().child(Label::new(label)))
                .child(div().flex_1().min_w_0().child(content))
        };
        // A birthday or a holiday picks its date from a month, as the original's "Select Date"; the others
        // take month, day and year fields.
        let date_row = if matches!(kind, CountdownKind::Birthday | CountdownKind::Holiday) {
            row(strings.filter_date, dropdown(DraftRow::Date, format_date(selected, strings), date_menu, cx))
        } else {
            row(
                strings.filter_date,
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(dropdown_w(DraftRow::Month, strings.months_short[selected.month0() as usize].to_owned(), month_menu, 110., cx)))
                    .child(div().flex_1().child(dropdown_w(DraftRow::Day, selected.day().to_string(), day_menu, 110., cx)))
                    .child(div().flex_1().child(dropdown_w(DraftRow::Year, selected.year().to_string(), year_menu, 110., cx)))
                    .into_any_element(),
            )
        };
        let reminder_row = row(strings.reminder, dropdown(DraftRow::Reminder, reminder_text, reminder_menu, cx));
        let repeat_row = row(strings.repeat, dropdown(DraftRow::Repeat, repeat_text.to_owned(), repeat_menu, cx));
        // As in the original, a birthday has Counting Mode where the others have Day Calculation Mode.
        // A holiday has neither, as in the original.
        let calc_row = match kind {
            CountdownKind::Birthday => Some(row(
                strings.countdown_counting_mode,
                dropdown(DraftRow::CountingMode, strings.countdown_counting_options[count_up as usize].to_owned(), counting_menu, cx),
            )),
            CountdownKind::Holiday => None,
            _ => Some(row(strings.countdown_calc_mode, dropdown(DraftRow::CalcMode, calc_text, calc_menu, cx))),
        };
        let show_row = row(strings.countdown_show, dropdown(DraftRow::Show, show_text, show_menu, cx));
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
        let presets = (open == Some(DraftRow::Presets)).then(|| {
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .min_w(px(260.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if let Some(draft) = this.draft.as_mut() {
                            draft.open = None;
                        }
                        cx.notify();
                    }))
                    .children(fern_core::countdown::presets(kind).iter().zip(if kind == CountdownKind::Anniversary {
                        strings.countdown_anniversary_presets
                    } else {
                        strings.countdown_presets
                    }).enumerate().map(|(i, (preset, name))| {
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
                                    .bg(crate::lists_panel::parse_color(Some(preset.color)).unwrap_or(theme.colors().accent).opacity(0.45))
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
                    })),
            )
        });

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
                            .pl_3()
                            .pr_1()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .child(div().flex_1().child(draft.name.clone()))
                            // The original's bookmark: ready-made entries to start from, for the
                            // kinds that have them.
                            .when(!fern_core::countdown::presets(kind).is_empty(), |el| el.child(
                                div()
                                    .relative()
                                    .child(
                                        IconButton::new("countdown-presets", IconName::Bookmark)
                                            .icon_color(Color::Muted)
                                            .on_click(set(Box::new(|d| {
                                                d.open = if d.open == Some(DraftRow::Presets) { None } else { Some(DraftRow::Presets) };
                                            }))),
                                    )
                                    .children(presets),
                            )),
                    ),
            )
            .child(date_row)
            .child(reminder_row)
            .child(repeat_row)
            .child(type_row)
            .children(calc_row)
            .child(show_row)
            // The card's picture: the file's name with "×" to take it off, or the chooser.
            .child(row(
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
                            .child(Label::new(draft.background.clone().unwrap_or_else(|| strings.countdown_choose_image.to_owned())).truncate().color(
                                if draft.background.is_some() { Color::Default } else { Color::Muted },
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.choose_background(cx))),
                    )
                    .when(draft.background.is_some(), |el| {
                        el.child(IconButton::new("countdown-background-clear", IconName::X).icon_color(Color::Muted).on_click(set(Box::new(|d| {
                            d.background = None;
                            d.new_background = None;
                        }))))
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
        let hide_group = self.store.setting(HIDE_GROUP_SETTING).ok().flatten().as_deref() == Some("1");
        let filters = [
            (Filter::All, strings.countdown_all),
            (Filter::Kind(CountdownKind::Countdown), strings.countdown_kind_countdown),
            (Filter::Kind(CountdownKind::Anniversary), strings.countdown_kind_anniversary),
            (Filter::Kind(CountdownKind::Holiday), strings.countdown_kind_holiday),
        ];
        let shown: Vec<Countdown> = self
            .countdowns
            .iter()
            .filter(|c| c.archived == self.show_archived)
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
                    // The original's "Countdown ▾": Active or Archived countdowns.
                    .child(
                        div()
                            .relative()
                            .child(
                                h_flex()
                                    .id("countdown-title")
                                    .gap_1()
                                    .cursor_pointer()
                                    .child(Label::new(strings.section_countdown).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.title_menu = !this.title_menu;
                                        cx.notify();
                                    })),
                            )
                            .when(self.title_menu, |el| {
                                let archived = self.show_archived;
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    popover(cx)
                                        .min_w(px(162.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.title_menu = false;
                                            cx.notify();
                                        }))
                                        .children([(false, strings.habit_active), (true, strings.habit_archived)].map(|(value, label)| {
                                            ListItem::new(if value { "countdowns-archived" } else { "countdowns-active" })
                                                .height(px(34.))
                                                .child(Label::new(label).color(if value == archived { Color::Accent } else { Color::Default }))
                                                .when(value == archived, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.show_archived = value;
                                                    this.title_menu = false;
                                                    cx.notify();
                                                }))
                                        })),
                                ))
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .relative()
                            .child(IconButton::new("countdown-add", IconName::Plus).icon_size(IconSize::XLarge).icon_color(Color::Default).on_click(cx.listener(|this, _, _, cx| {
                                this.add_menu = !this.add_menu;
                                cx.notify();
                            })))
                            // The original's "+" asks for the kind first: 162 px wide, a row every 34 px.
                            .when(self.add_menu, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .min_w(px(162.))
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
                    // The original's "…": Hide Group (the kind tabs) or Show Group.
                    .child(
                        div()
                            .relative()
                            .child(IconButton::new("countdown-more", IconName::Ellipsis).icon_color(Color::Default).on_click(cx.listener(|this, _, _, cx| {
                                this.header_menu = !this.header_menu;
                                cx.notify();
                            })))
                            .when(self.header_menu, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .min_w(px(162.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.header_menu = false;
                                            cx.notify();
                                        }))
                                        .child(
                                            ListItem::new("countdown-hide-group")
                                                .height(px(34.))
                                                .start_slot(Icon::new(IconName::Layers).color(Color::Muted))
                                                .child(Label::new(if hide_group { strings.countdown_show_group } else { strings.countdown_hide_group }))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.header_menu = false;
                                                    if let Err(err) = this.store.set_setting(HIDE_GROUP_SETTING, if hide_group { "0" } else { "1" }) {
                                                        eprintln!("failed to save the countdown setting: {err:#}");
                                                    }
                                                    cx.notify();
                                                })),
                                        ),
                                ))
                            }),
                    ),
            )
            .when(!hide_group, |el| el.child(h_flex().flex_none().px(px(20.)).pt_2().gap_1().pb(px(18.)).children(filters.map(|(filter, label)| {
                // The original's filter pills: 32 px, at the body size.
                let selected = self.filter == filter;
                div()
                    .id(label)
                    .h(px(32.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(16.))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(theme.colors().accent.opacity(0.2)))
                    .child(Label::new(label).color(if selected { Color::Accent } else { Color::Muted }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = filter;
                        cx.notify();
                    }))
            }))))
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
