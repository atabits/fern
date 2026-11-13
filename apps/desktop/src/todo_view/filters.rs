use super::*;
use crate::filter_draft::{InputError, Inputs};
use fern_core::{Condition, ConditionValue as Value, DateRule, Field, Join, SmartFilter};

pub(super) struct FilterInputs {
    keywords: Entity<TextInput>,
    days: Entity<TextInput>,
    from: Entity<TextInput>,
    to: Entity<TextInput>,
}
impl FilterInputs {
    fn text(&self, cx: &App) -> Inputs {
        Inputs {
            keywords: self.keywords.read(cx).text().to_owned(),
            days: self.days.read(cx).text().to_owned(),
            from: self.from.read(cx).text().to_owned(),
            to: self.to.read(cx).text().to_owned(),
        }
    }
}
pub(super) struct FilterEdit {
    pub(super) draft: SmartFilter,
    is_new: bool,
    name: Entity<TextInput>,
    pub(super) inputs: Vec<FilterInputs>,
    pub(super) chain_menu: Option<usize>,
    pub(super) preview: bool,
    pub(super) examples_open: bool,
    error: Option<&'static str>,
}
impl FilterEdit {
    fn snapshot(&self, cx: &App) -> Result<SmartFilter, InputError> {
        let mut filter = self.draft.clone();
        filter.name = self.name.read(cx).text().trim().to_owned();
        filter.rules.conditions = filter
            .rules
            .conditions
            .iter()
            .zip(&self.inputs)
            .map(|(c, inputs)| inputs.text(cx).apply(c))
            .collect::<Result<_, _>>()?;
        Ok(filter)
    }
}
fn field_name(field: Field, strings: &Strings) -> &'static str {
    match field {
        Field::Lists => strings.filter_lists,
        Field::Tags => strings.tags,
        Field::Date => strings.filter_date,
        Field::Priority => strings.filter_priority,
        Field::Include => strings.filter_include,
        Field::Type => strings.filter_type,
    }
}
fn input_error(error: InputError, strings: &Strings) -> &'static str {
    match error {
        InputError::Days => strings.filter_days_error,
        InputError::Range => strings.filter_range_error,
    }
}
fn input_field(input: Entity<TextInput>, cx: &App) -> impl IntoElement {
    h_flex()
        .h(px(30.))
        .min_w_0()
        .px_2()
        .rounded(px(6.))
        .bg(cx.theme().colors().background)
        .child(input)
}
fn chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .min_h(px(28.))
        .px_2()
        .py_1()
        .flex()
        .items_center()
        .rounded(px(6.))
        .cursor_pointer()
        .bg(if selected {
            cx.theme().colors().accent.opacity(0.25)
        } else {
            cx.theme().colors().element_hover
        })
        .hover(|s| s.bg(cx.theme().colors().element_hover))
        .child(Label::new(label).size(LabelSize::Small).color(if selected {
            Color::Accent
        } else {
            Color::Default
        }))
}
impl TodoView {
    fn filter_input(
        text: String,
        placeholder: &'static str,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, placeholder, cx);
            input.set_text(text, cx);
            input
        });
        cx.subscribe(&input, |this, _, _: &Changed, cx| {
            if let Some(edit) = this.filter_edit.as_mut() {
                edit.error = None;
            }
            cx.notify();
        })
        .detach();
        input
    }
    fn condition_inputs(value: &Value, cx: &mut Context<Self>) -> FilterInputs {
        let strings = i18n::t(cx);
        let input = Inputs::new(value);
        FilterInputs {
            keywords: Self::filter_input(input.keywords, strings.filter_keywords, cx),
            days: Self::filter_input(input.days, strings.filter_days, cx),
            from: Self::filter_input(input.from, "", cx),
            to: Self::filter_input(input.to, "", cx),
        }
    }
    pub(super) fn filter_inputs(filter: &SmartFilter, cx: &mut Context<Self>) -> Vec<FilterInputs> {
        filter
            .rules
            .conditions
            .iter()
            .map(|c| Self::condition_inputs(&c.value, cx))
            .collect()
    }
    pub(super) fn open_filter_edit(
        &mut self,
        existing: Option<SmartFilter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let strings = i18n::t(cx);
        let is_new = existing.is_none();
        let mut draft = existing.unwrap_or_else(SmartFilter::new);
        if !draft.rules.advanced {
            // Legacy normal rows may omit empty fields.
            for field in Field::ALL {
                if !draft
                    .rules
                    .conditions
                    .iter()
                    .any(|c| c.value.field() == field)
                {
                    draft.rules.conditions.push(Condition::empty(field));
                }
            }
            draft.rules.conditions.sort_by_key(|c| {
                Field::ALL
                    .iter()
                    .position(|f| *f == c.value.field())
                    .unwrap()
            });
        }
        let name = Self::filter_input(draft.name.clone(), strings.filter_name, cx);
        let inputs = Self::filter_inputs(&draft, cx);
        window.focus(&name.focus_handle(cx), cx);
        self.header_menu_open = false;
        self.filter_edit = Some(FilterEdit {
            draft,
            is_new,
            name,
            inputs,
            chain_menu: None,
            preview: false,
            examples_open: false,
            error: None,
        });
        cx.notify();
    }
    pub(super) fn set_filter_advanced(&mut self, advanced: bool, cx: &mut Context<Self>) {
        let Some(edit) = self.filter_edit.as_mut() else {
            return;
        };
        match edit.snapshot(cx) {
            Ok(mut draft) => {
                if draft.rules.set_advanced(advanced).is_ok() {
                    edit.inputs = Self::filter_inputs(&draft, cx);
                    edit.draft = draft;
                    edit.chain_menu = None;
                    edit.error = None;
                } else {
                    edit.error = Some(i18n::t(cx).filter_mode_error);
                }
            }
            Err(error) => edit.error = Some(input_error(error, i18n::t(cx))),
        }
        cx.notify();
    }
    fn update_condition(
        &mut self,
        i: usize,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut Condition),
    ) {
        if let Some(edit) = self.filter_edit.as_mut()
            && let Some(condition) = edit.draft.rules.conditions.get_mut(i)
        {
            update(condition);
            edit.error = None;
        }
        cx.notify();
    }
    fn set_condition_field(&mut self, i: usize, field: Field, cx: &mut Context<Self>) {
        if let Some(edit) = self.filter_edit.as_mut()
            && let Some(condition) = edit.draft.rules.conditions.get_mut(i)
        {
            if condition.value.field() != field {
                condition.value = Value::empty(field);
                edit.inputs[i] = Self::condition_inputs(&condition.value, cx);
            }
            edit.chain_menu = None;
            edit.error = None;
        }
        cx.notify();
    }
    fn set_filter_preview(&mut self, preview: bool, cx: &mut Context<Self>) {
        if let Some(edit) = self.filter_edit.as_mut() {
            if preview && let Err(error) = edit.snapshot(cx) {
                edit.error = Some(input_error(error, i18n::t(cx)));
                cx.notify();
                return;
            }
            edit.preview = preview;
            edit.error = None;
            edit.examples_open = false;
            edit.chain_menu = None;
        }
        cx.notify();
    }
    fn save_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.filter_edit.as_mut() else {
            return;
        };
        let filter = match edit.snapshot(cx) {
            Ok(filter) if !filter.name.is_empty() => filter,
            Ok(_) => {
                edit.preview = false;
                edit.error = Some(i18n::t(cx).filter_name_error);
                window.focus(&edit.name.focus_handle(cx), cx);
                cx.notify();
                return;
            }
            Err(error) => {
                edit.preview = false;
                edit.error = Some(input_error(error, i18n::t(cx)));
                cx.notify();
                return;
            }
        };
        let result = self.store.save_filter(&filter);
        if self.check(result).is_some() {
            self.filter_edit = None;
            self.view = View::Filter(filter.id);
            self.reload(cx);
        }
    }
    pub(super) fn delete_current_filter(&mut self, cx: &mut Context<Self>) {
        if let View::Filter(id) = self.view {
            let result = self.store.delete_filter(id);
            if self.check(result).is_some() {
                self.view = View::Inbox;
                self.reload(cx);
            }
        }
    }
    fn render_filter_preview(&self, edit: &FilterEdit, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Self::today();
        let draft = match edit.snapshot(cx) {
            Ok(draft) => draft,
            Err(error) => {
                return v_flex()
                    .child(Label::new(input_error(error, strings)).color(Color::Error))
                    .child(
                        chip("filter-preview-back", strings.cancel, false, cx).on_click(
                            cx.listener(|this, _, _, cx| this.set_filter_preview(false, cx)),
                        ),
                    )
                    .into_any_element();
            }
        };
        let tasks = self.store.tasks().unwrap_or_default();
        let shown = draft.preview(&tasks, today, self.preferences.week_start);
        let lists: Vec<(Option<Id>, SharedString)> =
            std::iter::once((None, SharedString::new_static(strings.inbox)))
                .chain(
                    self.lists
                        .iter()
                        .map(|l| (Some(l.id), l.name.clone().into())),
                )
                .collect();
        let mut body = v_flex()
            .id("filter-preview-tasks")
            .max_h(px(420.))
            .overflow_y_scroll()
            .gap_0p5();
        if shown.is_empty() {
            body = body.child(
                div()
                    .py_4()
                    .child(Label::new(strings.filter_preview_empty).color(Color::Muted)),
            );
        }
        for (list, name) in lists {
            let in_list: Vec<&&Task> = shown.iter().filter(|t| t.list_id == list).collect();
            if in_list.is_empty() {
                continue;
            }
            body = body.child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(name)
                            .size(LabelSize::Small)
                            .weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        Label::new(in_list.len().to_string())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            );
            for task in in_list {
                let due = task
                    .due_date
                    .map(|date| crate::task_rows::due_label(date, task.due_time, today, strings));
                body = body.child(
                    h_flex()
                        .h(px(34.))
                        .pl_4()
                        .gap_2()
                        .border_b_1()
                        .border_color(theme.colors().border_variant)
                        .child(
                            Icon::new(if task.kind == Kind::Note {
                                IconName::FileText
                            } else {
                                IconName::Square
                            })
                            .size(IconSize::Small)
                            .color(Color::Muted),
                        )
                        .child(
                            div().flex_1().min_w_0().child(
                                Label::new(task.title.clone())
                                    .size(LabelSize::Small)
                                    .truncate(),
                            ),
                        )
                        .children(task.tags.iter().map(|tag| {
                            Label::new(tag.clone())
                                .size(LabelSize::XSmall)
                                .color(Color::Muted)
                        }))
                        .children(due.map(|(label, overdue)| {
                            Label::new(label).size(LabelSize::XSmall).color(if overdue {
                                Color::Error
                            } else {
                                Color::Accent
                            })
                        })),
                );
            }
        }
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .debug_selector(|| id.to_owned())
                .px_4()
                .h(px(30.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| {
                    el.border_1().border_color(theme.colors().border)
                })
                .child(Label::new(label).size(LabelSize::Small))
        };
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.35))
            .child(
                v_flex()
                    .id("filter-preview-dialog")
                    .w(px(560.))
                    .p_4()
                    .gap_2()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.colors().border)
                    .bg(theme.colors().elevated_surface_background)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .justify_center()
                            .child(Label::new(strings.filter_preview).weight(FontWeight::SEMIBOLD)),
                    )
                    .child(body)
                    .child(
                        h_flex()
                            .gap_2()
                            .pt_2()
                            .justify_end()
                            .child(
                                button("filter-preview-cancel", strings.cancel, false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.set_filter_preview(false, cx)
                                    }),
                                ),
                            )
                            .child(
                                button("filter-preview-save", strings.save, true)
                                    .on_click(cx.listener(|this, _, window, cx| this.save_filter(window, cx))),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_filter_condition(
        &self,
        i: usize,
        edit: &FilterEdit,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let condition = &edit.draft.rules.conditions[i];
        let inputs = &edit.inputs[i];
        let id = |part: &str| format!("filter-{i}-{part}");
        let content = match &condition.value {
            Value::Lists(selected) => {
                let options: Vec<_> = std::iter::once((None, strings.inbox.to_owned()))
                    .chain(self.lists.iter().map(|l| (Some(l.id), l.name.clone())))
                    .collect();
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        chip(id("all"), strings.filter_all, selected.is_empty(), cx).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.update_condition(i, cx, |c| c.value = Value::Lists(vec![]))
                            }),
                        ),
                    )
                    .children(options.into_iter().map(|(list, name)| {
                        chip(
                            id(&format!("list-{list:?}")),
                            name,
                            selected.contains(&list),
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_condition(i, cx, |c| {
                                if let Value::Lists(values) = &mut c.value {
                                    if let Some(at) = values.iter().position(|v| *v == list) {
                                        values.remove(at);
                                    } else {
                                        values.push(list);
                                    }
                                }
                            })
                        }))
                    }))
                    .into_any_element()
            }
            Value::Tags { names, untagged } => h_flex()
                .flex_wrap()
                .gap_1()
                .child(
                    chip(
                        id("all"),
                        strings.filter_all,
                        names.is_empty() && !untagged,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_condition(i, cx, |c| c.value = Value::empty(Field::Tags))
                    })),
                )
                .child(
                    chip(id("no-tags"), strings.filter_no_tags, *untagged, cx).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.update_condition(i, cx, |c| {
                                if let Value::Tags { untagged, .. } = &mut c.value {
                                    *untagged = !*untagged;
                                }
                            })
                        }),
                    ),
                )
                .children(self.tags.iter().map(|tag| {
                    let name = tag.name.clone();
                    chip(
                        id(&format!("tag-{name}")),
                        format!("#{name}"),
                        names
                            .iter()
                            .any(|n| n.to_lowercase() == name.to_lowercase()),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_condition(i, cx, |c| {
                            if let Value::Tags { names, .. } = &mut c.value {
                                if let Some(at) = names
                                    .iter()
                                    .position(|n| n.to_lowercase() == name.to_lowercase())
                                {
                                    names.remove(at);
                                } else {
                                    names.push(name.clone());
                                }
                            }
                        })
                    }))
                }))
                .into_any_element(),
            Value::Date(dates) => {
                let days = dates.iter().any(|d| matches!(d, DateRule::NextDays(_)));
                let span = dates.iter().any(|d| matches!(d, DateRule::Span { .. }));
                v_flex()
                    .gap_2()
                    .child(h_flex().flex_wrap().gap_1().children(
                        DateRule::OPTIONS.into_iter().enumerate().map(|(n, rule)| {
                            let selected = if rule == DateRule::All {
                                dates.is_empty() || dates.contains(&rule)
                            } else {
                                dates.iter().any(|d| {
                                    std::mem::discriminant(d) == std::mem::discriminant(&rule)
                                })
                            };
                            chip(
                                id(&format!("date-{n}")),
                                strings.filter_date_label(rule),
                                selected,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.update_condition(i, cx, |c| {
                                        if let Value::Date(values) = &mut c.value {
                                            c.legacy_matching = false;
                                            if rule == DateRule::All {
                                                values.clear();
                                            } else if let Some(at) = values.iter().position(|d| {
                                                std::mem::discriminant(d)
                                                    == std::mem::discriminant(&rule)
                                            }) {
                                                values.remove(at);
                                            } else {
                                                values.retain(|d| {
                                                    *d != DateRule::All
                                                        && !(matches!(
                                                            rule,
                                                            DateRule::NextDays(_)
                                                                | DateRule::Span { .. }
                                                        ) && matches!(
                                                            d,
                                                            DateRule::NextDays(_)
                                                                | DateRule::Span { .. }
                                                        ))
                                                });
                                                values.push(rule);
                                            }
                                        }
                                    })
                                },
                            ))
                        }),
                    ))
                    .when(days, |el| {
                        el.child(
                            h_flex()
                                .flex_wrap()
                                .gap_2()
                                .child(Label::new(strings.filter_days).size(LabelSize::Small))
                                .child(div().w(px(82.)).child(input_field(inputs.days.clone(), cx)))
                                .child(
                                    Label::new(strings.filter_days_hint)
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        )
                    })
                    .when(span, |el| {
                        el.child(
                            h_flex()
                                .gap_2()
                                .child(Label::new(strings.filter_range_from).size(LabelSize::Small))
                                .child(div().w(px(72.)).child(input_field(inputs.from.clone(), cx)))
                                .child(Label::new(strings.filter_range_to).size(LabelSize::Small))
                                .child(div().w(px(72.)).child(input_field(inputs.to.clone(), cx))),
                        )
                        .child(
                            Label::new(strings.filter_range_hint)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .when_some(inputs.text(cx).apply(condition).err(), |el, error| {
                        el.child(
                            Label::new(input_error(error, strings))
                                .size(LabelSize::Small)
                                .color(Color::Error),
                        )
                    })
                    .into_any_element()
            }
            Value::Priority(selected) => h_flex()
                .flex_wrap()
                .gap_1()
                .child(
                    chip(id("all"), strings.filter_all, selected.is_empty(), cx).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.update_condition(i, cx, |c| c.value = Value::Priority(vec![]))
                        }),
                    ),
                )
                .children(
                    [
                        (Priority::High, strings.priority_high),
                        (Priority::Medium, strings.priority_medium),
                        (Priority::Low, strings.priority_low),
                        (Priority::None, strings.priority_none),
                    ]
                    .into_iter()
                    .map(|(priority, label)| {
                        chip(
                            id(&format!("priority-{}", priority.to_i64())),
                            label,
                            selected.contains(&priority),
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_condition(i, cx, |c| {
                                if let Value::Priority(values) = &mut c.value {
                                    if let Some(at) = values.iter().position(|p| *p == priority) {
                                        values.remove(at);
                                    } else {
                                        values.push(priority);
                                    }
                                }
                            })
                        }))
                    }),
                )
                .into_any_element(),
            Value::Include(_) => input_field(inputs.keywords.clone(), cx).into_any_element(),
            Value::Type(selected) => h_flex()
                .flex_wrap()
                .gap_1()
                .children(
                    [
                        (None, strings.filter_all),
                        (Some(Kind::Task), strings.filter_kinds[0]),
                        (Some(Kind::Note), strings.filter_kinds[1]),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(n, (kind, label))| {
                        chip(id(&format!("type-{n}")), label, *selected == kind, cx).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.update_condition(i, cx, |c| c.value = Value::Type(kind))
                            }),
                        )
                    }),
                )
                .into_any_element(),
        };
        if !edit.draft.rules.advanced {
            return h_flex()
                .items_start()
                .gap_3()
                .child(div().w(px(90.)).flex_shrink_0().pt_1().child(
                    Label::new(field_name(condition.value.field(), strings)).size(LabelSize::Small),
                ))
                .child(div().flex_1().min_w_0().child(content))
                .into_any_element();
        }
        let menu = (edit.chain_menu == Some(i)).then(|| {
            PopoverAttachment::new(
                Placement::BelowLeft,
                popover(cx)
                    .min_w(px(180.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if let Some(edit) = this.filter_edit.as_mut() {
                            edit.chain_menu = None;
                        }
                        cx.notify();
                    }))
                    .children(Field::ALL.into_iter().map(|field| {
                        ListItem::new(SharedString::from(id(field.key())))
                            .child(Label::new(field_name(field, strings)).size(LabelSize::Small))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_condition_field(i, field, cx)
                            }))
                    }))
                    .into_any_element(),
            )
        });
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .when(i > 0, |el| {
                        el.child(
                            chip(
                                id("join"),
                                strings.filter_logic[(condition.join == Join::Or) as usize],
                                true,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.update_condition(i, cx, |c| {
                                        c.join = if c.join == Join::And {
                                            Join::Or
                                        } else {
                                            Join::And
                                        }
                                    })
                                },
                            )),
                        )
                    })
                    .child(
                        div()
                            .relative()
                            .child(
                                chip(
                                    id("field"),
                                    field_name(condition.value.field(), strings),
                                    false,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if let Some(edit) = this.filter_edit.as_mut() {
                                            edit.chain_menu = if edit.chain_menu == Some(i) {
                                                None
                                            } else {
                                                Some(i)
                                            };
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                            .children(menu),
                    )
                    .child(
                        chip(id("not"), strings.filter_not, condition.not, cx).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.update_condition(i, cx, |c| c.not = !c.not)
                            }),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        chip(id("remove"), strings.delete, false, cx).on_click(cx.listener(
                            move |this, _, _, cx| {
                                if let Some(edit) = this.filter_edit.as_mut() {
                                    edit.draft.rules.conditions.remove(i);
                                    edit.inputs.remove(i);
                                    edit.chain_menu = None;
                                    edit.error = None;
                                }
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(content)
            .into_any_element()
    }

    pub(super) fn render_filter_edit(
        &self,
        edit: &FilterEdit,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if edit.preview {
            return self.render_filter_preview(edit, cx);
        }
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let advanced = edit.draft.rules.advanced;
        let mut body = v_flex()
            .id("filter-conditions")
            .min_h_0()
            .max_h((window.viewport_size().height - px(240.)).max(px(120.)))
            .overflow_y_scroll()
            .gap_4();
        for i in 0..edit.draft.rules.conditions.len() {
            body = body.child(self.render_filter_condition(i, edit, cx));
        }
        if advanced {
            body = body.child(
                chip("filter-add", strings.filter_add_condition, false, cx).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some(edit) = this.filter_edit.as_mut() {
                            let field = Field::ALL
                                .into_iter()
                                .find(|field| {
                                    !edit
                                        .draft
                                        .rules
                                        .conditions
                                        .iter()
                                        .any(|c| c.value.field() == *field)
                                })
                                .unwrap_or(Field::Tags);
                            let condition = Condition::empty(field);
                            edit.inputs
                                .push(Self::condition_inputs(&condition.value, cx));
                            edit.draft.rules.conditions.push(condition);
                            edit.chain_menu = Some(edit.draft.rules.conditions.len() - 1);
                        }
                        cx.notify();
                    },
                )),
            );
        }
        let examples = (advanced && edit.examples_open).then(|| {
            PopoverAttachment::new(
                Placement::AboveRight,
                popover(cx)
                    .min_w(px(260.))
                    .children(fern_core::filter::examples().into_iter().enumerate().map(
                        |(i, example)| {
                            ListItem::new(("filter-example", i))
                                .child(
                                    Label::new(strings.filter_example_names[i])
                                        .size(LabelSize::Small),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(edit) = this.filter_edit.as_mut() {
                                        edit.draft.rules = example.rules.clone();
                                        edit.inputs = Self::filter_inputs(&edit.draft, cx);
                                        edit.examples_open = false;
                                        edit.chain_menu = None;
                                        edit.error = None;
                                    }
                                    cx.notify();
                                }))
                        },
                    ))
                    .into_any_element(),
            )
        });
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.35))
            .child(
                v_flex()
                    .id("filter-dialog")
                    .w(px(600.).min(window.viewport_size().width - px(32.)))
                    .p_4()
                    .gap_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.colors().border)
                    .bg(theme.colors().elevated_surface_background)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Label::new(if edit.is_new {
                            strings.add_filter
                        } else {
                            strings.edit_filter
                        })
                        .weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        h_flex()
                            .justify_center()
                            .gap_2()
                            .children([false, true].map(|mode| {
                                chip(
                                    format!("filter-mode-{mode}"),
                                    strings.filter_modes[mode as usize],
                                    advanced == mode,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.set_filter_advanced(mode, cx),
                                ))
                            })),
                    )
                    .child(input_field(edit.name.clone(), cx))
                    .child(body)
                    .when_some(edit.error, |el, error| {
                        el.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                chip("filter-preview", strings.filter_preview, false, cx).on_click(
                                    cx.listener(|this, _, _, cx| this.set_filter_preview(true, cx)),
                                ),
                            )
                            .when(advanced, |el| {
                                el.child(
                                    div()
                                        .relative()
                                        .child(
                                            chip(
                                                "filter-examples",
                                                strings.filter_examples,
                                                false,
                                                cx,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    if let Some(edit) = this.filter_edit.as_mut() {
                                                        edit.examples_open = !edit.examples_open;
                                                    }
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                        .children(examples),
                                )
                            })
                            .child(div().flex_1())
                            .child(chip("filter-close", strings.close, false, cx).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.filter_edit = None;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                chip("filter-save", strings.save, true, cx)
                                    .on_click(cx.listener(|this, _, window, cx| this.save_filter(window, cx))),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, point};

    #[gpui::test]
    fn filter_preview_blocks_background_clicks_and_shows_missing_name(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Behind the dialog").unwrap();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
        let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
        let selector = format!("task-title-{}", task.id).leak();
        let task_bounds = cx.debug_bounds(selector).expect("task row is rendered");
        let position = point(task_bounds.left() + px(8.), task_bounds.center().y);
        cx.update(|window, cx| view.update(cx, |todo, cx| {
            todo.open_filter_edit(None, window, cx);
            todo.set_filter_preview(true, cx);
        }));
        cx.run_until_parked();

        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(position, None, modifiers);
        cx.simulate_click(position, modifiers);
        cx.update(|_, cx| assert_eq!(view.read(cx).selected, None));

        let save = cx.debug_bounds("filter-preview-save").expect("preview save button is rendered");
        cx.simulate_mouse_move(save.center(), None, modifiers);
        cx.simulate_click(save.center(), modifiers);
        cx.update(|_, cx| {
            let edit = view.read(cx).filter_edit.as_ref().expect("filter form remains open");
            assert!(!edit.preview);
            assert_eq!(edit.error, Some(i18n::EN.filter_name_error));
        });
        assert!(store.filters().unwrap().is_empty());
    }
}
