//! Text storage, edit transactions, undo/redo history, and document positions. Independent of GPUI.

mod positions;
mod storage;

pub use positions::{ByteOffset, Position, PositionError, TextRange};
pub use storage::{Buffer, DocumentId, Revision};
