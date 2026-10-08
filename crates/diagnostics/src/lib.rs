//! Source ranges, severity, and diagnostic collections shared by build and language services.

mod tex;
pub use tex::parse_tex;

use std::{collections::BTreeMap, ops::Range, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct BuildId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Information,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provider {
    Build(BuildId),
    LanguageServer(String),
}

/// One-based lines and Unicode scalar columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourcePosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceRange {
    /// Inclusive line bounds, without column precision.
    Lines { start: usize, end: usize },
    /// Start inclusive, end exclusive.
    Text { start: SourcePosition, end: SourcePosition },
}

impl SourceRange {
    pub fn byte_range(&self, text: &str) -> Option<Range<usize>> {
        let line = |number: usize| {
            let mut start = 0;
            for _ in 1..number {
                start += text.get(start..)?.find('\n')? + 1;
            }
            if number == 0 {
                return None;
            }
            let rest = text.get(start..)?;
            let end = start + rest.find('\n').unwrap_or(rest.len());
            let end = if text[..end].ends_with('\r') { end - 1 } else { end };
            Some(start..end)
        };
        let position = |position: SourcePosition, range: Range<usize>| {
            let content = &text[range.clone()];
            let column = position.column.checked_sub(1)?;
            let offset = content
                .char_indices()
                .map(|(offset, _)| offset)
                .chain(std::iter::once(content.len()))
                .nth(column)?;
            Some(range.start + offset)
        };
        let range = match self {
            Self::Lines { start, end } => {
                let first = line(*start)?;
                let last = if start == end { first.clone() } else { line(*end)? };
                first.start..last.end
            }
            Self::Text { start, end } => {
                let first = line(start.line)?;
                let last = if start.line == end.line { first.clone() } else { line(end.line)? };
                position(*start, first)?..position(*end, last)?
            }
        };
        (range.start <= range.end).then_some(range)
    }

    pub fn first_line(&self) -> usize {
        match self {
            Self::Lines { start, .. } => *start,
            Self::Text { start, .. } => start.line,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    pub severity: Severity,
    pub provider: Provider,
    pub message: String,
    pub path: Option<PathBuf>,
    pub range: Option<SourceRange>,
}

pub fn grouped(diagnostics: &[Diagnostic]) -> BTreeMap<(Option<PathBuf>, Severity), Vec<&Diagnostic>> {
    let mut groups = BTreeMap::new();
    for diagnostic in diagnostics {
        groups
            .entry((diagnostic.path.clone(), diagnostic.severity))
            .or_insert_with(Vec::new)
            .push(diagnostic);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_preserve_line_precision_and_validate_unicode_columns() {
        let text = "first\r\nα😀last\r\n";
        assert_eq!(SourceRange::Lines { start: 2, end: 2 }.byte_range(text), Some(7..17));
        assert_eq!(SourceRange::Lines { start: 3, end: 3 }.byte_range(text), Some(19..19));
        assert_eq!(SourceRange::Lines { start: 4, end: 4 }.byte_range(text), None);
        let range = SourceRange::Text {
            start: SourcePosition { line: 2, column: 2 },
            end: SourcePosition { line: 2, column: 3 },
        };
        assert_eq!(&text[range.byte_range(text).unwrap()], "😀");
        assert_eq!(SourceRange::Lines { start: 0, end: 1 }.byte_range(text), None);
    }

    #[test]
    fn exact_locations_accept_line_end_and_reject_invalid_columns() {
        let text = "first\r\nα😀\r\nlast";
        let range = |start, end| SourceRange::Text { start, end }.byte_range(text);
        let start = SourcePosition { line: 2, column: 2 };
        assert_eq!(range(start, SourcePosition { line: 2, column: 3 }), Some(9..13));
        assert_eq!(range(start, SourcePosition { line: 3, column: 1 }), Some(9..15));
        assert_eq!(range(start, SourcePosition { line: 2, column: 4 }), None);
        assert_eq!(range(SourcePosition { line: 2, column: 0 }, start), None);
        assert_eq!(range(start, SourcePosition { line: 1, column: 1 }), None);
    }
}
