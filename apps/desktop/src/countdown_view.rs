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
}

pub struct CountdownView {
    store: Rc<Store>,
    countdowns: Vec<Countdown>,
    filter: Filter,
    /// Cards switched to the "days only" format.
    days_only: HashSet<Id>,
    draft: Option<Draft>,
}

fn kind_icon(kind: CountdownKind) -> IconName {
    match kind {
        CountdownKind::Countdown => IconName::Hourglass,
        CountdownKind::Anniversary => IconName::Heart,
        CountdownKind::Holiday => IconName::Gift,
    }
}

fn kind_label(kind: CountdownKind, strings: &Strings) -> &'static str {
    match kind {
        CountdownKind::Countdown => strings.countdown_kind_countdown,
        CountdownKind::Anniversary => strings.countdown_kind_anniversary,
        CountdownKind::Holiday => strings.countdown_kind_holiday,
    }
}

fn format_date(date: NaiveDate, strings: &Strings) -> String {
    if strings.dotted_dates { date.format("%d.%m.%Y").to_string() } else { date.format("%m/%d/%Y").to_string() }
}

impl CountdownView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let mut this = Self { store, countdowns: Vec::new(), filter: Filter::All, days_only: HashSet::new(), draft: None };
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
        self.draft = Some(Draft { name, kind, date: today, month: calendar::month_start(today) });
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub fn dev_add(&mut self, name: &str, date: NaiveDate, kind: CountdownKind, cx: &mut Context<Self>) {
        self.store.add_countdown(name, date, kind).ok();
        self.reload(cx);
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.take() else { return };
        let name = draft.name.read(cx).text().to_owned();
        if let Err(err) = self.store.add_countdown(&name, draft.date, draft.kind) {
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
        let span = span(today, countdown.date);
        let color = if span.past { theme.status().warning } else { theme.colors().accent };
        let id = countdown.id;
        let group = SharedString::from(format!("countdown-{id}"));

        let number = |value: u32, unit: &str| {
            h_flex()
                .items_baseline()
                .child(div().text_size(px(34.)).text_color(color).child(value.to_string()))
                .child(div().pl_0p5().pr_1().text_size(px(13.)).text_color(color).child(unit.to_owned()))
        };
        let [y, m, d] = strings.unit_letters;
        let numbers = if self.days_only.contains(&id) {
            h_flex().child(number(span.total_days, d))
        } else {
            h_flex()
                .when(span.years > 0, |el| el.child(number(span.years, y)))
                .when(span.months > 0, |el| el.child(number(span.months, m)))
                .when(span.days > 0 || (span.years == 0 && span.months == 0), |el| el.child(number(span.days, d)))
        };
        let subtitle = format!(
            "{} {}",
            if span.past { strings.countdown_days_since } else { strings.countdown_days_until },
            format_date(countdown.date, strings)
        );

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
                        div()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(color.opacity(0.25))
                            .child(Icon::new(kind_icon(countdown.kind)).size(IconSize::XSmall).color(Color::Custom(color))),
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
            .child(
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(6.))
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .child(
                        IconButton::new("delete-countdown", IconName::X)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(move |this, _, _, cx| this.delete(id, cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_draft(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = self.draft.as_ref() else { return div().into_any_element() };
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let (selected, month, kind) = (draft.date, draft.month, draft.kind);

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
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(draft) = this.draft.as_mut() {
                                        draft.date = date;
                                    }
                                    cx.notify();
                                })),
                        )
                    }))
                    .into_any_element()
            })
            .collect();
        let kinds = [CountdownKind::Countdown, CountdownKind::Anniversary, CountdownKind::Holiday];
        let shift = |delta: i32| {
            move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let Some(draft) = this.draft.as_mut() {
                    draft.month = calendar::add_months(draft.month, delta);
                }
                cx.notify();
            }
        };

        popover(cx)
            .w(px(260.))
            .p_2()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.draft = None;
                cx.notify();
            }))
            .child(Label::new(strings.countdown_new).weight(FontWeight::SEMIBOLD))
            .child(
                h_flex()
                    .h(px(30.))
                    .px_2()
                    .rounded(px(6.))
                    .bg(theme.colors().background)
                    .child(draft.name.clone()),
            )
            .child(h_flex().gap_1().children(kinds.map(|option| {
                let selected = option == kind;
                div()
                    .id(SharedString::from(format!("countdown-kind-{}", option.as_str())))
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .bg(if selected { theme.colors().accent } else { theme.colors().element_hover })
                    .child(Label::new(kind_label(option, strings)).size(LabelSize::Small))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = this.draft.as_mut() {
                            draft.kind = option;
                        }
                        cx.notify();
                    }))
            })))
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(calendar::month_title(month, strings)).size(LabelSize::Small)))
                    .child(
                        IconButton::new("countdown-prev", IconName::ChevronLeft)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(shift(-1))),
                    )
                    .child(
                        IconButton::new("countdown-next", IconName::ChevronRight)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(shift(1))),
                    ),
            )
            .child(h_flex().children(crate::calendar::weekday_labels(&strings.weekdays_narrow).iter().map(|day| {
                div().flex_1().flex().justify_center().child(Label::new(*day).size(LabelSize::XSmall).color(Color::Muted))
            })))
            .child(v_flex().children(days))
            .child(
                div()
                    .id("countdown-save")
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .bg(theme.colors().accent)
                    .child(Label::new(strings.save).size(LabelSize::Small))
                    .on_click(cx.listener(|this, _, _, cx| this.save_draft(cx))),
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

        v_flex()
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
                            .child(
                                IconButton::new("countdown-add", IconName::Plus)
                                    .on_click(cx.listener(|this, _, window, cx| this.start_new(window, cx))),
                            )
                            .when(self.draft.is_some(), |el| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, self.render_draft(cx)))
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
    }
}
