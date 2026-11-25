//! Calendar placement and lifetime of the shared task detail editor.
use std::collections::HashSet;
use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Id, Store};
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, FontWeight};
use ui::prelude::*;

use crate::task_detail::{DraftClosed, TaskDetail};
use crate::i18n;

pub(super) enum EditorEvent {
    Saved(Id),
    Dismissed,
}

pub(super) struct CalendarTaskEditor {
    detail: Entity<TaskDetail>,
    focus_scope: FocusHandle,
}

impl EventEmitter<EditorEvent> for CalendarTaskEditor {}

impl Focusable for CalendarTaskEditor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.detail.focus_handle(cx)
    }
}

impl CalendarTaskEditor {
    pub(super) fn new(
        store: Rc<Store>,
        date: NaiveDate,
        list: Option<Id>,
        recognize_date: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let detail = cx.new(|cx| {
            let mut detail = TaskDetail::new(store, window, cx);
            detail.set_card(true);
            detail.begin_draft(date, list, recognize_date, cx);
            detail
        });
        cx.subscribe(&detail, |_, _, DraftClosed(task), cx| {
            cx.emit(task.map_or(EditorEvent::Dismissed, EditorEvent::Saved));
        })
        .detach();
        Self {
            detail,
            focus_scope: cx.focus_handle(),
        }
    }

    pub(super) fn owns_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_scope.contains_focused(window, cx)
    }

    pub(super) fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.detail.update(cx, |detail, cx| {
            if !detail.dismiss_popup(window, cx) {
                detail.finish_draft(cx);
            }
        });
    }

    pub(super) fn close(&mut self, cx: &mut Context<Self>) -> bool {
        self.detail.update(cx, |detail, cx| detail.finish_draft(cx))
    }
}

impl Render for CalendarTaskEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        div()
            .id("calendar-task-editor")
            .debug_selector(|| "calendar-task-editor".to_owned())
            .track_focus(&self.focus_scope)
            .occlude()
            .w(px(super::CARD_WIDTH))
            .h(px(super::CARD_HEIGHT))
            .max_w((window.viewport_size().width - px(16.)).max(px(0.)))
            .max_h((window.viewport_size().height - px(16.)).max(px(0.)))
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(colors.border)
            .bg(colors.background)
            .shadow_lg()
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                // Date, priority, list and tag pickers extend beyond the card's bounds.
                if this.detail.read(cx).has_popup() {
                    return;
                }
                if !this.close(cx) {
                    window.focus(&this.focus_handle(cx), cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.detail.clone())
    }
}

pub(super) fn month_picker(
    month: NaiveDate,
    selected: Option<NaiveDate>,
    busy: &HashSet<NaiveDate>,
    shift: impl Fn(&i32, &mut Window, &mut App) + 'static,
    go_today: impl Fn(&(), &mut Window, &mut App) + 'static,
    pick: impl Fn(&NaiveDate, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let strings = i18n::t(cx);
    let colors = cx.theme().colors();
    let today = Local::now().date_naive();
    let shift = Rc::new(shift);
    let pick = Rc::new(pick);
    let short_title = format!("{} {}", strings.months_short[month.month0() as usize], month.year());
    let mut header_cfg = desktop_common::month_calendar::MonthHeaderConfig::surface(
        month,
        strings,
        "calendar-picker-prev",
        "calendar-picker-next",
    );
    header_cfg.custom_title = Some(short_title);
    header_cfg.today_id = Some("calendar-picker-today");
    header_cfg.today_tooltip = Some(strings.today);
    header_cfg.sidebar_palette = true;
    header_cfg.header_height = Some(px(24.));
    v_flex()
        .gap_1()
        .child(div().px_1().child(desktop_common::month_calendar::render_month_header(
            header_cfg,
            cx,
            move |delta, window, cx| shift(&delta, window, cx),
            Some(Box::new(move |window, cx| go_today(&(), window, cx))),
        )))
        .child(div().h(px(22.)).child(desktop_common::month_calendar::render_weekday_row(
            strings,
            desktop_common::month_calendar::WeekdayStyle::Short,
            true,
            cx,
        )))
        .children(desktop_common::month_calendar::render_month_weeks(month, cx, |date, in_month| {
            let pick = pick.clone();
            let is_today = date == today;
            let is_selected = selected == Some(date);
            let has_dot = !is_today && busy.contains(&date);
            let color = if is_today {
                Color::OnAccent
            } else if !in_month {
                Color::Custom(colors.list_text_muted)
            } else {
                Color::Custom(colors.list_text)
            };
            div()
                .flex_1()
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    v_flex()
                        .id(SharedString::from(format!("calendar-picker-{date}")))
                        .w(px(26.))
                        .h(px(28.))
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(
                            div()
                                .size(px(22.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .when(is_today, |el| el.bg(colors.accent))
                                .when(!is_today && is_selected, |el| el.bg(colors.list_selected))
                                .when(!is_today && !is_selected, |el| {
                                    el.hover(|style| style.bg(colors.list_hover))
                                })
                                .child(
                                    Label::new(date.day().to_string())
                                        .centered()
                                        .size(LabelSize::Small)
                                        .when(is_today, |l| l.weight(FontWeight::SEMIBOLD))
                                        .color(color),
                                ),
                        )
                        .child(
                            div()
                                .size(px(3.5))
                                .rounded_full()
                                .when(has_dot, |el| el.bg(colors.accent)),
                        )
                        .on_click(move |_, window, cx| pick(&date, window, cx)),
                )
                .into_any_element()
        }))
}
