use super::*;

/// What the add field's "▾" menu set for the next task.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AddDraft {
    pub(super) priority: Option<Priority>,
    /// `Some(None)` is the Inbox.
    pub(super) list: Option<Option<Id>>,
    pub(super) note: bool,
    pub(super) tags: Vec<String>,
    /// The date picked with the field's calendar button, with its time, reminders and repeat;
    /// a typed date wins.
    pub(super) date: Option<NaiveDate>,
    pub(super) time: Option<chrono::NaiveTime>,
    /// `None` keeps the default reminder from the settings.
    pub(super) reminders: Option<Vec<i64>>,
    pub(super) repeat: Option<fern_core::Recurrence>,
    /// Files picked with the menu's Attachment, attached once the task is added.
    pub(super) files: Vec<std::path::PathBuf>,
    /// The end the picker's Duration tab set, with the start in `date` and `time`.
    pub(super) end: Option<(NaiveDate, Option<chrono::NaiveTime>)>,
    /// Horizon bucket chosen from the quick-add date picker (mutually exclusive with `date`).
    pub(super) horizon: Option<fern_core::Horizon>,
}


impl TodoView {
    /// Creates a task from the quick-add line: due date, time, tags, priority and list come from
    /// the text ("tomorrow 6pm #home !1 ^work"); anything missing comes from the current view —
    /// a due date in Today/Tomorrow, the tag in a tag view, the list in a list view.
    /// A task from a template, placed like a typed one: in the current list, tag or day.
    pub(super) fn add_from_template(&mut self, template: Id, cx: &mut Context<Self>) {
        self.quick_add.templates_open = false;
        let today = Self::today();
        let result = (|| -> anyhow::Result<Task> {
            let task = self.store.add_from_template(template)?;
            match &self.view {
                View::Today | View::Next7Days => self.store.set_due(task.id, Some(today), None)?,
                View::Tomorrow => self.store.set_due(task.id, Some(today + Duration::days(1)), None)?,
                View::List(list) => self.store.set_list(task.id, Some(*list))?,
                View::Tag(tag) if !task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) => {
                    let mut tags = task.tags.clone();
                    tags.push(tag.clone());
                    self.store.set_tags(task.id, &tags)?;
                }
                _ => {}
            }
            Ok(task)
        })();
        if let Some(task) = self.check(result) {
            crate::data_version::data_changed(cx.entity_id(), cx);
            self.reload(cx);
            self.select(task.id, cx);
        }
    }

    pub(super) fn render_templates(&self, strings: &Strings, cx: &mut Context<Self>) -> AnyElement {
        let templates = &self.cached_templates;
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(280.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.quick_add.templates_open = false;
                cx.notify();
            }))
            .child(div().px_2().py_1().child(Label::new(strings.templates).weight(FontWeight::SEMIBOLD)))
            .when(templates.is_empty(), |el| {
                el.child(div().px_2().py_2().child(Label::new(strings.no_templates).size(LabelSize::Small).color(Color::Muted)))
            })
            .children(templates.iter().cloned().map(|template| {
                let id = template.id;
                let template_title = template.title.clone();
                let group = SharedString::from(format!("template-{id}"));
                let tags = (!template.tags.is_empty())
                    .then(|| template.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "));
                let checklist = template.checklist.len();
                h_flex()
                    .id(group.clone())
                    .group(group.clone())
                    .when(cfg!(test), |el| el.debug_selector(move || format!("template-{id}")))
                    .h(px(34.))
                    .px_2()
                    .gap_2()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(Icon::new(IconName::LayoutTemplate).size(IconSize::Small).color(Color::Muted))
                    .child(div().flex_1().min_w_0().child(Label::new(template.title).truncate()))
                    .children(tags.map(|t| Label::new(t).size(LabelSize::XSmall).color(Color::Muted)))
                    .when(checklist > 0, |el| {
                        el.child(Icon::new(IconName::ListChecks).size(IconSize::XSmall).color(Color::Muted))
                            .child(Label::new(checklist.to_string()).size(LabelSize::XSmall).color(Color::Muted))
                    })
                    .child(
                        div()
                            .opacity(if crate::touch::active() { 1. } else { 0. })
                            .group_hover(group, |s| s.opacity(1.))
                            .when(cfg!(test), |el| el.debug_selector(move || format!("delete-template-{id}")))
                            .child(
                                IconButton::new(SharedString::from(format!("delete-template-{id}")), IconName::X)
                                    .size(px(20.))
                                    .icon_size(IconSize::XSmall)
                                    .icon_color(Color::Muted)
                                    .hover_icon_color(Color::Error)
                                    .tooltip(Tooltip::text(strings.delete))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.quick_add.templates_open = false;
                                        this.overlays.confirm_delete = Some(ConfirmDeleteTarget::Template(id, template_title.clone()));
                                        cx.notify();
                                    })),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.add_from_template(id, cx)))
            }))
            .into_any_element()
    }

    /// The system-wide Quick Add's task: added as from All, so that it takes the Settings'
    /// defaults (the default list among them) and nothing from the view or the add field's menu.
    /// Adds a task from the system-wide Quick Add window, with optional date, priority, and list.
    pub fn add_quick_with(
        &mut self,
        text: &str,
        date: Option<NaiveDate>,
        priority: Option<Priority>,
        list: Option<Option<Id>>,
        cx: &mut Context<Self>,
    ) {
        if self.refresh.is_dirty() {
            self.preferences = self.store.preferences().unwrap_or_default();
            if let Some(lists) = self.check(self.store.lists()) {
                self.lists = lists;
                self.refresh_list_colors();
                self.panel_dnd.lists_index_dirty = true;
            }
        }
        let draft = AddDraft { date, priority, list, ..Default::default() };
        let result = self.add_task_as(text, &View::All, &draft, cx);
        self.check(result);
        if self.refresh.is_active() {
            self.reload(cx);
        } else {
            crate::data_version::data_changed_scoped(
                cx.entity_id(),
                crate::data_version::DataScope::TASKS | crate::data_version::DataScope::LISTS,
                cx,
            );
            let _ = self.refresh.mark_dirty_or_reload();
        }
    }

    pub fn add_quick(&mut self, text: &str, cx: &mut Context<Self>) {
        self.add_quick_with(text, None, None, None, cx);
    }

    fn attach_draft_files(&mut self, task_id: Id, files: Vec<std::path::PathBuf>, cx: &App) {
        let strings = i18n::t(cx);
        let errors: Vec<_> = files
            .into_iter()
            .filter_map(|file| {
                let result =
                    crate::attachments_dir().and_then(|dir| self.store.add_attachment(task_id, &file, &dir));
                let error = result.err()?;
                let name = file.file_name().unwrap_or(file.as_os_str()).to_string_lossy();
                Some(
                    strings
                        .attachment_failed
                        .replace("{file}", &name)
                        .replace("{error}", &crate::sync::attachment_error(&error, strings)),
                )
            })
            .collect();
        self.quick_add.attachment_error = (!errors.is_empty()).then(|| errors.join("\n"));
    }

    pub(super) fn add_task(&mut self, input: &str, cx: &App) -> Option<Task> {
        let result = self.add_task_as(input, &self.view, &self.quick_add.draft, cx);
        let task = self.check(result)?;
        let files = std::mem::take(&mut self.quick_add.draft.files);
        self.attach_draft_files(task.id, files, cx);
        Some(task)
    }

    /// Computes the structured [`fern_core::TaskDraft`] and `top` flag corresponding to `input`
    /// typed in `view` with `draft`'s picker selections (used by `beforeTaskCreate` plugin hooks).
    fn build_hook_task_draft(
        input: &str,
        view: &View,
        draft: &AddDraft,
        lists: &[fern_core::List],
        preferences: &fern_core::Preferences,
        today: NaiveDate,
        horizons_enabled: bool,
    ) -> (fern_core::TaskDraft, bool) {
        let list_names: Vec<String> = lists.iter().map(|l| l.name.clone()).collect();
        let parsed = fern_core::parse_quick_add_with_horizons(
            input,
            today,
            &list_names,
            preferences.recognition,
            horizons_enabled,
        );
        let title = if parsed.title.is_empty() { input.trim() } else { parsed.title.as_str() };
        let view_due = match view {
            View::Today | View::Next7Days => Some(today),
            View::Tomorrow => Some(today + Duration::days(1)),
            View::Day(day) => Some(*day),
            _ => None,
        };
        let defaults = &preferences.task_defaults;
        let from_picker = parsed.due_date.is_none() && draft.date.is_some();
        let due_time = if from_picker { draft.time } else { parsed.due_time };
        let explicit_due = parsed.due_date.or(draft.date).or(view_due);
        let horizon = if explicit_due.is_none() {
            parsed.horizon.or(draft.horizon)
        } else {
            None
        };
        let due_date = explicit_due.or_else(|| {
            if horizon.is_some() {
                None
            } else {
                defaults.date.map(|d| d.date(today))
            }
        });
        let (end_date, end_time) = if due_date.is_some() && from_picker {
            match draft.end {
                Some((d, t)) => (Some(d), t),
                None => (None, None),
            }
        } else {
            (None, None)
        };
        let reminders = if due_date.is_some() {
            match draft.reminders.as_ref().filter(|_| from_picker) {
                Some(reminders) => reminders.clone(),
                None => {
                    let reminder = if due_time.is_some() {
                        defaults.timed_reminder
                    } else {
                        defaults.all_day_reminder
                    };
                    reminder.into_iter().collect()
                }
            }
        } else {
            Vec::new()
        };
        let mut tags = parsed.tags;
        if let Some(tag) = &defaults.tag
            && !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase())
        {
            tags.push(tag.clone());
        }
        if let View::Tag(tag) = view
            && !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase())
        {
            tags.push(tag.clone());
        }
        for tag in &draft.tags {
            if !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
                tags.push(tag.clone());
            }
        }
        let repeat = parsed
            .repeat
            .or_else(|| draft.repeat.clone().filter(|_| draft.date.is_some()));
        let draft_priority = draft.priority.filter(|p| *p != fern_core::Priority::None);
        let priority = parsed
            .priority
            .or(draft_priority)
            .or((defaults.priority != fern_core::Priority::None).then_some(defaults.priority))
            .unwrap_or(fern_core::Priority::None);
        let parsed_list = parsed
            .list
            .and_then(|name| lists.iter().find(|l| l.name == name))
            .map(|l| l.id);
        let view_list = match view {
            View::List(id) => Some(*id),
            _ => None,
        };
        let default_list = match view {
            View::Inbox | View::List(_) => None,
            _ => preferences.default_list,
        };
        let list_id = parsed_list
            .map(Some)
            .or(draft.list)
            .or(view_list.map(Some))
            .or(default_list.map(Some))
            .flatten();
        let note_list = list_id
            .is_some_and(|id| lists.iter().any(|l| l.id == id && l.kind == fern_core::Kind::Note));
        let kind = if draft.note || note_list {
            fern_core::Kind::Note
        } else {
            fern_core::Kind::Task
        };
        (
            fern_core::TaskDraft {
                title: title.to_owned(),
                list_id,
                kind,
                priority,
                due_date,
                due_time,
                end_date,
                end_time,
                repeat,
                reminders,
                tags,
                horizon,
                ..Default::default()
            },
            !defaults.add_to_bottom,
        )
    }

    /// Adds a task from the add field's text as if typed in `view` with `draft`'s choices.
    pub(super) fn add_task_as(&self, input: &str, view: &View, draft: &AddDraft, cx: &App) -> anyhow::Result<Task> {
        self.add_task_as_with(input, view, draft, crate::calendar::horizons_enabled(cx))
    }

    fn add_task_as_with(
        &self,
        input: &str,
        view: &View,
        draft: &AddDraft,
        horizons_enabled: bool,
    ) -> anyhow::Result<Task> {
        let today = Self::today();
        let list_names: Vec<String> = self.lists.iter().map(|l| l.name.clone()).collect();
        let parsed = fern_core::parse_quick_add_with_horizons(
            input,
            today,
            &list_names,
            self.preferences.recognition,
            horizons_enabled,
        );
        let title = if parsed.title.is_empty() { input.trim() } else { parsed.title.as_str() };
        let task = self.store.add_task_at(title, !self.preferences.task_defaults.add_to_bottom)?;

        let view_due = match view {
            View::Today | View::Next7Days => Some(today),
            View::Tomorrow => Some(today + Duration::days(1)),
            View::Day(day) => Some(*day),
            _ => None,
        };
        // Settings → More → Task Default fills in what neither the text nor the view gives.
        let defaults = &self.preferences.task_defaults;
        // The date picker's time and reminders go with its date, not with a typed one.
        let from_picker = parsed.due_date.is_none() && draft.date.is_some();
        let due_time = if from_picker { draft.time } else { parsed.due_time };
        let explicit_due = parsed.due_date.or(draft.date).or(view_due);
        let chosen_horizon = if explicit_due.is_none() {
            parsed.horizon.or(draft.horizon)
        } else {
            None
        };
        let resolved_due = explicit_due.or_else(|| {
            if chosen_horizon.is_some() {
                None
            } else {
                defaults.date.map(|d| d.date(today))
            }
        });
        if let Some(date) = resolved_due {
            self.store.set_due(task.id, Some(date), due_time)?;
            if from_picker && draft.end.is_some() {
                self.store.set_due_end(task.id, draft.end)?;
            }
            match draft.reminders.as_ref().filter(|_| from_picker) {
                Some(reminders) => self.store.set_reminders(task.id, reminders)?,
                None => {
                    let reminder = if due_time.is_some() { defaults.timed_reminder } else { defaults.all_day_reminder };
                    if let Some(offset) = reminder {
                        self.store.set_reminders(task.id, &[offset])?;
                    }
                }
            }
        } else if let Some(horizon) = chosen_horizon {
            self.store.set_horizon(task.id, Some(horizon))?;
        }

        let mut tags = parsed.tags;
        if let Some(tag) = &defaults.tag
            && !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase())
        {
            tags.push(tag.clone());
        }
        if let View::Tag(tag) = view
            && !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase())
        {
            tags.push(tag.clone());
        }
        for tag in &draft.tags {
            if !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
                tags.push(tag.clone());
            }
        }
        if !tags.is_empty() {
            self.store.set_tags(task.id, &tags)?;
        }
        if let Some(repeat) = parsed.repeat.or_else(|| draft.repeat.clone().filter(|_| draft.date.is_some())) {
            self.store.set_repeat(task.id, Some(repeat), today)?;
        }
        let draft_priority = draft.priority.filter(|p| *p != fern_core::Priority::None);
        if let Some(priority) = parsed
            .priority
            .or(draft_priority)
            .or((defaults.priority != fern_core::Priority::None).then_some(defaults.priority))
        {
            self.store.set_priority(task.id, priority)?;
        }

        let parsed_list =
            parsed.list.and_then(|name| self.lists.iter().find(|l| l.name == name)).map(|l| l.id);
        let view_list = match view {
            View::List(id) => Some(*id),
            _ => None,
        };
        // Outside the Inbox and lists, new tasks go to the default list from the Settings.
        let default_list = match view {
            View::Inbox | View::List(_) => None,
            _ => self.preferences.default_list,
        };
        // The "▾" menu's list goes before the view's and the default one.
        let draft_list = draft.list;
        if let Some(Some(list)) = parsed_list.map(Some).or(draft_list).or(view_list.map(Some)).or(default_list.map(Some)) {
            self.store.set_list(task.id, Some(list))?;
        }
        // In a note list (the original's List Type), new entries are notes.
        let note_list = self.store.task(task.id)?.and_then(|t| t.list_id).is_some_and(|id| self.lists.iter().any(|l| l.id == id && l.kind == fern_core::Kind::Note));
        if draft.note || note_list {
            self.store.set_kind(task.id, fern_core::Kind::Note)?;
        }
        Ok(task)
    }

    /// Highlights what quick add recognizes in the field and shows the recognized date or horizon.
    pub(super) fn update_quick_add_hint(&mut self, cx: &mut Context<Self>) {
        let text = self.quick_add.input.read(cx).text().to_owned();
        let list_names: Vec<String> = self.lists.iter().map(|l| l.name.clone()).collect();
        let parsed = fern_core::parse_quick_add_with_horizons(
            &text,
            Self::today(),
            &list_names,
            self.preferences.recognition,
            crate::calendar::horizons_enabled(cx),
        );
        let spans = parsed.spans.clone();
        self.quick_add.input.update(cx, |input, cx| input.set_highlights(spans, cx));
        self.quick_add.due = parsed.due_date.map(|date| (date, parsed.due_time));
        self.quick_add.horizon = if parsed.due_date.is_some() { None } else { parsed.horizon };
        self.quick_add.repeat = parsed.repeat.is_some();
        cx.notify();
    }

    /// The add field's placeholder: the Detailed box asks "What would you like to do?".
    pub(super) fn sync_input_placeholder(&mut self, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        // In the smart lists, as the original: 'Add task to "Inbox"', naming where new tasks go.
        let smart = matches!(self.view, View::All | View::Today | View::Tomorrow | View::Next7Days);
        let placeholder: SharedString = if self.preferences.detailed_input {
            strings.input_box_placeholder.into()
        } else if let View::Tag(tag) = &self.view {
            // As the original: the tag the new task gets, "#work".
            format!("#{tag}").into()
        } else if smart {
            let list = self.preferences.default_list.and_then(|id| self.lists.iter().find(|l| l.id == id)).map_or(strings.inbox.to_owned(), |l| l.name.clone());
            strings.add_task_to.replace("{list}", &list).into()
        } else {
            strings.add_task.into()
        };
        self.quick_add.input.update(cx, |input, cx| input.set_placeholder(placeholder, cx));
    }

    /// Adds the typed task, as Enter in the add field or its "Add" button does.
    pub(super) fn submit_new_task(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(hooks) = desktop_common::plugin_bus::PluginHooksGlobal::global(cx)
            .filter(desktop_common::plugin_bus::PluginHooksGlobal::is_active)
        {
            if self.quick_add.submitting_task {
                return;
            }
            self.quick_add.submitting_task = true;
            let raw_text = text.to_owned();
            let view = self.view.clone();
            let draft_snapshot = self.quick_add.draft.clone();
            let lists = self.lists.clone();
            let preferences = self.preferences.clone();
            let today = Self::today();
            let horizons_enabled = crate::calendar::horizons_enabled(cx);
            let window_handle = window.window_handle();
            cx.spawn(async move |this, cx| {
                let hooks_t = hooks.clone();
                let raw_for_t = raw_text.clone();
                let transform = cx
                    .background_spawn(async move { hooks_t.run_quick_add_transform(raw_for_t) })
                    .await;
                let transformed = match transform {
                    Ok(desktop_common::plugin_bus::PluginHookDecision::Cancel(reason)) => {
                        cx.update(|cx| {
                            let fallback = i18n::t(cx).plugins_task_create_cancelled;
                            desktop_common::plugin_bus::push_hook_cancel_toast(&reason, fallback, cx);
                        });
                        let _ = this.update(cx, |this, cx| {
                            this.quick_add.submitting_task = false;
                            cx.notify();
                        });
                        return;
                    }
                    Ok(desktop_common::plugin_bus::PluginHookDecision::Modify(val)) => {
                        desktop_common::plugin_bus::extract_transformed_quick_add(&val)
                            .unwrap_or(raw_text)
                    }
                    Ok(desktop_common::plugin_bus::PluginHookDecision::Allow) => raw_text,
                    Err(err) => {
                        desktop_common::logging::log_error(
                            "todo_ui",
                            format!("plugin quickAddTransform hook failed: {err:#}"),
                        );
                        raw_text
                    }
                };

                let (task_draft, default_top) = Self::build_hook_task_draft(
                    &transformed,
                    &view,
                    &draft_snapshot,
                    &lists,
                    &preferences,
                    today,
                    horizons_enabled,
                );
                let draft_json =
                    desktop_common::plugin_bus::task_draft_to_hook_json(&task_draft, default_top);
                let before_res = cx
                    .background_spawn(async move { hooks.run_before_task_create(draft_json) })
                    .await;

                match before_res {
                    Ok(desktop_common::plugin_bus::PluginHookDecision::Cancel(reason)) => {
                        cx.update(|cx| {
                            let fallback = i18n::t(cx).plugins_task_create_cancelled;
                            desktop_common::plugin_bus::push_hook_cancel_toast(&reason, fallback, cx);
                        });
                        let _ = this.update(cx, |this, cx| {
                            this.quick_add.submitting_task = false;
                            cx.notify();
                        });
                    }
                    Ok(desktop_common::plugin_bus::PluginHookDecision::Modify(modified_json)) => {
                        let parsed = desktop_common::plugin_bus::parse_task_draft_from_hook_json(
                            &modified_json,
                            default_top,
                        );
                        let mut input_to_clear = None;
                        let _ = this.update(cx, |this, cx| {
                            this.quick_add.submitting_task = false;
                            let created = match parsed {
                                Ok((modified_draft, top)) => {
                                    this.check(this.store.create_task(modified_draft, top))
                                }
                                Err(err) => {
                                    desktop_common::logging::log_error(
                                        "todo_ui",
                                        format!("plugin beforeTaskCreate returned invalid draft: {err:#}"),
                                    );
                                    this.check(this.add_task_as_with(
                                        &transformed,
                                        &view,
                                        &draft_snapshot,
                                        horizons_enabled,
                                    ))
                                }
                            };
                            if let Some(task) = created {
                                let files = std::mem::take(&mut this.quick_add.draft.files);
                                this.attach_draft_files(task.id, files, cx);
                                this.quick_add.due = None;
                                this.quick_add.horizon = None;
                                this.quick_add.draft = AddDraft::default();
                                input_to_clear = Some(this.quick_add.input.clone());
                                this.reload(cx);
                            }
                        });
                        if let Some(input) = input_to_clear {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                input.update(cx, |input, cx| input.clear(window, cx));
                            });
                        }
                    }
                    Ok(desktop_common::plugin_bus::PluginHookDecision::Allow) => {
                        let mut input_to_clear = None;
                        let _ = this.update(cx, |this, cx| {
                            this.quick_add.submitting_task = false;
                            let created = this.check(this.add_task_as_with(
                                &transformed,
                                &view,
                                &draft_snapshot,
                                horizons_enabled,
                            ));
                            if let Some(task) = created {
                                let files = std::mem::take(&mut this.quick_add.draft.files);
                                this.attach_draft_files(task.id, files, cx);
                                this.quick_add.due = None;
                                this.quick_add.horizon = None;
                                this.quick_add.draft = AddDraft::default();
                                input_to_clear = Some(this.quick_add.input.clone());
                                this.reload(cx);
                            }
                        });
                        if let Some(input) = input_to_clear {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                input.update(cx, |input, cx| input.clear(window, cx));
                            });
                        }
                    }
                    Err(err) => {
                        desktop_common::logging::log_error(
                            "todo_ui",
                            format!("plugin beforeTaskCreate hook failed: {err:#}"),
                        );
                        let mut input_to_clear = None;
                        let _ = this.update(cx, |this, cx| {
                            this.quick_add.submitting_task = false;
                            let created = this.check(this.add_task_as_with(
                                &transformed,
                                &view,
                                &draft_snapshot,
                                horizons_enabled,
                            ));
                            if let Some(task) = created {
                                let files = std::mem::take(&mut this.quick_add.draft.files);
                                this.attach_draft_files(task.id, files, cx);
                                this.quick_add.due = None;
                                this.quick_add.horizon = None;
                                this.quick_add.draft = AddDraft::default();
                                input_to_clear = Some(this.quick_add.input.clone());
                                this.reload(cx);
                            }
                        });
                        if let Some(input) = input_to_clear {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                input.update(cx, |input, cx| input.clear(window, cx));
                            });
                        }
                    }
                }
            })
            .detach();
            return;
        }
        if self.add_task(text, cx).is_some() {
            self.quick_add.input.update(cx, |input, cx| input.clear(window, cx));
            self.quick_add.due = None;
            self.quick_add.horizon = None;
            self.quick_add.draft = AddDraft::default();
            self.reload(cx);
        }
    }

    /// The popovers the add field's buttons open: the "▾" menu, its Tags and Input Box Setting,
    /// and the templates.
    pub(super) fn add_field_popovers(&self, el: Div, strings: &'static Strings, cx: &mut Context<Self>) -> Div {
        el.when(self.quick_add.menu_open, |el| {
            // As the original's, the menu hangs from the field's calendar button, opening rightwards.
            el.child(PopoverAttachment::new(Placement::BelowLeft, self.render_add_menu(strings, cx)))
        })
        .when_some(self.quick_add.tags.clone(), |el, picked| {
            el.child(PopoverAttachment::new(Placement::BelowRight, self.render_add_tags(&picked, strings, cx)))
        })
        .when(self.quick_add.input_box_open, |el| {
            el.child(PopoverAttachment::new(Placement::BelowRight, self.render_input_box_setting(strings, cx)))
        })
        .when(self.quick_add.templates_open, |el| {
            el.child(PopoverAttachment::new(Placement::BelowRight, self.render_templates(strings, cx)))
        })
    }

    /// The date the field shows: the typed one, or the one picked with its calendar button.
    pub(super) fn shown_add_due(&self) -> Option<(NaiveDate, Option<chrono::NaiveTime>)> {
        self.quick_add.due.or(self.quick_add.draft.date.map(|date| (date, self.quick_add.draft.time)))
    }

    /// The horizon the field shows when no concrete date is set.
    pub(super) fn shown_add_horizon(&self, cx: &App) -> Option<fern_core::Horizon> {
        if !crate::calendar::horizons_enabled(cx) || self.shown_add_due().is_some() {
            return None;
        }
        self.quick_add.horizon.or(self.quick_add.draft.horizon)
    }

    /// The field's calendar button, as in the original: it opens a date picker for the next task,
    /// placed under the button.
    pub(super) fn add_date_button(&self, placement: Placement, strings: &'static Strings, cx: &mut Context<Self>) -> Div {
        let picked = self.quick_add.draft.date.is_some() || self.quick_add.draft.horizon.is_some();
        let button = IconButton::new("add-date", IconName::CalendarDays)
            .size(px(24.))
            .icon_size(IconSize::Small)
            .icon_color(if picked { Color::Accent } else { Color::Muted })
            .tooltip(Tooltip::text(strings.due_date))
            .on_click(cx.listener(|this, _, _, cx| {
                this.quick_add.date_month = match this.quick_add.date_month {
                    Some(_) => None,
                    None => Some(crate::calendar::month_start(this.quick_add.draft.date.unwrap_or_else(Self::today))),
                };
                cx.notify();
            }));
        div().relative().child(button).when_some(self.quick_add.date_month, |el, month| {
            el.child(PopoverAttachment::new(placement, self.render_add_date(month, strings, cx)))
        })
    }

    /// Focuses the quick-add field (or, in kanban view, the first column's add input).
    pub fn focus_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refresh.is_dirty() {
            self.reload_inner(false, false, cx);
        }
        if self.is_read_only_view() {
            self.set_view(View::Inbox, cx);
        }
        if self.kanban.enabled && !self.timeline.enabled {
            let strings = i18n::t(cx);
            let columns = self.board_columns(strings, cx);
            if self.kanban.quick_add {
                self.sync_kanban_quick_adds(&columns, strings, cx);
                if let Some(col) = columns.iter().find(|c| c.drop.as_ref().is_some_and(|t| !matches!(t, DropTarget::Complete)))
                    && let Some((_, input)) = self.kanban.quick_adds.get(&col.key)
                {
                    window.focus(&input.focus_handle(cx), cx);
                    return;
                }
            } else if let Some((idx, target)) = columns
                .iter()
                .enumerate()
                .find(|(_, c)| c.drop.as_ref().is_some_and(|t| !matches!(t, DropTarget::Complete)))
                .and_then(|(idx, col)| col.drop.clone().map(|target| (idx, target)))
            {
                self.start_column_add(idx, target, window, cx);
                return;
            }
        }
        window.focus(&self.quick_add.input.focus_handle(cx), cx);
    }

}
