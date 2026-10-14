use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use gpui::{AsyncWindowContext, Context, IntoElement, PathPromptOptions, WeakEntity, Window, div, prelude::*, px};
use ligature_fs::TextCache;
use ligature_project::{IncludeGraph, Project};
use ligature_settings::Settings;
use ligature_theme::{colors, spacing};
use ligature_ui::{Button, caption, inline_error, panel_header};

use crate::{Workspace, lifecycle::Intent};

pub(super) async fn open_project(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let picker = cx
        .update(|_, cx| {
            cx.prompt_for_paths(PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Open a project folder".into()),
            })
        })
        .map_err(|error| error.to_string())?;
    let Some(paths) = picker.await.map_err(|error| error.to_string())?.map_err(|error| error.to_string())? else {
        return Ok(());
    };
    let Some(path) = paths.into_iter().next() else {
        return Ok(());
    };
    load_project(this, cx, path).await
}

pub(super) async fn load_project(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext, path: PathBuf) -> Result<(), String> {
    let (configuration, configuration_errors) = this
        .update_in(cx, |workspace, _, _| {
            (workspace.configuration.clone(), workspace.configuration_errors.clone())
        })
        .map_err(|error| error.to_string())?;
    let expected_configuration = configuration.clone();
    let (project, settings, errors) = cx
        .background_executor()
        .spawn(async move {
            let mut project = Project::open(&path)?;
            let mut settings = configuration.settings_for(Some(&project.root));
            let mut errors = configuration_errors;
            errors.extend(discover_root(
                &mut project,
                &mut settings,
                None,
                &BTreeMap::new(),
                &mut TextCache::default(),
            ));
            Ok::<_, String>((project, settings, errors))
        })
        .await?;
    this.update_in(cx, |workspace, window, cx| {
        workspace.clear_language_server(cx);
        workspace.language_ui.stop();
        workspace.queue_language_server(window, cx);
        workspace.semantic_ui.cancel();
        workspace.build_ui.reset();
        workspace.refresh_diagnostics(cx);
        workspace.preview.update(cx, |preview, cx| preview.clear(cx));
        workspace.project_cancel.cancel();
        workspace.project_task = None;
        workspace.project_disk = Arc::default();
        workspace.expanded.clear();
        workspace.directories.clear();
        workspace.expanded.insert(project.root.clone());
        workspace.project = Some(project);
        if workspace.configuration == expected_configuration {
            workspace.settings = settings;
            workspace.settings_errors = errors;
        } else {
            workspace.settings = workspace
                .configuration
                .settings_for(workspace.project.as_ref().map(|project| project.root.as_path()));
            workspace.settings_errors.clone_from(&workspace.configuration_errors);
        }
        workspace.apply_settings(cx);
        workspace.queue_semantics(cx);
        workspace.update_membership();
        workspace.start_project_refresh(window, cx);
        workspace.search.preview = None;
        workspace.queue_project_search(cx);
        cx.notify();
    })
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) type OpenText = BTreeMap<PathBuf, Arc<str>>;

pub(super) fn discover_root(
    project: &mut Project, settings: &mut Settings, active: Option<&Path>, text: &OpenText, cache: &mut TextCache,
) -> Vec<String> {
    discover_root_cancellable(project, settings, active, text, cache, || false)
}

fn discover_root_cancellable(
    project: &mut Project, settings: &mut Settings, active: Option<&Path>, text: &OpenText, cache: &mut TextCache, canceled: impl Fn() -> bool,
) -> Vec<String> {
    project.root_document = None;
    let mut errors = Vec::new();
    if let Some(root) = &settings.root_document
        && let Err(error) = project.set_root_document(root)
    {
        errors.push(error);
        settings.root_document = None;
    }
    project.discover_root_cancellable(
        &project.root.join(&settings.build.output_directory),
        active,
        |path| {
            text.get(path)
                .cloned()
                .map_or_else(|| cache.read(path).map_err(|error| error.to_string()), Ok)
        },
        canceled,
    );
    errors
}

impl Workspace {
    /// Immutable text of an open buffer, reused until the buffer's revision changes.
    pub(super) fn cached_text(&mut self, path: PathBuf, editor: &ligature_editor::Editor) -> Arc<str> {
        let buffer = editor.state().buffer();
        let version = (buffer.id(), buffer.revision());
        let source = self
            .open_text
            .entry(path)
            .or_insert_with(|| (version.0, version.1, Arc::from(buffer.text())));
        if (source.0, source.1) != version {
            *source = (version.0, version.1, Arc::from(buffer.text()));
        }
        Arc::clone(&source.2)
    }

    pub(super) fn project_text(&mut self, cx: &gpui::App) -> OpenText {
        let mut text = BTreeMap::new();
        if let Some(path) = self.document.as_ref().and_then(|document| document.path.clone()) {
            let editor = self.editor.clone();
            text.insert(path.clone(), self.cached_text(path, editor.read(cx)));
        }
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .flatten()
            .filter_map(|tab| tab.document.path.clone().map(|path| (path, tab.editor.clone())))
            .collect();
        for (path, editor) in tabs {
            text.insert(path.clone(), self.cached_text(path, editor.read(cx)));
        }
        text
    }

    pub(super) fn apply_settings(&mut self, cx: &mut Context<Self>) {
        self.configure_active_editor(cx);
        for tab in self.tabs.iter().flatten() {
            tab.editor.update(cx, |editor, cx| configure(editor, &self.settings, cx));
        }
    }

    pub(super) fn configure_active_editor(&mut self, cx: &mut Context<Self>) {
        let language = self
            .document
            .as_ref()
            .and_then(|document| document.path.as_ref())
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .map_or(ligature_syntax::Language::Tex, |extension| {
                if extension.eq_ignore_ascii_case("bib") {
                    ligature_syntax::Language::Bibliography
                } else if extension.eq_ignore_ascii_case("txt") {
                    ligature_syntax::Language::Plain
                } else {
                    ligature_syntax::Language::Tex
                }
            });
        self.editor.update(cx, |editor, cx| {
            editor.set_language(language, cx);
            editor.set_completion_keys(
                self.semantic_ui.index.labels.keys().cloned().collect(),
                self.semantic_ui.index.bibliography.entries.keys().cloned().collect(),
            );
            configure(editor, &self.settings, cx);
        });
    }

    pub(super) fn update_membership(&mut self) {
        if let Some(project) = &mut self.project {
            project.documents.clear();
            if let Some(path) = self.document.as_ref().and_then(|document| document.path.as_ref()) {
                project.add_document(path);
            }
            for tab in self.tabs.iter().flatten() {
                if let Some(path) = &tab.document.path {
                    project.add_document(path);
                }
            }
        }
    }

    pub(super) fn start_project_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_cancel.cancel();
        let cancellation = ligature_search::Cancellation::default();
        self.project_cancel = cancellation.clone();
        let cache = Arc::clone(&self.project_disk);
        self.project_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let snapshot = this.update_in(cx, |workspace, _, cx| {
                    let project = workspace.project.as_ref()?;
                    Some((
                        project.clone(),
                        workspace.expanded.clone(),
                        workspace.project_text(cx),
                        workspace.document.as_ref().and_then(|document| document.path.clone()),
                        workspace.build_ui.successful_artifacts(),
                        workspace.build_ui.diagnostic_snapshot(),
                        workspace.settings.clone(),
                        workspace.configuration_errors.clone(),
                    ))
                });
                let Ok(Some((
                    mut project,
                    expanded,
                    text,
                    active,
                    artifacts,
                    (diagnostic_id, diagnostic_sources, diagnostic_paths),
                    mut settings,
                    mut errors,
                ))) = snapshot
                else {
                    break;
                };
                let root = project.root.clone();
                let expected_root = root.clone();
                let expected_active = active.clone();
                let expected_settings = settings.clone();
                let disk = Arc::clone(&cache);
                let worker_cancel = cancellation.clone();
                let (directories, graph, root_document, settings, errors, stale_pdf, stale_diagnostics) = cx
                    .background_executor()
                    .spawn(async move {
                        let mut disk = disk.lock().unwrap();
                        errors.extend(discover_root_cancellable(
                            &mut project,
                            &mut settings,
                            active.as_deref(),
                            &text,
                            &mut disk,
                            || worker_cancel.is_canceled(),
                        ));
                        let output = root.join(&settings.build.output_directory);
                        let directories: BTreeMap<_, _> = expanded
                            .into_iter()
                            .take_while(|_| !worker_cancel.is_canceled())
                            .map(|path| {
                                let directory = ligature_project::directory(&path, &output);
                                (path, directory)
                            })
                            .collect();
                        let graph = project.root_document.as_ref().map_or_else(IncludeGraph::default, |path| {
                            IncludeGraph::load_cancellable(
                                &root.join(path),
                                |path| {
                                    text.get(path)
                                        .cloned()
                                        .map_or_else(|| disk.read(path).map_err(|error| error.to_string()), Ok)
                                },
                                || worker_cancel.is_canceled(),
                            )
                        });
                        disk.prune();
                        drop(disk);
                        let stale_pdf = artifacts.map(|artifacts| {
                            let stale = !worker_cancel.is_canceled() && !artifacts.sources.matches_disk(&root, &output);
                            (artifacts.pdf, stale)
                        });
                        let stale_diagnostics = diagnostic_paths
                            .into_iter()
                            .take_while(|_| !worker_cancel.is_canceled())
                            .filter(|path| diagnostic_sources.as_ref().is_none_or(|sources| !sources.matches_disk_file(path)))
                            .collect();
                        (directories, graph, project.root_document, settings, errors, stale_pdf, stale_diagnostics)
                    })
                    .await;
                if this
                    .update_in(cx, |workspace, _, cx| {
                        if cancellation.is_canceled()
                            || workspace.project.as_ref().is_none_or(|project| project.root != expected_root)
                            || workspace.document.as_ref().and_then(|document| document.path.as_ref()) != expected_active.as_ref()
                            || workspace.settings != expected_settings
                        {
                            return;
                        }
                        let changed = workspace.directories != directories
                            || workspace.includes != graph
                            || workspace.project.as_ref().is_some_and(|project| project.root_document != root_document)
                            || workspace.settings != settings
                            || workspace.settings_errors != errors;
                        if workspace.settings != settings {
                            workspace.settings = settings;
                            workspace.apply_settings(cx);
                            workspace.search.preview = None;
                            workspace.queue_project_search(cx);
                        }
                        if let Some(project) = &mut workspace.project {
                            project.root_document = root_document;
                        }
                        let mut directories = directories;
                        for (path, directory) in &mut directories {
                            if directory.error.is_some()
                                && let Some(previous) = workspace.directories.get(path)
                            {
                                directory.entries.clone_from(&previous.entries);
                            }
                        }
                        workspace.directories = directories;
                        workspace.includes = graph;
                        workspace.settings_errors = errors;
                        if let Some((pdf, stale)) = stale_pdf
                            && workspace.build_ui.successful_artifacts().is_some_and(|artifacts| artifacts.pdf == pdf)
                        {
                            workspace.build_ui.disk_stale = stale;
                        }
                        workspace.refresh_preview_staleness(cx);
                        let diagnostics_changed = workspace.build_ui.update_diagnostic_disk_staleness(diagnostic_id, stale_diagnostics);
                        workspace.refresh_diagnostics(cx);
                        if changed || diagnostics_changed {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(500)).await;
            }
        }));
    }

    pub(super) fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.project.as_ref().expect("project tree has project");
        let mut rows = Vec::new();
        self.tree_rows(&project.root, 0, &mut rows);
        let colors = colors();
        let active_path = self.document.as_ref().and_then(|document| document.path.as_ref());
        let content = div()
            .id("project-files")
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_y_scroll()
            .px(spacing::medium())
            .py(spacing::medium())
            .when_some(project.root_document.as_ref(), |view, path| {
                view.child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .mb(spacing::medium())
                        .child(caption(format!("Main: {}", path.file_name().unwrap_or_default().to_string_lossy()))),
                )
            })
            .when(rows.is_empty(), |view| view.child(caption("No files in this project.")))
            .children(rows.into_iter().enumerate().map(|(index, (path, depth, directory, error))| {
                if let Some(error) = error {
                    return div().pl(px(depth as f32 * 12.0)).child(inline_error(error));
                }
                let expanded = self.expanded.contains(&path);
                let selected = active_path == Some(&path);
                let label = format!(
                    "{}{}",
                    if directory { if expanded { "▾ " } else { "▸ " } } else { "" },
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
                div()
                    .debug_selector(move || format!("project-entry-{index}"))
                    .min_w(px(0.0))
                    .pl(px(depth.min(8) as f32 * 12.0))
                    .child(
                        Button::new(("tree-entry", index), label)
                            .quiet()
                            .full_width()
                            .tooltip(path.strip_prefix(&project.root).unwrap_or(&path).display().to_string())
                            .selected(selected)
                            .disabled(self.busy)
                            .on_click(cx.listener(move |workspace, _, window, cx| {
                                if directory {
                                    if !workspace.expanded.remove(&path) {
                                        workspace.expanded.insert(path.clone());
                                    }
                                    workspace.start_project_refresh(window, cx);
                                    cx.notify();
                                } else {
                                    workspace.start(Intent::OpenPath(path.clone()), window, cx);
                                }
                            })),
                    )
            }))
            .children(
                self.includes
                    .errors
                    .iter()
                    .map(|(path, error)| inline_error(format!("Include '{}': {error}", path.display()))),
            );
        div()
            .id("project-tree")
            .debug_selector(|| "project-tree".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.chrome)
            .child(panel_header("Files"))
            .child(content)
    }

    fn tree_rows(&self, path: &Path, depth: usize, rows: &mut Vec<(PathBuf, usize, bool, Option<String>)>) {
        if let Some(directory) = self.directories.get(path) {
            if let Some(error) = &directory.error {
                rows.push((path.to_owned(), depth, true, Some(error.clone())));
            }
            for entry in &directory.entries {
                rows.push((entry.path.clone(), depth, entry.is_directory, None));
                if entry.is_directory && self.expanded.contains(&entry.path) {
                    self.tree_rows(&entry.path, depth + 1, rows);
                }
            }
        }
    }
}

fn configure(editor: &mut ligature_editor::Editor, settings: &Settings, cx: &mut Context<ligature_editor::Editor>) {
    editor.set_font(settings.font.family.clone().map(Into::into), px(settings.font.size), cx);
    editor.update_state(
        |state| {
            state.set_indentation(
                match settings.indentation.style {
                    ligature_settings::Indentation::Tabs => ligature_editor::Indentation::Tabs,
                    ligature_settings::Indentation::Spaces => ligature_editor::Indentation::Spaces,
                },
                settings.indentation.width,
            )
        },
        cx,
    );
}
