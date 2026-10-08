use std::{ops::Range, time::Duration};

use gpui::{
    App, Bounds, ColorExt, Context, CursorStyle, ElementInputHandler, EntityInputHandler, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    MouseButton, Pixels, Point, Render, ShapedLine, Task, TextRun, UTF16Selection, Window, canvas, div, fill, point, prelude::*, px, size,
};
use ligature_theme::{colors, spacing, typography};

pub struct TextInput {
    focus: FocusHandle,
    text: String,
    placeholder: gpui::SharedString,
    cursor: usize,
    marked: Option<Range<usize>>,
    line: Option<ShapedLine>,
    bounds: Bounds<Pixels>,
    scroll: Pixels,
    caret_on: bool,
    blink: Option<Task<()>>,
    blink_epoch: u64,
}

impl TextInput {
    pub fn new(placeholder: impl Into<gpui::SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            text: String::new(),
            placeholder: placeholder.into(),
            cursor: 0,
            marked: None,
            line: None,
            bounds: Bounds::default(),
            scroll: px(0.0),
            caret_on: true,
            blink: None,
            blink_epoch: 0,
        }
    }

    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.caret_on = true;
        self.blink_epoch += 1;
        let epoch = self.blink_epoch;
        self.blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                let cont = this
                    .update(cx, |input, cx| {
                        if input.blink_epoch != epoch {
                            return false;
                        }
                        input.caret_on = !input.caret_on;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !cont {
                    break;
                }
            }
        }));
    }

    fn ensure_blink(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.focus.is_focused(window) {
            if self.blink.is_none() {
                self.restart_blink(cx);
            }
        } else {
            self.blink = None;
            self.blink_epoch += 1;
            self.caret_on = false;
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = single_line(text);
        self.cursor = self.text.len();
        self.marked = None;
        self.restart_blink(cx);
        cx.notify();
    }

    fn replace(&mut self, range: Range<usize>, text: &str) -> Range<usize> {
        let text = single_line(text);
        self.text.replace_range(range.clone(), &text);
        self.cursor = range.start + text.len();
        range.start..self.cursor
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let previous = self.text[..self.cursor].char_indices().next_back().map_or(0, |(index, _)| index);
        let next = self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |ch| self.cursor + ch.len_utf8());
        match event.keystroke.key.as_str() {
            "left" => self.cursor = previous,
            "right" => self.cursor = next,
            "home" => self.cursor = 0,
            "end" => self.cursor = self.text.len(),
            "backspace" => {
                self.replace(previous..self.cursor, "");
            }
            "delete" => {
                self.replace(self.cursor..next, "");
            }
            _ => return,
        }
        self.marked = None;
        window.prevent_default();
        cx.stop_propagation();
        self.restart_blink(cx);
        cx.notify();
    }
}

const CARET_BLINK: Duration = Duration::from_millis(530);

fn single_line(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_control()).collect()
}

fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        if units + ch.len_utf16() > utf16 {
            return offset;
        }
        units += ch.len_utf16();
    }
    text.len()
}

fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

fn byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    let start = byte_offset(text, range.start);
    start..byte_offset(text, range.end).max(start)
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self, range: Range<usize>, actual_range: &mut Option<Range<usize>>, _window: &mut Window, _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = byte_range(&self.text, range);
        *actual_range = Some(utf16_offset(&self.text, range.start)..utf16_offset(&self.text, range.end));
        Some(self.text[range].to_owned())
    }

    fn selected_text_range(&mut self, _ignore_disabled_input: bool, _window: &mut Window, _cx: &mut Context<Self>) -> Option<UTF16Selection> {
        let cursor = utf16_offset(&self.text, self.cursor);
        Some(UTF16Selection {
            range: cursor..cursor,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| utf16_offset(&self.text, range.start)..utf16_offset(&self.text, range.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let range = range
            .map(|range| byte_range(&self.text, range))
            .or_else(|| self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        self.replace(range, text);
        self.marked = None;
        self.restart_blink(cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self, range: Option<Range<usize>>, text: &str, selected: Option<Range<usize>>, _window: &mut Window, cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| byte_range(&self.text, range))
            .or_else(|| self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        let inserted = self.replace(range, text);
        if let Some(selected) = selected {
            self.cursor = inserted.start + byte_offset(&self.text[inserted.clone()], selected.end);
        }
        self.marked = (!inserted.is_empty()).then_some(inserted);
        self.restart_blink(cx);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self, range: Range<usize>, _bounds: Bounds<Pixels>, _window: &mut Window, _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = byte_range(&self.text, range);
        let line = self.line.as_ref()?;
        let start = if self.text.is_empty() { px(0.0) } else { line.x_for_index(range.start) };
        let end = if self.text.is_empty() { px(0.0) } else { line.x_for_index(range.end) };
        Some(Bounds::new(
            point(self.bounds.left() + start - self.scroll, self.bounds.top()),
            size((end - start).max(px(1.0)), self.bounds.size.height),
        ))
    }

    fn character_index_for_point(&mut self, position: Point<Pixels>, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        if self.text.is_empty() {
            return Some(0);
        }
        let byte = self.line.as_ref()?.closest_index_for_x(position.x - self.bounds.left() + self.scroll);
        Some(utf16_offset(&self.text, byte))
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_blink(window, cx);
        let colors = colors();
        let entity = cx.entity();
        let paint_entity = entity.clone();
        div()
            .id("text-input")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .w_full()
            .h(spacing::control_height())
            .px(spacing::medium())
            .flex()
            .items_center()
            .rounded(spacing::radius())
            .border_1()
            .border_color(colors.border)
            .hover(|style| style.border_color(colors.focus.opacity(0.7)))
            .focus(|style| style.border_color(colors.focus))
            .bg(colors.surface)
            .text_size(typography::control_size())
            .line_height(typography::line_height())
            .overflow_hidden()
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.cursor = if this.text.is_empty() {
                        0
                    } else {
                        this.line.as_ref().map_or(this.text.len(), |line| {
                            line.closest_index_for_x(event.position.x - this.bounds.left() + this.scroll)
                        })
                    };
                    this.marked = None;
                    this.restart_blink(cx);
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let input = entity.read(cx);
                        let empty = input.text.is_empty();
                        let text: gpui::SharedString = if empty { input.placeholder.clone() } else { input.text.clone().into() };
                        let style = window.text_style();
                        let line = window.text_system().shape_line(
                            text.clone(),
                            typography::control_size(),
                            &[TextRun {
                                len: text.len(),
                                font: style.font(),
                                color: if empty { colors.muted_text } else { colors.text },
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                                letter_spacing: None,
                            }],
                            None,
                        );
                        let caret = if empty { px(0.0) } else { line.x_for_index(input.cursor) };
                        let scroll = (caret - bounds.size.width + px(2.0)).max(px(0.0));
                        (line, caret, scroll)
                    },
                    move |bounds, (line, caret, scroll), window, cx| {
                        let origin = point(bounds.left() - scroll, bounds.top());
                        let focus = paint_entity.read(cx).focus.clone();
                        window.handle_input(&focus, ElementInputHandler::new(bounds, paint_entity.clone()), cx);
                        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                            line.paint(origin, bounds.size.height, gpui::TextAlign::Left, None, window, cx)
                                .expect("input text painting failed");
                            if focus.is_focused(window) && paint_entity.read(cx).caret_on {
                                window.paint_quad(fill(
                                    Bounds::new(point(origin.x + caret, origin.y), size(px(1.0), bounds.size.height)),
                                    colors.text,
                                ));
                            }
                        });
                        paint_entity.update(cx, |input, _| {
                            input.bounds = bounds;
                            input.line = Some(line);
                            input.scroll = scroll;
                        });
                    },
                )
                .w_full()
                .h(typography::line_height()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_ranges_do_not_split_surrogate_pairs() {
        let text = "a😀é";
        assert_eq!(byte_range(text, 1..2), 1..1);
        assert_eq!(byte_range(text, 1..3), 1..5);
        assert_eq!(byte_range(text, 3..99), 5..7);
        assert_eq!(utf16_offset(text, 5), 3);
    }

    #[test]
    fn input_filters_line_breaks_and_control_characters() {
        assert_eq!(single_line("a\r\nb\tc\0é"), "abcé");
    }
}
