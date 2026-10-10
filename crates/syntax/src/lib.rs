//! Error-tolerant lexical snapshots with conservative incremental restart points.

use std::{ops::Range, sync::Arc};

use ligature_buffer::{DocumentId, Revision};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Tex,
    Bibliography,
    Plain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Text,
    Command,
    Environment,
    Brace,
    Comment,
    MathDelimiter,
    Verbatim,
    EntryType,
    Field,
    Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub range: Range<usize>,
    pub category: Category,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub document: DocumentId,
    pub revision: Revision,
    pub language: Language,
    pub text: Arc<str>,
    pub tokens: Vec<Token>,
    checkpoints: Vec<usize>,
    pub reparsed: Range<usize>,
}

impl Snapshot {
    pub fn parse(document: DocumentId, revision: Revision, language: Language, text: Arc<str>) -> Self {
        let (tokens, checkpoints) = lex(language, &text, 0);
        let reparsed = 0..text.len();
        Self {
            document,
            revision,
            language,
            text,
            tokens,
            checkpoints,
            reparsed,
        }
    }

    pub fn is_current(&self, document: DocumentId, revision: Revision, language: Language) -> bool {
        self.document == document && self.revision == revision && self.language == language
    }

    pub fn update(&self, revision: Revision, range: Range<usize>, replacement: &str) -> Self {
        assert!(range.start <= range.end && range.end <= self.text.len());
        assert!(self.text.is_char_boundary(range.start) && self.text.is_char_boundary(range.end));
        let mut text = self.text.to_string();
        text.replace_range(range.clone(), replacement);
        self.reparse(revision, text.into(), range.start)
    }

    pub fn refresh(&self, revision: Revision, text: Arc<str>) -> Self {
        let mut start = self.text.bytes().zip(text.bytes()).take_while(|(old, new)| old == new).count();
        while !self.text.is_char_boundary(start) || !text.is_char_boundary(start) {
            start -= 1;
        }
        self.reparse(revision, text, start)
    }

    fn reparse(&self, revision: Revision, text: Arc<str>, changed: usize) -> Self {
        // Restart before a boundary edit, which may remove the newline or an entry opener.
        let start = self.checkpoints.iter().rev().copied().find(|&offset| offset < changed).unwrap_or(0);
        let (suffix, checkpoints) = lex(self.language, &text[start..], start);
        let mut tokens: Vec<_> = self.tokens.iter().take_while(|token| token.range.end <= start).cloned().collect();
        tokens.extend(suffix);
        let mut prefix: Vec<_> = self.checkpoints.iter().copied().take_while(|&offset| offset < start).collect();
        prefix.extend(checkpoints);
        let reparsed = start..text.len();
        Self {
            document: self.document,
            revision,
            language: self.language,
            text,
            tokens,
            checkpoints: prefix,
            reparsed,
        }
    }
}

fn lex(language: Language, text: &str, offset: usize) -> (Vec<Token>, Vec<usize>) {
    if language == Language::Bibliography {
        let bibliography = ligature_bibliography::parse(text);
        let mut checkpoints = vec![offset];
        let tokens = bibliography
            .highlights
            .into_iter()
            .map(|highlight| {
                use ligature_bibliography::Category as Bib;
                let category = match highlight.category {
                    Bib::EntryType => {
                        checkpoints.push(highlight.range.start + offset);
                        Category::EntryType
                    }
                    Bib::Field => Category::Field,
                    Bib::Value => Category::Value,
                    Bib::Comment => Category::Comment,
                };
                Token {
                    range: highlight.range.start + offset..highlight.range.end + offset,
                    category,
                }
            })
            .collect();
        checkpoints.sort_unstable();
        checkpoints.dedup();
        return (tokens, checkpoints);
    }
    if language == Language::Plain {
        return (Vec::new(), vec![offset]);
    }
    let mut lexer = Lexer {
        cursor: 0,
        offset,
        tokens: Vec::new(),
        checkpoints: vec![offset],
    };
    while lexer.cursor < text.len() {
        let start = lexer.cursor;
        match text.as_bytes()[start] {
            b'%' => {
                lexer.cursor += text[start..].find('\n').unwrap_or(text.len() - start);
                lexer.push(start, Category::Comment);
            }
            b'{' | b'}' => {
                lexer.cursor += 1;
                lexer.push(start, Category::Brace);
            }
            b'$' => {
                lexer.cursor += if text[start..].starts_with("$$") { 2 } else { 1 };
                lexer.push(start, Category::MathDelimiter);
            }
            b'\\' => {
                lexer.cursor += 1;
                while lexer.cursor < text.len() && text.as_bytes()[lexer.cursor].is_ascii_alphabetic() {
                    lexer.cursor += 1;
                }
                if lexer.cursor == start + 1 {
                    lexer.cursor += text[lexer.cursor..].chars().next().map_or(0, char::len_utf8);
                    let math = matches!(&text[start..lexer.cursor], "\\(" | "\\)" | "\\[" | "\\]");
                    lexer.push(start, if math { Category::MathDelimiter } else { Category::Command });
                    continue;
                }
                let command = &text[start + 1..lexer.cursor];
                lexer.push(start, Category::Command);
                if command == "verb" || command == "lstinline" {
                    let body = lexer.cursor;
                    if text[lexer.cursor..].starts_with('*') {
                        lexer.cursor += 1;
                    }
                    if let Some(delimiter) = text[lexer.cursor..].chars().next().filter(|c| !c.is_whitespace()) {
                        lexer.cursor += delimiter.len_utf8();
                        while let Some(c) = text[lexer.cursor..].chars().next() {
                            if c == '\n' {
                                break;
                            }
                            lexer.cursor += c.len_utf8();
                            if c == delimiter {
                                break;
                            }
                        }
                    }
                    lexer.push(body, Category::Verbatim);
                } else if matches!(command, "begin" | "end") {
                    let mut brace = lexer.cursor;
                    while text.as_bytes().get(brace).is_some_and(u8::is_ascii_whitespace) {
                        brace += 1;
                    }
                    if text.as_bytes().get(brace) == Some(&b'{')
                        && let Some(end) = text[brace + 1..].find(['}', '\n', '%', '\\'])
                    {
                        let end = brace + 1 + end;
                        if text.as_bytes()[end] == b'}' {
                            let environment = &text[brace + 1..end];
                            lexer.push_until(brace, Category::Text);
                            lexer.push_until(brace + 1, Category::Brace);
                            lexer.push_until(end, Category::Environment);
                            lexer.push_until(end + 1, Category::Brace);
                            if command == "begin"
                                && matches!(environment, "verbatim" | "verbatim*" | "Verbatim" | "lstlisting" | "minted" | "comment")
                            {
                                let body = lexer.cursor;
                                let closing = format!("\\end{{{environment}}}");
                                lexer.cursor = text[body..].find(&closing).map_or(text.len(), |end| body + end);
                                lexer.push(body, Category::Verbatim);
                            }
                        }
                    }
                }
            }
            _ => {
                while lexer.cursor < text.len() && !matches!(text.as_bytes()[lexer.cursor], b'%' | b'{' | b'}' | b'$' | b'\\') {
                    let byte = text.as_bytes()[lexer.cursor];
                    lexer.cursor += 1;
                    if byte == b'\n' {
                        break;
                    }
                }
                lexer.push(start, Category::Text);
                if text.as_bytes()[lexer.cursor - 1] == b'\n' {
                    lexer.checkpoints.push(lexer.cursor + offset);
                }
            }
        }
    }
    (lexer.tokens, lexer.checkpoints)
}

struct Lexer {
    cursor: usize,
    offset: usize,
    tokens: Vec<Token>,
    checkpoints: Vec<usize>,
}

impl Lexer {
    fn push(&mut self, start: usize, category: Category) {
        if start < self.cursor {
            self.tokens.push(Token {
                range: start + self.offset..self.cursor + self.offset,
                category,
            });
        }
    }
    fn push_until(&mut self, end: usize, category: Category) {
        let start = self.cursor;
        self.cursor = end;
        self.push(start, category);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ligature_buffer::{Buffer, ByteOffset};

    #[test]
    fn incomplete_tex_is_highlighted_and_verbatim_is_opaque() {
        let buffer = Buffer::from_text(
            "\\section{Unclosed\n% \\label{x}\n$math$ \\(x\\) \\%\n\\verb|\\label{hidden}|\n\\begin{verbatim}\n\\section{Hidden}\n\\end{verbatim}\n\\",
        );
        let snapshot = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, buffer.text().into());
        assert!(snapshot.tokens.iter().any(|token| token.category == Category::Comment));
        assert_eq!(
            snapshot.tokens.iter().filter(|token| token.category == Category::MathDelimiter).count(),
            4
        );
        assert_eq!(snapshot.tokens.iter().filter(|token| token.category == Category::Verbatim).count(), 2);
        assert!(
            !snapshot
                .tokens
                .iter()
                .any(|token| token.category == Category::Command && &snapshot.text[token.range.clone()] == "\\label")
        );
        for token in &snapshot.tokens {
            assert!(snapshot.text.get(token.range.clone()).is_some());
        }
    }

    #[test]
    fn incremental_parses_equal_full_parses_after_boundary_and_unicode_edits() {
        for language in [Language::Tex, Language::Bibliography] {
            let initial = if language == Language::Tex {
                "\\section{日本}\ntext\n\\begin{verbatim}\n$abc$\n\\end{verbatim}\nlast"
            } else {
                "@string{a={One}}\n@book{key,title=a}\n@book{last,title={日本}}"
            };
            for start in initial.char_indices().map(|(index, _)| index).chain([initial.len()]) {
                for replacement in ["", "\n", "\\", "{", "}", "@", "%", "日本"] {
                    let mut buffer = Buffer::from_text(initial);
                    let old = Snapshot::parse(buffer.id(), buffer.revision(), language, initial.into());
                    let end = initial[start..].chars().next().map_or(start, |c| start + c.len_utf8());
                    buffer
                        .replace(buffer.text_range(ByteOffset(start), ByteOffset(end)).unwrap(), replacement)
                        .unwrap();
                    let incremental = old.update(buffer.revision(), start..end, replacement);
                    let full = Snapshot::parse(buffer.id(), buffer.revision(), language, buffer.text().into());
                    assert_eq!(incremental.tokens, full.tokens, "{language:?} {start} {replacement:?}");
                    assert_eq!(old.refresh(buffer.revision(), buffer.text().into()).tokens, full.tokens);
                }
            }
        }
    }

    #[test]
    fn update_reuses_unaffected_prefix_and_tracks_identity_and_revision() {
        let mut buffer = Buffer::from_text("one\ntwo\nthree\nfour");
        let old = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, buffer.text().into());
        buffer.type_text(ByteOffset(15), "!").unwrap();
        let updated = old.refresh(buffer.revision(), buffer.text().into());
        assert_eq!(updated.reparsed.start, 14);
        assert!(!old.is_current(buffer.id(), buffer.revision(), Language::Tex));
        assert!(updated.is_current(buffer.id(), buffer.revision(), Language::Tex));
        assert!(!updated.is_current(Buffer::new().id(), buffer.revision(), Language::Tex));
    }
}
