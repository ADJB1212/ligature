//! UI colors, typography, spacing, and theme selection.

use gpui::{ColorExt, Hsla, Pixels, Rgba, px, rgb, rgb_to_hsla};

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
    let paper = rgb_to_hsla(rgb(0xdce3ea));
    Colors {
        background: rgb_to_hsla(rgb(0x1a2330)),
        chrome: rgb_to_hsla(rgb(0x151c27)),
        surface: rgb_to_hsla(rgb(0x232d3c)),
        gutter: rgb_to_hsla(rgb(0x161d28)),
        hover: rgb_to_hsla(rgb(0x334155)),
        press: rgb_to_hsla(rgb(0x121820)),
        text: paper,
        muted_text: rgb_to_hsla(rgb(0x8b97a8)),
        border: rgb_to_hsla(rgb(0x3a4a5c)),
        accent: paper,
        accent_hover: rgb_to_hsla(rgb(0xf2f5f8)),
        accent_press: rgb_to_hsla(rgb(0xc5ced8)),
        on_accent: rgb_to_hsla(rgb(0x1a2330)),
        focus: rgb_to_hsla(rgb(0x9eb0c4)),
        disabled_surface: rgb_to_hsla(rgb(0x2a3544)),
        disabled_text: rgb_to_hsla(rgb(0x6d7a8a)),
        error: rgb_to_hsla(rgb(0xe8a49c)),
        error_surface: rgb_to_hsla(rgb(0x3a2e32)),
        selection: paper.opacity(0.22),
        search_match: paper.opacity(0.12),
        current_line: paper.opacity(0.05),
    }
}

pub struct SyntaxStyle {
    pub color: Hsla,
    pub weight: gpui::FontWeight,
    pub style: gpui::FontStyle,
}

pub fn syntax_style(category: ligature_syntax::Category) -> SyntaxStyle {
    use ligature_syntax::Category;
    let colors = colors();
    SyntaxStyle {
        color: match category {
            Category::Comment => colors.muted_text,
            Category::Brace | Category::MathDelimiter => colors.focus,
            _ => colors.text,
        },
        weight: match category {
            Category::Command | Category::Environment | Category::EntryType | Category::Field => gpui::FontWeight::BOLD,
            _ => gpui::FontWeight::NORMAL,
        },
        style: if category == Category::Comment {
            gpui::FontStyle::Italic
        } else {
            gpui::FontStyle::Normal
        },
    }
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
        px(3.0)
    }
    pub fn pane_boundary() -> Pixels {
        px(6.0)
    }
}

/// Background of the workspace.
pub fn window_background() -> Rgba {
    rgb(0x1a2330)
}

#[cfg(test)]
mod tests {
    use super::*;

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
