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
}

impl BuildUi {
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
                            workspace.build_ui.last_success = Some(artifacts.clone());
                        }
                        workspace.build_ui.snapshot = snapshot;
                        if finished {
                            workspace.build_ui.task = None;
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
