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
| Checklist items (`items`) | ⬜ | |
| Repeat (`repeatFlag`, RRULE) | ⬜ | |
| Lists (projects), Inbox | ✅ | create, delete, move tasks with subtasks |
| Custom sections inside a list (columns) | ⬜ | groups are by kind for now |
| Manual ordering (`sortOrder`) | ⬜ | |
| Trash, "Won't Do" | ⬜ | |
| Reminders | ⬜ | |
| Settings (UI language) | ✅ | |

## Interface

| Feature | Status | Notes |
|---|---|---|
| Narrow sections sidebar | 🟡 | only Tasks works; outline icons where TickTick uses filled ones; no notification badge |
| Account menu with language switch | ✅ | English (default) and Russian |
| Lists column: Inbox / All / Today / Tomorrow / Next 7 Days | ✅ | counters; a new task gets the due date or tag of the current view |
| Lists column: user lists | ✅ | create inline; rename, color and delete from the "…" menu; no drag and drop |
| Lists column: tags with color dots and counters | ✅ | |
| Lists column: Filters, Won't Do, Trash | 🟡 | only Completed |
| Mini week at the bottom of the lists column | ⬜ | |
| List header with sidebar toggle | 🟡 | toggle and "…" work; sorting does not |
| "Add task" field | ✅ | parses date, time, tags, priority and `^list`; no live highlighting yet; `^list` matches one-word list names only |
| Rows by the measurements (40 px, separators) | ✅ | |
| Tag chips, due date and repeat in rows | 🟡 | tags and due date (overdue in red); no repeat |
| Subtasks with indentation and collapsing | ✅ | creating subtasks from the UI comes later |
| Groups "Tasks" / "Notes" with counters and collapsing | ✅ | plus a collapsed "Completed" group |
| Details: checkbox, title, notes | ✅ | |
| Details: due date | 🟡 | quick options and a month calendar; no time or repeat picker |
| Details: priority (flag) | ✅ | |
| Details: tags | ✅ | chips under the title, picker with "+ Tag" |
| Details: list | ✅ | list button in the bottom bar |
| Details: checklist | ⬜ | |
| Drag and drop | ⬜ | |
| Calendar, Pomodoro, Habits, Matrix, Countdown, Search | ⬜ | |
| Sync between devices | ⬜ | |

## How changes are checked

- `cargo test` covers the core (storage, smart views, tags, subtasks, lists, quick add) and the
  desktop's row, date and calendar logic in both languages.
- A screenshot of our window next to the original, plus pixel measurements. Demo data:
  `cargo run -p fern_core --example seed_demo -- <path to database>`.
- In debug builds `FERN_DEV_STATE` puts the UI into a given state without clicks, for example
  `FERN_DEV_STATE="add=Buy milk tomorrow #home;select=milk;popup=due"`; see
  `TodoView::apply_dev_state` for all commands. `FERN_APP_ID` changes the window's app id so
  window-manager rules can target dev builds.

## Next

1. Checklist items inside a task.
2. Trash instead of immediate deletion.
3. Time and repeat in the due-date picker.
4. Manual ordering and drag and drop.
5. Creating subtasks from the UI.
