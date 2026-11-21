use fern_core::matrix::{MatrixDate, MatrixKind, QuadrantRule};
use fern_core::Priority;
use gpui::FontWeight;
use ui::prelude::*;
use ui::{Button, ButtonSize, ButtonVariant, Modal, Placement, PopoverAttachment, ToggleChip, popover};

use desktop_common::habit_helpers::HABIT_EMOJI;
use fern_i18n as i18n;

use crate::MatrixView;

impl MatrixView {
    /// Modal dialog "Edit Matrix": custom name (with emoji picker & validation), lists, tags
    /// (case-insensitive + unavailable tag chips), dates, priorities, and item type.
    /// Clicking outside does NOT discard edits; only explicit Close / Escape does.
    pub(crate) fn render_edit_modal(&self, quadrant: usize, rule: &QuadrantRule, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let color = Self::quadrant_color(quadrant);
        let (numeral, _) = self.quadrant_title(quadrant, strings);
        let typed_name = self.name_input.read(cx).text().to_owned();
        let name_valid = QuadrantRule::validate_name(&typed_name);

        let chip = |id: SharedString, label: SharedString, on: bool, change: Box<dyn Fn(&mut QuadrantRule)>| {
            ToggleChip::new(id, label)
                .toggle_state(on)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some((_, rule)) = this.editing.as_mut() {
                        change(rule);
                    }
                    cx.notify();
                }))
        };
        fn toggle<T: PartialEq + Clone>(items: &mut Vec<T>, item: &T) {
            match items.iter().position(|i| i == item) {
                Some(at) => {
                    items.remove(at);
                }
                None => items.push(item.clone()),
            }
        }
        fn toggle_tag_ci(items: &mut Vec<String>, tag: &str) {
            let lower = tag.to_lowercase();
            match items.iter().position(|t| t.to_lowercase() == lower) {
                Some(at) => {
                    items.remove(at);
                }
                None => items.push(tag.to_owned()),
            }
        }
        let row = |label: &'static str, chips: Vec<ToggleChip>| {
            h_flex()
                .gap_2()
                .items_start()
                .child(div().w(px(74.)).flex_none().pt_1().child(Label::new(label).size(LabelSize::Small).color(Color::Muted)))
                .child(div().flex_1().flex().flex_wrap().gap_1p5().children(chips))
        };
        let all = |id: &'static str, empty: bool, clear: Box<dyn Fn(&mut QuadrantRule)>| {
            chip(id.into(), strings.filter_all.into(), empty, clear)
        };

        // Lists
        let mut list_chips = vec![all("matrix-lists-all", rule.lists.is_empty(), Box::new(|r: &mut QuadrantRule| r.lists.clear()))];
        let unavailable_lists = rule
            .lists
            .iter()
            .flatten()
            .copied()
            .filter(|id| !self.lists.iter().any(|list| list.id == *id))
            .map(|id| (Some(id), strings.matrix_unavailable_list.to_owned()));
        let list_choices = std::iter::once((None, strings.inbox.to_owned()))
            .chain(self.lists.iter().filter(|l| !l.archived).map(|list| (Some(list.id), list.name.clone())))
            .chain(unavailable_lists);
        for (id, name) in list_choices {
            list_chips.push(chip(
                SharedString::from(format!("matrix-list-{}", id.map_or("inbox".to_owned(), |i| i.to_string()))),
                name.into(),
                rule.lists.contains(&id),
                Box::new(move |r: &mut QuadrantRule| toggle(&mut r.lists, &id)),
            ));
        }

        // Tags (case-insensitive matching + display stale/unavailable tags)
        let mut tag_chips = vec![all("matrix-tags-all", rule.tags.is_empty(), Box::new(|r: &mut QuadrantRule| r.tags.clear()))];
        for tag in self.tags.iter().map(|t| &t.name) {
            let name = tag.clone();
            let active = rule.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase());
            tag_chips.push(chip(
                SharedString::from(format!("matrix-tag-{tag}")),
                tag.clone().into(),
                active,
                Box::new(move |r: &mut QuadrantRule| toggle_tag_ci(&mut r.tags, &name)),
            ));
        }
        for stale in rule
            .tags
            .iter()
            .filter(|rt| !self.tags.iter().any(|t| t.name.to_lowercase() == rt.to_lowercase()))
        {
            let label = format!("{stale} ({})", strings.matrix_unavailable_tag);
            let remove_name = stale.clone();
            tag_chips.push(chip(
                SharedString::from(format!("matrix-tag-stale-{stale}")),
                label.into(),
                true,
                Box::new(move |r: &mut QuadrantRule| toggle_tag_ci(&mut r.tags, &remove_name)),
            ));
        }

        // Dates
        let mut date_chips = vec![all("matrix-dates-all", rule.dates.is_empty(), Box::new(|r: &mut QuadrantRule| r.dates.clear()))];
        for (i, date) in MatrixDate::ALL.into_iter().enumerate() {
            date_chips.push(chip(
                SharedString::from(format!("matrix-date-{i}")),
                strings.matrix_dates[i].into(),
                rule.dates.contains(&date),
                Box::new(move |r: &mut QuadrantRule| toggle(&mut r.dates, &date)),
            ));
        }

        // Priorities
        let mut priority_chips =
            vec![all("matrix-priorities-all", rule.priorities.is_empty(), Box::new(|r: &mut QuadrantRule| r.priorities.clear()))];
        for (priority, label) in [
            (Priority::High, strings.priority_high),
            (Priority::Medium, strings.priority_medium),
            (Priority::Low, strings.priority_low),
            (Priority::None, strings.priority_none),
        ] {
            priority_chips.push(chip(
                SharedString::from(format!("matrix-priority-{}", priority.to_i64())),
                label.into(),
                rule.priorities.contains(&priority),
                Box::new(move |r: &mut QuadrantRule| toggle(&mut r.priorities, &priority)),
            ));
        }

        // Type (All / Task / Note)
        let kind_chips: Vec<ToggleChip> = MatrixKind::ALL
            .into_iter()
            .enumerate()
            .map(|(i, kind)| {
                chip(
                    SharedString::from(format!("matrix-kind-{i}")),
                    strings.matrix_kinds[i].into(),
                    rule.kind == kind,
                    Box::new(move |r: &mut QuadrantRule| r.kind = kind),
                )
            })
            .collect();

        Modal::new("matrix-edit-backdrop")
            .width(px(500.))
            .max_height(px(600.))
            .child(
                h_flex()
                    .justify_between()
                    .child(Label::new(strings.edit_matrix).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                    .child(
                        IconButton::new("matrix-edit-x", IconName::X)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.finish_edit(None, cx);
                            })),
                    ),
            )
            // Name row + emoji picker
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .h(px(34.))
                            .px_2()
                            .gap_2()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(if name_valid { theme.colors().border } else { theme.status().error })
                            .bg(theme.colors().background)
                            .child(
                                div()
                                    .size(px(16.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(color)
                                    .text_color(theme.colors().on_accent)
                                    .text_size(px(8.))
                                    .font_weight(FontWeight::BOLD)
                                    .child(numeral),
                            )
                            .child(div().flex_1().min_w_0().child(self.name_input.clone()))
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        IconButton::new("matrix-name-emoji", IconName::Smile)
                                            .icon_size(IconSize::Small)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.emoji_picker_open = !this.emoji_picker_open;
                                                cx.notify();
                                            })),
                                    )
                                    .when(self.emoji_picker_open, |el| {
                                        el.child(PopoverAttachment::new(
                                            Placement::BelowRight,
                                            popover(cx)
                                                .w(px(236.))
                                                .p_2()
                                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                                    this.emoji_picker_open = false;
                                                    cx.notify();
                                                }))
                                                .child(div().flex().flex_wrap().gap_1().children(
                                                    HABIT_EMOJI.iter().enumerate().map(|(i, emoji)| {
                                                        div()
                                                            .id(SharedString::from(format!("matrix-em-{i}")))
                                                            .size(px(28.))
                                                            .rounded(px(6.))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .cursor_pointer()
                                                            .hover(|s| s.bg(theme.colors().element_hover))
                                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                                let current = this.name_input.read(cx).text().to_owned();
                                                                let next = if current.trim().is_empty() {
                                                                    (*emoji).to_owned()
                                                                } else {
                                                                    format!("{emoji} {current}")
                                                                };
                                                                this.name_input.update(cx, |inp, cx| inp.set_text(next, cx));
                                                                this.emoji_picker_open = false;
                                                                cx.notify();
                                                            }))
                                                            .child(*emoji)
                                                    }),
                                                )),
                                        ))
                                    }),
                            ),
                    )
                    .when(!name_valid, |el| {
                        el.child(Label::new(strings.matrix_invalid_name).size(LabelSize::XSmall).color(Color::Error))
                    }),
            )
            .child(
                v_scroll(
                    "matrix-edit-rows",
                    v_flex()
                        .id("matrix-edit-rows-inner")
                        .gap_3()
                        .child(row(strings.filter_lists, list_chips))
                        .child(row(strings.tags, tag_chips))
                        .child(row(strings.filter_date, date_chips))
                        .child(row(strings.filter_priority, priority_chips))
                        .child(row(strings.matrix_kind_label, kind_chips)),
                )
                .min_h_0()
                .max_h(px(380.))
                .compact(),
            )
            .child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .child(
                        Button::new("matrix-restore", strings.restore)
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Sm)
                            .on_click(cx.listener(|this, _, _, cx| this.finish_edit(Some(false), cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("matrix-close", strings.close)
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.finish_edit(None, cx);
                            })),
                    )
                    .child(
                        Button::new("matrix-save", strings.save)
                            .variant(ButtonVariant::Primary)
                            .disabled(!name_valid)
                            .on_click(cx.listener(|this, _, _, cx| this.finish_edit(Some(true), cx))),
                    ),
            )
            .into_any_element()
    }
}
