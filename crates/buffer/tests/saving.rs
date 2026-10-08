use ligature_buffer::{Buffer, ByteOffset};

#[test]
fn completing_a_save_marks_only_the_written_history_state_clean() {
    let mut buffer = Buffer::from_text("original");
    buffer.type_text(ByteOffset(buffer.len()), " first").unwrap();
    let written = buffer.save_point();
    let text = buffer.text().to_owned();
    buffer.type_text(ByteOffset(buffer.len()), " later").unwrap();
    assert!(buffer.mark_saved_at(written));
    assert_eq!(buffer.saved_revision(), written.revision());
    assert!(buffer.is_dirty());
    buffer.undo();
    assert_eq!(buffer.text(), text);
    assert!(!buffer.is_dirty());
    buffer.undo();
    assert!(buffer.is_dirty());
    buffer.redo();
    assert!(!buffer.is_dirty());
}

#[test]
fn save_completion_after_undo_does_not_mark_the_older_text_clean() {
    let mut buffer = Buffer::from_text("original");
    buffer.insert(ByteOffset(buffer.len()), " changed").unwrap();
    let written = buffer.save_point();
    buffer.undo();
    buffer.mark_saved_at(written);
    assert!(buffer.is_dirty());
    buffer.redo();
    assert!(!buffer.is_dirty());
    let mut other = Buffer::new();
    assert!(!other.mark_saved_at(written));
    assert!(!other.is_dirty());
}

#[test]
fn reload_preserves_document_identity_and_revision_and_resets_history() {
    let mut buffer = Buffer::from_text("original");
    let id = buffer.id();
    buffer.insert(ByteOffset(0), "local ").unwrap();
    let revision = buffer.revision();
    buffer.reload("external\r\n");
    assert_eq!(buffer.id(), id);
    assert!(buffer.revision() > revision);
    assert_eq!(buffer.text(), "external\r\n");
    assert_eq!(buffer.line_count(), 2);
    assert!(!buffer.is_dirty());
    assert!(!buffer.can_undo());
    assert!(!buffer.can_redo());
}

#[test]
fn save_completion_for_a_discarded_branch_does_not_clean_identical_text() {
    let mut buffer = Buffer::from_text("original");
    buffer.insert(ByteOffset(buffer.len()), " changed").unwrap();
    let written = buffer.save_point();
    buffer.undo().unwrap();
    buffer.insert(ByteOffset(buffer.len()), " changed").unwrap();
    assert!(buffer.mark_saved_at(written));
    assert_eq!(buffer.text(), "original changed");
    assert!(buffer.is_dirty());
    assert!(!buffer.can_redo());
    buffer.undo().unwrap();
    assert!(buffer.is_dirty());
    buffer.redo().unwrap();
    assert!(buffer.is_dirty());
}

#[test]
fn identical_reload_clears_history_without_advancing_revision() {
    let mut buffer = Buffer::from_text("original");
    buffer.insert(ByteOffset(0), "local ").unwrap();
    buffer.insert(ByteOffset(buffer.len()), " later").unwrap();
    buffer.undo().unwrap();
    let revision = buffer.revision();
    assert!(buffer.can_undo());
    assert!(buffer.can_redo());
    buffer.reload("local original");
    assert_eq!(buffer.revision(), revision);
    assert_eq!(buffer.saved_revision(), revision);
    assert!(!buffer.is_dirty());
    assert_eq!(buffer.undo(), None);
    assert_eq!(buffer.redo(), None);
}

#[test]
fn typing_after_reload_undoes_only_the_new_text() {
    let mut buffer = Buffer::new();
    buffer.type_text(ByteOffset(0), "old").unwrap();
    buffer.reload("new");
    buffer.type_text(ByteOffset(3), " text").unwrap();
    buffer.undo().unwrap();
    assert_eq!(buffer.text(), "new");
    assert!(!buffer.is_dirty());
    assert_eq!(buffer.undo(), None);
}
