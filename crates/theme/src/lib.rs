//! UI colors, typography, spacing, and theme selection.

use gpui::{Hsla, Pixels, Rgba, px, rgb, rgb_to_hsla};

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
}

pub fn colors() -> Colors {
    Colors {
        background: rgb_to_hsla(rgb(0x1f363d)),
        surface: rgb_to_hsla(rgb(0x29464f)),
        text: rgb_to_hsla(rgb(0xcfe0c3)),
        muted_text: rgb_to_hsla(rgb(0x9ec1a3)),
        border: rgb_to_hsla(rgb(0x40798c)),
        accent: rgb_to_hsla(rgb(0x9ec1a3)),
        accent_hover: rgb_to_hsla(rgb(0xcfe0c3)),
        on_accent: rgb_to_hsla(rgb(0x1f363d)),
        focus: rgb_to_hsla(rgb(0x70a9a1)),
        disabled_surface: rgb_to_hsla(rgb(0x354c53)),
        disabled_text: rgb_to_hsla(rgb(0x91a0a1)),
        error: rgb_to_hsla(rgb(0xffb4ab)),
        error_surface: rgb_to_hsla(rgb(0x503638)),
    }
}

pub mod typography {
    use super::{Pixels, px};

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
    rgb(0x1f363d)
}
