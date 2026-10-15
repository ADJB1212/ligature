//! Built-in, tolerant TeX source formatting inspired by tex-fmt
//!
//! Formatting operates on snapshots without reading configuration or writing files.
//! TeX macros and catcode changes are not evaluated.

mod scan;
mod table;

use scan::{Kind, Token, scan};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indentation {
    Spaces(usize),
    Tabs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub indentation: Indentation,
    /// Maximum Unicode scalar width, including indentation. None disables wrapping.
    pub line_width: Option<usize>,
    pub format_tables: bool,
    pub list_environments: Vec<String>,
    pub verbatim_environments: Vec<String>,
    pub no_indent_environments: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            indentation: Indentation::Spaces(2),
            line_width: Some(80),
            format_tables: false,
            list_environments: names(&["itemize", "enumerate", "description", "inlineroman", "inventory"]),
            verbatim_environments: names(&["verbatim", "verbatim*", "Verbatim", "Verbatim*", "lstlisting", "minted", "comment"]),
            no_indent_environments: names(&["document"]),
        }
    }
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Format a complete TeX snapshot using the default options.
pub fn format(text: &str) -> String {
    format_with_options(text, &Options::default())
}

/// Format a snapshot, preserving raw lines in verbatim and fmt off/skip regions.
///
/// Normal lines use the first input line ending (LF for input without newlines),
/// at most one blank line, and a final newline. Empty input stays empty. Unmatched
/// closing constructs cannot make indentation negative. Inline verbatim lines are
/// preserved too. Options apply only to this call.
pub fn format_with_options(text: &str, options: &Options) -> String {
    let newline = text.find('\n').map_or("\n", |index| {
        if index > 0 && text.as_bytes()[index - 1] == b'\r' {
            "\r\n"
        } else {
            "\n"
        }
    });
    let mut formatter = Formatter {
        options,
        newline,
        lines: Vec::new(),
        environments: Vec::new(),
        groups: Vec::new(),
        verbatim: None,
        disabled: false,
    };
    for raw in text.split_inclusive('\n') {
        formatter.line(raw);
    }
    while formatter.lines.last().is_some_and(|line| !line.protected && line.text.is_empty()) {
        formatter.lines.pop();
    }
    if options.format_tables {
        table::align(&mut formatter.lines);
    }
    let mut output = String::with_capacity(text.len());
    for line in formatter.lines {
        output.push_str(&line.text);
        output.push_str(&line.ending);
    }
    output
}

struct Line {
    text: String,
    ending: String,
    protected: bool,
}

struct Environment {
    name: String,
    indent: usize,
}

struct Formatter<'a> {
    options: &'a Options,
    newline: &'a str,
    lines: Vec<Line>,
    environments: Vec<Environment>,
    groups: Vec<char>,
    verbatim: Option<String>,
    disabled: bool,
}

impl Formatter<'_> {
    fn raw(&mut self, raw: &str) {
        let text = raw.strip_suffix('\n').unwrap_or(raw);
        let text = if raw.ends_with("\r\n") { text.strip_suffix('\r').unwrap() } else { text };
        self.lines.push(Line {
            text: text.to_owned(),
            ending: raw[text.len()..].to_owned(),
            protected: true,
        });
    }

    fn line(&mut self, raw: &str) {
        let line = raw.strip_suffix("\r\n").or_else(|| raw.strip_suffix('\n')).unwrap_or(raw);
        if let Some(environment) = &self.verbatim {
            let end = format!("\\end{{{environment}}}");
            if line.trim_start().starts_with(&end) {
                self.verbatim = None;
            }
            self.raw(raw);
            return;
        }
        let tokens = scan(line);
        let directive = tokens
            .iter()
            .find(|token| matches!(token.kind, Kind::Comment))
            .map(|token| line[token.start..].trim_end());
        let was_disabled = self.disabled;
        match directive {
            Some("% fmt: off") => self.disabled = true,
            Some("% fmt: on") => self.disabled = false,
            _ => {}
        }
        if was_disabled || self.disabled || matches!(directive, Some("% fmt: on" | "% fmt: skip")) {
            self.raw(raw);
            return;
        }
        if tokens.iter().any(|token| matches!(token.kind, Kind::InlineVerbatim)) {
            self.raw(raw);
            return;
        }
        let mut start = 0;
        let mut group_depth = self.groups.len();
        for token in &tokens {
            match &token.kind {
                Kind::Open(_) => group_depth += 1,
                Kind::Close(_) => group_depth = group_depth.saturating_sub(1),
                _ => {}
            }
            if group_depth == 0 && token.kind.splits() && !line[start..token.start].trim().is_empty() {
                self.segment(&line[start..token.start]);
                start = token.start;
            }
            if let Kind::Begin(name) = &token.kind
                && self.options.verbatim_environments.iter().any(|value| value == name)
            {
                if !line[start..token.start].trim().is_empty() {
                    self.segment(&line[start..token.start]);
                    start = token.start;
                }
                let end = format!("\\end{{{name}}}");
                if !line[token.end..].contains(&end) {
                    self.verbatim = Some((*name).to_owned());
                }
                self.raw(&raw[start..]);
                return;
            }
        }
        self.segment(&line[start..]);
    }

    fn segment(&mut self, line: &str) {
        let line = trim_end(line.trim_start_matches([' ', '\t']));
        if line.is_empty() {
            if self.lines.last().is_some_and(|line| !line.text.is_empty() || line.protected) {
                self.push(String::new());
            }
            return;
        }
        let tokens = scan(line);
        let mut depth = self.depth();
        if let Some(Token { kind: Kind::End(name), .. }) = tokens.first()
            && let Some(index) = self.environments.iter().rposition(|environment| environment.name == *name)
        {
            depth = self.environments[..index].iter().map(|environment| environment.indent).sum::<usize>() + self.groups.len();
        } else if tokens.first().is_some_and(|token| matches!(token.kind, Kind::Item))
            && self.environments.last().is_some_and(|environment| environment.indent == 2)
        {
            depth = depth.saturating_sub(1);
        }
        let mut prefix = 0;
        for token in &tokens {
            if matches!(token.kind, Kind::Close(_)) && line[prefix..token.start].trim().is_empty() {
                depth = depth.saturating_sub(1);
                prefix = token.end;
            } else {
                break;
            }
        }
        self.wrap(line, &tokens, depth);
        for token in tokens {
            match token.kind {
                Kind::Begin(name) => {
                    let indent = if self.options.no_indent_environments.iter().any(|value| value == name) {
                        0
                    } else if self.options.list_environments.iter().any(|value| value == name) {
                        2
                    } else {
                        1
                    };
                    self.environments.push(Environment {
                        name: name.to_owned(),
                        indent,
                    });
                }
                Kind::End(name) => {
                    if let Some(index) = self.environments.iter().rposition(|environment| environment.name == name) {
                        self.environments.truncate(index);
                    }
                }
                Kind::Open(character) => self.groups.push(character),
                Kind::Close(character)
                    if self
                        .groups
                        .last()
                        .is_some_and(|open| matches!((*open, character), ('{', '}') | ('[', ']') | ('(', ')'))) =>
                {
                    self.groups.pop();
                }
                _ => {}
            }
        }
    }

    fn depth(&self) -> usize {
        self.environments.iter().map(|environment| environment.indent).sum::<usize>() + self.groups.len()
    }

    fn push(&mut self, text: String) {
        self.lines.push(Line {
            text,
            ending: self.newline.to_owned(),
            protected: false,
        });
    }

    fn wrap(&mut self, line: &str, tokens: &[Token<'_>], depth: usize) {
        let (unit, width) = match self.options.indentation {
            Indentation::Spaces(width) => (" ".repeat(width), width),
            Indentation::Tabs => ("\t".to_owned(), 2),
        };
        let indent = unit.repeat(depth);
        let in_table = self.environments.iter().any(|environment| table::is_table(&environment.name));
        let limit = self.options.line_width.filter(|_| !in_table && self.groups.is_empty());
        let comment_start = tokens.iter().find(|token| matches!(token.kind, Kind::Comment)).map(|token| token.start);
        let mut rest = line;
        let mut start = 0;
        loop {
            let prefix = if comment_start.is_some_and(|comment| comment < start) {
                "% "
            } else {
                ""
            };
            let available = limit.map(|width_limit| width_limit.saturating_sub(depth.saturating_mul(width).saturating_add(prefix.len())));
            let split = available
                .filter(|&width| rest.chars().nth(width).is_some())
                .and_then(|width| wrap_point(line, tokens, start, width));
            let (part, next) = split.map_or((rest, None), |index| {
                (&line[start..index], Some(line[index + 1..].trim_start_matches([' ', '\t'])))
            });
            self.push(format!("{indent}{prefix}{}", trim_end(part)));
            let Some(next) = next else { break };
            start = line.len() - next.len();
            rest = next;
        }
    }
}

fn trim_end(line: &str) -> &str {
    let trimmed = line.trim_end_matches([' ', '\t']);
    let slashes = trimmed.bytes().rev().take_while(|&byte| byte == b'\\').count();
    if slashes % 2 == 1 && line.as_bytes().get(trimmed.len()) == Some(&b' ') {
        &line[..trimmed.len() + 1]
    } else {
        trimmed
    }
}

fn wrap_point(line: &str, tokens: &[Token<'_>], start: usize, width: usize) -> Option<usize> {
    let mut token_index = tokens.partition_point(|token| token.end <= start);
    let mut groups = 0usize;
    let mut math = false;
    let mut comment = false;
    let mut candidate = None;
    let mut previous_space = false;
    let mut has_text = false;
    for (column, (offset, character)) in line[start..].char_indices().enumerate() {
        let index = start + offset;
        while let Some(token) = tokens.get(token_index).filter(|token| token.start <= index) {
            match token.kind {
                Kind::Open(_) if !comment => groups += 1,
                Kind::Close(_) if !comment => groups = groups.saturating_sub(1),
                Kind::Math if !comment => math = !math,
                Kind::Comment => comment = true,
                _ => {}
            }
            token_index += 1;
        }
        if column > width && candidate.is_some() {
            break;
        }
        let protected = token_index
            .checked_sub(1)
            .and_then(|index| tokens.get(index))
            .is_some_and(|token| token.start <= index && index < token.end && !matches!(token.kind, Kind::Comment));
        let space = character == ' ' || character == '\t';
        if space && !previous_space && !protected && (comment || (groups == 0 && !math)) && has_text && index + character.len_utf8() < line.len() {
            candidate = Some(index);
        }
        has_text |= !character.is_whitespace() && character != '%';
        previous_space = space;
    }
    candidate
}
