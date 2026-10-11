//! Versioned completion candidates, local TeX providers, and atomic edit plans.

use ligature_buffer::{DocumentId, Revision};
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub document: DocumentId,
    pub revision: Revision,
    pub caret: usize,
    pub serial: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Local,
    Server,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub detail: String,
    pub filter: String,
    pub sort: String,
    pub edit: Edit,
    pub additional: Vec<Edit>,
    pub snippet: bool,
    pub provider: Provider,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Command,
    Environment,
    Label,
    Citation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub kind: Kind,
    pub range: Range<usize>,
    pub prefix: String,
}

pub fn context(text: &str, caret: usize) -> Option<Context> {
    let before = text.get(..caret)?;
    let line = before.rsplit('\n').next()?;
    let mut escaped = false;
    for c in line.chars() {
        if c == '%' && !escaped {
            return None;
        }
        escaped = c == '\\' && !escaped;
    }
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_alphanumeric() && !matches!(c, ':' | '-' | '_' | '.' | '/'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    if before[..start].ends_with('\\') && !before[..start.saturating_sub(1)].ends_with('\\') {
        return Some(Context {
            kind: Kind::Command,
            range: start..caret,
            prefix: before[start..].into(),
        });
    }
    let brace = before.rfind('{')?;
    if before[brace + 1..].contains(['{', '}', '\\', '\n']) {
        return None;
    }
    let mut command = before[..brace].trim_end();
    while command.ends_with(']') {
        command = command.get(..command.rfind('[')?)?.trim_end();
    }
    let command = command.rsplit_once('\\')?.1.trim_end_matches('*');
    let kind = match command {
        "begin" | "end" => Kind::Environment,
        "ref" | "pageref" | "eqref" | "autoref" | "cref" | "Cref" | "vref" | "nameref" => Kind::Label,
        command if command.to_ascii_lowercase().contains("cite") => Kind::Citation,
        _ => return None,
    };
    let start = before[brace + 1..].rfind(',').map_or(brace + 1, |i| brace + i + 2);
    let start = start + before[start..].len() - before[start..].trim_start().len();
    Some(Context {
        kind,
        range: start..caret,
        prefix: before[start..].into(),
    })
}

pub fn local<'a>(context: &Context, labels: impl Iterator<Item = &'a str>, citations: impl Iterator<Item = &'a str>) -> Vec<Candidate> {
    let names: Box<dyn Iterator<Item = &str> + '_> = match context.kind {
        Kind::Command => Box::new(COMMANDS.iter().copied()),
        Kind::Environment => Box::new(ENVIRONMENTS.iter().copied()),
        Kind::Label => Box::new(labels),
        Kind::Citation => Box::new(citations),
    };
    let prefix = context.prefix.trim_start_matches('\\');
    let lower = prefix.to_lowercase();
    let mut unique = BTreeSet::new();
    let names = names
        .filter(|name| matches_prefix(name, &lower))
        .filter(|name| unique.insert(name.trim_start_matches('\\')))
        .collect();
    best(names, |a: &&str, b: &&str| priority(a, prefix).cmp(&priority(b, prefix)).then(a.cmp(b)))
        .into_iter()
        .map(|name| Candidate {
            label: name.into(),
            detail: format!("{:?}", context.kind),
            filter: name.into(),
            sort: name.into(),
            edit: Edit {
                range: context.range.clone(),
                text: name.into(),
            },
            additional: Vec::new(),
            snippet: false,
            provider: Provider::Local,
        })
        .collect()
}

fn matches_prefix(filter: &str, lower: &str) -> bool {
    let filter = filter.trim_start_matches('\\');
    if filter.is_ascii() {
        filter.get(..lower.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(lower))
    } else {
        filter.to_lowercase().starts_with(lower)
    }
}

fn priority(filter: &str, prefix: &str) -> (bool, bool) {
    let filter = filter.trim_start_matches('\\');
    (filter != prefix, !filter.starts_with(prefix))
}

fn best<T>(mut items: Vec<T>, compare: impl Fn(&T, &T) -> Ordering + Copy) -> Vec<T> {
    if items.len() > 200 {
        items.select_nth_unstable_by(200, compare);
        items.truncate(200);
    }
    items.sort_by(compare);
    items
}

pub fn rank(candidates: Vec<Candidate>, prefix: &str) -> Vec<Candidate> {
    let prefix = prefix.trim_start_matches('\\');
    let lower = prefix.to_lowercase();
    let mut unique: BTreeMap<String, Candidate> = BTreeMap::new();
    for candidate in candidates {
        if !matches_prefix(&candidate.filter, &lower) {
            continue;
        }
        let key = candidate.label.trim_start_matches('\\').to_owned();
        match unique.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) if candidate.provider == Provider::Server => {
                entry.insert(candidate);
            }
            _ => {}
        }
    }
    best(unique.into_values().collect(), |a: &Candidate, b: &Candidate| {
        priority(&a.filter, prefix)
            .cmp(&priority(&b.filter, prefix))
            .then(a.sort.cmp(&b.sort))
            .then(a.label.cmp(&b.label))
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub edit: Edit,
    pub placeholders: Vec<Range<usize>>,
    pub mirrors: Vec<(usize, Range<usize>)>,
    pub caret: usize,
}

pub fn validate(text: &str, candidate: &Candidate) -> Result<(), String> {
    if candidate.additional.is_empty() {
        if text.get(candidate.edit.range.clone()).is_none() {
            return Err("Completion contains invalid or overlapping edits".into());
        }
    } else {
        ordered_edits(text, candidate)?;
    }
    if candidate.snippet {
        snippet(&candidate.edit.text)?;
    }
    Ok(())
}

fn ordered_edits<'a>(text: &str, candidate: &'a Candidate) -> Result<Vec<(bool, &'a Edit)>, String> {
    let mut edits: Vec<_> = candidate.additional.iter().map(|edit| (false, edit)).collect();
    edits.push((true, &candidate.edit));
    edits.sort_by_key(|(_, edit)| (edit.range.start, edit.range.end));
    let mut previous = None;
    for (_, edit) in &edits {
        if text.get(edit.range.clone()).is_none() || previous.is_some_and(|end| edit.range.start < end) {
            return Err("Completion contains invalid or overlapping edits".into());
        }
        if previous == Some(edit.range.start) && edit.range.is_empty() {
            return Err("Completion contains ambiguous edits".into());
        }
        previous = Some(edit.range.end);
    }
    Ok(edits)
}

pub fn plan(text: &str, candidate: &Candidate) -> Result<Plan, String> {
    let (insert, placeholders, mirrors) = if candidate.snippet {
        let expansion = snippet(&candidate.edit.text)?;
        (Cow::Owned(expansion.text), expansion.placeholders, expansion.mirrors)
    } else {
        (Cow::Borrowed(candidate.edit.text.as_str()), Vec::new(), Vec::new())
    };
    let edits = ordered_edits(text, candidate)?;
    let start = edits.first().unwrap().1.range.start;
    let end = edits.last().unwrap().1.range.end;
    let capacity = end - start - edits.iter().map(|(_, edit)| edit.range.len()).sum::<usize>()
        + edits
            .iter()
            .map(|(primary, edit)| if *primary { insert.len() } else { edit.text.len() })
            .sum::<usize>();
    let mut replacement = String::with_capacity(capacity);
    let mut cursor = start;
    let mut insertion = 0;
    for (primary, edit) in edits {
        replacement.push_str(&text[cursor..edit.range.start]);
        if primary {
            insertion = start + replacement.len();
            replacement.push_str(&insert);
        } else {
            replacement.push_str(&edit.text);
        }
        cursor = edit.range.end;
    }
    let placeholders = placeholders
        .into_iter()
        .map(|range| insertion + range.start..insertion + range.end)
        .collect();
    let mirrors = mirrors
        .into_iter()
        .map(|(index, range)| (index, insertion + range.start..insertion + range.end))
        .collect();
    Ok(Plan {
        edit: Edit {
            range: start..end,
            text: replacement,
        },
        placeholders,
        mirrors,
        caret: insertion + insert.len(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    pub placeholders: Vec<Range<usize>>,
    pub mirrors: Vec<(usize, Range<usize>)>,
}

pub fn snippet(source: &str) -> Result<Expansion, String> {
    let mut parser = SnippetParser {
        source,
        cursor: 0,
        text: String::new(),
        stops: BTreeMap::new(),
        values: BTreeMap::new(),
        depth: 0,
    };
    parser.parse(false)?;
    let final_stop = parser
        .stops
        .remove(&0)
        .unwrap_or_else(|| std::iter::once(parser.text.len()..parser.text.len()).collect());
    let mut placeholders = Vec::new();
    let mut mirrors = Vec::new();
    for ranges in parser.stops.into_values().chain(std::iter::once(final_stop)) {
        let index = placeholders.len();
        placeholders.push(ranges[0].clone());
        mirrors.extend(ranges.into_iter().skip(1).map(|range| (index, range)));
    }
    Ok(Expansion {
        text: parser.text,
        placeholders,
        mirrors,
    })
}

struct SnippetParser<'a> {
    source: &'a str,
    cursor: usize,
    text: String,
    stops: BTreeMap<usize, Vec<Range<usize>>>,
    values: BTreeMap<usize, String>,
    depth: usize,
}

impl SnippetParser<'_> {
    fn parse(&mut self, nested: bool) -> Result<(), String> {
        while let Some(c) = self.source[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
            match c {
                '}' if nested => return Ok(()),
                '\\' => {
                    if let Some(c) = self.source[self.cursor..].chars().next().filter(|c| matches!(c, '$' | '}' | '\\')) {
                        self.cursor += c.len_utf8();
                        self.text.push(c);
                    } else {
                        self.text.push('\\');
                    }
                }
                '$' => {
                    let braced = self.source[self.cursor..].starts_with('{');
                    if braced {
                        self.cursor += 1;
                    }
                    let start = self.cursor;
                    while self.source.as_bytes().get(self.cursor).is_some_and(u8::is_ascii_digit) {
                        self.cursor += 1;
                    }
                    let number: usize = self.source[start..self.cursor]
                        .parse()
                        .map_err(|_| "Unsupported snippet variable or transform")?;
                    let start = self.text.len();
                    if braced {
                        let suffix = self.source.as_bytes().get(self.cursor).copied().ok_or("Incomplete snippet")?;
                        self.cursor += 1;
                        match suffix {
                            b':' => {
                                self.depth += 1;
                                if self.depth > 32 {
                                    return Err("Snippet nesting is too deep".into());
                                }
                                self.parse(true)?;
                                self.depth -= 1;
                            }
                            b'|' => {
                                let end = self.source[self.cursor..].find("|}").ok_or("Incomplete snippet choice")? + self.cursor;
                                let choice = self.source[self.cursor..end].split(',').next().unwrap_or_default();
                                self.text.push_str(choice);
                                self.cursor = end + 2;
                            }
                            b'}' => {
                                if let Some(value) = self.values.get(&number) {
                                    self.text.push_str(value);
                                }
                            }
                            _ => return Err("Unsupported snippet transform".into()),
                        }
                    } else if let Some(value) = self.values.get(&number) {
                        self.text.push_str(value);
                    }
                    self.stops.entry(number).or_default().push(start..self.text.len());
                    self.values.entry(number).or_insert_with(|| self.text[start..].into());
                }
                _ => self.text.push(c),
            }
        }
        if nested { Err("Incomplete snippet placeholder".into()) } else { Ok(()) }
    }
}

const COMMANDS: &[&str] = &[
    "documentclass",
    "usepackage",
    "begin",
    "end",
    "part",
    "chapter",
    "section",
    "subsection",
    "subsubsection",
    "paragraph",
    "label",
    "ref",
    "pageref",
    "eqref",
    "autoref",
    "cref",
    "cite",
    "citep",
    "citet",
    "parencite",
    "textcite",
    "addbibresource",
    "bibliography",
    "bibliographystyle",
    "printbibliography",
    "input",
    "include",
    "includegraphics",
    "caption",
    "item",
    "textbf",
    "textit",
    "emph",
    "texttt",
    "footnote",
    "title",
    "author",
    "date",
    "maketitle",
    "tableofcontents",
    "newcommand",
    "renewcommand",
    "frac",
    "sqrt",
    "sum",
    "int",
    "alpha",
    "beta",
    "gamma",
    "delta",
    "theta",
    "lambda",
    "pi",
    "sigma",
    "phi",
    "omega",
    "left",
    "right",
    "mathrm",
    "mathbf",
    "mathcal",
    "centering",
    "hline",
    "newline",
    "newpage",
];
const ENVIRONMENTS: &[&str] = &[
    "document",
    "abstract",
    "itemize",
    "enumerate",
    "description",
    "equation",
    "equation*",
    "align",
    "align*",
    "gather",
    "gather*",
    "split",
    "figure",
    "figure*",
    "table",
    "table*",
    "tabular",
    "tabular*",
    "center",
    "quote",
    "quotation",
    "verbatim",
    "minipage",
    "theorem",
    "proof",
];
