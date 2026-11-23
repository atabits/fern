use std::rc::Rc;

use chrono::NaiveDate;
use fern_core::{CountdownKind, DateFormat, Frequency, Store};
use gpui::{Focusable as _, Modifiers, TestAppContext, size};
use ui::prelude::*;

use desktop_common::calendar;
use desktop_common::data_version::{DataVersion, changed_elsewhere};
use desktop_common::keymap;
use fern_i18n as i18n;

use super::CountdownView;
use super::modal::DateField;

#[gpui::test]
fn save_click_keeps_invalid_dates_until_corrected_for_every_countdown_kind(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        calendar::set_date_format(Some(DateFormat::IsoYearMonthDay));
        assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|_, cx| CountdownView::new(store.clone(), cx));
    cx.simulate_resize(size(px(900.), px(700.)));
    let select_all = if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" };
    for (index, kind) in [
        CountdownKind::Countdown,
        CountdownKind::Anniversary,
        CountdownKind::Birthday,
        CountdownKind::Holiday,
    ]
    .into_iter()
    .enumerate()
    {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.start_new_kind(kind, window, cx);
                let draft = view.draft.as_mut().unwrap();
                draft.name.update(cx, |input, cx| input.set_text("Date validation", cx));
                draft.set_date(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap(), cx);
            })
        });
        if matches!(kind, CountdownKind::Birthday | CountdownKind::Holiday) {
            let trigger = cx.debug_bounds("countdown-date-popup-trigger").unwrap().center();
            cx.simulate_click(trigger, Modifiers::default());
        }
        let month = cx.debug_bounds(DateField::Month.id()).unwrap().center();
        cx.simulate_click(month, Modifiers::default());
        cx.simulate_keystrokes(select_all);
        cx.simulate_input("13");
        let save = cx.debug_bounds("countdown-save").unwrap().center();
        cx.simulate_click(save, Modifiers::default());
        assert_eq!(
            store.countdowns().unwrap().len(),
            index,
            "Save must reject the invalid {kind:?} date"
        );
        cx.update(|_, cx| {
            let draft = view.read(cx).draft.as_ref().unwrap();
            assert_eq!(
                draft.date_inputs.fields[DateField::Month as usize].read(cx).text(),
                "13"
            );
        });

        let day = cx.debug_bounds(DateField::Day.id()).unwrap().center();
        cx.simulate_click(day, Modifiers::default());
        cx.simulate_keystrokes("enter");
        cx.simulate_click(save, Modifiers::default());
        assert_eq!(
            store.countdowns().unwrap().len(),
            index,
            "blurring another field must not erase the invalid month"
        );

        let month = cx.debug_bounds(DateField::Month.id()).unwrap().center();
        cx.simulate_click(month, Modifiers::default());
        cx.simulate_keystrokes(select_all);
        cx.simulate_input("12");
        cx.simulate_click(save, Modifiers::default());
        let countdowns = store.countdowns().unwrap();
        assert_eq!(countdowns.len(), index + 1);
        assert_eq!(
            countdowns.last().unwrap().date,
            NaiveDate::from_ymd_opt(2026, 12, 16).unwrap()
        );
    }
    cx.update(|_, _| calendar::set_date_format(None));
}

#[gpui::test]
fn countdown_kinds_share_the_ordered_date_editor(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::ZhCn));
        calendar::set_date_format(None);
        assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|_, cx| CountdownView::new(store, cx));
    cx.simulate_resize(size(px(900.), px(600.)));

    for kind in [
        CountdownKind::Countdown,
        CountdownKind::Anniversary,
        CountdownKind::Birthday,
        CountdownKind::Holiday,
    ] {
        cx.update(|window, cx| view.update(cx, |view, cx| view.start_new_kind(kind, window, cx)));
        if matches!(kind, CountdownKind::Birthday | CountdownKind::Holiday) {
            assert!(cx.debug_bounds(DateField::Year.id()).is_none());
            let trigger = cx.debug_bounds("countdown-date-popup-trigger").unwrap().center();
            cx.simulate_click(trigger, Modifiers::default());
        }
        let year = cx.debug_bounds(DateField::Year.id()).unwrap();
        let month = cx.debug_bounds(DateField::Month.id()).unwrap();
        let day = cx.debug_bounds(DateField::Day.id()).unwrap();
        assert!(year.right() <= month.left() && month.right() <= day.left());
    }

    cx.update(|window, cx| {
        calendar::set_date_format(Some(DateFormat::DayMonthYear));
        view.update(cx, |view, cx| view.start_new_kind(CountdownKind::Anniversary, window, cx));
    });
    let year = cx.debug_bounds(DateField::Year.id()).unwrap();
    let month = cx.debug_bounds(DateField::Month.id()).unwrap();
    let day = cx.debug_bounds(DateField::Day.id()).unwrap();
    assert!(day.right() <= month.left() && month.right() <= year.left());
    cx.update(|_, _| calendar::set_date_format(None));
}

#[gpui::test]
fn birthday_accepts_a_distant_year_and_keeps_leap_day_valid(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        calendar::set_date_format(Some(DateFormat::IsoYearMonthDay));
        assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = CountdownView::new(store.clone(), cx);
        view.start_new_kind(CountdownKind::Birthday, window, cx);
        let draft = view.draft.as_mut().unwrap();
        draft.name.update(cx, |input, cx| input.set_text("Birthday", cx));
        draft.set_date(NaiveDate::from_ymd_opt(2028, 2, 29).unwrap(), cx);
        view
    });
    cx.simulate_resize(size(px(900.), px(600.)));
    let trigger = cx.debug_bounds("countdown-date-popup-trigger").unwrap().center();
    cx.simulate_click(trigger, Modifiers::default());
    let year_dropdown = cx.debug_bounds("countdown-date-year-toggle").unwrap().center();
    cx.simulate_click(year_dropdown, Modifiers::default());
    assert!(cx.debug_bounds("countdown-date-options").is_some());

    let select_all = if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" };
    cx.simulate_keystrokes(select_all);
    cx.simulate_input("1910");
    cx.simulate_keystrokes("enter");
    assert!(cx.debug_bounds("countdown-date-options").is_none());
    cx.update(|window, cx| {
        let draft = view.read(cx).draft.as_ref().unwrap();
        assert_eq!(draft.date, NaiveDate::from_ymd_opt(1910, 2, 28).unwrap());
        assert!(
            draft.date_inputs.fields[DateField::Month as usize]
                .focus_handle(cx)
                .is_focused(window)
        );
    });

    cx.simulate_keystrokes(select_all);
    cx.simulate_input("13");
    cx.simulate_keystrokes("enter");
    cx.update(|_, cx| view.update(cx, |view, cx| view.save_draft(cx)));
    assert!(store.countdowns().unwrap().is_empty());
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds(DateField::Year.id()).is_none());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| view.save_draft(cx));
        calendar::set_date_format(None);
    });
    let countdowns = store.countdowns().unwrap();
    assert_eq!(countdowns.len(), 1);
    assert_eq!(countdowns[0].date, NaiveDate::from_ymd_opt(1910, 2, 28).unwrap());
    assert_eq!(countdowns[0].repeat.as_ref().unwrap().frequency, Frequency::Yearly);
}

#[gpui::test]
fn countdown_mutations_emit_data_changed_and_empty_name_preserves_draft(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|_, cx| CountdownView::new(store.clone(), cx));
    let bumps = Rc::new(std::cell::Cell::new(0usize));
    let bumps_obs = bumps.clone();
    let _observer = cx.update(|_, cx| {
        cx.new(|cx| {
            cx.observe_global::<DataVersion>(move |_: &mut (), cx| {
                if changed_elsewhere(cx.entity_id(), cx) {
                    bumps_obs.set(bumps_obs.get() + 1);
                }
            })
            .detach();
        })
    });

    // 1. Saving with an empty/whitespace name must preserve the draft rather than dropping it.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.start_new_kind(CountdownKind::Anniversary, window, cx);
            let draft = v.draft.as_mut().unwrap();
            draft.name.update(cx, |inp, cx| inp.set_text("   ", cx));
            v.save_draft(cx);
            assert!(v.draft.is_some(), "empty name must not destroy draft");
            assert!(v.countdowns.is_empty());
        });
    });
    assert_eq!(bumps.get(), 0);

    // 2. Create emits data_changed.
    cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            v.draft
                .as_ref()
                .unwrap()
                .name
                .update(cx, |inp, cx| inp.set_text("Launch Day", cx));
            v.save_draft(cx);
            assert!(v.draft.is_none());
        });
    });
    assert_eq!(bumps.get(), 1, "create must emit data_changed");
    let id = store.countdowns().unwrap()[0].id;

    // 3. Edit emits data_changed.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.start_edit(id, window, cx);
            v.draft
                .as_ref()
                .unwrap()
                .name
                .update(cx, |inp, cx| inp.set_text("Launch Day v2", cx));
            v.save_draft(cx);
        });
    });
    assert_eq!(bumps.get(), 2, "edit must emit data_changed");

    // 4. Pin emits data_changed.
    cx.update(|_, cx| view.update(cx, |v, cx| v.toggle_pin(id, false, cx)));
    assert_eq!(bumps.get(), 3, "pin must emit data_changed");

    // 5. Archive emits data_changed.
    cx.update(|_, cx| view.update(cx, |v, cx| v.toggle_archive(id, false, cx)));
    assert_eq!(bumps.get(), 4, "archive must emit data_changed");

    // 6. Delete emits data_changed.
    cx.update(|_, cx| view.update(cx, |v, cx| v.delete(id, cx)));
    assert_eq!(bumps.get(), 5, "delete must emit data_changed");
    assert!(store.countdowns().unwrap().is_empty());
}

#[gpui::test]
fn new_inactive_skips_initial_reload_until_activated(cx: &mut TestAppContext) {
    use gpui::AppContext;
    let store = Rc::new(Store::open_in_memory().unwrap());
    let target = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
    store
        .add_countdown("Year End", target, CountdownKind::Countdown)
        .unwrap();

    cx.update(|cx| {
        let view = cx.new(|cx| CountdownView::new_inactive(store.clone(), cx));
        {
            let v = view.read(cx);
            assert!(!v.active);
            assert!(v.dirty);
            assert!(v.countdowns.is_empty());
        }

        view.update(cx, |v, cx| v.set_active(true, cx));
        let v = view.read(cx);
        assert!(v.active);
        assert!(!v.dirty);
        assert_eq!(v.countdowns.len(), 1);
        assert_eq!(v.countdowns[0].name, "Year End");
    });
}

#[gpui::test]
fn custom_reminder_escape_closes_form_and_confirm_adds_weeks_reminder(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|_, cx| CountdownView::new(store.clone(), cx));
    cx.simulate_resize(size(px(900.), px(700.)));

    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.start_new_kind(CountdownKind::Countdown, window, cx);
            v.draft
                .as_ref()
                .unwrap()
                .name
                .update(cx, |inp, cx| inp.set_text("Trip", cx));
            v.dev_open_row("reminder", cx);
            v.open_custom_reminder(window, cx);
        });
    });
    cx.run_until_parked();

    // Escape in custom reminder input dismisses only the custom form, keeping the modal open.
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        let v = view.read(cx);
        let draft = v.draft.as_ref().expect("draft must remain open");
        assert!(draft.custom.is_none());
    });

    // Re-open custom reminder, enter 2 weeks, confirm, and save.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.open_custom_reminder(window, cx);
            let custom = v.draft.as_mut().unwrap().custom.as_mut().unwrap();
            custom.weeks = true;
            custom.count.update(cx, |inp, cx| inp.set_text("2", cx));
            v.confirm_custom_reminder(cx);
            v.save_draft(cx);
        });
    });

    let countdowns = store.countdowns().unwrap();
    assert_eq!(countdowns.len(), 1);
    assert_eq!(countdowns[0].custom_reminders.len(), 1);
    assert_eq!(countdowns[0].custom_reminders[0].0, 14);
}

#[gpui::test]
fn escape_from_custom_reminder_restores_name_focus_and_escape_from_name_restores_view_focus(
    cx: &mut TestAppContext,
) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|_, cx| CountdownView::new(store, cx));
    cx.simulate_resize(size(px(900.), px(700.)));

    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.start_new_kind(CountdownKind::Countdown, window, cx);
            v.dev_open_row("reminder", cx);
            v.open_custom_reminder(window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let count = &view.read(cx).draft.as_ref().unwrap().custom.as_ref().unwrap().count;
        assert!(count.focus_handle(cx).is_focused(window));
    });

    // Escape in custom reminder closes it and restores focus to draft.name.
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        let draft = view.read(cx).draft.as_ref().expect("draft must remain open");
        assert!(draft.custom.is_none());
        assert!(draft.open.is_some());
        assert!(draft.name.focus_handle(cx).is_focused(window));
    });

    // Escape from draft.name dismisses the reminder popover first, keeping focus on draft.name.
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        let draft = view.read(cx).draft.as_ref().expect("draft must remain open");
        assert!(draft.open.is_none());
        assert!(draft.name.focus_handle(cx).is_focused(window));
    });

    // Next Escape from draft.name closes the modal and restores focus to CountdownView.
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert!(v.draft.is_none());
        assert!(v.focus_handle(cx).is_focused(window));
    });

    // Re-open draft and close via dismiss_transient(cx): render consumes pending_focus for CountdownView.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.start_new(window, cx);
            assert!(v.dismiss_transient(cx));
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert!(v.draft.is_none());
        assert!(v.focus_handle(cx).is_focused(window));
    });
}

#[gpui::test]
fn russian_countdown_modal_preserves_input_column_and_saves_every_kind(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::Ru));
        calendar::set_date_format(Some(DateFormat::DayMonthYear));
        assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|_, cx| CountdownView::new(store.clone(), cx));
    cx.simulate_resize(size(px(900.), px(700.)));

    for kind in [
        CountdownKind::Countdown,
        CountdownKind::Anniversary,
        CountdownKind::Birthday,
        CountdownKind::Holiday,
    ] {
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.start_new_kind(kind, window, cx);
                let draft = v.draft.as_mut().unwrap();
                draft.name.update(cx, |inp, cx| inp.set_text("Событие", cx));
            });
        });
        if matches!(kind, CountdownKind::Countdown | CountdownKind::Anniversary) {
            let day = cx.debug_bounds(DateField::Day.id()).unwrap();
            let year = cx.debug_bounds(DateField::Year.id()).unwrap();
            assert!(
                year.right() - day.left() >= px(300.),
                "widening label column must preserve the 308px input column for {kind:?}"
            );
        } else {
            assert!(cx.debug_bounds("countdown-date-popup-trigger").is_some());
        }
        let save = cx.debug_bounds("countdown-save").unwrap().center();
        cx.simulate_click(save, Modifiers::default());
        assert!(view.read_with(cx, |v, _| v.draft.is_none()));
    }

    assert_eq!(store.countdowns().unwrap().len(), 4);
    cx.update(|_, _| calendar::set_date_format(None));
}
