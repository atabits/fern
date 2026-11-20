use super::*;

/// A collapsed folder or parent tag being hovered during a drag so it auto-expands after a brief pause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DragExpandTarget {
    Folder(Id),
    Tag(String),
}

pub(super) struct PanelDragState {
    pub(super) item: lists_panel::ActiveDragItem,
    pub(super) grab_offset_y: f32,
    pub(super) highlight: Option<lists_panel::HighlightTarget>,
    /// Pre-drag heights of the non-dragged variable-height blocks in the group, keeping midpoint
    /// thresholds invariant while blocks swap.
    pub(super) block_spans: Vec<f32>,
    pub(super) orig_tag_pinned: bool,
    pub(super) orig_preferences: fern_core::Preferences,
    pub(super) orig_lists: Vec<List>,
    pub(super) orig_folders: Vec<ListFolder>,
    pub(super) orig_tags: Vec<Tag>,
    pub(super) orig_filters: Vec<(fern_core::SmartFilter, usize)>,
    pub(super) orig_collapsed_folders: HashSet<Id>,
    pub(super) orig_collapsed_tags: HashSet<String>,
}


impl TodoView {
    pub(super) fn schedule_drag_expand(&mut self, target: DragExpandTarget, cx: &mut Context<Self>) {
        if self.drag_expand.as_ref().is_some_and(|(cur, _)| *cur == target) {
            return;
        }
        let scheduled = target.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(900)).await;
            let _ = this.update(cx, |this, cx| {
                if !cx.has_active_drag() {
                    this.drag_expand = None;
                    return;
                }
                if !this.drag_expand.as_ref().is_some_and(|(cur, _)| *cur == scheduled) {
                    return;
                }
                this.drag_expand = None;
                match scheduled {
                    DragExpandTarget::Folder(id) => {
                        if this.collapsed_folders.remove(&id) {
                            if let Some(drag) = this.panel_drag.as_mut()
                                && let lists_panel::ActiveDragItem::List(dragged_id) = drag.item
                            {
                                drag.highlight = None;
                                move_list_into_folder_start(&mut this.lists, &this.folders, dragged_id, id);
                            }
                            cx.notify();
                        }
                    }
                    DragExpandTarget::Tag(tag) => {
                        if this.collapsed_tags.remove(&tag) {
                            if let Some(drag) = this.panel_drag.as_mut()
                                && let lists_panel::ActiveDragItem::Tag(dragged_name) = drag.item.clone()
                            {
                                drag.highlight = None;
                                move_tag_into_parent_start(&mut this.tags, &dragged_name, &tag);
                            }
                            cx.notify();
                        }
                    }
                }
            });
        });
        self.drag_expand = Some((target, task));
    }

    pub(super) fn persist_collapsed_sidebar_sets(&mut self) {
        let folders: Vec<String> = self.collapsed_folders.iter().map(Id::to_string).collect();
        let res = self.store.set_setting("collapsed_folders", &folders.join(","));
        self.check(res);
        let tags: Vec<&str> = self.collapsed_tags.iter().map(String::as_str).collect();
        let res = self.store.set_setting("collapsed_tags", &tags.join("\n"));
        self.check(res);
    }

    pub(super) fn panel_event(&mut self, event: lists_panel::PanelEvent, window: &mut Window, cx: &mut Context<Self>) {
        use lists_panel::PanelEvent;
        match event {
            PanelEvent::ToggleFolder(id) => {
                if !self.collapsed_folders.remove(&id) {
                    self.collapsed_folders.insert(id);
                }
                let value: Vec<String> = self.collapsed_folders.iter().map(Id::to_string).collect();
                let result = self.store.set_setting("collapsed_folders", &value.join(","));
                self.check(result);
            }
            PanelEvent::ToggleArchived => self.archived_open = !self.archived_open,
            PanelEvent::AddTag => return self.open_tag_dialog(None, None, window, cx),
            PanelEvent::TagsMenu(at) => self.list_menu = Some((ListMenuTarget::TagsHeading, at)),
            PanelEvent::SmartMenu(list, at) => self.list_menu = Some((ListMenuTarget::Smart(list), at)),
            PanelEvent::ToggleTags => {
                self.tags_collapsed = !self.tags_collapsed;
                let result = self.store.set_setting("tags_collapsed", if self.tags_collapsed { "1" } else { "0" });
                self.check(result);
            }
            PanelEvent::ToggleTag(tag) => {
                if !self.collapsed_tags.remove(&tag) {
                    self.collapsed_tags.insert(tag);
                }
                let value: Vec<&str> = self.collapsed_tags.iter().map(String::as_str).collect();
                let result = self.store.set_setting("collapsed_tags", &value.join("\n"));
                self.check(result);
            }
            PanelEvent::TagMenu(tag, at) => self.list_menu = Some((ListMenuTarget::Tag(tag), at)),
            PanelEvent::ListMenu(id, at) => self.list_menu = Some((ListMenuTarget::List(id), at)),
            PanelEvent::FolderMenu(id, at) => self.list_menu = Some((ListMenuTarget::Folder(id), at)),
            PanelEvent::StartDrag { item, grab_offset_y } => {
                self.start_panel_drag(item, grab_offset_y, cx);
                return;
            }
        }
        self.reload(cx);
    }

    pub(super) fn measure_non_dragged_block_spans(&self, item: &lists_panel::ActiveDragItem) -> Vec<f32> {
        use lists_panel::{ActiveDragItem, PanelChild};
        let rows = self.lists_rows.borrow();
        let item_top = |ix: usize| self.lists_scroll.bounds_for_item(ix).map_or(0., |b| f32::from(b.top()));
        let item_bot = |ix: usize| self.lists_scroll.bounds_for_item(ix).map_or(0., |b| f32::from(b.bottom()));

        match item {
            ActiveDragItem::SidebarSection(dragged_sec) => {
                let headings: Vec<(usize, fern_core::SidebarSection)> = rows
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, r)| match r {
                        PanelChild::SectionHeading(sec) => Some((ix, *sec)),
                        _ => None,
                    })
                    .collect();
                let Some(&(last_h, _)) = headings.last() else { return Vec::new() };
                let end_ix = rows
                    .iter()
                    .enumerate()
                    .skip(last_h + 1)
                    .find_map(|(ix, r)| {
                        matches!(r, PanelChild::Separator | PanelChild::BottomSmart(_)).then_some(ix)
                    })
                    .unwrap_or(rows.len());
                let mut spans = Vec::new();
                for (m, &(start_ix, sec)) in headings.iter().enumerate() {
                    if sec == *dragged_sec {
                        continue;
                    }
                    let top_y = item_top(start_ix);
                    let bot_y = if let Some(&(next_ix, _)) = headings.get(m + 1) {
                        item_top(next_ix)
                    } else {
                        item_bot(end_ix.saturating_sub(1).max(start_ix))
                    };
                    spans.push((bot_y - top_y).max(36.));
                }
                spans
            }
            ActiveDragItem::Folder(dragged_fid) => {
                let has_lists = self.lists.iter().any(|l| !l.archived && l.folder == Some(*dragged_fid));
                if !has_lists {
                    return Vec::new();
                }
                let blocks: Vec<(usize, bool)> = rows
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, r)| match r {
                        PanelChild::TopList(_) => Some((ix, false)),
                        PanelChild::FolderHeader { id, empty: false, .. } => Some((ix, *id == *dragged_fid)),
                        _ => None,
                    })
                    .collect();
                let Some(&(last_start, _)) = blocks.last() else { return Vec::new() };
                let end_ix = rows
                    .iter()
                    .enumerate()
                    .skip(last_start + 1)
                    .find_map(|(ix, r)| {
                        (!matches!(r, PanelChild::SubList { .. })).then_some(ix)
                    })
                    .unwrap_or(rows.len());
                let mut spans = Vec::new();
                for (m, &(start_ix, is_dragged)) in blocks.iter().enumerate() {
                    if is_dragged {
                        continue;
                    }
                    let next_ix = blocks.get(m + 1).map_or(end_ix, |&(ix, _)| ix);
                    let last_ix = next_ix.saturating_sub(1).max(start_ix);
                    spans.push((item_bot(last_ix) - item_top(start_ix)).max(36.));
                }
                spans
            }
            ActiveDragItem::Tag(dragged_name) => {
                let is_parent = self.tags.iter().any(|t| t.parent.as_deref() == Some(dragged_name.as_str()));
                if !is_parent {
                    return Vec::new();
                }
                let roots: Vec<(usize, bool)> = rows
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, r)| match r {
                        PanelChild::TopTag { name, .. } => Some((ix, name == dragged_name)),
                        _ => None,
                    })
                    .collect();
                let Some(&(last_start, _)) = roots.last() else { return Vec::new() };
                let end_ix = rows
                    .iter()
                    .enumerate()
                    .skip(last_start + 1)
                    .find_map(|(ix, r)| (!matches!(r, PanelChild::SubTag { .. })).then_some(ix))
                    .unwrap_or(rows.len());
                let mut spans = Vec::new();
                for (m, &(start_ix, is_dragged)) in roots.iter().enumerate() {
                    if is_dragged {
                        continue;
                    }
                    let next_ix = roots.get(m + 1).map_or(end_ix, |&(ix, _)| ix);
                    let last_ix = next_ix.saturating_sub(1).max(start_ix);
                    spans.push((item_bot(last_ix) - item_top(start_ix)).max(38.));
                }
                spans
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn start_panel_drag(&mut self, item: lists_panel::ActiveDragItem, grab_offset_y: f32, cx: &mut Context<Self>) {
        self.drag_expand = None;
        self.list_menu = None;
        let block_spans = self.measure_non_dragged_block_spans(&item);
        let orig_tag_pinned = match &item {
            lists_panel::ActiveDragItem::Tag(name) => {
                self.tags.iter().find(|t| t.name == *name).is_some_and(|t| t.pinned)
            }
            _ => false,
        };
        let orig_preferences = self.preferences.clone();
        let orig_lists = self.lists.clone();
        let orig_folders = self.folders.clone();
        let orig_tags = self.tags.clone();
        let orig_filters = self.filters.clone();
        let orig_collapsed_folders = self.collapsed_folders.clone();
        let orig_collapsed_tags = self.collapsed_tags.clone();

        normalize_lists_in_memory(&mut self.lists, &self.folders);
        normalize_tags_in_memory(&mut self.tags);

        self.panel_drag = Some(PanelDragState {
            item,
            grab_offset_y,
            highlight: None,
            block_spans,
            orig_tag_pinned,
            orig_preferences,
            orig_lists,
            orig_folders,
            orig_tags,
            orig_filters,
            orig_collapsed_folders,
            orig_collapsed_tags,
        });
        cx.notify();
    }

    pub(super) fn restore_panel_drag_snapshot(&mut self) -> bool {
        self.drag_expand = None;
        let Some(drag) = self.panel_drag.take() else { return false };
        self.preferences = drag.orig_preferences;
        self.lists = drag.orig_lists;
        self.folders = drag.orig_folders;
        self.tags = drag.orig_tags;
        self.filters = drag.orig_filters;
        self.collapsed_folders = drag.orig_collapsed_folders;
        self.collapsed_tags = drag.orig_collapsed_tags;
        true
    }

    pub(super) fn cancel_panel_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.restore_panel_drag_snapshot() {
            cx.stop_active_drag(window);
            cx.notify();
            return true;
        }
        false
    }


    pub(super) fn finish_panel_drag_up(
        &mut self,
        event: &gpui::MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button == MouseButton::Left {
            self.finish_panel_drag(window, cx);
        }
    }

    pub(super) fn finish_panel_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use lists_panel::{ActiveDragItem, HighlightTarget};
        self.drag_expand = None;
        let Some(drag) = self.panel_drag.take() else { return };
        cx.stop_active_drag(window);

        let collapsed_changed = self.collapsed_folders != drag.orig_collapsed_folders
            || self.collapsed_tags != drag.orig_collapsed_tags;
        if collapsed_changed {
            self.persist_collapsed_sidebar_sets();
        }

        let mut persisted = collapsed_changed;
        match drag.item {
            ActiveDragItem::SmartList(_) => {
                if self.preferences.smart_list_order != drag.orig_preferences.smart_list_order {
                    let order = self.preferences.smart_list_order.clone();
                    self.save_preferences(|p| p.smart_list_order = order, cx);
                    return;
                }
            }
            ActiveDragItem::SidebarSection(_) => {
                if self.preferences.sidebar_section_order != drag.orig_preferences.sidebar_section_order {
                    let order = self.preferences.sidebar_section_order.clone();
                    self.save_preferences(|p| p.sidebar_section_order = order, cx);
                    return;
                }
            }
            ActiveDragItem::Filter(_) => {
                let order: Vec<Id> = self.filters.iter().map(|(f, _)| f.id).collect();
                let orig_order: Vec<Id> = drag.orig_filters.iter().map(|(f, _)| f.id).collect();
                if order != orig_order {
                    let result = self.store.set_filter_order(&order);
                    self.check(result);
                    persisted = true;
                }
            }
            ActiveDragItem::Folder(_) => {
                if self.lists != drag.orig_lists || self.folders != drag.orig_folders {
                    let result = self.store.save_list_and_folder_order(&self.lists, &self.folders);
                    self.check(result);
                    persisted = true;
                }
            }
            ActiveDragItem::List(dragged_id) => match drag.highlight {
                Some(HighlightTarget::List(target_id)) if target_id != dragged_id => {
                    let name = i18n::t(cx).new_folder;
                    let result = self.store.group_lists_into_folder(dragged_id, target_id, name);
                    if let Some(folder) = self.check(result) {
                        self.reload(cx);
                        self.open_folder_dialog(Some(folder.id), window, cx);
                        return;
                    }
                    persisted = true;
                }
                Some(HighlightTarget::Folder(folder_id)) => {
                    move_list_into_folder_end(&mut self.lists, &self.folders, dragged_id, folder_id);
                    if self.lists != drag.orig_lists || self.folders != drag.orig_folders {
                        let result = self.store.save_list_and_folder_order(&self.lists, &self.folders);
                        self.check(result);
                        persisted = true;
                    }
                }
                _ => {
                    if self.lists != drag.orig_lists || self.folders != drag.orig_folders {
                        let result = self.store.save_list_and_folder_order(&self.lists, &self.folders);
                        self.check(result);
                        persisted = true;
                    }
                }
            },
            ActiveDragItem::Tag(dragged_name) => {
                if let Some(HighlightTarget::Tag(target_name)) = drag.highlight
                    && target_name != dragged_name
                {
                    move_tag_into_parent_end(&mut self.tags, &dragged_name, &target_name);
                    if self.collapsed_tags.remove(&target_name) {
                        self.persist_collapsed_sidebar_sets();
                        persisted = true;
                    }
                }
                if self.tags != drag.orig_tags {
                    let result = self.store.save_tag_hierarchy_and_order(&self.tags);
                    self.check(result);
                    persisted = true;
                }
            }
        }
        if persisted {
            self.reload(cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn lists_panel_content_y(&self, mouse_pos: Point<Pixels>, cx: &mut Context<Self>) -> f32 {
        let viewport = self.lists_scroll.bounds();
        let max_y = self.lists_scroll.max_offset().y;
        if max_y > px(0.)
            && viewport.size.height > px(64.)
            && mouse_pos.x >= viewport.left() - px(24.)
            && mouse_pos.x <= viewport.right() + px(80.)
        {
            let cur = self.lists_scroll.offset();
            let edge = px(28.);
            let next_y = if mouse_pos.y < viewport.top() + edge && cur.y < px(0.) {
                (cur.y + px(8.)).min(px(0.))
            } else if mouse_pos.y > viewport.bottom() - edge && -cur.y < max_y {
                (cur.y - px(8.)).max(-max_y)
            } else {
                cur.y
            };
            if next_y != cur.y {
                self.lists_scroll.set_offset(gpui::point(cur.x, next_y));
                cx.notify();
            }
        }
        f32::from(mouse_pos.y - self.lists_scroll.offset().y)
    }

    fn pointer_in_vacant_row(
        &mut self,
        vacant_ix: Option<usize>,
        content_y: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(v_ix) = vacant_ix else { return false };
        let Some(vb) = self.lists_scroll.bounds_for_item(v_ix) else { return false };
        if content_y < f32::from(vb.top()) - 3. || content_y > f32::from(vb.bottom()) + 3. {
            return false;
        }
        self.drag_expand = None;
        if let Some(drag) = self.panel_drag.as_mut()
            && drag.highlight.take().is_some()
        {
            cx.notify();
        }
        true
    }

    fn panel_drag_hit_row(
        &self,
        range: std::ops::Range<usize>,
        content_y: f32,
    ) -> Option<(usize, f32, f32)> {
        for r_ix in range {
            let Some(rb) = self.lists_scroll.bounds_for_item(r_ix) else { continue };
            let top = f32::from(rb.top());
            let bot = f32::from(rb.bottom());
            if content_y >= top && content_y <= bot {
                return Some((r_ix, top, bot));
            }
        }
        None
    }

    fn finish_hierarchical_drag_move(
        &mut self,
        changed: bool,
        next_highlight: Option<lists_panel::HighlightTarget>,
        keep_expand: bool,
        cx: &mut Context<Self>,
    ) {
        if !keep_expand {
            self.drag_expand = None;
        }
        let highlight_changed = self
            .panel_drag
            .as_mut()
            .is_some_and(|d| std::mem::replace(&mut d.highlight, next_highlight) != d.highlight);
        if changed || highlight_changed {
            cx.notify();
        }
    }

    pub(super) fn move_smart_list_drag(
        &mut self,
        event: &gpui::DragMoveEvent<DraggedSmartList>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use fern_core::SmartList;
        use lists_panel::{ActiveDragItem, PanelChild};
        let dragged = event.drag(cx).list;
        if !self.panel_drag.as_ref().is_some_and(|d| d.item == ActiveDragItem::SmartList(dragged)) {
            return;
        }
        let content_y = self.lists_panel_content_y(event.event.position, cx);
        let visible: Vec<(SmartList, f32)> = {
            let rows = self.lists_rows.borrow();
            rows.iter()
                .enumerate()
                .filter_map(|(ix, r)| match r {
                    PanelChild::SmartList(s) => {
                        let b = self.lists_scroll.bounds_for_item(ix)?;
                        Some((*s, f32::from(b.center().y)))
                    }
                    _ => None,
                })
                .collect()
        };
        let Some(cur_pos) = visible.iter().position(|(s, _)| *s == dragged) else { return };
        let centers: Vec<f32> = visible.iter().map(|(_, c)| *c).collect();
        let pos = hysteresis_slot(&centers, cur_pos, content_y);
        if pos != cur_pos {
            if self.preferences.smart_list_order.is_empty() {
                self.preferences.smart_list_order = SmartList::TOP_DRAGGABLE.to_vec();
            }
            let items: Vec<SmartList> = visible.into_iter().map(|(s, _)| s).collect();
            reorder_visible_subset(&mut self.preferences.smart_list_order, &items, cur_pos, pos);
            cx.notify();
        }
    }

    pub(super) fn move_sidebar_section_drag(
        &mut self,
        event: &gpui::DragMoveEvent<DraggedSidebarSection>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use fern_core::SidebarSection;
        use lists_panel::{ActiveDragItem, PanelChild};
        let dragged = event.drag(cx).section;
        let Some((grab_offset_y, spans)) = self
            .panel_drag
            .as_ref()
            .filter(|d| d.item == ActiveDragItem::SidebarSection(dragged))
            .map(|d| (d.grab_offset_y, d.block_spans.clone()))
        else {
            return;
        };
        let content_y = self.lists_panel_content_y(event.event.position, cx);
        let visible: Vec<(usize, SidebarSection)> = {
            let rows = self.lists_rows.borrow();
            rows.iter()
                .enumerate()
                .filter_map(|(ix, r)| match r {
                    PanelChild::SectionHeading(s) => Some((ix, *s)),
                    _ => None,
                })
                .collect()
        };
        let Some(&(first_ix, _)) = visible.first() else { return };
        let Some(cur_pos) = visible.iter().position(|(_, s)| *s == dragged) else { return };
        let Some(first_b) = self.lists_scroll.bounds_for_item(first_ix) else { return };
        let new_pos = invariant_block_slot(f32::from(first_b.top()), &spans, cur_pos, content_y - grab_offset_y);
        if new_pos != cur_pos {
            if self.preferences.sidebar_section_order.is_empty() {
                self.preferences.sidebar_section_order = SidebarSection::ALL.to_vec();
            }
            let items: Vec<SidebarSection> = visible.into_iter().map(|(_, s)| s).collect();
            reorder_visible_subset(&mut self.preferences.sidebar_section_order, &items, cur_pos, new_pos);
            cx.notify();
        }
    }

    pub(super) fn move_filter_drag(
        &mut self,
        event: &gpui::DragMoveEvent<DraggedFilter>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use lists_panel::{ActiveDragItem, PanelChild};
        let dragged = event.drag(cx).id;
        if !self.panel_drag.as_ref().is_some_and(|d| d.item == ActiveDragItem::Filter(dragged)) {
            return;
        }
        let content_y = self.lists_panel_content_y(event.event.position, cx);
        let (heading_bounds, visible): (Option<gpui::Bounds<Pixels>>, Vec<(Id, f32)>) = {
            let rows = self.lists_rows.borrow();
            let h = rows
                .iter()
                .position(|r| *r == PanelChild::SectionHeading(fern_core::SidebarSection::Filters))
                .and_then(|ix| self.lists_scroll.bounds_for_item(ix));
            let vis = rows
                .iter()
                .enumerate()
                .filter_map(|(ix, r)| match r {
                    PanelChild::Filter(id) => {
                        let b = self.lists_scroll.bounds_for_item(ix)?;
                        Some((*id, f32::from(b.center().y)))
                    }
                    _ => None,
                })
                .collect();
            (h, vis)
        };
        let Some(cur_pos) = visible.iter().position(|(id, _)| *id == dragged) else { return };
        let pos = if heading_bounds.is_some_and(|hb| content_y >= f32::from(hb.top()) && content_y <= f32::from(hb.bottom())) {
            0
        } else {
            let centers: Vec<f32> = visible.iter().map(|(_, c)| *c).collect();
            hysteresis_slot(&centers, cur_pos, content_y)
        };
        if pos != cur_pos
            && let Some(from) = self.filters.iter().position(|(f, _)| f.id == dragged)
        {
            let entry = self.filters.remove(from);
            let insert_at = pos.min(self.filters.len());
            self.filters.insert(insert_at, entry);
            cx.notify();
        }
    }

    pub(super) fn move_folder_drag(
        &mut self,
        event: &gpui::DragMoveEvent<DraggedFolder>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use lists_panel::{ActiveDragItem, PanelChild};
        let dragged_fid = event.drag(cx).id;
        let has_lists = event.drag(cx).has_lists;
        let Some((grab_offset_y, spans)) = self
            .panel_drag
            .as_ref()
            .filter(|d| d.item == ActiveDragItem::Folder(dragged_fid))
            .map(|d| (d.grab_offset_y, d.block_spans.clone()))
        else {
            return;
        };
        let content_y = self.lists_panel_content_y(event.event.position, cx);

        if !has_lists {
            // Empty folders reorder among empty folders at the bottom of Lists.
            let empty_vis: Vec<(Id, f32)> = {
                let rows = self.lists_rows.borrow();
                rows.iter()
                    .enumerate()
                    .filter_map(|(ix, r)| match r {
                        PanelChild::FolderHeader { id, empty: true, .. } => {
                            let b = self.lists_scroll.bounds_for_item(ix)?;
                            Some((*id, f32::from(b.center().y)))
                        }
                        _ => None,
                    })
                    .collect()
            };
            let Some(cur_pos) = empty_vis.iter().position(|(id, _)| *id == dragged_fid) else { return };
            let centers: Vec<f32> = empty_vis.iter().map(|(_, c)| *c).collect();
            let pos = hysteresis_slot(&centers, cur_pos, content_y);
            if pos != cur_pos {
                let empty_set: HashSet<Id> = empty_vis.iter().map(|(id, _)| *id).collect();
                let mut reordered: Vec<Id> = empty_vis.into_iter().map(|(id, _)| id).collect();
                let item = reordered.remove(cur_pos);
                reordered.insert(pos, item);
                let by_id: HashMap<Id, ListFolder> =
                    self.folders.iter().cloned().map(|f| (f.id, f)).collect();
                let mut it = reordered.into_iter();
                for slot in &mut self.folders {
                    if empty_set.contains(&slot.id)
                        && let Some(next_id) = it.next()
                        && let Some(next_folder) = by_id.get(&next_id)
                    {
                        *slot = next_folder.clone();
                    }
                }
                cx.notify();
            }
            return;
        }

        let (first_ix, cur_pos) = {
            let rows = self.lists_rows.borrow();
            let blocks: Vec<(usize, bool)> = rows
                .iter()
                .enumerate()
                .filter_map(|(ix, r)| match r {
                    PanelChild::TopList(_) => Some((ix, false)),
                    PanelChild::FolderHeader { id, empty: false, .. } => Some((ix, *id == dragged_fid)),
                    _ => None,
                })
                .collect();
            let Some(&(first_ix, _)) = blocks.first() else { return };
            let Some(cur_pos) = blocks.iter().position(|&(_, is_d)| is_d) else { return };
            (first_ix, cur_pos)
        };
        let Some(first_b) = self.lists_scroll.bounds_for_item(first_ix) else { return };
        let new_pos = invariant_block_slot(f32::from(first_b.top()), &spans, cur_pos, content_y - grab_offset_y);
        if new_pos != cur_pos {
            reorder_non_empty_folder_in_memory(&mut self.lists, &mut self.folders, dragged_fid, new_pos);
            cx.notify();
        }
    }

    pub(super) fn move_parent_tag_drag(
        &mut self,
        event: &gpui::DragMoveEvent<DraggedParentTag>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use lists_panel::{ActiveDragItem, PanelChild};
        let dragged_name = event.drag(cx).name.to_string();
        let Some((grab_offset_y, spans, orig_pinned)) = self
            .panel_drag
            .as_ref()
            .filter(|d| d.item == ActiveDragItem::Tag(dragged_name.clone()))
            .map(|d| (d.grab_offset_y, d.block_spans.clone(), d.orig_tag_pinned))
        else {
            return;
        };
        let content_y = self.lists_panel_content_y(event.event.position, cx);
        let (heading_bounds, roots) = {
            let rows = self.lists_rows.borrow();
            let h = rows
                .iter()
                .position(|r| *r == PanelChild::SectionHeading(fern_core::SidebarSection::Tags))
                .and_then(|ix| self.lists_scroll.bounds_for_item(ix));
            let vis: Vec<(usize, String, bool)> = rows
                .iter()
                .enumerate()
                .filter_map(|(ix, r)| match r {
                    PanelChild::TopTag { name, pinned, .. } => Some((ix, name.clone(), *pinned)),
                    _ => None,
                })
                .collect();
            (h, vis)
        };
        let Some(&(first_ix, _, _)) = roots.first() else { return };
        let Some(cur_pos) = roots.iter().position(|(_, n, _)| *n == dragged_name) else { return };
        let Some(first_b) = self.lists_scroll.bounds_for_item(first_ix) else { return };
        let new_pos = if heading_bounds.is_some_and(|hb| {
            content_y >= f32::from(hb.top()) && content_y <= f32::from(hb.bottom())
        }) {
            0
        } else {
            invariant_block_slot(f32::from(first_b.top()), &spans, cur_pos, content_y - grab_offset_y)
        };
        if new_pos != cur_pos {
            let visible_others: Vec<(String, bool)> = roots
                .into_iter()
                .filter(|(_, n, _)| *n != dragged_name)
                .map(|(_, n, p)| (n, p))
                .collect();
            reorder_parent_tag_in_memory(&mut self.tags, &dragged_name, &visible_others, new_pos, orig_pinned);
            cx.notify();
        }
    }

    pub(super) fn move_list_drag(
        &mut self,
        event: &gpui::DragMoveEvent<DraggedList>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use lists_panel::{ActiveDragItem, HighlightTarget, PanelChild};
        let dragged_id = event.drag(cx).id;
        if !self.panel_drag.as_ref().is_some_and(|d| d.item == ActiveDragItem::List(dragged_id)) {
            return;
        }
        let content_y = self.lists_panel_content_y(event.event.position, cx);
        let rows_snapshot: Vec<PanelChild> = self.lists_rows.borrow().clone();

        let vacant_ix = rows_snapshot.iter().position(|r| {
            matches!(r, PanelChild::TopList(id) | PanelChild::SubList { id, .. } if *id == dragged_id)
        });
        if self.pointer_in_vacant_row(vacant_ix, content_y, cx) {
            return;
        }

        let Some(heading_ix) = rows_snapshot
            .iter()
            .position(|r| *r == PanelChild::SectionHeading(fern_core::SidebarSection::Lists))
        else {
            return;
        };
        let active_end_ix = section_end_ix(&rows_snapshot, heading_ix, true);

        let mut next_highlight: Option<HighlightTarget> = None;
        let mut keep_expand = false;
        let mut lists_changed = false;

        let cur_folder = self.lists.iter().find(|l| l.id == dragged_id).and_then(|l| l.folder);
        let can_slide_adjacent = cur_folder.is_none_or(|fid| {
            self.lists.iter().any(|l| l.id != dragged_id && !l.archived && l.folder == Some(fid))
        });

        if let Some(hb) = self.lists_scroll.bounds_for_item(heading_ix)
            && content_y <= f32::from(hb.bottom())
        {
            lists_changed = move_list_first_in_memory(&mut self.lists, &self.folders, dragged_id);
        } else if active_end_ix > heading_ix + 1
            && let Some(last_b) = self.lists_scroll.bounds_for_item(active_end_ix - 1)
            && content_y > f32::from(last_b.bottom())
        {
            lists_changed = move_list_last_top_in_memory(&mut self.lists, &self.folders, dragged_id);
        } else if let Some((r_ix, top, bot)) = self.panel_drag_hit_row((heading_ix + 1)..active_end_ix, content_y) {
            let h = bot - top;
            let mid = (top + bot) * 0.5;
            let rel_y = content_y - top;
            let vacant_above = vacant_ix.is_none_or(|v| v < r_ix);

            match rows_snapshot[r_ix] {
                PanelChild::TopList(target_id) if target_id != dragged_id => {
                    match row_zone(rel_y, h, 9.5) {
                        RowZone::Before => {
                            lists_changed = move_list_relative_in_memory(
                                &mut self.lists,
                                &self.folders,
                                dragged_id,
                                target_id,
                                false,
                                None,
                            );
                        }
                        RowZone::Middle => {
                            next_highlight = Some(HighlightTarget::List(target_id));
                            if can_slide_adjacent {
                                lists_changed = move_list_relative_in_memory(
                                    &mut self.lists,
                                    &self.folders,
                                    dragged_id,
                                    target_id,
                                    !vacant_above,
                                    None,
                                );
                            }
                        }
                        RowZone::After => {
                            lists_changed = move_list_relative_in_memory(
                                &mut self.lists,
                                &self.folders,
                                dragged_id,
                                target_id,
                                true,
                                None,
                            );
                        }
                    }
                }
                PanelChild::FolderHeader { id: folder_id, collapsed, empty } => {
                    if collapsed {
                        match row_zone(rel_y, h, 9.) {
                            RowZone::Before => {
                                lists_changed = move_list_around_folder_in_memory(
                                    &mut self.lists,
                                    &self.folders,
                                    dragged_id,
                                    folder_id,
                                    false,
                                );
                            }
                            RowZone::Middle => {
                                next_highlight = Some(HighlightTarget::Folder(folder_id));
                                keep_expand = true;
                                self.schedule_drag_expand(DragExpandTarget::Folder(folder_id), cx);
                                if !empty && can_slide_adjacent {
                                    lists_changed = move_list_around_folder_in_memory(
                                        &mut self.lists,
                                        &self.folders,
                                        dragged_id,
                                        folder_id,
                                        !vacant_above,
                                    );
                                }
                            }
                            RowZone::After => {
                                lists_changed = move_list_around_folder_in_memory(
                                    &mut self.lists,
                                    &self.folders,
                                    dragged_id,
                                    folder_id,
                                    true,
                                );
                            }
                        }
                    } else if rel_y < 9. {
                        lists_changed = move_list_around_folder_in_memory(
                            &mut self.lists,
                            &self.folders,
                            dragged_id,
                            folder_id,
                            false,
                        );
                    } else {
                        lists_changed = move_list_into_folder_start(
                            &mut self.lists,
                            &self.folders,
                            dragged_id,
                            folder_id,
                        );
                    }
                }
                PanelChild::SubList { id: sub_id, folder: folder_id } if sub_id != dragged_id => {
                    let same_folder = |ix: usize| {
                        matches!(
                            rows_snapshot.get(ix),
                            Some(PanelChild::SubList { folder, .. }) if *folder == folder_id
                        )
                    };
                    let adj_before = vacant_ix == Some(r_ix.saturating_sub(1)) && same_folder(r_ix.saturating_sub(1));
                    let adj_after = vacant_ix == Some(r_ix + 1) && same_folder(r_ix + 1);
                    if let Some(after) = adjacent_sub_after(content_y, mid, adj_before, adj_after) {
                        lists_changed = move_list_relative_in_memory(
                            &mut self.lists,
                            &self.folders,
                            dragged_id,
                            sub_id,
                            after,
                            Some(folder_id),
                        );
                    }
                }
                _ => {}
            }
        }

        self.finish_hierarchical_drag_move(lists_changed, next_highlight, keep_expand, cx);
    }

    pub(super) fn move_tag_drag(
        &mut self,
        event: &gpui::DragMoveEvent<DraggedTag>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use lists_panel::{ActiveDragItem, HighlightTarget, PanelChild};
        let dragged_name = event.drag(cx).name.to_string();
        if !self.panel_drag.as_ref().is_some_and(|d| d.item == ActiveDragItem::Tag(dragged_name.clone())) {
            return;
        }
        let content_y = self.lists_panel_content_y(event.event.position, cx);
        let rows_snapshot: Vec<PanelChild> = self.lists_rows.borrow().clone();

        let vacant_ix = rows_snapshot.iter().position(|r| match r {
            PanelChild::TopTag { name, .. } | PanelChild::SubTag { name, .. } => *name == dragged_name,
            _ => false,
        });
        if self.pointer_in_vacant_row(vacant_ix, content_y, cx) {
            return;
        }

        let Some(heading_ix) = rows_snapshot
            .iter()
            .position(|r| *r == PanelChild::SectionHeading(fern_core::SidebarSection::Tags))
        else {
            return;
        };
        let tags_end_ix = section_end_ix(&rows_snapshot, heading_ix, false);

        let mut next_highlight: Option<HighlightTarget> = None;
        let mut keep_expand = false;
        let mut tags_changed = false;

        let cur_parent = self.tags.iter().find(|t| t.name == dragged_name).and_then(|t| t.parent.clone());
        let can_slide_adjacent = cur_parent.is_none()
            || self.preferences.visibility(fern_core::SmartList::Tags)
                != fern_core::Visibility::IfNotEmpty;

        if let Some(hb) = self.lists_scroll.bounds_for_item(heading_ix)
            && content_y <= f32::from(hb.bottom())
        {
            tags_changed = move_tag_first_in_memory(&mut self.tags, &dragged_name);
        } else if !self.tags_collapsed
            && tags_end_ix > heading_ix + 1
            && let Some(last_b) = self.lists_scroll.bounds_for_item(tags_end_ix - 1)
            && content_y > f32::from(last_b.bottom())
        {
            tags_changed = move_tag_last_top_in_memory(&mut self.tags, &dragged_name);
        } else if let Some((r_ix, top, bot)) = self.panel_drag_hit_row((heading_ix + 1)..tags_end_ix, content_y) {
            let h = bot - top;
            let mid = (top + bot) * 0.5;
            let rel_y = content_y - top;
            let vacant_above = vacant_ix.is_none_or(|v| v < r_ix);

            match &rows_snapshot[r_ix] {
                PanelChild::TopTag { name: target_name, is_parent: false, .. }
                    if *target_name != dragged_name =>
                {
                    match row_zone(rel_y, h, 9.5) {
                        RowZone::Before => {
                            tags_changed = move_tag_around_root_in_memory(
                                &mut self.tags,
                                &dragged_name,
                                target_name,
                                false,
                            );
                        }
                        RowZone::Middle => {
                            next_highlight = Some(HighlightTarget::Tag(target_name.clone()));
                            if can_slide_adjacent {
                                tags_changed = move_tag_around_root_in_memory(
                                    &mut self.tags,
                                    &dragged_name,
                                    target_name,
                                    !vacant_above,
                                );
                            }
                        }
                        RowZone::After => {
                            tags_changed = move_tag_around_root_in_memory(
                                &mut self.tags,
                                &dragged_name,
                                target_name,
                                true,
                            );
                        }
                    }
                }
                PanelChild::TopTag { name: target_name, is_parent: true, collapsed, .. }
                    if *target_name != dragged_name =>
                {
                    if *collapsed {
                        match row_zone(rel_y, h, 9.5) {
                            RowZone::Before => {
                                tags_changed = move_tag_around_root_in_memory(
                                    &mut self.tags,
                                    &dragged_name,
                                    target_name,
                                    false,
                                );
                            }
                            RowZone::Middle => {
                                next_highlight = Some(HighlightTarget::Tag(target_name.clone()));
                                keep_expand = true;
                                self.schedule_drag_expand(DragExpandTarget::Tag(target_name.clone()), cx);
                                if can_slide_adjacent {
                                    tags_changed = move_tag_around_root_in_memory(
                                        &mut self.tags,
                                        &dragged_name,
                                        target_name,
                                        !vacant_above,
                                    );
                                }
                            }
                            RowZone::After => {
                                tags_changed = move_tag_around_root_in_memory(
                                    &mut self.tags,
                                    &dragged_name,
                                    target_name,
                                    true,
                                );
                            }
                        }
                    } else if rel_y < 9.5 {
                        tags_changed = move_tag_around_root_in_memory(
                            &mut self.tags,
                            &dragged_name,
                            target_name,
                            false,
                        );
                    } else {
                        tags_changed = move_tag_into_parent_start(
                            &mut self.tags,
                            &dragged_name,
                            target_name,
                        );
                    }
                }
                PanelChild::SubTag { name: sub_name, parent: parent_name }
                    if *sub_name != dragged_name =>
                {
                    let same_parent = |ix: usize| {
                        matches!(
                            rows_snapshot.get(ix),
                            Some(PanelChild::SubTag { parent, .. }) if parent == parent_name
                        )
                    };
                    let adj_before = vacant_ix == Some(r_ix.saturating_sub(1)) && same_parent(r_ix.saturating_sub(1));
                    let adj_after = vacant_ix == Some(r_ix + 1) && same_parent(r_ix + 1);
                    if let Some(after) = adjacent_sub_after(content_y, mid, adj_before, adj_after) {
                        tags_changed = move_subtag_relative_in_memory(
                            &mut self.tags,
                            &dragged_name,
                            sub_name,
                            parent_name,
                            after,
                        );
                    }
                }
                _ => {}
            }
        }

        self.finish_hierarchical_drag_move(tags_changed, next_highlight, keep_expand, cx);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowZone {
    Before,
    Middle,
    After,
}

fn row_zone(rel_y: f32, height: f32, edge: f32) -> RowZone {
    if rel_y < edge {
        RowZone::Before
    } else if rel_y <= height - edge {
        RowZone::Middle
    } else {
        RowZone::After
    }
}

fn adjacent_sub_after(content_y: f32, mid: f32, adj_before: bool, adj_after: bool) -> Option<bool> {
    if adj_before {
        (content_y > mid + 4.).then_some(true)
    } else if adj_after {
        (content_y < mid - 4.).then_some(false)
    } else {
        Some(content_y >= mid)
    }
}

fn hysteresis_slot(centers: &[f32], cur_pos: usize, content_y: f32) -> usize {
    let mut pos = cur_pos;
    while pos > 0 && content_y < centers[pos - 1] - 4. {
        pos -= 1;
    }
    while pos + 1 < centers.len() && content_y > centers[pos + 1] + 4. {
        pos += 1;
    }
    pos
}

fn reorder_visible_subset<T: Copy + Eq + std::hash::Hash>(
    order: &mut [T],
    visible: &[T],
    from: usize,
    to: usize,
) {
    let vis_set: HashSet<T> = visible.iter().copied().collect();
    let mut reordered: Vec<T> = visible.to_vec();
    let item = reordered.remove(from);
    reordered.insert(to.min(reordered.len()), item);
    let mut it = reordered.into_iter();
    for slot in order {
        if vis_set.contains(slot)
            && let Some(next) = it.next()
        {
            *slot = next;
        }
    }
}

fn section_end_ix(rows: &[lists_panel::PanelChild], heading_ix: usize, stop_at_archived: bool) -> usize {
    use lists_panel::PanelChild;
    rows.iter()
        .enumerate()
        .skip(heading_ix + 1)
        .find_map(|(ix, r)| {
            let is_boundary = matches!(
                r,
                PanelChild::SectionHeading(_) | PanelChild::Separator | PanelChild::BottomSmart(_)
            ) || (stop_at_archived && matches!(r, PanelChild::ArchivedHeader));
            is_boundary.then_some(ix)
        })
        .unwrap_or(rows.len())
}

fn invariant_block_slot(first_top: f32, spans: &[f32], cur_slot: usize, drag_top_y: f32) -> usize {
    let mut thresholds = Vec::with_capacity(spans.len());
    let mut acc = first_top;
    for &span in spans {
        thresholds.push(acc + span * 0.5);
        acc += span;
    }
    let mut slot = cur_slot.min(spans.len());
    while slot > 0 && drag_top_y < thresholds[slot - 1] - 5. {
        slot -= 1;
    }
    while slot < thresholds.len() && drag_top_y > thresholds[slot] + 5. {
        slot += 1;
    }
    slot
}

fn normalize_lists_in_memory(lists: &mut Vec<List>, folders: &[ListFolder]) {
    let valid_folders: HashSet<Id> = folders.iter().map(|f| f.id).collect();
    let mut out = Vec::with_capacity(lists.len());
    let mut shown_folders: HashSet<Id> = HashSet::new();
    for list in lists.iter().filter(|l| !l.archived) {
        match list.folder.filter(|f| valid_folders.contains(f)) {
            None => {
                let mut item = list.clone();
                item.folder = None;
                out.push(item);
            }
            Some(fid) if shown_folders.insert(fid) => {
                out.extend(
                    lists
                        .iter()
                        .filter(|l| !l.archived && l.folder == Some(fid))
                        .cloned(),
                );
            }
            Some(_) => {}
        }
    }
    out.extend(lists.iter().filter(|l| l.archived).cloned());
    *lists = out;
}

fn move_list_first_in_memory(lists: &mut Vec<List>, folders: &[ListFolder], dragged_id: Id) -> bool {
    let before = lists.clone();
    normalize_lists_in_memory(lists, folders);
    let Some(pos) = lists.iter().position(|l| l.id == dragged_id) else { return false };
    let mut dragged = lists.remove(pos);
    dragged.folder = None;
    lists.insert(0, dragged);
    normalize_lists_in_memory(lists, folders);
    *lists != before
}

fn move_list_last_top_in_memory(lists: &mut Vec<List>, folders: &[ListFolder], dragged_id: Id) -> bool {
    let before = lists.clone();
    normalize_lists_in_memory(lists, folders);
    let Some(pos) = lists.iter().position(|l| l.id == dragged_id) else { return false };
    let mut dragged = lists.remove(pos);
    dragged.folder = None;
    let at = lists.iter().rposition(|l| !l.archived).map_or(0, |p| p + 1);
    lists.insert(at, dragged);
    normalize_lists_in_memory(lists, folders);
    *lists != before
}

fn move_list_relative_in_memory(
    lists: &mut Vec<List>,
    folders: &[ListFolder],
    dragged_id: Id,
    target_id: Id,
    after: bool,
    folder: Option<Id>,
) -> bool {
    if dragged_id == target_id {
        return false;
    }
    let before = lists.clone();
    normalize_lists_in_memory(lists, folders);
    let Some(from) = lists.iter().position(|l| l.id == dragged_id) else { return false };
    let mut dragged = lists.remove(from);
    dragged.folder = folder;
    let Some(to) = lists.iter().position(|l| l.id == target_id) else {
        lists.insert(from.min(lists.len()), dragged);
        return false;
    };
    lists.insert(to + usize::from(after), dragged);
    normalize_lists_in_memory(lists, folders);
    *lists != before
}

fn move_list_around_folder_in_memory(
    lists: &mut Vec<List>,
    folders: &[ListFolder],
    dragged_id: Id,
    folder_id: Id,
    after: bool,
) -> bool {
    let before = lists.clone();
    normalize_lists_in_memory(lists, folders);
    let Some(from) = lists.iter().position(|l| l.id == dragged_id) else { return false };
    let was_in_folder = lists[from].folder == Some(folder_id);
    let mut dragged = lists.remove(from);
    dragged.folder = None;
    let first = lists.iter().position(|l| !l.archived && l.folder == Some(folder_id));
    let last = lists.iter().rposition(|l| !l.archived && l.folder == Some(folder_id));
    let at = match (first, last) {
        (Some(f), Some(l)) => if !after { f } else { l + 1 },
        _ if was_in_folder => from.min(lists.len()),
        _ => lists.iter().rposition(|l| !l.archived).map_or(0, |p| p + 1),
    };
    lists.insert(at, dragged);
    normalize_lists_in_memory(lists, folders);
    *lists != before
}

fn move_list_into_folder_start(
    lists: &mut Vec<List>,
    folders: &[ListFolder],
    dragged_id: Id,
    folder_id: Id,
) -> bool {
    let before = lists.clone();
    normalize_lists_in_memory(lists, folders);
    if let Some(first_other) = lists
        .iter()
        .find(|l| l.id != dragged_id && !l.archived && l.folder == Some(folder_id))
        .map(|l| l.id)
    {
        return move_list_relative_in_memory(lists, folders, dragged_id, first_other, false, Some(folder_id));
    }
    let Some(from) = lists.iter().position(|l| l.id == dragged_id) else { return false };
    if lists[from].folder == Some(folder_id) {
        return false;
    }
    let mut dragged = lists.remove(from);
    dragged.folder = Some(folder_id);
    let at = lists.iter().rposition(|l| !l.archived).map_or(0, |p| p + 1);
    lists.insert(at, dragged);
    normalize_lists_in_memory(lists, folders);
    *lists != before
}

fn move_list_into_folder_end(
    lists: &mut Vec<List>,
    folders: &[ListFolder],
    dragged_id: Id,
    folder_id: Id,
) -> bool {
    let before = lists.clone();
    normalize_lists_in_memory(lists, folders);
    if let Some(last_other) = lists
        .iter()
        .rev()
        .find(|l| l.id != dragged_id && !l.archived && l.folder == Some(folder_id))
        .map(|l| l.id)
    {
        return move_list_relative_in_memory(lists, folders, dragged_id, last_other, true, Some(folder_id));
    }
    let Some(from) = lists.iter().position(|l| l.id == dragged_id) else { return false };
    let mut dragged = lists.remove(from);
    dragged.folder = Some(folder_id);
    let at = lists.iter().rposition(|l| !l.archived).map_or(0, |p| p + 1);
    lists.insert(at, dragged);
    normalize_lists_in_memory(lists, folders);
    *lists != before
}

fn reorder_non_empty_folder_in_memory(
    lists: &mut Vec<List>,
    folders: &mut [ListFolder],
    dragged_fid: Id,
    new_pos: usize,
) {
    normalize_lists_in_memory(lists, folders);
    let valid_folders: HashSet<Id> = folders.iter().map(|f| f.id).collect();
    let moving: Vec<List> = lists
        .iter()
        .filter(|l| !l.archived && l.folder == Some(dragged_fid))
        .cloned()
        .collect();
    if moving.is_empty() {
        return;
    }

    let mut blocks: Vec<(Option<Id>, Vec<List>)> = Vec::new();
    let mut seen_folders: HashSet<Id> = HashSet::new();
    for list in lists.iter().filter(|l| !l.archived && l.folder != Some(dragged_fid)) {
        match list.folder.filter(|f| valid_folders.contains(f)) {
            None => blocks.push((None, vec![list.clone()])),
            Some(fid) if seen_folders.insert(fid) => {
                let members: Vec<List> = lists
                    .iter()
                    .filter(|l| !l.archived && l.folder == Some(fid))
                    .cloned()
                    .collect();
                blocks.push((Some(fid), members));
            }
            Some(_) => {}
        }
    }
    let at = new_pos.min(blocks.len());
    blocks.insert(at, (Some(dragged_fid), moving));

    let archived: Vec<List> = lists.iter().filter(|l| l.archived).cloned().collect();
    let mut rebuilt = Vec::with_capacity(lists.len());
    let mut non_empty_folder_order = Vec::new();
    for (fid_opt, members) in blocks {
        if let Some(fid) = fid_opt {
            non_empty_folder_order.push(fid);
        }
        rebuilt.extend(members);
    }
    rebuilt.extend(archived);
    *lists = rebuilt;

    let non_empty_set: HashSet<Id> = non_empty_folder_order.iter().copied().collect();
    let by_id: HashMap<Id, ListFolder> = folders.iter().cloned().map(|f| (f.id, f)).collect();
    let mut it = non_empty_folder_order.into_iter();
    for slot in folders.iter_mut() {
        if non_empty_set.contains(&slot.id)
            && let Some(next_id) = it.next()
            && let Some(next_f) = by_id.get(&next_id)
        {
            *slot = next_f.clone();
        }
    }
}

fn normalize_tags_in_memory(tags: &mut Vec<Tag>) {
    let raw_roots: HashSet<String> = tags
        .iter()
        .filter(|t| t.parent.is_none() || !tags.iter().any(|p| Some(&p.name) == t.parent.as_ref()))
        .map(|t| t.name.clone())
        .collect();
    for tag in tags.iter_mut() {
        if tag.parent.as_ref().is_some_and(|p| p == &tag.name || !raw_roots.contains(p)) {
            tag.parent = None;
        }
        if tag.parent.is_some() {
            tag.pinned = false;
        }
    }
    let pinned_roots: Vec<Tag> = tags.iter().filter(|t| t.parent.is_none() && t.pinned).cloned().collect();
    let unpinned_roots: Vec<Tag> = tags.iter().filter(|t| t.parent.is_none() && !t.pinned).cloned().collect();
    let mut out = Vec::with_capacity(tags.len());
    for root in pinned_roots.into_iter().chain(unpinned_roots) {
        let root_name = root.name.clone();
        out.push(root);
        out.extend(tags.iter().filter(|c| c.parent.as_deref() == Some(root_name.as_str())).cloned());
    }
    *tags = out;
}

fn move_tag_first_in_memory(tags: &mut Vec<Tag>, dragged_name: &str) -> bool {
    let before = tags.clone();
    normalize_tags_in_memory(tags);
    let first_pinned = tags
        .iter()
        .find(|t| t.name != dragged_name && t.parent.is_none())
        .is_some_and(|t| t.pinned);
    let Some(pos) = tags.iter().position(|t| t.name == dragged_name) else { return false };
    let mut dragged = tags.remove(pos);
    dragged.parent = None;
    dragged.pinned = first_pinned;
    tags.insert(0, dragged);
    normalize_tags_in_memory(tags);
    *tags != before
}

fn move_tag_last_top_in_memory(tags: &mut Vec<Tag>, dragged_name: &str) -> bool {
    let before = tags.clone();
    normalize_tags_in_memory(tags);
    let last_pinned = tags
        .iter()
        .rev()
        .find(|t| t.name != dragged_name && t.parent.is_none())
        .is_some_and(|t| t.pinned);
    let Some(pos) = tags.iter().position(|t| t.name == dragged_name) else { return false };
    let mut dragged = tags.remove(pos);
    dragged.parent = None;
    dragged.pinned = last_pinned;
    tags.push(dragged);
    normalize_tags_in_memory(tags);
    *tags != before
}

fn move_tag_around_root_in_memory(
    tags: &mut Vec<Tag>,
    dragged_name: &str,
    target_root: &str,
    after: bool,
) -> bool {
    if dragged_name == target_root {
        return false;
    }
    let before = tags.clone();
    normalize_tags_in_memory(tags);
    let Some(target_pinned) = tags.iter().find(|t| t.name == target_root).map(|t| t.pinned) else {
        return false;
    };
    let Some(from) = tags.iter().position(|t| t.name == dragged_name) else { return false };
    let mut dragged = tags.remove(from);
    dragged.parent = None;
    dragged.pinned = target_pinned;
    let insert_at = if !after {
        tags.iter().position(|t| t.name == target_root).unwrap_or(tags.len())
    } else {
        tags.iter()
            .rposition(|t| t.name == target_root || t.parent.as_deref() == Some(target_root))
            .map_or(tags.len(), |p| p + 1)
    };
    tags.insert(insert_at, dragged);
    normalize_tags_in_memory(tags);
    *tags != before
}

fn move_tag_into_parent_start(tags: &mut Vec<Tag>, dragged_name: &str, parent_name: &str) -> bool {
    if dragged_name == parent_name {
        return false;
    }
    let before = tags.clone();
    normalize_tags_in_memory(tags);
    if tags.iter().any(|t| t.parent.as_deref() == Some(dragged_name)) {
        return false;
    }
    let Some(from) = tags.iter().position(|t| t.name == dragged_name) else { return false };
    let mut dragged = tags.remove(from);
    dragged.parent = Some(parent_name.to_owned());
    dragged.pinned = false;
    let at = tags.iter().position(|t| t.name == parent_name).map_or(tags.len(), |p| p + 1);
    tags.insert(at, dragged);
    normalize_tags_in_memory(tags);
    *tags != before
}

fn move_tag_into_parent_end(tags: &mut Vec<Tag>, dragged_name: &str, parent_name: &str) -> bool {
    if dragged_name == parent_name {
        return false;
    }
    let before = tags.clone();
    normalize_tags_in_memory(tags);
    if tags.iter().any(|t| t.parent.as_deref() == Some(dragged_name)) {
        return false;
    }
    let Some(from) = tags.iter().position(|t| t.name == dragged_name) else { return false };
    let mut dragged = tags.remove(from);
    dragged.parent = Some(parent_name.to_owned());
    dragged.pinned = false;
    let at = tags
        .iter()
        .rposition(|t| t.name == parent_name || t.parent.as_deref() == Some(parent_name))
        .map_or(tags.len(), |p| p + 1);
    tags.insert(at, dragged);
    normalize_tags_in_memory(tags);
    *tags != before
}

fn move_subtag_relative_in_memory(
    tags: &mut Vec<Tag>,
    dragged_name: &str,
    target_sub: &str,
    parent_name: &str,
    after: bool,
) -> bool {
    if dragged_name == target_sub || dragged_name == parent_name {
        return false;
    }
    let before = tags.clone();
    normalize_tags_in_memory(tags);
    if tags.iter().any(|t| t.parent.as_deref() == Some(dragged_name)) {
        return false;
    }
    let Some(from) = tags.iter().position(|t| t.name == dragged_name) else { return false };
    let mut dragged = tags.remove(from);
    dragged.parent = Some(parent_name.to_owned());
    dragged.pinned = false;
    let Some(to) = tags.iter().position(|t| t.name == target_sub) else {
        tags.insert(from.min(tags.len()), dragged);
        return false;
    };
    tags.insert(to + usize::from(after), dragged);
    normalize_tags_in_memory(tags);
    *tags != before
}

fn reorder_parent_tag_in_memory(
    tags: &mut Vec<Tag>,
    dragged_name: &str,
    visible_others: &[(String, bool)],
    new_pos: usize,
    orig_pinned: bool,
) {
    normalize_tags_in_memory(tags);
    let pinned_count = visible_others.iter().filter(|(_, p)| *p).count();
    let new_pinned = if orig_pinned {
        new_pos <= pinned_count
    } else {
        new_pos < pinned_count
    };

    let mut dragged_family: Vec<Tag> = tags
        .iter()
        .filter(|t| t.name == dragged_name || t.parent.as_deref() == Some(dragged_name))
        .cloned()
        .collect();
    let Some(root) = dragged_family.first_mut() else { return };
    root.parent = None;
    root.pinned = new_pinned;

    tags.retain(|t| t.name != dragged_name && t.parent.as_deref() != Some(dragged_name));

    let clamped = new_pos.min(visible_others.len());
    let insert_at = if clamped < visible_others.len() {
        let target_name = &visible_others[clamped].0;
        tags.iter().position(|t| t.name == *target_name).unwrap_or(tags.len())
    } else if let Some((last_vis, _)) = visible_others.last() {
        tags.iter()
            .rposition(|t| t.name == *last_vis || t.parent.as_deref() == Some(last_vis.as_str()))
            .map_or(tags.len(), |p| p + 1)
    } else {
        0
    };
    tags.splice(insert_at..insert_at, dragged_family);
    normalize_tags_in_memory(tags);
}

