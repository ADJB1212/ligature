//! Application startup, action registration, native menus, and lifecycle.

use gpui::{App, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions, actions, prelude::*, px, size};
use ligature_core::APP_NAME;
use ligature_workspace::Workspace;

actions!(ligature, [Quit]);

/// Start the native event loop and open the workspace window.
pub fn run() {
    gpui_platform::application().run(|cx: &mut App| {
        ligature_ui::init(cx);
        ligature_editor::init(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        #[cfg(target_os = "macos")]
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        #[cfg(not(target_os = "macos"))]
        cx.bind_keys([KeyBinding::new("ctrl-q", Quit, None)]);
        cx.set_menus(vec![Menu::new(APP_NAME).items([MenuItem::action("Quit Ligature", Quit)])]);
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
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .expect("failed to open the Ligature window");
        cx.activate(true);
    });
}
