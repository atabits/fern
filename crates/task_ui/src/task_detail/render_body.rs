use std::rc::Rc;

use chrono::{Local, NaiveDate, TimeZone as _};
use fern_core::{Id, Kind, Task};
use gpui::{ExternalPaths, FocusHandle, Focusable, FontWeight, MouseButton};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, TipSide, with_tip};

use crate::actions::{self as md, DismissTransient};
use crate::calendar;
use crate::due_picker;
use crate::i18n::{self, Strings};
use crate::markdown_toolbar::ToolbarAction;
use crate::task_rows;

use super::{
    FocusTarget, OpenTask, Popup, StickyPaletteTokens, TaskDetail, activity_text, sticky_checkbox,
};

/// A text button with an icon, used for the due date and the list.
fn label_button(
    id: &'static str,
    icon: IconName,
    label: SharedString,
    color: Color,
    hover: Hsla,
) -> Stateful<ui::HorizontalStack> {
    h_flex()
        .id(id)
        .relative()
        .gap_1p5()
        .px_1()
        .py_0p5()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(Icon::new(icon).size(IconSize::Large).color(color))
        .child(Label::new(label).color(color))
}

struct DueBadge {
    text: SharedString,
    color: Color,
    icon: IconName,
    has_date: bool,
    repeats: bool,
}

struct RenderMeta<'a> {
    task: &'a Task,
    kind: Kind,
    done: bool,
    in_progress: bool,
    due: DueBadge,
    priority_color: Color,
    list_icon: IconName,
    list_name: SharedString,
    read_only: bool,
    sticky: bool,
    sticky_tokens: Option<StickyPaletteTokens>,
    muted: Color,
    button_hover: Hsla,
    strings: &'static Strings,
    theme: &'a theme::Theme,
}

impl TaskDetail {
    /// The "Task Activities" panel at the bottom of the details, as in the original.
    pub(super) fn render_activity(
        &self,
        activity: &[fern_core::Activity],
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();

        v_scroll(
            "task-activity-scroll",
            v_flex()
                .id("task-activity")
                .px_5()
                .py_3()
                .gap_2()
                .child(
                    h_flex()
                        .child(
                            div().flex_1().child(
                                Label::new(strings.task_activities)
                                    .weight(FontWeight::SEMIBOLD)
                                    .when_some(sticky, |l, t| l.color(t.ink)),
                            ),
                        )
                        .child(
                            IconButton::new("close-activity", IconName::X)
                                .when_some(sticky, |b, t| b.icon_color(t.muted).hover_color(t.surface))
                                .tooltip(Tooltip::text(strings.close))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.session.activity = None;
                                    cx.notify();
                                })),
                        ),
                )
                .children(activity.iter().rev().map(|entry| {
                    let text = activity_text(&entry.kind, strings);
                    let when = chrono::Local
                        .timestamp_millis_opt(entry.at)
                        .single()
                        .map(|t| {
                            format!(
                                "{} {}",
                                calendar::format_day_month(t.date_naive(), strings),
                                calendar::format_time(t, strings)
                            )
                        })
                        .unwrap_or_default();
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(
                            div()
                                .mt(px(5.))
                                .size(px(8.))
                                .flex_none()
                                .rounded_full()
                                .bg(sticky.map_or(theme.colors().accent, |t| t.accent)),
                        )
                        .child(
                            v_flex()
                                .child(
                                    Label::new(text)
                                        .size(LabelSize::Small)
                                        .when_some(sticky, |l, t| l.color(t.ink)),
                                )
                                .child(
                                    Label::new(when)
                                        .size(LabelSize::XSmall)
                                        .color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted))),
                                ),
                        )
                })),
        )
        .max_h(px(240.))
        .border_t_1()
        .border_color(sticky.map_or(theme.colors().border, |t| t.border))
        .into_any_element()
    }

    fn render_tags_row(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();
        let read_only = self.is_read_only();
        h_flex()
            .flex_wrap()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // The Estimation (Start Focus → Estimation), which opens it again.
            .children(self.session.task.as_ref().and_then(|t| t.estimate).map(|estimate| {
                let text = match estimate {
                    fern_core::Estimate::Pomos(n) => format!("{n} {}", strings.pomo_unit),
                    fern_core::Estimate::Minutes(n) => strings.format_minutes(n as i64),
                };
                let chip_color = sticky.map_or(Color::Accent, |t| Color::Custom(t.accent));
                let chip_bg = sticky.map_or_else(|| theme.colors().accent.opacity(0.2), |t| t.surface);
                let chip_hover = sticky.map_or_else(|| theme.colors().accent.opacity(0.3), |t| t.accent.opacity(0.2));
                h_flex()
                    .id("estimate-chip")
                    .h(px(18.))
                    .px(px(6.))
                    .gap_1()
                    .rounded_full()
                    .bg(chip_bg)
                    .when(!read_only, |el| {
                        el.cursor_pointer()
                            .hover(move |s| s.bg(chip_hover))
                            .tooltip(Tooltip::text(strings.estimation))
                            .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Estimate, cx)))
                    })
                    .child(Icon::new(IconName::CircleDot).size(IconSize::XSmall).color(chip_color))
                    .child(Label::new(text).size(LabelSize::XSmall).color(chip_color))
            }))
            // As the original's, a tag is a plain chip; a click opens the tag picker, where it is
            // unticked.
            .children(self.task_tags().into_iter().map(|tag| {
                div()
                    .id(SharedString::from(format!("tag-chip-{tag}")))
                    .when(!read_only, |el| {
                        el.cursor_pointer().on_click(cx.listener(|this, _, window, cx| {
                            this.open_tags(Popup::InlineTags, window, cx);
                        }))
                    })
                    .child(match sticky {
                        Some(t) => div()
                            .h(px(18.))
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(t.surface)
                            .text_color(t.ink)
                            .text_size(px(11.))
                            .child(tag)
                            .into_any_element(),
                        None => {
                            let color = self.tag_color(&tag, cx);
                            Chip::new(tag, color).into_any_element()
                        }
                    })
            }))
            .when(!read_only, |el| {
                let add_border = sticky.map_or(theme.colors().accent, |t| t.accent);
                let add_hover = sticky.map_or(theme.colors().element_hover, |t| t.surface);
                let add_color = sticky.map_or(Color::Accent, |t| Color::Custom(t.accent));
                el.child(
                    // The original's "+" after the tags: an accent-outlined circle.
                    div()
                        .id("add-tag")
                        .relative()
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .border_1()
                        .border_color(add_border)
                        .cursor_pointer()
                        .hover(move |s| s.bg(add_hover))
                        .child(Icon::new(IconName::Plus).size(IconSize::Custom(px(11.))).color(add_color))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_tags(Popup::InlineTags, window, cx);
                        }))
                        .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::InlineTags), |el| {
                            el.child(PopoverAttachment::new(Placement::BelowLeft, self.render_tags_popup(cx)))
                        }),
                )
            })
    }

    fn due_badge(
        &self,
        task: &Task,
        kind: Kind,
        done: bool,
        today: NaiveDate,
        muted: Color,
        strings: &'static Strings,
    ) -> DueBadge {
        let preview = self.session.draft.as_ref().map(|_| self.due_preview(task, today));
        let (date, time, repeats, tz_name, is_floating, horizon) = preview.as_ref().map_or(
            (
                task.due_date,
                task.due_time,
                task.repeat.is_some(),
                task.timezone.as_deref(),
                task.is_floating,
                task.horizon,
            ),
            |due| {
                (
                    due.date,
                    due.time,
                    due.repeat.is_some(),
                    due.timezone.as_deref(),
                    due.is_floating,
                    due.horizon,
                )
            },
        );
        let tz_suffix = if time.is_some() {
            due_picker::short_timezone_label(tz_name, is_floating, strings)
        } else {
            None
        };
        let (text, color): (SharedString, Color) = match (date, horizon) {
            (Some(date), _) => {
                let (label, overdue) = task_rows::due_label(date, time, today, strings);
                let label = match task.end_date {
                    Some(end) => due_picker::range_label(date, time, end, task.end_time, strings),
                    None => label,
                };
                // The calendar's card writes the date out, as the original's.
                let mut label = if self.calendar_card && task.end_date.is_none() {
                    task_rows::card_due_label(date, time, today, strings)
                } else {
                    task_rows::detail_due_label(label, date, today, strings)
                };
                if let Some(tz) = tz_suffix {
                    label = format!("{label} · {tz}");
                }
                (label.into(), if overdue && !done { Color::Error } else { Color::Accent })
            }
            (None, Some(h)) => {
                let label = calendar::format_horizon_badge(h, strings);
                let is_past = h.is_past_relative_to(today);
                let is_current =
                    h != fern_core::Horizon::Life && h == fern_core::Horizon::current_for(h.granularity(), today);
                let color = if is_past && !done {
                    Color::Warning
                } else if is_current && !done {
                    Color::Accent
                } else {
                    muted
                };
                (label.into(), color)
            }
            // A note, as the original's, offers a reminder rather than a due date.
            (None, None) => (
                (if kind == Kind::Note {
                    strings.set_reminder
                } else {
                    strings.due_date
                })
                .into(),
                muted,
            ),
        };
        let icon = if date.is_none() && horizon.is_some() {
            IconName::FernHorizons
        } else {
            IconName::CalendarDays
        };
        DueBadge {
            text,
            color,
            icon,
            has_date: date.is_some(),
            repeats,
        }
    }

    fn render_empty_panel(
        panel: gpui::Div,
        theme: &theme::Theme,
        sticky_tokens: Option<StickyPaletteTokens>,
        muted: Color,
        strings: &'static Strings,
    ) -> gpui::Div {
        panel.items_center().justify_center().child(
            v_flex()
                .w_full()
                .min_w_0()
                .px_6()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .size(px(52.))
                        .rounded_full()
                        .bg(sticky_tokens.map_or(theme.colors().surface_background, |t| t.surface))
                        .border_1()
                        .border_color(sticky_tokens.map_or(theme.colors().border_variant, |t| t.border))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::SquareCheck).size(IconSize::Large).color(muted)),
                )
                .child(
                    div()
                        .w_full()
                        .text_center()
                        .text_size(px(13.))
                        .text_color(sticky_tokens.map_or(theme.colors().text_muted, |t| t.muted))
                        .child(strings.select_task),
                ),
        )
    }

    fn render_header(
        &self,
        meta: &RenderMeta<'_>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let task = meta.task;
        let due = &meta.due;
        let RenderMeta {
            kind,
            done,
            in_progress,
            priority_color,
            read_only,
            button_hover,
            strings,
            theme,
            ..
        } = *meta;
        h_flex()
            .debug_selector(|| "task-detail-header".into())
            .when(self.top_inset, |el| el.mt(desktop_common::layout::header_inset()))
            .gap_2()
            .h(px(48.))
            .flex_none()
            .px_5()
            .border_b_1()
            .border_color(theme.colors().border)
            // The original's 15 px box and the short line after it; a note has neither.
            .when(task.kind != Kind::Note && self.session.draft.is_none(), |el| {
                el.child(
                    Checkbox::new("detail-checkbox")
                        .size(px(15.))
                        .toggle_state(done)
                        .animate_completion(crate::completion_mark::started(task.id, cx))
                        .dismissed(task.is_wont_do())
                        .in_progress(in_progress)
                        .lines(!self.checklist.is_empty())
                        .tint(task_rows::priority_color(task.priority, theme))
                        .when(!read_only, |cb| {
                            cb.on_click(cx.listener(|this, _, _, cx| this.toggle_completed(cx)))
                        }),
                )
                .child(div().w(px(1.)).h(px(14.)).bg(theme.colors().border))
            })
            .child(if read_only {
                h_flex()
                    .id("due-date")
                    .debug_selector(|| "task-detail-due-date".into())
                    .gap_1p5()
                    .px_1p5()
                    .py_1()
                    .rounded(px(6.))
                    .opacity(0.75)
                    .child(Icon::new(due.icon).color(due.color))
                    .child(Label::new(due.text.clone()).color(due.color))
                    .when(due.repeats, |el| {
                        el.child(Icon::new(IconName::Repeat).size(IconSize::Small).color(due.color))
                    })
                    .into_any_element()
            } else {
                label_button("due-date", due.icon, due.text.clone(), due.color, button_hover)
                    .debug_selector(|| "task-detail-due-date".into())
                    .when(due.repeats, |el| {
                        el.child(Icon::new(IconName::Repeat).size(IconSize::Small).color(due.color))
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.open_popup_from_ui(Popup::Due, window, cx)))
                    .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Due), |el| {
                        el.child(PopoverAttachment::new(
                            Placement::BelowLeft,
                            self.render_due_popup(strings, window, cx),
                        ))
                    })
                    .into_any_element()
            })
            .when(in_progress, |el| {
                el.child(
                    h_flex()
                        .id("detail-in-progress-badge")
                        .px_1p5()
                        .py_0p5()
                        .gap_1()
                        .rounded(px(5.))
                        .bg(theme.colors().accent.opacity(0.14))
                        .child(Icon::new(IconName::Hourglass).size(IconSize::XSmall).color(Color::Accent))
                        .child(Label::new(strings.in_progress).size(LabelSize::XSmall).color(Color::Accent))
                        .when(!read_only, |el| {
                            el.cursor_pointer()
                                .tooltip(Tooltip::text(strings.clear_in_progress))
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_in_progress(cx)))
                        }),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .debug_selector(|| "task-detail-priority".to_owned())
                    .relative()
                    .child(if read_only {
                        h_flex()
                            .size(px(28.))
                            .justify_center()
                            .opacity(0.75)
                            .child(
                                Icon::new(if kind == Kind::Note {
                                    IconName::CircleAlert
                                } else {
                                    IconName::Flag
                                })
                                .color(priority_color),
                            )
                            .into_any_element()
                    } else {
                        // A note's priority shows as the original's "!" in a circle.
                        IconButton::new(
                            "priority",
                            if kind == Kind::Note {
                                IconName::CircleAlert
                            } else {
                                IconName::Flag
                            },
                        )
                        .icon_color(priority_color)
                        .tip(strings.summary_priority, TipSide::Below)
                        .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Priority, cx)))
                        .into_any_element()
                    })
                    .when(!read_only && self.sticky_style.is_none() && self.session.popup == Some(Popup::Priority), |el| {
                        el.child(PopoverAttachment::new(
                            Placement::BelowRight,
                            self.render_priority_popup(strings, cx),
                        ))
                    }),
            )
    }

    fn render_parent_row(
        &self,
        parent: &Task,
        read_only: bool,
        sticky: bool,
        muted: Color,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let parent_id = parent.id;
        let parent_title: SharedString = if parent.title.is_empty() {
            strings.untitled.into()
        } else {
            parent.title.clone().into()
        };
        h_flex()
            .id("detail-parent")
            .group("detail-parent-row")
            .mb(px(-6.))
            .gap_1()
            .min_w_0()
            .items_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .id("detail-parent-open")
                    .min_w_0()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenTask(parent_id))))
                    .child(Label::new(parent_title).color(muted).truncate())
                    .child(Icon::new(IconName::ChevronRight).size(IconSize::Small).color(muted)),
            )
            .when(!read_only && !sticky, |el| {
                el.child(
                    h_flex()
                        .flex_none()
                        .gap_0p5()
                        .opacity(if crate::touch::active() { 1. } else { 0. })
                        .group_hover("detail-parent-row", |s| s.opacity(1.))
                        .child(
                            IconButton::new("detail-edit-parent", IconName::Pencil)
                                .size(px(22.))
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text(strings.link_parent))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.open_popup_from_ui(Popup::LinkParent, window, cx);
                                    window.focus(&this.link_search.focus_handle(cx), cx);
                                })),
                        )
                        .child(
                            div()
                                .debug_selector(|| "detail-unlink-parent".to_owned())
                                .child(
                                    IconButton::new("detail-unlink-parent-btn", IconName::X)
                                        .size(px(22.))
                                        .icon_size(IconSize::XSmall)
                                        .icon_color(Color::Muted)
                                        .tooltip(Tooltip::text(strings.unlink_parent))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.unlink_current_parent(cx);
                                        })),
                                ),
                        ),
                )
            })
    }

    fn render_notes_section(
        &self,
        task: &Task,
        read_only: bool,
        sticky: bool,
        notes_editing: bool,
        notes_focus_click: FocusHandle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if read_only {
            if !task.notes.trim().is_empty() {
                div()
                    .id("notes-formatted")
                    .debug_selector(|| "task-notes-preview".to_owned())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(render_markdown_interactive(
                        &task.notes,
                        self.sticky_style,
                        Rc::new(|_, _, _| {}),
                        cx,
                    ))
                    .into_any_element()
            } else {
                div().into_any_element()
            }
        } else if !notes_editing && !task.notes.trim().is_empty() {
            div()
                .id("notes-formatted")
                .debug_selector(|| "task-notes-preview".to_owned())
                .cursor_text()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                // On the release, so that a click on a link opens it instead.
                .on_click(move |_, window, cx| window.focus(&notes_focus_click, cx))
                .child(render_markdown_interactive(
                    &task.notes,
                    self.sticky_style,
                    {
                        let view = cx.entity().downgrade();
                        Rc::new(move |line_index, _, cx| {
                            view.update(cx, |this, cx| this.toggle_markdown_check(line_index, cx))
                                .ok();
                        })
                    },
                    cx,
                ))
                .into_any_element()
        } else {
            div()
                .debug_selector(|| "task-notes-editor".to_owned())
                .key_context("SummaryEditor")
                .on_action(cx.listener(|this, _: &md::Bold, window, cx| {
                    this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Bold)), window, cx)
                }))
                .on_action(cx.listener(|this, _: &md::Highlight, window, cx| {
                    this.toolbar_action(
                        ToolbarAction::Format(Some(fern_core::markdown::Format::Highlight)),
                        window,
                        cx,
                    )
                }))
                .on_action(cx.listener(|this, _: &md::CheckItem, window, cx| {
                    this.toolbar_action(
                        ToolbarAction::Format(Some(fern_core::markdown::Format::Checklist)),
                        window,
                        cx,
                    )
                }))
                .on_action(cx.listener(|this, _: &md::BulletedList, window, cx| {
                    this.toolbar_action(
                        ToolbarAction::Format(Some(fern_core::markdown::Format::Bullets)),
                        window,
                        cx,
                    )
                }))
                .on_action(cx.listener(|this, _: &md::NumberedList, window, cx| {
                    this.toolbar_action(
                        ToolbarAction::Format(Some(fern_core::markdown::Format::Numbered)),
                        window,
                        cx,
                    )
                }))
                .on_action(cx.listener(|this, _: &md::Italic, window, cx| {
                    this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Italic)), window, cx)
                }))
                .on_action(cx.listener(|this, _: &md::Underline, window, cx| {
                    this.toolbar_action(
                        ToolbarAction::Format(Some(fern_core::markdown::Format::Underline)),
                        window,
                        cx,
                    )
                }))
                .on_action(cx.listener(|this, _: &md::Strike, window, cx| {
                    this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Strike)), window, cx)
                }))
                .on_action(cx.listener(|this, _: &md::Link, window, cx| {
                    this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Link)), window, cx)
                }))
                .on_action(cx.listener(|this, _: &md::Code, window, cx| {
                    this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Code)), window, cx)
                }))
                .on_action(cx.listener(|this, _: &md::Quote, window, cx| {
                    this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Quote)), window, cx)
                }))
                .when(sticky && !notes_editing, |el| el.opacity(0.))
                .text_size(px(15.))
                .line_height(px(22.))
                .when_some(self.sticky_style, |el, style| {
                    el.text_size(px(style.font_px())).line_height(px(style.font_px() + 8.))
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(self.notes.clone())
                .into_any_element()
        }
    }

    fn render_location_row(
        &self,
        read_only: bool,
        strings: &'static Strings,
        theme: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("detail-location-row")
            .h(px(30.))
            .px_2()
            .gap_2()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().border_variant)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(Icon::new(IconName::MapPin).size(IconSize::Small).color(Color::Muted))
            .child(div().flex_1().min_w_0().text_size(px(13.)).child(self.location_input.clone()))
            .when(!read_only, |el| {
                el.child(
                    IconButton::new("detail-location-clear", IconName::X)
                        .size(px(20.))
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text(strings.clear))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.location_open = false;
                            this.location_input.update(cx, |input, cx| input.clear(window, cx));
                            if !this.edit_draft(cx, |task, _| task.location = None) {
                                this.save(cx, |store, id| store.set_location(id, None));
                            }
                            cx.notify();
                        })),
                )
            })
    }

    fn render_footer_bar(
        &self,
        meta: &RenderMeta<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let task = meta.task;
        let due = &meta.due;
        let list_name = meta.list_name.clone();
        let RenderMeta {
            list_icon,
            read_only,
            sticky,
            sticky_tokens,
            muted,
            button_hover,
            strings,
            theme,
            ..
        } = *meta;
        h_flex()
            .gap_2()
            // As the original's: no line over the bottom bar.
            .h(px(44.))
            .flex_none()
            .px_5()
            .when(sticky, |bar| bar.h(px(29.)).px_3())
            .map(|bar| {
                if task.is_deleted() {
                    bar.child(
                        label_button(
                            "restore-task",
                            IconName::ArchiveRestore,
                            strings.restore.into(),
                            sticky_tokens.map_or(Color::Default, |t| Color::Custom(t.ink)),
                            button_hover,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.restore(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        IconButton::new("delete-forever", IconName::Trash)
                            .when_some(sticky_tokens, |b, t| b.icon_color(t.muted).hover_color(t.surface))
                            .tooltip(Tooltip::text(strings.delete_forever))
                            .on_click(cx.listener(|this, _, _, cx| this.delete_forever(cx))),
                    )
                } else if read_only {
                    bar.when(sticky, |bar| {
                        bar.child(
                            div()
                                .id("sticky-date")
                                .debug_selector(|| "task-detail-due-date".into())
                                .flex()
                                .items_center()
                                .min_w_0()
                                .text_size(px(11.))
                                .text_color(sticky_tokens.map_or_else(|| theme.colors().text.opacity(0.68), |t| t.muted))
                                .when(!due.has_date, |date| date.opacity(0.))
                                .child(div().truncate().child(due.text.clone())),
                        )
                    })
                    .when(!sticky, |bar| {
                        bar.child(
                            h_flex()
                                .id("list")
                                .debug_selector(|| "task-detail-list".to_owned())
                                .gap_1p5()
                                .px_1p5()
                                .py_1()
                                .opacity(0.75)
                                .child(Icon::new(list_icon).color(muted))
                                .child(Label::new(list_name).color(muted)),
                        )
                    })
                    .child(div().flex_1())
                    .when(!sticky && self.word_count > 0, |bar| {
                        bar.child(
                            Label::new(strings.word_count.replace("{n}", &self.word_count.to_string()))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                } else {
                    bar.when(sticky, |bar| {
                        bar.child(
                            div()
                                .id("sticky-date")
                                .debug_selector(|| "task-detail-due-date".into())
                                .flex()
                                .items_center()
                                .relative()
                                .min_w_0()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(sticky_tokens.map_or_else(|| theme.colors().text.opacity(0.68), |t| t.muted))
                                .when(!due.has_date, |date| {
                                    date.opacity(0.).group_hover("sticky-window", |style| style.opacity(1.))
                                })
                                .child(div().truncate().child(due.text.clone()))
                                .on_click(cx.listener(|this, _, window, cx| this.open_popup_from_ui(Popup::Due, window, cx)))
                                .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Due), |el| {
                                    el.child(PopoverAttachment::new(
                                        Placement::AboveLeft,
                                        self.render_due_popup(strings, window, cx),
                                    ))
                                }),
                        )
                    })
                    .when(!sticky, |bar| {
                        bar.child(
                            with_tip(
                                label_button("list", list_icon, list_name, muted, button_hover)
                                    .debug_selector(|| "task-detail-list".to_owned()),
                                "list-tip",
                                strings.move_to,
                                TipSide::Above,
                                window,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.open_popup_from_ui(Popup::List, window, cx)))
                            .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::List), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::AboveLeft,
                                    self.render_list_popup(strings, cx),
                                ))
                            }),
                        )
                    })
                    .child(div().flex_1())
                    .when(self.copied_feedback, |bar| {
                        let chip_bg = sticky_tokens.map_or_else(|| theme.colors().accent.opacity(0.14), |t| t.surface);
                        let chip_color = sticky_tokens.map_or(Color::Accent, |t| Color::Custom(t.accent));
                        bar.child(
                            h_flex()
                                .gap_1()
                                .px_2()
                                .py_0p5()
                                .rounded(px(4.))
                                .bg(chip_bg)
                                .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(chip_color))
                                .child(Label::new(strings.summary_copied).size(LabelSize::XSmall).color(chip_color)),
                        )
                    })
                    .when(!sticky && self.word_count > 0, |bar| {
                        bar.child(
                            Label::new(strings.word_count.replace("{n}", &self.word_count.to_string()))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .when(!sticky && self.session.draft.is_none(), |bar| {
                        bar.child(
                            IconButton::new("format", IconName::Baseline)
                                .toggle_state(self.format_bar)
                                .tip(strings.text_format, TipSide::Above)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.format_bar = !this.format_bar;
                                    this.format_headings = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            IconButton::new("comments", IconName::MessageSquareText)
                                .toggle_state(self.comment_open)
                                .tip(strings.comment_tip, TipSide::Above)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.comment_open = !this.comment_open;
                                    if this.comment_open {
                                        window.focus(&this.comment_input.focus_handle(cx), cx);
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .debug_selector(|| "task-detail-more".to_owned())
                            .relative()
                            .child(
                                IconButton::new("more", IconName::Ellipsis)
                                    .when(sticky, |button| {
                                        button
                                            .size(px(20.))
                                            .icon_size(IconSize::XSmall)
                                            .icon_color(muted)
                                            .hover_color(button_hover)
                                    })
                                    .when(!sticky && !matches!(self.session.popup, Some(Popup::More | Popup::Tags)), |b| {
                                        b.tip(strings.more, TipSide::Above)
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::More, cx))),
                            )
                            .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::More), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::AboveRight,
                                    self.render_more_popup(strings, cx),
                                ))
                            })
                            .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Tags), |el| {
                                el.child(PopoverAttachment::new(Placement::AboveRight, self.render_tags_popup(cx)))
                            })
                            .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Templates), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::AboveRight,
                                    self.render_draft_templates(cx),
                                ))
                            })
                            .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Estimate), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::AboveRight,
                                    self.render_estimate(strings, cx),
                                ))
                            })
                            .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::LinkParent), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::AboveRight,
                                    self.render_link_parent(strings, cx),
                                ))
                            }),
                    )
                }
            })
    }
}

impl Render for TaskDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let sticky_tokens = self.sticky_tokens();
        let button_hover = sticky_tokens.map_or(theme.colors().element_hover, |t| t.surface);
        let muted = sticky_tokens.map_or(Color::Muted, |t| Color::Custom(t.muted));
        let sticky = sticky_tokens.is_some();
        if let Some(target) = self.editor_window.take_pending_focus(window) {
            let input = match target {
                FocusTarget::NewItem => Some(self.new_item.clone()),
                FocusTarget::Item(id) => self
                    .checklist
                    .iter()
                    .find(|row| row.item.id == id)
                    .map(|row| row.input.clone()),
            };
            if let Some(input) = input {
                window.focus(&input.focus_handle(cx), cx);
            }
        }
        let panel = v_flex()
            .size_full()
            .key_context(if self.session.draft.is_some() {
                "TaskDetail TaskDraft"
            } else {
                "TaskDetail"
            })
            .on_action(cx.listener(|this, _: &crate::actions::SaveDraft, window, cx| {
                if this.session.draft.is_some() && this.session.popup == Some(Popup::Due) {
                    this.apply_due(window, cx);
                }
                this.finish_draft(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::PrintDetail, _, cx| {
                this.print_current(cx);
            }))
            .on_action(cx.listener(|this, _: &DismissTransient, window, cx| {
                if !this.dismiss_popup(window, cx) {
                    if this.session.draft.is_some() {
                        this.finish_draft(cx);
                    } else {
                        cx.propagate();
                    }
                }
            }))
            .map(|el| match self.sticky_style {
                Some(style) => el.text_color(gpui::rgb(style.foreground())),
                None => el
                    .bg(theme.colors().background)
                    .text_color(theme.colors().text)
                    .when(!self.card, |el| el.border_l_1().border_color(theme.colors().border)),
            });

        let Some(task) = self.session.task.as_ref() else {
            return Self::render_empty_panel(panel, &theme, sticky_tokens, muted, strings);
        };
        let done = task.is_completed();
        let notes_focus = self.notes.focus_handle(cx);
        let notes_editing = notes_focus.is_focused(window);
        let notes_focus_click = notes_focus.clone();
        let today = Local::now().date_naive();
        let kind = self.kind_preview(task);
        let due = self.due_badge(task, kind, done, today, muted, strings);
        let in_progress = task.is_in_progress();
        let read_only = self.is_read_only();
        let show_location = self.location_open || task.location.is_some();
        let priority_color = task_rows::priority_color(task.priority, &theme).map_or(Color::Muted, Color::Custom);
        let list_id: Option<Id> = self.list_preview(task);
        let list_name: SharedString = list_id
            .and_then(|id| self.session.lists.iter().find(|l| l.id == id))
            .map(|l| l.name.clone().into())
            .or_else(|| self.session.archived_list_name.clone().map(Into::into))
            .unwrap_or_else(|| strings.inbox.into());
        let list_icon = if list_id.is_some() { IconName::List } else { IconName::Inbox };
        let task_id = task.id;
        let drop_color = sticky_tokens.map_or_else(|| theme.colors().accent.opacity(0.08), |t| t.surface);
        let meta = RenderMeta {
            task,
            kind,
            done,
            in_progress,
            due,
            priority_color,
            list_icon,
            list_name,
            read_only,
            sticky,
            sticky_tokens,
            muted,
            button_hover,
            strings,
            theme: &theme,
        };

        panel
            .when(!read_only, |el| {
                el.drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop_color))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        this.attach_paths(task_id, paths.paths().to_vec(), cx);
                    }))
            })
            // Top bar: checkbox | due date ……… priority
            .when(!sticky, |panel| panel.child(self.render_header(&meta, window, cx)))
            // Title, checklist, notes and tags; clicking empty space puts the caret into the notes.
            .child(
                v_scroll(
                    "detail-body-scroll",
                    v_flex()
                        .id("detail-body")
                        .debug_selector(|| "task-detail-body".to_owned())
                        .when_some(self.sticky_style, |el, style| el.text_size(px(style.font_px())))
                        .px_5()
                        .pt_4()
                        .pb_6()
                        .gap_3()
                        .when(sticky, |el| el.px_3().pt_0p5().pb_2().gap_1())
                        .when(!read_only, |el| {
                            el.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                window.focus(&notes_focus, cx);
                            })
                        })
                        // A subtask names its parent over the title, as the original; a click opens it,
                        // and hovering exposes Edit and Unlink Parent Task buttons.
                        .when_some(self.session.parent.as_ref(), |el, parent| {
                            el.child(self.render_parent_row(parent, read_only, sticky, muted, strings, cx))
                        })
                        .child(
                            h_flex()
                                .items_start()
                                .gap_3()
                                .child(
                                    div()
                                        .debug_selector(|| "task-detail-title".to_owned())
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(20.))
                                        .when_some(self.sticky_style, |el, style| el.text_size(px(style.font_px())))
                                        .line_height(px(28.))
                                        .when_some(self.sticky_style, |el, style| {
                                            el.line_height(px(style.font_px() + 5.))
                                        })
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .when(done, |el| {
                                            el.text_color(sticky_tokens.map_or(theme.colors().text_muted, |t| t.muted))
                                        })
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                        .child(self.title.clone()),
                                )
                                .when(!sticky && !read_only, |title| {
                                    title.child(
                                        div()
                                            .debug_selector(|| "task-detail-checklist-toggle".to_owned())
                                            .pt_0p5()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                            .child(
                                                IconButton::new("checklist", IconName::ListChecks)
                                                    .tooltip(Tooltip::text(i18n::t(cx).checklist))
                                                    .on_click(cx.listener(|this, _, _, cx| this.start_checklist(cx))),
                                            ),
                                    )
                                }),
                        )
                        // As in the original, the description comes first and the checklist under it.
                        // While it is not edited, the description is drawn formatted, as the original
                        // shows it; a click opens it for editing.
                        .child(self.render_notes_section(
                            task,
                            read_only,
                            sticky,
                            notes_editing,
                            notes_focus_click,
                            cx,
                        ))
                        .when(self.show_checklist, |el| el.child(self.render_checklist(cx)))
                        .when(!self.session.attachments.is_empty(), |el| {
                            el.child(self.render_attachments(cx))
                        })
                        .children(self.session.attachment_errors.iter().map(|error| {
                            Label::new(error.clone()).color(sticky_tokens.map_or(Color::Error, |t| Color::Custom(t.error)))
                        }))
                        .when(show_location && !sticky, |el| {
                            el.child(self.render_location_row(read_only, strings, &theme, cx))
                        })
                        // Tags sit under the notes, as in the original.
                        // The calendar's card leaves out the empty tags and subtasks, as the original's.
                        // As the original's, the tags row only once there are tags (the "…" menu's Tags
                        // adds the first), and the subtasks only once there are some or one is begun.
                        .when(
                            !task.tags.is_empty()
                                || task.estimate.is_some()
                                || self.session.popup == Some(Popup::InlineTags),
                            |el| el.child(div().pt(px(18.)).child(self.render_tags_row(strings, cx))),
                        )
                        .when(!self.session.subtasks.is_empty() || self.subtask_open, |el| {
                            let divider = match sticky_tokens {
                                Some(t) => div().flex_none().h(px(1.)).w_full().bg(t.border).into_any_element(),
                                None => Divider::horizontal().into_any_element(),
                            };
                            el.child(div().pt_2().child(divider))
                                .child(self.render_subtasks(strings, cx))
                        })
                        .when(!self.session.comments.is_empty(), |el| {
                            el.child(self.render_comments(strings, cx))
                        }),
                )
                .track_scroll(&self.body_scroll)
                .flex_1()
                .min_h_0()
                .when_some(self.sticky_style, |el, style| {
                    el.color(gpui::rgb(style.foreground()).into())
                }),
            )
            .when(self.session.draft.as_ref().is_some_and(|draft| draft.save_failed), |el| {
                el.child(
                    div().px_5().py_1().child(
                        Label::new(strings.task_save_failed)
                            .color(sticky_tokens.map_or(Color::Error, |t| Color::Custom(t.error))),
                    ),
                )
            })
            .when(
                self.session.draft.as_ref().is_some_and(|draft| draft.pending_attachments > 0),
                |el| {
                    el.child(
                        div()
                            .px_5()
                            .py_1()
                            .child(Label::new(strings.attachment_preparing).color(muted)),
                    )
                },
            )
            // The bottom bar's "A" shows the description's Markdown toolbar over it, as the original.
            .when(self.format_bar && !read_only, |el| {
                let on_action: crate::markdown_toolbar::OnAction<Self> =
                    Rc::new(|this, action, window, cx| this.toolbar_action(action, window, cx));
                el.child(
                    div().px_3().pb_2().child(
                        crate::markdown_toolbar::render("detail-tool", self.format_headings, true, cx, on_action)
                            .flex_wrap()
                            .px_1()
                            .py_0p5()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .bg(theme.colors().elevated_surface_background),
                    ),
                )
            })
            .when(self.comment_open && !read_only, |el| el.child(self.render_comment_box(cx)))
            .when_some(self.session.activity.clone(), |el, activity| {
                el.child(self.render_activity(&activity, strings, cx))
            })
            // Bottom bar: list ……… formatting, comments, more; for a trashed task, as in the
            // original: Restore ……… delete forever; for an archived-list task, read-only list label.
            .child(self.render_footer_bar(&meta, window, cx))
    }
}

/// A description drawn as the original shows it while it is not edited: headings, numbered items
/// with their numbers in the accent color, bullets, check items, quotes and lines, with bold,
/// italic, underlined, struck, highlighted, code and link runs.
type MarkdownCheckToggle = Rc<dyn Fn(usize, &mut Window, &mut App)>;

pub fn render_markdown(
    text: &str,
    sticky: Option<crate::sticky_settings::StickySettings>,
    cx: &App,
) -> AnyElement {
    render_markdown_blocks(fern_core::markdown::blocks(text), sticky, true, None, cx)
}

fn render_markdown_interactive(
    text: &str,
    sticky: Option<crate::sticky_settings::StickySettings>,
    on_toggle_check: MarkdownCheckToggle,
    cx: &App,
) -> AnyElement {
    render_markdown_blocks(
        fern_core::markdown::blocks(text),
        sticky,
        true,
        Some(on_toggle_check),
        cx,
    )
}

pub fn render_release_markdown(text: &str, cx: &App) -> AnyElement {
    use fern_core::markdown::{Block, BlockKind, blocks};

    // The published Markdown is hard-wrapped in the source. Let the window wrap its prose,
    // keeping paragraph breaks and list markers where the author put them.
    let source = text.lines().map(str::trim_start).collect::<Vec<_>>().join("\n");
    let mut reflowed: Vec<Block> = Vec::new();
    for mut block in blocks(&source) {
        for (_, style) in &mut block.runs {
            if style
                .link
                .as_deref()
                .is_some_and(|url| !url.starts_with("https://") && !url.starts_with("http://"))
            {
                style.link = None;
            }
        }
        if block.kind == BlockKind::Paragraph
            && let Some(previous) = reflowed.last_mut()
            && matches!(
                previous.kind,
                BlockKind::Paragraph | BlockKind::Bullet | BlockKind::Numbered(_)
            )
        {
            previous.text.push(' ');
            let offset = previous.text.len();
            previous.text.push_str(&block.text);
            previous.runs.extend(
                block
                    .runs
                    .into_iter()
                    .map(|(range, style)| ((range.start + offset)..(range.end + offset), style)),
            );
        } else {
            reflowed.push(block);
        }
    }
    render_markdown_blocks(reflowed, None, false, None, cx)
}

fn render_markdown_blocks(
    blocks: Vec<fern_core::markdown::Block>,
    sticky: Option<crate::sticky_settings::StickySettings>,
    show_images: bool,
    on_toggle_check: Option<MarkdownCheckToggle>,
    cx: &App,
) -> AnyElement {
    use fern_core::markdown::BlockKind;
    let theme = cx.theme();
    let sticky_tokens = sticky.map(StickyPaletteTokens::from_settings);
    let line_height = px(sticky.map_or(22., |style| style.font_px() + 8.));
    let accent = sticky_tokens.map_or(theme.colors().text_accent, |t| t.accent);
    let muted = sticky_tokens.map_or(theme.colors().text_muted, |t| t.muted);
    let styled = |text: String, runs: Vec<(std::ops::Range<usize>, fern_core::markdown::Style)>| {
        let highlights = runs
            .into_iter()
            .map(|(range, style)| {
                let mut h = gpui::HighlightStyle::default();
                if style.bold {
                    h.font_weight = Some(FontWeight::BOLD);
                }
                if style.italic {
                    h.font_style = Some(gpui::FontStyle::Italic);
                }
                if style.strike {
                    h.strikethrough = Some(gpui::StrikethroughStyle {
                        thickness: px(1.),
                        color: None,
                    });
                }
                if style.underline || style.link.is_some() {
                    h.underline = Some(gpui::UnderlineStyle {
                        thickness: px(1.),
                        color: None,
                        wavy: false,
                    });
                }
                if style.link.is_some() {
                    h.color = Some(accent);
                }
                if style.highlight {
                    h.background_color = Some(theme.status().warning.opacity(0.35));
                }
                if style.code {
                    h.background_color = Some(sticky_tokens.map_or(theme.colors().element_hover, |t| t.surface));
                }
                (range, h)
            })
            .collect::<Vec<_>>();
        gpui::StyledText::new(text).with_highlights(highlights)
    };
    // Each Markdown block already accounts for its source line, including blank lines.
    v_flex()
        .text_size(px(15.))
        .line_height(line_height)
        .when_some(sticky, |el, settings| el.text_size(px(settings.font_px())))
        .children(blocks.into_iter().enumerate().map(|(ix, block)| {
            // Links open in the browser on a click, as Zed's Markdown opens them. The click is
            // taken on the release by the block, before the description's own click opens it for
            // editing; the text only gives links the pointing hand.
            let links: Vec<_> = block
                .runs
                .iter()
                .filter_map(|(range, style)| Some((range.clone(), style.link.clone()?)))
                .collect();
            let ranges = links.iter().map(|(range, _)| range.clone()).collect();
            let styled_text = styled(block.text.clone(), block.runs);
            let layout = styled_text.layout().clone();
            let text = gpui::InteractiveText::new(("md-block", ix), styled_text).on_click(ranges, |_, _, _| {});
            let body = div()
                .id(("md-body", ix))
                .flex_1()
                .min_w_0()
                .child(text)
                .when(!links.is_empty(), |el| {
                    el.on_click(move |event, _, cx| {
                        let Ok(at) = layout.index_for_position(event.position()) else { return };
                        if let Some((_, url)) = links.iter().find(|(range, _)| range.contains(&at)) {
                            cx.stop_propagation();
                            cx.open_url(url);
                        }
                    })
                });
            match block.kind {
                BlockKind::BlankLine => div().h(line_height).flex_none().into_any_element(),
                BlockKind::Heading(level) => div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(match level {
                        1 => 20.,
                        2 => 18.,
                        _ => 16.,
                    }))
                    .child(body)
                    .into_any_element(),
                BlockKind::Numbered(n) => h_flex()
                    .items_start()
                    .child(div().w(px(19.)).flex_none().text_color(accent).child(format!("{n}.")))
                    .child(body)
                    .into_any_element(),
                BlockKind::Bullet => h_flex()
                    .items_start()
                    .child(div().w(px(22.)).flex_none().text_color(accent).child("•"))
                    .child(body)
                    .into_any_element(),
                BlockKind::Check(done) => {
                    let cb = if let Some(t) = sticky_tokens {
                        let cb = sticky_checkbox(("md-check", ix), done, false, false, None, t);
                        if let Some(toggle) = on_toggle_check.clone() {
                            cb.cursor_pointer()
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    toggle(ix, window, cx);
                                })
                                .into_any_element()
                        } else {
                            cb.into_any_element()
                        }
                    } else {
                        let cb = Checkbox::new(("md-check", ix)).size(px(14.)).toggle_state(done);
                        if let Some(toggle) = on_toggle_check.clone() {
                            cb.on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                toggle(ix, window, cx);
                            })
                            .into_any_element()
                        } else {
                            cb.into_any_element()
                        }
                    };
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(div().pt(px(4.)).child(cb))
                        .child(body.when(done, |el| el.line_through().text_color(muted)))
                        .into_any_element()
                }
                BlockKind::Quote => div()
                    .pl_3()
                    .border_l_2()
                    .border_color(accent)
                    .text_color(muted)
                    .child(body)
                    .into_any_element(),
                BlockKind::Divider => div()
                    .my_1()
                    .h(px(1.))
                    .bg(sticky_tokens.map_or(theme.colors().border, |t| t.border))
                    .into_any_element(),
                BlockKind::Image(_) if !show_images => body.text_color(muted).into_any_element(),
                // An image line shows the image at most as wide as the panel, as the original; a
                // relative address is a file among the attachments.
                BlockKind::Image(url) => {
                    let source: gpui::ImageSource = if url.starts_with("http://") || url.starts_with("https://") {
                        gpui::SharedUri::from(url).into()
                    } else {
                        let path = std::path::PathBuf::from(url.strip_prefix("file://").unwrap_or(&url));
                        let path = if path.is_relative() {
                            crate::attachments_dir()
                                .map(|dir| dir.join(&path))
                                .unwrap_or(path)
                        } else {
                            path
                        };
                        path.into()
                    };
                    let alt = SharedString::from(block.text);
                    // In a block of its own, as Zed's Markdown wraps its images, so that the
                    // column does not squeeze it to nothing.
                    div()
                        .flex_none()
                        .child(
                            gpui::img(source)
                                .id(("md-image", ix))
                                .min_w_0()
                                .max_w_full()
                                .rounded_md()
                                .with_fallback(move || div().text_color(muted).child(alt.clone()).into_any_element()),
                        )
                        .into_any_element()
                }
                BlockKind::Paragraph => body.into_any_element(),
            }
        }))
        .into_any_element()
}
