use ligature_search::{Cancellation, Match, Options, Query, Search, describe_matches, navigate, relocate_match, replacement_edit};

fn search(query: &str, whole_word: bool) -> Search {
    Search::new(&Query {
        text: query.into(),
        options: Options {
            case_sensitive: true,
            whole_word,
        },
    })
    .unwrap()
}

#[test]
fn regex_metacharacters_are_searched_literally() {
    for query in ["[a-z]+", "\\input{file}", "^.*$", "(cat|dog)?"] {
        let text = format!("before {query} after {query}");
        let second = 7 + query.len() + 7;
        assert_eq!(search(query, false).find(&text), vec![7..7 + query.len(), second..second + query.len()]);
    }
}

#[test]
fn overlapping_candidates_produce_nonoverlapping_matches() {
    assert_eq!(search("aa", false).find("aaaaa"), vec![0..2, 2..4]);
    assert_eq!(search("猫猫", false).find("猫猫猫"), vec![0..6]);
}

#[test]
fn cancellation_is_shared_and_suppresses_matches() {
    let cancellation = Cancellation::default();
    let worker = cancellation.clone();
    assert_eq!(search("cat", false).find_cancellable("cat cat", &worker), vec![0..3, 4..7]);
    cancellation.cancel();
    assert!(worker.is_canceled());
    assert!(search("cat", false).find_cancellable("cat cat", &worker).is_empty());
}

#[test]
fn navigation_from_a_caret_respects_direction_and_wraps() {
    let matches = vec![2..5, 8..11, 14..17];
    for (caret, forward, backward) in [(0, 0, 2), (2, 0, 2), (4, 1, 0), (8, 1, 0), (12, 2, 1), (17, 0, 2)] {
        let selection = caret..caret;
        assert_eq!(navigate(&matches, &selection, caret, false), Some(forward));
        assert_eq!(navigate(&matches, &selection, caret, true), Some(backward));
    }
}

#[test]
fn a_selection_with_only_a_matching_start_does_not_skip_the_match() {
    let matches = vec![2..5, 8..11];
    assert_eq!(navigate(&matches, &(2..4), 2, false), Some(0));
    assert_eq!(navigate(&matches, &(2..5), 2, false), Some(1));
}

#[test]
fn navigation_with_one_match_always_returns_that_match() {
    let matched = 2..5;
    let matches = std::slice::from_ref(&matched);
    for backwards in [false, true] {
        assert_eq!(navigate(matches, &(2..5), 5, backwards), Some(0));
        assert_eq!(navigate(matches, &(0..0), 0, backwards), Some(0));
    }
}

#[test]
fn replacement_preserves_unicode_gaps_and_surrounding_text() {
    let original = "prefix 猫\r\nβ 猫 suffix";
    let matches = search("猫", false).find(original);
    let edit = replacement_edit(original, &matches, "🦀\n").unwrap();
    assert_eq!(edit.range, 7..18);
    assert_eq!(edit.text, "🦀\n\r\nβ 🦀\n");
    assert_eq!(edit.count, 2);
    let mut replaced = original.to_owned();
    replaced.replace_range(edit.range, &edit.text);
    assert_eq!(replaced, "prefix 🦀\n\r\nβ 🦀\n suffix");
}

#[test]
fn empty_replacement_deletes_only_matched_text() {
    let edit = replacement_edit("cat 猫 cat", &[0..3, 8..11], "").unwrap();
    assert_eq!(edit.range, 0..11);
    assert_eq!(edit.text, " 猫 ");
    assert_eq!(edit.count, 2);
    assert_eq!(replacement_edit("cat", &[], "dog"), None);
}

#[test]
fn relocation_rechecks_whole_word_boundaries() {
    let search = search("cat", true);
    assert_eq!(relocate_match("cat tail", "cat! tail", &(0..3), &search), Some(0..3));
    assert_eq!(relocate_match("cat tail", "cats tail", &(0..3), &search), None);
    assert_eq!(relocate_match("head cat", "head scat", &(5..8), &search), None);
    assert_eq!(relocate_match("head cat", "cat", &(5..8), &search), Some(0..3));
}

#[test]
fn relocation_preserves_nonoverlapping_match_selection() {
    let search = search("aa", false);
    assert_eq!(relocate_match("xaaa", "aaaa", &(1..3), &search), None);
    assert_eq!(relocate_match("xaaaa", "aaaaa", &(3..5), &search), None);
    assert_eq!(relocate_match("xaaaa", "y aaaa", &(3..5), &search), Some(4..6));
}

#[test]
fn match_descriptions_use_scalar_columns_and_strip_crlf_terminators() {
    let text = "é猫 cat\r\n\r\nβ cat\ncat";
    let ranges = search("cat", false).find(text);
    let described = describe_matches(text, ranges.clone());
    assert_eq!(described, ranges.into_iter().map(|range| Match::new(text, range)).collect::<Vec<_>>());
    assert_eq!(
        described.iter().map(|matched| (matched.line, matched.column)).collect::<Vec<_>>(),
        vec![(1, 4), (3, 3), (4, 1)]
    );
    assert_eq!(
        described.iter().map(|matched| matched.excerpt.as_str()).collect::<Vec<_>>(),
        vec!["é猫 cat", "β cat", "cat"]
    );
}

#[test]
fn sparse_match_descriptions_preserve_unicode_columns_across_large_gaps() {
    let text = format!("{}猫 hit 😀 hit\r\n{}尾 hit\n", "日本語\r\n\n".repeat(2048), "é".repeat(4096));
    let ranges = search("hit", false).find(&text);
    let described = describe_matches(&text, ranges.clone());
    assert_eq!(described, ranges.into_iter().map(|range| Match::new(&text, range)).collect::<Vec<_>>());
    assert_eq!(
        described.iter().map(|matched| (matched.line, matched.column)).collect::<Vec<_>>(),
        [(4097, 3), (4097, 9), (4098, 4099)]
    );
}

#[test]
fn match_excerpts_are_bounded_by_unicode_scalars() {
    let text = format!("{}cat{}", "猫".repeat(80), "é".repeat(150));
    let matched = Match::new(&text, 240..243);
    assert_eq!(matched.column, 81);
    assert_eq!(matched.excerpt, format!("{}cat{}", "猫".repeat(60), "é".repeat(117)));
}
