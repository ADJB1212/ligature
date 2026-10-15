use std::sync::Arc;

use ligature_buffer::{Buffer, ByteOffset};
use ligature_syntax::{Category, Language, Snapshot, Token};

fn snapshot(language: Language, text: &str) -> Snapshot {
    let buffer = Buffer::from_text(text);
    Snapshot::parse(buffer.id(), buffer.revision(), language, buffer.text().into())
}

fn tex(text: &str) -> Snapshot {
    snapshot(Language::Tex, text)
}

fn pieces(snapshot: &Snapshot, category: Category) -> Vec<&str> {
    snapshot
        .tokens
        .iter()
        .filter(|token| token.category == category)
        .map(|token| &snapshot.text[token.range.clone()])
        .collect()
}

fn assert_well_formed(snapshot: &Snapshot) {
    let mut previous_end = 0;
    for Token { range, .. } in &snapshot.tokens {
        assert!(range.start < range.end, "empty token {range:?}");
        assert!(range.start >= previous_end, "overlapping token {range:?}");
        assert!(snapshot.text.get(range.clone()).is_some(), "invalid boundary {range:?}");
        previous_end = range.end;
    }
}

#[test]
fn plain_documents_have_no_tokens_and_are_fully_reparsed() {
    let snapshot = snapshot(Language::Plain, "\\section{x} % not tex\n");
    assert!(snapshot.tokens.is_empty());
    assert_eq!(snapshot.reparsed, 0..snapshot.text.len());
}

#[test]
fn empty_documents_have_no_tokens() {
    for language in [Language::Tex, Language::Bibliography, Language::Plain] {
        assert!(snapshot(language, "").tokens.is_empty());
    }
}

#[test]
fn a_simple_command_is_split_into_command_braces_and_text() {
    let snapshot = tex("\\section{Intro}");
    let tokens: Vec<_> = snapshot
        .tokens
        .iter()
        .map(|token| (token.category, &snapshot.text[token.range.clone()]))
        .collect();
    assert_eq!(
        tokens,
        [
            (Category::Command, "\\section"),
            (Category::Brace, "{"),
            (Category::Text, "Intro"),
            (Category::Brace, "}"),
        ]
    );
}

#[test]
fn control_symbols_escape_special_characters() {
    let snapshot = tex("\\% \\{ \\} \\$ \\\\ \\& done");
    assert_eq!(pieces(&snapshot, Category::Comment), Vec::<&str>::new());
    assert!(pieces(&snapshot, Category::Brace).is_empty());
    assert!(pieces(&snapshot, Category::MathDelimiter).is_empty());
    assert_eq!(pieces(&snapshot, Category::Command), ["\\%", "\\{", "\\}", "\\$", "\\\\", "\\&"]);
}

#[test]
fn comments_end_before_the_newline() {
    let snapshot = tex("a % one\nb %two");
    assert_eq!(pieces(&snapshot, Category::Comment), ["% one", "%two"]);
}

#[test]
fn display_and_inline_math_delimiters_are_recognized() {
    let snapshot = tex("$a$ $$b$$ \\(c\\) \\[d\\]");
    assert_eq!(
        pieces(&snapshot, Category::MathDelimiter),
        ["$", "$", "$$", "$$", "\\(", "\\)", "\\[", "\\]"]
    );
}

#[test]
fn inline_verbatim_uses_any_delimiter_and_stops_at_the_line_end() {
    let snapshot = tex("\\verb|a{b|\\verb*+c d+ \\lstinline!x!\n\\verb|unterminated\nnext");
    assert_eq!(pieces(&snapshot, Category::Verbatim), ["|a{b|", "*+c d+", "!x!", "|unterminated"]);
    assert!(pieces(&snapshot, Category::Brace).is_empty());
    assert_well_formed(&snapshot);
}

#[test]
fn long_unicode_text_preserves_tokens_at_scan_boundaries() {
    for length in [0, 15, 16, 31, 32, 33, 255, 256, 257, 287, 288, 511, 512, 1024] {
        let prefix = format!("{}日本語", "a".repeat(length));
        for (delimiter, category) in [
            ("{", Category::Brace),
            ("}", Category::Brace),
            ("$", Category::MathDelimiter),
            ("\\cmd", Category::Command),
            ("% comment", Category::Comment),
        ] {
            let text = format!("{prefix}{delimiter} 尾");
            let snapshot = tex(&text);
            assert_well_formed(&snapshot);
            assert_eq!(snapshot.tokens[0].range, 0..prefix.len());
            assert_eq!(snapshot.tokens[1].category, category);
            assert_eq!(snapshot.tokens[1].range.start, prefix.len());
            assert_eq!(snapshot.tokens.last().unwrap().range.end, text.len());
        }
        let snapshot = tex(&format!("{prefix}\n尾"));
        assert_eq!(pieces(&snapshot, Category::Text), [format!("{prefix}\n"), "尾".to_owned()]);
    }
}

#[test]
fn long_inline_verbatim_preserves_ascii_and_unicode_delimiters() {
    let body = "日本語 { \\ $ % ".repeat(128);
    for delimiter in ['|', '!', 'é', '😀'] {
        let snapshot = tex(&format!("\\verb{delimiter}{body}{delimiter}\\label{{visible}}"));
        assert_eq!(pieces(&snapshot, Category::Verbatim), [format!("{delimiter}{body}{delimiter}")]);
        assert_eq!(pieces(&snapshot, Category::Command), ["\\verb", "\\label"]);
        assert_well_formed(&snapshot);
        let snapshot = tex(&format!("\\verb{delimiter}{body}\nnext"));
        assert_eq!(pieces(&snapshot, Category::Verbatim), [format!("{delimiter}{body}")]);
        assert_well_formed(&snapshot);
    }
}

#[test]
fn verbatim_without_a_delimiter_consumes_nothing() {
    let snapshot = tex("\\verb \\section{x}");
    assert!(pieces(&snapshot, Category::Verbatim).is_empty());
    assert!(pieces(&snapshot, Category::Command).contains(&"\\section"));
}

#[test]
fn verbatim_environments_are_opaque_until_their_matching_end() {
    for name in ["verbatim", "verbatim*", "Verbatim", "lstlisting", "minted", "comment"] {
        let text = format!("\\begin{{{name}}}\n\\section{{hidden}} %\n\\end{{{name}}}\\label{{shown}}");
        let snapshot = tex(&text);
        assert_eq!(pieces(&snapshot, Category::Environment), [name, name], "{name}");
        let hidden = pieces(&snapshot, Category::Verbatim);
        assert_eq!(hidden.len(), 1, "{name}");
        assert!(hidden[0].contains("\\section{hidden}"), "{name}");
        assert!(pieces(&snapshot, Category::Command).contains(&"\\label"), "{name}");
        assert_well_formed(&snapshot);
    }
}

#[test]
fn unclosed_verbatim_environment_extends_to_the_end_of_the_document() {
    let text = "\\begin{verbatim}\n\\section{x}\n$ never closed";
    let snapshot = tex(text);
    let verbatim = pieces(&snapshot, Category::Verbatim);
    assert_eq!(verbatim, ["\n\\section{x}\n$ never closed"]);
}

#[test]
fn a_verbatim_environment_only_closes_on_its_own_name() {
    let snapshot = tex("\\begin{verbatim}\\end{lstlisting}\\end{verbatim}\\ref{x}");
    assert_eq!(pieces(&snapshot, Category::Verbatim), ["\\end{lstlisting}"]);
}

#[test]
fn begin_and_end_with_malformed_names_do_not_create_environment_tokens() {
    for text in ["\\begin", "\\begin{", "\\begin{unclosed\nname}", "\\end{a%b}", "\\begin x"] {
        assert!(pieces(&tex(text), Category::Environment).is_empty(), "{text:?}");
    }
}

#[test]
fn whitespace_between_begin_and_name_is_tolerated() {
    let snapshot = tex("\\begin {center}\\end\n{center}");
    assert_eq!(pieces(&snapshot, Category::Environment), ["center", "center"]);
}

#[test]
fn lone_trailing_backslash_is_a_command_token() {
    let snapshot = tex("text \\");
    assert_eq!(pieces(&snapshot, Category::Command), ["\\"]);
}

#[test]
fn unicode_text_and_commands_keep_char_boundaries() {
    let snapshot = tex("日本語 \\命令 😀 \\é{ü}\n");
    assert_well_formed(&snapshot);
    assert!(pieces(&snapshot, Category::Text).iter().any(|text| text.contains("日本語")));
}

#[test]
fn crlf_documents_tokenize_without_gaps_or_overlaps() {
    let snapshot = tex("\\section{A}\r\n% c\r\n$x$\r\n");
    assert_eq!(pieces(&snapshot, Category::Comment), ["% c\r"]);
    assert_well_formed(&snapshot);
}

#[test]
fn bibliography_language_maps_entry_parts_to_categories() {
    let snapshot = snapshot(Language::Bibliography, "% c\n@book{k, title={T}, year=2026}");
    assert_eq!(pieces(&snapshot, Category::EntryType), ["@book"]);
    assert_eq!(pieces(&snapshot, Category::Field), ["title", "year"]);
    assert_eq!(pieces(&snapshot, Category::Value), ["{T}", "2026"]);
    assert_eq!(pieces(&snapshot, Category::Comment), ["% c"]);
    assert_well_formed(&snapshot);
}

#[test]
fn snapshot_identity_follows_document_revision_and_language() {
    let mut buffer = Buffer::from_text("a");
    let snapshot = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, buffer.text().into());
    assert!(snapshot.is_current(buffer.id(), buffer.revision(), Language::Tex));
    assert!(!snapshot.is_current(buffer.id(), buffer.revision(), Language::Bibliography));
    buffer.type_text(ByteOffset(1), "b").unwrap();
    assert!(!snapshot.is_current(buffer.id(), buffer.revision(), Language::Tex));
}

#[test]
fn update_retains_language_and_document_identity() {
    let mut buffer = Buffer::from_text("one\ntwo");
    let old = Snapshot::parse(buffer.id(), buffer.revision(), Language::Bibliography, buffer.text().into());
    buffer.type_text(ByteOffset(7), "!").unwrap();
    let updated = old.update(buffer.revision(), 7..7, "!");
    assert!(updated.is_current(buffer.id(), buffer.revision(), Language::Bibliography));
    assert_eq!(&*updated.text, "one\ntwo!");
}

#[test]
fn refreshing_with_identical_text_yields_identical_tokens() {
    let old = tex("\\section{A}\n$x$\n% c\nlast");
    let refreshed = old.refresh(old.revision, Arc::clone(&old.text));
    assert_eq!(refreshed.tokens, old.tokens);
    assert_eq!(refreshed.reparsed.end, old.text.len());
}

#[test]
fn refresh_handles_total_replacement_and_truncation_to_empty() {
    let old = tex("\\section{A}\nbody");
    let replaced = old.refresh(old.revision, "completely different\n$x$".into());
    assert_eq!(replaced.tokens, tex("completely different\n$x$").tokens);
    assert_eq!(replaced.reparsed.start, 0);
    let emptied = old.refresh(old.revision, "".into());
    assert!(emptied.tokens.is_empty());
}

#[test]
fn refresh_never_splits_a_multibyte_character_when_prefixes_overlap_partially() {
    let old = tex("猫a");
    let refreshed = old.refresh(old.revision, "犬a".into());
    assert_eq!(refreshed.tokens, tex("犬a").tokens);
}

#[test]
fn incremental_updates_match_full_parses_for_verbatim_math_and_comment_edits() {
    let initial = "\\section{A}\n$x$ % c\n\\verb|q|\n\\begin{lstlisting}\ncode\n\\end{lstlisting}\n$$y$$\n";
    let edits = ["", "\n", "%", "$", "\\", "{", "}", "|", "\\end{lstlisting}", "\\begin{verbatim}", "日"];
    for start in initial.char_indices().map(|(index, _)| index).chain([initial.len()]) {
        for replacement in edits {
            let mut buffer = Buffer::from_text(initial);
            let old = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, initial.into());
            let end = initial[start..].chars().next().map_or(start, |c| start + c.len_utf8());
            buffer
                .replace(buffer.text_range(ByteOffset(start), ByteOffset(end)).unwrap(), replacement)
                .unwrap();
            let incremental = old.update(buffer.revision(), start..end, replacement);
            let full = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, buffer.text().into());
            assert_eq!(incremental.tokens, full.tokens, "{start} {replacement:?}");
        }
    }
}

#[test]
fn sequential_updates_do_not_accumulate_drift() {
    let mut buffer = Buffer::from_text("\\section{A}\nbody\n");
    let mut snapshot = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, buffer.text().into());
    for (offset, text) in [(0, "% "), (2, "$"), (3, "x$ "), (6, "\\verb|"), (12, "|"), (13, "\n\\begin{verbatim}")] {
        let offset = offset.min(buffer.text().len());
        buffer.type_text(ByteOffset(offset), text).unwrap();
        snapshot = snapshot.update(buffer.revision(), offset..offset, text);
        let full = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, buffer.text().into());
        assert_eq!(snapshot.tokens, full.tokens, "after inserting {text:?} at {offset}");
    }
}

#[test]
#[should_panic]
fn updating_past_the_end_panics() {
    let snapshot = tex("abc");
    snapshot.update(snapshot.revision, 2..9, "");
}

#[test]
#[should_panic]
fn updating_inside_a_multibyte_character_panics() {
    let snapshot = tex("猫");
    snapshot.update(snapshot.revision, 1..1, "x");
}
