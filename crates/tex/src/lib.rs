//! Literal TeX document structure extracted from tolerant syntax snapshots.

use std::{cell::OnceCell, collections::BTreeMap, ops::Range};

use ligature_syntax::{Category, Snapshot};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub title: String,
    pub level: usize,
    pub range: Range<usize>,
    pub title_range: Range<usize>,
    pub parent: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    pub name: String,
    pub begin: Range<usize>,
    pub end: Option<Range<usize>>,
    pub parent: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub name: String,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub headings: Vec<Heading>,
    pub environments: Vec<Environment>,
    pub labels: Vec<Key>,
    pub references: Vec<Key>,
    pub citations: Vec<Key>,
    pub bibliographies: Vec<Key>,
    pub includes: Vec<Key>,
}

pub fn parse(snapshot: &Snapshot) -> Document {
    let mut document = Document::default();
    let closing_braces = closing_braces(snapshot);
    let optional_brackets = OnceCell::new();
    let mut environments = Vec::new();
    let mut environment_positions: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut headings: Vec<usize> = Vec::new();
    for token in snapshot.tokens.iter().filter(|token| token.category == Category::Command) {
        let name = snapshot.text[token.range.clone()].trim_start_matches('\\');
        let level = ["part", "chapter", "section", "subsection", "subsubsection", "paragraph", "subparagraph"]
            .iter()
            .position(|&heading| name == heading);
        let citation = citation_command(name);
        if level.is_none()
            && !citation
            && !matches!(
                name,
                "begin"
                    | "end"
                    | "label"
                    | "ref"
                    | "pageref"
                    | "eqref"
                    | "autoref"
                    | "cref"
                    | "Cref"
                    | "vref"
                    | "Vref"
                    | "nameref"
                    | "bibliography"
                    | "addbibresource"
                    | "addglobalbib"
                    | "addsectionbib"
                    | "input"
                    | "include"
                    | "subfile"
            )
        {
            continue;
        }
        let Some(argument) = argument(snapshot, &closing_braces, &optional_brackets, token.range.end) else {
            if matches!(name, "input" | "include" | "subfile") {
                let tail = &snapshot.text[token.range.end..];
                let start = token.range.end + tail.len() - tail.trim_start_matches([' ', '\t']).len();
                let end = snapshot.text[start..]
                    .find(|c: char| c.is_whitespace() || matches!(c, '%' | '{' | '}' | '\\'))
                    .map_or(snapshot.text.len(), |end| start + end);
                keys(&snapshot.text, start..end, &mut document.includes);
            }
            continue;
        };
        let range = token.range.start..argument.end;
        if let Some(level) = level {
            while headings.last().is_some_and(|&index| document.headings[index].level >= level) {
                headings.pop();
            }
            let parent = headings.last().copied();
            headings.push(document.headings.len());
            document.headings.push(Heading {
                title: clean(&snapshot.text[argument.content.clone()]),
                level,
                range,
                title_range: argument.content,
                parent,
            });
            continue;
        }
        if !argument.closed
            && matches!(
                name,
                "input" | "include" | "subfile" | "bibliography" | "addbibresource" | "addglobalbib" | "addsectionbib"
            )
        {
            continue;
        }
        match name {
            "begin" => {
                let name = clean(&snapshot.text[argument.content]);
                environment_positions.entry(name.clone()).or_default().push(environments.len());
                document.environments.push(Environment {
                    name,
                    begin: range,
                    end: None,
                    parent: environments.last().copied(),
                });
                environments.push(document.environments.len() - 1);
            }
            "end" => {
                let name = clean(&snapshot.text[argument.content]);
                if let Some(position) = environment_positions.get(&name).and_then(|positions| positions.last()).copied() {
                    document.environments[environments[position]].end = Some(range);
                    for index in environments.drain(position..) {
                        environment_positions.get_mut(&document.environments[index].name).unwrap().pop();
                    }
                }
            }
            "label" => keys(&snapshot.text, argument.content, &mut document.labels),
            "ref" | "pageref" | "eqref" | "autoref" | "cref" | "Cref" | "vref" | "Vref" | "nameref" => {
                keys(&snapshot.text, argument.content, &mut document.references);
            }
            "bibliography" | "addbibresource" | "addglobalbib" | "addsectionbib" => {
                keys(&snapshot.text, argument.content, &mut document.bibliographies)
            }
            "input" | "include" | "subfile" => keys(&snapshot.text, argument.content, &mut document.includes),
            _ if citation => {
                keys(&snapshot.text, argument.content, &mut document.citations);
                if name.ends_with('s') {
                    let mut cursor = argument.end;
                    while let Some(argument) = self::argument(snapshot, &closing_braces, &optional_brackets, cursor) {
                        keys(&snapshot.text, argument.content, &mut document.citations);
                        cursor = argument.end;
                    }
                }
            }
            _ => {}
        }
    }
    document
}

fn citation_command(name: &str) -> bool {
    let name = name.strip_suffix(['s', 'S']).unwrap_or(name);
    [
        "cite",
        "citep",
        "citet",
        "citealt",
        "citealp",
        "citeauthor",
        "citeyear",
        "citeyearpar",
        "nocite",
        "parencite",
        "textcite",
        "autocite",
        "footcite",
        "smartcite",
        "supercite",
        "fullcite",
        "footfullcite",
    ]
    .iter()
    .any(|command| name.eq_ignore_ascii_case(command))
}

fn clean(text: &str) -> String {
    text.lines()
        .map(|line| {
            let mut end = line.len();
            let mut escaped = false;
            for (index, c) in line.char_indices() {
                if c == '%' && !escaped {
                    end = index;
                    break;
                }
                escaped = c == '\\' && !escaped;
            }
            line[..end].trim()
        })
        .filter(|line| !line.is_empty())
        .fold(String::new(), |mut result, line| {
            if !result.is_empty() {
                result.push(' ');
            }
            result.push_str(line);
            result
        })
}

fn keys(text: &str, range: Range<usize>, result: &mut Vec<Key>) {
    let mut start = range.start;
    for key in text[range].split(',') {
        let trimmed = key.trim();
        if !trimmed.is_empty() && !trimmed.contains(['\\', '{', '}', '%', '#']) {
            let offset = start + key.len() - key.trim_start().len();
            result.push(Key {
                name: trimmed.to_owned(),
                range: offset..offset + trimmed.len(),
            });
        }
        start += key.len() + 1;
    }
}

struct Argument {
    content: Range<usize>,
    end: usize,
    closed: bool,
}

fn closing_braces(snapshot: &Snapshot) -> Vec<Range<usize>> {
    let mut closing: Vec<Range<usize>> = Vec::new();
    let mut opening = Vec::new();
    for token in &snapshot.tokens {
        if token.category != Category::Brace {
            continue;
        }
        if snapshot.text.as_bytes()[token.range.start] == b'{' {
            opening.push(closing.len());
            closing.push(token.range.start..usize::MAX);
        } else if let Some(start) = opening.pop() {
            closing[start].end = token.range.end;
        }
    }
    closing
}

fn optional_brackets(text: &str) -> Vec<Range<usize>> {
    let delimiters: Vec<_> = text
        .bytes()
        .enumerate()
        .filter(|(_, byte)| matches!(byte, b'[' | b']' | b'{' | b'}' | b'\\' | b'%' | b'\n'))
        .collect();
    let mut braces = vec![None; delimiters.len() + 1];
    let mut brackets = vec![None; delimiters.len() + 1];
    let mut next_line = delimiters.len();
    // Resolve suffixes backwards so nested and incomplete arguments share scans.
    for index in (0..delimiters.len()).rev() {
        let next =
            index + 1 + usize::from(delimiters[index].1 == b'\\' && delimiters.get(index + 1).is_some_and(|next| next.0 == delimiters[index].0 + 1));
        braces[index] = match delimiters[index].1 {
            b'}' => Some(index + 1),
            b'{' => braces[index + 1].and_then(|end| braces[end]),
            b'%' => braces[next_line],
            _ => braces[next],
        };
        brackets[index] = match delimiters[index].1 {
            b']' => Some(index + 1),
            b'[' => brackets[index + 1].and_then(|end| brackets[end]),
            b'{' => braces[index + 1].and_then(|end| brackets[end]),
            b'%' => brackets[next_line],
            _ => brackets[next],
        };
        if delimiters[index].1 == b'\n' {
            next_line = index + 1;
        }
    }
    delimiters
        .iter()
        .enumerate()
        .filter(|(_, (_, byte))| *byte == b'[')
        .map(|(index, &(start, _))| start..brackets[index + 1].map_or(usize::MAX, |end| delimiters[end - 1].0 + 1))
        .collect()
}

fn argument(snapshot: &Snapshot, closing_braces: &[Range<usize>], optional: &OnceCell<Vec<Range<usize>>>, mut cursor: usize) -> Option<Argument> {
    let text = &snapshot.text;
    if text[cursor..].starts_with('*') {
        cursor += 1;
    }
    loop {
        while text.as_bytes().get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if text.as_bytes().get(cursor) == Some(&b'%') {
            cursor += text[cursor..].find('\n').unwrap_or(text.len() - cursor);
            continue;
        }
        if text.as_bytes().get(cursor) != Some(&b'[') {
            break;
        }
        let brackets = optional.get_or_init(|| optional_brackets(text));
        let opening = brackets.binary_search_by_key(&cursor, |range| range.start).ok()?;
        if brackets[opening].end == usize::MAX {
            return None;
        }
        cursor = brackets[opening].end;
    }
    if text.as_bytes().get(cursor) != Some(&b'{') {
        return None;
    }
    let start = cursor + 1;
    if let Ok(opening) = closing_braces.binary_search_by_key(&cursor, |range| range.start)
        && closing_braces[opening].end != usize::MAX
    {
        let end = closing_braces[opening].end;
        return Some(Argument {
            content: start..end - 1,
            end,
            closed: true,
        });
    }
    let end = start + text[start..].find('\n').unwrap_or(text.len() - start);
    Some(Argument {
        content: start..end,
        end,
        closed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ligature_buffer::Buffer;
    use ligature_syntax::Language;

    fn document(text: &str) -> Document {
        let buffer = Buffer::from_text(text);
        parse(&Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, text.into()))
    }

    #[test]
    fn incomplete_headings_keep_hierarchy_and_source_ranges() {
        let text = "\\section*{First}\n\\subsection[Short]{Nested {Title}}\n\\section{Unclosed\n\\label{last}";
        let document = document(text);
        assert_eq!(
            document.headings.iter().map(|heading| heading.title.as_str()).collect::<Vec<_>>(),
            ["First", "Nested {Title}", "Unclosed"]
        );
        assert_eq!(document.headings[1].parent, Some(0));
        assert_eq!(document.headings[2].parent, None);
        assert_eq!(&text[document.headings[1].title_range.clone()], "Nested {Title}");
        assert_eq!(document.labels[0].name, "last");
    }

    #[test]
    fn optional_arguments_preserve_nested_braces_comments_and_escaped_delimiters() {
        for optional in [
            "[short]",
            "[[nested]]",
            "[{ignore ] [ in braces}]",
            "[\\] escaped]",
            "[\\{ escaped brace]",
            "[% ignored ]\nshort]",
            "[% ignored ]\\\nshort]",
            "[\\% literal percent]",
            "[} unmatched closing brace]",
            "[{\\} escaped closing brace}]",
            "[\\😀 Unicode escape]",
        ] {
            let text = format!("\\section{optional}{{Title}}\\ref{optional}{{key}}");
            let parsed = document(&text);
            assert_eq!(parsed.headings[0].title, "Title", "{optional}");
            assert_eq!(parsed.references[0].name, "key", "{optional}");
        }
        for optional in ["[unclosed", "[{unclosed]", "[[nested]", "[\\]", "[% comment ]"] {
            let parsed = document(&format!("\\section{optional}"));
            assert!(parsed.headings.is_empty(), "{optional}");
        }
        let parsed = document("\\verb|%| \\section[short]{Title}");
        assert_eq!(parsed.headings[0].title, "Title");
    }

    #[test]
    fn repeated_unclosed_optional_arguments_do_not_create_references() {
        for command in ["\\ref[", "\\ref[{"] {
            let parsed = document(&command.repeat(10_000));
            assert!(parsed.references.is_empty());
        }
    }

    #[test]
    fn indexed_optional_boundaries_match_scanning_for_malformed_delimiters() {
        let mut seed = 1u64;
        for _ in 0..256 {
            let text: String = (0..48)
                .map(|_| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    ['[', ']', '{', '}', '\\', '%', '\n', 'a'][(seed >> 32) as usize % 8]
                })
                .collect();
            for range in optional_brackets(&text) {
                let (mut cursor, mut depth, mut braces) = (range.start + 1, 1usize, 0usize);
                while cursor < text.len() && depth > 0 {
                    match text.as_bytes()[cursor] {
                        b'\\' => {
                            cursor += 1;
                            cursor += usize::from(cursor < text.len());
                            continue;
                        }
                        b'%' => {
                            cursor += text[cursor..].find('\n').unwrap_or(text.len() - cursor);
                            continue;
                        }
                        b'{' => braces += 1,
                        b'}' => braces = braces.saturating_sub(1),
                        b'[' if braces == 0 => depth += 1,
                        b']' if braces == 0 => depth -= 1,
                        _ => {}
                    }
                    cursor += 1;
                }
                assert_eq!(range.end, if depth == 0 { cursor } else { usize::MAX }, "{text:?}, start {}", range.start);
            }
        }
    }

    #[test]
    fn comments_escaped_braces_and_verbatim_do_not_create_false_symbols() {
        let text = "% \\label{comment}\n\\verb|\\label{inline}|\n\\begin{verbatim}\n\\label{hidden}\n\\end{verbatim}\n\\section{A \\} B}\n\\label{real} \\cref{real, other} \\parencite[see][p. 2]{key, second}\n\\bibliography{one,two}\n\\input child";
        let document = document(text);
        assert_eq!(document.labels.len(), 1);
        assert_eq!(document.references.len(), 2);
        assert_eq!(document.citations.len(), 2);
        assert_eq!(document.bibliographies.len(), 2);
        assert_eq!(document.includes[0].name, "child");
        assert!(document.environments[0].end.is_some());
        assert_eq!(document.headings[0].title, "A \\} B");
    }

    #[test]
    fn unmatched_environment_boundaries_are_preserved() {
        let document = document("\\begin{document}\\begin{itemize}\\begin{quote}\\end{itemize}");
        assert_eq!(document.environments[1].parent, Some(0));
        assert!(document.environments[0].end.is_none());
        assert!(document.environments[1].end.is_some());
        assert!(document.environments[2].end.is_none());
    }

    #[test]
    fn multiline_headings_and_multicites_preserve_balanced_arguments() {
        let document = document("\\section[A {B]C}]{First\n\\emph{Second}}\n\\parencites[see]{one}[also]{two,three}\n\\excite{ignored}");
        assert_eq!(document.headings[0].title, "First \\emph{Second}");
        assert_eq!(
            document.citations.iter().map(|key| key.name.as_str()).collect::<Vec<_>>(),
            ["one", "two", "three"]
        );
    }
}
