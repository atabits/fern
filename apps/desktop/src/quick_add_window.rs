//! The original's system-wide Quick Add: a floating launcher over every window that adds a task
//! without bringing the app forward, with live date recognition, quick date chips, priority,
//! and list selection. Enter adds the task, Escape or a click elsewhere closes it.

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime};
use fern_core::{Id, List, Priority, Recognition};
use gpui::{
    AnyWindowHandle, Bounds, Entity, Focusable as _, Global, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, point, size,
};
use ui::prelude::*;

use crate::actions::CloseWindow;
use crate::app_view::AppView;
use crate::i18n;
use crate::lists_panel;
use crate::task_rows;
use crate::text_input::{Cancelled, Changed, Mode, MultiLinePasted, Submitted, TextInput};

const WIDTH: f32 = 560.;
const HEIGHT: f32 = 96.;

/// The open Quick Add window, which the shortcut brings back rather than opening another.
struct OpenQuickAdd(AnyWindowHandle);

impl Global for OpenQuickAdd {}

pub fn open(main: WindowHandle<AppView>, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenQuickAdd>().map(|open| open.0)
        && open.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    // Centered, a third of the way down the screen, as launchers sit.
    let mut bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
    if let Some(display) = cx.primary_display() {
        let screen = display.bounds();
        bounds.origin = point(bounds.origin.x, screen.origin.y + screen.size.height / 3.);
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        kind: WindowKind::PopUp,
        app_id: Some(std::env::var("FERN_APP_ID").unwrap_or_else(|_| "fern-tasks".into())),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| cx.new(|cx| QuickAddWindow::new(main, window, cx))) {
        Ok(handle) => {
            handle.update(cx, |_, window, _| window.activate_window()).ok();
            cx.set_global(OpenQuickAdd(handle.into()));
        }
        Err(err) => eprintln!("failed to open Quick Add: {err:#}"),
    }
}

pub struct QuickAddWindow {
    main: WindowHandle<AppView>,
    input: Entity<TextInput>,
    lists: Vec<List>,
    recognition: Recognition,
    picked_date: Option<NaiveDate>,
    parsed_due: Option<(NaiveDate, Option<NaiveTime>)>,
    parsed_repeat: bool,
    parsed_tags: Vec<String>,
    parsed_priority: Option<Priority>,
    parsed_list: Option<Id>,
    batch_lines: Vec<String>,
    priority: Priority,
    /// `None` means Inbox; `Some(id)` targets a specific list.
    list_id: Option<Id>,
}

impl QuickAddWindow {
    fn new(main: WindowHandle<AppView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        #[cfg(windows)]
        desktop_common::sticky_windows::set_topmost(window, true);
        let (lists, recognition) = main
            .read_with(cx, |view, _| view.quick_add_context())
            .unwrap_or_default();
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_task, cx).intercept_multiline_paste());
        cx.subscribe_in(&input, window, |this, _, Submitted(_), window, cx| {
            this.submit(window, cx);
        })
        .detach();
        cx.subscribe(&input, |this, _, MultiLinePasted(lines), cx| {
            this.batch_lines = lines.clone();
            cx.notify();
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Changed, cx| {
            if !this.batch_lines.is_empty() {
                this.batch_lines.clear();
            }
            this.update_parsed_due(cx);
            cx.notify();
        })
        .detach();
        cx.subscribe_in(&input, window, |_, _, _: &Cancelled, window, cx| close(window, cx)).detach();
        // A click in another window puts it away, as the original's does.
        cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() {
                close(window, cx);
            }
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        Self {
            main,
            input,
            lists,
            recognition,
            picked_date: None,
            parsed_due: None,
            parsed_repeat: false,
            parsed_tags: Vec::new(),
            parsed_priority: None,
            parsed_list: None,
            batch_lines: Vec::new(),
            priority: Priority::None,
            list_id: None,
        }
    }

    fn update_parsed_due(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text().to_owned();
        let today = Local::now().date_naive();
        let list_names: Vec<String> = self.lists.iter().map(|l| l.name.clone()).collect();
        let parsed = fern_core::parse_quick_add_with(&text, today, &list_names, self.recognition);
        self.parsed_due = parsed.due_date.map(|d| (d, parsed.due_time));
        self.parsed_repeat = parsed.repeat.is_some();
        self.parsed_tags = parsed.tags;
        self.parsed_priority = parsed.priority.filter(|p| *p != Priority::None);
        self.parsed_list = parsed
            .list
            .and_then(|name| self.lists.iter().find(|l| l.name == name))
            .map(|l| l.id);
        self.input.update(cx, |input, cx| input.set_highlights(parsed.spans, cx));
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let date = self.picked_date;
        let priority = (self.priority != Priority::None).then_some(self.priority);
        let list = self.list_id.map(Some);
        if !self.batch_lines.is_empty() {
            let lines = std::mem::take(&mut self.batch_lines);
            self.main
                .update(cx, |view, _, cx| {
                    for line in &lines {
                        view.add_quick_task_with(line, date, priority, list, cx);
                    }
                })
                .ok();
            close(window, cx);
            return;
        }
        let text = self.input.read(cx).text().trim().to_owned();
        if !text.is_empty() {
            self.main
                .update(cx, |view, _, cx| view.add_quick_task_with(&text, date, priority, list, cx))
                .ok();
        }
        close(window, cx);
    }

    fn toggle_date(&mut self, target: NaiveDate, cx: &mut Context<Self>) {
        self.picked_date = if self.picked_date == Some(target) { None } else { Some(target) };
        cx.notify();
    }

    fn cycle_priority(&mut self, cx: &mut Context<Self>) {
        self.priority = match self.priority {
            Priority::None => Priority::Low,
            Priority::Low => Priority::Medium,
            Priority::Medium => Priority::High,
            Priority::High => Priority::None,
        };
        cx.notify();
    }

    fn cycle_list(&mut self, cx: &mut Context<Self>) {
        if self.lists.is_empty() {
            self.list_id = None;
            return;
        }
        self.list_id = match self.list_id {
            None => Some(self.lists[0].id),
            Some(current) => self
                .lists
                .iter()
                .position(|l| l.id == current)
                .and_then(|i| self.lists.get(i + 1))
                .map(|l| l.id),
        };
        cx.notify();
    }
}

fn close(window: &mut Window, cx: &mut App) {
    if cx.has_global::<OpenQuickAdd>() {
        cx.remove_global::<OpenQuickAdd>();
    }
    window.remove_window();
}

impl Render for QuickAddWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let strings = i18n::t(cx);
        let today = Local::now().date_naive();
        let tomorrow = today + Duration::days(1);
        let next_week = today + Duration::days(7 - today.weekday().num_days_from_monday() as i64);
        let batch_count = self.batch_lines.len();
        let has_text = batch_count > 0 || !self.input.read(cx).text().trim().is_empty();
        let add_label: SharedString = if batch_count > 0 {
            format!("{} ({batch_count})", strings.add).into()
        } else {
            strings.add.into()
        };
        let shown_due = self.parsed_due.or_else(|| self.picked_date.map(|d| (d, None)));

        let date_chip = |id: &'static str, label: &'static str, date: NaiveDate, cx: &mut Context<Self>| {
            let active = self.picked_date == Some(date);
            h_flex()
                .id(id)
                .h(px(24.))
                .px_2()
                .rounded(px(5.))
                .cursor_pointer()
                .when(active, |el| el.bg(theme.colors().accent.opacity(0.15)))
                .hover(|s| s.bg(theme.colors().ghost_element_hover))
                .tooltip(Tooltip::text(desktop_common::calendar::format_date(date, strings)))
                .child(Label::new(label).size(LabelSize::XSmall).color(if active { Color::Accent } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_date(date, cx)))
        };

        let effective_list = self.list_id.or(self.parsed_list);
        let (list_label, list_icon, list_color) = match effective_list.and_then(|id| self.lists.iter().find(|l| l.id == id)) {
            Some(list) => (
                SharedString::from(list.name.clone()),
                IconName::List,
                lists_panel::color(list.color.as_deref(), cx),
            ),
            None => (
                SharedString::new_static(strings.inbox),
                IconName::Inbox,
                theme.colors().icon_muted,
            ),
        };

        let effective_priority = if self.priority != Priority::None {
            self.priority
        } else {
            self.parsed_priority.unwrap_or(Priority::None)
        };
        let priority_color = task_rows::priority_color(effective_priority, &theme).unwrap_or(theme.colors().icon_muted);
        let priority_tip = match effective_priority {
            Priority::High => strings.priority_high,
            Priority::Medium => strings.priority_medium,
            Priority::Low => strings.priority_low,
            Priority::None => strings.filter_priority,
        };

        v_flex()
            .size_full()
            .on_action(|_: &CloseWindow, window, cx| {
                window.defer(cx, close);
            })
            .px_4()
            .py_2()
            .justify_between()
            .bg(theme.colors().elevated_surface_background)
            .border_1()
            .border_color(theme.colors().border)
            .text_color(theme.colors().text)
            .child(
                h_flex()
                    .h(px(42.))
                    .gap_3()
                    .text_size(px(16.))
                    .child(Icon::new(IconName::Plus).color(Color::Accent))
                    .child(div().flex_1().min_w_0().overflow_hidden().child(self.input.clone()))
                    .when_some(shown_due, |el, (date, time)| {
                        let (label, _) = task_rows::due_label(date, time, today, strings);
                        el.child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .px_2()
                                .h(px(22.))
                                .rounded(px(5.))
                                .bg(theme.colors().accent.opacity(0.12))
                                .child(Icon::new(IconName::CalendarDays).size(IconSize::XSmall).color(Color::Accent))
                                .when(self.parsed_repeat, |el| {
                                    el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                                })
                                .child(Label::new(label).size(LabelSize::XSmall).color(Color::Accent)),
                        )
                    })
                    .children(self.parsed_tags.iter().take(3).map(|tag| {
                        h_flex()
                            .flex_none()
                            .px_1p5()
                            .h(px(22.))
                            .rounded(px(5.))
                            .bg(theme.colors().element_hover)
                            .child(Label::new(format!("#{tag}")).size(LabelSize::XSmall).color(Color::Muted))
                    }))
                    .when(batch_count > 0, |el| {
                        el.child(
                            h_flex()
                                .id("qa-batch-pill")
                                .h(px(22.))
                                .px_1p5()
                                .gap_1()
                                .rounded(px(4.))
                                .bg(theme.colors().accent.opacity(0.14))
                                .child(
                                    Label::new(strings.batch_selected.replace("{n}", &batch_count.to_string()))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Accent),
                                )
                                .child(
                                    IconButton::new("qa-batch-clear", IconName::X)
                                        .size(px(16.))
                                        .icon_size(IconSize::XSmall)
                                        .icon_color(Color::Accent)
                                        .tooltip(Tooltip::text(strings.clear))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.batch_lines.clear();
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .when(shown_due.is_none() && self.parsed_repeat, |el| {
                        el.child(
                            h_flex()
                                .h(px(22.))
                                .px_1p5()
                                .rounded(px(4.))
                                .bg(theme.colors().accent.opacity(0.12))
                                .child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent)),
                        )
                    }),
            )
            .child(
                h_flex()
                    .h(px(30.))
                    .gap_1p5()
                    .items_center()
                    .child(date_chip("qa-today", strings.today, today, cx))
                    .child(date_chip("qa-tomorrow", strings.tomorrow, tomorrow, cx))
                    .child(date_chip("qa-next-week", strings.next_week, next_week, cx))
                    .child(
                        IconButton::new("qa-priority", IconName::Flag)
                            .size(px(24.))
                            .icon_size(IconSize::Small)
                            .icon_color(priority_color)
                            .tooltip(Tooltip::text(priority_tip))
                            .on_click(cx.listener(|this, _, _, cx| this.cycle_priority(cx))),
                    )
                    .child(
                        h_flex()
                            .id("qa-list")
                            .h(px(24.))
                            .px_2()
                            .gap_1()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.colors().ghost_element_hover))
                            .tooltip(Tooltip::text(list_label.clone()))
                            .child(Icon::new(list_icon).size(IconSize::XSmall).color(list_color))
                            .child(Label::new(list_label).size(LabelSize::XSmall).color(Color::Muted).truncate())
                            .on_click(cx.listener(|this, _, _, cx| this.cycle_list(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .id("qa-submit")
                            .h(px(26.))
                            .px_3()
                            .rounded(px(5.))
                            .when(has_text, |el| el.cursor_pointer())
                            .bg(if has_text { theme.colors().accent } else { theme.colors().element_hover })
                            .hover(|s| {
                                s.bg(if has_text {
                                    theme.colors().accent.opacity(0.9)
                                } else {
                                    theme.colors().element_hover
                                })
                            })
                            .child(
                                Label::new(add_label)
                                    .size(LabelSize::Small)
                                    .color(if has_text { Color::OnAccent } else { Color::Muted }),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[gpui::test]
    fn quick_add_window_applies_picked_date_priority_and_list(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let store = Rc::new(fern_core::Store::open_in_memory().unwrap());
        let work = store.add_list(&fern_core::ListDraft { name: "Work".into(), ..Default::default() }).unwrap();
        let (main, main_cx) = cx.add_window_view(|window, cx| AppView::new(store.clone(), false, window, cx));
        let main_handle = main_cx.window_handle().downcast::<AppView>().unwrap();

        let (qa, qa_cx) = cx.add_window_view(|window, cx| QuickAddWindow::new(main_handle, window, cx));
        let tomorrow = Local::now().date_naive() + Duration::days(1);
        qa_cx.update(|window, cx| {
            qa.update(cx, |qa, cx| {
                qa.input.update(cx, |input, cx| input.set_text("Ship release", cx));
                qa.toggle_date(tomorrow, cx);
                qa.cycle_priority(cx); // Low
                qa.cycle_priority(cx); // Medium
                qa.cycle_priority(cx); // High
                qa.cycle_list(cx); // Work
                qa.submit(window, cx);
            });
        });
        cx.run_until_parked();
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Ship release");
        assert_eq!(tasks[0].due_date, Some(tomorrow));
        assert_eq!(tasks[0].priority, Priority::High);
        assert_eq!(tasks[0].list_id, Some(work.id));
        let _ = main;
    }
}
