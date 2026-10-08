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
        let position = |position: SourcePosition| {
            let range = line(position.line)?;
            let content = &text[range.clone()];
            let column = position.column.checked_sub(1)?;
            let offset = if column == content.chars().count() {
                content.len()
            } else {
                content.char_indices().nth(column)?.0
            };
            Some(range.start + offset)
        };
        let range = match self {
            Self::Lines { start, end } => line(*start)?.start..line(*end)?.end,
            Self::Text { start, end } => position(*start)?..position(*end)?,
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
}
