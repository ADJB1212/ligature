use std::path::PathBuf;

use gpui::{AsyncWindowContext, Context, PathPromptOptions, PromptButton, PromptLevel, WeakEntity, Window};
use ligature_fs::{FileState, decode};

use crate::Workspace;

#[derive(Clone)]
pub(super) enum Intent {
    New,
    Open,
    OpenPath(PathBuf),
    OpenSource(ligature_synctex::SourcePosition),
    OpenDiagnostic(ligature_diagnostics::Diagnostic),
    OpenSearchResult {
        path: PathBuf,
        original: String,
        range: std::ops::Range<usize>,
        query: ligature_search::Query,
    },
    OpenProject,
    SelectRoot,
    Restore,
    Save,
    SaveAs,
    Build,
    Close,
    CloseTab(usize),
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
            let mut result = perform(&this, cx, intent.clone()).await;
            if result.is_ok() && !matches!(intent, Intent::Quit | Intent::CloseWindow | Intent::Restore) {
                result = super::session::save_session(&this, cx).await;
            }
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
        Intent::Open | Intent::OpenPath(_) => {
            let path = if let Intent::OpenPath(path) = intent {
                path
            } else {
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
                path
            };
            let path = cx
                .background_executor()
                .spawn(async move { ligature_fs::resolve(&path) })
                .await
                .map_err(|err| err.to_string())?;
            let same = this
                .update_in(cx, |workspace, window, cx| workspace.activate_path(&path, window, cx))
                .map_err(|err| err.to_string())?;
            if same {
                return Ok(());
            }
            let loaded = cx
                .background_executor()
                .spawn(async move { ligature_fs::read(&path) })
                .await
                .map_err(|err| err.to_string())?;
            this.update_in(cx, |workspace, window, cx| workspace.replace_document(Some(loaded), window, cx))
                .map_err(|err| err.to_string())?;
        }
        Intent::OpenDiagnostic(diagnostic) => {
            let path = diagnostic
                .path
                .ok_or("This diagnostic has no source file. See the raw build log for details.")?;
            open_path(this, cx, path).await?;
            this.update_in(cx, |workspace, window, cx| {
                if let Some(range) = diagnostic.range {
                    let range = range
                        .byte_range(workspace.editor.read(cx).state().text())
                        .ok_or("The diagnostic source location no longer exists. Build again.")?;
                    workspace
                        .editor
                        .update(cx, |editor, cx| editor.update_state(|state| state.select(range.start, range.end), cx));
                }
                workspace.focus_editor(window, cx);
                Ok::<_, String>(())
            })
            .map_err(|error| error.to_string())??;
        }
        Intent::OpenSource(source) => {
            open_path(this, cx, source.path).await?;
            this.update_in(cx, |workspace, window, cx| {
                let line = source.line.saturating_sub(1);
                let state = workspace.editor.read(cx).state();
                if line >= state.buffer().line_count() {
                    return Err("The SyncTeX source line no longer exists. Rebuild the document.".into());
                }
                let mut range = state.line_range(line);
                if source.column > 0 {
                    let text = &state.text()[range.clone()];
                    let column = text.char_indices().nth(source.column - 1).map_or(text.len(), |(offset, _)| offset);
                    let start = range.start + column;
                    range = start..start;
                }
                workspace
                    .editor
                    .update(cx, |editor, cx| editor.update_state(|state| state.select(range.start, range.end), cx));
                workspace.focus_editor(window, cx);
                Ok::<_, String>(())
            })
            .map_err(|error| error.to_string())??;
        }
        Intent::New => {
            this.update_in(cx, |workspace, window, cx| workspace.replace_document(None, window, cx))
                .map_err(|err| err.to_string())?;
        }
        Intent::OpenSearchResult {
            path,
            original,
            range,
            query,
        } => {
            open_path(this, cx, path).await?;
            this.update_in(cx, |workspace, window, cx| {
                let search = ligature_search::Search::new(&query)?;
                let current = workspace.editor.read(cx).state().text();
                let range = ligature_search::relocate_match(&original, current, &range, &search)
                    .ok_or("This search result changed. Refresh the project search before opening it.")?;
                workspace
                    .editor
                    .update(cx, |editor, cx| editor.update_state(|state| state.select_search_match(range), cx));
                workspace.focus_editor(window, cx);
                Ok::<_, String>(())
            })
            .map_err(|error| error.to_string())??;
        }
        Intent::CloseTab(index) => {
            let original = this
                .update_in(cx, |workspace, window, cx| {
                    let original = workspace.active_tab;
                    workspace.switch_tab(index, window, cx);
                    original
                })
                .map_err(|error| error.to_string())?;
            let decision = confirm_discard(this, cx).await;
            this.update_in(cx, |workspace, window, cx| {
                if matches!(decision, Ok(true)) {
                    workspace.close_document(window, cx);
                    if original != index {
                        workspace.switch_tab(if original > index { original - 1 } else { original }, window, cx);
                    }
                } else {
                    workspace.switch_tab(original, window, cx);
                }
            })
            .map_err(|error| error.to_string())?;
            decision?;
        }
        Intent::Close | Intent::CloseWindow | Intent::Quit => {
            let all = matches!(intent, Intent::CloseWindow | Intent::Quit);
            if all {
                let (count, original) = this
                    .update_in(cx, |workspace, _, _| (workspace.tabs.len(), workspace.active_tab))
                    .map_err(|error| error.to_string())?;
                for index in 0..count {
                    this.update_in(cx, |workspace, window, cx| workspace.switch_tab(index, window, cx))
                        .map_err(|error| error.to_string())?;
                    let decision = confirm_discard(this, cx).await;
                    if !matches!(decision, Ok(true)) {
                        this.update_in(cx, |workspace, window, cx| workspace.switch_tab(original, window, cx))
                            .map_err(|error| error.to_string())?;
                        decision?;
                        return Ok(());
                    }
                }
                this.update_in(cx, |workspace, window, cx| workspace.switch_tab(original, window, cx))
                    .map_err(|error| error.to_string())?;
                super::session::save_session(this, cx).await?;
                super::builds::shutdown(this, cx).await?;
                this.update_in(cx, |workspace, window, cx| {
                    workspace.project_task = None;
                    workspace.search.cancel();
                    while workspace.document.is_some() {
                        workspace.close_document(window, cx);
                    }
                    if matches!(intent, Intent::Quit) {
                        cx.quit();
                    } else {
                        window.remove_window();
                    }
                })
                .map_err(|error| error.to_string())?;
            } else if confirm_discard(this, cx).await? {
                this.update_in(cx, |workspace, window, cx| workspace.close_document(window, cx))
                    .map_err(|error| error.to_string())?;
            }
        }
        Intent::CheckExternal => check_external(this, cx).await?,
        Intent::OpenProject => super::project::open_project(this, cx).await?,
        Intent::SelectRoot => super::project::select_root(this, cx).await?,
        Intent::Restore => super::session::restore(this, cx).await?,
        Intent::Build => super::builds::prepare(this, cx).await?,
    }
    Ok(())
}

async fn open_path(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext, path: PathBuf) -> Result<(), String> {
    if this
        .update_in(cx, |workspace, window, cx| workspace.activate_build_source(&path, window, cx))
        .map_err(|error| error.to_string())??
    {
        return Ok(());
    }
    let path = cx
        .background_executor()
        .spawn(async move { ligature_fs::resolve(&path) })
        .await
        .map_err(|error| error.to_string())?;
    if this
        .update_in(cx, |workspace, window, cx| workspace.activate_path(&path, window, cx))
        .map_err(|error| error.to_string())?
    {
        return Ok(());
    }
    let loaded = cx
        .background_executor()
        .spawn(async move { ligature_fs::read(&path) })
        .await
        .map_err(|error| error.to_string())?;
    this.update_in(cx, |workspace, window, cx| workspace.replace_document(Some(loaded), window, cx))
        .map_err(|error| error.to_string())?;
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

pub(super) async fn save(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext, save_as: bool) -> Result<bool, String> {
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
    let duplicate = this
        .update_in(cx, |workspace, _, _| {
            workspace.tabs.iter().flatten().any(|tab| tab.document.path.as_ref() == Some(&path))
        })
        .map_err(|err| err.to_string())?;
    if duplicate {
        return Err("This file is already open in another tab. Close that tab before using Save As.".into());
    }
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
        workspace.update_membership();
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
