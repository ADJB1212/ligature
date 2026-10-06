//! Top-level window views, pane layout, tabs, and feature coordination.

use gpui::{Context, Entity, IntoElement, Render, SharedString, Window, div, prelude::*};
use ligature_theme::{colors, spacing, typography};
use ligature_ui::{Button, SplitPane, TextInput, inline_error};

/// Root view for a Ligature window.
pub struct Workspace {
    panes: Entity<SplitPane>,
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let controls = cx.new(Controls::new);
        let preview = cx.new(|_| PreviewPlaceholder);
        Self {
            panes: cx.new(|_| SplitPane::new(controls.into(), preview.into())),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("LigatureControls")
            .on_action(ligature_ui::focus_next)
            .on_action(ligature_ui::focus_previous)
            .size_full()
            .bg(ligature_theme::window_background())
            .text_color(colors().text)
            .text_size(typography::control_size())
            .line_height(typography::line_height())
            .child(self.panes.clone())
    }
}

struct Controls {
    name: Entity<TextInput>,
    title: SharedString,
    error: Option<SharedString>,
}

impl Controls {
    fn new(cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            let mut input = TextInput::new("Document name", cx);
            input.set_text("Untitled.tex", cx);
            input
        });
        Self {
            name,
            title: "Untitled.tex".into(),
            error: None,
        }
    }
}

impl Render for Controls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(spacing::large())
            .p(spacing::large())
            .bg(colors().surface)
            .child(self.title.clone())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing::small())
                    .child("Document name")
                    .child(self.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap(spacing::medium())
                    .child(Button::new("apply-name", "Apply").on_click(cx.listener(|this, _, _, cx| {
                        let name = this.name.read(cx).text().trim();
                        if name.is_empty() {
                            this.error = Some("Enter a document name.".into());
                        } else {
                            this.title = name.to_owned().into();
                            this.error = None;
                        }
                        cx.notify();
                    })))
                    .child(Button::new("build", "Build").disabled(true)),
            )
            .when_some(self.error.clone(), |view, error| view.child(inline_error(error)))
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
