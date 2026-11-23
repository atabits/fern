use super::*;

/// An open filter dropdown of the Completed and Won't Do views. The lists filter is edited as a
/// draft until "OK", as in the original.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ClosedMenu {
    Dates,
    /// "Other Date": the range being picked and the calendar shown for one of its ends.
    OtherDate { start: Option<NaiveDate>, end: Option<NaiveDate>, picking: Option<RangeEnd>, month: NaiveDate },
    Lists(Option<Vec<Option<Id>>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SortMenu {
    Group,
    Sort,
    Order,
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RangeEnd {
    Start,
    End,
}


/// What a list menu acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ListMenuTarget {
    List(Id),
    Folder(Id),
    Tag(String),
    /// The Tags heading's "…": whether tags show.
    TagsHeading,
    /// A smart list's "…": Hide.
    Smart(fern_core::SmartList),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ListAction {
    Edit,
    Duplicate,
    Archive,
    Delete,
    AddList,
    Ungroup,
    Pin,
    AddSubtag,
    Merge,
    ShowTags(fern_core::Visibility),
}

/// An entry of the task list: a row, or what the list shows around the rows.
#[allow(clippy::large_enum_variant)]
impl TodoView {
    pub(super) fn open_list_activity(&mut self, cx: &mut Context<Self>) {
        let list = match self.view {
            View::List(id) => Some(id),
            _ => None,
        };
        self.list_activity = Some(self.store.list_activity(list, 300).unwrap_or_default());
        self.header_menu_open = false;
        cx.notify();
    }

    /// The original's List Activities: the list's task events by day, newest first, each with
    /// its task, which a click opens.
    pub(super) fn render_list_activity(&self, entries: &[(fern_core::Activity, Id, String)], top_inset: bool, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Self::today();
        let mut items: Vec<AnyElement> = Vec::new();
        let mut current_day = None;
        for (activity, task, title) in entries {
            let Some(when) = Local.timestamp_millis_opt(activity.at).single() else { continue };
            let day = when.date_naive();
            if current_day != Some(day) {
                current_day = Some(day);
                items.push(
                    div()
                        .pt_3()
                        .pb_1()
                        .child(Label::new(task_rows::day_header(day, today, strings)).size(LabelSize::XSmall).color(Color::Muted))
                        .into_any_element(),
                );
            }
            let (icon, color) = match activity.kind.as_str() {
                "created" => (IconName::Plus, theme.colors().accent),
                "completed" => (IconName::Check, theme.status().success),
                "deleted" => (IconName::Trash, theme.status().error),
                _ => (IconName::Pencil, theme.colors().text_muted),
            };
            let task = *task;
            items.push(
                h_flex()
                    .items_start()
                    .gap_2()
                    .py_1()
                    .child(
                        div()
                            .mt(px(2.))
                            .size(px(18.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(color)
                            .child(Icon::new(icon).size(IconSize::XSmall).color(Color::Custom(theme::foreground_on(&[color], 3.)))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(Label::new(crate::task_detail::activity_text(&activity.kind, strings).to_owned()).size(LabelSize::Small).color(Color::Muted))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("activity-{task}-{}", activity.at)))
                                            .min_w_0()
                                            .cursor_pointer()
                                            .child(Label::new(title.clone()).size(LabelSize::Small).color(Color::Accent).truncate())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.list_activity = None;
                                                this.select(task, cx);
                                            })),
                                    ),
                            )
                            .child(Label::new(format!("{} {}", crate::calendar::format_day_month(when.date_naive(), strings), crate::calendar::format_time(when, strings))).size(LabelSize::XSmall).color(Color::Muted)),
                    )
                    .into_any_element(),
            );
        }
        v_flex()
            .size_full()
            .bg(theme.colors().background)
            .border_l_1()
            .border_color(theme.colors().border)
            .child(
                h_flex()
                    .when(top_inset, |el| el.mt(crate::title_bar::header_inset()))
                    .h(px(48.))
                    .px_5()
                    .border_b_1()
                    .border_color(theme.colors().border)
                    .child(div().flex_1().child(Label::new(strings.list_activities).size(LabelSize::Large)))
                    .child(
                        IconButton::new("close-list-activity", IconName::X)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.list_activity = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_scroll(
                    "list-activity-scroll",
                    v_flex()
                        .id("list-activity")
                        .px_5()
                        .pb_4()
                        .when(entries.is_empty(), |el| el.child(div().pt_4().child(Label::new(strings.stats_no_data).color(Color::Muted))))
                        .children(items),
                )
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }


    pub(super) fn refresh_suggestions(&mut self) {
        let tasks = self.store.tasks().unwrap_or_default();
        self.cached_suggestions = fern_core::suggest::suggestions(&tasks, Self::today());
    }

    /// The original's Suggested Tasks: Recently Added and Long Unfinished, each with "+" to plan
    /// the task for today; four of the long unfinished ones until "More".
    pub(super) fn render_suggestions(&self, all: bool, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let today = Self::today();
        let found = &self.cached_suggestions;
        let row = |task: &Task, note: String, cx: &mut Context<Self>| {
            let id = task.id;
            h_flex()
                .id(SharedString::from(format!("suggestion-{id}")))
                .py_1p5()
                .gap_2()
                .items_start()
                .border_b_1()
                .border_color(theme.colors().border_variant)
                .child(div().pt(px(3.)).child(Checkbox::new(SharedString::from(format!("suggestion-box-{id}"))).size(px(14.))))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(Label::new(task.title.clone()).truncate())
                        .child(Label::new(note).size(LabelSize::XSmall).color(Color::Muted)),
                )
                .child(
                    IconButton::new(SharedString::from(format!("suggestion-add-{id}")), IconName::Plus)
                        .icon_color(Color::Accent)
                        .tooltip(Tooltip::text(strings.add_to_today))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let result = this.store.move_dates(id, Self::today());
                            this.check(result);
                            desktop_common::data_version::data_changed(cx.entity_id(), cx);
                            this.reload(cx);
                        })),
                )
                .into_any_element()
        };
        let recent: Vec<AnyElement> = found
            .recent
            .iter()
            .map(|task| {
                let when = fern_core::suggest::created_day(task)
                    .zip(chrono::DateTime::from_timestamp_millis(task.created_at))
                    .map(|(day, at)| task_rows::due_label(day, Some(at.with_timezone(&Local).time()), today, strings).0)
                    .unwrap_or_default();
                row(task, strings.added_on.replace("{when}", &when), cx)
            })
            .collect();
        let shown = if all { found.long_unfinished.len() } else { found.long_unfinished.len().min(4) };
        let long: Vec<AnyElement> = found.long_unfinished[..shown]
            .iter()
            .map(|task| {
                let days = fern_core::suggest::created_day(task).map_or(0, |d| (today - d).num_days());
                row(task, strings.created_days_ago.replace("{n}", &days.to_string()), cx)
            })
            .collect();
        let more = !all && found.long_unfinished.len() > 4;
        let heading = |text: &'static str| Label::new(text).size(LabelSize::Small).weight(FontWeight::SEMIBOLD);
        div()
            .absolute()
            .right(px(12.))
            .bottom(px(12.))
            .child(
                v_scroll(
                    "suggested-tasks-scroll",
                    v_flex()
                        .id("suggested-tasks")
                        .p_4()
                        .gap_2()
                        .child(
                            h_flex()
                                .child(div().flex_1().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(strings.suggested_tasks))
                                .child(
                                    IconButton::new("suggestions-x", IconName::X)
                                        .icon_color(Color::Muted)
                                        .tooltip(Tooltip::text(strings.close))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.suggestions_open = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .when(recent.is_empty() && long.is_empty(), |el| el.child(Label::new(strings.no_suggestions).color(Color::Muted)))
                        .when(!recent.is_empty(), |el| el.child(heading(strings.recently_added)).children(recent))
                        .when(!long.is_empty(), |el| {
                            el.child(
                                h_flex()
                                    .pt_2()
                                    .child(div().flex_1().child(heading(strings.long_unfinished)))
                                    .when(more, |el| {
                                        el.child(
                                            h_flex()
                                                .id("suggestions-more")
                                                .cursor_pointer()
                                                .child(Label::new(strings.more).size(LabelSize::Small).color(Color::Muted))
                                                .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Muted))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.suggestions_open = Some(true);
                                                    cx.notify();
                                                })),
                                        )
                                    }),
                            )
                            .children(long)
                        }),
                )
                .w(px(370.))
                .max_h(px(560.))
                .rounded(px(10.))
                .bg(theme.colors().elevated_surface_background)
                .border_1()
                .border_color(theme.colors().border)
                .shadow_lg()
                .overflow_hidden(),
            )
            .into_any_element()
    }


    /// The original's View Options: Show Date by (Task Time or Countdown) and the Task Detail
    /// Layout for a narrow window (Side peek or Pop-up), each drawn as the original's sketches.
    pub(super) fn render_view_options(&self, menu_open: bool, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let countdown = self.preferences.countdown_dates;
        let popup = self.preferences.popup_details;
        let choice = |id: &'static str, label: &'static str, on: bool| {
            ListItem::new(id)
                .height(px(34.))
                .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
                .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
        };
        let menu = menu_open.then(|| {
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .min_w(px(196.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.view_options = Some(false);
                        cx.notify();
                    }))
                    .child(choice("show-date-time", strings.task_time, !countdown).on_click(cx.listener(|this, _, _, cx| {
                        this.view_options = Some(false);
                        this.save_preferences(|p| p.countdown_dates = false, cx);
                    })))
                    .child(choice("show-date-countdown", strings.date_countdown, countdown).on_click(cx.listener(|this, _, _, cx| {
                        this.view_options = Some(false);
                        this.save_preferences(|p| p.countdown_dates = true, cx);
                    }))),
            )
        });
        // A small sketch of the window: the details beside the list, or over it.
        let sketch = |over: bool| {
            let line = |w: f32| div().w(px(w)).h(px(3.)).rounded(px(1.)).bg(theme.colors().text_muted.opacity(0.3));
            div()
                .relative()
                .w(px(140.))
                .h(px(92.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .bg(theme.colors().background)
                .child(div().absolute().left(px(6.)).top(px(8.)).child(v_flex().gap_1().children([40., 30., 36., 28.].map(line))))
                .child(if over {
                    div()
                        .absolute()
                        .left(px(40.))
                        .top(px(18.))
                        .w(px(80.))
                        .h(px(58.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.colors().border)
                        .bg(theme.colors().elevated_surface_background)
                        .p_1p5()
                        .child(v_flex().gap_1().children([50., 60., 40.].map(line)))
                } else {
                    div()
                        .absolute()
                        .right_0()
                        .top_0()
                        .w(px(64.))
                        .h_full()
                        .border_l_1()
                        .border_color(theme.colors().border)
                        .p_1p5()
                        .child(v_flex().gap_1().children([40., 50., 30.].map(line)))
                })
        };
        let layout = |id: &'static str, label: &'static str, over: bool, cx: &mut Context<Self>| {
            let on = popup == over;
            v_flex()
                .id(id)
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(sketch(over))
                .child(
                    h_flex()
                        .gap_1p5()
                        .child(
                            div()
                                .size(px(16.))
                                .rounded_full()
                                .border_1()
                                .border_color(if on { theme.colors().accent } else { theme.colors().border })
                                .when(on, |el| el.bg(theme.colors().accent).flex().items_center().justify_center().child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::IconOnAccent))),
                        )
                        .child(Label::new(label)),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.save_preferences(|p| p.popup_details = over, cx)))
        };
        let section = || v_flex().p_4().gap_1().rounded(px(8.)).bg(theme.colors().surface_background);
        Modal::new("view-options")
            .width(px(440.))
            .padding(px(20.))
            .gap(px(12.))
            .on_dismiss(cx.listener(|this, _, _, cx| {
                this.view_options = None;
                cx.notify();
            }))
            .child(
                h_flex()
                    .child(div().flex_1().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(strings.view_options))
                    .child(
                        IconButton::new("view-options-x", IconName::X)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.view_options = None;
                                cx.notify();
                            })),
                    ),
            )
                    .child(
                        h_flex()
                            .h(px(48.))
                            .px_4()
                            .rounded(px(8.))
                            .bg(theme.colors().surface_background)
                            .child(div().flex_1().child(Label::new(strings.show_date_by)))
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        h_flex()
                                            .id("show-date-by")
                                            .gap_1()
                                            .cursor_pointer()
                                            .child(Label::new(if countdown { strings.date_countdown } else { strings.task_time }).color(Color::Muted))
                                            .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.view_options = Some(!this.view_options.unwrap_or(false));
                                                cx.notify();
                                            })),
                                    )
                                    .children(menu),
                            ),
                    )
                    .when(self.kanban, |el| {
                        let size = self.kanban_column_size;
                        let col_color = self.kanban_column_color;
                        let quick_add = self.kanban_quick_add;
                        let size_btn = |id: &'static str, label: &'static str, value: u8, cx: &mut Context<Self>| {
                            let active = size == value;
                            div()
                                .id(id)
                                .px_2p5()
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .when(active, |el| el.bg(theme.colors().element_selected))
                                .hover(|s| s.bg(theme.colors().ghost_element_hover))
                                .child(Label::new(label).size(LabelSize::Small).color(if active { Color::Accent } else { Color::Default }))
                                .on_click(cx.listener(move |this, _, _, cx| this.set_kanban_column_size(value, cx)))
                        };
                        el.child(
                            v_flex()
                                .rounded(px(8.))
                                .bg(theme.colors().surface_background)
                                .child(
                                    h_flex()
                                        .h(px(48.))
                                        .px_4()
                                        .child(div().flex_1().child(Label::new(strings.kanban_size)))
                                        .child(
                                            h_flex()
                                                .p(px(2.))
                                                .gap_0p5()
                                                .rounded(px(8.))
                                                .bg(theme.colors().background)
                                                .child(size_btn("kanban-size-s", strings.kanban_size_small, 0, cx))
                                                .child(size_btn("kanban-size-m", strings.kanban_size_medium, 1, cx))
                                                .child(size_btn("kanban-size-l", strings.kanban_size_large, 2, cx)),
                                        ),
                                )
                                .child(div().px_4().child(Divider::horizontal()))
                                .child(
                                    h_flex()
                                        .id("kanban-toggle-color")
                                        .h(px(48.))
                                        .px_4()
                                        .cursor_pointer()
                                        .child(div().flex_1().child(Label::new(strings.kanban_column_color)))
                                        .child(ui::Switch::new().toggle_state(col_color))
                                        .on_click(cx.listener(|this, _, _, cx| this.toggle_kanban_column_color(cx))),
                                )
                                .child(div().px_4().child(Divider::horizontal()))
                                .child(
                                    h_flex()
                                        .id("kanban-toggle-quick-add")
                                        .h(px(48.))
                                        .px_4()
                                        .cursor_pointer()
                                        .child(div().flex_1().child(Label::new(strings.kanban_show_input_box)))
                                        .child(ui::Switch::new().toggle_state(quick_add))
                                        .on_click(cx.listener(|this, _, _, cx| this.toggle_kanban_quick_add(cx))),
                                )
                                .child(div().px_4().child(Divider::horizontal()))
                                .child(
                                    h_flex()
                                        .id("kanban-toggle-workflow")
                                        .h(px(48.))
                                        .px_4()
                                        .cursor_pointer()
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .child(Label::new(strings.kanban_workflow))
                                                .child(Label::new(strings.kanban_workflow_hint).size(LabelSize::XSmall).color(Color::Muted)),
                                        )
                                        .child(ui::Switch::new().toggle_state(self.kanban_workflow))
                                        .on_click(cx.listener(|this, _, _, cx| this.toggle_kanban_workflow(cx))),
                                ),
                        )
                    })
                    .when(self.timeline && !self.kanban, |el| {
                        let show_weekends = self.timeline_show_weekends;
                        let color_by = self.timeline_color_by;
                        let color_btn = |id: &'static str, label: &'static str, choice: ColorBy, cx: &mut Context<Self>| {
                            let active = color_by == choice;
                            div()
                                .id(id)
                                .px_2()
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .when(active, |el| el.bg(theme.colors().element_selected))
                                .hover(|s| s.bg(theme.colors().ghost_element_hover))
                                .child(Label::new(label).size(LabelSize::Small).color(if active { Color::Accent } else { Color::Default }))
                                .on_click(cx.listener(move |this, _, _, cx| this.set_timeline_color_by(choice, cx)))
                        };
                        el.child(
                            v_flex()
                                .rounded(px(8.))
                                .bg(theme.colors().surface_background)
                                .child(
                                    h_flex()
                                        .id("timeline-toggle-weekends")
                                        .h(px(48.))
                                        .px_4()
                                        .cursor_pointer()
                                        .child(div().flex_1().child(Label::new(strings.timeline_show_weekends)))
                                        .child(ui::Switch::new().toggle_state(show_weekends))
                                        .on_click(cx.listener(|this, _, _, cx| this.toggle_timeline_weekends(cx))),
                                )
                                .child(div().px_4().child(Divider::horizontal()))
                                .child(
                                    h_flex()
                                        .h(px(48.))
                                        .px_4()
                                        .child(div().flex_1().child(Label::new(strings.color_by)))
                                        .child(
                                            h_flex()
                                                .p(px(2.))
                                                .gap_0p5()
                                                .rounded(px(8.))
                                                .bg(theme.colors().background)
                                                .child(color_btn("timeline-color-list", strings.color_by_options[0], ColorBy::List, cx))
                                                .child(color_btn("timeline-color-tag", strings.color_by_options[1], ColorBy::Tag, cx))
                                                .child(color_btn("timeline-color-prio", strings.color_by_options[2], ColorBy::Priority, cx)),
                                        ),
                                ),
                        )
                    })
                    .child(
                        section()
                            .child(Label::new(strings.task_detail_layout))
                            .child(Label::new(strings.task_detail_layout_hint).size(LabelSize::Small).color(Color::Muted))
                            .child(
                                h_flex()
                                    .pt_3()
                                    .justify_around()
                                    .child(layout("layout-side", strings.side_peek, false, cx))
                                    .child(layout("layout-popup", strings.pop_up, true, cx)),
                            ),
                    )
            .into_any_element()
    }


    /// The original's sort menu: "Group by" and "Sort by", each opening its options in place.
    pub(super) fn render_sort_menu(&self, expanded: Option<SortMenu>, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let (sort, group) = self.sort_settings();
        let descending = self.view_flag("descending", false);
        let row = |id: &'static str, icon: IconName, label: &'static str, value: &'static str, which: SortMenu| {
            ListItem::new(id)
                .start_slot(Icon::new(icon).color(Color::Muted))
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(div().flex_1().child(Label::new(label)))
                        .child(Label::new(value).color(Color::Muted))
                        .child(
                            Icon::new(if expanded == Some(which) { IconName::ChevronDown } else { IconName::ChevronRight })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sort_menu = Some(if expanded == Some(which) { None } else { Some(which) });
                    cx.notify();
                }))
        };
        let option = |id: String, label: &'static str, selected: bool| {
            ListItem::new(SharedString::from(id))
                .child(div().pl_6().child(Label::new(label).color(if selected { Color::Accent } else { Color::Default })))
                .when(selected, |item| item.end_slot(Label::new("✓").color(Color::Accent)))
        };
        popover(cx)
            .min_w(px(250.))
            .track_focus(&self.sort_menu_focus)
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_sort_menu(window, cx)))
            .child(row("group-by", IconName::Rows3, strings.group_by, group.label(strings), SortMenu::Group))
            .when(expanded == Some(SortMenu::Group), |el| {
                let kanban_board = self.kanban && !self.timeline;
                el.children(
                    GroupBy::ALL
                        .into_iter()
                        .filter(|g| !(kanban_board && *g == GroupBy::None))
                        .map(|g| {
                            option(format!("group-{}", g.key()), g.label(strings), g == group)
                                .on_click(cx.listener(move |this, _, window, cx| this.set_sort(None, Some(g), window, cx)))
                        }),
                )
            })
            .child(row("sort-by", IconName::ArrowDownUp, strings.sort_by, sort.label(strings), SortMenu::Sort))
            .when(expanded == Some(SortMenu::Sort), |el| {
                el.children(SortBy::ALL.into_iter().map(|s| {
                    option(format!("sort-{}", s.key()), s.label(strings), s == sort)
                        .on_click(cx.listener(move |this, _, window, cx| this.set_sort(Some(s), None, window, cx)))
                }))
            })
            // The original's Order: Ascending or Descending, kept per list; with the manual
            // (Custom) sort, as there, no Order.
            .when(sort != SortBy::Custom, |el| el.child(row("order-by", IconName::ArrowDownUp, strings.order, strings.order_directions[descending as usize], SortMenu::Order)))
            .when(sort != SortBy::Custom && expanded == Some(SortMenu::Order), |el| {
                el.children([false, true].map(|value| {
                    option(format!("order-{value}"), strings.order_directions[value as usize], value == descending).on_click(cx.listener(move |this, _, window, cx| {
                        if this.view_flag("descending", false) != value {
                            this.toggle_view_flag("descending", false, cx);
                        }
                        this.close_sort_menu(window, cx);
                    }))
                }))
            })
            .into_any_element()
    }


    pub(super) fn list_action(&mut self, action: ListAction, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.list_menu.take().map(|(t, _)| t);
        let current = match self.view {
            View::List(id) => Some(id),
            _ => None,
        };
        let list = match target {
            Some(ListMenuTarget::List(id)) => Some(id),
            _ => current,
        };
        match (action, target) {
            (ListAction::Edit, Some(ListMenuTarget::Tag(tag))) => return self.open_tag_dialog(Some(tag), None, window, cx),
            (ListAction::AddSubtag, Some(ListMenuTarget::Tag(tag))) => return self.open_tag_dialog(None, Some(tag), window, cx),
            (ListAction::Merge, Some(ListMenuTarget::Tag(tag))) => return self.open_merge_dialog(tag, window, cx),
            (ListAction::Pin, Some(ListMenuTarget::Tag(tag))) => {
                let pinned = self.tags.iter().any(|t| t.name == tag && t.pinned);
                let result = self.store.set_tag_pinned(&tag, !pinned);
                self.check(result);
            }
            (ListAction::Delete, Some(ListMenuTarget::Tag(tag))) => {
                self.confirm_delete = Some(ConfirmDeleteTarget::Tag(tag));
                self.header_menu_open = false;
                cx.notify();
                return;
            }
            (ListAction::ShowTags(visibility), _) => {
                self.save_preferences(|p| p.set_visibility(fern_core::SmartList::Tags, visibility), cx);
            }
            (ListAction::Edit, Some(ListMenuTarget::Folder(folder))) => return self.open_folder_dialog(Some(folder), window, cx),
            (ListAction::AddList, Some(ListMenuTarget::Folder(folder))) => return self.open_list_dialog(None, Some(folder), window, cx),
            (ListAction::Ungroup, Some(ListMenuTarget::Folder(folder))) => {
                let result = self.store.ungroup_list_folder(folder);
                self.check(result);
            }
            (ListAction::Edit, _) => return self.open_list_dialog(list, None, window, cx),
            (ListAction::Duplicate, _) => {
                if let Some(id) = list {
                    let result = self.store.duplicate_list(id, i18n::t(cx).list_copy_suffix);
                    if let Some(copy) = self.check(result) {
                        self.header_menu_open = false;
                        crate::data_version::data_changed(cx.entity_id(), cx);
                        self.set_view(View::List(copy.id), cx);
                        return;
                    }
                }
            }
            (ListAction::Archive, _) => {
                if let Some(list) = list.and_then(|id| self.lists.iter().find(|l| l.id == id)) {
                    let (id, archived) = (list.id, !list.archived);
                    let result = self.store.set_list_archived(id, archived);
                    self.check(result);
                    self.archived_open |= archived;
                }
            }
            (ListAction::Delete, _) => {
                if let Some(id) = list {
                    let name = self
                        .lists
                        .iter()
                        .find(|l| l.id == id)
                        .map(|l| l.name.clone())
                        .unwrap_or_default();
                    self.confirm_delete = Some(ConfirmDeleteTarget::List(id, name));
                    self.header_menu_open = false;
                    cx.notify();
                    return;
                }
            }
            _ => {}
        }
        self.header_menu_open = false;
        self.reload(cx);
    }

    /// The menu of a list (Edit, Duplicate, Archive or Unarchive, Delete) or of a folder (Edit,
    /// Add List, Ungroup), from the list header's "…" or a right click in the lists column.
    pub(super) fn render_list_items(&self, target: ListMenuTarget, strings: &'static Strings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let item = |id: &'static str, icon: IconName, label: &'static str, action: ListAction, cx: &mut Context<Self>| {
            ListItem::new(id)
                .height(px(34.))
                .start_slot(Icon::new(icon).color(if action == ListAction::Delete { Color::Error } else { Color::Icon }))
                .child(Label::new(label).color(if action == ListAction::Delete { Color::Error } else { Color::Default }))
                .on_click(cx.listener(move |this, _, window, cx| this.list_action(action, window, cx)))
                .into_any_element()
        };
        match target {
            ListMenuTarget::List(id) => {
                let archived = self.lists.iter().any(|l| l.id == id && l.archived);
                vec![
                    item("list-menu-edit", IconName::Pencil, strings.edit, ListAction::Edit, cx),
                    item("list-menu-duplicate", IconName::Copy, strings.duplicate, ListAction::Duplicate, cx),
                    item(
                        "list-menu-archive",
                        if archived { IconName::ArchiveRestore } else { IconName::Archive },
                        if archived { strings.unarchive_list } else { strings.archive_list },
                        ListAction::Archive,
                        cx,
                    ),
                    div().my_1().child(Divider::horizontal()).into_any_element(),
                    ListItem::new("list-menu-delete")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(Label::new(strings.delete).color(Color::Error))
                        .on_click(cx.listener(|this, _, window, cx| this.list_action(ListAction::Delete, window, cx)))
                        .into_any_element(),
                ]
            }
            ListMenuTarget::Tag(tag) => {
                // As in the original: words only. "Move to Shared Tags" needs shared lists.
                let tag = self.tags.iter().find(|t| t.name == tag);
                let pinned = tag.is_some_and(|t| t.pinned);
                let top = tag.is_some_and(|t| t.parent.is_none());
                let text = |id: &'static str, label: &'static str, action: ListAction, cx: &mut Context<Self>| {
                    ListItem::new(id)
                        .height(px(34.))
                        .child(Label::new(label))
                        .on_click(cx.listener(move |this, _, window, cx| this.list_action(action, window, cx)))
                        .into_any_element()
                };
                let mut items = vec![
                    text("tag-menu-edit", strings.edit, ListAction::Edit, cx),
                    text("tag-menu-pin", if pinned { strings.unpin } else { strings.pin }, ListAction::Pin, cx),
                ];
                if top {
                    items.push(text("tag-menu-subtag", strings.add_subtag, ListAction::AddSubtag, cx));
                }
                items.push(text("tag-menu-merge", strings.merge_tags, ListAction::Merge, cx));
                items.push(div().my_1().child(Divider::horizontal()).into_any_element());
                items.push(
                    ListItem::new("tag-menu-delete")
                        .height(px(34.))
                        .child(Label::new(strings.delete).color(Color::Error))
                        .on_click(cx.listener(|this, _, window, cx| this.list_action(ListAction::Delete, window, cx)))
                        .into_any_element(),
                );
                items
            }
            ListMenuTarget::TagsHeading => {
                use fern_core::Visibility;
                let current = self.preferences.visibility(fern_core::SmartList::Tags);
                [(Visibility::IfNotEmpty, strings.visibility_names[1]), (Visibility::Show, strings.visibility_names[0]), (Visibility::Hide, strings.visibility_names[2])]
                    .into_iter()
                    .map(|(visibility, label)| {
                        let on = current == visibility;
                        ListItem::new(SharedString::from(format!("tags-show-{label}")))
                            .height(px(34.))
                            .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
                            .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                            .on_click(cx.listener(move |this, _, window, cx| this.list_action(ListAction::ShowTags(visibility), window, cx)))
                            .into_any_element()
                    })
                    .collect()
            }
            ListMenuTarget::Smart(list) => vec![
                ListItem::new("smart-hide")
                    .height(px(34.))
                    .child(Label::new(strings.visibility_names[2]))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.list_menu = None;
                        this.save_preferences(|p| p.set_visibility(list, fern_core::Visibility::Hide), cx);
                    }))
                    .into_any_element(),
            ],
            ListMenuTarget::Folder(_) => vec![
                item("folder-menu-edit", IconName::Pencil, strings.edit, ListAction::Edit, cx),
                item("folder-menu-add", IconName::Plus, strings.add_list, ListAction::AddList, cx),
                item("folder-menu-ungroup", IconName::FolderOpen, strings.ungroup, ListAction::Ungroup, cx),
            ],
        }
    }

    pub(super) fn render_list_menu(&self, strings: &'static Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (target, at) = self.list_menu.clone()?;
        // The tag menu is the original's 163 px; the others as the task menu.
        let width = match target {
            ListMenuTarget::Tag(_) | ListMenuTarget::TagsHeading => 163.,
            // The original's smart-list menu, with its one Hide.
            ListMenuTarget::Smart(_) => 163.,
            _ => 196.,
        };
        let items = self.render_list_items(target, strings, cx);
        let menu = popover(cx)
            .min_w(px(width))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.list_menu = None;
                cx.notify();
            }))
            .children(items)
            .into_any_element();
        Some(task_menu::place(at, menu))
    }


    /// A right-click menu choice (see `task_menu`).
    pub(super) fn context_action(&mut self, action: task_menu::MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        use task_menu::MenuAction;
        let Some((id, _, _, _)) = self.context_menu.as_ref().map(|(t, p, n)| (t.id, *p, *n, t.title.clone())) else { return };
        if let MenuAction::Side(side) = action {
            self.context_side = side;
            cx.notify();
            return;
        }
        let Some((cached_task, _, _)) = self.context_menu.take() else { return };
        self.context_side = None;
        let task = self
            .tasks
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .or_else(|| self.store.task(id).ok().flatten())
            .unwrap_or(cached_task);
        if task.is_deleted() {
            match action {
                MenuAction::Restore => self.restore(id, cx),
                MenuAction::DeleteForever => self.request_purge_task(id, cx),
                _ => {}
            }
            cx.notify();
            return;
        }
        if self.is_task_read_only(&task) {
            return;
        }
        if let Some(result) = task_menu::apply(&self.store, &task, action, Self::today()) {
            self.check(result);
            self.reload(cx);
            return;
        }
        match action {
            MenuAction::Due(None) => self.start_detail(id, "due", window, cx),
            MenuAction::AddSubtask => self.start_detail(id, "subtask", window, cx),
            MenuAction::LinkParent => self.start_detail(id, "link-parent", window, cx),
            MenuAction::Tags => self.start_detail(id, "tags", window, cx),
            MenuAction::Focus(stopwatch) => cx.emit(StartFocus(id, task.title, stopwatch)),
            MenuAction::CopyLink => cx.write_to_clipboard(gpui::ClipboardItem::new_string(fern_core::link::task_link(id))),
            MenuAction::StickyNote => crate::sticky_opener::open_sticky_note(self.store.clone(), id, cx),
            // Deleting goes the list's usual way, with its Undo.
            MenuAction::Delete => self.delete(id, cx),
            _ => {}
        }
        cx.notify();
    }

    /// The task's right-click menu, as the original's, at the pointer.
    pub(super) fn render_context_menu(&self, strings: &'static Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (cached_task, position, nesting) = self.context_menu.as_ref()?;
        let task = self.tasks.iter().find(|t| t.id == cached_task.id).unwrap_or(cached_task);
        let menu = task_menu::render(
            task,
            &self.active_lists(),
            task_menu::MenuKind::List,
            *nesting,
            self.context_side,
            strings,
            cx,
            std::rc::Rc::new(|this: &mut Self, action, window, cx| this.context_action(action, window, cx)),
        );
        Some(task_menu::place(*position, menu))
    }

    pub(super) fn set_closed_menu(&mut self, menu: Option<ClosedMenu>, cx: &mut Context<Self>) {
        self.closed_menu = menu;
        cx.notify();
    }

    pub(super) fn open_other_date(&mut self, cx: &mut Context<Self>) {
        let (start, end) = match self.closed_range {
            ClosedRange::Custom(start, end) => (Some(start), Some(end)),
            _ => (None, None),
        };
        let month = crate::calendar::month_start(start.unwrap_or_else(Self::today));
        self.set_closed_menu(Some(ClosedMenu::OtherDate { start, end, picking: None, month }), cx);
    }

    pub(super) fn update_other_date(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Option<NaiveDate>, &mut Option<NaiveDate>, &mut Option<RangeEnd>, &mut NaiveDate),
    ) {
        if let Some(ClosedMenu::OtherDate { start, end, picking, month }) = self.closed_menu.as_mut() {
            change(start, end, picking, month);
        }
        cx.notify();
    }

    /// The "Other Date" popover: Start and End fields, a calendar for the one being picked.
    pub(super) fn render_other_date(
        &self,
        start: Option<NaiveDate>,
        end: Option<NaiveDate>,
        picking: Option<RangeEnd>,
        month: NaiveDate,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let today = Self::today();
        let field = |which: RangeEnd, value: Option<NaiveDate>, placeholder: &'static str| {
            let label: SharedString =
                value.map_or(placeholder.into(), |date| task_rows::short_date(date, strings).into());
            h_flex()
                .id(if which == RangeEnd::Start { "range-start" } else { "range-end" })
                .h(px(30.))
                .px_2()
                .rounded(px(6.))
                .border_1()
                .border_color(if picking == Some(which) { theme.colors().accent } else { theme.colors().border })
                .cursor_pointer()
                .child(Label::new(label).color(if value.is_some() { Color::Default } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_other_date(cx, |_, _, picking, _| *picking = Some(which))
                }))
        };
        let calendar = picking.map(|which| {
            let selected = if which == RangeEnd::Start { start } else { end };
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .child(div().flex_1().child(Label::new(crate::calendar::month_title(month, strings)).weight(FontWeight::SEMIBOLD)))
                        .child(
                            IconButton::new("range-prev", IconName::ChevronLeft)
                                .size(px(22.))
                                .tooltip(Tooltip::text("←"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_other_date(cx, |_, _, _, month| *month = crate::calendar::add_months(*month, -1))
                                })),
                        )
                        .child(
                            IconButton::new("range-next", IconName::ChevronRight)
                                .size(px(22.))
                                .tooltip(Tooltip::text("→"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_other_date(cx, |_, _, _, month| *month = crate::calendar::add_months(*month, 1))
                                })),
                        ),
                )
                .child(h_flex().children(crate::calendar::weekday_labels(&strings.weekdays_narrow).iter().map(|day| {
                    div().flex_1().flex().justify_center().child(Label::new(*day).size(LabelSize::XSmall).color(Color::Muted))
                })))
                .children(crate::calendar::month_grid(month).chunks(7).map(|week| {
                    h_flex().children(week.iter().map(|&date| {
                        let is_selected = selected == Some(date);
                        div()
                            .id(SharedString::from(format!("range-day-{date}")))
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .when(is_selected, |el| el.bg(theme.colors().accent))
                            .when(!is_selected, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                            .child(
                                Label::new(date.day().to_string()).centered().size(LabelSize::Small).color(if is_selected {
                                    Color::OnAccent
                                } else if date == today {
                                    Color::Accent
                                } else if date.month() != month.month() {
                                    Color::Muted
                                } else {
                                    Color::Default
                                }),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_other_date(cx, |start, end, picking, _| {
                                    match which {
                                        RangeEnd::Start => *start = Some(date),
                                        RangeEnd::End => *end = Some(date),
                                    }
                                    *picking = None;
                                })
                            }))
                    }))
                }))
        });
        let (start_calendar, end_calendar) =
            if picking == Some(RangeEnd::Start) { (calendar, None) } else { (None, calendar) };
        let ready = start.is_some() && end.is_some();
        popover(cx)
            .w(px(240.))
            .p_2()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_closed_menu(None, cx)))
            .child(field(RangeEnd::Start, start, strings.range_start))
            .children(start_calendar)
            .child(field(RangeEnd::End, end, strings.range_end))
            .children(end_calendar)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id("range-cancel")
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .cursor_pointer()
                            .child(Label::new(strings.cancel).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| this.set_closed_menu(None, cx))),
                    )
                    .child(
                        div()
                            .id("range-confirm")
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(theme.colors().accent)
                            .when(!ready, |el| el.opacity(0.5))
                            .when(ready, |el| el.cursor_pointer())
                            .child(Label::new(strings.confirm).size(LabelSize::Small).color(Color::OnAccent))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let (Some(start), Some(end)) = (start, end) {
                                    this.closed_range = ClosedRange::custom(start, end);
                                    this.set_closed_menu(None, cx);
                                    this.sync_closed_filter(cx);
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn sync_closed_filter(&mut self, cx: &mut Context<Self>) {
        let visible = self.selected.is_some_and(|selected| {
            self.rows(i18n::t(cx)).into_iter().any(|row| matches!(row, Row::Task { task, .. } if task.id == selected))
        });
        self.list_state = Self::new_list_state();
        self.list_entries.clear();
        if !visible {
            self.sync_layout_selection(cx);
        }
        cx.notify();
    }

    /// Toggles a list in the lists-filter draft; "All Lists" is `None`.
    pub(super) fn toggle_closed_list(&mut self, list: Option<Option<Id>>, cx: &mut Context<Self>) {
        if let Some(ClosedMenu::Lists(draft)) = self.closed_menu.as_mut() {
            *draft = match (list, draft.take()) {
                (None, _) => None,
                (Some(list), None) => Some(vec![list]),
                (Some(list), Some(mut lists)) => {
                    if let Some(at) = lists.iter().position(|l| *l == list) {
                        lists.remove(at);
                    } else {
                        lists.push(list);
                    }
                    (!lists.is_empty()).then_some(lists)
                }
            };
        }
        cx.notify();
    }

    /// The "All Dates" / "All Lists" filter buttons of the Completed and Won't Do views.
    pub(super) fn render_closed_filters(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let list_name = |list: Option<Id>| -> SharedString {
            list.and_then(|id| self.lists.iter().find(|l| l.id == id))
                .map_or(SharedString::new_static(strings.inbox), |l| l.name.clone().into())
        };
        let lists_label: SharedString = match &self.closed_lists {
            None => strings.all_lists.into(),
            Some(lists) if lists.len() == 1 => list_name(lists[0]),
            Some(lists) => format!("{} ({})", strings.all_lists, lists.len()).into(),
        };
        let open = self.closed_menu.clone();
        let filter_button = |id: &'static str, label: SharedString, active: bool| {
            h_flex()
                .id(id)
                .h(px(24.))
                .px_2()
                .gap_1()
                .rounded(px(5.))
                .border_1()
                .border_color(if active { theme.colors().accent } else { theme.colors().border })
                .cursor_pointer()
                .child(Label::new(label).size(LabelSize::XSmall))
                .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
        };
        let check = || Label::new("✓").color(Color::Accent);

        let dates_menu = popover(cx)
            .min_w(px(180.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_closed_menu(None, cx)))
            .children(ClosedRange::PRESETS.into_iter().map(|range| {
                let selected = range == self.closed_range;
                ListItem::new(SharedString::from(format!("closed-range-{range:?}")))
                    .child(Label::new(range.label(strings)).color(if selected { Color::Accent } else { Color::Default }))
                    .when(selected, |item| item.end_slot(check()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.closed_range = range;
                        this.set_closed_menu(None, cx);
                        this.sync_closed_filter(cx);
                    }))
            }))
            .child({
                let custom = matches!(self.closed_range, ClosedRange::Custom(..));
                ListItem::new("closed-range-other")
                    .child(Label::new(strings.other_date).color(if custom { Color::Accent } else { Color::Default }))
                    .when(custom, |item| item.end_slot(check()))
                    .on_click(cx.listener(|this, _, _, cx| this.open_other_date(cx)))
            });
        let other_menu = match &open {
            Some(ClosedMenu::OtherDate { start, end, picking, month }) => {
                Some(self.render_other_date(*start, *end, *picking, *month, strings, cx))
            }
            _ => None,
        };

        let draft = match &open {
            Some(ClosedMenu::Lists(draft)) => draft.clone(),
            _ => None,
        };
        let entries: Vec<(Option<Option<Id>>, SharedString, IconName)> = std::iter::once((None, strings.all_lists.into(), IconName::Layers))
            .chain(std::iter::once((Some(None), strings.inbox.into(), IconName::Inbox)))
            .chain(self.lists.iter().map(|l| (Some(Some(l.id)), l.name.clone().into(), IconName::List)))
            .collect();
        let lists_menu = popover(cx)
            .min_w(px(240.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_closed_menu(None, cx)))
            .children(entries.into_iter().map(|(list, label, icon)| {
                let selected = match (&list, &draft) {
                    (None, None) => true,
                    (Some(list), Some(lists)) => lists.contains(list),
                    _ => false,
                };
                let id = match list {
                    None => "closed-list-all".to_owned(),
                    Some(None) => "closed-list-inbox".to_owned(),
                    Some(Some(id)) => format!("closed-list-{id}"),
                };
                ListItem::new(SharedString::from(id))
                    .start_slot(Icon::new(icon).color(if selected { Color::Accent } else { Color::Muted }))
                    .child(Label::new(label).color(if selected { Color::Accent } else { Color::Default }))
                    .when(selected, |item| item.end_slot(check()))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_closed_list(list, cx)))
            }))
            .child(div().py_1().child(Divider::horizontal()))
            .child(
                h_flex()
                    .gap_2()
                    .p_1()
                    .child(
                        div()
                            .id("closed-lists-cancel")
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .cursor_pointer()
                            .child(Label::new(strings.cancel).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| this.set_closed_menu(None, cx))),
                    )
                    .child(
                        div()
                            .id("closed-lists-ok")
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(theme.colors().accent)
                            .cursor_pointer()
                            .child(Label::new(strings.ok).size(LabelSize::Small).color(Color::OnAccent))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(ClosedMenu::Lists(draft)) = this.closed_menu.take() {
                                    this.closed_lists = draft;
                                }
                                this.sync_closed_filter(cx);
                            })),
                    ),
            );

        h_flex()
            .h(px(30.))
            .flex_none()
            .px(px(24.))
            .gap_2()
            .child(
                div()
                    .relative()
                    .child(
                        filter_button(
                            "closed-dates",
                            self.closed_range.label(strings).into(),
                            matches!(open, Some(ClosedMenu::Dates | ClosedMenu::OtherDate { .. })),
                        )
                            .on_click(cx.listener(|this, _, _, cx| this.set_closed_menu(Some(ClosedMenu::Dates), cx))),
                    )
                    .when(open == Some(ClosedMenu::Dates), |el| {
                        el.child(PopoverAttachment::new(Placement::BelowLeft, dates_menu))
                    })
                    .when_some(other_menu, |el, menu| el.child(PopoverAttachment::new(Placement::BelowLeft, menu))),
            )
            .child(
                div()
                    .relative()
                    .child(
                        filter_button("closed-lists", lists_label, matches!(open, Some(ClosedMenu::Lists(_))))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let draft = this.closed_lists.clone();
                                this.set_closed_menu(Some(ClosedMenu::Lists(draft)), cx)
                            })),
                    )
                    .when(matches!(open, Some(ClosedMenu::Lists(_))), |el| {
                        el.child(PopoverAttachment::new(Placement::BelowLeft, lists_menu))
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_header_menu(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let in_list = matches!(self.view, View::List(_));
        let in_trash = self.view == View::Trash;
        popover(cx)
            .min_w(px(236.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.header_menu_open = false;
                cx.notify();
            }))
            .when_some(match self.view {
                View::List(id) => Some(id),
                _ => None,
            }, |el, id| el.children(self.render_list_items(ListMenuTarget::List(id), strings, cx)).child(div().my_1().child(Divider::horizontal())))
            .when_some(match self.view {
                View::Filter(id) => self.filters.iter().find(|(f, _)| f.id == id).map(|(f, _)| f.clone()),
                _ => None,
            }, |el, filter| {
                el.child(
                    ListItem::new("edit-filter")
                        .start_slot(Icon::new(IconName::Pencil).color(Color::Icon))
                        .child(Label::new(strings.edit_filter))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_filter_edit(Some(filter.clone()), window, cx))),
                )
                .child(
                    ListItem::new("delete-filter")
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(Label::new(strings.delete_filter).color(Color::Error))
                        .on_click(cx.listener(|this, _, _, cx| this.delete_current_filter(cx))),
                )
                .child(div().my_1().child(Divider::horizontal()))
            })
            .when(in_trash, |el| {
                el.child(
                    ListItem::new("empty-trash")
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(Label::new(strings.empty_trash).color(Color::Error))
                        .on_click(cx.listener(|this, _, _, cx| this.request_empty_trash(cx))),
                )
            })
            .when(!in_trash && !matches!(self.view, View::Completed | View::WontDo | View::Search(_)), |el| {
                let completed = self.view_flag("show_completed", true);
                let details = self.view_flag("show_details", false);
                let kanban = self.kanban;
                let timeline = self.timeline && !kanban;
                let theme = cx.theme().clone();
                let layout = |id: &'static str, icon: IconName, on: bool| {
                    div()
                        .id(id)
                        .flex_1()
                        .h(px(30.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(on, |el| el.bg(theme.colors().element_selected))
                        .hover(|s| s.bg(theme.colors().ghost_element_hover))
                        .child(Icon::new(icon).color(if on { Color::Accent } else { Color::Icon }))
                };
                el.when(in_list, |el| el.child(div().my_1().child(Divider::horizontal())))
                    .child(div().px_2().pt_1().child(Label::new(strings.view_label).size(LabelSize::Small).color(Color::Muted)))
                    .child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_1()
                            .child(layout("layout-list", IconName::List, !kanban && !timeline).on_click(cx.listener(|this, _, _, cx| this.set_layout(false, false, cx))))
                            .child(layout("layout-kanban", IconName::Kanban, kanban).on_click(cx.listener(|this, _, _, cx| this.set_layout(true, false, cx))))
                            .child(layout("layout-timeline", IconName::ChartGantt, timeline).on_click(cx.listener(|this, _, _, cx| this.set_layout(false, true, cx)))),
                    )
                    .child(div().my_1().child(Divider::horizontal()))
                    .when(timeline && !self.is_current_view_archived_list(), |el| {
                        el.child(
                            ListItem::new("arrange-tasks")
                                .start_slot(Icon::new(IconName::CalendarPlus).color(Color::Icon))
                                .child(Label::new(strings.arrange_tasks))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.arrange_open = true;
                                    this.header_menu_open = false;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        ListItem::new("toggle-completed")
                            .start_slot(Icon::new(IconName::SquareCheck).color(Color::Icon))
                            .child(Label::new(if completed { strings.hide_completed } else { strings.show_completed }))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_view_flag("show_completed", true, cx))),
                    )
                    .child(
                        ListItem::new("toggle-details")
                            .start_slot(Icon::new(IconName::FileText).color(Color::Icon))
                            .child(Label::new(if details { strings.hide_details } else { strings.show_details }))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_view_flag("show_details", false, cx))),
                    )
                    .when(!timeline, |el| {
                        let parents = self.parent_task_ids(strings);
                        if parents.is_empty() {
                            return el;
                        }
                        let any_expanded = parents.iter().any(|id| !self.collapsed_tasks.contains(id));
                        el.child(
                            ListItem::new("toggle-all-subtasks")
                                .start_slot(Icon::new(IconName::ChevronsUpDown).color(Color::Icon))
                                .child(Label::new(if any_expanded {
                                    strings.collapse_all_subtasks
                                } else {
                                    strings.expand_all_subtasks
                                }))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.header_menu_open = false;
                                    this.toggle_all_subtasks_now(cx);
                                })),
                        )
                    })
                    .child(
                        ListItem::new("view-options")
                            .start_slot(Icon::new(IconName::Settings2).color(Color::Icon))
                            .child(Label::new(strings.view_options))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.header_menu_open = false;
                                this.view_options = Some(false);
                                cx.notify();
                            })),
                    )
                    .child(div().my_1().child(Divider::horizontal()))
                    .when(self.section_owner().is_some() && !self.is_current_view_archived_list(), |el| {
                        el.child(
                            ListItem::new("add-section")
                                .start_slot(Icon::new(IconName::Plus).color(Color::Icon))
                                .child(Label::new(strings.add_section))
                                .on_click(cx.listener(|this, _, window, cx| this.start_new_section(window, cx))),
                        )
                    })
                    .when(in_list || self.view == View::Inbox, |el| {
                        el.child(
                            ListItem::new("list-activities")
                                .start_slot(Icon::new(IconName::ListTree).color(Color::Icon))
                                .child(Label::new(strings.list_activities))
                                .on_click(cx.listener(|this, _, _, cx| this.open_list_activity(cx))),
                        )
                    })
                    .child(
                        ListItem::new("print-list")
                            .start_slot(Icon::new(IconName::Printer).color(Color::Icon))
                            .child(Label::new(strings.print))
                            .on_click(cx.listener(|this, _, _, cx| this.print_list(cx))),
                    )
            })
            .when(!in_list && !in_trash && matches!(self.view, View::Completed | View::WontDo | View::Search(_)), |el| {
                el.child(div().px_2().py_1().child(Label::new(strings.no_actions).color(Color::Muted)))
            })
            .into_any_element()
    }
}
