use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{Context, Entity, Focusable, IntoElement, Task, Window, div, prelude::*, px};
use ligature_buffer::{DocumentId, Revision};
use ligature_completion::Target;
use ligature_diagnostics::{Provider, Store};
use ligature_editor::Editor;
use ligature_lsp::{Client, Event, Request, ServerConfig};
use ligature_ui::{caption, chrome_strip};
use serde_json::json;

use crate::Workspace;

#[derive(Default)]
pub(super) struct LanguageUi {
    pub visible: bool,
    root: Option<PathBuf>,
    generation: u64,
    started_generation: u64,
    client: Option<Arc<Client>>,
    startup_cancel: Option<Arc<AtomicBool>>,
    documents: BTreeMap<String, Document>,
    diagnostics: Store,
    status: String,
    stderr: String,
    task: Option<Task<()>>,
    completion_seen: Option<Target>,
    hover_seen: Option<Target>,
    completion_request: Option<Pending>,
    hover_request: Option<Pending>,
    completion_since: Option<Instant>,
}

impl LanguageUi {
    pub(super) fn stop(&mut self) -> Option<Arc<Client>> {
        self.generation += 1;
        self.root = None;
        if let Some(cancel) = self.startup_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.task = None;
        self.completion_request = None;
        self.hover_request = None;
        self.documents.clear();
        self.diagnostics.clear_provider(&provider());
        self.client.take()
    }
}

impl Drop for LanguageUi {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
#[path = "language_tests.rs"]
mod tests;

struct Document {
    path: PathBuf,
    id: DocumentId,
    revision: Revision,
    version: i64,
    text: Arc<str>,
}

struct Pending {
    target: Target,
    request: Request,
    text: Arc<str>,
    uri: String,
}

fn provider() -> Provider {
    Provider::LanguageServer("Texlab".into())
}

impl Workspace {
    fn language_root(&self) -> Option<PathBuf> {
        if !self.settings.language_server.enabled {
            return None;
        }
        self.project.as_ref().map(|project| project.root.clone()).or_else(|| {
            self.document
                .as_ref()
                .and_then(|document| document.path.as_ref())
                .and_then(|path| path.parent())
                .map(PathBuf::from)
        })
    }

    pub(super) fn restart_language_server(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.language_ui.startup_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.language_ui.generation += 1;
        cx.notify();
    }

    pub(super) fn clear_language_server(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.language_ui.startup_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.language_ui.completion_request = None;
        self.language_ui.hover_request = None;
        self.language_ui.completion_seen = None;
        self.language_ui.hover_seen = None;
        self.language_ui.diagnostics.clear_provider(&provider());
        self.language_ui.documents.clear();
        self.language_ui.client = None;
        for tab in &self.tabs {
            let editor = tab.as_ref().map_or(&self.editor, |tab| &tab.editor);
            editor.update(cx, |editor, cx| {
                editor.dismiss_assistance();
                editor.set_server_diagnostics(Vec::new(), cx);
                editor.set_hover_available(false, cx);
            });
        }
    }

    pub(super) fn language_document_closed(&mut self, id: DocumentId, cx: &mut Context<Self>) {
        if self
            .language_ui
            .completion_request
            .as_ref()
            .is_some_and(|request| request.target.document == id)
        {
            self.language_ui.completion_request = None;
            self.language_ui.completion_seen = None;
        }
        if self
            .language_ui
            .hover_request
            .as_ref()
            .is_some_and(|request| request.target.document == id)
        {
            self.language_ui.hover_request = None;
            self.language_ui.hover_seen = None;
        }
        if let Some(path) = self.document.as_ref().and_then(|document| document.path.as_ref()) {
            self.language_ui.diagnostics.clear_document(path);
        }
        if self.tabs.len() == 1 && self.project.is_none() && self.language_ui.task.is_some() {
            self.clear_language_server(cx);
        }
    }

    pub(super) fn queue_language_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.language_ui.task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(start) = this.update_in(cx, |workspace, window, cx| {
                    let root = workspace.language_root();
                    if root != workspace.language_ui.root || workspace.language_ui.generation != workspace.language_ui.started_generation {
                        workspace.clear_language_server(cx);
                        workspace.language_ui.root.clone_from(&root);
                        workspace.language_ui.started_generation = workspace.language_ui.generation;
                        workspace.language_ui.stderr.clear();
                        workspace.language_ui.status = if root.is_some() { "Starting Texlab..." } else { "Local completion available. Open a project or save a document to start Texlab." }.into();
                        cx.notify();
                        return root.map(|root| {
                            let cancel = Arc::new(AtomicBool::new(false));
                            workspace.language_ui.startup_cancel = Some(Arc::clone(&cancel));
                            (root, workspace.language_ui.generation, cancel)
                        });
                    }
                    workspace.poll_language_server(window, cx);
                    None
                }) else { break; };
                if let Some((root, generation, cancel)) = start {
                    let start_root = root.clone();
                    let result = cx.background_executor().spawn(async move {
                        let resources = ligature_toolchain::BundledResources::discover()?;
                        let settings = json!({"texlab":{
                            "build":{"executable":resources.executable,"args":["--only-cached","--untrusted","--bundle",resources.bundle,"--synctex","%f"],"onSave":false,"forwardSearchAfter":false},
                            "chktex":{"onOpenAndSave":false,"onEdit":false},
                            "latexindent":{"local":null},"diagnosticsDelay":200
                        }});
                        Client::start_cancellable(ServerConfig { executable: resources.language_server()?, arguments: vec!["run".into()], root: start_root, settings }, &cancel).map(Arc::new)
                    }).await;
                    if this.update_in(cx, |workspace, _, cx| {
                        if workspace.language_root().as_ref() != Some(&root) || workspace.language_ui.generation != generation { return; }
                        match result {
                            Ok(client) => { workspace.language_ui.client = Some(client); workspace.language_ui.status = "Texlab ready".into(); }
                            Err(error) => { workspace.language_ui.status = error; workspace.language_ui.visible = true; }
                        }
                        cx.notify();
                    }).is_err() { break; }
                }
                cx.background_executor().timer(Duration::from_millis(40)).await;
            }
        }));
    }

    fn language_editors(&self) -> Vec<(PathBuf, Entity<Editor>)> {
        self.tabs
            .iter()
            .filter_map(|tab| {
                let (document, editor) = tab
                    .as_ref()
                    .map_or((self.document.as_ref(), &self.editor), |tab| (Some(&tab.document), &tab.editor));
                let path = document?.path.as_ref()?;
                let extension = path.extension()?.to_str()?;
                (extension.eq_ignore_ascii_case("tex") || extension.eq_ignore_ascii_case("bib")).then(|| (path.clone(), editor.clone()))
            })
            .collect()
    }

    fn sync_language_documents(&mut self, client: &Client, cx: &mut Context<Self>) -> Result<(), String> {
        let editors = self.language_editors();
        let mut open = BTreeMap::new();
        for (path, editor) in &editors {
            open.insert(ligature_lsp::uri(path)?, editor);
        }
        let closed: Vec<_> = self
            .language_ui
            .documents
            .keys()
            .filter(|uri| !open.contains_key(*uri))
            .cloned()
            .collect();
        for uri in closed {
            if self.language_ui.completion_request.as_ref().is_some_and(|request| request.uri == uri) {
                self.language_ui.completion_request = None;
                self.language_ui.completion_seen = None;
            }
            if self.language_ui.hover_request.as_ref().is_some_and(|request| request.uri == uri) {
                self.language_ui.hover_request = None;
                self.language_ui.hover_seen = None;
            }
            let document = self.language_ui.documents.remove(&uri).unwrap();
            self.language_ui.diagnostics.clear_document(&document.path);
            if client.capabilities.open_close {
                client.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))?;
            }
        }
        for (path, editor) in editors {
            let uri = ligature_lsp::uri(&path)?;
            let buffer = editor.read(cx).state().buffer();
            let identity = (buffer.id(), buffer.revision());
            if self
                .language_ui
                .documents
                .get(&uri)
                .is_some_and(|document| (document.id, document.revision) == identity)
            {
                continue;
            }
            let text: Arc<str> = Arc::from(buffer.text());
            let previous = self.language_ui.documents.remove(&uri);
            let version = previous.as_ref().map_or(1, |document| document.version + 1);
            self.language_ui.diagnostics.clear_document(&path);
            editor.update(cx, |editor, cx| editor.set_server_diagnostics(Vec::new(), cx));
            if let Some(previous) = previous.as_ref().filter(|document| document.id == identity.0) {
                if let Some(change) = client.capabilities.change(&previous.text, &text) {
                    client.notify(
                        "textDocument/didChange",
                        json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[change]}),
                    )?;
                }
            } else if client.capabilities.open_close {
                if previous.is_some() {
                    client.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))?;
                }
                client.notify("textDocument/didOpen", json!({"textDocument":{"uri":uri,"languageId":if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("bib")) {"bibtex"} else {"latex"},"version":version,"text":text.as_ref()}}))?;
            }
            self.language_ui.documents.insert(
                uri,
                Document {
                    path,
                    id: identity.0,
                    revision: identity.1,
                    version,
                    text,
                },
            );
        }
        Ok(())
    }

    pub(super) fn language_saved(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.language_ui.client.clone() else {
            return;
        };
        if self.sync_language_documents(&client, cx).is_err() || !client.capabilities.save {
            return;
        }
        let Some(path) = self.document.as_ref().and_then(|document| document.path.as_ref()) else {
            return;
        };
        let Ok(uri) = ligature_lsp::uri(path) else {
            return;
        };
        if !self.language_ui.documents.contains_key(&uri) {
            return;
        }
        let mut params = json!({"textDocument":{"uri":uri}});
        if client.capabilities.save_text {
            params["text"] = json!(self.editor.read(cx).state().buffer().text());
        }
        let _ = client.notify("textDocument/didSave", params);
    }

    fn poll_language_server(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(client) = self.language_ui.client.clone() else {
            return;
        };
        for event in client.events() {
            match event {
                Event::Exited(error) => {
                    self.language_ui.status = error;
                }
                Event::Stderr(text) => {
                    self.language_ui.stderr.push_str(&text);
                    if self.language_ui.stderr.len() > 16 * 1024 {
                        let mut start = self.language_ui.stderr.len() - 16 * 1024;
                        while !self.language_ui.stderr.is_char_boundary(start) {
                            start += 1;
                        }
                        self.language_ui.stderr.drain(..start);
                    }
                    if self.language_ui.visible {
                        cx.notify();
                    }
                }
                Event::Notification { method, params } if method == "textDocument/publishDiagnostics" => {
                    let Some(uri) = params["uri"].as_str() else {
                        continue;
                    };
                    let Some(document) = self.language_ui.documents.get(uri) else {
                        continue;
                    };
                    if params["version"].as_i64().is_some_and(|version| version != document.version) {
                        continue;
                    }
                    if !self.language_editors().iter().any(|(path, editor)| {
                        let buffer = editor.read(cx).state().buffer();
                        *path == document.path && buffer.id() == document.id && buffer.revision() == document.revision
                    }) {
                        continue;
                    }
                    let diagnostics = ligature_lsp::diagnostics(&document.text, &document.path, &params["diagnostics"], client.capabilities.encoding);
                    self.language_ui.diagnostics.replace(document.path.clone(), provider(), diagnostics);
                    for (path, editor) in self.language_editors() {
                        if path == document.path && editor.read(cx).state().buffer().revision() == document.revision {
                            let diagnostics = self.language_ui.diagnostics.document(&path, &provider()).to_vec();
                            editor.update(cx, |editor, cx| editor.set_server_diagnostics(diagnostics, cx));
                        }
                    }
                    if self.language_ui.visible {
                        cx.notify();
                    }
                }
                Event::Notification { method, params } if method == "window/showMessage" || method == "window/logMessage" => {
                    if let Some(message) = params["message"].as_str() {
                        self.language_ui.status = message.into();
                        cx.notify();
                    }
                }
                _ => {}
            }
        }
        if let Some(error) = client.failure() {
            self.clear_language_server(cx);
            self.language_ui.status = error;
            self.language_ui.visible = true;
            cx.notify();
            return;
        }
        if let Err(error) = self.sync_language_documents(&client, cx) {
            self.language_ui.status = error;
            self.clear_language_server(cx);
            cx.notify();
            return;
        }
        let focused = self.document.is_some() && self.editor.focus_handle(cx).is_focused(window);
        let editor = self.editor.read(cx);
        let completion = focused.then(|| editor.completion_target()).flatten();
        let hover = focused.then(|| editor.hover_target()).flatten();
        self.editor.update(cx, |editor, cx| {
            editor.set_hover_available(self.settings.language_server.hover && client.capabilities.hover, cx)
        });
        if self.language_ui.completion_seen != completion {
            self.language_ui.completion_seen = completion;
            self.language_ui.completion_request = None;
            self.language_ui.completion_since = Some(Instant::now());
        }
        let hover_start = self.language_ui.hover_seen != hover;
        if hover_start {
            self.language_ui.hover_seen = hover;
            self.language_ui.hover_request = None;
        }
        let document = self
            .document
            .as_ref()
            .and_then(|document| document.path.as_ref())
            .and_then(|path| ligature_lsp::uri(path).ok())
            .and_then(|uri| self.language_ui.documents.get(&uri).map(|document| (uri, Arc::clone(&document.text))));
        if let Some((uri, text)) = document {
            let mut start = |target: Target, method| -> Option<Pending> {
                let position = if method == "textDocument/completion" {
                    ligature_lsp::completion_position(&text, target.caret, client.capabilities.encoding)?
                } else {
                    ligature_lsp::position(&text, target.caret, client.capabilities.encoding)?
                };
                match client.request(method, json!({"textDocument":{"uri":uri},"position":position})) {
                    Ok(request) => Some(Pending {
                        target,
                        request,
                        text: Arc::clone(&text),
                        uri: uri.clone(),
                    }),
                    Err(error) => {
                        self.language_ui.status = error;
                        None
                    }
                }
            };
            if self.settings.language_server.completion
                && client.capabilities.completion
                && self.language_ui.completion_request.is_none()
                && self
                    .language_ui
                    .completion_since
                    .is_some_and(|since| since.elapsed() >= Duration::from_millis(100))
            {
                if let Some(target) = completion {
                    self.language_ui.completion_request = start(target, "textDocument/completion");
                }
                self.language_ui.completion_since = None;
            }
            if hover_start
                && self.settings.language_server.hover
                && client.capabilities.hover
                && self.language_ui.hover_request.is_none()
                && let Some(target) = hover
                && self.language_ui.hover_seen == Some(target)
            {
                self.language_ui.hover_request = start(target, "textDocument/hover");
            }
        }
        if let Some(pending) = &mut self.language_ui.completion_request
            && let Some(result) = pending.request.poll()
        {
            if let Ok(result) = result {
                let fallback = ligature_completion::context(&pending.text, pending.target.caret)
                    .map_or(pending.target.caret..pending.target.caret, |context| context.range);
                let candidates = ligature_lsp::completions(&pending.text, &result, fallback, client.capabilities.encoding);
                let target = pending.target;
                self.editor.update(cx, |editor, cx| {
                    editor.publish_completion(target, candidates, cx);
                });
            }
            self.language_ui.completion_request = None;
        }
        if let Some(pending) = &mut self.language_ui.hover_request
            && let Some(result) = pending.request.poll()
        {
            if let Ok(result) = result {
                let target = pending.target;
                let text = ligature_lsp::hover(&result);
                self.editor.update(cx, |editor, cx| {
                    editor.publish_hover(target, text, cx);
                });
            }
            self.language_ui.hover_request = None;
        }
    }

    pub(super) fn render_language_server(&self) -> impl IntoElement {
        chrome_strip()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(caption(if self.language_ui.status.is_empty() {
                        "Local completion available"
                    } else {
                        &self.language_ui.status
                    })),
            )
            .children(self.language_ui.diagnostics.iter().take(20).map(|diagnostic| {
                caption(format!(
                    "{}: {}",
                    diagnostic.path.as_ref().map_or(String::new(), |path| path.display().to_string()),
                    diagnostic.message
                ))
            }))
            .when(!self.language_ui.stderr.is_empty(), |view| {
                view.child(div().max_h(px(100.0)).overflow_hidden().child(caption(self.language_ui.stderr.clone())))
            })
    }
}
