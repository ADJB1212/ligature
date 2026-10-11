//! SyncTeX source-to-page and page-to-source position mapping.

use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
/// One-based line and character column; column zero means no column mapping.
pub struct SourcePosition {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}

/// One-based page and top-left coordinates in PDF points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfPosition {
    pub page: usize,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug)]
struct Record {
    path: Arc<PathBuf>,
    line: usize,
    column: usize,
    pdf: PdfPosition,
    container: bool,
}

#[derive(Clone, Debug)]
pub struct Mapping {
    records: Vec<Record>,
    sources: BTreeMap<Arc<PathBuf>, Vec<usize>>,
    pages: BTreeMap<usize, Vec<usize>>,
}

pub fn sidecar(pdf: &Path) -> Option<PathBuf> {
    [pdf.with_extension("synctex.gz"), pdf.with_extension("synctex")]
        .into_iter()
        .find(|path| path.is_file())
}

impl Mapping {
    pub fn load(path: &Path, source_directory: &Path, page_count: usize) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|error| format!("Cannot read SyncTeX: {error}"))?;
        let reader: Box<dyn Read> = if path.extension().is_some_and(|extension| extension == "gz") {
            Box::new(flate2::read::GzDecoder::new(file))
        } else {
            Box::new(file)
        };
        let mut text = String::new();
        reader
            .take(MAX_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|error| format!("Cannot decode SyncTeX: {error}"))?;
        if text.len() as u64 > MAX_BYTES {
            return Err("SyncTeX exceeds the 64 MiB limit".into());
        }
        Self::parse(&text, source_directory, page_count)
    }

    pub fn parse(text: &str, source_directory: &Path, page_count: usize) -> Result<Self, String> {
        if !text.starts_with("SyncTeX Version:1\n") || !text.contains("\nOutput:pdf\n") || !text.contains("\nPostamble:\n") {
            return Err("Unsupported or incomplete SyncTeX document".into());
        }
        let mut inputs = BTreeMap::new();
        let mut unit = 1.0;
        let mut magnification = 1000.0;
        let mut offsets = (0.0, 0.0);
        let mut page = 0;
        let mut records = Vec::new();
        for line in text.lines() {
            if let Some(input) = line.strip_prefix("Input:") {
                let (tag, name) = input.split_once(':').ok_or("Invalid SyncTeX input")?;
                let tag: usize = tag.parse().map_err(|_| "Invalid SyncTeX input tag")?;
                let path = Path::new(name);
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    source_directory.join(path)
                };
                inputs.insert(tag, Arc::new(std::fs::canonicalize(&path).unwrap_or(path)));
            } else if let Some(value) = line.strip_prefix("Unit:") {
                unit = number(value)?;
            } else if let Some(value) = line.strip_prefix("Magnification:") {
                magnification = number(value)?;
            } else if let Some(value) = line.strip_prefix("X Offset:") {
                offsets.0 = number(value)?;
            } else if let Some(value) = line.strip_prefix("Y Offset:") {
                offsets.1 = number(value)?;
            } else if let Some(value) = line.strip_prefix('{') {
                page = value.parse().map_err(|_| "Invalid SyncTeX page")?;
                if page == 0 || page > page_count {
                    return Err("SyncTeX pages do not match the current PDF".into());
                }
            } else if line.starts_with('}') {
                page = 0;
            } else if page > 0 && line.starts_with(['x', 'g', 'k', 'h', 'v', '[', '(', 'r', '$']) {
                let mut fields = line[1..].trim_start().split(':');
                let link = fields.next().ok_or("Missing SyncTeX source")?;
                let point = fields.next().ok_or("Missing SyncTeX point")?;
                let mut link = link.split(',');
                let tag: usize = link.next().unwrap_or_default().parse().map_err(|_| "Invalid SyncTeX tag")?;
                let source_line: usize = link.next().unwrap_or_default().parse().map_err(|_| "Invalid SyncTeX line")?;
                let column = link
                    .next()
                    .map(str::parse)
                    .transpose()
                    .map_err(|_| "Invalid SyncTeX column")?
                    .unwrap_or(0);
                let (x, y) = point.split_once(',').ok_or("Invalid SyncTeX point")?;
                let factor = unit * magnification / 1000.0 / 65536.0 * 72.0 / 72.27;
                if !factor.is_finite() || factor <= 0.0 {
                    return Err("Invalid SyncTeX scale".into());
                }
                if source_line > 0 {
                    let path = inputs.get(&tag).ok_or("Unknown SyncTeX input tag")?;
                    records.push(Record {
                        container: line.starts_with(['[', '(']),
                        path: Arc::clone(path),
                        line: source_line,
                        column,
                        pdf: PdfPosition {
                            page,
                            x: (number(x)? + offsets.0) * factor,
                            y: (number(y)? + offsets.1) * factor,
                        },
                    });
                }
            }
        }
        if records.is_empty() {
            return Err("SyncTeX contains no source mappings".into());
        }
        let mut sources: BTreeMap<Arc<PathBuf>, Vec<usize>> = BTreeMap::new();
        let mut pages: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (index, record) in records.iter().enumerate() {
            sources.entry(Arc::clone(&record.path)).or_default().push(index);
            pages.entry(record.pdf.page).or_default().push(index);
        }
        for indices in sources.values_mut() {
            indices.sort_by_key(|&index| records[index].line);
        }
        Ok(Self { records, sources, pages })
    }

    pub fn forward(&self, source: &SourcePosition) -> Option<PdfPosition> {
        let indices = self.sources.get(&source.path).or_else(|| {
            let path = std::fs::canonicalize(&source.path).ok()?;
            self.sources.get(&path)
        })?;
        let next = indices.partition_point(|&index| self.records[index].line < source.line);
        let distance = indices
            .get(next)
            .into_iter()
            .chain(next.checked_sub(1).and_then(|previous| indices.get(previous)))
            .map(|&index| self.records[index].line.abs_diff(source.line))
            .min()?;
        let first = indices.partition_point(|&index| self.records[index].line < source.line.saturating_sub(distance));
        let end = indices.partition_point(|&index| self.records[index].line <= source.line.saturating_add(distance));
        indices[first..end]
            .iter()
            .copied()
            .min_by_key(|&index| {
                let record = &self.records[index];
                (
                    record.line.abs_diff(source.line),
                    record.container,
                    record.column.abs_diff(source.column),
                    index,
                )
            })
            .map(|index| self.records[index].pdf)
    }

    pub fn inverse(&self, point: PdfPosition) -> Option<SourcePosition> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        self.pages
            .get(&point.page)?
            .iter()
            .map(|&index| {
                let record = &self.records[index];
                let distance = (record.pdf.x - point.x).powi(2) + (record.pdf.y - point.y).powi(2);
                (record, distance)
            })
            .min_by(|(a, a_distance), (b, b_distance)| a_distance.total_cmp(b_distance).then(a.container.cmp(&b.container)))
            .map(|(record, _)| SourcePosition {
                path: (*record.path).clone(),
                line: record.line,
                column: record.column,
            })
    }
}

fn number(value: &str) -> Result<f64, String> {
    let value: f64 = value.parse().map_err(|_| "Invalid SyncTeX number")?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err("Invalid SyncTeX number".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = "SyncTeX Version:1\nInput:1:main.tex\nInput:2:chapters/with spaces.tex\nOutput:pdf\nMagnification:1000\nUnit:1\nX Offset:0\nY Offset:0\nContent:\n{1\nx1,3:65536,131072\nx2,12:196608,262144\n}1\nPostamble:\nCount:2\nPost scriptum:\n";

    #[test]
    fn forward_lookup_preserves_nearest_line_content_column_and_record_order() {
        let data = DATA.replace(
            "}1\n",
            "x1,7,8:196608,262144\n[1,5,2:327680,393216\nx1,5,9:458752,524288\nx1,3,4:589824,655360\nx1,5,1:720896,786432\n}1\n",
        );
        let mapping = Mapping::parse(&data, Path::new("/project"), 1).unwrap();
        for line in [0, 1, 3, 4, 5, 6, 7, 8, usize::MAX] {
            for column in [0, 1, 2, 4, 8, 9, usize::MAX] {
                let source = SourcePosition {
                    path: "/project/main.tex".into(),
                    line,
                    column,
                };
                let expected = mapping
                    .records
                    .iter()
                    .filter(|record| record.path.as_ref() == &source.path)
                    .min_by_key(|record| (record.line.abs_diff(line), record.container, record.column.abs_diff(column)))
                    .map(|record| record.pdf);
                assert_eq!(mapping.forward(&source), expected, "line {line}, column {column}");
            }
        }
    }

    #[test]
    fn indexed_lookups_preserve_record_order_and_page_isolation() {
        let data = DATA.replace("}1\n", "x1,3:327680,393216\n}1\n{2\nx2,20:65536,131072\n}2\n");
        let mapping = Mapping::parse(&data, Path::new("/project"), 2).unwrap();
        let source = SourcePosition {
            path: "/project/main.tex".into(),
            line: 3,
            column: 0,
        };
        let point = mapping.forward(&source).unwrap();
        assert_eq!(point, mapping.records[0].pdf);
        assert_eq!(mapping.inverse(point), Some(source));
        let other = mapping.inverse(PdfPosition { page: 2, ..point }).unwrap();
        assert_eq!(other.path, Path::new("/project/chapters/with spaces.tex"));
        assert_eq!(other.line, 20);
        assert!(mapping.inverse(PdfPosition { page: 3, ..point }).is_none());
        assert!(
            mapping
                .forward(&SourcePosition {
                    path: "/project/missing.tex".into(),
                    line: 3,
                    column: 0
                })
                .is_none()
        );
    }

    #[test]
    fn repeated_input_records_preserve_independent_source_positions() {
        let data = DATA.replace("x2,12:196608,262144", "x1,3,8:196608,262144\nx1,12,2:327680,393216");
        let mapping = Mapping::parse(&data, Path::new("/project"), 1).unwrap();
        for (line, column) in [(3, 0), (3, 8), (12, 2)] {
            let source = SourcePosition {
                path: "/project/main.tex".into(),
                line,
                column,
            };
            assert_eq!(mapping.inverse(mapping.forward(&source).unwrap()), Some(source));
        }
    }

    #[test]
    fn included_files_and_spaces_map_in_both_directions() {
        let mapping = Mapping::parse(DATA, Path::new("/project"), 1).unwrap();
        let source = SourcePosition {
            path: "/project/chapters/with spaces.tex".into(),
            line: 12,
            column: 0,
        };
        let pdf = mapping.forward(&source).unwrap();
        assert_eq!(mapping.inverse(pdf), Some(source));
        assert!((pdf.x - 3.0 * 72.0 / 72.27).abs() < 0.0001);
    }

    #[test]
    fn rejects_incomplete_and_mismatched_sidecars() {
        assert!(Mapping::parse(DATA, Path::new("/project"), 0).is_err());
        assert!(Mapping::parse(&DATA.replace("Postamble:", ""), Path::new("/project"), 1).is_err());
        assert!(Mapping::parse(&DATA.replace("Unit:1", "Unit:NaN"), Path::new("/project"), 1).is_err());
    }

    #[test]
    fn loads_gzip_sidecars_and_reports_corrupt_compression() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let pdf = directory.path().join("proof with spaces.pdf");
        let path = pdf.with_extension("synctex.gz");
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(DATA.as_bytes()).unwrap();
        std::fs::write(&path, gzip.finish().unwrap()).unwrap();
        assert_eq!(sidecar(&pdf), Some(path.clone()));
        assert!(Mapping::load(&path, directory.path(), 1).is_ok());
        std::fs::write(&path, "broken gzip").unwrap();
        assert!(Mapping::load(&path, directory.path(), 1).is_err());
        assert!(Mapping::load(&directory.path().join("missing.synctex"), directory.path(), 1).is_err());
    }

    #[test]
    fn forward_prefers_content_nodes_over_enclosing_boxes() {
        let data = DATA.replace("x2,12:196608,262144", "[1,3:0,0:65536,65536,0\nx2,12:196608,262144");
        let mapping = Mapping::parse(&data, Path::new("/project"), 1).unwrap();
        let point = mapping
            .forward(&SourcePosition {
                path: "/project/main.tex".into(),
                line: 3,
                column: 0,
            })
            .unwrap();
        assert!(point.x > 0.0);
    }

    #[test]
    fn inverse_prefers_content_nodes_over_enclosing_boxes() {
        // Enclosing box and content node at exactly the same coordinates
        let data = DATA.replace("x1,3:65536,131072", "[1,3:0,0:65536,131072,0\nx1,4:65536,131072");
        let mapping = Mapping::parse(&data, Path::new("/project"), 1).unwrap();

        let pdf_position = PdfPosition {
            page: 1,
            x: 65536.0 * 1.0 * 1000.0 / 1000.0 / 65536.0 * 72.0 / 72.27,
            y: 131072.0 * 1.0 * 1000.0 / 1000.0 / 65536.0 * 72.0 / 72.27,
        };

        let source = mapping.inverse(pdf_position).unwrap();
        // Since both have the same distance, the one that is not a container (container=false) should be chosen.
        // The container is at line 3, the non-container is at line 4.
        assert_eq!(source.line, 4);
    }
}
