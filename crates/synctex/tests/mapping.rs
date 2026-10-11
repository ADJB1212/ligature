use std::{io::Write, path::Path};

use ligature_synctex::{Mapping, PdfPosition, SourcePosition, sidecar};

const POINT: f64 = 72.0 / 72.27;

fn document(header: &str, body: &str) -> String {
    format!("SyncTeX Version:1\nInput:1:main.tex\nInput:2:sub/other.tex\nOutput:pdf\n{header}Content:\n{body}Postamble:\nCount:1\n")
}

fn parse(body: &str) -> Result<Mapping, String> {
    Mapping::parse(
        &document("Magnification:1000\nUnit:1\nX Offset:0\nY Offset:0\n", body),
        Path::new("/project"),
        2,
    )
}

fn source(path: &str, line: usize, column: usize) -> SourcePosition {
    SourcePosition {
        path: path.into(),
        line,
        column,
    }
}

fn sp(value: f64) -> f64 {
    value * POINT
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn headers_outputs_and_postambles_are_required() {
    let valid = document("", "{1\nx1,3:65536,65536\n}1\n");
    assert!(Mapping::parse(&valid, Path::new("/p"), 1).is_ok());
    for broken in [
        valid.replace("Version:1", "Version:2"),
        valid.replace("Output:pdf", "Output:dvi"),
        valid.replace("Postamble:", "Pre:"),
        format!("\n{valid}"),
        String::new(),
    ] {
        assert!(Mapping::parse(&broken, Path::new("/p"), 1).is_err());
    }
}

#[test]
fn pages_must_exist_in_the_current_pdf() {
    for page in ["0", "3", "99999999999999999999", "x"] {
        let body = format!("{{{page}\nx1,3:1,1\n}}{page}\n");
        assert!(parse(&body).is_err(), "page {page}");
    }
    assert!(parse("{2\nx1,3:1,1\n}2\n").is_ok());
}

#[test]
fn documents_without_usable_records_are_rejected() {
    for body in ["", "{1\n}1\n", "{1\nx1,0:1,1\n}1\n", "x1,3:1,1\n", "{1\nQ1,3:1,1\n}1\n"] {
        assert!(parse(body).is_err(), "{body:?}");
    }
}

#[test]
fn malformed_records_are_rejected_instead_of_skipped() {
    for record in [
        "x1,3",
        "x1,3:100",
        "x1,3:a,b",
        "x1,3:1,inf",
        "x1,three:1,1",
        "xone,3:1,1",
        "x1,3,col:1,1",
        "x9,3:1,1",
        "x1",
    ] {
        assert!(parse(&format!("{{1\n{record}\n}}1\n")).is_err(), "{record}");
    }
}

#[test]
fn malformed_inputs_and_metrics_are_rejected() {
    let body = "{1\nx1,3:1,1\n}1\n";
    for header in [
        "Unit:0\n",
        "Unit:-1\n",
        "Unit:abc\n",
        "Magnification:0\n",
        "Magnification:inf\n",
        "X Offset:NaN\n",
        "Y Offset:\n",
    ] {
        assert!(Mapping::parse(&document(header, body), Path::new("/p"), 1).is_err(), "{header:?}");
    }
    for input in ["Input:main.tex", "Input:x:main.tex"] {
        let text = document("", body).replace("Input:1:main.tex", input);
        assert!(Mapping::parse(&text, Path::new("/p"), 1).is_err(), "{input}");
    }
}

#[test]
fn scale_unit_magnification_and_offsets_convert_to_pdf_points() {
    let body = "{1\nx1,3:65536,131072\n}1\n";
    let position = |header: &str| {
        let mapping = Mapping::parse(&document(header, body), Path::new("/project"), 1).unwrap();
        mapping.forward(&source("/project/main.tex", 3, 0)).unwrap()
    };
    let base = position("");
    assert!(close(base.x, sp(1.0)) && close(base.y, sp(2.0)));
    let magnified = position("Magnification:2000\n");
    assert!(close(magnified.x, sp(2.0)) && close(magnified.y, sp(4.0)));
    let unit = position("Unit:2\n");
    assert!(close(unit.x, sp(2.0)) && close(unit.y, sp(4.0)));
    let offset = position("X Offset:65536\nY Offset:-65536\n");
    assert!(close(offset.x, sp(2.0)) && close(offset.y, sp(1.0)));
    assert_eq!(base.page, 1);
}

#[test]
fn absolute_input_paths_ignore_the_source_directory_and_relative_ones_use_it() {
    let text = document("", "{1\nx1,3:1,1\nx2,4:2,2\n}1\n").replace("Input:1:main.tex", "Input:1:/elsewhere/abs.tex");
    let mapping = Mapping::parse(&text, Path::new("/project"), 1).unwrap();
    assert!(mapping.forward(&source("/elsewhere/abs.tex", 3, 0)).is_some());
    assert!(mapping.forward(&source("/project/abs.tex", 3, 0)).is_none());
    assert!(mapping.forward(&source("/project/sub/other.tex", 4, 0)).is_some());
}

#[test]
fn forward_picks_the_nearest_line_then_the_nearest_column() {
    let mapping = parse("{1\nx1,3:65536,65536\nx1,10:131072,131072\nx1,10,4:196608,196608\nx1,10,40:262144,262144\n}1\n").unwrap();
    let at = |line, column| mapping.forward(&source("/project/main.tex", line, column)).unwrap();
    assert!(close(at(1, 0).x, sp(1.0)));
    assert!(close(at(5, 0).x, sp(1.0)));
    assert!(close(at(7, 0).x, sp(2.0)));
    assert!(close(at(100, 0).x, sp(2.0)));
    assert!(close(at(10, 5).x, sp(3.0)));
    assert!(close(at(10, 30).x, sp(4.0)));
}

#[test]
fn forward_requires_a_known_source_file() {
    let mapping = parse("{1\nx1,3:1,1\n}1\n").unwrap();
    assert!(mapping.forward(&source("/project/sub/other.tex", 3, 0)).is_none());
    assert!(mapping.forward(&source("relative.tex", 3, 0)).is_none());
}

#[test]
fn records_outside_page_blocks_are_ignored() {
    let mapping = parse("x1,3:1,1\n{1\nx1,5:65536,65536\n}1\nx1,7:131072,131072\n").unwrap();
    assert!(close(mapping.forward(&source("/project/main.tex", 7, 0)).unwrap().x, sp(1.0)));
}

#[test]
fn all_record_kinds_contribute_mappings() {
    for kind in ["x", "g", "k", "h", "v", "[", "(", "r", "$"] {
        let mapping = parse(&format!("{{1\n{kind}1,3:65536,65536\n}}1\n")).unwrap();
        assert!(mapping.forward(&source("/project/main.tex", 3, 0)).is_some(), "{kind}");
    }
}

#[test]
fn inverse_returns_the_nearest_record_on_the_requested_page_only() {
    let mapping = parse("{1\nx1,3:65536,65536\nx1,9:655360,655360\n}1\n{2\nx2,20:65536,65536\n}2\n").unwrap();
    let near_second = mapping
        .inverse(PdfPosition {
            page: 1,
            x: sp(9.5),
            y: sp(9.5),
        })
        .unwrap();
    assert_eq!(near_second.line, 9);
    let page_two = mapping
        .inverse(PdfPosition {
            page: 2,
            x: sp(9.5),
            y: sp(9.5),
        })
        .unwrap();
    assert_eq!((page_two.line, page_two.path.ends_with("sub/other.tex")), (20, true));
    assert!(mapping.inverse(PdfPosition { page: 0, x: 0.0, y: 0.0 }).is_none());
    assert!(mapping.inverse(PdfPosition { page: 3, x: 0.0, y: 0.0 }).is_none());
}

#[test]
fn inverse_rejects_non_finite_coordinates() {
    let mapping = parse("{1\nx1,3:1,1\n}1\n").unwrap();
    for (x, y) in [(f64::NAN, 0.0), (0.0, f64::NAN), (f64::INFINITY, 0.0), (0.0, f64::NEG_INFINITY)] {
        assert!(mapping.inverse(PdfPosition { page: 1, x, y }).is_none());
    }
}

#[test]
fn inverse_reports_columns_when_present_and_zero_otherwise() {
    let mapping = parse("{1\nx1,3,7:65536,65536\nx1,4:655360,655360\n}1\n").unwrap();
    let with_column = mapping
        .inverse(PdfPosition {
            page: 1,
            x: sp(1.0),
            y: sp(1.0),
        })
        .unwrap();
    let without = mapping
        .inverse(PdfPosition {
            page: 1,
            x: sp(10.0),
            y: sp(10.0),
        })
        .unwrap();
    assert_eq!((with_column.line, with_column.column), (3, 7));
    assert_eq!((without.line, without.column), (4, 0));
}

#[test]
fn sidecars_prefer_compressed_files_and_keep_dotted_stems() {
    let directory = tempfile::tempdir().unwrap();
    let pdf = directory.path().join("my.paper v2.pdf");
    assert_eq!(sidecar(&pdf), None);
    let plain = directory.path().join("my.paper v2.synctex");
    std::fs::write(&plain, "").unwrap();
    assert_eq!(sidecar(&pdf), Some(plain));
    let compressed = directory.path().join("my.paper v2.synctex.gz");
    std::fs::write(&compressed, "").unwrap();
    assert_eq!(sidecar(&pdf), Some(compressed));
}

#[test]
fn sidecar_directories_do_not_count_as_files() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("a.synctex.gz")).unwrap();
    assert_eq!(sidecar(&directory.path().join("a.pdf")), None);
}

#[test]
fn plain_and_gzip_files_load_and_resolve_relative_inputs_against_the_source_directory() {
    let directory = tempfile::tempdir().unwrap();
    let text = document("", "{1\nx2,12:65536,65536\n}1\n");
    let plain = directory.path().join("a.synctex");
    std::fs::write(&plain, &text).unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(text.as_bytes()).unwrap();
    let compressed = directory.path().join("b.synctex.gz");
    std::fs::write(&compressed, encoder.finish().unwrap()).unwrap();
    let target = directory.path().join("sub/other.tex");
    for path in [plain, compressed] {
        let mapping = Mapping::load(&path, directory.path(), 1).unwrap();
        assert!(mapping.forward(&source(target.to_str().unwrap(), 12, 0)).is_some());
    }
}

#[test]
fn invalid_utf8_and_missing_files_are_reported() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bad.synctex");
    std::fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
    assert!(Mapping::load(&path, directory.path(), 1).unwrap_err().contains("decode"));
    assert!(
        Mapping::load(&directory.path().join("none.synctex"), directory.path(), 1)
            .unwrap_err()
            .contains("Cannot read")
    );
}

#[test]
fn decompressed_size_is_limited_to_64_mib() {
    let directory = tempfile::tempdir().unwrap();
    let valid = document("", "{1\nx1,3:1,1\n}1\n");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(valid.as_bytes()).unwrap();
    let chunk = vec![b' '; 1024 * 1024];
    for _ in 0..64 {
        encoder.write_all(&chunk).unwrap();
    }
    let path = directory.path().join("huge.synctex.gz");
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    assert!(Mapping::load(&path, directory.path(), 1).unwrap_err().contains("64 MiB"));
}
