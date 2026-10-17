use gpui::{
    App, Bounds, Context, FocusHandle, FontWeight, KeyBinding, Render, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div,
    prelude::*, px, size,
};
use ligature_core::APP_NAME;
use ligature_theme::{colors, spacing, typography, window_background};
use ligature_ui::{caption, hairline};
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
        let bounds = Bounds::centered(None, size(px(440.0), px(360.0)), cx);
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
            .bg(window_background())
            .text_color(colors().text)
            .text_size(typography::control_size())
            .line_height(typography::line_height())
            .px(px(32.0))
            .pt(px(28.0))
            .pb(px(32.0))
            .gap(px(20.0))
            .child(identity())
            .child(hairline())
            .child(facts())
    }
}

fn identity() -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(spacing::medium())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(22.0))
                        .line_height(px(28.0))
                        .child(APP_NAME),
                )
                .child(caption(concat!("Version ", env!("CARGO_PKG_VERSION")))),
        )
        .child(caption("A TeX editor"))
}

fn facts() -> impl IntoElement {
    let people = credits(env!("CARGO_PKG_AUTHORS"));
    let author_label = if people.len() == 1 { "Author" } else { "Authors" };
    div()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .when(!people.is_empty(), |facts| {
            facts.child(entry(
                author_label,
                div().flex().flex_col().gap(px(8.0)).children(people.into_iter().map(|(name, email)| {
                    div()
                        .flex()
                        .flex_col()
                        .child(name)
                        .when_some(email, |person, email| person.child(caption(email)))
                })),
            ))
        })
        .child(entry("License", env!("CARGO_PKG_LICENSE")))
        .when_some(package_source(), |facts, source| facts.child(entry("Source", source)))
}

fn entry(label: &'static str, body: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(typography::small_size())
                .line_height(typography::line_height())
                .text_color(colors().muted_text)
                .child(label),
        )
        .child(body)
}

fn package_source() -> Option<&'static str> {
    let source = source_label(env!("CARGO_PKG_REPOSITORY"));
    (!source.is_empty()).then_some(source)
}

fn credits(authors: &str) -> Vec<(&str, Option<&str>)> {
    authors
        .split(':')
        .filter(|author| !author.is_empty())
        .filter_map(|author| {
            let (name, email) = author.split_once('<').unwrap_or((author, ""));
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            let email = email.trim();
            let email = email.strip_suffix('>').unwrap_or(email).trim();
            Some((name, (!email.is_empty()).then_some(email)))
        })
        .collect()
}

fn source_label(repository: &str) -> &str {
    repository.trim().trim_start_matches("https://").trim_start_matches("http://")
}

#[cfg(test)]
mod tests {
    use super::{credits, source_label};

    #[test]
    fn credits_split_names_and_email_addresses() {
        assert_eq!(
            credits("Andrew Jaffe <adjaffeb@gmail.com>"),
            vec![("Andrew Jaffe", Some("adjaffeb@gmail.com"))]
        );
        assert_eq!(
            credits("Ada Lovelace <ada@example.com>:Grace Hopper"),
            vec![("Ada Lovelace", Some("ada@example.com")), ("Grace Hopper", None)]
        );
        assert!(credits("").is_empty());
        assert!(credits(":").is_empty());
        assert!(credits(" <>").is_empty());
    }

    #[test]
    fn source_label_drops_the_web_scheme() {
        assert_eq!(source_label("https://github.com/ADJB1212/ligature"), "github.com/ADJB1212/ligature");
        assert_eq!(source_label("http://example.com"), "example.com");
        assert_eq!(source_label(" github.com/ADJB1212/ligature "), "github.com/ADJB1212/ligature");
    }
}
