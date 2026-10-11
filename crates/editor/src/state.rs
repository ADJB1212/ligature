use std::ops::Range;

use ligature_buffer::{Buffer, ByteOffset, Position, SavePoint};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indentation {
    Tabs,
    Spaces,
}

#[derive(Clone, Copy, Debug)]
pub enum Movement {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
    WordLeft,
    WordRight,
}

struct Composition {
    original: Range<usize>,
    original_selection: (usize, usize),
    display: String,
    line_starts: Vec<usize>,
    replacement: Range<usize>,
    marked: Range<usize>,
}

pub struct EditorState {
    buffer: Buffer,
    anchor: usize,
    caret: usize,
    preferred_column: Option<usize>,
    composition: Option<Composition>,
    line_ending: LineEnding,
    indentation: Indentation,
    tab_size: usize,
}

impl EditorState {
    pub fn new(buffer: Buffer) -> Self {
        let line_ending = if buffer.text().contains("\r\n") {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        Self {
            buffer,
            anchor: 0,
            caret: 0,
            preferred_column: None,
            composition: None,
            line_ending,
            indentation: Indentation::Spaces,
            tab_size: 4,
        }
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn save_snapshot(&mut self) -> (SavePoint, String) {
        self.commit_composition();
        (self.buffer.save_point(), self.buffer.text().to_owned())
    }

    pub fn mark_saved_at(&mut self, point: SavePoint) -> bool {
        self.buffer.mark_saved_at(point)
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty() || self.composition.is_some()
    }

    pub fn reload(&mut self, text: String) {
        self.reload_text(&text);
    }

    pub(crate) fn reload_text(&mut self, text: &str) {
        let anchor = self.anchor;
        let caret = self.caret;
        self.buffer.reload(text);
        self.composition = None;
        self.line_ending = if text.contains("\r\n") { LineEnding::CrLf } else { LineEnding::Lf };
        self.select(anchor, caret);
    }

    pub fn text(&self) -> &str {
        self.composition
            .as_ref()
            .map_or_else(|| self.buffer.text(), |composition| composition.display.as_str())
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn anchor(&self) -> usize {
        self.anchor
    }

    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    pub fn select_search_match(&mut self, range: Range<usize>) {
        self.commit_composition();
        self.buffer
            .text_range(ByteOffset(range.start), ByteOffset(range.end))
            .expect("valid search range");
        self.buffer.end_typing_group();
        self.anchor = range.start;
        self.caret = range.end;
        self.preferred_column = None;
    }

    pub fn selected_text(&self) -> &str {
        &self.text()[self.selection()]
    }

    pub fn tab_size(&self) -> usize {
        self.tab_size
    }

    pub fn set_indentation(&mut self, indentation: Indentation, tab_size: usize) {
        self.indentation = indentation;
        self.tab_size = tab_size.clamp(1, 16);
        self.preferred_column = None;
    }

    pub fn set_line_ending(&mut self, line_ending: LineEnding) {
        self.line_ending = line_ending;
    }

    pub fn select(&mut self, anchor: usize, caret: usize) {
        self.commit_composition();
        self.anchor = snap(self.text(), anchor);
        self.caret = snap(self.text(), caret);
        self.preferred_column = None;
        self.buffer.end_typing_group();
    }

    pub fn select_all(&mut self) {
        self.commit_composition();
        self.select(0, self.text().len());
    }

    pub fn select_word(&mut self, offset: usize) {
        self.commit_composition();
        let offset = snap(self.text(), offset);
        let range = self
            .text()
            .split_word_bound_indices()
            .find_map(|(start, word)| (start <= offset && offset < start + word.len()).then_some(start..start + word.len()))
            .unwrap_or(offset..offset);
        self.select(range.start, range.end);
    }

    pub fn move_to(&mut self, offset: usize, extend: bool) {
        let anchor = if extend { self.anchor } else { offset };
        self.select(anchor, offset);
    }

    pub fn move_cursor(&mut self, movement: Movement, extend: bool) {
        self.commit_composition();
        let selection = self.selection();
        if !extend && !selection.is_empty() && matches!(movement, Movement::Left | Movement::Right) {
            self.move_to(
                if matches!(movement, Movement::Left) {
                    selection.start
                } else {
                    selection.end
                },
                false,
            );
            return;
        }
        let line = self.line_index(self.caret);
        let range = self.line_range(line);
        let mut preferred = None;
        let offset = match movement {
            Movement::Left => previous(self.text(), self.caret),
            Movement::Right => next(self.text(), self.caret),
            Movement::LineStart => range.start,
            Movement::LineEnd => range.end,
            Movement::DocumentStart => 0,
            Movement::DocumentEnd => self.text().len(),
            Movement::WordLeft => self.text()[..self.caret].unicode_word_indices().next_back().map_or(0, |(start, _)| start),
            Movement::WordRight => self.text()[self.caret..]
                .unicode_word_indices()
                .next()
                .map_or_else(|| self.text().len(), |(start, word)| self.caret + start + word.len()),
            Movement::Up | Movement::Down => {
                let column = self
                    .preferred_column
                    .unwrap_or_else(|| visual_column(&self.text()[range.start..self.caret], self.tab_size));
                preferred = Some(column);
                let target = if matches!(movement, Movement::Up) {
                    line.saturating_sub(1)
                } else {
                    (line + 1).min(self.line_count() - 1)
                };
                let range = self.line_range(target);
                range.start + byte_for_column(&self.text()[range], column, self.tab_size)
            }
        };
        self.move_to(offset, extend);
        self.preferred_column = preferred;
    }

    pub fn insert(&mut self, text: &str, typing: bool) {
        self.commit_composition();
        self.replace(self.selection(), text, typing);
    }

    pub fn replace(&mut self, range: Range<usize>, text: &str, typing: bool) {
        assert!(self.composition.is_none());
        let range = self
            .buffer
            .text_range(ByteOffset(range.start), ByteOffset(range.end))
            .expect("valid editor range");
        if typing && range.is_empty() {
            self.buffer.type_text(range.start(), text).expect("valid typing offset");
        } else {
            self.buffer.replace(range, text).expect("valid replacement range");
        }
        self.caret = range.start().0 + text.len();
        let boundary = snap(self.buffer.text(), self.caret);
        if boundary != self.caret {
            self.caret = next(self.buffer.text(), boundary);
        }
        self.anchor = self.caret;
        self.preferred_column = None;
    }

    pub fn newline(&mut self) {
        let text = match self.line_ending {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        };
        self.insert(text, false);
    }

    pub fn indent(&mut self) {
        self.commit_composition();
        let column = visual_column(
            &self.text()[self.line_range(self.line_index(self.caret)).start..self.caret],
            self.tab_size,
        );
        let text = match self.indentation {
            Indentation::Tabs => "\t".to_owned(),
            Indentation::Spaces => " ".repeat(self.tab_size - column % self.tab_size),
        };
        self.insert(&text, false);
    }

    pub fn delete(&mut self, backward: bool) {
        self.commit_composition();
        let mut range = self.selection();
        if range.is_empty() {
            if snap(self.text(), self.caret) != self.caret {
                range = previous(self.text(), self.caret)..next(self.text(), self.caret);
            } else if backward {
                range.start = previous(self.text(), self.caret);
            } else {
                range.end = next(self.text(), self.caret);
            }
        }
        self.replace(range, "", false);
    }

    pub fn cut(&mut self) -> Option<String> {
        self.commit_composition();
        if self.selection().is_empty() {
            return None;
        }
        let text = self.selected_text().to_owned();
        self.delete(false);
        Some(text)
    }

    pub fn copy(&mut self) -> Option<String> {
        self.commit_composition();
        self.buffer.end_typing_group();
        (!self.selection().is_empty()).then(|| self.selected_text().to_owned())
    }

    pub fn undo(&mut self) {
        if self.composition.is_some() {
            self.cancel_composition();
            return;
        }
        if let Some(edit) = self.buffer.undo() {
            self.select(edit.new_range.end().0, edit.new_range.end().0);
        }
    }

    pub fn redo(&mut self) {
        self.commit_composition();
        if let Some(edit) = self.buffer.redo() {
            self.select(edit.new_range.end().0, edit.new_range.end().0);
        }
    }

    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.composition.as_ref().map(|composition| composition.marked.clone())
    }

    pub fn mark(&mut self, range: Option<Range<usize>>, text: &str, selected: Option<Range<usize>>) {
        let range = range.or_else(|| self.marked_range()).unwrap_or_else(|| self.selection());
        let mut display = if let Some(composition) = &mut self.composition {
            std::mem::take(&mut composition.display)
        } else {
            self.buffer.text().to_owned()
        };
        display.replace_range(range.clone(), text);
        let mut line_starts = self
            .composition
            .as_mut()
            .map_or_else(Vec::new, |composition| std::mem::take(&mut composition.line_starts));
        line_starts.clear();
        line_starts.push(0);
        line_starts.extend(
            display
                .bytes()
                .enumerate()
                .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
        );
        let marked = range.start..range.start + text.len();
        if let Some(composition) = &mut self.composition {
            let original_start = map_to_original(range.start, &composition.replacement, &composition.original, false).min(composition.original.start);
            let original_end = map_to_original(range.end, &composition.replacement, &composition.original, true).max(composition.original.end);
            let replacement_end = if range.end >= composition.replacement.end {
                marked.end
            } else {
                composition.replacement.end - range.len() + text.len()
            };
            composition.replacement = composition.replacement.start.min(range.start)..replacement_end;
            composition.original = original_start..original_end;
            composition.display = display;
            composition.line_starts = line_starts;
            composition.marked = marked.clone();
        } else {
            self.buffer.end_typing_group();
            self.composition = Some(Composition {
                original: range,
                original_selection: (self.anchor, self.caret),
                display,
                line_starts,
                replacement: marked.clone(),
                marked: marked.clone(),
            });
        }
        let selected = selected.unwrap_or(text.len()..text.len());
        self.anchor = marked.start + byte_offset(text, selected.start);
        self.caret = marked.start + byte_offset(text, selected.end);
        self.preferred_column = None;
    }

    pub fn commit_text(&mut self, range: Option<Range<usize>>, text: &str) {
        if self.composition.is_some() {
            self.mark(range, text, None);
            self.commit_composition();
        } else {
            self.replace(range.unwrap_or_else(|| self.selection()), text, true);
        }
    }

    pub fn commit_composition(&mut self) {
        if let Some(composition) = self.composition.take() {
            let text = &composition.display[composition.replacement.clone()];
            let selection = (self.anchor, self.caret);
            self.replace(composition.original, text, false);
            (self.anchor, self.caret) = selection;
        }
    }

    pub fn cancel_composition(&mut self) {
        if let Some(composition) = self.composition.take() {
            (self.anchor, self.caret) = composition.original_selection;
        }
    }

    pub fn line_count(&self) -> usize {
        self.composition
            .as_ref()
            .map_or_else(|| self.buffer.line_count(), |composition| composition.line_starts.len())
    }

    pub fn line_index(&self, offset: usize) -> usize {
        self.composition.as_ref().map_or_else(
            || self.buffer.offset_to_position(ByteOffset(offset)).expect("valid caret").line,
            |composition| composition.line_starts.partition_point(|&start| start <= offset) - 1,
        )
    }

    pub fn line_range(&self, line: usize) -> Range<usize> {
        let (start, end) = if let Some(composition) = &self.composition {
            let start = composition.line_starts[line];
            let end = composition.line_starts.get(line + 1).map_or(composition.display.len(), |next| next - 1);
            (start, end)
        } else {
            let start = self.buffer.position_to_offset(Position { line, byte_column: 0 }).expect("valid line").0;
            (start, start + self.buffer.line(line).expect("valid line").len())
        };
        let end = if end > start && self.text().as_bytes()[end - 1] == b'\r' {
            end - 1
        } else {
            end
        };
        start..end
    }
}

fn map_to_original(offset: usize, marked: &Range<usize>, original: &Range<usize>, end: bool) -> usize {
    if offset < marked.start {
        offset
    } else if offset > marked.end {
        original.end + offset - marked.end
    } else if end {
        original.end
    } else {
        original.start
    }
}

pub(crate) fn previous(text: &str, offset: usize) -> usize {
    GraphemeCursor::new(offset, text.len(), true)
        .prev_boundary(text, 0)
        .expect("complete text")
        .unwrap_or(0)
}

pub(crate) fn next(text: &str, offset: usize) -> usize {
    GraphemeCursor::new(offset, text.len(), true)
        .next_boundary(text, 0)
        .expect("complete text")
        .unwrap_or(text.len())
}

pub(crate) fn snap(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let mut cursor = GraphemeCursor::new(offset, text.len(), true);
    if cursor.is_boundary(text, 0).expect("complete text") {
        offset
    } else {
        cursor.prev_boundary(text, 0).expect("complete text").unwrap_or(0)
    }
}

pub(crate) fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        if units + ch.len_utf16() > utf16 {
            return offset;
        }
        units += ch.len_utf16();
    }
    text.len()
}

pub(crate) fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

fn visual_column(text: &str, tab_size: usize) -> usize {
    text.graphemes(true).fold(0, |column, grapheme| {
        column + if grapheme == "\t" { tab_size - column % tab_size } else { 1 }
    })
}

fn byte_for_column(text: &str, target: usize, tab_size: usize) -> usize {
    let mut column = 0;
    for (index, grapheme) in text.grapheme_indices(true) {
        let width = if grapheme == "\t" { tab_size - column % tab_size } else { 1 };
        if target < column + width {
            return if target - column >= width.div_ceil(2) {
                index + grapheme.len()
            } else {
                index
            };
        }
        column += width;
    }
    text.len()
}
