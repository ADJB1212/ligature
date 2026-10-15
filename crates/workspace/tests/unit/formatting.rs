use std::fs;

use gpui::{Entity, TestAppContext};

use crate::{
    Workspace,
    lifecycle::Intent,
    tests::{open, start, type_text, workspace},
};

fn text(cx: &TestAppContext, root: &Entity<Workspace>) -> String {
    root.read_with(cx, |workspace, cx| workspace.editor.read(cx).state().text().to_owned())
}

#[test]
fn format_shortcut_uses_indentation_is_undoable_and_repeated_formatting_is_a_noop() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    let source = "\\begin{quote}\ntext\n\\end{quote}\n";
    type_text(&mut cx, &root, source);
    root.update(&mut cx, |workspace, cx| {
        workspace.settings.indentation.width = 3;
        workspace
            .editor
            .update(cx, |editor, cx| editor.update_state(|state| state.select(source.len(), source.len()), cx));
    });
    let shortcut = if cfg!(target_os = "macos") { "cmd-shift-i" } else { "ctrl-shift-i" };
    cx.simulate_keystrokes(window.into(), shortcut);
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "\\begin{quote}\n   text\n\\end{quote}\n");
    let revision = root.read_with(&cx, |workspace, cx| {
        let state = workspace.editor.read(cx).state();
        assert_eq!(state.caret(), state.text().len());
        assert!(state.is_dirty());
        state.buffer().revision()
    });
    cx.simulate_keystrokes(window.into(), shortcut);
    cx.run_until_parked();
    root.update(&mut cx, |workspace, cx| {
        workspace.editor.update(cx, |editor, cx| {
            assert_eq!(editor.state().buffer().revision(), revision);
            editor.update_state(|state| state.undo(), cx);
        })
    });
    assert_eq!(text(&cx, &root), source);
}

#[test]
fn format_on_save_updates_buffer_and_disk_preserves_crlf_and_marks_the_formatted_revision_clean() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("saved.tex");
    let source = "\\begin{quote}\r\ntext\r\n\\end{quote}\r\n";
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, source);
    root.update(&mut cx, |workspace, _| workspace.settings.format_on_save = true);
    start(&mut cx, window, Intent::Save);
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    let expected = "\\begin{quote}\r\n    text\r\n\\end{quote}\r\n";
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    assert_eq!(text(&cx, &root), expected);
    root.update(&mut cx, |workspace, cx| {
        workspace.editor.update(cx, |editor, cx| {
            assert!(!editor.state().is_dirty());
            editor.update_state(|state| state.undo(), cx);
            assert!(editor.state().is_dirty());
        })
    });
    assert_eq!(text(&cx, &root), source);
}

#[test]
fn default_saves_and_canceled_save_as_leave_text_unformatted() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("saved.tex");
    let source = "\\begin{quote}\ntext\n\\end{quote}";
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, source);
    start(&mut cx, window, Intent::Save);
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    root.update(&mut cx, |workspace, _| workspace.settings.format_on_save = true);
    start(&mut cx, window, Intent::SaveAs);
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), source);
    start(&mut cx, window, Intent::Save);
    assert_eq!(text(&cx, &root), "\\begin{quote}\n    text\n\\end{quote}\n");
    assert_eq!(fs::read_to_string(&path).unwrap(), text(&cx, &root));
}

#[test]
fn tex_formatting_skips_bibliographies_and_plain_text_including_save_as_targets() {
    for extension in ["bib", "txt"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("saved.{extension}"));
        let source = "\\begin{quote}\ntext\n\\end{quote}";
        let mut cx = TestAppContext::single();
        let (window, root) = workspace(&mut cx);
        type_text(&mut cx, &root, source);
        root.update(&mut cx, |workspace, _| workspace.settings.format_on_save = true);
        start(&mut cx, window, Intent::Save);
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        cx.run_until_parked();
        root.update(&mut cx, |workspace, cx| workspace.format_document(cx));
        assert_eq!(text(&cx, &root), source);
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
        open(&mut cx, window, &path);
        assert_eq!(text(&cx, &root), source);
    }
}

#[test]
fn conflicting_save_keeps_formatting_undoable_and_does_not_overwrite_disk() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("saved.tex");
    fs::write(&path, "").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &path);
    let source = "\\begin{quote}\ntext\n\\end{quote}";
    type_text(&mut cx, &root, source);
    root.update(&mut cx, |workspace, _| {
        workspace.settings.format_on_save = true;
        workspace.watch_task = None;
        workspace.watcher = None;
    });
    fs::write(&path, "external changes").unwrap();
    start(&mut cx, window, Intent::Save);
    assert_eq!(fs::read_to_string(&path).unwrap(), "external changes");
    assert_eq!(text(&cx, &root), "\\begin{quote}\n    text\n\\end{quote}\n");
    root.update(&mut cx, |workspace, cx| {
        assert!(workspace.error.is_some());
        workspace.editor.update(cx, |editor, cx| {
            assert!(editor.state().is_dirty());
            editor.update_state(|state| state.undo(), cx);
        });
    });
    assert_eq!(text(&cx, &root), source);
}
