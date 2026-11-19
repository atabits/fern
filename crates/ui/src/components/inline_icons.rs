use std::{cell::RefCell, rc::Rc};

use gpui::{App, Bounds, Global, Pixels, Window, px};
use smallvec::SmallVec;

/// Baselines for the icons that are direct children of the same horizontal row.
/// Labels in flexible content slots report here during prepaint, before any icon paints.
#[derive(Clone, Default)]
pub(super) struct RowCaptions(Rc<RefCell<SmallVec<[Caption; 2]>>>);

struct Caption {
    bounds: Bounds<Pixels>,
    axis: Pixels,
}

#[derive(Default)]
struct ActiveRow(Rc<RefCell<Option<RowCaptions>>>);

impl Global for ActiveRow {}

struct RestoreRow {
    slot: Rc<RefCell<Option<RowCaptions>>>,
    parent: Option<RowCaptions>,
}

impl Drop for RestoreRow {
    fn drop(&mut self) {
        self.slot.replace(self.parent.take());
    }
}

impl RowCaptions {
    /// Keyed controls start their own scope even if they contain only text.
    pub(super) fn collect<R>(
        row: Option<&Self>,
        cx: &mut App,
        draw: impl FnOnce(&mut App) -> R,
    ) -> R {
        if let Some(row) = row {
            row.0.borrow_mut().clear();
        } else if cx
            .try_global::<ActiveRow>()
            .is_none_or(|active| active.0.borrow().is_none())
        {
            return draw(cx);
        }
        if !cx.has_global::<ActiveRow>() {
            cx.set_global(ActiveRow::default());
        }
        let slot = cx.global::<ActiveRow>().0.clone();
        let parent = slot.replace(row.cloned());
        let _restore = RestoreRow { slot, parent };
        draw(cx)
    }

    pub(super) fn add(bounds: Bounds<Pixels>, cx: &App, axis: impl FnOnce() -> Pixels) {
        if let Some(active) = cx.try_global::<ActiveRow>()
            && let Some(row) = active.0.borrow().as_ref()
        {
            row.0.borrow_mut().push(Caption {
                bounds,
                axis: axis(),
            });
        }
    }

    pub(super) fn axis_for(&self, icon: Bounds<Pixels>, window: &Window) -> Option<Pixels> {
        // Flex snaps each box independently. Centers can differ by a physical
        // pixel when a row pairs odd and even sizes at fractional display scales,
        // and touching zero-gap siblings can overlap by a fraction of a pixel.
        let pixel = px(1. / window.scale_factor());
        let gap_tolerance = -pixel * 0.5;
        self.0
            .borrow()
            .iter()
            .filter_map(|caption| {
                let gap = (caption.bounds.left() - icon.right())
                    .max(icon.left() - caption.bounds.right());
                let same_line = (caption.bounds.center().y - icon.center().y).abs() <= pixel;
                (same_line && gap >= gap_tolerance).then_some((gap.max(Pixels::ZERO), caption.axis))
            })
            .min_by_key(|(gap, _)| *gap)
            .map(|(_, axis)| axis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, point, size};

    #[gpui::test]
    fn inline_icons_use_the_nearest_caption_in_their_own_row(cx: &mut TestAppContext) {
        cx.add_empty_window().update(|window, cx| {
            let outer = RowCaptions::default();
            let nested = RowCaptions::default();
            let slot = |x: f32, y: f32| Bounds::new(point(px(x), px(y)), size(px(16.), px(16.)));
            for scale in [1., 1.25, 1.5, 2.] {
                window.set_scale_factor(scale);
                RowCaptions::collect(Some(&outer), cx, |cx| {
                    RowCaptions::add(slot(20., 0.), cx, || px(6.));
                    RowCaptions::collect(Some(&nested), cx, |cx| {
                        RowCaptions::add(slot(65., 0.), cx, || px(7.));
                    });
                    RowCaptions::collect(None, cx, |cx| {
                        RowCaptions::add(slot(58., 0.), cx, || px(3.));
                    });
                    RowCaptions::add(slot(85., 0.), cx, || px(8.));
                });
                assert_eq!(outer.axis_for(slot(0., 0.), window), Some(px(6.)));
                assert_eq!(outer.axis_for(slot(4.2, 0.), window), Some(px(6.)));
                assert_eq!(outer.axis_for(slot(42., 0.), window), Some(px(6.)));
                assert_eq!(outer.axis_for(slot(65., 0.), window), Some(px(8.)));
                assert_eq!(nested.axis_for(slot(45., 0.), window), Some(px(7.)));
                assert_eq!(outer.axis_for(slot(0., 18.), window), None);
                assert_eq!(RowCaptions::default().axis_for(slot(0., 0.), window), None);
            }
        });
    }
}
