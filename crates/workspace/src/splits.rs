use gpui::{AnyElement, DragMoveEvent, EntityId, relative};

use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    fn stacked(self) -> bool {
        matches!(self, Self::Top | Self::Bottom)
    }

    fn before(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Layout {
    Tab(usize),
    Split {
        id: usize,
        stacked: bool,
        ratio: f32,
        leading: Box<Self>,
        trailing: Box<Self>,
    },
}

impl Layout {
    pub(super) fn first(&self) -> usize {
        match self {
            Self::Tab(tab) => *tab,
            Self::Split { leading, .. } => leading.first(),
        }
    }

    fn contains(&self, index: usize) -> bool {
        match self {
            Self::Tab(tab) => *tab == index,
            Self::Split { leading, trailing, .. } => leading.contains(index) || trailing.contains(index),
        }
    }

    fn replace(&mut self, old: usize, new: usize) {
        match self {
            Self::Tab(tab) if *tab == old => *tab = new,
            Self::Split { leading, trailing, .. } => {
                leading.replace(old, new);
                trailing.replace(old, new);
            }
            Self::Tab(_) => {}
        }
    }

    fn remove(self, index: usize, renumber: bool) -> Option<Self> {
        match self {
            Self::Tab(tab) => (tab != index).then(|| Self::Tab(tab - usize::from(renumber && tab > index))),
            Self::Split {
                id,
                stacked,
                ratio,
                leading,
                trailing,
            } => match (leading.remove(index, renumber), trailing.remove(index, renumber)) {
                (Some(leading), Some(trailing)) => Some(Self::Split {
                    id,
                    stacked,
                    ratio,
                    leading: Box::new(leading),
                    trailing: Box::new(trailing),
                }),
                (remaining, None) | (None, remaining) => remaining,
            },
        }
    }

    pub(super) fn close(self, index: usize) -> Option<Self> {
        self.remove(index, true)
    }

    fn split(&mut self, source: usize, target: usize, edge: Edge, id: usize) {
        match self {
            Self::Tab(tab) if *tab == target => {
                let (leading, trailing) = if edge.before() { (source, target) } else { (target, source) };
                *self = Self::Split {
                    id,
                    stacked: edge.stacked(),
                    ratio: 0.5,
                    leading: Box::new(Self::Tab(leading)),
                    trailing: Box::new(Self::Tab(trailing)),
                };
            }
            Self::Split { leading, trailing, .. } => {
                leading.split(source, target, edge, id);
                trailing.split(source, target, edge, id);
            }
            Self::Tab(_) => {}
        }
    }

    fn resize(&mut self, target: usize, value: f32) {
        if let Self::Split {
            id,
            ratio,
            leading,
            trailing,
            ..
        } = self
        {
            if *id == target {
                *ratio = value;
            } else {
                leading.resize(target, value);
                trailing.resize(target, value);
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct TabDrag {
    pub workspace: EntityId,
    pub editor: EntityId,
    pub name: String,
}

impl Render for TabDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(spacing::medium())
            .py(spacing::small())
            .bg(colors().chrome)
            .text_color(colors().text)
            .child(self.name.clone())
    }
}

#[derive(Clone)]
struct DividerDrag {
    workspace: EntityId,
    id: usize,
}

impl Render for DividerDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Workspace {
    pub(super) fn show_tab(&mut self, previous: usize) {
        if let Some(layout) = &mut self.editor_layout {
            if !layout.contains(self.active_tab) {
                let target = if layout.contains(previous) { previous } else { layout.first() };
                layout.replace(target, self.active_tab);
            }
        } else {
            self.editor_layout = Some(Layout::Tab(self.active_tab));
        }
    }

    pub(super) fn observe_editor_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.editor.entity_id();
        let subscription = cx.on_focus_in(&self.editor.focus_handle(cx), window, move |this, window, cx| {
            if let Some(index) = this
                .tabs
                .iter()
                .position(|tab| tab.as_ref().is_some_and(|tab| tab.editor.entity_id() == id))
            {
                this.activate_tab(index, window, cx);
            }
        });
        self.editor_focus.insert(id, subscription);
    }

    fn split_tab(&mut self, source: usize, target: usize, edge: Edge, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy
            || source == target
            || source >= self.tabs.len()
            || target >= self.tabs.len()
            || !self.editor_layout.as_ref().is_some_and(|layout| layout.contains(target))
        {
            return;
        }
        if let Some(mut layout) = self.editor_layout.take().and_then(|layout| layout.remove(source, false)) {
            layout.split(source, target, edge, self.next_split);
            self.next_split += 1;
            self.editor_layout = Some(layout);
            self.activate_tab(source, window, cx);
            self.focus_editor(window, cx);
            cx.notify();
        }
    }

    pub(super) fn render_editor_layout(&self, layout: &Layout, cx: &mut Context<Self>) -> AnyElement {
        match layout {
            Layout::Tab(index) => {
                let index = *index;
                let editor = if index == self.active_tab {
                    &self.editor
                } else {
                    &self.tabs[index].as_ref().expect("visible tab exists").editor
                };
                div()
                    .id(("editor-pane", index))
                    .debug_selector(move || format!("editor-pane-{index}"))
                    .relative()
                    .size_full()
                    .overflow_hidden()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.activate_tab(index, window, cx)),
                    )
                    .child(editor.clone())
                    .when(cx.has_active_drag() && !self.busy, |pane| {
                        pane.children(
                            [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom]
                                .into_iter()
                                .enumerate()
                                .map(|(zone, edge)| {
                                    div()
                                        .id(("split-drop", zone))
                                        .absolute()
                                        .when(edge.stacked(), |view| view.left(relative(0.25)).w(relative(0.5)).h(relative(0.25)))
                                        .when(!edge.stacked(), |view| view.top_0().h_full().w(relative(0.25)))
                                        .when(edge == Edge::Left, |view| view.left_0())
                                        .when(edge == Edge::Right, |view| view.right_0())
                                        .when(edge == Edge::Top, |view| view.top_0())
                                        .when(edge == Edge::Bottom, |view| view.bottom_0())
                                        .drag_over::<TabDrag>(|style, _, _, _| style.bg(colors().hover).border_1().border_color(colors().focus))
                                        .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                                            if drag.workspace != cx.entity_id() {
                                                return;
                                            }
                                            let source = if this.editor.entity_id() == drag.editor {
                                                Some(this.active_tab)
                                            } else {
                                                this.tabs
                                                    .iter()
                                                    .position(|tab| tab.as_ref().is_some_and(|tab| tab.editor.entity_id() == drag.editor))
                                            };
                                            if let Some(source) = source {
                                                this.split_tab(source, index, edge, window, cx);
                                            }
                                        }))
                                }),
                        )
                    })
                    .into_any_element()
            }
            Layout::Split {
                id,
                stacked,
                ratio,
                leading,
                trailing,
            } => {
                let (id, stacked, ratio) = (*id, *stacked, *ratio);
                let workspace = cx.entity_id();
                div()
                    .id(("editor-split", id))
                    .size_full()
                    .flex()
                    .overflow_hidden()
                    .when(stacked, |view| view.flex_col())
                    .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<DividerDrag>, _, cx| {
                        if event.drag(cx).id != id || event.drag(cx).workspace != cx.entity_id() {
                            return;
                        }
                        let (position, extent) = if stacked {
                            (event.event.position.y - event.bounds.top(), event.bounds.size.height)
                        } else {
                            (event.event.position.x - event.bounds.left(), event.bounds.size.width)
                        };
                        let available = (f32::from(extent) - f32::from(spacing::pane_boundary())).max(0.0);
                        let minimum = 100.0_f32.min(available / 2.0);
                        if extent > px(0.0)
                            && let Some(layout) = &mut this.editor_layout
                        {
                            layout.resize(id, f32::from(position).clamp(minimum, available - minimum) / f32::from(extent));
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .when(stacked, |view| view.h(relative(ratio)).w_full())
                            .when(!stacked, |view| view.w(relative(ratio)).h_full())
                            .child(self.render_editor_layout(leading, cx)),
                    )
                    .child(
                        div()
                            .id(("editor-divider", id))
                            .debug_selector(move || format!("editor-divider-{id}"))
                            .flex_shrink_0()
                            .bg(colors().border)
                            .hover(|style| style.bg(colors().focus))
                            .when(stacked, |view| {
                                view.w_full().h(spacing::pane_boundary()).cursor(gpui::CursorStyle::ResizeUpDown)
                            })
                            .when(!stacked, |view| {
                                view.h_full().w(spacing::pane_boundary()).cursor(gpui::CursorStyle::ResizeLeftRight)
                            })
                            .on_drag(DividerDrag { workspace, id }, |drag, _, _, cx| cx.new(|_| drag.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.render_editor_layout(trailing, cx)),
                    )
                    .into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_visible_tab_collapses_its_old_split_and_preserves_nested_layouts() {
        let mut layout = Layout::Tab(0);
        layout.split(1, 0, Edge::Right, 0);
        layout.split(2, 1, Edge::Bottom, 1);
        let mut layout = layout.remove(0, false).unwrap();
        layout.split(0, 2, Edge::Top, 2);
        assert_eq!(layout.first(), 1);
        assert!(layout.contains(0) && layout.contains(1) && layout.contains(2));
        let layout = layout.close(1).unwrap();
        assert_eq!(
            layout,
            Layout::Split {
                id: 2,
                stacked: true,
                ratio: 0.5,
                leading: Box::new(Layout::Tab(0)),
                trailing: Box::new(Layout::Tab(1)),
            }
        );
    }

    #[test]
    fn closing_hidden_tabs_renumbers_visible_tabs_and_last_pane_disappears() {
        let mut layout = Layout::Tab(1);
        layout.split(3, 1, Edge::Left, 0);
        let layout = layout.close(0).unwrap();
        assert_eq!(layout.first(), 2);
        let layout = layout.close(2).unwrap();
        assert_eq!(layout, Layout::Tab(0));
        assert_eq!(layout.close(0), None);
    }

    #[test]
    fn dragging_tabs_to_each_editor_edge_creates_the_requested_split() {
        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            let mut app = gpui::TestAppContext::single();
            let (window, root) = crate::tests::workspace(&mut app);
            cx_new_document(&mut app, window);
            let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
            cx.simulate_resize(gpui::size(px(1200.0), px(800.0)));
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let tab = cx.debug_bounds("document-tab-0").unwrap();
            let pane = cx.debug_bounds("editor-pane-1").unwrap();
            let mut destination = pane.center();
            match edge {
                Edge::Left => destination.x = pane.left() + px(10.0),
                Edge::Right => destination.x = pane.right() - px(10.0),
                Edge::Top => destination.y = pane.top() + px(10.0),
                Edge::Bottom => destination.y = pane.bottom() - px(10.0),
            }
            cx.simulate_mouse_down(tab.center(), gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_move(destination, gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.simulate_mouse_move(destination, gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_up(destination, gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let first = cx.debug_bounds("editor-pane-0").unwrap();
            let second = cx.debug_bounds("editor-pane-1").unwrap();
            match edge {
                Edge::Left => assert!(first.right() <= second.left()),
                Edge::Right => assert!(second.right() <= first.left()),
                Edge::Top => assert!(first.bottom() <= second.top()),
                Edge::Bottom => assert!(second.bottom() <= first.top()),
            }
            root.read_with(&cx, |workspace, _| assert_eq!(workspace.active_tab, 0));
            let divider = cx.debug_bounds("editor-divider-0").unwrap();
            let mut resized = divider.center();
            if edge.stacked() {
                resized.y += px(40.0);
            } else {
                resized.x += px(40.0);
            }
            cx.simulate_mouse_down(divider.center(), gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_move(resized, gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.simulate_mouse_move(resized, gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_up(resized, gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let moved = cx.debug_bounds("editor-divider-0").unwrap();
            if edge.stacked() {
                assert!(moved.top() > divider.top());
            } else {
                assert!(moved.left() > divider.left());
            }
        }
    }

    fn cx_new_document(cx: &mut gpui::TestAppContext, window: gpui::WindowHandle<Workspace>) {
        cx.simulate_keystrokes(window.into(), "cmd-n");
        cx.run_until_parked();
    }

    #[test]
    fn split_focus_preserves_buffers_and_closing_collapses_the_pane() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.tex");
        let second = directory.path().join("second.tex");
        std::fs::write(&first, "first").unwrap();
        std::fs::write(&second, "second").unwrap();
        let mut cx = gpui::TestAppContext::single();
        let (window, root) = crate::tests::workspace(&mut cx);
        crate::tests::open(&mut cx, window, &first);
        let first_editor = root.read_with(&cx, |workspace, _| workspace.editor.clone());
        crate::tests::open(&mut cx, window, &second);
        cx.update_window(window.into(), |_, window, cx| {
            root.update(cx, |workspace, cx| workspace.split_tab(0, 1, Edge::Left, window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.activate_window();
            root.update(cx, |workspace, cx| workspace.activate_tab(1, window, cx));
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.focus(&first_editor.focus_handle(cx), cx))
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx)).unwrap();
        cx.run_until_parked();
        root.read_with(&cx, |workspace, _| {
            assert_eq!(workspace.active_tab, 0);
            assert_eq!(workspace.editor.entity_id(), first_editor.entity_id());
            assert!(workspace.editor_layout.as_ref().unwrap().contains(1));
        });
        cx.simulate_input(window.into(), "edited");
        let layout = root.read_with(&cx, |workspace, _| workspace.editor_layout.clone());
        cx.simulate_keystrokes(window.into(), "cmd-w");
        cx.run_until_parked();
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        root.read_with(&cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 2);
            assert_eq!(workspace.editor_layout, layout);
        });
        cx.simulate_keystrokes(window.into(), "cmd-s");
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "editedfirst");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "second");
        cx.simulate_keystrokes(window.into(), "cmd-w");
        cx.run_until_parked();
        root.read_with(&cx, |workspace, cx| {
            assert_eq!(workspace.tabs.len(), 1);
            assert_eq!(workspace.editor.read(cx).state().text(), "second");
            assert_eq!(workspace.editor_layout, Some(Layout::Tab(0)));
        });
    }
}
