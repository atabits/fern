//! The calendar's View Options dialog, as the original's: Color and Style pages, then switches
//! for weekends, completed tasks, repeat cycles and countdowns.

use std::rc::Rc;

use fern_core::{CalendarOptions, ColorBy, CountdownColor, HabitColor, Priority, Store};
use gpui::{EventEmitter, FontWeight, Hsla};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, Switch, popover};

use crate::i18n::{self, Strings};
use crate::lists_panel::parse_color;
use crate::task_rows;

/// The original's dialog width.
const WIDTH: f32 = 440.;

/// The fixed colors the Color page offers for habits and countdowns, as the original's: orange,
/// lime, green, teal, blue, purple, pink and red.
pub const PALETTE: [u32; 8] = [0xf5a54a, 0xb5cf2f, 0x76c95a, 0x4bbfae, 0x6b9cf2, 0xae8cf0, 0xee7fb6, 0xee6458];

pub fn palette(index: u8) -> gpui::Hsla {
    gpui::rgb(PALETTE[index as usize % PALETTE.len()]).into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Main,
    Color,
    Style,
}

pub enum OptionsEvent {
    Changed(CalendarOptions),
    Closed,
}

pub struct CalendarOptionsDialog {
    store: Rc<Store>,
    options: CalendarOptions,
    page: Page,
    color_menu: bool,
    /// The Color page's tab: Task, Habit or Countdown.
    color_tab: usize,
}

impl EventEmitter<OptionsEvent> for CalendarOptionsDialog {}

impl CalendarOptionsDialog {
    pub fn new(store: Rc<Store>, page: Page) -> Self {
        let options = store.calendar_options().unwrap_or_default();
        Self { store, options, page, color_menu: false, color_tab: 0 }
    }

    /// Opens the Color By dropdown, for the debug-build dev state.
    #[cfg(debug_assertions)]
    pub fn dev_color_tab(&mut self, tab: usize) {
        self.color_tab = tab;
    }

    #[cfg(debug_assertions)]
    pub fn dev_color_menu(&mut self) {
        self.color_menu = true;
    }

    fn change(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut CalendarOptions)) {
        change(&mut self.options);
        if let Err(err) = self.store.save_calendar_options(&self.options) {
            eprintln!("failed to save the calendar options: {err:#}");
        }
        cx.emit(OptionsEvent::Changed(self.options));
        cx.notify();
    }

    fn header(&self, title: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .child(div().flex_1().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(IconButton::new("calendar-options-x", IconName::X).icon_color(Color::Muted).on_click(cx.listener(|_, _, _, cx| cx.emit(OptionsEvent::Closed))))
    }

    fn render_main(&self, strings: &'static Strings, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let group = || v_flex().py_1().rounded(px(8.)).bg(theme.colors().element_selected);
        let page_row = |id: &'static str, label: &'static str, page: Page, cx: &mut Context<Self>| {
            h_flex()
                .id(id)
                .h(px(40.))
                .px_4()
                .cursor_pointer()
                .child(div().flex_1().child(Label::new(label)))
                .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.page = page;
                    cx.notify();
                }))
        };
        let switch = |id: &'static str, label: &'static str, on: bool, flip: fn(&mut CalendarOptions), cx: &mut Context<Self>| {
            h_flex()
                .id(id)
                .h(px(40.))
                .px_4()
                .cursor_pointer()
                .child(div().flex_1().child(Label::new(label)))
                .child(Switch::new().toggle_state(on))
                .on_click(cx.listener(move |this, _, _, cx| this.change(cx, flip)))
        };
        let o = self.options;
        v_flex()
            .gap_3()
            .child(self.header(strings.view_options, cx))
            .child(
                group()
                    .child(page_row("calendar-color", strings.calendar_color, Page::Color, cx))
                    .child(page_row("calendar-style", strings.calendar_style, Page::Style, cx))
                    .child(switch("calendar-weekends", strings.show_weekends, o.show_weekends, |o| o.show_weekends = !o.show_weekends, cx)),
            )
            .child(
                group()
                    .child(switch("calendar-completed", strings.show_completed, o.show_completed, |o| o.show_completed = !o.show_completed, cx))
                    .child(switch("calendar-repeats", strings.show_all_repeats, o.show_repeats, |o| o.show_repeats = !o.show_repeats, cx)),
            )
            .child(
                group()
                    .child(switch("calendar-habits", strings.show_habits, o.show_habits, |o| o.show_habits = !o.show_habits, cx))
                    .child(switch("calendar-focus", strings.show_focus, o.show_focus, |o| o.show_focus = !o.show_focus, cx))
                    .child(switch("calendar-countdowns", strings.show_countdowns, o.show_countdowns, |o| o.show_countdowns = !o.show_countdowns, cx)),
            )
    }

    /// The Color page, as the original's: Task, Habit and Countdown tabs, each with its "Color By",
    /// what it means, and the colors it takes.
    fn render_color(&self, strings: &'static Strings, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let accent = theme.colors().accent;
        let o = self.options;
        let swatch = |color: Hsla| div().size(px(14.)).rounded_full().bg(color);
        let row = |icon: IconName, label: SharedString, color: Hsla| {
            h_flex()
                .h(px(40.))
                .px_4()
                .gap_2()
                .child(Icon::new(icon).size(IconSize::Small).color(Color::Muted))
                .child(div().flex_1().child(Label::new(label).truncate()))
                .child(swatch(color))
        };
        // The tab's choices, the chosen one, what it means, and what follows it.
        let (choices, chosen, hint, rows): (Vec<&'static str>, usize, Option<&'static str>, Vec<AnyElement>) = match self.color_tab {
            0 => {
                let index = ColorBy::ALL.iter().position(|c| *c == o.color_by).unwrap_or(0);
                let rows = match o.color_by {
                    ColorBy::List => std::iter::once(row(IconName::Inbox, strings.inbox.into(), accent))
                        .chain(self.store.lists().unwrap_or_default().into_iter().filter(|l| !l.archived).map(|l| {
                            row(IconName::List, l.name.clone().into(), parse_color(l.color.as_deref()).unwrap_or(accent))
                        }))
                        .map(IntoElement::into_any_element)
                        .collect(),
                    ColorBy::Tag => self
                        .store
                        .tags()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|t| row(IconName::Tag, t.name.clone().into(), parse_color(t.color.as_deref()).unwrap_or(accent)).into_any_element())
                        .collect(),
                    ColorBy::Priority => [Priority::High, Priority::Medium, Priority::Low, Priority::None]
                        .into_iter()
                        .zip([strings.priority_high, strings.priority_medium, strings.priority_low, strings.priority_none])
                        .map(|(p, label)| row(IconName::Flag, label.into(), task_rows::priority_color(p, &theme).unwrap_or(accent)).into_any_element())
                        .collect(),
                };
                (strings.color_by_options.to_vec(), index, Some(strings.color_by_hints[index]), rows)
            }
            1 => {
                let index = match o.habit_color {
                    HabitColor::Theme => 0,
                    HabitColor::Fixed => 1,
                    HabitColor::Habit => 2,
                };
                let hint = match o.habit_color {
                    HabitColor::Theme => None,
                    HabitColor::Fixed => Some(strings.habit_fixed_hint),
                    HabitColor::Habit => Some(strings.habit_own_hint),
                };
                let rows = if o.habit_color == HabitColor::Fixed { vec![self.palette_row(o.habit_fixed, true, strings, cx)] } else { Vec::new() };
                (strings.habit_color_options.to_vec(), index, hint, rows)
            }
            _ => {
                let fixed = o.countdown_color == CountdownColor::Fixed;
                let rows = if fixed { vec![self.palette_row(o.countdown_fixed, false, strings, cx)] } else { Vec::new() };
                (strings.countdown_color_options.to_vec(), usize::from(!fixed), Some(if fixed { strings.countdown_fixed_hint } else { strings.countdown_own_hint }), rows)
            }
        };
        let tab = self.color_tab;
        let menu = self.color_menu.then(|| {
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .w(px(162.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.color_menu = false;
                        cx.notify();
                    }))
                    .children(choices.iter().enumerate().map(|(i, label)| {
                        let on = i == chosen;
                        ListItem::new(SharedString::from(format!("color-by-{tab}-{i}")))
                            .height(px(34.))
                            .child(Label::new(*label).color(if on { Color::Accent } else { Color::Default }))
                            .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.color_menu = false;
                                this.change(cx, |o| match tab {
                                    0 => o.color_by = ColorBy::ALL[i],
                                    1 => o.habit_color = [HabitColor::Theme, HabitColor::Fixed, HabitColor::Habit][i],
                                    _ => o.countdown_color = [CountdownColor::Fixed, CountdownColor::Countdown][i],
                                });
                            }))
                    })),
            )
        });
        let tabs = h_flex().flex_1().justify_center().gap_2().children(strings.color_tabs.iter().enumerate().map(|(i, label)| {
            let on = i == tab;
            div()
                .id(SharedString::from(format!("color-tab-{i}")))
                .px_3()
                .py_1()
                .rounded_full()
                .cursor_pointer()
                .bg(if on { accent.opacity(0.15) } else { theme.colors().element_selected })
                .child(Label::new(*label).color(if on { Color::Accent } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.color_tab = i;
                    this.color_menu = false;
                    cx.notify();
                }))
        }));
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .child(div().w(px(26.)))
                    .child(tabs)
                    .child(IconButton::new("calendar-options-x", IconName::X).icon_color(Color::Muted).on_click(cx.listener(|_, _, _, cx| cx.emit(OptionsEvent::Closed)))),
            )
            .child(
                h_flex()
                    .h(px(48.))
                    .px_4()
                    .rounded(px(8.))
                    .bg(theme.colors().element_selected)
                    .child(div().flex_1().child(Label::new(strings.color_by)))
                    .child(
                        div().relative().child(
                            h_flex()
                                .id("color-by")
                                .gap_1()
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .when(self.color_menu, |el| el.bg(theme.colors().element_hover))
                                .child(Label::new(choices[chosen]).color(Color::Muted))
                                .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.color_menu = !this.color_menu;
                                    cx.notify();
                                })),
                        )
                        .children(menu),
                    ),
            )
            .when_some(hint, |el, hint| el.child(div().px_4().child(Label::new(hint).size(LabelSize::Small).color(Color::Muted))))
            .when(!rows.is_empty(), |el| {
                el.child(v_flex().id("color-rows").max_h(px(320.)).overflow_y_scroll().py_1().rounded(px(8.)).bg(theme.colors().element_selected).children(rows))
            })
    }

    /// The "Color" row of fixed colors, the chosen one ringed.
    fn palette_row(&self, chosen: u8, habit: bool, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .h(px(48.))
            .px_4()
            .gap_2()
            .child(div().flex_1().child(Label::new(strings.calendar_color)))
            .children((0..PALETTE.len() as u8).map(|i| {
                let color = palette(i);
                div()
                    .id(SharedString::from(format!("palette-{habit}-{i}")))
                    .size(px(20.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(i == chosen, |el| el.border_2().border_color(color))
                    .child(div().size(px(if i == chosen { 12. } else { 18. })).rounded_full().bg(color))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change(cx, |o| if habit { o.habit_fixed = i } else { o.countdown_fixed = i })
                    }))
            }))
            .into_any_element()
    }

    /// The Style page: Item Style (Modern or Classic, each with a sketch) and the Year view's
    /// Heatmap.
    fn render_style(&self, strings: &'static Strings, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let accent = theme.colors().accent;
        let radio = |on: bool, label: &'static str| {
            h_flex()
                .gap_1p5()
                .child(
                    div()
                        .size(px(16.))
                        .rounded_full()
                        .border_1()
                        .border_color(if on { accent } else { theme.colors().border })
                        .when(on, |el| el.bg(accent).flex().items_center().justify_center().child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Custom(gpui::white())))),
                )
                .child(Label::new(label))
        };
        let item = |modern: bool| {
            let card = div()
                .w(px(100.))
                .h(px(44.))
                .px_1p5()
                .py_1()
                .rounded(px(3.))
                .child(Label::new(strings.preview_task).size(LabelSize::XSmall))
                .child(Label::new("09:00-10:00").size(LabelSize::XSmall).color(Color::Muted));
            if modern { card.bg(accent.opacity(0.3)).border_l_2().border_color(accent) } else { card.bg(accent.opacity(0.65)) }
        };
        let hours = || v_flex().gap(px(16.)).children(["09:00", "10:00"].map(|h| Label::new(h).size(LabelSize::XSmall).color(Color::Muted)));
        let style_choice = |id: &'static str, modern: bool, label: &'static str, cx: &mut Context<Self>| {
            let on = self.options.modern == modern;
            v_flex()
                .id(id)
                .flex_1()
                .items_center()
                .gap_2()
                .py_3()
                .cursor_pointer()
                .child(h_flex().gap_2().items_start().child(hours()).child(item(modern)))
                .child(radio(on, label))
                .on_click(cx.listener(move |this, _, _, cx| this.change(cx, |o| o.modern = modern)))
        };
        let month = |shown: bool| {
            let cell = move |day: u32| {
                let hot = shown && day % 3 != 0;
                div()
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(hot, |el| el.bg(accent.opacity(0.45)))
                    .child(Label::new(day.to_string()).size(LabelSize::XSmall))
            };
            v_flex().children((0..4).map(move |week| h_flex().children((1..=7).map(move |d| cell(week * 7 + d)))))
        };
        let heat_choice = |id: &'static str, shown: bool, label: &'static str, cx: &mut Context<Self>| {
            let on = self.options.heatmap == shown;
            v_flex()
                .id(id)
                .flex_1()
                .items_center()
                .gap_2()
                .py_3()
                .cursor_pointer()
                .child(month(shown))
                .child(radio(on, label))
                .on_click(cx.listener(move |this, _, _, cx| this.change(cx, |o| o.heatmap = shown)))
        };
        let card = || h_flex().rounded(px(8.)).bg(theme.colors().element_selected);
        let heading = |title: &'static str, hint: &'static str| {
            v_flex().gap_0p5().child(Label::new(title).weight(FontWeight::SEMIBOLD)).child(Label::new(hint).size(LabelSize::Small).color(Color::Muted))
        };
        v_flex()
            .gap_3()
            .child(self.header(strings.calendar_style, cx))
            .child(heading(strings.item_style, strings.item_style_hint))
            .child(card().child(style_choice("style-modern", true, strings.style_modern, cx)).child(style_choice("style-classic", false, strings.style_classic, cx)))
            .child(heading(strings.heatmap, strings.heatmap_hint))
            .child(card().child(heat_choice("heatmap-hide", false, strings.heatmap_hide, cx)).child(heat_choice("heatmap-show", true, strings.heatmap_show, cx)))
    }
}

impl Render for CalendarOptionsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let body = match self.page {
            Page::Main => self.render_main(strings, cx),
            Page::Color => self.render_color(strings, cx),
            Page::Style => self.render_style(strings, cx),
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.35))
            .occlude()
            .id("calendar-options-backdrop")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(OptionsEvent::Closed)))
            .child(
                div()
                    .id("calendar-options")
                    .w(px(WIDTH))
                    .p_5()
                    .rounded(px(10.))
                    .bg(theme.colors().elevated_surface_background)
                    .border_1()
                    .border_color(theme.colors().border)
                    .shadow_lg()
                    // Clicks inside stay inside.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            cx.emit(OptionsEvent::Closed);
                        }
                    }))
                    .child(body),
            )
    }
}
