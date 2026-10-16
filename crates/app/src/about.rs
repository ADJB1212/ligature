use gpui::{
    App, Bounds, Context, FocusHandle, KeyBinding, Render, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, size,
};
use ligature_core::APP_NAME;
use ligature_theme::{colors, spacing, typography, window_background};
use ligature_workspace::Close;

actions!(ligature_app, [OpenAbout]);

pub(super) fn init(cx: &mut App) {
    let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    cx.bind_keys([
        KeyBinding::new(&format!("{modifier}-w"), Close, Some("LigatureAbout")),
        KeyBinding::new("escape", Close, Some("LigatureAbout")),
    ]);
    cx.on_action(|_: &OpenAbout, cx| {
        if let Some(handle) = cx.windows().into_iter().find_map(|window| window.downcast::<AboutView>())
            && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
        {
            return;
        }
        let bounds = Bounds::centered(None, size(px(400.0), px(260.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("About {APP_NAME}").into()),
                    ..Default::default()
                }),
                is_resizable: false,
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    AboutView { focus }
                })
            },
        ) {
            eprintln!("Could not open About: {error}");
        }
    });
}

struct AboutView {
    focus: FocusHandle,
}

impl Render for AboutView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("LigatureAbout")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Close, window, _| window.remove_window()))
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(spacing::medium())
            .bg(window_background())
            .text_color(colors().text)
            .text_size(typography::control_size())
            .child(div().text_size(px(26.0)).child(APP_NAME))
            .child(ligature_ui::caption(concat!("Version ", env!("CARGO_PKG_VERSION"))))
            .child("A TeX editor")
            .child(ligature_ui::caption(env!("CARGO_PKG_AUTHORS")))
            .child(ligature_ui::caption(concat!(env!("CARGO_PKG_LICENSE"), " license")))
    }
}
