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
}

pub(super) fn normalize_lists_in_memory(lists: &mut Vec<List>, folders: &[ListFolder]) {
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

pub(super) fn move_list_relative_in_memory(
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

pub(super) fn move_list_into_folder_start(
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

pub(super) fn normalize_tags_in_memory(tags: &mut Vec<Tag>) {
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

pub(super) fn move_tag_into_parent_start(tags: &mut Vec<Tag>, dragged_name: &str, parent_name: &str) -> bool {
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

