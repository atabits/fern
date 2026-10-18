use std::f32::consts::{FRAC_PI_2, TAU};

use gpui::{
    AnyElement, App, Hsla, IntoElement, ParentElement, PathBuilder, Pixels, RenderOnce, Styled,
    Window, canvas, div, point, px,
};
use theme::ActiveTheme;

/// A ring chart: segments in proportion to their values, clockwise from the top, with an
/// optional element in the middle — the original's "Completion Rate Distribution". An empty
/// chart is a plain grey ring.
#[derive(IntoElement)]
pub struct Donut {
    segments: Vec<(f32, Hsla)>,
    size: Pixels,
    thickness: Pixels,
    center: Option<AnyElement>,
}

impl Donut {
    pub fn new(segments: impl IntoIterator<Item = (f32, Hsla)>) -> Self {
        Self {
            segments: segments.into_iter().filter(|(v, _)| *v > 0.).collect(),
            size: px(120.),
            thickness: px(12.),
            center: None,
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn thickness(mut self, thickness: Pixels) -> Self {
        self.thickness = thickness;
        self
    }

    pub fn center(mut self, center: impl IntoElement) -> Self {
        self.center = Some(center.into_any_element());
        self
    }
}

/// The share of a full turn each segment takes, in order.
pub fn donut_shares(values: &[f32]) -> Vec<f32> {
    let total: f32 = values.iter().filter(|v| **v > 0.).sum();
    if total <= 0. {
        return Vec::new();
    }
    values.iter().map(|v| v.max(0.) / total).collect()
}

impl RenderOnce for Donut {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let track = cx.theme().colors().element_hover;
        let thickness = self.thickness;
        let segments = self.segments;
        let ring = canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let center = bounds.center();
                let radius = bounds.size.width.min(bounds.size.height) / 2. - thickness / 2.;
                // Each arc is drawn as a stroked polyline, fine enough to look round.
                let mut arc = |from: f32, to: f32, color: Hsla| {
                    let steps = ((to - from) / TAU * 120.).ceil().max(2.) as usize;
                    let mut path = PathBuilder::stroke(thickness);
                    for i in 0..=steps {
                        let angle = from + (to - from) * i as f32 / steps as f32 - FRAC_PI_2;
                        let p = point(center.x + radius * angle.cos(), center.y + radius * angle.sin());
                        if i == 0 { path.move_to(p) } else { path.line_to(p) }
                    }
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color);
                    }
                };
                arc(0., TAU, track);
                let shares = donut_shares(&segments.iter().map(|(v, _)| *v).collect::<Vec<_>>());
                let mut start = 0.;
                for (share, (_, color)) in shares.into_iter().zip(&segments) {
                    let end = start + share * TAU;
                    arc(start, end, *color);
                    start = end;
                }
            },
        )
        .absolute()
        .size_full();
        div()
            .relative()
            .flex_none()
            .size(self.size)
            .flex()
            .items_center()
            .justify_center()
            .child(ring)
            .children(self.center)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_add_up_to_a_turn() {
        assert_eq!(donut_shares(&[1., 3.]), [0.25, 0.75]);
        assert!(donut_shares(&[0., 0.]).is_empty());
        assert_eq!(donut_shares(&[2., 0., 2.]), [0.5, 0., 0.5]);
    }
}
