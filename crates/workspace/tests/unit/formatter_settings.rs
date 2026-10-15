use gpui::{TestAppContext, VisualTestContext};

#[test]
fn format_on_save_toggle_stays_in_the_draft_until_saved_and_persists() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let mut app = TestAppContext::single();
    let (_, root) = crate::tests::workspace(&mut app);
    root.update(&mut app, |workspace, _| workspace.configuration_path = Some(path.clone()));
    root.update(&mut app, |workspace, cx| workspace.open_settings(cx));
    let window = root.read_with(&app, |workspace, _| workspace.settings_window.unwrap());
    let view = window.root(&mut app).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), &app);
    let toggle = cx.debug_bounds("settings-format-on-save").unwrap();
    cx.simulate_click(toggle.center(), Default::default());
    view.read_with(&app, |view, _| assert!(view.fields.settings.format_on_save));
    root.read_with(&app, |workspace, _| assert!(!workspace.settings.format_on_save));
    cx.simulate_keystrokes("cmd-s");
    app.run_until_parked();
    assert!(ligature_settings::Configuration::load(&path).0.defaults.format_on_save);
    root.read_with(&app, |workspace, _| assert!(workspace.settings.format_on_save));
}
