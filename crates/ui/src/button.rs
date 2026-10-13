use gpui::{App, ClickEvent, ColorExt, Context, ElementId, IntoElement, Render, RenderOnce, SharedString, Window, div, prelude::*};
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
    full_width: bool,
    tooltip: Option<SharedString>,
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
            full_width: false,
            tooltip: None,
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

    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
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
        let filled = matches!(self.style, ButtonStyle::Filled);
        let (background, foreground) = if self.disabled {
            (
                if filled { colors.disabled_surface } else { gpui::transparent_black() },
                colors.disabled_text,
            )
        } else if filled {
            (colors.accent, colors.on_accent)
        } else if self.selected {
            (colors.text.opacity(0.08), colors.text)
        } else {
            (gpui::transparent_black(), if quiet { colors.muted_text } else { colors.text })
        };
        div()
            .id(self.id)
            .flex()
            .min_w(gpui::px(0.0))
            .when(self.full_width, |button| button.w_full())
            .items_center()
            .when(quiet, |button| button.justify_start())
            .when(!quiet, |button| button.justify_center())
            .h(if quiet { spacing::compact_height() } else { spacing::control_height() })
            .px(if quiet { spacing::medium() } else { spacing::large() })
            .rounded(spacing::radius())
            .border_1()
            .border_color(gpui::transparent_black())
            .text_size(typography::control_size())
            .line_height(typography::line_height())
            .bg(background)
            .text_color(foreground)
            .when(filled, |button| button.font_weight(gpui::FontWeight::SEMIBOLD))
            .when_some(self.tooltip, |button, text| {
                button.tooltip(move |_, cx| cx.new(|_| ButtonTooltip { text: text.clone() }).into())
            })
            .when(!self.disabled, |button| {
                button
                    .focusable()
                    .tab_stop(true)
                    .cursor_pointer()
                    .hover(|style| {
                        if filled {
                            style.bg(colors.accent_hover)
                        } else {
                            style.bg(colors.hover).text_color(colors.text)
                        }
                    })
                    .active(|style| {
                        if filled {
                            style.bg(colors.accent_press)
                        } else {
                            style.bg(colors.press).text_color(colors.text)
                        }
                    })
                    .focus(|style| style.border_color(colors.focus))
                    .when_some(self.on_click, |button, handler| button.on_click(handler))
            })
            .child(div().min_w(gpui::px(0.0)).overflow_hidden().text_ellipsis().child(self.label))
    }
}

struct ButtonTooltip {
    text: SharedString,
}

impl Render for ButtonTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(gpui::px(480.0))
            .px(spacing::medium())
            .py(spacing::small())
            .rounded(spacing::radius())
            .border_1()
            .border_color(colors().border)
            .bg(colors().surface)
            .text_color(colors().text)
            .text_size(typography::small_size())
            .line_height(typography::line_height())
            .child(self.text.clone())
    }
}
