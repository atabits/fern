//! The Eisenhower Matrix, matched to TickTick's: four quadrants by priority, tasks grouped by
//! list inside each quadrant. Dragging a task to another quadrant changes its priority.

use std::collections::HashMap;
use std::rc::Rc;

use chrono::Local;
use fern_core::{Id, List, Priority, Store, Task, View};
use gpui::{EventEmitter, FontWeight};
use ui::prelude::*;

use crate::i18n::{self, Strings};
use crate::lists_panel;
use crate::task_rows;

/// The user clicked a task in the matrix.
pub struct OpenTask(pub Id);

/// The four quadrants, in reading order. TickTick's default rules map them to priorities.
const QUADRANTS: [Priority; 4] = [Priority::High, Priority::Medium, Priority::Low, Priority::None];

#[derive(Clone)]
struct DraggedTask {
    id: Id,
    title: SharedString,
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

pub struct MatrixView {
    store: Rc<Store>,
    tasks: Vec<Task>,
    lists: Vec<List>,
}

impl EventEmitter<OpenTask> for MatrixView {}

impl MatrixView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let mut this = Self { store, tasks: Vec::new(), lists: Vec::new() };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        self.tasks = self
            .store
            .tasks_in(&View::All, today)
            .unwrap_or_default()
            .into_iter()
            .filter(|t| !t.is_completed())
            .collect();
        self.lists = self.store.lists().unwrap_or_default();
        cx.notify();
    }

    fn set_priority(&mut self, id: Id, priority: Priority, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_priority(id, priority) {
            eprintln!("failed to change the priority: {err:#}");
        }
        self.reload(cx);
    }

    fn complete(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_completed(id, true) {
            eprintln!("failed to complete the task: {err:#}");
        }
        self.reload(cx);
    }

    fn quadrant_title(priority: Priority, strings: &Strings) -> (&'static str, &'static str) {
        match priority {
            Priority::High => ("I", strings.matrix_urgent_important),
            Priority::Medium => ("II", strings.matrix_not_urgent_important),
            Priority::Low => ("III", strings.matrix_urgent_unimportant),
            Priority::None => ("IV", strings.matrix_not_urgent_unimportant),
        }
    }

    fn quadrant_color(priority: Priority, cx: &App) -> Hsla {
        let status = cx.theme().status();
        match priority {
            Priority::High => status.error,
            Priority::Medium => status.warning,
            Priority::Low => status.info,
            Priority::None => status.success,
        }
    }

    fn render_task(&self, task: &Task, list_name: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = task.id;
        let today = Local::now().date_naive();
        let tag_color = |name: &str| {
            let tags = self.store.tags().unwrap_or_default();
            lists_panel::color(tags.iter().find(|t| t.name == name).and_then(|t| t.color.as_deref()), cx)
        };
        let due = task.due_date.map(|date| task_rows::due_label(date, task.due_time, today, strings));
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let tags: Vec<AnyElement> =
            task.tags.iter().map(|tag| Chip::new(tag.clone(), tag_color(tag)).into_any_element()).collect();

        h_flex()
            .id(ElementId::Uuid(id))
            .h(px(40.))
            .flex_none()
            .gap(px(10.))
            .px_2()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.colors().ghost_element_hover))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenTask(id))))
            .child(
                Checkbox::new("matrix-checkbox")
                    .tint(task_rows::priority_color(task.priority, &theme))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.complete(id, cx);
                    })),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(div().flex_1().min_w_0().child(Label::new(task.title.clone()).truncate()))
                    .children(tags)
                    .child(Label::new(list_name.clone()).size(LabelSize::Small).color(Color::Muted))
                    .when(task.repeat.is_some(), |el| {
                        el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                    })
                    .when_some(due, |el, (label, overdue)| {
                        el.child(
                            Label::new(label).size(LabelSize::Small).color(if overdue { Color::Error } else { Color::Accent }),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_quadrant(&self, priority: Priority, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let color = Self::quadrant_color(priority, cx);
        let (numeral, title) = Self::quadrant_title(priority, strings);
        // Subtasks stay under their parent, as in the original; only top-level open tasks are listed.
        let open: std::collections::HashSet<Id> = self.tasks.iter().map(|t| t.id).collect();
        let tasks: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| t.priority == priority && !t.parent_id.is_some_and(|p| open.contains(&p)))
            .collect();

        // Group by list, keeping the order in which lists first appear.
        let mut groups: Vec<(Option<Id>, Vec<&Task>)> = Vec::new();
        let mut index: HashMap<Option<Id>, usize> = HashMap::new();
        for task in tasks {
            let at = *index.entry(task.list_id).or_insert_with(|| {
                groups.push((task.list_id, Vec::new()));
                groups.len() - 1
            });
            groups[at].1.push(task);
        }
        let list_name = |list: Option<Id>| -> SharedString {
            list.and_then(|id| self.lists.iter().find(|l| l.id == id))
                .map_or(SharedString::new_static(strings.inbox), |l| l.name.clone().into())
        };
        let content: Vec<AnyElement> = groups
            .iter()
            .flat_map(|(list, tasks)| {
                let name = list_name(*list);
                let header = h_flex()
                    .h(px(28.))
                    .gap_1()
                    .px_1()
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                    .child(Label::new(name.clone()).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(tasks.len().to_string()).size(LabelSize::XSmall).color(Color::Muted))
                    .into_any_element();
                std::iter::once(header)
                    .chain(tasks.iter().map(|task| self.render_task(task, &name, cx)))
                    .collect::<Vec<_>>()
            })
            .collect();
        let empty = content.is_empty();

        v_flex()
            .id(SharedString::from(format!("quadrant-{numeral}")))
            .flex_1()
            .min_w_0()
            .h_full()
            .p_2()
            .gap_1()
            .rounded(px(8.))
            .bg(theme.colors().panel_background)
            .drag_over::<DraggedTask>(move |style, _, _, _| style.border_1().border_color(color))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.set_priority(dragged.id, priority, cx)))
            .child(
                h_flex()
                    .h(px(24.))
                    .gap_1p5()
                    .child(
                        div()
                            .size(px(14.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(color)
                            .text_color(theme.colors().background)
                            .text_size(px(7.))
                            .font_weight(FontWeight::BOLD)
                            .child(numeral),
                    )
                    .child(Label::new(title).size(LabelSize::Small).color(Color::Custom(color))),
            )
            .child(
                v_flex()
                    .id(SharedString::from(format!("quadrant-tasks-{numeral}")))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(empty, |el| {
                        el.items_center()
                            .justify_center()
                            .child(Label::new(strings.no_tasks).size(LabelSize::Small).color(Color::Muted))
                    })
                    .children(content),
            )
            .into_any_element()
    }
}

impl Render for MatrixView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let [q1, q2, q3, q4] = QUADRANTS.map(|priority| self.render_quadrant(priority, cx));
        v_flex()
            .size_full()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(
                h_flex()
                    .h(px(52.))
                    .flex_none()
                    .px(px(22.))
                    .child(div().flex_1().child(Label::new(strings.section_matrix).size(LabelSize::Large).weight(FontWeight::BOLD)))
                    .child(IconButton::new("matrix-more", IconName::Ellipsis)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px(px(14.))
                    .pb(px(14.))
                    .gap(px(12.))
                    .child(h_flex().flex_1().min_h_0().items_stretch().gap(px(12.)).child(q1).child(q2))
                    .child(h_flex().flex_1().min_h_0().items_stretch().gap(px(12.)).child(q3).child(q4)),
            )
    }
}
