use super::*;
use menus::RangeEnd;

impl TodoView {
    /// Debug builds only: puts the UI into a given state without clicks, for screenshots.
    /// `FERN_DEV_STATE="add=<quick-add text>;list=<new list>;view=<list name>;select=<title part>;
    /// popup=due|priority|tags|list;menu=header;new-list=1"`
    pub(super) fn apply_dev_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(state) = std::env::var("FERN_DEV_STATE") else { return };
        for part in state.split(';') {
            match part.split_once('=') {
                Some(("add", text)) => {
                    self.add_task(text, cx);
                    self.reload(cx);
                }
                Some(("list", name)) => {
                    let result = self.store.create_list(name);
                    if let Some(list) = self.check(result) {
                        self.set_view(View::List(list.id), cx);
                    } else {
                        self.reload(cx);
                    }
                }
                Some(("view", "trash")) => self.set_view(View::Trash, cx),
                Some(("view", "wontdo")) => self.set_view(View::WontDo, cx),
                Some(("view", "today")) => self.set_view(View::Today, cx),
                Some(("view", "horizons")) => self.set_view(View::Horizons, cx),
                Some(("view", "summary")) => self.set_view(View::Summary, cx),
                Some(("horizons-mode", spec)) => self.dev_horizons_mode(spec, cx),
                Some(("view", "next7")) => self.set_view(View::Next7Days, cx),
                Some(("view", "completed")) => self.set_view(View::Completed, cx),
                Some(("view", name)) if name.starts_with('#') => self.set_view(View::Tag(name[1..].to_owned()), cx),
                Some(("view", "tomorrow")) => self.set_view(View::Tomorrow, cx),
                // `type=<text>`: text in the quick-add field, as if typed (not added).
                Some(("type", text)) => {
                    self.input.update(cx, |input, cx| input.set_text(text.to_owned(), cx));
                    self.update_quick_add_hint(cx);
                }
                // `mini=month`: the mini calendar expanded to the month.
                Some(("mini", "month")) => self.mini_expanded = true,
                // `view=day+N`: the mini calendar's day N days from today.
                Some(("view", day)) if day.starts_with("day") => {
                    let offset: i64 = day.trim_start_matches("day").trim_start_matches('+').parse().unwrap_or(0);
                    self.set_view(View::Day(Self::today() + Duration::days(offset)), cx);
                }
                // `summary=<period|status|grouping|fields>` opens a Summary dropdown.
                Some(("summary", menu)) => self.summary.update(cx, |summary, cx| summary.dev_open(menu, window, cx)),
                Some(("drop", spec)) => {
                    // `drop=<title part>><today|tomorrow|inbox|completed|trash|list:Name|tag:Name>`:
                    // the same path as dropping a task on the lists column.
                    let all = self.store.tasks().unwrap_or_default();
                    if let Some((needle, target)) = spec.split_once('>')
                        && let Some(id) = all.iter().find(|t| t.title.contains(needle)).map(|t| t.id)
                    {
                        let view = match target {
                            "today" => Some(View::Today),
                            "tomorrow" => Some(View::Tomorrow),
                            "inbox" => Some(View::Inbox),
                            "completed" => Some(View::Completed),
                            "wontdo" => Some(View::WontDo),
                            "trash" => Some(View::Trash),
                            other => other
                                .strip_prefix("tag:")
                                .map(|t| View::Tag(t.to_owned()))
                                .or_else(|| {
                                    let name = other.strip_prefix("list:")?;
                                    self.lists.iter().find(|l| l.name == name).map(|l| View::List(l.id))
                                }),
                        };
                        if let Some(view) = view {
                            self.drop_on_view(view, id, cx);
                        }
                    }
                }
                Some(("move", spec)) => {
                    // `move=<dragged title part>><target title part>`: same path as a mouse drop.
                    let find = |needle: &str| self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id);
                    if let Some((dragged, target)) = spec.split_once('>')
                        && let (Some(dragged), Some(target)) = (find(dragged), find(target))
                    {
                        self.drop_task(dragged, target, cx);
                    }
                }
                Some(("span", spec)) => {
                    // `span=<title part>><days from today> [HH:MM]><days from today> [HH:MM]`.
                    let at = |part: &str| {
                        let mut words = part.split_whitespace();
                        let days: i64 = words.next()?.parse().ok()?;
                        let time = words.next().and_then(crate::due_picker::parse_time);
                        Some((Self::today() + Duration::days(days), time))
                    };
                    let parts: Vec<&str> = spec.split('>').collect();
                    if let [needle, start, end] = parts[..]
                        && let (Some((start, start_time)), Some(end)) = (at(start), at(end))
                        && let Some(id) = self.store.tasks().unwrap_or_default().iter().find(|t| t.title.contains(needle)).map(|t| t.id)
                    {
                        self.store.set_due(id, Some(start), start_time).ok();
                        self.store.set_due_end(id, Some(end)).ok();
                        self.reload(cx);
                    }
                }
                Some(("search", query)) => {
                    let query = query.to_owned();
                    self.search_input.update(cx, |input, cx| input.set_text(query, cx));
                    self.start_search(window, cx);
                }
                Some(("view", name)) => {
                    if let Some(id) = self.lists.iter().find(|l| l.name == name).map(|l| l.id) {
                        self.set_view(View::List(id), cx);
                    }
                }
                Some(("list-section", spec)) => {
                    // `list-section=<name>` adds a section to the current list; `list-section=<name>><task>` also files a task.
                    let (name, task) = spec.split_once('>').map_or((spec, None), |(n, t)| (n, Some(t)));
                    if let Some(list) = self.section_owner() {
                        let existing = self.store.list_sections(list).unwrap_or_default().into_iter().find(|s| s.name == name);
                        let section = existing.or_else(|| self.store.add_list_section(list, name).ok());
                        if let (Some(section), Some(task)) = (section, task)
                            && let Some(id) = self.tasks.iter().find(|t| t.title.contains(task)).map(|t| t.id)
                        {
                            self.store.set_section(id, Some(section.id)).ok();
                        }
                        self.reload(cx);
                    }
                }
                Some(("kdrop", spec)) => {
                    // `kdrop=<task title part>><column number>`: drop a card on a kanban column.
                    let strings = i18n::t(cx);
                    let columns = self.board_columns(strings);
                    if let Some((needle, column)) = spec.split_once('>')
                        && let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id)
                        && let Some(target) = column.parse::<usize>().ok().and_then(|c| columns.get(c)).and_then(|col| col.drop.clone())
                    {
                        self.drop_on_column(id, target, cx);
                    }
                }
                Some(("column-menu", name)) => {
                    self.column_menu = self.sections.iter().find(|s| s.name == name).map(|s| (s.id, false));
                }
                Some(("section-menu", name)) => {
                    self.section_menu = self.sections.iter().find(|s| s.name == name).map(|s| s.id);
                }
                Some(("template", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        self.store.save_template(id).ok();
                    }
                }
                Some(("menu", "templates")) => {
                    self.cached_templates = self.store.templates().unwrap_or_default();
                    self.templates_open = true;
                }
                Some(("menu", "input-box")) => self.input_box_open = true,
                Some(("menu", "timeline")) => self.set_layout(false, true, cx),
                Some(("menu", "kanban")) => self.set_layout(true, false, cx),
                Some(("menu", "view-options")) => self.view_options = Some(false),
                Some(("menu", "suggestions")) => {
                    self.suggestions_open = Some(false);
                    self.refresh_suggestions();
                }
                Some(("menu", "view-options-date")) => self.view_options = Some(true),
                Some(("show-date", "countdown")) => self.save_preferences(|p| p.countdown_dates = true, cx),
                Some(("menu", "arrange")) => {
                    self.set_layout(false, true, cx);
                    self.arrange_open = true;
                }
                Some(("menu", "timeline-week")) => {
                    self.set_layout(false, true, cx);
                    self.timeline_scale = 1;
                }
                // `menu=add-date-repeat`: the picker with a time and a weekly repeat, Repeat open.
                Some(("menu", "add-date-repeat")) => {
                    self.add_draft.date = Some(Self::today() + chrono::Duration::days(1));
                    self.add_draft.time = chrono::NaiveTime::from_hms_opt(9, 0, 0);
                    self.add_draft.repeat = Some(fern_core::Recurrence::new(fern_core::Frequency::Weekly, 1));
                    self.add_date_month = Some(crate::calendar::month_start(Self::today()));
                    self.add_date_row = Some(2);
                }
                // `menu=add-date`: the field's date picker open, with tomorrow picked.
                // The add field's picker on its Duration tab, from the next full hour.
                Some(("menu", "add-duration")) => {
                    self.add_date_month = Some(crate::calendar::month_start(Self::today()));
                    let mut range = crate::due_picker::DueDraft { month: crate::calendar::month_start(Self::today()), ..Default::default() };
                    range.start_duration(Local::now().naive_local(), self.preferences.task_defaults.duration);
                    self.add_range = Some(range);
                }
                Some(("menu", "add-date")) => {
                    self.add_draft.date = Some(Self::today() + chrono::Duration::days(1));
                    self.add_date_month = Some(crate::calendar::month_start(Self::today()));
                }
                Some(("menu", "detailed-input")) => {
                    self.save_preferences(|p| p.detailed_input = true, cx);
                    self.sync_input_placeholder(cx);
                }
                Some(("menu", "add-tags")) => {
                    self.add_tags = Some(vec!["home".to_owned()]);
                }
                Some(("menu", "add")) => {
                    self.add_menu_open = true;
                    self.add_list_side = true;
                }
                Some(("menu", "activity")) => self.open_list_activity(cx),
                // `context=<title part>`: that task's right-click menu, opened beside its row.
                Some(("context", needle)) => {
                    if let Some(task) = self.tasks.iter().find(|t| t.title.contains(needle)).cloned() {
                        self.open_context_menu(task.id, gpui::point(px(420.), px(200.)), true, window, cx);
                    }
                }
                // `filter-new=normal|advanced`, `filter-new=chain` (an Advanced chain with a
                // condition's menu open), `filter-new=preview` or `filter-new=examples`.
                Some(("filter-new", mode)) => {
                    self.open_filter_edit(None, window, cx);
                    if mode != "normal" {
                        self.set_filter_advanced(true, window, cx);
                    }
                    if let Some(edit) = self.filter_edit.as_mut() {
                        edit.preview = mode == "preview";
                        edit.examples_open = mode == "examples";
                    }
                    if mode == "chain"
                        && let Some(edit) = self.filter_edit.as_mut()
                    {
                        edit.draft.rules = fern_core::filter::examples()[2].rules.clone();
                        edit.inputs = Self::filter_inputs(&edit.draft, window, cx);
                        edit.chain_menu = Some(1);
                    }
                }
                Some(("filter", spec)) => {
                    // `filter=<name>><tag>`: a saved filter of one tag, opened.
                    let (name, tag) = spec.split_once('>').unwrap_or((spec, ""));
                    let names = if tag.is_empty() { Vec::new() } else { vec![tag.to_owned()] };
                    let filter = fern_core::SmartFilter {
                        name: name.to_owned(),
                        rules: fern_core::FilterRules { conditions: vec![fern_core::Condition::new(fern_core::ConditionValue::Tags { names, untagged: false })], ..Default::default() },
                        ..fern_core::SmartFilter::new()
                    };
                    if self.store.save_filter(&filter).is_ok() {
                        self.set_view(View::Filter(filter.id), cx);
                    }
                }
                Some(("flag", name)) => {
                    let default = name == "show_completed";
                    self.toggle_view_flag(name, default, cx);
                }
                Some(("pin", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        let result = self.store.set_pinned(id, true);
                        self.check(result);
                        self.reload(cx);
                    }
                }
                // `estimate=<title part>>p3|m90`: an Estimation, as Start Focus → Estimation sets it.
                Some(("estimate", spec)) => {
                    let all = self.store.tasks().unwrap_or_default();
                    if let Some((needle, value)) = spec.split_once('>')
                        && let Some(task) = all.iter().find(|t| t.title.contains(needle))
                    {
                        let estimate = fern_core::Estimate::from_storage(&value.replacen(['p', 'm'], &format!("{}:", &value[..1]), 1));
                        self.store.set_estimate(task.id, estimate).ok();
                        self.reload(cx);
                    }
                }
                Some(("select", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        self.select(id, cx);
                    }
                }
                Some(("popup", popup)) => {
                    let popup = popup.to_owned();
                    self.detail.update(cx, |detail, cx| detail.dev_open_popup(&popup, cx));
                }
                Some(("menu", "header")) => self.header_menu_open = true,
                Some(("menu", "sort")) => self.sort_menu = Some(Some(SortMenu::Sort)),
                Some(("menu", "sort-root")) => self.sort_menu = Some(None),
                Some(("menu", "group")) => self.sort_menu = Some(Some(SortMenu::Group)),
                Some(("group", key)) => self.set_sort(None, Some(GroupBy::from_key(key)), window, cx),
                Some(("sort", key)) => self.set_sort(Some(SortBy::from_key(key)), None, window, cx),
                Some(("menu", "dates")) => self.closed_menu = Some(ClosedMenu::Dates),
                Some(("menu", "other-date")) => {
                    self.open_other_date(cx);
                    self.update_other_date(cx, |start, _, picking, _| {
                        *start = Some(Self::today() - Duration::days(7));
                        *picking = Some(RangeEnd::End);
                    });
                }
                Some(("menu", "lists")) => self.closed_menu = Some(ClosedMenu::Lists(self.closed_lists.clone())),
                Some(("range", "this-week")) => {
                    self.closed_range = ClosedRange::ThisWeek;
                    self.invalidate_rows();
                }
                Some(("complete", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        self.toggle(id, true, cx);
                    }
                }
                Some(("delete", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        self.delete(id, cx);
                    }
                }
                Some(("new-list", _)) => self.start_new_list(window, cx),
                // `detail-bar=format|format-headings|comment|comment-emoji|comment:<text>`.
                Some(("detail-bar", what)) => self.detail.update(cx, |detail, cx| detail.dev_bar(what, window, cx)),
                // `list-dialog=new|<list name>[>icon|folder|type|smart|new-folder|kanban|timeline]`.
                Some(("list-dialog", spec)) => {
                    let (which, menu) = spec.split_once('>').unwrap_or((spec, ""));
                    let list = self.lists.iter().find(|l| l.name == which).map(|l| l.id);
                    self.open_list_dialog(list, None, window, cx);
                    if let Some(dialog) = &self.list_dialog {
                        dialog.update(cx, |dialog, cx| dialog.dev_open(menu, window, cx));
                    }
                }
                // `folder=<name>><list>,<list>`: a folder with these lists in it (dev data only).
                Some(("folder", spec)) => {
                    let (name, lists) = spec.split_once('>').unwrap_or((spec, ""));
                    if let Ok(folder) = self.store.add_list_folder(name) {
                        for list in self.lists.clone().iter().filter(|l| lists.split(',').any(|n| n == l.name)) {
                            let draft = fern_core::ListDraft { folder: Some(folder.id), ..fern_core::ListDraft::of(list) };
                            let result = self.store.update_list(list.id, &draft);
                            self.check(result);
                        }
                    }
                    self.reload(cx);
                }
                // `list-icon=<list>><emoji>`, `archive-list=<list>`: dev data only.
                Some(("list-icon", spec)) => {
                    let (name, icon) = spec.split_once('>').unwrap_or((spec, ""));
                    if let Some(list) = self.lists.iter().find(|l| l.name == name).cloned() {
                        let result = self.store.update_list(list.id, &fern_core::ListDraft { icon: Some(icon.to_owned()), ..fern_core::ListDraft::of(&list) });
                        self.check(result);
                    }
                    self.reload(cx);
                }
                Some(("archive-list", name)) => {
                    if let Some(id) = self.lists.iter().find(|l| l.name == name).map(|l| l.id) {
                        let result = self.store.set_list_archived(id, true);
                        self.check(result);
                        self.archived_open = true;
                    }
                    self.reload(cx);
                }
                // `tag-dialog=new|<tag>[>parent]`, `merge-dialog=<tag>[>open]`, `tag-menu=<tag>`,
                // `tags-menu`, `sub-tag=<tag>><parent>` (dev data only).
                Some(("tag-dialog", spec)) => {
                    let (which, menu) = spec.split_once('>').unwrap_or((spec, ""));
                    let tag = (which != "new").then(|| which.to_owned());
                    self.open_tag_dialog(tag, None, window, cx);
                    if menu == "parent"
                        && let Some(dialog) = &self.tag_dialog
                    {
                        dialog.update(cx, |dialog, cx| dialog.dev_open_parent(cx));
                    }
                }
                Some(("merge-dialog", spec)) => {
                    let (tag, menu) = spec.split_once('>').unwrap_or((spec, ""));
                    self.open_merge_dialog(tag.to_owned(), window, cx);
                    if menu == "open"
                        && let Some(dialog) = &self.merge_dialog
                    {
                        dialog.update(cx, |dialog, cx| dialog.dev_open(cx));
                    }
                }
                Some(("tag-menu", tag)) => self.list_menu = Some((ListMenuTarget::Tag(tag.to_owned()), gpui::point(px(150.), px(420.)))),
                Some(("tags-menu", _)) => self.list_menu = Some((ListMenuTarget::TagsHeading, gpui::point(px(170.), px(330.)))),
                Some(("sub-tag", spec)) => {
                    let (tag, parent) = spec.split_once('>').unwrap_or((spec, ""));
                    let result = self.store.update_tag(tag, tag, None, Some(parent));
                    self.check(result);
                    self.reload(cx);
                }
                // `list-menu=<list>` and `folder-menu=<folder>` open their right-click menus.
                Some(("list-menu", name)) => {
                    if let Some(id) = self.lists.iter().find(|l| l.name == name).map(|l| l.id) {
                        self.list_menu = Some((ListMenuTarget::List(id), gpui::point(px(180.), px(260.))));
                    }
                }
                Some(("folder-menu", name)) => {
                    if let Some(id) = self.folders.iter().find(|f| f.name == name).map(|f| f.id) {
                        self.list_menu = Some((ListMenuTarget::Folder(id), gpui::point(px(180.), px(260.))));
                    }
                }
                _ => {}
            }
        }
    }
}
