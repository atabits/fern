//! Life Clock view: a meditative 50/50 split timeline (past vs. future) with live age readout,
//! rotating time quotes, and minimalist birth-date and appearance settings.

mod quotes;
#[cfg(test)]
mod tests;
mod timeline_canvas;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
#[cfg(not(test))]
use std::time::Duration;

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime};
use desktop_common::actions::DismissTransient;
use desktop_common::calendar;
use desktop_common::data_version::{DataScope, DataVersion, ViewRefreshState, data_changed_scoped};
use desktop_common::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};
use fern_core::life_clock::{
    TimelineSnapshot, compute_age, format_birth_date, parse_birth_date,
};
use fern_core::{DateFormat, Store};
use fern_i18n::{Locale, Strings};
use gpui::{
    Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, MouseDownEvent,
    Render, SharedString, Subscription, Window, canvas, div, px, relative,
};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::timeline_canvas::{
    GlyphCache, LifeClockPalette, TimelineCacheState, render_timeline_canvas,
};

const SETTING_BIRTH_DATE: &str = "life_clock_birth_date";
const SETTING_PALETTE: &str = "life_clock_palette";
const PALETTE_VALUE_CLASSIC: &str = "classic";
const PALETTE_VALUE_THEME: &str = "theme";

/// Target pacing intervals:
/// - Active window: 30 FPS (33 ms) — fastest row moves ~20 px/s (< 0.7 px/frame), avoiding
///   144/240 Hz GPU spin on high-refresh displays.
/// - Unfocused visible window: 15 FPS (66 ms) — still smooth on a side monitor at half the power.
/// - System Reduce Motion: 1 FPS (1000 ms).
const ACTIVE_FRAME_MS: u64 = 33;
const UNFOCUSED_FRAME_MS: u64 = 66;
const REDUCED_MOTION_FRAME_MS: u64 = 1000;

fn desired_frame_interval_ms(active: bool, reduce_motion: bool) -> u64 {
    if reduce_motion {
        REDUCED_MOTION_FRAME_MS
    } else if active {
        ACTIVE_FRAME_MS
    } else {
        UNFOCUSED_FRAME_MS
    }
}

/// Visual palette mode for the Life Clock screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LifeClockPaletteMode {
    #[default]
    Classic,
    FernTheme,
}

impl LifeClockPaletteMode {
    fn from_storage(value: Option<&str>) -> Self {
        match value {
            Some(PALETTE_VALUE_THEME) => Self::FernTheme,
            _ => Self::Classic,
        }
    }

    fn as_storage(self) -> &'static str {
        match self {
            Self::Classic => PALETTE_VALUE_CLASSIC,
            Self::FernTheme => PALETTE_VALUE_THEME,
        }
    }
}

/// Events emitted by [`LifeClockView`] to the desktop window shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifeClockEvent {
    ToggleSidebar,
    PaletteChanged(LifeClockPaletteMode),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BirthDatePart {
    Day,
    Month,
    Year,
}

impl BirthDatePart {
    fn ordered(strings: &Strings, cx: &App) -> [Self; 3] {
        Self::ordered_for_format(calendar::effective_date_format(strings, cx))
    }

    fn ordered_for_format(format: DateFormat) -> [Self; 3] {
        match format {
            DateFormat::MonthDayYear => [Self::Month, Self::Day, Self::Year],
            DateFormat::DayMonthYear => [Self::Day, Self::Month, Self::Year],
            DateFormat::YearMonthDay | DateFormat::IsoYearMonthDay => {
                [Self::Year, Self::Month, Self::Day]
            }
        }
    }

    fn label(self, strings: &Strings) -> &'static str {
        match self {
            Self::Day => strings.day,
            Self::Month => strings.month,
            Self::Year => strings.year,
        }
    }

    fn debug_name(self) -> &'static str {
        match self {
            Self::Day => "life-clock-birth-day",
            Self::Month => "life-clock-birth-month",
            Self::Year => "life-clock-birth-year",
        }
    }
}

pub struct LifeClockView {
    store: Rc<Store>,
    focus_handle: FocusHandle,
    birth_date: Option<NaiveDate>,
    palette_mode: LifeClockPaletteMode,
    quote_index: usize,
    sidebar_hidden: bool,
    popover_open: bool,
    invalid_date: bool,
    day_input: Entity<TextInput>,
    month_input: Entity<TextInput>,
    year_input: Entity<TextInput>,
    help_button_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    glyph_cache: GlyphCache,
    cached_age: Option<(i64, Option<NaiveDate>, Locale, SharedString)>,
    cached_quote: Option<(usize, Locale, SharedString)>,
    _subscriptions: Vec<Subscription>,
    #[cfg(not(test))]
    tick_timer: Option<(u64, gpui::Task<()>)>,
    refresh: ViewRefreshState,
}

impl EventEmitter<LifeClockEvent> for LifeClockView {}

impl Focusable for LifeClockView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl LifeClockView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, true, cx)
    }

    pub fn new_inactive(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, false, cx)
    }

    fn new_with_active(store: Rc<Store>, initial_active: bool, cx: &mut Context<Self>) -> Self {
        let birth_date = if initial_active {
            load_birth_date(&store)
        } else {
            None
        };
        let palette_mode = load_palette_mode(&store);
        let today = Local::now().date_naive();

        let day_input = cx.new(|cx| TextInput::new(Mode::SingleLine, "20", cx));
        let month_input = cx.new(|cx| TextInput::new(Mode::SingleLine, "11", cx));
        let year_input = cx.new(|cx| TextInput::new(Mode::SingleLine, "1998", cx));

        let mut subscriptions = Vec::new();
        for input in [&day_input, &month_input, &year_input] {
            subscriptions.push(cx.subscribe(input, |this, _, _: &Changed, cx| {
                this.validate_and_save_inputs(false, cx);
            }));
            subscriptions.push(cx.subscribe(input, |this, _, _: &Submitted, cx| {
                this.validate_and_save_inputs(true, cx);
            }));
            subscriptions.push(cx.subscribe(input, |this, _, _: &Cancelled, cx| {
                this.dismiss_transient(cx);
            }));
        }

        subscriptions.push(cx.observe_global::<DataVersion>(|this, cx| {
            if this.refresh.on_data_version(cx.entity_id(), cx) {
                this.reload(cx);
            }
        }));

        let mut view = Self {
            store,
            focus_handle: cx.focus_handle(),
            birth_date,
            palette_mode,
            quote_index: quotes::default_index_for_date(today),
            sidebar_hidden: false,
            popover_open: false,
            invalid_date: false,
            day_input,
            month_input,
            year_input,
            help_button_bounds: Rc::new(Cell::new(None)),
            glyph_cache: Rc::new(RefCell::new(TimelineCacheState::default())),
            cached_age: None,
            cached_quote: None,
            _subscriptions: subscriptions,
            #[cfg(not(test))]
            tick_timer: None,
            refresh: ViewRefreshState::new(initial_active, DataScope::PREFS, cx),
        };
        if initial_active {
            view.refresh.mark_clean(cx);
            view.sync_date_inputs(cx);
        }
        view
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.refresh.set_active(active, cx) {
            self.reload(cx);
        }
    }

    pub fn sync_external(&mut self, cx: &mut Context<Self>) {
        if self.refresh.mark_dirty_or_reload() {
            self.reload(cx);
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.refresh.mark_clean(cx);
        let new_birth = load_birth_date(&self.store);
        let new_palette = load_palette_mode(&self.store);
        let birth_changed = self.birth_date != new_birth;
        let palette_changed = self.palette_mode != new_palette;

        self.birth_date = new_birth;
        self.palette_mode = new_palette;
        if birth_changed || !self.popover_open {
            self.invalid_date = false;
            self.sync_date_inputs(cx);
        }
        if palette_changed {
            cx.emit(LifeClockEvent::PaletteChanged(new_palette));
        }
        cx.notify();
    }

    pub fn palette_mode(&self) -> LifeClockPaletteMode {
        self.palette_mode
    }

    pub fn set_sidebar_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        if self.sidebar_hidden != hidden {
            self.sidebar_hidden = hidden;
            cx.notify();
        }
    }

    pub fn dismiss_transient(&mut self, cx: &mut Context<Self>) -> bool {
        if self.popover_open {
            self.popover_open = false;
            self.invalid_date = false;
            self.sync_date_inputs(cx);
            cx.notify();
            return true;
        }
        false
    }

    #[cfg(debug_assertions)]
    pub fn dev_set_birth_date(&mut self, date: Option<NaiveDate>, cx: &mut Context<Self>) {
        if self.refresh.is_dirty() {
            self.reload(cx);
        }
        self.invalid_date = false;
        self.persist_birth_date(date, cx);
        self.sync_date_inputs(cx);
    }

    #[cfg(debug_assertions)]
    pub fn dev_set_palette(&mut self, mode: LifeClockPaletteMode, cx: &mut Context<Self>) {
        if self.refresh.is_dirty() {
            self.reload(cx);
        }
        self.set_palette(mode, cx);
    }

    #[cfg(debug_assertions)]
    pub fn dev_open_popover(&mut self, open: bool, cx: &mut Context<Self>) {
        if open {
            self.open_popover(cx);
        } else {
            self.dismiss_transient(cx);
        }
    }

    fn open_popover(&mut self, cx: &mut Context<Self>) {
        if self.refresh.is_dirty() {
            self.reload(cx);
        }
        if !self.popover_open {
            self.popover_open = true;
            self.invalid_date = false;
            self.sync_date_inputs(cx);
            cx.notify();
        }
    }

    fn toggle_popover(&mut self, cx: &mut Context<Self>) {
        if self.popover_open {
            self.dismiss_transient(cx);
        } else {
            self.open_popover(cx);
        }
    }

    fn cycle_quote(&mut self, cx: &mut Context<Self>) {
        self.quote_index = (self.quote_index + 1) % quotes::QUOTE_COUNT;
        cx.notify();
    }

    fn set_palette(&mut self, mode: LifeClockPaletteMode, cx: &mut Context<Self>) {
        if self.palette_mode == mode {
            return;
        }
        self.palette_mode = mode;
        let _ = self.store.set_setting(SETTING_PALETTE, mode.as_storage());
        data_changed_scoped(cx.entity_id(), DataScope::PREFS, cx);
        cx.emit(LifeClockEvent::PaletteChanged(mode));
        cx.notify();
    }

    fn persist_birth_date(&mut self, date: Option<NaiveDate>, cx: &mut Context<Self>) {
        self.birth_date = date;
        match date {
            Some(d) => {
                let _ = self
                    .store
                    .set_setting(SETTING_BIRTH_DATE, &format_birth_date(d));
            }
            None => {
                let _ = self.store.delete_setting(SETTING_BIRTH_DATE);
            }
        }
        data_changed_scoped(cx.entity_id(), DataScope::PREFS, cx);
        cx.notify();
    }

    fn clear_birth_date(&mut self, cx: &mut Context<Self>) {
        self.invalid_date = false;
        self.persist_birth_date(None, cx);
        self.sync_date_inputs(cx);
    }

    fn sync_date_inputs(&self, cx: &mut Context<Self>) {
        let (day, month, year) = match self.birth_date {
            Some(date) => (
                format!("{:02}", date.day()),
                format!("{:02}", date.month()),
                format!("{:04}", date.year()),
            ),
            None => (String::new(), String::new(), String::new()),
        };
        self.day_input.update(cx, |input, cx| input.set_text(day, cx));
        self.month_input
            .update(cx, |input, cx| input.set_text(month, cx));
        self.year_input
            .update(cx, |input, cx| input.set_text(year, cx));
    }

    fn validate_and_save_inputs(&mut self, strict: bool, cx: &mut Context<Self>) {
        let day_str = self.day_input.read(cx).text().trim().to_owned();
        let month_str = self.month_input.read(cx).text().trim().to_owned();
        let year_str = self.year_input.read(cx).text().trim().to_owned();

        if day_str.is_empty() && month_str.is_empty() && year_str.is_empty() {
            self.invalid_date = false;
            if self.birth_date.is_some() {
                self.persist_birth_date(None, cx);
            } else {
                cx.notify();
            }
            return;
        }

        let digits_only = [&day_str, &month_str, &year_str]
            .iter()
            .all(|s| s.bytes().all(|b| b.is_ascii_digit()));
        if !digits_only {
            self.invalid_date = true;
            cx.notify();
            return;
        }

        let month_bad = !month_str.is_empty()
            && !(month_str == "0" && !strict)
            && month_str
                .parse::<u32>()
                .ok()
                .is_none_or(|m| !(1..=12).contains(&m));
        let day_bad = !day_str.is_empty()
            && !(day_str == "0" && !strict)
            && day_str
                .parse::<u32>()
                .ok()
                .is_none_or(|d| !(1..=31).contains(&d));
        if month_bad || day_bad {
            self.invalid_date = true;
            cx.notify();
            return;
        }

        if day_str.is_empty()
            || month_str.is_empty()
            || year_str.len() < 4
            || (!strict && (day_str == "0" || month_str == "0"))
        {
            self.invalid_date = strict;
            cx.notify();
            return;
        }

        let today = Local::now().date_naive();
        let parsed = parse_ymd_past_or_today(&year_str, &month_str, &day_str, today);

        if let Some(date) = parsed {
            self.invalid_date = false;
            if self.birth_date != Some(date) {
                self.persist_birth_date(Some(date), cx);
            } else {
                cx.notify();
            }
        } else {
            self.invalid_date = true;
            cx.notify();
        }
    }

    fn input_for_part(&self, part: BirthDatePart) -> Entity<TextInput> {
        match part {
            BirthDatePart::Day => self.day_input.clone(),
            BirthDatePart::Month => self.month_input.clone(),
            BirthDatePart::Year => self.year_input.clone(),
        }
    }

    #[cfg(not(test))]
    fn schedule_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        if viewport.width <= px(0.) || viewport.height <= px(0.) {
            self.tick_timer = None;
            return;
        }
        let interval_ms = desired_frame_interval_ms(window.is_window_active(), cx.reduce_motion());
        // Reuse the in-flight one-shot timer unless we need a faster cadence (e.g. window
        // just became active while a slower 66ms/1000ms timer was pending).
        if let Some((pending_ms, _)) = &self.tick_timer
            && *pending_ms <= interval_ms
        {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(interval_ms))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.tick_timer = None;
                cx.notify();
            });
        });
        self.tick_timer = Some((interval_ms, task));
    }

    #[cfg(test)]
    fn schedule_tick(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn age_header_strings(
        &mut self,
        locale: Locale,
        strings: &'static Strings,
        now: NaiveDateTime,
    ) -> (SharedString, SharedString) {
        let Some(birth) = self.birth_date else {
            self.cached_age = None;
            return (
                SharedString::new_static(strings.life_clock_set_birth_title),
                SharedString::new_static(strings.life_clock_set_birth_hint),
            );
        };
        let sec_stamp = now.and_utc().timestamp();
        let subtitle = match &self.cached_age {
            Some((ts, cached_birth, cached_locale, text))
                if *ts == sec_stamp && *cached_birth == Some(birth) && *cached_locale == locale =>
            {
                text.clone()
            }
            _ => match compute_age(birth, now) {
                Some(age) => {
                    let formatted = SharedString::from(fern_i18n::format_life_clock_age(
                        locale,
                        age.years,
                        age.months,
                        age.days,
                        age.hours,
                        age.minutes,
                        age.seconds,
                    ));
                    self.cached_age = Some((sec_stamp, Some(birth), locale, formatted.clone()));
                    formatted
                }
                None => {
                    self.cached_age = None;
                    return (
                        SharedString::new_static(strings.life_clock_set_birth_title),
                        SharedString::new_static(strings.life_clock_set_birth_hint),
                    );
                }
            },
        };
        (SharedString::new_static(strings.life_clock_you_are), subtitle)
    }

    fn quote_header_strings(&mut self, locale: Locale) -> (SharedString, &'static str) {
        let quote = quotes::quote_for(locale, self.quote_index);
        let display = match &self.cached_quote {
            Some((idx, cached_locale, text))
                if *idx == self.quote_index && *cached_locale == locale =>
            {
                text.clone()
            }
            _ => {
                let formatted = SharedString::from(match locale {
                    Locale::Ru => format!("\u{00AB}{}\u{00BB}", quote.text),
                    Locale::En | Locale::ZhCn => format!("\u{201C}{}\u{201D}", quote.text),
                });
                self.cached_quote = Some((self.quote_index, locale, formatted.clone()));
                formatted
            }
        };
        (display, quote.author)
    }

    fn render_header(
        &mut self,
        locale: Locale,
        strings: &'static Strings,
        palette: LifeClockPalette,
        now: NaiveDateTime,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (age_heading, age_subtitle) = self.age_header_strings(locale, strings, now);
        let (quote_display, quote_author) = self.quote_header_strings(locale);

        h_flex()
            .absolute()
            .top(desktop_common::layout::title_bar_height(window, cx) + px(14.))
            .left_0()
            .right_0()
            .w_full()
            .items_start()
            .child(
                div()
                    .w(relative(0.5))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .pl(px(52.))
                    .pr(px(36.))
                    .child(
                        v_flex()
                            .id("life-clock-age")
                            .debug_selector(|| "life-clock-age".to_owned())
                            .w(px(340.))
                            .max_w_full()
                            .gap(px(2.))
                            .rounded(px(6.))
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .hover(|s| s.bg(palette.age_heading.opacity(0.08)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_popover(cx);
                            }))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .line_height(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(palette.age_heading)
                                    .child(age_heading),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(17.))
                                    .text_color(palette.age_value)
                                    .child(age_subtitle),
                            ),
                    ),
            )
            .child(
                div()
                    .w(relative(0.5))
                    .flex_none()
                    .pl(px(18.))
                    .pr(px(148.))
                    .child(
                        v_flex()
                            .id("life-clock-quote")
                            .debug_selector(|| "life-clock-quote".to_owned())
                            .max_w(px(330.))
                            .gap(px(4.))
                            .rounded(px(6.))
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .hover(|s| s.bg(palette.quote_text.opacity(0.05)))
                            .tooltip(Tooltip::text(strings.life_clock_quote_hint))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cycle_quote(cx);
                            }))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .line_height(px(17.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(palette.quote_text)
                                    .child(quote_display),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .line_height(px(16.))
                                    .text_color(palette.quote_author)
                                    .child(quote_author),
                            ),
                    ),
            )
    }

    fn render_sidebar_toggle(
        &self,
        strings: &'static Strings,
        palette: LifeClockPalette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tip = if self.sidebar_hidden {
            strings.life_clock_show_sidebar
        } else {
            strings.life_clock_hide_sidebar
        };
        let rail_fg = palette.rail_btn_fg;

        div()
            .absolute()
            .top(desktop_common::layout::title_bar_height(window, cx) + px(14.))
            .left(px(14.))
            .debug_selector(|| "life-clock-sidebar-toggle".to_owned())
            .child(
                IconButton::new("life-clock-sidebar-toggle", IconName::PanelLeft)
                    .size(px(28.))
                    .icon_size(IconSize::Medium)
                    .icon_color(rail_fg.opacity(0.30))
                    .hover_icon_color(rail_fg.opacity(0.85))
                    .hover_color(rail_fg.opacity(0.10))
                    .tooltip(Tooltip::text(tip))
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(LifeClockEvent::ToggleSidebar);
                    })),
            )
    }

    fn render_help_corner(
        &self,
        strings: &'static Strings,
        palette: LifeClockPalette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let help_bounds = self.help_button_bounds.clone();
        let help_fg = palette.quote_text;

        div()
            .absolute()
            .top(desktop_common::layout::title_bar_height(window, cx) + px(14.))
            .right(px(16.))
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .relative()
                            .debug_selector(|| "life-clock-help-button".to_owned())
                            .child(
                                IconButton::new(
                                    "life-clock-help-button",
                                    IconName::CircleQuestionMark,
                                )
                                .size(px(28.))
                                .icon_size(IconSize::Medium)
                                .icon_color(
                                    help_fg.opacity(if self.popover_open { 0.90 } else { 0.50 }),
                                )
                                .hover_icon_color(help_fg.opacity(0.90))
                                .hover_color(help_fg.opacity(0.08))
                                .tooltip(Tooltip::text(strings.life_clock_help_tooltip))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_popover(cx);
                                })),
                            )
                            .child(
                                canvas(
                                    move |bounds, _, _| {
                                        help_bounds.set(Some(bounds));
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            ),
                    )
                    .when(self.popover_open, |el| {
                        el.child(PopoverAttachment::new(
                            Placement::BelowRight,
                            self.render_popover(strings, cx),
                        ))
                    }),
            )
    }

    fn render_popover(
        &self,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let border_color = if self.invalid_date {
            cx.theme().status().error
        } else {
            colors.border
        };
        let help_bounds = self.help_button_bounds.clone();
        let parts = BirthDatePart::ordered(strings, cx);

        popover(cx)
            .debug_selector(|| "life-clock-popover".to_owned())
            .w(px(280.))
            .p_3()
            .gap_3()
            .on_mouse_down_out(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if !help_bounds
                    .get()
                    .is_some_and(|bounds| bounds.contains(&event.position))
                {
                    this.dismiss_transient(cx);
                }
            }))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(strings.life_clock_about_title)
                            .size(LabelSize::Default)
                            .weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(colors.text_muted)
                            .child(strings.life_clock_about_body),
                    ),
            )
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                Label::new(strings.life_clock_birth_date)
                                    .size(LabelSize::Small)
                                    .weight(FontWeight::MEDIUM),
                            )
                            .when(self.birth_date.is_some(), |el| {
                                el.child(
                                    div()
                                        .debug_selector(|| "life-clock-clear-birth".to_owned())
                                        .child(
                                            Button::new("life-clock-clear-birth", strings.clear)
                                                .variant(ButtonVariant::Ghost)
                                                .size(ButtonSize::Sm)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.clear_birth_date(cx);
                                                })),
                                        ),
                                )
                            }),
                    )
                    .child(
                        h_flex().gap_2().items_end().children(parts.into_iter().map(|part| {
                            let input = self.input_for_part(part);
                            let selector = part.debug_name();
                            v_flex()
                                .gap_1()
                                .when(part == BirthDatePart::Year, |el| el.w(px(92.)).flex_none())
                                .when(part != BirthDatePart::Year, |el| el.flex_1().min_w_0())
                                .child(
                                    Label::new(part.label(strings))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .child(
                                    div()
                                        .debug_selector(move || selector.to_owned())
                                        .h(px(28.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.))
                                        .bg(colors.surface_background)
                                        .border_1()
                                        .border_color(border_color)
                                        .text_size(px(13.))
                                        .child(input),
                                )
                        })),
                    )
                    .when(self.invalid_date, |el| {
                        el.child(
                            div()
                                .debug_selector(|| "life-clock-invalid-date".to_owned())
                                .child(
                                    Label::new(strings.life_clock_invalid_date)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Error),
                                ),
                        )
                    }),
            )
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        Label::new(strings.life_clock_palette)
                            .size(LabelSize::Small)
                            .weight(FontWeight::MEDIUM),
                    )
                    .child(
                        div()
                            .debug_selector(|| "life-clock-palette-tabs".to_owned())
                            .w_full()
                            .child(
                                SegmentedTabs::new()
                                    .full_width()
                                    .tab(
                                        SegmentedTab::new(
                                            "life-clock-palette-classic",
                                            strings.life_clock_palette_classic,
                                        )
                                        .toggle_state(
                                            self.palette_mode == LifeClockPaletteMode::Classic,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set_palette(LifeClockPaletteMode::Classic, cx);
                                        })),
                                    )
                                    .tab(
                                        SegmentedTab::new(
                                            "life-clock-palette-theme",
                                            strings.life_clock_palette_theme,
                                        )
                                        .toggle_state(
                                            self.palette_mode == LifeClockPaletteMode::FernTheme,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set_palette(LifeClockPaletteMode::FernTheme, cx);
                                        })),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .line_height(px(16.))
                    .text_color(colors.text_muted)
                    .child(strings.life_clock_quote_hint),
            )
    }
}

impl Render for LifeClockView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.schedule_tick(window, cx);
        if !self.popover_open
            && (self.day_input.focus_handle(cx).is_focused(window)
                || self.month_input.focus_handle(cx).is_focused(window)
                || self.year_input.focus_handle(cx).is_focused(window))
        {
            let focus = self.focus_handle.clone();
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }

        let locale = fern_i18n::locale(cx);
        let strings = locale.strings();
        let palette = LifeClockPalette::resolve(self.palette_mode, cx.theme());
        let now = Local::now().naive_local();
        let snapshot = TimelineSnapshot::from_datetime(now);

        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &DismissTransient, _, cx| {
                if !this.dismiss_transient(cx) {
                    cx.propagate();
                }
            }))
            .child(render_timeline_canvas(
                snapshot,
                locale,
                strings.life_clock_months,
                palette,
                self.glyph_cache.clone(),
                window,
                cx,
            ))
            .child(self.render_header(locale, strings, palette, now, window, cx))
            .child(self.render_sidebar_toggle(strings, palette, window, cx))
            .child(self.render_help_corner(strings, palette, window, cx))
    }
}

fn load_birth_date(store: &Store) -> Option<NaiveDate> {
    store
        .setting(SETTING_BIRTH_DATE)
        .ok()
        .flatten()
        .and_then(|value| parse_birth_date(&value))
}

fn load_palette_mode(store: &Store) -> LifeClockPaletteMode {
    let raw = store.setting(SETTING_PALETTE).ok().flatten();
    LifeClockPaletteMode::from_storage(raw.as_deref())
}

fn parse_ymd_past_or_today(
    year_str: &str,
    month_str: &str,
    day_str: &str,
    today: NaiveDate,
) -> Option<NaiveDate> {
    if year_str.len() != 4 {
        return None;
    }
    let year = year_str.parse::<i32>().ok()?;
    let month = month_str.parse::<u32>().ok()?;
    let day = day_str.parse::<u32>().ok()?;
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    (date <= today).then_some(date)
}
