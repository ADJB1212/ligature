use crate::{ByteOffset, Revision, TextRange};

/// One text change. `old_range` uses `old_revision` coordinates and `new_range`
/// uses `new_revision` coordinates. Undo and redo also produce new revisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    pub old_range: TextRange,
    pub new_range: TextRange,
    pub old_revision: Revision,
    pub new_revision: Revision,
}

#[derive(Debug)]
pub(crate) struct Transaction {
    pub start: ByteOffset,
    pub removed: String,
    pub inserted: String,
    pub before_state: Revision,
    pub after_state: Revision,
}
