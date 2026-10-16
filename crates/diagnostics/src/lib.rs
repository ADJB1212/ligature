//! Source ranges, severity, and diagnostic collections shared by build and language services.

mod tex;
pub use tex::parse_tex;

use std::{collections::BTreeMap, ops::Range, path::PathBuf};

pub const MAX_DIAGNOSTICS: usize = 1000;
pub const MAX_MESSAGE_BYTES: usize = 4096;
const MAX_STORE_DIAGNOSTICS: usize = 10_000;

pub fn bound_message(message: &mut String) {
    let mut end = message.len().min(MAX_MESSAGE_BYTES);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
}

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
        let (mut line, mut start) = (1, 0);
        self.byte_range_with_lines(text, |number| {
            if number == 0 || number < line {
                return None;
            }
            while line < number {
                start += text.get(start..)?.find('\n')? + 1;
                line += 1;
            }
            let rest = text.get(start..)?;
            let end = start + rest.find('\n').unwrap_or(rest.len());
            Some(start..end)
        })
    }

    /// Uses one-based line bounds excluding LF, supplied by an existing line index.
    pub fn byte_range_with_lines(&self, text: &str, mut line: impl FnMut(usize) -> Option<Range<usize>>) -> Option<Range<usize>> {
        let mut line = |number| {
            if number == 0 {
                return None;
            }
            let mut range = line(number)?;
            if text.get(range.clone())?.ends_with('\r') {
                range.end -= 1;
            }
            Some(range)
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
        let mut lines = |start: usize, end: usize| {
            if end < start {
                return None;
            }
            let first = line(start)?;
            let last = if start == end { first.clone() } else { line(end)? };
            Some((first, last))
        };
        let range = match self {
            Self::Lines { start, end } => {
                let (first, last) = lines(*start, *end)?;
                first.start..last.end
            }
            Self::Text { start, end } => {
                let (first, last) = lines(start.line, end.line)?;
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

#[derive(Clone, Debug, Default)]
pub struct Store {
    entries: BTreeMap<PathBuf, BTreeMap<Provider, Vec<Diagnostic>>>,
    retained: usize,
}

impl Store {
    pub fn replace(&mut self, path: PathBuf, provider: Provider, mut diagnostics: Vec<Diagnostic>) {
        let providers = self.entries.entry(path.clone()).or_default();
        if let Some(previous) = providers.remove(&provider) {
            self.retained -= previous.len();
        }
        diagnostics.truncate(MAX_DIAGNOSTICS.min(MAX_STORE_DIAGNOSTICS.saturating_sub(self.retained)));
        for diagnostic in &mut diagnostics {
            bound_message(&mut diagnostic.message);
            diagnostic.path = Some(path.clone());
            diagnostic.provider = provider.clone();
        }
        if diagnostics.is_empty() {
            if providers.is_empty() {
                self.entries.remove(&path);
            }
        } else {
            self.retained += diagnostics.len();
            providers.insert(provider, diagnostics);
        }
    }

    pub fn document(&self, path: &std::path::Path, provider: &Provider) -> &[Diagnostic] {
        self.entries
            .get(path)
            .and_then(|providers| providers.get(provider))
            .map_or(&[], Vec::as_slice)
    }

    pub fn clear_document(&mut self, path: &std::path::Path) {
        if let Some(providers) = self.entries.remove(path) {
            self.retained -= providers.values().map(Vec::len).sum::<usize>();
        }
    }
    pub fn clear_provider(&mut self, provider: &Provider) {
        self.entries.retain(|_, providers| {
            if let Some(removed) = providers.remove(provider) {
                self.retained -= removed.len();
            }
            !providers.is_empty()
        });
    }
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.entries.values().flat_map(|providers| providers.values()).flatten()
    }
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
    fn replacement_and_provider_removal_release_capacity_for_other_documents() {
        let mut store = Store::default();
        let server = Provider::LanguageServer("Texlab".into());
        let build = Provider::Build(BuildId(1));
        let diagnostic = Diagnostic {
            severity: Severity::Warning,
            provider: server.clone(),
            message: "problem".into(),
            path: None,
            range: None,
        };
        for index in 0..5 {
            for provider in [&server, &build] {
                store.replace(
                    format!("/p/{index}.tex").into(),
                    provider.clone(),
                    vec![diagnostic.clone(); MAX_DIAGNOSTICS],
                );
            }
        }
        store.replace("/p/0.tex".into(), server.clone(), vec![diagnostic.clone()]);
        store.replace("/p/new.tex".into(), server.clone(), vec![diagnostic.clone(); MAX_DIAGNOSTICS]);
        assert_eq!(store.document(std::path::Path::new("/p/new.tex"), &server).len(), MAX_DIAGNOSTICS - 1);
        assert_eq!(store.iter().count(), MAX_STORE_DIAGNOSTICS);
        store.clear_provider(&server);
        assert_eq!(store.iter().count(), 5 * MAX_DIAGNOSTICS);
        store.clear_document(std::path::Path::new("/p/0.tex"));
        store.replace("/p/new.tex".into(), server.clone(), vec![diagnostic; MAX_DIAGNOSTICS]);
        assert_eq!(store.document(std::path::Path::new("/p/new.tex"), &server).len(), MAX_DIAGNOSTICS);
        assert_eq!(store.iter().count(), 5 * MAX_DIAGNOSTICS);
    }

    #[test]
    fn diagnostic_retention_bounds_counts_and_unicode_messages() {
        let mut store = Store::default();
        let provider = Provider::LanguageServer("Texlab".into());
        let diagnostic = Diagnostic {
            severity: Severity::Warning,
            provider: provider.clone(),
            message: "猫".repeat(MAX_MESSAGE_BYTES),
            path: None,
            range: None,
        };
        for index in 0..12 {
            store.replace(
                format!("/p/{index}.tex").into(),
                provider.clone(),
                vec![diagnostic.clone(); MAX_DIAGNOSTICS + 1],
            );
        }
        assert_eq!(store.iter().count(), MAX_STORE_DIAGNOSTICS);
        assert!(
            store
                .iter()
                .all(|diagnostic| diagnostic.message.len() <= MAX_MESSAGE_BYTES && diagnostic.message.ends_with('猫'))
        );
        store.clear_document(std::path::Path::new("/p/0.tex"));
        store.replace("/p/11.tex".into(), provider.clone(), vec![diagnostic]);
        assert_eq!(store.document(std::path::Path::new("/p/11.tex"), &provider).len(), 1);
    }

    #[test]
    fn replacements_and_clears_preserve_other_documents_and_providers() {
        let mut store = Store::default();
        let server = Provider::LanguageServer("Texlab".into());
        let build = Provider::Build(BuildId(1));
        let diagnostic = |message: &str| Diagnostic {
            severity: Severity::Warning,
            provider: server.clone(),
            message: message.into(),
            path: None,
            range: None,
        };
        let first = PathBuf::from("/p/first.tex");
        let second = PathBuf::from("/p/second.tex");
        store.replace(first.clone(), server.clone(), vec![diagnostic("old")]);
        store.replace(first.clone(), build.clone(), vec![diagnostic("build")]);
        store.replace(second, server.clone(), vec![diagnostic("other")]);
        store.replace(first.clone(), server.clone(), vec![diagnostic("new")]);
        assert_eq!(store.document(&first, &server)[0].message, "new");
        assert_eq!(store.document(&first, &build)[0].provider, build);
        store.replace(first.clone(), server.clone(), Vec::new());
        assert_eq!(store.iter().count(), 2);
        store.clear_provider(&server);
        assert_eq!(store.iter().count(), 1);
        store.clear_document(&first);
        assert_eq!(store.iter().count(), 0);
    }

    #[test]
    fn indexed_ranges_match_scanned_ranges() {
        let text = "first\r\nα😀last\r\n\n";
        let starts: Vec<_> = std::iter::once(0).chain(text.match_indices('\n').map(|(offset, _)| offset + 1)).collect();
        let indexed = |range: &SourceRange| {
            range.byte_range_with_lines(text, |line| {
                let index = line.checked_sub(1)?;
                Some(*starts.get(index)?..starts.get(index + 1).map_or(text.len(), |next| next - 1))
            })
        };
        for start in 0..=5 {
            for end in 0..=5 {
                let range = SourceRange::Lines { start, end };
                assert_eq!(indexed(&range), range.byte_range(text));
                for column in 0..=9 {
                    let range = SourceRange::Text {
                        start: SourcePosition { line: start, column },
                        end: SourcePosition {
                            line: end,
                            column: column + 1,
                        },
                    };
                    assert_eq!(indexed(&range), range.byte_range(text));
                }
            }
        }
    }

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

    #[test]
    fn multiline_locations_span_lines_correctly() {
        let text = "a\nb\nc";
        assert_eq!(SourceRange::Lines { start: 1, end: 2 }.byte_range(text), Some(0..3));
        assert_eq!(SourceRange::Lines { start: 1, end: 3 }.byte_range(text), Some(0..5));
        assert_eq!(SourceRange::Lines { start: 2, end: 3 }.byte_range(text), Some(2..5));
    }
}
