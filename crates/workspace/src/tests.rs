use std::{cell::Cell, fs, path::Path, rc::Rc};

use gpui::{AppContext, Entity, TestAppContext, WindowHandle};

use super::*;

fn workspace(cx: &mut TestAppContext) -> (WindowHandle<Workspace>, Entity<Workspace>) {
    cx.update(init);
    let window = cx.add_window(Workspace::new);
    let root = window.root(cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    (window, root)
}

fn start(cx: &mut TestAppContext, window: WindowHandle<Workspace>, intent: Intent) {
    cx.update_window(window.into(), |root, window, cx| {
        root.downcast::<Workspace>()
            .unwrap()
            .update(cx, |workspace, cx| workspace.start(intent, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
}

fn type_text(cx: &mut TestAppContext, root: &Entity<Workspace>, text: &str) {
    root.update(cx, |workspace, cx| {
        workspace
            .editor
            .update(cx, |editor, cx| editor.update_state(|state| state.insert(text, true), cx))
    });
}

fn text(cx: &TestAppContext, root: &Entity<Workspace>) -> String {
    root.read_with(cx, |workspace, cx| workspace.editor.read(cx).state().text().to_owned())
}

fn open(cx: &mut TestAppContext, window: WindowHandle<Workspace>, path: &Path) {
    start(cx, window, Intent::Open);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files);
        assert!(!options.directories);
        assert!(!options.multiple);
        Some(vec![path.to_owned()])
    });
    cx.run_until_parked();
}

#[test]
fn open_reuses_the_buffer_and_preserves_unsaved_edits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("example.tex");
    fs::write(&path, "loaded\r\n").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &path);
    assert_eq!(text(&cx, &root), "loaded\r\n");
    let id = root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().buffer().id());
    type_text(&mut cx, &root, "local ");
    open(&mut cx, window, &directory.path().join("./example.tex"));
    assert_eq!(text(&cx, &root), "local loaded\r\n");
    assert!(!cx.has_pending_prompt());
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().buffer().id()), id);
}

#[test]
fn canceled_open_and_failed_reads_leave_the_document_open() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.tex");
    fs::write(&path, [0xff]).unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "local");
    start(&mut cx, window, Intent::Open);
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "local");
    open(&mut cx, window, &path);
    assert_eq!(text(&cx, &root), "local");
    assert!(root.read_with(&cx, |workspace, _| workspace.error.as_ref().unwrap().contains("unsupported encoding")));
    assert!(!cx.has_pending_prompt());
}

#[test]
fn save_as_associates_the_path_and_later_saves_preserve_crlf() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("saved.tex");
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "first\r\n");
    start(&mut cx, window, Intent::Save);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&path).unwrap(), "first\r\n");
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    assert_eq!(
        root.read_with(&cx, |workspace, _| workspace.document.as_ref().unwrap().path.clone()),
        Some(fs::canonicalize(&path).unwrap())
    );
    assert!(root.read_with(&cx, |workspace, _| workspace.watcher.is_some()));
    type_text(&mut cx, &root, "second\r\n");
    start(&mut cx, window, Intent::Save);
    assert!(!cx.did_prompt_for_new_path());
    assert_eq!(fs::read_to_string(&path).unwrap(), "first\r\nsecond\r\n");
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    assert!(!cx.has_pending_prompt());
}

#[test]
fn close_new_and_quit_cancel_keep_dirty_documents_and_restore_focus() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "unsaved");
    for intent in [Intent::Close, Intent::New, Intent::Quit, Intent::CloseWindow] {
        start(&mut cx, window, intent);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(text(&cx, &root), "unsaved");
        assert!(root.read_with(&cx, |workspace, _| workspace.document.is_some()));
        cx.update_window(window.into(), |_, window, cx| {
            assert!(root.read(cx).editor.focus_handle(cx).is_focused(window))
        })
        .unwrap();
    }
    start(&mut cx, window, Intent::New);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "");
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
}

#[test]
fn canceled_save_and_failed_save_abort_the_close_flow() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "unsaved");
    start(&mut cx, window, Intent::Close);
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "unsaved");
    assert!(root.read_with(&cx, |workspace, _| workspace.document.is_some()));
    let directory = tempfile::tempdir().unwrap();
    start(&mut cx, window, Intent::Close);
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    cx.simulate_new_path_selection(|_| Some(directory.path().to_owned()));
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "unsaved");
    assert!(root.read_with(&cx, |workspace, _| workspace.error.is_some()));
    assert!(root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    assert!(root.read_with(&cx, |workspace, _| workspace.document.as_ref().unwrap().path.is_none()));
}

#[test]
fn saving_during_close_writes_before_releasing_the_document_and_watcher() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("close.tex");
    fs::write(&path, "original").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &path);
    type_text(&mut cx, &root, "local ");
    start(&mut cx, window, Intent::Close);
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&path).unwrap(), "local original");
    assert!(root.read_with(&cx, |workspace, _| workspace.document.is_none()
        && workspace.watcher.is_none()
        && workspace.watch_task.is_none()));
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), if cfg!(target_os = "macos") { "cmd-n" } else { "ctrl-n" });
    cx.run_until_parked();
    assert!(root.read_with(&cx, |workspace, _| workspace.document.is_some()));
    assert_eq!(text(&cx, &root), "");
}

#[test]
fn clean_external_changes_reload_and_dirty_changes_offer_reload_or_keep_local() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("external.tex");
    fs::write(&path, "original").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &path);
    let id = root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().buffer().id());
    fs::write(&path, "external\r\n").unwrap();
    start(&mut cx, window, Intent::CheckExternal);
    assert_eq!(text(&cx, &root), "external\r\n");
    assert!(!cx.has_pending_prompt());
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().buffer().id()), id);
    type_text(&mut cx, &root, "local ");
    fs::write(&path, "disk changed").unwrap();
    start(&mut cx, window, Intent::CheckExternal);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Keep Local");
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "local external\r\n");
    assert!(root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    start(&mut cx, window, Intent::CheckExternal);
    assert!(!cx.has_pending_prompt());
    fs::write(&path, "newest").unwrap();
    start(&mut cx, window, Intent::CheckExternal);
    cx.simulate_prompt_answer("Reload");
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "newest");
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    assert!(!root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().buffer().can_undo()));
}

#[test]
fn application_saves_are_ignored_and_deleted_files_can_be_recreated() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("deleted.tex");
    fs::write(&path, "original").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &path);
    type_text(&mut cx, &root, "saved ");
    start(&mut cx, window, Intent::Save);
    start(&mut cx, window, Intent::CheckExternal);
    assert!(!cx.has_pending_prompt());
    assert!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().buffer().can_undo()));
    fs::rename(&path, directory.path().join("moved.tex")).unwrap();
    start(&mut cx, window, Intent::CheckExternal);
    assert_eq!(text(&cx, &root), "saved original");
    assert!(root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    assert!(root.read_with(&cx, |workspace, _| workspace.error.as_ref().unwrap().contains("deleted or moved")));
    start(&mut cx, window, Intent::Save);
    assert_eq!(fs::read_to_string(&path).unwrap(), "saved original");
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
}

#[test]
fn external_changes_before_save_do_not_get_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conflict.tex");
    fs::write(&path, "original").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &path);
    type_text(&mut cx, &root, "local ");
    fs::write(&path, "external").unwrap();
    start(&mut cx, window, Intent::Save);
    assert_eq!(fs::read_to_string(&path).unwrap(), "external");
    assert!(root.read_with(&cx, |workspace, _| workspace.error.as_ref().unwrap().contains("changed on disk")));
    assert!(root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
}

#[test]
fn edits_during_save_remain_dirty_and_undo_returns_to_the_written_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot.tex");
    fs::write(&path, "original").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &path);
    type_text(&mut cx, &root, "first ");
    let editor = root.read_with(&cx, |workspace, _| workspace.editor.clone());
    let edited = Rc::new(Cell::new(false));
    let flag = Rc::clone(&edited);
    let weak = root.downgrade();
    let subscription = cx.update(|cx| {
        cx.observe(&editor, move |editor, cx| {
            if !flag.replace(true) {
                assert!(weak.read_with(cx, |workspace, _| workspace.busy).unwrap());
                editor.update(cx, |editor, cx| editor.update_state(|state| state.insert("later ", true), cx));
            }
        })
    });
    start(&mut cx, window, Intent::Save);
    assert!(edited.get());
    assert_eq!(fs::read_to_string(&path).unwrap(), "first original");
    assert_eq!(text(&cx, &root), "first later original");
    assert!(root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    root.update(&mut cx, |workspace, cx| {
        workspace.editor.update(cx, |editor, cx| editor.update_state(|state| state.undo(), cx))
    });
    assert_eq!(text(&cx, &root), "first original");
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    drop(subscription);
}

#[test]
fn edits_during_close_save_require_another_decision_before_closing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("closing.tex");
    fs::write(&path, "original").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &path);
    type_text(&mut cx, &root, "first ");
    start(&mut cx, window, Intent::Close);
    let editor = root.read_with(&cx, |workspace, _| workspace.editor.clone());
    let edited = Rc::new(Cell::new(false));
    let subscription = cx.update(|cx| {
        cx.observe(&editor, move |editor, cx| {
            if !edited.replace(true) {
                editor.update(cx, |editor, cx| editor.update_state(|state| state.insert("later ", true), cx));
            }
        })
    });
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&path).unwrap(), "first original");
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "first later original");
    assert!(root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    drop(subscription);
}

#[test]
fn externally_invalid_text_preserves_local_text_and_recovery_clears_the_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("encoding.tex");
    fs::write(&path, "original").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &path);
    fs::write(&path, [0xff]).unwrap();
    start(&mut cx, window, Intent::CheckExternal);
    assert_eq!(text(&cx, &root), "original");
    assert!(root.read_with(&cx, |workspace, _| workspace.error.as_ref().unwrap().contains("unsupported encoding")));
    assert!(root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    fs::write(&path, "original").unwrap();
    start(&mut cx, window, Intent::CheckExternal);
    cx.simulate_prompt_answer("Reload");
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "original");
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    assert!(root.read_with(&cx, |workspace, _| workspace.error.is_none()));
}
