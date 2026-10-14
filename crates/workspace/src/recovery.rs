use std::time::Duration;

use gpui::{App, AsyncWindowContext, Context, PromptButton, PromptLevel, Task, WeakEntity};
use ligature_fs::FileState;
use ligature_settings::{Recovery, RecoveryDocument, RecoveryStore, RecoveryWrite};

use crate::{Document, Workspace};

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;

#[derive(Default)]
pub(super) struct RecoveryUi {
    pub store: Option<RecoveryStore>,
    ready: bool,
    task: Option<Task<()>>,
    pending: bool,
}

impl Workspace {
    fn recovery_snapshot(&self, cx: &App) -> Recovery {
        let documents = self
            .tabs
            .iter()
            .filter_map(|tab| {
                let (document, editor) = tab
                    .as_ref()
                    .map_or((self.document.as_ref(), &self.editor), |tab| (Some(&tab.document), &tab.editor));
                let document = document?;
                let state = editor.read(cx).state();
                (document.diverged || state.is_dirty()).then(|| RecoveryDocument {
                    path: document.path.clone(),
                    text: state.text().to_owned(),
                })
            })
            .collect();
        Recovery { documents }
    }

    fn prepare_recovery(&mut self, cx: &App) -> Option<RecoveryWrite> {
        if !self.recovery.ready {
            return None;
        }
        let store = self.recovery.store.as_ref()?;
        Some(store.prepare(self.recovery_snapshot(cx)))
    }

    pub(super) fn queue_recovery(&mut self, cx: &mut Context<Self>) {
        if !self.recovery.ready || self.recovery.store.is_none() {
            return;
        }
        self.recovery.pending = true;
        if self.recovery.task.is_some() {
            return;
        }
        self.recovery.task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let Ok(Some(write)) = this.update(cx, |workspace, cx| {
                    workspace.recovery.pending = false;
                    workspace.prepare_recovery(cx)
                }) else {
                    break;
                };
                let result = cx.background_executor().spawn(async move { write.save() }).await;
                let repeat = this
                    .update(cx, |workspace, cx| {
                        if let Err(error) = result {
                            workspace.error = Some(format!("Could not save recovery snapshot: {error}"));
                            cx.notify();
                        }
                        if workspace.recovery.pending {
                            true
                        } else {
                            workspace.recovery.task = None;
                            false
                        }
                    })
                    .unwrap_or(false);
                if !repeat {
                    break;
                }
            }
        }));
    }
}

pub(super) async fn flush(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let write = this
        .update_in(cx, |workspace, _, cx| {
            workspace.recovery.task = None;
            workspace.recovery.pending = false;
            workspace.prepare_recovery(cx)
        })
        .map_err(|error| error.to_string())?;
    if let Some(write) = write {
        cx.background_executor().spawn(async move { write.save() }).await?;
    }
    Ok(())
}

pub(super) async fn restore(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let store = this
        .update_in(cx, |workspace, _, _| workspace.recovery.store.clone())
        .map_err(|error| error.to_string())?;
    let Some(store) = store else { return Ok(()) };
    let load = store.clone();
    let recovery = cx.background_executor().spawn(async move { load.load() }).await?;
    if !recovery.documents.is_empty() {
        let prompt = this
            .update_in(cx, |_, window, cx| {
                window.prompt(
                    PromptLevel::Warning,
                    "Recover unsaved documents?",
                    Some(
                        "Ligature found unsaved text from an interrupted session. Recovery opens it in editable tabs without changing files on disk.",
                    ),
                    &[PromptButton::ok("Recover"), PromptButton::Other("Discard".into())],
                    cx,
                )
            })
            .map_err(|error| error.to_string())?;
        let decision = prompt.await.map_err(|error| error.to_string())?;
        if decision > 1 {
            return Err("Document recovery was canceled. Recovery data has been kept for the next launch.".into());
        }
        if decision == 0 {
            for recovered in recovery.documents {
                let path = recovered.path.clone();
                let disk = cx
                    .background_executor()
                    .spawn(async move { path.as_deref().map_or(FileState::Missing, FileState::read) })
                    .await;
                this.update_in(cx, |workspace, window, cx| {
                    let open = recovered.path.as_ref().is_some_and(|path| workspace.activate_path(path, window, cx));
                    if !open {
                        if workspace.tabs.len() == 1
                            && workspace.document.as_ref().is_some_and(|document| document.path.is_none())
                            && !workspace.dirty(cx)
                            && matches!(workspace.editor.read(cx).state().text(), "" | crate::NEW_DOCUMENT)
                        {
                            workspace.close_document(window, cx);
                        }
                        workspace.replace_document(None, window, cx);
                    }
                    let base = match &disk {
                        FileState::Present(bytes) => String::from_utf8(bytes.clone()).ok(),
                        _ => None,
                    };
                    workspace.document = Some(Document {
                        path: recovered.path,
                        diverged: base.is_none(),
                        disk,
                    });
                    workspace.editor.update(cx, |editor, cx| {
                        editor.update_state(
                            |state| {
                                state.reload(base.unwrap_or_default());
                                if state.text() != recovered.text {
                                    state.select_all();
                                    state.insert(&recovered.text, false);
                                }
                            },
                            cx,
                        );
                    });
                    workspace.configure_active_editor(cx);
                    workspace.start_watching(window, cx);
                    workspace.update_membership();
                    workspace.editor_changed(cx);
                })
                .map_err(|error| error.to_string())?;
            }
        }
    }
    this.update_in(cx, |workspace, _, _| workspace.recovery.ready = true)
        .map_err(|error| error.to_string())?;
    flush(this, cx).await
}
