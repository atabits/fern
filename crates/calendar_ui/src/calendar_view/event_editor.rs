use chrono::{Local, NaiveDate, NaiveTime};
use desktop_common::text_input::{Changed, Mode, Submitted, TextInput};
use fern_core::Id;
use gpui::{Focusable, FontWeight};
use ui::prelude::*;
use ui::Switch;

use crate::calendar;
use crate::drag::DraggedEvent;
use crate::i18n;

use super::{CalendarView, EventBar};

fn unfold_ics_lines(ics: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in ics.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t'))
            && let Some(prev) = out.last_mut()
        {
            prev.push_str(rest);
            continue;
        }
        out.push(line.to_owned());
    }
    out
}

/// Returns `true` if any top-level `VEVENT` property in `ics` defines recurrence state
/// (`RRULE`, `RDATE`, `EXDATE`, or `RECURRENCE-ID`) that single-event `EventDraft` serialization
/// would otherwise clobber.
pub(super) fn ics_has_recurrence(ics: &str) -> bool {
    let mut in_vevent = false;
    let mut nested_depth = 0usize;
    for line in unfold_ics_lines(ics) {
        let upper = line.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" {
            in_vevent = true;
            nested_depth = 0;
            continue;
        }
        if upper == "END:VEVENT" {
            in_vevent = false;
            continue;
        }
        if !in_vevent {
            continue;
        }
        if upper.starts_with("BEGIN:") {
            nested_depth += 1;
            continue;
        }
        if upper.starts_with("END:") {
            nested_depth = nested_depth.saturating_sub(1);
            continue;
        }
        if nested_depth > 0 {
            continue;
        }
        let prop = upper
            .split_once(':')
            .map_or(upper.as_str(), |(lhs, _)| lhs)
            .split_once(';')
            .map_or_else(
                || upper.split_once(':').map_or(upper.as_str(), |(lhs, _)| lhs),
                |(name, _)| name,
            );
        if matches!(prop, "RRULE" | "RDATE" | "EXDATE" | "RECURRENCE-ID") {
            return true;
        }
    }
    false
}

/// Extracts the IANA timezone (`TZID=...` or `UTC` for `Z`-suffixed instants) from a `VEVENT`'s
/// `DTSTART`, returning `None` for floating local times or all-day `DATE` values.
pub(super) fn extract_ics_tzid(ics: &str) -> Option<String> {
    let mut in_vevent = false;
    let mut nested_depth = 0usize;
    for line in unfold_ics_lines(ics) {
        let upper = line.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" {
            in_vevent = true;
            nested_depth = 0;
            continue;
        }
        if upper == "END:VEVENT" {
            in_vevent = false;
            continue;
        }
        if !in_vevent {
            continue;
        }
        if upper.starts_with("BEGIN:") {
            nested_depth += 1;
            continue;
        }
        if upper.starts_with("END:") {
            nested_depth = nested_depth.saturating_sub(1);
            continue;
        }
        if nested_depth > 0 {
            continue;
        }
        let Some((lhs, value)) = line.split_once(':') else { continue };
        let mut parts = lhs.split(';');
        let Some(name) = parts.next() else { continue };
        if !name.eq_ignore_ascii_case("DTSTART") {
            continue;
        }
        for param in parts {
            if let Some((k, v)) = param.split_once('=')
                && k.eq_ignore_ascii_case("TZID")
            {
                let trimmed = v.trim().trim_matches('"');
                if let Some(zone) = calendar::canonical_timezone_name(trimmed) {
                    return Some(zone.to_owned());
                }
            }
        }
        if value.trim().ends_with('Z') {
            return Some("UTC".to_owned());
        }
        return None;
    }
    None
}

pub(super) struct EventEditor {
    pub(super) subscription_id: Id,
    pub(super) uid: Option<String>,
    pub(super) span_days: i64,
    pub(super) timezone: Option<String>,
    pub(super) error: Option<String>,
    pub(super) title: gpui::Entity<TextInput>,
    pub(super) location: gpui::Entity<TextInput>,
    pub(super) notes: gpui::Entity<TextInput>,
    pub(super) date: gpui::Entity<TextInput>,
    pub(super) start_time: gpui::Entity<TextInput>,
    pub(super) end_time: gpui::Entity<TextInput>,
    pub(super) all_day: bool,
    pub(super) _subs: Vec<gpui::Subscription>,
}

impl CalendarView {
    fn event_editor_subs(
        inputs: &[&gpui::Entity<TextInput>],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::Subscription> {
        let mut subs = Vec::with_capacity(inputs.len() * 2);
        for input in inputs {
            subs.push(cx.subscribe_in(input, window, |this, _, _: &Submitted, window, cx| {
                this.save_event_editor(window, cx);
            }));
            subs.push(cx.subscribe(input, |this, _, _: &Changed, cx| {
                if let Some(ed) = &mut this.event_editor {
                    ed.error = None;
                }
                cx.notify();
            }));
        }
        subs
    }

    pub(super) fn open_new_event_editor(&mut self, date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sub_id) = crate::subscriptions::feeds(cx)
            .and_then(|feeds| feeds.read(cx).writable_calendars().first().map(|c| c.id))
        else {
            return;
        };
        let date_str = date.format("%Y-%m-%d").to_string();
        let title = cx.new(|cx| TextInput::new(Mode::SingleLine, "", cx).without_cancel());
        let location = cx.new(|cx| TextInput::new(Mode::SingleLine, "", cx).without_cancel());
        let notes = cx.new(|cx| TextInput::new(Mode::MultiLine, "", cx).without_cancel());
        let date_in = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "YYYY-MM-DD", cx).without_cancel();
            input.set_text(date_str, cx);
            input
        });
        let start_in = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "09:00", cx).without_cancel();
            input.set_text("09:00", cx);
            input
        });
        let end_in = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "10:00", cx).without_cancel();
            input.set_text("10:00", cx);
            input
        });
        let _subs = Self::event_editor_subs(&[&title, &location, &date_in, &start_in, &end_in], window, cx);
        window.focus(&title.focus_handle(cx), cx);
        self.open_event = None;
        self.event_editor = Some(EventEditor {
            subscription_id: sub_id,
            uid: None,
            span_days: 0,
            timezone: None,
            error: None,
            title,
            location,
            notes,
            date: date_in,
            start_time: start_in,
            end_time: end_in,
            all_day: false,
            _subs,
        });
        cx.notify();
    }

    pub(super) fn open_edit_event_editor(&mut self, event: &EventBar, window: &mut Window, cx: &mut Context<Self>) {
        let stored = crate::subscriptions::feeds(cx)
            .and_then(|feeds| feeds.read(cx).stored_event(event.subscription_id, &event.key));
        if event.recurring || stored.as_ref().is_some_and(|s| ics_has_recurrence(&s.ics)) {
            return;
        }
        let timezone = stored.as_ref().and_then(|s| extract_ics_tzid(&s.ics));
        let date_str = event.start.format("%Y-%m-%d").to_string();
        let start_str = event.start_time.map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
        let end_str = event.end_time.map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
        let title_text = event.title.clone();
        let location_text = event.location.clone();
        let notes_text = event.notes.clone();
        let title = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "", cx).without_cancel();
            input.set_text(title_text, cx);
            input
        });
        let location = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "", cx).without_cancel();
            input.set_text(location_text, cx);
            input
        });
        let notes = cx.new(|cx| {
            let mut input = TextInput::new(Mode::MultiLine, "", cx).without_cancel();
            input.set_text(notes_text, cx);
            input
        });
        let date_in = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "YYYY-MM-DD", cx).without_cancel();
            input.set_text(date_str, cx);
            input
        });
        let start_in = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "09:00", cx).without_cancel();
            input.set_text(start_str, cx);
            input
        });
        let end_in = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "10:00", cx).without_cancel();
            input.set_text(end_str, cx);
            input
        });
        let _subs = Self::event_editor_subs(&[&title, &location, &date_in, &start_in, &end_in], window, cx);
        window.focus(&title.focus_handle(cx), cx);
        self.open_event = None;
        self.event_editor = Some(EventEditor {
            subscription_id: event.subscription_id,
            uid: Some(event.key.clone()),
            span_days: (event.end - event.start).num_days().max(0),
            timezone,
            error: None,
            title,
            location,
            notes,
            date: date_in,
            start_time: start_in,
            end_time: end_in,
            all_day: event.start_time.is_none(),
            _subs,
        });
        cx.notify();
    }

    pub(super) fn save_event_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.event_editor.as_ref() else { return };
        let strings = i18n::t(cx);
        let title = editor.title.read(cx).text().trim().to_owned();
        if title.is_empty() {
            if let Some(ed) = self.event_editor.as_mut() {
                ed.error = Some(strings.cal_event_title.to_owned());
                window.focus(&ed.title.focus_handle(cx), cx);
            }
            cx.notify();
            return;
        }
        let subscription_id = editor.subscription_id;
        let uid = editor.uid.clone();
        let span_days = editor.span_days.max(0);
        let all_day = editor.all_day;
        let preserved_timezone = editor.timezone.clone();
        let location = editor.location.read(cx).text().trim().to_owned();
        let notes = editor.notes.read(cx).text().trim().to_owned();
        let date_raw = editor.date.read(cx).text().trim().to_owned();
        let start_raw = editor.start_time.read(cx).text().trim().to_owned();
        let end_raw = editor.end_time.read(cx).text().trim().to_owned();

        let today = Local::now().date_naive();
        let Some(date) = NaiveDate::parse_from_str(&date_raw, "%Y-%m-%d")
            .ok()
            .or_else(|| calendar::parse_date(&date_raw, today, strings))
        else {
            if let Some(ed) = self.event_editor.as_mut() {
                ed.error = Some(strings.calendar_invalid_year_date.to_owned());
                window.focus(&ed.date.focus_handle(cx), cx);
            }
            cx.notify();
            return;
        };
        let parse_hhmm = |s: &str| {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            calendar::parse_time(s)
                .or_else(|| NaiveTime::parse_from_str(s, "%H:%M").ok())
                .or_else(|| NaiveTime::parse_from_str(s, "%H:%M:%S").ok())
        };
        let (local_start_time, local_end_time) = if all_day {
            (None, None)
        } else {
            let Some(st) = parse_hhmm(&start_raw) else {
                if let Some(ed) = self.event_editor.as_mut() {
                    ed.error = Some(format!("{}: {}", strings.range_start, strings.time_placeholder));
                    window.focus(&ed.start_time.focus_handle(cx), cx);
                }
                cx.notify();
                return;
            };
            let Some(et) = parse_hhmm(&end_raw) else {
                if let Some(ed) = self.event_editor.as_mut() {
                    ed.error = Some(format!("{}: {}", strings.range_end, strings.time_placeholder));
                    window.focus(&ed.end_time.focus_handle(cx), cx);
                }
                cx.notify();
                return;
            };
            (Some(st), Some(et))
        };
        let effective_all_day = all_day;
        let local_end_date = date + chrono::Duration::days(span_days);

        let Some(feeds) = crate::subscriptions::feeds(cx) else {
            if let Some(ed) = self.event_editor.as_mut() {
                ed.error = Some(strings.task_save_failed.to_owned());
            }
            cx.notify();
            return;
        };
        // Never overwrite a recurring master event with a single-event draft (would strip RRULE).
        if let Some(existing_uid) = uid.as_deref()
            && feeds
                .read(cx)
                .stored_event(subscription_id, existing_uid)
                .is_some_and(|stored| ics_has_recurrence(&stored.ics))
        {
            if let Some(ed) = self.event_editor.as_mut() {
                ed.error = Some(strings.filter_date_recurring.to_owned());
            }
            cx.notify();
            return;
        }

        let (start_date, start_time, end_date, end_time, timezone) = if effective_all_day {
            (date, None, local_end_date, None, None)
        } else if let Some(tz) = preserved_timezone
            .as_deref()
            .and_then(calendar::canonical_timezone_name)
        {
            let st = local_start_time.unwrap_or(NaiveTime::MIN);
            let et = local_end_time.unwrap_or(st);
            let adjusted_end_date = if span_days == 0 && et < st {
                date + chrono::Duration::days(1)
            } else {
                local_end_date
            };
            let (z_sd, z_st) = calendar::local_to_timezone(date, st, tz);
            let (z_ed, z_et) = calendar::local_to_timezone(adjusted_end_date, et, tz);
            (z_sd, Some(z_st), z_ed.max(z_sd), Some(z_et), Some(tz.to_owned()))
        } else {
            (date, local_start_time, local_end_date, local_end_time, None)
        };

        let draft = fern_core::ical::EventDraft {
            uid: uid.unwrap_or_else(|| Id::now_v7().to_string()),
            title,
            location,
            notes,
            start_date,
            start_time,
            end_date,
            end_time,
            all_day: effective_all_day,
            timezone,
        };
        let save_result = feeds.update(cx, |feeds, cx| feeds.save_event(subscription_id, &draft, cx));
        match save_result {
            Ok(()) => {
                self.event_editor = None;
                window.focus(&self.focus_handle, cx);
            }
            Err(err) => {
                eprintln!("failed to save CalDAV event: {err:#}");
                if let Some(ed) = self.event_editor.as_mut() {
                    ed.error = Some(format!("{err:#}"));
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_event_editor_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = self.event_editor.as_ref()?;
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let title_label = if editor.uid.is_some() { strings.cal_edit_event } else { strings.cal_new_event };
        let can_save = !editor.title.read(cx).text().trim().is_empty();
        let field = |label: &'static str, input: gpui::Entity<TextInput>, height: f32| {
            v_flex()
                .gap_1()
                .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted))
                .child(
                    div()
                        .min_h(px(height))
                        .px_2p5()
                        .py_1()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.colors().border)
                        .bg(theme.colors().background)
                        .child(input),
                )
        };
        Some(
            Modal::new("calendar-event-editor-backdrop")
                .width(px(380.))
                .padding(px(16.))
                .gap(px(12.))
                .on_dismiss(cx.listener(|this, _, window, cx| {
                    this.event_editor = None;
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                }))
                .child(
                    h_flex()
                        .child(div().flex_1().child(Label::new(title_label).weight(FontWeight::SEMIBOLD)))
                        .child(
                            IconButton::new("event-editor-close", IconName::X)
                                .tooltip(Tooltip::text(strings.close))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.event_editor = None;
                                    window.focus(&this.focus_handle, cx);
                                    cx.notify();
                                })),
                        ),
                )
                .child(field(strings.cal_event_title, editor.title.clone(), 30.))
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(field(strings.filter_date, editor.date.clone(), 30.)))
                        .when(!editor.all_day, |el| {
                            el.child(div().w(px(96.)).child(field(strings.range_start, editor.start_time.clone(), 30.)))
                                .child(div().w(px(96.)).child(field(strings.range_end, editor.end_time.clone(), 30.)))
                        }),
                )
                .child(
                    h_flex()
                        .id("event-editor-all-day")
                        .cursor_pointer()
                        .child(div().flex_1().child(Label::new(strings.cal_all_day).size(LabelSize::Small)))
                        .child(Switch::new().toggle_state(editor.all_day))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(ed) = &mut this.event_editor {
                                ed.all_day = !ed.all_day;
                                ed.error = None;
                                if !ed.all_day {
                                    if ed.start_time.read(cx).text().trim().is_empty() {
                                        ed.start_time.update(cx, |inp, cx| inp.set_text("09:00", cx));
                                    }
                                    if ed.end_time.read(cx).text().trim().is_empty() {
                                        ed.end_time.update(cx, |inp, cx| inp.set_text("10:00", cx));
                                    }
                                }
                                cx.notify();
                            }
                        })),
                )
                .child(field(strings.location, editor.location.clone(), 30.))
                .child(field(strings.notes_placeholder, editor.notes.clone(), 68.))
                .when_some(editor.error.clone(), |el, err| {
                    el.child(Label::new(err).size(LabelSize::Small).color(Color::Error))
                })
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("event-editor-cancel", strings.cancel)
                                .variant(ButtonVariant::Secondary)
                                .size(ButtonSize::Sm)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.event_editor = None;
                                    window.focus(&this.focus_handle, cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("event-editor-save", strings.save)
                                .variant(ButtonVariant::Primary)
                                .size(ButtonSize::Sm)
                                .disabled(!can_save)
                                .on_click(cx.listener(|this, _, window, cx| this.save_event_editor(window, cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn move_event_to_day(&mut self, dragged: &DraggedEvent, target_day: NaiveDate, cx: &mut Context<Self>) {
        let range = self.events_between(self.anchor - chrono::Duration::days(365), self.anchor + chrono::Duration::days(365), cx);
        let Some(existing) = range.into_iter().find(|e| e.subscription_id == dragged.subscription_id && e.key == dragged.uid) else {
            return;
        };
        let Some(feeds) = crate::subscriptions::feeds(cx) else {
            return;
        };
        let stored = feeds.read(cx).stored_event(dragged.subscription_id, &dragged.uid);
        // Never clobber a recurring master series via single-event drag-move.
        if existing.recurring || stored.as_ref().is_some_and(|s| ics_has_recurrence(&s.ics)) {
            return;
        }
        let span = (existing.end - existing.start).num_days().max(0);
        let target_end_day = target_day + chrono::Duration::days(span);
        let preserved_tz = stored
            .as_ref()
            .and_then(|s| extract_ics_tzid(&s.ics))
            .and_then(|z| calendar::canonical_timezone_name(&z));
        let (start_date, start_time, end_date, end_time, timezone) = match (existing.start_time, preserved_tz) {
            (Some(st), Some(tz)) => {
                let et = existing.end_time.unwrap_or_else(|| {
                    let end_dt = target_end_day.and_time(st) + chrono::Duration::hours(1);
                    end_dt.time()
                });
                let adjusted_end_day = if span == 0 && et < st {
                    target_day + chrono::Duration::days(1)
                } else {
                    target_end_day
                };
                let (z_sd, z_st) = calendar::local_to_timezone(target_day, st, tz);
                let (z_ed, z_et) = calendar::local_to_timezone(adjusted_end_day, et, tz);
                (z_sd, Some(z_st), z_ed.max(z_sd), Some(z_et), Some(tz.to_owned()))
            }
            _ => (target_day, existing.start_time, target_end_day, existing.end_time, None),
        };
        let draft = fern_core::ical::EventDraft {
            uid: existing.key,
            title: existing.title,
            location: existing.location,
            notes: existing.notes,
            start_date,
            start_time,
            end_date,
            end_time,
            all_day: existing.start_time.is_none(),
            timezone,
        };
        feeds.update(cx, |feeds, cx| {
            if let Err(err) = feeds.save_event(dragged.subscription_id, &draft, cx) {
                eprintln!("failed to move CalDAV event: {err:#}");
            }
        });
        cx.notify();
    }
}
