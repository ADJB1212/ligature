use crate::{
    Line,
    scan::{Kind, scan},
};

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

fn cells(line: &str) -> Vec<&str> {
    let mut cells = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    for token in scan(line) {
        match token.kind {
            Kind::Open(_) => depth += 1,
            Kind::Close(_) => depth = depth.saturating_sub(1),
            Kind::Alignment if depth == 0 => {
                cells.push(crate::trim_end(&line[start..token.start]));
                start = token.end;
            }
            _ => {}
        }
    }
    if start != 0 {
        cells.push(&line[start..]);
    }
    cells
}

fn align_rows(lines: &mut [Line]) {
    for line in lines.iter() {
        let mut depth = 0usize;
        for token in scan(&line.text) {
            match token.kind {
                Kind::Begin(_) | Kind::End(_) => return,
                Kind::Open(_) => depth += 1,
                Kind::Close(_) => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if depth != 0 {
            return;
        }
    }
    let mut widths = Vec::new();
    for line in lines.iter() {
        let cells = cells(&line.text);
        for (column, cell) in cells.iter().enumerate().take(cells.len().saturating_sub(1)) {
            let width = if column == 0 {
                cell.chars().count()
            } else {
                cell.trim_start_matches([' ', '\t']).chars().count()
            };
            if column == widths.len() {
                widths.push(width);
            } else {
                widths[column] = widths[column].max(width);
            }
        }
    }
    for line in lines {
        let cells = cells(&line.text);
        if cells.is_empty() {
            continue;
        }
        let mut text = String::new();
        for (column, cell) in cells.iter().enumerate() {
            let cell = if column == 0 { cell } else { cell.trim_start_matches([' ', '\t']) };
            text.push_str(cell);
            if column + 1 < cells.len() {
                text.extend(std::iter::repeat_n(' ', widths[column] - cell.chars().count() + 1));
                text.push_str("& ");
            }
        }
        line.text = text;
    }
}
