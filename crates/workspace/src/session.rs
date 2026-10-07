use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use ligature_settings::Session;

use crate::{Workspace, lifecycle::Intent};

impl Workspace {
    pub fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session_path = Session::default_path();
        self.start(Intent::Restore, window, cx);
    }

    fn session(&self) -> Session {
        let documents = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| {
                if index == self.active_tab {
                    self.document.as_ref()?.path.clone()
                } else {
                    tab.as_ref()?.document.path.clone()
                }
            })
            .collect();
        Session {
            project: self.project.as_ref().map(|project| project.root.clone()),
            documents,
            active_document: self.document.as_ref().and_then(|document| document.path.clone()),
        }
    }
}

pub(super) async fn save_session(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let snapshot = this
        .update_in(cx, |workspace, _, _| {
            workspace.session_path.clone().map(|path| (path, workspace.session()))
        })
        .map_err(|error| error.to_string())?;
    if let Some((path, session)) = snapshot {
        cx.background_executor().spawn(async move { session.save(&path) }).await?;
    }
    Ok(())
}

pub(super) async fn restore(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let path = this
        .update_in(cx, |workspace, _, _| workspace.session_path.clone())
        .map_err(|error| error.to_string())?;
    let Some(path) = path else {
        return Ok(());
    };
    let session = cx.background_executor().spawn(async move { Session::load(&path) }).await?;
    let mut errors = Vec::new();
    if let Some(path) = session.project
        && let Err(error) = super::project::load_project(this, cx, path).await
    {
        errors.push(error);
    }
    for path in session.documents {
        let file = cx.background_executor().spawn(async move { ligature_fs::read(&path) }).await;
        match file {
            Ok(loaded) => {
                this.update_in(cx, |workspace, window, cx| {
                    if !workspace.activate_path(&loaded.path, window, cx) {
                        workspace.replace_document(Some(loaded), window, cx);
                    }
                })
                .map_err(|error| error.to_string())?;
            }
            Err(error) => errors.push(format!("Skipped restored document: {error}")),
        }
    }
    this.update_in(cx, |workspace, window, cx| {
        if let Some(path) = session.active_document {
            workspace.activate_path(&path, window, cx);
        }
        if !errors.is_empty() {
            workspace.error = Some(errors.join("; "));
        }
        cx.notify();
    })
    .map_err(|error| error.to_string())?;
    Ok(())
}
