use gpui::{App, DragMoveEvent, IntoElement, Pixels, Render, Window, div, prelude::*, px, relative};
use ligature_theme::{colors, spacing};

#[derive(Clone)]
struct PanelDrag;

impl Render for PanelDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
    }
}

fn resized_height(position: Pixels, height: Pixels) -> Pixels {
    let maximum = height * 0.7;
    let minimum = px(100.0).min(maximum);
    (height - position - spacing::pane_boundary() / 2.0).clamp(minimum, maximum)
}

pub fn bottom_pane(
    leading: impl IntoElement, trailing: impl IntoElement, height: Pixels, on_resize: impl Fn(&Pixels, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let colors = colors();
    div()
        .id("bottom-pane")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_hidden()
        .on_drag_move(move |event: &DragMoveEvent<PanelDrag>, window, cx| {
            on_resize(
                &resized_height(event.event.position.y - event.bounds.top(), event.bounds.size.height),
                window,
                cx,
            );
        })
        .child(div().flex().flex_col().flex_1().min_h(px(0.0)).overflow_hidden().child(leading))
        .child(
            div()
                .id("bottom-pane-boundary")
                .debug_selector(|| "bottom-pane-boundary".into())
                .group("bottom-pane-split")
                .h(spacing::pane_boundary())
                .w_full()
                .flex()
                .flex_shrink_0()
                .items_center()
                .hover(|style| style.bg(colors.hover))
                .active(|style| style.bg(colors.press))
                .cursor(gpui::CursorStyle::ResizeUpDown)
                .on_drag(PanelDrag, |_, _, _, cx| cx.new(|_| PanelDrag))
                .child(
                    div()
                        .id("bottom-pane-boundary-line")
                        .h(px(1.0))
                        .w_full()
                        .bg(colors.border)
                        .group_hover("bottom-pane-split", |style| style.h(px(2.0)).bg(colors.text))
                        .group_active("bottom-pane-split", |style| style.h(px(2.0)).bg(colors.text)),
                ),
        )
        .child(
            div()
                .id("bottom-pane-content")
                .debug_selector(|| "bottom-pane-content".into())
                .h(height)
                .max_h(relative(0.7))
                .flex_shrink_0()
                .min_h(px(0.0))
                .overflow_hidden()
                .child(trailing),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizing_keeps_the_panel_and_content_visible() {
        assert_eq!(resized_height(px(10.0), px(600.0)), px(420.0));
        assert_eq!(resized_height(px(397.0), px(600.0)), px(200.0));
        assert_eq!(resized_height(px(1000.0), px(600.0)), px(100.0));
    }

    #[test]
    fn resizing_handles_short_and_zero_height_containers() {
        assert_eq!(resized_height(px(0.0), px(100.0)), px(70.0));
        assert_eq!(resized_height(px(0.0), px(0.0)), px(0.0));
    }
}
