use std::collections::BTreeSet;

use desktop_common::data_version::data_changed;
use fern_core::plugin_store::UserScript;
use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px,
};
use icons::IconName;
use plugins::PermissionSet;
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Checkbox, Chip, Color, Label, LabelSize, ListItem,
    SegmentedTab, SegmentedTabs, Switch, v_scroll,
};
use uuid::Uuid;

use super::PluginSettingsView;
use super::helpers::{render_log_entries_box, trigger_kind_label};
use crate::host::PluginHost;

impl PluginSettingsView {
    pub(super) fn load_script_into_editor(&mut self, script: &UserScript, cx: &mut Context<Self>) {
        self.selected_script_id = Some(script.id.clone());
        self.script_enabled = script.enabled;
        self.script_trigger_kind = if script.trigger_kind.is_empty() {
            "manual".to_owned()
        } else {
            script.trigger_kind.clone()
        };
        self.script_permissions = PermissionSet::from_json_str(&script.permissions)
            .unwrap_or_default()
            .to_vec()
            .into_iter()
            .map(|p| p.as_str().to_owned())
            .collect();
        let name = script.name.clone();
        let arg = script.trigger_arg.clone();
        let code = script.code.clone();
        self.script_name_input
            .update(cx, |inp, cx| inp.set_text(name, cx));
        self.script_trigger_arg_input
            .update(cx, |inp, cx| inp.set_text(arg, cx));
        self.script_code_input
            .update(cx, |inp, cx| inp.set_text(code, cx));
        self.script_run_seq = self.script_run_seq.wrapping_add(1);
        self.script_running = false;
        self.script_result = None;
        cx.notify();
    }

    pub(super) fn reset_new_script_editor(&mut self, cx: &mut Context<Self>) {
        self.selected_script_id = None;
        self.script_enabled = true;
        self.script_trigger_kind = "manual".to_owned();
        self.script_permissions = BTreeSet::from(["tasks:read".to_owned()]);
        self.script_name_input
            .update(cx, |inp, cx| inp.set_text("New Script", cx));
        self.script_trigger_arg_input
            .update(cx, |inp, cx| inp.set_text("", cx));
        self.script_code_input.update(cx, |inp, cx| {
            inp.set_text(
                "const open = fern.tasks.list({ completed: false });\nconsole.log(\"Open tasks:\", open.length);\nreturn { open: open.length };\n",
                cx,
            );
        });
        self.script_run_seq = self.script_run_seq.wrapping_add(1);
        self.script_running = false;
        self.script_result = None;
        cx.notify();
    }

    fn build_draft_script(&self, cx: &App) -> UserScript {
        let id = self
            .selected_script_id
            .clone()
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        let name = self.script_name_input.read(cx).text().trim().to_owned();
        let final_name = if name.is_empty() {
            "Untitled Script".to_owned()
        } else {
            name
        };
        let perms: Vec<String> = self.script_permissions.iter().cloned().collect();
        let perms_json = serde_json::to_string(&perms).unwrap_or_else(|_| "[]".to_owned());

        UserScript {
            id,
            name: final_name,
            enabled: self.script_enabled,
            trigger_kind: self.script_trigger_kind.clone(),
            trigger_arg: self
                .script_trigger_arg_input
                .read(cx)
                .text()
                .trim()
                .to_owned(),
            permissions: perms_json,
            code: self.script_code_input.read(cx).text().to_owned(),
            updated_at: 0,
        }
    }

    fn save_current_script(&mut self, cx: &mut Context<Self>) {
        let draft = self.build_draft_script(cx);
        match self.store.save_user_script(&draft) {
            Ok(()) => {
                self.selected_script_id = Some(draft.id);
                self.scripts = self.store.user_scripts().unwrap_or_default();
                self.status_banner = None;
                data_changed(cx.entity_id(), cx);
            }
            Err(err) => {
                self.status_banner = Some((format!("{err:#}"), true));
            }
        }
        cx.notify();
    }

    fn delete_current_script(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_script_id.clone() else {
            return;
        };
        match self.store.delete_user_script(&id) {
            Ok(_) => {
                self.scripts = self.store.user_scripts().unwrap_or_default();
                if let Some(first) = self.scripts.first().cloned() {
                    self.load_script_into_editor(&first, cx);
                } else {
                    self.reset_new_script_editor(cx);
                }
                data_changed(cx.entity_id(), cx);
            }
            Err(err) => {
                self.status_banner = Some((format!("{err:#}"), true));
            }
        }
        cx.notify();
    }

    fn test_run_current_script(&mut self, cx: &mut Context<Self>) {
        let Some(mgr) = PluginHost::global(cx).and_then(|h| h.manager().cloned()) else {
            self.status_banner = Some((i18n::t(cx).plugins_safe_mode_active.to_owned(), true));
            cx.notify();
            return;
        };
        let draft = self.build_draft_script(cx);
        self.script_run_seq = self.script_run_seq.wrapping_add(1);
        let run_seq = self.script_run_seq;
        self.script_running = true;
        self.script_result = None;
        cx.notify();

        let task = cx.background_spawn(async move { mgr.test_run_script(draft, None) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(run_res) = &res
                    && run_res.data_changed
                {
                    data_changed(cx.entity_id(), cx);
                }
                if this.script_run_seq != run_seq {
                    return;
                }
                this.script_running = false;
                match res {
                    Ok(run_res) => {
                        this.script_result = Some(run_res);
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

    pub(super) fn render_scripts_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();

        let list_items: Vec<AnyElement> = if self.scripts.is_empty() {
            vec![
                div()
                    .p_3()
                    .child(
                        Label::new(strings.plugins_script_empty)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            ]
        } else {
            self.scripts
                .iter()
                .map(|script| {
                    let selected =
                        self.selected_script_id.as_deref() == Some(script.id.as_str());
                    let sid_select: SharedString = script.id.clone().into();
                    let sid_toggle = sid_select.clone();
                    let enabled = script.enabled;
                    ListItem::new(ElementId::Name(SharedString::from(format!(
                        "script-row-{}",
                        script.id
                    ))))
                    .toggle_state(selected)
                    .height(px(38.))
                    .end_slot(
                        h_flex()
                            .gap_2()
                            .child(Chip::new(
                                trigger_kind_label(&script.trigger_kind, strings),
                                colors.accent,
                            ))
                            .child(
                                div()
                                    .id(ElementId::Name(SharedString::from(format!(
                                        "script-enable-{}",
                                        script.id
                                    ))))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let _ = this
                                            .store
                                            .set_user_script_enabled(sid_toggle.as_ref(), !enabled);
                                        this.scripts =
                                            this.store.user_scripts().unwrap_or_default();
                                        if this.selected_script_id.as_deref()
                                            == Some(sid_toggle.as_ref())
                                        {
                                            this.script_enabled = !enabled;
                                        }
                                        data_changed(cx.entity_id(), cx);
                                        cx.notify();
                                    }))
                                    .child(Switch::new().toggle_state(enabled)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(sc) =
                            this.scripts.iter().find(|s| s.id == sid_select.as_ref()).cloned()
                        {
                            this.load_script_into_editor(&sc, cx);
                        }
                    }))
                    .child(Label::new(script.name.clone()).size(LabelSize::Small).truncate())
                    .into_any_element()
                })
                .collect()
        };

        let triggers = ["manual", "command", "event", "interval"];
        let mut trigger_tabs = SegmentedTabs::new();
        for trig in triggers {
            trigger_tabs = trigger_tabs.tab(
                SegmentedTab::new(
                    ElementId::Name(SharedString::from(format!("script-trig-{trig}"))),
                    trigger_kind_label(trig, strings),
                )
                .toggle_state(self.script_trigger_kind == trig)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.script_trigger_kind = trig.to_owned();
                    cx.notify();
                })),
            );
        }

        let perm_choices = [
            "tasks:read",
            "tasks:write",
            "lists:read",
            "lists:write",
            "tags:read",
            "habits:read",
            "habits:write",
            "focus:read",
            "countdowns:read",
            "storage",
            "meta",
            "notifications",
            "clipboard",
        ];

        let perm_chips = h_flex().flex_wrap().gap_1p5().children(perm_choices.iter().map(|&perm| {
            let active = self.script_permissions.contains(perm);
            div()
                .id(ElementId::Name(SharedString::from(format!("script-perm-{perm}"))))
                .px_2()
                .h(px(24.))
                .flex()
                .items_center()
                .gap_1()
                .rounded(px(5.))
                .border_1()
                .border_color(if active { colors.accent } else { colors.border })
                .when(active, |el| el.bg(colors.accent.opacity(0.14)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.script_permissions.remove(perm) {
                        this.script_permissions.insert(perm.to_owned());
                    }
                    cx.notify();
                }))
                .child(
                    Checkbox::new(ElementId::Name(SharedString::from(format!(
                        "script-perm-cb-{perm}"
                    ))))
                    .size(px(12.))
                    .accent()
                    .toggle_state(active),
                )
                .child(Label::new(perm).size(LabelSize::XSmall))
        }));

        let result_panel = self.script_result.as_ref().map(|res| {
            let status_color = if res.ok {
                theme.status().success
            } else {
                theme.status().error
            };
            let ret_str = res
                .return_value
                .as_ref()
                .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()));
            v_flex()
                .w_full()
                .p_3()
                .gap_2()
                .rounded(px(8.))
                .bg(colors.surface_background)
                .border_1()
                .border_color(colors.border_variant)
                .child(
                    h_flex().justify_between().child(
                        h_flex()
                            .gap_2()
                            .child(Chip::new(
                                if res.ok {
                                    strings.plugin_run_ok
                                } else {
                                    strings.plugin_run_err
                                },
                                status_color,
                            ))
                            .child(
                                Label::new(format!("{} ms", res.duration_ms))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    ),
                )
                .when_some(res.error.as_deref(), |col, err| {
                    col.child(
                        Label::new(err.to_owned())
                            .size(LabelSize::Small)
                            .color(Color::Error),
                    )
                })
                .when_some(ret_str, |col, val| {
                    col.child(
                        div()
                            .p_2()
                            .rounded(px(6.))
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border)
                            .text_size(LabelSize::XSmall.pixels())
                            .text_color(colors.text)
                            .child(val),
                    )
                })
                .child(render_log_entries_box(
                    &res.logs,
                    strings.plugin_no_logs,
                    colors,
                    theme.status(),
                ))
        });

        v_flex()
            .w_full()
            .gap_4()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Label::new(strings.plugins_scripts)
                            .size(LabelSize::Default)
                            .weight(FontWeight::MEDIUM),
                    )
                    .child(
                        Button::new("scripts-new", strings.plugins_script_new)
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reset_new_script_editor(cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .p_1p5()
                    .gap_1()
                    .rounded(px(8.))
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.border_variant)
                    .children(list_items),
            )
            .child(
                v_flex()
                    .w_full()
                    .p_4()
                    .gap_3()
                    .rounded(px(8.))
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.border_variant)
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div().w(px(110.)).child(
                                    Label::new(strings.plugins_script_name).size(LabelSize::Small),
                                ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(30.))
                                    .px_2p5()
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .rounded(px(6.))
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .child(self.script_name_input.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div().w(px(110.)).child(
                                    Label::new(strings.plugins_script_trigger)
                                        .size(LabelSize::Small),
                                ),
                            )
                            .child(trigger_tabs),
                    )
                    .when(self.script_trigger_kind != "manual", |col| {
                        col.child(
                            h_flex()
                                .gap_3()
                                .child(
                                    div().w(px(110.)).child(
                                        Label::new(strings.plugin_trigger_arg)
                                            .size(LabelSize::Small),
                                    ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .h(px(30.))
                                        .px_2p5()
                                        .flex()
                                        .items_center()
                                        .overflow_hidden()
                                        .rounded(px(6.))
                                        .bg(colors.background)
                                        .border_1()
                                        .border_color(colors.border)
                                        .child(self.script_trigger_arg_input.clone()),
                                ),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new(strings.plugins_permissions)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(perm_chips),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new(strings.plugin_code_label)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(
                                v_scroll(
                                    "script-code-scroll",
                                    div()
                                        .id("script-code-vp")
                                        .w_full()
                                        .p_2()
                                        .child(self.script_code_input.clone()),
                                )
                                .track_scroll(&self.script_code_scroll)
                                .h(px(180.))
                                .w_full()
                                .rounded(px(6.))
                                .bg(colors.background)
                                .border_1()
                                .border_color(colors.border),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("script-save", strings.plugins_script_save)
                                            .variant(ButtonVariant::Primary)
                                            .size(ButtonSize::Sm)
                                            .icon(IconName::Check)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.save_current_script(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("script-run", strings.plugins_script_run)
                                            .variant(ButtonVariant::Secondary)
                                            .size(ButtonSize::Sm)
                                            .icon(IconName::Play)
                                            .disabled(self.script_running)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.test_run_current_script(cx);
                                            })),
                                    ),
                            )
                            .when(self.selected_script_id.is_some(), |row| {
                                row.child(
                                    Button::new("script-delete", strings.plugins_script_delete)
                                        .variant(ButtonVariant::Destructive)
                                        .size(ButtonSize::Sm)
                                        .icon(IconName::Trash)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.delete_current_script(cx);
                                        })),
                                )
                            }),
                    ),
            )
            .children(result_panel)
            .into_any_element()
    }
}
