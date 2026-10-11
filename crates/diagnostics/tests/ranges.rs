use ligature_diagnostics::{SourcePosition, SourceRange};

fn at(line: usize, column: usize) -> SourcePosition {
    SourcePosition { line, column }
}

fn text(start: SourcePosition, end: SourcePosition) -> SourceRange {
    SourceRange::Text { start, end }
}

#[test]
fn first_line_reports_the_start_of_either_range_kind() {
    assert_eq!(SourceRange::Lines { start: 4, end: 9 }.first_line(), 4);
    assert_eq!(text(at(7, 1), at(8, 1)).first_line(), 7);
}

#[test]
fn empty_documents_have_a_single_empty_first_line() {
    assert_eq!(SourceRange::Lines { start: 1, end: 1 }.byte_range(""), Some(0..0));
    assert_eq!(SourceRange::Lines { start: 2, end: 2 }.byte_range(""), None);
    assert_eq!(text(at(1, 1), at(1, 1)).byte_range(""), Some(0..0));
    assert_eq!(text(at(1, 2), at(1, 2)).byte_range(""), None);
}

#[test]
fn documents_without_a_final_newline_resolve_their_last_line() {
    let content = "ab\ncd";
    assert_eq!(SourceRange::Lines { start: 2, end: 2 }.byte_range(content), Some(3..5));
    assert_eq!(text(at(2, 2), at(2, 3)).byte_range(content), Some(4..5));
    assert_eq!(SourceRange::Lines { start: 3, end: 3 }.byte_range(content), None);
}

#[test]
fn exclusive_ends_may_sit_at_the_start_of_the_next_line() {
    let content = "ab\ncd";
    assert_eq!(text(at(1, 2), at(2, 1)).byte_range(content), Some(1..3));
    assert_eq!(text(at(1, 1), at(1, 3)).byte_range(content), Some(0..2));
}

#[test]
fn zero_width_ranges_are_accepted_inside_and_at_the_end_of_a_line() {
    let content = "abc\n";
    assert_eq!(text(at(1, 2), at(1, 2)).byte_range(content), Some(1..1));
    assert_eq!(text(at(1, 4), at(1, 4)).byte_range(content), Some(3..3));
    assert_eq!(text(at(1, 5), at(1, 5)).byte_range(content), None);
}

#[test]
fn reversed_ranges_are_rejected() {
    let content = "abcdef\nghi";
    assert_eq!(text(at(1, 4), at(1, 2)).byte_range(content), None);
    assert_eq!(text(at(2, 1), at(1, 1)).byte_range(content), None);
    assert_eq!(SourceRange::Lines { start: 2, end: 1 }.byte_range(content), None);
}

#[test]
fn columns_count_unicode_scalars_not_bytes_or_graphemes() {
    let content = "e\u{301}😀x";
    let range = |column| text(at(1, column), at(1, column + 1)).byte_range(content).map(|range| &content[range]);
    assert_eq!(range(1), Some("e"));
    assert_eq!(range(2), Some("\u{301}"));
    assert_eq!(range(3), Some("😀"));
    assert_eq!(range(4), Some("x"));
    assert_eq!(range(5), None);
}

#[test]
fn carriage_returns_are_excluded_only_when_they_end_a_line() {
    let content = "a\rb\r\nc\r\n";
    assert_eq!(SourceRange::Lines { start: 1, end: 1 }.byte_range(content), Some(0..3));
    assert_eq!(text(at(1, 4), at(1, 4)).byte_range(content), Some(3..3));
    assert_eq!(text(at(1, 5), at(1, 5)).byte_range(content), None);
    assert_eq!(SourceRange::Lines { start: 1, end: 2 }.byte_range(content), Some(0..6));
}

#[test]
fn line_zero_and_column_zero_are_invalid() {
    let content = "abc";
    assert_eq!(SourceRange::Lines { start: 0, end: 0 }.byte_range(content), None);
    assert_eq!(text(at(0, 1), at(1, 1)).byte_range(content), None);
    assert_eq!(text(at(1, 0), at(1, 1)).byte_range(content), None);
    assert_eq!(text(at(1, 1), at(1, 0)).byte_range(content), None);
}

#[test]
fn line_ranges_span_from_first_line_start_to_last_line_end_without_the_newline() {
    let content = "one\ntwo\nthree\n";
    assert_eq!(SourceRange::Lines { start: 1, end: 3 }.byte_range(content), Some(0..13));
    assert_eq!(SourceRange::Lines { start: 3, end: 4 }.byte_range(content), Some(8..14));
    assert_eq!(SourceRange::Lines { start: 4, end: 4 }.byte_range(content), Some(14..14));
    assert_eq!(SourceRange::Lines { start: 3, end: 5 }.byte_range(content), None);
}

#[test]
fn supplied_line_indexes_that_do_not_match_the_text_are_rejected_without_panicking() {
    let content = "héllo";
    let ranges = [0..99, 2..3, std::ops::Range { start: 3, end: 2 }, 1..1, 5..5];
    for range in ranges {
        let lines = SourceRange::Lines { start: 1, end: 1 };
        let result = lines.byte_range_with_lines(content, |_| Some(range.clone()));
        if let Some(result) = result {
            assert!(content.get(result).is_some());
        }
        let exact = text(at(1, 1), at(1, 3));
        if let Some(result) = exact.byte_range_with_lines(content, |_| Some(range.clone())) {
            assert!(content.get(result).is_some());
        }
    }
    assert_eq!(SourceRange::Lines { start: 1, end: 1 }.byte_range_with_lines(content, |_| None), None);
}

#[test]
fn source_ranges_and_positions_order_by_line_then_column() {
    assert!(at(1, 9) < at(2, 1));
    assert!(at(2, 1) < at(2, 2));
    assert!(SourceRange::Lines { start: 1, end: 2 } < SourceRange::Lines { start: 1, end: 3 });
}
