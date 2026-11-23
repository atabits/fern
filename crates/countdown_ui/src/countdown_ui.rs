//! The Countdown section, matched to TickTick's: filter tabs and a grid of cards showing the time
//! until (or since) a date; clicking the numbers switches between Y/M/D and plain days.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::countdown::{DatePart, days_in_month, with_date_part};
use fern_core::{Countdown, CountdownKind, DateFormat, Id, Store};
use gpui::{Entity, Focusable as _, FontWeight, ScrollHandle, Subscription};
use ui::prelude::*;
use ui::{Button, ButtonSize, ButtonVariant, ConfirmDialog, EmptyState, Modal, Placement, PopoverAttachment, popover};

use desktop_common::data_version::{DataVersion, changed_elsewhere, data_changed};
use desktop_common::{actions, calendar, colors as lists_panel, layout, paths::attachments_dir, touch};
use desktop_common::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};
use fern_i18n::{self as i18n, Strings};
#[cfg(test)]
use desktop_common::keymap;

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
    date_inputs: DateInputs,
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
    DatePart(DateField),
    Reminder,
    Repeat,
    Type,
    CalcMode,
    CountingMode,
    Show,
}

/// The direct date inputs used by all four countdown types.
struct DateInputs {
    fields: [Entity<TextInput>; 3],
    scroll: ScrollHandle,
    _focus_subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DateField {
    Year,
    Month,
    Day,
}

impl DateField {
    const ALL: [Self; 3] = [Self::Year, Self::Month, Self::Day];

    fn ordered(strings: &Strings) -> [Self; 3] {
        match calendar::effective_date_format(strings) {
            DateFormat::MonthDayYear => [Self::Month, Self::Day, Self::Year],
            DateFormat::DayMonthYear => [Self::Day, Self::Month, Self::Year],
            DateFormat::YearMonthDay | DateFormat::IsoYearMonthDay => Self::ALL,
        }
    }

    fn id(self) -> &'static str {
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
        with_date_part(date, match self {
            Self::Year => DatePart::Year(value),
            Self::Month => DatePart::Month(value as u32),
            Self::Day => DatePart::Day(value as u32),
        })
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
    fn set_date(&mut self, date: NaiveDate, cx: &mut Context<CountdownView>) {
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
}

/// Emoji offered for countdowns; the original's illustrated icon set is its own artwork.
const COUNTDOWN_EMOJI: [&str; 14] = ["🎂", "🎉", "❤️", "💍", "✈️", "🎄", "🎓", "⭐", "🏆", "🎯", "🚩", "🎮", "👶", "🏠"];

pub struct CountdownView {
    store: Rc<Store>,
    focus_handle: gpui::FocusHandle,
    countdowns: Vec<Countdown>,
    filter: Filter,
    /// Cards switched to the "days only" format.
    days_only: HashSet<Id>,
    draft: Option<Draft>,
    /// The card whose "…" menu is open.
    card_menu: Option<Id>,
    /// Countdown pending deletion confirmation: (id, name).
    confirm_delete: Option<(Id, String)>,
    /// The "+" menu of kinds, as in the original.
    add_menu: bool,
    /// The title's "Countdown ▾" menu (Active / Archived) and the header's "…" menu, while open.
    title_menu: bool,
    header_menu: bool,
    /// Listing the archived countdowns.
    show_archived: bool,
    /// Cached value of `HIDE_GROUP_SETTING` (loaded in `reload`, never queried during `render`).
    hide_group: bool,
    /// Resolved on-disk background image paths, refreshed in `reload` so `render_card` performs zero `stat()` calls.
    resolved_backgrounds: HashMap<Id, PathBuf>,
    active: bool,
    dirty: bool,
}

/// Hides the kind tabs (the header "…" menu's Hide Group).
const HIDE_GROUP_SETTING: &str = "countdown_hide_group";

pub use desktop_common::countdown_helpers::{
    Face, badge, countdown_color, face, format_date, kind_icon, render_summary,
};

fn kind_label(kind: CountdownKind, strings: &Strings) -> &'static str {
    match kind {
        CountdownKind::Countdown => strings.countdown_kind_countdown,
        CountdownKind::Anniversary => strings.countdown_kind_anniversary,
        CountdownKind::Birthday => strings.countdown_kind_birthday,
        CountdownKind::Holiday => strings.countdown_kind_holiday,
    }
}

impl gpui::Focusable for CountdownView {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl CountdownView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, true, cx)
    }

    pub fn new_inactive(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, false, cx)
    }

    fn new_with_active(store: Rc<Store>, initial_active: bool, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            store,
            focus_handle: cx.focus_handle(),
            countdowns: Vec::new(),
            filter: Filter::All,
            days_only: HashSet::new(),
            draft: None,
            card_menu: None,
            confirm_delete: None,
            add_menu: false,
            title_menu: false,
            header_menu: false,
            show_archived: false,
            hide_group: false,
            resolved_backgrounds: HashMap::new(),
            active: initial_active,
            dirty: !initial_active,
        };
        if initial_active {
            this.reload(cx);
        }
        cx.observe_global::<DataVersion>(|this, cx| {
            if changed_elsewhere(cx.entity_id(), cx) {
                this.sync_external(cx);
            }
        })
        .detach();
        this
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if active && self.dirty {
            self.dirty = false;
            self.reload(cx);
        }
    }

    pub fn sync_external(&mut self, cx: &mut Context<Self>) {
        if self.active {
            self.reload(cx);
        } else {
            self.dirty = true;
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        self.countdowns = self.store.countdowns().unwrap_or_default();
        self.hide_group = self
            .store
            .setting(HIDE_GROUP_SETTING)
            .ok()
            .flatten()
            .as_deref()
            == Some("1");
        self.resolved_backgrounds = match attachments_dir() {
            Ok(dir) => self
                .countdowns
                .iter()
                .filter_map(|c| {
                    let name = c.background.as_ref()?;
                    let path = dir.join(name);
                    path.exists().then_some((c.id, path))
                })
                .collect(),
            Err(_) => HashMap::new(),
        };
        cx.notify();
    }

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
            }).detach();
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
            }).detach();
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
        DateInputs { fields, scroll: ScrollHandle::new(), _focus_subscriptions: focus_subscriptions }
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
        draft.date_inputs.scroll.scroll_to_top_of_item(selected.saturating_sub(3).max(0) as usize);
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
        let date_inputs = Self::date_inputs(today, window, cx);
        self.draft = Some(Draft { name, kind, date: today, date_inputs, icon: None, color: None, repeat: None, editing: None, show_age: false, open: None, reminders: fern_core::countdown::DEFAULT_REMINDERS.to_vec(), custom_reminders: Vec::new(), custom: None, constant_reminder: false, show_in_list: Default::default(), plus_one_day: false, count_up: false, background: None, new_background: None });
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
        let time = field(&calendar::format_time(fern_core::countdown::reminder_time(), i18n::t(cx)), cx);
        for input in [&count, &time] {
            cx.subscribe(input, |_, _, _: &Changed, cx| cx.notify()).detach();
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
        let time = calendar::parse_time(custom.time.read(cx).text());
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
                "month" => DraftRow::DatePart(DateField::Month),
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
        if self.dirty {
            self.reload(cx);
        }
        let Some(countdown) = self.countdowns.iter().find(|c| c.id == id).cloned() else { return };
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

    /// `menu` opens the card's "…" menu, `edit` its edit popover, with a monthly repeat.
    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, name: &str, edit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            self.reload(cx);
        }
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
        let added = match draft.editing {
            Some(id) => self.store.update_countdown(id, &name, date, draft.kind, draft.repeat.as_ref()).map(|()| id),
            None => self.store.add_countdown(&name, date, draft.kind).and_then(|countdown| {
                self.store.update_countdown(countdown.id, &name, date, draft.kind, draft.repeat.as_ref()).map(|()| countdown.id)
            }),
        }
        .and_then(|id| {
            self.store.set_countdown_look(id, draft.icon.as_deref(), draft.color.as_deref())?;
            self.store.set_countdown_show_age(id, draft.kind == CountdownKind::Birthday && draft.show_age)?;
            self.store.set_countdown_reminders(id, &draft.reminders, &draft.custom_reminders, draft.constant_reminder)?;
            // A new picture is copied in; one taken off is cleared.
            if draft.new_background.is_some() || draft.background.is_none() {
                let dir = attachments_dir()?;
                self.store.set_countdown_background(id, draft.new_background.as_deref(), &dir)?;
            }
            self.store.set_countdown_display(id, draft.show_in_list, draft.plus_one_day, draft.kind == CountdownKind::Birthday && draft.count_up)
        });
        if let Err(err) = added {
            eprintln!("failed to add the countdown: {err:#}");
        } else {
            data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.delete_countdown(id) {
            eprintln!("failed to delete the countdown: {err:#}");
        } else {
            data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    fn toggle_pin(&mut self, id: Id, pinned: bool, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_countdown_pinned(id, !pinned) {
            eprintln!("failed to pin the countdown: {err:#}");
        } else {
            data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    fn toggle_archive(&mut self, id: Id, archived: bool, cx: &mut Context<Self>) {
        self.card_menu = None;
        if let Err(err) = self.store.set_countdown_archived(id, !archived) {
            eprintln!("failed to archive the countdown: {err:#}");
        } else {
            data_changed(cx.entity_id(), cx);
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
        let background: Option<PathBuf> = self.resolved_backgrounds.get(&id).cloned();
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
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    this.start_edit(id, window, cx);
                }
            }))
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
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .when(has_picture, |el| el.text_color(gpui::white()))
                            .child(countdown.name.clone()),
                    ),
            )
            // The original's spacing: 20 px down to the numbers, 16 px on to the dates.
            .child(
                div()
                    .id(SharedString::from(format!("countdown-numbers-{id}")))
                    .mt(px(20.))
                    .mb(px(16.))
                    .cursor_pointer()
                    .tooltip(Tooltip::text(strings.countdown_switch_hint))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_format(id, cx);
                    }))
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
                    .opacity(if pinned || touch::active() { 1. } else { 0. })
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .child(
                        IconButton::new(SharedString::from(format!("countdown-pin-{id}")), IconName::Pin)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .icon_color(if pinned { Color::Accent } else { Color::Muted })
                            .tooltip(Tooltip::text(if pinned { strings.unpin } else { strings.pin }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_pin(id, pinned, cx);
                            })),
                    )
            })
            .child(
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(6.))
                    .opacity(if touch::active() { 1. } else { 0. })
                    .group_hover(group, |s| s.opacity(1.))
                    .when(self.card_menu == Some(id), |el| el.opacity(1.))
                    .child(
                        // The original's "…" on a hovered card: Edit and Delete.
                        IconButton::new(SharedString::from(format!("countdown-more-{id}")), IconName::Ellipsis)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text(strings.more))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
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
                                        .start_slot(Icon::new(IconName::Pencil).color(Color::Icon))
                                        .child(Label::new(strings.edit))
                                        .on_click(cx.listener(move |this, _, window, cx| this.start_edit(id, window, cx))),
                                )
                                .child({
                                    let archived = countdown.archived;
                                    ListItem::new("countdown-archive")
                                        .start_slot(Icon::new(if archived { IconName::ArchiveRestore } else { IconName::Archive }).color(Color::Icon))
                                        .child(Label::new(if archived { strings.habit_unarchive } else { strings.habit_archive }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_archive(id, archived, cx);
                                        }))
                                })
                                .child({
                                    let name = countdown.name.clone();
                                    ListItem::new("countdown-delete")
                                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                        .child(Label::new(strings.delete).color(Color::Error))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.card_menu = None;
                                            this.confirm_delete = Some((id, name.clone()));
                                            cx.notify();
                                        }))
                                })
                                .into_any_element(),
                        ))
                    }),
            )
            .into_any_element()
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
                    .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
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
                        .child(div().debug_selector(move || format!("{}-toggle", field.id())).child(
                            IconButton::new(("countdown-date-options-toggle", field as usize), IconName::ChevronDown)
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
                                            window.focus(&draft.date_inputs.fields[field as usize].focus_handle(cx), cx);
                                            cx.notify();
                                        }
                                    })),
                        )),
                )
                .when(is_open && !in_popover, |el| el.child(PopoverAttachment::new(
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
                )))
        }));

        v_flex().gap_1().child(fields).when_some(active.filter(|_| in_popover), |el, field| {
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
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
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
            .when(open, |el| el.child(PopoverAttachment::new(
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
            )))
    }

    /// The original's add / edit dialog: the icon and name on top, then a label on the left and
    /// the value in a field with a ▾ for Date, Repeat and Type, "Show Age" for a birthday, and
    /// below a divider Cancel and Save.
    fn render_draft(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = self.draft.as_ref() else { return div().into_any_element() };
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let (kind, open) = (draft.kind, draft.open);
        let set = |f: Box<dyn Fn(&mut Draft)>| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let Some(draft) = this.draft.as_mut() {
                    f(draft);
                }
                cx.notify();
            })
        };

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
            format!("{} ({})", strings.countdown_custom_label.replace("{n}", &days.to_string()), calendar::format_time(time, strings))
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
        let time = calendar::format_time(fern_core::countdown::reminder_time(), strings);
        let constant = draft.constant_reminder;
        let custom_form = draft.custom.as_ref().map(|custom| {
            let weeks = custom.weeks;
            let count: u32 = custom.count.read(cx).text().trim().parse().unwrap_or(0);
            let days = if weeks { count * 7 } else { count };
            let field = |input: Entity<TextInput>| {
                h_flex().w(px(100.)).h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().element_hover).child(input)
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
                        .when(!on, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
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
                        .child(
                            div().flex_1().child(
                                Button::new("countdown-custom-cancel", strings.cancel)
                                    .variant(ButtonVariant::Secondary)
                                    .size(ButtonSize::Sm)
                                    .full_width()
                                    .on_click(set(Box::new(|d| d.custom = None))),
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
                    Button::new("countdown-reminder-ok", strings.ok)
                        .variant(ButtonVariant::Primary)
                        .size(ButtonSize::Sm)
                        .on_click(set(Box::new(|d| d.open = None))),
                ),
            )
            .into_any_element();
        // The original's Custom shows its form in place of the list.
        let reminder_menu = custom_form.unwrap_or(reminder_menu);

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
            .child(h_flex().gap_1p5().children(std::iter::once(None).chain(lists_panel::LIST_COLORS.iter().map(|c| Some(*c))).map(|color| {
                let on = draft.color.as_deref() == color;
                let fill = lists_panel::parse_color(color).unwrap_or(theme.colors().accent);
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
                        .hover(|s| s.bg(theme.colors().element_hover))
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
        let date_row = row(strings.filter_date, self.render_date_editor(draft, window, cx).into_any_element());
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
                    .bg(lists_panel::parse_color(draft.color.as_deref()).unwrap_or(theme.colors().accent).opacity(0.35))
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
                                    .bg(lists_panel::parse_color(Some(preset.color)).unwrap_or(theme.colors().accent).opacity(0.45))
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

        Modal::new("countdown-modal")
            .width(px(430.))
            .padding(px(16.))
            .gap(px(8.))
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(if draft.editing.is_some() { strings.countdown_edit } else { strings.countdown_new }).weight(FontWeight::BOLD)))
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
                            .when(!fern_core::countdown::presets(kind).is_empty(), |el| el.child(
                                div()
                                    .relative()
                                    .child(
                                        IconButton::new("countdown-presets", IconName::Bookmark)
                                            .icon_color(Color::Muted)
                                            .tooltip(Tooltip::text(strings.templates))
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
                        el.child(
                            IconButton::new("countdown-background-clear", IconName::X)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text(strings.clear))
                                .on_click(set(Box::new(|d| {
                                    d.background = None;
                                    d.new_background = None;
                                }))),
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
                        .on_click(set(Box::new(|d| d.show_age = !d.show_age))),
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
                        div().debug_selector(|| "countdown-save".to_owned()).child(
                            Button::new("countdown-save", strings.save)
                                .variant(ButtonVariant::Primary)
                                .on_click(cx.listener(|this, _, _, cx| this.save_draft(cx))),
                        ),
                    ),
            )
            .into_any_element()
    }

    pub fn dismiss_transient(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(draft) = self.draft.as_mut()
            && (draft.custom.take().is_some() || draft.open.take().is_some())
        {
            cx.notify();
            return true;
        }
        if std::mem::take(&mut self.title_menu)
            || std::mem::take(&mut self.add_menu)
            || std::mem::take(&mut self.header_menu)
            || self.card_menu.take().is_some()
            || self.confirm_delete.take().is_some()
            || self.draft.take().is_some()
        {
            cx.notify();
            true
        } else {
            false
        }
    }
}

impl Render for CountdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let hide_group = self.hide_group;
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
        let modal = self.draft.is_some().then(|| self.render_draft(window, cx));

        v_flex()
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("Countdown")
            .on_action(cx.listener(|this, _: &actions::DismissTransient, _, cx| {
                if !this.dismiss_transient(cx) {
                    cx.propagate();
                }
            }))
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(
                h_flex()
                    .mt(layout::header_inset())
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
                            .child(
                                IconButton::new("countdown-add", IconName::Plus)
                                    .icon_size(IconSize::XLarge)
                                    .icon_color(Color::Default)
                                    .tooltip(Tooltip::text(strings.countdown_new))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.add_menu = !this.add_menu;
                                        cx.notify();
                                    })),
                            )
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
                            .child(
                                IconButton::new("countdown-more", IconName::Ellipsis)
                                    .icon_color(Color::Default)
                                    .tooltip(Tooltip::text(strings.more))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.header_menu = !this.header_menu;
                                        cx.notify();
                                    })),
                            )
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
                                                    let next = !hide_group;
                                                    this.hide_group = next;
                                                    if let Err(err) = this.store.set_setting(HIDE_GROUP_SETTING, if next { "1" } else { "0" }) {
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
                    .when(!selected, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                    .child(Label::new(label).color(if selected { Color::Accent } else { Color::Muted }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = filter;
                        cx.notify();
                    }))
            }))))
            .child(
                v_scroll(
                    "countdown-grid-scroll",
                    div()
                        .id("countdown-grid")
                        .px(px(22.))
                        .when(cards.is_empty(), |el| {
                            let default_kind = match self.filter {
                                Filter::All => CountdownKind::Countdown,
                                Filter::Kind(k) => k,
                            };
                            el.child(
                                div().w_full().py(px(40.)).child(
                                    EmptyState::new(IconName::FernCountdown, strings.countdown_empty).action(
                                        "countdown-empty-add",
                                        strings.countdown_new,
                                        cx.listener(move |this, _, window, cx| {
                                            this.start_new_kind(default_kind, window, cx);
                                        }),
                                    ),
                                ),
                            )
                        })
                        .child(div().flex().flex_wrap().gap(px(20.)).children(cards)),
                )
                .flex_1()
                .min_h_0(),
            )
            .children(modal)
            .when_some(self.confirm_delete.clone(), |el, (id, name)| {
                el.child(
                    ConfirmDialog::new("countdown-delete-modal", strings.confirm_delete_countdown)
                        .width(px(340.))
                        .description(name)
                        .cancel_label(strings.cancel)
                        .confirm_label(strings.delete)
                        .destructive(true)
                        .on_cancel(cx.listener(|this, _, _, cx| {
                            this.confirm_delete = None;
                            cx.notify();
                        }))
                        .on_confirm(cx.listener(move |this, _, _, cx| {
                            this.confirm_delete = None;
                            this.delete(id, cx);
                        })),
                )
            })
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use fern_core::Frequency;
    use gpui::{Modifiers, TestAppContext, size};

    #[gpui::test]
    fn save_click_keeps_invalid_dates_until_corrected_for_every_countdown_kind(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            calendar::set_date_format(Some(DateFormat::IsoYearMonthDay));
            assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|_, cx| CountdownView::new(store.clone(), cx));
        cx.simulate_resize(size(px(900.), px(700.)));
        let select_all = if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" };
        for (index, kind) in [CountdownKind::Countdown, CountdownKind::Anniversary, CountdownKind::Birthday, CountdownKind::Holiday].into_iter().enumerate() {
            cx.update(|window, cx| view.update(cx, |view, cx| {
                view.start_new_kind(kind, window, cx);
                let draft = view.draft.as_mut().unwrap();
                draft.name.update(cx, |input, cx| input.set_text("Date validation", cx));
                draft.set_date(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap(), cx);
            }));
            if matches!(kind, CountdownKind::Birthday | CountdownKind::Holiday) {
                let trigger = cx.debug_bounds("countdown-date-popup-trigger").unwrap().center();
                cx.simulate_click(trigger, Modifiers::default());
            }
            let month = cx.debug_bounds(DateField::Month.id()).unwrap().center();
            cx.simulate_click(month, Modifiers::default());
            cx.simulate_keystrokes(select_all);
            cx.simulate_input("13");
            let save = cx.debug_bounds("countdown-save").unwrap().center();
            cx.simulate_click(save, Modifiers::default());
            assert_eq!(store.countdowns().unwrap().len(), index, "Save must reject the invalid {kind:?} date");
            cx.update(|_, cx| {
                let draft = view.read(cx).draft.as_ref().unwrap();
                assert_eq!(draft.date_inputs.fields[DateField::Month as usize].read(cx).text(), "13");
            });

            let day = cx.debug_bounds(DateField::Day.id()).unwrap().center();
            cx.simulate_click(day, Modifiers::default());
            cx.simulate_keystrokes("enter");
            cx.simulate_click(save, Modifiers::default());
            assert_eq!(store.countdowns().unwrap().len(), index, "blurring another field must not erase the invalid month");

            let month = cx.debug_bounds(DateField::Month.id()).unwrap().center();
            cx.simulate_click(month, Modifiers::default());
            cx.simulate_keystrokes(select_all);
            cx.simulate_input("12");
            cx.simulate_click(save, Modifiers::default());
            let countdowns = store.countdowns().unwrap();
            assert_eq!(countdowns.len(), index + 1);
            assert_eq!(countdowns.last().unwrap().date, NaiveDate::from_ymd_opt(2026, 12, 16).unwrap());
        }
        cx.update(|_, _| calendar::set_date_format(None));
    }

    #[gpui::test]
    fn countdown_kinds_share_the_ordered_date_editor(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::ZhCn));
            calendar::set_date_format(None);
            assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|_, cx| CountdownView::new(store, cx));
        cx.simulate_resize(size(px(900.), px(600.)));

        for kind in [CountdownKind::Countdown, CountdownKind::Anniversary, CountdownKind::Birthday, CountdownKind::Holiday] {
            cx.update(|window, cx| view.update(cx, |view, cx| view.start_new_kind(kind, window, cx)));
            if matches!(kind, CountdownKind::Birthday | CountdownKind::Holiday) {
                assert!(cx.debug_bounds(DateField::Year.id()).is_none());
                let trigger = cx.debug_bounds("countdown-date-popup-trigger").unwrap().center();
                cx.simulate_click(trigger, Modifiers::default());
            }
            let year = cx.debug_bounds(DateField::Year.id()).unwrap();
            let month = cx.debug_bounds(DateField::Month.id()).unwrap();
            let day = cx.debug_bounds(DateField::Day.id()).unwrap();
            assert!(year.right() <= month.left() && month.right() <= day.left());
        }

        cx.update(|window, cx| {
            calendar::set_date_format(Some(DateFormat::DayMonthYear));
            view.update(cx, |view, cx| view.start_new_kind(CountdownKind::Anniversary, window, cx));
        });
        let year = cx.debug_bounds(DateField::Year.id()).unwrap();
        let month = cx.debug_bounds(DateField::Month.id()).unwrap();
        let day = cx.debug_bounds(DateField::Day.id()).unwrap();
        assert!(day.right() <= month.left() && month.right() <= year.left());
        cx.update(|_, _| calendar::set_date_format(None));
    }

    #[gpui::test]
    fn birthday_accepts_a_distant_year_and_keeps_leap_day_valid(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            calendar::set_date_format(Some(DateFormat::IsoYearMonthDay));
            assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CountdownView::new(store.clone(), cx);
            view.start_new_kind(CountdownKind::Birthday, window, cx);
            let draft = view.draft.as_mut().unwrap();
            draft.name.update(cx, |input, cx| input.set_text("Birthday", cx));
            draft.set_date(NaiveDate::from_ymd_opt(2028, 2, 29).unwrap(), cx);
            view
        });
        cx.simulate_resize(size(px(900.), px(600.)));
        let trigger = cx.debug_bounds("countdown-date-popup-trigger").unwrap().center();
        cx.simulate_click(trigger, Modifiers::default());
        let year_dropdown = cx.debug_bounds("countdown-date-year-toggle").unwrap().center();
        cx.simulate_click(year_dropdown, Modifiers::default());
        assert!(cx.debug_bounds("countdown-date-options").is_some());

        let select_all = if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" };
        cx.simulate_keystrokes(select_all);
        cx.simulate_input("1910");
        cx.simulate_keystrokes("enter");
        assert!(cx.debug_bounds("countdown-date-options").is_none());
        cx.update(|window, cx| {
            let draft = view.read(cx).draft.as_ref().unwrap();
            assert_eq!(draft.date, NaiveDate::from_ymd_opt(1910, 2, 28).unwrap());
            assert!(draft.date_inputs.fields[DateField::Month as usize].focus_handle(cx).is_focused(window));
        });

        cx.simulate_keystrokes(select_all);
        cx.simulate_input("13");
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| view.update(cx, |view, cx| view.save_draft(cx)));
        assert!(store.countdowns().unwrap().is_empty());
        cx.simulate_keystrokes("escape");
        assert!(cx.debug_bounds(DateField::Year.id()).is_none());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| view.save_draft(cx));
            calendar::set_date_format(None);
        });
        let countdowns = store.countdowns().unwrap();
        assert_eq!(countdowns.len(), 1);
        assert_eq!(countdowns[0].date, NaiveDate::from_ymd_opt(1910, 2, 28).unwrap());
        assert_eq!(countdowns[0].repeat.as_ref().unwrap().frequency, Frequency::Yearly);
    }

    #[gpui::test]
    fn countdown_mutations_emit_data_changed_and_empty_name_preserves_draft(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|_, cx| CountdownView::new(store.clone(), cx));
        let bumps = Rc::new(std::cell::Cell::new(0usize));
        let bumps_obs = bumps.clone();
        let _observer = cx.update(|_, cx| {
            cx.new(|cx| {
                cx.observe_global::<DataVersion>(move |_: &mut (), cx| {
                    if changed_elsewhere(cx.entity_id(), cx) {
                        bumps_obs.set(bumps_obs.get() + 1);
                    }
                })
                .detach();
            })
        });

        // 1. Saving with an empty/whitespace name must preserve the draft rather than dropping it.
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.start_new_kind(CountdownKind::Anniversary, window, cx);
                let draft = v.draft.as_mut().unwrap();
                draft.name.update(cx, |inp, cx| inp.set_text("   ", cx));
                v.save_draft(cx);
                assert!(v.draft.is_some(), "empty name must not destroy draft");
                assert!(v.countdowns.is_empty());
            });
        });
        assert_eq!(bumps.get(), 0);

        // 2. Create emits data_changed.
        cx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.draft.as_ref().unwrap().name.update(cx, |inp, cx| inp.set_text("Launch Day", cx));
                v.save_draft(cx);
                assert!(v.draft.is_none());
            });
        });
        assert_eq!(bumps.get(), 1, "create must emit data_changed");
        let id = store.countdowns().unwrap()[0].id;

        // 3. Edit emits data_changed.
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.start_edit(id, window, cx);
                v.draft.as_ref().unwrap().name.update(cx, |inp, cx| inp.set_text("Launch Day v2", cx));
                v.save_draft(cx);
            });
        });
        assert_eq!(bumps.get(), 2, "edit must emit data_changed");

        // 4. Pin emits data_changed.
        cx.update(|_, cx| view.update(cx, |v, cx| v.toggle_pin(id, false, cx)));
        assert_eq!(bumps.get(), 3, "pin must emit data_changed");

        // 5. Archive emits data_changed.
        cx.update(|_, cx| view.update(cx, |v, cx| v.toggle_archive(id, false, cx)));
        assert_eq!(bumps.get(), 4, "archive must emit data_changed");

        // 6. Delete emits data_changed.
        cx.update(|_, cx| view.update(cx, |v, cx| v.delete(id, cx)));
        assert_eq!(bumps.get(), 5, "delete must emit data_changed");
        assert!(store.countdowns().unwrap().is_empty());
    }

    #[gpui::test]
    fn new_inactive_skips_initial_reload_until_activated(cx: &mut TestAppContext) {
        use gpui::AppContext;
        let store = Rc::new(Store::open_in_memory().unwrap());
        let target = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        store.add_countdown("Year End", target, CountdownKind::Countdown).unwrap();

        cx.update(|cx| {
            let view = cx.new(|cx| CountdownView::new_inactive(store.clone(), cx));
            {
                let v = view.read(cx);
                assert!(!v.active);
                assert!(v.dirty);
                assert!(v.countdowns.is_empty());
            }

            view.update(cx, |v, cx| v.set_active(true, cx));
            let v = view.read(cx);
            assert!(v.active);
            assert!(!v.dirty);
            assert_eq!(v.countdowns.len(), 1);
            assert_eq!(v.countdowns[0].name, "Year End");
        });
    }
}
