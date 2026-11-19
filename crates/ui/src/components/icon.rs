use std::{collections::HashMap, panic::Location};

use gpui::{
    App, Bounds, Element, ElementId, Global, GlobalElementId, InspectorElementId,
    InteractiveElement as _, IntoElement, LayoutId, Pixels, Point, SharedString, Styled, Svg,
    Window, point, px, svg,
};
use icons::IconName;

use super::inline_icons::RowCaptions;
use crate::Color;

#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub enum IconSize {
    /// 12px
    XSmall,
    /// 14px
    Small,
    /// 16px
    #[default]
    Medium,
    /// 18px
    Large,
    /// 20px
    XLarge,
    Custom(Pixels),
}

impl IconSize {
    pub fn pixels(self) -> Pixels {
        match self {
            IconSize::XSmall => px(12.),
            IconSize::Small => px(14.),
            IconSize::Medium => px(16.),
            IconSize::Large => px(18.),
            IconSize::XLarge => px(20.),
            IconSize::Custom(size) => size,
        }
    }
}

pub struct Icon {
    name: IconName,
    size: IconSize,
    color: Color,
    hover_group: Option<(SharedString, Color)>,
    pub(super) row: Option<RowCaptions>,
    source: &'static Location<'static>,
}

impl Icon {
    #[track_caller]
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: IconSize::default(),
            color: Color::Muted,
            hover_group: None,
            row: None,
            source: Location::caller(),
        }
    }

    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: impl Into<Color>) -> Self {
        self.color = color.into();
        self
    }

    pub fn group_hover_color(mut self, group: impl Into<SharedString>, color: impl Into<Color>) -> Self {
        self.hover_group = Some((group.into(), color.into()));
        self
    }
}

/// The displacement from the artwork's center to its viewport's center, in units
/// of viewport width. GPUI scales both axes of an SVG by the requested width.
#[derive(Default)]
struct IconMetrics(HashMap<IconName, Point<f32>>);

impl Global for IconMetrics {}

impl IconMetrics {
    fn offset(name: IconName, path: &str, cx: &mut App) -> Point<f32> {
        if let Some(offset) = cx
            .try_global::<Self>()
            .and_then(|metrics| metrics.0.get(&name))
        {
            return *offset;
        }
        let offset = if matches!(name, IconName::Play) {
            // Lucide intentionally offsets directional play triangles right for optical balance.
            Point::default()
        } else {
            cx.asset_source()
                .load(path)
                .ok()
                .flatten()
                .and_then(|bytes| artwork_offset(&bytes))
                .unwrap_or_default()
        };
        cx.default_global::<Self>().0.insert(name, offset);
        offset
    }
}

fn artwork_offset(bytes: &[u8]) -> Option<Point<f32>> {
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).ok()?;
    let viewport = tree.size();
    let drawing = tree.root().abs_stroke_bounding_box();
    Some(point(
        (viewport.width() - drawing.left() - drawing.right()) / (2. * viewport.width()),
        (viewport.height() - drawing.top() - drawing.bottom()) / (2. * viewport.width()),
    ))
}

impl IntoElement for Icon {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Icon {
    type RequestLayoutState = (Svg, <Svg as Element>::RequestLayoutState, Point<f32>);
    type PrepaintState = <Svg as Element>::PrepaintState;
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
    ) -> (LayoutId, Self::RequestLayoutState) {
        let path = self.name.path();
        let offset = IconMetrics::offset(self.name, &path, cx);
        let mut svg = svg()
            .path(path)
            .flex_none()
            .size(self.size.pixels())
            .text_color(self.color.color(cx));
        if let Some((group, hover_color)) = &self.hover_group {
            let hover_color = hover_color.color(cx);
            svg = svg.group_hover(group.clone(), move |el| el.text_color(hover_color));
        }
        let (layout, state) = svg.request_layout(id, inspector, window, cx);
        (layout, (svg, state, offset))
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let (svg, layout, _) = layout;
        svg.prepaint(id, inspector, bounds, layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        mut bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (svg, layout, offset) = layout;
        let axis = self
            .row
            .as_ref()
            .and_then(|row| row.axis_for(bounds, window))
            .unwrap_or(bounds.center().y);
        bounds.origin = window.pixel_snap_point(point(
            bounds.left() + bounds.size.width * offset.x,
            axis - bounds.size.height / 2. + bounds.size.width * offset.y,
        ));
        svg.paint(id, inspector, bounds, layout, state, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asymmetric_artwork_is_centered_at_the_svg_paint_scale() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
                <rect x="4" y="5" width="16" height="4" stroke="black" stroke-width="2"/>
            </svg>"#;
        assert_eq!(artwork_offset(svg), Some(point(8. / 40., 3. / 40.)));
        assert_eq!(
            artwork_offset(include_bytes!("../../../../assets/icons/clock.svg")),
            Some(Point::default())
        );
    }
}
