use std::{cell::RefCell, collections::HashMap, panic::Location, sync::Arc};

use gpui::{
    App, Bounds, DevicePixels, Element, ElementId, Global, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, PlatformTextSystem, RenderGlyphParams,
    SharedString, TextAlign, TextLayout, TextRenderingMode, Window, WrappedLineLayout,
};

use super::inline_icons::RowCaptions;

/// Use the same native font and glyph IDs that GPUI uses to shape and paint the window.
pub fn init_label_metrics(text_system: Arc<dyn PlatformTextSystem>, cx: &mut App) {
    cx.set_global(LabelMetrics {
        text_system,
        glyphs: RefCell::default(),
    });
}

struct LabelMetrics {
    text_system: Arc<dyn PlatformTextSystem>,
    glyphs: RefCell<HashMap<RenderGlyphParams, Bounds<DevicePixels>>>,
}

impl Global for LabelMetrics {}

const MAX_CACHED_GLYPHS: usize = 8192;

impl LabelMetrics {
    /// Union of the drawable glyphs, relative to the shaped line's baseline.
    fn ink_bounds(
        &self,
        line: &WrappedLineLayout,
        scale: f32,
        color: Hsla,
    ) -> Option<Bounds<Pixels>> {
        let mut ink: Option<Bounds<Pixels>> = None;
        let dilation = self.text_system.glyph_dilation_for_color(color);
        let mut cache = self.glyphs.borrow_mut();
        for run in line.runs() {
            let subpixel = self
                .text_system
                .recommended_rendering_mode(run.font_id, line.font_size())
                == TextRenderingMode::Subpixel;
            for glyph in &run.glyphs {
                let params = RenderGlyphParams {
                    font_id: run.font_id,
                    glyph_id: glyph.id,
                    font_size: line.font_size(),
                    scale_factor: scale,
                    subpixel_variant: Default::default(),
                    is_emoji: glyph.is_emoji,
                    subpixel_rendering: subpixel && !glyph.is_emoji,
                    dilation,
                };
                let raster = if let Some(bounds) = cache.get(&params) {
                    *bounds
                } else {
                    let bounds = self
                        .text_system
                        .glyph_raster_bounds(&params)
                        .unwrap_or_default();
                    // Bound the working set when a profile uses many scripts or display scales.
                    if cache.len() >= MAX_CACHED_GLYPHS {
                        cache.clear();
                    }
                    cache.insert(params, bounds);
                    bounds
                };
                if raster.size.width.0 <= 0 || raster.size.height.0 <= 0 {
                    continue;
                }
                let mut bounds = raster.to_pixels(scale);
                bounds.origin += glyph.position;
                ink = Some(ink.map_or(bounds, |ink| ink.union(&bounds)));
            }
        }
        ink
    }

    fn cap_height(&self, line: &WrappedLineLayout) -> Pixels {
        line.runs()
            .first()
            .map(|run| self.text_system.font_metrics(run.font_id).cap_height(line.font_size()))
            .unwrap_or(Pixels::ZERO)
    }
}

/// Text for a UI label that inherits its parent's style. GPUI still measures, wraps,
/// truncates and paints the text. A single visible line is centered by its letter body.
pub struct LabelText {
    text: SharedString,
    source: &'static Location<'static>,
}

impl LabelText {
    #[track_caller]
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            source: Location::caller(),
        }
    }
}

/// Center of the main letter body above the baseline, ignoring descenders and using the
/// font's capital height for cased scripts so neighboring labels share a stable baseline.
fn body_center_y(text: &str, ink: Bounds<Pixels>, cap_height: Pixels) -> Pixels {
    if ink.top() < Pixels::ZERO
        && text
            .chars()
            .any(|ch| ch.is_uppercase() || ch.is_lowercase())
    {
        let top = if cap_height > Pixels::ZERO {
            -cap_height
        } else {
            ink.top()
        };
        (top + ink.bottom().min(Pixels::ZERO)) / 2.
    } else {
        ink.center().y
    }
}

#[allow(clippy::too_many_arguments)]
fn align_line(
    mut bounds: Bounds<Pixels>,
    text: &str,
    line: &WrappedLineLayout,
    line_height: Pixels,
    ink: Bounds<Pixels>,
    cap_height: Pixels,
    align: TextAlign,
    window: &Window,
) -> Bounds<Pixels> {
    let baseline = baseline_offset(line, line_height);
    let center_y = body_center_y(text, ink, cap_height);
    bounds.origin.y = window.pixel_snap(bounds.center().y - center_y) - baseline;
    if align == TextAlign::Center {
        bounds.origin.x =
            window.pixel_snap(bounds.origin.x + line.unwrapped_layout.width / 2. - ink.center().x);
    }
    bounds
}

fn baseline_offset(line: &WrappedLineLayout, line_height: Pixels) -> Pixels {
    // GPUI divides extra leading equally above the ascent and below the descent.
    (line_height + line.ascent() - line.descent()) / 2.
}

/// A single-line text editor paints at a fixed baseline as the user types. Centers its
/// capital height inside `bounds` (matching `LabelText`), aligns neighboring icons to the
/// same axis, and returns the vertical paint offset from `bounds.top()`.
pub fn align_icons_to_input(
    bounds: Bounds<Pixels>,
    line: &WrappedLineLayout,
    line_height: Pixels,
    window: &Window,
    cx: &App,
) -> Pixels {
    let cap_height = line
        .runs()
        .first()
        .map(|run| window.text_system().cap_height(run.font_id, line.font_size()))
        .unwrap_or(Pixels::ZERO);
    let (offset, axis) = if cap_height > Pixels::ZERO {
        let baseline = baseline_offset(line, line_height);
        let center_y = -cap_height / 2.;
        let aligned_top = window.pixel_snap(bounds.center().y - center_y) - baseline;
        (aligned_top - bounds.top(), aligned_top + baseline + center_y)
    } else {
        (Pixels::ZERO, bounds.center().y)
    };
    RowCaptions::add(bounds, cx, || axis);
    offset
}

impl IntoElement for LabelText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for LabelText {
    type RequestLayoutState = TextLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static Location<'static>> {
        Some(self.source)
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, TextLayout) {
        self.text.request_layout(id, inspector, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        mut bounds: Bounds<Pixels>,
        layout: &mut TextLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let style = window.text_style();
        if let Some(metrics) = cx.try_global::<LabelMetrics>()
            && let [line] = layout.line_layouts().as_slice()
            && line.wrap_boundaries().is_empty()
            && let Some(ink) = metrics.ink_bounds(line, window.scale_factor(), style.color)
        {
            let cap_height = metrics.cap_height(line);
            let aligned = align_line(
                bounds,
                &self.text,
                line,
                layout.line_height(),
                ink,
                cap_height,
                style.text_align,
                window,
            );
            RowCaptions::add(bounds, cx, || {
                let baseline = aligned.top() + baseline_offset(line, layout.line_height());
                baseline + body_center_y(&self.text, ink, cap_height)
            });
            bounds = aligned;
        }
        self.text.prepaint(id, inspector, bounds, layout, window, cx);
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut TextLayout,
        prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text
            .paint(id, inspector, bounds, layout, prepaint, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{NoopTextSystem, TestAppContext, point, px, size};

    #[gpui::test]
    fn single_line_labels_apply_native_ink_bounds_and_wrapped_labels_keep_their_layout(
        cx: &mut TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let origin = point(px(9.), px(17.));
        let space = size(px(120.), px(48.));
        let (plain, _) = cx.draw(origin, space, |_, _| SharedString::from("18"));
        let line = plain.line_layout_for_index(0).unwrap();
        let scale = cx.update(|window, _| window.scale_factor());
        // The headless renderer has no bitmaps. Give the shaped digits known pixel bounds.
        let raster = Bounds::new(
            point(DevicePixels(1), DevicePixels(-7)),
            size(DevicePixels(6), DevicePixels(8)),
        );
        let mut glyphs = HashMap::new();
        for run in line.runs() {
            for glyph in &run.glyphs {
                glyphs.insert(
                    RenderGlyphParams {
                        font_id: run.font_id,
                        glyph_id: glyph.id,
                        font_size: line.font_size(),
                        scale_factor: scale,
                        subpixel_variant: Default::default(),
                        is_emoji: false,
                        subpixel_rendering: false,
                        dilation: 0,
                    },
                    raster,
                );
            }
        }
        cx.update(|_, cx| {
            cx.set_global(LabelMetrics {
                text_system: Arc::new(NoopTextSystem::new()),
                glyphs: RefCell::new(glyphs),
            })
        });
        let (aligned, _) = cx.draw(origin, space, |_, _| LabelText::new("18"));
        let baseline =
            aligned.bounds().top() + (aligned.line_height() + line.ascent() - line.descent()) / 2.;
        let visible_center = baseline + raster.to_pixels(scale).center().y;
        assert!((visible_center - plain.bounds().center().y).abs() <= px(0.51 / scale));
        assert_eq!(aligned.bounds().left(), plain.bounds().left());

        for (text, width) in [("18\n18", px(120.)), ("18 18 18", px(24.))] {
            let space = size(width, px(80.));
            let (plain, _) = cx.draw(origin, space, |_, _| SharedString::from(text));
            let (label, _) = cx.draw(origin, space, |_, _| LabelText::new(text));
            let lines = label.line_layouts();
            assert!(lines.len() > 1 || !lines[0].wrap_boundaries().is_empty());
            assert_eq!(label.bounds(), plain.bounds());
        }
    }

    #[gpui::test]
    fn labels_center_the_ink_of_dates_and_descenders_at_each_scale(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|window, _| {
            let style = window.text_style();
            for scale in [1., 1.25, 1.5, 2.] {
                window.set_scale_factor(scale);
                for (text, align, ink) in [
                    (
                        "18",
                        TextAlign::Center,
                        Bounds::new(point(px(2.), px(-10.)), size(px(13.), px(10.))),
                    ),
                    (
                        "Входящие",
                        TextAlign::Left,
                        Bounds::new(point(px(1.), px(-10.)), size(px(64.), px(13.))),
                    ),
                ] {
                    let line = WrappedLineLayout {
                        unwrapped_layout: window.text_system().layout_line(
                            text,
                            px(14.),
                            &[style.to_run(text.len())],
                            None,
                        ),
                        ..Default::default()
                    };
                    let bounds = Bounds::new(point(px(9.), px(17.)), size(px(90.), px(22.)));
                    let painted =
                        align_line(bounds, text, &line, px(22.), ink, px(10.), align, window);
                    let baseline = painted.top()
                        + (px(22.) - line.ascent() - line.descent()) / 2.
                        + line.ascent();
                    let center_y = baseline + body_center_y(text, ink, px(10.));
                    assert!((center_y - bounds.center().y).abs() <= px(0.51 / scale));
                    assert_eq!(painted.size, bounds.size);
                    if align == TextAlign::Center {
                        let start =
                            painted.left() + (bounds.size.width - line.unwrapped_layout.width) / 2.;
                        assert!((start + ink.center().x - bounds.center().x).abs() <= px(0.51 / scale));
                    } else {
                        assert_eq!(painted.left(), bounds.left());
                    }
                }
                // Words with/without descenders or ascenders share the same baseline on a row.
                let plain_line = WrappedLineLayout {
                    unwrapped_layout: window.text_system().layout_line(
                        "Отмена",
                        px(14.),
                        &[style.to_run("Отмена".len())],
                        None,
                    ),
                    ..Default::default()
                };
                let desc_line = WrappedLineLayout {
                    unwrapped_layout: window.text_system().layout_line(
                        "Удалить",
                        px(14.),
                        &[style.to_run("Удалить".len())],
                        None,
                    ),
                    ..Default::default()
                };
                let lower_line = WrappedLineLayout {
                    unwrapped_layout: window.text_system().layout_line(
                        "дом",
                        px(14.),
                        &[style.to_run("дом".len())],
                        None,
                    ),
                    ..Default::default()
                };
                let bounds = Bounds::new(point(px(9.), px(17.)), size(px(90.), px(22.)));
                let plain_ink = Bounds::new(point(px(1.), px(-10.)), size(px(50.), px(10.)));
                let desc_ink = Bounds::new(point(px(1.), px(-10.)), size(px(56.), px(13.)));
                let lower_ink = Bounds::new(point(px(1.), px(-7.)), size(px(26.), px(7.)));
                let plain_top = align_line(bounds, "Отмена", &plain_line, px(22.), plain_ink, px(10.), TextAlign::Left, window).top();
                assert_eq!(
                    plain_top,
                    align_line(bounds, "Удалить", &desc_line, px(22.), desc_ink, px(10.), TextAlign::Left, window).top(),
                );
                assert_eq!(
                    plain_top,
                    align_line(bounds, "дом", &lower_line, px(22.), lower_ink, px(10.), TextAlign::Left, window).top(),
                );
            }
        });
    }
}
