//! Reusable GPUI controls such as buttons, inputs, menus, and panels. Feature views belong to their own crates.

mod button;
mod input;
mod split_pane;

pub use button::Button;
pub use input::TextInput;
pub use split_pane::SplitPane;

use gpui::{App, IntoElement, KeyBinding, Window, actions, div, prelude::*};
use ligature_theme::{colors, spacing, typography};

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

pub fn inline_error(message: impl Into<gpui::SharedString>) -> impl IntoElement {
    let colors = colors();
    div()
        .flex()
        .gap(spacing::medium())
        .p(spacing::medium())
        .rounded(spacing::radius())
        .bg(colors.error_surface)
        .text_color(colors.error)
        .text_size(typography::small_size())
        .line_height(typography::line_height())
        .child("Error:")
        .child(message.into())
}
