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
    ]
);

actions!(task_list, [SelectNext, SelectPrevious, ToggleComplete, DeleteTask]);
