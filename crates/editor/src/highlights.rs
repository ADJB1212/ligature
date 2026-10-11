use std::sync::Arc;

use ligature_buffer::DocumentId;
use ligature_syntax::{Category, Language, Snapshot, Token};

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
        let mut start = self.text.bytes().zip(text.bytes()).take_while(|(old, new)| old == new).count();
        while !self.text.is_char_boundary(start) || !text.is_char_boundary(start) {
            start -= 1;
        }
        let suffix_limit = (self.text.len() - start).min(text.len() - start);
        let mut suffix = self
            .text
            .bytes()
            .rev()
            .zip(text.bytes().rev())
            .take(suffix_limit)
            .take_while(|(old, new)| old == new)
            .count();
        while !self.text.is_char_boundary(self.text.len() - suffix) || !text.is_char_boundary(text.len() - suffix) {
            suffix -= 1;
        }
        let old_end = self.text.len() - suffix;
        let new_end = text.len() - suffix;
        let inserted_newline = text[start..new_end].contains('\n');
        let shift = |offset: usize| new_end + (offset - old_end);
        let mut tokens = Vec::with_capacity(self.tokens.len());
        for token in &self.tokens {
            let mut range = token.range.clone();
            if range.end <= start {
                if range.end == start && token.category == Category::Comment && !inserted_newline {
                    range.end = new_end;
                }
            } else if range.start >= old_end {
                range = shift(range.start)..shift(range.end);
            } else if range.start < start && range.end > old_end && !inserted_newline {
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
        self.tokens = tokens;
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
}
