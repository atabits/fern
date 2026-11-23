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
        ShowAll,
        ShowInbox,
        ShowToday,
        ShowTomorrow,
        ShowNext7Days,
        ShowCompleted,
        ShowWontDo,
        ShowTrash,
        GoToHorizons,
        GoToSummary,
        PrintList,
        PrintDetail,
        ShowShortcuts,
        OpenSettings,
        DismissTransient,
        MinimizeWindow,
    ]
);

actions!(settings, [DismissSettings]);
actions!(task_detail, [SaveDraft]);

actions!(calendar, [DayView, WeekView, MonthView, YearView, AgendaView, MultiDayMode, PreviousPeriod, NextPeriod, PreviousYear, NextYear, CalendarToday, PrintYear]);

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
        ToggleAllGroups,
        ToggleAllSubtasks,
        AddSubtask,
        OpenAsSticky,
        ViewList,
        ViewKanban,
        ViewTimeline,
        Undo,
        Redo,
    ]
);

actions!(focus, [TogglePause, StartStop]);

actions!(
    sticky,
    [TogglePin, ToggleColor, ToggleCollapse, AlignTop, AlignLeft, AlignRight]
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

actions!(things, [OpenWhen]);
