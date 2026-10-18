//! App-wide actions. Key bindings for them live in `assets/keymaps/default.json`.

use gpui::actions;

actions!(
    fern,
    [
        Quit,
        NewTask,
        Search,
        ToggleSidebar,
        ShowInbox,
        ShowToday,
        ShowTomorrow,
        ShowNext7Days,
        ShowShortcuts,
        OpenSettings,
    ]
);

actions!(settings, [DismissSettings]);

actions!(calendar, [DayView, WeekView, MonthView, YearView, AgendaView]);

actions!(
    task_list,
    [
        SelectNext,
        SelectPrevious,
        ToggleComplete,
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
    ]
);

actions!(
    markdown,
    [Bold, Highlight, CheckItem, BulletedList, NumberedList, Italic, Underline, Strike, Link, Code, Quote]
);
