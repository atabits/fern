use chrono::{Datelike, NaiveDate};
use gpui::FontWeight;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::calendar;
use crate::i18n;
use crate::task_rows;

use super::{AGENDA_DAYS, CalendarView, EventBar, OpenTask};

impl CalendarView {
    /// An event's row in the Agenda, as a task's: its time, the calendar's dot and a card in the
    /// calendar's color, which opens the event's card.
    fn agenda_event_row(&self, event: EventBar, date: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let when = event.start_time.filter(|_| event.start == date).map_or(strings.range_all_day.to_owned(), |time| calendar::format_time(time, strings, cx));
        let key = (event.key.clone(), event.start);
        let open = self.open_event.as_ref() == Some(&key);
        h_flex()
            .gap_3()
            .child(div().w(px(72.)).flex_none().child(Label::new(when).size(LabelSize::Small).color(Color::Muted)))
            .child(div().w(px(16.)).flex_none().flex().justify_center().child(div().size(px(10.)).rounded_full().bg(event.color)))
            .child(
                h_flex()
                    .id(SharedString::from(format!("agenda-event-{}-{}", event.key, event.start)))
                    .flex_1()
                    .min_w_0()
                    .h(px(40.))
                    .px_3()
                    .rounded(px(6.))
                    .border_l_3()
                    .border_color(event.color)
                    .bg(event.color.opacity(0.12))
                    .hover(|s| s.bg(event.color.opacity(0.2)))
                    .cursor_pointer()
                    .child(div().flex_1().min_w_0().child(Label::new(event.title.clone()).truncate()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_event = if this.open_event.as_ref() == Some(&key) { None } else { Some(key.clone()) };
                        window.focus(&this.focus_handle, cx);
                        cx.notify();
                    }))
                    .when(open, |el| el.relative().child(PopoverAttachment::new(Placement::BelowRight, self.render_event_card(&event, cx)))),
            )
            .into_any_element()
    }

    /// The original's Agenda: a row per day — the day, "All Day" or the time, a circle and a
    /// card per task — with a heading where a new month starts, and today always shown.
    pub(super) fn render_agenda(&self, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let start = self.agenda_start();
        let mut days: std::collections::BTreeMap<
            NaiveDate,
            (Vec<fern_core::agenda::AgendaItem>, Vec<EventBar>),
        > = std::collections::BTreeMap::new();
        for (date, items) in &self.agenda {
            days.entry(*date).or_default().0 = items.clone();
        }
        if start == today {
            days.entry(today).or_default();
        }
        // The subscribed calendars' events on their first day (or the Agenda's first, for one
        // begun before), with a day of their own where there are no tasks.
        for event in self.events_between(start, start + chrono::Duration::days(AGENDA_DAYS), cx) {
            days.entry(event.start.max(start)).or_default().1.push(event);
        }
        let mut month = calendar::month_start(start);
        let mut rows: Vec<AnyElement> = Vec::new();
        for (date, (items, day_events)) in days {
            if calendar::month_start(date) != month {
                month = calendar::month_start(date);
                rows.push(
                    div()
                        .pt(px(28.))
                        .pb(px(10.))
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .child(calendar::month_title(month, strings))
                        .into_any_element(),
                );
            }
            let is_today = date == today;
            // The original's day label: the number semibold, the weekday beside it on its baseline
            // (the boxes' bottoms, less the number's deeper descent).
            let day = h_flex()
                .w(px(120.))
                .flex_none()
                .gap_2()
                .items_end()
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if is_today { theme.colors().text_accent } else { theme.colors().text })
                        .child(date.day().to_string()),
                )
                .child(
                    div().pb(px(4.)).child(
                        Label::new(strings.weekdays_short[date.weekday().num_days_from_monday() as usize])
                            .size(LabelSize::XSmall)
                            .weight(FontWeight::MEDIUM)
                            .color(if is_today { Color::Accent } else { Color::Muted }),
                    ),
                );
            let entries: Vec<AnyElement> = if items.is_empty() && day_events.is_empty() {
                vec![
                    h_flex()
                        .gap_3()
                        .child(div().w(px(72.)).flex_none().child(Label::new(strings.range_all_day).size(LabelSize::Small).color(Color::Muted)))
                        .child(div().size(px(10.)).flex_none().rounded_full().bg(theme.colors().element_hover))
                        .child(
                            div()
                                .flex_1()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(theme.colors().border_variant)
                                .bg(theme.colors().surface_background)
                                .child(EmptyState::new(IconName::CalendarCheck, strings.no_tasks_today)),
                        )
                        .into_any_element(),
                ]
            } else {
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let id = item.task;
                        // Countdowns, as in the original: an orange card and a filled dot instead of
                        // the task's circle; a click shows the countdown's card.
                        let countdown = item.countdown;
                        let item_color = item.color.clone();
                        let tint = if countdown {
                            theme.status().warning
                        } else {
                            task_rows::priority_color(item.priority, &theme).unwrap_or(theme.colors().accent)
                        };
                        let when = item.time.map_or(strings.range_all_day.to_owned(), |time| calendar::format_time(time, strings, cx));
                        h_flex()
                            .gap_3()
                            .child(div().w(px(72.)).flex_none().child(Label::new(when).size(LabelSize::Small).color(Color::Muted)))
                            // As in the original: a countdown's round icon, a repeating task's repeat
                            // mark, or a task's circle.
                            .child(div().w(px(16.)).flex_none().flex().justify_center().child(if countdown {
                                div()
                                    .size(px(16.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(crate::countdown_view::countdown_color(item_color.as_deref(), cx).opacity(0.5))
                                    .text_size(px(9.))
                                    .child(match item.icon.clone() {
                                        Some(icon) => icon.into_any_element(),
                                        None => Icon::new(crate::countdown_view::kind_icon(item.kind.unwrap_or_default()))
                                            .size(IconSize::XSmall)
                                            .color(Color::Default)
                                            .into_any_element(),
                                    })
                                    .into_any_element()
                            } else if item.repeats {
                                Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Muted).into_any_element()
                            } else {
                                div().size(px(10.)).rounded_full().border_1().border_color(theme.colors().text_muted).into_any_element()
                            }))
                            .child(
                                h_flex()
                                    .id(SharedString::from(format!("agenda-{date}-{i}")))
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(40.))
                                    .pl_3()
                                    .pr_3()
                                    .gap_2()
                                    .rounded(px(6.))
                                    .border_l_3()
                                    .border_color(tint)
                                    .bg(tint.opacity(0.12))
                                    .hover(|s| s.bg(tint.opacity(0.2)))
                                    .cursor_pointer()
                                    .when(!countdown, |el| {
                                        el.child(
                                            div()
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                                .child(
                                                    Checkbox::new(SharedString::from(format!("agenda-check-{date}-{i}")))
                                                        .size(px(14.))
                                                        .tint(task_rows::priority_color(item.priority, &theme))
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.toggle_task_completed(id, true, cx);
                                                        })),
                                                ),
                                        )
                                    })
                                    .child(div().flex_1().min_w_0().child(Label::new(if item.title.trim().is_empty() { strings.untitled.to_owned() } else { item.title }).truncate()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if countdown {
                                            this.open_countdown = Some((date, id));
                                            window.focus(&this.focus_handle, cx);
                                            cx.notify();
                                        } else {
                                            cx.emit(OpenTask(id))
                                        }
                                    }))
                                    .when(!countdown, |el| {
                                        el.on_mouse_down(
                                            gpui::MouseButton::Right,
                                            cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                                                this.open_context(id, event.position, cx);
                                                window.focus(&this.focus_handle, cx);
                                            }),
                                        )
                                    })
                                    .when_some(
                                        self.open_countdown
                                            .filter(|open| *open == (date, id))
                                            .and_then(|_| self.countdowns.iter().find(|c| c.id == id)),
                                        |el, shown| {
                                            el.relative().child(PopoverAttachment::new(
                                                Placement::BelowRight,
                                                popover(cx)
                                                    .w(px(220.))
                                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                                        this.open_countdown = None;
                                                        cx.notify();
                                                    }))
                                                    .child(crate::countdown_view::render_summary(shown, date, cx))
                                                    .into_any_element(),
                                            ))
                                        },
                                    ),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .chain(day_events.into_iter().map(|event| self.agenda_event_row(event, date, cx)))
                    .collect()
            };
            rows.push(
                h_flex()
                    .py(px(30.))
                    .items_start()
                    .child(div().pt(px(6.)).child(day))
                    .child(v_flex().flex_1().min_w_0().gap_2().children(entries))
                    .into_any_element(),
            );
        }
        v_scroll(
            "agenda-scroll",
            v_flex()
                .id("agenda")
                .pb_6()
                .child(
                    v_flex()
                        .w_full()
                        .max_w(px(760.))
                        .mx_auto()
                        .px_6()
                        .pt(px(14.))
                        .when(rows.is_empty(), |el| {
                            el.child(
                                div()
                                    .w_full()
                                    .py_10()
                                    .child(EmptyState::new(IconName::CalendarCheck, strings.no_tasks_scheduled)),
                            )
                        })
                        .children(rows),
                ),
        )
        .flex_1()
        .min_h_0()
        .into_any_element()
    }
}
