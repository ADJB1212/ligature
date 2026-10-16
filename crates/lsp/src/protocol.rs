use std::{
    io::{self, BufRead, Read, Write},
    ops::Range,
    path::Path,
};

use ligature_completion::{Candidate, Edit, Provider};
use ligature_diagnostics::{Diagnostic, Severity, SourcePosition, SourceRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_FRAME: usize = 16 * 1024 * 1024;

pub fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    let mut total = 0;
    loop {
        let mut header = Vec::new();
        let count = (&mut *reader).take(8193).read_until(b'\n', &mut header)?;
        if count == 0 && total == 0 {
            return Ok(None);
        }
        total += count;
        if count == 0 || total > 8192 || !header.ends_with(b"\r\n") {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid LSP header"));
        }
        if header == b"\r\n" {
            break;
        }
        let header = std::str::from_utf8(&header).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let (name, value) = header
            .trim()
            .split_once(':')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid LSP header"))?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Duplicate LSP content length"));
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            );
        }
    }
    let length = length
        .filter(|length| *length <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid LSP content length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn write_frame(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "LSP message is too large"));
    }
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

fn units(text: &str, encoding: Encoding) -> usize {
    match encoding {
        Encoding::Utf8 => text.len(),
        Encoding::Utf16 => text.encode_utf16().count(),
        Encoding::Utf32 => text.chars().count(),
    }
}

fn lines(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start > text.len() {
            return None;
        }
        let end = memchr::memchr2(b'\r', b'\n', &text.as_bytes()[start..]).map_or(text.len(), |end| start + end);
        let range = start..end;
        start = if text[end..].starts_with("\r\n") { end + 2 } else { end + 1 };
        Some(range)
    })
}

pub fn position(text: &str, offset: usize, encoding: Encoding) -> Option<Position> {
    if !text.is_char_boundary(offset) {
        return None;
    }
    lines(text).enumerate().find_map(|(line, range)| {
        (offset >= range.start && offset <= range.end).then(|| Position {
            line,
            character: units(&text[range.start..offset], encoding),
        })
    })
}

fn offset(text: &str, position: Position, encoding: Encoding) -> Option<usize> {
    let range = lines(text).nth(position.line)?;
    offset_in_line(text, range, position.character, encoding)
}

fn offset_in_line(text: &str, range: Range<usize>, character: usize, encoding: Encoding) -> Option<usize> {
    let line = &text[range.clone()];
    let mut column = 0;
    for (index, c) in line.char_indices() {
        if column == character {
            return Some(range.start + index);
        }
        column += match encoding {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
            Encoding::Utf32 => 1,
        };
        if column > character {
            return None;
        }
    }
    (column == character).then_some(range.end)
}

struct LineIndex<'a> {
    text: &'a str,
    lines: Vec<Range<usize>>,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            lines: lines(text).collect(),
        }
    }

    fn offset(&self, value: &Value, encoding: Encoding) -> Option<usize> {
        let position: Position = serde_json::from_value(value.clone()).ok()?;
        offset_in_line(self.text, self.lines.get(position.line)?.clone(), position.character, encoding)
    }

    fn byte_range(&self, value: &Value, encoding: Encoding) -> Option<Range<usize>> {
        let start = self.offset(value.get("start")?, encoding)?;
        let end = self.offset(value.get("end")?, encoding)?;
        (start <= end).then_some(start..end)
    }

    fn scalar_position(&self, offset: usize) -> SourcePosition {
        let line = self.lines.partition_point(|range| range.start <= offset) - 1;
        SourcePosition {
            line: line + 1,
            column: self.text[self.lines[line].start..offset].chars().count() + 1,
        }
    }
}

pub fn completion_position(text: &str, caret: usize, encoding: Encoding) -> Option<Position> {
    // Texlab excludes EOF when locating completion tokens.
    let offset = if caret == text.len() {
        text.char_indices().next_back().map_or(0, |(offset, _)| offset)
    } else {
        caret
    };
    position(text, offset, encoding)
}

pub fn byte_range(text: &str, value: &Value, encoding: Encoding) -> Option<Range<usize>> {
    let start = offset(text, serde_json::from_value(value.get("start")?.clone()).ok()?, encoding)?;
    let end = offset(text, serde_json::from_value(value.get("end")?.clone()).ok()?, encoding)?;
    (start <= end).then_some(start..end)
}

pub fn uri(path: &Path) -> Result<String, String> {
    url::Url::from_file_path(path)
        .map(String::from)
        .map_err(|()| "Language-server document paths must be absolute".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMode {
    None,
    Full,
    Incremental,
}

#[derive(Clone, Debug)]
pub struct Capabilities {
    pub encoding: Encoding,
    pub sync: SyncMode,
    pub open_close: bool,
    pub save: bool,
    pub save_text: bool,
    pub completion: bool,
    pub hover: bool,
}

impl Capabilities {
    pub fn parse(value: &Value) -> Result<Self, String> {
        let encoding = match value.get("positionEncoding").and_then(Value::as_str).unwrap_or("utf-16") {
            "utf-8" => Encoding::Utf8,
            "utf-16" => Encoding::Utf16,
            "utf-32" => Encoding::Utf32,
            _ => return Err("Language server selected an unsupported position encoding".into()),
        };
        let sync = &value["textDocumentSync"];
        let legacy = sync.as_u64();
        let mode = legacy.or_else(|| sync["change"].as_u64()).unwrap_or(0);
        Ok(Self {
            encoding,
            sync: match mode {
                1 => SyncMode::Full,
                2 => SyncMode::Incremental,
                _ => SyncMode::None,
            },
            open_close: legacy.is_some_and(|mode| mode != 0) || sync["openClose"] == true,
            save: sync["save"] == true || sync["save"].is_object(),
            save_text: sync["save"]["includeText"] == true,
            completion: value["completionProvider"].is_object(),
            hover: value["hoverProvider"] == true || value["hoverProvider"].is_object(),
        })
    }

    pub fn change(&self, old: &str, new: &str) -> Option<Value> {
        match self.sync {
            SyncMode::None => None,
            SyncMode::Full => Some(json!({"text":new})),
            SyncMode::Incremental => {
                let mut start = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
                while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
                    start -= 1;
                }
                let mut suffix = old[start..]
                    .bytes()
                    .rev()
                    .zip(new[start..].bytes().rev())
                    .take_while(|(a, b)| a == b)
                    .count();
                while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
                    suffix -= 1;
                }
                let mut end = old.len() - suffix;
                let mut new_end = new.len() - suffix;
                if old[..start].ends_with('\r') || new[..start].ends_with('\r') {
                    start = start.saturating_sub(1);
                }
                if old[end..].starts_with('\n') && old[..end].ends_with('\r') {
                    end += 1;
                    new_end += 1;
                }
                Some(json!({"range":{"start":position(old,start,self.encoding)?,"end":position(old,end,self.encoding)?},"text":&new[start..new_end]}))
            }
        }
    }
}

pub fn completions(text: &str, result: &Value, fallback: Range<usize>, encoding: Encoding) -> Vec<Candidate> {
    let Some(items) = result.as_array().or_else(|| result["items"].as_array()) else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let lines = std::cell::LazyCell::new(|| LineIndex::new(text));
    items
        .iter()
        .take(2000)
        .filter_map(|item| {
            let label = item["label"].as_str()?;
            let insertion = item["textEdit"]["newText"]
                .as_str()
                .or_else(|| item["insertText"].as_str())
                .unwrap_or(label);
            let range = if item["textEdit"].is_object() {
                lines.byte_range(item["textEdit"].get("range").or_else(|| item["textEdit"].get("replace"))?, encoding)?
            } else {
                fallback.clone()
            };
            let additional: Option<Vec<_>> = item["additionalTextEdits"]
                .as_array()
                .map(|edits| {
                    edits
                        .iter()
                        .map(|edit| {
                            Some(Edit {
                                range: lines.byte_range(&edit["range"], encoding)?,
                                text: edit["newText"].as_str()?.into(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_else(|| Some(Vec::new()));
            let candidate = Candidate {
                label: label.into(),
                detail: item["detail"].as_str().unwrap_or_default().into(),
                filter: item["filterText"].as_str().unwrap_or(label).into(),
                sort: item["sortText"].as_str().unwrap_or(label).into(),
                edit: Edit {
                    range,
                    text: insertion.into(),
                },
                additional: additional?,
                snippet: item["insertTextFormat"] == 2,
                provider: Provider::Server,
            };
            ligature_completion::validate(text, &candidate).ok()?;
            Some(candidate)
        })
        .collect()
}

pub fn diagnostics(text: &str, path: &Path, items: &Value, encoding: Encoding) -> Vec<Diagnostic> {
    let Some(items) = items.as_array().filter(|items| !items.is_empty()) else {
        return Vec::new();
    };
    let lines = LineIndex::new(text);
    items
        .iter()
        .take(ligature_diagnostics::MAX_DIAGNOSTICS)
        .filter_map(|item| {
            let range = lines.byte_range(&item["range"], encoding)?;
            let mut message = item["message"].as_str()?.to_owned();
            ligature_diagnostics::bound_message(&mut message);
            Some(Diagnostic {
                severity: match item["severity"].as_u64().unwrap_or(1) {
                    1 => Severity::Error,
                    2 => Severity::Warning,
                    _ => Severity::Information,
                },
                provider: ligature_diagnostics::Provider::LanguageServer("Texlab".into()),
                message,
                path: Some(path.into()),
                range: Some(SourceRange::Text {
                    start: lines.scalar_position(range.start),
                    end: lines.scalar_position(range.end),
                }),
            })
        })
        .collect()
}

pub fn hover(result: &Value) -> Option<String> {
    fn append(text: &str, output: &mut String, remaining: &mut usize) {
        let end = text
            .char_indices()
            .take(*remaining)
            .map(|(offset, character)| {
                *remaining -= 1;
                offset + character.len_utf8()
            })
            .last()
            .unwrap_or(0);
        output.push_str(&text[..end]);
    }
    fn content(value: &Value, output: &mut String, remaining: &mut usize) {
        if *remaining == 0 {
            return;
        }
        if let Some(text) = value.as_str() {
            append(text, output, remaining);
        } else if let Some(items) = value.as_array() {
            for (index, item) in items.iter().enumerate() {
                if *remaining == 0 {
                    break;
                }
                if index > 0 {
                    append("\n\n", output, remaining);
                }
                content(item, output, remaining);
            }
        } else {
            append(value["value"].as_str().unwrap_or_default(), output, remaining);
        }
    }
    let mut text = String::new();
    content(&result["contents"], &mut text, &mut 8000);
    (!text.trim().is_empty()).then_some(text)
}
