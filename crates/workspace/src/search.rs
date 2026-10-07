use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crate::{Workspace, lifecycle::Intent};
use gpui::{Context, Entity, Focusable, IntoElement, Subscription, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list};
use ligature_buffer::{DocumentId, Revision};
use ligature_editor::Editor;
use ligature_fs::FileState;
use ligature_search::{BufferVersion, Cancellation, FileReplacement, ProjectResults, Query, ReplacementPreview, SourceSnapshot};
use ligature_theme::{colors, spacing, typography};
use ligature_ui::{Button, TextInput, caption, inline_error};

pub(super) struct SearchUi {
    pub visible: bool,
    pub project_scope: bool,
    pub input: Entity<TextInput>,
    pub replacement: Entity<TextInput>,
    pub query: Query,
    replacement_value: String,
    error: Option<String>,
    pub results: Arc<ProjectResults>,
    pub preview: Option<ReplacementPreview>,
    pub preview_stale: bool,
    pub running: bool,
    pub previewing: bool,
    pub applying: bool,
    pub report: Option<ApplyReport>,
    observed: Option<(DocumentId, Revision, Option<String>)>,
    generation: u64,
    cancellation: Cancellation,
    task: Option<Task<()>>,
    rows: Vec<ResultRow>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy)]
enum ResultRow {
    File(usize),
    Match(usize, usize),
}

#[derive(Clone, Debug, Default)]
pub(super) struct ApplyReport {
    pub applied: usize,
    pub failures: BTreeMap<PathBuf, String>,
}

impl SearchUi {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        let input = cx.new(|cx| TextInput::new("Find literal text", cx));
        let replacement = cx.new(|cx| TextInput::new("Replace with", cx));
        let subscriptions = vec![
            cx.observe(&input, |workspace, _, cx| workspace.search_query_changed(cx)),
            cx.observe(&replacement, |workspace, _, cx| {
                let text = workspace.search.replacement.read(cx).text();
                if text == workspace.search.replacement_value {
                    return;
                }
                workspace.search.replacement_value = text.to_owned();
                if workspace.search.previewing {
                    workspace.search.cancel();
                }
                workspace.search.preview = None;
                workspace.search.report = None;
                cx.notify();
            }),
        ];
        Self {
            visible: false,
            project_scope: false,
            input,
            replacement,
            query: Query::default(),
            replacement_value: String::new(),
            error: None,
            results: Arc::new(ProjectResults::default()),
            preview: None,
            preview_stale: false,
            running: false,
            previewing: false,
            applying: false,
            report: None,
            observed: None,
            generation: 0,
            cancellation: Cancellation::default(),
            task: None,
            rows: Vec::new(),
            scroll: UniformListScrollHandle::default(),
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn cancel(&mut self) {
        self.cancellation.cancel();
        self.task = None;
        self.running = false;
        self.previewing = false;
        self.generation += 1;
    }

    fn set_results(&mut self, results: ProjectResults) {
        self.rows = results
            .files
            .iter()
            .enumerate()
            .flat_map(|(file, results)| {
                std::iter::once(ResultRow::File(file)).chain((0..results.matches.len()).map(move |matched| ResultRow::Match(file, matched)))
            })
            .collect();
        self.results = Arc::new(results);
        self.scroll = UniformListScrollHandle::default();
    }
}

impl Drop for SearchUi {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl Workspace {
    pub(super) fn search_sources_changed(&mut self, cx: &mut Context<Self>) {
        if self.search.applying {
            return;
        }
        if self.search.preview.is_some() {
            self.search.preview_stale = true;
        } else {
            self.queue_project_search(cx);
        }
    }
    pub(super) fn show_search(&mut self, project_scope: bool, replacement: bool, window: &mut Window, cx: &mut Context<Self>) {
        if project_scope && self.project.is_none() {
            return;
        }
        self.search.visible = true;
        self.search.project_scope = project_scope;
        self.search.preview = None;
        self.refresh_document_search(cx);
        if project_scope {
            self.queue_project_search(cx);
        } else {
            self.search.cancel();
        }
        window.focus(
            &if replacement {
                self.search.replacement.focus_handle(cx)
            } else {
                self.search.input.focus_handle(cx)
            },
            cx,
        );
        cx.notify();
    }

    pub(super) fn hide_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.visible = false;
        self.search.cancel();
        self.search.preview = None;
        self.refresh_document_search(cx);
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn search_query_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.search.input.read(cx).text();
        if self.search.query.text == text {
            return;
        }
        self.search.query.text = text.to_owned();
        self.search_options_changed(cx);
    }

    pub(super) fn search_options_changed(&mut self, cx: &mut Context<Self>) {
        self.search.preview = None;
        self.search.report = None;
        self.refresh_document_search(cx);
        self.queue_project_search(cx);
        cx.notify();
    }

    fn refresh_document_search(&mut self, cx: &mut Context<Self>) {
        let query = if self.search.visible {
            self.search.query.clone()
        } else {
            Query::default()
        };
        self.search.error = self.editor.update(cx, |editor, cx| editor.set_search(query, cx)).err();
    }

    pub(super) fn editor_changed(&mut self, cx: &mut Context<Self>) {
        let state = self.editor.read(cx).state();
        let observed = (
            state.buffer().id(),
            state.buffer().revision(),
            state.marked_range().map(|_| state.text().to_owned()),
        );
        let changed = self.search.observed.as_ref() != Some(&observed);
        if changed {
            self.refresh_preview_staleness(cx);
        }
        self.search.observed = Some(observed);
        self.refresh_document_search(cx);
        if changed && !self.search.applying {
            if let Some(preview) = &self.search.preview {
                let state = self.editor.read(cx).state();
                self.search.preview_stale |= preview.files.iter().any(|file| {
                    file.file.source.buffer.is_some_and(|version| {
                        version.document == state.buffer().id()
                            && (version.revision != state.buffer().revision() || file.file.source.text != state.text())
                    })
                });
            } else {
                self.queue_project_search(cx);
            }
        }
        cx.notify();
    }

    pub(super) fn navigate_search(&mut self, backwards: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if self.document.is_none() || self.search.project_scope {
            return;
        }
        self.editor.update(cx, |editor, cx| editor.navigate_match(backwards, cx));
        cx.notify();
    }

    pub(super) fn replace_document_matches(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.document.is_none() {
            return;
        }
        let replacement = self.search.replacement.read(cx).text().to_owned();
        self.editor.update(cx, |editor, cx| editor.replace_match(&replacement, all, cx));
        self.focus_editor(window, cx);
        cx.notify();
    }

    fn open_sources(&self, cx: &gpui::App) -> BTreeMap<PathBuf, SourceSnapshot> {
        let mut sources = BTreeMap::new();
        let mut add = |document: &crate::Document, editor: &Entity<Editor>| {
            if let Some(path) = &document.path {
                let Some(project) = &self.project else {
                    return;
                };
                if !ligature_project::eligible_search_path(&project.root, path, &project.root.join(&self.settings.build.output_directory)) {
                    return;
                }
                let state = editor.read(cx).state();
                sources.insert(
                    path.clone(),
                    SourceSnapshot {
                        text: state.text().to_owned(),
                        buffer: Some(BufferVersion {
                            document: state.buffer().id(),
                            revision: state.buffer().revision(),
                        }),
                        disk: document.disk.clone(),
                    },
                );
            }
        };
        if let Some(document) = &self.document {
            add(document, &self.editor);
        }
        for tab in self.tabs.iter().flatten() {
            add(&tab.document, &tab.editor);
        }
        sources
    }

    pub(super) fn queue_project_search(&mut self, cx: &mut Context<Self>) {
        self.search.cancel();
        if !self.search.visible || !self.search.project_scope || self.search.applying {
            return;
        }
        self.search.set_results(ProjectResults::default());
        self.search.preview = None;
        let Some(project) = &self.project else {
            return;
        };
        if self.search.query.text.is_empty() {
            return;
        }
        let root = project.root.clone();
        let output = root.join(&self.settings.build.output_directory);
        let mut open = self.open_sources(cx);
        let query = self.search.query.clone();
        let generation = self.search.generation;
        let cancellation = Cancellation::default();
        self.search.cancellation = cancellation.clone();
        self.search.running = true;
        self.search.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(120)).await;
            let result = cx
                .background_executor()
                .spawn(async move {
                    for (path, source) in &mut open {
                        if cancellation.is_canceled() {
                            return Ok(ProjectResults {
                                canceled: true,
                                ..ProjectResults::default()
                            });
                        }
                        source.disk = FileState::read(path);
                    }
                    ligature_search::search_project(&root, &output, &open, query, &cancellation)
                })
                .await;
            this.update(cx, |workspace, cx| {
                if workspace.search.generation != generation {
                    return;
                }
                workspace.search.running = false;
                match result {
                    Ok(results) if !results.canceled => workspace.search.set_results(results),
                    Ok(_) => {}
                    Err(error) => workspace.search.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn preview_project_replacements(&mut self, cx: &mut Context<Self>) {
        if self.search.running || self.search.previewing || self.search.results.query != self.search.query {
            return;
        }
        let results = Arc::clone(&self.search.results);
        let replacement = self.search.replacement.read(cx).text().to_owned();
        let generation = self.search.generation;
        let cancellation = Cancellation::default();
        self.search.cancellation = cancellation.clone();
        self.search.previewing = true;
        self.search.task = Some(cx.spawn(async move |this, cx| {
            let preview = cx
                .background_executor()
                .spawn(async move { results.preview_cancellable(&replacement, &cancellation) })
                .await;
            this.update(cx, |workspace, cx| {
                if workspace.search.generation != generation {
                    return;
                }
                workspace.search.previewing = false;
                workspace.search.preview = preview;
                cx.notify();
            })
            .ok();
        }));
        self.search.preview_stale = false;
        self.search.report = None;
        cx.notify();
    }

    fn editor_for_path(&self, path: &Path) -> Option<Entity<Editor>> {
        if self.document.as_ref().is_some_and(|document| document.path.as_deref() == Some(path)) {
            return Some(self.editor.clone());
        }
        self.tabs
            .iter()
            .flatten()
            .find(|tab| tab.document.path.as_deref() == Some(path))
            .map(|tab| tab.editor.clone())
    }

    fn apply_open_replacement(&mut self, replacement: &FileReplacement, cx: &mut Context<Self>) -> Result<bool, String> {
        let Some(editor) = self.editor_for_path(&replacement.file.path) else {
            if replacement.file.source.buffer.is_some() {
                return Err("The document was closed or moved since this preview".into());
            }
            return Ok(false);
        };
        let state = editor.read(cx).state();
        let version = BufferVersion {
            document: state.buffer().id(),
            revision: state.buffer().revision(),
        };
        if state.marked_range().is_some()
            || state.text() != replacement.file.source.text
            || replacement.file.source.buffer.is_some_and(|expected| expected != version)
            || (replacement.file.source.buffer.is_none() && state.is_dirty())
        {
            return Err("The open document changed since this preview; no replacements were applied".into());
        }
        editor.update(cx, |editor, cx| {
            editor.update_state(|state| state.replace(replacement.edit.range.clone(), &replacement.edit.text, false), cx)
        });
        Ok(true)
    }

    pub(super) fn apply_project_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(preview) = self.search.preview.take() else {
            return;
        };
        self.search.cancel();
        self.search.applying = true;
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let mut report = ApplyReport::default();
            for replacement in preview.files {
                let path = replacement.file.path.clone();
                let check_path = path.clone();
                let state = cx.background_executor().spawn(async move { FileState::read(&check_path) }).await;
                let result = if state != replacement.file.source.disk {
                    Err("The file changed on disk since this preview; no replacements were applied".into())
                } else {
                    match this.update_in(cx, |workspace, _, cx| workspace.apply_open_replacement(&replacement, cx)) {
                        Ok(Ok(true)) => Ok(()),
                        Ok(Ok(false)) => {
                            cx.background_executor()
                                .spawn(async move { ligature_search::apply_disk_replacement(&replacement) })
                                .await
                        }
                        Ok(Err(error)) => Err(error),
                        Err(error) => Err(error.to_string()),
                    }
                };
                match result {
                    Ok(()) => report.applied += 1,
                    Err(error) => {
                        report.failures.insert(path, error);
                    }
                }
            }
            this.update_in(cx, |workspace, window, cx| {
                workspace.busy = false;
                workspace.search.applying = false;
                workspace.search.report = Some(report);
                workspace.queue_project_search(cx);
                if workspace.external_pending {
                    workspace.external_pending = false;
                    workspace.start(Intent::CheckExternal, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (current, count) = self.editor.read(cx).search_status();
        let disabled = self.busy || self.document.is_none() || count == 0;
        let colors = colors();
        div()
            .key_context("LigatureSearch")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(spacing::small())
            .px(spacing::medium())
            .py(spacing::medium())
            .bg(colors.chrome)
            .when_some(self.search.error.clone(), |view, error| view.child(inline_error(error)))
            .child(
                div()
                    .key_context("LigatureControls")
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(spacing::small())
                    .on_action(ligature_ui::focus_next)
                    .on_action(ligature_ui::focus_previous)
                    .child(div().w(px(220.0)).child(self.search.input.clone()))
                    .child(div().w(px(220.0)).child(self.search.replacement.clone()))
                    .child(
                        Button::new("match-case", "Match Case")
                            .selected(self.search.query.options.case_sensitive)
                            .disabled(self.busy)
                            .on_click(cx.listener(|workspace, _, _, cx| {
                                workspace.search.query.options.case_sensitive = !workspace.search.query.options.case_sensitive;
                                workspace.search_options_changed(cx);
                            })),
                    )
                    .child(
                        Button::new("whole-word", "Whole Word")
                            .selected(self.search.query.options.whole_word)
                            .disabled(self.busy)
                            .on_click(cx.listener(|workspace, _, _, cx| {
                                workspace.search.query.options.whole_word = !workspace.search.query.options.whole_word;
                                workspace.search_options_changed(cx);
                            })),
                    )
                    .child(
                        Button::new("search-scope", if self.search.project_scope { "Project" } else { "Document" })
                            .selected(self.search.project_scope)
                            .disabled(self.busy || self.project.is_none())
                            .on_click(cx.listener(|workspace, _, window, cx| {
                                workspace.show_search(!workspace.search.project_scope, false, window, cx);
                            })),
                    )
                    .child(Button::new("close-search", "Done").on_click(cx.listener(|workspace, _, window, cx| workspace.hide_search(window, cx)))),
            )
            .when(!self.search.project_scope, |view| {
                view.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(spacing::small())
                        .items_center()
                        .child(caption(format!("{} of {count}", current.map_or(0, |index| index + 1))))
                        .child(
                            Button::new("previous-match", "Previous")
                                .disabled(disabled)
                                .on_click(cx.listener(|workspace, _, window, cx| workspace.navigate_search(true, window, cx))),
                        )
                        .child(
                            Button::new("next-match", "Next")
                                .disabled(disabled)
                                .on_click(cx.listener(|workspace, _, window, cx| workspace.navigate_search(false, window, cx))),
                        )
                        .child(
                            Button::new("replace-match", "Replace")
                                .disabled(disabled)
                                .on_click(cx.listener(|workspace, _, window, cx| workspace.replace_document_matches(false, window, cx))),
                        )
                        .child(
                            Button::new("replace-all", "Replace All")
                                .filled()
                                .disabled(disabled)
                                .on_click(cx.listener(|workspace, _, window, cx| workspace.replace_document_matches(true, window, cx))),
                        ),
                )
            })
            .when(self.search.project_scope, |view| {
                view.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(spacing::small())
                        .child(caption(if self.search.applying {
                            "Applying replacements...".to_owned()
                        } else if self.search.previewing {
                            "Preparing preview...".to_owned()
                        } else if self.search.running {
                            "Searching...".to_owned()
                        } else {
                            format!("{} matches in {} files", self.search.results.count(), self.search.results.files.len())
                        }))
                        .child(
                            Button::new("refresh-search", "Refresh")
                                .disabled(self.busy)
                                .on_click(cx.listener(|workspace, _, _, cx| {
                                    workspace.search.preview = None;
                                    workspace.queue_project_search(cx);
                                })),
                        )
                        .child(
                            Button::new("preview-replacement", "Preview Replacements")
                                .disabled(self.busy || self.search.running || self.search.previewing || self.search.results.count() == 0)
                                .on_click(cx.listener(|workspace, _, _, cx| workspace.preview_project_replacements(cx))),
                        ),
                )
                .when(self.search.preview.is_none() && !self.search.rows.is_empty(), |view| {
                    view.child(
                        uniform_list(
                            "project-search-results",
                            self.search.rows.len(),
                            cx.processor(|workspace, range: Range<usize>, _, cx| workspace.render_result_rows(range, cx)),
                        )
                        .track_scroll(&self.search.scroll)
                        .h(px(200.0)),
                    )
                })
                .when_some(self.search.preview.as_ref(), |view, preview| view.child(self.render_preview(preview, cx)))
                .children(
                    self.search
                        .results
                        .errors
                        .iter()
                        .map(|(path, error)| inline_error(format!("{}: {error}", path.display()))),
                )
            })
            .when_some(self.search.report.as_ref(), |view, report| {
                view.child(format!(
                    "Replaced {} files; {} files skipped. Open documents keep their changes unsaved.",
                    report.applied,
                    report.failures.len()
                ))
                .children(
                    report
                        .failures
                        .iter()
                        .map(|(path, error)| inline_error(format!("{}: {error}", path.display()))),
                )
            })
    }

    fn render_result_rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        range
            .filter_map(|index| self.search.rows.get(index).copied().map(|row| (index, row)))
            .map(|(index, row)| match row {
                ResultRow::File(file) => {
                    let file = &self.search.results.files[file];
                    div()
                        .h(px(28.0))
                        .flex()
                        .items_center()
                        .text_size(typography::small_size())
                        .text_color(colors().text)
                        .child(format!("{} ({} matches)", self.project_label(&file.path), file.matches.len()))
                        .into_any_element()
                }
                ResultRow::Match(file, matched) => {
                    let result = &self.search.results.files[file].matches[matched];
                    let generation = self.search.generation;
                    div()
                        .h(px(28.0))
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .child(
                            Button::new(("search-result", index), format!("{}:{}  {}", result.line, result.column, result.excerpt))
                                .quiet()
                                .disabled(self.busy)
                                .on_click(cx.listener(move |workspace, _, window, cx| {
                                    if workspace.search.generation != generation {
                                        return;
                                    }
                                    let result = &workspace.search.results.files[file];
                                    let intent = Intent::OpenSearchResult {
                                        path: result.path.clone(),
                                        original: result.source.text.clone(),
                                        range: result.matches[matched].range.clone(),
                                        query: workspace.search.results.query.clone(),
                                    };
                                    workspace.start(intent, window, cx);
                                })),
                        )
                        .into_any_element()
                }
            })
            .collect()
    }

    fn project_label(&self, path: &Path) -> String {
        self.project
            .as_ref()
            .and_then(|project| path.strip_prefix(&project.root).ok())
            .unwrap_or(path)
            .display()
            .to_string()
    }

    fn render_preview(&self, preview: &ReplacementPreview, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(spacing::small())
            .child(caption(
                "Review replacements. Open documents change in memory; other files are saved atomically.",
            ))
            .when(self.search.preview_stale, |view| {
                view.child(caption("Documents changed since this preview. Changed files will be skipped."))
            })
            .child(
                uniform_list(
                    "replacement-preview",
                    self.search.rows.len(),
                    cx.processor(|workspace, range: Range<usize>, _, cx| workspace.render_preview_rows(range, cx)),
                )
                .h(px(200.0)),
            )
            .child(
                Button::new("apply-project-replacement", format!("Apply to {} Files", preview.files.len()))
                    .filled()
                    .disabled(self.busy || preview.files.is_empty())
                    .on_click(cx.listener(|workspace, _, window, cx| workspace.apply_project_preview(window, cx))),
            )
    }

    fn render_preview_rows(&self, range: Range<usize>, _cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let Some(preview) = &self.search.preview else {
            return Vec::new();
        };
        range
            .filter_map(|index| self.search.rows.get(index))
            .map(|row| match *row {
                ResultRow::File(index) => {
                    let file = &preview.files[index];
                    div()
                        .h(px(28.0))
                        .flex()
                        .items_center()
                        .text_size(typography::small_size())
                        .child(format!("{}: {} replacements", self.project_label(&file.file.path), file.edit.count))
                        .into_any_element()
                }
                ResultRow::Match(file, matched) => {
                    let file = &preview.files[file];
                    div()
                        .h(px(48.0))
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .child(caption(format!(
                            "Before {}: {}",
                            file.file.matches[matched].line, file.file.matches[matched].excerpt
                        )))
                        .child(format!(
                            "After {}: {}",
                            file.after_matches[matched].line, file.after_matches[matched].excerpt
                        ))
                        .into_any_element()
                }
            })
            .collect()
    }
}
