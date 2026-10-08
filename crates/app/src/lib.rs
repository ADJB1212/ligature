//! Application startup, action registration, native menus, and lifecycle.

use gpui::{App, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};
use ligature_core::APP_NAME;
use ligature_workspace::{Close, New, NextTab, Open, OpenProject, PreviousTab, Quit, Save, SaveAs, SelectRoot, ToggleBuildOutput, Workspace};
use ligature_workspace::{Find, FindInProject, FindNext, FindPrevious, Replace};

/// Start the native event loop and open the workspace window.
pub fn run() {
    gpui_platform::application().run(|cx: &mut App| {
        ligature_ui::init(cx);
        ligature_editor::init(cx);
        ligature_workspace::init(cx);
        cx.on_action(|_: &Quit, cx| {
            if let Some(window) = cx.windows().first().and_then(|window| window.downcast::<Workspace>()) {
                window.update(cx, |workspace, window, cx| workspace.request_quit(window, cx)).ok();
            } else {
                cx.quit();
            }
        });
        cx.set_menus(vec![
            Menu::new(APP_NAME).items([MenuItem::action("Quit Ligature", Quit)]),
            Menu::new("File").items([
                MenuItem::action("New", New),
                MenuItem::action("Open...", Open),
                MenuItem::action("Open Folder...", OpenProject),
                MenuItem::separator(),
                MenuItem::action("Save", Save),
                MenuItem::action("Save As...", SaveAs),
                MenuItem::separator(),
                MenuItem::action("Close", Close),
                MenuItem::separator(),
                MenuItem::action("Set Active Document as Project Root", SelectRoot),
            ]),
            Menu::new("Window").items([
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Previous Tab", PreviousTab),
                MenuItem::separator(),
                MenuItem::action("Toggle Build Output", ToggleBuildOutput),
            ]),
            Menu::new("Search").items([
                MenuItem::action("Find...", Find),
                MenuItem::action("Replace...", Replace),
                MenuItem::action("Find in Project...", FindInProject),
                MenuItem::separator(),
                MenuItem::action("Find Next", FindNext),
                MenuItem::action("Find Previous", FindPrevious),
            ]),
        ]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(APP_NAME.into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(640.0), px(480.0))),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let mut workspace = Workspace::new(window, cx);
                    workspace.restore_session(window, cx);
                    workspace
                })
            },
        )
        .expect("failed to open the Ligature window");
        cx.activate(true);
    });
}
