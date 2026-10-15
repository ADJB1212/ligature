use std::fs;

use gpui::{AppContext, TestAppContext, WindowHandle};

use super::*;

fn open_settings(cx: &mut TestAppContext, root: &Entity<Workspace>) -> (WindowHandle<SettingsView>, Entity<SettingsView>) {
    cx.update(ligature_ui::init);
    root.update(cx, |workspace, cx| workspace.open_settings(cx));
    let window = root.read_with(cx, |workspace, _| workspace.settings_window.unwrap());
    let view = window.root(cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx)).unwrap();
    (window, view)
}

#[test]
fn settings_shortcut_reuses_the_window_and_close_discards_unsaved_changes() {
    let mut cx = TestAppContext::single();
    let (workspace_window, root) = crate::tests::workspace(&mut cx);
    cx.simulate_keystrokes(workspace_window.into(), "cmd-,");
    cx.run_until_parked();
    let first = root.read_with(&cx, |workspace, _| workspace.settings_window.unwrap());
    let (window, view) = open_settings(&mut cx, &root);
    assert_eq!(first, window);
    view.update(&mut cx, |view, cx| view.fields.width.update(cx, |input, cx| input.set_text("8", cx)));
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.run_until_parked();
    assert!(cx.update_window(window.into(), |_, _, _| ()).is_err());
    let (reopened, view) = open_settings(&mut cx, &root);
    assert_ne!(window, reopened);
    view.read_with(&cx, |view, cx| assert_eq!(view.fields.width.read(cx).text(), "4"));
    root.read_with(&cx, |workspace, cx| assert_eq!(workspace.editor.read(cx).state().tab_size(), 4));
}

#[test]
fn theme_picker_keeps_changes_in_the_draft_until_saved() {
    let mut app = TestAppContext::single();
    let (_, root) = crate::tests::workspace(&mut app);
    let (window, view) = open_settings(&mut app, &root);
    let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
    let option = cx.debug_bounds("midnight").unwrap();
    cx.simulate_click(option.center(), Default::default());
    cx.update(|_, cx| {
        assert_eq!(view.read(cx).configuration.theme.as_deref(), Some("midnight"));
        assert!(!view.read(cx).saved);
        assert_eq!(ligature_theme::current().id, "press_bed");
    });
    cx.simulate_keystrokes("cmd-w");
    app.run_until_parked();
    let (_, reopened) = open_settings(&mut app, &root);
    reopened.read_with(&app, |view, _| assert!(view.configuration.theme.is_none()));
    assert_eq!(ligature_theme::current().id, "press_bed");
}

#[test]
fn saving_defaults_updates_all_editors_and_persists_centrally() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config/ligature/settings.json");
    let source = directory.path().join("source.tex");
    fs::write(&source, "saved source").unwrap();
    let mut cx = TestAppContext::single();
    let (workspace_window, root) = crate::tests::workspace(&mut cx);
    crate::tests::type_text(&mut cx, &root, "untitled changes");
    crate::tests::open(&mut cx, workspace_window, &source);
    root.update(&mut cx, |workspace, _| workspace.configuration_path = Some(path.clone()));
    let (window, view) = open_settings(&mut cx, &root);
    view.update(&mut cx, |view, cx| {
        view.configuration.theme = Some("daylight".into());
        view.fields.width.update(cx, |input, cx| input.set_text("2", cx));
        view.fields.size.update(cx, |input, cx| input.set_text("18.5", cx));
        view.fields.reruns.update(cx, |input, cx| input.set_text("3", cx));
    });
    cx.simulate_keystrokes(window.into(), "cmd-s");
    cx.run_until_parked();
    let (configuration, errors) = Configuration::load(&path);
    assert!(errors.is_empty());
    assert_eq!(configuration.theme.as_deref(), Some("daylight"));
    assert_eq!(ligature_theme::current().id, "daylight");
    assert_eq!(colors().background, ligature_theme::find("daylight").unwrap().colors().background);
    assert_eq!(configuration.defaults.font.size, 18.5);
    assert_eq!(configuration.defaults.build.reruns, Some(3));
    assert_eq!(configuration.defaults.indentation.width, 2);
    assert!(!directory.path().join(".ligature.json").exists());
    root.read_with(&cx, |workspace, cx| {
        assert_eq!(workspace.editor.read(cx).state().tab_size(), 2);
        assert_eq!(workspace.editor.read(cx).state().text(), "saved source");
        let tab = workspace.tabs.iter().flatten().next().unwrap();
        assert_eq!(tab.editor.read(cx).state().tab_size(), 2);
        assert_eq!(tab.editor.read(cx).state().text(), "untitled changes");
        assert!(tab.editor.read(cx).state().is_dirty());
    });
    view.read_with(&cx, |view, _| {
        assert!(view.saved);
        assert!(!view.saving);
        assert!(view.error.is_none());
    });
    view.update(&mut cx, |view, cx| view.fields.width.update(cx, |input, cx| input.set_text("3", cx)));
    cx.run_until_parked();
    assert!(!view.read_with(&cx, |view, _| view.saved));
}

#[test]
fn invalid_settings_leave_saved_configuration_and_editor_state_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let mut cx = TestAppContext::single();
    let (_, root) = crate::tests::workspace(&mut cx);
    root.update(&mut cx, |workspace, _| {
        workspace.configuration_path = Some(path.clone());
        workspace.configuration.save(&path).unwrap();
    });
    let before = fs::read(&path).unwrap();
    let (_, view) = open_settings(&mut cx, &root);
    for invalid in ["0", "17", "many"] {
        view.update(&mut cx, |view, cx| {
            view.fields.width.update(cx, |input, cx| input.set_text(invalid, cx));
            view.save(cx);
        });
        cx.run_until_parked();
        view.read_with(&cx, |view, _| assert!(view.error.is_some()));
        assert_eq!(fs::read(&path).unwrap(), before);
        root.read_with(&cx, |workspace, cx| assert_eq!(workspace.editor.read(cx).state().tab_size(), 4));
    }
}

#[test]
fn failed_settings_writes_report_errors_and_do_not_apply_changes() {
    let directory = tempfile::tempdir().unwrap();
    let blocked = directory.path().join("blocked");
    fs::write(&blocked, "a file").unwrap();
    let mut cx = TestAppContext::single();
    let (_, root) = crate::tests::workspace(&mut cx);
    root.update(&mut cx, |workspace, _| workspace.configuration_path = Some(blocked.join("settings.json")));
    let (_, view) = open_settings(&mut cx, &root);
    view.update(&mut cx, |view, cx| {
        view.configuration.theme = Some("midnight".into());
        view.fields.width.update(cx, |input, cx| input.set_text("2", cx));
        view.save(cx);
    });
    cx.run_until_parked();
    view.read_with(&cx, |view, _| {
        assert!(view.error.is_some());
        assert!(!view.saved);
        assert!(!view.saving);
    });
    assert_eq!(ligature_theme::current().id, "press_bed");
    root.read_with(&cx, |workspace, cx| assert_eq!(workspace.editor.read(cx).state().tab_size(), 4));
}

#[test]
fn project_settings_preserve_application_drafts_and_can_return_to_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("main.tex"), "\\documentclass{article}").unwrap();
    let mut cx = TestAppContext::single();
    let (workspace_window, root) = crate::tests::workspace(&mut cx);
    let project = ligature_project::Project::open(&project).unwrap();
    let project_path = project.root.clone();
    let path = directory.path().join("config/settings.json");
    root.update(&mut cx, |workspace, _| {
        workspace.project = Some(project);
        workspace.configuration_path = Some(path.clone());
    });
    workspace_window
        .update(&mut cx, |workspace, window, cx| workspace.start_project_refresh(window, cx))
        .unwrap();
    cx.run_until_parked();
    let (window, view) = open_settings(&mut cx, &root);
    view.update(&mut cx, |view, cx| view.fields.width.update(cx, |input, cx| input.set_text("3", cx)));
    window.update(&mut cx, |view, window, cx| view.switch_scope(true, window, cx)).unwrap();
    view.update(&mut cx, |view, cx| {
        assert!(view.inherits());
        assert_eq!(view.fields.width.read(cx).text(), "3");
        view.toggle_project_settings(cx);
        view.fields.root.update(cx, |input, cx| input.set_text("missing.tex", cx));
        view.save(cx);
    });
    cx.run_until_parked();
    assert!(!path.exists());
    view.read_with(&cx, |view, _| assert!(view.error.is_some()));
    view.update(&mut cx, |view, cx| {
        view.fields.width.update(cx, |input, cx| input.set_text("8", cx));
        view.fields.root.update(cx, |input, cx| input.set_text("main.tex", cx));
    });
    window.update(&mut cx, |view, window, cx| view.switch_scope(false, window, cx)).unwrap();
    view.update(&mut cx, |view, cx| {
        assert_eq!(view.fields.width.read(cx).text(), "3");
        view.save(cx);
    });
    cx.run_until_parked();
    workspace_window
        .update(&mut cx, |workspace, window, cx| workspace.start_project_refresh(window, cx))
        .unwrap();
    cx.run_until_parked();
    root.read_with(&cx, |workspace, cx| assert_eq!(workspace.editor.read(cx).state().tab_size(), 8));
    let (configuration, errors) = Configuration::load(&path);
    assert!(errors.is_empty());
    assert_eq!(configuration.defaults.indentation.width, 3);
    assert_eq!(configuration.projects[&project_path].indentation.width, 8);
    assert_eq!(configuration.projects[&project_path].root_document, Some("main.tex".into()));
    assert!(!project_path.join(".ligature.json").exists());
    window.update(&mut cx, |view, window, cx| view.switch_scope(true, window, cx)).unwrap();
    view.update(&mut cx, |view, cx| {
        view.toggle_project_settings(cx);
        view.save(cx);
    });
    cx.run_until_parked();
    assert!(Configuration::load(&path).0.projects.is_empty());
    root.read_with(&cx, |workspace, cx| assert_eq!(workspace.editor.read(cx).state().tab_size(), 3));
}

#[test]
fn missing_config_directory_reports_an_error_without_applying_settings() {
    let mut cx = TestAppContext::single();
    let (_, root) = crate::tests::workspace(&mut cx);
    let (_, view) = open_settings(&mut cx, &root);
    view.update(&mut cx, |view, cx| {
        view.fields.width.update(cx, |input, cx| input.set_text("2", cx));
        view.save(cx);
        assert!(view.error.as_ref().unwrap().contains("XDG_CONFIG_HOME"));
    });
    root.read_with(&cx, |workspace, cx| assert_eq!(workspace.editor.read(cx).state().tab_size(), 4));
}

#[test]
fn settings_window_keeps_actions_visible_and_supports_focus_traversal() {
    let mut app = TestAppContext::single();
    let (_, root) = crate::tests::workspace(&mut app);
    let (window, view) = open_settings(&mut app, &root);
    let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
    cx.simulate_resize(size(px(480.0), px(400.0)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let content = cx.debug_bounds("settings-content").unwrap();
    let footer = cx.debug_bounds("settings-footer").unwrap();
    assert!(content.bottom() <= footer.top());
    assert_eq!(footer.bottom(), px(400.0));
    assert!(content.size.height > px(0.0));
    cx.update(|window, cx| assert!(view.read(cx).fields.family.focus_handle(cx).is_focused(window)));
    cx.simulate_keystrokes("tab");
    cx.update(|window, cx| assert!(view.read(cx).fields.size.focus_handle(cx).is_focused(window)));
}
