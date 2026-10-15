use std::{cell::Cell, fs, path::Path, rc::Rc};

use gpui::{AppContext, Entity, TestAppContext, WindowHandle};

use super::*;

pub(super) fn workspace(cx: &mut TestAppContext) -> (WindowHandle<Workspace>, Entity<Workspace>) {
    let (window, root) = workspace_with_template(cx);
    root.update(cx, |workspace, cx| {
        workspace
            .editor
            .update(cx, |editor, cx| editor.update_state(|state| state.reload(String::new()), cx));
    });
    (window, root)
}

fn workspace_with_template(cx: &mut TestAppContext) -> (WindowHandle<Workspace>, Entity<Workspace>) {
    cx.update(init);
    let window = cx.add_window(Workspace::new);
    let root = window.root(cx).unwrap();
    root.update(cx, |workspace, cx| {
        workspace.configuration = ligature_settings::Configuration::default();
        workspace.configuration.defaults.language_server.enabled = false;
        workspace.settings = workspace.configuration.settings_for(None);
        workspace.configuration_path = None;
        workspace.configuration_errors.clear();
        workspace.settings_errors.clear();
        workspace.apply_settings(cx);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    (window, root)
}

#[cfg(target_os = "macos")]
fn load_export_preview(cx: &mut TestAppContext, root: &Entity<Workspace>, path: &Path) {
    root.update(cx, |workspace, cx| {
        workspace.preview.update(cx, |preview, cx| {
            preview.load(path.to_owned(), None, path.parent().unwrap().to_owned(), cx)
        });
    });
    cx.run_until_parked();
    assert!(root.read_with(cx, |workspace, cx| workspace.preview.read(cx).pdf_snapshot().is_some()));
}

#[test]
fn export_shortcut_is_unavailable_without_a_pdf() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    cx.simulate_keystrokes(window.into(), "cmd-shift-e");
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path());
    assert!(!root.read_with(&cx, |workspace, _| workspace.busy));
}

#[cfg(target_os = "macos")]
#[test]
fn pdf_export_keeps_the_selected_snapshot_and_untitled_buffer_state() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("Untitled.pdf");
    let destination = directory.path().join("export.pdf");
    let bytes = include_bytes!("../../pdf/tests/fixtures/pages.pdf");
    fs::write(&source, bytes).unwrap();
    fs::write(&destination, "previous export").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "unsaved source");
    let identity = root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().buffer().id());
    load_export_preview(&mut cx, &root, &source);
    root.update(&mut cx, |workspace, cx| {
        workspace.preview.update(cx, |preview, cx| preview.set_stale(true, cx));
    });
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx)).unwrap();
    cx.simulate_keystrokes(window.into(), "cmd-shift-e");
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    let mut replacement = bytes.to_vec();
    replacement.extend_from_slice(b"\n% replacement build\n");
    fs::write(&source, &replacement).unwrap();
    load_export_preview(&mut cx, &root, &source);
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    assert_eq!(fs::read(&destination).unwrap(), bytes);
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.editor.read(cx).state().buffer().id(), identity);
        assert_eq!(workspace.editor.read(cx).state().text(), "unsaved source");
        assert!(workspace.dirty(cx));
        assert!(workspace.document.as_ref().unwrap().path.is_none());
        assert!(workspace.project.is_none());
        assert!(workspace.error.is_none());
        assert!(!workspace.busy);
        assert_eq!(workspace.preview.read(cx).pdf_snapshot().unwrap().1.bytes().as_ref(), replacement);
    });
}

#[cfg(target_os = "macos")]
#[test]
fn canceled_pdf_export_leaves_existing_files_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("main.pdf");
    fs::write(&source, include_bytes!("../../pdf/tests/fixtures/pages.pdf")).unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    load_export_preview(&mut cx, &root, &source);
    start(&mut cx, window, Intent::ExportPdf);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    root.read_with(&cx, |workspace, _| {
        assert!(workspace.error.is_none());
        assert!(!workspace.busy);
    });
}

#[cfg(target_os = "macos")]
#[test]
fn failed_pdf_export_reports_the_destination_and_allows_retry() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("main.pdf");
    let destination = directory.path().join("readonly.pdf");
    fs::write(&source, include_bytes!("../../pdf/tests/fixtures/pages.pdf")).unwrap();
    fs::write(&destination, "original").unwrap();
    let mut permissions = fs::metadata(&destination).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&destination, permissions).unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    load_export_preview(&mut cx, &root, &source);
    start(&mut cx, window, Intent::ExportPdf);
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&destination).unwrap(), "original");
    root.read_with(&cx, |workspace, _| {
        let error = workspace.error.as_ref().unwrap();
        assert!(error.contains("readonly.pdf"));
        assert!(error.contains("read-only"));
        assert!(!workspace.busy);
    });
    let retry = directory.path().join("retry.pdf");
    start(&mut cx, window, Intent::ExportPdf);
    cx.simulate_new_path_selection(|_| Some(retry.clone()));
    cx.run_until_parked();
    assert_eq!(fs::read(&retry).unwrap(), include_bytes!("../../pdf/tests/fixtures/pages.pdf"));
    assert!(root.read_with(&cx, |workspace, _| workspace.error.is_none()));
}

#[test]
fn outline_navigation_preserves_untitled_identity_and_dirty_state() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "\\section{First}\n\\subsection{Child}\n\\label{key}\\ref{key}");
    cx.run_until_parked();
    let (identity, location) = root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.semantic_ui.index.outline[1].parent, Some(0));
        (
            workspace.editor.read(cx).state().buffer().id(),
            workspace.semantic_ui.index.outline[1].location.clone(),
        )
    });
    cx.update_window(window.into(), |_, window, cx| {
        root.update(cx, |workspace, cx| workspace.navigate_semantic(location, window, cx));
    })
    .unwrap();
    root.read_with(&cx, |workspace, cx| {
        assert!(workspace.document.as_ref().unwrap().path.is_none());
        assert_eq!(workspace.editor.read(cx).state().buffer().id(), identity);
        assert!(workspace.dirty(cx));
        assert_eq!(workspace.editor.read(cx).state().selected_text(), "\\subsection{Child}");
    });
}

#[test]
fn tab_changes_dismiss_completion_and_reject_previous_tab_responses() {
    let mut cx = TestAppContext::single();
    cx.update(ligature_editor::init);
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "\\sec");
    let target = root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).completion_target().unwrap());
    cx.update_window(window.into(), |_, window, cx| {
        root.update(cx, |workspace, cx| {
            workspace.replace_document(None, window, cx);
            assert!(
                !workspace
                    .editor
                    .update(cx, |editor, cx| editor.publish_completion(target, Vec::new(), cx))
            );
            workspace.switch_tab(0, window, cx);
            assert!(workspace.editor.read(cx).completion_target().is_none());
        });
    })
    .unwrap();
}

#[test]
fn definition_shortcut_opens_labels_in_included_documents() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main.tex");
    let child = directory.path().join("child.tex");
    fs::write(&main, "\\input{child}\n\\ref{target}").unwrap();
    fs::write(&child, "\\section{Child}\n\\label{target}").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &main);
    root.update(&mut cx, |workspace, cx| {
        workspace.editor.update(cx, |editor, cx| {
            editor.update_state(
                |state| {
                    let offset = state.text().find("ref{target}").unwrap() + 4;
                    state.select(offset, offset);
                },
                cx,
            )
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "f12");
    cx.run_until_parked();
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(
            workspace.document.as_ref().unwrap().path.as_ref(),
            Some(&fs::canonicalize(child).unwrap())
        );
        assert_eq!(workspace.editor.read(cx).state().selected_text(), "target");
        assert_eq!(workspace.tabs.len(), 2);
        assert!(workspace.error.is_none(), "{:?}", workspace.error);
    });
}

#[test]
fn citation_navigation_opens_and_highlights_the_bibliography_key() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main.tex");
    let bibliography = directory.path().join("references.bib");
    fs::write(&main, "\\bibliography{references}\n\\cite{book}").unwrap();
    fs::write(&bibliography, "@book{book, title={Title}, year=2026}").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &main);
    root.update(&mut cx, |workspace, cx| {
        workspace.editor.update(cx, |editor, cx| {
            editor.update_state(
                |state| {
                    let offset = state.text().find("cite{book}").unwrap() + 5;
                    state.select(offset, offset);
                },
                cx,
            )
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "f12");
    cx.run_until_parked();
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(
            workspace.document.as_ref().unwrap().path.as_ref(),
            Some(&fs::canonicalize(bibliography).unwrap())
        );
        assert_eq!(workspace.editor.read(cx).state().selected_text(), "book");
        assert_eq!(
            workspace.editor.read(cx).syntax_snapshot().unwrap().language,
            ligature_syntax::Language::Bibliography
        );
    });
}

#[test]
fn duplicate_reference_navigation_requires_selecting_a_definition() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "\\label{key}\\label{key}\\ref{key}");
    root.update(&mut cx, |workspace, cx| {
        workspace.editor.update(cx, |editor, cx| {
            editor.update_state(
                |state| {
                    let offset = state.text().rfind("key").unwrap();
                    state.select(offset, offset);
                },
                cx,
            )
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "f12");
    root.read_with(&cx, |workspace, cx| {
        assert!(workspace.semantic_ui.visible);
        assert!(workspace.error.as_ref().unwrap().contains("multiple definitions"));
        assert!(workspace.editor.read(cx).state().selection().is_empty());
    });
}

#[test]
fn stale_outline_locations_do_not_select_changed_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("changed.tex");
    fs::write(&path, "\\section{Old}").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    let intent = Intent::OpenSemantic {
        location: ligature_project::Location {
            path: path.clone(),
            range: 0..13,
        },
        original: "\\section{Old}".into(),
    };
    fs::write(path, "Different source").unwrap();
    start(&mut cx, window, intent);
    root.read_with(&cx, |workspace, cx| {
        assert!(workspace.error.as_ref().unwrap().contains("source location changed"));
        assert!(workspace.editor.read(cx).state().selection().is_empty());
    });
}

#[test]
fn unopened_file_changes_refresh_references_and_bibliography_entries() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main.tex");
    let child = directory.path().join("child.tex");
    let bibliography = directory.path().join("references.bib");
    fs::write(&main, "\\input{child}\\ref{key}\\bibliography{references}\\cite{book}").unwrap();
    fs::write(&child, "\\label{key}").unwrap();
    fs::write(&bibliography, "@book{book,title={Old}}").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open(&mut cx, window, &main);
    fs::write(child, "\\section{Changed}").unwrap();
    fs::write(bibliography, "@book{book,title={New}}").unwrap();
    cx.executor().advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    root.read_with(&cx, |workspace, _| {
        let index = &workspace.semantic_ui.index;
        assert!(index.definitions("key").is_empty());
        assert_eq!(index.unresolved_references[0].0, "key");
        assert_eq!(index.bibliography.resolve("book")[0].entry.title(), Some("New"));
    });
}

pub(super) fn start(cx: &mut TestAppContext, window: WindowHandle<Workspace>, intent: Intent) {
    cx.update_window(window.into(), |root, window, cx| {
        root.downcast::<Workspace>()
            .unwrap()
            .update(cx, |workspace, cx| workspace.start(intent, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn type_text(cx: &mut TestAppContext, root: &Entity<Workspace>, text: &str) {
    root.update(cx, |workspace, cx| {
        workspace
            .editor
            .update(cx, |editor, cx| editor.update_state(|state| state.insert(text, true), cx))
    });
}

fn text(cx: &TestAppContext, root: &Entity<Workspace>) -> String {
    root.read_with(cx, |workspace, cx| workspace.editor.read(cx).state().text().to_owned())
}

pub(super) fn open(cx: &mut TestAppContext, window: WindowHandle<Workspace>, path: &Path) {
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
fn compact_chrome_keeps_editor_and_preview_headers_aligned_at_minimum_window_size() {
    let mut app = TestAppContext::single();
    let (window, _) = workspace(&mut app);
    let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
    cx.simulate_resize(gpui::size(px(640.0), px(480.0)));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let toolbar = cx.debug_bounds("workspace-toolbar").unwrap();
    let tabs = cx.debug_bounds("tabs").unwrap();
    let preview = cx.debug_bounds("preview-toolbar").unwrap();
    assert_eq!(toolbar.size.height, px(44.0));
    assert_eq!(tabs.size.height, px(40.0));
    assert_eq!(preview.top(), tabs.top());
    assert_eq!(preview.bottom(), tabs.bottom());
    assert!(preview.right() <= px(640.0));
}

#[test]
fn files_and_outline_share_a_sidebar_without_clipping_either_working_pane() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("a-very-long-document-name-that-should-fit-inside-a-tab.tex");
    fs::write(&path, "\\section{A heading that is wider than the sidebar}").unwrap();
    let mut app = TestAppContext::single();
    let (window, root) = workspace(&mut app);
    open_project(&mut app, window, directory.path());
    open(&mut app, window, &path);
    root.update(&mut app, |workspace, cx| {
        workspace.semantic_ui.visible = true;
        cx.notify();
    });
    let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
    cx.simulate_resize(gpui::size(px(640.0), px(480.0)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let sidebar = cx.debug_bounds("workspace-sidebar").unwrap();
    let files = cx.debug_bounds("project-tree").unwrap();
    let outline = cx.debug_bounds("document-outline").unwrap();
    let source = cx.debug_bounds("leading-pane").unwrap();
    let preview = cx.debug_bounds("trailing-pane").unwrap();
    let tab = cx.debug_bounds("document-tab-0").unwrap();
    let status = cx.debug_bounds("workspace-status").unwrap();
    assert_eq!(sidebar.size.width, px(200.0));
    assert_eq!(files.left(), outline.left());
    assert_eq!(files.size.width, outline.size.width);
    assert!(files.bottom() <= outline.top());
    assert!(outline.bottom() <= sidebar.bottom());
    assert!(source.size.width >= px(200.0));
    assert!(preview.size.width >= px(200.0));
    assert!(source.right() <= preview.left());
    assert!(preview.right() <= px(640.0));
    assert!(tab.size.width <= px(200.0));
    assert!(tab.right() <= source.right());
    assert_eq!(status.size.height, px(28.0));
    assert_eq!(status.bottom(), px(480.0));
}

#[test]
fn sidebar_resizing_preserves_editor_space_and_width_when_hidden() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = TestAppContext::single();
    let (window, root) = workspace(&mut app);
    open_project(&mut app, window, directory.path());
    root.update(&mut app, |workspace, cx| {
        workspace.semantic_ui.visible = true;
        cx.notify();
    });
    let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
    cx.simulate_resize(gpui::size(px(1200.0), px(800.0)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for (destination, expected) in [(303.0, 300.0), (1.0, 120.0), (1190.0, 788.0)] {
        let boundary = cx.debug_bounds("sidebar-boundary").unwrap();
        let destination = gpui::point(px(destination), boundary.center().y);
        cx.simulate_mouse_down(boundary.center(), gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(destination, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_move(destination, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(destination, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let sidebar = cx.debug_bounds("workspace-sidebar").unwrap();
        let source = cx.debug_bounds("leading-pane").unwrap();
        let preview = cx.debug_bounds("trailing-pane").unwrap();
        assert_eq!(sidebar.size.width, px(expected));
        assert!(source.size.width >= px(200.0));
        assert!(preview.size.width >= px(200.0));
        assert!(preview.right() <= px(1200.0));
        assert_eq!(cx.debug_bounds("project-tree").unwrap().size.width, sidebar.size.width);
        assert_eq!(cx.debug_bounds("document-outline").unwrap().size.width, sidebar.size.width);
    }
    cx.simulate_resize(gpui::size(px(640.0), px(480.0)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(cx.debug_bounds("workspace-sidebar").unwrap().size.width, px(228.0));
    assert!(cx.debug_bounds("trailing-pane").unwrap().right() <= px(640.0));
    root.update(&mut cx, |workspace, cx| {
        workspace.project_visible = false;
        workspace.semantic_ui.visible = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("sidebar-boundary").is_none());
    root.update(&mut cx, |workspace, cx| {
        workspace.project_visible = true;
        cx.notify();
    });
    cx.simulate_resize(gpui::size(px(1200.0), px(800.0)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(cx.debug_bounds("workspace-sidebar").unwrap().size.width, px(788.0));
}

#[test]
fn sidebar_file_rows_open_documents_when_clicked_at_the_right_edge() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.tex");
    fs::write(&path, "project source").unwrap();
    let mut app = TestAppContext::single();
    let (window, root) = workspace(&mut app);
    open_project(&mut app, window, directory.path());
    let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let entry = cx.debug_bounds("project-entry-0").unwrap();
    cx.simulate_click(gpui::point(entry.right() - px(8.0), entry.center().y), Default::default());
    cx.run_until_parked();
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(
            workspace.document.as_ref().unwrap().path.as_ref().unwrap(),
            &fs::canonicalize(path).unwrap()
        );
        assert_eq!(workspace.editor.read(cx).state().text(), "project source");
    });
}

#[test]
fn build_status_control_toggles_output_and_preserves_editor_focus() {
    let mut app = TestAppContext::single();
    let (window, root) = workspace(&mut app);
    type_text(&mut app, &root, "unsaved source");
    let identity = root.read_with(&app, |workspace, cx| workspace.editor.read(cx).state().buffer().id());
    let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
    for visible in [true, false] {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let control = cx.debug_bounds("build-status-control").unwrap();
        cx.simulate_click(control.center(), Default::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let workspace = root.read(cx);
            assert_eq!(workspace.build_ui.visible, visible);
            assert!(workspace.editor.focus_handle(cx).is_focused(window));
            assert_eq!(workspace.editor.read(cx).state().buffer().id(), identity);
            assert_eq!(workspace.editor.read(cx).state().text(), "unsaved source");
            assert!(workspace.dirty(cx));
        });
    }
}

#[test]
fn empty_workspace_new_action_creates_and_focuses_a_document() {
    let mut app = TestAppContext::single();
    let (window, root) = workspace(&mut app);
    start(&mut app, window, Intent::Close);
    assert!(root.read_with(&app, |workspace, _| workspace.document.is_none()));
    let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let control = cx.debug_bounds("empty-new-action").unwrap();
    cx.simulate_click(control.center(), Default::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let workspace = root.read(cx);
        assert!(workspace.document.as_ref().unwrap().path.is_none());
        assert_eq!(workspace.editor.read(cx).state().text(), NEW_DOCUMENT);
        assert!(workspace.editor.focus_handle(cx).is_focused(window));
        assert!(!workspace.dirty(cx));
    });
}

#[test]
fn document_status_follows_saving_and_subsequent_edits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.tex");
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.document_status(cx)), "Unsaved document");
    type_text(&mut cx, &root, "source");
    start(&mut cx, window, Intent::Save);
    cx.simulate_new_path_selection(|_| Some(path));
    cx.run_until_parked();
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.document_status(cx)), "Saved");
    type_text(&mut cx, &root, " edit");
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.document_status(cx)), "Unsaved changes");
    root.update(&mut cx, |workspace, cx| {
        workspace.external_pending = true;
        assert_eq!(workspace.document_status(cx), "Changed on disk");
    });
}

#[test]
fn caret_status_counts_unicode_characters_and_tracks_uncommitted_composition() {
    let mut cx = TestAppContext::single();
    let (_, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "é😀\r\n漢字");
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.caret_status(cx)), "Ln 2, Col 3");
    root.update(&mut cx, |workspace, cx| {
        workspace
            .editor
            .update(cx, |editor, cx| editor.update_state(|state| state.select(2, 2), cx));
        assert_eq!(workspace.caret_status(cx), "Ln 1, Col 2");
        workspace
            .editor
            .update(cx, |editor, cx| editor.update_state(|state| state.mark(None, "あ\n文", None), cx));
        assert_eq!(workspace.caret_status(cx), "Ln 2, Col 2");
        assert_eq!(workspace.editor.read(cx).state().buffer().text(), "é😀\r\n漢字");
        workspace
            .editor
            .update(cx, |editor, cx| editor.update_state(|state| state.cancel_composition(), cx));
        assert_eq!(workspace.caret_status(cx), "Ln 1, Col 2");
    });
}

#[test]
fn panel_menu_actions_work_from_editor_focus_and_preserve_the_document() {
    let directory = tempfile::tempdir().unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open_project(&mut cx, window, directory.path());
    type_text(&mut cx, &root, "\\section{Draft}");
    let identity = root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().buffer().id());
    for visible in [false, true] {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            window.dispatch_action(Box::new(ToggleProject), cx);
        })
        .unwrap();
        cx.run_until_parked();
        root.read_with(&cx, |workspace, cx| {
            assert_eq!(workspace.project_visible, visible);
            assert_eq!(workspace.editor.read(cx).state().buffer().id(), identity);
            assert!(workspace.dirty(cx));
        });
    }
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_action(Box::new(ToggleOutline), cx);
        window.dispatch_action(Box::new(ToggleLanguageServer), cx);
        window.dispatch_action(Box::new(ToggleTexResources), cx);
    })
    .unwrap();
    cx.run_until_parked();
    root.read_with(&cx, |workspace, _| {
        assert!(workspace.semantic_ui.visible);
        assert!(workspace.language_ui.visible);
        assert!(workspace.bundle_ui.visible);
    });
    cx.update_window(window.into(), |_, window, cx| {
        assert!(root.read(cx).editor.focus_handle(cx).is_focused(window));
        assert!(window.is_action_available(&ZoomIn, cx));
        assert!(window.is_action_available(&ZoomOut, cx));
        assert!(window.is_action_available(&FitWidth, cx));
        assert!(window.is_action_available(&FitPage, cx));
    })
    .unwrap();
}

#[test]
fn showing_and_resizing_build_output_preserves_the_pdf_preview_bounds() {
    let mut app = TestAppContext::single();
    let (window, _) = workspace(&mut app);
    let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
    cx.simulate_resize(gpui::size(px(1200.0), px(800.0)));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let preview = cx.debug_bounds("trailing-pane").unwrap();
    let shortcut = if cfg!(target_os = "macos") { "cmd-j" } else { "ctrl-j" };
    cx.simulate_keystrokes(shortcut);
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert_eq!(cx.debug_bounds("trailing-pane").unwrap(), preview);
    let panel = cx.debug_bounds("bottom-pane-content").unwrap();
    let editor = cx.debug_bounds("leading-pane").unwrap();
    assert_eq!(panel.left(), editor.left());
    assert_eq!(panel.right(), editor.right());
    assert_eq!(panel.bottom(), editor.bottom());
    let start = cx.debug_bounds("bottom-pane-boundary").unwrap().center();
    let target = gpui::point(start.x, start.y - px(100.0));
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, Default::default());
    cx.simulate_mouse_move(gpui::point(start.x, start.y - px(10.0)), gpui::MouseButton::Left, Default::default());
    cx.simulate_mouse_move(target, gpui::MouseButton::Left, Default::default());
    cx.simulate_mouse_up(target, gpui::MouseButton::Left, Default::default());
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("bottom-pane-content").unwrap().size.height > panel.size.height);
    assert_eq!(cx.debug_bounds("trailing-pane").unwrap(), preview);
    cx.simulate_keystrokes(shortcut);
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert_eq!(cx.debug_bounds("trailing-pane").unwrap(), preview);
    assert!(cx.debug_bounds("bottom-pane-content").is_none());
}

#[test]
fn build_output_shortcut_preserves_height_and_restores_focus_when_hidden() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    let shortcut = if cfg!(target_os = "macos") { "cmd-j" } else { "ctrl-j" };
    cx.simulate_keystrokes(window.into(), shortcut);
    assert!(root.read_with(&cx, |workspace, _| workspace.build_ui.visible));
    cx.update_window(window.into(), |_, window, cx| {
        root.update(cx, |workspace, cx| {
            workspace.build_output_height = px(180.0);
            window.focus(&workspace.build_log.focus_handle(cx), cx);
        });
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), shortcut);
    assert!(!root.read_with(&cx, |workspace, _| workspace.build_ui.visible));
    cx.update_window(window.into(), |_, window, cx| {
        assert!(root.read(cx).editor.focus_handle(cx).is_focused(window));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), shortcut);
    root.read_with(&cx, |workspace, _| {
        assert!(workspace.build_ui.visible);
        assert_eq!(workspace.build_output_height, px(180.0));
    });
}

#[test]
fn startup_and_new_documents_have_a_clean_latex_template() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace_with_template(&mut cx);
    assert_eq!(
        text(&cx, &root),
        "\\documentclass{article}\n\n\\begin{document}\n\\null\n\n\\end{document}\n"
    );
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    type_text(&mut cx, &root, "edited ");
    start(&mut cx, window, Intent::New);
    assert_eq!(text(&cx, &root), NEW_DOCUMENT);
    assert!(!root.read_with(&cx, |workspace, cx| workspace.dirty(cx)));
    assert!(root.read_with(&cx, |workspace, _| workspace.document.as_ref().unwrap().path.is_none()));
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.tabs.len()), 2);
}

#[test]
fn open_reuses_the_buffer_and_preserves_unsaved_edits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("example.tex");
    fs::write(&path, "loaded\r\n").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace_with_template(&mut cx);
    open(&mut cx, window, &path);
    assert_eq!(text(&cx, &root), "loaded\r\n");
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.tabs.len()), 1);
    let id = root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().buffer().id());
    type_text(&mut cx, &root, "local ");
    open(&mut cx, window, &directory.path().join("./example.tex"));
    assert_eq!(text(&cx, &root), "local loaded\r\n");
    assert!(!cx.has_pending_prompt());
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().buffer().id()), id);
}

#[test]
fn diagnostic_navigation_opens_included_files_and_reports_missing_locations() {
    use ligature_diagnostics::{BuildId, Diagnostic, Provider, Severity, SourceRange};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("included α.tex");
    fs::write(&path, "first\nsecond\nthird").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    let mut diagnostic = Diagnostic {
        severity: Severity::Error,
        provider: Provider::Build(BuildId(1)),
        path: Some(path.clone()),
        range: Some(SourceRange::Lines { start: 2, end: 2 }),
        message: "Undefined control sequence".into(),
    };
    start(&mut cx, window, Intent::OpenDiagnostic(diagnostic.clone()));
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.document.as_ref().unwrap().path, Some(fs::canonicalize(&path).unwrap()));
        assert_eq!(workspace.editor.read(cx).state().selected_text(), "second");
    });
    diagnostic.range = Some(SourceRange::Lines { start: 99, end: 99 });
    start(&mut cx, window, Intent::OpenDiagnostic(diagnostic.clone()));
    assert!(root.read_with(&cx, |workspace, _| workspace.error.as_ref().unwrap().contains("no longer exists")));
    diagnostic.path = Some(directory.path().join("missing.tex"));
    start(&mut cx, window, Intent::OpenDiagnostic(diagnostic.clone()));
    assert!(root.read_with(&cx, |workspace, _| workspace.error.is_some()));
    assert_eq!(text(&cx, &root), "first\nsecond\nthird");
    diagnostic.path = Some(path);
    diagnostic.range = None;
    start(&mut cx, window, Intent::OpenDiagnostic(diagnostic));
    assert!(root.read_with(&cx, |workspace, _| workspace.error.is_none()));
}

#[test]
fn inverse_navigation_opens_an_included_file_and_reveals_its_line() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chapter with spaces.tex");
    fs::write(&path, "first\nsecond\nthird\n").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    start(
        &mut cx,
        window,
        Intent::OpenSource(ligature_synctex::SourcePosition {
            path: path.clone(),
            line: 2,
            column: 0,
        }),
    );
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.document.as_ref().unwrap().path, Some(fs::canonicalize(&path).unwrap()));
        assert_eq!(workspace.editor.read(cx).state().selected_text(), "second");
    });
    cx.update_window(window.into(), |_, window, cx| {
        assert!(root.read(cx).editor.focus_handle(cx).is_focused(window));
    })
    .unwrap();
    start(
        &mut cx,
        window,
        Intent::OpenSource(ligature_synctex::SourcePosition { path, line: 20, column: 0 }),
    );
    assert!(root.read_with(&cx, |workspace, _| workspace.error.as_ref().unwrap().contains("no longer exists")));
    start(
        &mut cx,
        window,
        Intent::OpenSource(ligature_synctex::SourcePosition {
            path: directory.path().join("missing.tex"),
            line: 1,
            column: 0,
        }),
    );
    assert!(root.read_with(&cx, |workspace, _| workspace.error.is_some()));
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
    assert_eq!(text(&cx, &root), NEW_DOCUMENT);
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
    assert_eq!(text(&cx, &root), NEW_DOCUMENT);
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
    cx.run_until_parked();
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
    assert_eq!(text(&cx, &root), NEW_DOCUMENT);
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
    assert_eq!(text(&cx, &root), format!("second{NEW_DOCUMENT}"));
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(workspace.tabs[0].as_ref().unwrap().editor.read(cx).state().text(), "first");
    });
}

#[test]
fn window_close_preserves_edits_to_a_tab_that_already_passed_confirmation() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "first");
    let first = root.read_with(&cx, |workspace, _| workspace.editor.clone());
    start(&mut cx, window, Intent::New);
    type_text(&mut cx, &root, "second");
    start(&mut cx, window, Intent::CloseWindow);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    first.update(&mut cx, |editor, cx| editor.update_state(|state| state.insert(" later", true), cx));
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(first.read(cx).state().text(), "first later");
        assert!(workspace.error.as_ref().unwrap().contains("changed while closing"));
        assert!(!workspace.busy);
    });
    assert!(cx.update_window(window.into(), |_, _, _| ()).is_ok());
}

#[test]
fn directory_picker_uses_central_settings_and_ignores_project_local_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main file.tex");
    fs::write(&main, "\\documentclass{article}\\input{missing}").unwrap();
    fs::write(directory.path().join(".ligature.json"), r#"{"indentation":{"width":8}}"#).unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    let config_path = directory.path().join("config/ligature/settings.json");
    let project_path = fs::canonicalize(directory.path()).unwrap();
    root.update(&mut cx, |workspace, _| {
        let mut settings = workspace.configuration.defaults.clone();
        settings.indentation.width = 2;
        workspace.configuration.projects.insert(project_path, settings);
        workspace.configuration.save(&config_path).unwrap();
        workspace.configuration_path = Some(config_path.clone());
    });
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
        assert_eq!(project.root_document.as_deref(), Some(Path::new("main file.tex")));
        assert!(!workspace.includes.errors.is_empty());
    });
    open(&mut cx, window, &main);
    let (configuration, errors) = ligature_settings::Configuration::load(&config_path);
    assert!(errors.is_empty());
    assert!(configuration.projects.values().all(|settings| settings.root_document.is_none()));
    assert_eq!(
        fs::read_to_string(directory.path().join(".ligature.json")).unwrap(),
        r#"{"indentation":{"width":8}}"#
    );
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
    fs::write(&main, "\\documentclass{article}\\input{chapter}").unwrap();
    fs::write(&chapter, "chapter").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    start(&mut cx, window, Intent::OpenProject);
    cx.simulate_path_prompt_response(|_| Some(vec![directory.path().to_owned()]));
    cx.run_until_parked();
    open(&mut cx, window, &main);
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

#[test]
fn pending_watcher_setup_attaches_to_its_tab_after_switching_documents() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.tex");
    let second = directory.path().join("second.tex");
    fs::write(&first, "first").unwrap();
    fs::write(&second, "second").unwrap();
    let first = ligature_fs::read(&first).unwrap();
    let second = ligature_fs::read(&second).unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    cx.update_window(window.into(), |_, window, cx| {
        root.update(cx, |workspace, cx| {
            workspace.replace_document(Some(first), window, cx);
            assert!(workspace.watcher.is_none());
            workspace.replace_document(Some(second), window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    root.read_with(&cx, |workspace, _| {
        assert_eq!(workspace.tabs.len(), 2);
        assert!(workspace.watcher.is_some());
        let first = workspace.tabs[0].as_ref().unwrap();
        assert!(first.watcher.is_some());
        assert!(first.watch_task.is_some());
    });
}

#[test]
fn closing_a_document_cancels_pending_watcher_setup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("closed.tex");
    fs::write(&path, "closed").unwrap();
    let loaded = ligature_fs::read(&path).unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    cx.update_window(window.into(), |_, window, cx| {
        root.update(cx, |workspace, cx| {
            workspace.replace_document(Some(loaded), window, cx);
            workspace.close_document(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    root.read_with(&cx, |workspace, _| {
        assert!(workspace.document.is_none());
        assert!(workspace.watcher.is_none());
        assert!(workspace.watch_task.is_none());
    });
}

fn show_search(cx: &mut TestAppContext, window: WindowHandle<Workspace>, project: bool) {
    cx.update_window(window.into(), |root, window, cx| {
        root.downcast::<Workspace>()
            .unwrap()
            .update(cx, |workspace, cx| workspace.show_search(project, false, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
}

fn search_query(cx: &mut TestAppContext, root: &Entity<Workspace>, query: &str) {
    root.update(cx, |workspace, cx| {
        workspace.search.input.update(cx, |input, cx| input.set_text(query, cx))
    });
    cx.run_until_parked();
}

fn finish_project_search(cx: &mut TestAppContext) {
    cx.executor().advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
}

fn open_project(cx: &mut TestAppContext, window: WindowHandle<Workspace>, path: &Path) {
    start(cx, window, Intent::OpenProject);
    cx.simulate_path_prompt_response(|_| Some(vec![path.to_owned()]));
    cx.run_until_parked();
}

#[test]
fn document_search_highlights_without_changing_selection_and_tracks_edits() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "cat Cat concatenate");
    let selection = root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().selection());
    show_search(&mut cx, window, false);
    search_query(&mut cx, &root, "cat");
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.editor.read(cx).search_matches().len(), 3);
        assert_eq!(workspace.editor.read(cx).state().selection(), selection);
    });
    root.update(&mut cx, |workspace, cx| {
        workspace.search.query.options.whole_word = true;
        workspace.search_options_changed(cx);
    });
    cx.run_until_parked();
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).search_matches().len()), 2);
    type_text(&mut cx, &root, " cat");
    cx.run_until_parked();
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).search_matches().len()), 3);
    cx.update_window(window.into(), |root, window, cx| {
        root.downcast::<Workspace>()
            .unwrap()
            .update(cx, |workspace, cx| workspace.navigate_search(false, window, cx));
        window.draw(cx).clear(cx);
    })
    .unwrap();
    assert_eq!(
        root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).search_status()),
        (Some(0), 3)
    );
    search_query(&mut cx, &root, "");
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).search_status()), (None, 0));
}

#[test]
fn find_shortcuts_focus_the_query_and_enter_navigates_without_editing_the_document() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "cat cat");
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), if cfg!(target_os = "macos") { "cmd-f" } else { "ctrl-f" });
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        assert!(root.read(cx).search.input.focus_handle(cx).is_focused(window));
    })
    .unwrap();
    cx.simulate_input(window.into(), "cat");
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    assert_eq!(text(&cx, &root), "cat cat");
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().selection()), 0..3);
    cx.simulate_keystrokes(window.into(), "shift-enter");
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().selection()), 4..7);
    cx.simulate_keystrokes(window.into(), "escape");
    assert!(!root.read_with(&cx, |workspace, _| workspace.search.visible));
    assert!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).search_matches().is_empty()));
}

#[test]
fn replace_all_is_one_undo_transaction_and_does_not_search_inserted_text() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "cat cat");
    show_search(&mut cx, window, false);
    search_query(&mut cx, &root, "cat");
    root.update(&mut cx, |workspace, cx| {
        workspace.search.replacement.update(cx, |input, cx| input.set_text("catcat", cx))
    });
    cx.run_until_parked();
    cx.update_window(window.into(), |root, window, cx| {
        root.downcast::<Workspace>()
            .unwrap()
            .update(cx, |workspace, cx| workspace.replace_document_matches(true, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "catcat catcat");
    assert_eq!(root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).search_matches().len()), 4);
    root.update(&mut cx, |workspace, cx| {
        workspace.editor.update(cx, |editor, cx| editor.update_state(|state| state.undo(), cx))
    });
    assert_eq!(text(&cx, &root), "cat cat");
}

#[test]
fn search_navigation_selects_partial_grapheme_matches_and_wraps() {
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    type_text(&mut cx, &root, "e\u{301} e");
    show_search(&mut cx, window, false);
    search_query(&mut cx, &root, "e");
    for expected in [0..1, 4..5, 0..1] {
        cx.update_window(window.into(), |root, window, cx| {
            root.downcast::<Workspace>()
                .unwrap()
                .update(cx, |workspace, cx| workspace.navigate_search(false, window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            root.read_with(&cx, |workspace, cx| workspace.editor.read(cx).state().selection()),
            expected
        );
    }
}

#[test]
fn changing_project_queries_cancels_obsolete_results_and_searches_dirty_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.tex");
    fs::write(&path, "disk cat").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open_project(&mut cx, window, dir.path());
    open(&mut cx, window, &path);
    type_text(&mut cx, &root, "unsaved dog ");
    start(&mut cx, window, Intent::New);
    show_search(&mut cx, window, true);
    search_query(&mut cx, &root, "cat");
    assert!(root.read_with(&cx, |workspace, _| workspace.search.running));
    search_query(&mut cx, &root, "dog");
    finish_project_search(&mut cx);
    root.read_with(&cx, |workspace, _| {
        assert_eq!(workspace.search.results.query.text, "dog");
        assert_eq!(workspace.search.results.count(), 1);
        assert!(workspace.search.results.files[0].source.buffer.is_some());
        assert_eq!(workspace.search.results.files[0].matches[0].excerpt, "unsaved dog disk cat");
    });
    search_query(&mut cx, &root, "");
    assert_eq!(root.read_with(&cx, |workspace, _| workspace.search.results.count()), 0);
    assert!(!root.read_with(&cx, |workspace, _| workspace.search.running));
}

#[test]
fn project_results_open_at_positions_after_unrelated_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.tex");
    fs::write(&path, "before cat after").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open_project(&mut cx, window, dir.path());
    show_search(&mut cx, window, true);
    search_query(&mut cx, &root, "cat");
    finish_project_search(&mut cx);
    let file = root.read_with(&cx, |workspace, _| workspace.search.results.files[0].clone());
    fs::write(&path, "猫\nbefore cat after").unwrap();
    start(
        &mut cx,
        window,
        Intent::OpenSearchResult {
            path: file.path,
            original: file.source.text,
            range: file.matches[0].range.clone(),
            query: ligature_search::Query {
                text: "cat".into(),
                ..Default::default()
            },
        },
    );
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.editor.read(cx).state().selected_text(), "cat");
        assert_eq!(workspace.editor.read(cx).state().selection(), 11..14);
        assert!(workspace.error.is_none());
    });
}

#[test]
fn project_replacement_preview_reports_changed_files_and_edits_open_buffers_with_undo() {
    let dir = tempfile::tempdir().unwrap();
    let opened = dir.path().join("opened.tex");
    let changed = dir.path().join("changed.tex");
    let closed = dir.path().join("closed.tex");
    for path in [&opened, &changed, &closed] {
        fs::write(path, "cat cat").unwrap();
    }
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open_project(&mut cx, window, dir.path());
    open(&mut cx, window, &opened);
    type_text(&mut cx, &root, "local ");
    show_search(&mut cx, window, true);
    search_query(&mut cx, &root, "cat");
    finish_project_search(&mut cx);
    root.update(&mut cx, |workspace, cx| {
        workspace.search.replacement.update(cx, |input, cx| input.set_text("dog", cx))
    });
    cx.run_until_parked();
    root.update(&mut cx, |workspace, cx| workspace.preview_project_replacements(cx));
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "local cat cat");
    assert_eq!(fs::read_to_string(&closed).unwrap(), "cat cat");
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    fs::write(&changed, "external edit").unwrap();
    cx.update_window(window.into(), |root, window, cx| {
        root.downcast::<Workspace>()
            .unwrap()
            .update(cx, |workspace, cx| workspace.apply_project_preview(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "local dog dog");
    assert_eq!(fs::read_to_string(&opened).unwrap(), "cat cat");
    assert_eq!(fs::read_to_string(&closed).unwrap(), "dog dog");
    assert_eq!(fs::read_to_string(&changed).unwrap(), "external edit");
    root.read_with(&cx, |workspace, cx| {
        assert!(workspace.editor.read(cx).state().is_dirty());
        let report = workspace.search.report.as_ref().unwrap();
        assert_eq!(report.applied, 2);
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures.keys().next().unwrap().ends_with("changed.tex"));
    });
    root.update(&mut cx, |workspace, cx| {
        workspace.editor.update(cx, |editor, cx| editor.update_state(|state| state.undo(), cx))
    });
    assert_eq!(text(&cx, &root), "local cat cat");
}

#[test]
fn project_replacement_rejects_buffers_edited_after_preview() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.tex");
    fs::write(&path, "cat").unwrap();
    let mut cx = TestAppContext::single();
    let (window, root) = workspace(&mut cx);
    open_project(&mut cx, window, dir.path());
    open(&mut cx, window, &path);
    show_search(&mut cx, window, true);
    search_query(&mut cx, &root, "cat");
    finish_project_search(&mut cx);
    root.update(&mut cx, |workspace, cx| {
        workspace.search.replacement.update(cx, |input, cx| input.set_text("dog", cx))
    });
    cx.run_until_parked();
    root.update(&mut cx, |workspace, cx| workspace.preview_project_replacements(cx));
    cx.run_until_parked();
    type_text(&mut cx, &root, "local ");
    cx.run_until_parked();
    assert!(root.read_with(&cx, |workspace, _| workspace.search.preview_stale));
    cx.update_window(window.into(), |root, window, cx| {
        root.downcast::<Workspace>()
            .unwrap()
            .update(cx, |workspace, cx| workspace.apply_project_preview(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(text(&cx, &root), "local cat");
    assert_eq!(
        root.read_with(&cx, |workspace, _| workspace.search.report.as_ref().unwrap().failures.len()),
        1
    );
}
