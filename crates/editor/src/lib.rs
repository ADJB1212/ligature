//! GPUI text editor views, cursor movement, selections, and editing commands.

mod state;
mod view;

pub use state::{EditorState, Indentation, LineEnding, Movement};
pub use view::{Copy, Cut, Editor, Paste, Redo, SelectAll, Undo, init};
