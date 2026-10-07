//! Top-level window views, document lifecycle, pane layout, and feature coordination.

mod lifecycle;
mod project;
mod session;

#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use gpui::{App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Render, Subscription, Task, Window, actions, div, prelude::*};
use ligature_buffer::Buffer;
use ligature_editor::Editor;
use ligature_fs::{FileState, FileWatcher, LoadedFile};
use ligature_theme::{colors, spacing, typography};
use ligature_ui::{Button, SplitPane, inline_error};

use lifecycle::Intent;

actions!(
    ligature_workspace,
    [New, Open, OpenProject, Save, SaveAs, Close, Quit, NextTab, PreviousTab, SelectRoot]
);

pub fn init(cx: &mut App) {
    let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    cx.bind_keys([
        KeyBinding::new(&format!("{modifier}-n"), New, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-o"), Open, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-s"), Save, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-shift-s"), SaveAs, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-w"), Close, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-q"), Quit, None),
        KeyBinding::new(&format!("{modifier}-shift-o"), OpenProject, Some("LigatureWorkspace")),
        KeyBinding::new("ctrl-tab", NextTab, Some("LigatureWorkspace")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("LigatureWorkspace")),
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
    tabs: Vec<Option<Tab>>,
    active_tab: usize,
    project: Option<ligature_project::Project>,
    directories: BTreeMap<PathBuf, ligature_project::Directory>,
    expanded: BTreeSet<PathBuf>,
    settings: ligature_settings::Settings,
    settings_errors: Vec<String>,
    includes: ligature_project::IncludeGraph,
    project_task: Option<Task<()>>,
    session_path: Option<PathBuf>,
}

struct Tab {
    document: Document,
    editor: Entity<Editor>,
    watcher: Option<FileWatcher>,
    watch_task: Option<Task<()>>,
    external_pending: bool,
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
            tabs: vec![None],
            active_tab: 0,
            project: None,
            directories: BTreeMap::new(),
            expanded: BTreeSet::new(),
            settings: ligature_settings::Settings::default(),
            settings_errors: Vec::new(),
            includes: ligature_project::IncludeGraph::default(),
            project_task: None,
            session_path: None,
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
        let reuse_empty = loaded.is_some()
            && self.tabs.len() == 1
            && self.document.as_ref().is_some_and(|document| document.path.is_none())
            && self.editor.read(cx).state().text().is_empty()
            && !self.dirty(cx);
        if reuse_empty {
            self.stop_watching();
        } else if self.document.is_some() {
            self.park_active();
            self.active_tab = self.tabs.len();
            self.tabs.push(None);
        } else {
            self.active_tab = 0;
            self.tabs.push(None);
        }
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
        self.editor = cx.new(|cx| Editor::new(Buffer::from_text(text), cx));
        self._editor_subscription = cx.observe(&self.editor, |_, _, cx| cx.notify());
        self.configure_active_editor(cx);
        let editor = self.editor.clone().into();
        self.panes.update(cx, |panes, cx| panes.set_leading(editor, cx));
        self.start_watching(window, cx);
        self.focus_editor(window, cx);
        self.update_membership();
    }

    fn close_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.document.is_none() {
            return;
        }
        self.stop_watching();
        self.document = None;
        self.tabs.remove(self.active_tab);
        if !self.tabs.is_empty() {
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
            self.restore_active(window, cx);
            self.update_membership();
            return;
        }
        let empty = cx.new(|_| EmptyEditor);
        self.panes.update(cx, |panes, cx| panes.set_leading(empty.into(), cx));
        self.update_membership();
    }

    fn park_active(&mut self) {
        if let Some(document) = self.document.take() {
            self.tabs[self.active_tab] = Some(Tab {
                document,
                editor: self.editor.clone(),
                watcher: self.watcher.take(),
                watch_task: self.watch_task.take(),
                external_pending: self.external_pending,
            });
            self.external_pending = false;
        }
    }

    fn restore_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.tabs[self.active_tab].take().expect("inactive tab exists");
        self.document = Some(tab.document);
        self.editor = tab.editor;
        self.watcher = tab.watcher;
        self.watch_task = tab.watch_task;
        self.external_pending = tab.external_pending;
        self._editor_subscription = cx.observe(&self.editor, |_, _, cx| cx.notify());
        let editor = self.editor.clone().into();
        self.panes.update(cx, |panes, cx| panes.set_leading(editor, cx));
        self.focus_editor(window, cx);
        cx.notify();
    }

    fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || index == self.active_tab || index >= self.tabs.len() {
            return;
        }
        self.switch_tab(index, window, cx);
        self.start(Intent::CheckExternal, window, cx);
    }

    fn switch_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == self.active_tab || index >= self.tabs.len() {
            return;
        }
        self.park_active();
        self.active_tab = index;
        self.restore_active(window, cx);
    }

    fn cycle_tab(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() < 2 {
            return;
        }
        let index = (self.active_tab + if backwards { self.tabs.len() - 1 } else { 1 }) % self.tabs.len();
        self.activate_tab(index, window, cx);
    }

    fn activate_path(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.document.as_ref().is_some_and(|document| document.path.as_deref() == Some(path)) {
            return true;
        }
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.as_ref().is_some_and(|tab| tab.document.path.as_deref() == Some(path)));
        if let Some(index) = index {
            self.park_active();
            self.active_tab = index;
            self.restore_active(window, cx);
            self.external_pending = true;
            true
        } else {
            false
        }
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
                let editor_id = self.editor.entity_id();
                self.watcher = Some(watcher);
                self.watch_task = Some(cx.spawn_in(window, async move |this, cx| {
                    while let Ok(state) = events.recv().await {
                        if this
                            .update_in(cx, |workspace, window, cx| {
                                if workspace.editor.entity_id() != editor_id {
                                    workspace.refresh_inactive(editor_id, state.clone(), cx);
                                    return;
                                }
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

    fn refresh_inactive(&mut self, editor_id: gpui::EntityId, state: FileState, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter_mut().flatten().find(|tab| tab.editor.entity_id() == editor_id) else {
            return;
        };
        if tab.document.disk == state {
            return;
        }
        if !tab.document.diverged
            && !tab.editor.read(cx).state().is_dirty()
            && let FileState::Present(bytes) = &state
            && let Ok(text) = ligature_fs::decode(tab.document.path.as_deref().expect("watched file has path"), bytes.clone())
        {
            tab.editor.update(cx, |editor, cx| editor.update_state(|state| state.reload(text), cx));
            tab.document.disk = state;
        } else {
            tab.external_pending = true;
        }
        cx.notify();
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
            .when(!self.busy, |view| {
                view.on_action(cx.listener(|this, _: &New, window, cx| this.start(Intent::New, window, cx)))
                    .on_action(cx.listener(|this, _: &Open, window, cx| this.start(Intent::Open, window, cx)))
                    .on_action(cx.listener(|this, _: &OpenProject, window, cx| this.start(Intent::OpenProject, window, cx)))
            })
            .when(!self.busy && self.tabs.len() > 1, |view| {
                view.on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(false, window, cx)))
                    .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.cycle_tab(true, window, cx)))
            })
            .when(
                !unavailable && self.project.is_some() && self.document.as_ref().is_some_and(|document| document.path.is_some()),
                |view| view.on_action(cx.listener(|this, _: &SelectRoot, window, cx| this.start(Intent::SelectRoot, window, cx))),
            )
            .when(!unavailable, |view| {
                view.on_action(cx.listener(|this, _: &Save, window, cx| this.start(Intent::Save, window, cx)))
                    .on_action(cx.listener(|this, _: &SaveAs, window, cx| this.start(Intent::SaveAs, window, cx)))
                    .on_action(cx.listener(|this, _: &Close, window, cx| this.start(Intent::Close, window, cx)))
            })
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
                        Button::new("open-project", "Open Folder")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.start(Intent::OpenProject, window, cx))),
                    )
                    .child(
                        Button::new("root-document", "Set Root")
                            .disabled(unavailable || self.project.is_none() || self.document.as_ref().is_none_or(|document| document.path.is_none()))
                            .on_click(cx.listener(|this, _, window, cx| this.start(Intent::SelectRoot, window, cx))),
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
            .children(self.settings_errors.iter().map(|error| inline_error(error.clone())))
            .child(
                div()
                    .id("tabs")
                    .flex()
                    .overflow_x_scroll()
                    .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                        let (document, editor) = tab.as_ref().map_or_else(
                            || (self.document.as_ref().expect("active tab exists"), &self.editor),
                            |tab| (&tab.document, &tab.editor),
                        );
                        let dirty = document.diverged || editor.read(cx).state().is_dirty();
                        div()
                            .id(("tab", index))
                            .flex()
                            .items_center()
                            .gap(spacing::small())
                            .p(spacing::medium())
                            .when(index == self.active_tab, |view| view.bg(colors().surface))
                            .child(
                                Button::new(("activate-tab", index), format!("{}{}", document.name(), if dirty { " *" } else { "" }))
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, window, cx| this.activate_tab(index, window, cx))),
                            )
                            .child(
                                Button::new(("close-tab", index), "x")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start(Intent::CloseTab(index), window, cx);
                                    })),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(gpui::px(0.0))
                    .when(self.project.is_some(), |view| view.child(self.render_tree(cx)))
                    .child(div().flex_1().min_w(gpui::px(0.0)).child(self.panes.clone())),
            )
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
