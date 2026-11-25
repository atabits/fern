use super::*;
use crate::resize::{self, DraggedEdge, Panel};

impl TodoView {
    /// Reads a per-view boolean flag from SQLite (used only in `reload_view_settings`).
    fn load_view_flag(&self, name: &str, default: bool) -> bool {
        let key = format!("{name}:{}", self.view_key());
        self.store.setting(&key).ok().flatten().map_or(default, |v| v == "1")
    }

    /// Loads all per-view flags and sort/group settings from SQLite into cached fields so
    /// `render()`, `rows()`, `board_columns()`, and `move_task_drag()` perform zero DB queries.
    pub(super) fn reload_view_settings(&mut self) {
        self.show_details = self.load_view_flag("show_details", false);
        self.kanban.enabled = self.load_view_flag("kanban", false);
        self.timeline.enabled = self.load_view_flag("timeline", false);
        self.kanban.workflow = self.load_view_flag("kanban_workflow", false);
        self.show_completed = self.load_view_flag("show_completed", true);
        self.sort_descending = self.load_view_flag("descending", false);
        self.hide_new_section = self.load_view_flag("kanban:hide_new_section", false);

        let key = self.view_key();
        let get = |what: &str| self.store.setting(&format!("{what}:{key}")).ok().flatten().unwrap_or_default();
        // A tag's view is grouped by list until chosen otherwise, as the original's.
        let group = match get("group") {
            g if g.is_empty() && matches!(self.view, View::Tag(_)) => GroupBy::List,
            // The original's board of a smart list has a column per list.
            g if g.is_empty() && self.kanban.enabled && self.view.shows_list_names() => GroupBy::List,
            g => GroupBy::from_key(&g),
        };
        let group = if self.kanban.enabled && !self.timeline.enabled && group == GroupBy::None {
            if self.view.shows_list_names() { GroupBy::List } else { GroupBy::Custom }
        } else {
            group
        };
        self.sort_by = SortBy::from_key(&get("sort"));
        self.group_by = group;
        self.invalidate_rows();
    }

    /// Cached per-list switch of the header's "…" menu ("Show Completed", "Show Details", etc.).
    /// Pure memory read — zero SQLite queries.
    pub(super) fn view_flag(&self, name: &str, default: bool) -> bool {
        match name {
            "show_details" => self.show_details,
            "kanban" => self.kanban.enabled,
            "timeline" => self.timeline.enabled,
            "kanban_workflow" => self.kanban.workflow,
            "show_completed" => self.show_completed,
            "descending" => self.sort_descending,
            "kanban:hide_new_section" => self.hide_new_section,
            _ => default,
        }
    }

    pub(super) fn toggle_view_flag(&mut self, name: &str, default: bool, cx: &mut Context<Self>) {
        let key = format!("{name}:{}", self.view_key());
        let value = !self.view_flag(name, default);
        let result = self.store.set_setting(&key, if value { "1" } else { "0" });
        self.check(result);
        self.reload_view_settings();
        self.overlays.header_menu_open = false;
        if name == "show_completed" && matches!(self.view, View::Today | View::Tomorrow | View::Next7Days) {
            if value {
                self.collapsed_sections.remove(&Section::Completed);
            }
            self.reload(cx);
        } else {
            self.sync_kanban_column_state(cx);
            cx.notify();
        }
    }

    /// The header menu's View: list, kanban or timeline, one at a time.
    pub(super) fn set_layout(&mut self, kanban: bool, timeline: bool, cx: &mut Context<Self>) {
        let was_board = self.uses_board_layout();
        self.completing.clear();
        self.close_card(cx);
        let key = self.view_key();
        for (name, on) in [("kanban", kanban), ("timeline", timeline)] {
            let result = self.store.set_setting(&format!("{name}:{key}"), if on { "1" } else { "0" });
            self.check(result);
        }
        self.reload_view_settings();
        self.timeline.from = Self::today();
        self.overlays.header_menu_open = false;
        if kanban && !timeline {
            let strings = i18n::t(cx);
            self.ensure_kanban_sections(strings, cx);
        }
        self.sync_kanban_column_state(cx);
        if was_board != self.uses_board_layout() {
            self.sync_layout_selection(cx);
        }
        cx.notify();
    }

    pub(super) fn sort_settings(&self) -> (SortBy, GroupBy) {
        (self.sort_by, self.group_by)
    }

    pub(super) fn set_sort(&mut self, sort: Option<SortBy>, group: Option<GroupBy>, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.view_key();
        let result = sort
            .map(|s| self.store.set_setting(&format!("sort:{key}"), s.key()))
            .into_iter()
            .chain(group.map(|g| self.store.set_setting(&format!("group:{key}"), g.key())))
            .collect::<anyhow::Result<Vec<()>>>();
        self.check(result);
        self.reload_view_settings();
        if group == Some(GroupBy::Custom) {
            let strings = i18n::t(cx);
            self.ensure_kanban_sections(strings, cx);
        }
        self.sync_kanban_column_state(cx);
        self.close_sort_menu(window, cx);
    }

    /// Follows a dragged panel edge. `event.bounds` is this view's.
    pub(super) fn resize_panel(&mut self, event: &gpui::DragMoveEvent<DraggedEdge>, _: &mut Window, cx: &mut Context<Self>) {
        let panel = event.drag(cx).0;
        let bounds = event.bounds;
        let x = event.event.position.x;
        let prev = (self.resizing, self.show_lists, self.lists_width, self.detail_width);
        self.resizing = Some(panel);
        match panel {
            Panel::Lists => {
                let width = f32::from(x - bounds.left());
                // Dragged almost shut, the column hides; dragged back out, it shows again.
                self.show_lists = width >= lists_panel::COLLAPSE_BELOW;
                if self.show_lists {
                    self.lists_width = width.clamp(lists_panel::MIN_WIDTH, lists_panel::MAX_WIDTH);
                }
            }
            Panel::Detail => {
                let width = f32::from(bounds.right() - x);
                self.detail_width = width.clamp(DETAIL_MIN_WIDTH, self.max_detail_width(f32::from(bounds.size.width)).max(DETAIL_MIN_WIDTH));
            }
        }
        let widths_changed = (self.show_lists, self.lists_width, self.detail_width) != (prev.1, prev.2, prev.3);
        if widths_changed {
            self.schedule_save_widths(cx);
        }
        if widths_changed || self.resizing != prev.0 {
            cx.notify();
        }
    }

    /// The widest the details may be in a view `view_width` wide, leaving the task list its room.
    pub(super) fn max_detail_width(&self, view_width: f32) -> f32 {
        let lists = if self.show_lists { self.lists_width } else { 0. };
        view_width - lists - LIST_MIN_WIDTH
    }

    /// A double click on an edge: the panel's default width.
    fn reset_panel_width(&mut self, panel: Panel, cx: &mut Context<Self>) {
        match panel {
            Panel::Lists => self.lists_width = lists_panel::WIDTH,
            Panel::Detail => self.detail_width = DETAIL_WIDTH,
        }
        self.schedule_save_widths(cx);
        cx.notify();
    }

    fn schedule_save_widths(&mut self, cx: &mut Context<Self>) {
        self.save_widths = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
            this.update(cx, |this, _| {
                for (key, width) in [("lists_width", this.lists_width), ("detail_width", this.detail_width)] {
                    if let Err(err) = this.store.set_setting(key, &width.round().to_string()) {
                        desktop_common::logging::log_error(
                            "todo_ui",
                            format!("failed to save the panel width: {err:#}"),
                        );
                    }
                }
            })
            .ok();
        }));
    }

    /// An edge's handle; `panel` takes its width from where the edge is dropped.
    pub(super) fn resize_handle(&self, panel: Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let dragged = self.resizing == Some(panel) && cx.has_active_drag();
        resize::handle(
            panel,
            dragged,
            move |_, cx| {
                this.update(cx, |this, cx| this.reset_panel_width(panel, cx)).ok();
            },
            cx,
        )
    }
}
