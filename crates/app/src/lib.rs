//! Application startup, action registration, native menus, and lifecycle.

use gpui::{App, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};
use ligature_core::APP_NAME;
use ligature_workspace::{
    Build, CancelBuild, Close, ExportPdf, Find, FindInProject, FindNext, FindPrevious, FitPage, FitWidth, FormatDocument, GoToDefinition, New,
    NextTab, Open, OpenProject, OpenSettings, PreviousTab, Quit, Replace, RestartLanguageServer, Save, SaveAs, ShowInPreview, ToggleBuildOutput,
    ToggleLanguageServer, ToggleOutline, ToggleProject, ToggleTexResources, Workspace, ZoomIn, ZoomOut,
};

mod about;

/// Start the native event loop and open the workspace window.
pub fn run() {
    gpui_platform::application().run(|cx: &mut App| {
        ligature_ui::init(cx);
        ligature_editor::init(cx);
        ligature_workspace::init(cx);
        about::init(cx);
        cx.on_action(|_: &Quit, cx| {
            if let Some(window) = cx.windows().into_iter().find_map(|window| window.downcast::<Workspace>()) {
                window.update(cx, |workspace, window, cx| workspace.request_quit(window, cx)).ok();
            } else {
                cx.quit();
            }
        });
        cx.on_action(|_: &OpenSettings, cx| {
            if let Some(window) = cx.windows().into_iter().find_map(|window| window.downcast::<Workspace>()) {
                window.update(cx, |workspace, _, cx| workspace.open_settings(cx)).ok();
            }
        });
        cx.set_menus(vec![
            Menu::new(APP_NAME).items([
                MenuItem::action("About Ligature", about::OpenAbout),
                MenuItem::separator(),
                MenuItem::action("Settings...", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("TeX Resources...", ToggleTexResources),
                MenuItem::separator(),
                MenuItem::action("Quit Ligature", Quit),
            ]),
            Menu::new("File").items([
                MenuItem::action("New", New),
                MenuItem::action("Open...", Open),
                MenuItem::action("Open Folder...", OpenProject),
                MenuItem::separator(),
                MenuItem::action("Save", Save),
                MenuItem::action("Save As...", SaveAs),
                MenuItem::action("Export PDF...", ExportPdf),
                MenuItem::separator(),
                MenuItem::action("Close", Close),
            ]),
            Menu::new("Edit").items([
                MenuItem::action("Undo", ligature_editor::Undo),
                MenuItem::action("Redo", ligature_editor::Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", ligature_editor::Cut),
                MenuItem::action("Copy", ligature_editor::Copy),
                MenuItem::action("Paste", ligature_editor::Paste),
                MenuItem::action("Select All", ligature_editor::SelectAll),
                MenuItem::separator(),
                MenuItem::action("Find...", Find),
                MenuItem::action("Replace...", Replace),
                MenuItem::action("Find in Project...", FindInProject),
                MenuItem::action("Find Next", FindNext),
                MenuItem::action("Find Previous", FindPrevious),
                MenuItem::separator(),
                MenuItem::action("Complete", ligature_editor::Complete),
                MenuItem::action("Format Document", FormatDocument),
            ]),
            Menu::new("View").items([
                MenuItem::action("Toggle Project Sidebar", ToggleProject),
                MenuItem::action("Toggle Outline", ToggleOutline),
                MenuItem::action("Toggle Build Output", ToggleBuildOutput),
                MenuItem::separator(),
                MenuItem::submenu(Menu::new("PDF Zoom").items([
                    MenuItem::action("Zoom In", ZoomIn),
                    MenuItem::action("Zoom Out", ZoomOut),
                    MenuItem::separator(),
                    MenuItem::action("Fit Width", FitWidth),
                    MenuItem::action("Fit Page", FitPage),
                ])),
                MenuItem::separator(),
                MenuItem::action("Language Server Status", ToggleLanguageServer),
                MenuItem::action("Restart Language Server", RestartLanguageServer),
            ]),
            Menu::new("Build").items([MenuItem::action("Build Document", Build), MenuItem::action("Cancel Build", CancelBuild)]),
            Menu::new("Navigate").items([
                MenuItem::action("Go to Definition", GoToDefinition),
                MenuItem::action("Show Hover", ligature_editor::ShowHover),
                MenuItem::separator(),
                MenuItem::action("Show in PDF", ShowInPreview),
            ]),
            Menu::new("Window").items([MenuItem::action("Next Tab", NextTab), MenuItem::action("Previous Tab", PreviousTab)]),
        ]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1700.0), px(1000.0)), cx);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_compiles() {
        // App startup logic is currently tested via integration and manual testing.
        // This test ensures the `run` signature remains intact.
        let _f: fn() = run;
    }
}
