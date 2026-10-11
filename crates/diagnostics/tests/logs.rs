use std::path::{Path, PathBuf};

use ligature_diagnostics::{BuildId, Diagnostic, Provider, Severity, SourceRange, grouped, parse_tex};

fn parse(log: &str) -> Vec<Diagnostic> {
    parse_tex(log, Path::new("/project"), Path::new("main.tex"), BuildId(1))
}

fn lines(start: usize, end: usize) -> Option<SourceRange> {
    Some(SourceRange::Lines { start, end })
}

#[test]
fn empty_and_clean_logs_produce_no_diagnostics() {
    assert!(parse("").is_empty());
    assert!(parse("This is a clean log.\n(./main.tex\n)\nOutput written on main.pdf.\n").is_empty());
}

#[test]
fn errors_default_to_the_root_document_and_take_the_following_line_number() {
    let diagnostics = parse("! Undefined control sequence.\nl.17 \\oops\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(diagnostics[0].path, Some(PathBuf::from("/project/main.tex")));
    assert_eq!(diagnostics[0].range, lines(17, 17));
    assert_eq!(diagnostics[0].message, "Undefined control sequence.");
}

#[test]
fn root_document_paths_are_resolved_against_the_working_directory() {
    let diagnostics = parse_tex("! Oops.", Path::new("/project/sub"), Path::new("../root/./main.tex"), BuildId(0));
    assert_eq!(diagnostics[0].path, Some(PathBuf::from("/project/root/main.tex")));
}

#[test]
fn file_line_error_style_messages_carry_their_own_location() {
    let diagnostics = parse("./chapters/one.tex:12: Undefined control sequence.\n../shared/macros.sty:3: Missing $ inserted.");
    assert_eq!(diagnostics[0].path, Some(PathBuf::from("/project/chapters/one.tex")));
    assert_eq!(diagnostics[0].range, lines(12, 12));
    assert_eq!(diagnostics[0].message, "Undefined control sequence.");
    assert_eq!(diagnostics[1].path, Some(PathBuf::from("/shared/macros.sty")));
    assert_eq!(diagnostics[1].message, "Missing $ inserted.");
}

#[test]
fn engine_prefixed_errors_and_warnings_have_the_right_severity() {
    let diagnostics = parse("error: main.tex:4: Runaway argument.\nwarning: main.tex:9: Font shape unavailable.\nerror: resource missing\n");
    assert_eq!(
        diagnostics.iter().map(|diagnostic| diagnostic.severity).collect::<Vec<_>>(),
        [Severity::Error, Severity::Warning, Severity::Error]
    );
    assert_eq!(diagnostics[1].range, lines(9, 9));
    assert_eq!(diagnostics[2].path, None);
    assert_eq!(diagnostics[2].range, None);
    assert_eq!(diagnostics[2].message, "resource missing");
}

#[test]
fn line_zero_and_empty_file_names_are_not_locations() {
    for log in ["error: main.tex:0: nope", "error: :5: nope"] {
        let diagnostics = parse(log);
        assert_eq!(diagnostics[0].path, None, "{log}");
        assert_eq!(diagnostics[0].range, None, "{log}");
    }
}

#[test]
fn quoted_file_names_with_spaces_are_supported_in_context_and_locations() {
    let diagnostics = parse("(\"./my chapter.tex\"\n! Missing number.\nl.2 x\n)\nerror: \"my chapter.tex\":8: Bad.");
    assert_eq!(diagnostics[0].path, Some(PathBuf::from("/project/my chapter.tex")));
    assert_eq!(diagnostics[1].path, Some(PathBuf::from("/project/my chapter.tex")));
    assert_eq!(diagnostics[1].range, lines(8, 8));
}

#[test]
fn closing_parentheses_restore_the_enclosing_file() {
    let log = "(./a.tex\n(./b.tex\n)\n! In a.\nl.3 x\n)\n! In root.\nl.4 y\n";
    let diagnostics = parse(log);
    assert_eq!(diagnostics[0].path, Some(PathBuf::from("/project/a.tex")));
    assert_eq!(diagnostics[1].path, Some(PathBuf::from("/project/main.tex")));
}

#[test]
fn file_context_may_open_and_close_on_one_line() {
    let diagnostics = parse("(./a.tex (./b.tex) ! after b\n! Real.\nl.1 x\n");
    assert_eq!(diagnostics.last().unwrap().path, Some(PathBuf::from("/project/a.tex")));
}

#[test]
fn absolute_paths_are_not_rebased() {
    let diagnostics = parse("(/usr/share/texmf/article.cls\n! Inside class.\nl.1 x\n");
    assert_eq!(diagnostics[0].path, Some(PathBuf::from("/usr/share/texmf/article.cls")));
}

#[test]
fn separator_lines_reset_the_include_stack() {
    let diagnostics = parse("(./a.tex\n--- TeX said ---\n! After reset.\nl.1 x\n");
    assert_eq!(diagnostics[0].path, Some(PathBuf::from("/project/main.tex")));
}

#[test]
fn warnings_with_input_lines_and_line_spans_are_located() {
    let log = "LaTeX Warning: Reference `a' on page 1 undefined on input line 31.\nOverfull \\hbox (3.0pt too wide) in paragraph at lines 4--9\n";
    let diagnostics = parse(log);
    assert_eq!(diagnostics.len(), 2);
    assert!(diagnostics.iter().all(|diagnostic| diagnostic.severity == Severity::Warning));
    assert_eq!(diagnostics[0].range, lines(31, 31));
    assert_eq!(diagnostics[1].range, lines(4, 9));
}

#[test]
fn single_line_box_warnings_without_ranges_have_no_range() {
    let diagnostics = parse("Underfull \\hbox (badness 10000) has occurred while \\output is active\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].range, None);
}

#[test]
fn warnings_without_sentence_ends_are_flushed_by_blank_lines() {
    let diagnostics = parse("LaTeX Warning: Label(s) may have changed\n\nnext line\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].message, "LaTeX Warning: Label(s) may have changed");
}

#[test]
fn indented_continuation_lines_extend_errors_before_their_line_marker() {
    let diagnostics = parse("! Package foo Error: Bad option.\n(foo)                Try again.\n    context\nl.9 \\usepackage{foo}\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].range, lines(9, 9));
    assert!(diagnostics[0].message.contains("Bad option."));
    assert!(diagnostics[0].message.contains("Try again."));
}

#[test]
#[ignore = "known limitation: unindented help text between a LaTeX error and its l.N marker discards the line number"]
fn help_text_between_an_error_and_its_line_marker_keeps_the_line_number() {
    let diagnostics =
        parse("! LaTeX Error: Environment foo undefined.\n\nSee the LaTeX manual or LaTeX Companion for explanation.\nl.9 \\begin{foo}\n");
    assert_eq!(diagnostics[0].range, lines(9, 9));
}

#[test]
fn identical_diagnostics_are_deduplicated_but_different_lines_are_not() {
    let diagnostics = parse("! Oops.\nl.1 x\n! Oops.\nl.1 x\n! Oops.\nl.2 x\n");
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0].range, lines(1, 1));
    assert_eq!(diagnostics[1].range, lines(2, 2));
}

#[test]
fn every_diagnostic_records_the_build_that_produced_it() {
    let diagnostics = parse_tex("! A.\nl.1 x\nwarning: w", Path::new("/p"), Path::new("m.tex"), BuildId(77));
    assert_eq!(diagnostics.len(), 2);
    assert!(diagnostics.iter().all(|diagnostic| diagnostic.provider == Provider::Build(BuildId(77))));
}

#[test]
fn absurd_line_numbers_do_not_panic_or_create_ranges() {
    let diagnostics = parse("! Huge.\nl.99999999999999999999999999 x\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].range, None);
}

#[test]
fn line_markers_without_a_pending_error_are_ignored() {
    assert!(parse("l.5 stray\nl.\nl.x\n").is_empty());
}

#[test]
fn crlf_logs_parse_like_lf_logs() {
    let lf = "(./a.tex\n! Bad.\nl.2 x\n)\nwarning: a.tex:5: careful\n";
    assert_eq!(parse(&lf.replace('\n', "\r\n")), parse(lf));
}

#[test]
fn truncated_logs_never_panic() {
    let log = "(./a.tex\n(\"./b c.tex\"\n! Bad é 日本.\nl.2 x\n)\nLaTeX Warning: Ref on input line 3.\nOverfull \\hbox in paragraph at lines 4--5\n--- end ---\nerror: a.tex:7: end\n";
    for end in log.char_indices().map(|(index, _)| index).chain([log.len()]) {
        parse(&log[..end]);
    }
}

#[test]
fn diagnostics_group_by_path_then_severity_preserving_order() {
    let make = |severity, path: Option<&str>, message: &str| Diagnostic {
        severity,
        provider: Provider::LanguageServer("test".into()),
        message: message.into(),
        path: path.map(PathBuf::from),
        range: None,
    };
    let all = [
        make(Severity::Warning, Some("b.tex"), "w1"),
        make(Severity::Error, Some("b.tex"), "e1"),
        make(Severity::Error, None, "global"),
        make(Severity::Warning, Some("b.tex"), "w2"),
        make(Severity::Error, Some("a.tex"), "e2"),
        make(Severity::Information, Some("a.tex"), "i"),
    ];
    let groups = grouped(&all);
    let keys: Vec<_> = groups.keys().cloned().collect();
    assert_eq!(
        keys,
        [
            (None, Severity::Error),
            (Some(PathBuf::from("a.tex")), Severity::Error),
            (Some(PathBuf::from("a.tex")), Severity::Information),
            (Some(PathBuf::from("b.tex")), Severity::Error),
            (Some(PathBuf::from("b.tex")), Severity::Warning),
        ]
    );
    let warnings = &groups[&(Some(PathBuf::from("b.tex")), Severity::Warning)];
    assert_eq!(
        warnings.iter().map(|diagnostic| diagnostic.message.as_str()).collect::<Vec<_>>(),
        ["w1", "w2"]
    );
    assert!(grouped(&[]).is_empty());
}
