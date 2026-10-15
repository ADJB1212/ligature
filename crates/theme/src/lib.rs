//! UI colors, typography, spacing, and theme selection.

use std::cell::Cell;

use gpui::{App, ColorExt, Hsla, Pixels, Rgba, px, rgb, rgb_to_hsla};

mod builtins;
pub use builtins::THEMES;

pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub palette: Palette,
    pub syntax: SyntaxPalette,
}

pub struct Palette {
    pub background: u32,
    pub chrome: u32,
    pub surface: u32,
    pub gutter: u32,
    pub text: u32,
    pub muted_text: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub accent_press: u32,
    pub on_accent: u32,
    pub focus: u32,
    pub disabled_surface: u32,
    pub disabled_text: u32,
    pub error: u32,
    pub error_surface: u32,
}

pub struct SyntaxPalette {
    pub command: u32,
    pub environment: u32,
    pub brace: u32,
    pub comment: u32,
    pub math_delimiter: u32,
    pub verbatim: u32,
    pub entry_type: u32,
    pub field: u32,
    pub value: u32,
}

impl Theme {
    pub fn colors(&self) -> Colors {
        let palette = &self.palette;
        let text = rgb_to_hsla(rgb(palette.text));
        Colors {
            background: rgb_to_hsla(rgb(palette.background)),
            chrome: rgb_to_hsla(rgb(palette.chrome)),
            surface: rgb_to_hsla(rgb(palette.surface)),
            gutter: rgb_to_hsla(rgb(palette.gutter)),
            hover: text.opacity(0.08),
            press: text.opacity(0.12),
            text,
            muted_text: rgb_to_hsla(rgb(palette.muted_text)),
            border: text.opacity(0.1),
            accent: rgb_to_hsla(rgb(palette.accent)),
            accent_hover: rgb_to_hsla(rgb(palette.accent_hover)),
            accent_press: rgb_to_hsla(rgb(palette.accent_press)),
            on_accent: rgb_to_hsla(rgb(palette.on_accent)),
            focus: rgb_to_hsla(rgb(palette.focus)),
            disabled_surface: rgb_to_hsla(rgb(palette.disabled_surface)),
            disabled_text: rgb_to_hsla(rgb(palette.disabled_text)),
            error: rgb_to_hsla(rgb(palette.error)),
            error_surface: rgb_to_hsla(rgb(palette.error_surface)),
            selection: rgb_to_hsla(rgb(palette.accent)).opacity(0.22),
            search_match: rgb_to_hsla(rgb(palette.accent)).opacity(0.12),
            current_line: text.opacity(0.05),
        }
    }

    pub fn syntax_color(&self, category: ligature_syntax::Category) -> Hsla {
        use ligature_syntax::Category;
        let syntax = &self.syntax;
        rgb_to_hsla(rgb(match category {
            Category::Text => self.palette.text,
            Category::Command => syntax.command,
            Category::Environment => syntax.environment,
            Category::Brace => syntax.brace,
            Category::Comment => syntax.comment,
            Category::MathDelimiter => syntax.math_delimiter,
            Category::Verbatim => syntax.verbatim,
            Category::EntryType => syntax.entry_type,
            Category::Field => syntax.field,
            Category::Value => syntax.value,
        }))
    }
}

// GPUI renders and applies settings on the UI thread.
thread_local! {
    static ACTIVE: Cell<&'static Theme> = const { Cell::new(&THEMES[0]) };
}

pub fn current() -> &'static Theme {
    ACTIVE.with(Cell::get)
}

pub fn find(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|theme| theme.id == id)
}

pub fn resolve(id: Option<&str>) -> &'static Theme {
    id.and_then(find).unwrap_or(&THEMES[0])
}

pub fn set_theme(id: Option<&str>, cx: &mut App) {
    let theme = resolve(id);
    ACTIVE.with(|active| active.set(theme));
    cx.refresh_windows();
}

pub struct Colors {
    pub background: Hsla,
    pub chrome: Hsla,
    pub surface: Hsla,
    pub gutter: Hsla,
    pub hover: Hsla,
    pub press: Hsla,
    pub text: Hsla,
    pub muted_text: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_press: Hsla,
    pub on_accent: Hsla,
    pub focus: Hsla,
    pub disabled_surface: Hsla,
    pub disabled_text: Hsla,
    pub error: Hsla,
    pub error_surface: Hsla,
    pub selection: Hsla,
    pub search_match: Hsla,
    pub current_line: Hsla,
}

pub fn colors() -> Colors {
    current().colors()
}

pub fn syntax_color(category: ligature_syntax::Category) -> Hsla {
    current().syntax_color(category)
}

pub mod typography {
    use super::{Pixels, px};

    pub fn editor_size() -> Pixels {
        px(15.0)
    }

    pub fn editor_line_height() -> Pixels {
        px(22.5)
    }

    pub fn control_size() -> Pixels {
        px(13.0)
    }

    pub fn small_size() -> Pixels {
        px(11.0)
    }

    pub fn line_height() -> Pixels {
        px(18.0)
    }
}

pub mod spacing {
    use super::{Pixels, px};

    pub fn small() -> Pixels {
        px(4.0)
    }
    pub fn medium() -> Pixels {
        px(8.0)
    }
    pub fn large() -> Pixels {
        px(16.0)
    }
    pub fn control_height() -> Pixels {
        px(28.0)
    }
    pub fn compact_height() -> Pixels {
        px(24.0)
    }
    pub fn radius() -> Pixels {
        px(4.0)
    }
    pub fn pane_boundary() -> Pixels {
        px(6.0)
    }
}

/// Background of the workspace.
pub fn window_background() -> Rgba {
    rgb(current().palette.background)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_unknown_preferences_use_the_default_theme() {
        assert_eq!(resolve(None).id, "press_bed");
        assert_eq!(resolve(Some("removed_theme")).id, "press_bed");
        for theme in THEMES {
            assert_eq!(resolve(Some(theme.id)).name, theme.name);
            assert_eq!(THEMES.iter().filter(|other| other.id == theme.id).count(), 1);
        }
    }

    #[test]
    fn built_in_text_and_syntax_have_readable_contrast() {
        fn luminance(color: u32) -> f64 {
            let linear = |channel: u32| {
                let value = f64::from(channel & 0xff) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color >> 16) + 0.7152 * linear(color >> 8) + 0.0722 * linear(color)
        }
        for theme in THEMES {
            let background = luminance(theme.palette.background);
            let syntax = &theme.syntax;
            for color in [
                theme.palette.text,
                theme.palette.muted_text,
                syntax.command,
                syntax.environment,
                syntax.brace,
                syntax.comment,
                syntax.math_delimiter,
                syntax.verbatim,
                syntax.entry_type,
                syntax.field,
                syntax.value,
            ] {
                let foreground = luminance(color);
                let contrast = (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05);
                assert!(contrast >= 4.5, "{}: {color:06x} has contrast {contrast}", theme.name);
            }
        }
    }

    #[test]
    fn test_colors_initialization() {
        let c = colors();
        // Just verify we can instantiate it
        let _ = c.background;
    }

    #[test]
    fn test_typography_and_spacing() {
        assert_eq!(typography::editor_size(), px(15.0));
        assert_eq!(spacing::small(), px(4.0));
    }
}
