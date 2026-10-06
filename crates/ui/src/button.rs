use gpui::{App, ClickEvent, ElementId, IntoElement, RenderOnce, SharedString, Window, div, prelude::*};
use ligature_theme::{colors, spacing, typography};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            disabled: false,
            on_click: None,
        }
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
        div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .h(spacing::control_height())
            .px(spacing::large())
            .rounded(spacing::radius())
            .border_2()
            .border_color(gpui::transparent_black())
            .text_size(typography::control_size())
            .line_height(typography::line_height())
            .bg(if self.disabled { colors.disabled_surface } else { colors.accent })
            .text_color(if self.disabled { colors.disabled_text } else { colors.on_accent })
            .when(!self.disabled, |button| {
                button
                    .focusable()
                    .tab_stop(true)
                    .cursor_pointer()
                    .hover(|style| style.bg(colors.accent_hover))
                    .focus(|style| style.border_color(colors.focus))
                    .when_some(self.on_click, |button, handler| button.on_click(handler))
            })
            .child(self.label)
    }
}
