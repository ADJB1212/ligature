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
fn close_and_quit_cancel_keep_dirty_documents_and_restore_focus() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "unsaved");
    for intent in [Intent::Close, Intent::Quit, Intent::CloseWindow] {
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
    start(&mut cx, window, Intent::Close);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    start(&mut cx, window, Intent::New);
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

#[test]
fn switching_tabs_preserves_selection_scroll_and_buffer_identity() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.tex");
    let second = directory.path().join("second.tex");
    fs::write(&first, format!("{}\n", "long line ".repeat(100)).repeat(100)).unwrap();
    fs::write(&second, "second").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &first);
    root.update(&mut cx, |workspace, cx| {
        workspace
            .editor
            .update(cx, |editor, cx| editor.update_state(|state| state.select(4, 10), cx))
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.dispatch_event(
            gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                position: gpui::point(gpui::px(200.0), gpui::px(300.0)),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(-100.0), gpui::px(-180.0))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            }),
            cx,
        );
        window.draw(cx).clear(cx);
    })
    .unwrap();
    let (id, scroll) = root.read_with(&cx, |workspace, cx| {
        (workspace.editor.entity_id(), workspace.editor.read(cx).scroll_offset())
    });
    assert!(scroll.y > gpui::px(0.0));
    open(&mut cx, window, &second);
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.tabs.len()), 2);
    open(&mut cx, window, &first);
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.editor.entity_id(), id);
        assert_eq!(workspace.editor.read(cx).state().selection(), 4..10);
        assert_eq!(workspace.editor.read(cx).scroll_offset(), scroll);
    });
}

#[test]
fn new_keeps_dirty_tabs_and_closing_selects_a_remaining_tab() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "first");
    let id = root.read_with(&cx, |workspace, _| workspace.editor.entity_id());
    start(&mut cx, window, Intent::New);
    assert!(!cx.has_pending_prompt());
    assert_eq!(text(&cx, &root), "");
    start(&mut cx, window, Intent::Close);
    assert_eq!(text(&cx, &root), "first");
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.editor.entity_id()), id);
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.tabs.len()), 1);
}

#[test]
fn closing_an_inactive_tab_preserves_the_active_tab_on_close_or_cancel() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "first");
    start(&mut cx, window, Intent::New);
    type_text(&mut cx, &root, "second");
    start(&mut cx, window, Intent::New);
    let id = root.read_with(&cx, |workspace, _| workspace.editor.entity_id());
    start(&mut cx, window, Intent::CloseTab(0));
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.editor.entity_id()), id);
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.tabs.len()), 3);
    start(&mut cx, window, Intent::CloseTab(0));
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.editor.entity_id()), id);
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.tabs.len()), 2);
}

#[test]
fn canceling_window_close_keeps_all_tabs_even_after_discarding_another() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "first");
    start(&mut cx, window, Intent::New);
    type_text(&mut cx, &root, "second");
    start(&mut cx, window, Intent::CloseWindow);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "second");
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(workspace.tabs[0].as_ref().unwrap().editor.read(cx).state().text(), "first");
    });
}

#[test]
fn directory_picker_loads_project_settings_and_persists_root_selection() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main file.tex");
    fs::write(&main, "\\input{missing}").unwrap();
    fs::write(directory.path().join(ligature_settings::PROJECT_FILE), r#"{"indentation":{"width":2}}"#).unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    start(&mut cx, window, Intent::OpenProject);
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories);
        assert!(!options.files);
        Some(vec![directory.path().to_owned()])
    });
    cx.run_until_parked();
    root.read_with(&cx, |workspace, _| {
        let project = workspace.project.as_ref().unwrap();
        assert_eq!(workspace.settings.indentation.width, 2);
        assert!(workspace.directories[&project.root].error.is_none());
    });
    open(&mut cx, window, &main);
    start(&mut cx, window, Intent::SelectRoot);
    let (settings, errors) = ligature_settings::Settings::load(&directory.path().join(ligature_settings::PROJECT_FILE));
    assert!(errors.is_empty());
    assert_eq!(settings.root_document, Some("main file.tex".into()));
    root.read_with(&cx, |workspace, _| {
        assert!(workspace.project.as_ref().unwrap().documents.contains(Path::new("main file.tex")));
    });
}

#[test]
fn restoration_skips_missing_files_and_projects_with_a_visible_explanation() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("restored.tex");
    let session_path = directory.path().join("session.json");
    fs::write(&main, "restored text").unwrap();
    ligature_settings::Session {
        project: Some(directory.path().join("missing project")),
        documents: vec![main.clone(), directory.path().join("missing.tex")],
        active_document: Some(main.clone()),
    }
    .save(&session_path)
    .unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    root.update(&mut cx, |workspace, _| workspace.session_path = Some(session_path.clone()));
    start(&mut cx, window, Intent::Restore);
    assert_eq!(text(&cx, &root), "restored text");
    root.read_with(&cx, |workspace, _| {
        assert!(workspace.project.is_none());
        let error = workspace.error.as_ref().unwrap();
        assert!(error.contains("missing project"));
        assert!(error.contains("missing.tex"));
    });
    start(&mut cx, window, Intent::Save);
    let session = ligature_settings::Session::load(&session_path).unwrap();
    assert_eq!(session.documents, vec![fs::canonicalize(main).unwrap()]);
}

#[test]
fn project_tree_refreshes_changes_and_retains_entries_when_a_directory_disappears() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("chapters");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("one.tex"), "one").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    start(&mut cx, window, Intent::OpenProject);
    cx.simulate_path_prompt_response(|_| Some(vec![directory.path().to_owned()]));
    cx.run_until_parked();
    let folder = fs::canonicalize(folder).unwrap();
    root.update(&mut cx, |workspace, _| {
        workspace.expanded.insert(folder.clone());
    });
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.directories[&folder].entries.len()), 1);
    fs::write(folder.join("two.tex"), "two").unwrap();
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.directories[&folder].entries.len()), 2);
    fs::rename(&folder, folder.with_file_name("moved")).unwrap();
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    root.read_with(&cx, |workspace, _| {
        assert!(workspace.directories[&folder].error.is_some());
        assert_eq!(workspace.directories[&folder].entries.len(), 2);
        assert!(workspace.project.is_some());
    });
}

#[test]
fn includes_follow_unsaved_text_in_inactive_tabs_and_keyboard_cycles_tabs() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main.tex");
    let chapter = directory.path().join("chapter.tex");
    fs::write(&main, "\\input{chapter}").unwrap();
    fs::write(&chapter, "chapter").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    start(&mut cx, window, Intent::OpenProject);
    cx.simulate_path_prompt_response(|_| Some(vec![directory.path().to_owned()]));
    cx.run_until_parked();
    open(&mut cx, window, &main);
    start(&mut cx, window, Intent::SelectRoot);
    type_text(&mut cx, &root, "\\input{missing}\n");
    open(&mut cx, window, &chapter);
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    let canonical_main = fs::canonicalize(&main).unwrap();
    root.read_with(&cx, |workspace, _| {
        let includes = &workspace.includes.files[&canonical_main];
        assert_eq!(includes.len(), 2);
        assert!(!includes[0].resolved);
        assert!(includes[1].resolved);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "ctrl-tab");
    cx.run_until_parked();
    assert_eq!(
        root.read_with(&cx, |workspace, _| workspace.document.as_ref().unwrap().path.clone()),
        Some(canonical_main)
    );
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.tabs.len()), 2);
}

#[test]
fn inactive_clean_files_reload_and_dirty_files_defer_the_external_change_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.tex");
    let second = directory.path().join("second.tex");
    fs::write(&first, "first").unwrap();
    fs::write(&second, "second").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &first);
    let id = root.read_with(&cx, |workspace, _| workspace.editor.entity_id());
    open(&mut cx, window, &second);
    root.update(&mut cx, |workspace, cx| {
        workspace.refresh_inactive(id, FileState::Present(b"external".to_vec()), cx)
    });
    assert_eq!(
        root.read_with(&cx, |workspace, cx| workspace.tabs[0]
            .as_ref()
            .unwrap()
            .editor
            .read(cx)
            .state()
            .text()
            .to_owned()),
        "external"
    );
    fs::write(&first, "external").unwrap();
    open(&mut cx, window, &first);
    type_text(&mut cx, &root, "local ");
    open(&mut cx, window, &second);
    fs::write(&first, "new external").unwrap();
    root.update(&mut cx, |workspace, cx| {
        workspace.refresh_inactive(id, FileState::Present(b"new external".to_vec()), cx)
    });
    assert!(!cx.has_pending_prompt());
    open(&mut cx, window, &first);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Keep Local");
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "local external");
}
