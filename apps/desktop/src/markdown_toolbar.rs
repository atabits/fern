//! The original's Markdown toolbar, shared by the Summary editor and the task description:
//! headings, inline formats, lists, a line, the time, links, code and quotes, with the Markdown
//! each button writes in its tooltip.

use std::rc::Rc;

use fern_core::markdown::Format;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, Tooltip, popover};

use crate::i18n;

/// What a toolbar button asks of its editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarAction {
    /// Format the selection; `None` is the Time button, which types the current time.
    Format(Option<Format>),
    /// Open or close the Heading Options menu.
    ToggleHeadings,
    /// The description's paperclip: Upload Attachment.
    Attach,
}

pub type OnAction<V> = Rc<dyn Fn(&mut V, ToolbarAction, &mut Window, &mut Context<V>)>;

/// The toolbar row. `id` keeps two toolbars' buttons apart; `attach` makes it the description's:
/// 19 px buttons close together, to fit the details' width in one row, and the paperclip at the end, as the
/// original's.
pub fn render<V: 'static>(id: &'static str, headings_open: bool, attach: bool, cx: &mut Context<V>, on_action: OnAction<V>) -> Div {
    let size = if attach { px(19.) } else { px(28.) };
    let strings = i18n::t(cx);
    let colors = cx.theme().colors().clone();
    let names = strings.markdown_tools;
    // (format, icon, the tooltip's sample of the Markdown, the shortcut)
    let tools: [(Option<Format>, IconName, String, Option<&str>); 13] = [
        (Some(Format::Bold), IconName::Bold, format!("**{}**", names[0]), Some("secondary-b")),
        (Some(Format::Highlight), IconName::Highlighter, format!("::{}::", names[1]), Some("secondary-shift-y")),
        (Some(Format::Checklist), IconName::SquareCheck, format!("- [ ] {}", names[2]), Some("secondary-shift-s")),
        (Some(Format::Bullets), IconName::List, format!("* {}", names[3]), Some("secondary-shift-8")),
        (Some(Format::Numbered), IconName::ListOrdered, format!("1. {}", names[4]), Some("secondary-shift-7")),
        (Some(Format::Italic), IconName::Italic, format!("*{}*", names[5]), Some("secondary-i")),
        (Some(Format::Underline), IconName::Underline, format!("~{}~", names[6]), Some("secondary-u")),
        (Some(Format::Strike), IconName::Strikethrough, format!("~~{}~~", names[7]), Some("secondary-shift-x")),
        (Some(Format::Divider), IconName::SeparatorHorizontal, format!("--- {}", names[8]), None),
        (None, IconName::History, names[9].to_owned(), None),
        (Some(Format::Link), IconName::Link, format!("[{}](URL)", names[10]), Some("secondary-k")),
        (Some(Format::Code), IconName::Code, format!("`{}`", names[11]), Some("secondary-alt-c")),
        (Some(Format::Quote), IconName::Quote, format!("> {}", names[12]), Some("secondary-shift-u")),
    ];
    let listener = |action: ToolbarAction, cx: &Context<V>| {
        let (on_action, view) = (on_action.clone(), cx.entity().downgrade());
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| on_action(this, action, window, cx)).ok();
        }
    };
    let separator = || div().w(px(1.)).h(px(16.)).when(attach, |el| el.mx_0p5()).when(!attach, |el| el.mx_1()).bg(colors.border_variant);
    let heading_menu = headings_open.then(|| {
        let close = {
            let (on_action, view) = (on_action.clone(), cx.entity().downgrade());
            move |_: &gpui::MouseDownEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |this, cx| on_action(this, ToolbarAction::ToggleHeadings, window, cx)).ok();
            }
        };
        let items: Vec<_> = (1..=3u8)
            .map(|level| {
                ListItem::new((id, level as usize))
                    .start_slot(Label::new(format!("H{level}")).size(LabelSize::Small).color(Color::Muted))
                    .child(Label::new(format!("{} {level}", strings.heading)))
                    .on_click(listener(ToolbarAction::Format(Some(Format::Heading(level))), cx))
            })
            .collect();
        PopoverAttachment::new(Placement::BelowLeft, popover(cx).w(px(170.)).on_mouse_down_out(close).children(items))
    });
    let mut row = h_flex().when(!attach, |el| el.gap_0p5()).child(
        div()
            .relative()
            .child(
                IconButton::new(SharedString::from(format!("{id}-heading")), IconName::Heading)
                    .size(size)
                    .icon_color(Color::Muted)
                    .tooltip(Tooltip::text(strings.heading_options))
                    .on_click(listener(ToolbarAction::ToggleHeadings, cx)),
            )
            .children(heading_menu),
    );
    for (i, (format, icon, sample, keys)) in tools.into_iter().enumerate() {
        // The original groups its buttons: text, lists, styles and lines, then inserts.
        if [2, 5, 10].contains(&i) {
            row = row.child(separator());
        }
        let tooltip = match keys {
            Some(keys) => format!("{sample} ({})", crate::keymap::display_keys(keys)),
            None => sample,
        };
        row = row.child(
            IconButton::new((id, i), icon)
                .size(size)
                .icon_color(if format == Some(Format::Highlight) { Color::Warning } else { Color::Muted })
                .tooltip(Tooltip::text(tooltip))
                .on_click(listener(ToolbarAction::Format(format), cx)),
        );
    }
    if attach {
        row = row.child(separator()).child(
            IconButton::new(SharedString::from(format!("{id}-attach")), IconName::Paperclip)
                .size(size)
                .icon_color(Color::Muted)
                .tooltip(Tooltip::text(strings.upload_attachment))
                .on_click(listener(ToolbarAction::Attach, cx)),
        );
    }
    row
}
