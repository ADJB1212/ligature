//! Top-level window views, pane layout, tabs, and feature coordination.

use gpui::{Context, IntoElement, Render, Window, div, prelude::*};

/// Root view for a Ligature window.
pub struct Workspace;

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(ligature_theme::window_background())
    }
}
