use crate::Workspace;
use gpui::{Context, IntoElement, Task, Window, div, prelude::*};
use ligature_theme::{colors, spacing};
use ligature_toolchain::{BundleInstallProgress, BundledResources, install_upstream_bundle, installation_directory};
use ligature_ui::{Button, caption, inline_error};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Default)]
pub(super) struct BundleUi {
    pub visible: bool,
    task: Option<Task<()>>,
    cancel: Option<Arc<AtomicBool>>,
    status: Option<String>,
    error: Option<String>,
}

impl Drop for BundleUi {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Workspace {
    fn download_bundle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bundle_ui.task.is_some() {
            return;
        }
        self.bundle_ui.error = None;
        self.bundle_ui.status = Some("Preparing download...".into());
        let cancel = Arc::new(AtomicBool::new(false));
        self.bundle_ui.cancel = Some(cancel.clone());
        let progress = Arc::new(Mutex::new(None));
        let worker_progress = progress.clone();
        let worker_cancel = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        cx.background_executor()
            .spawn(async move {
                let result = (|| {
                    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
                    let cache = std::env::temp_dir().join("ligature-resource-validation");
                    let resources = BundledResources::for_executable(&executable, &cache)?;
                    let directory = installation_directory()?;
                    install_upstream_bundle(&resources, &directory, |status| {
                        *worker_progress.lock().unwrap() = Some(status);
                        !worker_cancel.load(Ordering::Relaxed)
                    })
                })();
                let _ = sender.send(result);
            })
            .detach();
        self.bundle_ui.task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let result = receiver.try_recv();
                let status = progress.lock().unwrap().clone();
                let finished = !matches!(result, Err(mpsc::TryRecvError::Empty));
                if this
                    .update_in(cx, |workspace, _, cx| {
                        if finished {
                            workspace.bundle_ui.task = None;
                            workspace.bundle_ui.cancel = None;
                            if cancel.load(Ordering::Relaxed) {
                                workspace.bundle_ui.status = Some("Download canceled.".into());
                            } else {
                                match result {
                                    Ok(Ok(_)) => {
                                        workspace.bundle_ui.status =
                                            Some("Full TeX bundle installed. Future builds will use it automatically.".into())
                                    }
                                    Ok(Err(error)) => {
                                        workspace.bundle_ui.status = None;
                                        workspace.bundle_ui.error = Some(error);
                                    }
                                    Err(_) => {
                                        workspace.bundle_ui.status = None;
                                        workspace.bundle_ui.error = Some("The bundle installer stopped unexpectedly. Try again.".into());
                                    }
                                }
                            }
                        } else if let Some(status) = status {
                            workspace.bundle_ui.status = Some(match status {
                                BundleInstallProgress::Downloading { received, total } => match total {
                                    Some(total) => format!(
                                        "Downloading and installing: {:.0}% ({:.0} / {:.0} MB)",
                                        received as f64 / total.max(1) as f64 * 100.0,
                                        received as f64 / 1_000_000.0,
                                        total as f64 / 1_000_000.0
                                    ),
                                    None => format!("Downloading and installing: {:.0} MB", received as f64 / 1_000_000.0),
                                },
                                BundleInstallProgress::Verifying => "Verifying installation...".into(),
                            });
                        }
                        cx.notify();
                    })
                    .is_err()
                    || finished
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(250)).await;
            }
        }));
        cx.notify();
    }

    pub(super) fn render_resources(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(spacing::small())
            .px(spacing::medium())
            .py(spacing::small())
            .bg(colors().chrome)
            .child(caption(
                "The editor includes common TeX packages. Download the full upstream bundle for additional packages (about 2.9 GB).",
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(spacing::small())
                    .child(
                        Button::new("download-tex-bundle", "Download full TeX bundle")
                            .disabled(self.bundle_ui.task.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.download_bundle(window, cx))),
                    )
                    .when(self.bundle_ui.task.is_some(), |view| {
                        view.child(
                            Button::new("cancel-tex-download", "Cancel")
                                .quiet()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(cancel) = &this.bundle_ui.cancel {
                                        cancel.store(true, Ordering::Relaxed);
                                        this.bundle_ui.status = Some("Canceling download...".into());
                                        cx.notify();
                                    }
                                })),
                        )
                    })
                    .when_some(self.bundle_ui.status.clone(), |view, status| view.child(caption(status))),
            )
            .when_some(self.bundle_ui.error.clone(), |view, error| view.child(inline_error(error)))
    }
}
