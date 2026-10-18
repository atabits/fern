# Feature parity with TickTick

A working list of what TickTick has, what we already have, and what comes next.
It is based on how TickTick 8.0 for desktop looks and behaves (screenshots and pixel
measurements) and on the public shape of its Open API. TickTick's client code is not inspected.

Status: ✅ done · 🟡 partial / visual only · ⬜ missing

## Measurements (logical pixels, dark theme)

| Element | Value |
|---|---|
| Narrow sections sidebar | ≈ 50 wide |
| Lists column | ≈ 212 wide, items ≈ 36 tall, selection radius ≈ 6 |
| Task row | 40 tall, 1 px separator at the bottom, selection ~6% lighter than the background |
| "Add task" field | ≈ 38 tall, lighter background |
| List header | ≈ 60 tall, sidebar toggle on the left, sort and "…" on the right |
| Tag chip | ≈ 18 tall, fully rounded, tag color at ~30% as background, lighter tag color as text |
| Due date / repeat in a row | accent blue, right-aligned |
| Task details panel | top bar ≈ 48, bottom bar ≈ 44 |

## Data model

| Feature | Status | Notes |
|---|---|---|
| Task: title, completion | ✅ | |
| Task notes (`content`) | ✅ | |
| Due date (`dueDate`, all-day or with time) | ✅ | |
| Priority 0/1/3/5 | ✅ | same values as TickTick |
| Tags with colors | ✅ | case-insensitive, including non-ASCII |
| Subtasks (`parentId`) | ✅ | cascade delete, no cycles |
| Item kind: task / note / checklist (`kind`) | 🟡 | task and note; checklist later |
| Checklist items (`items`) | ✅ | ordered items, insert after, completion, progress |
| Repeat (`repeatFlag`, RRULE) | 🟡 | FREQ, INTERVAL and weekly BYDAY (every weekday); completing moves the task to the next date and keeps a completed copy; no BYMONTHDAY or custom rules yet |
| Lists (projects), Inbox | ✅ | create, delete, move tasks with subtasks |
| Custom sections inside a list (columns) | ⬜ | groups are by kind for now |
| Manual ordering (`sortOrder`) | ✅ | new tasks on top; renumbered with gaps on move |
| Trash, "Won't Do" | 🟡 | trash matched to the original: flat list, broom button empties it; a trashed task's panel has "Restore" and "Delete forever" in its bottom bar; no "Won't Do" |
| Reminders | ⬜ | |
| Settings (UI language) | ✅ | |

## Interface

| Feature | Status | Notes |
|---|---|---|
| Narrow sections sidebar | 🟡 | Tasks, Calendar, Search, Matrix and Countdown work; outline icons where TickTick uses filled ones; no notification badge |
| Account menu with language switch | ✅ | English (default) and Russian |
| Lists column: Inbox / All / Today / Tomorrow / Next 7 Days | ✅ | counters; a new task gets the due date or tag of the current view |
| Lists column: user lists | ✅ | create inline; rename, color and delete from the "…" menu; no drag and drop |
| Lists column: tags with color dots and counters | ✅ | |
| Lists column: Filters, Won't Do, Trash | 🟡 | Completed and Trash |
| Mini week at the bottom of the lists column | 🟡 | current week, today highlighted (matched to a screenshot); clicking a day does nothing yet — the original behavior is not verified |
| List header with sidebar toggle | 🟡 | toggle and "…" work; sorting does not |
| "Add task" field | ✅ | parses date, time, repeat, tags, priority and `^list`; no live highlighting yet; `^list` matches one-word list names only |
| Rows by the measurements (40 px, separators) | ✅ | |
| Tag chips, due date and repeat in rows | ✅ | overdue dates in red |
| Subtasks with indentation and collapsing | ✅ | created from the details panel |
| Groups "Tasks" / "Notes" with counters and collapsing | ✅ | plus a collapsed "Completed" group |
| Details: checkbox, title, notes | ✅ | |
| Details: due date | 🟡 | popover matched to the original: quick picks with tooltips, month calendar, Time (30-minute list) and Repeat (with date hints and "Every Weekday"), Clear/OK with a draft; no Duration tab, Reminder or Custom repeat |
| Details: priority (flag) | ✅ | |
| Details: tags | ✅ | chips under the notes as in the original; the "+ Tag" picker is our addition (TickTick adds tags with "#" in the text) |
| Details: list | ✅ | list button in the bottom bar |
| Details: checklist | ✅ | Enter adds the next item; progress "1/3" in rows |
| Details: subtasks | ✅ | list with checkboxes, open a subtask, "Add subtask" |
| Drag and drop | 🟡 | reorder rows; drop on a lists-column entry (list, tag, Today/Tomorrow, Completed, Trash) or on a calendar day; between matrix quadrants. The original's drag behavior was not observed (dragging in TickTick would change data); no nesting by drag |
| Keyboard shortcuts | ✅ | JSON keymap (Zed format): new task, search, smart lists, list navigation, complete, delete |
| Search | 🟡 | popover matched to the original: Task/Tag/List filters, results grouped with highlighted matches, list name, note excerpts, "View more" opens the full list; no "Event" filter (no calendar events) |
| Calendar | 🟡 | month and week views matched to the original (week: all-day strip, time axis with folded night hours, timed tasks placed by time); view menu with Week/Month; tasks can be dragged between days; no Day, Year, Agenda or multi-day views |
| Eisenhower Matrix | 🟡 | matched to the original: quadrants by priority (TickTick's default rules), grouped by list, drag a task to another quadrant to change its priority; no custom quadrant rules |
| Countdown | 🟡 | matched to the original: filter tabs, card grid with Y/M/D or days-only (click the numbers), until/since colors, add with name, type and date, delete on hover; no background images, editing or repeating dates |
| Pomodoro, Habits | ⬜ | |
| Sync between devices | ⬜ | |

## How changes are checked

- Every feature is checked against a screenshot of the same TickTick screen before and after it
  is built.

- `cargo test` covers the core (storage, smart views, tags, subtasks, lists, quick add) and the
  desktop's row, date and calendar logic in both languages.
- A screenshot of our window next to the original, plus pixel measurements. Demo data:
  `cargo run -p fern_core --example seed_demo -- <path to database>`.
- In debug builds `FERN_DEV_STATE` puts the UI into a given state without clicks, for example
  `FERN_DEV_STATE="add=Buy milk tomorrow #home;select=milk;popup=due"`; see
  `TodoView::apply_dev_state` for all commands. `FERN_APP_ID` changes the window's app id so
  window-manager rules can target dev builds.

## Next

1. Habits section.
2. Pomodoro section.
3. "Won't Do" status.
3. "Won't Do" status.
4. Custom time and "every N" in the due-date picker.
5. Eisenhower matrix view.
