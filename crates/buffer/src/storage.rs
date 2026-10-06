use std::sync::atomic::{AtomicU64, Ordering};

use crate::{ByteOffset, Position, PositionError, TextRange};

static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

/// A process-local document identity independent of paths and text contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentId(u64);

/// A buffer-local revision, starting at zero and increasing on text changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(u64);

impl Revision {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Contiguous UTF-8 storage with indexed line starts. Line lookup and slicing
/// borrow text; replacement moves the suffix and rebuilds the index in O(n).
/// LF separates lines; CR is preserved as line content, including in CRLF text.
/// Every buffer has at least one line, and a final LF creates an empty last line.
#[derive(Debug)]
pub struct Buffer {
    id: DocumentId,
    revision: Revision,
    text: String,
    line_starts: Vec<usize>,
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

    /// Revalidates the range against current text. Identical replacements do
    /// not advance the revision; every successful text change advances it once.
    pub fn replace(&mut self, range: TextRange, text: &str) -> Result<(), PositionError> {
        if self.slice(range)? == text {
            return Ok(());
        }
        let revision = Revision(self.revision.0.checked_add(1).expect("buffer revision exhausted"));
        self.text.replace_range(range.start().0..range.end().0, text);
        self.line_starts = Self::index_lines(&self.text);
        self.revision = revision;
        Ok(())
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
        starts.extend(text.bytes().enumerate().filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)));
        starts
    }
}

#[cfg(test)]
mod tests;
