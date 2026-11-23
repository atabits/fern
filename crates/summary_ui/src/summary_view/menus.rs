use std::cell::Cell;
use std::rc::Rc;

use chrono::Local;
use fern_core::Priority;
use fern_core::summary::{Field, Grouping, Period, StatusFilter};
use gpui::{Entity, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, SubmenuItem, Tooltip, popover};

use crate::i18n;
use crate::text_input::TextInput;

use super::{Menu, Pick, SummaryView, period_label};

impl SummaryView {
    /// A Filter or Display Options row whose value opens a dropdown.
    fn row(
        &self,
        label: &'static str,
        value: String,
        menu: Menu,
        options: Vec<(String, bool, Pick)>,
        // Fields stay open to pick several.
        keep_open: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.menu == Some(menu);
        let dropdown = open.then(|| {
            let options = Rc::new(options);
            let items = (0..options.len()).map(|i| {
                let (label, selected, _) = &options[i];
                let options = options.clone();
                ListItem::new(SharedString::from(format!("summary-option-{i}")))
                    .child(Label::new(label.clone()).color(if *selected { Color::Accent } else { Color::Default }))
                    .when(*selected, |item| item.end_slot(Icon::new(IconName::Check).color(Color::Accent).size(IconSize::Small)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        (options[i].2)(this, window, cx);
                        if !keep_open {
                            this.set_menu(None, cx);
                        }
                        if this.pending_options.is_some() {
                            window.focus(&this.focus_handle, cx);
                        }
                    }))
            });
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .min_w(px(200.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_menu(None, cx)))
                    .child(
                        v_scroll(
                            ("summary-options-scroll-view", menu as usize),
                            v_flex().id(("summary-options-scroll", menu as usize)).children(items),
                        )
                        .max_h(px(280.))
                        .compact(),
                    ),
            )
        });
        let hover_bg = cx.theme().colors().ghost_element_hover;
        h_flex()
            .h(px(40.))
            .px_4()
            .child(div().flex_1().child(Label::new(label)))
            .child(
                h_flex()
                    .id(SharedString::from(format!("summary-{label}")))
                    .relative()
                    .px_1p5()
                    .py_0p5()
                    .gap_1()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .child(Label::new(value).color(Color::Muted))
                    .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_menu((!open).then_some(menu), cx)))
                    .children(dropdown),
            )
    }

    /// The original's Template row: saved option sets to apply with a click, and "+" to save the
    /// current ones under a name.
    fn render_templates(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let templates = self
            .cache
            .as_ref()
            .map(|c| c.templates.as_slice())
            .unwrap_or(&[]);
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(templates.iter().map(|template| {
                        let selected = template.options == self.options;
                        let (id, template_name, options) = (template.id, template.name.clone(), template.options.clone());
                        let group = SharedString::from(format!("template-{id}"));
                        h_flex()
                            .id(group.clone())
                            .group(group.clone())
                            .when(cfg!(test), |el| el.debug_selector(move || format!("template-{id}")))
                            .h(px(26.))
                            .pl_2()
                            .pr_1()
                            .gap_1()
                            .rounded(px(13.))
                            .border_1()
                            .border_color(if selected { colors.accent } else { colors.border })
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.ghost_element_hover))
                            .child(Label::new(template.name.clone()).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }))
                            .child(
                                div()
                                    .when(!crate::touch::active(), |el| el.invisible())
                                    .group_hover(group, |s| s.visible())
                                    .when(cfg!(test), |el| el.debug_selector(move || format!("delete-template-{id}")))
                                    .child(
                                        IconButton::new(SharedString::from(format!("delete-template-{id}")), IconName::X)
                                            .size(px(16.))
                                            .icon_size(IconSize::XSmall)
                                            .icon_color(Color::Muted)
                                            .hover_icon_color(Color::Error)
                                            .tooltip(Tooltip::text(strings.delete))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.confirm_delete_template = Some((id, template_name.clone()));
                                                window.focus(&this.focus_handle, cx);
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let options = options.clone();
                                this.update(cx, |o| *o = options);
                                if this.pending_options.is_some() {
                                    window.focus(&this.focus_handle, cx);
                                }
                            }))
                    }))
                    .child(
                        h_flex()
                            .id("add-summary-template")
                            .h(px(26.))
                            .justify_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.surface_background)
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.element_hover))
                            .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Muted))
                            .when(templates.is_empty(), |el| {
                                el.px_2()
                                    .gap_1()
                                    .child(Label::new(strings.summary_save_template).size(LabelSize::Small))
                            })
                            .when(!templates.is_empty(), |el| {
                                el.w(px(26.))
                                    .tooltip(Tooltip::text(strings.summary_save_template))
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.start_template(window, cx))),
                    ),
            )
            .children(self.naming.clone().map(|input| {
                let can_save = !input.read(cx).text().trim().is_empty();
                v_flex()
                    .gap_1p5()
                    .child(Label::new(strings.summary_template_hint).size(LabelSize::XSmall).color(Color::Muted))
                    .child(h_flex().h(px(28.)).px_2().rounded(px(6.)).bg(colors.surface_background).child(input.clone()))
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                div()
                                    .id("template-close")
                                    .px_3()
                                    .h(px(26.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(colors.border)
                                    .hover(|s| s.bg(colors.element_hover))
                                    .cursor_pointer()
                                    .child(Label::new(strings.close).size(LabelSize::Small))
                                    .on_click(cx.listener(|this, _, window, cx| this.cancel_template(window, cx))),
                            )
                            .child(
                                div()
                                    .id("template-save")
                                    .px_3()
                                    .h(px(26.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(6.))
                                    .bg(colors.accent.opacity(if can_save { 1. } else { 0.5 }))
                                    .when(can_save, |el| el.cursor_pointer().hover(|s| s.opacity(0.9)))
                                    .child(Label::new(strings.save).size(LabelSize::Small).color(Color::OnAccent))
                                    .when(can_save, |el| {
                                        el.on_click(cx.listener(move |this, _, window, cx| {
                                            let name = input.read(cx).text().to_owned();
                                            this.save_template(name, window, cx);
                                        }))
                                    }),
                            ),
                    )
            }))
    }

    /// The original's "More" filter: Tag and Priority side menus with checkable entries.
    fn render_more_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let options = &self.options;
        let open = self.menu == Some(Menu::More);
        let priorities = [
            (Priority::High, strings.priority_high),
            (Priority::Medium, strings.priority_medium),
            (Priority::Low, strings.priority_low),
            (Priority::None, strings.priority_none),
        ];
        let mut parts = Vec::new();
        if let Some(tags) = &options.tags {
            parts.push(tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(", "));
        }
        if let Some(chosen) = &options.priorities {
            parts.push(priorities.iter().filter(|(p, _)| chosen.contains(p)).map(|(_, n)| *n).collect::<Vec<_>>().join(", "));
        }
        let value = if parts.is_empty() { strings.none_option.to_owned() } else { parts.join("; ") };
        let check = |selected: bool, item: ListItem| {
            item.when(selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
        };
        let dropdown = open.then(|| {
            let tags = self
                .cache
                .as_ref()
                .map(|c| c.tags.as_slice())
                .unwrap_or(&[]);
            let tag_items = std::iter::once(
                check(options.tags.is_none(), ListItem::new("more-tag-all").child(Label::new(strings.summary_all_tags)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.update(cx, |o| o.tags = None);
                        if this.pending_options.is_some() {
                            window.focus(&this.focus_handle, cx);
                        }
                    }))
                    .into_any_element(),
            )
            .chain(tags.iter().map(|tag| {
                let name = tag.name.clone();
                let selected = options.tags.as_ref().is_some_and(|t| t.contains(&name));
                check(selected, ListItem::new(SharedString::from(format!("more-tag-{name}"))).child(Label::new(format!("#{name}"))))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let name = name.clone();
                        this.update(cx, |o| {
                            let mut chosen = o.tags.take().unwrap_or_default();
                            match chosen.iter().position(|t| *t == name) {
                                Some(i) => {
                                    chosen.remove(i);
                                }
                                None => chosen.push(name),
                            }
                            o.tags = (!chosen.is_empty()).then_some(chosen);
                        });
                        if this.pending_options.is_some() {
                            window.focus(&this.focus_handle, cx);
                        }
                    }))
                    .into_any_element()
            }))
            .collect::<Vec<_>>();
            let priority_items = std::iter::once(
                check(options.priorities.is_none(), ListItem::new("more-priority-all").child(Label::new(strings.summary_all_priorities)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.update(cx, |o| o.priorities = None);
                        if this.pending_options.is_some() {
                            window.focus(&this.focus_handle, cx);
                        }
                    }))
                    .into_any_element(),
            )
            .chain(priorities.into_iter().map(|(priority, name)| {
                let selected = options.priorities.as_ref().is_some_and(|p| p.contains(&priority));
                check(selected, ListItem::new(SharedString::from(format!("more-priority-{}", priority.to_i64()))).child(Label::new(name)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.update(cx, |o| {
                            let mut chosen = o.priorities.take().unwrap_or_default();
                            match chosen.iter().position(|p| *p == priority) {
                                Some(i) => {
                                    chosen.remove(i);
                                }
                                None => chosen.push(priority),
                            }
                            o.priorities = (!chosen.is_empty()).then_some(chosen);
                        });
                        if this.pending_options.is_some() {
                            window.focus(&this.focus_handle, cx);
                        }
                    }))
                    .into_any_element()
            }))
            .collect::<Vec<_>>();
            let tag_bounds = Rc::new(Cell::new(None::<gpui::Bounds<Pixels>>));
            let priority_bounds = Rc::new(Cell::new(None::<gpui::Bounds<Pixels>>));
            let tag_bounds_out = tag_bounds.clone();
            let priority_bounds_out = priority_bounds.clone();
            let side = |index: u8,
                        id: &'static str,
                        label: &'static str,
                        items: Vec<AnyElement>,
                        bounds: Rc<Cell<Option<gpui::Bounds<Pixels>>>>,
                        cx: &mut Context<Self>| {
                SubmenuItem::new(id, label)
                    .open(self.more_side == Some(index))
                    .track_bounds(bounds)
                    .items(items)
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.more_side != Some(index) {
                            this.more_side = Some(index);
                            cx.notify();
                        }
                    }))
            };
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .min_w(px(200.))
                    .on_mouse_down_out(cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        if tag_bounds_out.get().is_some_and(|b| b.contains(&event.position))
                            || priority_bounds_out.get().is_some_and(|b| b.contains(&event.position))
                        {
                            return;
                        }
                        this.more_side = None;
                        this.set_menu(None, cx);
                    }))
                    .child(side(0, "more-tag", strings.summary_tag, tag_items, tag_bounds, cx))
                    .child(side(1, "more-priority", strings.summary_priority, priority_items, priority_bounds, cx)),
            )
        });
        let hover_bg = cx.theme().colors().ghost_element_hover;
        h_flex()
            .h(px(40.))
            .px_4()
            .child(div().flex_1().child(Label::new(strings.summary_more)))
            .child(
                h_flex()
                    .id("summary-more")
                    .relative()
                    .px_1p5()
                    .py_0p5()
                    .gap_1()
                    .max_w(px(170.))
                    .rounded(px(5.))
                    .hover(move |s| s.bg(hover_bg))
                    .cursor_pointer()
                    .child(Label::new(value).color(Color::Muted).truncate())
                    .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_menu((!open).then_some(Menu::More), cx)))
                    .children(dropdown),
            )
    }

    pub(super) fn render_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let today = Local::now().date_naive();
        let options = &self.options;
        let group = || v_flex().py_1().rounded(px(8.)).bg(colors.surface_background);
        let heading = |text: &'static str| div().pt_3().pb_1p5().child(Label::new(text).weight(FontWeight::SEMIBOLD));

        let mut periods: Vec<(String, bool, Pick)> = Period::ALL
            .into_iter()
            .map(|period| {
                let pick: Pick = Box::new(move |this, _, cx| this.update(cx, |o| o.period = period));
                (period_label(period, today, strings), period == options.period, pick)
            })
            .collect();
        let custom = matches!(options.period, Period::Custom(..));
        periods.push((
            strings.summary_custom.to_owned(),
            custom,
            Box::new(|this, window, cx| this.open_custom(window, cx)),
        ));
        let lists = self
            .cache
            .as_ref()
            .map(|c| c.lists.as_slice())
            .unwrap_or(&[]);
        let list_label = match &options.lists {
            None if options.habits => format!("{}, {}", strings.summary_all_lists, strings.summary_habits),
            None => strings.summary_all_lists.to_owned(),
            Some(chosen) => {
                let mut names: Vec<String> = chosen
                    .iter()
                    .map(|id| match id {
                        None => strings.inbox.to_owned(),
                        Some(id) => lists.iter().find(|l| l.id == *id).map(|l| l.name.clone()).unwrap_or_default(),
                    })
                    .collect();
                if options.habits {
                    names.push(strings.summary_habits.to_owned());
                }
                names.join(", ")
            }
        };
        let mut list_options: Vec<(String, bool, Pick)> = vec![(
            strings.summary_all_lists.to_owned(),
            options.lists.is_none(),
            Box::new(|this, _, cx| this.update(cx, |o| o.lists = None)),
        )];
        list_options.push((
            strings.summary_habits.to_owned(),
            options.habits,
            Box::new(|this, _, cx| this.update(cx, |o| o.habits = !o.habits)),
        ));
        let choices = std::iter::once((None, strings.inbox.to_owned())).chain(lists.iter().map(|l| (Some(l.id), l.name.clone())));
        list_options.extend(choices.map(|(id, name)| {
            let selected = options.lists.as_ref().is_some_and(|chosen| chosen.contains(&id));
            let pick: Pick = Box::new(move |this, _, cx| {
                this.update(cx, |o| {
                    let mut chosen = o.lists.take().unwrap_or_default();
                    match chosen.iter().position(|c| *c == id) {
                        Some(i) => {
                            chosen.remove(i);
                        }
                        None => chosen.push(id),
                    }
                    o.lists = (!chosen.is_empty()).then_some(chosen);
                })
            });
            (name, selected, pick)
        }));
        let statuses = StatusFilter::ALL
            .into_iter()
            .zip(strings.summary_statuses)
            .map(|(status, name)| {
                let pick: Pick = Box::new(move |this, _, cx| this.update(cx, |o| o.status = status));
                (name.to_owned(), status == options.status, pick)
            })
            .collect();
        let groupings = Grouping::ALL
            .into_iter()
            .zip(strings.summary_groupings)
            .map(|(grouping, name)| {
                let pick: Pick = Box::new(move |this, _, cx| this.update(cx, |o| o.grouping = grouping));
                (name.to_owned(), grouping == options.grouping, pick)
            })
            .collect();
        let fields = Field::ALL
            .into_iter()
            .zip(strings.summary_fields)
            .map(|(field, name)| {
                let pick: Pick = Box::new(move |this, _, cx| {
                    this.update(cx, |o| match o.fields.iter().position(|f| *f == field) {
                        Some(i) => {
                            o.fields.remove(i);
                        }
                        None => o.fields.push(field),
                    })
                });
                (name.to_owned(), options.fields.contains(&field), pick)
            })
            .collect();
        let status_index = StatusFilter::ALL.iter().position(|s| *s == options.status).unwrap_or(0);
        let grouping_index = Grouping::ALL.iter().position(|g| *g == options.grouping).unwrap_or(0);

        let content = v_flex()
            .id("summary-options-sidebar-content")
            .px_4()
            .pb_4()
            .child(heading(strings.summary_template))
            .child(self.render_templates(cx))
            .child(heading(strings.summary_filter))
            .child(
                group()
                    .child(self.row(strings.summary_date, period_label(options.period, today, strings), Menu::Period, periods, false, cx))
                    .children(self.custom.clone().map(|(start, end)| {
                        let field = |input: Entity<TextInput>| {
                            h_flex().h(px(28.)).px_2().rounded(px(6.)).bg(colors.background).child(input)
                        };
                        v_flex()
                            .mx_3()
                            .mb_2()
                            .gap_1p5()
                            .child(field(start))
                            .child(field(end))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(
                                        Button::new("summary-custom-cancel", strings.cancel)
                                            .variant(ButtonVariant::Secondary)
                                            .size(ButtonSize::Sm)
                                            .on_click(cx.listener(|this, _, window, cx| this.cancel_custom(window, cx))),
                                    )
                                    .child(
                                        Button::new("summary-custom-confirm", strings.confirm)
                                            .variant(ButtonVariant::Primary)
                                            .size(ButtonSize::Sm)
                                            .on_click(cx.listener(|this, _, window, cx| this.confirm_custom(window, cx))),
                                    ),
                            )
                    }))
                    .child(self.row(strings.summary_lists, list_label, Menu::Lists, list_options, true, cx))
                    .child(self.row(strings.summary_status, strings.summary_statuses[status_index].to_owned(), Menu::Status, statuses, false, cx))
                    .child(self.render_more_row(cx)),
            )
            .child(heading(strings.summary_display_options))
            .child(
                group()
                    .child(self.row(strings.summary_grouping, strings.summary_groupings[grouping_index].to_owned(), Menu::Grouping, groupings, false, cx))
                    .child(self.row(
                        strings.summary_fields_label,
                        strings.summary_selected.replace("{n}", &options.fields.len().to_string()),
                        Menu::Fields,
                        fields,
                        true,
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .id("summary-next-period")
                    .pt_3()
                    .cursor_pointer()
                    .child(div().flex_1().child(Label::new(strings.summary_next_period).weight(FontWeight::SEMIBOLD)))
                    .child(Switch::new().toggle_state(options.next_period))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.update(cx, |o| o.next_period = !o.next_period);
                        if this.pending_options.is_some() {
                            window.focus(&this.focus_handle, cx);
                        }
                    })),
            );

        v_flex()
            .flex_none()
            .w(px(320.))
            .h_full()
            .pt(crate::title_bar::height())
            .border_l_1()
            .border_color(colors.border_variant)
            .child(v_scroll("summary-options-sidebar-scroll", content).flex_1().min_h_0())
    }
}
