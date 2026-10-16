use ligature_syntax::{Category, Language, tokenize};

#[derive(Debug)]
pub(super) struct Token<'a> {
    pub start: usize,
    pub end: usize,
    pub kind: Kind<'a>,
}

#[derive(Debug)]
pub(super) enum Kind<'a> {
    Begin(&'a str),
    End(&'a str),
    Item,
    Section,
    Command,
    Comment,
    InlineVerbatim,
    Open(char),
    Close(char),
    Math,
    Alignment,
}

impl Kind<'_> {
    pub fn splits(&self) -> bool {
        matches!(self, Self::Begin(_) | Self::End(_) | Self::Item | Self::Section)
    }
}

pub(super) fn scan(line: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let lexical = tokenize(Language::Tex, line);
    let mut cursor = 0;
    for token in lexical {
        let start = token.range.start;
        if start < cursor {
            continue;
        }
        let mut end = token.range.end;
        let value = &line[token.range];
        let kind = match token.category {
            Category::Comment => Kind::Comment,
            Category::MathDelimiter => Kind::Math,
            Category::Brace => {
                if value == "{" {
                    Kind::Open('{')
                } else {
                    Kind::Close('}')
                }
            }
            Category::Command => match value {
                "\\verb" | "\\lstinline" | "\\mintinline" => {
                    tokens.push(Token {
                        start,
                        end: line.len(),
                        kind: Kind::InlineVerbatim,
                    });
                    break;
                }
                "\\begin" | "\\end" => {
                    let rest = line[end..].trim_start_matches([' ', '\t']);
                    if let Some(name) = rest.strip_prefix('{').and_then(|rest| {
                        let end = rest.find(['}', '{', '\\', '%'])?;
                        (rest.as_bytes()[end] == b'}').then_some(&rest[..end])
                    }) && !name.is_empty()
                    {
                        end = line.len() - rest.len() + name.len() + 2;
                        if value == "\\begin" { Kind::Begin(name) } else { Kind::End(name) }
                    } else {
                        Kind::Command
                    }
                }
                "\\item" => Kind::Item,
                "\\part" | "\\chapter" | "\\section" | "\\subsection" | "\\subsubsection" => Kind::Section,
                _ => Kind::Command,
            },
            Category::Text => {
                for (offset, character) in value.char_indices() {
                    let kind = match character {
                        '(' | '[' => Kind::Open(character),
                        ')' | ']' => Kind::Close(character),
                        '&' => Kind::Alignment,
                        _ => continue,
                    };
                    tokens.push(Token {
                        start: start + offset,
                        end: start + offset + 1,
                        kind,
                    });
                }
                continue;
            }
            _ => continue,
        };
        cursor = end;
        tokens.push(Token { start, end, kind });
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_malformed_environments_preserve_the_following_valid_environment() {
        for command in ["begin", "end"] {
            let line = format!("{}quote}}", format!("\\{command}{{").repeat(10_000));
            let tokens = scan(&line);
            assert_eq!(tokens.iter().filter(|token| matches!(token.kind, Kind::Command)).count(), 9_999);
            let last = tokens.last().unwrap();
            assert_eq!(last.end, line.len());
            assert!(matches!(last.kind, Kind::Begin("quote") | Kind::End("quote")));
        }
    }
}
