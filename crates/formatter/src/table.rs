use crate::{
    Line,
    scan::{Kind, scan},
};
use std::ops::Range;

pub(super) fn is_table(name: &str) -> bool {
    matches!(name, "tabular" | "tabular*" | "tabularx" | "longtable" | "xltabular")
}

pub(super) fn align(lines: &mut [Line]) {
    let mut start = None;
    for index in 0..lines.len() {
        if lines[index].protected {
            start = None;
            continue;
        }
        let tokens = scan(&lines[index].text);
        for token in tokens {
            match token.kind {
                Kind::Begin(name) if is_table(name) && lines[index].text[..token.start].trim().is_empty() => {
                    start = Some((index + 1, name.to_owned()));
                }
                Kind::End(name) if start.as_ref().is_some_and(|(_, open)| open == name) && lines[index].text[..token.start].trim().is_empty() => {
                    let (begin, _) = start.take().unwrap();
                    if begin <= index {
                        align_rows(&mut lines[begin..index]);
                    }
                    break;
                }
                _ => {}
            }
        }
    }
}

struct Cell {
    range: Range<usize>,
    width: usize,
}

fn cells(line: &str) -> Option<Vec<Cell>> {
    let mut cells = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    for token in scan(line) {
        match token.kind {
            Kind::Begin(_) | Kind::End(_) => return None,
            Kind::Open(_) => depth += 1,
            Kind::Close(_) => depth = depth.saturating_sub(1),
            Kind::Alignment if depth == 0 => {
                cells.push(cell(line, start, start + crate::trim_end(&line[start..token.start]).len()));
                start = token.end;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    if start != 0 {
        cells.push(cell(line, start, line.len()));
    }
    Some(cells)
}

fn cell(line: &str, start: usize, end: usize) -> Cell {
    let text = &line[start..end];
    let text = if start == 0 { text } else { text.trim_start_matches([' ', '\t']) };
    Cell {
        range: end - text.len()..end,
        width: text.chars().count(),
    }
}

fn align_rows(lines: &mut [Line]) {
    let Some(rows) = lines.iter().map(|line| cells(&line.text)).collect::<Option<Vec<_>>>() else {
        return;
    };
    let mut widths = Vec::new();
    for cells in &rows {
        for (column, cell) in cells.iter().enumerate().take(cells.len().saturating_sub(1)) {
            if column == widths.len() {
                widths.push(cell.width);
            } else {
                widths[column] = widths[column].max(cell.width);
            }
        }
    }
    for (line, cells) in lines.iter_mut().zip(rows) {
        if cells.is_empty() {
            continue;
        }
        let mut text = String::with_capacity(line.text.len());
        for (column, cell) in cells.iter().enumerate() {
            text.push_str(&line.text[cell.range.clone()]);
            if column + 1 < cells.len() {
                text.extend(std::iter::repeat_n(' ', widths[column] - cell.width + 1));
                text.push_str("& ");
            }
        }
        line.text = text;
    }
}
