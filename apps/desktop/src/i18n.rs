//! UI translations. English is the default; every locale fills the same `Strings` struct,
//! so a missing translation is a compile error rather than a blank label.

use gpui::{App, Global};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    En,
    Ru,
}

impl Locale {
    pub const ALL: [Locale; 2] = [Locale::En, Locale::Ru];

    pub fn code(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Ru => "ru",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.code() == code)
    }

    /// The language name in that language, for the language menu.
    pub fn native_name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::Ru => "Русский",
        }
    }

    pub fn strings(self) -> &'static Strings {
        match self {
            Locale::En => &EN,
            Locale::Ru => &RU,
        }
    }
}

/// The current UI language, stored as a GPUI global so views re-render when it changes.
pub struct CurrentLocale(pub Locale);

impl Global for CurrentLocale {}

pub fn locale(cx: &App) -> Locale {
    cx.try_global::<CurrentLocale>().map(|l| l.0).unwrap_or_default()
}

/// Strings for the current UI language.
pub fn t(cx: &App) -> &'static Strings {
    locale(cx).strings()
}

pub struct Strings {
    // Sections of the narrow sidebar.
    pub section_tasks: &'static str,
    pub section_calendar: &'static str,
    pub section_habits: &'static str,
    pub section_pomodoro: &'static str,
    pub section_search: &'static str,
    pub section_matrix: &'static str,
    pub section_countdown: &'static str,
    pub coming_soon: &'static str,
    pub search_placeholder: &'static str,
    pub search_hint: &'static str,
    pub nothing_found: &'static str,
    pub language: &'static str,

    // Smart lists and the lists column.
    pub inbox: &'static str,
    pub all: &'static str,
    pub today: &'static str,
    pub tomorrow: &'static str,
    pub next_7_days: &'static str,
    /// Shorter label for the lists column, where the full title may not fit.
    pub next_7_days_short: &'static str,
    pub completed: &'static str,
    pub trash: &'static str,
    pub empty_trash: &'static str,
    pub trash_is_empty: &'static str,
    pub restore: &'static str,
    pub tags: &'static str,
    pub lists: &'static str,
    pub new_list: &'static str,
    pub delete_list: &'static str,
    pub rename_list: &'static str,
    pub color: &'static str,
    /// Shown in the "…" menu when the current view has no actions.
    pub no_actions: &'static str,

    // Task list.
    pub add_task: &'static str,
    pub no_tasks: &'static str,
    pub no_completed_tasks: &'static str,
    pub untitled: &'static str,
    pub group_tasks: &'static str,
    pub group_notes: &'static str,
    pub group_completed: &'static str,

    // Task details.
    pub select_task: &'static str,
    pub notes_placeholder: &'static str,
    pub due_date: &'static str,
    pub next_week: &'static str,
    pub clear_date: &'static str,
    pub time: &'static str,
    pub no_time: &'static str,
    pub repeat: &'static str,
    pub repeat_none: &'static str,
    pub repeat_daily: &'static str,
    pub repeat_weekly: &'static str,
    pub repeat_monthly: &'static str,
    pub repeat_yearly: &'static str,
    pub priority_high: &'static str,
    pub priority_medium: &'static str,
    pub priority_low: &'static str,
    pub priority_none: &'static str,
    pub add_tag: &'static str,
    pub new_tag: &'static str,
    pub add_item: &'static str,
    pub add_subtask: &'static str,

    // Dates.
    pub yesterday: &'static str,
    /// Monday first.
    pub weekdays_short: [&'static str; 7],
    /// One-letter weekday names, Monday first, for the mini week.
    pub weekdays_narrow: [&'static str; 7],
    pub months: [&'static str; 12],
    pub months_short: [&'static str; 12],
    /// "13 Nov" (`true`) or "Nov 13" (`false`).
    pub day_before_month: bool,
}

pub static EN: Strings = Strings {
    section_tasks: "Tasks",
    section_calendar: "Calendar",
    section_habits: "Habits",
    section_pomodoro: "Pomodoro",
    section_search: "Search",
    section_matrix: "Eisenhower Matrix",
    section_countdown: "Countdown",
    coming_soon: "Coming soon",
    search_placeholder: "Search tasks",
    search_hint: "Search by title, notes or tags",
    nothing_found: "Nothing found",
    language: "Language",

    inbox: "Inbox",
    all: "All",
    today: "Today",
    tomorrow: "Tomorrow",
    next_7_days: "Next 7 Days",
    next_7_days_short: "Next 7 Days",
    completed: "Completed",
    trash: "Trash",
    empty_trash: "Empty trash",
    trash_is_empty: "The trash is empty",
    restore: "Restore",
    tags: "Tags",
    lists: "Lists",
    new_list: "New list",
    delete_list: "Delete list",
    rename_list: "Rename",
    color: "Color",
    no_actions: "No actions",

    add_task: "Add task",
    no_tasks: "No tasks",
    no_completed_tasks: "No completed tasks",
    untitled: "Untitled",
    group_tasks: "Tasks",
    group_notes: "Notes",
    group_completed: "Completed",

    select_task: "Select a task to see its details",
    notes_placeholder: "Write something",
    due_date: "Due date",
    next_week: "Next week",
    clear_date: "Clear date",
    time: "Time",
    no_time: "None",
    repeat: "Repeat",
    repeat_none: "None",
    repeat_daily: "Daily",
    repeat_weekly: "Weekly",
    repeat_monthly: "Monthly",
    repeat_yearly: "Yearly",
    priority_high: "High",
    priority_medium: "Medium",
    priority_low: "Low",
    priority_none: "No priority",
    add_tag: "+ Tag",
    new_tag: "New tag",
    add_item: "Add item",
    add_subtask: "Add subtask",

    yesterday: "Yesterday",
    weekdays_short: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    weekdays_narrow: ["M", "T", "W", "T", "F", "S", "S"],
    months: [
        "January", "February", "March", "April", "May", "June", "July", "August", "September",
        "October", "November", "December",
    ],
    months_short: [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    day_before_month: false,
};

pub static RU: Strings = Strings {
    section_tasks: "Задачи",
    section_calendar: "Календарь",
    section_habits: "Привычки",
    section_pomodoro: "Помодоро",
    section_search: "Поиск",
    section_matrix: "Матрица Эйзенхауэра",
    section_countdown: "Обратный отсчёт",
    coming_soon: "Скоро",
    search_placeholder: "Поиск задач",
    search_hint: "Ищет по названию, заметке и тегам",
    nothing_found: "Ничего не найдено",
    language: "Язык",

    inbox: "Входящие",
    all: "Все",
    today: "Сегодня",
    tomorrow: "Завтра",
    next_7_days: "Следующие 7 дней",
    next_7_days_short: "7 дней",
    completed: "Выполненные",
    trash: "Корзина",
    empty_trash: "Очистить корзину",
    trash_is_empty: "Корзина пуста",
    restore: "Восстановить",
    tags: "Теги",
    lists: "Списки",
    new_list: "Новый список",
    delete_list: "Удалить список",
    rename_list: "Переименовать",
    color: "Цвет",
    no_actions: "Нет действий",

    add_task: "Добавить задачу",
    no_tasks: "Задач нет",
    no_completed_tasks: "Выполненных задач нет",
    untitled: "Без названия",
    group_tasks: "Задачи",
    group_notes: "Заметки",
    group_completed: "Выполненные",

    select_task: "Выбери задачу, чтобы увидеть подробности",
    notes_placeholder: "Напиши что-нибудь",
    due_date: "Срок",
    next_week: "Через неделю",
    clear_date: "Убрать срок",
    time: "Время",
    no_time: "Нет",
    repeat: "Повтор",
    repeat_none: "Нет",
    repeat_daily: "День",
    repeat_weekly: "Неделя",
    repeat_monthly: "Месяц",
    repeat_yearly: "Год",
    priority_high: "Высокий",
    priority_medium: "Средний",
    priority_low: "Низкий",
    priority_none: "Без приоритета",
    add_tag: "+ Тег",
    new_tag: "Новый тег",
    add_item: "Добавить пункт",
    add_subtask: "Добавить подзадачу",

    yesterday: "Вчера",
    weekdays_short: ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"],
    weekdays_narrow: ["П", "В", "С", "Ч", "П", "С", "В"],
    months: [
        "Январь", "Февраль", "Март", "Апрель", "Май", "Июнь", "Июль", "Август", "Сентябрь",
        "Октябрь", "Ноябрь", "Декабрь",
    ],
    months_short: [
        "янв.", "февр.", "мар.", "апр.", "мая", "июн.", "июл.", "авг.", "сент.", "окт.", "нояб.",
        "дек.",
    ],
    day_before_month: true,
};
