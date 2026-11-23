use chrono::{Duration, Local};
use desktop_common::text_input::{Mode, Submitted, TextInput};
use fern_core::{Id, Kind, Priority};
use fern_i18n as i18n;
use gpui::{AnyElement, Focusable, FontWeight, Point};
use task_ui::task_detail::StartFocus;
use task_ui::task_rows;
use ui::prelude::*;
use ui::{Button, ButtonVariant, ConfirmDialog, Modal, Placement, PopoverAttachment, popover};

use crate::{MatrixView, OpenTask};

pub(super) enum MatrixNotice {
    SavedElsewhere(SharedString),
    AlreadyInQuadrant,
    DropUnsupported,
    Failed,
}

impl MatrixView {
    pub(super) fn profile_setting_key(&self, base: &str) -> String {
        let first_id = self.profiles.first().map(|p| p.id);
        match self.active_profile_id {
            Some(id) if Some(id) != first_id => format!("{base}_{id}"),
            _ => base.to_owned(),
        }
    }

    pub(super) fn switch_profile(&mut self, id: Id, cx: &mut Context<Self>) {
        self.profile_menu = false;
        self.editing = None;
        self.emoji_picker_open = false;
        if let Err(err) = self.store.set_active_matrix_profile(id) {
            eprintln!("failed to switch matrix profile: {err:#}");
            return;
        }
        self.reload(cx);
    }

    pub(super) fn open_profile_editor(
        &mut self,
        editing: Option<Id>,
        initial: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.profile_menu = false;
        let placeholder = i18n::t(cx).matrix_profile_name;
        let initial = initial.to_owned();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, placeholder, cx).without_cancel();
            if !initial.is_empty() {
                input.set_text(initial, cx);
            }
            input
        });
        cx.subscribe(&input, |this, _, _: &Submitted, cx| {
            this.save_profile_editor(cx);
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.profile_editor = Some((editing, input));
        cx.notify();
    }

    pub(super) fn save_profile_editor(&mut self, cx: &mut Context<Self>) {
        let Some((editing, input)) = self.profile_editor.take() else { return };
        let name = input.read(cx).text().trim().to_owned();
        if name.is_empty() {
            cx.notify();
            return;
        }
        match editing {
            Some(id) => {
                if let Err(err) = self.store.rename_matrix_profile(id, &name) {
                    eprintln!("failed to rename matrix profile: {err:#}");
                } else {
                    self.text_edit_sync_task = None;
                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                }
            }
            None => match self.store.create_matrix_profile(&name, None) {
                Ok(profile) => {
                    self.editing = None;
                    self.emoji_picker_open = false;
                    let _ = self.store.set_active_matrix_profile(profile.id);
                    self.text_edit_sync_task = None;
                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                }
                Err(err) => eprintln!("failed to create matrix profile: {err:#}"),
            },
        }
        self.reload(cx);
    }

    pub(super) fn delete_profile(&mut self, id: Id, cx: &mut Context<Self>) {
        self.profile_menu = false;
        self.editing = None;
        self.emoji_picker_open = false;
        match self.store.delete_matrix_profile(id) {
            Ok(true) => {
                self.text_edit_sync_task = None;
                desktop_common::data_version::data_changed(cx.entity_id(), cx);
            }
            Ok(false) => {}
            Err(err) => eprintln!("failed to delete matrix profile: {err:#}"),
        }
        self.reload(cx);
    }

    pub(super) fn render_task_context_menu(
        &self,
        id: Id,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
            return div().into_any_element();
        };
        let strings = i18n::t(cx);
        let today = Local::now().date_naive();
        let title_pomo = task.title.clone();
        let title_sw = task.title.clone();
        let is_note = task.kind == Kind::Note;
        let read_only = self.is_task_read_only(id);

        div()
            .id("matrix-task-ctx")
            .absolute()
            .left(pos.x)
            .top(pos.y)
            .occlude()
            .child(
                popover(cx)
                    .min_w(px(190.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.task_menu = None;
                        cx.notify();
                    }))
                    .child(
                        ListItem::new("mctx-open")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::FernTasks).color(Color::Muted))
                            .child(Label::new(strings.matrix_open_in_tasks))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.task_menu = None;
                                this.close_detail_card(cx);
                                cx.emit(OpenTask(id));
                                cx.notify();
                            })),
                    )
                    .when(!read_only && !is_note, |el| {
                        el.child(
                            h_flex().px_2().py_1().gap_1().children(
                                [
                                    (Priority::High, strings.priority_high),
                                    (Priority::Medium, strings.priority_medium),
                                    (Priority::Low, strings.priority_low),
                                    (Priority::None, strings.priority_none),
                                ]
                                .map(|(p, tip)| {
                                    let active = task.priority == p;
                                    IconButton::new(SharedString::from(format!("mctx-p-{}", p.to_i64())), IconName::FlagFilled)
                                        .size(px(26.))
                                        .icon_size(IconSize::Small)
                                        .icon_color(task_rows::priority_color(p, cx.theme()).map(Color::Custom).unwrap_or(Color::Muted))
                                        .toggle_state(active)
                                        .tooltip(Tooltip::text(tip))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.task_menu = None;
                                            let _ = this.store.set_priority(id, p);
                                            this.text_edit_sync_task = None;
                                            desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                            this.reload(cx);
                                        }))
                                }),
                            ),
                        )
                    })
                    .when(!read_only, |el| {
                        el.child(
                            ListItem::new("mctx-today")
                                .height(px(30.))
                                .start_slot(Icon::new(IconName::CalendarDays).color(Color::Muted))
                                .child(Label::new(strings.today))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.task_menu = None;
                                    let _ = this.store.move_dates(id, today);
                                    this.text_edit_sync_task = None;
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload(cx);
                                })),
                        )
                        .child(
                            ListItem::new("mctx-tomorrow")
                                .height(px(30.))
                                .start_slot(Icon::new(IconName::Sunrise).color(Color::Muted))
                                .child(Label::new(strings.tomorrow))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.task_menu = None;
                                    let _ = this.store.move_dates(id, today + Duration::days(1));
                                    this.text_edit_sync_task = None;
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload(cx);
                                })),
                        )
                        .child(
                            ListItem::new("mctx-clear-date")
                                .height(px(30.))
                                .start_slot(Icon::new(IconName::CalendarX).color(Color::Muted))
                                .child(Label::new(strings.clear))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.task_menu = None;
                                    let _ = this.store.set_due(id, None, None);
                                    this.text_edit_sync_task = None;
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload(cx);
                                })),
                        )
                        .child(
                            ListItem::new("mctx-pomo")
                                .height(px(30.))
                                .start_slot(Icon::new(IconName::Timer).color(Color::Muted))
                                .child(Label::new(strings.start_pomo))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.task_menu = None;
                                    cx.emit(StartFocus(id, title_pomo.clone(), false));
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("mctx-sw")
                                .height(px(30.))
                                .start_slot(Icon::new(IconName::Clock).color(Color::Muted))
                                .child(Label::new(strings.start_stopwatch))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.task_menu = None;
                                    cx.emit(StartFocus(id, title_sw.clone(), true));
                                    cx.notify();
                                })),
                        )
                        .child(div().my_1().child(Divider::horizontal()))
                        .child(
                            ListItem::new("mctx-delete")
                                .height(px(30.))
                                .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                .child(Label::new(strings.delete).color(Color::Error))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.task_menu = None;
                                    this.delete_task(id, cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_profile_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let active_name = self
            .active_profile_id
            .and_then(|id| self.profiles.iter().find(|p| p.id == id))
            .or_else(|| self.profiles.first())
            .map(|p| if p.name == "Default" { strings.matrix_default_profile.to_owned() } else { p.name.clone() })
            .unwrap_or_else(|| strings.matrix_default_profile.to_owned());
        let can_delete = self.profiles.len() > 1;
        let menu = self.profile_menu.then(|| {
            PopoverAttachment::new(
                Placement::BelowLeft,
                popover(cx)
                    .min_w(px(220.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.profile_menu = false;
                        cx.notify();
                    }))
                    .children(self.profiles.iter().map(|profile| {
                        let id = profile.id;
                        let active = self.active_profile_id == Some(id);
                        let display_name = if profile.name == "Default" {
                            strings.matrix_default_profile.to_owned()
                        } else {
                            profile.name.clone()
                        };
                        let raw_name = profile.name.clone();
                        ListItem::new(SharedString::from(format!("matrix-profile-{id}")))
                            .height(px(34.))
                            .start_slot(Icon::new(IconName::LayoutGrid).size(IconSize::XSmall).color(if active { Color::Accent } else { Color::Muted }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Label::new(display_name.clone()).color(if active { Color::Accent } else { Color::Default }).truncate()),
                            )
                            .end_slot(
                                h_flex()
                                    .gap_1()
                                    .when(active, |el| el.child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                                    .child(
                                        IconButton::new(SharedString::from(format!("matrix-profile-rename-{id}")), IconName::Pencil)
                                            .size(px(20.))
                                            .icon_size(IconSize::XSmall)
                                            .icon_color(Color::Muted)
                                            .tooltip(Tooltip::text(strings.edit))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.open_profile_editor(Some(id), &raw_name, window, cx);
                                            })),
                                    )
                                    .when(can_delete, |el| {
                                        let del_name = display_name.clone();
                                        el.child(
                                            IconButton::new(SharedString::from(format!("matrix-profile-delete-{id}")), IconName::Trash)
                                                .size(px(20.))
                                                .icon_size(IconSize::XSmall)
                                                .icon_color(Color::Muted)
                                                .hover_icon_color(theme.status().error)
                                                .tooltip(Tooltip::text(strings.delete))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.profile_menu = false;
                                                    this.confirm_delete_profile = Some((id, del_name.clone()));
                                                    cx.notify();
                                                })),
                                        )
                                    }),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.switch_profile(id, cx)))
                    }))
                    .child(div().my_1().child(Divider::horizontal()))
                    .child(
                        ListItem::new("matrix-profile-new")
                            .height(px(34.))
                            .start_slot(Icon::new(IconName::Plus).color(Color::Accent))
                            .child(Label::new(strings.matrix_new_profile).color(Color::Accent))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_profile_editor(None, "", window, cx);
                            })),
                    ),
            )
        });
        div()
            .relative()
            .child(
                h_flex()
                    .id("matrix-profile-switcher")
                    .h(px(28.))
                    .px_2p5()
                    .gap_1()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.colors().border)
                    .bg(theme.colors().surface_background)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().ghost_element_hover))
                    .child(Icon::new(IconName::LayoutGrid).size(IconSize::XSmall).color(Color::Muted))
                    .child(Label::new(active_name).size(LabelSize::Small))
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.profile_menu = !this.profile_menu;
                        cx.notify();
                    })),
            )
            .children(menu)
            .into_any_element()
    }

    pub(super) fn render_header_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        div()
            .debug_selector(|| "matrix-header-menu-trigger".to_owned())
            .relative()
            .child(
                IconButton::new("matrix-more", IconName::Ellipsis)
                    .tooltip(Tooltip::text(strings.more))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.header_menu = !this.header_menu;
                        window.focus(&this.focus, cx);
                        cx.notify();
                    })),
            )
            .when(self.header_menu, |el| {
                el.child(PopoverAttachment::new(
                    Placement::BelowRight,
                    popover(cx)
                        .min_w(px(230.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.header_menu = false;
                            cx.notify();
                        }))
                        .child(
                            ListItem::new("matrix-show-completed")
                                .height(px(34.))
                                .start_slot(Icon::new(IconName::SquareCheck).color(Color::Icon))
                                .child(Label::new(if self.show_completed {
                                    strings.hide_completed
                                } else {
                                    strings.show_completed
                                }))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.header_menu = false;
                                    let value = if this.show_completed { "0" } else { "1" };
                                    if let Err(err) = this.store.set_setting("matrix_show_completed", value) {
                                        eprintln!("failed to save the setting: {err:#}");
                                    }
                                    this.reload(cx);
                                })),
                        )
                        .child(
                            ListItem::new("matrix-show-subtasks")
                                .height(px(34.))
                                .start_slot(Icon::new(IconName::ListTree).color(Color::Icon))
                                .child(Label::new(if self.show_subtasks {
                                    strings.matrix_hide_subtasks
                                } else {
                                    strings.matrix_show_subtasks
                                }))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.header_menu = false;
                                    let value = if this.show_subtasks { "0" } else { "1" };
                                    if let Err(err) = this.store.set_setting("matrix_show_subtasks", value) {
                                        eprintln!("failed to save the setting: {err:#}");
                                    }
                                    this.reload(cx);
                                })),
                        )
                        .child(
                            ListItem::new("matrix-apply-template")
                                .height(px(34.))
                                .start_slot(Icon::new(IconName::LayoutTemplate).color(Color::Icon))
                                .child(Label::new(strings.matrix_apply_date_template))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.header_menu = false;
                                    let res = match this.active_profile_id {
                                        Some(pid) => this.store.apply_matrix_profile_date_template(pid).map(|_| ()),
                                        None => this.store.apply_matrix_date_template().map(|_| ()),
                                    };
                                    if let Err(err) = res {
                                        eprintln!("failed to apply date template: {err:#}");
                                    } else {
                                        this.text_edit_sync_task = None;
                                        desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    }
                                    this.reload(cx);
                                })),
                        ),
                ))
            })
            .into_any_element()
    }

    pub(super) fn render_notice_banner(&self, notice: &MatrixNotice, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme();
        let (message, color) = match notice {
            MatrixNotice::SavedElsewhere(destination) => (
                strings.matrix_saved_elsewhere.replace("{destination}", destination.as_ref()),
                Color::Default,
            ),
            MatrixNotice::AlreadyInQuadrant => (strings.matrix_already_in_quadrant.to_owned(), Color::Muted),
            MatrixNotice::DropUnsupported => (strings.matrix_drop_unsupported.to_owned(), Color::Warning),
            MatrixNotice::Failed => (strings.task_save_failed.to_owned(), Color::Error),
        };
        h_flex()
            .flex_none()
            .mx(px(22.))
            .px_2()
            .py_1()
            .gap_2()
            .rounded(px(6.))
            .bg(theme.colors().surface_background)
            .child(div().flex_1().min_w_0().child(Label::new(message).color(color)))
            .child(
                IconButton::new("matrix-dismiss-add-notice", IconName::X)
                    .tooltip(Tooltip::text(strings.close))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_floating_detail(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let max_detail_h = window.viewport_size().height * 0.85;
        self.detail_task.and(self.detail.clone()).map(|detail| {
            Modal::new("matrix-detail-backdrop")
                .width(px(460.))
                .height(px(560.))
                .max_height(max_detail_h)
                .padding(px(0.))
                .gap(px(0.))
                .overflow_hidden()
                .on_dismiss(cx.listener(|this, _, _, cx| {
                    if this.detail.as_ref().is_none_or(|d| !d.read(cx).has_popup()) {
                        this.close_detail_card(cx);
                    }
                }))
                .child(detail)
                .into_any_element()
        })
    }

    pub(super) fn render_confirm_delete_profile(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let strings = i18n::t(cx);
        self.confirm_delete_profile.clone().map(|(id, name)| {
            ConfirmDialog::new("matrix-delete-profile-modal", format!("{} \"{name}\"?", strings.delete))
                .width(px(340.))
                .cancel_label(strings.cancel)
                .confirm_label(strings.delete)
                .destructive(true)
                .on_cancel(cx.listener(|this, _, window, cx| {
                    this.confirm_delete_profile = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }))
                .on_confirm(cx.listener(move |this, _, window, cx| {
                    this.confirm_delete_profile = None;
                    this.delete_profile(id, cx);
                    window.focus(&this.focus, cx);
                }))
                .into_any_element()
        })
    }

    pub(super) fn render_profile_editor_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        self.profile_editor.as_ref().map(|(editing, input)| {
            let heading = if editing.is_some() {
                strings.matrix_rename_profile
            } else {
                strings.matrix_new_profile
            };
            let can_save = !input.read(cx).text().trim().is_empty();
            Modal::new("matrix-profile-editor-backdrop")
                .width(px(320.))
                .padding(px(16.))
                .on_dismiss(cx.listener(|this, _, window, cx| {
                    this.profile_editor = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }))
                .child(Label::new(heading).weight(FontWeight::SEMIBOLD))
                .child(Label::new(strings.matrix_profile_name).size(LabelSize::XSmall).color(Color::Muted))
                .child(
                    div()
                        .h(px(32.))
                        .px_2p5()
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.colors().border)
                        .bg(theme.colors().background)
                        .child(input.clone()),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("matrix-profile-cancel", strings.cancel)
                                .variant(ButtonVariant::Secondary)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.profile_editor = None;
                                    window.focus(&this.focus, cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("matrix-profile-save", strings.save)
                                .variant(ButtonVariant::Primary)
                                .disabled(!can_save)
                                .on_click(cx.listener(|this, _, _, cx| this.save_profile_editor(cx))),
                        ),
                )
                .into_any_element()
        })
    }
}
