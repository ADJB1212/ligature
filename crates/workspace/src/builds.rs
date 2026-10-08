use crate::{Workspace, lifecycle::Intent};
use gpui::{App, AsyncWindowContext, Context, IntoElement, Task, WeakEntity, Window, div, prelude::*};
use ligature_buffer::DocumentId;
use ligature_build::{BuildArtifacts, BuildJob, BuildPreparation, BuildRequest, BuildSnapshot, BuildState, TemporarySource, TextBuild};
use ligature_diagnostics::{Diagnostic, Severity, grouped};
use ligature_theme::spacing;
use ligature_toolchain::BundledResources;
use ligature_ui::{Button, Indicator, caption, chrome_strip, indicator, inline_error};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

#[derive(Default)]
pub(super) struct BuildUi {
    pub visible: bool,
    job: Option<BuildJob>,
    task: Option<Task<()>>,
    snapshot: BuildSnapshot,
    last_success: Option<BuildArtifacts>,
    pub disk_stale: bool,
    diagnostics: Vec<Diagnostic>,
    diagnostic_sources: Option<Arc<ligature_build::SourceSnapshot>>,
    diagnostic_id: ligature_diagnostics::BuildId,
    diagnostic_disk_stale: BTreeSet<PathBuf>,
    diagnostic_document: Option<DocumentId>,
    diagnostic_temporary_source: Option<TemporarySource>,
    preview_document: Option<DocumentId>,
}

impl BuildUi {
    fn record_completed(&mut self, snapshot: &BuildSnapshot) {
        if matches!(snapshot.state, BuildState::Succeeded | BuildState::Failed) {
            self.diagnostics.clone_from(&snapshot.diagnostics);
            self.diagnostic_sources.clone_from(&snapshot.sources);
            self.diagnostic_id = snapshot.id;
            self.diagnostic_disk_stale.clear();
            self.diagnostic_temporary_source.clone_from(&snapshot.temporary_source);
        }
    }
    pub fn diagnostic_snapshot(&self) -> (ligature_diagnostics::BuildId, Option<Arc<ligature_build::SourceSnapshot>>, Vec<PathBuf>) {
        (
            self.diagnostic_id,
            self.diagnostic_sources.clone(),
            self.diagnostics.iter().filter_map(|diagnostic| diagnostic.path.clone()).collect(),
        )
    }

    pub fn update_diagnostic_disk_staleness(&mut self, id: ligature_diagnostics::BuildId, stale: BTreeSet<PathBuf>) -> bool {
        if id != self.diagnostic_id || stale == self.diagnostic_disk_stale {
            return false;
        }
        self.diagnostic_disk_stale = stale;
        true
    }
    pub fn successful_artifacts(&self) -> Option<BuildArtifacts> {
        self.last_success.clone()
    }
    pub fn running(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|job| matches!(job.snapshot().state, BuildState::Idle | BuildState::Running))
    }

    pub fn reset(&mut self) {
        if let Some(job) = &self.job {
            job.cancel();
        }
        *self = Self::default();
    }
}

impl Drop for BuildUi {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel();
        }
    }
}

impl Workspace {
    pub(super) fn can_build(&self) -> bool {
        self.project.is_some() || self.document.as_ref().is_some_and(|document| document.path.is_none())
    }

    fn diagnostic_matches(&self, diagnostic: &Diagnostic, path: Option<&std::path::PathBuf>, id: DocumentId) -> bool {
        if self
            .build_ui
            .diagnostic_temporary_source
            .as_ref()
            .is_some_and(|source| diagnostic.path.as_ref() == Some(&source.root_document))
        {
            self.build_ui.diagnostic_document == Some(id)
        } else {
            path.is_some() && diagnostic.path.as_ref() == path
        }
    }

    pub(super) fn activate_build_source(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) -> Result<bool, String> {
        let document = if self
            .build_ui
            .diagnostic_temporary_source
            .as_ref()
            .is_some_and(|source| source.root_document == path)
        {
            self.build_ui.diagnostic_document
        } else if self
            .build_ui
            .last_success
            .as_ref()
            .and_then(|artifacts| artifacts.temporary_source.as_ref())
            .is_some_and(|source| source.root_document == path)
        {
            self.build_ui.preview_document
        } else {
            return Ok(false);
        };
        let index = self
            .tabs
            .iter()
            .position(|tab| {
                let editor = tab.as_ref().map_or(&self.editor, |tab| &tab.editor);
                Some(editor.read(cx).state().buffer().id()) == document
            })
            .ok_or("The untitled source document was closed. Open a document and build again.")?;
        self.switch_tab(index, window, cx);
        Ok(true)
    }

    fn diagnostic_stale(&self, diagnostic: &Diagnostic, cx: &App) -> bool {
        let Some(path) = &diagnostic.path else { return false };
        let open = self.tabs.iter().find_map(|tab| {
            let (document, editor) = match tab {
                Some(tab) => (Some(&tab.document), &tab.editor),
                None => (self.document.as_ref(), &self.editor),
            };
            self.diagnostic_matches(
                diagnostic,
                document.and_then(|document| document.path.as_ref()),
                editor.read(cx).state().buffer().id(),
            )
            .then(|| {
                self.build_ui
                    .diagnostic_sources
                    .as_ref()
                    .is_none_or(|sources| !sources.matches_text(path, editor.read(cx).state().text()))
            })
        });
        open.unwrap_or_else(|| self.build_ui.diagnostic_disk_stale.contains(path))
    }

    pub(super) fn refresh_diagnostics(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            let (document, editor) = match tab {
                Some(tab) => (Some(&tab.document), &tab.editor),
                None => (self.document.as_ref(), &self.editor),
            };
            let path = document.and_then(|document| document.path.as_ref());
            let diagnostics: Vec<_> = self
                .build_ui
                .diagnostics
                .iter()
                .filter(|diagnostic| self.diagnostic_matches(diagnostic, path, editor.read(cx).state().buffer().id()))
                .cloned()
                .collect();
            let stale = diagnostics.iter().any(|diagnostic| self.diagnostic_stale(diagnostic, cx));
            editor.update(cx, |editor, cx| editor.set_diagnostics(&diagnostics, stale, cx));
        }
    }

    pub(super) fn refresh_preview_staleness(&mut self, cx: &mut Context<Self>) {
        let stale = self.build_ui.last_success.as_ref().is_some_and(|artifacts| {
            if artifacts.temporary_source.is_some() {
                return self
                    .tabs
                    .iter()
                    .find_map(|tab| {
                        let editor = tab.as_ref().map_or(&self.editor, |tab| &tab.editor);
                        let state = editor.read(cx).state();
                        (Some(state.buffer().id()) == self.build_ui.preview_document)
                            .then(|| !artifacts.sources.matches_text(&artifacts.root_document, state.text()))
                    })
                    .unwrap_or(true);
            }
            artifacts.sources_changed
                || self.build_ui.disk_stale
                || self.project.as_ref().is_none_or(|project| {
                    project
                        .root_document
                        .as_ref()
                        .is_none_or(|root| project.root.join(root) != artifacts.root_document)
                })
                || self.tabs.iter().any(|tab| {
                    let (document, editor) = match tab {
                        Some(tab) => (Some(&tab.document), &tab.editor),
                        None => (self.document.as_ref(), &self.editor),
                    };
                    document.and_then(|document| document.path.as_ref()).is_some_and(|path| {
                        self.project.as_ref().is_some_and(|project| project.relative(path).is_some())
                            && !artifacts.sources.matches_text(path, editor.read(cx).state().text())
                    })
                })
        });
        self.preview.update(cx, |preview, cx| preview.set_stale(stale, cx));
    }

    pub(super) fn show_in_preview(&mut self, cx: &mut Context<Self>) {
        let temporary_path = (self.build_ui.preview_document == Some(self.editor.read(cx).state().buffer().id()))
            .then(|| {
                self.build_ui
                    .last_success
                    .as_ref()
                    .and_then(|artifacts| artifacts.temporary_source.as_ref())
                    .map(|source| source.root_document.clone())
            })
            .flatten();
        let Some(path) = temporary_path.or_else(|| self.document.as_ref().and_then(|document| document.path.clone())) else {
            self.error = Some("Build the source document before navigating to the preview".into());
            cx.notify();
            return;
        };
        let state = self.editor.read(cx).state();
        let position = state.buffer().offset_to_position(ligature_buffer::ByteOffset(state.caret())).unwrap();
        let line = state.buffer().line(position.line).unwrap();
        let column = line[..position.byte_column].chars().count() + 1;
        self.refresh_preview_staleness(cx);
        self.preview.update(cx, |preview, cx| {
            preview.forward(
                ligature_synctex::SourcePosition {
                    path,
                    line: position.line + 1,
                    column,
                },
                cx,
            )
        });
    }

    pub(super) fn start_build(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.can_build() || self.build_ui.running() {
            return;
        }
        self.build_ui.visible = true;
        self.start(Intent::Build, window, cx);
    }

    pub(super) fn cancel_build(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &self.build_ui.job {
            job.cancel();
        }
        cx.notify();
    }

    fn dirty_project_paths(&self, cx: &App) -> Vec<PathBuf> {
        let Some(project) = &self.project else { return Vec::new() };
        self.tabs
            .iter()
            .map(|tab| match tab {
                Some(tab) => (Some(&tab.document), &tab.editor),
                None => (self.document.as_ref(), &self.editor),
            })
            .filter_map(|(document, editor)| {
                let document = document?;
                let path = document.path.as_ref()?;
                (project.relative(path).is_some() && (document.diverged || editor.read(cx).state().is_dirty())).then(|| path.clone())
            })
            .collect()
    }

    fn launch_build(&mut self, request: BuildRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.launch_job(move |worker, resources| worker.run(&request, &resources), None, window, cx);
    }

    fn launch_job(
        &mut self, run: impl FnOnce(BuildJob, BundledResources) + Send + 'static, document: Option<DocumentId>, window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let job = BuildJob::default();
        let worker = job.clone();
        self.build_ui.job = Some(job.clone());
        self.build_ui.snapshot = BuildSnapshot {
            state: BuildState::Running,
            id: job.snapshot().id,
            ..BuildSnapshot::default()
        };
        cx.background_executor()
            .spawn(async move {
                match BundledResources::discover() {
                    Ok(resources) => run(worker, resources),
                    Err(error) => worker.fail_installation(error),
                }
            })
            .detach();
        self.build_ui.task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let snapshot = job.snapshot();
                let finished = matches!(snapshot.state, BuildState::Succeeded | BuildState::Failed | BuildState::Canceled);
                if this
                    .update_in(cx, |workspace, _, cx| {
                        if let Some(artifacts) = &snapshot.artifacts {
                            if workspace
                                .build_ui
                                .last_success
                                .as_ref()
                                .is_none_or(|previous| previous.pdf != artifacts.pdf)
                            {
                                workspace.preview.update(cx, |preview, cx| {
                                    preview.load(artifacts.pdf.clone(), artifacts.synctex.clone(), artifacts.source_directory.clone(), cx)
                                });
                            }
                            workspace.build_ui.last_success = Some(artifacts.clone());
                            workspace.build_ui.preview_document = document;
                            workspace.build_ui.disk_stale = false;
                        }
                        if finished {
                            workspace.build_ui.record_completed(&snapshot);
                            if snapshot.state != BuildState::Canceled {
                                workspace.build_ui.diagnostic_document = document;
                            }
                            workspace.refresh_diagnostics(cx);
                            workspace.build_ui.task = None;
                            workspace.refresh_preview_staleness(cx);
                        }
                        workspace.build_ui.snapshot = snapshot;
                        cx.notify();
                    })
                    .is_err()
                    || finished
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(40)).await;
            }
        }));
        cx.notify();
    }

    pub(super) fn render_build_output(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = &self.build_ui.snapshot;
        let status = match snapshot.state {
            BuildState::Idle => "No build yet",
            BuildState::Running => "Building...",
            BuildState::Succeeded => "Build succeeded",
            BuildState::Failed => "Build failed",
            BuildState::Canceled => "Build canceled",
        };
        let errors = self
            .build_ui
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .count();
        let warnings = self
            .build_ui
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Warning)
            .count();
        let mut entries = div()
            .id("build-diagnostics")
            .max_h(gpui::px(180.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(spacing::small());
        let mut index = 0_usize;
        for ((path, severity), diagnostics) in grouped(&self.build_ui.diagnostics) {
            let name = path.as_ref().map_or_else(
                || "Build".into(),
                |path| {
                    if self
                        .build_ui
                        .diagnostic_temporary_source
                        .as_ref()
                        .is_some_and(|source| source.root_document == *path)
                    {
                        "Untitled.tex".into()
                    } else {
                        path.display().to_string()
                    }
                },
            );
            entries = entries.child(caption(format!("{name} - {severity:?}")));
            for diagnostic in diagnostics {
                let location = diagnostic
                    .range
                    .as_ref()
                    .map_or(String::new(), |range| format!("Line {}: ", range.first_line()));
                let stale = if self.diagnostic_stale(diagnostic, cx) { " (older source)" } else { "" };
                let provider = match &diagnostic.provider {
                    ligature_diagnostics::Provider::Build(id) => format!("Build {}", id.0),
                    ligature_diagnostics::Provider::LanguageServer(name) => format!("Language server: {name}"),
                };
                let diagnostic = diagnostic.clone();
                let label = format!("{provider}: {location}{}{stale}", diagnostic.message);
                entries = entries.child(
                    Button::new(("diagnostic", index), label)
                        .quiet()
                        .disabled(diagnostic.path.is_none() || self.busy)
                        .on_click(
                            cx.listener(move |workspace, _, window, cx| workspace.start(Intent::OpenDiagnostic(diagnostic.clone()), window, cx)),
                        ),
                );
                index += 1;
            }
        }
        chrome_strip()
            .flex()
            .flex_col()
            .gap(spacing::small())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(spacing::small())
                    .child(indicator(
                        "build-status",
                        match snapshot.state {
                            BuildState::Running => Indicator::Live,
                            BuildState::Succeeded => Indicator::On,
                            BuildState::Failed => Indicator::Alert,
                            BuildState::Idle | BuildState::Canceled => Indicator::Idle,
                        },
                    ))
                    .child(caption(format!("{status} - {errors} errors, {warnings} warnings"))),
            )
            .child(entries)
            .when_some(snapshot.failure.as_ref(), |view, failure| {
                view.child(inline_error(failure.message.clone()))
            })
            .when_some(self.build_ui.last_success.as_ref(), |view, artifacts| {
                let name = if artifacts.temporary_source.is_some() {
                    "Untitled.pdf".into()
                } else {
                    artifacts.pdf.display().to_string()
                };
                view.child(caption(format!("Last successful PDF: {name}")))
            })
            .when(snapshot.truncated, |view| view.child(caption("Earlier output omitted.")))
            .child(
                div()
                    .id("build-log")
                    .max_h(gpui::px(180.0))
                    .overflow_y_scroll()
                    .child(snapshot.output.clone()),
            )
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use ligature_diagnostics::{BuildId, Provider};

    #[test]
    fn diagnostic_staleness_tracks_edits_and_undo_against_the_producing_build() {
        use gpui::{AppContext, TestAppContext};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.tex");
        std::fs::write(&path, "original").unwrap();
        let path = std::fs::canonicalize(path).unwrap();
        let sources = Arc::new(ligature_build::SourceSnapshot::capture(directory.path(), &directory.path().join("build")));
        let mut cx = TestAppContext::single();
        let window = cx.add_window(Workspace::new);
        let root = window.root(&mut cx).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            root.update(cx, |workspace, cx| workspace.start(Intent::OpenPath(path.clone()), window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        let diagnostic = Diagnostic {
            provider: Provider::Build(BuildId(1)),
            severity: Severity::Error,
            message: "Failed".into(),
            path: Some(path),
            range: Some(ligature_diagnostics::SourceRange::Lines { start: 1, end: 1 }),
        };
        root.update(&mut cx, |workspace, cx| {
            workspace.build_ui.record_completed(&BuildSnapshot {
                id: BuildId(1),
                state: BuildState::Failed,
                diagnostics: vec![diagnostic.clone()],
                sources: Some(sources),
                ..BuildSnapshot::default()
            });
            assert!(!workspace.diagnostic_stale(&diagnostic, cx));
            workspace
                .editor
                .update(cx, |editor, cx| editor.update_state(|state| state.insert("new", false), cx));
            assert!(workspace.diagnostic_stale(&diagnostic, cx));
            workspace.editor.update(cx, |editor, cx| editor.update_state(|state| state.undo(), cx));
            assert!(!workspace.diagnostic_stale(&diagnostic, cx));
        });
    }

    #[test]
    fn completed_builds_replace_diagnostics_while_cancellation_preserves_them() {
        let mut ui = BuildUi::default();
        let diagnostic = Diagnostic {
            provider: Provider::Build(BuildId(1)),
            severity: Severity::Error,
            message: "Failed".into(),
            path: None,
            range: None,
        };
        ui.record_completed(&BuildSnapshot {
            id: BuildId(1),
            state: BuildState::Failed,
            diagnostics: vec![diagnostic.clone()],
            ..BuildSnapshot::default()
        });
        for state in [BuildState::Running, BuildState::Canceled] {
            ui.record_completed(&BuildSnapshot {
                state,
                ..BuildSnapshot::default()
            });
            assert_eq!(ui.diagnostics, vec![diagnostic.clone()]);
        }
        ui.record_completed(&BuildSnapshot {
            id: BuildId(2),
            state: BuildState::Succeeded,
            ..BuildSnapshot::default()
        });
        assert!(ui.diagnostics.is_empty());
        assert!(!ui.update_diagnostic_disk_staleness(BuildId(1), BTreeSet::from([PathBuf::from("old.tex")])));
    }
}

pub(super) async fn prepare(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let untitled = this
        .update_in(cx, |workspace, _, cx| {
            workspace.document.as_ref().filter(|document| document.path.is_none()).map(|_| {
                let state = workspace.editor.read(cx).state();
                (state.buffer().id(), state.text().to_owned(), workspace.settings.build.clone())
            })
        })
        .map_err(|error| error.to_string())?;
    if let Some((document, text, settings)) = untitled {
        let build = cx
            .background_executor()
            .spawn(async move { TextBuild::prepare(&text, &settings) })
            .await?;
        this.update_in(cx, |workspace, window, cx| {
            workspace.launch_job(move |worker, resources| worker.run_text(&build, &resources), Some(document), window, cx);
        })
        .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let preparation = this
        .update_in(cx, |workspace, _, cx| {
            let project = workspace.project.as_ref().ok_or("Open a project before building")?;
            BuildRequest::prepare(project, &workspace.settings.build, &workspace.dirty_project_paths(cx))
        })
        .map_err(|error| error.to_string())??;
    if let BuildPreparation::SaveRequired(paths) = preparation {
        let original = this
            .update_in(cx, |workspace, _, _| workspace.active_tab)
            .map_err(|error| error.to_string())?;
        let result = save_project(this, cx, &paths).await;
        this.update_in(cx, |workspace, window, cx| workspace.switch_tab(original, window, cx))
            .map_err(|error| error.to_string())?;
        result?;
    }
    let (project, settings, dirty) = this
        .update_in(cx, |workspace, _, cx| {
            (
                workspace.project.clone(),
                workspace.settings.build.clone(),
                workspace.dirty_project_paths(cx),
            )
        })
        .map_err(|error| error.to_string())?;
    let request = cx
        .background_executor()
        .spawn(async move {
            let project = project.ok_or("The project closed before building")?;
            match BuildRequest::prepare(&project, &settings, &dirty)? {
                BuildPreparation::Ready(request) => Ok::<_, String>(request),
                BuildPreparation::SaveRequired(_) => Err("Project documents changed while saving. Save them and build again.".into()),
            }
        })
        .await?;
    this.update_in(cx, |workspace, window, cx| workspace.launch_build(request, window, cx))
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn save_project(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext, paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        this.update_in(cx, |workspace, window, cx| {
            let index = workspace
                .tabs
                .iter()
                .position(|tab| {
                    tab.as_ref()
                        .map_or_else(|| workspace.document.as_ref(), |tab| Some(&tab.document))
                        .is_some_and(|document| document.path.as_ref() == Some(path))
                })
                .ok_or("A dirty project document closed before saving")?;
            workspace.switch_tab(index, window, cx);
            Ok::<_, String>(())
        })
        .map_err(|error| error.to_string())??;
        if !super::lifecycle::save(this, cx, false).await? {
            return Err("Build canceled while saving project documents".into());
        }
    }
    Ok(())
}

pub(super) async fn shutdown(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let job = this
        .update_in(cx, |workspace, _, _| workspace.build_ui.job.clone())
        .map_err(|error| error.to_string())?;
    if let Some(job) = job {
        job.cancel();
        while matches!(job.snapshot().state, BuildState::Idle | BuildState::Running) {
            cx.background_executor().timer(Duration::from_millis(15)).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[cfg(target_os = "macos")]
    #[test]
    fn untitled_render_job_updates_the_preview_without_a_project_or_save_prompt() {
        let directory = tempfile::tempdir().unwrap();
        let resources = BundledResources::new(
            std::path::Path::new(ligature_toolchain::BUILT_RESOURCE_DIRECTORY),
            &directory.path().join("cache"),
        )
        .unwrap();
        let mut cx = TestAppContext::single();
        cx.update(crate::init);
        let window = cx.add_window(Workspace::new);
        let root = window.root(&mut cx).unwrap();
        let document = root.update(&mut cx, |workspace, cx| {
            workspace.editor.update(cx, |editor, cx| {
                editor.update_state(
                    |state| {
                        state.insert("\\documentclass{article}\n\\begin{document}\nUntitled preview\n\\end{document}\n", false);
                    },
                    cx,
                )
            });
            workspace.editor.read(cx).state().buffer().id()
        });
        let build = root
            .read_with(&cx, |workspace, cx| {
                TextBuild::prepare(workspace.editor.read(cx).state().text(), &workspace.settings.build)
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            root.update(cx, |workspace, cx| {
                workspace.launch_job(move |worker, _| worker.run_text(&build, &resources), Some(document), window, cx);
            })
        })
        .unwrap();
        for _ in 0..100 {
            cx.run_until_parked();
            let finished = root.read_with(&cx, |workspace, _| {
                matches!(workspace.build_ui.snapshot.state, BuildState::Succeeded | BuildState::Failed)
            });
            if finished {
                break;
            }
            cx.executor().advance_clock(Duration::from_millis(40));
        }
        root.read_with(&cx, |workspace, cx| {
            assert_eq!(
                workspace.build_ui.snapshot.state,
                BuildState::Succeeded,
                "{:?}; {:?}",
                workspace.build_ui.snapshot,
                workspace.error
            );
            assert_eq!(workspace.build_ui.preview_document, Some(document));
            assert!(workspace.build_ui.last_success.as_ref().unwrap().pdf.is_file());
            assert!(workspace.document.as_ref().unwrap().path.is_none());
            assert!(workspace.editor.read(cx).state().is_dirty());
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.project.is_none());
            assert!(!workspace.busy);
        });
        assert!(!cx.has_pending_prompt());
    }

    #[test]
    fn untitled_build_diagnostics_and_synctex_reuse_the_original_tab() {
        use ligature_diagnostics::{Provider, SourceRange};
        let mut cx = TestAppContext::single();
        cx.update(crate::init);
        let window = cx.add_window(Workspace::new);
        let root = window.root(&mut cx).unwrap();
        let build = TextBuild::prepare("first\nsecond\n", &ligature_settings::BuildSettings::default()).unwrap();
        let diagnostic = Diagnostic {
            severity: Severity::Error,
            provider: Provider::Build(ligature_diagnostics::BuildId(1)),
            path: Some(build.source.root_document.clone()),
            range: Some(SourceRange::Lines { start: 2, end: 2 }),
            message: "Error".into(),
        };
        let document = root.update(&mut cx, |workspace, cx| {
            assert!(workspace.can_build());
            assert!(workspace.project.is_none());
            workspace
                .editor
                .update(cx, |editor, cx| editor.update_state(|state| state.insert("first\nsecond\n", false), cx));
            let document = workspace.editor.read(cx).state().buffer().id();
            workspace.build_ui.record_completed(&BuildSnapshot {
                id: ligature_diagnostics::BuildId(1),
                state: BuildState::Failed,
                diagnostics: vec![diagnostic.clone()],
                temporary_source: Some(build.source.clone()),
                sources: Some(Arc::new(ligature_build::SourceSnapshot::capture(
                    build.request.filesystem_root(),
                    build.request.output_directory(),
                ))),
                ..BuildSnapshot::default()
            });
            workspace.build_ui.diagnostic_document = Some(document);
            assert!(!workspace.diagnostic_stale(&diagnostic, cx));
            document
        });
        for intent in [
            Intent::New,
            Intent::OpenDiagnostic(diagnostic.clone()),
            Intent::OpenSource(ligature_synctex::SourcePosition {
                path: build.source.root_document.clone(),
                line: 2,
                column: 0,
            }),
        ] {
            cx.update_window(window.into(), |_, window, cx| {
                root.update(cx, |workspace, cx| workspace.start(intent, window, cx))
            })
            .unwrap();
            cx.run_until_parked();
        }
        root.update(&mut cx, |workspace, cx| {
            assert!(workspace.error.is_none(), "{:?}", workspace.error);
            assert_eq!(workspace.tabs.len(), 2);
            assert_eq!(workspace.editor.read(cx).state().buffer().id(), document);
            assert_eq!(workspace.editor.read(cx).state().selected_text(), "second");
            assert!(workspace.document.as_ref().unwrap().path.is_none());
            assert!(workspace.editor.read(cx).state().is_dirty());
            workspace
                .editor
                .update(cx, |editor, cx| editor.update_state(|state| state.insert("changed", false), cx));
            assert!(workspace.diagnostic_stale(&diagnostic, cx));
        });
        assert!(!cx.has_pending_prompt());
        cx.update_window(window.into(), |_, window, cx| {
            root.update(cx, |workspace, cx| {
                workspace.editor.update(cx, |editor, cx| editor.update_state(|state| state.undo(), cx));
                let index = workspace.active_tab;
                workspace.close_document(window, cx);
                assert_eq!(workspace.tabs.len(), 1);
                assert!(workspace.activate_build_source(&build.source.root_document, window, cx).is_err());
                assert_eq!(workspace.active_tab, index.min(workspace.tabs.len() - 1));
            });
        })
        .unwrap();
    }

    #[test]
    fn repeated_build_actions_leave_the_active_job_in_place() {
        let directory = tempfile::tempdir().unwrap();
        let project = ligature_project::Project::open(directory.path()).unwrap();
        let mut cx = TestAppContext::single();
        cx.update(crate::init);
        let window = cx.add_window(Workspace::new);
        let job = BuildJob::default();
        cx.update_window(window.into(), |root, window, cx| {
            root.downcast::<Workspace>().unwrap().update(cx, |workspace, cx| {
                workspace.project = Some(project);
                workspace.build_ui.job = Some(job.clone());
                workspace.start_build(window, cx);
                workspace.start_build(window, cx);
                assert!(!workspace.busy);
                assert!(workspace.build_ui.running());
                job.fail_installation("fixture failure".into());
                assert!(!workspace.build_ui.running());
            });
        })
        .unwrap();
    }

    #[test]
    fn resetting_or_dropping_build_ui_cancels_pending_work() {
        for reset in [false, true] {
            let job = BuildJob::default();
            let mut ui = BuildUi::default();
            ui.job = Some(job.clone());
            if reset {
                ui.reset();
                assert!(!ui.running());
            }
            drop(ui);
            job.fail_installation("fixture failure".into());
            assert_eq!(job.snapshot().state, BuildState::Canceled);
            assert!(job.snapshot().failure.is_none());
        }
    }

    #[test]
    fn shutdown_waits_for_the_canceled_worker_to_retire() {
        use std::{cell::Cell, rc::Rc};

        let mut cx = TestAppContext::single();
        cx.update(crate::init);
        let window = cx.add_window(Workspace::new);
        let job = BuildJob::default();
        let retired = Rc::new(Cell::new(false));
        let completed = Rc::clone(&retired);
        let _task = cx
            .update_window(window.into(), |root, window, cx| {
                root.downcast::<Workspace>().unwrap().update(cx, |workspace, cx| {
                    workspace.build_ui.job = Some(job.clone());
                    cx.spawn_in(window, async move |this, cx| {
                        shutdown(&this, cx).await.unwrap();
                        completed.set(true);
                    })
                })
            })
            .unwrap();
        cx.run_until_parked();
        assert!(!retired.get());
        job.fail_installation("fixture failure".into());
        assert_eq!(job.snapshot().state, BuildState::Canceled);
        cx.background_executor.advance_clock(Duration::from_millis(15));
        cx.run_until_parked();
        assert!(retired.get());
    }
}
