use super::*;
use gpui::TestAppContext;
use std::{fs, thread};

fn request(client: &Client, method: &str, params: serde_json::Value) -> serde_json::Value {
    let mut request = client.request(method, params).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(response) = request.poll() {
            return response.unwrap();
        }
        assert!(Instant::now() < deadline, "{method} timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn client(root: &std::path::Path) -> Client {
    let resources = ligature_toolchain::BundledResources::discover().unwrap();
    Client::start(ServerConfig {
        executable: resources.language_server().unwrap(),
        arguments: vec!["run".into()],
        root: root.into(),
        settings: json!({"texlab":{"build":{"onSave":false},"chktex":{"onOpenAndSave":false,"onEdit":false},"diagnosticsDelay":0}}),
    })
    .unwrap()
}

#[test]
fn bundled_texlab_completes_snippets_and_publishes_diagnostics_in_a_real_project() {
    let directory = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    let path = root.join("main.tex");
    let text = "\\documentclass{article}\n\\input{child}\n\\bibliography{refs}\n\\begin{document}\nα😀\\be";
    fs::write(&path, text).unwrap();
    fs::write(root.join("child.tex"), "\\section{Included}\\label{target}").unwrap();
    fs::write(root.join("refs.bib"), "@book{book, title={Title}, year={2026}}").unwrap();
    let client = client(&root);
    assert!(client.capabilities.completion && client.capabilities.hover && client.capabilities.open_close);
    let uri = ligature_lsp::uri(&path).unwrap();
    client
        .notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"latex","version":1,"text":text}}),
        )
        .unwrap();
    let result = request(
        &client,
        "textDocument/completion",
        json!({"textDocument":{"uri":uri},"position":ligature_lsp::completion_position(text,text.len(),client.capabilities.encoding)}),
    );
    let candidates = ligature_lsp::completions(text, &result, text.len() - 2..text.len(), client.capabilities.encoding);
    let begin = candidates
        .iter()
        .find(|candidate| candidate.label == "begin")
        .unwrap_or_else(|| panic!("Texlab must provide the begin snippet: {result}; candidates: {candidates:?}"));
    let plan = ligature_completion::plan(text, begin).unwrap();
    assert!(begin.snippet);
    assert_eq!(plan.placeholders.len(), 2);
    assert_eq!(plan.mirrors.len(), 1);
    let changed = "\\documentclass{article}\n\\begin{document}\n}\n\\end{document}";
    client
        .notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[client.capabilities.change(text,changed).unwrap()]}),
        )
        .unwrap();
    client.notify("textDocument/didSave", json!({"textDocument":{"uri":uri}})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut diagnostic = false;
    while Instant::now() < deadline && !diagnostic {
        for event in client.events() {
            if let Event::Notification { method, params } = event
                && method == "textDocument/publishDiagnostics"
                && params["uri"] == uri
            {
                diagnostic |= !ligature_lsp::diagnostics(changed, &path, &params["diagnostics"], client.capabilities.encoding).is_empty();
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(diagnostic, "Texlab must diagnose the unmatched brace");
    let result = request(
        &client,
        "textDocument/hover",
        json!({"textDocument":{"uri":uri},"position":{"line":0,"character":2}}),
    );
    assert!(result.is_null() || result.get("contents").is_some());
    client.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}})).unwrap();
}

#[test]
fn document_sync_tracks_versions_saves_and_close_and_clears_provider_diagnostics() {
    let directory = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    let path = root.join("main.tex");
    fs::write(&path, "α😀\\sec").unwrap();
    let client = client(&root);
    let mut cx = TestAppContext::single();
    let window = cx.add_window(Workspace::new);
    let workspace = window.root(&mut cx).unwrap();
    workspace.update(&mut cx, |workspace, cx| {
        workspace.language_ui.task = None;
        workspace.document.as_mut().unwrap().path = Some(path.clone());
        workspace
            .editor
            .update(cx, |editor, cx| editor.set_buffer(ligature_buffer::Buffer::from_text("α😀\\sec"), cx));
        workspace.language_ui.client = Some(Arc::new(client));
        let client = workspace.language_ui.client.clone().unwrap();
        workspace.sync_language_documents(&client, cx).unwrap();
        let uri = ligature_lsp::uri(&path).unwrap();
        assert_eq!(workspace.language_ui.documents[&uri].version, 1);
        workspace.editor.update(cx, |editor, cx| {
            editor.update_state(|state| state.move_to(state.text().len(), false), cx);
        });
        workspace.sync_language_documents(&client, cx).unwrap();
        assert_eq!(workspace.language_ui.documents[&uri].version, 1);
        workspace.editor.update(cx, |editor, cx| {
            editor.update_state(|state| state.insert("tion", true), cx);
        });
        workspace.sync_language_documents(&client, cx).unwrap();
        assert_eq!(workspace.language_ui.documents[&uri].version, 2);
        assert_eq!(workspace.language_ui.documents[&uri].text.as_ref(), "α😀\\section");
        workspace.language_saved(cx);
        workspace.language_ui.diagnostics.replace(
            path.clone(),
            provider(),
            vec![ligature_diagnostics::Diagnostic {
                severity: ligature_diagnostics::Severity::Error,
                provider: provider(),
                path: Some(path.clone()),
                range: None,
                message: "problem".into(),
            }],
        );
        workspace.document = None;
        workspace.tabs.clear();
        workspace.sync_language_documents(&client, cx).unwrap();
        assert!(workspace.language_ui.documents.is_empty());
        assert_eq!(workspace.language_ui.diagnostics.iter().count(), 0);
        workspace.clear_language_server(cx);
        assert!(workspace.language_ui.client.is_none());
    });
}
