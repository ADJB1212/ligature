use ligature_buffer::Buffer;
use ligature_editor::{EditorState, Indentation, LineEnding, Movement};

fn editor(text: &str) -> EditorState {
    EditorState::new(Buffer::from_text(text))
}

#[test]
fn composition_line_queries_follow_multiline_updates_commit_and_cancel() {
    let original = "head\r\nold\n猫\r\n";
    let mut editor = editor(original);
    editor.select(6, 10);
    for replacement in ["", "a\nb\r\n", "猫\r\n🦀\n", "\n", "final"] {
        editor.mark(None, replacement, None);
        assert_editor_lines(&editor);
        assert_eq!(editor.buffer().text(), original);
    }
    let committed = editor.text().to_owned();
    editor.commit_composition();
    assert_editor_lines(&editor);
    assert_eq!(editor.buffer().text(), committed);
    editor.undo();
    assert_eq!(editor.text(), original);
    assert_editor_lines(&editor);
    editor.select_all();
    editor.mark(None, "\n猫\r\n", None);
    assert_editor_lines(&editor);
    editor.cancel_composition();
    assert_eq!(editor.text(), original);
    assert_editor_lines(&editor);
}

fn assert_editor_lines(editor: &EditorState) {
    let mut start = 0;
    let lines: Vec<_> = editor.text().split('\n').collect();
    assert_eq!(editor.line_count(), lines.len());
    for (line, text) in lines.into_iter().enumerate() {
        let end = start + text.strip_suffix('\r').unwrap_or(text).len();
        assert_eq!(editor.line_range(line), start..end);
        for offset in (0..=text.len()).filter(|&offset| text.is_char_boundary(offset)) {
            assert_eq!(editor.line_index(start + offset), line);
        }
        start += text.len() + 1;
    }
}

#[test]
fn typing_replaces_selection_in_one_undo_transaction() {
    let mut editor = editor("before middle after");
    editor.select(7, 13);
    editor.insert("replacement", true);
    assert_eq!(editor.text(), "before replacement after");
    assert_eq!(editor.selection(), 18..18);
    editor.undo();
    assert_eq!(editor.text(), "before middle after");
    editor.redo();
    assert_eq!(editor.text(), "before replacement after");
}

#[test]
fn copy_cut_and_paste_preserve_transaction_boundaries() {
    let mut editor = editor("hello world");
    editor.select(0, 5);
    assert_eq!(editor.copy().as_deref(), Some("hello"));
    assert!(!editor.buffer().is_dirty());
    assert_eq!(editor.cut().as_deref(), Some("hello"));
    assert_eq!(editor.text(), " world");
    editor.insert("pasted\ntext", false);
    editor.insert("!", true);
    editor.undo();
    assert_eq!(editor.text(), "pasted\ntext world");
    editor.undo();
    assert_eq!(editor.text(), " world");
    editor.undo();
    assert_eq!(editor.text(), "hello world");
    editor.select(0, 0);
    assert_eq!(editor.copy(), None);
    assert_eq!(editor.cut(), None);
}

#[test]
fn movement_and_deletion_preserve_combining_marks_and_emoji_sequences() {
    let graphemes = ["a", "e\u{301}", "👩🏽‍💻", "🇨🇦", "z"];
    let text = graphemes.concat();
    let mut editor = editor(&text);
    let mut offset = 0;
    for grapheme in graphemes {
        editor.move_cursor(Movement::Right, false);
        offset += grapheme.len();
        assert_eq!(editor.caret(), offset);
    }
    editor.move_cursor(Movement::Left, false);
    editor.delete(true);
    assert_eq!(editor.text(), "ae\u{301}👩🏽‍💻z");
    editor.move_cursor(Movement::Left, false);
    editor.delete(false);
    assert_eq!(editor.text(), "ae\u{301}z");
    editor.select(2, 3);
    assert_eq!(editor.selection(), 1..1);
}

#[test]
fn insertion_that_joins_emoji_leaves_caret_at_a_grapheme_boundary() {
    let mut editor = editor("👩💻");
    editor.select("👩".len(), "👩".len());
    editor.insert("\u{200d}", true);
    assert_eq!(editor.text(), "👩‍💻");
    assert_eq!(editor.caret(), editor.text().len());
    editor.delete(true);
    assert_eq!(editor.text(), "");
    editor.undo();
    editor.undo();
    assert_eq!(editor.text(), "👩💻");
}

#[test]
fn vertical_movement_preserves_column_across_short_lines() {
    let mut editor = editor("abcdef\nx\nabcdef\n\tabc\nabcdef");
    editor.select(5, 5);
    editor.move_cursor(Movement::Down, false);
    assert_eq!(editor.caret(), 8);
    editor.move_cursor(Movement::Down, false);
    assert_eq!(editor.caret(), 14);
    editor.move_cursor(Movement::Down, false);
    assert_eq!(editor.caret(), 18);
    editor.move_cursor(Movement::Down, true);
    assert_eq!(editor.caret(), 26);
    assert_eq!(editor.anchor(), 18);
    editor.move_cursor(Movement::Up, true);
    assert_eq!(editor.caret(), 18);
}

#[test]
fn line_document_word_and_shift_movements_select_expected_text() {
    let mut editor = editor("first word\nsecond");
    editor.move_cursor(Movement::WordRight, true);
    assert_eq!(editor.selected_text(), "first");
    editor.move_cursor(Movement::Right, false);
    assert_eq!(editor.caret(), 5);
    editor.move_cursor(Movement::WordRight, false);
    assert_eq!(editor.caret(), 10);
    editor.move_cursor(Movement::WordLeft, true);
    assert_eq!(editor.selected_text(), "word");
    editor.move_cursor(Movement::LineStart, false);
    assert_eq!(editor.caret(), 0);
    editor.move_cursor(Movement::LineEnd, false);
    assert_eq!(editor.caret(), 10);
    editor.move_cursor(Movement::DocumentEnd, true);
    assert_eq!(editor.selected_text(), "\nsecond");
    editor.move_cursor(Movement::DocumentStart, false);
    assert_eq!(editor.caret(), 0);
    editor.select_word(13);
    assert_eq!(editor.selected_text(), "second");
    editor.select_all();
    assert_eq!(editor.selected_text(), editor.text());
}

#[test]
fn word_selection_preserves_unicode_and_line_ending_boundaries() {
    let prefix = "earlier text\n".repeat(4096);
    let text = format!("{prefix}cafe\u{301} word\r\nlast\n");
    let mut editor = editor(&text);
    for (offset, selected) in [
        (1, "cafe\u{301}"),
        (7, "word"),
        (11, "\r\n"),
        (12, "\r\n"),
        (14, "last"),
        (17, "\n"),
        (18, ""),
    ] {
        editor.select_word(prefix.len() + offset);
        assert_eq!(editor.selected_text(), selected, "offset {offset}");
    }
}

#[test]
fn newlines_and_grapheme_deletion_preserve_crlf() {
    let mut editor = editor("a\r\nb");
    editor.select(1, 1);
    editor.newline();
    assert_eq!(editor.text(), "a\r\n\r\nb");
    editor.delete(true);
    assert_eq!(editor.text(), "a\r\nb");
    editor.delete(false);
    assert_eq!(editor.text(), "ab");
    editor.set_line_ending(LineEnding::Lf);
    editor.newline();
    assert_eq!(editor.text(), "a\nb");
}

#[test]
fn indentation_advances_to_configured_tab_stops() {
    let mut editor = editor("abc");
    editor.select(3, 3);
    editor.indent();
    assert_eq!(editor.text(), "abc ");
    editor.indent();
    assert_eq!(editor.text(), "abc     ");
    editor.set_indentation(Indentation::Tabs, 8);
    editor.indent();
    assert_eq!(editor.text(), "abc     \t");
    editor.undo();
    assert_eq!(editor.text(), "abc     ");
}

#[test]
fn ime_updates_commit_as_one_transaction_and_cancel_without_changes() {
    let mut editor = editor("before old after");
    editor.select(7, 10);
    editor.mark(None, "に", Some(1..1));
    assert_eq!(editor.text(), "before に after");
    assert_eq!(editor.buffer().text(), "before old after");
    assert!(!editor.buffer().is_dirty());
    editor.mark(None, "日本", Some(2..2));
    editor.commit_text(None, "日本語");
    assert_eq!(editor.text(), "before 日本語 after");
    assert_eq!(editor.marked_range(), None);
    editor.undo();
    assert_eq!(editor.text(), "before old after");
    assert!(!editor.buffer().is_dirty());
    editor.select(10, 7);
    editor.mark(None, "draft", None);
    editor.cancel_composition();
    assert_eq!(editor.text(), "before old after");
    assert_eq!(editor.anchor(), 10);
    assert_eq!(editor.caret(), 7);
    assert!(!editor.buffer().can_undo());
}

#[test]
fn unmark_commits_and_multiline_composition_exposes_display_lines() {
    let mut editor = editor("a\nz");
    editor.select(1, 1);
    editor.mark(None, "😀\nβ", Some(2..4));
    assert_eq!(editor.selected_text(), "\nβ");
    assert_eq!(editor.line_count(), 3);
    assert_eq!(editor.line_range(1), 6..8);
    editor.commit_composition();
    assert_eq!(editor.buffer().text(), "a😀\nβ\nz");
    editor.undo();
    assert_eq!(editor.text(), "a\nz");
}

#[test]
fn ime_partial_and_expanded_replacements_preserve_the_entire_draft() {
    let mut editor = editor("before old after");
    editor.select(7, 10);
    editor.mark(None, "abcdef", None);
    editor.mark(Some(9..11), "XY", None);
    assert_eq!(editor.text(), "before abXYef after");
    editor.commit_text(None, "Z");
    assert_eq!(editor.text(), "before abZef after");
    editor.undo();
    assert_eq!(editor.text(), "before old after");
    editor.select(7, 10);
    editor.mark(None, "draft", None);
    editor.mark(Some(6..13), "new", None);
    editor.commit_composition();
    assert_eq!(editor.text(), "beforenewafter");
    editor.undo();
    assert_eq!(editor.text(), "before old after");
}

#[test]
fn empty_and_trailing_lines_support_all_movements() {
    let mut editor = editor("");
    for movement in [
        Movement::Left,
        Movement::Right,
        Movement::Up,
        Movement::Down,
        Movement::WordLeft,
        Movement::WordRight,
    ] {
        editor.move_cursor(movement, false);
        assert_eq!(editor.caret(), 0);
    }
    editor.delete(true);
    editor.delete(false);
    assert!(!editor.buffer().can_undo());
    editor.insert("x\n", false);
    assert_eq!(editor.line_range(1), 2..2);
    editor.move_cursor(Movement::Up, false);
    assert_eq!(editor.caret(), 0);
}
