use std::path::PathBuf;

use ligature_bibliography::{Bibliography, Category, Index, parse};

fn keys(bibliography: &Bibliography) -> Vec<&str> {
    bibliography.entries.iter().map(|entry| entry.key.as_str()).collect()
}

fn highlighted<'a>(text: &'a str, bibliography: &Bibliography, category: Category) -> Vec<&'a str> {
    bibliography
        .highlights
        .iter()
        .filter(|highlight| highlight.category == category)
        .map(|highlight| &text[highlight.range.clone()])
        .collect()
}

#[test]
fn empty_and_entry_free_text_produces_nothing() {
    for text in ["", "   \n\t", "plain prose with no entries", "mail me @ home", "@", "@@@"] {
        let result = parse(text);
        assert!(result.entries.is_empty(), "{text:?}");
        assert!(result.malformed.is_empty(), "{text:?}");
    }
}

#[test]
fn parenthesized_entries_are_equivalent_to_braced_entries() {
    let result = parse("@book(key, title={One}, year=2026)");
    assert_eq!(keys(&result), ["key"]);
    assert_eq!(result.entries[0].title(), Some("One"));
    assert_eq!(result.entries[0].year(), Some("2026"));
    assert!(result.malformed.is_empty());
}

#[test]
fn entry_types_and_field_names_are_lowercased_but_keys_are_preserved() {
    let result = parse("@ARTICLE{MixedCase:Key.1, TITLE={T}, Author={A}}");
    let entry = &result.entries[0];
    assert_eq!(entry.entry_type, "article");
    assert_eq!(entry.key, "MixedCase:Key.1");
    assert_eq!(entry.title(), Some("T"));
    assert_eq!(entry.author(), Some("A"));
}

#[test]
fn entry_and_key_ranges_slice_the_source() {
    let text = "junk\n@book{first, title={One}}\n% note\n@misc{second}";
    let result = parse(text);
    assert_eq!(&text[result.entries[0].range.clone()], "@book{first, title={One}}");
    assert_eq!(&text[result.entries[0].key_range.clone()], "first");
    assert_eq!(&text[result.entries[1].range.clone()], "@misc{second}");
    assert_eq!(&text[result.entries[1].key_range.clone()], "second");
}

#[test]
fn trailing_commas_comments_and_whitespace_between_fields_are_accepted() {
    let text = "@book{k, % about the key\n  title = {X} , % trailing\n  year\n=\n2026,\n}";
    let result = parse(text);
    assert!(result.malformed.is_empty());
    assert_eq!(result.entries[0].title(), Some("X"));
    assert_eq!(result.entries[0].year(), Some("2026"));
}

#[test]
fn year_falls_back_to_date_and_prefers_year() {
    let date_only = parse("@book{a, date={2020-05-01}}");
    assert_eq!(date_only.entries[0].year(), Some("2020-05-01"));
    let both = parse("@book{a, date={2020-05-01}, year={2019}}");
    assert_eq!(both.entries[0].year(), Some("2019"));
    assert_eq!(parse("@book{a}").entries[0].year(), None);
}

#[test]
fn concatenation_mixes_literals_numbers_and_strings() {
    let result = parse("@string{venue = {Proc.}}\n@inproceedings{k, booktitle = venue # \" of \" # {the} # { } # 2026}");
    assert_eq!(result.entries[0].fields["booktitle"], "Proc. of the 2026");
}

#[test]
fn string_names_are_case_insensitive_and_unknown_names_stay_literal() {
    let result = parse("@STRING{Journal = {Nature}}\n@article{k, journal = JOURNAL, note = undefined}");
    assert_eq!(result.entries[0].fields["journal"], "Nature");
    assert_eq!(result.entries[0].fields["note"], "undefined");
    assert_eq!(result.strings["journal"], "Nature");
}

#[test]
fn builtin_month_names_expand_and_are_exposed() {
    let result = parse("@misc{k, month = dec}");
    assert_eq!(result.entries[0].fields["month"], "December");
    assert_eq!(result.strings.len(), 12);
    assert_eq!(result.strings["sep"], "September");
}

#[test]
fn braces_quotes_and_escapes_are_preserved_inside_values() {
    let result = parse(r#"@book{k, a={x \} y}, b="say {"} hi", c={{Deep {Nest}} text}, d="esc \" quote", e={50\% off}}"#);
    let fields = &result.entries[0].fields;
    assert_eq!(fields["a"], r"x \} y");
    assert_eq!(fields["b"], r#"say {"} hi"#);
    assert_eq!(fields["c"], "{Deep {Nest}} text");
    assert_eq!(fields["d"], r#"esc \" quote"#);
    assert_eq!(fields["e"], r"50\% off");
    assert!(result.malformed.is_empty());
}

#[test]
fn long_values_preserve_nested_braces_escapes_and_unicode() {
    for length in [0, 15, 16, 17, 255, 256, 257, 271, 272, 511, 512, 1024] {
        let value = format!("{} {{nested}} \\}} \\😀 \\\" user@example.org", "é".repeat(length));
        for (open, close) in [('{', '}'), ('"', '"')] {
            let text = format!("@book{{key,title={open}{value}{close},year=2026}}");
            let parsed = parse(&text);
            assert!(parsed.malformed.is_empty(), "{length} {open}");
            assert_eq!(parsed.entries[0].title(), Some(value.as_str()));
            assert_eq!(parsed.entries[0].year(), Some("2026"));
            assert_eq!(highlighted(&text, &parsed, Category::Value)[0], format!("{open}{value}{close}"));
        }
    }
}

#[test]
fn entry_discovery_skips_long_prose_and_preserves_comments() {
    let text = format!(
        "{}% @book{{hidden}}\n@book{{visible,title={{日本語}}}}",
        "plain prose 日本語 ".repeat(1024)
    );
    let parsed = parse(&text);
    assert_eq!(keys(&parsed), ["visible"]);
    assert_eq!(highlighted(&text, &parsed, Category::Comment), ["% @book{hidden}"]);
}

#[test]
fn comment_and_preamble_blocks_are_skipped_even_with_nested_braces() {
    let text = "@comment{ @book{hidden, title={x}} {nested} }\n@preamble{ \"\\newcommand{\\a}{b}\" }\n@book{shown}";
    let result = parse(text);
    assert_eq!(keys(&result), ["shown"]);
    assert!(result.malformed.is_empty());
    assert!(!highlighted(text, &result, Category::Comment).is_empty());
}

#[test]
fn text_between_entries_is_ignored() {
    let result = parse("preface\n@book{a}\nmiddle prose { unbalanced\n@book{b}\ntail");
    assert_eq!(keys(&result), ["a", "b"]);
}

#[test]
fn entries_without_keys_are_malformed_and_not_indexed() {
    for text in ["@book{, title={x}}", "@book{}", "@book{ ,}"] {
        let result = parse(text);
        assert!(result.entries.is_empty(), "{text:?}");
        assert_eq!(result.malformed.len(), 1, "{text:?}");
    }
}

#[test]
fn missing_equals_and_missing_values_mark_the_entry_malformed_but_keep_it() {
    for text in ["@book{k, title {x}}", "@book{k, title=}", "@book{k, title={x} year={1}}"] {
        let result = parse(&format!("{text}\n@book{{after, title={{ok}}}}"));
        assert!(result.entries.iter().any(|entry| entry.key == "k"), "{text:?}");
        let after = result.entries.iter().find(|entry| entry.key == "after").unwrap();
        assert_eq!(after.title(), Some("ok"), "{text:?}");
        assert!(!result.malformed.is_empty(), "{text:?}");
    }
}

#[test]
fn unterminated_entries_at_end_of_input_do_not_panic() {
    for text in [
        "@book{k, title={abc",
        "@book{k, title=\"abc",
        "@book{k,",
        "@book{k",
        "@book(",
        "@string{a=",
        "@string{a={x",
    ] {
        let result = parse(text);
        assert!(result.entries.len() <= 1, "{text:?}");
    }
}

#[test]
fn string_definitions_may_reference_later_and_self_referential_definitions_terminate() {
    let result = parse("@string{a = b # {-a}}\n@string{b = {B}}\n@string{loop = loop # {!}}\n@misc{k, x = a, y = loop}");
    assert_eq!(result.entries[0].fields["x"], "B-a");
    assert!(result.entries[0].fields["y"].ends_with('!'));
}

#[test]
fn malformed_string_definitions_are_reported() {
    let result = parse("@string{broken = }\n@book{k}");
    assert_eq!(keys(&result), ["k"]);
    assert_eq!(result.malformed.len(), 1);
}

#[test]
fn highlights_cover_entry_types_fields_values_and_comments() {
    let text = "% leading\n@book{key, title = {Hello}, year = 2026}";
    let result = parse(text);
    assert_eq!(highlighted(text, &result, Category::EntryType), ["@book"]);
    assert_eq!(highlighted(text, &result, Category::Field), ["title", "year"]);
    assert_eq!(highlighted(text, &result, Category::Value), ["{Hello}", "2026"]);
    assert_eq!(highlighted(text, &result, Category::Comment), ["% leading"]);
    assert!(result.highlights.iter().all(|highlight| !highlight.range.is_empty()));
}

#[test]
fn recovery_stops_an_unclosed_value_at_the_next_entry_on_its_own_line() {
    let text = "@book{a, title={never closed\n@book{b, title={ok}}";
    let result = parse(text);
    assert_eq!(keys(&result), ["a", "b"]);
    assert_eq!(result.entries[1].title(), Some("ok"));
    assert_eq!(result.malformed.len(), 1);
}

#[test]
fn at_signs_inside_values_do_not_end_them_unless_they_look_like_entries_at_line_start() {
    let result = parse("@misc{k, note={mail a@b.org and\n  @home}}");
    assert!(result.malformed.is_empty());
    assert_eq!(result.entries[0].fields["note"], "mail a@b.org and\n  @home");
}

#[test]
fn every_prefix_of_a_complex_file_parses_with_valid_ranges() {
    let text = "% 日本\n@string{j = {J}}\n@article{日本語, title = j # {é}, author = \"A {B} \\\" C\", year = 2026,}\n@comment{x}\n@book(k, t={😀})\n@misc{m, n=1 # j}\n";
    for end in text.char_indices().map(|(index, _)| index).chain([text.len()]) {
        let prefix = &text[..end];
        let result = parse(prefix);
        for entry in &result.entries {
            assert!(prefix.get(entry.range.clone()).is_some(), "{end}: {:?}", entry.range);
            assert!(prefix.get(entry.key_range.clone()).is_some(), "{end}: {:?}", entry.key_range);
            assert!(entry.key_range.start >= entry.range.start && entry.key_range.end <= entry.range.end);
        }
        for highlight in &result.highlights {
            assert!(prefix.get(highlight.range.clone()).is_some(), "{end}: {:?}", highlight.range);
        }
        for range in &result.malformed {
            assert!(prefix.get(range.clone()).is_some(), "{end}: {range:?}");
        }
    }
}

#[test]
fn index_preserves_insertion_order_across_files_and_reports_only_duplicates() {
    let mut index = Index::default();
    index.insert(PathBuf::from("a.bib"), &parse("@book{shared, title={A}}\n@book{unique}"));
    index.insert(PathBuf::from("b.bib"), &parse("@book{shared, title={B}}"));
    let definitions = index.resolve("shared");
    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0].path, PathBuf::from("a.bib"));
    assert_eq!(definitions[1].entry.title(), Some("B"));
    assert_eq!(index.resolve("unique").len(), 1);
    assert_eq!(index.duplicates().map(|(key, _)| key).collect::<Vec<_>>(), ["shared"]);
}

#[test]
fn index_keys_are_case_sensitive() {
    let mut index = Index::default();
    index.insert(PathBuf::from("a.bib"), &parse("@book{Key}"));
    assert_eq!(index.resolve("Key").len(), 1);
    assert!(index.resolve("key").is_empty());
}
