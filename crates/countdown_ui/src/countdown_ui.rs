//! The Countdown section, matched to TickTick's: filter tabs and a grid of cards showing the time
//! until (or since) a date; clicking the numbers switches between Y/M/D and plain days.

mod modal;
mod modal_menus;
#[cfg(all(test, debug_assertions))]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use chrono::Local;
use fern_core::{Countdown, CountdownKind, Id, Store};
use gpui::{Focusable as _, FontWeight, prelude::*};
use ui::prelude::*;
use ui::{ConfirmDialog, EmptyState, Placement, PopoverAttachment, popover};

use desktop_common::data_version::{DataVersion, changed_elsewhere, data_changed};
use desktop_common::{actions, layout, paths::attachments_dir, touch};
use fern_i18n::{self as i18n, Strings};

use modal::Draft;
#[cfg(debug_assertions)]
use modal::{DateField, DraftRow};

const CARD_WIDTH: f32 = 289.;
const CARD_HEIGHT: f32 = 181.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingFocus {
    View,
    DraftName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Filter {
    All,
    Kind(CountdownKind),
}

pub struct CountdownView {
    store: Rc<Store>,
    focus_handle: gpui::FocusHandle,
    pub(crate) pending_focus: Option<PendingFocus>,
    pub(crate) countdowns: Vec<Countdown>,
    pub(crate) filter: Filter,
    /// Cards switched to the "days only" format.
    days_only: HashSet<Id>,
    pub(crate) draft: Option<Draft>,
    /// The card whose "…" menu is open.
    pub(crate) card_menu: Option<Id>,
    /// Countdown pending deletion confirmation: (id, name).
    confirm_delete: Option<(Id, String)>,
    /// The "+" menu of kinds, as in the original.
    pub(crate) add_menu: bool,
    /// The title's "Countdown ▾" menu (Active / Archived) and the header's "…" menu, while open.
    title_menu: bool,
    header_menu: bool,
    /// Listing the archived countdowns.
    show_archived: bool,
    /// Cached value of `HIDE_GROUP_SETTING` (loaded in `reload`, never queried during `render`).
    hide_group: bool,
    /// Resolved on-disk background image paths, refreshed in `reload` so `render_card` performs zero `stat()` calls.
    resolved_backgrounds: HashMap<Id, PathBuf>,
    pub(crate) active: bool,
    pub(crate) dirty: bool,
}

/// Hides the kind tabs (the header "…" menu's Hide Group).
const HIDE_GROUP_SETTING: &str = "countdown_hide_group";

pub use desktop_common::countdown_helpers::{
    Face, badge, countdown_color, face, format_date, kind_icon, render_summary,
};

pub(crate) fn kind_label(kind: CountdownKind, strings: &Strings) -> &'static str {
    match kind {
        CountdownKind::Countdown => strings.countdown_kind_countdown,
        CountdownKind::Anniversary => strings.countdown_kind_anniversary,
        CountdownKind::Birthday => strings.countdown_kind_birthday,
        CountdownKind::Holiday => strings.countdown_kind_holiday,
    }
}

impl gpui::Focusable for CountdownView {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl CountdownView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, true, cx)
    }

    pub fn new_inactive(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, false, cx)
    }

    fn new_with_active(store: Rc<Store>, initial_active: bool, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            store,
            focus_handle: cx.focus_handle(),
            pending_focus: None,
            countdowns: Vec::new(),
            filter: Filter::All,
            days_only: HashSet::new(),
            draft: None,
            card_menu: None,
            confirm_delete: None,
            add_menu: false,
            title_menu: false,
            header_menu: false,
            show_archived: false,
            hide_group: false,
            resolved_backgrounds: HashMap::new(),
            active: initial_active,
            dirty: !initial_active,
        };
        if initial_active {
            this.reload(cx);
        }
        cx.observe_global::<DataVersion>(|this, cx| {
            if changed_elsewhere(cx.entity_id(), cx) {
                this.sync_external(cx);
            }
        })
        .detach();
        this
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if active && self.dirty {
            self.dirty = false;
            self.reload(cx);
        }
    }

    pub fn sync_external(&mut self, cx: &mut Context<Self>) {
        if self.active {
            self.reload(cx);
        } else {
            self.dirty = true;
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        self.countdowns = self.store.countdowns().unwrap_or_default();
        self.hide_group = self
            .store
            .setting(HIDE_GROUP_SETTING)
            .ok()
            .flatten()
            .as_deref()
            == Some("1");
        self.resolved_backgrounds = match attachments_dir() {
            Ok(dir) => self
                .countdowns
                .iter()
                .filter_map(|c| {
                    let name = c.background.as_ref()?;
                    let path = dir.join(name);
                    path.exists().then_some((c.id, path))
                })
                .collect(),
            Err(_) => HashMap::new(),
        };
        cx.notify();
    }

    /// The new countdown's `reminder`, `calc` or `show` dropdown, for screenshots.
    #[cfg(debug_assertions)]
    pub fn dev_open_row(&mut self, row: &str, cx: &mut Context<Self>) {
        if let Some(draft) = self.draft.as_mut() {
            draft.open = Some(match row {
                "calc" => DraftRow::CalcMode,
                "presets" => DraftRow::Presets,
                "counting" => DraftRow::CountingMode,
                "month" => DraftRow::DatePart(DateField::Month),
                "show" => DraftRow::Show,
                _ => DraftRow::Reminder,
            });
        }
        cx.notify();
    }

    /// The header's "Countdown ▾" (or with `more`, its "…") menu, for screenshots.
    #[cfg(debug_assertions)]
    pub fn dev_header_menu(&mut self, more: bool, cx: &mut Context<Self>) {
        self.title_menu = !more;
        self.header_menu = more;
        cx.notify();
    }

    /// Opens the "+" menu from the debug-build dev state (`countdown-add-menu`).
    #[cfg(debug_assertions)]
    pub fn dev_add_menu(&mut self, cx: &mut Context<Self>) {
        self.add_menu = true;
        cx.notify();
    }

    /// `menu` opens the card's "…" menu, `edit` its edit popover, with a monthly repeat.
    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, name: &str, edit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            self.reload(cx);
        }
        let Some(id) = self
            .countdowns
            .iter()
            .find(|c| c.name.contains(name))
            .map(|c| c.id)
        else {
            return;
        };
        if edit {
            self.start_edit(id, window, cx);
            if let Some(draft) = self.draft.as_mut() {
                draft.repeat = Some(fern_core::Recurrence::new(fern_core::Frequency::Monthly, 1));
            }
        } else {
            self.card_menu = Some(id);
        }
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub fn dev_add(
        &mut self,
        name: &str,
        date: chrono::NaiveDate,
        kind: CountdownKind,
        look: Option<(&str, &str)>,
        cx: &mut Context<Self>,
    ) {
        if let Ok(countdown) = self.store.add_countdown(name, date, kind)
            && let Some((icon, color)) = look
        {
            self.store
                .set_countdown_look(countdown.id, Some(icon), (!color.is_empty()).then_some(color))
                .ok();
        }
        self.reload(cx);
    }

    fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.delete_countdown(id) {
            eprintln!("failed to delete the countdown: {err:#}");
        } else {
            data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    fn toggle_pin(&mut self, id: Id, pinned: bool, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_countdown_pinned(id, !pinned) {
            eprintln!("failed to pin the countdown: {err:#}");
        } else {
            data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    fn toggle_archive(&mut self, id: Id, archived: bool, cx: &mut Context<Self>) {
        self.card_menu = None;
        if let Err(err) = self.store.set_countdown_archived(id, !archived) {
            eprintln!("failed to archive the countdown: {err:#}");
        } else {
            data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    fn toggle_format(&mut self, id: Id, cx: &mut Context<Self>) {
        if !self.days_only.remove(&id) {
            self.days_only.insert(id);
        }
        cx.notify();
    }

    fn render_card(&self, countdown: &Countdown, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let id = countdown.id;
        let group = SharedString::from(format!("countdown-{id}"));
        let Face {
            color,
            numbers,
            subtitle,
        } = face(countdown, today, self.days_only.contains(&id), cx);
        let background: Option<PathBuf> = self.resolved_backgrounds.get(&id).cloned();
        let has_picture = background.is_some();

        v_flex()
            .id(group.clone())
            .group(group.clone())
            .relative()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .bg(theme.colors().surface_background)
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    this.start_edit(id, window, cx);
                }
            }))
            // The card's picture behind its numbers, darkened so they stay readable.
            .when_some(background, |el, path| {
                el.child(
                    gpui::img(path)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .rounded(px(8.))
                        .object_fit(gpui::ObjectFit::Cover),
                )
                .child(div().absolute().inset_0().rounded(px(8.)).bg(gpui::black().opacity(0.35)))
            })
            .child(
                h_flex()
                    .gap_1p5()
                    .child(badge(countdown, color, cx))
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .when(has_picture, |el| el.text_color(gpui::white()))
                            .child(countdown.name.clone()),
                    ),
            )
            // The original's spacing: 20 px down to the numbers, 16 px on to the dates.
            .child(
                div()
                    .id(SharedString::from(format!("countdown-numbers-{id}")))
                    .mt(px(20.))
                    .mb(px(16.))
                    .cursor_pointer()
                    .tooltip(Tooltip::text(strings.countdown_switch_hint))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_format(id, cx);
                    }))
                    .child(numbers),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .when(has_picture, |el| el.text_color(gpui::white().opacity(0.85)))
                    .when(!has_picture, |el| el.text_color(theme.colors().text_muted))
                    .child(subtitle),
            )
            // The original's pin, beside the "…": a pinned card stays first and shows it.
            .child({
                let pinned = countdown.pinned_at.is_some();
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(32.))
                    .opacity(if pinned || touch::active() { 1. } else { 0. })
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .child(
                        IconButton::new(SharedString::from(format!("countdown-pin-{id}")), IconName::Pin)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .icon_color(if pinned { Color::Accent } else { Color::Muted })
                            .tooltip(Tooltip::text(if pinned { strings.unpin } else { strings.pin }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_pin(id, pinned, cx);
                            })),
                    )
            })
            .child(
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(6.))
                    .opacity(if touch::active() { 1. } else { 0. })
                    .group_hover(group, |s| s.opacity(1.))
                    .when(self.card_menu == Some(id), |el| el.opacity(1.))
                    .child(
                        // The original's "…" on a hovered card: Edit and Delete.
                        IconButton::new(SharedString::from(format!("countdown-more-{id}")), IconName::Ellipsis)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text(strings.more))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.card_menu = if this.card_menu == Some(id) { None } else { Some(id) };
                                cx.notify();
                            })),
                    )
                    .when(self.card_menu == Some(id), |el| {
                        el.child(PopoverAttachment::new(
                            Placement::BelowRight,
                            popover(cx)
                                .min_w(px(150.))
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.card_menu = None;
                                    cx.notify();
                                }))
                                .child(
                                    ListItem::new("countdown-edit")
                                        .start_slot(Icon::new(IconName::Pencil).color(Color::Icon))
                                        .child(Label::new(strings.edit))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.start_edit(id, window, cx);
                                        })),
                                )
                                .child({
                                    let archived = countdown.archived;
                                    ListItem::new("countdown-archive")
                                        .start_slot(
                                            Icon::new(if archived {
                                                IconName::ArchiveRestore
                                            } else {
                                                IconName::Archive
                                            })
                                            .color(Color::Icon),
                                        )
                                        .child(Label::new(if archived {
                                            strings.habit_unarchive
                                        } else {
                                            strings.habit_archive
                                        }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_archive(id, archived, cx);
                                        }))
                                })
                                .child({
                                    let name = countdown.name.clone();
                                    ListItem::new("countdown-delete")
                                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                        .child(Label::new(strings.delete).color(Color::Error))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.card_menu = None;
                                            this.confirm_delete = Some((id, name.clone()));
                                            cx.notify();
                                        }))
                                })
                                .into_any_element(),
                        ))
                    }),
            )
            .into_any_element()
    }

    pub(crate) fn apply_pending_focus(&mut self, window: &mut Window, cx: &mut App) {
        match self.pending_focus.take() {
            Some(PendingFocus::View) => window.focus(&self.focus_handle, cx),
            Some(PendingFocus::DraftName) => {
                if let Some(draft) = self.draft.as_ref() {
                    window.focus(&draft.name.focus_handle(cx), cx);
                } else {
                    window.focus(&self.focus_handle, cx);
                }
            }
            None => {}
        }
    }

    pub fn dismiss_transient(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(draft) = self.draft.as_mut()
            && draft.dismiss_popover()
        {
            self.pending_focus = Some(PendingFocus::DraftName);
            cx.notify();
            return true;
        }
        if std::mem::take(&mut self.title_menu)
            || std::mem::take(&mut self.add_menu)
            || std::mem::take(&mut self.header_menu)
            || self.card_menu.take().is_some()
            || self.confirm_delete.take().is_some()
        {
            cx.notify();
            true
        } else if self.draft.take().is_some() {
            self.pending_focus = Some(PendingFocus::View);
            cx.notify();
            true
        } else {
            false
        }
    }
}

impl Render for CountdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_pending_focus(window, cx);
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let hide_group = self.hide_group;
        let filters = [
            (Filter::All, strings.countdown_all),
            (Filter::Kind(CountdownKind::Countdown), strings.countdown_kind_countdown),
            (Filter::Kind(CountdownKind::Anniversary), strings.countdown_kind_anniversary),
            (Filter::Kind(CountdownKind::Holiday), strings.countdown_kind_holiday),
        ];
        let shown: Vec<Countdown> = self
            .countdowns
            .iter()
            .filter(|c| c.archived == self.show_archived)
            .filter(|c| match self.filter {
                Filter::All => true,
                Filter::Kind(kind) => c.kind == kind,
            })
            .cloned()
            .collect();
        let cards: Vec<AnyElement> = shown.iter().map(|c| self.render_card(c, cx)).collect();
        // The add / edit dialog over a dimmed background, as the original shows it.
        let modal = self.draft.is_some().then(|| self.render_draft(window, cx));

        v_flex()
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("Countdown")
            .on_action(cx.listener(|this, _: &actions::DismissTransient, window, cx| {
                if !this.dismiss_transient(cx) {
                    cx.propagate();
                } else {
                    this.apply_pending_focus(window, cx);
                }
            }))
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(
                h_flex()
                    .mt(layout::header_inset())
                    .h(px(52.))
                    .flex_none()
                    .px(px(22.))
                    .gap_1()
                    // The original's "Countdown ▾": Active or Archived countdowns.
                    .child(
                        div()
                            .relative()
                            .child(
                                h_flex()
                                    .id("countdown-title")
                                    .gap_1()
                                    .cursor_pointer()
                                    .child(
                                        Label::new(strings.section_countdown)
                                            .size(LabelSize::Large)
                                            .weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.title_menu = !this.title_menu;
                                        cx.notify();
                                    })),
                            )
                            .when(self.title_menu, |el| {
                                let archived = self.show_archived;
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    popover(cx)
                                        .min_w(px(162.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.title_menu = false;
                                            cx.notify();
                                        }))
                                        .children(
                                            [(false, strings.habit_active), (true, strings.habit_archived)]
                                                .map(|(value, label)| {
                                                    ListItem::new(if value {
                                                        "countdowns-archived"
                                                    } else {
                                                        "countdowns-active"
                                                    })
                                                    .height(px(34.))
                                                    .child(Label::new(label).color(if value == archived {
                                                        Color::Accent
                                                    } else {
                                                        Color::Default
                                                    }))
                                                    .when(value == archived, |item| {
                                                        item.end_slot(
                                                            Icon::new(IconName::Check)
                                                                .size(IconSize::XSmall)
                                                                .color(Color::Accent),
                                                        )
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.show_archived = value;
                                                        this.title_menu = false;
                                                        cx.notify();
                                                    }))
                                                }),
                                        ),
                                ))
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .relative()
                            .child(
                                IconButton::new("countdown-add", IconName::Plus)
                                    .icon_size(IconSize::XLarge)
                                    .icon_color(Color::Default)
                                    .tooltip(Tooltip::text(strings.countdown_new))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.add_menu = !this.add_menu;
                                        cx.notify();
                                    })),
                            )
                            // The original's "+" asks for the kind first: 162 px wide, a row every 34 px.
                            .when(self.add_menu, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .min_w(px(162.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.add_menu = false;
                                            cx.notify();
                                        }))
                                        .children(
                                            [
                                                CountdownKind::Countdown,
                                                CountdownKind::Anniversary,
                                                CountdownKind::Birthday,
                                                CountdownKind::Holiday,
                                            ]
                                            .into_iter()
                                            .map(|kind| {
                                                ListItem::new(SharedString::from(format!(
                                                    "countdown-add-{}",
                                                    kind.as_str()
                                                )))
                                                .height(px(34.))
                                                .start_slot(Icon::new(kind_icon(kind)).color(Color::Muted))
                                                .child(Label::new(kind_label(kind, strings)))
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.start_new_kind(kind, window, cx)
                                                }))
                                            }),
                                        ),
                                ))
                            }),
                    )
                    // The original's "…": Hide Group (the kind tabs) or Show Group.
                    .child(
                        div()
                            .relative()
                            .child(
                                IconButton::new("countdown-more", IconName::Ellipsis)
                                    .icon_color(Color::Default)
                                    .tooltip(Tooltip::text(strings.more))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.header_menu = !this.header_menu;
                                        cx.notify();
                                    })),
                            )
                            .when(self.header_menu, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .min_w(px(162.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.header_menu = false;
                                            cx.notify();
                                        }))
                                        .child(
                                            ListItem::new("countdown-hide-group")
                                                .height(px(34.))
                                                .start_slot(Icon::new(IconName::Layers).color(Color::Muted))
                                                .child(Label::new(if hide_group {
                                                    strings.countdown_show_group
                                                } else {
                                                    strings.countdown_hide_group
                                                }))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.header_menu = false;
                                                    let next = !hide_group;
                                                    this.hide_group = next;
                                                    if let Err(err) = this.store.set_setting(
                                                        HIDE_GROUP_SETTING,
                                                        if next { "1" } else { "0" },
                                                    ) {
                                                        eprintln!("failed to save the countdown setting: {err:#}");
                                                    }
                                                    cx.notify();
                                                })),
                                        ),
                                ))
                            }),
                    ),
            )
            .when(!hide_group, |el| {
                el.child(
                    h_flex()
                        .flex_none()
                        .px(px(20.))
                        .pt_2()
                        .gap_1()
                        .pb(px(18.))
                        .children(filters.map(|(filter, label)| {
                            // The original's filter pills: 32 px, at the body size.
                            let selected = self.filter == filter;
                            div()
                                .id(label)
                                .h(px(32.))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded(px(16.))
                                .cursor_pointer()
                                .when(selected, |el| el.bg(theme.colors().accent.opacity(0.2)))
                                .when(!selected, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                                .child(
                                    Label::new(label)
                                        .color(if selected { Color::Accent } else { Color::Muted }),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.filter = filter;
                                    cx.notify();
                                }))
                        })),
                )
            })
            .child(
                v_scroll(
                    "countdown-grid-scroll",
                    div()
                        .id("countdown-grid")
                        .px(px(22.))
                        .when(cards.is_empty(), |el| {
                            let default_kind = match self.filter {
                                Filter::All => CountdownKind::Countdown,
                                Filter::Kind(k) => k,
                            };
                            el.child(
                                div().w_full().py(px(40.)).child(
                                    EmptyState::new(IconName::FernCountdown, strings.countdown_empty).action(
                                        "countdown-empty-add",
                                        strings.countdown_new,
                                        cx.listener(move |this, _, window, cx| {
                                            this.start_new_kind(default_kind, window, cx);
                                        }),
                                    ),
                                ),
                            )
                        })
                        .child(div().flex().flex_wrap().gap(px(20.)).children(cards)),
                )
                .flex_1()
                .min_h_0(),
            )
            .children(modal)
            .when_some(self.confirm_delete.clone(), |el, (id, name)| {
                el.child(
                    ConfirmDialog::new("countdown-delete-modal", strings.confirm_delete_countdown)
                        .width(px(340.))
                        .description(name)
                        .cancel_label(strings.cancel)
                        .confirm_label(strings.delete)
                        .destructive(true)
                        .on_cancel(cx.listener(|this, _, _, cx| {
                            this.confirm_delete = None;
                            cx.notify();
                        }))
                        .on_confirm(cx.listener(move |this, _, _, cx| {
                            this.confirm_delete = None;
                            this.delete(id, cx);
                        })),
                )
            })
    }
}
