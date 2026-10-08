//! Document and project text search, result indexing, and replacement operations.

mod project;

pub use project::{
    BufferVersion, FileReplacement, FileResults, ProjectResults, ReplacementPreview, SourceSnapshot, apply_disk_replacement, search_project,
};

use regex::{Regex, RegexBuilder};
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub case_sensitive: bool,
    pub whole_word: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub options: Options,
}

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

pub struct Search {
    pattern: Option<Regex>,
    whole_word: bool,
}

impl Search {
    pub fn new(query: &Query) -> Result<Self, String> {
        let pattern = if query.text.is_empty() {
            None
        } else {
            Some(
                RegexBuilder::new(&regex::escape(&query.text))
                    .case_insensitive(!query.options.case_sensitive)
                    .build()
                    .map_err(|error| format!("Could not search: {error}"))?,
            )
        };
        Ok(Self {
            pattern,
            whole_word: query.options.whole_word,
        })
    }

    pub fn find(&self, text: &str) -> Vec<Range<usize>> {
        self.find_cancellable(text, &Cancellation::default())
    }

    pub fn find_cancellable(&self, text: &str, cancellation: &Cancellation) -> Vec<Range<usize>> {
        if cancellation.is_canceled() {
            return Vec::new();
        }
        let Some(pattern) = &self.pattern else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for matched in pattern.find_iter(text) {
            if cancellation.is_canceled() {
                return Vec::new();
            }
            let range = matched.range();
            if !self.accepts(text, &range) {
                continue;
            }
            found.push(range);
        }
        if cancellation.is_canceled() { Vec::new() } else { found }
    }

    fn accepts(&self, text: &str, range: &Range<usize>) -> bool {
        !self.whole_word
            || (!text[..range.start].chars().next_back().is_some_and(regex_syntax::is_word_character)
                && !text[range.end..].chars().next().is_some_and(regex_syntax::is_word_character))
    }
}

pub fn navigate(matches: &[Range<usize>], selected: &Range<usize>, caret: usize, backwards: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    if let Some(index) = matches
        .binary_search_by_key(&selected.start, |range| range.start)
        .ok()
        .filter(|&index| matches[index] == *selected)
    {
        return Some((index + if backwards { matches.len() - 1 } else { 1 }) % matches.len());
    }
    if backwards {
        let next = matches.partition_point(|range| range.start < caret);
        Some(if next == 0 { matches.len() - 1 } else { next - 1 })
    } else {
        let next = matches.partition_point(|range| range.start < caret);
        Some(if next == matches.len() { 0 } else { next })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacementEdit {
    pub range: Range<usize>,
    pub text: String,
    pub count: usize,
}

pub fn replacement_edit(text: &str, matches: &[Range<usize>], replacement: &str) -> Option<ReplacementEdit> {
    replacement_edit_cancellable(text, matches.iter().cloned(), replacement, &Cancellation::default())
}

pub(crate) fn replacement_edit_cancellable(
    text: &str, matches: impl Iterator<Item = Range<usize>> + Clone, replacement: &str, cancellation: &Cancellation,
) -> Option<ReplacementEdit> {
    if cancellation.is_canceled() {
        return None;
    }
    let mut ranges = matches.clone();
    let first = ranges.next()?;
    let mut range = first.clone();
    let mut removed = first.len();
    let mut count = 1;
    for matched in ranges {
        if cancellation.is_canceled() {
            return None;
        }
        range.end = matched.end;
        removed += matched.len();
        count += 1;
    }
    let mut result = String::with_capacity(range.len() - removed + replacement.len() * count);
    let mut offset = range.start;
    for matched in matches {
        if cancellation.is_canceled() {
            return None;
        }
        result.push_str(&text[offset..matched.start]);
        result.push_str(replacement);
        offset = matched.end;
    }
    result.push_str(&text[offset..range.end]);
    Some(ReplacementEdit { range, text: result, count })
}

pub fn relocate_match(original: &str, current: &str, range: &Range<usize>, search: &Search) -> Option<Range<usize>> {
    let prefix = original
        .chars()
        .zip(current.chars())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    let suffix = original[prefix..]
        .chars()
        .rev()
        .zip(current[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    let candidate = if range.end <= prefix {
        range.clone()
    } else if range.start >= original.len() - suffix {
        let distance = original.len() - range.start;
        let start = current.len().checked_sub(distance)?;
        start..start + range.len()
    } else {
        return None;
    };
    let matched = search
        .pattern
        .as_ref()?
        .find_iter(current)
        .find(|matched| matched.start() >= candidate.start)?;
    (matched.range() == candidate && search.accepts(current, &candidate)).then_some(candidate)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub range: Range<usize>,
    pub line: usize,
    pub column: usize,
    pub excerpt: String,
}
impl Match {
    pub fn new(text: &str, range: Range<usize>) -> Self {
        let line_start = text[..range.start].rfind('\n').map_or(0, |offset| offset + 1);
        let line = text[..range.start].bytes().filter(|&byte| byte == b'\n').count() + 1;
        let column = text[line_start..range.start].chars().count() + 1;
        let line_end = text[range.start..].find('\n').map_or(text.len(), |offset| range.start + offset);
        Self::at(text, range, line, column, line_start, line_end)
    }

    fn at(text: &str, range: Range<usize>, line: usize, column: usize, line_start: usize, line_end: usize) -> Self {
        let before = text[line_start..range.start]
            .char_indices()
            .rev()
            .take(60)
            .last()
            .map_or(range.start, |(offset, _)| line_start + offset);
        let after = text[range.start..line_end]
            .char_indices()
            .nth(120)
            .map_or(line_end, |(offset, _)| range.start + offset);
        Self {
            line,
            column,
            range,
            excerpt: text[before..after].trim_end_matches('\r').to_owned(),
        }
    }
}

pub fn describe_matches(text: &str, ranges: Vec<Range<usize>>) -> Vec<Match> {
    describe_matches_cancellable(text, ranges, &Cancellation::default())
}

pub(crate) fn describe_matches_cancellable(text: &str, ranges: Vec<Range<usize>>, cancellation: &Cancellation) -> Vec<Match> {
    let (mut offset, mut line, mut column) = (0, 1, 1);
    let mut line_start = 0;
    let mut line_end = text.find('\n').unwrap_or(text.len());
    ranges
        .into_iter()
        .take_while(|_| !cancellation.is_canceled())
        .map(|range| {
            for (index, ch) in text[offset..range.start].char_indices() {
                if ch == '\n' {
                    line += 1;
                    column = 1;
                    line_start = offset + index + 1;
                } else {
                    column += 1;
                }
            }
            if range.start > line_end {
                line_end = text[range.start..].find('\n').map_or(text.len(), |offset| range.start + offset);
            }
            offset = range.start;
            Match::at(text, range, line, column, line_start, line_end)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn replacement_builder_preserves_gaps_and_cancels_during_iteration() {
        let text = "α cat\ncat 😀";
        let ranges = [3..6, 7..10];
        for replacement in ["", "猫", "longer replacement"] {
            let edit = super::replacement_edit(text, &ranges, replacement).unwrap();
            assert_eq!(edit.range, 3..10);
            assert_eq!(edit.text, format!("{replacement}\n{replacement}"));
            assert_eq!(edit.count, 2);
        }
        let cancellation = super::Cancellation::default();
        let matches = ranges.into_iter().inspect(|_| cancellation.cancel());
        assert!(super::replacement_edit_cancellable(text, matches, "replacement", &cancellation).is_none());
    }

    use super::*;
    fn search(text: &str, case_sensitive: bool, whole_word: bool) -> Search {
        Search::new(&Query {
            text: text.into(),
            options: Options { case_sensitive, whole_word },
        })
        .unwrap()
    }
    #[test]
    fn unicode_case_and_word_boundaries_preserve_byte_offsets() {
        assert_eq!(search("é", false, true).find("É élan aé é"), vec![0..2, 13..15]);
        assert_eq!(search("e", true, true).find("e e\u{301} _e e2"), vec![0..1]);
        assert_eq!(search("猫", true, true).find("猫 子猫 猫"), vec![0..3, 11..14]);
        assert_eq!(search(".", true, false).find("a.b"), vec![1..2]);
    }

    #[test]
    fn whole_word_search_rejects_unicode_marks_connectors_numbers_and_join_controls() {
        let search = search("e", true, true);
        for neighbor in ['\u{301}', '\u{203f}', '\u{661}', '\u{200c}', '\u{200d}'] {
            assert!(search.find(&format!("{neighbor}e e{neighbor}")).is_empty());
        }
        assert_eq!(search.find("(e)"), vec![1..2]);
    }
    #[test]
    fn empty_queries_and_zero_matches_have_no_navigation_target() {
        assert!(search("", false, false).find("text").is_empty());
        assert_eq!(navigate(&[], &(0..0), 0, false), None);
        assert_eq!(navigate(&[], &(0..0), 0, true), None);
        let found = search("one", true, false).find("one two one");
        assert_eq!(navigate(&found, &(8..11), 11, false), Some(0));
        assert_eq!(navigate(&found, &(0..3), 3, true), Some(1));
    }
    #[test]
    fn replacement_containing_query_only_changes_original_matches() {
        let original = "cat cat";
        let edit = replacement_edit(original, &search("cat", true, false).find(original), "catcat").unwrap();
        assert_eq!(edit.text, "catcat catcat");
        assert_eq!(edit.count, 2);
    }
    #[test]
    fn moved_matches_relocate_and_deleted_matches_are_rejected() {
        let search = search("cat", true, false);
        assert_eq!(relocate_match("one cat", "猫 one cat", &(4..7), &search), Some(8..11));
        assert_eq!(relocate_match("one cat", "one dog", &(4..7), &search), None);
    }

    #[test]
    fn excerpts_limit_unicode_context_and_trim_crlf() {
        let text = format!("{}cat{}\r\ncat\r\n", "猫".repeat(80), "😀".repeat(140));
        let matches = describe_matches(&text, search("cat", true, false).find(&text));
        assert_eq!(matches[0].excerpt, format!("{}cat{}", "猫".repeat(60), "😀".repeat(117)));
        assert_eq!((matches[0].line, matches[0].column), (1, 81));
        assert_eq!(matches[1].excerpt, "cat");
        assert_eq!((matches[1].line, matches[1].column), (2, 1));
    }
}
