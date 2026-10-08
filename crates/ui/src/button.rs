use gpui::{App, ClickEvent, ColorExt, ElementId, IntoElement, RenderOnce, SharedString, Window, div, prelude::*};
use ligature_theme::{colors, spacing, typography};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone, Copy, Default)]
pub enum ButtonStyle {
    #[default]
    Ghost,
    Filled,
    Quiet,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    disabled: bool,
    selected: bool,
    style: ButtonStyle,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            disabled: false,
            selected: false,
            style: ButtonStyle::Ghost,
            on_click: None,
        }
    }

    pub fn filled(mut self) -> Self {
        self.style = ButtonStyle::Filled;
        self
    }

    pub fn quiet(mut self) -> Self {
        self.style = ButtonStyle::Quiet;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = colors();
        let quiet = matches!(self.style, ButtonStyle::Quiet);
        let filled = matches!(self.style, ButtonStyle::Filled) || (self.selected && matches!(self.style, ButtonStyle::Ghost));
        let (background, foreground) = if self.disabled {
            (
                if filled { colors.disabled_surface } else { gpui::transparent_black() },
                colors.disabled_text,
            )
        } else if filled {
            (colors.accent, colors.on_accent)
        } else if self.selected {
            (colors.text.opacity(0.14), colors.text)
        } else {
            (gpui::transparent_black(), if quiet { colors.muted_text } else { colors.text })
        };
        div()
            .id(self.id)
            .flex()
            .items_center()
            .when(quiet, |button| button.justify_start())
            .when(!quiet, |button| button.justify_center())
            .h(if quiet { spacing::compact_height() } else { spacing::control_height() })
            .px(if quiet { spacing::medium() } else { spacing::large() })
            .rounded(spacing::radius())
            .border_2()
            .border_color(gpui::transparent_black())
            .text_size(if quiet { typography::small_size() } else { typography::control_size() })
            .line_height(typography::line_height())
            .bg(background)
            .text_color(foreground)
            .when(!self.disabled, |button| {
                button
                    .focusable()
                    .tab_stop(true)
                    .cursor_pointer()
                    .hover(|style| {
                        if filled {
                            style.bg(colors.accent_hover)
                        } else {
                            style.bg(colors.hover).text_color(colors.text).border_color(colors.text.opacity(0.22))
                        }
                    })
                    .active(|style| {
                        if filled {
                            style.bg(colors.accent_press)
                        } else {
                            style.bg(colors.press).text_color(colors.text).border_color(colors.text.opacity(0.4))
                        }
                    })
                    .focus(|style| style.border_color(colors.focus))
                    .when_some(self.on_click, |button, handler| button.on_click(handler))
            })
            .child(self.label)
    }
}
