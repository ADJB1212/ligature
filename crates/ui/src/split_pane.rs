use gpui::{AnyView, App, Context, DragMoveEvent, IntoElement, Render, Window, div, prelude::*, px, relative};
use ligature_theme::{colors, spacing};

const MIN_PANE_WIDTH: f32 = 200.0;

pub struct SplitPane {
    leading: AnyView,
    trailing: AnyView,
    ratio: f32,
}

impl SplitPane {
    pub fn set_leading(&mut self, leading: AnyView, cx: &mut Context<Self>) {
        self.leading = leading;
        cx.notify();
    }

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
        split_pane(
            self.leading.clone(),
            self.trailing.clone(),
            self.ratio,
            cx.listener(|this, ratio: &f32, _, cx| {
                this.ratio = *ratio;
                cx.notify();
            }),
        )
    }
}

pub fn split_pane(
    leading: impl IntoElement, trailing: impl IntoElement, ratio: f32, on_resize: impl Fn(&f32, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let colors = colors();
    div()
        .id("split-pane")
        .size_full()
        .flex()
        .overflow_hidden()
        .on_drag_move(move |event: &DragMoveEvent<PaneDrag>, window, cx| {
            let ratio = resized_ratio(
                f32::from(event.event.position.x - event.bounds.left()),
                f32::from(event.bounds.size.width),
                f32::from(spacing::pane_boundary()),
            );
            on_resize(&ratio, window, cx);
        })
        .child(
            div()
                .id("leading-pane")
                .debug_selector(|| "leading-pane".into())
                .w(relative(ratio))
                .min_w(px(MIN_PANE_WIDTH))
                .h_full()
                .overflow_hidden()
                .child(leading),
        )
        .child(
            div()
                .id("pane-boundary")
                .debug_selector(|| "pane-boundary".into())
                .group("pane-split")
                .w(spacing::pane_boundary())
                .h_full()
                .flex()
                .flex_shrink_0()
                .justify_center()
                .hover(|style| style.bg(colors.hover))
                .active(|style| style.bg(colors.press))
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                .on_drag(PaneDrag, |_, _, _, cx| cx.new(|_| PaneDrag))
                .child(
                    div()
                        .id("pane-boundary-line")
                        .w(gpui::px(1.0))
                        .h_full()
                        .bg(colors.border)
                        .group_hover("pane-split", |style| style.w(gpui::px(2.0)).bg(colors.text))
                        .group_active("pane-split", |style| style.w(gpui::px(2.0)).bg(colors.text)),
                ),
        )
        .child(
            div()
                .id("trailing-pane")
                .debug_selector(|| "trailing-pane".into())
                .flex_1()
                .min_w(px(MIN_PANE_WIDTH))
                .h_full()
                .overflow_hidden()
                .child(trailing),
        )
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
