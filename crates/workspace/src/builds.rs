use crate::{Workspace, lifecycle::Intent};
use gpui::{App, AsyncWindowContext, Context, IntoElement, Task, WeakEntity, Window, div, prelude::*};
use ligature_build::{BuildArtifacts, BuildJob, BuildPreparation, BuildRequest, BuildSnapshot, BuildState};
use ligature_theme::{colors, spacing};
use ligature_toolchain::BundledResources;
use ligature_ui::{caption, inline_error};
use std::{path::PathBuf, time::Duration};

#[derive(Default)]
pub(super) struct BuildUi {
    pub visible: bool,
    job: Option<BuildJob>,
    task: Option<Task<()>>,
    snapshot: BuildSnapshot,
    last_success: Option<BuildArtifacts>,
    pub disk_stale: bool,
}

impl BuildUi {
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
    pub(super) fn refresh_preview_staleness(&mut self, cx: &mut Context<Self>) {
        let stale = self.build_ui.last_success.as_ref().is_some_and(|artifacts| {
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
        let Some(path) = self.document.as_ref().and_then(|document| document.path.clone()) else {
            self.error = Some("Save the source document before navigating to the preview".into());
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
        if self.busy || self.project.is_none() || self.build_ui.running() {
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
        let job = BuildJob::default();
        let worker = job.clone();
        self.build_ui.job = Some(job.clone());
        self.build_ui.snapshot = BuildSnapshot {
            state: BuildState::Running,
            ..BuildSnapshot::default()
        };
        cx.background_executor()
            .spawn(async move {
                match BundledResources::discover() {
                    Ok(resources) => worker.run(&request, &resources),
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
                            workspace.build_ui.disk_stale = false;
                        }
                        workspace.build_ui.snapshot = snapshot;
                        if finished {
                            workspace.build_ui.task = None;
                            workspace.refresh_preview_staleness(cx);
                        }
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

    pub(super) fn render_build_output(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = &self.build_ui.snapshot;
        let status = match snapshot.state {
            BuildState::Idle => "No build yet",
            BuildState::Running => "Building...",
            BuildState::Succeeded => "Build succeeded",
            BuildState::Failed => "Build failed",
            BuildState::Canceled => "Build canceled",
        };
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(spacing::small())
            .px(spacing::medium())
            .py(spacing::small())
            .bg(colors().chrome)
            .child(caption(status))
            .when_some(snapshot.failure.as_ref(), |view, failure| {
                view.child(inline_error(failure.message.clone()))
            })
            .when_some(self.build_ui.last_success.as_ref(), |view, artifacts| {
                view.child(caption(format!("Last successful PDF: {}", artifacts.pdf.display())))
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

pub(super) async fn prepare(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
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
