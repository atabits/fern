use std::collections::HashMap;

use desktop_common::text_input::{Changed, Mode, Submitted, TextInput};
use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px,
};
use icons::IconName;
use plugins::PluginInfo;
use plugins::manifest::{SettingKind, SettingSpec};
use serde_json::{Map as JsonMap, Value as JsonValue};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Color, Icon, IconButton, IconSize, Label, LabelSize,
    ListItem, Placement, PopoverAttachment, Switch, Tooltip, popover, v_scroll,
};

use super::helpers::{
    SETTING_SAVE_DEBOUNCE, clamp_number, format_number_clean, parse_setting_input_value,
    setting_row_shell,
};
use super::{PluginSettingsView, SecretInputEntry, SettingInputEntry};
use crate::host::PluginHost;

fn effective_setting_value<'a>(
    pending: &'a HashMap<String, JsonMap<String, JsonValue>>,
    info: &'a PluginInfo,
    key: &str,
) -> Option<&'a JsonValue> {
    pending
        .get(&info.manifest.id)
        .and_then(|map| map.get(key))
        .or_else(|| info.settings_values.get(key))
}

fn ensure_setting_input_in(
    setting_inputs: &mut HashMap<(String, String), SettingInputEntry>,
    plugin_id: &str,
    spec: &SettingSpec,
    current_text: &str,
    cx: &mut Context<PluginSettingsView>,
) -> Entity<TextInput> {
    let map_key = (plugin_id.to_owned(), spec.key.clone());
    if let Some(entry) = setting_inputs.get_mut(&map_key) {
        if entry.last_synced != current_text && !entry.input.read(cx).text().eq(current_text) {
            // Only update external value when no active edit differs from last_synced.
            if entry.input.read(cx).text() == entry.last_synced {
                let next = current_text.to_owned();
                entry.last_synced = next.clone();
                entry.input.update(cx, |inp, cx| inp.set_text(next, cx));
            }
        }
        return entry.input.clone();
    }

    let placeholder = spec.placeholder.clone().unwrap_or_default();
    let initial = current_text.to_owned();
    let input = cx.new(|cx| {
        let mut inp = TextInput::new(Mode::SingleLine, placeholder, cx);
        inp.set_text(initial, cx);
        inp
    });

    let pid_change = plugin_id.to_owned();
    let skey_change = spec.key.clone();
    let skind_change = spec.kind;
    let min_v = spec.min;
    let max_v = spec.max;
    let input_for_change = input.clone();
    let sub_changed = cx.subscribe(&input, move |this, _, _: &Changed, cx| {
        let pid = pid_change.clone();
        let skey = skey_change.clone();
        let inp = input_for_change.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTING_SAVE_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                let raw = inp.read(cx).text().to_owned();
                if let Some(val) = parse_setting_input_value(skind_change, &raw, min_v, max_v) {
                    if let Some(entry) =
                        this.setting_inputs.get_mut(&(pid.clone(), skey.clone()))
                    {
                        entry.last_synced = raw;
                    }
                    this.update_plugin_setting_value(&pid, &skey, val, cx);
                }
            });
        });
        if let Some(entry) = this
            .setting_inputs
            .get_mut(&(pid_change.clone(), skey_change.clone()))
        {
            entry.save_task = Some(task);
        }
    });

    let pid_sub = plugin_id.to_owned();
    let skey_sub = spec.key.clone();
    let skind_sub = spec.kind;
    let sub_submit = cx.subscribe(&input, move |this, _, ev: &Submitted, cx| {
        if let Some(entry) = this
            .setting_inputs
            .get_mut(&(pid_sub.clone(), skey_sub.clone()))
        {
            entry.save_task = None;
        }
        if let Some(val) = parse_setting_input_value(skind_sub, &ev.0, min_v, max_v) {
            if let Some(entry) = this
                .setting_inputs
                .get_mut(&(pid_sub.clone(), skey_sub.clone()))
            {
                entry.last_synced = ev.0.clone();
            }
            this.update_plugin_setting_value(&pid_sub, &skey_sub, val, cx);
        }
    });

    setting_inputs.insert(
        map_key,
        SettingInputEntry {
            input: input.clone(),
            last_synced: current_text.to_owned(),
            save_task: None,
            _subs: [sub_changed, sub_submit],
        },
    );
    input
}

impl PluginSettingsView {

    pub(super) fn update_plugin_setting_value(
        &mut self,
        plugin_id: &str,
        key: &str,
        value: JsonValue,
        cx: &mut Context<Self>,
    ) {
        if PluginHost::global(cx)
            .and_then(|h| h.manager())
            .is_none()
        {
            return;
        }
        self.pending_setting_overrides
            .entry(plugin_id.to_owned())
            .or_default()
            .insert(key.to_owned(), value);
        cx.notify();
        self.flush_plugin_settings(plugin_id, cx);
    }

    fn flush_plugin_settings(&mut self, plugin_id: &str, cx: &mut Context<Self>) {
        if self.setting_update_in_flight.contains(plugin_id) {
            self.setting_update_dirty.insert(plugin_id.to_owned());
            return;
        }
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        let mut merged = PluginHost::global(cx)
            .and_then(|h| h.plugin(plugin_id))
            .map(|p| p.settings_values.clone())
            .unwrap_or_default();
        let sent_overrides = self
            .pending_setting_overrides
            .get(plugin_id)
            .cloned()
            .unwrap_or_default();
        for (k, v) in &sent_overrides {
            merged.insert(k.clone(), v.clone());
        }
        let pid = plugin_id.to_owned();
        let merged_for_host = merged.clone();
        self.setting_update_in_flight.insert(pid.clone());
        let pid_bg = pid.clone();
        let task = cx.background_spawn(async move { mgr.update_settings(&pid_bg, merged) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.setting_update_in_flight.remove(&pid);
                match res {
                    Ok(()) => {
                        PluginHost::apply_local_settings(&pid, merged_for_host, cx);
                        if let Some(pending) = this.pending_setting_overrides.get_mut(&pid) {
                            for (k, v) in &sent_overrides {
                                if pending.get(k) == Some(v) {
                                    pending.remove(k);
                                }
                            }
                            if pending.is_empty() {
                                this.pending_setting_overrides.remove(&pid);
                            }
                        }
                    }
                    Err(err) => {
                        if let Some(pending) = this.pending_setting_overrides.get_mut(&pid) {
                            for (k, v) in &sent_overrides {
                                if pending.get(k) == Some(v) {
                                    pending.remove(k);
                                }
                            }
                            if pending.is_empty() {
                                this.pending_setting_overrides.remove(&pid);
                            }
                        }
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                if this.setting_update_dirty.remove(&pid) {
                    this.flush_plugin_settings(&pid, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_secret_input(
        &mut self,
        plugin_id: &str,
        spec: &SettingSpec,
        is_present: bool,
        strings: &i18n::Strings,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let map_key = (plugin_id.to_owned(), spec.key.clone());
        let desired_ph = if is_present {
            strings.plugins_secret_placeholder.to_owned()
        } else {
            spec.placeholder.clone().unwrap_or_default()
        };
        if let Some(entry) = self.secret_inputs.get_mut(&map_key) {
            if entry.placeholder != desired_ph {
                entry.placeholder = desired_ph.clone();
                entry
                    .input
                    .update(cx, |inp, cx| inp.set_placeholder(desired_ph, cx));
            }
            return entry.input.clone();
        }
        let ph_for_input = desired_ph.clone();
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, ph_for_input, cx).masked());
        self.secret_inputs.insert(
            map_key,
            SecretInputEntry {
                input: input.clone(),
                placeholder: desired_ph,
            },
        );
        input
    }

    fn save_secret_setting(&mut self, plugin_id: String, key: String, cx: &mut Context<Self>) {
        let map_key = (plugin_id.clone(), key.clone());
        if self.in_flight_secrets.contains(&map_key) {
            return;
        }
        let Some(input) = self.secret_inputs.get(&map_key).map(|e| e.input.clone()) else {
            return;
        };
        let val = input.read(cx).text().to_owned();
        if val.is_empty() {
            return;
        }
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        self.in_flight_secrets.insert(map_key.clone());
        cx.notify();
        let task = cx.background_spawn(async move { mgr.set_secret(&plugin_id, &key, &val) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.in_flight_secrets.remove(&map_key);
                match res {
                    Ok(()) => {
                        let placeholder = i18n::t(cx).plugins_secret_placeholder.to_owned();
                        if let Some(entry) = this.secret_inputs.get_mut(&map_key) {
                            entry.placeholder = placeholder.clone();
                            entry.input.update(cx, |inp, cx| {
                                inp.set_text("", cx);
                                inp.set_placeholder(placeholder, cx);
                            });
                        }
                        this.status_banner = None;
                    }
                    Err(err) => {
                        this.status_banner = Some((format!("{err:#}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete_secret_setting(&mut self, plugin_id: String, key: String, cx: &mut Context<Self>) {
        let map_key = (plugin_id.clone(), key.clone());
        if self.in_flight_secrets.contains(&map_key) {
            return;
        }
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            return;
        };
        self.in_flight_secrets.insert(map_key.clone());
        cx.notify();
        let task = cx.background_spawn(async move { mgr.delete_secret(&plugin_id, &key) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.in_flight_secrets.remove(&map_key);
                if let Err(err) = res {
                    this.status_banner = Some((format!("{err:#}"), true));
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_plugin_settings_form(
        &mut self,
        info: &PluginInfo,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let (bg, border, element_hover) = {
            let colors = cx.theme().colors();
            (colors.background, colors.border, colors.element_hover)
        };
        let pid: SharedString = info.manifest.id.clone().into();
        let mut rows = Vec::with_capacity(info.manifest.settings.len());

        for spec in &info.manifest.settings {
            let row = match spec.kind {
                SettingKind::Text => {
                    let current =
                        effective_setting_value(&self.pending_setting_overrides, info, &spec.key)
                            .and_then(JsonValue::as_str)
                            .unwrap_or("");
                    let input = ensure_setting_input_in(
                        &mut self.setting_inputs,
                        pid.as_ref(),
                        spec,
                        current,
                        cx,
                    );
                    setting_row_shell(
                        spec,
                        div()
                            .w_full()
                            .h(px(30.))
                            .px_2p5()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .rounded(px(6.))
                            .bg(bg)
                            .border_1()
                            .border_color(border)
                            .child(input)
                            .into_any_element(),
                    )
                }
                SettingKind::Number => {
                    let num =
                        effective_setting_value(&self.pending_setting_overrides, info, &spec.key)
                            .and_then(JsonValue::as_f64)
                            .unwrap_or(0.0);
                    let current_str = format_number_clean(num);
                    let input = ensure_setting_input_in(
                        &mut self.setting_inputs,
                        pid.as_ref(),
                        spec,
                        &current_str,
                        cx,
                    );
                    let step = spec.step.unwrap_or(1.0);
                    let min_v = spec.min;
                    let max_v = spec.max;
                    let pid_dec = pid.clone();
                    let key_dec: SharedString = spec.key.clone().into();
                    let pid_inc = pid.clone();
                    let key_inc = key_dec.clone();
                    let inp_dec = input.clone();
                    let inp_inc = input.clone();

                    setting_row_shell(
                        spec,
                        h_flex()
                            .gap_1p5()
                            .child(
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "set-num-dec-{pid}-{}",
                                        spec.key
                                    ))),
                                    IconName::Minus,
                                )
                                .size(px(28.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let current = inp_dec
                                        .read(cx)
                                        .text()
                                        .trim()
                                        .parse::<f64>()
                                        .ok()
                                        .filter(|v| v.is_finite())
                                        .unwrap_or(num);
                                    let next = clamp_number(current - step, min_v, max_v);
                                    let text = format_number_clean(next);
                                    inp_dec.update(cx, |inp, cx| inp.set_text(text.clone(), cx));
                                    if let Some(entry) = this.setting_inputs.get_mut(&(
                                        pid_dec.as_ref().to_owned(),
                                        key_dec.as_ref().to_owned(),
                                    )) {
                                        entry.save_task = None;
                                        entry.last_synced = text;
                                    }
                                    if let Some(n) = serde_json::Number::from_f64(next) {
                                        this.update_plugin_setting_value(
                                            pid_dec.as_ref(),
                                            key_dec.as_ref(),
                                            JsonValue::Number(n),
                                            cx,
                                        );
                                    }
                                })),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(72.))
                                    .h(px(30.))
                                    .px_2p5()
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .rounded(px(6.))
                                    .bg(bg)
                                    .border_1()
                                    .border_color(border)
                                    .child(input),
                            )
                            .child(
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "set-num-inc-{pid}-{}",
                                        spec.key
                                    ))),
                                    IconName::Plus,
                                )
                                .size(px(28.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let current = inp_inc
                                        .read(cx)
                                        .text()
                                        .trim()
                                        .parse::<f64>()
                                        .ok()
                                        .filter(|v| v.is_finite())
                                        .unwrap_or(num);
                                    let next = clamp_number(current + step, min_v, max_v);
                                    let text = format_number_clean(next);
                                    inp_inc.update(cx, |inp, cx| inp.set_text(text.clone(), cx));
                                    if let Some(entry) = this.setting_inputs.get_mut(&(
                                        pid_inc.as_ref().to_owned(),
                                        key_inc.as_ref().to_owned(),
                                    )) {
                                        entry.save_task = None;
                                        entry.last_synced = text;
                                    }
                                    if let Some(n) = serde_json::Number::from_f64(next) {
                                        this.update_plugin_setting_value(
                                            pid_inc.as_ref(),
                                            key_inc.as_ref(),
                                            JsonValue::Number(n),
                                            cx,
                                        );
                                    }
                                })),
                            )
                            .into_any_element(),
                    )
                }
                SettingKind::Boolean => {
                    let checked =
                        effective_setting_value(&self.pending_setting_overrides, info, &spec.key)
                            .and_then(JsonValue::as_bool)
                            .unwrap_or(false);
                    let pid_b = pid.clone();
                    let key_b: SharedString = spec.key.clone().into();
                    setting_row_shell(
                        spec,
                        div()
                            .id(ElementId::Name(SharedString::from(format!(
                                "set-bool-{pid_b}-{key_b}"
                            ))))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_plugin_setting_value(
                                    pid_b.as_ref(),
                                    key_b.as_ref(),
                                    JsonValue::Bool(!checked),
                                    cx,
                                );
                            }))
                            .child(Switch::new().toggle_state(checked))
                            .into_any_element(),
                    )
                }
                SettingKind::Select => {
                    let current_val =
                        effective_setting_value(&self.pending_setting_overrides, info, &spec.key)
                            .and_then(JsonValue::as_str)
                            .unwrap_or("");
                    let current_lbl = spec
                        .options
                        .iter()
                        .find(|o| o.value == current_val)
                        .map_or(current_val, |o| o.label.as_str())
                        .to_owned();
                    let is_open = self
                        .open_select_setting
                        .as_ref()
                        .is_some_and(|(p, k)| p == pid.as_ref() && k == &spec.key);
                    let pid_open = pid.clone();
                    let key_open: SharedString = spec.key.clone().into();
                    let menu = if is_open {
                        let items: Vec<AnyElement> = spec
                            .options
                            .iter()
                            .enumerate()
                            .map(|(i, opt)| {
                                let selected = opt.value == current_val;
                                let val = opt.value.clone();
                                let pid_pick = pid.clone();
                                let key_pick = key_open.clone();
                                let mut item = ListItem::new(ElementId::Name(SharedString::from(
                                    format!("set-sel-{pid}-{}-{i}", spec.key),
                                )))
                                .toggle_state(selected);
                                if selected {
                                    item = item.end_slot(
                                        Icon::new(IconName::Check)
                                            .size(IconSize::XSmall)
                                            .color(Color::Accent),
                                    );
                                }
                                item.on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_select_setting = None;
                                    this.update_plugin_setting_value(
                                        pid_pick.as_ref(),
                                        key_pick.as_ref(),
                                        JsonValue::String(val.clone()),
                                        cx,
                                    );
                                    cx.notify();
                                }))
                                .child(Label::new(opt.label.clone()).size(LabelSize::Small))
                                .into_any_element()
                            })
                            .collect();
                        let scroll_id = ElementId::Name(SharedString::from(format!(
                            "set-sel-scroll-{pid}-{}",
                            spec.key
                        )));
                        let vp_id = ElementId::Name(SharedString::from(format!(
                            "set-sel-vp-{pid}-{}",
                            spec.key
                        )));
                        Some(PopoverAttachment::new(
                            Placement::BelowRight,
                            popover(cx)
                                .min_w(px(160.))
                                .max_w(px(280.))
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.open_select_setting = None;
                                    cx.stop_propagation();
                                    cx.notify();
                                }))
                                .child(
                                    v_scroll(scroll_id, v_flex().id(vp_id).w_full().children(items))
                                        .compact()
                                        .max_h(px(260.))
                                        .w_full(),
                                ),
                        ))
                    } else {
                        None
                    };

                    setting_row_shell(
                        spec,
                        div()
                            .relative()
                            .child(
                                h_flex()
                                    .id(ElementId::Name(SharedString::from(format!(
                                        "set-sel-btn-{pid}-{}",
                                        spec.key
                                    ))))
                                    .h(px(30.))
                                    .px_2p5()
                                    .gap_2()
                                    .justify_between()
                                    .rounded(px(6.))
                                    .bg(bg)
                                    .border_1()
                                    .border_color(border)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(element_hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let is_same = this.open_select_setting.as_ref().is_some_and(
                                            |(p, k)| p == pid_open.as_ref() && k == key_open.as_ref(),
                                        );
                                        if is_same {
                                            this.open_select_setting = None;
                                        } else {
                                            this.open_select_setting = Some((
                                                pid_open.as_ref().to_owned(),
                                                key_open.as_ref().to_owned(),
                                            ));
                                        }
                                        cx.notify();
                                    }))
                                    .child(Label::new(current_lbl).size(LabelSize::Small).truncate())
                                    .child(
                                        Icon::new(IconName::ChevronDown)
                                            .size(IconSize::XSmall)
                                            .color(Color::IconMuted),
                                    ),
                            )
                            .children(menu)
                            .into_any_element(),
                    )
                }
                SettingKind::Secret => {
                    let is_present = info
                        .secret_keys_present
                        .get(&spec.key)
                        .copied()
                        .unwrap_or(false);
                    let secret_busy = self
                        .in_flight_secrets
                        .iter()
                        .any(|(p, k)| p == pid.as_ref() && k == &spec.key);
                    let input =
                        self.ensure_secret_input(pid.as_ref(), spec, is_present, strings, cx);
                    let pid_save = pid.clone();
                    let key_save: SharedString = spec.key.clone().into();
                    let pid_del = pid.clone();
                    let key_del = key_save.clone();

                    setting_row_shell(
                        spec,
                        h_flex()
                            .gap_1p5()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(30.))
                                    .px_2p5()
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .rounded(px(6.))
                                    .bg(bg)
                                    .border_1()
                                    .border_color(border)
                                    .child(input),
                            )
                            .child(
                                Button::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "set-secret-save-{pid}-{}",
                                        spec.key
                                    ))),
                                    strings.save,
                                )
                                .variant(ButtonVariant::Secondary)
                                .size(ButtonSize::Sm)
                                .disabled(secret_busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_secret_setting(
                                        pid_save.as_ref().to_owned(),
                                        key_save.as_ref().to_owned(),
                                        cx,
                                    );
                                })),
                            )
                            .when(is_present, |row| {
                                row.child(
                                    IconButton::new(
                                        ElementId::Name(SharedString::from(format!(
                                            "set-secret-del-{pid}-{}",
                                            spec.key
                                        ))),
                                        IconName::Trash,
                                    )
                                    .size(px(28.))
                                    .disabled(secret_busy)
                                    .tooltip(Tooltip::text(strings.plugin_clear_secret))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.delete_secret_setting(
                                            pid_del.as_ref().to_owned(),
                                            key_del.as_ref().to_owned(),
                                            cx,
                                        );
                                    })),
                                )
                            })
                            .into_any_element(),
                    )
                }
            };
            rows.push(row);
        }

        v_flex().w_full().gap_2p5().pt_1().children(rows).into_any_element()
    }
}
