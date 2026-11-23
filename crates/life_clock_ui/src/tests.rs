use super::*;
use gpui::{Modifiers, TestAppContext, point, size};

fn setup_app(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        assert!(
            desktop_common::keymap::load_default(&assets::Assets, cx)
                .expect("load default keymap")
                .is_empty()
        );
    });
}

#[test]
fn birth_date_part_ordering_follows_effective_date_format() {
    calendar::set_date_format(Some(DateFormat::DayMonthYear));
    assert_eq!(
        BirthDatePart::ordered(&fern_i18n::EN),
        [BirthDatePart::Day, BirthDatePart::Month, BirthDatePart::Year]
    );
    calendar::set_date_format(Some(DateFormat::MonthDayYear));
    assert_eq!(
        BirthDatePart::ordered(&fern_i18n::EN),
        [BirthDatePart::Month, BirthDatePart::Day, BirthDatePart::Year]
    );
    calendar::set_date_format(Some(DateFormat::IsoYearMonthDay));
    assert_eq!(
        BirthDatePart::ordered(&fern_i18n::EN),
        [BirthDatePart::Year, BirthDatePart::Month, BirthDatePart::Day]
    );
    calendar::set_date_format(None);
}

#[gpui::test]
fn life_clock_renders_and_cycles_quotes_without_hanging(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let (view, cx) = cx.add_window_view(|_, cx| LifeClockView::new(store.clone(), cx));
    cx.simulate_resize(size(px(1200.), px(760.)));
    cx.run_until_parked();

    assert!(cx.debug_bounds("life-clock-age").is_some());
    assert!(cx.debug_bounds("life-clock-quote").is_some());
    assert!(cx.debug_bounds("life-clock-sidebar-toggle").is_some());
    assert!(cx.debug_bounds("life-clock-help-button").is_some());
    assert!(cx.debug_bounds("life-clock-popover").is_none());

    let initial_index = view.read_with(cx, |v, _| v.quote_index);
    let quote_pos = cx
        .debug_bounds("life-clock-quote")
        .expect("quote bounds")
        .center();
    cx.simulate_click(quote_pos, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.quote_index),
        (initial_index + 1) % quotes::QUOTE_COUNT
    );
}

#[gpui::test]
fn life_clock_popover_open_dismiss_and_sidebar_event(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let (view, cx) = cx.add_window_view(|_, cx| LifeClockView::new(store.clone(), cx));
    let events = Rc::new(RefCell::new(Vec::<LifeClockEvent>::new()));
    let events_sub = events.clone();
    let _sub = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &LifeClockEvent, _| {
            events_sub.borrow_mut().push(*event);
        })
    });

    cx.simulate_resize(size(px(1200.), px(760.)));
    cx.run_until_parked();

    // Click sidebar toggle emits ToggleSidebar.
    let toggle_pos = cx
        .debug_bounds("life-clock-sidebar-toggle")
        .expect("sidebar toggle bounds")
        .center();
    cx.simulate_click(toggle_pos, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(*events.borrow(), vec![LifeClockEvent::ToggleSidebar]);

    // set_sidebar_hidden updates internal state cleanly.
    cx.update(|_, cx| view.update(cx, |v, cx| v.set_sidebar_hidden(true, cx)));
    assert!(view.read_with(cx, |v, _| v.sidebar_hidden));

    // Clicking `?` opens popover; clicking `?` again closes it.
    let help_pos = cx
        .debug_bounds("life-clock-help-button")
        .expect("help button bounds")
        .center();
    cx.simulate_click(help_pos, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("life-clock-popover").is_some());

    cx.simulate_click(help_pos, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("life-clock-popover").is_none());

    // Clicking the age readout opens popover, and dismiss_transient closes it.
    let age_pos = cx
        .debug_bounds("life-clock-age")
        .expect("age bounds")
        .center();
    cx.simulate_click(age_pos, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("life-clock-popover").is_some());

    let dismissed = cx.update(|_, cx| view.update(cx, |v, cx| v.dismiss_transient(cx)));
    cx.run_until_parked();
    assert!(dismissed);
    assert!(cx.debug_bounds("life-clock-popover").is_none());
    let dismissed_again = cx.update(|_, cx| view.update(cx, |v, cx| v.dismiss_transient(cx)));
    assert!(!dismissed_again);
}

#[gpui::test]
fn life_clock_birth_date_save_validate_clear_and_reload(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let (view, cx) = cx.add_window_view(|_, cx| LifeClockView::new(store.clone(), cx));
    let version_bumps = Rc::new(Cell::new(0usize));
    let bumps_obs = version_bumps.clone();
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

    cx.simulate_resize(size(px(1200.), px(760.)));
    cx.run_until_parked();

    // Open popover and type a valid birth date: 20 / 11 / 1998.
    let help_pos = cx
        .debug_bounds("life-clock-help-button")
        .expect("help button bounds")
        .center();
    cx.simulate_click(help_pos, Modifiers::default());
    cx.run_until_parked();

    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.day_input
                .update(cx, |inp, cx| inp.edit("20".into(), 2..2, window, cx));
            v.month_input
                .update(cx, |inp, cx| inp.edit("11".into(), 2..2, window, cx));
            v.year_input
                .update(cx, |inp, cx| inp.edit("1998".into(), 4..4, window, cx));
        });
    });
    cx.run_until_parked();

    let expected = NaiveDate::from_ymd_opt(1998, 11, 20).expect("valid date");
    assert_eq!(view.read_with(cx, |v, _| v.birth_date), Some(expected));
    assert!(!view.read_with(cx, |v, _| v.invalid_date));
    assert_eq!(
        store.setting(SETTING_BIRTH_DATE).expect("read setting").as_deref(),
        Some("1998-11-20")
    );
    assert!(version_bumps.get() >= 1);

    // Invalid future date flags error and preserves previously stored valid date.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.year_input
                .update(cx, |inp, cx| inp.edit("2099".into(), 4..4, window, cx));
        });
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.invalid_date));
    assert!(cx.debug_bounds("life-clock-invalid-date").is_some());
    assert_eq!(
        store.setting(SETTING_BIRTH_DATE).expect("read setting").as_deref(),
        Some("1998-11-20")
    );

    // Non-leap Feb 29 also rejected.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.day_input
                .update(cx, |inp, cx| inp.edit("29".into(), 2..2, window, cx));
            v.month_input
                .update(cx, |inp, cx| inp.edit("02".into(), 2..2, window, cx));
            v.year_input
                .update(cx, |inp, cx| inp.edit("2025".into(), 4..4, window, cx));
        });
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.invalid_date));

    // Reloading into a fresh view reads the saved birth date cleanly.
    let reloaded_birth = cx.update(|_, cx| {
        let fresh = cx.new(|cx| LifeClockView::new(store.clone(), cx));
        fresh.read(cx).birth_date
    });
    assert_eq!(reloaded_birth, Some(expected));

    // Clicking Clear removes birth date from store and clears error state.
    let clear_pos = cx
        .debug_bounds("life-clock-clear-birth")
        .expect("clear button bounds")
        .center();
    cx.simulate_click(clear_pos, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.birth_date), None);
    assert!(!view.read_with(cx, |v, _| v.invalid_date));
    assert_eq!(store.setting(SETTING_BIRTH_DATE).expect("read setting"), None);
}

#[gpui::test]
fn life_clock_palette_toggle_persists_and_emits_events(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let (view, cx) = cx.add_window_view(|_, cx| LifeClockView::new(store.clone(), cx));
    let events = Rc::new(RefCell::new(Vec::<LifeClockEvent>::new()));
    let events_sub = events.clone();
    let _sub = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &LifeClockEvent, _| {
            events_sub.borrow_mut().push(*event);
        })
    });

    cx.simulate_resize(size(px(1200.), px(760.)));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.palette_mode()), LifeClockPaletteMode::Classic);

    let help_pos = cx
        .debug_bounds("life-clock-help-button")
        .expect("help button bounds")
        .center();
    cx.simulate_click(help_pos, Modifiers::default());
    cx.run_until_parked();

    // Click right half of segmented tabs ("Fern theme").
    let tabs = cx
        .debug_bounds("life-clock-palette-tabs")
        .expect("palette tabs bounds");
    let theme_tab = point(tabs.left() + tabs.size.width * 0.75, tabs.center().y);
    cx.simulate_click(theme_tab, Modifiers::default());
    cx.run_until_parked();

    assert_eq!(
        view.read_with(cx, |v, _| v.palette_mode()),
        LifeClockPaletteMode::FernTheme
    );
    assert_eq!(
        store.setting(SETTING_PALETTE).expect("read palette").as_deref(),
        Some("theme")
    );
    assert_eq!(
        *events.borrow(),
        vec![LifeClockEvent::PaletteChanged(LifeClockPaletteMode::FernTheme)]
    );

    // Click left half ("Classic B&W").
    let classic_tab = point(tabs.left() + tabs.size.width * 0.25, tabs.center().y);
    cx.simulate_click(classic_tab, Modifiers::default());
    cx.run_until_parked();

    assert_eq!(
        view.read_with(cx, |v, _| v.palette_mode()),
        LifeClockPaletteMode::Classic
    );
    assert_eq!(
        store.setting(SETTING_PALETTE).expect("read palette").as_deref(),
        Some("classic")
    );
    assert_eq!(
        *events.borrow(),
        vec![
            LifeClockEvent::PaletteChanged(LifeClockPaletteMode::FernTheme),
            LifeClockEvent::PaletteChanged(LifeClockPaletteMode::Classic),
        ]
    );
}

#[gpui::test]
fn life_clock_leading_zero_typing_and_escape_focus_recovery(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let (view, cx) = cx.add_window_view(|_, cx| LifeClockView::new(store.clone(), cx));
    cx.simulate_resize(size(px(1200.), px(760.)));
    cx.run_until_parked();

    // Open popover and focus day input.
    let help_pos = cx
        .debug_bounds("life-clock-help-button")
        .expect("help button bounds")
        .center();
    cx.simulate_click(help_pos, Modifiers::default());
    cx.run_until_parked();

    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.year_input
                .update(cx, |inp, cx| inp.edit("1995".into(), 4..4, window, cx));
            v.month_input
                .update(cx, |inp, cx| inp.edit("0".into(), 1..1, window, cx));
        });
    });
    cx.run_until_parked();
    assert!(
        !view.read_with(cx, |v, _| v.invalid_date),
        "single leading zero while typing month should not flash invalid_date"
    );

    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.month_input
                .update(cx, |inp, cx| inp.edit("09".into(), 2..2, window, cx));
            v.day_input
                .update(cx, |inp, cx| inp.edit("0".into(), 1..1, window, cx));
        });
    });
    cx.run_until_parked();
    assert!(
        !view.read_with(cx, |v, _| v.invalid_date),
        "single leading zero while typing day should not flash invalid_date"
    );

    // Completing "05" saves cleanly.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.day_input
                .update(cx, |inp, cx| inp.edit("05".into(), 2..2, window, cx));
        });
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.birth_date),
        NaiveDate::from_ymd_opt(1995, 9, 5)
    );
    assert!(!view.read_with(cx, |v, _| v.invalid_date));

    // Double zero "00" is rejected immediately.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.day_input
                .update(cx, |inp, cx| inp.edit("00".into(), 2..2, window, cx));
        });
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.invalid_date));

    // Focusing day_input and pressing Escape closes popover and restores focus to LifeClockView.
    cx.update(|window, cx| {
        let day_focus = view.read(cx).day_input.focus_handle(cx);
        window.focus(&day_focus, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("life-clock-popover").is_none());
    cx.update(|window, cx| {
        assert!(
            view.read(cx).focus_handle(cx).is_focused(window),
            "focus should return to LifeClockView after closing popover from date input"
        );
    });
}

#[gpui::test]
fn life_clock_header_halves_stay_strictly_aligned_to_center_seam(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let (_view, cx) = cx.add_window_view(|_, cx| LifeClockView::new(store.clone(), cx));

    for (w, h) in [(1922.0, 1140.0), (1200.0, 760.0), (900.0, 500.0)] {
        cx.simulate_resize(size(px(w), px(h)));
        cx.run_until_parked();

        let center_x = px(w * 0.5);
        let age = cx.debug_bounds("life-clock-age").expect("age bounds");
        let quote = cx.debug_bounds("life-clock-quote").expect("quote bounds");
        let toggle = cx
            .debug_bounds("life-clock-sidebar-toggle")
            .expect("sidebar toggle bounds");
        let help = cx
            .debug_bounds("life-clock-help-button")
            .expect("help button bounds");

        assert!(
            age.right() <= center_x,
            "at {w}x{h}: age block right ({:?}) crossed center seam ({:?})",
            age.right(),
            center_x
        );
        assert!(
            quote.left() >= center_x,
            "at {w}x{h}: quote block left ({:?}) crossed center seam ({:?})",
            quote.left(),
            center_x
        );
        assert!(
            toggle.right() <= age.left(),
            "at {w}x{h}: top-left sidebar toggle ({:?}) overlapped age block ({:?})",
            toggle.right(),
            age.left()
        );
        assert!(
            quote.right() <= help.left(),
            "at {w}x{h}: top-right help button ({:?}) overlapped quote block ({:?})",
            help.left(),
            quote.right()
        );
        assert_eq!(help.top(), toggle.top());
    }
}

#[test]
fn frame_pacing_intervals_scale_down_when_unfocused_or_reduced_motion() {
    assert_eq!(desired_frame_interval_ms(true, false), ACTIVE_FRAME_MS);
    assert_eq!(desired_frame_interval_ms(false, false), UNFOCUSED_FRAME_MS);
    assert_eq!(desired_frame_interval_ms(true, true), REDUCED_MOTION_FRAME_MS);
    assert_eq!(desired_frame_interval_ms(false, true), REDUCED_MOTION_FRAME_MS);
}

#[gpui::test]
fn new_inactive_preserves_palette_mode_and_defers_birth_date_until_activated(
    cx: &mut TestAppContext,
) {
    use gpui::AppContext;
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    store.set_setting(SETTING_PALETTE, PALETTE_VALUE_THEME).unwrap();
    store.set_setting(SETTING_BIRTH_DATE, "1998-11-20").unwrap();

    cx.update(|cx| {
        let view = cx.new(|cx| LifeClockView::new_inactive(store.clone(), cx));
        {
            let v = view.read(cx);
            assert!(!v.active);
            assert!(v.dirty);
            assert_eq!(v.palette_mode(), LifeClockPaletteMode::FernTheme);
            assert_eq!(v.birth_date, None);
            assert_eq!(v.year_input.read(cx).text(), "");
        }

        view.update(cx, |v, cx| v.set_active(true, cx));
        let v = view.read(cx);
        assert!(v.active);
        assert!(!v.dirty);
        assert_eq!(v.palette_mode(), LifeClockPaletteMode::FernTheme);
        assert_eq!(v.birth_date, NaiveDate::from_ymd_opt(1998, 11, 20));
        assert_eq!(v.year_input.read(cx).text(), "1998");
    });
}
