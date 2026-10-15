//! Reusable GPUI controls such as buttons, inputs, menus, and panels. Feature views belong to their own crates.

mod bottom_pane;
mod button;
mod input;
mod sidebar_pane;
mod split_pane;

pub use bottom_pane::bottom_pane;
pub use button::Button;
pub use input::TextInput;
pub use sidebar_pane::sidebar_pane;
pub use split_pane::{SplitPane, split_pane};

use gpui::{Animation, AnimationExt, App, ElementId, IntoElement, KeyBinding, Window, actions, div, prelude::*, pulsating_between, px};
use ligature_theme::{colors, spacing, typography};
use std::time::Duration;

actions!(ligature_ui, [FocusNext, FocusPrevious]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, Some("LigatureControls")),
        KeyBinding::new("shift-tab", FocusPrevious, Some("LigatureControls")),
    ]);
}

pub fn focus_next(_: &FocusNext, window: &mut Window, cx: &mut App) {
    window.focus_next(cx);
}

pub fn focus_previous(_: &FocusPrevious, window: &mut Window, cx: &mut App) {
    window.focus_prev(cx);
}

pub fn chrome_strip() -> gpui::Div {
    div().flex_shrink_0().px(spacing::large()).py(spacing::medium()).bg(colors().chrome)
}

pub fn panel_header(text: impl Into<gpui::SharedString>) -> gpui::Div {
    chrome_strip()
        .h(px(40.0))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(colors().border)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(div().flex_1().min_w(px(0.0)).overflow_hidden().text_ellipsis().child(text.into()))
}

pub fn hairline() -> impl IntoElement {
    div().h(gpui::px(1.0)).w_full().flex_shrink_0().bg(colors().border)
}

pub fn vertical_seam() -> impl IntoElement {
    div().w(gpui::px(1.0)).h_full().flex_shrink_0().bg(colors().border)
}

pub fn inline_error(message: impl Into<gpui::SharedString>) -> impl IntoElement {
    let colors = colors();
    div()
        .flex()
        .gap(spacing::medium())
        .px(spacing::medium())
        .py(spacing::small())
        .bg(colors.error_surface)
        .text_color(colors.error)
        .text_size(typography::small_size())
        .line_height(typography::line_height())
        .child("Error:")
        .child(message.into())
}

pub fn rule() -> impl IntoElement {
    div().w(gpui::px(1.0)).h(gpui::px(16.0)).flex_shrink_0().bg(colors().border)
}

pub fn caption(text: impl Into<gpui::SharedString>) -> impl IntoElement {
    div()
        .text_size(typography::small_size())
        .line_height(typography::line_height())
        .text_color(colors().muted_text)
        .child(text.into())
}

#[derive(Clone, Copy)]
pub enum Indicator {
    Idle,
    Live,
    On,
    Alert,
}

pub fn indicator(id: impl Into<ElementId>, kind: Indicator) -> gpui::AnyElement {
    let colors = colors();
    let (color, pulse) = match kind {
        Indicator::Idle => (colors.muted_text, false),
        Indicator::Live => (colors.text, true),
        Indicator::On => (colors.text, false),
        Indicator::Alert => (colors.error, false),
    };
    let lamp = div().size(px(5.0)).rounded(px(99.0)).flex_shrink_0().bg(color);
    if pulse {
        lamp.with_animation(
            id,
            Animation::new(Duration::from_millis(1100))
                .repeat()
                .with_easing(pulsating_between(0.35, 1.0)),
            |this, delta| this.opacity(delta),
        )
        .into_any_element()
    } else {
        lamp.id(id).into_any_element()
    }
}
