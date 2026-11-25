//! Dual-clipped GPUI canvas rendering the 50/50 split background and six continuously scrolling
//! time rows (`Second`, `Minute`, `Hour`, `Day`, `Month`, `Year`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{Datelike, Duration, Timelike};
use fern_core::life_clock::{TimelineSnapshot, TimelineUnit};
use fern_i18n::Locale;
use gpui::{
    App, Bounds, ContentMask, FontWeight, Hsla, Pixels, ShapedLine, SharedString, TextAlign,
    TextRun, Window, canvas, fill, point, px, rgb,
};
use ui::prelude::*;

use crate::LifeClockPaletteMode;

/// Resolved colors for the Life Clock canvas and chrome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LifeClockPalette {
    pub past_bg: Hsla,
    pub future_bg: Hsla,
    pub past_text: Hsla,
    pub future_text: Hsla,
    pub age_heading: Hsla,
    pub age_value: Hsla,
    pub quote_text: Hsla,
    pub quote_author: Hsla,
    pub help_bg: Hsla,
    pub help_fg: Hsla,
    pub rail_btn_fg: Hsla,
}

impl LifeClockPalette {
    pub fn resolve(mode: LifeClockPaletteMode, theme: &theme::Theme) -> Self {
        match mode {
            LifeClockPaletteMode::Classic => Self {
                past_bg: rgb(0x000000).into(),
                future_bg: rgb(0xFFFFFF).into(),
                past_text: rgb(0x333333).into(),
                future_text: rgb(0x262626).into(),
                age_heading: rgb(0xF2F2F2).into(),
                age_value: rgb(0x8C8C8C).into(),
                quote_text: rgb(0x1A1A1A).into(),
                quote_author: rgb(0x888888).into(),
                help_bg: rgb(0x1A1A1A).into(),
                help_fg: rgb(0xFFFFFF).into(),
                rail_btn_fg: rgb(0xFFFFFF).into(),
            },
            LifeClockPaletteMode::FernTheme => {
                let colors = theme.colors();
                if !theme.is_dark() {
                    Self {
                        past_bg: rgb(0x16161a).into(),
                        future_bg: colors.background,
                        past_text: rgb(0x3f3f48).into(),
                        future_text: colors.text,
                        age_heading: rgb(0xf4f4f5).into(),
                        age_value: rgb(0xa1a1aa).into(),
                        quote_text: colors.text,
                        quote_author: colors.text_muted,
                        help_bg: colors.text,
                        help_fg: colors.background,
                        rail_btn_fg: rgb(0xf4f4f5).into(),
                    }
                } else {
                    let past_bg: Hsla = rgb(0x09090c).into();
                    let future_bg = ensure_dark_split_contrast(past_bg, colors.elevated_surface_background);
                    Self {
                        past_bg,
                        future_bg,
                        past_text: colors.text_muted.opacity(0.32),
                        future_text: colors.text.opacity(0.90),
                        age_heading: colors.text,
                        age_value: colors.text_muted,
                        quote_text: colors.text,
                        quote_author: colors.text_muted,
                        help_bg: colors.element_hover,
                        help_fg: colors.text,
                        rail_btn_fg: colors.text,
                    }
                }
            }
        }
    }
}

/// Ensures the future half stays visibly lighter than `past_bg` even on near-black custom dark themes.
fn ensure_dark_split_contrast(past_bg: Hsla, future_bg: Hsla) -> Hsla {
    const MIN_LIGHTNESS_GAP: f32 = 0.08;
    let gap = (past_bg.l + MIN_LIGHTNESS_GAP) - future_bg.l;
    if gap > 0.0 {
        let lift = (gap * 1.4).clamp(0.06, 0.22);
        future_bg.blend(gpui::white().opacity(lift))
    } else {
        future_bg
    }
}

/// Compact `Copy` identifier for any timeline token so cache lookups allocate zero heap memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum TokenId {
    TwoDigit(u8),
    Month(u8),
    Year(i32),
    /// Representative calibration sample (`"88"` / `"8888"`) for width measurements.
    SampleTwoDigit88,
    SampleYear8888,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GlyphKey {
    row_idx: u8,
    locale_tag: u8,
    token: TokenId,
    font_size_tenths: u32,
    color_u32: u32,
}

#[derive(Default)]
pub(crate) struct TimelineCacheState {
    glyphs: HashMap<GlyphKey, Rc<ShapedLine>>,
    rep_widths_key: Option<(u32, u32, u8, i32)>,
    rep_widths: [f32; 6],
}

pub(crate) type GlyphCache = Rc<RefCell<TimelineCacheState>>;

const TWO_DIGIT_LABELS: [&str; 60] = [
    "00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13", "14",
    "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29",
    "30", "31", "32", "33", "34", "35", "36", "37", "38", "39", "40", "41", "42", "43", "44",
    "45", "46", "47", "48", "49", "50", "51", "52", "53", "54", "55", "56", "57", "58", "59",
];

fn locale_tag(locale: Locale) -> u8 {
    match locale {
        Locale::En => 0,
        Locale::Ru => 1,
        Locale::ZhCn => 2,
    }
}

fn token_id_for_offset(snapshot: TimelineSnapshot, unit: TimelineUnit, offset: i64) -> TokenId {
    match unit {
        TimelineUnit::Second => {
            TokenId::TwoDigit((snapshot.now.second() as i64 + offset).rem_euclid(60) as u8)
        }
        TimelineUnit::Minute => {
            TokenId::TwoDigit((snapshot.now.minute() as i64 + offset).rem_euclid(60) as u8)
        }
        TimelineUnit::Hour => {
            TokenId::TwoDigit((snapshot.now.hour() as i64 + offset).rem_euclid(24) as u8)
        }
        TimelineUnit::Day => {
            let day = snapshot
                .now
                .date()
                .checked_add_signed(Duration::days(offset))
                .map_or_else(
                    || (snapshot.now.day0() as i64 + offset).rem_euclid(31) as u8 + 1,
                    |d| d.day() as u8,
                );
            TokenId::TwoDigit(day.min(59))
        }
        TimelineUnit::Month => {
            TokenId::Month((snapshot.now.month0() as i64 + offset).rem_euclid(12) as u8)
        }
        TimelineUnit::Year => {
            let clamped = offset.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            TokenId::Year(snapshot.now.year().saturating_add(clamped))
        }
    }
}

fn label_for_token(token: TokenId, months_short: &[&'static str; 12]) -> SharedString {
    match token {
        TokenId::TwoDigit(n) => {
            let idx = usize::from(n.min(59));
            SharedString::new_static(TWO_DIGIT_LABELS[idx])
        }
        TokenId::Month(m) => {
            let idx = usize::from(m % 12);
            SharedString::new_static(months_short[idx])
        }
        TokenId::Year(y) => SharedString::from(y.to_string()),
        TokenId::SampleTwoDigit88 => SharedString::new_static("88"),
        TokenId::SampleYear8888 => SharedString::new_static("8888"),
    }
}

const MAX_GLYPH_CACHE_ENTRIES: usize = 1024;
const BOTTOM_MARGIN_PX: f32 = 18.0;
const HEADER_EXTRA_RESERVE_PX: f32 = 120.0;
const REFERENCE_WIDTH_PX: f32 = 1760.0;
const MIN_SCALE: f32 = 0.42;
const MAX_SCALE: f32 = 1.45;

#[derive(Clone, Copy, Debug)]
struct RowSpec {
    unit: TimelineUnit,
    font_size: f32,
    row_height: f32,
    base_pitch: f32,
    weight: FontWeight,
}

const ROW_SPECS: [RowSpec; 6] = [
    RowSpec {
        unit: TimelineUnit::Second,
        font_size: 11.5,
        row_height: 22.0,
        base_pitch: 18.5,
        weight: FontWeight::MEDIUM,
    },
    RowSpec {
        unit: TimelineUnit::Minute,
        font_size: 17.5,
        row_height: 32.0,
        base_pitch: 28.5,
        weight: FontWeight::MEDIUM,
    },
    RowSpec {
        unit: TimelineUnit::Hour,
        font_size: 28.0,
        row_height: 46.0,
        base_pitch: 45.5,
        weight: FontWeight::SEMIBOLD,
    },
    RowSpec {
        unit: TimelineUnit::Day,
        font_size: 45.0,
        row_height: 66.0,
        base_pitch: 72.0,
        weight: FontWeight::BOLD,
    },
    RowSpec {
        unit: TimelineUnit::Month,
        font_size: 74.0,
        row_height: 96.0,
        base_pitch: 148.0,
        weight: FontWeight::BOLD,
    },
    RowSpec {
        unit: TimelineUnit::Year,
        font_size: 116.0,
        row_height: 132.0,
        base_pitch: 300.0,
        weight: FontWeight::BOLD,
    },
];

const TOTAL_BASE_HEIGHT: f32 = 22.0 + 32.0 + 46.0 + 66.0 + 96.0 + 132.0;

#[derive(Clone, Copy, Debug)]
struct ComputedRow {
    row_idx: u8,
    spec: RowSpec,
    y: f32,
    row_h: f32,
    font_size: f32,
    cell_w: f32,
    frac: f32,
}

fn color_to_u32(color: Hsla) -> u32 {
    let rgba = color.to_rgb();
    let r = (rgba.r.clamp(0.0, 1.0) * 255.0).round() as u32;
    let g = (rgba.g.clamp(0.0, 1.0) * 255.0).round() as u32;
    let b = (rgba.b.clamp(0.0, 1.0) * 255.0).round() as u32;
    let a = (rgba.a.clamp(0.0, 1.0) * 255.0).round() as u32;
    (r << 24) | (g << 16) | (b << 8) | a
}

fn compute_scale(width: f32, height: f32, title_bar_h: f32) -> f32 {
    let header_reserve = title_bar_h + HEADER_EXTRA_RESERVE_PX;
    let avail_h = (height - header_reserve - BOTTOM_MARGIN_PX).max(1.0);
    let target_h = (height * 0.48).min(avail_h);
    let scale_h = target_h / TOTAL_BASE_HEIGHT;
    let scale_w = width / REFERENCE_WIDTH_PX;
    scale_h.min(scale_w).clamp(MIN_SCALE, MAX_SCALE)
}

struct TimelinePainter<'a> {
    snapshot: TimelineSnapshot,
    locale_tag: u8,
    months_short: &'a [&'static str; 12],
    cache: &'a GlyphCache,
}

impl TimelinePainter<'_> {
    fn shape(
        &self,
        row_idx: u8,
        token: TokenId,
        font_size: f32,
        weight: FontWeight,
        color: Hsla,
        window: &Window,
    ) -> Rc<ShapedLine> {
        let font_size_tenths = (font_size * 10.0).round().max(1.0) as u32;
        let color_u32 = color_to_u32(color);
        let key = GlyphKey {
            row_idx,
            locale_tag: if matches!(token, TokenId::Month(_)) {
                self.locale_tag
            } else {
                0
            },
            token,
            font_size_tenths,
            color_u32,
        };

        if let Some(existing) = self.cache.borrow().glyphs.get(&key) {
            return Rc::clone(existing);
        }

        let label = label_for_token(token, self.months_short);
        let mut font = window.text_style().font();
        font.weight = weight;
        let run = TextRun {
            len: label.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = Rc::new(window.text_system().shape_line(
            label,
            px(font_size_tenths as f32 / 10.0),
            &[run],
            None,
        ));

        let mut state = self.cache.borrow_mut();
        if state.glyphs.len() >= MAX_GLYPH_CACHE_ENTRIES {
            state.glyphs.clear();
        }
        state.glyphs.insert(key, Rc::clone(&shaped));
        shaped
    }

    fn representative_widths(
        &self,
        scale: f32,
        color: Hsla,
        window: &Window,
    ) -> [f32; 6] {
        let scale_key = (scale * 1000.0).round() as u32;
        let color_key = color_to_u32(color);
        let year = self.snapshot.now.year();
        let cache_key = (scale_key, color_key, self.locale_tag, year);

        {
            let state = self.cache.borrow();
            if state.rep_widths_key == Some(cache_key) {
                return state.rep_widths;
            }
        }

        let mut widths = [0.0_f32; 6];
        for (idx, spec) in ROW_SPECS.iter().copied().enumerate() {
            let row_idx = idx as u8;
            let font_size = spec.font_size * scale;
            widths[idx] = match spec.unit {
                TimelineUnit::Second
                | TimelineUnit::Minute
                | TimelineUnit::Hour
                | TimelineUnit::Day => {
                    let w0 = f32::from(
                        self.shape(row_idx, TokenId::TwoDigit(0), font_size, spec.weight, color, window)
                            .width(),
                    );
                    let w8 = f32::from(
                        self.shape(row_idx, TokenId::SampleTwoDigit88, font_size, spec.weight, color, window)
                            .width(),
                    );
                    w0.max(w8)
                }
                TimelineUnit::Month => (0_u8..12)
                    .map(|m| {
                        f32::from(
                            self.shape(row_idx, TokenId::Month(m), font_size, spec.weight, color, window)
                                .width(),
                        )
                    })
                    .fold(0.0_f32, f32::max),
                TimelineUnit::Year => {
                    let wy = f32::from(
                        self.shape(row_idx, TokenId::Year(year), font_size, spec.weight, color, window)
                            .width(),
                    );
                    let w8 = f32::from(
                        self.shape(row_idx, TokenId::SampleYear8888, font_size, spec.weight, color, window)
                            .width(),
                    );
                    wy.max(w8)
                }
            };
        }

        let mut state = self.cache.borrow_mut();
        state.rep_widths_key = Some(cache_key);
        state.rep_widths = widths;
        widths
    }

    fn paint_half(
        &self,
        rows: &[ComputedRow; 6],
        clip_range: (f32, f32),
        center_x: f32,
        color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (clip_left, clip_right) = clip_range;
        for row in rows {
            if row.cell_w <= 1.0 {
                continue;
            }
            let i_min = ((clip_left - center_x) / row.cell_w + row.frac).floor() as i64 - 1;
            let i_max = ((clip_right - center_x) / row.cell_w + row.frac).ceil() as i64 + 1;

            for offset in i_min..=i_max {
                let x_left = center_x + (offset as f32 - row.frac) * row.cell_w;
                if x_left + row.cell_w < clip_left || x_left > clip_right {
                    continue;
                }
                let token = token_id_for_offset(self.snapshot, row.spec.unit, offset);
                let line = self.shape(
                    row.row_idx,
                    token,
                    row.font_size,
                    row.spec.weight,
                    color,
                    window,
                );
                let glyph_w = f32::from(line.width());
                let token_x = x_left + (row.cell_w - glyph_w) * 0.5;
                let _ = line.paint(
                    point(px(token_x), px(row.y)),
                    px(row.row_h),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }
    }
}

pub(crate) fn render_timeline_canvas(
    snapshot: TimelineSnapshot,
    locale: Locale,
    months_short: [&'static str; 12],
    palette: LifeClockPalette,
    glyph_cache: GlyphCache,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let title_bar_h = f32::from(desktop_common::layout::title_bar_height(window, cx));
    let ltag = locale_tag(locale);
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, (), window: &mut Window, cx: &mut App| {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            if width <= 1.0 || height <= 1.0 {
                return;
            }

            let scale_factor = window.scale_factor().max(1.0);
            let left_edge = f32::from(bounds.left());
            let right_edge = f32::from(bounds.right());
            let center_x =
                ((left_edge + width * 0.5) * scale_factor).round() / scale_factor;

            let left_bounds = Bounds::from_corners(
                bounds.origin,
                point(px(center_x), bounds.bottom()),
            );
            let right_bounds = Bounds::from_corners(
                point(px(center_x), bounds.origin.y),
                point(bounds.right(), bounds.bottom()),
            );

            // 1. Solid 50/50 split background.
            window.paint_quad(fill(left_bounds, palette.past_bg));
            window.paint_quad(fill(right_bounds, palette.future_bg));

            let painter = TimelinePainter {
                snapshot,
                locale_tag: ltag,
                months_short: &months_short,
                cache: &glyph_cache,
            };

            // 2. Compute responsive row metrics anchored to the bottom edge.
            let scale = compute_scale(width, height, title_bar_h);
            let total_h = TOTAL_BASE_HEIGHT * scale;
            let mut cursor_y = (f32::from(bounds.bottom()) - BOTTOM_MARGIN_PX - total_h)
                .max(f32::from(bounds.top()) + 4.0);
            let rep_widths = painter.representative_widths(scale, palette.future_text, window);

            let mut rows = [ComputedRow {
                row_idx: 0,
                spec: ROW_SPECS[0],
                y: 0.0,
                row_h: 0.0,
                font_size: 0.0,
                cell_w: 0.0,
                frac: 0.0,
            }; 6];

            for (idx, spec) in ROW_SPECS.iter().copied().enumerate() {
                let row_idx = idx as u8;
                let font_size = spec.font_size * scale;
                let row_h = spec.row_height * scale;
                let cell_w = (spec.base_pitch * scale).max(rep_widths[idx] + font_size * 0.24);
                let frac = snapshot.progress(spec.unit) as f32;
                rows[idx] = ComputedRow {
                    row_idx,
                    spec,
                    y: cursor_y,
                    row_h,
                    font_size,
                    cell_w,
                    frac,
                };
                cursor_y += row_h;
            }

            // 3. Paint past half clipped to left_bounds.
            window.with_content_mask(Some(ContentMask { bounds: left_bounds }), |window| {
                painter.paint_half(
                    &rows,
                    (left_edge, center_x),
                    center_x,
                    palette.past_text,
                    window,
                    cx,
                );
            });

            // 4. Paint future half clipped to right_bounds.
            window.with_content_mask(Some(ContentMask { bounds: right_bounds }), |window| {
                painter.paint_half(
                    &rows,
                    (center_x, right_edge),
                    center_x,
                    palette.future_text,
                    window,
                    cx,
                );
            });
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_classic_matches_exact_black_and_white_tokens() {
        let light = theme::Theme::from_preset(&theme::LIGHT_PRESETS[0]);
        let dark = theme::Theme::from_preset(&theme::DARK_PRESETS[0]);
        let p_light = LifeClockPalette::resolve(LifeClockPaletteMode::Classic, &light);
        let p_dark = LifeClockPalette::resolve(LifeClockPaletteMode::Classic, &dark);
        assert_eq!(p_light, p_dark);
        assert_eq!(p_light.past_bg, Hsla::from(rgb(0x000000)));
        assert_eq!(p_light.future_bg, Hsla::from(rgb(0xFFFFFF)));
        assert_eq!(p_light.past_text, Hsla::from(rgb(0x333333)));
        assert_eq!(p_light.future_text, Hsla::from(rgb(0x262626)));
    }

    #[test]
    fn palette_fern_theme_preserves_visible_split_in_light_and_dark() {
        for preset in &theme::LIGHT_PRESETS {
            let t = theme::Theme::from_preset(preset);
            let p = LifeClockPalette::resolve(LifeClockPaletteMode::FernTheme, &t);
            assert!(p.future_bg.l > p.past_bg.l + 0.3, "light preset {}", preset.id);
        }
        for preset in &theme::DARK_PRESETS {
            let t = theme::Theme::from_preset(preset);
            let p = LifeClockPalette::resolve(LifeClockPaletteMode::FernTheme, &t);
            assert!(
                p.future_bg.l >= p.past_bg.l + 0.08,
                "dark preset {} lacked split contrast: past={} future={}",
                preset.id,
                p.past_bg.l,
                p.future_bg.l
            );
        }

        // Near-black custom dark elevated background still gets lifted for clear split.
        let past: Hsla = rgb(0x09090c).into();
        let lifted = ensure_dark_split_contrast(past, rgb(0x0a0a0d).into());
        assert!(lifted.l > past.l + 0.05);
    }

    #[test]
    fn responsive_scale_clamps_smoothly_across_window_sizes() {
        assert!((compute_scale(1760.0, 1080.0, 32.0) - 1.0).abs() < 1e-4);
        assert_eq!(compute_scale(300.0, 200.0, 32.0), MIN_SCALE);
        assert_eq!(compute_scale(4000.0, 2500.0, 32.0), MAX_SCALE);
        // At 1922x1140, ~6 years remain visible with generous upper breathing space.
        let qa_scale = compute_scale(1922.0, 1140.0, 32.0);
        assert!(qa_scale > 1.0 && qa_scale < 1.15, "unexpected qa scale: {qa_scale}");
        assert!(compute_scale(1000.0, 500.0, 32.0) > MIN_SCALE);
        assert!(compute_scale(1000.0, 500.0, 32.0) < 1.0);
    }

    #[test]
    fn zero_alloc_token_ids_match_core_token_labels() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 23)
            .unwrap()
            .and_hms_opt(3, 53, 16)
            .unwrap();
        let snap = TimelineSnapshot::from_datetime(now);
        let months = &fern_i18n::RU.life_clock_months;
        for unit in TimelineUnit::ALL {
            for offset in -40..=40 {
                let token = token_id_for_offset(snap, unit, offset);
                let fast = label_for_token(token, months);
                let core = snap.token_label(unit, offset, months);
                assert_eq!(fast.as_ref(), core.as_str(), "mismatch for {unit:?} offset={offset}");
            }
        }
    }
}
