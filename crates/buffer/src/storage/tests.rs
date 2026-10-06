use super::*;

#[test]
fn empty_buffer_has_one_line_and_valid_eof() {
    let buffer = Buffer::new();
    assert!(buffer.is_empty());
    assert_eq!(buffer.len(), 0);
    assert_eq!(buffer.revision().get(), 0);
    assert_eq!(buffer.line_count(), 1);
    assert_eq!(buffer.line(0), Ok(""));
    assert_eq!(buffer.offset_to_position(ByteOffset(0)), Ok(Position::default()));
    assert_eq!(buffer.position_to_offset(Position::default()), Ok(ByteOffset(0)));
    let range = buffer.text_range(ByteOffset(0), ByteOffset(0)).unwrap();
    assert!(range.is_empty());
    assert_eq!(buffer.slice(range), Ok(""));
}

#[test]
fn documents_with_identical_text_have_distinct_identities() {
    let first = Buffer::from_text("same");
    let second = Buffer::from_text("same".to_string());
    assert_ne!(first.id(), second.id());
    assert_eq!(first.text(), second.text());
}

#[test]
fn lines_preserve_text_and_exclude_only_lf() {
    let buffer = Buffer::from_text("first\r\n\nlast\r");
    assert_eq!(buffer.line_count(), 3);
    assert_eq!(buffer.line(0), Ok("first\r"));
    assert_eq!(buffer.line(1), Ok(""));
    assert_eq!(buffer.line(2), Ok("last\r"));
    assert_eq!(buffer.line(0).unwrap().as_ptr(), buffer.text().as_ptr());
}

#[test]
fn final_newline_creates_a_trailing_empty_line() {
    let buffer = Buffer::from_text("a\n\n");
    assert_eq!(buffer.line_count(), 3);
    assert_eq!(buffer.line(2), Ok(""));
    assert_eq!(buffer.offset_to_position(ByteOffset(3)), Ok(Position { line: 2, byte_column: 0 }));
}

#[test]
fn newline_and_eof_positions_use_line_end_byte_columns() {
    let buffer = Buffer::from_text("é\n猫");
    assert_eq!(buffer.offset_to_position(ByteOffset(2)), Ok(Position { line: 0, byte_column: 2 }));
    assert_eq!(buffer.offset_to_position(ByteOffset(3)), Ok(Position { line: 1, byte_column: 0 }));
    assert_eq!(buffer.offset_to_position(ByteOffset(6)), Ok(Position { line: 1, byte_column: 3 }));
}

#[test]
fn every_character_boundary_round_trips_through_positions() {
    for text in ["", "plain", "\n", "\n\n", "a\n", "a\r\nb\rc", "é猫\n🦀e\u{301}\t\n"] {
        let buffer = Buffer::from_text(text);
        for offset in 0..=text.len() {
            if text.is_char_boundary(offset) {
                let position = buffer.offset_to_position(ByteOffset(offset)).unwrap();
                assert_eq!(buffer.position_to_offset(position), Ok(ByteOffset(offset)));
            } else {
                assert_eq!(
                    buffer.offset_to_position(ByteOffset(offset)),
                    Err(PositionError::NotCharBoundary { offset: ByteOffset(offset) })
                );
            }
        }
    }
}

#[test]
fn invalid_offsets_and_positions_are_rejected() {
    let buffer = Buffer::from_text("é\nx");
    assert_eq!(
        buffer.offset_to_position(ByteOffset(usize::MAX)),
        Err(PositionError::OffsetOutOfBounds {
            offset: ByteOffset(usize::MAX),
            len: 4
        })
    );
    assert_eq!(
        buffer.line(usize::MAX),
        Err(PositionError::LineOutOfBounds {
            line: usize::MAX,
            line_count: 2
        })
    );
    assert_eq!(
        buffer.position_to_offset(Position { line: 2, byte_column: 0 }),
        Err(PositionError::LineOutOfBounds { line: 2, line_count: 2 })
    );
    let position = Position {
        line: 0,
        byte_column: usize::MAX,
    };
    assert_eq!(
        buffer.position_to_offset(position),
        Err(PositionError::ColumnOutOfBounds { position, line_len: 2 })
    );
    assert_eq!(
        buffer.position_to_offset(Position { line: 0, byte_column: 1 }),
        Err(PositionError::NotCharBoundary { offset: ByteOffset(1) })
    );
}

#[test]
fn ranges_validate_order_bounds_and_character_boundaries() {
    let buffer = Buffer::from_text("a猫z");
    let range = buffer.text_range(ByteOffset(1), ByteOffset(4)).unwrap();
    assert_eq!(range.len(), 3);
    assert!(!range.is_empty());
    assert_eq!(buffer.slice(range), Ok("猫"));
    assert_eq!(
        buffer.text_range(ByteOffset(4), ByteOffset(1)),
        Err(PositionError::ReversedRange {
            start: ByteOffset(4),
            end: ByteOffset(1)
        })
    );
    for (start, end, expected) in [
        (2, 4, PositionError::NotCharBoundary { offset: ByteOffset(2) }),
        (1, 3, PositionError::NotCharBoundary { offset: ByteOffset(3) }),
        (
            1,
            6,
            PositionError::OffsetOutOfBounds {
                offset: ByteOffset(6),
                len: 5,
            },
        ),
    ] {
        assert_eq!(buffer.text_range(ByteOffset(start), ByteOffset(end)), Err(expected));
    }
}

#[test]
fn text_changes_advance_revision_and_preserve_identity() {
    let mut buffer = Buffer::from_text("abc");
    let id = buffer.id();
    let range = buffer.text_range(ByteOffset(1), ByteOffset(2)).unwrap();
    buffer.replace(range, "猫\n").unwrap();
    assert_eq!(buffer.text(), "a猫\nc");
    assert_eq!(buffer.revision().get(), 1);
    assert_eq!(buffer.id(), id);
    assert_eq!(buffer.line_count(), 2);
    assert_eq!(buffer.line(0), Ok("a猫"));
    assert_eq!(buffer.line(1), Ok("c"));
    assert_eq!(buffer.position_to_offset(Position { line: 1, byte_column: 1 }), Ok(ByteOffset(6)));
    let range = buffer.text_range(ByteOffset(0), ByteOffset(6)).unwrap();
    buffer.replace(range, "").unwrap();
    assert_eq!(buffer.revision().get(), 2);
    assert_eq!(buffer.line_count(), 1);
    assert_eq!(buffer.line(0), Ok(""));
}

#[test]
fn identical_replacements_preserve_revision() {
    let mut buffer = Buffer::from_text("abc");
    let range = buffer.text_range(ByteOffset(0), ByteOffset(3)).unwrap();
    buffer.replace(range, "abc").unwrap();
    let range = buffer.text_range(ByteOffset(3), ByteOffset(3)).unwrap();
    buffer.replace(range, "").unwrap();
    assert_eq!(buffer.revision().get(), 0);
}

#[test]
fn stale_ranges_are_revalidated_before_slicing_or_replacement() {
    let mut buffer = Buffer::from_text("abcd");
    let stale = buffer.text_range(ByteOffset(1), ByteOffset(3)).unwrap();
    let whole = buffer.text_range(ByteOffset(0), ByteOffset(4)).unwrap();
    buffer.replace(whole, "猫x").unwrap();
    let expected = Err(PositionError::NotCharBoundary { offset: ByteOffset(1) });
    assert_eq!(buffer.slice(stale), expected);
    assert_eq!(buffer.replace(stale, "bad"), expected.map(|_| ()));
    assert_eq!(buffer.text(), "猫x");
    assert_eq!(buffer.revision().get(), 1);
    let whole = buffer.text_range(ByteOffset(0), ByteOffset(4)).unwrap();
    buffer.replace(whole, "").unwrap();
    assert_eq!(
        buffer.slice(stale),
        Err(PositionError::OffsetOutOfBounds {
            offset: ByteOffset(1),
            len: 0
        })
    );
}
