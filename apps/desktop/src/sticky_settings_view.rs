use super::*;
use crate::sticky_settings::{PALETTE, StickySettings};

impl SettingsWindow {
    pub(super) fn render_sticky(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let settings = StickySettings::load(&self.store);
        let row = |label: &'static str| h_flex().min_h(px(40.)).px_4().gap_2()
            .child(div().flex_1().child(Label::new(label)));
        let palette = h_flex().gap_2().flex_wrap().children(PALETTE.iter().enumerate().map(|(index, (background, _))| {
            div().id(SharedString::from(format!("sticky-color-{index}")))
                .size(px(24.)).rounded_full().bg(gpui::rgb(*background))
                .border_2().border_color(if settings.color == index { cx.theme().colors().accent } else { cx.theme().colors().border })
                .cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.update_sticky(cx, |s| s.color = index)))
        }));
        v_flex().gap_4()
            .child(Label::new(strings.sticky_notes).weight(FontWeight::MEDIUM))
            .child(row(strings.color).child(div().max_w(px(208.)).py_2().child(palette)))
            .when(cfg!(windows), |panel| panel.child(row(strings.sticky_opacity)
                .child(IconButton::new("sticky-opacity-less", IconName::Minus).on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.opacity = s.opacity.saturating_sub(10).max(10)))))
                .child(Label::new(format!("{}%", settings.opacity)))
                .child(IconButton::new("sticky-opacity-more", IconName::Plus).on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.opacity = (s.opacity + 10).min(100)))))))
            .child(row(strings.sticky_font_size)
                .child(IconButton::new("sticky-font-less", IconName::Minus).on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.font = s.font.saturating_sub(1)))))
                .child(Label::new(strings.sticky_font_sizes[settings.font]))
                .child(IconButton::new("sticky-font-more", IconName::Plus).on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.font = (s.font + 1).min(3))))))
            .when(cfg!(windows), |panel| panel.child(row(strings.sticky_pin_default).id("sticky-pin-default").cursor_pointer()
                .child(Switch::new().toggle_state(settings.pin_default))
                .on_click(cx.listener(|this, _, _, cx| this.update_sticky(cx, |s| s.pin_default = !s.pin_default)))))
            .children(self.backup_message.clone().map(|message| Label::new(message).color(Color::Error)))
            .into_any_element()
    }

    fn update_sticky(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut StickySettings)) {
        let mut settings = StickySettings::load(&self.store);
        change(&mut settings);
        self.backup_message = settings.save(&self.store).err().map(|err| {
            eprintln!("failed to save sticky note settings: {err:#}");
            i18n::t(cx).sticky_save_failed.to_owned()
        });
        crate::sticky_note::data_changed(cx.entity_id(), cx);
        cx.notify();
    }
}
