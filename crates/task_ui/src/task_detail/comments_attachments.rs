use chrono::Local;
use gpui::{Focusable, MouseButton};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::i18n::{self, Strings};

use super::{TaskDetail, TaskUpdated, format_size};

impl TaskDetail {
    /// Formats a task's comments section.
    pub(super) fn render_comments(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();
        let read_only = self.is_read_only();
        v_flex()
            .pt_3()
            .gap_2()
            .child(
                Label::new(format!("{} {}", strings.comments, self.session.comments.len()))
                    .size(LabelSize::Small)
                    .color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted))),
            )
            .children(self.session.comments.iter().map(|comment| {
                let id = comment.id;
                let when = chrono::DateTime::from_timestamp_millis(comment.created_at)
                    .map(|t| t.with_timezone(&Local).naive_local())
                    .map(|t| format!("{} {}", crate::calendar::format_date(t.date(), strings), crate::calendar::format_time(t.time(), strings)))
                    .unwrap_or_default();
                let group_name = SharedString::from(format!("comment-row-{id}"));
                h_flex()
                    .id(SharedString::from(format!("comment-{id}")))
                    .group(group_name.clone())
                    .items_start()
                    .gap_2()
                    .p_2()
                    .rounded(px(6.))
                    .bg(sticky.map_or(theme.colors().surface_background, |t| t.surface))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(Label::new(comment.text.clone()).when_some(sticky, |l, t| l.color(t.ink)))
                            .child(
                                Label::new(when)
                                    .size(LabelSize::XSmall)
                                    .color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted))),
                            ),
                    )
                    .when(!read_only, |el| {
                        el.child(
                            div().when(!crate::touch::active(), |el| el.invisible()).group_hover(group_name, |s| s.visible()).child(
                                IconButton::new(SharedString::from(format!("comment-delete-{id}")), IconName::X)
                                    .icon_size(IconSize::XSmall)
                                    .icon_color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted)))
                                    .when_some(sticky, |b, t| b.hover_color(t.surface))
                                    .tooltip(Tooltip::text(strings.delete))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.is_read_only() {
                                            return;
                                        }
                                        if this.session.store.delete_comment(id).is_ok() {
                                            cx.emit(TaskUpdated);
                                        }
                                        this.sync_comments();
                                        cx.notify();
                                    })),
                            ),
                        )
                    })
            }))
            .into_any_element()
    }

    /// The original's comment field over the bottom bar: "Write a comment" with an emoji button.
    pub(super) fn render_comment_box(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();
        let emoji_menu = self.comment_emoji.then(|| {
            let picks: Vec<_> = crate::emoji::search("").into_iter().take(40).map(|e| {
                div()
                    .id(SharedString::from(format!("comment-emoji-{e}")))
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(div().text_size(px(18.)).child(e))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.comment_emoji = false;
                        this.comment_input.update(cx, |input, cx| {
                            let (text, selection) = fern_core::markdown::insert(input.text(), input.selection(), e);
                            input.edit(text, selection, window, cx);
                        });
                        window.focus(&this.comment_input.focus_handle(cx), cx);
                        cx.notify();
                    }))
            }).collect();
            PopoverAttachment::new(
                Placement::AboveRight,
                popover(cx)
                    .w(px(260.))
                    .p_2()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.comment_emoji = false;
                        cx.notify();
                    }))
                    .child(h_flex().flex_wrap().children(picks)),
            )
        });
        let has_comment_text = !self.comment_input.read(cx).text().trim().is_empty();
        div()
            .when(sticky.is_some(), |el| el.px_3())
            .when(sticky.is_none(), |el| el.px_5())
            .pb_2()
            .child(
                h_flex()
                    .h(px(36.))
                    .pl_3()
                    .pr_1()
                    .gap_1()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(sticky.map_or(theme.colors().accent, |t| t.accent))
                    .bg(sticky.map_or(theme.colors().surface_background, |t| t.surface))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().flex_1().child(self.comment_input.clone()))
                    .child(
                        div().relative().child(
                            IconButton::new("comment-emoji", IconName::Smile)
                                .icon_color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted)))
                                .when_some(sticky, |b, t| b.hover_color(t.surface))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.comment_emoji = !this.comment_emoji;
                                    cx.notify();
                                })),
                        )
                        .children(emoji_menu),
                    )
                    .when(has_comment_text, |el| {
                        el.child(
                            IconButton::new("comment-send", IconName::ArrowUp)
                                .icon_color(sticky.map_or(Color::Accent, |t| Color::Custom(t.accent)))
                                .when_some(sticky, |b, t| b.hover_color(t.surface))
                                .tooltip(Tooltip::text(i18n::t(cx).add))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let text = this.comment_input.read(cx).text().trim().to_owned();
                                    if !text.is_empty()
                                        && let Some(task) = this.task_id()
                                        && this.session.store.add_comment(task, &text).is_ok()
                                    {
                                        this.comment_input.update(cx, |input, cx| input.clear(window, cx));
                                        this.sync_comments();
                                        cx.emit(TaskUpdated);
                                        cx.notify();
                                    }
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The attached files under the notes: a click opens one with the system's app.
    pub(super) fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();
        let strings = i18n::t(cx);
        let read_only = self.is_read_only();
        let hover_bg = sticky.map_or(theme.colors().element_hover, |t| t.surface);
        let row_border = sticky.map_or(theme.colors().border_variant, |t| t.border);
        let muted_color = sticky.map_or(Color::Muted, |t| Color::Custom(t.muted));
        let delete_hover_color = sticky.map_or(theme.status().error, |t| t.error);
        v_flex().gap_1().children(self.session.attachments.iter().map(|attachment| {
            let path = match &self.session.draft {
                Some(draft) => draft.attachments.iter().find(|prepared| prepared.metadata().id == attachment.id).map(|prepared| prepared.path().to_owned()),
                None => self.session.attachments_dir.as_ref().map(|dir| attachment.path(dir)),
            };
            let removed = attachment.clone();
            let att_id = attachment.id;
            let group_name = SharedString::from(format!("attachment-{att_id}"));
            let is_image = attachment
                .name
                .rsplit('.')
                .next()
                .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"));
            h_flex()
                .id(group_name.clone())
                .debug_selector(|| "task-attachment".to_owned())
                .group(group_name)
                .h(px(36.))
                .px_2()
                .gap_2()
                .rounded(px(6.))
                .border_1()
                .border_color(row_border)
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .child(
                    Icon::new(if is_image { IconName::Image } else { IconName::Paperclip })
                        .size(IconSize::Small)
                        .color(muted_color),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Label::new(attachment.name.clone())
                            .size(LabelSize::Small)
                            .when_some(sticky, |l, t| l.color(t.ink))
                            .truncate(),
                    ),
                )
                .child(
                    Label::new(format_size(attachment.size))
                        .size(LabelSize::XSmall)
                        .color(muted_color),
                )
                .when(!read_only, |el| {
                    el.child(
                        div()
                            .when(cfg!(test), |el| {
                                el.debug_selector(|| "task-attachment-delete".to_owned())
                            })
                            .child(
                                IconButton::new(SharedString::from(format!("attachment-delete-{att_id}")), IconName::X)
                                    .size(px(20.))
                                    .icon_size(IconSize::XSmall)
                                    .when_some(sticky, |b, t| b.icon_color(t.muted).hover_color(t.surface))
                                    .hover_icon_color(delete_hover_color)
                                    .tooltip(Tooltip::text(strings.delete))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        if this.is_read_only() {
                                            return;
                                        }
                                        if let Some(draft) = &mut this.session.draft {
                                            draft.attachments.retain(|prepared| prepared.metadata().id != removed.id);
                                            this.sync_attachments();
                                            cx.notify();
                                            return;
                                        }
                                        this.confirm_delete_attachment = Some(removed.clone());
                                        this.confirm_return_focus =
                                            window.focused(cx).map(|handle| handle.downgrade());
                                        window.focus(&this.focus, cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                })
                .on_click(move |_, _, cx| {
                    if let Some(path) = &path {
                        cx.open_with_system(path);
                    }
                })
        }))
    }
}
