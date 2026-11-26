//! The narrow sidebar with app sections, modeled on TickTick's.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use desktop_common::plugin_bus::{PluginBus, PluginSidebarViewSpec};
use fern_core::{RailItemId, RailSection};
use gpui::{AnyElement, ClickEvent, Global, Hsla, RenderImage, ScrollHandle};
use ui::prelude::*;

use crate::i18n::{self, Strings};

const RAIL_ICON_PX: u32 = 48;
const MAX_RAIL_SVG_CACHE: usize = 128;

#[derive(Default)]
struct RailSvgCache(HashMap<(u64, u32), Option<Arc<RenderImage>>>);

impl Global for RailSvgCache {}

/// Payload carried while dragging a sidebar rail icon (built-in section or plugin view).
#[derive(Clone, Debug)]
pub struct DraggedRailItem {
    pub item: RailItemId,
    pub icon: IconName,
    pub icon_svg: Option<Arc<str>>,
    pub active: bool,
}

impl Render for DraggedRailItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let icon_color = if self.active {
            colors.sidebar_icon
        } else {
            colors.sidebar_icon_muted
        };
        let custom_img = self
            .icon_svg
            .as_deref()
            .and_then(|svg| rasterize_rail_svg(svg, icon_color, cx));
        div()
            .size(px(40.))
            .rounded(px(8.))
            .bg(colors.sidebar_background.opacity(0.95))
            .border_1()
            .border_color(colors.sidebar_border)
            .shadow_md()
            .flex()
            .items_center()
            .justify_center()
            .child(if let Some(img) = custom_img {
                let alpha = icon_color.to_rgb().a;
                let mut el = gpui::img(img).size(px(24.));
                if alpha < 0.999 {
                    el = el.opacity(alpha);
                }
                el.into_any_element()
            } else {
                Icon::new(self.icon)
                    .size(IconSize::Custom(px(24.)))
                    .color(icon_color)
                    .into_any_element()
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Tasks,
    Things,
    Calendar,
    Habits,
    Pomodoro,
    Search,
    Matrix,
    Countdown,
    LifeClock,
    Plugin {
        plugin_id: SharedString,
        view_id: SharedString,
    },
}

impl From<RailSection> for Section {
    fn from(section: RailSection) -> Self {
        match section {
            RailSection::Tasks => Section::Tasks,
            RailSection::Things => Section::Things,
            RailSection::Calendar => Section::Calendar,
            RailSection::Pomodoro => Section::Pomodoro,
            RailSection::Habits => Section::Habits,
            RailSection::Search => Section::Search,
            RailSection::Matrix => Section::Matrix,
            RailSection::Countdown => Section::Countdown,
            RailSection::LifeClock => Section::LifeClock,
        }
    }
}

impl Section {
    pub fn try_from_rail_item(item: &RailItemId) -> Self {
        match item {
            RailItemId::BuiltIn(section) => Self::from(*section),
            RailItemId::Plugin { plugin_id, view_id } => Self::Plugin {
                plugin_id: plugin_id.clone().into(),
                view_id: view_id.clone().into(),
            },
        }
    }

    /// Whether Settings → Features leaves the section on; Search and active plugin views are shown.
    pub fn enabled(&self, features: fern_core::Features) -> bool {
        match self {
            Section::Tasks => features.tasks,
            Section::Things => features.things,
            Section::Search => true,
            Section::Calendar => features.calendar,
            Section::Matrix => features.matrix,
            Section::Habits => features.habits,
            Section::Pomodoro => features.focus,
            Section::Countdown => features.countdown,
            Section::LifeClock => features.life_clock,
            Section::Plugin { .. } => true,
        }
    }

    pub fn title(&self, strings: &Strings) -> SharedString {
        match self {
            Section::Tasks => strings.section_tasks.into(),
            Section::Things => strings.section_things.into(),
            Section::Calendar => strings.section_calendar.into(),
            Section::Habits => strings.section_habits.into(),
            Section::Pomodoro => strings.section_pomodoro.into(),
            Section::Search => strings.section_search.into(),
            Section::Matrix => strings.section_matrix.into(),
            Section::Countdown => strings.section_countdown.into(),
            Section::LifeClock => strings.section_life_clock.into(),
            Section::Plugin { plugin_id, .. } => plugin_id.clone(),
        }
    }

    pub fn icon(&self) -> IconName {
        match self {
            // Filled glyphs as in the original, drawn for Fern (`assets/icons/fern_*.svg`): a target
            // for Focus and a clock for Habit.
            Section::Tasks => IconName::FernTasks,
            Section::Things => IconName::Layers,
            Section::Calendar => IconName::FernCalendar,
            Section::Habits => IconName::FernHabit,
            Section::Pomodoro => IconName::FernFocus,
            Section::Search => IconName::Search,
            Section::Matrix => IconName::FernMatrix,
            Section::Countdown => IconName::FernCountdown,
            Section::LifeClock => IconName::FernLifeClock,
            Section::Plugin { .. } => IconName::Code,
        }
    }

    fn id(&self) -> SharedString {
        match self {
            Section::Tasks => "section-tasks".into(),
            Section::Things => "section-things".into(),
            Section::Calendar => "section-calendar".into(),
            Section::Habits => "section-habits".into(),
            Section::Pomodoro => "section-pomodoro".into(),
            Section::Search => "section-search".into(),
            Section::Matrix => "section-matrix".into(),
            Section::Countdown => "section-countdown".into(),
            Section::LifeClock => "section-life-clock".into(),
            Section::Plugin { plugin_id, view_id } => {
                format!("section-plugin-{}:{plugin_id}/{view_id}", plugin_id.len()).into()
            }
        }
    }
}

#[derive(Clone)]
struct ResolvedRailEntry {
    item: RailItemId,
    section: Section,
    title: SharedString,
    icon_svg: Option<Arc<str>>,
}

fn resolve_rail_entries(
    order: &[RailItemId],
    features: fern_core::Features,
    plugin_views: &[PluginSidebarViewSpec],
    strings: &Strings,
) -> Vec<ResolvedRailEntry> {
    let view_by_key: HashMap<(&str, &str), &PluginSidebarViewSpec> = plugin_views
        .iter()
        .map(|spec| ((spec.plugin_id.as_str(), spec.view_id.as_str()), spec))
        .collect();
    let mut seen_builtins = HashSet::new();
    let mut seen_plugins = HashSet::new();
    let mut entries = Vec::new();

    for item in order {
        match item {
            RailItemId::BuiltIn(rail_sec) => {
                if seen_builtins.insert(*rail_sec) {
                    let section = Section::from(*rail_sec);
                    if section.enabled(features) {
                        let title = section.title(strings);
                        entries.push(ResolvedRailEntry {
                            item: RailItemId::BuiltIn(*rail_sec),
                            section,
                            title,
                            icon_svg: None,
                        });
                    }
                }
            }
            RailItemId::Plugin { plugin_id, view_id } => {
                let key = (plugin_id.as_str(), view_id.as_str());
                if let Some(spec) = view_by_key.get(&key)
                    && seen_plugins.insert((spec.plugin_id.as_str(), spec.view_id.as_str()))
                {
                    entries.push(ResolvedRailEntry {
                        item: item.clone(),
                        section: Section::Plugin {
                            plugin_id: spec.plugin_id.clone().into(),
                            view_id: spec.view_id.clone().into(),
                        },
                        title: spec.title.clone().into(),
                        icon_svg: spec.icon_svg.clone(),
                    });
                }
            }
        }
    }

    for rail_sec in RailSection::ALL {
        if seen_builtins.insert(rail_sec) {
            let section = Section::from(rail_sec);
            if section.enabled(features) {
                let title = section.title(strings);
                entries.push(ResolvedRailEntry {
                    item: RailItemId::BuiltIn(rail_sec),
                    section,
                    title,
                    icon_svg: None,
                });
            }
        }
    }

    for spec in plugin_views {
        if seen_plugins.insert((spec.plugin_id.as_str(), spec.view_id.as_str())) {
            entries.push(ResolvedRailEntry {
                item: RailItemId::Plugin {
                    plugin_id: spec.plugin_id.clone(),
                    view_id: spec.view_id.clone(),
                },
                section: Section::Plugin {
                    plugin_id: spec.plugin_id.clone().into(),
                    view_id: spec.view_id.clone().into(),
                },
                title: spec.title.clone().into(),
                icon_svg: spec.icon_svg.clone(),
            });
        }
    }

    entries
}

/// Visible rail items in exact visual top-to-bottom order for hit-testing and reordering.
pub fn visible_rail_items(
    order: &[RailItemId],
    features: fern_core::Features,
    plugin_views: &[PluginSidebarViewSpec],
) -> Vec<RailItemId> {
    resolve_rail_entries(order, features, plugin_views, &i18n::EN)
        .into_iter()
        .map(|entry| entry.item)
        .collect()
}

pub use desktop_common::layout::SIDEBAR_WIDTH as WIDTH;

/// Height of one rail icon slot (`40 px` icon + `5 px` vertical padding on each side, zero dead gap),
/// matching TickTick's `px-[4px] py-[5px]` + `w-[40px] h-[40px]` rail entries.
pub const SLOT_HEIGHT: f32 = 50.;

/// Distance from the top of the sidebar to the first rail icon slot (`avatar 32 + mb 7 + gap 4`).
pub const SECTIONS_TOP_OFFSET: f32 = 43.;

type ClickCallback = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The notification bell: a red badge with the unread count, as in the original.
pub struct Bell {
    pub unread: usize,
    pub on_click: ClickCallback,
    /// The notification list or the update notice attached to the bell.
    pub menu: Option<AnyElement>,
}

/// The sync button: its tooltip says how the last sync went; a failed one shows in red. Fern
/// works locally until a server is set, and until then there is no button.
pub struct SyncButton {
    pub tooltip: String,
    pub failed: bool,
    pub on_click: ClickCallback,
}

/// Optional signed-in Fern Cloud avatar appearance for the top-left account tile.
#[derive(Clone, Debug, Default)]
pub struct AvatarBadge {
    pub initials: Option<String>,
    pub status_dot: Option<Hsla>,
    pub tooltip: Option<String>,
}

/// `account_menu` is rendered next to the avatar while the account menu is open.
#[allow(clippy::too_many_arguments)]
pub fn render(
    active: &Section,
    hovered: Option<&Section>,
    order: &[RailItemId],
    dragging: Option<&Section>,
    rail_scroll: &ScrollHandle,
    on_hover: impl Fn(Section, bool, &mut App) + Clone + 'static,
    on_select: impl Fn(Section, &mut Window, &mut App) + Clone + 'static,
    on_start_drag: impl Fn(Section, &mut App) + Clone + 'static,
    on_avatar: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    avatar_badge: AvatarBadge,
    account_menu: Option<AnyElement>,
    bell: Bell,
    sync: Option<SyncButton>,
    focus_clock: Option<(String, bool)>,
    features: fern_core::Features,
    window: &Window,
    cx: &mut App,
) -> AnyElement {
    let plugin_views = cx
        .try_global::<PluginBus>()
        .map(|bus| bus.sidebar_views.clone())
        .unwrap_or_default();
    let theme = cx.theme().clone();
    let colors = theme.colors();
    let error_color = cx.theme().status().error;
    let on_error_color = cx.theme().status().on_error;
    let strings = i18n::t(cx);
    let active_drag = cx.has_active_drag();
    let dragging = dragging.filter(|_| active_drag);
    let button = |id: &'static str, icon: IconName| {
        IconButton::new(id, icon)
            .size(px(36.))
            .icon_size(IconSize::XLarge)
            .icon_color(colors.sidebar_icon_bottom)
            .hover_icon_color(colors.sidebar_icon_hover)
            .hover_color(gpui::transparent_black())
    };
    let entries = resolve_rail_entries(order, features, &plugin_views, strings);
    let mut rail_rows: Vec<AnyElement> = Vec::with_capacity(entries.len());
    for entry in entries {
        let section = entry.section;
        let is_active = &section == active;
        let is_hovered = hovered == Some(&section) && !active_drag;
        let is_dragged = dragging == Some(&section);
        let icon_color = if is_active {
            colors.sidebar_icon
        } else if is_hovered {
            colors.sidebar_icon_hover
        } else {
            colors.sidebar_icon_muted
        };
        let custom_img = entry
            .icon_svg
            .as_deref()
            .filter(|_| !is_dragged)
            .and_then(|svg| rasterize_rail_svg(svg, icon_color, cx));
        let tooltip_el = is_hovered.then(|| side_tooltip(entry.title, section == Section::Things, cx));
        let on_select = on_select.clone();
        let on_hover = on_hover.clone();
        let on_start_drag = on_start_drag.clone();
        let section_id = section.id();
        let section_id_dbg = section_id.clone();
        let payload = DraggedRailItem {
            item: entry.item,
            icon: section.icon(),
            icon_svg: entry.icon_svg.clone(),
            active: is_active,
        };
        let hover_section = section.clone();
        let click_section = section.clone();
        let drag_section = section.clone();
        // Each entry occupies a contiguous 50 px slot (40 px icon + 5 px top/bottom)
        // with zero dead gaps; while dragged, it leaves a vacant slot as neighbors slide.
        let slot = div()
            .id(section_id)
            .debug_selector(move || section_id_dbg.to_string())
            .relative()
            .flex_none()
            .w(px(48.))
            .h(px(SLOT_HEIGHT))
            .on_hover(move |hovered: &bool, _, cx| {
                on_hover(hover_section.clone(), *hovered, cx)
            })
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_click(move |_, window, cx| on_select(click_section.clone(), window, cx))
            .on_drag(payload, move |dragged, _, _, cx| {
                on_start_drag(drag_section.clone(), cx);
                cx.new(|_| dragged.clone())
            })
            .child(if is_dragged {
                div()
                    .size(px(40.))
                    .rounded(px(8.))
                    .bg(colors.sidebar_icon_muted.opacity(0.10))
                    .into_any_element()
            } else if section == Section::Pomodoro
                && let Some((clock, paused)) = &focus_clock
            {
                let pomo_color = if is_active || !*paused {
                    colors.sidebar_icon
                } else {
                    colors.sidebar_icon_muted
                };
                v_flex()
                    .size(px(40.))
                    .items_center()
                    .justify_center()
                    .gap(px(1.))
                    .child(
                        Icon::new(section.icon())
                            .size(IconSize::Custom(px(20.)))
                            .color(pomo_color),
                    )
                    .child(
                        div()
                            .px(px(3.))
                            .rounded(px(3.))
                            .bg(colors.sidebar_icon.opacity(if *paused { 0.14 } else { 0.22 }))
                            .text_size(px(9.))
                            .line_height(px(11.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(if *paused {
                                colors.sidebar_icon_muted
                            } else {
                                colors.sidebar_icon
                            })
                            .child(clock.clone()),
                    )
                    .into_any_element()
            } else if let Some(img) = custom_img {
                let alpha = icon_color.to_rgb().a;
                let mut el = gpui::img(img).size(px(24.));
                if alpha < 0.999 {
                    el = el.opacity(alpha);
                }
                div()
                    .size(px(40.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(el)
                    .into_any_element()
            } else {
                div()
                    .size(px(40.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(section.icon())
                            .size(IconSize::Custom(px(24.)))
                            .color(icon_color),
                    )
                    .into_any_element()
            })
            .children(tooltip_el)
            .into_any_element();
        rail_rows.push(slot);
    }

    v_flex()
        .debug_selector(|| "sidebar".to_owned())
        .flex_none()
        .w(px(WIDTH))
        .h_full()
        .items_center()
        .pt(crate::title_bar::height(window, cx))
        .pb_3()
        .gap_1()
        .bg(colors.sidebar_background)
        .border_r_1()
        .border_color(colors.sidebar_border)
        .child(
            div()
                .id("avatar")
                .relative()
                .flex_none()
                .mb(px(7.))
                .size(px(32.))
                .rounded(px(6.))
                .bg(if avatar_badge.initials.is_some() {
                    colors.accent.opacity(0.20)
                } else {
                    colors.avatar_background
                })
                .when(avatar_badge.initials.is_some(), |el| {
                    el.border_1().border_color(colors.accent.opacity(0.45))
                })
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .when(account_menu.is_none(), |el| {
                    let tip = avatar_badge
                        .tooltip
                        .unwrap_or_else(|| strings.settings.to_owned());
                    el.tooltip(Tooltip::text(tip))
                })
                .on_click(on_avatar)
                .child(if let Some(initials) = avatar_badge.initials {
                    div()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(colors.sidebar_icon)
                        .child(initials)
                        .into_any_element()
                } else {
                    Icon::new(IconName::User)
                        .color(colors.avatar_icon)
                        .into_any_element()
                })
                .when_some(avatar_badge.status_dot, |el, dot| {
                    el.child(
                        div()
                            .absolute()
                            .bottom(px(-2.))
                            .right(px(-2.))
                            .size(px(9.))
                            .rounded_full()
                            .bg(dot)
                            .border_1()
                            .border_color(colors.sidebar_border),
                    )
                })
                .children(account_menu),
        )
        // Scroll the section icons in a short window so the controls below stay reachable.
        .child(
            v_scroll(
                "sidebar-sections-scroll",
                v_flex()
                    .id("sidebar-sections")
                    .items_center()
                    .children(rail_rows),
            )
            .flex_1()
            .min_h_0()
            .w_full()
            .track_scroll(rail_scroll)
            .color(colors.sidebar_icon)
            .inset(px(1.))
            .compact(),
        )
        .children(sync.map(|sync| {
            button("sync", IconName::RefreshCw)
                .when(sync.failed, |b| b.icon_color(Color::Error).hover_icon_color(Color::Error))
                .tooltip(Tooltip::text(sync.tooltip))
                .on_click(sync.on_click)
        }))
        .child(
            div()
                .id("notification-bell")
                .debug_selector(|| "notification-bell".to_owned())
                .relative()
                .flex_none()
                .child(button("notifications", IconName::Bell).tooltip(Tooltip::text(strings.notifications)).on_click(bell.on_click))
                .when(bell.unread > 0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .right(px(2.))
                            .min_w(px(14.))
                            .h(px(14.))
                            .px(px(3.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(error_color)
                            .text_color(on_error_color)
                            .text_size(px(9.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(if bell.unread > 99 { "99+".to_owned() } else { bell.unread.to_string() }),
                    )
                })
                .children(bell.menu),
        )
        .child(
            button("help", IconName::CircleQuestionMark)
                .tooltip(Tooltip::text(strings.shortcuts))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::actions::ShowShortcuts), cx)),
        )
        .into_any_element()
}

/// A tooltip at the right of a sidebar icon, level with it, shown while it is hovered, as the
/// original's sidebar shows them.
fn side_tooltip(text: impl Into<SharedString>, beta: bool, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let colors = theme.colors();
    let warning = theme.status().warning;
    let beta_label = i18n::t(cx).beta_badge;
    div().absolute().left(px(46.)).top_0().h_full().flex().items_center().child(gpui::deferred(
        h_flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .text_color(colors.text)
            .text_size(px(12.))
            .whitespace_nowrap()
            .child(text.into())
            .when(beta, |el| el.child(Chip::new(beta_label, warning))),
    ))
}

fn rasterize_rail_svg(
    raw_svg: &str,
    color: Hsla,
    cx: &mut App,
) -> Option<Arc<RenderImage>> {
    let ink = color.to_rgb();
    let r = (ink.r.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (ink.g.clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (ink.b.clamp(0.0, 1.0) * 255.0).round() as u8;
    let ink_rgb = u32::from_be_bytes([r, g, b, 0xFF]);

    // Key the cache on the raw SVG + resolved ink BEFORE sanitizing so per-frame renders never
    // repeat sanitization/allocations, and negative-cache invalid SVGs as `None`.
    let key = (fnv1a_64(raw_svg.as_bytes()), ink_rgb);
    if let Some(cached) = cx.try_global::<RailSvgCache>().and_then(|c| c.0.get(&key)).cloned() {
        return cached;
    }

    let render_img = plugin_ui::rasterize_svg(raw_svg, RAIL_ICON_PX, RAIL_ICON_PX, (r, g, b), cx);
    if cx.try_global::<RailSvgCache>().is_none() {
        cx.set_global(RailSvgCache::default());
    }
    cx.update_global::<RailSvgCache, _>(|cache, _| {
        if cache.0.len() >= MAX_RAIL_SVG_CACHE {
            cache.0.clear();
        }
        cache.0.insert(key, render_img.clone());
    });
    render_img
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_rail_items_filters_frontends_and_merges_active_plugins() {
        let mut features = fern_core::Features {
            tasks: true,
            things: false,
            ..Default::default()
        };

        let order = vec![
            RailItemId::BuiltIn(RailSection::Tasks),
            RailItemId::BuiltIn(RailSection::Things),
            RailItemId::Plugin {
                plugin_id: "disabled-plugin".into(),
                view_id: "tab".into(),
            },
            RailItemId::Plugin {
                plugin_id: "active-plugin".into(),
                view_id: "board".into(),
            },
            RailItemId::BuiltIn(RailSection::Calendar),
        ];
        let active_plugins = vec![
            PluginSidebarViewSpec {
                plugin_id: "active-plugin".into(),
                view_id: "board".into(),
                title: "Board".into(),
                icon_svg: None,
            },
            PluginSidebarViewSpec {
                plugin_id: "new-plugin".into(),
                view_id: "home".into(),
                title: "Home".into(),
                icon_svg: None,
            },
        ];

        let visible = visible_rail_items(&order, features, &active_plugins);
        assert!(visible.contains(&RailItemId::BuiltIn(RailSection::Tasks)));
        assert!(!visible.contains(&RailItemId::BuiltIn(RailSection::Things)));
        assert!(!visible.contains(&RailItemId::Plugin {
            plugin_id: "disabled-plugin".into(),
            view_id: "tab".into(),
        }));
        assert!(visible.contains(&RailItemId::Plugin {
            plugin_id: "active-plugin".into(),
            view_id: "board".into(),
        }));
        assert_eq!(
            visible.last(),
            Some(&RailItemId::Plugin {
                plugin_id: "new-plugin".into(),
                view_id: "home".into(),
            })
        );

        // Switching to Things only hides Tasks and shows Things.
        features.tasks = false;
        features.things = true;
        let visible_things = visible_rail_items(&order, features, &active_plugins);
        assert!(!visible_things.contains(&RailItemId::BuiltIn(RailSection::Tasks)));
        assert!(visible_things.contains(&RailItemId::BuiltIn(RailSection::Things)));
    }

    #[test]
    fn plugin_section_ids_do_not_collide_on_hyphens() {
        let a = Section::Plugin {
            plugin_id: "a-b".into(),
            view_id: "c".into(),
        };
        let b = Section::Plugin {
            plugin_id: "a".into(),
            view_id: "b-c".into(),
        };
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn usvg_safe_options_disables_external_and_data_image_resolvers() {
        let opts = plugin_ui::safe_usvg_options();
        assert!((opts.image_href_resolver.resolve_string)("C:\\Windows\\win.ini", &opts).is_none());
        assert!((opts.image_href_resolver.resolve_string)("\\\\attacker\\share\\a.png", &opts).is_none());
        assert!((opts.image_href_resolver.resolve_data)("image/svg+xml", Arc::new(Vec::new()), &opts).is_none());
    }

    #[gpui::test]
    fn rail_svg_cache_negative_caches_invalid_svg_and_caches_valid_svg(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            let ink = cx.theme().colors().sidebar_icon;
            let invalid = "not-valid-svg";
            assert!(rasterize_rail_svg(invalid, ink, cx).is_none());
            // Negative entry must be cached so subsequent frames skip sanitization/parsing.
            let cache_len_after_invalid = cx.global::<RailSvgCache>().0.len();
            assert_eq!(cache_len_after_invalid, 1);
            assert!(rasterize_rail_svg(invalid, ink, cx).is_none());
            assert_eq!(cx.global::<RailSvgCache>().0.len(), 1);

            let valid = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8" fill="currentColor"/></svg>"#;
            let first = rasterize_rail_svg(valid, ink, cx).expect("valid SVG rasterizes");
            let second = rasterize_rail_svg(valid, ink, cx).expect("cached SVG hits");
            assert!(Arc::ptr_eq(&first, &second), "second frame must hit RailSvgCache without re-rasterizing");
        });
    }
}
