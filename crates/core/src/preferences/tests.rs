use super::*;

#[test]
fn defaults_when_never_saved() -> Result<()> {
    let store = Store::open_in_memory()?;
    assert_eq!(store.preferences()?, Preferences::default());
    Ok(())
}

#[test]
fn existing_theme_mode_and_new_palettes_survive_unrelated_settings_edits() -> Result<()> {
    let store = Store::open_in_memory()?;
    store.set_setting(THEME, "light")?;
    let mut selection = store.preferences()?.theme;
    assert_eq!(selection, ThemeSelection { mode: ThemeMode::Light, ..Default::default() });
    selection.light = Some("turquoise".into());
    selection.dark = Some("darkpink".into());
    selection.mode = ThemeMode::System;
    store.save_theme_selection(&selection)?;
    store.update_preferences(|p| p.detailed_input = true)?;
    assert_eq!(store.preferences()?.theme, selection);
    Ok(())
}

#[test]
fn roundtrip() -> Result<()> {
    let store = Store::open_in_memory()?;
    let list = store.create_list("Work")?;
    let preferences =
        Preferences {
        theme: ThemeSelection { mode: ThemeMode::System, light: Some("matcha".into()), dark: Some("darkgreen".into()) },
        week_start: Weekday::Sun,
        hour12: true,
        date_format: Some(DateFormat::YearMonthDay),
        week_numbers: true,
        mini_calendar: false,
        detailed_input: true,
        countdown_dates: true,
        popup_details: true,
        features: Features { matrix: false, countdown: false, ..Features::default() },
        notifications: Notifications { daily: chrono::NaiveTime::from_hms_opt(8, 0, 0), desktop: false, constant: true },
        recognition: Recognition { remove_date_text: false, tags: false, ..Default::default() },
        task_defaults: TaskDefaults {
            date: Some(DefaultDate::Tomorrow),
            duration: 30,
            timed_reminder: None,
            all_day_reminder: Some(24 * 60),
            priority: Priority::High,
            tag: Some("work".into()),
            add_to_bottom: true,
            overdue_last: true,
        },
        default_list: Some(list.id),
        ..Default::default()
    };
    store.save_preferences(&preferences)?;
    assert_eq!(store.preferences()?, preferences);

    store.save_preferences(&Preferences { date_format: Some(DateFormat::IsoYearMonthDay), ..preferences.clone() })?;
    assert_eq!(store.preferences()?.date_format, Some(DateFormat::IsoYearMonthDay));

    store.save_preferences(&Preferences { default_list: None, ..preferences })?;
    assert_eq!(store.preferences()?.default_list, None);
    Ok(())
}

#[test]
fn deleted_default_list_falls_back_to_inbox() -> Result<()> {
    let store = Store::open_in_memory()?;
    let list = store.create_list("Work")?;
    store.save_preferences(&Preferences { default_list: Some(list.id), ..Default::default() })?;
    store.delete_list(list.id)?;
    assert_eq!(store.preferences()?.default_list, None);
    Ok(())
}

#[test]
fn smart_list_visibility() -> Result<()> {
    let store = Store::open_in_memory()?;
    let mut preferences = Preferences::default();
    preferences.set_visibility(SmartList::Tomorrow, Visibility::Hide);
    preferences.set_visibility(SmartList::Today, Visibility::IfNotEmpty);
    // The Trash is only shown or hidden.
    preferences.set_visibility(SmartList::Trash, Visibility::IfNotEmpty);
    store.save_preferences(&preferences)?;

    let loaded = store.preferences()?;
    assert_eq!(loaded.visibility(SmartList::Tomorrow), Visibility::Hide);
    assert_eq!(loaded.visibility(SmartList::Today), Visibility::IfNotEmpty);
    assert_eq!(loaded.visibility(SmartList::Trash), Visibility::Show);
    assert!(!Visibility::IfNotEmpty.shows(0));
    assert!(Visibility::IfNotEmpty.shows(2));
    assert!(!Visibility::Hide.shows(5));
    Ok(())
}

#[test]
fn default_dates() {
    let sunday = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    assert_eq!(DefaultDate::Tomorrow.date(sunday), NaiveDate::from_ymd_opt(2026, 10, 19).unwrap());
    assert_eq!(DefaultDate::NextWeek.date(sunday), NaiveDate::from_ymd_opt(2026, 10, 19).unwrap());
    let monday = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
    assert_eq!(DefaultDate::NextWeek.date(monday), NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
    assert_eq!(TaskDefaults::default().timed_reminder, Some(0));

    // Canonical next_week_date & QuickDate respect week_start.
    assert_eq!(next_week_date(sunday, Weekday::Mon), monday);
    assert_eq!(next_week_date(monday, Weekday::Mon), NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
    assert_eq!(next_week_date(sunday, Weekday::Sun), NaiveDate::from_ymd_opt(2026, 10, 25).unwrap());
    let saturday = NaiveDate::from_ymd_opt(2026, 10, 17).unwrap();
    assert_eq!(next_week_date(saturday, Weekday::Sun), sunday);
    assert_eq!(QuickDate::Today.resolve(sunday, Weekday::Mon), Some(sunday));
    assert_eq!(QuickDate::Tomorrow.resolve(sunday, Weekday::Mon), Some(monday));
    assert_eq!(QuickDate::NextWeek.resolve(sunday, Weekday::Mon), Some(monday));
    assert_eq!(QuickDate::NoDate.resolve(sunday, Weekday::Mon), None);
}

#[test]
fn unsupported_values_are_ignored() -> Result<()> {
    let store = Store::open_in_memory()?;
    store.set_setting(THEME, "neon")?;
    store.set_setting(WEEK_START, "Wed")?;
    assert_eq!(store.preferences()?, Preferences::default());
    Ok(())
}

#[test]
fn daily_notification_once_a_day() {
    let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    let at = |h| day.and_hms_opt(h, 0, 0).unwrap();
    let off = Notifications::default();
    assert!(!off.daily_due(at(9), None));
    let on = Notifications { daily: chrono::NaiveTime::from_hms_opt(8, 0, 0), ..off };
    assert!(!on.daily_due(at(7), None), "not before its time");
    assert!(on.daily_due(at(9), None));
    assert!(on.daily_due(at(9), day.pred_opt()));
    assert!(!on.daily_due(at(9), Some(day)), "once a day");
}

#[test]
fn sidebar_order_moves_and_persists() -> Result<()> {
    let store = Store::open_in_memory()?;
    let mut prefs = Preferences::default();
    assert_eq!(prefs.smart_list_order, SmartList::TOP_DRAGGABLE);
    assert_eq!(prefs.sidebar_section_order, SidebarSection::ALL);
    assert_eq!(prefs.rail_order, RailSection::ALL);

    prefs.move_smart_list(SmartList::Today, SmartList::Inbox, false);
    assert_eq!(prefs.smart_list_order[..3], [SmartList::Today, SmartList::Inbox, SmartList::All]);
    prefs.move_smart_list(SmartList::Inbox, SmartList::Tomorrow, true);
    assert_eq!(
        prefs.smart_list_order[..4],
        [SmartList::Today, SmartList::All, SmartList::Tomorrow, SmartList::Inbox]
    );

    prefs.move_sidebar_section(SidebarSection::Filters, SidebarSection::Lists, false);
    assert_eq!(prefs.sidebar_section_order, [SidebarSection::Filters, SidebarSection::Lists, SidebarSection::Tags]);

    prefs.move_rail_section(RailSection::Countdown, RailSection::Tasks, false);
    assert_eq!(prefs.rail_order[0], RailSection::Countdown);
    assert_eq!(prefs.rail_order[1], RailSection::Tasks);

    store.save_preferences(&prefs)?;
    let loaded = store.preferences()?;
    assert_eq!(loaded.smart_list_order, prefs.smart_list_order);
    assert_eq!(loaded.sidebar_section_order, prefs.sidebar_section_order);
    assert_eq!(loaded.rail_order, prefs.rail_order);

    // Unknown / duplicate entries are ignored and missing items append in default order.
    store.set_setting(SIDEBAR_SECTION_ORDER, "tags,bogus,tags")?;
    let recovered = store.preferences()?;
    assert_eq!(
        recovered.sidebar_section_order,
        [SidebarSection::Tags, SidebarSection::Lists, SidebarSection::Filters]
    );
    Ok(())
}

#[test]
fn features_task_frontends_and_rail_items_backwards_compat() -> Result<()> {
    let store = Store::open_in_memory()?;

    // Legacy storage without tasks/+things flags defaults to tasks=true, things=false.
    store.set_setting(FEATURES, "matrix,countdown")?;
    let loaded = store.preferences()?.features;
    assert!(loaded.tasks);
    assert!(!loaded.things);
    assert!(!loaded.matrix);
    assert!(!loaded.countdown);
    assert!(loaded.has_any_task_frontend());

    // Switching to Things-only persists cleanly and recovers if both are turned off.
    let mut prefs = store.preferences()?;
    prefs.features.tasks = false;
    prefs.features.things = true;
    store.save_preferences(&prefs)?;
    let reloaded = store.preferences()?.features;
    assert!(!reloaded.tasks);
    assert!(reloaded.things);
    assert!(reloaded.has_any_task_frontend());

    let mut invalid = Features { tasks: false, things: false, ..Features::default() };
    assert!(!invalid.has_any_task_frontend());
    invalid.sanitize();
    assert!(invalid.tasks);
    assert!(invalid.has_any_task_frontend());

    // RailItemId roundtrip with plugin entries and legacy rail_order compatibility.
    let plugin_item = RailItemId::Plugin {
        plugin_id: "com.example.github".into(),
        view_id: "issues".into(),
    };
    assert_eq!(RailItemId::from_code(&plugin_item.code()), Some(plugin_item.clone()));
    assert_eq!(RailItemId::from_code("things"), Some(RailItemId::BuiltIn(RailSection::Things)));

    prefs.set_rail_items(vec![
        plugin_item.clone(),
        RailItemId::BuiltIn(RailSection::Things),
        RailItemId::BuiltIn(RailSection::Tasks),
    ]);
    store.save_preferences(&prefs)?;
    let loaded_prefs = store.preferences()?;
    assert_eq!(loaded_prefs.rail_items[0], plugin_item);
    assert_eq!(loaded_prefs.rail_items[1], RailItemId::BuiltIn(RailSection::Things));
    assert_eq!(loaded_prefs.rail_order[0], RailSection::Things);
    assert!(loaded_prefs.rail_items.contains(&RailItemId::BuiltIn(RailSection::Countdown)));

    // Direct rail_order update preserves plugin entries in rail_items.
    store.update_preferences(|p| p.move_rail_section(RailSection::Calendar, RailSection::Things, false))?;
    let after_builtin_move = store.preferences()?;
    assert_eq!(after_builtin_move.rail_items[0], plugin_item);
    assert_eq!(after_builtin_move.rail_order[0], RailSection::Calendar);
    Ok(())
}

#[test]
fn update_preferences_sanitizes_features_and_rail_item_validation_is_atomic() -> Result<()> {
    let store = Store::open_in_memory()?;

    // Turning off both task frontends sanitizes the returned Preferences to match persisted DB.
    let returned = store.update_preferences(|p| {
        p.features.tasks = false;
        p.features.things = false;
    })?;
    assert!(returned.features.tasks);
    assert!(!returned.features.things);
    assert_eq!(returned.features, store.preferences()?.features);

    // Commas and slashes in plugin rail IDs are rejected so CSV serialization cannot be corrupted.
    assert!(RailItemId::from_code("plugin:com.example.a/bad,view").is_none());
    assert!(RailItemId::from_code("plugin:com.example,a/view").is_none());
    assert!(RailItemId::from_code("plugin:com.example.a/a/b").is_none());

    // move_rail_item does not partially mutate when target is absent or an ID is invalid.
    let mut prefs = Preferences::default();
    let before = prefs.rail_items.clone();
    let new_plugin = RailItemId::Plugin { plugin_id: "com.example.p".into(), view_id: "v".into() };
    let missing_target = RailItemId::Plugin { plugin_id: "com.example.missing".into(), view_id: "x".into() };
    prefs.move_rail_item(&new_plugin, &missing_target, false);
    assert_eq!(prefs.rail_items, before, "missing target must leave rail_items untouched");

    let comma_plugin = RailItemId::Plugin { plugin_id: "com.example.p".into(), view_id: "bad,id".into() };
    prefs.move_rail_item(&comma_plugin, &RailItemId::BuiltIn(RailSection::Tasks), false);
    assert_eq!(prefs.rail_items, before, "invalid comma ID must be rejected");
    Ok(())
}

#[test]
fn life_clock_feature_and_rail_section_roundtrip_and_compat() -> Result<()> {
    let store = Store::open_in_memory()?;

    // Default placement: right after Countdown and before Things.
    let countdown_idx = RailSection::ALL.iter().position(|s| *s == RailSection::Countdown);
    let life_clock_idx = RailSection::ALL.iter().position(|s| *s == RailSection::LifeClock);
    let things_idx = RailSection::ALL.iter().position(|s| *s == RailSection::Things);
    assert_eq!(life_clock_idx, countdown_idx.map(|i| i + 1));
    assert_eq!(things_idx, life_clock_idx.map(|i| i + 1));
    assert_eq!(RailSection::LifeClock.code(), "life_clock");
    assert_eq!(RailSection::from_code("life_clock"), Some(RailSection::LifeClock));
    assert_eq!(
        RailItemId::from_code("life_clock"),
        Some(RailItemId::BuiltIn(RailSection::LifeClock))
    );

    // Legacy storage without `life_clock` token defaults `features.life_clock` to `true`,
    // and legacy rail_order without `life_clock` appends it during normalization.
    store.set_setting(FEATURES, "matrix,countdown")?;
    store.set_setting(RAIL_ORDER, "tasks,calendar,focus,habits,search,matrix,countdown,things")?;
    let legacy = store.preferences()?;
    assert!(legacy.features.life_clock);
    assert!(!legacy.features.matrix);
    assert!(!legacy.features.countdown);
    assert!(legacy.rail_order.contains(&RailSection::LifeClock));
    assert!(legacy.rail_items.contains(&RailItemId::BuiltIn(RailSection::LifeClock)));

    // Disabling `features.life_clock` persists cleanly across reload.
    store.update_preferences(|p| {
        p.features.life_clock = false;
        p.move_rail_section(RailSection::LifeClock, RailSection::Tasks, false);
    })?;
    let disabled = store.preferences()?;
    assert!(!disabled.features.life_clock);
    assert_eq!(disabled.rail_order[0], RailSection::LifeClock);
    assert_eq!(disabled.rail_items[0], RailItemId::BuiltIn(RailSection::LifeClock));

    // Re-enabling persists back to true.
    store.update_preferences(|p| p.features.life_clock = true)?;
    assert!(store.preferences()?.features.life_clock);
    Ok(())
}

#[test]
fn horizons_feature_and_smart_list_roundtrip_and_compat() -> Result<()> {
    let store = Store::open_in_memory()?;

    // SmartList::Horizons sits right before SmartList::Summary in both ALL and TOP_DRAGGABLE.
    let all_h = SmartList::ALL.iter().position(|s| *s == SmartList::Horizons);
    let all_s = SmartList::ALL.iter().position(|s| *s == SmartList::Summary);
    assert_eq!(all_s, all_h.map(|i| i + 1));
    let top_h = SmartList::TOP_DRAGGABLE.iter().position(|s| *s == SmartList::Horizons);
    let top_s = SmartList::TOP_DRAGGABLE.iter().position(|s| *s == SmartList::Summary);
    assert_eq!(top_s, top_h.map(|i| i + 1));
    assert_eq!(SmartList::Horizons.code(), "horizons");
    assert_eq!(SmartList::from_code("horizons"), Some(SmartList::Horizons));
    assert_eq!(SmartList::Horizons.options(), &[Visibility::Show, Visibility::Hide]);
    // No duplicate RailSection::Horizons in the navigation rail.
    assert_eq!(RailSection::from_code("horizons"), None);

    // Legacy storage without `horizons` token defaults `features.horizons` to `true`,
    // and legacy smart_list_order without `horizons` appends it during normalization.
    store.set_setting(FEATURES, "matrix,countdown")?;
    store.set_setting(SMART_LIST_ORDER, "inbox,all,today,tomorrow,next_7_days,summary")?;
    let legacy = store.preferences()?;
    assert!(legacy.features.horizons);
    assert_eq!(legacy.visibility(SmartList::Horizons), Visibility::Show);
    assert!(legacy.smart_list_order.contains(&SmartList::Horizons));

    // Disabling `features.horizons`, hiding `SmartList::Horizons`, and reordering persist cleanly.
    store.update_preferences(|p| {
        p.features.horizons = false;
        p.set_visibility(SmartList::Horizons, Visibility::Hide);
        p.move_smart_list(SmartList::Horizons, SmartList::Inbox, false);
    })?;
    let disabled = store.preferences()?;
    assert!(!disabled.features.horizons);
    assert_eq!(disabled.visibility(SmartList::Horizons), Visibility::Hide);
    assert_eq!(disabled.smart_list_order[0], SmartList::Horizons);

    // `IfNotEmpty` is ignored for SmartList::Horizons (only Show/Hide offered).
    store.update_preferences(|p| {
        p.features.horizons = true;
        p.set_visibility(SmartList::Horizons, Visibility::Show);
        p.set_visibility(SmartList::Horizons, Visibility::IfNotEmpty);
    })?;
    let reloaded = store.preferences()?;
    assert!(reloaded.features.horizons);
    assert_eq!(reloaded.visibility(SmartList::Horizons), Visibility::Show);
    Ok(())
}
