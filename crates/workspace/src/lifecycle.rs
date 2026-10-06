use std::path::PathBuf;

use gpui::{AsyncWindowContext, Context, PathPromptOptions, PromptButton, PromptLevel, WeakEntity, Window};
use ligature_fs::{FileState, decode};

use crate::Workspace;

#[derive(Clone, Copy)]
pub(super) enum Intent {
    New,
    Open,
    Save,
    SaveAs,
    Close,
    CloseWindow,
    Quit,
    CheckExternal,
}

impl Workspace {
    pub(super) fn start(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        if !matches!(intent, Intent::CheckExternal) {
            self.error = None;
        }
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = perform(&this, cx, intent).await;
            this.update_in(cx, |workspace, window, cx| {
                workspace.busy = false;
                if let Err(error) = result {
                    workspace.error = Some(error);
                }
                workspace.focus_editor(window, cx);
                cx.notify();
                if workspace.external_pending {
                    workspace.external_pending = false;
                    workspace.start(Intent::CheckExternal, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

async fn perform(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext, intent: Intent) -> Result<(), String> {
    match intent {
        Intent::Save | Intent::SaveAs => {
            save(this, cx, matches!(intent, Intent::SaveAs)).await?;
        }
        Intent::Open => {
            let picker = cx
                .update(|_, cx| {
                    cx.prompt_for_paths(PathPromptOptions {
                        files: true,
                        directories: false,
                        multiple: false,
                        prompt: Some("Open a TeX document".into()),
                    })
                })
                .map_err(|err| err.to_string())?;
            let Some(paths) = picker.await.map_err(|err| err.to_string())?.map_err(|err| err.to_string())? else {
                return Ok(());
            };
            let Some(path) = paths.into_iter().next() else {
                return Ok(());
            };
            let path = cx
                .background_executor()
                .spawn(async move { ligature_fs::resolve(&path) })
                .await
                .map_err(|err| err.to_string())?;
            let same = this
                .update_in(cx, |workspace, _, _| {
                    workspace.document.as_ref().is_some_and(|document| document.path.as_ref() == Some(&path))
                })
                .map_err(|err| err.to_string())?;
            if same {
                return Ok(());
            }
            let loaded = cx
                .background_executor()
                .spawn(async move { ligature_fs::read(&path) })
                .await
                .map_err(|err| err.to_string())?;
            if confirm_discard(this, cx).await? {
                this.update_in(cx, |workspace, window, cx| workspace.replace_document(Some(loaded), window, cx))
                    .map_err(|err| err.to_string())?;
            }
        }
        Intent::New => {
            if confirm_discard(this, cx).await? {
                this.update_in(cx, |workspace, window, cx| workspace.replace_document(None, window, cx))
                    .map_err(|err| err.to_string())?;
            }
        }
        Intent::Close | Intent::CloseWindow | Intent::Quit => {
            if confirm_discard(this, cx).await? {
                this.update_in(cx, |workspace, window, cx| {
                    workspace.close_document(cx);
                    match intent {
                        Intent::Quit => cx.quit(),
                        Intent::CloseWindow => window.remove_window(),
                        _ => {}
                    }
                })
                .map_err(|err| err.to_string())?;
            }
        }
        Intent::CheckExternal => check_external(this, cx).await?,
    }
    Ok(())
}

async fn confirm_discard(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<bool, String> {
    loop {
        let prompt = this
            .update_in(cx, |workspace, window, cx| {
                if !workspace.dirty(cx) {
                    return None;
                }
                let name = workspace.document.as_ref().expect("dirty document exists").name();
                Some(window.prompt(
                    PromptLevel::Warning,
                    &format!("Save changes to {name}?"),
                    Some("Your changes will be lost if you discard them."),
                    &[
                        PromptButton::ok("Save"),
                        PromptButton::Other("Discard".into()),
                        PromptButton::cancel("Cancel"),
                    ],
                    cx,
                ))
            })
            .map_err(|err| err.to_string())?;
        let Some(prompt) = prompt else {
            return Ok(true);
        };
        match prompt.await.map_err(|err| err.to_string())? {
            0 => {
                if !save(this, cx, false).await? {
                    return Ok(false);
                }
            }
            1 => return Ok(true),
            _ => return Ok(false),
        }
    }
}

async fn save(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext, save_as: bool) -> Result<bool, String> {
    let location = this
        .update_in(cx, |workspace, _, _| {
            workspace
                .document
                .as_ref()
                .map(|document| (document.path.clone(), document.disk.clone(), document.name()))
        })
        .map_err(|err| err.to_string())?;
    let Some((existing_path, disk, name)) = location else {
        return Ok(false);
    };
    let path = match (&existing_path, save_as) {
        (Some(path), false) => path.clone(),
        _ => {
            let directory = existing_path
                .as_ref()
                .and_then(|path| path.parent())
                .map_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")), |path| path.to_owned());
            let picker = cx
                .update(|_, cx| cx.prompt_for_new_path(&directory, Some(&name)))
                .map_err(|err| err.to_string())?;
            let Some(path) = picker.await.map_err(|err| err.to_string())?.map_err(|err| err.to_string())? else {
                return Ok(false);
            };
            path
        }
    };
    let path = cx
        .background_executor()
        .spawn(async move { ligature_fs::resolve(&path) })
        .await
        .map_err(|err| err.to_string())?;
    let expected = (existing_path.as_ref() == Some(&path)).then_some(disk);
    let (point, text) = this
        .update_in(cx, |workspace, _, cx| {
            workspace
                .editor
                .update(cx, |editor, cx| editor.update_state(|state| state.save_snapshot(), cx))
        })
        .map_err(|err| err.to_string())?;
    let bytes = text.as_bytes().to_vec();
    let saved_path = cx
        .background_executor()
        .spawn(async move { ligature_fs::write_atomic_checked(&path, &text, expected.as_ref()) })
        .await
        .map_err(|err| err.to_string())?;
    this.update_in(cx, |workspace, window, cx| {
        workspace.editor.update(cx, |editor, cx| {
            editor.update_state(|state| state.mark_saved_at(point), cx);
        });
        if let Some(document) = &mut workspace.document {
            document.path = Some(saved_path);
            document.disk = FileState::Present(bytes);
            document.diverged = false;
        }
        workspace.start_watching(window, cx);
    })
    .map_err(|err| err.to_string())?;
    Ok(true)
}

async fn check_external(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let path = this
        .update_in(cx, |workspace, _, _| {
            workspace.document.as_ref().and_then(|document| document.path.clone())
        })
        .map_err(|err| err.to_string())?;
    let Some(path) = path else {
        return Ok(());
    };
    let read_path = path.clone();
    let state = cx.background_executor().spawn(async move { FileState::read(&read_path) }).await;
    let changed = this
        .update_in(cx, |workspace, _, _| {
            workspace.document.as_ref().is_some_and(|document| document.disk != state)
        })
        .map_err(|err| err.to_string())?;
    if !changed {
        return Ok(());
    }
    match state {
        FileState::Present(bytes) => {
            let text = match decode(&path, bytes.clone()) {
                Ok(text) => text,
                Err(error) => {
                    this.update_in(cx, |workspace, _, _| {
                        if let Some(document) = &mut workspace.document {
                            document.disk = FileState::Present(bytes);
                            document.diverged = true;
                        }
                    })
                    .map_err(|err| err.to_string())?;
                    return Err(error.to_string());
                }
            };
            let prompt = this
                .update_in(cx, |workspace, window, cx| {
                    workspace.dirty(cx).then(|| {
                        window.prompt(
                            PromptLevel::Warning,
                            "This document changed on disk.",
                            Some("Reload the disk version and discard local changes, or keep your local text?"),
                            &[PromptButton::Other("Reload".into()), PromptButton::cancel("Keep Local")],
                            cx,
                        )
                    })
                })
                .map_err(|err| err.to_string())?;
            let reload = if let Some(prompt) = prompt {
                prompt.await.map_err(|err| err.to_string())? == 0
            } else {
                true
            };
            this.update_in(cx, |workspace, _, cx| {
                if reload {
                    workspace
                        .editor
                        .update(cx, |editor, cx| editor.update_state(|state| state.reload(text), cx));
                }
                if let Some(document) = &mut workspace.document {
                    document.disk = FileState::Present(bytes);
                    document.diverged = !reload;
                }
                workspace.error = None;
                cx.notify();
            })
            .map_err(|err| err.to_string())?;
        }
        FileState::Missing => {
            this.update_in(cx, |workspace, _, cx| {
                if let Some(document) = &mut workspace.document {
                    document.disk = FileState::Missing;
                    document.diverged = true;
                }
                workspace.error = Some(format!(
                    "'{}' was deleted or moved. Your text is still open; Save recreates the file, or use Save As.",
                    path.display()
                ));
                cx.notify();
            })
            .map_err(|err| err.to_string())?;
        }
        FileState::Unreadable(error) => {
            this.update_in(cx, |workspace, _, _| {
                if let Some(document) = &mut workspace.document {
                    document.disk = FileState::Unreadable(error.clone());
                    document.diverged = true;
                }
            })
            .map_err(|err| err.to_string())?;
            return Err(error);
        }
    }
    Ok(())
}
