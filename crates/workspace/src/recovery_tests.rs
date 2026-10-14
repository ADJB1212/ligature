use super::*;
use crate::{
    lifecycle::Intent,
    tests::{open, start, type_text, workspace},
};
use gpui::{Entity, TestAppContext, WindowHandle};

fn setup(cx: &mut TestAppContext, directory: &std::path::Path, ready: bool) -> (WindowHandle<Workspace>, Entity<Workspace>, RecoveryStore) {
    let (window, root) = workspace(cx);
    let store = RecoveryStore::new(directory.join("recovery.json"));
    root.update(cx, |workspace, _| {
        workspace.session_path = Some(directory.join("session.json"));
        workspace.recovery.store = Some(store.clone());
        workspace.recovery.ready = ready;
    });
    (window, root, store)
}

fn tick(cx: &mut TestAppContext, millis: u64) {
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(millis));
    cx.run_until_parked();
}

#[test]
fn continuous_typing_persists_the_latest_text_without_waiting_for_a_pause() {
    let directory = tempfile::tempdir().unwrap();
    let mut cx = TestAppContext::single();
    let (_, root, store) = setup(&mut cx, directory.path(), true);
    type_text(&mut cx, &root, "one");
    tick(&mut cx, 250);
    assert!(store.load().unwrap().documents.is_empty());
    type_text(&mut cx, &root, "猫\r\n");
    tick(&mut cx, 250);
    assert_eq!(store.load().unwrap().documents[0].text, "one猫\r\n");
    type_text(&mut cx, &root, "three");
    tick(&mut cx, 500);
    assert_eq!(store.load().unwrap().documents[0].text, "one猫\r\nthree");
}

#[test]
fn saving_one_tab_preserves_other_unsaved_tabs_and_canceled_close_keeps_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("saved.tex");
    let mut cx = TestAppContext::single();
    let (window, root, store) = setup(&mut cx, directory.path(), true);
    type_text(&mut cx, &root, "first draft");
    start(&mut cx, window, Intent::New);
    root.update(&mut cx, |workspace, cx| {
        workspace.editor.update(cx, |editor, cx| {
            editor.update_state(
                |state| {
                    state.select_all();
                    state.insert("second draft", false);
                },
                cx,
            )
        });
    });
    start(&mut cx, window, Intent::Save);
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    assert_eq!(
        store.load().unwrap().documents,
        vec![RecoveryDocument {
            path: None,
            text: "first draft".into()
        }]
    );
    type_text(&mut cx, &root, " edited");
    tick(&mut cx, 500);
    let before = store.load().unwrap();
    start(&mut cx, window, Intent::Close);
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(store.load().unwrap(), before);
    start(&mut cx, window, Intent::Close);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert_eq!(store.load().unwrap().documents.len(), 1);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "second draft");
}

#[test]
fn recovery_restores_saved_deleted_and_untitled_documents_without_changing_disk() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.tex");
    std::fs::write(&path, "external version").unwrap();
    let path = std::fs::canonicalize(path).unwrap();
    let missing = directory.path().join("deleted.tex");
    let mut cx = TestAppContext::single();
    let (window, root, store) = setup(&mut cx, directory.path(), false);
    let recovery = Recovery {
        documents: vec![
            RecoveryDocument {
                path: Some(path.clone()),
                text: "unsaved 猫\r\n".into(),
            },
            RecoveryDocument {
                path: Some(missing.clone()),
                text: "deleted draft".into(),
            },
            RecoveryDocument {
                path: None,
                text: "untitled draft".into(),
            },
        ],
    };
    store.prepare(recovery.clone()).save().unwrap();
    start(&mut cx, window, Intent::Restore);
    assert!(cx.has_pending_prompt());
    assert_eq!(store.load().unwrap(), recovery);
    cx.simulate_prompt_answer("Recover");
    cx.run_until_parked();
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.tabs.len(), 3);
        assert_eq!(workspace.recovery_snapshot(cx), recovery);
        assert!(workspace.dirty(cx));
        assert!(workspace.document.as_ref().unwrap().path.is_none());
    });
    assert_eq!(std::fs::read_to_string(path).unwrap(), "external version");
    assert!(!missing.exists());
    assert_eq!(store.load().unwrap(), recovery);
}

#[test]
fn discarding_recovery_removes_only_recovery_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.tex");
    std::fs::write(&path, "disk").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root, store) = setup(&mut cx, directory.path(), false);
    store
        .prepare(Recovery {
            documents: vec![RecoveryDocument {
                path: Some(path.clone()),
                text: "draft".into(),
            }],
        })
        .save()
        .unwrap();
    start(&mut cx, window, Intent::Restore);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert!(store.load().unwrap().documents.is_empty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "disk");
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
}

#[test]
fn malformed_recovery_is_visible_and_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery.json");
    std::fs::write(&path, "{broken").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root, _) = setup(&mut cx, directory.path(), false);
    start(&mut cx, window, Intent::Restore);
    type_text(&mut cx, &root, "new text");
    tick(&mut cx, 500);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "{broken");
    assert!(root.read_with(&cx, |workspace, _| workspace.error.as_ref().unwrap().contains("Invalid recovery")));
}

#[test]
fn orderly_window_close_clears_recovery_and_releases_retained_views_and_watchers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.tex");
    std::fs::write(&path, "disk").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root, store) = setup(&mut cx, directory.path(), true);
    open(&mut cx, window, &path);
    let events = root.read_with(&cx, |workspace, _| workspace.watcher.as_ref().unwrap().events());
    type_text(&mut cx, &root, "draft");
    tick(&mut cx, 500);
    assert!(!store.load().unwrap().documents.is_empty());
    start(&mut cx, window, Intent::CloseWindow);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert!(events.is_closed());
    assert!(store.load().unwrap().documents.is_empty());
    root.read_with(&cx, |workspace, _| {
        assert!(workspace.tabs.is_empty());
        assert!(workspace.watcher.is_none());
        assert!(workspace.watch_task.is_none());
        assert!(workspace.project_task.is_none());
        assert!(workspace.open_text.is_empty());
    });
}

#[test]
fn edits_during_save_remain_recoverable_and_undo_to_the_save_clears_them() {
    use std::{cell::Cell, rc::Rc};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.tex");
    std::fs::write(&path, "disk").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root, store) = setup(&mut cx, directory.path(), true);
    open(&mut cx, window, &path);
    type_text(&mut cx, &root, "first ");
    cx.run_until_parked();
    let editor = root.read_with(&cx, |workspace, _| workspace.editor.clone());
    let edited = Rc::new(Cell::new(false));
    let subscription = cx.update(|cx| {
        cx.observe(&editor, move |editor, cx| {
            if !edited.replace(true) {
                editor.update(cx, |editor, cx| editor.update_state(|state| state.insert("later ", true), cx));
            }
        })
    });
    start(&mut cx, window, Intent::Save);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first disk");
    assert_eq!(store.load().unwrap().documents[0].text, "first later disk");
    drop(subscription);
    editor.update(&mut cx, |editor, cx| editor.update_state(|state| state.undo(), cx));
    tick(&mut cx, 500);
    assert!(store.load().unwrap().documents.is_empty());
}

#[test]
fn already_saved_recovery_is_cleaned_and_invalid_sessions_still_offer_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.tex");
    std::fs::write(&path, "saved").unwrap();
    std::fs::write(directory.path().join("session.json"), "{broken").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root, store) = setup(&mut cx, directory.path(), false);
    store
        .prepare(Recovery {
            documents: vec![RecoveryDocument {
                path: Some(path),
                text: "saved".into(),
            }],
        })
        .save()
        .unwrap();
    start(&mut cx, window, Intent::Restore);
    cx.simulate_prompt_answer("Recover");
    cx.run_until_parked();
    assert!(store.load().unwrap().documents.is_empty());
    root.read_with(&cx, |workspace, cx| {
        assert!(!workspace.dirty(cx));
        assert!(workspace.error.as_ref().unwrap().contains("Invalid session"));
        assert_eq!(workspace.tabs.len(), 1);
    });
}
