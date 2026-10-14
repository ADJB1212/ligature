use crate::Workspace;
use gpui::{Context, IntoElement, Task, Window, div, prelude::*, px, relative};
use ligature_theme::{colors, spacing};
use ligature_toolchain::{BundleInstallProgress, BundledResources, install_upstream_bundle, installation_directory, installed_bundle_path};
use ligature_ui::{Button, Indicator, caption, chrome_strip, indicator, inline_error};
use std::{
    path::PathBuf,
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
    inspection: Option<Task<()>>,
    installed: Option<PathBuf>,
    cancel: Option<Arc<AtomicBool>>,
    status: Option<String>,
    download_progress: Option<(u64, u64)>,
    error: Option<String>,
}

impl BundleUi {
    pub(super) fn cancel(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.task = None;
        self.inspection = None;
    }

    pub(super) fn busy(&self) -> bool {
        self.task.is_some()
    }
}

impl Drop for BundleUi {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Workspace {
    pub(super) fn check_installed_bundle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bundle_ui.busy() || self.bundle_ui.inspection.is_some() {
            return;
        }
        self.bundle_ui.installed = None;
        self.bundle_ui.error = None;
        self.bundle_ui.status = Some("Checking installed TeX bundle...".into());
        let check = cx
            .background_executor()
            .spawn(async { installed_bundle_path(&installation_directory()?) });
        self.bundle_ui.inspection = Some(cx.spawn_in(window, async move |this, cx| {
            let result = check.await;
            let _ = this.update_in(cx, |workspace, _, cx| {
                workspace.bundle_ui.inspection = None;
                match result {
                    Ok(path) => {
                        workspace.bundle_ui.status = path
                            .as_ref()
                            .map(|_| "Full TeX bundle installed. Future builds will use it automatically.".into());
                        workspace.bundle_ui.installed = path;
                    }
                    Err(error) => {
                        workspace.bundle_ui.status = None;
                        workspace.bundle_ui.error = Some(error);
                    }
                }
                cx.notify();
            });
        }));
    }

    fn download_bundle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bundle_ui.busy() || self.bundle_ui.inspection.is_some() || self.bundle_ui.installed.is_some() {
            return;
        }
        self.bundle_ui.error = None;
        self.bundle_ui.status = Some("Preparing download...".into());
        self.bundle_ui.download_progress = None;
        let cancel = Arc::new(AtomicBool::new(false));
        self.bundle_ui.cancel = Some(Arc::clone(&cancel));
        let progress = Arc::new(Mutex::new(None));
        let worker_progress = Arc::clone(&progress);
        let worker_cancel = Arc::clone(&cancel);
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
                            workspace.bundle_ui.download_progress = None;
                            if cancel.load(Ordering::Relaxed) {
                                workspace.bundle_ui.status = Some("Download canceled.".into());
                            } else {
                                match result {
                                    Ok(Ok(path)) => {
                                        workspace.bundle_ui.installed = Some(path);
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
                        } else if let Some(status) = status
                            && !cancel.load(Ordering::Relaxed)
                        {
                            workspace.bundle_ui.download_progress = match status {
                                BundleInstallProgress::Downloading {
                                    received,
                                    total: Some(total),
                                } if total > 0 => Some((received, total)),
                                _ => None,
                            };
                            workspace.bundle_ui.status = Some(match status {
                                BundleInstallProgress::Downloading { received, total } => match total {
                                    Some(total) if total > 0 => "Downloading and installing TeX bundle...".into(),
                                    _ if received == 0 => "Connecting to download...".into(),
                                    _ => format!("Downloading and installing: {:.0} MB", received as f64 / 1_000_000.0),
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
        chrome_strip()
            .flex()
            .flex_col()
            .gap(spacing::small())
            .child(caption(
                "The editor includes common TeX packages. Download the full upstream bundle for additional packages (about 2.9 GB).",
            ))
            .when_some(installation_directory().ok(), |view, directory| {
                view.child(caption(format!("Installation directory: {}", directory.display())))
            })
            .when_some(self.bundle_ui.installed.as_ref(), |view, path| {
                view.child(caption(format!("Installed bundle: {}", path.display())))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(spacing::small())
                    .child(
                        Button::new(
                            "download-tex-bundle",
                            if self.bundle_ui.installed.is_some() {
                                "Full TeX bundle installed"
                            } else {
                                "Download full TeX bundle"
                            },
                        )
                        .disabled(self.bundle_ui.busy() || self.bundle_ui.inspection.is_some() || self.bundle_ui.installed.is_some())
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
                    }),
            )
            .when_some(self.bundle_ui.status.clone(), |view, status| {
                view.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(spacing::medium())
                        .when(self.bundle_ui.busy() || self.bundle_ui.inspection.is_some(), |row| {
                            row.child(indicator("bundle-live", Indicator::Live))
                        })
                        .child(caption(status)),
                )
            })
            .when_some(self.bundle_ui.download_progress, |view, (received, total)| {
                let fraction = (received as f64 / total as f64).clamp(0.0, 1.0) as f32;
                view.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(spacing::small())
                        .child(
                            div()
                                .h(px(4.0))
                                .w_full()
                                .rounded(spacing::radius())
                                .overflow_hidden()
                                .bg(colors().border)
                                .child(div().h_full().w(relative(fraction)).bg(colors().accent)),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .child(caption(format!(
                                    "{:.2} / {:.2} GB",
                                    received as f64 / 1_000_000_000.0,
                                    total as f64 / 1_000_000_000.0
                                )))
                                .child(caption(format!("{:.0}%", fraction * 100.0))),
                        ),
                )
            })
            .when_some(self.bundle_ui.error.clone(), |view, error| view.child(inline_error(error)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[test]
    fn an_installed_bundle_prevents_starting_another_download() {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(Workspace::new);
        let root = window.root(&mut cx).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            root.update(cx, |workspace, cx| {
                let path = PathBuf::from("/installed/full-bundle.zip");
                workspace.bundle_ui.installed = Some(path.clone());
                workspace.download_bundle(window, cx);
                assert!(!workspace.bundle_ui.busy());
                assert!(workspace.bundle_ui.cancel.is_none());
                assert_eq!(workspace.bundle_ui.installed, Some(path));
            });
        })
        .unwrap();
    }
}
