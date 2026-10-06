use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementInputHandler, EntityInputHandler, FocusHandle, Focusable, Font, IntoElement, KeyBinding,
    KeyDownEvent, MouseButton, Pixels, Point, Render, ShapedLine, SharedString, TextRun, UTF16Selection, Window, actions, canvas, div, fill, point,
    prelude::*, px, size,
};
use ligature_buffer::Buffer;
use ligature_theme::{colors, typography};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    EditorState, Movement,
    state::{byte_offset, utf16_offset},
};

actions!(ligature_editor, [SelectAll, Copy, Cut, Paste, Undo, Redo]);

pub fn init(cx: &mut App) {
    let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    cx.bind_keys([
        KeyBinding::new(&format!("{modifier}-a"), SelectAll, Some("LigatureEditor")),
        KeyBinding::new(&format!("{modifier}-c"), Copy, Some("LigatureEditor")),
        KeyBinding::new(&format!("{modifier}-x"), Cut, Some("LigatureEditor")),
        KeyBinding::new(&format!("{modifier}-v"), Paste, Some("LigatureEditor")),
        KeyBinding::new(&format!("{modifier}-z"), Undo, Some("LigatureEditor")),
        KeyBinding::new(&format!("{modifier}-shift-z"), Redo, Some("LigatureEditor")),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([KeyBinding::new("ctrl-y", Redo, Some("LigatureEditor"))]);
}

pub struct Editor {
    state: EditorState,
    focus: FocusHandle,
    bounds: Bounds<Pixels>,
    scroll: Point<Pixels>,
    gutter: Pixels,
    font: Option<Font>,
    font_family: Option<SharedString>,
    font_size: Pixels,
    line_height: Pixels,
    preferred_x: Option<Pixels>,
    reveal_caret: bool,
    dragging: bool,
}

impl Editor {
    pub fn new(buffer: Buffer, cx: &mut Context<Self>) -> Self {
        Self {
            state: EditorState::new(buffer),
            focus: cx.focus_handle().tab_stop(true),
            bounds: Bounds::default(),
            scroll: point(px(0.0), px(0.0)),
            gutter: px(48.0),
            font: None,
            font_family: None,
            font_size: typography::editor_size(),
            line_height: typography::editor_line_height(),
            preferred_x: None,
            reveal_caret: true,
            dragging: false,
        }
    }

    pub fn state(&self) -> &EditorState {
        &self.state
    }

    pub fn set_buffer(&mut self, buffer: Buffer, cx: &mut Context<Self>) {
        self.state = EditorState::new(buffer);
        self.scroll = point(px(0.0), px(0.0));
        self.preferred_x = None;
        self.reveal_caret = true;
        cx.notify();
    }

    pub fn update_state<R>(&mut self, update: impl FnOnce(&mut EditorState) -> R, cx: &mut Context<Self>) -> R {
        let result = update(&mut self.state);
        self.preferred_x = None;
        self.reveal_caret = true;
        cx.notify();
        result
    }

    pub fn set_font(&mut self, family: Option<SharedString>, font_size: Pixels, cx: &mut Context<Self>) {
        self.font_family = family;
        self.font_size = font_size.max(px(8.0));
        self.line_height = self.font_size * 1.5;
        self.font = None;
        self.preferred_x = None;
        self.reveal_caret = true;
        cx.notify();
    }

    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_caret = true;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if self.state.marked_range().is_some() {
            if key != "escape" {
                return;
            }
            self.state.cancel_composition();
        } else {
            let document = modifiers.secondary();
            let word = if cfg!(target_os = "macos") { modifiers.alt } else { modifiers.control };
            let movement = match key {
                "left" if document && cfg!(target_os = "macos") => Some(Movement::LineStart),
                "right" if document && cfg!(target_os = "macos") => Some(Movement::LineEnd),
                "left" if word => Some(Movement::WordLeft),
                "right" if word => Some(Movement::WordRight),
                "left" => Some(Movement::Left),
                "right" => Some(Movement::Right),
                "up" if document => Some(Movement::DocumentStart),
                "down" if document => Some(Movement::DocumentEnd),
                "up" => Some(Movement::Up),
                "down" => Some(Movement::Down),
                "home" if document => Some(Movement::DocumentStart),
                "end" if document => Some(Movement::DocumentEnd),
                "home" => Some(Movement::LineStart),
                "end" => Some(Movement::LineEnd),
                _ => None,
            };
            if let Some(movement) = movement {
                if matches!(movement, Movement::Up | Movement::Down) {
                    let line = self.state.line_index(self.state.caret());
                    let shaped = self.shape(line, window);
                    let x = self.preferred_x.unwrap_or_else(|| shaped.x_for_byte(self.state.caret()));
                    let target = if matches!(movement, Movement::Up) {
                        line.saturating_sub(1)
                    } else {
                        (line + 1).min(self.state.line_count() - 1)
                    };
                    let offset = self.shape(target, window).byte_for_x(x);
                    self.state.move_to(offset, modifiers.shift);
                    self.preferred_x = Some(x);
                } else {
                    self.preferred_x = None;
                    self.state.move_cursor(movement, modifiers.shift);
                }
            } else {
                if modifiers.control || modifiers.platform || modifiers.alt {
                    return;
                }
                self.preferred_x = None;
                match key {
                    "backspace" => self.state.delete(true),
                    "delete" => self.state.delete(false),
                    "enter" => self.state.newline(),
                    "tab" => self.state.indent(),
                    _ => return,
                }
            }
        }
        window.prevent_default();
        cx.stop_propagation();
        self.changed(window, cx);
    }

    fn shape(&self, line: usize, window: &Window) -> LayoutLine {
        let range = self.state.line_range(line);
        let text = &self.state.text()[range.clone()];
        let tab_width = self.shape_text(" ".into(), window, false).width.max(px(1.0)) * self.state.tab_size() as f32;
        let mut pieces = Vec::new();
        let mut boundaries = Vec::new();
        let mut x = px(0.0);
        let mut byte = 0;
        for part in text.split('\t') {
            let shaped = self.shape_text(part.to_owned().into(), window, false);
            boundaries.extend(
                part.grapheme_indices(true)
                    .map(|(offset, _)| (byte + offset, x + shaped.x_for_index(offset))),
            );
            x += shaped.width;
            byte += part.len();
            boundaries.push((byte, x));
            pieces.push((x - shaped.width, shaped));
            if byte < text.len() {
                x = tab_width * ((x / tab_width).floor() + 1.0);
                byte += 1;
            }
        }
        LayoutLine {
            range,
            pieces,
            boundaries,
            width: x,
        }
    }

    fn shape_text(&self, text: SharedString, window: &Window, muted: bool) -> ShapedLine {
        let font = self.font.clone().unwrap_or_else(|| window.text_style().font());
        window.text_system().shape_line(
            text.clone(),
            self.font_size,
            &[TextRun {
                len: text.len(),
                font,
                color: if muted { colors().muted_text } else { colors().text },
                background_color: None,
                underline: None,
                strikethrough: None,
                letter_spacing: None,
            }],
            None,
        )
    }

    fn hit_test(&self, position: Point<Pixels>, window: &Window) -> usize {
        let y = f32::from(position.y - self.bounds.top() + self.scroll.y).max(0.0);
        let line = ((y / f32::from(self.line_height)).floor() as usize).min(self.state.line_count() - 1);
        self.shape(line, window)
            .byte_for_x(position.x - self.bounds.left() - self.gutter + self.scroll.x)
    }

    fn mouse_down(&mut self, event: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.state.commit_composition();
        let offset = self.hit_test(event.position, window);
        if event.click_count >= 3 {
            let line = self.state.line_index(offset);
            let range = self.state.line_range(line);
            let end = if line + 1 < self.state.line_count() {
                self.state.line_range(line + 1).start
            } else {
                range.end
            };
            self.state.select(range.start, end);
        } else if event.click_count == 2 {
            self.state.select_word(offset);
        } else {
            self.state.move_to(offset, event.modifiers.shift);
        }
        self.dragging = true;
        self.preferred_x = None;
        self.changed(window, cx);
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &gpui::MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !event.dragging() {
            self.dragging = false;
        }
        if !self.dragging {
            return;
        }
        let y = event.position.y;
        if y < self.bounds.top() {
            self.scroll.y = (self.scroll.y - self.line_height).max(px(0.0));
        }
        if y > self.bounds.bottom() {
            self.scroll.y += self.line_height;
        }
        if event.position.x < self.bounds.left() + self.gutter {
            self.scroll.x = (self.scroll.x - px(20.0)).max(px(0.0));
        }
        if event.position.x > self.bounds.right() {
            self.scroll.x += px(20.0);
        }
        let offset = self.hit_test(event.position, window);
        self.state.move_to(offset, true);
        self.changed(window, cx);
    }

    fn scroll(&mut self, event: &gpui::ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(self.line_height);
        let delta = if event.modifiers.shift && delta.x == px(0.0) {
            point(delta.y, px(0.0))
        } else {
            delta
        };
        self.scroll.x = (self.scroll.x - delta.x).max(px(0.0));
        self.scroll.y = (self.scroll.y - delta.y).max(px(0.0));
        self.reveal_caret = false;
        cx.stop_propagation();
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.state.select_all();
        self.preferred_x = None;
        self.changed(window, cx);
    }

    fn copy(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.state.copy() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        self.changed(window, cx);
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.state.cut() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        self.preferred_x = None;
        self.changed(window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.state.insert(&text, false);
        }
        self.preferred_x = None;
        self.changed(window, cx);
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.state.undo();
        self.preferred_x = None;
        self.changed(window, cx);
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.state.redo();
        self.preferred_x = None;
        self.changed(window, cx);
    }
}

struct LayoutLine {
    range: Range<usize>,
    pieces: Vec<(Pixels, ShapedLine)>,
    boundaries: Vec<(usize, Pixels)>,
    width: Pixels,
}

impl LayoutLine {
    fn x_for_byte(&self, byte: usize) -> Pixels {
        let column = byte.saturating_sub(self.range.start).min(self.range.len());
        let index = self.boundaries.partition_point(|&(byte, _)| byte <= column).saturating_sub(1);
        self.boundaries[index].1
    }

    fn byte_for_x(&self, x: Pixels) -> usize {
        let boundary = self
            .boundaries
            .iter()
            .min_by(|a, b| {
                let a = f32::from(a.1 - x).abs();
                let b = f32::from(b.1 - x).abs();
                a.total_cmp(&b)
            })
            .expect("line has an end boundary");
        self.range.start + boundary.0
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, _window: &mut Window, _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.byte_range(range);
        *actual = Some(self.utf16_range(range.clone()));
        Some(self.state.text()[range].to_owned())
    }

    fn selected_text_range(&mut self, _ignore_disabled: bool, _window: &mut Window, _cx: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.utf16_range(self.state.selection()),
            reversed: self.state.caret() < self.state.anchor(),
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.state.marked_range().map(|range| self.utf16_range(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.commit_composition();
        self.changed(window, cx);
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let range = range.map(|range| self.byte_range(range));
        self.state.commit_text(range, text);
        self.preferred_x = None;
        self.changed(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self, range: Option<Range<usize>>, text: &str, selected: Option<Range<usize>>, window: &mut Window, cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| self.byte_range(range));
        self.state.mark(range, text, selected);
        self.preferred_x = None;
        self.changed(window, cx);
    }

    fn bounds_for_range(
        &mut self, range: Range<usize>, _bounds: Bounds<Pixels>, window: &mut Window, _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.byte_range(range);
        let line = self.state.line_index(range.start);
        let shaped = self.shape(line, window);
        let start = shaped.x_for_byte(range.start);
        let end = shaped.x_for_byte(range.end.min(shaped.range.end));
        Some(Bounds::new(
            point(
                self.bounds.left() + self.gutter + start - self.scroll.x,
                self.bounds.top() + self.line_height * line as f32 - self.scroll.y,
            ),
            size((end - start).max(px(1.0)), self.line_height),
        ))
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        Some(utf16_offset(self.state.text(), self.hit_test(point, window)))
    }
}

impl Editor {
    fn byte_range(&self, range: Range<usize>) -> Range<usize> {
        let start = byte_offset(self.state.text(), range.start);
        start..byte_offset(self.state.text(), range.end).max(start)
    }

    fn utf16_range(&self, range: Range<usize>) -> Range<usize> {
        utf16_offset(self.state.text(), range.start)..utf16_offset(self.state.text(), range.end)
    }

    fn layout(&mut self, bounds: Bounds<Pixels>, window: &mut Window) -> Vec<(usize, LayoutLine, ShapedLine)> {
        let resized = self.bounds.size != bounds.size;
        self.bounds = bounds;
        self.font = Some(window.text_style().font());
        let digits = self.state.line_count().to_string();
        self.gutter = self.shape_text(digits.into(), window, true).width + px(24.0);
        let caret_line = self.state.line_index(self.state.caret());
        let caret = self.shape(caret_line, window);
        let max_y = (self.line_height * self.state.line_count() as f32 - bounds.size.height).max(px(0.0));
        if self.reveal_caret || resized {
            let y = self.line_height * caret_line as f32;
            self.scroll.y = reveal(self.scroll.y, bounds.size.height, y, self.line_height);
            self.scroll.x = reveal(
                self.scroll.x,
                (bounds.size.width - self.gutter).max(px(1.0)),
                caret.x_for_byte(self.state.caret()),
                px(2.0),
            );
        }
        self.scroll.y = self.scroll.y.clamp(px(0.0), max_y);
        let first = (f32::from(self.scroll.y) / f32::from(self.line_height)).floor() as usize;
        let count = (f32::from(bounds.size.height) / f32::from(self.line_height)).ceil() as usize + 1;
        let lines: Vec<_> = (first..(first + count).min(self.state.line_count()))
            .map(|line| {
                (
                    line,
                    self.shape(line, window),
                    self.shape_text((line + 1).to_string().into(), window, true),
                )
            })
            .collect();
        let width = lines.iter().map(|(_, line, _)| line.width).fold(caret.width, Pixels::max);
        self.scroll.x = self.scroll.x.min((width + px(8.0) - (bounds.size.width - self.gutter)).max(px(0.0)));
        self.reveal_caret = false;
        lines
    }

    fn paint(&self, lines: Vec<(usize, LayoutLine, ShapedLine)>, window: &mut Window, cx: &mut App) {
        let colors = colors();
        let text_bounds = Bounds::new(
            point(self.bounds.left() + self.gutter, self.bounds.top()),
            size((self.bounds.size.width - self.gutter).max(px(0.0)), self.bounds.size.height),
        );
        let selection = self.state.selection();
        let marked = self.state.marked_range();
        let caret_line = self.state.line_index(self.state.caret());
        window.with_content_mask(Some(gpui::ContentMask { bounds: text_bounds }), |window| {
            for (index, line, _) in &lines {
                let origin = point(
                    text_bounds.left() - self.scroll.x,
                    self.bounds.top() + self.line_height * *index as f32 - self.scroll.y,
                );
                let line_end = if index + 1 < self.state.line_count() {
                    self.state.line_range(index + 1).start
                } else {
                    line.range.end
                };
                if selection.start < line_end && selection.end > line.range.start {
                    let start = line.x_for_byte(selection.start.max(line.range.start));
                    let end = line.x_for_byte(selection.end.min(line.range.end)) + if selection.end > line.range.end { px(8.0) } else { px(0.0) };
                    window.paint_quad(fill(
                        Bounds::new(point(origin.x + start, origin.y), size((end - start).max(px(1.0)), self.line_height)),
                        colors.selection,
                    ));
                }
                for (x, shaped) in &line.pieces {
                    shaped
                        .paint(point(origin.x + *x, origin.y), self.line_height, gpui::TextAlign::Left, None, window, cx)
                        .expect("editor text painting failed");
                }
                if let Some(marked) = &marked
                    && marked.start <= line.range.end
                    && marked.end >= line.range.start
                {
                    let start = line.x_for_byte(marked.start.max(line.range.start));
                    let end = line.x_for_byte(marked.end.min(line.range.end));
                    window.paint_quad(fill(
                        Bounds::new(
                            point(origin.x + start, origin.y + self.line_height - px(2.0)),
                            size((end - start).max(px(1.0)), px(1.0)),
                        ),
                        colors.text,
                    ));
                }
                if *index == caret_line && self.focus.is_focused(window) {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(origin.x + line.x_for_byte(self.state.caret()), origin.y),
                            size(px(1.0), self.line_height),
                        ),
                        colors.text,
                    ));
                }
            }
        });
        window.with_content_mask(Some(gpui::ContentMask { bounds: self.bounds }), |window| {
            window.paint_quad(fill(
                Bounds::new(self.bounds.origin, size(self.gutter, self.bounds.size.height)),
                colors.background,
            ));
            for (index, _, number) in lines {
                let origin = point(
                    self.bounds.left() + self.gutter - px(12.0) - number.width,
                    self.bounds.top() + self.line_height * index as f32 - self.scroll.y,
                );
                number
                    .paint(origin, self.line_height, gpui::TextAlign::Left, None, window, cx)
                    .expect("gutter painting failed");
            }
        });
    }
}

fn reveal(scroll: Pixels, viewport: Pixels, position: Pixels, extent: Pixels) -> Pixels {
    if position < scroll {
        position
    } else if position + extent > scroll + viewport {
        (position + extent - viewport).max(px(0.0))
    } else {
        scroll
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let paint_entity = entity.clone();
        div()
            .id("editor")
            .debug_selector(|| "editor".into())
            .key_context("LigatureEditor")
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .cursor(CursorStyle::IBeam)
            .bg(colors().background)
            .text_color(colors().text)
            .text_size(self.font_size)
            .line_height(self.line_height)
            .when_some(self.font_family.clone(), |view, family| view.font_family(family))
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .child(
                canvas(
                    move |bounds, window, cx| entity.update(cx, |editor, _| editor.layout(bounds, window)),
                    move |bounds, lines, window, cx| {
                        let mouse_entity = paint_entity.clone();
                        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                            if phase == gpui::DispatchPhase::Bubble {
                                mouse_entity.update(cx, |editor, cx| editor.mouse_move(event, window, cx));
                            }
                        });
                        paint_entity.update(cx, |editor, cx| {
                            window.handle_input(&editor.focus, ElementInputHandler::new(bounds, paint_entity.clone()), cx);
                            editor.paint(lines, window, cx);
                        });
                    },
                )
                .size_full(),
            )
    }
}
