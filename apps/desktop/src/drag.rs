//! The value carried while a task is dragged between the list, the matrix, the calendar and the
//! lists column; it also renders the drag preview.

use fern_core::Id;
use ui::prelude::*;

#[derive(Clone)]
pub struct DraggedTask {
    pub id: Id,
    pub title: SharedString,
}

impl Render for DraggedTask {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        div()
            .px_3()
            .py_1p5()
            .rounded(px(6.))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .child(Label::new(self.title.clone()))
    }
}
