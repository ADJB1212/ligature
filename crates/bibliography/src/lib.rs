//! Tolerant BibTeX parsing and citation lookup, independent of GPUI.

use std::{collections::BTreeMap, ops::Range, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    EntryType,
    Field,
    Value,
    Comment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Highlight {
    pub range: Range<usize>,
    pub category: Category,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub entry_type: String,
    pub range: Range<usize>,
    pub key_range: Range<usize>,
    pub fields: BTreeMap<String, String>,
}

impl Entry {
    pub fn title(&self) -> Option<&str> {
        self.fields.get("title").map(String::as_str)
    }

    pub fn author(&self) -> Option<&str> {
        self.fields.get("author").map(String::as_str)
    }

    pub fn year(&self) -> Option<&str> {
        self.fields.get("year").or_else(|| self.fields.get("date")).map(String::as_str)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bibliography {
    pub entries: Vec<Entry>,
    pub highlights: Vec<Highlight>,
    pub malformed: Vec<Range<usize>>,
    pub strings: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Literal(String),
    String(String),
}

pub fn parse(text: &str) -> Bibliography {
    let mut parser = Parser {
        text,
        cursor: 0,
        result: Bibliography::default(),
    };
    let mut strings: BTreeMap<String, Vec<Part>> = BTreeMap::new();
    let mut fields = Vec::new();
    while parser.seek_entry() {
        let start = parser.cursor;
        parser.cursor += 1;
        let kind_range = parser.identifier();
        let kind = text[kind_range.clone()].to_ascii_lowercase();
        parser.highlight(start..kind_range.end, Category::EntryType);
        parser.space();
        let Some(open @ (b'{' | b'(')) = parser.byte() else { continue };
        parser.cursor += 1;
        let close = if open == b'{' { b'}' } else { b')' };
        if kind == "comment" || kind == "preamble" {
            let body = parser.cursor;
            parser.skip_body(close);
            parser.highlight(body..parser.cursor, Category::Comment);
            continue;
        }
        if kind == "string" {
            parser.space();
            let name = parser.identifier();
            parser.highlight(name.clone(), Category::Field);
            parser.space();
            if parser.take(b'=') {
                if let Some(value) = parser.value() {
                    strings.insert(text[name].to_ascii_lowercase(), value);
                } else {
                    parser.result.malformed.push(start..parser.cursor);
                }
            }
            parser.space();
            parser.take(b',');
            parser.space();
            parser.take(close);
            continue;
        }
        parser.space();
        let key_start = parser.cursor;
        while parser
            .byte()
            .is_some_and(|byte| !matches!(byte, b',' | b'}' | b')' | b'@') && !byte.is_ascii_whitespace())
        {
            parser.cursor += 1;
        }
        let key_range = key_start..parser.cursor;
        parser.space();
        if !key_range.is_empty() && parser.take(close) {
            parser.result.entries.push(Entry {
                key: text[key_range.clone()].to_owned(),
                entry_type: kind,
                range: start..parser.cursor,
                key_range,
                fields: BTreeMap::new(),
            });
            fields.push(BTreeMap::new());
            continue;
        }
        if key_range.is_empty() || !parser.take(b',') {
            parser.result.malformed.push(start..parser.cursor);
            continue;
        }
        let mut entry_fields = BTreeMap::new();
        let mut valid = true;
        loop {
            parser.space();
            if parser.take(close) {
                break;
            }
            if parser.byte().is_none_or(|byte| byte == b'@') {
                valid = false;
                break;
            }
            let name = parser.identifier();
            if name.is_empty() {
                valid = false;
                parser.cursor += text[parser.cursor..].chars().next().map_or(0, char::len_utf8);
                continue;
            }
            parser.highlight(name.clone(), Category::Field);
            parser.space();
            if !parser.take(b'=') {
                valid = false;
                continue;
            }
            if let Some(value) = parser.value() {
                entry_fields.insert(text[name].to_ascii_lowercase(), value);
            } else {
                valid = false;
                break;
            }
            parser.space();
            if parser.take(b',') {
                continue;
            }
            if parser.byte() != Some(close) {
                valid = false;
            }
        }
        if !valid {
            parser.result.malformed.push(start..parser.cursor);
        }
        parser.result.entries.push(Entry {
            key: text[key_range.clone()].to_owned(),
            entry_type: kind,
            range: start..parser.cursor,
            key_range,
            fields: BTreeMap::new(),
        });
        fields.push(entry_fields);
    }
    let mut expanded = month_strings();
    for key in strings.keys() {
        expanded.remove(key);
    }
    for key in strings.keys() {
        let value = expand(&strings[key], &strings, &mut vec![key.clone()], &expanded);
        expanded.insert(key.clone(), value);
    }
    for (entry, fields) in parser.result.entries.iter_mut().zip(fields) {
        entry.fields = fields
            .into_iter()
            .map(|(key, value)| (key, expand(&value, &strings, &mut Vec::new(), &expanded)))
            .collect();
    }
    parser.result.strings = expanded;
    parser.result
}

fn month_strings() -> BTreeMap<String, String> {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .into_iter()
    .map(|month| (month[..3].to_ascii_lowercase(), month.to_owned()))
    .collect()
}

fn expand(parts: &[Part], strings: &BTreeMap<String, Vec<Part>>, visiting: &mut Vec<String>, expanded: &BTreeMap<String, String>) -> String {
    let mut result = String::new();
    append_expansion(parts, strings, visiting, expanded, &mut result);
    result
}

fn append_expansion(
    parts: &[Part], strings: &BTreeMap<String, Vec<Part>>, visiting: &mut Vec<String>, expanded: &BTreeMap<String, String>, result: &mut String,
) {
    for part in parts {
        match part {
            Part::Literal(value) => result.push_str(value),
            Part::String(key) => {
                if visiting.contains(key) || visiting.len() >= 64 {
                    result.push_str(key);
                } else if let Some(value) = expanded.get(key) {
                    result.push_str(value);
                } else if let Some(value) = strings.get(key) {
                    visiting.push(key.clone());
                    append_expansion(value, strings, visiting, expanded, result);
                    visiting.pop();
                } else {
                    result.push_str(key);
                }
            }
        }
    }
}

struct Parser<'a> {
    text: &'a str,
    cursor: usize,
    result: Bibliography,
}

impl Parser<'_> {
    fn byte(&self) -> Option<u8> {
        self.text.as_bytes().get(self.cursor).copied()
    }
    fn take(&mut self, byte: u8) -> bool {
        if self.byte() == Some(byte) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn highlight(&mut self, range: Range<usize>, category: Category) {
        if !range.is_empty() {
            self.result.highlights.push(Highlight { range, category });
        }
    }
    fn space(&mut self) {
        loop {
            while self.byte().is_some_and(|byte| byte.is_ascii_whitespace()) {
                self.cursor += 1;
            }
            if self.byte() != Some(b'%') {
                break;
            }
            let start = self.cursor;
            self.cursor += self.text[self.cursor..].find('\n').unwrap_or(self.text.len() - self.cursor);
            self.highlight(start..self.cursor, Category::Comment);
        }
    }
    fn seek_entry(&mut self) -> bool {
        while self.cursor < self.text.len() {
            self.space();
            if self.byte() == Some(b'@') {
                return true;
            }
            self.cursor += self.text[self.cursor..].chars().next().map_or(0, char::len_utf8);
        }
        false
    }
    fn identifier(&mut self) -> Range<usize> {
        let start = self.cursor;
        while self
            .byte()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.'))
        {
            self.cursor += 1;
        }
        start..self.cursor
    }
    fn recovery(&self) -> bool {
        if self.byte() != Some(b'@')
            || !self.text[..self.cursor]
                .chars()
                .rev()
                .take_while(|&character| character != '\n')
                .all(char::is_whitespace)
        {
            return false;
        }
        let tail = &self.text[self.cursor + 1..];
        let end = tail.bytes().take_while(u8::is_ascii_alphabetic).count();
        end > 0 && tail[end..].trim_start().starts_with(['{', '('])
    }
    fn skip_body(&mut self, close: u8) {
        let mut depth = 0usize;
        while let Some(byte) = self.byte() {
            if byte == b'\\' {
                self.cursor += 1;
                self.cursor += self.text[self.cursor..].chars().next().map_or(0, char::len_utf8);
                continue;
            }
            self.cursor += 1;
            if byte == b'{' {
                depth += 1;
            } else if byte == b'}' && depth > 0 {
                depth -= 1;
            } else if byte == close {
                break;
            }
        }
    }
    fn value(&mut self) -> Option<Vec<Part>> {
        let mut parts = Vec::new();
        loop {
            self.space();
            let start = self.cursor;
            match self.byte()? {
                open @ (b'{' | b'"') => {
                    self.cursor += 1;
                    let content = self.cursor;
                    let mut depth = 0usize;
                    let mut end = None;
                    while let Some(byte) = self.byte() {
                        if self.recovery() {
                            break;
                        }
                        if byte == b'\\' {
                            self.cursor += 1;
                            self.cursor += self.text[self.cursor..].chars().next().map_or(0, char::len_utf8);
                            continue;
                        }
                        if (open == b'{' && byte == b'}' || open == b'"' && byte == b'"') && depth == 0 {
                            end = Some(self.cursor);
                            self.cursor += 1;
                            break;
                        }
                        if byte == b'{' {
                            depth += 1;
                        }
                        if byte == b'}' {
                            depth = depth.saturating_sub(1);
                        }
                        self.cursor += 1;
                    }
                    self.highlight(start..self.cursor, Category::Value);
                    parts.push(Part::Literal(self.text[content..end?].to_owned()));
                }
                _ => {
                    let range = self.identifier();
                    if range.is_empty() {
                        return None;
                    }
                    let value = &self.text[range.clone()];
                    parts.push(if value.bytes().all(|byte| byte.is_ascii_digit()) {
                        Part::Literal(value.to_owned())
                    } else {
                        Part::String(value.to_ascii_lowercase())
                    });
                    self.highlight(range, Category::Value);
                }
            }
            self.space();
            if !self.take(b'#') {
                break;
            }
        }
        Some(parts)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub path: PathBuf,
    pub entry: Entry,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub entries: BTreeMap<String, Vec<Definition>>,
}

impl Index {
    pub fn insert(&mut self, path: PathBuf, bibliography: &Bibliography) {
        for entry in &bibliography.entries {
            self.entries.entry(entry.key.clone()).or_default().push(Definition {
                path: path.clone(),
                entry: entry.clone(),
            });
        }
    }
    pub fn resolve(&self, key: &str) -> &[Definition] {
        self.entries.get(key).map_or(&[], Vec::as_slice)
    }
    pub fn duplicates(&self) -> impl Iterator<Item = (&str, &[Definition])> {
        self.entries
            .iter()
            .filter(|(_, definitions)| definitions.len() > 1)
            .map(|(key, definitions)| (key.as_str(), definitions.as_slice()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_strings_and_preserves_nested_and_quoted_values() {
        let text = "@string{journal = prefix # { Review}}\n@string{prefix = \"Science\"}\n@article{key, title={A {Nested} Title}, author=\"A {B} C\", year=2026, journal=journal, month=jan}";
        let result = parse(text);
        let entry = &result.entries[0];
        assert_eq!(entry.title(), Some("A {Nested} Title"));
        assert_eq!(entry.author(), Some("A {B} C"));
        assert_eq!(entry.year(), Some("2026"));
        assert_eq!(entry.fields["journal"], "Science Review");
        assert_eq!(entry.fields["month"], "January");
        assert_eq!(&text[entry.key_range.clone()], "key");
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn recovers_valid_entries_around_malformed_entries() {
        let result = parse("@book{first, title={One}}\n@book{bad, title={unclosed\n@article{last, title=\"Two\"}");
        assert_eq!(
            result.entries.iter().map(|entry| entry.key.as_str()).collect::<Vec<_>>(),
            ["first", "bad", "last"]
        );
        assert_eq!(result.entries[2].title(), Some("Two"));
        assert_eq!(result.malformed.len(), 1);
    }

    #[test]
    fn cycles_and_unicode_are_bounded_and_duplicates_are_retained() {
        let result = parse("@string{a=b}\n@string{b=a}\n@book{日本, title=a}\n@book{日本, title={日本語}}");
        let mut index = Index::default();
        index.insert(PathBuf::from("one.bib"), &result);
        index.insert(PathBuf::from("two.bib"), &parse("@book{日本, year=2025}"));
        assert_eq!(index.resolve("日本").len(), 3);
        assert_eq!(index.duplicates().count(), 1);
        assert!(index.resolve("missing").is_empty());
    }

    #[test]
    fn forward_string_definitions_override_builtin_month_names() {
        let result = parse("@string{a=jan}\n@string{jan={Custom}}\n@book{key,title=a}");
        assert_eq!(result.entries[0].title(), Some("Custom"));
    }

    #[test]
    fn empty_entries_comments_and_multiline_at_signs_are_preserved() {
        let result = parse("@comment{\n@book{hidden,title={Hidden}}\n}\n@misc{empty}\n@book{real,title={Line\n@example.org\nEnd}}");
        assert_eq!(
            result.entries.iter().map(|entry| entry.key.as_str()).collect::<Vec<_>>(),
            ["empty", "real"]
        );
        assert_eq!(result.entries[1].title(), Some("Line\n@example.org\nEnd"));
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn repeated_at_signs_in_long_values_do_not_trigger_entry_recovery() {
        let title = "user@example.org ".repeat(20_000);
        let text = format!("@book{{first,title={{{title}}}}}\n@book{{broken,title={{unclosed\n\u{2003}@article{{last,title={{Recovered}}}}");
        let result = parse(&text);
        assert_eq!(result.entries[0].title(), Some(title.as_str()));
        assert_eq!(result.entries[2].title(), Some("Recovered"));
        assert_eq!(result.malformed.len(), 1);
    }
}
