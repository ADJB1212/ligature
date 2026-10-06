use gpui::{AnyView, Context, DragMoveEvent, IntoElement, Render, Window, div, prelude::*, px, relative};
use ligature_theme::{colors, spacing};

const MIN_PANE_WIDTH: f32 = 200.0;

pub struct SplitPane {
    leading: AnyView,
    trailing: AnyView,
    ratio: f32,
}

impl SplitPane {
    pub fn new(leading: AnyView, trailing: AnyView) -> Self {
        Self {
            leading,
            trailing,
            ratio: 0.5,
        }
    }
}

#[derive(Clone)]
struct PaneDrag;

impl Render for PaneDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn resized_ratio(position: f32, width: f32, boundary: f32) -> f32 {
    let available = (width - boundary).max(0.0);
    let minimum = MIN_PANE_WIDTH.min(available / 2.0);
    if width <= 0.0 {
        return 0.5;
    }
    position.clamp(minimum, available - minimum) / width
}

impl Render for SplitPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = colors();
        div()
            .id("split-pane")
            .size_full()
            .flex()
            .overflow_hidden()
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<PaneDrag>, _, cx| {
                this.ratio = resized_ratio(
                    f32::from(event.event.position.x - event.bounds.left()),
                    f32::from(event.bounds.size.width),
                    f32::from(spacing::pane_boundary()),
                );
                cx.notify();
            }))
            .child(
                div()
                    .w(relative(self.ratio))
                    .min_w(px(MIN_PANE_WIDTH))
                    .h_full()
                    .overflow_hidden()
                    .child(self.leading.clone()),
            )
            .child(
                div()
                    .id("pane-boundary")
                    .debug_selector(|| "pane-boundary".into())
                    .w(spacing::pane_boundary())
                    .h_full()
                    .flex_shrink_0()
                    .bg(colors.border)
                    .hover(|style| style.bg(colors.focus))
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .on_drag(PaneDrag, |_, _, _, cx| cx.new(|_| PaneDrag)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(MIN_PANE_WIDTH))
                    .h_full()
                    .overflow_hidden()
                    .child(self.trailing.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizing_preserves_both_pane_minimums() {
        assert_eq!(resized_ratio(-10.0, 1000.0, 6.0), 0.2);
        assert_eq!(resized_ratio(1100.0, 1000.0, 6.0), 0.794);
        assert_eq!(resized_ratio(500.0, 1000.0, 6.0), 0.5);
    }

    #[test]
    fn resizing_handles_narrow_and_zero_widths() {
        assert_eq!(resized_ratio(20.0, 300.0, 6.0), 0.49);
        assert_eq!(resized_ratio(20.0, 0.0, 6.0), 0.5);
    }
}
