//! UI translations. English is the default; every locale fills the same `Strings` struct,
//! so a missing translation is a compile error rather than a blank label.

use gpui::{App, Global};

const ZH_CN_STRINGS: Option<&Strings> = Some(&ZH_CN);
/// Default Chinese public holiday iCal feed URL.
pub const CHINA_HOLIDAY_GOOGLE_URL: &str = "https://calendar.google.com/calendar/ical/zh.china%23holiday%40group.v.calendar.google.com/public/basic.ics";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    En,
    Ru,
    ZhCn,
}

impl Locale {
    pub fn available() -> impl Iterator<Item = Self> {
        [Self::En, Self::Ru, Self::ZhCn]
            .into_iter()
            .filter(|locale| locale.translation().is_some())
    }

    pub fn code(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Ru => "ru",
            Locale::ZhCn => "zh-CN",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::available().find(|locale| locale.code() == code)
    }

    /// The system's language when Fern speaks it, else English: the language of the first start.
    pub fn system() -> Self {
        sys_locale::get_locales()
            .find_map(|tag| Self::from_system_tag(&tag))
            .unwrap_or_default()
    }

    fn from_system_tag(tag: &str) -> Option<Self> {
        let tag = tag.to_ascii_lowercase();
        let mut subtags = tag.split(['-', '_']);
        let language = subtags.next()?;
        let code = match (language, subtags.next()) {
            ("zh", None | Some("hans" | "cn" | "sg")) => "zh-CN",
            _ => language,
        };
        Self::from_code(code)
    }

    /// The language name in that language, for the language menu.
    pub fn native_name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::Ru => "Русский",
            Locale::ZhCn => "简体中文",
        }
    }

    pub fn strings(self) -> &'static Strings {
        self.translation().unwrap_or(&EN)
    }

    fn translation(self) -> Option<&'static Strings> {
        match self {
            Locale::En => Some(&EN),
            Locale::Ru => Some(&RU),
            Locale::ZhCn => ZH_CN_STRINGS,
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
    pub pomodoro: &'static str,
    pub focus: &'static str,
    pub focus_overview: &'static str,
    pub focus_record: &'static str,
    pub focus_note_placeholder: &'static str,
    pub focus_today_pomo: &'static str,
    pub focus_today_focus: &'static str,
    pub focus_total_pomo: &'static str,
    pub focus_total_duration: &'static str,
    pub focus_no_tasks: &'static str,
    pub focus_no_record: &'static str,
    pub habit_name: &'static str,
    pub habit_days: &'static str,
    pub habit_streak: &'static str,
    pub habit_empty: &'static str,
    pub habit_monthly_checkins: &'static str,
    pub habit_total_checkins: &'static str,
    pub habit_monthly_rate: &'static str,
    pub habit_current_streak: &'static str,
    /// The unit after a number of days in the habit statistics.
    pub habit_unit_days: &'static str,
    /// The unit for 0 or 1 days, as the original's "0 Day".
    pub habit_unit_day: &'static str,
    pub habit_select: &'static str,
    pub habit_monthly_completion: &'static str,
    pub habit_daily_goals: &'static str,
    pub habit_total_completion: &'static str,
    /// "Habit Log on October" / "Журнал: октябрь"; the month name follows.
    pub habit_log_on: &'static str,
    pub habit_log_empty: &'static str,
    pub habit_note_placeholder: &'static str,
    pub edit: &'static str,
    pub edit_habit: &'static str,
    pub habit_goal: &'static str,
    pub habit_frequency: &'static str,
    pub habit_section: &'static str,
    pub habit_checked_in: &'static str,
    pub habit_start_anytime: &'static str,
    pub focus_add_record: &'static str,
    pub focus_batch_delete_title: &'static str,
    pub focus_batch_delete_message: &'static str,
    pub focus_settings: &'static str,
    pub statistics: &'static str,
    pub start_pomo: &'static str,
    pub estimation: &'static str,
    pub estimated_pomo: &'static str,
    pub estimated_duration: &'static str,
    /// After an estimated number of pomos.
    pub pomo_unit: &'static str,
    pub start_stopwatch: &'static str,
    pub checked_in_style: &'static str,
    pub checkin_style_checkmark: &'static str,
    pub checkin_style_icon: &'static str,
    pub summary: &'static str,
    pub summary_filter: &'static str,
    pub summary_lists: &'static str,
    pub summary_more: &'static str,
    pub summary_tag: &'static str,
    pub summary_priority: &'static str,
    pub summary_all_tags: &'static str,
    pub summary_all_priorities: &'static str,
    pub summary_all_lists: &'static str,
    pub summary_custom: &'static str,
    pub summary_start: &'static str,
    pub summary_end: &'static str,
    pub summary_next_period: &'static str,
    pub summary_habits: &'static str,
    /// A habit in the Summary: "{name}" and its check-in days "{n}".
    pub summary_habit_line: &'static str,
    pub summary_date: &'static str,
    pub summary_status: &'static str,
    pub summary_display_options: &'static str,
    pub summary_grouping: &'static str,
    pub summary_fields_label: &'static str,
    /// "{n} selected".
    pub summary_selected: &'static str,
    pub summary_copy: &'static str,
    pub summary_save_as: &'static str,
    pub summary_template: &'static str,
    pub summary_save_template: &'static str,
    pub summary_template_hint: &'static str,
    pub summary_template_name: &'static str,
    pub summary_pdf: &'static str,
    pub summary_image: &'static str,
    pub summary_send_email: &'static str,
    pub summary_copied: &'static str,
    pub summary_empty: &'static str,
    pub summary_overwrite_title: &'static str,
    pub summary_overwrite_message: &'static str,
    /// In the order of `summary::Period::ALL`.
    pub summary_periods: [&'static str; 8],
    /// All, Completed, Uncompleted, Won't Do.
    pub summary_statuses: [&'static str; 5],
    /// In the order of `summary::Grouping::ALL`.
    pub summary_groupings: [&'static str; 6],
    /// In the order of `summary::Field::ALL`.
    pub summary_fields: [&'static str; 10],
    pub stats_achievement_score: &'static str,
    /// "{n}" is replaced by the share in percent, which is highlighted.
    pub stats_more_productive: &'static str,
    pub stats_day: &'static str,
    pub stats_week: &'static str,
    pub stats_month: &'static str,
    pub stats_this_week: &'static str,
    pub stats_this_month: &'static str,
    /// The score's badge; "{level}" is replaced.
    pub stats_level: &'static str,
    /// Overview, Task, Focus.
    pub stats_tabs: [&'static str; 3],
    /// "{tasks}", "{completed}", "{lists}" and "{days}" are replaced.
    pub stats_summary: &'static str,
    pub stats_today_completed: &'static str,
    pub stats_total_completed: &'static str,
    pub stats_completion_curve: &'static str,
    pub stats_rate_curve: &'static str,
    pub stats_pomo_curve: &'static str,
    pub stats_focus_curve: &'static str,
    pub stats_weekly_habits: &'static str,
    pub stats_completed_tasks: &'static str,
    pub stats_completion_rate: &'static str,
    pub stats_by_list: &'static str,
    pub stats_by_list_option: &'static str,
    pub stats_by_tag_option: &'static str,
    pub stats_by_task_option: &'static str,
    pub stats_details: &'static str,
    pub stats_timeline: &'static str,
    pub stats_no_data: &'static str,
    pub done: &'static str,
    pub stats_trends: &'static str,
    pub stats_most_focused: &'static str,
    pub stats_year_grids: &'static str,
    /// "{n}" is replaced by the change.
    pub stats_from_yesterday: &'static str,
    pub stats_from_last_week: &'static str,
    pub stats_from_last_month: &'static str,
    pub stats_daily: &'static str,
    pub stats_weekly: &'static str,
    pub stats_monthly: &'static str,
    pub stats_completion_distribution: &'static str,
    pub stats_rate_distribution: &'static str,
    pub stats_overdue: &'static str,
    pub stats_on_time: &'static str,
    pub stats_undated: &'static str,
    pub stats_uncompleted: &'static str,
    /// "{t}" is replaced by the average.
    pub stats_daily_average: &'static str,
    pub focus_timer_option: &'static str,
    pub focus_pomo_duration: &'static str,
    pub focus_short_break: &'static str,
    pub focus_long_break: &'static str,
    pub focus_limit_notice: &'static str,
    pub focus_long_break_every: &'static str,
    pub focus_auto_break: &'static str,
    pub focus_auto_next: &'static str,
    pub focus_auto_count: &'static str,
    pub focus_daily_goal: &'static str,
    pub focus_goal_off: &'static str,
    pub focus_white_noise: &'static str,
    pub focus_white_noise_names: [&'static str; 2],
    pub focus_limit_title: &'static str,
    pub focus_end_question: &'static str,
    pub focus_end_limit_detail: &'static str,
    pub focus_end_save_detail: &'static str,
    pub focus_end_short_detail: &'static str,
    pub focus_discard: &'static str,
    pub focus_later: &'static str,
    pub focus_review_title: &'static str,
    pub focus_review_heading: &'static str,
    pub focus_review_limit: &'static str,
    pub focus_review_paused: &'static str,
    pub focus_review_hours: &'static str,
    pub focus_review_minutes: &'static str,
    pub focus_review_invalid_duration: &'static str,
    pub focus_session_unavailable: &'static str,
    pub focus_settings_save_failed: &'static str,
    pub focus_review_open_failed: &'static str,
    pub focus_save_failed: &'static str,
    pub minutes: &'static str,
    /// Short duration units used in compact timers and stats ("h"/"m", "ч"/"м", "小时"/"分").
    pub duration_hour_short: &'static str,
    pub duration_min_short: &'static str,
    pub focus_record_title: &'static str,
    pub focus_record_start: &'static str,
    pub focus_record_minutes: &'static str,
    pub delete_all: &'static str,
    pub delete_all_confirm: &'static str,
    pub settings: &'static str,
    pub export: &'static str,
    pub habit_settings: &'static str,
    pub habit_today_amount: &'static str,
    /// "{n}": today's amount so far.
    pub habit_done_so_far: &'static str,
    pub habit_record: &'static str,
    pub habits_in_today: &'static str,
    pub habits_sort_by_status: &'static str,
    pub habits_sort_hint: &'static str,
    /// "{path}" is replaced by the file.
    pub exported_to: &'static str,
    pub habit_add_section: &'static str,
    pub habit_section_name: &'static str,
    /// Morning, Afternoon, Night, Others.
    pub habit_sections: [&'static str; 4],
    pub habit_active: &'static str,
    pub habit_archived: &'static str,
    pub habit_archive: &'static str,
    pub habit_unarchive: &'static str,
    pub habit_no_archived: &'static str,
    pub habit_start_date: &'static str,
    pub habit_goal_days: &'static str,
    /// The "?" beside Goal Days.
    pub habit_goal_days_hint: &'static str,
    pub habit_forever: &'static str,
    pub habit_custom: &'static str,
    /// The habit icon picker's tabs: Icon (emoji on colored circles), Emoji, Text.
    pub habit_icon_tabs: [&'static str; 3],
    pub habit_text_placeholder: &'static str,
    pub habit_frequency_kinds: [&'static str; 3],
    /// "{n}" is replaced by the number of days.
    pub habit_days_per_week: &'static str,
    pub habit_every_n_days: &'static str,
    pub habit_goal_all: &'static str,
    pub habit_constant_reminder: &'static str,
    pub habit_goal_amount: &'static str,
    pub habit_unit_placeholder: &'static str,
    pub habit_per_day: &'static str,
    pub habit_add_title: &'static str,
    pub habit_checkin_mode_label: &'static str,
    pub habit_checkin_auto: &'static str,
    pub habit_checkin_manual: &'static str,
    pub habit_checkin_all: &'static str,
    pub habit_step: &'static str,
    pub habit_auto_log: &'static str,
    pub habit_do_not_show_again: &'static str,
    pub habit_skip: &'static str,
    pub habit_unskip: &'static str,
    pub habit_unachieved: &'static str,
    pub habit_reset: &'static str,
    pub habit_reset_confirm: &'static str,
    pub habit_delete_confirm: &'static str,
    pub habit_delete_log_confirm: &'static str,
    pub habit_delete_section_confirm: &'static str,
    pub habit_best_streak: &'static str,
    pub habit_cycles: &'static str,
    pub habit_cycle_progress: &'static str,
    pub habit_cycle_completed_count: &'static str,
    pub habit_cycle_done_title: &'static str,
    pub habit_cycle_done_body: &'static str,
    pub habit_end_and_archive: &'static str,
    pub habit_continue_cycle: &'static str,
    pub habit_cycle_reset_confirm: &'static str,
    pub habit_completed_section: &'static str,
    pub habit_how_much: &'static str,
    pub habit_increase: &'static str,
    pub habit_total: &'static str,
    pub habit_expand: &'static str,
    pub habit_collapse: &'static str,
    pub habit_mood_prompt: &'static str,
    pub habit_past_limit: &'static str,
    pub habit_i_did_it: &'static str,
    pub habit_clear_date: &'static str,
    pub habit_start_pomo: &'static str,
    pub habit_start_stopwatch: &'static str,
    pub save: &'static str,
    pub countdown_all: &'static str,
    /// The header "…" menu's Hide Group / Show Group (the kind tabs).
    pub countdown_hide_group: &'static str,
    /// The dialog's Background row: its label, the picture chooser and "none".
    pub countdown_background: &'static str,
    pub countdown_choose_image: &'static str,
    pub countdown_show_group: &'static str,
    pub countdown_kind_countdown: &'static str,
    pub countdown_kind_anniversary: &'static str,
    pub countdown_kind_holiday: &'static str,
    pub countdown_kind_birthday: &'static str,
    /// A birthday's "Show Age" switch, and the card's line with the age: "Turns {n}".
    pub countdown_show_age: &'static str,
    pub countdown_turns: &'static str,
    /// The Reminder choices, in `fern_core::countdown::REMINDER_DAYS` order.
    pub countdown_reminder_days: [&'static str; 5],
    /// The Reminder's Custom: its name, the Day / Week unit, "Days in advance" / "Weeks in
    /// advance", "Remind me at", the summary ("{n} days ahead") and a custom reminder's label.
    /// The names of `fern_core::countdown::PRESETS`, in order.
    pub countdown_presets: [&'static str; 8],
    /// The names of `fern_core::countdown::ANNIVERSARY_PRESETS`, in order.
    pub countdown_anniversary_presets: [&'static str; 8],
    pub countdown_custom: &'static str,
    pub countdown_custom_units: [&'static str; 2],
    pub countdown_custom_advance: [&'static str; 2],
    pub countdown_custom_at: &'static str,
    pub countdown_custom_ahead: &'static str,
    pub countdown_custom_label: &'static str,
    /// Day Calculation Mode and its two choices with their explanations.
    pub countdown_calc_mode: &'static str,
    pub countdown_calc_options: [(&'static str, &'static str); 2],
    /// Show, the heading of its choices, and the choices in `ShowInList::ALL` order.
    /// A birthday's Counting Mode and its choices: Countdown, Count Up.
    pub countdown_counting_mode: &'static str,
    pub countdown_counting_options: [&'static str; 2],
    pub countdown_show: &'static str,
    pub countdown_show_heading: &'static str,
    pub countdown_show_options: [&'static str; 5],
    pub countdown_new: &'static str,
    pub countdown_edit: &'static str,
    pub countdown_name: &'static str,
    pub countdown_days_until: &'static str,
    pub countdown_days_since: &'static str,
    pub countdown_switch_hint: &'static str,
    pub countdown_empty: &'static str,
    pub confirm_delete_countdown: &'static str,
    pub confirm_delete_timer: &'static str,
    pub confirm_delete_summary_template: &'static str,
    pub confirm_delete_task_template: &'static str,
    pub confirm_delete_script: &'static str,
    pub confirm_delete_attachment: &'static str,
    /// Unit letters for years, months, days in countdown cards.
    pub unit_letters: [&'static str; 3],
    /// A countdown's date under its numbers, as a `task_rows::format_day` template; `None` for
    /// the numeric date format.
    pub countdown_date: Option<&'static str>,
    /// "MM/DD/YYYY" (`false`) or "DD.MM.YYYY" (`true`) in countdown subtitles.
    pub dotted_dates: bool,
    pub year_first_dates: bool,
    pub matrix_urgent_important: &'static str,
    pub matrix_not_urgent_important: &'static str,
    pub matrix_urgent_unimportant: &'static str,
    pub matrix_not_urgent_unimportant: &'static str,
    pub search_placeholder: &'static str,
    pub search_hint: &'static str,
    pub nothing_found: &'static str,
    pub search_filter_task: &'static str,
    pub search_filter_tag: &'static str,
    pub search_filter_list: &'static str,
    pub view_more: &'static str,
    pub language: &'static str,

    // Smart lists and the lists column.
    pub inbox: &'static str,
    pub all: &'static str,
    pub today: &'static str,
    pub tomorrow: &'static str,
    pub month: &'static str,
    pub agenda: &'static str,
    pub no_tasks_today: &'static str,
    pub week: &'static str,
    pub day: &'static str,
    pub multi_day: &'static str,
    /// "{n} Days".
    pub n_days: &'static str,
    pub one_day: &'static str,
    pub year: &'static str,
    pub multi_week: &'static str,
    pub n_weeks: &'static str,
    pub noon: &'static str,
    pub next_7_days: &'static str,
    /// Shorter label for the lists column, where the full title may not fit.
    pub next_7_days_short: &'static str,
    pub completed: &'static str,
    pub wont_do: &'static str,
    /// The menus' item that marks a task "Won't Do".
    pub mark_wont_do: &'static str,
    /// The task's right-click menu: "Move to", a list side menu.
    pub move_to: &'static str,
    pub all_dates: &'static str,
    pub this_week: &'static str,
    pub last_week: &'static str,
    pub this_month: &'static str,
    pub all_lists: &'static str,
    pub other_date: &'static str,
    pub range_start: &'static str,
    pub range_end: &'static str,
    pub confirm: &'static str,
    pub no_wont_do: &'static str,
    pub reopen: &'static str,
    pub duplicate: &'static str,
    pub pin: &'static str,
    pub save_as_template: &'static str,
    pub start_focus: &'static str,
    pub print: &'static str,
    pub copy_link: &'static str,
    pub copy_markdown: &'static str,
    pub copy_plain_text: &'static str,
    /// Word count shown in the task/note detail footer; `{n}` is the count.
    pub word_count: &'static str,
    /// The task menu's item that opens the task in a window of its own over the others.
    pub open_sticky_note: &'static str,
    pub sticky_notes: &'static str,
    pub sticky_opacity: &'static str,
    pub sticky_font_size: &'static str,
    pub sticky_pin_default: &'static str,
    pub sticky_hide_from_taskbar: &'static str,
    pub sticky_task_view_hint: &'static str,
    pub sticky_font_sizes: [&'static str; 4],
    pub sticky_save_failed: &'static str,
    pub shortcuts: &'static str,
    pub toggle_sidebar: &'static str,
    pub edit_keymap: &'static str,
    pub reload_keymap: &'static str,
    pub keymap_errors: &'static str,
    /// Labels of `keymap::SHORTCUT_SPECS`, in order.
    pub shortcut_labels: [&'static str; 60],
    /// Section titles in Settings → Shortcuts: General, Global, New Task, Quick Add, Edit Task,
    /// Navigation, Switch Views, Focus, Sticky Note.
    pub shortcut_groups: [&'static str; 9],
    /// Optional helper subtitles under each section title in Settings → Shortcuts.
    pub shortcut_group_hints: [Option<&'static str>; 9],
    pub shortcut_set: &'static str,
    pub shortcut_then: &'static str,
    pub shortcut_or: &'static str,
    /// "{other}" is replaced with the conflicting shortcut name(s).
    pub shortcut_conflict: &'static str,
    pub shortcut_reset_all: &'static str,
    pub shortcut_reset_confirm_title: &'static str,
    pub shortcut_reset_confirm_body: &'static str,
    /// Recording rule tooltips for `[modifier_or_fn, context_any, global]`.
    pub shortcut_rule_tips: [&'static str; 3],
    /// Live recording hints for `[modifier_or_fn, context_any, global]`.
    pub shortcut_record_hints: [&'static str; 3],
    /// Rejection explanations when a disallowed key is pressed for `[modifier_or_fn, context_any, global]`.
    pub shortcut_record_rejected: [&'static str; 3],
    /// Quick add syntax: (what, example).
    pub quick_add_help: [(&'static str, &'static str); 4],
    pub undo: &'static str,
    /// "{title}" is replaced: "Completed “{title}”".
    pub undo_completed: &'static str,
    pub undo_deleted: &'static str,
    pub filters: &'static str,
    pub filters_hint: &'static str,
    pub lists_hint: &'static str,
    pub add_filter: &'static str,
    pub edit_filter: &'static str,
    pub delete_filter: &'static str,
    pub filter_name: &'static str,
    pub filter_keywords: &'static str,
    pub filter_lists: &'static str,
    pub filter_date: &'static str,
    pub filter_priority: &'static str,
    pub filter_include: &'static str,
    pub filter_type: &'static str,
    pub filter_status: &'static str,
    /// "Normal" and "Advanced": the conditions, or a chain of them with AND and OR.
    pub filter_modes: [&'static str; 2],
    /// The Advanced chain's "Logic" between conditions: AND, OR.
    pub filter_logic: [&'static str; 2],
    /// A chain condition that must not match.
    pub filter_not: &'static str,
    pub filter_all: &'static str,
    pub filter_date_overdue: &'static str,
    pub filter_date_today: &'static str,
    pub filter_date_tomorrow: &'static str,
    pub filter_date_this_week: &'static str,
    pub filter_date_no_date: &'static str,
    pub filter_date_next_week: &'static str,
    pub filter_date_this_month: &'static str,
    pub filter_date_next_month: &'static str,
    pub filter_date_next_days: &'static str,
    pub filter_date_recurring: &'static str,
    pub filter_date_range: &'static str,
    pub filter_range_from: &'static str,
    pub filter_range_to: &'static str,
    pub filter_range_hint: &'static str,
    pub filter_range_error: &'static str,
    pub filter_mode_error: &'static str,
    pub filter_name_error: &'static str,
    pub filter_add_condition: &'static str,
    pub filter_no_tags: &'static str,
    pub filter_days: &'static str,
    pub filter_days_hint: &'static str,
    pub filter_days_error: &'static str,
    /// Task, Note (after "All").
    pub filter_kinds: [&'static str; 2],
    /// To Do, In Progress (after "All").
    pub filter_statuses: [&'static str; 2],
    pub filter_preview: &'static str,
    pub filter_preview_empty: &'static str,
    pub filter_examples: &'static str,
    /// The names of `fern_core::filter::examples()`, in its order.
    pub filter_example_names: [&'static str; 4],
    pub close: &'static str,
    pub edit_matrix: &'static str,
    pub matrix_dates: [&'static str; 5],
    pub order: &'static str,
    pub order_options: [&'static str; 2],
    pub order_directions: [&'static str; 2],
    pub focus_modes: [&'static str; 2],
    /// The full focus page's buttons: Start, Pause, Continue, End, and its collapse arrow.
    pub focus_start: &'static str,
    pub focus_pause: &'static str,
    pub focus_continue: &'static str,
    pub focus_end: &'static str,
    pub focus_collapse: &'static str,
    /// The focus page's button that opens the clock in a small window over the others.
    pub focus_mini_window: &'static str,
    /// The Shortcuts panel's system-wide shortcuts (Windows).
    pub global_shortcuts: &'static str,
    pub global_quick_add: &'static str,
    pub global_show_hide: &'static str,
    pub global_focus_mini: &'static str,
    pub global_pin_window: &'static str,
    pub press_shortcut: &'static str,
    pub shortcut_off: &'static str,
    pub shortcut_taken: &'static str,
    /// The focus page's picker tabs: Task, Timer, Habit.
    pub focus_picker_tabs: [&'static str; 3],
    pub focus_picker_search: &'static str,
    /// After a timer's Pomo minutes field: "mins".
    pub timer_mins: &'static str,
    pub templates: &'static str,
    pub no_templates: &'static str,
    pub show_completed: &'static str,
    pub add_section: &'static str,
    pub rename_section: &'static str,
    pub delete_section: &'static str,
    pub not_sectioned: &'static str,
    pub section_name: &'static str,
    pub view_label: &'static str,
    pub hide_completed: &'static str,
    pub show_details: &'static str,
    pub hide_details: &'static str,
    pub link_parent: &'static str,
    pub unlink_parent: &'static str,
    pub linked_parent: &'static str,
    pub link_other_parent: &'static str,
    pub collapse_all_subtasks: &'static str,
    pub expand_all_subtasks: &'static str,
    pub unpin: &'static str,
    pub task_activities: &'static str,
    pub list_activities: &'static str,
    /// Created, renamed, completed, reopened, date, date cleared, priority, moved, deleted,
    /// restored, won't do, in progress: what the activity log says for each kind.
    pub activity_kinds: [&'static str; 12],
    pub convert_to_note: &'static str,
    pub snooze: &'static str,
    /// The overdue group's button that moves its tasks to a later day.
    pub postpone: &'static str,
    pub check_in: &'static str,
    pub add_from_template: &'static str,
    pub convert_to_task: &'static str,
    pub delete: &'static str,
    /// Full month names in the genitive ("September" / "сентября") for day headers.
    pub months_genitive: [&'static str; 12],
    pub trash: &'static str,
    pub empty_trash: &'static str,
    pub confirm_empty_trash: &'static str,
    pub confirm_empty_trash_hint: &'static str,
    pub confirm_delete_list_hint: &'static str,
    pub confirm_delete_tag_hint: &'static str,
    pub confirm_delete_filter_hint: &'static str,
    pub confirm_delete_forever_hint: &'static str,
    pub confirm_delete_forever_single_hint: &'static str,
    pub confirm_delete_section_hint: &'static str,
    pub batch_add_tasks: &'static str,
    /// "{n}" is replaced with the number of lines to add.
    pub batch_add_tasks_desc: &'static str,
    pub batch_add_as_one: &'static str,
    pub trash_is_empty: &'static str,
    pub restore: &'static str,
    pub delete_forever: &'static str,
    pub tags: &'static str,
    pub upload_attachment: &'static str,
    pub attachment_failed: &'static str,
    pub attachment_size_limit: &'static str,
    pub sync_file_failed: &'static str,
    pub task_save_failed: &'static str,
    pub new_task_prompt: &'static str,
    pub attachment_preparing: &'static str,
    pub attachment: &'static str,
    pub type_in_tag: &'static str,
    pub checklist: &'static str,
    pub checklist_uncheck_all: &'static str,
    pub checklist_clear_completed: &'static str,
    pub sync: &'static str,
    pub features: &'static str,
    pub arrange_tasks: &'static str,
    pub add_timer: &'static str,
    pub edit_timer: &'static str,
    pub timer_name: &'static str,
    pub timer_mode: &'static str,
    /// The editor toolbar's buttons after the heading, as the original's tooltips name them:
    /// Bold, Highlight, Check Item, Bulleted List, Numbered List, Italic, Underline, Strike,
    /// Horizontal Line, Time, Link, Code, Quote.
    pub markdown_tools: [&'static str; 13],
    pub heading_options: &'static str,
    /// "Heading" before its level: "Heading 1".
    pub heading: &'static str,
    pub batch_management: &'static str,
    /// `{n}` records chosen in Batch Management.
    pub n_selected: &'static str,
    pub select_all: &'static str,
    /// Batch Management's filters: their names and choices.
    pub batch_filters: [&'static str; 3],
    pub batch_dates: [&'static str; 8],
    pub batch_lengths: [&'static str; 6],
    pub batch_linked: [&'static str; 3],
    /// The Custom duration's hint, with its Min and Max placeholders.
    pub batch_minutes_hint: &'static str,
    pub batch_min: &'static str,
    pub batch_max: &'static str,
    pub notifications_page: &'static str,
    /// Settings → Notifications: each switch's name and what it does.
    pub notification_items: [(&'static str, &'static str); 3],
    pub minimize_to_tray: &'static str,
    pub completion_sound: &'static str,
    pub tray_focus: &'static str,
    pub tray_start_pomo: &'static str,
    pub tray_stopwatch: &'static str,
    pub tray_always_on_top: &'static str,
    pub tray_quit: &'static str,
    pub daily_time: &'static str,
    /// The daily notification's title; `{n}` is how many tasks.
    pub daily_title: &'static str,
    pub daily_empty: &'static str,
    /// Settings → Features: each section's name and what it is for.
    pub feature_items: [(&'static str, &'static str); 7],
    pub sync_server: &'static str,
    pub sync_token: &'static str,
    pub sync_passphrase: &'static str,
    pub sync_passphrase_placeholder: &'static str,
    pub sync_passphrase_hint: &'static str,
    pub sync_hint: &'static str,
    pub sync_never: &'static str,
    pub sync_done: &'static str,
    pub sync_failed: &'static str,
    /// Protocol version mismatch; `{client}` and `{server}` are replaced.
    pub sync_incompatible_protocol: &'static str,
    pub sync_e2ee_passphrase_required: &'static str,
    pub sync_e2ee_wrong_passphrase: &'static str,
    pub sync_off: &'static str,
    /// Settings → Sync: the local data folder above the optional server.
    pub data_and_sync: &'static str,
    /// Settings → Integrations: calendars subscribed to by address.
    pub integrations: &'static str,
    pub subscribed_calendars: &'static str,
    pub subscriptions_hint: &'static str,
    pub add_calendar: &'static str,
    pub subscription_name: &'static str,
    pub subscription_name_placeholder: &'static str,
    pub color: &'static str,
    pub subscription_reading: &'static str,
    pub subscription_events: &'static str,
    pub subscription_show: &'static str,
    pub subscription_hide: &'static str,
    pub subscription_read_now: &'static str,
    pub subscription_edit: &'static str,
    pub subscription_delete: &'static str,
    pub subscription_change_source: &'static str,
    /// The original's Holidays: a public calendar of the language's country.
    pub add_holidays: &'static str,
    pub holidays_name: &'static str,
    pub holidays_url: &'static str,
    pub holiday_source_failed: &'static str,
    pub data_on_device: &'static str,
    pub data_folder: &'static str,
    pub open_folder: &'static str,
    pub data_hint: &'static str,
    pub sync_optional: &'static str,
    /// The tips added to the Inbox on the first start: titles and notes (Markdown).
    pub welcome_tasks: [(&'static str, &'static str); 6],
    pub sync_running: &'static str,
    pub input_box_setting: &'static str,
    pub input_box_simple: &'static str,
    pub input_box_detailed: &'static str,
    pub input_box_placeholder: &'static str,
    pub add: &'static str,
    pub lists: &'static str,
    /// Shown in the "…" menu when the current view has no actions.
    pub no_actions: &'static str,

    // Task list.
    pub add_task: &'static str,
    pub no_tasks: &'static str,
    pub empty_tasks_hint: &'static str,
    pub free_day: &'static str,
    pub free_day_hint: &'static str,
    pub no_completed_tasks: &'static str,
    pub untitled: &'static str,
    pub group_tasks: &'static str,
    pub group_notes: &'static str,
    pub group_completed: &'static str,
    pub group_by: &'static str,
    pub sort_by: &'static str,
    /// Custom, Date, Created Time, Modified Time, Title, Tag, Priority.
    pub sort_options: [&'static str; 7],
    /// Custom, Date, Created Time, Tag, Priority, None.
    pub group_options: [&'static str; 7],
    pub group_overdue: &'static str,
    pub group_later: &'static str,
    pub group_no_date: &'static str,
    pub group_no_tags: &'static str,

    // Task details.
    pub select_task: &'static str,
    pub notes_placeholder: &'static str,
    pub due_date: &'static str,
    pub next_week: &'static str,
    pub time: &'static str,
    pub no_time: &'static str,
    pub time_placeholder: &'static str,
    pub reminder: &'static str,
    pub duration_tab: &'static str,
    pub range_all_day: &'static str,
    /// Custom reminders: "{n}" is replaced by the amount.
    pub reminder_minutes_early: &'static str,
    pub reminder_hours_early: &'static str,
    pub reminder_days_early: &'static str,
    pub reminder_weeks_early: &'static str,
    pub reminder_custom_units: [&'static str; 4],
    pub reminder_in_advance: &'static str,
    pub reminder_at: &'static str,
    /// "Remind at {time} on {date}."
    pub reminder_summary: &'static str,
    pub notifications: &'static str,
    pub no_notifications: &'static str,
    /// Reminder offsets for tasks with a time, matching `fern_core::reminder::TIMED_OFFSETS`.
    pub reminders_timed: [&'static str; 5],
    /// Reminder offsets for all-day tasks, matching `fern_core::reminder::ALL_DAY_OFFSETS`.
    pub reminders_all_day: [&'static str; 5],
    pub repeat_custom: &'static str,
    pub repeat_every: &'static str,
    pub repeat_each: &'static str,
    pub repeat_on_the: &'static str,
    pub repeat_workday: &'static str,
    pub repeat_workday_nth: [&'static str; 2],
    pub repeat_workday_word: &'static str,
    pub repeat_by_due: &'static str,
    pub repeat_by_completion: &'static str,
    pub repeat_by_specific: &'static str,
    pub repeat_specific_dates: &'static str,
    pub repeat_skip_weekends: &'static str,
    /// first, second, third, fourth, last.
    pub repeat_nth: [&'static str; 5],
    pub repeat_last_day: &'static str,
    pub repeat_last_day_title: &'static str,
    /// Units for the custom repeat: day, week, month, year.
    pub repeat_units: [&'static str; 4],
    /// The same units after a number: "Every 2 weeks".
    pub repeat_units_plural: [&'static str; 4],
    /// Joins a repeat with its weekdays: "Weekly on Mo, We".
    pub repeat_on: &'static str,
    pub weekdays_min: [&'static str; 7],
    pub cancel: &'static str,
    pub repeat: &'static str,
    pub repeat_none: &'static str,
    pub repeat_daily: &'static str,
    pub repeat_weekly: &'static str,
    pub repeat_monthly: &'static str,
    pub repeat_yearly: &'static str,
    pub repeat_weekdays: &'static str,
    /// The hint next to "Every Weekday".
    pub weekdays_range: &'static str,
    pub date_tab: &'static str,
    pub next_month: &'static str,
    pub clear: &'static str,
    pub ok: &'static str,
    pub priority_high: &'static str,
    pub priority_medium: &'static str,
    pub priority_low: &'static str,
    pub priority_none: &'static str,
    pub new_tag: &'static str,
    pub add_item: &'static str,
    pub add_subtask: &'static str,

    // Dates.
    pub yesterday: &'static str,
    /// Monday first.
    pub weekdays_short: [&'static str; 7],
    /// Compact column headings for the year overview, Monday first.
    pub weekdays_year: [&'static str; 7],
    /// One-letter weekday names, Monday first, for the mini week.
    pub weekdays_narrow: [&'static str; 7],
    /// Full weekday names, Monday first.
    pub weekdays_full: [&'static str; 7],
    pub months: [&'static str; 12],
    pub months_short: [&'static str; 12],
    /// "13 Nov" (`true`) or "Nov 13" (`false`).
    pub day_before_month: bool,
    /// Closed tasks' day groups (Completed, Won't Do), in this year and another, as the
    /// original writes them in this language; see `task_rows::format_day` for the fields.
    pub closed_day: [&'static str; 2],
    /// A closed task's date at its row's end, in this year and another.
    pub closed_date: [&'static str; 2],
    pub month_title: &'static str,
    pub short_date: [&'static str; 2],
    pub day_header: [&'static str; 2],
    pub time_meridiem: [&'static str; 2],
    /// A day named after today ("Today", "Yesterday", "Today's Habit"), with its weekday.
    pub relative_day: &'static str,
    /// Settings window pages: Smart List, Date & Time, Appearance, More, Shortcuts, About.
    pub settings_pages: [&'static str; 6],
    /// Show, Show if not empty, Hide, and Show (Omit Empty Tags) for the Tags section.
    pub visibility_names: [&'static str; 4],
    pub start_week_on: &'static str,
    pub time_format: &'static str,
    pub date_format: &'static str,
    pub show_week_numbers: &'static str,
    pub show_countdown_mode: &'static str,
    /// 24-hour, 12-hour.
    pub time_formats: [&'static str; 2],
    /// Monday, Sunday, Saturday, as in `fern_core::WEEK_STARTS`.
    pub week_start_names: [&'static str; 3],
    pub color_series: &'static str,
    /// Display names in the order of the bundled preset catalog (`theme::LIGHT_PRESETS` / `DARK_PRESETS`).
    pub light_theme_names: [&'static str; 11],
    pub dark_theme_names: [&'static str; 6],
    pub dark_theme: &'static str,
    pub night_theme: &'static str,
    pub switch_theme: &'static str,
    pub theme_override_hint: &'static str,
    pub theme_once: &'static str,
    pub theme_always: &'static str,
    pub theme_save_failed: &'static str,
    pub follow_system_dark_mode: &'static str,
    pub custom_themes: &'static str,
    pub custom_themes_hint: &'static str,
    pub create_theme_template: &'static str,
    pub open_themes_folder: &'static str,
    pub reload_themes: &'static str,
    pub theme_light_badge: &'static str,
    pub theme_dark_badge: &'static str,
    pub task_default: &'static str,
    pub smart_recognition: &'static str,
    pub mini_calendar: &'static str,
    pub show_mini_calendar: &'static str,
    pub date_recognition: &'static str,
    pub date_recognition_hint: &'static str,
    pub remove_date_text: &'static str,
    pub tag_recognition: &'static str,
    pub tag_recognition_hint: &'static str,
    pub remove_tag_text: &'static str,
    pub default_list: &'static str,
    pub default_date: &'static str,
    pub default_duration: &'static str,
    pub default_timed_reminder: &'static str,
    pub default_all_day_reminder: &'static str,
    pub default_priority: &'static str,
    pub default_tag: &'static str,
    pub default_add_to: &'static str,
    pub overdue_section_at: &'static str,
    pub add_to_top: &'static str,
    pub add_to_bottom: &'static str,
    /// "None" in a settings dropdown.
    pub none_option: &'static str,
    /// Today, Tomorrow, Day after Tomorrow, Next Week.
    pub default_dates: [&'static str; 4],
    /// In the order of `fern_core::DURATIONS`.
    pub durations: [&'static str; 10],
    pub default_list_hint: &'static str,
    pub about_tagline: &'static str,
    pub backup_restore: &'static str,
    pub generate_backup: &'static str,
    pub import_backups: &'static str,
    pub restore_on_restart: &'static str,
    pub auto_backups: &'static str,
    pub auto_backups_empty: &'static str,
    pub restore_action: &'static str,
    pub restore_backup_confirm_title: &'static str,
    /// `{file}` is replaced by the backup filename.
    pub restore_backup_confirm_body: &'static str,
    pub diagnostics: &'static str,
    pub db_integrity: &'static str,
    pub db_integrity_ok: &'static str,
    pub db_integrity_issue: &'static str,
    /// `{file}` is replaced by the preserved corrupt database filename.
    pub db_corrupt_recovered: &'static str,
    pub save_diagnostic_log: &'static str,
    /// `{path}` is replaced by the saved file path.
    pub diagnostic_log_saved: &'static str,
    /// `{error}` is replaced by the redacted error message.
    pub diagnostic_log_failed: &'static str,
    /// After importing a TickTick backup; `{tasks}` and `{lists}` are counts.
    pub imported_from_ticktick: &'static str,
    pub version: &'static str,
    pub license: &'static str,
    pub updates: &'static str,
    pub check_updates_automatically: &'static str,
    pub check_for_updates: &'static str,
    pub update_checking: &'static str,
    pub update_up_to_date: &'static str,
    /// `{version}` is the new version.
    pub update_downloading: &'static str,
    pub update_ready: &'static str,
    pub update_available: &'static str,
    pub update_installed: &'static str,
    /// `{error}` is what went wrong.
    pub update_failed: &'static str,
    pub restart_to_update: &'static str,
    /// The title bar's button and the release's page; `{version}` is the new version.
    pub update_to: &'static str,
    pub whats_new: &'static str,
    pub whats_new_in_version: &'static str,
    pub release_notes_missing: &'static str,
    pub release_on_github: &'static str,
    pub add_list: &'static str,
    pub edit_list: &'static str,
    pub list_name: &'static str,
    pub list_color: &'static str,
    pub list_view_type: &'static str,
    pub list_folder: &'static str,
    pub list_type: &'static str,
    pub task_list: &'static str,
    pub note_list: &'static str,
    pub show_in_smart_list: &'static str,
    pub smart_all_tasks: &'static str,
    pub smart_do_not_show: &'static str,
    pub new_folder: &'static str,
    pub edit_folder: &'static str,
    pub ungroup: &'static str,
    pub archive_list: &'static str,
    pub unarchive_list: &'static str,
    pub archived_lists: &'static str,
    pub list_copy_suffix: &'static str,
    pub layout_list: &'static str,
    pub layout_kanban: &'static str,
    pub layout_timeline: &'static str,
    pub emoji_search: &'static str,
    pub emoji_random: &'static str,
    pub emoji_reset: &'static str,
    /// The emoji picker's categories, in `emoji::CATEGORIES` order.
    pub emoji_categories: [&'static str; 8],
    pub tag_color: &'static str,
    pub parent_tag: &'static str,
    pub add_tags: &'static str,
    pub edit_tag: &'static str,
    pub merge_tags: &'static str,
    pub merge_tags_hint: &'static str,
    pub select_tag: &'static str,
    pub add_subtag: &'static str,
    pub skip_recurrence: &'static str,
    pub complete_forever: &'static str,
    pub custom_date: &'static str,
    pub no_tasks_scheduled: &'static str,
    pub write_comment: &'static str,
    pub no_tasks_tomorrow: &'static str,
    pub plan_ahead: &'static str,
    pub add_task_to: &'static str,
    pub next_weekday: &'static str,
    pub todays_habit: &'static str,
    pub suggested_tasks: &'static str,
    pub recently_added: &'static str,
    pub long_unfinished: &'static str,
    pub added_on: &'static str,
    pub created_days_ago: &'static str,
    pub add_to_today: &'static str,
    pub more: &'static str,
    pub no_suggestions: &'static str,
    pub matrix_add_placeholder: &'static str,
    pub matrix_saved_elsewhere: &'static str,
    pub matrix_unavailable_list: &'static str,
    pub matrix_unavailable_tag: &'static str,
    pub matrix_apply_date_template: &'static str,
    pub matrix_reset_manual_order: &'static str,
    pub matrix_already_in_quadrant: &'static str,
    pub matrix_drop_unsupported: &'static str,
    pub matrix_kind_label: &'static str,
    pub matrix_kinds: [&'static str; 3],
    pub matrix_invalid_name: &'static str,
    pub matrix_order_asc: &'static str,
    pub matrix_order_desc: &'static str,
    pub matrix_show_subtasks: &'static str,
    pub matrix_hide_subtasks: &'static str,
    pub matrix_open_in_tasks: &'static str,
    pub in_days: &'static str,
    pub days_ago: &'static str,
    pub view_options: &'static str,
    pub show_date_by: &'static str,
    pub task_time: &'static str,
    pub date_countdown: &'static str,
    pub task_detail_layout: &'static str,
    pub task_detail_layout_hint: &'static str,
    pub side_peek: &'static str,
    pub pop_up: &'static str,
    pub add_section_left: &'static str,
    pub add_section_right: &'static str,
    pub search_hint_task: &'static str,
    pub search_hint_tag: &'static str,
    pub search_hint_list: &'static str,
    pub with_tags: &'static str,
    pub no_tags: &'static str,
    pub all_option: &'static str,
    pub next_week_range: &'static str,
    pub last_month: &'static str,
    pub custom_range: &'static str,
    pub range_from: &'static str,
    pub range_until: &'static str,
    pub comments: &'static str,
    pub text_format: &'static str,
    pub set_reminder: &'static str,
    /// The board's "+ New section" after its last column.
    pub new_section: &'static str,
    pub days_left: &'static str,
    /// After an overdue date in the details: "Oct 21, 5d overdue".
    pub days_overdue: &'static str,
    pub comment_tip: &'static str,
    pub show_habits: &'static str,
    pub show_focus: &'static str,
    pub countdown_fixed_hint: &'static str,
    pub countdown_own_hint: &'static str,
    pub habit_fixed_hint: &'static str,
    pub habit_own_hint: &'static str,
    pub color_tabs: [&'static str; 3],
    pub habit_color_options: [&'static str; 3],
    pub countdown_color_options: [&'static str; 2],
    pub add_subtask_item: &'static str,
    pub group_and_sort: &'static str,
    pub calendar_color: &'static str,
    pub calendar_style: &'static str,
    pub show_weekends: &'static str,
    pub show_all_repeats: &'static str,
    pub show_countdowns: &'static str,
    pub color_by: &'static str,
    pub item_style: &'static str,
    pub item_style_hint: &'static str,
    pub style_modern: &'static str,
    pub style_classic: &'static str,
    pub calendar_go_to_date: &'static str,
    pub calendar_year_date_hint: &'static str,
    pub calendar_invalid_year_date: &'static str,
    pub calendar_saved_outside_filter: &'static str,
    pub heatmap: &'static str,
    pub heatmap_hint: &'static str,
    pub heatmap_hide: &'static str,
    pub heatmap_show: &'static str,
    pub arrange_tasks_title: &'static str,
    pub arrange_empty: &'static str,
    pub preview_task: &'static str,
    pub color_by_options: [&'static str; 3],
    pub color_by_hints: [&'static str; 3],
    pub arrange_tabs: [&'static str; 3],
    pub kanban_size: &'static str,
    pub kanban_size_small: &'static str,
    pub kanban_size_medium: &'static str,
    pub kanban_size_large: &'static str,
    pub kanban_column_color: &'static str,
    pub kanban_show_input_box: &'static str,
    pub group_pinned: &'static str,
    pub group_unpinned: &'static str,
    pub batch_selected: &'static str,
    pub clear_selection: &'static str,
    pub batch_complete: &'static str,
    pub batch_reopen: &'static str,
    pub batch_merge: &'static str,
    pub in_progress: &'static str,
    pub mark_in_progress: &'static str,
    pub clear_in_progress: &'static str,
    pub location: &'static str,
    pub add_location: &'static str,
    pub location_placeholder: &'static str,
    pub timezone: &'static str,
    pub timezone_local: &'static str,
    pub timezone_floating: &'static str,
    pub timezone_floating_hint: &'static str,
    pub timezone_search: &'static str,
    pub repeat_forgetting_curve: &'static str,
    pub repeat_forgetting_curve_hint: &'static str,
    pub repeat_forgetting_step: &'static str,
    pub repeat_lunar: &'static str,
    pub repeat_lunar_yearly: &'static str,
    pub repeat_lunar_monthly: &'static str,
    pub repeat_skip_holidays: &'static str,
    pub snooze_5m: &'static str,
    pub snooze_15m: &'static str,
    pub snooze_1h: &'static str,
    pub dismiss: &'static str,
    pub mark_done: &'static str,
    pub annoying_alert: &'static str,
    pub annoying_alert_hint: &'static str,
    pub cal_source_ics: &'static str,
    pub cal_source_caldav: &'static str,
    pub cal_provider: &'static str,
    pub cal_provider_url: &'static str,
    pub cal_provider_caldav: &'static str,
    pub cal_provider_icloud: &'static str,
    pub cal_provider_google: &'static str,
    pub cal_provider_outlook: &'static str,
    pub cal_provider_nextcloud: &'static str,
    pub cal_provider_fastmail: &'static str,
    pub cal_username: &'static str,
    pub cal_password: &'static str,
    pub cal_discover: &'static str,
    pub cal_writable: &'static str,
    pub cal_badge_caldav: &'static str,
    pub cal_new_event: &'static str,
    pub cal_edit_event: &'static str,
    pub cal_event_title: &'static str,
    pub cal_calendar_target: &'static str,
    pub cal_all_day: &'static str,
    pub secondary_timezone: &'static str,
    pub secondary_timezone_none: &'static str,
    pub secondary_timezone_hint: &'static str,
    pub timeline_show_weekends: &'static str,
    pub kanban_workflow: &'static str,
    pub kanban_workflow_hint: &'static str,
    pub col_todo: &'static str,
    pub matrix_profile: &'static str,
    pub matrix_new_profile: &'static str,
    pub matrix_profile_name: &'static str,
    pub matrix_rename_profile: &'static str,
    pub matrix_delete_profile: &'static str,
    pub matrix_default_profile: &'static str,
    // Things frontend & multi-frontend settings.
    pub beta_badge: &'static str,
    pub section_things: &'static str,
    pub feature_tasks_item: (&'static str, &'static str),
    pub feature_things_item: (&'static str, &'static str),
    pub feature_last_frontend_hint: &'static str,
    pub things_inbox: &'static str,
    pub things_today: &'static str,
    pub things_today_evening: &'static str,
    pub things_upcoming: &'static str,
    pub things_anytime: &'static str,
    pub things_someday: &'static str,
    pub things_logbook: &'static str,
    pub things_areas: &'static str,
    pub things_projects: &'static str,
    pub things_new_todo: &'static str,
    pub things_new_project: &'static str,
    pub things_new_area: &'static str,
    pub things_when: &'static str,
    pub things_deadline: &'static str,
    pub things_clear_when: &'static str,
    pub things_notes_placeholder: &'static str,
    pub things_project_notes_placeholder: &'static str,
    pub things_checklist_add: &'static str,
    pub things_logged_items: &'static str,
    pub things_empty_inbox: &'static str,
    pub things_empty_today: &'static str,
    pub things_empty_upcoming: &'static str,
    pub things_empty_anytime: &'static str,
    pub things_empty_someday: &'static str,
    pub things_empty_logbook: &'static str,
    pub things_empty_project: &'static str,
    pub things_empty_area: &'static str,
    pub things_drop_evening_hint: &'static str,
    pub things_date_placeholder: &'static str,
    pub things_complete_project: &'static str,
    pub things_reopen_project: &'static str,
    pub things_move: &'static str,
    // Plugins & user scripts.
    pub plugins: &'static str,
    pub plugins_installed: &'static str,
    pub plugins_scripts: &'static str,
    pub plugins_browse: &'static str,
    pub plugins_safe_mode: &'static str,
    pub plugins_safe_mode_hint: &'static str,
    pub plugins_safe_mode_active: &'static str,
    pub plugins_new_template: &'static str,
    pub plugins_install_zip: &'static str,
    pub plugins_open_folder: &'static str,
    pub plugins_reload: &'static str,
    pub plugins_reload_all: &'static str,
    pub plugins_uninstall: &'static str,
    pub plugins_enable: &'static str,
    pub plugins_disable: &'static str,
    pub plugins_disabled: &'static str,
    pub plugins_task_create_cancelled: &'static str,
    pub plugins_task_complete_cancelled: &'static str,
    pub plugins_permissions: &'static str,
    pub plugins_permissions_approve: &'static str,
    pub plugins_permissions_changed: &'static str,
    pub plugins_settings: &'static str,
    pub plugins_open: &'static str,
    pub plugins_back: &'static str,
    pub plugins_logs: &'static str,
    pub plugins_open_logs: &'static str,
    pub plugins_clear_logs: &'static str,
    pub plugins_reset_enable: &'static str,
    pub plugins_circuit_broken: &'static str,
    pub plugins_empty: &'static str,
    pub plugins_metrics: &'static str,
    pub plugins_script_new: &'static str,
    pub plugins_script_run: &'static str,
    pub plugins_script_save: &'static str,
    pub plugins_script_delete: &'static str,
    pub plugins_script_name: &'static str,
    pub plugins_script_trigger: &'static str,
    pub plugins_script_empty: &'static str,
    pub plugins_registries: &'static str,
    pub plugins_registry_refresh: &'static str,
    pub plugins_registry_install: &'static str,
    pub plugins_registry_update: &'static str,
    pub plugins_registry_installed: &'static str,
    pub plugins_registry_add_url: &'static str,
    pub plugins_view_error_title: &'static str,
    pub plugins_commands: &'static str,
    pub plugins_secret_placeholder: &'static str,
    pub plugin_status_active: &'static str,
    pub plugin_status_idle: &'static str,
    pub plugin_status_broken: &'static str,
    pub plugin_status_error: &'static str,
    pub plugin_clear_secret: &'static str,
    pub plugin_no_logs: &'static str,
    pub plugin_trigger_arg: &'static str,
    pub plugin_code_label: &'static str,
    pub plugin_run_ok: &'static str,
    pub plugin_run_err: &'static str,
    pub plugin_loading: &'static str,
    pub plugin_trigger_manual: &'static str,
    pub plugin_trigger_command: &'static str,
    pub plugin_trigger_event: &'static str,
    pub plugin_trigger_interval: &'static str,
    // Life Clock.
    pub section_life_clock: &'static str,
    pub life_clock_months: [&'static str; 12],
    pub life_clock_you_are: &'static str,
    pub life_clock_age_suffix: &'static str,
    pub life_clock_and: &'static str,
    /// Order: years, months, days, hours, minutes, seconds; triplet `[one, few, many]`.
    pub life_clock_units: [[&'static str; 3]; 6],
    pub life_clock_set_birth_title: &'static str,
    pub life_clock_set_birth_hint: &'static str,
    pub life_clock_about_title: &'static str,
    pub life_clock_about_body: &'static str,
    pub life_clock_birth_date: &'static str,
    pub life_clock_invalid_date: &'static str,
    pub life_clock_palette: &'static str,
    pub life_clock_palette_classic: &'static str,
    pub life_clock_palette_theme: &'static str,
    pub life_clock_quote_hint: &'static str,
    pub life_clock_hide_sidebar: &'static str,
    pub life_clock_show_sidebar: &'static str,
    pub life_clock_help_tooltip: &'static str,
    // Horizons.
    pub section_horizons: &'static str,
    pub horizons_overview: &'static str,
    pub horizons_days: &'static str,
    pub horizons_weeks: &'static str,
    pub horizons_months: &'static str,
    pub horizons_quarters: &'static str,
    pub horizons_years: &'static str,
    pub horizons_decades: &'static str,
    pub horizons_life: &'static str,
    pub horizons_boards: &'static str,
    pub horizons_add_placeholder: &'static str,
    pub horizons_new_board: &'static str,
    pub horizons_add_column: &'static str,
    pub horizons_week_prefix: &'static str,
    pub horizons_week_suffix: &'static str,
    pub horizons_quarter_prefix: &'static str,
    pub horizons_decade_suffix: &'static str,
    /// "{n}" is replaced by the count of incomplete items from earlier periods.
    pub horizons_rollover_label: &'static str,
    pub horizons_move_all_here: &'static str,
    pub horizons_move_here: &'static str,
    pub horizons_jump_today: &'static str,
    pub horizons_columns: &'static str,
    pub horizons_show_dated_in_week: &'static str,
    pub horizons_show_completed: &'static str,
    pub horizons_show_prompt: &'static str,
    pub horizons_filter_title: &'static str,
    pub horizons_all_boards: &'static str,
    pub horizons_all_tags: &'static str,
    pub horizons_reset_filters: &'static str,
    pub horizons_horizon_label: &'static str,
    pub horizons_clear_horizon: &'static str,
    pub horizons_toggle_nav: &'static str,
    pub horizons_empty_inbox: &'static str,
    pub horizons_empty_board: &'static str,
    pub horizons_default_column: &'static str,
    pub horizons_prompt_hint: &'static str,
    pub horizons_dated_in_week: &'static str,
    pub horizons_drop_hint: &'static str,
    pub horizon_this_week: &'static str,
    pub horizon_this_month: &'static str,
    pub horizon_this_quarter: &'static str,
    pub horizon_this_year: &'static str,
    pub horizon_future: &'static str,
    pub calendar_week_goals: &'static str,
    pub calendar_month_goals: &'static str,
    pub calendar_year_goals: &'static str,
    pub calendar_drop_to_schedule: &'static str,
}

mod en;
mod ru;
mod zh_cn;

pub use en::EN;
pub use ru::RU;
pub use zh_cn::ZH_CN;

fn life_clock_plural_index(locale: Locale, n: u32) -> usize {
    match locale {
        Locale::En => usize::from(n != 1),
        Locale::Ru => {
            let mod100 = n % 100;
            if (11..=14).contains(&mod100) {
                2
            } else {
                match n % 10 {
                    1 => 0,
                    2..=4 => 1,
                    _ => 2,
                }
            }
        }
        Locale::ZhCn => 0,
    }
}

/// Formats a live Life Clock age breakdown in the given locale with proper pluralization.
pub fn format_life_clock_age(
    locale: Locale,
    years: u32,
    months: u32,
    days: u32,
    hours: u32,
    minutes: u32,
    seconds: u32,
) -> String {
    let s = locale.strings();
    let unit = |idx: usize, n: u32| s.life_clock_units[idx][life_clock_plural_index(locale, n)];
    let (y, mo, d, h, min, sec) = (
        unit(0, years),
        unit(1, months),
        unit(2, days),
        unit(3, hours),
        unit(4, minutes),
        unit(5, seconds),
    );
    match locale {
        Locale::ZhCn => format!(
            "{years} {y} {months} {mo} {days} {d} {hours} {h} {minutes} {min} {seconds} {sec}{}",
            s.life_clock_age_suffix,
        ),
        Locale::En | Locale::Ru => format!(
            "{years} {y}, {months} {mo}, {days} {d}, {hours} {h}, {minutes} {min} {} {seconds} {sec}{}",
            s.life_clock_and, s.life_clock_age_suffix,
        ),
    }
}

impl Strings {
    /// Localized compact duration ("2h30m" / "2ч30м" / "2小时30分", "0m" / "0м" / "0分").
    pub fn format_minutes(&self, minutes: i64) -> String {
        match (minutes / 60, minutes % 60) {
            (0, m) => format!("{m}{}", self.duration_min_short),
            (h, 0) => format!("{h}{}", self.duration_hour_short),
            (h, m) => format!("{h}{}{m}{}", self.duration_hour_short, self.duration_min_short),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_preset_name_counts_match_catalog() {
        assert_eq!(EN.light_theme_names.len(), theme::LIGHT_PRESETS.len());
        assert_eq!(EN.dark_theme_names.len(), theme::DARK_PRESETS.len());
    }

    #[test]
    fn chinese_welcome_examples_set_due_dates() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
        let mut examples = ZH_CN.welcome_tasks[1].1.lines().filter_map(|line| {
            line.strip_prefix("- **")?.split_once("**").map(|(input, _)| input)
        });

        let tomorrow = fern_core::quick_add::parse_quick_add(examples.next().unwrap(), today, &[]);
        assert_eq!(tomorrow.title, "联系 Sam");
        assert_eq!(tomorrow.due_date, today.succ_opt());
        assert_eq!(tomorrow.due_time, chrono::NaiveTime::from_hms_opt(18, 0, 0));

        let friday = fern_core::quick_add::parse_quick_add(examples.next().unwrap(), today, &[]);
        assert_eq!(friday.title, "交房租");
        assert_eq!(friday.due_date, chrono::NaiveDate::from_ymd_opt(2026, 11, 13));
        assert_eq!(friday.tags, ["居家"]);
        assert_eq!(friday.priority, Some(fern_core::Priority::High));
    }

    #[test]
    fn recovery_sync_and_diagnostic_placeholders_exist_in_all_locales() {
        for locale in [Locale::En, Locale::Ru, Locale::ZhCn] {
            let s = locale.strings();
            assert!(s.sync_incompatible_protocol.contains("{client}"), "{locale:?}");
            assert!(s.sync_incompatible_protocol.contains("{server}"), "{locale:?}");
            assert!(s.restore_backup_confirm_body.contains("{file}"), "{locale:?}");
            assert!(s.db_corrupt_recovered.contains("{file}"), "{locale:?}");
            assert!(s.diagnostic_log_saved.contains("{path}"), "{locale:?}");
            assert!(s.diagnostic_log_failed.contains("{error}"), "{locale:?}");
            assert!(s.plugins_metrics.contains("{mem}"), "{locale:?}");
            assert!(s.plugins_metrics.contains("{ms}"), "{locale:?}");
        }
    }

    #[test]
    fn format_life_clock_age_english() {
        assert_eq!(
            format_life_clock_age(Locale::En, 28, 0, 3, 3, 53, 16),
            "28 years, 0 months, 3 days, 3 hours, 53 minutes and 16 seconds old",
        );
        assert_eq!(
            format_life_clock_age(Locale::En, 1, 1, 1, 1, 1, 1),
            "1 year, 1 month, 1 day, 1 hour, 1 minute and 1 second old",
        );
        assert_eq!(
            format_life_clock_age(Locale::En, 0, 1, 2, 11, 21, 59),
            "0 years, 1 month, 2 days, 11 hours, 21 minutes and 59 seconds old",
        );
    }

    #[test]
    fn format_life_clock_age_russian() {
        assert_eq!(
            format_life_clock_age(Locale::Ru, 28, 0, 3, 3, 53, 16),
            "28 лет, 0 месяцев, 3 дня, 3 часа, 53 минуты и 16 секунд",
        );
        assert_eq!(
            format_life_clock_age(Locale::Ru, 1, 1, 1, 1, 1, 1),
            "1 год, 1 месяц, 1 день, 1 час, 1 минута и 1 секунда",
        );
        assert_eq!(
            format_life_clock_age(Locale::Ru, 2, 2, 2, 2, 2, 2),
            "2 года, 2 месяца, 2 дня, 2 часа, 2 минуты и 2 секунды",
        );
        assert_eq!(
            format_life_clock_age(Locale::Ru, 5, 5, 5, 5, 5, 5),
            "5 лет, 5 месяцев, 5 дней, 5 часов, 5 минут и 5 секунд",
        );
        assert_eq!(
            format_life_clock_age(Locale::Ru, 11, 11, 11, 11, 11, 11),
            "11 лет, 11 месяцев, 11 дней, 11 часов, 11 минут и 11 секунд",
        );
        assert_eq!(
            format_life_clock_age(Locale::Ru, 12, 12, 14, 13, 12, 14),
            "12 лет, 12 месяцев, 14 дней, 13 часов, 12 минут и 14 секунд",
        );
        assert_eq!(
            format_life_clock_age(Locale::Ru, 21, 1, 21, 21, 21, 21),
            "21 год, 1 месяц, 21 день, 21 час, 21 минута и 21 секунда",
        );
        assert_eq!(
            format_life_clock_age(Locale::Ru, 111, 0, 112, 113, 114, 121),
            "111 лет, 0 месяцев, 112 дней, 113 часов, 114 минут и 121 секунда",
        );
    }

    #[test]
    fn format_life_clock_age_chinese() {
        assert_eq!(
            format_life_clock_age(Locale::ZhCn, 28, 0, 3, 3, 53, 16),
            "28 岁 0 个月 3 天 3 小时 53 分钟 16 秒",
        );
        assert_eq!(
            format_life_clock_age(Locale::ZhCn, 1, 1, 1, 1, 1, 1),
            "1 岁 1 个月 1 天 1 小时 1 分钟 1 秒",
        );
    }

    #[test]
    fn horizons_strings_and_placeholders_valid_in_all_locales() {
        for locale in [Locale::En, Locale::Ru, Locale::ZhCn] {
            let s = locale.strings();
            assert!(!s.section_horizons.is_empty(), "{locale:?}");
            assert_eq!(s.feature_items[6].0, s.section_horizons, "{locale:?}");
            assert!(!s.feature_items[6].1.is_empty(), "{locale:?}");
            assert!(s.horizons_rollover_label.contains("{n}"), "{locale:?}");
            for field in [
                s.horizons_move_all_here,
                s.horizon_this_week,
                s.horizon_this_month,
                s.horizon_this_quarter,
                s.horizon_this_year,
                s.horizon_future,
                s.calendar_week_goals,
                s.calendar_month_goals,
                s.calendar_year_goals,
                s.calendar_drop_to_schedule,
            ] {
                assert!(!field.is_empty(), "{locale:?}");
            }
        }
    }
}
