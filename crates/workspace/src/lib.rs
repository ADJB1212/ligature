//! Top-level window views, pane layout, tabs, and feature coordination.

use gpui::{Context, Entity, Focusable, IntoElement, Render, Window, div, prelude::*};
use ligature_buffer::Buffer;
use ligature_editor::Editor;
use ligature_theme::{colors, spacing, typography};
use ligature_ui::SplitPane;

/// Root view for a Ligature window.
pub struct Workspace {
    panes: Entity<SplitPane>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| Editor::new(Buffer::new(), cx));
        window.focus(&editor.focus_handle(cx), cx);
        let preview = cx.new(|_| PreviewPlaceholder);
        Self {
            panes: cx.new(|_| SplitPane::new(editor.into(), preview.into())),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(ligature_theme::window_background())
            .text_color(colors().text)
            .text_size(typography::control_size())
            .line_height(typography::line_height())
            .child(self.panes.clone())
    }
}

struct PreviewPlaceholder;

impl Render for PreviewPlaceholder {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(spacing::medium())
            .p(spacing::large())
            .child("PDF Preview")
            .child(div().text_color(colors().muted_text).child("No compiled document."))
    }
}
