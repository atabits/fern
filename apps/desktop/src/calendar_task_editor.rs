//! Calendar placement and lifetime of the shared task detail editor.
use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Id, Store};
use gpui::{Entity, EventEmitter, FocusHandle, Focusable};
use ui::prelude::*;

use crate::task_detail::{DraftClosed, TaskDetail};
use crate::{calendar, i18n};

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
    selected: NaiveDate,
    shift: impl Fn(&i32, &mut Window, &mut App) + 'static,
    pick: impl Fn(&NaiveDate, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let strings = i18n::t(cx);
    let colors = cx.theme().colors();
    let today = Local::now().date_naive();
    let shift = Rc::new(shift);
    let pick = Rc::new(pick);
    let arrow = |id, icon, delta| {
        let shift = shift.clone();
        IconButton::new(id, icon)
            .size(px(24.))
            .icon_size(IconSize::Small)
            .icon_color(colors.list_text_muted)
            .hover_icon_color(colors.list_text)
            .hover_color(colors.list_hover)
            .on_click(move |_, window, cx| shift(&delta, window, cx))
    };
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Label::new(calendar::month_title(month, strings)).color(colors.list_text).truncate()),
                )
                .child(arrow("calendar-picker-prev", IconName::ChevronLeft, -1))
                .child(arrow("calendar-picker-next", IconName::ChevronRight, 1)),
        )
        .child(h_flex().h(px(20.)).children(
            calendar::weekday_labels(&strings.weekdays_narrow).map(|day| {
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .child(Label::new(day).size(LabelSize::XSmall).color(colors.list_text_muted))
            }),
        ))
        .children(calendar::month_grid(month).chunks(7).map(|week| {
            h_flex().children(week.iter().map(|&date| {
                let pick = pick.clone();
                let color = if date == selected {
                    Color::OnAccent
                } else if date == today {
                    Color::Accent
                } else if date.month() != month.month() {
                    Color::Custom(colors.list_text_muted)
                } else {
                    Color::Custom(colors.list_text)
                };
                div().flex_1().flex().justify_center().child(
                    div()
                        .id(SharedString::from(format!("calendar-picker-{date}")))
                        .size(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .when(date == selected, |el| el.bg(colors.accent))
                        .when(date != selected, |el| {
                            el.hover(|style| style.bg(colors.list_hover))
                        })
                        .child(
                            Label::new(date.day().to_string())
                                .centered()
                                .size(LabelSize::Small)
                                .color(color),
                        )
                        .on_click(move |_, window, cx| pick(&date, window, cx)),
                )
            }))
        }))
}
