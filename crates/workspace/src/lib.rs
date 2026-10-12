//! Top-level window views, document lifecycle, pane layout, and feature coordination.

mod builds;
mod export;
mod language;
mod lifecycle;
mod project;
mod resources;
mod search;
mod semantics;
mod session;

#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Pixels, Render, Subscription, Task, Window, actions, div, prelude::*, px,
};
use ligature_buffer::Buffer;
use ligature_editor::Editor;
use ligature_fs::{FileState, FileWatcher, LoadedFile};
use ligature_theme::{colors, spacing, typography};
use ligature_ui::{Button, Indicator, bottom_pane, caption, chrome_strip, indicator, inline_error, split_pane, vertical_seam};

pub use ligature_preview::{FitPage, FitWidth, ZoomIn, ZoomOut};

use lifecycle::Intent;

const NEW_DOCUMENT: &str = "\\documentclass{article}\n\n\\begin{document}\n\\null\n\n\\end{document}\n";

actions!(
    ligature_workspace,
    [
        New,
        Open,
        OpenProject,
        Save,
        SaveAs,
        ExportPdf,
        Close,
        Quit,
        NextTab,
        PreviousTab,
        SelectRoot,
        Find,
        FindNext,
        FindPrevious,
        Replace,
        FindInProject,
        HideSearch,
        Build,
        CancelBuild,
        ShowInPreview,
        ToggleBuildOutput,
        ToggleProject,
        ToggleOutline,
        ToggleTexResources,
        GoToDefinition,
        RestartLanguageServer,
        ToggleLanguageServer
    ]
);

pub fn init(cx: &mut App) {
    ligature_preview::init(cx);
    let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    cx.bind_keys([
        KeyBinding::new(&format!("{modifier}-n"), New, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-o"), Open, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-s"), Save, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-shift-s"), SaveAs, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-shift-e"), ExportPdf, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-w"), Close, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-q"), Quit, None),
        KeyBinding::new(&format!("{modifier}-shift-o"), OpenProject, Some("LigatureWorkspace")),
        KeyBinding::new("ctrl-tab", NextTab, Some("LigatureWorkspace")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-f"), Find, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-alt-f"), Replace, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-shift-f"), FindInProject, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-g"), FindNext, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-shift-g"), FindPrevious, Some("LigatureWorkspace")),
        KeyBinding::new("f3", FindNext, Some("LigatureWorkspace")),
        KeyBinding::new("shift-f3", FindPrevious, Some("LigatureWorkspace")),
        KeyBinding::new("enter", FindNext, Some("LigatureSearch")),
        KeyBinding::new("shift-enter", FindPrevious, Some("LigatureSearch")),
        KeyBinding::new("escape", HideSearch, Some("LigatureSearch")),
        KeyBinding::new(&format!("{modifier}-b"), Build, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-shift-b"), CancelBuild, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-shift-j"), ShowInPreview, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-j"), ToggleBuildOutput, Some("LigatureWorkspace")),
        KeyBinding::new("f12", GoToDefinition, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-shift-l"), ToggleOutline, Some("LigatureWorkspace")),
        KeyBinding::new(&format!("{modifier}-alt-1"), ToggleProject, Some("LigatureWorkspace")),
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
    pane_ratio: f32,
    preview: Entity<ligature_preview::Preview>,
    _preview_subscription: Subscription,
    _preview_observation: Subscription,
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
    project_visible: bool,
    directories: BTreeMap<PathBuf, ligature_project::Directory>,
    expanded: BTreeSet<PathBuf>,
    settings: ligature_settings::Settings,
    settings_errors: Vec<String>,
    includes: ligature_project::IncludeGraph,
    project_task: Option<Task<()>>,
    session_path: Option<PathBuf>,
    search: search::SearchUi,
    bundle_ui: resources::BundleUi,
    build_ui: builds::BuildUi,
    build_log: Entity<Editor>,
    build_output_height: Pixels,
    semantic_ui: semantics::SemanticUi,
    language_ui: language::LanguageUi,
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
        let editor = cx.new(|cx| Editor::new(Buffer::from_text(NEW_DOCUMENT), cx));
        let build_log = cx.new(|cx| {
            let mut log = Editor::read_only(Buffer::new(), cx);
            log.set_font(None, typography::control_size(), cx);
            log
        });
        window.focus(&editor.focus_handle(cx), cx);
        let preview = cx.new(ligature_preview::Preview::new);
        let preview_subscription = cx.subscribe_in(
            &preview,
            window,
            |workspace, _, event: &ligature_preview::InverseNavigation, window, cx| {
                workspace.start(Intent::OpenSource(event.0.clone()), window, cx);
            },
        );
        let mut export_available = false;
        let preview_observation = cx.observe(&preview, move |_, preview, cx| {
            let available = preview.read(cx).pdf_snapshot().is_some();
            if available != export_available {
                export_available = available;
                cx.notify();
            }
        });
        let subscription = cx.observe(&editor, |workspace, _, cx| workspace.editor_changed(cx));
        let search = search::SearchUi::new(cx);
        let entity = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            entity.update(cx, |workspace, cx| workspace.start(Intent::CloseWindow, window, cx)).ok();
            false
        });
        let mut workspace = Self {
            focus: cx.focus_handle(),
            editor,
            pane_ratio: 0.5,
            preview,
            _preview_subscription: preview_subscription,
            _preview_observation: preview_observation,
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
            project_visible: true,
            directories: BTreeMap::new(),
            expanded: BTreeSet::new(),
            settings: ligature_settings::Settings::default(),
            settings_errors: Vec::new(),
            includes: ligature_project::IncludeGraph::default(),
            project_task: None,
            session_path: None,
            search,
            bundle_ui: resources::BundleUi::default(),
            build_ui: builds::BuildUi::default(),
            build_log,
            build_output_height: px(240.0),
            semantic_ui: semantics::SemanticUi::default(),
            language_ui: language::LanguageUi::default(),
        };
        workspace.queue_semantics(cx);
        workspace.queue_language_server(window, cx);
        workspace
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
        self.editor.update(cx, |editor, _| editor.dismiss_assistance());
        let reuse_starter = loaded.is_some()
            && self.tabs.len() == 1
            && self.document.as_ref().is_some_and(|document| document.path.is_none())
            && matches!(self.editor.read(cx).state().text(), "" | NEW_DOCUMENT)
            && !self.dirty(cx);
        if reuse_starter {
            self.stop_watching(cx);
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
            (Document::untitled(), NEW_DOCUMENT.to_owned())
        };
        self.document = Some(document);
        self.editor = cx.new(|cx| Editor::new(Buffer::from_text(text), cx));
        self._editor_subscription = cx.observe(&self.editor, |workspace, _, cx| workspace.editor_changed(cx));
        self.configure_active_editor(cx);
        self.start_watching(window, cx);
        self.focus_editor(window, cx);
        self.update_membership();
        self.editor_changed(cx);
    }

    fn close_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.document.is_none() {
            return;
        }
        self.stop_watching(cx);
        self.document = None;
        self.tabs.remove(self.active_tab);
        if !self.tabs.is_empty() {
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
            self.restore_active(window, cx);
            self.update_membership();
            return;
        }
        self.update_membership();
        self.search_sources_changed(cx);
        self.refresh_preview_staleness(cx);
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
        self._editor_subscription = cx.observe(&self.editor, |workspace, _, cx| workspace.editor_changed(cx));
        self.focus_editor(window, cx);
        self.editor_changed(cx);
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
        self.editor.update(cx, |editor, _| editor.dismiss_assistance());
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

    fn stop_watching(&mut self, cx: &mut Context<Self>) {
        self.watch_task = None;
        if let Some(watcher) = self.watcher.take() {
            cx.background_executor().spawn(async move { drop(watcher) }).detach();
        }
        self.external_pending = false;
    }

    fn start_watching(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_watching(cx);
        let Some(document) = &self.document else {
            return;
        };
        let Some(path) = &document.path else {
            return;
        };
        let path = path.clone();
        let watched_path = path.clone();
        let initial = document.disk.clone();
        let editor_id = self.editor.entity_id();
        self.watch_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { FileWatcher::new(watched_path, initial) })
                .await;
            let events = this.update_in(cx, |workspace, _, cx| {
                let target = if workspace.editor.entity_id() == editor_id
                    && workspace.document.as_ref().is_some_and(|document| document.path.as_ref() == Some(&path))
                {
                    Some(&mut workspace.watcher)
                } else {
                    workspace
                        .tabs
                        .iter_mut()
                        .flatten()
                        .find(|tab| tab.editor.entity_id() == editor_id && tab.document.path.as_ref() == Some(&path))
                        .map(|tab| &mut tab.watcher)
                }?;
                match result {
                    Ok(watcher) => {
                        let events = watcher.events();
                        *target = Some(watcher);
                        Some(events)
                    }
                    Err(error) => {
                        workspace.error = Some(format!("Could not watch '{}': {error}", path.display()));
                        cx.notify();
                        None
                    }
                }
            });
            let Ok(Some(events)) = events else { return };
            while let Ok(state) = events.recv().await {
                if this
                    .update_in(cx, |workspace, window, cx| {
                        if workspace.editor.entity_id() != editor_id {
                            workspace.refresh_inactive(editor_id, state, cx);
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
        self.search_sources_changed(cx);
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
            .when(!self.busy && self.preview.read(cx).pdf_snapshot().is_some(), |view| {
                view.on_action(cx.listener(|this, _: &ExportPdf, window, cx| this.start(Intent::ExportPdf, window, cx)))
            })
            .when(!self.busy && self.can_build() && !self.build_ui.running(), |view| {
                view.on_action(cx.listener(|this, _: &Build, window, cx| this.start_build(window, cx)))
            })
            .when(self.build_ui.running(), |view| {
                view.on_action(cx.listener(|this, _: &CancelBuild, _, cx| this.cancel_build(cx)))
            })
            .on_action(cx.listener(|this, _: &ShowInPreview, _, cx| this.show_in_preview(cx)))
            .on_action(cx.listener(|this, _: &ToggleBuildOutput, window, cx| this.toggle_build_output(window, cx)))
            .when(self.project.is_some(), |view| {
                view.on_action(cx.listener(|this, _: &ToggleProject, window, cx| {
                    this.project_visible = !this.project_visible;
                    this.focus_editor(window, cx);
                    cx.notify();
                }))
            })
            .on_action(cx.listener(|this, _: &ToggleOutline, _, cx| {
                this.semantic_ui.visible = !this.semantic_ui.visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GoToDefinition, window, cx| this.go_to_definition(window, cx)))
            .on_action(cx.listener(|this, _: &RestartLanguageServer, _, cx| this.restart_language_server(cx)))
            .on_action(cx.listener(|this, _: &ToggleLanguageServer, _, cx| {
                this.language_ui.visible = !this.language_ui.visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleTexResources, window, cx| {
                this.bundle_ui.visible = !this.bundle_ui.visible;
                if this.bundle_ui.visible {
                    this.check_installed_bundle(window, cx);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.preview.update(cx, |preview, cx| preview.zoom_in(cx))))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.preview.update(cx, |preview, cx| preview.zoom_out(cx))))
            .on_action(cx.listener(|this, _: &FitWidth, _, cx| {
                this.preview
                    .update(cx, |preview, cx| preview.set_zoom(ligature_preview::Zoom::FitWidth, cx));
            }))
            .on_action(cx.listener(|this, _: &FitPage, _, cx| {
                this.preview
                    .update(cx, |preview, cx| preview.set_zoom(ligature_preview::Zoom::FitPage, cx));
            }))
            .when(!unavailable, |view| {
                view.on_action(cx.listener(|this, _: &Find, window, cx| this.show_search(false, false, window, cx)))
                    .on_action(cx.listener(|this, _: &Replace, window, cx| this.show_search(false, true, window, cx)))
            })
            .when(self.project.is_some(), |view| {
                view.on_action(cx.listener(|this, _: &FindInProject, window, cx| this.show_search(true, false, window, cx)))
            })
            .when(self.search.visible, |view| {
                view.on_action(cx.listener(|this, _: &HideSearch, window, cx| this.hide_search(window, cx)))
                    .on_action(cx.listener(|this, _: &FindNext, window, cx| this.navigate_search(false, window, cx)))
                    .on_action(cx.listener(|this, _: &FindPrevious, window, cx| this.navigate_search(true, window, cx)))
            })
            .child(self.render_toolbar(cx))
            .when(self.language_ui.visible, |view| view.child(self.render_language_server()))
            .when(self.bundle_ui.visible, |view| view.child(self.render_resources(cx)))
            .when_some(self.error.clone(), |view, error| view.child(inline_error(error)))
            .children(self.settings_errors.iter().map(|error| inline_error(error.clone())))
            .child({
                let editor = if self.document.is_some() {
                    self.editor.clone().into_any_element()
                } else {
                    empty_editor().into_any_element()
                };
                let editor = if self.build_ui.visible {
                    bottom_pane(
                        editor,
                        self.render_build_output(cx),
                        self.build_output_height,
                        cx.listener(|this, height: &Pixels, _, cx| {
                            this.build_output_height = *height;
                            cx.notify();
                        }),
                    )
                    .into_any_element()
                } else {
                    editor
                };
                let editor_pane = div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(self.render_tabs(cx))
                    .when(self.search.visible, |view| view.child(self.render_search(cx)))
                    .child(div().flex().flex_col().flex_1().min_h(px(0.0)).child(editor));
                div()
                    .flex()
                    .flex_1()
                    .min_h(gpui::px(0.0))
                    .when(self.project.is_some() && self.project_visible, |view| {
                        view.child(self.render_tree(cx)).child(vertical_seam())
                    })
                    .when(self.semantic_ui.visible, |view| {
                        view.child(self.render_outline(cx)).child(vertical_seam())
                    })
                    .child(div().flex_1().min_w(gpui::px(0.0)).child(split_pane(
                        editor_pane,
                        self.preview.clone(),
                        self.pane_ratio,
                        cx.listener(|this, ratio: &f32, _, cx| {
                            this.pane_ratio = *ratio;
                            cx.notify();
                        }),
                    )))
            })
    }
}

impl Workspace {
    fn toggle_build_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.build_ui.visible = !self.build_ui.visible;
        if !self.build_ui.visible && self.build_log.focus_handle(cx).is_focused(window) {
            self.focus_editor(window, cx);
        }
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        chrome_strip()
            .id("workspace-toolbar")
            .debug_selector(|| "workspace-toolbar".into())
            .flex()
            .h(px(44.0))
            .items_center()
            .gap(spacing::large())
            .border_b_1()
            .border_color(colors().border)
            .child(
                Button::new("open-project", "Open Project")
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.start(Intent::OpenProject, window, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.project.as_ref().map_or_else(
                        || "Ligature".to_owned(),
                        |project| project.root.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(spacing::medium())
                    .when(
                        self.build_ui.running()
                            || self.bundle_ui.busy()
                            || (self.search.visible && (self.search.running || self.search.previewing || self.search.applying)),
                        |view| view.child(indicator("toolbar-live", Indicator::Live)).child(caption("Working...")),
                    )
                    .child(if self.build_ui.running() {
                        Button::new("cancel-build", "Stop").on_click(cx.listener(|this, _, _, cx| this.cancel_build(cx)))
                    } else {
                        Button::new("build", "Build")
                            .filled()
                            .disabled(self.busy || !self.can_build())
                            .on_click(cx.listener(|this, _, window, cx| this.start_build(window, cx)))
                    }),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = colors();
        div()
            .id("tabs")
            .debug_selector(|| "tabs".into())
            .flex()
            .overflow_x_scroll()
            .flex_shrink_0()
            .h(px(40.0))
            .items_center()
            .px(spacing::medium())
            .gap(spacing::small())
            .bg(colors.chrome)
            .border_b_1()
            .border_color(colors.border)
            .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                let (document, editor) = tab.as_ref().map_or_else(
                    || (self.document.as_ref().expect("active tab exists"), &self.editor),
                    |tab| (&tab.document, &tab.editor),
                );
                let dirty = document.diverged || editor.read(cx).state().is_dirty();
                let active = index == self.active_tab;
                let group = format!("tab-{index}");
                div()
                    .id(("tab", index))
                    .group(group.clone())
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .rounded(spacing::radius())
                    .when(active, |view| view.bg(colors.surface))
                    .when(!active, |view| view.hover(|style| style.bg(colors.hover)))
                    .child(
                        Button::new(("activate-tab", index), document.name())
                            .quiet()
                            .selected(active)
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| this.activate_tab(index, window, cx))),
                    )
                    .when(dirty, |view| view.child(indicator(("dirty-tab", index), Indicator::On)))
                    .child(
                        div()
                            .when(!active && !dirty, |view| view.invisible().group_hover(group, |style| style.visible()))
                            .child(Button::new(("close-tab", index), "×").quiet().disabled(self.busy).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.start(Intent::CloseTab(index), window, cx);
                                },
                            ))),
                    )
            }))
    }
}

fn empty_editor() -> impl IntoElement {
    let colors = colors();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(spacing::small())
        .bg(colors.background)
        .child(caption("DOCUMENT"))
        .child("Create or open a TeX file.")
}
