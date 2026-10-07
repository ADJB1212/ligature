//! UI colors, typography, spacing, and theme selection.

use gpui::{ColorExt, Hsla, Pixels, Rgba, px, rgb, rgb_to_hsla};

pub struct Colors {
    pub background: Hsla,
    pub surface: Hsla,
    pub text: Hsla,
    pub muted_text: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub on_accent: Hsla,
    pub focus: Hsla,
    pub disabled_surface: Hsla,
    pub disabled_text: Hsla,
    pub error: Hsla,
    pub error_surface: Hsla,
    pub selection: Hsla,
    pub search_match: Hsla,
}

pub fn colors() -> Colors {
    Colors {
        background: rgb_to_hsla(rgb(0x1a2330)),
        surface: rgb_to_hsla(rgb(0x232d3c)),
        text: rgb_to_hsla(rgb(0xdce3ea)),
        muted_text: rgb_to_hsla(rgb(0x8b97a8)),
        border: rgb_to_hsla(rgb(0x3a4a5c)),
        accent: rgb_to_hsla(rgb(0xdce3ea)),
        accent_hover: rgb_to_hsla(rgb(0xf2f5f8)),
        on_accent: rgb_to_hsla(rgb(0x1a2330)),
        focus: rgb_to_hsla(rgb(0x9eb0c4)),
        disabled_surface: rgb_to_hsla(rgb(0x2a3544)),
        disabled_text: rgb_to_hsla(rgb(0x6d7a8a)),
        error: rgb_to_hsla(rgb(0xe8a49c)),
        error_surface: rgb_to_hsla(rgb(0x3a2e32)),
        selection: rgb_to_hsla(rgb(0xdce3ea)).opacity(0.22),
        search_match: rgb_to_hsla(rgb(0xdce3ea)).opacity(0.12),
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
        px(14.0)
    }
    pub fn small_size() -> Pixels {
        px(12.0)
    }
    pub fn line_height() -> Pixels {
        px(20.0)
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
        px(32.0)
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
    rgb(0x1a2330)
}
