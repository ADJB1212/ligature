use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use gpui::{AsyncWindowContext, Context, IntoElement, PathPromptOptions, WeakEntity, Window, div, prelude::*, px};
use ligature_project::{IncludeGraph, Project};
use ligature_settings::{PROJECT_FILE, Settings};
use ligature_theme::{colors, spacing};
use ligature_ui::{Button, caption, inline_error};

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
    let (project, settings, errors) = cx
        .background_executor()
        .spawn(async move {
            let mut project = Project::open(&path)?;
            let (mut settings, mut errors) = Settings::load(&project.root.join(PROJECT_FILE));
            if let Some(root) = &settings.root_document
                && let Err(error) = project.set_root_document(root)
            {
                errors.push(error);
                settings.root_document = None;
            }
            Ok::<_, String>((project, settings, errors))
        })
        .await?;
    this.update_in(cx, |workspace, window, cx| {
        workspace.build_ui.reset();
        workspace.refresh_diagnostics(cx);
        workspace.preview.update(cx, |preview, cx| preview.clear(cx));
        workspace.project_task = None;
        workspace.expanded.clear();
        workspace.directories.clear();
        workspace.expanded.insert(project.root.clone());
        workspace.project = Some(project);
        workspace.settings = settings;
        workspace.apply_settings(cx);
        workspace.update_membership();
        workspace.settings_errors = errors;
        workspace.start_project_refresh(window, cx);
        workspace.search.preview = None;
        workspace.queue_project_search(cx);
        cx.notify();
    })
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) async fn select_root(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let snapshot = this
        .update_in(cx, |workspace, _, _| {
            Some((
                workspace.project.clone()?,
                workspace.settings.clone(),
                workspace.document.as_ref()?.path.clone()?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let Some((mut project, mut settings, path)) = snapshot else {
        return Err("Open a project and save a TeX document before selecting its root".into());
    };
    let (project, settings) = cx
        .background_executor()
        .spawn(async move {
            project.set_root_document(&path)?;
            settings.root_document.clone_from(&project.root_document);
            settings.save(&project.root.join(PROJECT_FILE))?;
            Ok::<_, String>((project, settings))
        })
        .await?;
    this.update_in(cx, |workspace, window, cx| {
        workspace.project = Some(project);
        workspace.settings = settings;
        workspace.update_membership();
        workspace.start_project_refresh(window, cx);
        cx.notify();
    })
    .map_err(|error| error.to_string())?;
    Ok(())
}

impl Workspace {
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

    fn start_project_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let snapshot = this.update_in(cx, |workspace, _, cx| {
                    let project = workspace.project.as_ref()?;
                    let mut text = BTreeMap::new();
                    if let Some(path) = workspace.document.as_ref().and_then(|document| document.path.as_ref()) {
                        text.insert(path.clone(), workspace.editor.read(cx).state().text().to_owned());
                    }
                    for tab in workspace.tabs.iter().flatten() {
                        if let Some(path) = &tab.document.path {
                            text.insert(path.clone(), tab.editor.read(cx).state().text().to_owned());
                        }
                    }
                    Some((
                        project.clone(),
                        workspace.expanded.clone(),
                        text,
                        workspace.build_ui.successful_artifacts(),
                        workspace.build_ui.diagnostic_snapshot(),
                    ))
                });
                let Ok(Some((mut project, expanded, text, artifacts, (diagnostic_id, diagnostic_sources, diagnostic_paths)))) = snapshot else {
                    break;
                };
                let root = project.root.clone();
                let expected_root = root.clone();
                let (directories, graph, settings, errors, stale_pdf, stale_diagnostics) = cx
                    .background_executor()
                    .spawn(async move {
                        let (mut settings, mut errors) = Settings::load(&root.join(PROJECT_FILE));
                        if let Some(path) = &settings.root_document
                            && let Err(error) = project.set_root_document(path)
                        {
                            errors.push(error);
                            settings.root_document = None;
                        }
                        let output = root.join(&settings.build.output_directory);
                        let directories: BTreeMap<_, _> = expanded
                            .into_iter()
                            .map(|path| {
                                let directory = ligature_project::directory(&path, &output);
                                (path, directory)
                            })
                            .collect();
                        let graph = settings.root_document.as_ref().map_or_else(IncludeGraph::default, |path| {
                            IncludeGraph::load_with(&root.join(path), |path| {
                                text.get(path)
                                    .cloned()
                                    .map_or_else(|| ligature_fs::read(path).map(|file| file.text).map_err(|error| error.to_string()), Ok)
                            })
                        });
                        let stale_pdf = artifacts.map(|artifacts| {
                            let stale = !artifacts.sources.matches_disk(&root, &output);
                            (artifacts.pdf, stale)
                        });
                        let stale_diagnostics = diagnostic_paths
                            .into_iter()
                            .filter(|path| {
                                diagnostic_sources
                                    .as_ref()
                                    .is_none_or(|sources| std::fs::read_to_string(path).ok().is_none_or(|text| !sources.matches_text(path, &text)))
                            })
                            .collect();
                        (directories, graph, settings, errors, stale_pdf, stale_diagnostics)
                    })
                    .await;
                if this
                    .update_in(cx, |workspace, _, cx| {
                        if workspace.project.as_ref().is_none_or(|project| project.root != expected_root) {
                            return;
                        }
                        let changed = workspace.directories != directories
                            || workspace.includes != graph
                            || workspace.settings != settings
                            || workspace.settings_errors != errors;
                        if workspace.settings != settings {
                            workspace.settings = settings;
                            workspace.apply_settings(cx);
                            workspace.search.preview = None;
                            workspace.queue_project_search(cx);
                        }
                        if let Some(project) = &mut workspace.project {
                            project.root_document.clone_from(&workspace.settings.root_document);
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
        div()
            .id("project-tree")
            .w(px(220.0))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .px(spacing::medium())
            .py(spacing::small())
            .bg(colors.background)
            .child(caption("PROJECT"))
            .child(project.root.file_name().unwrap_or_default().to_string_lossy().into_owned())
            .when_some(project.root_document.as_ref(), |view, path| {
                view.child(caption(format!("Root {}", path.display())))
            })
            .child(div().h(spacing::medium()))
            .children(rows.into_iter().enumerate().map(|(index, (path, depth, directory, error))| {
                if let Some(error) = error {
                    return div().pl(px(depth as f32 * 12.0)).child(inline_error(error));
                }
                let expanded = self.expanded.contains(&path);
                let selected = active_path == Some(&path);
                let label = format!(
                    "{}{}",
                    if directory { if expanded { "- " } else { "+ " } } else { "" },
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
                div().pl(px(depth as f32 * 12.0)).child(
                    Button::new(("tree-entry", index), label)
                        .quiet()
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
            )
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
