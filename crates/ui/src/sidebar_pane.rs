use gpui::{App, DragMoveEvent, IntoElement, Pixels, Render, Window, div, prelude::*, px};
use ligature_theme::{colors, spacing};

#[derive(Clone)]
struct SidebarDrag;

impl Render for SidebarDrag {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
    }
}

fn resized_width(position: Pixels, available: Pixels, minimum_content: Pixels) -> Pixels {
    let maximum = (available - minimum_content - spacing::pane_boundary()).max(px(0.0));
    let minimum = px(120.0).min(maximum);
    (position - spacing::pane_boundary() / 2.0).clamp(minimum, maximum)
}

pub fn sidebar_pane(
    sidebar: impl IntoElement, content: impl IntoElement, width: Pixels, minimum_content: Pixels,
    on_resize: impl Fn(&Pixels, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id("sidebar-pane")
        .size_full()
        .flex()
        .overflow_hidden()
        .on_drag_move(move |event: &DragMoveEvent<SidebarDrag>, window, cx| {
            on_resize(
                &resized_width(event.event.position.x - event.bounds.left(), event.bounds.size.width, minimum_content),
                window,
                cx,
            );
        })
        .child(div().w(width).min_w(px(120.0)).h_full().overflow_hidden().child(sidebar))
        .child(
            div()
                .id("sidebar-boundary")
                .debug_selector(|| "sidebar-boundary".into())
                .group("sidebar-split")
                .w(spacing::pane_boundary())
                .h_full()
                .flex()
                .flex_shrink_0()
                .justify_center()
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                .hover(|style| style.bg(colors().hover))
                .active(|style| style.bg(colors().press))
                .on_drag(SidebarDrag, |_, _, _, cx| cx.new(|_| SidebarDrag))
                .child(
                    div()
                        .id("sidebar-boundary-line")
                        .w(px(1.0))
                        .h_full()
                        .bg(colors().border)
                        .group_hover("sidebar-split", |style| style.bg(colors().text))
                        .group_active("sidebar-split", |style| style.bg(colors().text)),
                ),
        )
        .child(div().flex_1().min_w(minimum_content).h_full().child(content))
}
