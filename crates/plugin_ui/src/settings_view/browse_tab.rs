use std::sync::Arc;

use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px,
};
use icons::IconName;
use plugins::registry::DEFAULT_REGISTRY_URL;
use plugins::{PermissionApprovalStatus, PluginInfo, RegistryPluginEntry, RegistrySource};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Chip, Color, EmptyState, Icon, IconButton, IconSize, Label,
    LabelSize, Switch,
};

use super::PluginSettingsView;
use super::helpers::REGISTRIES_SETTING_KEY;
use super::modals::PermissionPromptState;
use crate::host::PluginHost;

#[derive(Clone)]
pub(super) struct CatalogItem {
    pub(super) source_url: String,
    pub(super) entry: RegistryPluginEntry,
}

impl PluginSettingsView {
    pub(super) fn load_cached_registries_async(&mut self, cx: &mut Context<Self>) {
        let Some(client) = PluginHost::global(cx).map(|h| h.registry_client().clone()) else {
            return;
        };
        let enabled_urls: Vec<String> = self
            .registries
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.url.clone())
            .collect();
        let generation = self.browse_generation;
        let fetch_seq = self.browse_fetch_seq;

        let task = cx.background_spawn(async move {
            let mut items = Vec::new();
            for url in enabled_urls {
                if let Ok(Some(cached)) = client.load_cached_index(&url) {
                    for entry in cached.index.plugins {
                        items.push(CatalogItem {
                            source_url: url.clone(),
                            entry,
                        });
                    }
                }
            }
            items
        });
        cx.spawn(async move |this, cx| {
            let items = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.browse_generation == generation
                    && this.browse_completed_fetch_seq <= fetch_seq
                {
                    this.catalog_items = items;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn refresh_registries(&mut self, cx: &mut Context<Self>) {
        let Some(client) = PluginHost::global(cx).map(|h| h.registry_client().clone()) else {
            return;
        };
        let sources = self.registries.clone();
        self.browse_fetch_seq = self.browse_fetch_seq.wrapping_add(1);
        let generation = self.browse_generation;
        let fetch_seq = self.browse_fetch_seq;
        self.browse_loading = true;
        self.registry_errors.clear();
        cx.notify();

        let task = cx.background_spawn(async move { client.fetch_all_resilient(&sources) });
        cx.spawn(async move |this, cx| {
            let results = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.browse_generation != generation || this.browse_fetch_seq != fetch_seq {
                    return;
                }
                this.browse_loading = false;
                this.browse_completed_fetch_seq = fetch_seq;
                let mut items = Vec::new();
                let mut errors = Vec::new();
                for (url, res) in results {
                    match res {
                        Ok(idx) => {
                            for entry in idx.index.plugins {
                                items.push(CatalogItem {
                                    source_url: url.clone(),
                                    entry,
                                });
                            }
                        }
                        Err(err) => {
                            errors.push(format!("{url}: {err:#}"));
                        }
                    }
                }
                this.catalog_items = items;
                this.registry_errors = errors;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn save_registries(&mut self) {
        if let Ok(json) = serde_json::to_string(&self.registries) {
            let _ = self.store.set_setting(REGISTRIES_SETTING_KEY, &json);
        }
    }

    fn remove_registry_source(&mut self, url: &str, cx: &mut Context<Self>) {
        self.registries.retain(|s| s.url != url);
        if self.registries.is_empty() {
            self.registries.push(RegistrySource::default());
        }
        self.browse_generation = self.browse_generation.wrapping_add(1);
        self.browse_fetch_seq = self.browse_fetch_seq.wrapping_add(1);
        self.browse_loading = false;
        self.save_registries();
        self.load_cached_registries_async(cx);
        cx.notify();
    }

    fn install_from_registry(&mut self, entry: RegistryPluginEntry, cx: &mut Context<Self>) {
        let Some(client) = PluginHost::global(cx).map(|h| h.registry_client().clone()) else {
            return;
        };
        let pid = entry.id.clone();
        if !self.installing_plugin_ids.insert(pid.clone()) {
            return;
        }
        let mgr = PluginHost::global(cx).and_then(|h| h.manager().cloned());
        let existing_state = self.store.plugin_state(&entry.id).ok().flatten();
        cx.notify();

        let task = cx.background_spawn(async move {
            let outcome = client.install_from_registry(&entry, existing_state.as_ref())?;
            if let Some(mgr) = mgr {
                let _ = mgr.reload_all()?;
            }
            Ok::<_, anyhow::Error>(outcome)
        });

        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.installing_plugin_ids.remove(&pid);
                match res {
                    Ok(out) => {
                        this.status_banner = Some((
                            format!("{} v{}", out.manifest.name, out.manifest.version),
                            false,
                        ));
                        if let PermissionApprovalStatus::NeedsApproval {
                            diff,
                            is_first_install,
                        } = out.approval_status
                        {
                            this.pending_permission_modal = Some(PermissionPromptState {
                                plugin_id: out.manifest.id,
                                plugin_name: out.manifest.name,
                                version: out.manifest.version,
                                added_permissions: diff.added,
                                is_first_install,
                                pending_zip_overwrite: None,
                            });
                        }
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

    pub(super) fn render_browse_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();

        let installed_plugins: Arc<Vec<PluginInfo>> = PluginHost::global(cx)
            .map(PluginHost::plugins_arc)
            .unwrap_or_default();

        let query = self
            .browse_search_input
            .read(cx)
            .text()
            .trim()
            .to_ascii_lowercase();

        let filtered: Vec<&CatalogItem> = self
            .catalog_items
            .iter()
            .filter(|item| {
                if query.is_empty() {
                    return true;
                }
                let e = &item.entry;
                e.name.to_ascii_lowercase().contains(&query)
                    || e.id.to_ascii_lowercase().contains(&query)
                    || e.description.to_ascii_lowercase().contains(&query)
                    || e.author.to_ascii_lowercase().contains(&query)
                    || e.tags.iter().any(|t| t.to_ascii_lowercase().contains(&query))
            })
            .collect();

        let sources_panel = v_flex()
            .w_full()
            .p_4()
            .gap_2p5()
            .rounded(px(8.))
            .bg(colors.surface_background)
            .border_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        Label::new(strings.plugins_registries)
                            .size(LabelSize::Small)
                            .weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        Button::new("browse-refresh", strings.plugins_registry_refresh)
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .icon(IconName::RefreshCw)
                            .disabled(self.browse_loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_registries(cx);
                            })),
                    ),
            )
            .children(self.registries.iter().enumerate().map(|(idx, src)| {
                let is_default = src.url == DEFAULT_REGISTRY_URL;
                let url_del = (!is_default).then(|| src.url.clone());
                let src_label = match &src.name {
                    Some(n) => format!("{n} ({})", src.url),
                    None => src.url.clone(),
                };
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .id(ElementId::Name(SharedString::from(format!(
                                        "reg-toggle-{idx}"
                                    ))))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(s) = this.registries.get_mut(idx) {
                                            s.enabled = !s.enabled;
                                        }
                                        this.browse_generation =
                                            this.browse_generation.wrapping_add(1);
                                        this.browse_fetch_seq =
                                            this.browse_fetch_seq.wrapping_add(1);
                                        this.browse_loading = false;
                                        this.save_registries();
                                        this.load_cached_registries_async(cx);
                                        cx.notify();
                                    }))
                                    .child(Switch::new().toggle_state(src.enabled)),
                            )
                            .child(
                                Label::new(src_label)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .truncate(),
                            ),
                    )
                    .when_some(url_del, |row, url_del| {
                        row.child(
                            IconButton::new(
                                ElementId::Name(SharedString::from(format!("reg-remove-{idx}"))),
                                IconName::Trash,
                            )
                            .size(px(24.))
                            .icon_size(IconSize::XSmall)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_registry_source(&url_del, cx);
                            })),
                        )
                    })
            }))
            .child(
                h_flex()
                    .gap_2()
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
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border)
                            .child(self.registry_url_input.clone()),
                    )
                    .child(
                        Button::new("reg-add-btn", strings.add)
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.add_registry_url(cx);
                            })),
                    ),
            );

        let search_bar = div()
            .w_full()
            .h(px(32.))
            .px_2p5()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(6.))
            .bg(colors.background)
            .border_1()
            .border_color(colors.border)
            .child(
                Icon::new(IconName::Search)
                    .size(IconSize::Small)
                    .color(Color::IconMuted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.browse_search_input.clone()),
            );

        let catalog_cards: Vec<AnyElement> = if filtered.is_empty() {
            vec![
                div()
                    .w_full()
                    .py_10()
                    .child(EmptyState::new(
                        IconName::Search,
                        if self.browse_loading {
                            strings.plugin_loading
                        } else {
                            strings.nothing_found
                        },
                    ))
                    .into_any_element(),
            ]
        } else {
            filtered
                .into_iter()
                .enumerate()
                .map(|(cat_idx, item)| {
                    let e = &item.entry;
                    let installed_ver = installed_plugins
                        .iter()
                        .find(|p| p.manifest.id == e.id)
                        .map(|p| p.manifest.version.as_str());
                    let is_busy = self.installing_plugin_ids.contains(&e.id);
                    let pid: SharedString = e.id.clone().into();
                    let src_url: SharedString = item.source_url.clone().into();

                    let make_install_click = |pid_i: SharedString, src_i: SharedString| {
                        cx.listener(move |this, _, _, cx| {
                            if let Some(entry) = this
                                .catalog_items
                                .iter()
                                .find(|c| {
                                    c.source_url == src_i.as_ref()
                                        && c.entry.id == pid_i.as_ref()
                                })
                                .map(|c| c.entry.clone())
                            {
                                this.install_from_registry(entry, cx);
                            }
                        })
                    };
                    let make_remove_click = |pid_r: SharedString| {
                        cx.listener(move |this, _, _, cx| {
                            this.uninstall_plugin(pid_r.as_ref().to_owned(), cx);
                        })
                    };

                    let action_btn = match installed_ver {
                        None => Button::new(
                            ElementId::Name(SharedString::from(format!(
                                "cat-inst-{cat_idx}-{}",
                                e.id
                            ))),
                            strings.plugins_registry_install,
                        )
                        .variant(ButtonVariant::Primary)
                        .size(ButtonSize::Sm)
                        .disabled(is_busy)
                        .on_click(make_install_click(pid, src_url.clone()))
                        .into_any_element(),
                        Some(v) if v != e.version => h_flex()
                            .gap_1p5()
                            .child(
                                Button::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "cat-upd-{cat_idx}-{}",
                                        e.id
                                    ))),
                                    strings.plugins_registry_update,
                                )
                                .variant(ButtonVariant::Primary)
                                .size(ButtonSize::Sm)
                                .disabled(is_busy)
                                .on_click(make_install_click(pid.clone(), src_url.clone())),
                            )
                            .child(
                                Button::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "cat-rem-{cat_idx}-{}",
                                        e.id
                                    ))),
                                    strings.plugins_uninstall,
                                )
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .on_click(make_remove_click(pid)),
                            )
                            .into_any_element(),
                        Some(_) => h_flex()
                            .gap_1p5()
                            .child(Chip::new(
                                strings.plugins_registry_installed,
                                theme.status().success,
                            ))
                            .child(
                                Button::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "cat-rem-{cat_idx}-{}",
                                        e.id
                                    ))),
                                    strings.plugins_uninstall,
                                )
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .on_click(make_remove_click(pid)),
                            )
                            .into_any_element(),
                    };

                    let is_beta = e.version.to_ascii_lowercase().contains("beta")
                        || e.tags.iter().any(|t| t.eq_ignore_ascii_case("beta"));
                    let has_display_tags = e.tags.iter().any(|t| !t.eq_ignore_ascii_case("beta"));

                    v_flex()
                        .w_full()
                        .p_4()
                        .gap_2p5()
                        .rounded(px(8.))
                        .bg(colors.surface_background)
                        .border_1()
                        .border_color(colors.border_variant)
                        .child(
                            h_flex()
                                .justify_between()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_0p5()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .flex_wrap()
                                                .child(
                                                    Label::new(e.name.clone())
                                                        .weight(FontWeight::SEMIBOLD),
                                                )
                                                .child(
                                                    Label::new(format!("v{}", e.version))
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Muted),
                                                )
                                                .when(is_beta, |row| {
                                                    row.child(Chip::new(
                                                        strings.beta_badge,
                                                        theme.status().warning,
                                                    ))
                                                })
                                                .when(!e.author.is_empty(), |row| {
                                                    row.child(
                                                        Label::new(e.author.clone())
                                                            .size(LabelSize::XSmall)
                                                            .color(Color::Muted),
                                                    )
                                                }),
                                        )
                                        .when(!e.description.is_empty(), |col| {
                                            col.child(
                                                Label::new(e.description.clone())
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted),
                                            )
                                        }),
                                )
                                .child(action_btn),
                        )
                        .when(has_display_tags, |col| {
                            col.child(
                                h_flex().flex_wrap().gap_1().children(
                                    e.tags
                                        .iter()
                                        .filter(|t| !t.eq_ignore_ascii_case("beta"))
                                        .map(|t| Chip::new(t.clone(), colors.accent)),
                                ),
                            )
                        })
                        .when(!e.permissions.is_empty(), |col| {
                            col.child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .child(
                                        Label::new(format!("{}:", strings.plugins_permissions))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .children(
                                        e.permissions
                                            .iter()
                                            .map(|p| Chip::new(p.summary(), colors.accent)),
                                    ),
                            )
                        })
                        .child(
                            Label::new(src_url)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted)
                                .truncate(),
                        )
                        .into_any_element()
                })
                .collect()
        };

        v_flex()
            .w_full()
            .gap_4()
            .child(sources_panel)
            .when(!self.registry_errors.is_empty(), |col| {
                col.children(self.registry_errors.iter().map(|err| {
                    Label::new(err.clone())
                        .size(LabelSize::XSmall)
                        .color(Color::Error)
                }))
            })
            .child(search_bar)
            .children(catalog_cards)
            .into_any_element()
    }
}
