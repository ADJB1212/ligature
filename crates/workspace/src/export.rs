use std::path::PathBuf;

use gpui::{AsyncWindowContext, WeakEntity};

use crate::Workspace;

pub(super) async fn export_pdf(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let (directory, name, bytes) = this
        .update_in(cx, |workspace, _, cx| {
            let (path, document) = workspace
                .preview
                .read(cx)
                .pdf_snapshot()
                .ok_or("Build a document before exporting a PDF.")?;
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let directory = workspace
                .project
                .as_ref()
                .map(|project| project.root.clone())
                .or_else(|| workspace.document.as_ref()?.path.as_ref()?.parent().map(|path| path.to_owned()))
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            Ok::<_, String>((directory, name, document.bytes()))
        })
        .map_err(|error| error.to_string())??;
    let picker = cx
        .update(|_, cx| cx.prompt_for_new_path(&directory, Some(&name)))
        .map_err(|error| error.to_string())?;
    let Some(path) = picker.await.map_err(|error| error.to_string())?.map_err(|error| error.to_string())? else {
        return Ok(());
    };
    cx.background_executor()
        .spawn(async move { ligature_fs::write_atomic_bytes(&path, &bytes) })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}
