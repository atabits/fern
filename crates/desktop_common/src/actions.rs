//! App-wide actions. Key bindings for them live in `assets/keymaps/default.json`.

use gpui::actions;

actions!(
    fern,
    [
        Quit,
        CloseWindow,
        NewTask,
        Search,
        SyncNow,
        ToggleSidebar,
        ShowInbox,
        ShowToday,
        ShowTomorrow,
        ShowNext7Days,
        ShowShortcuts,
        OpenSettings,
        DismissTransient,
        MinimizeWindow,
    ]
);

actions!(settings, [DismissSettings]);
actions!(task_detail, [SaveDraft]);

actions!(calendar, [DayView, WeekView, MonthView, YearView, AgendaView, PreviousYear, NextYear, CalendarToday, PrintYear]);

actions!(
    task_list,
    [
        SelectNext,
        SelectPrevious,
        SelectAllTasks,
        ExtendSelectionNext,
        ExtendSelectionPrevious,
        ToggleComplete,
        ToggleInProgress,
        DeleteTask,
        PinTask,
        SetDate,
        ClearDate,
        SetToday,
        SetTomorrow,
        SetNextWeek,
        PriorityNone,
        PriorityLow,
        PriorityMedium,
        PriorityHigh,
        ToggleAllSubtasks,
        AddSubtask,
        Undo,
    ]
);

actions!(
    habits,
    [SelectNextHabit, SelectPreviousHabit, CheckInSelectedHabit, NewHabit]
);

actions!(
    matrix,
    [SelectNextMatrixTask, SelectPreviousMatrixTask, ToggleMatrixTask, OpenMatrixTask, DeleteMatrixTask]
);

actions!(
    markdown,
    [Bold, Highlight, CheckItem, BulletedList, NumberedList, Italic, Underline, Strike, Link, Code, Quote]
);
