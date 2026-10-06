//! Top-level window views, document lifecycle, pane layout, and feature coordination.

mod lifecycle;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use gpui::{App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Render, Subscription, Task, Window, actions, div, prelude::*};
use ligature_buffer::Buffer;
use ligature_editor::Editor;
use ligature_fs::{FileState, FileWatcher, LoadedFile};
use ligature_theme::{colors, spacing, typography};
use ligature_ui::{Button, SplitPane, inline_error};

use lifecycle::Intent;

actions!(ligature_workspace, [New, Open, Save, SaveAs, Close, Quit]);

pub fn init(cx: &mut App) {
    let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    cx.bind_keys([
        KeyBinding::new(&format!("{modifier}-n"), New, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-o"), Open, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-s"), Save, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-shift-s"), SaveAs, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-w"), Close, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-q"), Quit, None),
    ]);
}

struct Document {
    path: Option<PathBuf>,
    disk: FileState,
    diverged: bool,
}

impl Document {
    fn untitled() -> Self {
        Self {
            path: None,
            disk: FileState::Missing,
            diverged: false,
        }
    }

    fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .map_or_else(|| "Untitled.tex".to_owned(), |name| name.to_string_lossy().into_owned())
    }
}

pub struct Workspace {
    focus: FocusHandle,
    editor: Entity<Editor>,
    panes: Entity<SplitPane>,
    document: Option<Document>,
    busy: bool,
    external_pending: bool,
    error: Option<String>,
    watcher: Option<FileWatcher>,
    watch_task: Option<Task<()>>,
    _editor_subscription: Subscription,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| Editor::new(Buffer::new(), cx));
        window.focus(&editor.focus_handle(cx), cx);
        let preview = cx.new(|_| PreviewPlaceholder);
        let panes = cx.new(|_| SplitPane::new(editor.clone().into(), preview.into()));
        let subscription = cx.observe(&editor, |_, _, cx| cx.notify());
        let entity = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            entity.update(cx, |workspace, cx| workspace.start(Intent::CloseWindow, window, cx)).ok();
            false
        });
        Self {
            focus: cx.focus_handle(),
            editor,
            panes,
            document: Some(Document::untitled()),
            busy: false,
            external_pending: false,
            error: None,
            watcher: None,
            watch_task: None,
            _editor_subscription: subscription,
        }
    }

    pub fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start(Intent::Quit, window, cx);
    }

    fn dirty(&self, cx: &App) -> bool {
        self.document
            .as_ref()
            .is_some_and(|document| document.diverged || self.editor.read(cx).state().is_dirty())
    }

    fn focus_editor(&self, window: &mut Window, cx: &mut App) {
        if self.document.is_some() {
            window.focus(&self.editor.focus_handle(cx), cx);
        } else {
            window.focus(&self.focus, cx);
        }
    }

    fn replace_document(&mut self, loaded: Option<LoadedFile>, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_watching();
        let (document, text) = if let Some(loaded) = loaded {
            let disk = FileState::Present(loaded.text.as_bytes().to_vec());
            (
                Document {
                    path: Some(loaded.path),
                    disk,
                    diverged: false,
                },
                loaded.text,
            )
        } else {
            (Document::untitled(), String::new())
        };
        self.document = Some(document);
        self.editor.update(cx, |editor, cx| editor.set_buffer(Buffer::from_text(text), cx));
        let editor = self.editor.clone().into();
        self.panes.update(cx, |panes, cx| panes.set_leading(editor, cx));
        self.start_watching(window, cx);
        self.focus_editor(window, cx);
    }

    fn close_document(&mut self, cx: &mut Context<Self>) {
        self.stop_watching();
        self.document = None;
        self.editor.update(cx, |editor, cx| editor.set_buffer(Buffer::new(), cx));
        let empty = cx.new(|_| EmptyEditor);
        self.panes.update(cx, |panes, cx| panes.set_leading(empty.into(), cx));
    }

    fn stop_watching(&mut self) {
        self.watch_task = None;
        self.watcher = None;
        self.external_pending = false;
    }

    fn start_watching(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_watching();
        let Some(document) = &self.document else {
            return;
        };
        let Some(path) = &document.path else {
            return;
        };
        match FileWatcher::new(path.clone(), document.disk.clone()) {
            Ok(watcher) => {
                let events = watcher.events();
                self.watcher = Some(watcher);
                self.watch_task = Some(cx.spawn_in(window, async move |this, cx| {
                    while let Ok(state) = events.recv().await {
                        if this
                            .update_in(cx, |workspace, window, cx| {
                                if workspace.document.as_ref().is_some_and(|document| document.disk != state) {
                                    if workspace.busy {
                                        workspace.external_pending = true;
                                    } else {
                                        workspace.start(Intent::CheckExternal, window, cx);
                                    }
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
            }
            Err(error) => self.error = Some(format!("Could not watch '{}': {error}", path.display())),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.dirty(cx);
        let name = self.document.as_ref().map_or_else(|| "No document".to_owned(), Document::name);
        let title = format!("{name}{} - Ligature", if dirty { " *" } else { "" });
        window.set_window_title(&title);
        let unavailable = self.busy || self.document.is_none();
        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context("LigatureWorkspace")
            .track_focus(&self.focus)
            .bg(ligature_theme::window_background())
            .text_color(colors().text)
            .text_size(typography::control_size())
            .line_height(typography::line_height())
            .on_action(cx.listener(|this, _: &New, window, cx| this.start(Intent::New, window, cx)))
            .on_action(cx.listener(|this, _: &Open, window, cx| this.start(Intent::Open, window, cx)))
            .on_action(cx.listener(|this, _: &Save, window, cx| this.start(Intent::Save, window, cx)))
            .on_action(cx.listener(|this, _: &SaveAs, window, cx| this.start(Intent::SaveAs, window, cx)))
            .on_action(cx.listener(|this, _: &Close, window, cx| this.start(Intent::Close, window, cx)))
            .on_action(cx.listener(|this, _: &Quit, window, cx| this.request_quit(window, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(spacing::small())
                    .p(spacing::medium())
                    .flex_shrink_0()
                    .child(
                        Button::new("new", "New")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.start(Intent::New, window, cx))),
                    )
                    .child(
                        Button::new("open", "Open")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.start(Intent::Open, window, cx))),
                    )
                    .child(
                        Button::new("save", "Save")
                            .disabled(unavailable)
                            .on_click(cx.listener(|this, _, window, cx| this.start(Intent::Save, window, cx))),
                    )
                    .child(
                        Button::new("save-as", "Save As")
                            .disabled(unavailable)
                            .on_click(cx.listener(|this, _, window, cx| this.start(Intent::SaveAs, window, cx))),
                    )
                    .child(
                        Button::new("close", "Close")
                            .disabled(unavailable)
                            .on_click(cx.listener(|this, _, window, cx| this.start(Intent::Close, window, cx))),
                    )
                    .child(div().ml(spacing::medium()).child(name).when(dirty, |view| view.child(" *"))),
            )
            .when_some(self.error.clone(), |view, error| view.child(inline_error(error)))
            .child(div().flex_1().min_h(gpui::px(0.0)).child(self.panes.clone()))
    }
}

struct EmptyEditor;
impl Render for EmptyEditor {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p(spacing::large()).child("Create or open a document.")
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
