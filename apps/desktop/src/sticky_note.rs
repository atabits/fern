//! The original's "Open as Sticky Note": a task in a small window of its own that stays over the
//! other windows, edited there as in the details. Each window shows the others' changes through
//! [`DataVersion`].

use std::rc::Rc;

use fern_core::{Id, Store};
use gpui::{App, Bounds, Entity, EntityId, Global, MouseButton, WindowBounds, WindowControlArea, WindowKind, WindowOptions, size};
use ui::prelude::*;

use crate::task_detail::{OpenTask, TaskDetail, TaskUpdated};

/// Counts the changes made in any window, so that the other windows show them; `by` is the view
/// that made the last one, which has it already.
#[derive(Default)]
pub struct DataVersion {
    version: u64,
    by: Option<EntityId>,
}

impl Global for DataVersion {}

/// Tells the other windows that `by` changed the data.
pub fn data_changed(by: EntityId, cx: &mut App) {
    let data = cx.default_global::<DataVersion>();
    data.version += 1;
    data.by = Some(by);
}

/// Whether the last change was made by a view other than `observer`.
pub fn changed_elsewhere(observer: EntityId, cx: &App) -> bool {
    cx.try_global::<DataVersion>().is_some_and(|data| data.by != Some(observer))
}

/// The note's size when it opens.
const WIDTH: f32 = 320.;
const HEIGHT: f32 = 360.;

/// Opens a task as a sticky note over the other windows.
pub fn open(store: Rc<Store>, id: Id, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        // Above every other window, as the original's notes stay.
        kind: WindowKind::PopUp,
        app_id: Some(std::env::var("FERN_APP_ID").unwrap_or_else(|_| "fern-tasks".into())),
        ..Default::default()
    };
    if let Err(err) = cx.open_window(options, |window, cx| cx.new(|cx| StickyNote::new(store, id, window, cx))) {
        eprintln!("failed to open the sticky note: {err:#}");
    }
}

pub struct StickyNote {
    store: Rc<Store>,
    detail: Entity<TaskDetail>,
    /// Linux and macOS: a press on the bar turns into a window move once the mouse moves.
    should_move: bool,
}

impl StickyNote {
    fn new(store: Rc<Store>, id: Id, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let detail = cx.new(|cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.set_card(false);
            detail
        });
        cx.subscribe(&detail, |this, _, _: &TaskUpdated, cx| {
            data_changed(cx.entity_id(), cx);
            this.refresh(cx);
        })
        .detach();
        // A subtask or parent clicked in the note opens in the note.
        cx.subscribe(&detail, |this, _, OpenTask(id), cx| this.show(*id, cx)).detach();
        cx.observe_global::<DataVersion>(|this, cx| {
            if changed_elsewhere(cx.entity_id(), cx) {
                this.refresh(cx);
            }
        })
        .detach();
        let mut this = Self { store, detail, should_move: false };
        this.show(id, cx);
        this
    }

    fn show(&mut self, id: Id, cx: &mut Context<Self>) {
        let task = self.store.task(id).ok().flatten();
        let lists = self.store.lists().unwrap_or_default();
        self.detail.update(cx, |detail, cx| {
            detail.set_lists(lists);
            detail.show(task, cx);
        });
    }

    /// Shows the task as it is now; a task deleted elsewhere leaves the note empty.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.detail.read(cx).task_id() else { return };
        let task = self.store.task(id).ok().flatten().filter(|task| !task.is_deleted());
        self.detail.update(cx, |detail, cx| detail.refresh(task, cx));
    }
}

impl Render for StickyNote {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .bg(theme.colors().background)
            .border_1()
            .border_color(theme.colors().border)
            .text_color(theme.colors().text)
            // A strip to move the note by, with its close button.
            .child(
                h_flex()
                    .flex_none()
                    .h(px(28.))
                    .pr_1()
                    .bg(theme.colors().sidebar_background)
                    .child(
                        // As the title bar: Windows moves the window from the tagged area, the
                        // others once the pressed mouse moves. The close button stays out of it.
                        div()
                            .id("sticky-drag")
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag)
                            .when(!cfg!(windows), |bar| {
                                bar.on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = true))
                                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = false))
                                    .on_mouse_move(cx.listener(|this, _, window, _| {
                                        if this.should_move {
                                            this.should_move = false;
                                            window.start_window_move();
                                        }
                                    }))
                            }),
                    )
                    .child(
                        IconButton::new("sticky-close", IconName::X)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .on_click(|_, window, _| window.remove_window()),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.detail.clone()))
    }
}
