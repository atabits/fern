use super::*;
use desktop_common::sticky_settings::{PALETTE, StickySettings};

impl SettingsWindow {
    pub(super) fn render_sticky(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let settings = StickySettings::current(&self.store, cx);
        let group = || v_flex().py_1p5().rounded(px(8.)).bg(colors.surface_background);
        let row = |label: &'static str| h_flex().min_h(px(40.)).px_4().gap_2()
            .child(div().flex_1().child(Label::new(label)));
        let palette = h_flex().gap_2().flex_wrap().children(PALETTE.iter().enumerate().map(|(index, (background, _))| {
            let selected = settings.color == index;
            div().id(SharedString::from(format!("sticky-color-{index}")))
                .size(px(24.)).rounded_full().bg(gpui::rgb(*background))
                .border_2().border_color(if selected { colors.accent } else { colors.border })
                .when(!selected, |el| el.hover(|s| s.border_color(colors.text_muted)))
                .cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.update_sticky(cx, |s| s.color = index)))
        }));
        let opacity_at_min = settings.opacity <= 10;
        let opacity_at_max = settings.opacity >= 100;
        let font_at_min = settings.font == 0;
        let font_at_max = settings.font >= 3;
        v_flex().gap_4()
            .child(Label::new(strings.sticky_notes).weight(FontWeight::MEDIUM))
            .child(
                group()
                    .child(row(strings.color).child(div().max_w(px(208.)).py_2().child(palette)))
                    .child(row(strings.sticky_opacity)
                        .child(div().when(opacity_at_min, |el| el.opacity(0.35)).child(
                            IconButton::new("sticky-opacity-less", IconName::Minus)
                                .when(!opacity_at_min, |btn| btn.on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.opacity = s.opacity.saturating_sub(10).max(10)))))
                        ))
                        .child(div().min_w(px(40.)).flex().justify_center().child(Label::new(format!("{}%", settings.opacity))))
                        .child(div().when(opacity_at_max, |el| el.opacity(0.35)).child(
                            IconButton::new("sticky-opacity-more", IconName::Plus)
                                .when(!opacity_at_max, |btn| btn.on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.opacity = (s.opacity + 10).min(100)))))
                        )))
                    .child(row(strings.sticky_font_size)
                        .child(div().when(font_at_min, |el| el.opacity(0.35)).child(
                            IconButton::new("sticky-font-less", IconName::Minus)
                                .when(!font_at_min, |btn| btn.on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.font = s.font.saturating_sub(1)))))
                        ))
                        .child(div().min_w(px(48.)).flex().justify_center().child(Label::new(strings.sticky_font_sizes[settings.font])))
                        .child(div().when(font_at_max, |el| el.opacity(0.35)).child(
                            IconButton::new("sticky-font-more", IconName::Plus)
                                .when(!font_at_max, |btn| btn.on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.font = (s.font + 1).min(3)))))
                        ))),
            )
            .child(
                group()
                    .child(row(strings.sticky_pin_default).id("sticky-pin-default").cursor_pointer()
                        .hover(|s| s.bg(colors.ghost_element_hover))
                        .child(Switch::new().toggle_state(settings.pin_default))
                        .on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.pin_default = !s.pin_default))))
                    .when(cfg!(any(windows, target_os = "linux")), |panel| panel.child(v_flex()
                        .child(row(strings.sticky_hide_from_taskbar).id("sticky-hide-from-taskbar").cursor_pointer()
                            .hover(|s| s.bg(colors.ghost_element_hover))
                            .child(Switch::new().toggle_state(settings.hide_from_taskbar))
                            .on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.hide_from_taskbar = !s.hide_from_taskbar))))
                        .when(cfg!(windows), |section| {
                            section.child(div().px_4().pb_2().child(Label::new(strings.sticky_task_view_hint).size(LabelSize::Small).color(Color::Muted)))
                        }))),
            )
            .children(self.sticky_error.clone().map(|message| Label::new(message).color(Color::Error)))
            .into_any_element()
    }

    fn update_sticky(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut StickySettings)) {
        let mut settings = StickySettings::current(&self.store, cx);
        change(&mut settings);
        self.sticky_error = match settings.save(&self.store) {
            Ok(()) => {
                cx.set_global(settings);
                None
            }
            Err(err) => {
                eprintln!("failed to save sticky note settings: {err:#}");
                Some(i18n::t(cx).sticky_save_failed.to_owned())
            }
        };
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn saving_note_preferences_leaves_task_data_untouched(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = Rc::new(Store::open_in_memory().unwrap());
            let settings = cx.new(|cx| SettingsWindow::new(store.clone(), cx));
            settings.update(cx, |settings, cx| settings.update_sticky(cx, |prefs| prefs.font = 3));
            let saved = StickySettings::load(&store);
            assert_eq!(saved.font, 3);
            assert_eq!(*cx.global::<StickySettings>(), saved);
            assert!(cx.try_global::<desktop_common::data_version::DataVersion>().is_none());
        });
    }
}
