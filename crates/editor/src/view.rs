use std::{ops::Range, sync::Arc, time::Duration};

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementInputHandler, EntityInputHandler, FocusHandle, Focusable, Font, IntoElement, KeyBinding,
    KeyDownEvent, MouseButton, Pixels, Point, Render, ShapedLine, SharedString, Task, TextRun, UTF16Selection, Window, actions, canvas, div, fill,
    point, prelude::*, px, size,
};
use ligature_buffer::Buffer;
use ligature_theme::{colors, typography};
use smallvec::SmallVec;
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    EditorState, Movement,
    state::{byte_offset, utf16_offset},
};

#[path = "assistance.rs"]
mod assistance;
pub use assistance::{Complete, ShowHover};

#[path = "highlights.rs"]
mod highlights;
use highlights::Highlights;

actions!(ligature_editor, [SelectAll, Copy, Cut, Paste, Undo, Redo]);

pub fn init(cx: &mut App) {
    let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    cx.bind_keys([
        KeyBinding::new("ctrl-space", Complete, Some("LigatureEditor")),
        KeyBinding::new("shift-f1", ShowHover, Some("LigatureEditor")),
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
    read_only: bool,
    focus: FocusHandle,
    bounds: Bounds<Pixels>,
    scroll: Point<Pixels>,
    gutter: Pixels,
    font: Option<Font>,
    font_family: Option<SharedString>,
    font_size: Pixels,
    line_height: Pixels,
    metrics: Option<Metrics>,
    preferred_x: Option<Pixels>,
    reveal_caret: bool,
    caret_on: bool,
    blink: Option<Task<()>>,
    blink_epoch: u64,
    dragging: bool,
    search_query: ligature_search::Query,
    search_error: Option<String>,
    search: Option<ligature_search::Search>,
    search_matches: Vec<Range<usize>>,
    search_limited: bool,
    search_revision: Option<(ligature_buffer::Revision, bool)>,
    diagnostics: Vec<(Range<usize>, ligature_diagnostics::Severity)>,
    diagnostic_revision: Option<ligature_buffer::Revision>,
    language: ligature_syntax::Language,
    syntax: Option<Arc<ligature_syntax::Snapshot>>,
    highlights: Option<Highlights>,
    structure: ligature_tex::Document,
    parse_requested: Option<(ligature_buffer::DocumentId, ligature_buffer::Revision, ligature_syntax::Language)>,
    parse_task: Option<Task<()>>,
    assistance: assistance::Assistance,
}

impl Editor {
    pub fn cancel_background_work(&mut self) {
        self.parse_task = None;
        self.parse_requested = None;
        self.blink = None;
        self.dismiss_assistance();
    }

    pub fn new(buffer: Buffer, cx: &mut Context<Self>) -> Self {
        Self::with_language(buffer, false, ligature_syntax::Language::Tex, cx)
    }

    pub fn read_only(buffer: Buffer, cx: &mut Context<Self>) -> Self {
        Self::with_language(buffer, true, ligature_syntax::Language::Plain, cx)
    }

    fn with_language(buffer: Buffer, read_only: bool, language: ligature_syntax::Language, cx: &mut Context<Self>) -> Self {
        let mut editor = Self {
            state: EditorState::new(buffer),
            read_only,
            focus: cx.focus_handle().tab_stop(true),
            bounds: Bounds::default(),
            scroll: point(px(0.0), px(0.0)),
            gutter: px(48.0),
            font: None,
            font_family: None,
            font_size: typography::editor_size(),
            line_height: typography::editor_line_height(),
            metrics: None,
            preferred_x: None,
            reveal_caret: true,
            caret_on: true,
            blink: None,
            blink_epoch: 0,
            dragging: false,
            search_query: ligature_search::Query::default(),
            search_error: None,
            search: None,
            search_matches: Vec::new(),
            search_limited: false,
            search_revision: None,
            diagnostics: Vec::new(),
            diagnostic_revision: None,
            language,
            syntax: None,
            highlights: None,
            structure: ligature_tex::Document::default(),
            parse_requested: None,
            parse_task: None,
            assistance: assistance::Assistance::default(),
        };
        editor.queue_parse(cx);
        editor.refresh_assistance(cx);
        editor
    }

    pub fn set_language(&mut self, language: ligature_syntax::Language, cx: &mut Context<Self>) {
        if self.language != language {
            self.dismiss_assistance();
            self.language = language;
            self.syntax = None;
            self.highlights = None;
            self.structure = ligature_tex::Document::default();
            self.parse_requested = None;
            self.queue_parse(cx);
            cx.notify();
        }
    }

    pub fn syntax_snapshot(&self) -> Option<&ligature_syntax::Snapshot> {
        self.syntax.as_deref().filter(|snapshot| {
            self.state.marked_range().is_none() && snapshot.is_current(self.state.buffer().id(), self.state.buffer().revision(), self.language)
        })
    }

    pub fn structure(&self) -> Option<&ligature_tex::Document> {
        self.syntax_snapshot()
            .filter(|snapshot| snapshot.language == ligature_syntax::Language::Tex)
            .map(|_| &self.structure)
    }

    fn queue_parse(&mut self, cx: &mut Context<Self>) {
        if self.language == ligature_syntax::Language::Plain || self.state.marked_range().is_some() {
            self.parse_task = None;
            self.parse_requested = None;
            return;
        }
        let key = (self.state.buffer().id(), self.state.buffer().revision(), self.language);
        if self.parse_requested == Some(key) {
            return;
        }
        self.parse_requested = Some(key);
        let text: Arc<str> = self.state.buffer().text().into();
        if let Some(highlights) = &mut self.highlights {
            if highlights.document == key.0 && highlights.language == key.2 {
                highlights.update(Arc::clone(&text));
            } else {
                self.highlights = None;
            }
        }
        let previous = self.syntax.clone();
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let (snapshot, structure) = cx
                .background_executor()
                .spawn(async move {
                    let snapshot = if let Some(previous) = previous.filter(|snapshot| snapshot.document == key.0 && snapshot.language == key.2) {
                        previous.refresh(key.1, text)
                    } else {
                        ligature_syntax::Snapshot::parse(key.0, key.1, key.2, text)
                    };
                    let structure = if key.2 == ligature_syntax::Language::Tex {
                        ligature_tex::parse(&snapshot)
                    } else {
                        ligature_tex::Document::default()
                    };
                    (Arc::new(snapshot), structure)
                })
                .await;
            this.update(cx, |editor, cx| {
                if editor.parse_requested == Some(key)
                    && editor.state.marked_range().is_none()
                    && snapshot.is_current(editor.state.buffer().id(), editor.state.buffer().revision(), editor.language)
                {
                    editor.highlights = Some(Highlights::new(&snapshot));
                    editor.syntax = Some(snapshot);
                    editor.structure = structure;
                    editor.suppress_opaque_assistance();
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    pub fn state(&self) -> &EditorState {
        &self.state
    }

    pub fn set_read_only_text(&mut self, text: &str, cx: &mut Context<Self>) {
        assert!(self.read_only);
        if self.state.text() != text {
            self.state.reload_text(text);
            self.preferred_x = None;
            cx.notify();
        }
    }

    pub fn set_diagnostics<'a>(
        &mut self, diagnostics: impl IntoIterator<Item = &'a ligature_diagnostics::Diagnostic>, stale: bool, cx: &mut Context<Self>,
    ) {
        let mut messages = Vec::new();
        let mut ranges = Vec::new();
        if !stale {
            for diagnostic in diagnostics {
                if let Some(range) = self.diagnostic_byte_range(diagnostic) {
                    messages.push((range.clone(), diagnostic.message.clone()));
                    ranges.push((range, diagnostic.severity));
                }
            }
        }
        self.assistance.build_messages = messages;
        let revision = self.state.buffer().revision();
        if self.diagnostics != ranges || self.diagnostic_revision != Some(revision) {
            self.diagnostics = ranges;
            self.diagnostic_revision = Some(revision);
            cx.notify();
        }
    }

    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll
    }

    fn diagnostic_byte_range(&self, diagnostic: &ligature_diagnostics::Diagnostic) -> Option<Range<usize>> {
        diagnostic.range.as_ref()?.byte_range_with_lines(self.state.text(), |line| {
            (line > 0 && line <= self.state.line_count()).then(|| {
                let start = self.state.line_range(line - 1).start;
                let end = if line < self.state.line_count() {
                    self.state.line_range(line).start - 1
                } else {
                    self.state.text().len()
                };
                start..end
            })
        })
    }

    pub fn set_buffer(&mut self, buffer: Buffer, cx: &mut Context<Self>) {
        self.dismiss_assistance();
        self.set_server_diagnostics(Vec::new(), cx);
        self.state = EditorState::new(buffer);
        self.scroll = point(px(0.0), px(0.0));
        self.preferred_x = None;
        self.reveal_caret = true;
        self.search_revision = None;
        self.diagnostics.clear();
        self.diagnostic_revision = None;
        self.refresh_search();
        self.queue_parse(cx);
        self.refresh_assistance(cx);
        cx.notify();
    }

    pub fn update_state<R>(&mut self, update: impl FnOnce(&mut EditorState) -> R, cx: &mut Context<Self>) -> R {
        let result = update(&mut self.state);
        self.preferred_x = None;
        self.reveal_caret = true;
        self.refresh_search();
        self.queue_parse(cx);
        self.refresh_assistance(cx);
        cx.notify();
        result
    }

    pub fn set_search(&mut self, query: ligature_search::Query, cx: &mut Context<Self>) -> Result<(), String> {
        if self.search_query == query {
            return self.search_error.clone().map_or(Ok(()), Err);
        }
        match ligature_search::Search::new(&query) {
            Ok(search) => {
                self.search = Some(search);
                self.search_error = None;
            }
            Err(error) => {
                self.search = None;
                self.search_error = Some(error);
            }
        }
        self.search_query = query;
        self.search_revision = None;
        self.refresh_search();
        cx.notify();
        self.search_error.clone().map_or(Ok(()), Err)
    }

    fn refresh_search(&mut self) {
        let revision = (self.state.buffer().revision(), self.state.marked_range().is_some());
        if self.search_revision == Some(revision) && !revision.1 {
            return;
        }
        self.search_matches = self.search.as_ref().map_or_else(Vec::new, |search| {
            search.find_limited(
                self.state.text(),
                &ligature_search::Cancellation::default(),
                ligature_search::MAX_DOCUMENT_MATCHES + 1,
            )
        });
        self.search_limited = self.search_matches.len() > ligature_search::MAX_DOCUMENT_MATCHES;
        self.search_matches.truncate(ligature_search::MAX_DOCUMENT_MATCHES);
        if self.search.is_some() {
            self.search_error = self
                .search_limited
                .then(|| "Document search reached its retention limit. Narrow the search before replacing all matches.".into());
        }
        self.search_revision = Some(revision);
    }

    pub fn search_matches(&self) -> &[Range<usize>] {
        &self.search_matches
    }

    pub fn search_status(&self) -> (Option<usize>, usize) {
        let selection = self.state.selection();
        (
            self.search_matches
                .binary_search_by_key(&selection.start, |range| range.start)
                .ok()
                .filter(|&index| self.search_matches[index] == selection),
            self.search_matches.len(),
        )
    }

    pub fn navigate_match(&mut self, backwards: bool, cx: &mut Context<Self>) {
        self.state.commit_composition();
        self.refresh_search();
        if let Some(index) = ligature_search::navigate(&self.search_matches, &self.state.selection(), self.state.caret(), backwards) {
            let range = self.search_matches[index].clone();
            self.update_state(|state| state.select_search_match(range), cx);
        }
    }

    pub fn replace_match(&mut self, replacement: &str, all: bool, cx: &mut Context<Self>) -> usize {
        if self.read_only {
            return 0;
        }
        self.state.commit_composition();
        self.refresh_search();
        let matches = if all {
            if self.search_limited {
                return 0;
            }
            self.search_matches.as_slice()
        } else {
            self.search_status()
                .0
                .or_else(|| ligature_search::navigate(&self.search_matches, &self.state.selection(), self.state.caret(), false))
                .map_or(&[][..], |index| &self.search_matches[index..index + 1])
        };
        let Some(edit) = ligature_search::replacement_edit(self.state.text(), matches, replacement) else {
            return 0;
        };
        let count = edit.count;
        self.update_state(|state| state.replace(edit.range, &edit.text, false), cx);
        count
    }

    pub fn set_font(&mut self, family: Option<SharedString>, font_size: Pixels, cx: &mut Context<Self>) {
        self.font_family = family;
        self.font_size = font_size.max(px(8.0));
        self.line_height = self.font_size * 1.5;
        self.font = None;
        self.metrics = None;
        self.preferred_x = None;
        self.reveal_caret = true;
        cx.notify();
    }

    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_caret = true;
        self.refresh_search();
        self.queue_parse(cx);
        self.refresh_assistance(cx);
        window.focus(&self.focus, cx);
        self.restart_blink(cx);
        cx.notify();
    }

    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.caret_on = true;
        self.blink_epoch += 1;
        let epoch = self.blink_epoch;
        self.blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                let cont = this
                    .update(cx, |editor, cx| {
                        if editor.blink_epoch != epoch {
                            return false;
                        }
                        editor.caret_on = !editor.caret_on;
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
            self.dismiss_assistance();
            self.blink = None;
            self.blink_epoch += 1;
            self.caret_on = false;
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistance_key(event, window, cx) {
            return;
        }
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
                    let x = self
                        .preferred_x
                        .unwrap_or_else(|| self.shape(line, window).x_for_byte(self.state.caret()));
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
                if self.read_only {
                    return;
                }
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

    fn measure(&self, window: &Window) -> Metrics {
        self.metrics.unwrap_or_else(|| Metrics {
            space: self.shape_text(" ".into(), window, false).width.max(px(1.0)),
            digit: (0..=9)
                .map(|digit| self.shape_text(digit.to_string().into(), window, true).width)
                .fold(px(0.0), Pixels::max),
        })
    }

    fn shape(&self, line: usize, window: &Window) -> LayoutLine {
        let range = self.state.line_range(line);
        let text = &self.state.text()[range.clone()];
        let tab_width = self.measure(window).space * self.state.tab_size() as f32;
        let mut pieces = Vec::new();
        let mut boundaries = Vec::new();
        let mut x = px(0.0);
        let mut byte = 0;
        for part in text.split('\t') {
            let shaped = self.shape_source(part.to_owned().into(), range.start + byte, window);
            let mut glyphs = shaped.runs.iter().flat_map(|run| &run.glyphs).peekable();
            for (offset, _) in part.grapheme_indices(true) {
                while glyphs.peek().is_some_and(|glyph| glyph.index < offset) {
                    glyphs.next();
                }
                boundaries.push((byte + offset, x + glyphs.peek().map_or(shaped.width, |glyph| glyph.position.x)));
            }
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

    fn shape_source(&self, text: SharedString, start: usize, window: &Window) -> ShapedLine {
        let Some(highlights) = self.highlights.as_ref().filter(|_| self.state.marked_range().is_none()) else {
            return self.shape_text(text, window, false);
        };
        let end = start + text.len();
        let mut runs = SmallVec::<[TextRun; 4]>::new();
        let mut cursor = start;
        let first = highlights.tokens.partition_point(|token| token.range.end <= start);
        let font = self.font.clone().unwrap_or_else(|| window.text_style().font());
        let mut append = |len, category| {
            if len == 0 {
                return;
            }
            runs.push(TextRun {
                len,
                font: font.clone(),
                color: ligature_theme::syntax_color(category),
                background_color: None,
                underline: None,
                strikethrough: None,
                letter_spacing: None,
            });
        };
        for token in highlights.tokens[first..].iter().take_while(|token| token.range.start < end) {
            let token_start = token.range.start.max(cursor);
            append(token_start - cursor, ligature_syntax::Category::Text);
            let token_end = token.range.end.min(end);
            append(token_end - token_start, token.category);
            cursor = token_end;
        }
        append(end - cursor, ligature_syntax::Category::Text);
        window.text_system().shape_line(text, self.font_size, &runs, None)
    }

    fn hit_test(&self, position: Point<Pixels>, window: &Window) -> usize {
        let y = f32::from(position.y - self.bounds.top() + self.scroll.y).max(0.0);
        let line = ((y / f32::from(self.line_height)).floor() as usize).min(self.state.line_count() - 1);
        self.shape(line, window)
            .byte_for_x(position.x - self.bounds.left() - self.gutter + self.scroll.x)
    }

    fn mouse_down(&mut self, event: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_assistance();
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
        if self.read_only {
            cx.stop_propagation();
            return;
        }
        if let Some(text) = self.state.cut() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        self.preferred_x = None;
        self.changed(window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            cx.stop_propagation();
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
            && !self.replace_snippet(self.state.selection(), &text)
        {
            self.state.insert(&text, false);
        }
        self.preferred_x = None;
        self.changed(window, cx);
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            cx.stop_propagation();
            return;
        }
        self.state.undo();
        self.preferred_x = None;
        self.changed(window, cx);
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            cx.stop_propagation();
            return;
        }
        self.state.redo();
        self.preferred_x = None;
        self.changed(window, cx);
    }
}

#[derive(Clone, Copy)]
struct Metrics {
    space: Pixels,
    digit: Pixels,
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
        if self.read_only {
            return;
        }
        let range = range.map(|range| self.byte_range(range));
        if !self.replace_snippet(range.clone().unwrap_or_else(|| self.state.selection()), text) {
            self.state.commit_text(range, text);
        }
        self.preferred_x = None;
        self.changed(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self, range: Option<Range<usize>>, text: &str, selected: Option<Range<usize>>, window: &mut Window, cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
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
        let font = window.text_style().font();
        if self.font.as_ref() != Some(&font) {
            self.font = Some(font);
            self.metrics = None;
        }
        let metrics = self.measure(window);
        self.metrics = Some(metrics);
        self.gutter = if self.read_only {
            px(8.0)
        } else {
            let digits = self.state.line_count().checked_ilog10().map_or(1, |log| log as usize + 1).max(2);
            px(f32::from(metrics.digit * digits as f32 + px(24.0)).ceil())
        };
        let caret_line = self.state.line_index(self.state.caret());
        let caret = self.shape(caret_line, window);
        let caret_width = caret.width;
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
        let mut caret = Some(caret);
        let lines: Vec<_> = (first..(first + count).min(self.state.line_count()))
            .map(|line| {
                (
                    line,
                    if line == caret_line {
                        caret.take().expect("caret line is laid out once")
                    } else {
                        self.shape(line, window)
                    },
                    self.shape_text((line + 1).to_string().into(), window, true),
                )
            })
            .collect();
        let width = lines.iter().map(|(_, line, _)| line.width).fold(caret_width, Pixels::max);
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
        let diagnostics = if self.diagnostic_revision == Some(self.state.buffer().revision()) {
            self.diagnostics.as_slice()
        } else {
            &[]
        };
        window.with_content_mask(Some(gpui::ContentMask { bounds: text_bounds }), |window| {
            for (index, line, _) in &lines {
                let origin = point(
                    text_bounds.left() - self.scroll.x,
                    self.bounds.top() + self.line_height * *index as f32 - self.scroll.y,
                );
                if *index == caret_line && !self.read_only {
                    window.paint_quad(fill(
                        Bounds::new(point(text_bounds.left(), origin.y), size(text_bounds.size.width, self.line_height)),
                        colors.current_line,
                    ));
                }
                let line_end = if index + 1 < self.state.line_count() {
                    self.state.line_range(index + 1).start
                } else {
                    line.range.end
                };
                let first_match = self.search_matches.partition_point(|range| range.end <= line.range.start);
                for matched in self.search_matches[first_match..].iter().take_while(|range| range.start < line_end) {
                    let start = line.x_for_byte(matched.start.max(line.range.start));
                    let end = line.x_for_byte(matched.end.min(line.range.end));
                    window.paint_quad(fill(
                        Bounds::new(point(origin.x + start, origin.y), size((end - start).max(px(1.0)), self.line_height)),
                        colors.search_match,
                    ));
                }
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
                if marked.is_none() {
                    for (range, severity) in diagnostics
                        .iter()
                        .map(|(range, severity)| (range, *severity))
                        .chain(self.server_diagnostic_ranges())
                    {
                        if (range.start < line_end && range.end > line.range.start)
                            || (range.is_empty() && range.start >= line.range.start && range.start <= line.range.end)
                        {
                            let start = line.x_for_byte(range.start.max(line.range.start));
                            let end = line.x_for_byte(range.end.min(line.range.end));
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(origin.x + start, origin.y + self.line_height - px(2.0)),
                                    size((end - start).max(px(4.0)), px(1.0)),
                                ),
                                if severity == ligature_diagnostics::Severity::Error {
                                    colors.error
                                } else {
                                    colors.muted_text
                                },
                            ));
                        }
                    }
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
                if *index == caret_line && !self.read_only && self.focus.is_focused(window) && self.caret_on {
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
        if self.read_only {
            return;
        }
        window.with_content_mask(Some(gpui::ContentMask { bounds: self.bounds }), |window| {
            window.paint_quad(fill(
                Bounds::new(self.bounds.origin, size(self.gutter, self.bounds.size.height)),
                colors.gutter,
            ));
            window.paint_quad(fill(
                Bounds::new(
                    point(self.bounds.left() + self.gutter - px(1.0), self.bounds.top()),
                    size(px(1.0), self.bounds.size.height),
                ),
                colors.border,
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

const CARET_BLINK: Duration = Duration::from_millis(530);

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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_blink(window, cx);
        let assistance = self.render_assistance(window, cx);
        let entity = cx.entity();
        let paint_entity = entity.clone();
        div()
            .id("editor")
            .debug_selector(|| "editor".into())
            .key_context("LigatureEditor")
            .track_focus(&self.focus)
            .size_full()
            .relative()
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
            .when(!self.read_only && self.language != ligature_syntax::Language::Plain, |view| {
                view.on_action(cx.listener(Self::complete))
            })
            .when(self.hover_available(), |view| view.on_action(cx.listener(Self::show_hover)))
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
                            if !editor.read_only {
                                window.handle_input(&editor.focus, ElementInputHandler::new(bounds, paint_entity.clone()), cx);
                            }
                            editor.paint(lines, window, cx);
                        });
                    },
                )
                .size_full(),
            )
            .child(assistance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use ligature_buffer::Buffer;

    #[test]
    fn closing_retained_editors_cancels_parse_and_blink_tasks() {
        let mut cx = TestAppContext::single();
        let editor = cx.new(|cx| Editor::new(Buffer::from_text("\\section{Closed}"), cx));
        editor.update(&mut cx, |editor, cx| {
            editor.restart_blink(cx);
            assert!(editor.parse_task.is_some());
            assert!(editor.blink.is_some());
            editor.cancel_background_work();
        });
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            assert!(editor.parse_task.is_none());
            assert!(editor.parse_requested.is_none());
            assert!(editor.blink.is_none());
            assert!(editor.syntax_snapshot().is_none());
        });
    }

    #[test]
    fn limited_document_search_blocks_replace_all_and_allows_single_replacements() {
        let mut cx = TestAppContext::single();
        let text = "x".repeat(ligature_search::MAX_DOCUMENT_MATCHES + 1);
        let editor = cx.new(|cx| Editor::new(Buffer::from_text(&text), cx));
        editor.update(&mut cx, |editor, cx| {
            assert!(
                editor
                    .set_search(
                        ligature_search::Query {
                            text: "x".into(),
                            ..Default::default()
                        },
                        cx
                    )
                    .is_err()
            );
            assert_eq!(editor.search_matches().len(), ligature_search::MAX_DOCUMENT_MATCHES);
            assert_eq!(editor.replace_match("y", true, cx), 0);
            assert_eq!(editor.state.text(), text);
            assert_eq!(editor.replace_match("y", false, cx), 1);
            assert!(!editor.search_limited);
            assert!(editor.search_error.is_none());
            assert!(editor.state.text().starts_with('y'));
        });
    }

    #[test]
    fn read_only_logs_do_not_schedule_syntax_parsing() {
        let mut cx = TestAppContext::single();
        let editor = cx.new(|cx| Editor::read_only(Buffer::from_text("\\section{Build output}"), cx));
        editor.read_with(&cx, |editor, _| {
            assert!(editor.read_only);
            assert_eq!(editor.language, ligature_syntax::Language::Plain);
            assert!(editor.parse_requested.is_none());
            assert!(editor.parse_task.is_none());
        });
    }

    #[test]
    fn background_highlighting_preserves_selection_scroll_and_history() {
        let mut cx = TestAppContext::single();
        let editor = cx.new(|cx| Editor::new(Buffer::from_text("\\section{First}\nbody"), cx));
        editor.update(&mut cx, |editor, cx| {
            editor.update_state(|state| state.insert("% comment\n", false), cx);
            editor.state.select(12, 19);
            editor.scroll = point(px(8.0), px(24.0));
            editor.reveal_caret = false;
        });
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            let snapshot = editor.syntax_snapshot().unwrap();
            assert_eq!(snapshot.text.as_ref(), editor.state.text());
            assert_eq!(editor.state.selection(), 12..19);
            assert_eq!(editor.scroll, point(px(8.0), px(24.0)));
            assert!(!editor.reveal_caret);
            assert_eq!(editor.structure().unwrap().headings[0].title, "First");
            assert!(editor.state.is_dirty());
        });
        editor.update(&mut cx, |editor, cx| editor.update_state(|state| state.undo(), cx));
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.syntax_snapshot().unwrap().text.as_ref(), "\\section{First}\nbody");
            assert!(!editor.state.is_dirty());
        });
    }

    #[test]
    fn obsolete_parses_are_hidden_across_edits_reload_and_document_replacement() {
        let mut cx = TestAppContext::single();
        let editor = cx.new(|cx| Editor::new(Buffer::from_text("\\label{old}"), cx));
        cx.run_until_parked();
        editor.update(&mut cx, |editor, cx| {
            assert!(editor.syntax_snapshot().is_some());
            editor.update_state(|state| state.reload("\\label{new}".into()), cx);
            assert!(editor.syntax_snapshot().is_none());
            editor.set_buffer(Buffer::from_text("\\section{Replacement}"), cx);
            assert!(editor.syntax_snapshot().is_none());
        });
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.structure().unwrap().headings[0].title, "Replacement");
            assert!(editor.structure().unwrap().labels.is_empty());
        });
    }

    #[test]
    fn composition_and_language_changes_invalidate_highlights() {
        let mut cx = TestAppContext::single();
        let editor = cx.new(|cx| Editor::new(Buffer::from_text("\\label{key}"), cx));
        cx.run_until_parked();
        editor.update(&mut cx, |editor, cx| {
            editor.update_state(|state| state.mark(Some(0..0), "draft", None), cx);
            assert!(editor.syntax_snapshot().is_none());
            editor.update_state(|state| state.commit_composition(), cx);
        });
        cx.run_until_parked();
        editor.update(&mut cx, |editor, cx| {
            assert!(editor.syntax_snapshot().is_some());
            editor.set_language(ligature_syntax::Language::Bibliography, cx);
            editor.update_state(|state| state.reload("@book{key,title={Title}}".into()), cx);
            assert!(editor.syntax_snapshot().is_none());
        });
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            assert!(
                editor
                    .syntax_snapshot()
                    .unwrap()
                    .tokens
                    .iter()
                    .any(|token| token.category == ligature_syntax::Category::EntryType)
            );
            assert!(editor.structure().is_none());
        });
    }

    #[test]
    fn opaque_caret_lookup_preserves_comment_and_verbatim_boundaries() {
        let text = "text % comment\n\\verb|literal| tail\n\\begin{verbatim}body\\end{verbatim}";
        let mut cx = TestAppContext::single();
        let editor = cx.new(|cx| Editor::new(Buffer::from_text(text), cx));
        cx.run_until_parked();
        editor.update(&mut cx, |editor, _| {
            for caret in 0..=text.len() {
                editor.state.move_to(caret, false);
                let expected = editor.syntax_snapshot().unwrap().tokens.iter().any(|token| {
                    token.range.start < caret
                        && token.range.end >= caret
                        && matches!(token.category, ligature_syntax::Category::Comment | ligature_syntax::Category::Verbatim)
                });
                assert_eq!(editor.caret_is_opaque(), expected, "caret {caret}");
            }
        });
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn native_rendering_keeps_unchanged_braces_and_comments_identical_during_typing() {
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        let window = cx
            .open_window(size(px(640.0), px(480.0)), |_, cx| {
                cx.new(|cx| Editor::new(Buffer::from_text("\\section{Title}\n% comment\nbody"), cx))
            })
            .unwrap();
        let editor = window.root(&mut cx).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.update_state(|state| state.move_to(state.text().len(), false), cx);
            });
            window.focus(&editor.focus_handle(cx), cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        let before = cx.capture_screenshot(window.into()).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.replace_text_in_range(None, " typed", window, cx);
                assert!(editor.syntax_snapshot().is_none());
            });
            window.draw(cx).clear(cx);
        })
        .unwrap();
        let pending = cx.capture_screenshot(window.into()).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx)).unwrap();
        let parsed = cx.capture_screenshot(window.into()).unwrap();
        let rows = before.width() as usize * 4 * (before.height() as usize * 40 / 480);
        assert!(
            before.as_raw()[..rows] == pending.as_raw()[..rows],
            "unchanged text changed while parsing was pending"
        );
        assert!(
            before.as_raw()[..rows] == parsed.as_raw()[..rows],
            "unchanged text changed after parsing finished"
        );
    }

    #[test]
    fn typing_preserves_brace_highlights_while_semantic_results_are_stale() {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, cx| Editor::new(Buffer::from_text("\\section{Title}"), cx));
        let editor = window.root(&mut cx).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.state.move_to(10, false);
                for text in ["X", "猫", "Y"] {
                    editor.replace_text_in_range(None, text, window, cx);
                    assert!(editor.syntax_snapshot().is_none());
                    assert!(editor.structure().is_none());
                    let braces: Vec<_> = editor
                        .highlights
                        .as_ref()
                        .unwrap()
                        .tokens
                        .iter()
                        .filter(|token| token.category == ligature_syntax::Category::Brace)
                        .map(|token| token.range.clone())
                        .collect();
                    let closing = editor.state.text().find('}').unwrap();
                    assert_eq!(braces, [8..9, closing..closing + 1]);
                }
            });
        })
        .unwrap();
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            assert!(editor.syntax_snapshot().is_some());
            assert_eq!(editor.highlights.as_ref().unwrap().tokens, editor.syntax_snapshot().unwrap().tokens);
        });
    }

    #[test]
    fn line_number_gutter_stays_fixed_within_its_reserved_digit_width() {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, cx| Editor::new(Buffer::new(), cx));
        let editor = window.root(&mut cx).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| {
                let bounds = Bounds::new(Point::default(), size(px(640.0), px(480.0)));
                let mut gutter = None;
                for lines in [1, 8, 11, 88, 99] {
                    editor.update_state(|state| state.reload("\n".repeat(lines - 1)), cx);
                    editor.layout(bounds, window);
                    assert_eq!(*gutter.get_or_insert(editor.gutter), editor.gutter);
                }
            });
        })
        .unwrap();
    }

    #[test]
    fn syntax_publication_preserves_font_and_glyph_positions_while_typing() {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, cx| Editor::new(Buffer::new(), cx));
        let editor = window.root(&mut cx).unwrap();
        for (language, text) in [
            (ligature_syntax::Language::Tex, "\\documentclass{article} % office affine"),
            (ligature_syntax::Language::Bibliography, "@book{key, title={office affine}}"),
        ] {
            editor.update(&mut cx, |editor, cx| {
                editor.set_language(language, cx);
                editor.update_state(|state| state.reload(text.into()), cx);
            });
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                window.focus(&editor.focus_handle(cx), cx);
                window.draw(cx).clear(cx);
            })
            .unwrap();
            let pending = cx
                .update_window(window.into(), |_, window, cx| {
                    editor.update(cx, |editor, cx| {
                        editor.replace_text_in_range(None, "typed ", window, cx);
                        assert!(editor.syntax_snapshot().is_none());
                        editor.shape(0, window)
                    })
                })
                .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                editor.read_with(cx, |editor, _| {
                    assert!(editor.syntax_snapshot().is_some());
                    let highlighted = editor.shape(0, window);
                    assert_eq!(highlighted.width, pending.width);
                    assert_eq!(highlighted.boundaries, pending.boundaries);
                    for ((_, highlighted), (_, pending)) in highlighted.pieces.iter().zip(&pending.pieces) {
                        let glyphs = |line: &ShapedLine| {
                            line.runs
                                .iter()
                                .flat_map(|run| {
                                    run.glyphs
                                        .iter()
                                        .map(move |glyph| (run.font_id, glyph.id, glyph.position, glyph.index, glyph.is_emoji))
                                })
                                .collect::<Vec<_>>()
                        };
                        assert_eq!(glyphs(highlighted), glyphs(pending));
                    }
                });
            })
            .unwrap();
        }
    }

    #[test]
    fn grapheme_layout_preserves_shaped_caret_positions() {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, cx| Editor::new(Buffer::new(), cx));
        let editor = window.root(&mut cx).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |editor, _| {
                for text in ["", "office affine", "α😀e\u{301}猫", "abc אבג xyz"] {
                    editor.state = EditorState::new(Buffer::from_text(text));
                    let layout = editor.shape(0, window);
                    let shaped = editor.shape_text(text.to_owned().into(), window, false);
                    let expected: Vec<_> = text
                        .grapheme_indices(true)
                        .map(|(offset, _)| (offset, shaped.x_for_index(offset)))
                        .chain(std::iter::once((text.len(), shaped.width)))
                        .collect();
                    assert_eq!(layout.boundaries, expected, "{text}");
                }
            });
        })
        .unwrap();
    }

    #[test]
    fn read_only_text_can_be_selected_and_copied_but_cannot_be_edited() {
        let mut cx = TestAppContext::single();
        cx.update(init);
        let window = cx.add_window(|_, cx| Editor::read_only(Buffer::from_text("alpha\nbeta"), cx));
        let editor = window.root(&mut cx).unwrap();
        editor.update(&mut cx, |editor, cx| {
            editor.update_state(
                |state| {
                    state.insert("log ", false);
                    state.insert("unused", false);
                    state.undo();
                },
                cx,
            );
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.focus(&editor.focus_handle(cx), cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
        cx.simulate_keystrokes(window.into(), &format!("{modifier}-a {modifier}-c"));
        let expected = "log alpha\nbeta";
        assert_eq!(editor.read_with(&cx, |editor, _| editor.state.selected_text().to_owned()), expected);
        assert_eq!(
            cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text())).as_deref(),
            Some(expected)
        );
        for key in [
            "backspace".to_owned(),
            "delete".to_owned(),
            "enter".to_owned(),
            "tab".to_owned(),
            format!("{modifier}-x"),
            format!("{modifier}-v"),
            format!("{modifier}-z"),
            format!("{modifier}-shift-z"),
        ] {
            cx.simulate_keystrokes(window.into(), &key);
            assert_eq!(editor.read_with(&cx, |editor, _| editor.state.text().to_owned()), expected, "{key}");
        }
        cx.simulate_input(window.into(), "typed");
        assert_eq!(editor.read_with(&cx, |editor, _| editor.state.text().to_owned()), expected);
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.replace_text_in_range(None, "typed", window, cx);
                editor.replace_and_mark_text_in_range(None, "composed", Some(0..1), window, cx);
                assert_eq!(editor.state.text(), expected);
                assert!(editor.state.marked_range().is_none());
            });
        })
        .unwrap();
    }

    #[test]
    fn read_only_log_updates_preserve_selection_and_scroll() {
        let mut cx = TestAppContext::single();
        let editor = cx.new(|cx| Editor::read_only(Buffer::from_text("first\nsecond"), cx));
        editor.update(&mut cx, |editor, cx| {
            editor.state.select(6, 12);
            editor.scroll = point(px(10.0), px(20.0));
            editor.reveal_caret = false;
            editor.set_read_only_text("first\nsecond\nthird", cx);
            assert_eq!(editor.state.selected_text(), "second");
            assert_eq!(editor.scroll, point(px(10.0), px(20.0)));
            assert!(!editor.reveal_caret);
            assert!(!editor.state.is_dirty());
            editor.set_read_only_text("short", cx);
            assert_eq!(editor.state.selection(), 5..5);
        });
    }

    #[test]
    fn diagnostic_markers_follow_source_precision_and_clear_when_stale() {
        use ligature_diagnostics::{BuildId, Diagnostic, Provider, Severity, SourceRange};
        let mut cx = TestAppContext::single();
        let editor = cx.new(|cx| Editor::new(Buffer::from_text("first\nα😀last"), cx));
        let diagnostic = Diagnostic {
            severity: Severity::Error,
            provider: Provider::Build(BuildId(1)),
            message: "Error".into(),
            path: None,
            range: Some(SourceRange::Lines { start: 2, end: 2 }),
        };
        editor.update(&mut cx, |editor, cx| editor.set_diagnostics(std::slice::from_ref(&diagnostic), false, cx));
        editor.read_with(&cx, |editor, _| assert_eq!(editor.diagnostics, vec![(6..16, Severity::Error)]));
        editor.update(&mut cx, |editor, cx| editor.update_state(|state| state.insert("changed", false), cx));
        editor.read_with(&cx, |editor, _| {
            assert_ne!(editor.diagnostic_revision, Some(editor.state.buffer().revision()))
        });
        editor.update(&mut cx, |editor, cx| editor.set_diagnostics(&[diagnostic], true, cx));
        editor.read_with(&cx, |editor, _| assert!(editor.diagnostics.is_empty()));
    }

    #[test]
    fn focused_caret_blinks_and_reappears_after_movement() {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, cx| Editor::new(Buffer::from_text("ab"), cx));
        let editor = window.root(&mut cx).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.focus(&editor.focus_handle(cx), cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert!(editor.read_with(&cx, |editor, _| editor.caret_on));
        cx.executor().advance_clock(CARET_BLINK);
        cx.run_until_parked();
        assert!(!editor.read_with(&cx, |editor, _| editor.caret_on));
        cx.simulate_keystrokes(window.into(), "right");
        assert!(editor.read_with(&cx, |editor, _| editor.caret_on));
    }
}
