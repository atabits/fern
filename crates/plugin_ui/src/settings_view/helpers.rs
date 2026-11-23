use std::time::Duration;

use chrono::{DateTime, Local, TimeZone as _};
use fern_core::Store;
use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, px};
use plugins::manifest::{SettingKind, SettingSpec};
use plugins::{LogEntry, LogLevel, RegistrySource};
use serde_json::Value as JsonValue;
use ui::prelude::*;
use ui::{Color, Label, LabelSize};

pub(super) const SAFE_MODE_SETTING_KEY: &str = "plugins_safe_mode";
pub(super) const REGISTRIES_SETTING_KEY: &str = "plugin_registries";
pub(super) const SETTING_SAVE_DEBOUNCE: Duration = Duration::from_millis(350);

pub(super) fn setting_row_shell(spec: &SettingSpec, control: AnyElement) -> AnyElement {
    h_flex()
        .w_full()
        .gap_3()
        .items_center()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(Label::new(spec.label.clone()).size(LabelSize::Small))
                .when_some(spec.description.as_deref(), |col, desc| {
                    col.child(
                        Label::new(desc.to_owned())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                }),
        )
        .child(div().w(px(240.)).flex_none().child(control))
        .into_any_element()
}

pub(super) fn render_log_entries_box(
    logs: &[LogEntry],
    empty_label: &'static str,
    colors: &theme::ThemeColors,
    status: &theme::StatusColors,
) -> AnyElement {
    div()
        .w_full()
        .p_2p5()
        .rounded(px(6.))
        .bg(colors.background)
        .border_1()
        .border_color(colors.border)
        .child(if logs.is_empty() {
            Label::new(empty_label)
                .size(LabelSize::XSmall)
                .color(Color::Muted)
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .gap_1()
                .children(logs.iter().rev().take(60).rev().map(|entry| {
                    let (lvl_text, lvl_color) = match entry.level {
                        LogLevel::Debug => ("DBG", colors.text_muted),
                        LogLevel::Info => ("INF", status.info),
                        LogLevel::Warn => ("WRN", status.warning),
                        LogLevel::Error => ("ERR", status.error),
                    };
                    let ts = format_timestamp(entry.timestamp_ms);
                    h_flex()
                        .w_full()
                        .items_start()
                        .gap_2()
                        .child(
                            Label::new(ts)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new(lvl_text)
                                .size(LabelSize::XSmall)
                                .color(Color::Custom(lvl_color))
                                .weight(FontWeight::SEMIBOLD),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(LabelSize::XSmall.pixels())
                                .text_color(colors.text)
                                .child(entry.message.clone()),
                        )
                }))
                .into_any_element()
        })
        .into_any_element()
}

pub(super) fn load_registries_from_store(store: &Store) -> Vec<RegistrySource> {
    let Ok(Some(raw)) = store.setting(REGISTRIES_SETTING_KEY) else {
        return vec![RegistrySource::default()];
    };
    let parsed: Vec<RegistrySource> = serde_json::from_str(&raw).unwrap_or_default();
    if parsed.is_empty() {
        vec![RegistrySource::default()]
    } else {
        parsed
    }
}

pub(super) fn parse_setting_input_value(
    kind: SettingKind,
    raw: &str,
    min: Option<f64>,
    max: Option<f64>,
) -> Option<JsonValue> {
    match kind {
        SettingKind::Text => Some(JsonValue::String(raw.to_owned())),
        SettingKind::Number => {
            let parsed = raw.trim().parse::<f64>().ok()?;
            if !parsed.is_finite() {
                return None;
            }
            let clamped = clamp_number(parsed, min, max);
            serde_json::Number::from_f64(clamped).map(JsonValue::Number)
        }
        SettingKind::Boolean | SettingKind::Select | SettingKind::Secret => None,
    }
}

pub(super) fn clamp_number(val: f64, min: Option<f64>, max: Option<f64>) -> f64 {
    let mut out = val;
    if let Some(lo) = min {
        out = out.max(lo);
    }
    if let Some(hi) = max {
        out = out.min(hi);
    }
    out
}

pub(super) fn format_number_clean(val: f64) -> String {
    if val.fract().abs() < 1e-9 {
        format!("{}", val.round() as i64)
    } else {
        format!("{val}")
    }
}

pub(super) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

pub(super) fn format_timestamp(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%H:%M:%S").to_string(),
        None => {
            let dt: DateTime<Local> = Local::now();
            dt.format("%H:%M:%S").to_string()
        }
    }
}

pub(super) fn trigger_kind_label(kind: &str, strings: &i18n::Strings) -> SharedString {
    match kind {
        "manual" => strings.plugin_trigger_manual.into(),
        "command" => strings.plugin_trigger_command.into(),
        "event" => strings.plugin_trigger_event.into(),
        "interval" => strings.plugin_trigger_interval.into(),
        other => other.to_owned().into(),
    }
}
