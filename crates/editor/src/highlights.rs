use std::sync::Arc;

use ligature_buffer::DocumentId;
use ligature_syntax::{Category, Language, Snapshot, Token};
use smallvec::SmallVec;

pub(super) struct Highlights {
    pub document: DocumentId,
    pub language: Language,
    pub tokens: Vec<Token>,
    text: Arc<str>,
}

impl Highlights {
    pub fn new(snapshot: &Snapshot) -> Self {
        Self {
            document: snapshot.document,
            language: snapshot.language,
            tokens: snapshot.tokens.clone(),
            text: Arc::clone(&snapshot.text),
        }
    }

    pub fn update(&mut self, text: Arc<str>) {
        let start = ligature_syntax::common_prefix(&self.text, &text);
        if start == self.text.len() && start == text.len() {
            self.text = text;
            return;
        }
        let suffix = ligature_syntax::common_suffix(&self.text[start..], &text[start..]);
        let old_end = self.text.len() - suffix;
        let new_end = text.len() - suffix;
        let inserted_newline = text[start..new_end].contains('\n');
        let shift = |offset: usize| new_end + (offset - old_end);
        let first = self.tokens.partition_point(|token| token.range.end <= start);
        let last = self.tokens.partition_point(|token| token.range.start < old_end).max(first);
        if first > 0 {
            let previous = &mut self.tokens[first - 1];
            if previous.range.end == start && previous.category == Category::Comment && !inserted_newline {
                previous.range.end = new_end;
            }
        }
        let mut tokens = SmallVec::<[Token; 2]>::new();
        for token in &self.tokens[first..last] {
            let mut range = token.range.clone();
            if range.start < start && range.end > old_end && !inserted_newline {
                range.end = shift(range.end);
            } else {
                if range.start < start {
                    tokens.push(Token {
                        range: range.start..start,
                        category: token.category,
                    });
                }
                if range.end <= old_end {
                    continue;
                }
                range = new_end..shift(range.end);
            }
            if !range.is_empty() {
                tokens.push(Token {
                    range,
                    category: token.category,
                });
            }
        }
        if new_end != old_end {
            for token in &mut self.tokens[last..] {
                token.range = shift(token.range.start)..shift(token.range.end);
            }
        }
        if first != last || !tokens.is_empty() {
            self.tokens.splice(first..last, tokens);
        }
        self.text = text;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ligature_buffer::Buffer;

    #[test]
    fn typing_preserves_braces_and_comment_colors_before_reparsing() {
        let buffer = Buffer::from_text("\\section{Title} % comment");
        let snapshot = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, buffer.text().into());
        let mut highlights = Highlights::new(&snapshot);
        for text in [
            "\\section{TitleX} % comment",
            "\\section{TitleXY} % comment",
            "\\section{TitleXY} % comment typed",
        ] {
            highlights.update(text.into());
            let expected = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, text.into());
            let colored = |tokens: &[Token]| {
                tokens
                    .iter()
                    .filter(|token| matches!(token.category, Category::Brace | Category::Comment))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert_eq!(colored(&highlights.tokens), colored(&expected.tokens));
        }
    }

    #[test]
    fn replacements_keep_ranges_ordered_and_on_unicode_boundaries() {
        let buffer = Buffer::from_text("\\section{猫é} % comment\n$body$");
        let snapshot = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, buffer.text().into());
        for text in [
            "\\section{犬😀} % comment\n$body$",
            "\\section{} % comment\n$body$",
            "new\n{猫}",
            "",
            "\\section{猫é}\nnew % comment\n$body$",
        ] {
            let mut highlights = Highlights::new(&snapshot);
            highlights.update(text.into());
            let mut end = 0;
            for token in &highlights.tokens {
                assert!(token.range.start >= end);
                assert!(token.range.start < token.range.end);
                assert!(text.is_char_boundary(token.range.start));
                assert!(text.is_char_boundary(token.range.end));
                end = token.range.end;
            }
        }
    }

    #[test]
    fn newline_insertions_split_existing_highlights_and_shift_later_tokens() {
        let buffer = Buffer::from_text("% cat\n{猫}");
        let snapshot = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, buffer.text().into());
        let mut highlights = Highlights::new(&snapshot);
        highlights.update("% c\nat\n{猫}".into());
        assert_eq!(
            highlights.tokens,
            vec![
                Token {
                    range: 0..3,
                    category: Category::Comment
                },
                Token {
                    range: 4..6,
                    category: Category::Comment
                },
                Token {
                    range: 6..7,
                    category: Category::Text
                },
                Token {
                    range: 7..8,
                    category: Category::Brace
                },
                Token {
                    range: 8..11,
                    category: Category::Text
                },
                Token {
                    range: 11..12,
                    category: Category::Brace
                },
            ]
        );
        highlights.update("new".into());
        assert!(highlights.tokens.is_empty());
        highlights.update("".into());
        assert!(highlights.tokens.is_empty());
    }

    #[test]
    fn repeated_typing_reuses_highlight_storage_and_preserves_unaffected_tokens() {
        let buffer = Buffer::from_text("\\section{猫x} % tail");
        let snapshot = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, buffer.text().into());
        let mut highlights = Highlights::new(&snapshot);
        let allocation = highlights.tokens.as_ptr();
        for count in 1..=100 {
            let text = format!("\\section{{猫{}x}} % tail", "é".repeat(count));
            highlights.update(Arc::from(text.as_str()));
            assert_eq!(highlights.tokens.as_ptr(), allocation);
            let expected = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, text.into());
            assert_eq!(highlights.tokens, expected.tokens);
        }
    }
}
