use std::sync::atomic::{AtomicU64, Ordering};

use crate::{ByteOffset, Position, PositionError, TextRange};
use crate::{Edit, history::Transaction};

static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

/// A process-local document identity independent of paths and text contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentId(u64);

/// A buffer-local revision, starting at zero and increasing on text changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavePoint {
    document: DocumentId,
    revision: Revision,
    state: Revision,
}

impl SavePoint {
    pub fn revision(self) -> Revision {
        self.revision
    }
}

impl Revision {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Contiguous UTF-8 storage with indexed line starts. Line lookup and slicing
/// borrow text; replacement moves the text suffix and updates affected line starts.
/// LF separates lines; CR is preserved as line content, including in CRLF text.
/// Every buffer has at least one line, and a final LF creates an empty last line.
#[derive(Debug)]
pub struct Buffer {
    id: DocumentId,
    revision: Revision,
    text: String,
    line_starts: Vec<usize>,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    typing_group: bool,
    current_state: Revision,
    saved_state: Revision,
    saved_revision: Revision,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    pub fn new() -> Self {
        Self::from_text("")
    }

    pub fn from_text(text: impl Into<String>) -> Self {
        let id = NEXT_DOCUMENT_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("document identities exhausted");
        let text = text.into();
        let line_starts = Self::index_lines(&text);
        Self {
            id: DocumentId(id),
            revision: Revision::default(),
            text,
            line_starts,
            undo: Vec::new(),
            redo: Vec::new(),
            typing_group: false,
            current_state: Revision::default(),
            saved_state: Revision::default(),
            saved_revision: Revision::default(),
        }
    }

    pub fn id(&self) -> DocumentId {
        self.id
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn reload(&mut self, text: &str) {
        if self.text != text {
            self.apply(TextRange::new(ByteOffset(0), ByteOffset(self.len())), text);
        }
        self.undo.clear();
        self.redo.clear();
        self.current_state = self.revision;
        self.mark_saved();
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Returns a borrowed line without its LF terminator.
    pub fn line(&self, line: usize) -> Result<&str, PositionError> {
        let (start, end) = self.line_bounds(line)?;
        Ok(&self.text[start..end])
    }

    pub fn validate_offset(&self, offset: ByteOffset) -> Result<(), PositionError> {
        if offset.0 > self.len() {
            return Err(PositionError::OffsetOutOfBounds { offset, len: self.len() });
        }
        if !self.text.is_char_boundary(offset.0) {
            return Err(PositionError::NotCharBoundary { offset });
        }
        Ok(())
    }

    pub fn text_range(&self, start: ByteOffset, end: ByteOffset) -> Result<TextRange, PositionError> {
        if start > end {
            return Err(PositionError::ReversedRange { start, end });
        }
        self.validate_offset(start)?;
        self.validate_offset(end)?;
        Ok(TextRange::new(start, end))
    }

    pub fn slice(&self, range: TextRange) -> Result<&str, PositionError> {
        self.text_range(range.start(), range.end())?;
        Ok(&self.text[range.start().0..range.end().0])
    }

    /// EOF is valid. An LF offset is the end of its preceding line.
    pub fn offset_to_position(&self, offset: ByteOffset) -> Result<Position, PositionError> {
        self.validate_offset(offset)?;
        let line = self.line_starts.partition_point(|&start| start <= offset.0) - 1;
        Ok(Position {
            line,
            byte_column: offset.0 - self.line_starts[line],
        })
    }

    /// The column may equal the line's byte length, but cannot cross its LF.
    pub fn position_to_offset(&self, position: Position) -> Result<ByteOffset, PositionError> {
        let (start, end) = self.line_bounds(position.line)?;
        if position.byte_column > end - start {
            return Err(PositionError::ColumnOutOfBounds {
                position,
                line_len: end - start,
            });
        }
        let offset = ByteOffset(start + position.byte_column);
        self.validate_offset(offset)?;
        Ok(offset)
    }

    /// Inserts one explicit transaction, including paste. Empty text is a no-op.
    pub fn insert(&mut self, offset: ByteOffset, text: &str) -> Result<Option<Edit>, PositionError> {
        let range = self.text_range(offset, offset)?;
        self.replace(range, text)
    }

    pub fn delete(&mut self, range: TextRange) -> Result<Option<Edit>, PositionError> {
        self.replace(range, "")
    }

    /// Revalidates the range and records one undo transaction. Identical
    /// replacements return `None` and preserve revision, dirty state, and redo.
    pub fn replace(&mut self, range: TextRange, text: &str) -> Result<Option<Edit>, PositionError> {
        self.edit(range, text, false)
    }

    /// Groups adjacent insertions until an explicit edit, save, undo, redo, or
    /// `end_typing_group`. Newlines are separate transactions and end the group.
    pub fn type_text(&mut self, offset: ByteOffset, text: &str) -> Result<Option<Edit>, PositionError> {
        let range = self.text_range(offset, offset)?;
        self.edit(range, text, !text.contains('\n'))
    }

    /// Call when cursor movement or another UI action interrupts typing.
    pub fn end_typing_group(&mut self) {
        self.typing_group = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> Option<Edit> {
        self.end_typing_group();
        let transaction = self.undo.pop()?;
        let range = TextRange::new(transaction.start, ByteOffset(transaction.start.0 + transaction.inserted.len()));
        let edit = self.apply(range, &transaction.removed);
        self.current_state = transaction.before_state;
        self.redo.push(transaction);
        Some(edit)
    }

    pub fn redo(&mut self) -> Option<Edit> {
        self.end_typing_group();
        let transaction = self.redo.pop()?;
        let range = TextRange::new(transaction.start, ByteOffset(transaction.start.0 + transaction.removed.len()));
        let edit = self.apply(range, &transaction.inserted);
        self.current_state = transaction.after_state;
        self.undo.push(transaction);
        Some(edit)
    }

    /// Marks current text as saved. Newly constructed buffers start clean.
    pub fn mark_saved(&mut self) {
        let point = self.save_point();
        self.mark_saved_at(point);
    }

    pub fn save_point(&mut self) -> SavePoint {
        self.end_typing_group();
        SavePoint {
            document: self.id,
            revision: self.revision,
            state: self.current_state,
        }
    }

    pub fn mark_saved_at(&mut self, point: SavePoint) -> bool {
        if point.document != self.id {
            return false;
        }
        self.saved_state = point.state;
        self.saved_revision = point.revision;
        true
    }

    /// The revision at the last save, which does not rewind during undo/redo.
    pub fn saved_revision(&self) -> Revision {
        self.saved_revision
    }

    /// Clean means the current history state is the saved state. Independently
    /// editing to identical contents does not restore the saved history state.
    pub fn is_dirty(&self) -> bool {
        self.current_state != self.saved_state
    }

    fn edit(&mut self, range: TextRange, text: &str, typing: bool) -> Result<Option<Edit>, PositionError> {
        let removed = self.slice(range)?;
        if removed == text {
            if !typing {
                self.end_typing_group();
            }
            return Ok(None);
        }
        let removed = removed.to_owned();
        if !typing {
            self.end_typing_group();
        }
        let before_state = self.current_state;
        let edit = self.apply(range, text);
        let merge = typing
            && self.typing_group
            && self
                .undo
                .last()
                .is_some_and(|transaction| transaction.removed.is_empty() && transaction.start.0 + transaction.inserted.len() == range.start().0);
        if merge {
            let transaction = self.undo.last_mut().expect("typing group has a transaction");
            transaction.inserted.push_str(text);
            transaction.after_state = self.revision;
        } else {
            self.undo.push(Transaction {
                start: range.start(),
                removed,
                inserted: text.to_owned(),
                before_state,
                after_state: self.revision,
            });
        }
        self.redo.clear();
        self.current_state = self.revision;
        self.typing_group = typing;
        Ok(Some(edit))
    }

    fn apply(&mut self, range: TextRange, text: &str) -> Edit {
        let old_revision = self.revision;
        let new_revision = Revision(old_revision.0.checked_add(1).expect("buffer revision exhausted"));
        self.text.replace_range(range.start().0..range.end().0, text);
        let start = range.start().0;
        let end = range.end().0;
        let first = self.line_starts.partition_point(|&offset| offset <= start);
        let last = self.line_starts.partition_point(|&offset| offset <= end);
        if text.len() != end - start {
            for offset in &mut self.line_starts[last..] {
                *offset = start + text.len() + (*offset - end);
            }
        }
        self.line_starts
            .splice(first..last, memchr::memchr_iter(b'\n', text.as_bytes()).map(|index| start + index + 1));
        self.revision = new_revision;
        Edit {
            old_range: range,
            new_range: TextRange::new(range.start(), ByteOffset(range.start().0 + text.len())),
            old_revision,
            new_revision,
        }
    }

    fn line_bounds(&self, line: usize) -> Result<(usize, usize), PositionError> {
        let start = *self.line_starts.get(line).ok_or_else(|| PositionError::LineOutOfBounds {
            line,
            line_count: self.line_count(),
        })?;
        let end = self.line_starts.get(line + 1).map_or(self.len(), |next| next - 1);
        Ok((start, end))
    }

    fn index_lines(text: &str) -> Vec<usize> {
        let mut starts = vec![0];
        starts.extend(memchr::memchr_iter(b'\n', text.as_bytes()).map(|index| index + 1));
        starts
    }
}

#[cfg(test)]
mod tests;
