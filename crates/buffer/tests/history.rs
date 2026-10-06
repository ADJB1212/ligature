use ligature_buffer::{Buffer, ByteOffset, Edit, Position, PositionError, TextRange};

fn range(buffer: &Buffer, start: usize, end: usize) -> TextRange {
    buffer.text_range(ByteOffset(start), ByteOffset(end)).unwrap()
}

fn assert_text(buffer: &Buffer, expected: &str) {
    assert_eq!(buffer.text(), expected);
    let lines: Vec<_> = expected.split('\n').collect();
    assert_eq!(buffer.line_count(), lines.len());
    for (line, text) in lines.iter().enumerate() {
        assert_eq!(buffer.line(line), Ok(*text));
    }
    for offset in (0..=expected.len()).filter(|&offset| expected.is_char_boundary(offset)) {
        let prefix = &expected[..offset];
        let position = Position {
            line: prefix.bytes().filter(|&byte| byte == b'\n').count(),
            byte_column: prefix.rfind('\n').map_or(offset, |newline| offset - newline - 1),
        };
        assert_eq!(buffer.offset_to_position(ByteOffset(offset)), Ok(position));
        assert_eq!(buffer.position_to_offset(position), Ok(ByteOffset(offset)));
    }
}

fn assert_edit(edit: Edit, old: (usize, usize), new: (usize, usize), revision: u64) {
    assert_eq!(edit.old_range.start(), ByteOffset(old.0));
    assert_eq!(edit.old_range.end(), ByteOffset(old.1));
    assert_eq!(edit.new_range.start(), ByteOffset(new.0));
    assert_eq!(edit.new_range.end(), ByteOffset(new.1));
    assert_eq!(edit.old_revision.get(), revision);
    assert_eq!(edit.new_revision.get(), revision + 1);
}

#[test]
fn insertions_at_start_middle_and_end_are_separate_transactions() {
    let mut buffer = Buffer::from_text("猫");
    assert_edit(buffer.insert(ByteOffset(0), "é").unwrap().unwrap(), (0, 0), (0, 2), 0);
    assert_edit(buffer.insert(ByteOffset(2), "\n").unwrap().unwrap(), (2, 2), (2, 3), 1);
    assert_edit(buffer.insert(ByteOffset(6), "🦀").unwrap().unwrap(), (6, 6), (6, 10), 2);
    assert_text(&buffer, "é\n猫🦀");
    buffer.undo().unwrap();
    assert_text(&buffer, "é\n猫");
    buffer.undo().unwrap();
    assert_text(&buffer, "é猫");
    buffer.undo().unwrap();
    assert_text(&buffer, "猫");
    assert!(!buffer.is_dirty());
}

#[test]
fn deletion_of_multiline_text_restores_newlines_on_undo() {
    let mut buffer = Buffer::from_text("a\né\n猫\n");
    let deleted = range(&buffer, 1, 8);
    assert_edit(buffer.delete(deleted).unwrap().unwrap(), (1, 8), (1, 1), 0);
    assert_text(&buffer, "a\n");
    assert_edit(buffer.undo().unwrap(), (1, 1), (1, 8), 1);
    assert_text(&buffer, "a\né\n猫\n");
    assert_edit(buffer.redo().unwrap(), (1, 8), (1, 1), 2);
    assert_text(&buffer, "a\n");
}

#[test]
fn replacement_is_one_transaction_with_ranges_in_each_revision() {
    let mut buffer = Buffer::from_text("first\r\nsecond\nlast");
    let replaced = range(&buffer, 5, 14);
    assert_edit(buffer.replace(replaced, "\n猫\n🦀\n").unwrap().unwrap(), (5, 14), (5, 15), 0);
    assert_text(&buffer, "first\n猫\n🦀\nlast");
    assert_edit(buffer.undo().unwrap(), (5, 15), (5, 14), 1);
    assert_text(&buffer, "first\r\nsecond\nlast");
    assert!(!buffer.can_undo());
    assert_edit(buffer.redo().unwrap(), (5, 14), (5, 15), 2);
    assert_text(&buffer, "first\n猫\n🦀\nlast");
}

#[test]
fn mixed_edits_undo_and_redo_in_order() {
    let mut buffer = Buffer::from_text("alpha\nβeta\n");
    let original = buffer.text().to_owned();
    buffer.insert(ByteOffset(0), "猫\n").unwrap();
    let first = buffer.text().to_owned();
    let deleted = range(&buffer, 4, 10);
    buffer.delete(deleted).unwrap();
    let second = buffer.text().to_owned();
    let replaced = range(&buffer, 4, 6);
    buffer.replace(replaced, "🦀\n").unwrap();
    let third = buffer.text().to_owned();
    for expected in [&second, &first, &original] {
        buffer.undo().unwrap();
        assert_text(&buffer, expected);
    }
    assert_eq!(buffer.revision().get(), 6);
    assert_eq!(buffer.undo(), None);
    for expected in [&first, &second, &third] {
        buffer.redo().unwrap();
        assert_text(&buffer, expected);
    }
    assert_eq!(buffer.revision().get(), 9);
    assert_eq!(buffer.redo(), None);
    assert_eq!(buffer.revision().get(), 9);
}

#[test]
fn adjacent_unicode_typing_is_one_undo_transaction() {
    let mut buffer = Buffer::new();
    buffer.type_text(ByteOffset(0), "é").unwrap();
    buffer.type_text(ByteOffset(2), "猫").unwrap();
    buffer.type_text(ByteOffset(5), "e\u{301}").unwrap();
    assert_text(&buffer, "é猫e\u{301}");
    assert_eq!(buffer.revision().get(), 3);
    assert_edit(buffer.undo().unwrap(), (0, 8), (0, 0), 3);
    assert_text(&buffer, "");
    assert!(!buffer.can_undo());
    assert_edit(buffer.redo().unwrap(), (0, 0), (0, 8), 4);
    assert_text(&buffer, "é猫e\u{301}");
}

#[test]
fn cursor_boundaries_and_nonadjacent_typing_start_new_groups() {
    let mut buffer = Buffer::new();
    buffer.type_text(ByteOffset(0), "a").unwrap();
    buffer.end_typing_group();
    buffer.type_text(ByteOffset(1), "b").unwrap();
    buffer.type_text(ByteOffset(0), "c").unwrap();
    assert_text(&buffer, "cab");
    for expected in ["ab", "a", ""] {
        buffer.undo().unwrap();
        assert_text(&buffer, expected);
    }
}

#[test]
fn paste_and_explicit_commands_do_not_join_typing_groups() {
    let mut buffer = Buffer::new();
    buffer.type_text(ByteOffset(0), "a").unwrap();
    buffer.type_text(ByteOffset(1), "b").unwrap();
    buffer.insert(ByteOffset(2), "paste\n猫").unwrap();
    buffer.type_text(ByteOffset(11), "c").unwrap();
    let replaced = range(&buffer, 0, 2);
    buffer.replace(replaced, "x").unwrap();
    for expected in ["abpaste\n猫c", "abpaste\n猫", "ab", ""] {
        buffer.undo().unwrap();
        assert_text(&buffer, expected);
    }
}

#[test]
fn newline_typing_is_a_separate_transaction() {
    let mut buffer = Buffer::new();
    buffer.type_text(ByteOffset(0), "a").unwrap();
    buffer.type_text(ByteOffset(1), "\n").unwrap();
    buffer.type_text(ByteOffset(2), "b").unwrap();
    for expected in ["a\n", "a", ""] {
        buffer.undo().unwrap();
        assert_text(&buffer, expected);
    }
}

#[test]
fn new_edit_after_undo_discards_redo() {
    let mut buffer = Buffer::new();
    buffer.insert(ByteOffset(0), "a").unwrap();
    buffer.insert(ByteOffset(1), "b").unwrap();
    buffer.undo().unwrap();
    assert!(buffer.can_redo());
    buffer.type_text(ByteOffset(1), "c").unwrap();
    assert!(!buffer.can_redo());
    assert_eq!(buffer.redo(), None);
    assert_text(&buffer, "ac");
    buffer.undo().unwrap();
    assert_text(&buffer, "a");
}

#[test]
fn typing_after_redo_starts_a_new_transaction() {
    let mut buffer = Buffer::new();
    buffer.type_text(ByteOffset(0), "a").unwrap();
    buffer.undo().unwrap();
    buffer.redo().unwrap();
    buffer.type_text(ByteOffset(1), "b").unwrap();
    buffer.undo().unwrap();
    assert_text(&buffer, "a");
    buffer.undo().unwrap();
    assert_text(&buffer, "");
}

#[test]
fn invalid_edits_preserve_text_revision_saved_state_and_redo() {
    let mut buffer = Buffer::from_text("猫");
    let stale = range(&buffer, 0, 3);
    buffer.insert(ByteOffset(3), "x").unwrap();
    buffer.undo().unwrap();
    let revision = buffer.revision();
    let expected = Err(PositionError::NotCharBoundary { offset: ByteOffset(1) });
    assert_eq!(buffer.insert(ByteOffset(1), "bad"), expected);
    assert_eq!(buffer.type_text(ByteOffset(1), "bad"), expected);
    let other = Buffer::from_text("abc");
    let invalid = range(&other, 1, 2);
    assert_eq!(buffer.delete(invalid), expected);
    assert_eq!(buffer.replace(invalid, "bad"), expected);
    assert_eq!(buffer.revision(), revision);
    assert!(!buffer.is_dirty());
    assert!(buffer.can_redo());
    assert_text(&buffer, "猫");
    buffer.redo().unwrap();
    buffer.delete(stale).unwrap();
    assert_text(&buffer, "x");
}

#[test]
fn no_op_edits_preserve_revision_redo_and_dirty_state() {
    let mut buffer = Buffer::from_text("abc");
    buffer.insert(ByteOffset(3), "x").unwrap();
    buffer.undo().unwrap();
    let revision = buffer.revision();
    let empty = range(&buffer, 0, 0);
    let whole = range(&buffer, 0, 3);
    assert_eq!(buffer.delete(empty), Ok(None));
    assert_eq!(buffer.insert(ByteOffset(3), ""), Ok(None));
    assert_eq!(buffer.type_text(ByteOffset(3), ""), Ok(None));
    assert_eq!(buffer.replace(whole, "abc"), Ok(None));
    assert_eq!(buffer.revision(), revision);
    assert!(!buffer.is_dirty());
    assert!(buffer.can_redo());
    assert!(!buffer.can_undo());
}

#[test]
fn save_point_is_clean_when_revisited_by_undo_or_redo() {
    let mut buffer = Buffer::new();
    assert!(!buffer.is_dirty());
    assert_eq!(buffer.saved_revision().get(), 0);
    buffer.type_text(ByteOffset(0), "a").unwrap();
    assert!(buffer.is_dirty());
    buffer.mark_saved();
    assert!(!buffer.is_dirty());
    assert_eq!(buffer.saved_revision().get(), 1);
    buffer.type_text(ByteOffset(1), "b").unwrap();
    assert!(buffer.is_dirty());
    buffer.undo().unwrap();
    assert_text(&buffer, "a");
    assert!(!buffer.is_dirty());
    buffer.undo().unwrap();
    assert_text(&buffer, "");
    assert!(buffer.is_dirty());
    buffer.redo().unwrap();
    assert_text(&buffer, "a");
    assert!(!buffer.is_dirty());
    buffer.redo().unwrap();
    assert_text(&buffer, "ab");
    assert!(buffer.is_dirty());
    assert_eq!(buffer.revision().get(), 6);
    assert_eq!(buffer.saved_revision().get(), 1);
}

#[test]
fn save_after_undo_preserves_redo_and_records_current_revision() {
    let mut buffer = Buffer::new();
    buffer.insert(ByteOffset(0), "a").unwrap();
    buffer.insert(ByteOffset(1), "b").unwrap();
    buffer.undo().unwrap();
    buffer.mark_saved();
    assert_eq!(buffer.saved_revision().get(), 3);
    assert!(buffer.can_redo());
    buffer.redo().unwrap();
    assert!(buffer.is_dirty());
    buffer.undo().unwrap();
    assert!(!buffer.is_dirty());
    assert_eq!(buffer.saved_revision().get(), 3);
}

#[test]
fn abandoned_save_point_does_not_match_a_new_branch_with_identical_text() {
    let mut buffer = Buffer::new();
    buffer.insert(ByteOffset(0), "a").unwrap();
    buffer.mark_saved();
    buffer.undo().unwrap();
    buffer.insert(ByteOffset(0), "a").unwrap();
    assert_text(&buffer, "a");
    assert!(buffer.is_dirty());
    assert!(!buffer.can_redo());
    buffer.undo().unwrap();
    buffer.redo().unwrap();
    assert!(buffer.is_dirty());
    buffer.mark_saved();
    assert!(!buffer.is_dirty());
}

#[test]
fn save_at_initial_state_is_restored_after_grouped_typing() {
    let mut buffer = Buffer::new();
    buffer.mark_saved();
    buffer.type_text(ByteOffset(0), "a").unwrap();
    buffer.type_text(ByteOffset(1), "b").unwrap();
    buffer.undo().unwrap();
    assert!(!buffer.is_dirty());
    buffer.redo().unwrap();
    assert!(buffer.is_dirty());
}

#[test]
fn all_small_unicode_replacements_restore_text_positions_and_saved_state() {
    for original in ["", "a", "é猫", "\n", "a\n", "a\r\nb\n", "🦀\né\n"] {
        let boundaries: Vec<_> = (0..=original.len()).filter(|&offset| original.is_char_boundary(offset)).collect();
        for &start in &boundaries {
            for &end in boundaries.iter().filter(|&&end| end >= start) {
                for replacement in ["", "x", "猫", "\n", "é\n🦀\n"] {
                    let mut buffer = Buffer::from_text(original);
                    let replaced = range(&buffer, start, end);
                    let mut expected = original.to_owned();
                    expected.replace_range(start..end, replacement);
                    let edit = buffer.replace(replaced, replacement).unwrap();
                    assert_text(&buffer, &expected);
                    if expected == original {
                        assert_eq!(edit, None);
                        assert!(!buffer.can_undo());
                        assert!(!buffer.is_dirty());
                    } else {
                        assert_edit(edit.unwrap(), (start, end), (start, start + replacement.len()), 0);
                        assert!(buffer.is_dirty());
                        buffer.undo().unwrap();
                        assert_text(&buffer, original);
                        assert!(!buffer.is_dirty());
                        buffer.redo().unwrap();
                        assert_text(&buffer, &expected);
                        assert!(buffer.is_dirty());
                    }
                }
            }
        }
    }
}
