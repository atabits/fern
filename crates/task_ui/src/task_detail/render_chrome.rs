use chrono::NaiveDate;
use fern_core::{Kind, Task};
use gpui::{Focusable, MouseButton};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, TipSide, with_tip};

use crate::calendar;
use crate::due_picker;
use crate::i18n::Strings;
use crate::task_rows;

use super::{OpenTask, Popup, StickyPaletteTokens, TaskDetail};

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

pub(super) struct DueBadge {
    text: SharedString,
    color: Color,
    icon: IconName,
    has_date: bool,
    repeats: bool,
}

pub(super) struct RenderMeta<'a> {
    pub(super) task: &'a Task,
    pub(super) kind: Kind,
    pub(super) done: bool,
    pub(super) in_progress: bool,
    pub(super) due: DueBadge,
    pub(super) priority_color: Color,
    pub(super) list_icon: IconName,
    pub(super) list_name: SharedString,
    pub(super) read_only: bool,
    pub(super) sticky: bool,
    pub(super) sticky_tokens: Option<StickyPaletteTokens>,
    pub(super) muted: Color,
    pub(super) button_hover: Hsla,
    pub(super) strings: &'static Strings,
    pub(super) theme: &'a theme::Theme,
}

impl TaskDetail {
    pub(super) fn render_tags_row(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn due_badge(
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

    pub(super) fn render_header(
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

    pub(super) fn render_parent_row(
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

    pub(super) fn render_location_row(
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

    pub(super) fn render_footer_bar(
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
                                    } else {
                                        this.comment_emoji = false;
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
