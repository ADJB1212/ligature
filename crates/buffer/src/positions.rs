use std::{error::Error, fmt};

/// A UTF-8 byte offset from the start of a document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteOffset(pub usize);

/// A zero-based line and UTF-8 byte column, not a visual or character column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Position {
    pub line: usize,
    pub byte_column: usize,
}

/// A half-open range whose endpoints were validated against a buffer.
/// Validation must be repeated when used after the buffer changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRange {
    start: ByteOffset,
    end: ByteOffset,
}

impl TextRange {
    pub(crate) fn new(start: ByteOffset, end: ByteOffset) -> Self {
        Self { start, end }
    }

    pub fn start(self) -> ByteOffset {
        self.start
    }

    pub fn end(self) -> ByteOffset {
        self.end
    }

    pub fn len(self) -> usize {
        self.end.0 - self.start.0
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionError {
    OffsetOutOfBounds { offset: ByteOffset, len: usize },
    NotCharBoundary { offset: ByteOffset },
    ReversedRange { start: ByteOffset, end: ByteOffset },
    LineOutOfBounds { line: usize, line_count: usize },
    ColumnOutOfBounds { position: Position, line_len: usize },
}

impl fmt::Display for PositionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OffsetOutOfBounds { offset, len } => {
                write!(f, "byte offset {} exceeds buffer length {len}", offset.0)
            }
            Self::NotCharBoundary { offset } => {
                write!(f, "byte offset {} is not a UTF-8 character boundary", offset.0)
            }
            Self::ReversedRange { start, end } => {
                write!(f, "range start {} exceeds end {}", start.0, end.0)
            }
            Self::LineOutOfBounds { line, line_count } => {
                write!(f, "line {line} is outside {line_count} lines")
            }
            Self::ColumnOutOfBounds { position, line_len } => {
                write!(f, "byte column {} exceeds line {} length {line_len}", position.byte_column, position.line)
            }
        }
    }
}

impl Error for PositionError {}
