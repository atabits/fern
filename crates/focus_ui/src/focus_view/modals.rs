//! Modal dialogs, popovers, manual record form, timer editor, batch management, and white noise controls.

use super::*;

/// The Focus Record's "+" form and "…" menu, as in the original.
/// The "Add Timer" popover: the timer it edits (or a new one), its name, Pomo length and mode.
pub(super) struct TimerDraft {
    pub(super) timer: fern_core::focus::FocusTimer,
    pub(super) name: Entity<TextInput>,
    /// The Pomo minutes, typed as in the original.
    pub(super) minutes: Entity<TextInput>,
    pub(super) new: bool,
    /// The icon picker under the name, while open: 0 Emoji, 1 Text (the name's initial).
    pub(super) icon_tab: Option<usize>,
    /// The icon and color before the picker opened, which its Cancel puts back.
    pub(super) icon_backup: (Option<String>, Option<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecordMenu {
    Add,
    More,
    /// The Focus Record's "Batch Management": records to tick and delete together.
    Batch,
    /// The Pomodoro header's "…" and its Focus Settings.
    HeaderMore,
    Settings,
}

impl FocusView {
    pub(super) fn set_record_menu(&mut self, menu: Option<RecordMenu>, cx: &mut Context<Self>) {
        self.record_menu = menu;
        cx.notify();
    }

    /// Saves the "+" form: a session today from the typed start time for the chosen minutes.
    fn add_record(&mut self, cx: &mut Context<Self>) {
        let title = self.add_title.read(cx).text().trim().to_owned();
        let start = crate::due_picker::parse_time(self.add_start.read(cx).text());
        let (Some(start), false) = (start, title.is_empty()) else { return };
        let today = Local::now().date_naive();
        let local_dt = today.and_time(start);
        // Resolve DST fall-back ambiguity via earliest/latest, and handle spring-forward gaps
        // cleanly instead of silently discarding the user's session.
        let started = resolve_local_datetime(local_dt.and_local_timezone(Local)).or_else(|| {
            resolve_local_datetime((local_dt + chrono::Duration::hours(1)).and_local_timezone(Local))
        });
        let Some(started) = started else { return };
        let ended = started + self.add_minutes * 60_000;
        if let Err(err) = self.store.add_focus_session(None, &title, started, ended) {
            desktop_common::logging::log_error("focus_ui", format!("failed to add the focus record: {err:#}"));
            return;
        }
        self.add_title.update(cx, |input, cx| input.set_text("", cx));
        self.add_start.update(cx, |input, cx| input.set_text("", cx));
        self.record_menu = None;
        self.reload(cx);
        desktop_common::data_version::data_changed_scoped(
            cx.entity_id(),
            desktop_common::data_version::DataScope::FOCUS,
            cx,
        );
    }

    /// Opens a focus record's dialog, its note in the field.
    pub(super) fn open_record(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let note = self.sessions.iter().find(|s| s.id == id).map(|s| s.note.clone()).unwrap_or_default();
        self.record_note.update(cx, |input, cx| input.set_text(note, cx));
        window.focus(&self.record_note.focus_handle(cx), cx);
        self.open_record = Some(id);
        cx.notify();
    }

    /// Closes the dialog, keeping what was written in the note.
    pub(super) fn close_record(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.open_record.take() {
            let note = self.record_note.read(cx).text().to_owned();
            if let Err(err) = self.store.set_focus_note(id, &note) {
                desktop_common::logging::log_error("focus_ui", format!("failed to save the focus note: {err:#}"));
            }
            self.pending_focus = Some(PendingFocus::View);
            self.reload(cx);
        }
        cx.notify();
    }

    /// The original's Focus Record dialog: the record, its time and length, and a note.
    pub(super) fn render_record_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.open_record?;
        let session = self.sessions.iter().find(|s| s.id == id)?;
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let start = session.start_local();
        let when = format!(
            "{}, {} - {}",
            crate::task_rows::short_date(start.date_naive(), strings),
            crate::calendar::format_time(start, strings, cx),
            crate::calendar::format_time(session.end_local(), strings, cx)
        );
        Some(
            Modal::new("focus-record-modal")
                .width(px(420.))
                .padding(px(16.))
                .gap(px(8.))
                .on_dismiss(cx.listener(|this, _, _, cx| this.close_record(cx)))
                .child(
                    h_flex()
                        .child(div().flex_1().child(Label::new(strings.focus_record).weight(FontWeight::SEMIBOLD)))
                        .child(
                            IconButton::new("close-record", IconName::X)
                                .tooltip(Tooltip::text(strings.close))
                                .on_click(cx.listener(|this, _, _, cx| this.close_record(cx))),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Icon::new(if session.stopwatch { IconName::Timer } else { IconName::CircleDot }).color(Color::Accent),
                        )
                        .child(Label::new(session.title.clone()).truncate()),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(IconName::CalendarDays).size(IconSize::Small).color(Color::Muted))
                        .child(Label::new(when).size(LabelSize::Small).color(Color::Muted)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(IconName::Clock).size(IconSize::Small).color(Color::Muted))
                        .child(Label::new(format_minutes(session.minutes(), strings)).size(LabelSize::Small).color(Color::Muted)),
                )
                .child(
                    div()
                        .min_h(px(110.))
                        .p_2()
                        .rounded(px(6.))
                        .bg(theme.colors().surface_background)
                        .child(self.record_note.clone()),
                )
                .child(
                    h_flex().justify_end().child(
                        div()
                            .when(cfg!(test), |el| {
                                el.debug_selector(|| "delete-open-record".to_owned())
                            })
                            .child(
                                IconButton::new("delete-open-record", IconName::Trash)
                                    .icon_color(Color::Muted)
                                    .hover_icon_color(Color::Error)
                                    .tooltip(Tooltip::text(strings.delete))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.request_delete_record(id, window, cx);
                                    })),
                            ),
                    ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn request_delete_record(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let desc = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| {
                let title = if s.title.trim().is_empty() {
                    strings.focus
                } else {
                    s.title.trim()
                };
                format!("{title} · {}", format_minutes(s.minutes(), strings))
            })
            .unwrap_or_else(|| strings.focus.to_owned());
        self.confirm_delete_record = Some((id, desc));
        self.pending_focus = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    pub(super) fn cancel_delete_record(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if self.confirm_delete_record.take().is_none() {
            return;
        }
        let target = if self.open_record.is_some() {
            PendingFocus::RecordNote
        } else {
            PendingFocus::View
        };
        if let Some(window) = window {
            let handle = match target {
                PendingFocus::RecordNote => self.record_note.focus_handle(cx),
                PendingFocus::View => self.focus_handle.clone(),
            };
            window.focus(&handle, cx);
            self.pending_focus = None;
        } else {
            self.pending_focus = Some(target);
        }
        cx.notify();
    }

    pub(super) fn render_confirm_delete_record(&self, id: Id, desc: String, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        div()
            .absolute()
            .inset_0()
            .when(cfg!(test), |el| {
                el.debug_selector(|| "focus-record-delete-modal".to_owned())
            })
            .child(
                ConfirmDialog::new(
                    "focus-record-delete",
                    strings.confirm_delete_focus_record,
                    strings.cancel,
                    strings.delete,
                )
                    .width(px(360.))
                    .description(desc)
                    .destructive(true)
                    .on_cancel(cx.listener(|this, _, window, cx| {
                        this.cancel_delete_record(Some(window), cx);
                    }))
                    .on_confirm(cx.listener(move |this, _, window, cx| {
                        this.confirm_delete_record = None;
                        this.open_record = None;
                        this.pending_focus = None;
                        window.focus(&this.focus_handle, cx);
                        this.delete_record(id, cx);
                    })),
            )
            .into_any_element()
    }

    pub(super) fn delete_record(&mut self, id: fern_core::Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.delete_focus_session(id) {
            desktop_common::logging::log_error("focus_ui", format!("failed to delete the focus record: {err:#}"));
        }
        self.reload(cx);
        desktop_common::data_version::data_changed_scoped(
            cx.entity_id(),
            desktop_common::data_version::DataScope::FOCUS,
            cx,
        );
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        let name = format!("fern-focus-{}.csv", Local::now().format("%Y-%m-%d"));
        let csv = fern_core::focus::export_csv(&self.sessions).into_bytes();
        crate::export::save_as(name, csv, cx, |this, result, cx| {
            if let Some(message) = crate::export::outcome(result, i18n::t(cx)) {
                this.message = Some(message);
            }
            cx.notify();
        });
        self.set_record_menu(None, cx);
    }

    /// A filter's Custom choice: Start and End days for Date, Min and Max minutes for Duration.
    pub(super) fn open_batch_custom(&mut self, which: u8, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let (first, second) = if which == 0 { (strings.summary_start, strings.summary_end) } else { (strings.batch_min, strings.batch_max) };
        let field = |placeholder: &'static str, cx: &mut Context<Self>| {
            let input = cx.new(|cx| TextInput::new(Mode::SingleLine, placeholder, cx));
            cx.subscribe(&input, |this, _, _: &crate::text_input::Submitted, cx| this.confirm_batch_custom(cx)).detach();
            cx.subscribe(&input, |this, _, _: &crate::text_input::Cancelled, cx| {
                if this.batch_custom.take().is_some() {
                    this.pending_focus = Some(PendingFocus::View);
                    cx.notify();
                }
            })
            .detach();
            input
        };
        let (first, second) = (field(first, cx), field(second, cx));
        window.focus(&first.focus_handle(cx), cx);
        self.batch_menu = None;
        self.batch_custom = Some((which, first, second));
        cx.notify();
    }

    /// Confirm (or OK): applies the Custom fields when they read as a range; otherwise they stay
    /// open.
    fn confirm_batch_custom(&mut self, cx: &mut Context<Self>) {
        let Some((which, first, second)) = self.batch_custom.clone() else { return };
        let (first, second) = (first.read(cx).text().to_owned(), second.read(cx).text().to_owned());
        if which == 0 {
            let today = Local::now().date_naive();
            let strings = i18n::t(cx);
            let parse = |text: &str| crate::calendar::parse_date(text, today, strings, cx);
            let (Some(start), Some(end)) = (parse(&first), parse(&second)) else { return };
            self.batch_filter.date = 7;
            self.batch_filter.days = Some((start.min(end), start.max(end)));
        } else {
            let Some(range) = fern_core::focus::RecordFilter::parse_minutes(&first, &second) else { return };
            self.batch_filter.minutes = Some(range);
            self.batch_filter.under = None;
        }
        self.batch_custom = None;
        self.pending_focus = Some(PendingFocus::View);
        cx.notify();
    }

    fn render_batch_custom(&self, which: u8, first: Entity<TextInput>, second: Entity<TextInput>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let field = |input: Entity<TextInput>| h_flex().flex_1().h(px(28.)).px_2().rounded(px(6.)).border_1().border_color(theme.colors().border).child(input);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        let fields = if which == 0 {
            v_flex().gap_2().child(field(first)).child(field(second))
        } else {
            v_flex()
                .gap_2()
                .child(Label::new(strings.batch_minutes_hint).size(LabelSize::XSmall).color(Color::Muted))
                .child(h_flex().gap_1().child(field(first)).child(Label::new("–").color(Color::Muted)).child(field(second)))
        };
        PopoverAttachment::new(
            Placement::BelowLeft,
            popover(cx)
                .w(px(if which == 0 { 190. } else { 230. }))
                .p_2()
                .gap_2()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.batch_custom = None;
                    this.pending_focus = Some(PendingFocus::View);
                    cx.notify();
                }))
                .child(fields)
                .child(
                    h_flex()
                        .gap_2()
                        .child(button("batch-custom-cancel", strings.cancel, false).on_click(cx.listener(|this, _, window, cx| {
                            this.batch_custom = None;
                            this.pending_focus = None;
                            window.focus(&this.focus_handle, cx);
                            cx.notify();
                        })))
                        .child(
                            button("batch-custom-ok", if which == 0 { strings.confirm } else { strings.ok }, true)
                                .on_click(cx.listener(|this, _, _, cx| this.confirm_batch_custom(cx))),
                        ),
                )
                .into_any_element(),
        )
        .into_any_element()
    }

    /// The original's Batch Management: the records by day, each and each day with a checkbox,
    /// "Select All", and Delete for the ticked ones.
    /// Batch Management or Add Timer, as the original shows them: in the middle of the view
    /// over a dimmed background.
    pub(super) fn render_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(draft) = self.timer_draft.as_ref() {
            Some(self.render_timer_draft(draft, cx))
        } else if self.record_menu == Some(RecordMenu::Batch) {
            Some(self.render_batch(cx))
        } else {
            None
        }
    }

    fn render_batch(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let today = chrono::Local::now().date_naive();
        let filter = self.batch_filter;
        let shown: Vec<&FocusSession> =
            self.sessions.iter().filter(|s| filter.matches(s, today, crate::calendar::week_start(cx))).collect();
        let mut days: Vec<(chrono::NaiveDate, Vec<&FocusSession>)> = Vec::new();
        for session in shown.iter().copied() {
            let day = session.start_local().date_naive();
            match days.last_mut() {
                Some((d, list)) if *d == day => list.push(session),
                _ => days.push((day, vec![session])),
            }
        }
        let all: Vec<Id> = shown.iter().map(|s| s.id).collect();
        // The original's Date, Duration and Linked Status filters, each a dropdown.
        const LENGTHS: [Option<i64>; 5] = [None, Some(10), Some(25), Some(45), Some(60)];
        const LINKED: [Option<bool>; 3] = [None, Some(true), Some(false)];
        let current = [
            filter.date as usize,
            if filter.minutes.is_some() { LENGTHS.len() } else { LENGTHS.iter().position(|l| *l == filter.under).unwrap_or(0) },
            LINKED.iter().position(|l| *l == filter.linked).unwrap_or(0),
        ];
        let choices: [&[&'static str]; 3] = [&strings.batch_dates, &strings.batch_lengths, &strings.batch_linked];
        let choices_len = move |which: u8| choices[which as usize].len();
        let theme = cx.theme().clone();
        let filters = h_flex().gap_1().children((0..3u8).map(|which| {
            let open = self.batch_menu == Some(which);
            let label: SharedString = match (which, filter.days, filter.minutes) {
                (0, Some((first, last)), _) if filter.date == 7 => {
                    format!("{} - {}", crate::task_rows::short_date(first, strings), crate::task_rows::short_date(last, strings)).into()
                }
                (1, _, Some((least, most))) => format!("{}–{}", format_minutes(least, strings), format_minutes(most, strings)).into(),
                _ if current[which as usize] == 0 => strings.batch_filters[which as usize].into(),
                _ => choices[which as usize][current[which as usize]].into(),
            };
            let custom = self.batch_custom.clone().filter(|(w, ..)| *w == which).map(|(_, first, second)| self.render_batch_custom(which, first, second, cx));
            div()
                .relative()
                .child(
                    h_flex()
                        .id(SharedString::from(format!("batch-filter-{which}")))
                        .px_2()
                        .h(px(22.))
                        .gap_1()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.colors().border)
                        .cursor_pointer()
                        .child(Label::new(label).size(LabelSize::XSmall).color(if current[which as usize] == 0 { Color::Muted } else { Color::Accent }))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.batch_menu = if this.batch_menu == Some(which) { None } else { Some(which) };
                            cx.notify();
                        })),
                )
                .when(open, |el| {
                    el.child(PopoverAttachment::new(
                        Placement::BelowLeft,
                        popover(cx)
                            .min_w(px(170.))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.batch_menu = None;
                                cx.notify();
                            }))
                            .children(choices[which as usize].iter().enumerate().map(|(i, choice)| {
                                let selected = current[which as usize] == i;
                                ListItem::new(SharedString::from(format!("batch-filter-{which}-{i}")))
                                    .child(Label::new(*choice).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }))
                                    .when(selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        match which {
                                            // The last choice of Date and Duration is Custom.
                                            0 | 1 if i == choices_len(which) - 1 => {
                                                this.open_batch_custom(which, window, cx);
                                                return;
                                            }
                                            0 => {
                                                this.batch_filter.date = i as u8;
                                                this.batch_filter.days = None;
                                            }
                                            1 => {
                                                this.batch_filter.under = LENGTHS[i];
                                                this.batch_filter.minutes = None;
                                            }
                                            _ => this.batch_filter.linked = LINKED[i],
                                        }
                                        this.batch_menu = None;
                                        cx.notify();
                                    }))
                            }))
                            .into_any_element(),
                    ))
                })
                .children(custom)
        }));
        let chosen = self.batch.len();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (day, sessions) in days {
            let ids: Vec<Id> = sessions.iter().map(|s| s.id).collect();
            let whole = ids.iter().all(|id| self.batch.contains(id));
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("batch-day-{day}")))
                    .gap_2()
                    .py_1()
                    .cursor_pointer()
                    .child(Checkbox::new(SharedString::from(format!("batch-day-box-{day}"))).size(px(14.)).toggle_state(whole))
                    .child(Label::new(crate::task_rows::short_date(day, strings)).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        for id in &ids {
                            if whole {
                                this.batch.remove(id);
                            } else {
                                this.batch.insert(*id);
                            }
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            for session in sessions {
                let id = session.id;
                let on = self.batch.contains(&id);
                let span = format!(
                    "{} - {}",
                    crate::calendar::format_time(session.start_local().time(), strings, cx),
                    crate::calendar::format_time(session.end_local().time(), strings, cx)
                );
                rows.push(
                    h_flex()
                        .id(SharedString::from(format!("batch-{id}")))
                        .gap_2()
                        .py_1()
                        .items_start()
                        .cursor_pointer()
                        .child(Checkbox::new(SharedString::from(format!("batch-box-{id}"))).size(px(14.)).toggle_state(on))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(Label::new(span).size(LabelSize::XSmall).color(Color::Muted))
                                .child(Label::new(session.title.clone()).size(LabelSize::Small).truncate()),
                        )
                        .child(Label::new(format_minutes(session.minutes(), strings)).size(LabelSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.batch.remove(&id) {
                                this.batch.insert(id);
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        // A dialog in the middle of the window, closed with its ×, as in the original.
        Modal::new("focus-modal")
            .width(px(440.))
            .max_height(px(560.))
            .padding(px(16.))
            .gap(px(8.))
            .child(
                h_flex()
                    .child(
                        div().flex_1().child(
                            Label::new(if chosen == 0 {
                                strings.batch_management.to_owned()
                            } else {
                                strings.n_selected.replace("{n}", &chosen.to_string())
                            })
                            .weight(FontWeight::SEMIBOLD),
                        ),
                    )
                    .child(
                        IconButton::new("batch-close", IconName::X)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, _, cx| this.set_record_menu(None, cx))),
                    ),
            )
            .child(filters)
            .child(v_scroll("batch-list-scroll", v_flex().id("batch-list").children(rows)).flex_1().min_h_0())
            .child(
                h_flex()
                    .child(
                        div()
                            .id("batch-select-all")
                            .cursor_pointer()
                            .child(Label::new(strings.select_all).size(LabelSize::Small).color(Color::Accent))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.batch = all.iter().copied().collect();
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .when(chosen > 0, |el| {
                        el.child(
                            div()
                                .id("batch-delete")
                                .cursor_pointer()
                                .child(Label::new(strings.delete).size(LabelSize::Small).color(Color::Error))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirm_batch_delete = true;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    /// "+" in the Pomodoro header, or Edit in a timer's menu: the original's "Add Timer".
    pub(super) fn open_timer(&mut self, timer: Option<fern_core::focus::FocusTimer>, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let new = timer.is_none();
        // AddTimerWindow::GetDefaultTimer (RVA 0x000c6170) inherits PomoDuration.
        let timer = timer.unwrap_or_else(|| {
            let mut timer = fern_core::focus::FocusTimer::new("");
            timer.minutes = self.minutes(POMO_SETTING);
            timer
        });
        let name = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.timer_name, cx);
            input.set_text(timer.name.clone(), cx);
            input
        });
        let minutes = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "", cx);
            input.set_text(timer.pomo_minutes().to_string(), cx);
            input
        });
        for input in [&name, &minutes] {
            cx.subscribe(input, |this, _, _: &crate::text_input::Submitted, cx| this.save_timer(cx)).detach();
        }
        window.focus(&name.focus_handle(cx), cx);
        self.timer_menu = None;
        let icon_backup = (timer.icon.clone(), timer.color.clone());
        self.timer_draft = Some(TimerDraft { timer, name, minutes, new, icon_tab: None, icon_backup });
        cx.notify();
    }

    fn save_timer(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.timer_draft.take() else { return };
        // A Pomo needs minutes that read; the dialog stays open until they do.
        let minutes = fern_core::focus::FocusTimer::parse_minutes(draft.minutes.read(cx).text());
        if minutes.is_none() && !draft.timer.stopwatch {
            self.timer_draft = Some(draft);
            return;
        }
        let mut timer = draft.timer;
        timer.minutes = minutes.unwrap_or(timer.minutes);
        timer.name = draft.name.read(cx).text().trim().to_owned();
        if let Err(err) = self.store.save_focus_timer(&timer) {
            desktop_common::logging::log_error("focus_ui", format!("failed to save the timer: {err:#}"));
            if !timer.name.is_empty() {
                return;
            }
        }
        self.reload(cx);
    }

    /// The original's "Add Timer" dialog, by its measures: the title and ×, a 35 px icon beside
    /// the name, the bold "Timer Mode", Pomo with its minutes in a field and Stopwatch, then Close
    /// and Save on the right.
    fn render_timer_draft(&self, draft: &TimerDraft, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let timer = &draft.timer;
        let icons = draft.icon_tab.map(|tab| self.render_timer_icons(timer, tab, cx));
        let avatar = icon_avatar(timer.icon.as_deref(), timer.color.as_deref(), draft.name.read(cx).text(), px(35.), cx);
        let change = |f: fn(&mut fern_core::focus::FocusTimer)| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let Some(draft) = this.timer_draft.as_mut() {
                    f(&mut draft.timer);
                }
                cx.notify();
            })
        };
        let radio = |on: bool| {
            div()
                .size(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(if on { theme.colors().accent } else { theme.colors().text_muted })
                .when(on, |el| el.child(div().size(px(8.)).rounded_full().bg(theme.colors().accent)))
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .w(px(100.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        let close = cx.listener(|this, _, _, cx| {
            this.timer_draft = None;
            cx.notify();
        });
        Modal::new("focus-modal")
            .width(px(442.))
            .padding(px(20.))
            .gap(px(14.))
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(if draft.new { strings.add_timer } else { strings.edit_timer }).weight(FontWeight::BOLD)))
                    .child(
                        IconButton::new("timer-x", IconName::X)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(close),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        div()
                            .relative()
                            .child(
                                div()
                                    .id("timer-icon-button")
                                    .cursor_pointer()
                                    .child(avatar)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(draft) = this.timer_draft.as_mut() {
                                            draft.icon_tab = if draft.icon_tab.is_some() { None } else { Some(0) };
                                            draft.icon_backup = (draft.timer.icon.clone(), draft.timer.color.clone());
                                        }
                                        cx.notify();
                                    })),
                            )
                            // Like the original's, the picker hangs under the icon.
                            .children(icons.map(|icons| PopoverAttachment::new(Placement::BelowLeft, icons))),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .h(px(30.))
                            .px_3()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .child(draft.name.clone()),
                    ),
            )
            .child(div().pt_1().child(Label::new(strings.timer_mode).weight(FontWeight::SEMIBOLD)))
            .child(
                h_flex()
                    .h(px(33.))
                    .gap_3()
                    .child(
                        h_flex()
                            .id("timer-pomo")
                            .gap_2()
                            .cursor_pointer()
                            .child(radio(!timer.stopwatch))
                            .child(Label::new(strings.focus_modes[0]))
                            .on_click(change(|t| t.stopwatch = false)),
                    )
                    .child(
                        h_flex()
                            .w(px(79.))
                            .h(px(33.))
                            .px_2()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(theme.colors().element_hover)
                            .child(draft.minutes.clone()),
                    )
                    .child(Label::new(strings.timer_mins))
                    .when(!timer.stopwatch, |row| {
                        let current_mins = draft.minutes.read(cx).text().trim().parse::<i64>().ok();
                        row.children([15_i64, 25, 45, 60].into_iter().map(|mins| {
                            let active = current_mins == Some(mins);
                            div()
                                .id(SharedString::from(format!("timer-preset-{mins}")))
                                .px_2()
                                .py_0p5()
                                .rounded(px(5.))
                                .cursor_pointer()
                                .bg(if active {
                                    theme.colors().accent.opacity(0.18)
                                } else {
                                    theme.colors().element_hover
                                })
                                .hover(|s| s.bg(theme.colors().accent.opacity(0.24)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(draft) = this.timer_draft.as_ref() {
                                        draft.minutes.update(cx, |input, cx| {
                                            input.set_text(mins.to_string(), cx);
                                        });
                                    }
                                    cx.notify();
                                }))
                                .child(
                                    Label::new(mins.to_string())
                                        .size(LabelSize::XSmall)
                                        .color(if active { Color::Accent } else { Color::Muted }),
                                )
                        }))
                    }),
            )
            .child(
                h_flex()
                    .id("timer-stopwatch")
                    .h(px(33.))
                    .gap_2()
                    .cursor_pointer()
                    .child(radio(timer.stopwatch))
                    .child(Label::new(strings.focus_modes[1]))
                    .on_click(change(|t| t.stopwatch = true)),
            )
            .child(
                h_flex()
                    .pt_3()
                    .gap(px(8.))
                    .justify_end()
                    .child(button("timer-close", strings.close, false).on_click(cx.listener(|this, _, _, cx| {
                        this.timer_draft = None;
                        cx.notify();
                    })))
                    .child(button("timer-save", strings.save, true).on_click(cx.listener(|this, _, _, cx| this.save_timer(cx)))),
            )
            .into_any_element()
    }

    /// The timer's icon picker, laid out as the original's: a round button for the icons and an
    /// "A" for the name's initial on top, the icons as a grid of colored circles (emoji stand in
    /// for the original's own artwork, each on a color of the palette, which picking it takes too),
    /// with the picked one ticked, then Cancel (back to the icon before) and OK. The "A" choice
    /// shows the palette for its background.
    fn render_timer_icons(&self, timer: &fern_core::focus::FocusTimer, tab: usize, cx: &mut Context<Self>) -> AnyElement {
        const COLORS: [&str; 8] = crate::lists_panel::LIST_COLORS;
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let edit = |f: Box<dyn Fn(&mut TimerDraft)>| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let Some(draft) = this.timer_draft.as_mut() {
                    f(draft);
                }
                cx.notify();
            })
        };
        let ring = |on: bool, el: Div| {
            div()
                .p(px(3.))
                .rounded_full()
                .border_2()
                .border_color(if on { theme.colors().accent.opacity(0.6) } else { gpui::transparent_black() })
                .child(el)
        };
        let name = timer.name.clone();
        let top = h_flex()
            .gap_2()
            .child(
                div()
                    .id("timer-icons-tab")
                    .cursor_pointer()
                    .child(ring(tab == 0, icon_avatar(timer.icon.as_deref().or(Some(HABIT_EMOJI[0])), timer.color.as_deref(), &name, px(32.), cx)))
                    .on_click(edit(Box::new(|d| d.icon_tab = Some(0)))),
            )
            .child(
                div()
                    .id("timer-text-tab")
                    .cursor_pointer()
                    .child(ring(
                        tab == 1,
                        div()
                            .size(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(theme.colors().accent.opacity(0.25))
                            .child(Label::new("A").color(Color::Accent)),
                    ))
                    .on_click(edit(Box::new(|d| {
                        d.icon_tab = Some(1);
                        d.timer.icon = None;
                    }))),
            );
        let body: AnyElement = if tab == 0 {
            v_scroll(
                "timer-icon-grid-scroll",
                div()
                    .id("timer-icon-grid")
                    .child(div().flex().flex_wrap().gap_y(px(16.)).children(HABIT_EMOJI.iter().enumerate().map(|(i, emoji)| {
                    let color = COLORS[i % COLORS.len()];
                    let selected = timer.icon.as_deref() == Some(*emoji);
                    div()
                        .id(SharedString::from(format!("timer-emoji-{emoji}")))
                        .w(px(46.))
                        .flex()
                        .justify_center()
                        .cursor_pointer()
                        .child(
                            div()
                                .relative()
                                .size(px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(crate::lists_panel::parse_color(Some(color)).unwrap_or(theme.colors().accent).opacity(0.45))
                                .text_size(px(16.))
                                .child(*emoji)
                                .when(selected, |el| {
                                    el.child(
                                        div()
                                            .absolute()
                                            .right(px(-4.))
                                            .bottom(px(-4.))
                                            .size(px(14.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_full()
                                            .bg(theme.colors().accent)
                                            .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::IconOnAccent)),
                                    )
                                }),
                        )
                        .on_click(edit(Box::new(move |d| {
                            d.timer.icon = Some((*emoji).to_owned());
                            d.timer.color = Some(color.to_owned());
                        })))
                }))),
            )
            .h(px(158.))
            .compact()
            .into_any_element()
        } else {
            h_flex()
                .h(px(40.))
                .gap_2()
                .children(std::iter::once(None).chain(COLORS.iter().map(|c| Some(*c))).map(|color| {
                    let selected = timer.color.as_deref() == color;
                    let fill = crate::lists_panel::parse_color(color).unwrap_or(theme.colors().accent);
                    div()
                        .id(SharedString::from(format!("timer-color-{}", color.unwrap_or("none"))))
                        .size(px(24.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(fill.opacity(if color.is_none() { 0.35 } else { 1. }))
                        .when(selected, |el| el.border_2().border_color(theme.colors().text))
                        .on_click(edit(Box::new(move |d| d.timer.color = color.map(str::to_owned))))
                }))
                .into_any_element()
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .w(px(100.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        popover(cx)
            .w(px(404.))
            .px(px(16.))
            .py(px(20.))
            .gap(px(16.))
            .child(top)
            .child(body)
            .child(
                h_flex()
                    .gap(px(12.))
                    .justify_end()
                    .child(button("timer-icon-cancel", strings.cancel, false).on_click(edit(Box::new(|d| {
                        (d.timer.icon, d.timer.color) = d.icon_backup.clone();
                        d.icon_tab = None;
                    }))))
                    .child(button("timer-icon-ok", strings.ok, true).on_click(edit(Box::new(|d| d.icon_tab = None)))),
            )
            .into_any_element()
    }

    pub(super) fn render_record_menu(&self, menu: RecordMenu, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let close = cx.listener(|this, _, _, cx| this.set_record_menu(None, cx));
        match menu {
            // As the original's: 163 px wide (wider when a label needs it), a row every 34 px.
            RecordMenu::HeaderMore => popover(cx)
                .min_w(px(163.))
                .on_mouse_down_out(close)
                .child(
                    ListItem::new("focus-statistics")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::ChartPie).color(Color::Muted))
                        .child(Label::new(strings.statistics))
                        .on_click(cx.listener(|this, _, _, cx| this.open_stats(cx))),
                )
                .child(
                    ListItem::new("focus-settings")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::Settings2).color(Color::Muted))
                        .child(Label::new(strings.focus_settings))
                        .on_click(cx.listener(|this, _, _, cx| this.set_record_menu(Some(RecordMenu::Settings), cx))),
                )
                .into_any_element(),
            RecordMenu::Settings => {
                let row = |id: &'static str, label: &'static str, setting: (&'static str, i64, i64, i64), cx: &mut Context<Self>| {
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Label::new(label).size(LabelSize::Small)))
                        .child(
                            IconButton::new(SharedString::from(format!("{id}-less")), IconName::Minus)
                                .tooltip(Tooltip::text(format!("-1 {}", strings.duration_min_short)))
                                .on_click(cx.listener(move |this, _, _, cx| this.set_minutes(setting, -1, cx))),
                        )
                        .child(div().min_w(px(28.)).flex().justify_center().child(Label::new(self.minutes(setting).to_string())))
                        .child(
                            IconButton::new(SharedString::from(format!("{id}-more")), IconName::Plus)
                                .tooltip(Tooltip::text(format!("+1 {}", strings.duration_min_short)))
                                .on_click(cx.listener(move |this, _, _, cx| this.set_minutes(setting, 1, cx))),
                        )
                        .child(Label::new(strings.minutes).size(LabelSize::XSmall).color(Color::Muted))
                };
                popover(cx)
                    .w(px(340.))
                    .p_3()
                    .gap_2()
                    .on_mouse_down_out(close)
                    .child(Label::new(strings.focus_settings).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(strings.focus_timer_option).size(LabelSize::Small).color(Color::Muted))
                    .child(row("pomo", strings.focus_pomo_duration, POMO_SETTING, cx))
                    .child(row("short-break", strings.focus_short_break, SHORT_BREAK_SETTING, cx))
                    .child(row("long-break", strings.focus_long_break, LONG_BREAK_SETTING, cx))
                    .child({
                        let goal = self.minutes(DAILY_GOAL_SETTING);
                        let value = if goal == 0 {
                            strings.focus_goal_off.to_owned()
                        } else {
                            format_minutes(goal, strings)
                        };
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Label::new(strings.focus_daily_goal).size(LabelSize::Small)))
                            .child(
                                IconButton::new("daily-goal-less", IconName::Minus)
                                    .tooltip(Tooltip::text(format!("-15 {}", strings.duration_min_short)))
                                    .on_click(cx.listener(|this, _, _, cx| this.set_minutes(DAILY_GOAL_SETTING, -15, cx))),
                            )
                            .child(div().min_w(px(44.)).flex().justify_center().child(Label::new(value).size(LabelSize::Small)))
                            .child(
                                IconButton::new("daily-goal-more", IconName::Plus)
                                    .tooltip(Tooltip::text(format!("+15 {}", strings.duration_min_short)))
                                    .on_click(cx.listener(|this, _, _, cx| this.set_minutes(DAILY_GOAL_SETTING, 15, cx))),
                            )
                    })
                    .child({
                        let current = self.white_noise();
                        let label = strings.focus_white_noise_names[current.index()];
                        let hover_bg = cx.theme().colors().ghost_element_hover;
                        h_flex()
                            .id("focus-white-noise-setting")
                            .min_h(px(30.))
                            .px_1()
                            .rounded(px(5.))
                            .gap_2()
                            .hover(move |s| s.bg(hover_bg))
                            .cursor_pointer()
                            .child(div().flex_1().child(Label::new(strings.focus_white_noise).size(LabelSize::Small)))
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(5.))
                                    .bg(if current != desktop_common::sound::WhiteNoise::Off {
                                        cx.theme().colors().accent.opacity(0.15)
                                    } else {
                                        cx.theme().colors().element_hover
                                    })
                                    .child(
                                        Label::new(label).size(LabelSize::XSmall).color(
                                            if current != desktop_common::sound::WhiteNoise::Off {
                                                Color::Accent
                                            } else {
                                                Color::Muted
                                            },
                                        ),
                                    ),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cycle_white_noise(cx);
                            }))
                    })
                    .child(self.render_automation(cx))
                    .into_any_element()
            }
            RecordMenu::Batch => div().into_any_element(),
            // As the original's: 178 px wide (wider when a label needs it), a row every 34 px.
            RecordMenu::More => popover(cx)
                .min_w(px(178.))
                .on_mouse_down_out(close)
                .child(
                    ListItem::new("focus-batch")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::ListChecks).color(Color::Muted))
                        .child(Label::new(strings.batch_management))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.batch.clear();
                            this.batch_filter = Default::default();
                            this.set_record_menu(Some(RecordMenu::Batch), cx)
                        })),
                )
                .child(
                    ListItem::new("focus-export")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::SquareArrowOutUpRight).color(Color::Muted))
                        .child(Label::new(strings.export))
                        .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
                )
                .child(
                    div()
                        .when(cfg!(test), |el| el.debug_selector(|| "focus-delete-all-item".to_owned()))
                        .child(
                            ListItem::new("focus-delete-all")
                                .height(px(34.))
                                .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                .child(Label::new(strings.delete_all).color(Color::Error))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.record_menu = None;
                                    this.confirm_delete_all = true;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
            RecordMenu::Add => {
                let field = |input: Entity<TextInput>| {
                    h_flex().h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().background).child(input)
                };
                popover(cx)
                    .w(px(280.))
                    .p_3()
                    .gap_2()
                    .on_mouse_down_out(close)
                    .child(Label::new(strings.focus_add_record).weight(FontWeight::SEMIBOLD))
                    .child(field(self.add_title.clone()))
                    .child(field(self.add_start.clone()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Label::new(strings.focus_record_minutes).size(LabelSize::Small)))
                            .child(
                                IconButton::new("record-shorter", IconName::Minus)
                                    .tooltip(Tooltip::text(format!("-5 {}", strings.duration_min_short)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.add_minutes = (this.add_minutes - 5).max(5);
                                        cx.notify();
                                    })),
                            )
                            .child(Label::new(format_minutes(self.add_minutes, strings)).size(LabelSize::Small))
                            .child(
                                IconButton::new("record-longer", IconName::Plus)
                                    .tooltip(Tooltip::text(format!("+5 {}", strings.duration_min_short)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.add_minutes = (this.add_minutes + 5).min(12 * 60);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("record-save")
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(theme.colors().accent)
                            .cursor_pointer()
                            .child(Label::new(strings.save).size(LabelSize::Small).color(Color::OnAccent))
                            .on_click(cx.listener(|this, _, _, cx| this.add_record(cx))),
                    )
                    .into_any_element()
            }
        }
    }

    pub(super) fn render_white_noise_button(&self, id: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let noise = self.white_noise();
        let active_noise = noise != desktop_common::sound::WhiteNoise::Off;
        let name = strings.focus_white_noise_names[noise.index()];
        h_flex()
            .id(id)
            .h(px(28.))
            .px_2()
            .gap_1()
            .rounded(px(6.))
            .cursor_pointer()
            .when(active_noise, |el| el.bg(theme.colors().accent.opacity(0.14)))
            .hover(|s| s.bg(theme.colors().ghost_element_hover))
            .tooltip(Tooltip::text(format!("{}: {name}", strings.focus_white_noise)))
            .child(
                Icon::new(IconName::Bell)
                    .size(IconSize::XSmall)
                    .color(if active_noise { Color::Accent } else { Color::Muted }),
            )
            .when(active_noise, |el| {
                el.child(Label::new(name).size(LabelSize::XSmall).color(Color::Accent))
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.cycle_white_noise(cx);
            }))
    }
}
