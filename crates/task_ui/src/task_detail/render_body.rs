use std::rc::Rc;

use chrono::{Local, TimeZone as _};
use fern_core::{Id, Task};
use gpui::{ExternalPaths, FocusHandle, Focusable, FontWeight, MouseButton};
use ui::prelude::*;

use crate::actions::{self as md, DismissTransient};
use crate::calendar;
use crate::i18n::{self, Strings};
use crate::markdown_toolbar::ToolbarAction;
use crate::task_rows;

use super::render_chrome::RenderMeta;
use super::{
    FocusTarget, PendingDelete, Popup, StickyPaletteTokens, TaskDetail, activity_text,
    sticky_checkbox,
};

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

    fn render_pending_delete(
        &self,
        pending: PendingDelete,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (dialog_id, modal_selector, title, description) = match &pending {
            PendingDelete::Attachment(att) => (
                "task-attachment-delete",
                "task-attachment-delete-modal",
                strings.confirm_delete_attachment,
                att.name.clone(),
            ),
            PendingDelete::Comment { preview, .. } => (
                "task-comment-delete",
                "task-comment-delete-modal",
                strings.confirm_delete_comment,
                preview.clone(),
            ),
        };
        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus)
            .when(cfg!(test), |el| {
                el.debug_selector(move || modal_selector.to_owned())
            })
            .child(
                ui::ConfirmDialog::new(dialog_id, title)
                    .width(px(340.))
                    .when(!description.is_empty(), |dialog| {
                        dialog.description(description)
                    })
                    .cancel_label(strings.cancel)
                    .confirm_label(strings.delete)
                    .destructive(true)
                    .on_cancel(cx.listener(|this, _, window, cx| {
                        this.dismiss_pending_delete(window, cx);
                    }))
                    .on_confirm(cx.listener(move |this, _, window, cx| {
                        this.confirm_pending_delete(pending.clone(), window, cx);
                    })),
            )
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
            .relative()
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
                if this.dismiss_pending_delete(window, cx) {
                    return;
                }
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
            .when_some(self.pending_delete.clone(), |el, pending| {
                el.child(self.render_pending_delete(pending, strings, cx))
            })
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
