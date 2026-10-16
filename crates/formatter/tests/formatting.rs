use ligature_formatter::{Indentation, Options, format, format_with_options};

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn check(source: &str, expected: &str, options: &Options) {
    let formatted = format_with_options(source, options);
    assert_eq!(formatted, expected);
    assert_eq!(format_with_options(&formatted, options), formatted, "formatting must be idempotent");
}

#[test]
fn indents_nested_environments_and_list_continuations() {
    check(
        "\\begin{document}\n\\begin{itemize}\n\\item One\ncontinued\n\\begin{enumerate}\n\\item Two\ncontinued\n\\end{enumerate}\n\\item Three\n\\end{itemize}\n\\end{document}",
        "\\begin{document}\n\\begin{itemize}\n  \\item One\n    continued\n    \\begin{enumerate}\n      \\item Two\n        continued\n    \\end{enumerate}\n  \\item Three\n\\end{itemize}\n\\end{document}\n",
        &Options::default(),
    );
}

#[test]
fn splits_structural_commands_without_splitting_arguments_or_comments() {
    check(
        "intro \\section{A \\section{B}}\n\\begin{quote}text \\end{quote}\n% text \\section{Hidden}\n\\newcommand{\\foo}{\\begin{quote}hi\\end{quote}}",
        "intro\n\\section{A \\section{B}}\n\\begin{quote}text\n\\end{quote}\n% text \\section{Hidden}\n\\newcommand{\\foo}{\\begin{quote}hi\\end{quote}}\n",
        &Options::default(),
    );
}

#[test]
fn indents_groups_and_ignores_escaped_delimiters_and_comments() {
    check(
        "\\foo{\ntext \\{ \\} \\% % } ] \\end{quote}\n{\nnested\n}\n}\n\\begin{quote}\n\\\\% \\begin{ignored}\ntext\n\\end{quote}",
        "\\foo{\n  text \\{ \\} \\% % } ] \\end{quote}\n  {\n    nested\n  }\n}\n\\begin{quote}\n  \\\\% \\begin{ignored}\n  text\n\\end{quote}\n",
        &Options::default(),
    );
}

#[test]
fn preserves_verbatim_bytes_and_does_not_read_directives_inside_them() {
    let source = "\\begin{quote}\n  \\begin{verbatim}\n\t { text   \n\n\n% fmt: off\n\\end{another}\n  \\end{verbatim}\ntext\n\\end{quote}";
    let expected = "\\begin{quote}\n  \\begin{verbatim}\n\t { text   \n\n\n% fmt: off\n\\end{another}\n  \\end{verbatim}\n  text\n\\end{quote}\n";
    check(source, expected, &Options::default());
    check(
        "\\begin{minted}{rust}\n\tfn main() {  ",
        "\\begin{minted}{rust}\n\tfn main() {  ",
        &Options::default(),
    );
}

#[test]
fn preserves_inline_verbatim_and_disabled_lines() {
    let source = "\\begin{quote}\n   \\verb!% { \\end{quote}!  \n% fmt: off\n\t\\end{quote}  \n\n\n% fmt: on\ntext % fmt: skip\ntext\n\\end{quote}\n";
    let expected =
        "\\begin{quote}\n   \\verb!% { \\end{quote}!  \n% fmt: off\n\t\\end{quote}  \n\n\n% fmt: on\ntext % fmt: skip\n  text\n\\end{quote}\n";
    check(source, expected, &Options::default());
    check("\\verb|x| % fmt: off\ntext", "\\verb|x| % fmt: off\ntext\n", &Options::default());
}

#[test]
fn wraps_unicode_text_and_repeats_comment_prefixes() {
    let options = Options {
        line_width: Some(15),
        ..Options::default()
    };
    check(
        "日本語 words more words here\n% alpha beta gamma delta epsilon",
        "日本語 words more\nwords here\n% alpha beta\n% gamma delta\n% epsilon\n",
        &options,
    );
    check("text % alpha beta gamma delta", "text % alpha\n% beta gamma\n% delta\n", &options);
}

#[test]
fn wrapping_preserves_control_spaces_math_and_arguments() {
    let options = Options {
        line_width: Some(10),
        ..Options::default()
    };
    check("one\\ two three four", "one\\ two\nthree four\n", &options);
    check("\\foo{one two three four}", "\\foo{one two three four}\n", &options);
    check("$one two three$ four five", "$one two three$\nfour five\n", &options);
    check("\\(one two three\\) four five", "\\(one two three\\)\nfour five\n", &options);
    check("foo\\   \nbar", "foo\\ \nbar\n", &options);
}

#[test]
fn normalizes_blank_lines_and_preserves_crlf() {
    check("", "", &Options::default());
    check(" \n\n", "", &Options::default());
    check("\n text \n\n\n\n next\t \n\n", "text\n\nnext\n", &Options::default());
    check(
        "\\begin{quote}\r\n text\r\n\\end{quote}\r\n\r\n",
        "\\begin{quote}\r\n  text\r\n\\end{quote}\r\n",
        &Options::default(),
    );
}

#[test]
fn supports_custom_environments_tabs_and_disabled_wrapping() {
    let options = Options {
        indentation: Indentation::Tabs,
        line_width: None,
        list_environments: names(&["mylist"]),
        verbatim_environments: names(&["raw"]),
        no_indent_environments: names(&["quote"]),
        ..Options::default()
    };
    check(
        "\\begin{quote}\n\\begin{mylist}\n\\item Text\ncontinued\n\\end{mylist}\n\\end{quote}",
        "\\begin{quote}\n\\begin{mylist}\n\t\\item Text\n\t\tcontinued\n\\end{mylist}\n\\end{quote}\n",
        &options,
    );
    check(
        "\\begin{raw}\n   literal   \n\\end{raw}",
        "\\begin{raw}\n   literal   \n\\end{raw}",
        &options,
    );
}

#[test]
fn malformed_input_and_deep_nesting_do_not_panic() {
    let options = Options {
        line_width: Some(0),
        ..Options::default()
    };
    for source in [
        "} ] ) \\end{missing}\ntext",
        "\\begin{\nfoo",
        "\\",
        "\\verb",
        "\\mintinline",
        "%",
        "words words",
        "é日",
    ] {
        let formatted = format_with_options(source, &options);
        assert_eq!(format_with_options(&formatted, &options), formatted);
    }
    let source = format!("{}\ntext\n{}", "{".repeat(150), "}".repeat(150));
    let formatted = format(&source);
    assert!(formatted.contains(&format!("\n{}text\n", " ".repeat(300))));
    assert_eq!(format(&formatted), formatted);
}

#[test]
fn aligns_table_cells_without_changing_escaped_or_grouped_ampersands() {
    let options = Options {
        format_tables: true,
        ..Options::default()
    };
    check(
        "\\begin{tabular}{ll}\n日 & b \\\\\nlong & c \\\\\nx\\&y & {a&b} \\\\\n% no & change\n\\end{tabular}",
        "\\begin{tabular}{ll}\n  日    & b \\\\\n  long & c \\\\\n  x\\&y & {a&b} \\\\\n  % no & change\n\\end{tabular}\n",
        &options,
    );
}

#[test]
fn table_alignment_respects_disabled_regions() {
    let options = Options {
        format_tables: true,
        ..Options::default()
    };
    let source = "% fmt: off\n\\begin{tabular}{ll}\nx & y \\\\\nlong & z \\\\\n\\end{tabular}\n% fmt: on\n";
    check(source, source, &options);
}

#[test]
fn table_alignment_preserves_empty_cells_and_unicode_widths() {
    let options = Options {
        format_tables: true,
        ..Options::default()
    };
    check(
        "\\begin{tabular}{lll}\n& & 猫\nlong & é &\n\\end{tabular}",
        "\\begin{tabular}{lll}\n       &   & 猫\n  long & é & \n\\end{tabular}\n",
        &options,
    );
}

#[test]
fn same_line_verbatim_closing_does_not_hide_following_text() {
    check(
        "\\begin{verbatim} raw  \\end{verbatim}\n\\begin{quote}\ntext\n\\end{quote}",
        "\\begin{verbatim} raw  \\end{verbatim}\n\\begin{quote}\n  text\n\\end{quote}\n",
        &Options::default(),
    );
}

#[test]
fn table_alignment_leaves_multiline_groups_and_macro_bodies_alone() {
    let options = Options {
        format_tables: true,
        ..Options::default()
    };
    check(
        "\\begin{tabular}{ll}\n\\foo{\nx & y\n}\nlong & z\n\\end{tabular}",
        "\\begin{tabular}{ll}\n  \\foo{\n    x & y\n  }\n  long & z\n\\end{tabular}\n",
        &options,
    );
    check(
        "\\newcommand{\\foo}{\\begin{tabular}{ll}x&y\\end{tabular}}",
        "\\newcommand{\\foo}{\\begin{tabular}{ll}x&y\\end{tabular}}\n",
        &options,
    );
}

#[test]
fn formatting_is_stable_across_small_widths_and_mixed_constructs() {
    let lines = [
        "alpha beta gamma delta",
        "日本語 words é more",
        "text % more words here",
        "% words here and here",
        "\\foo{two words} after",
        "\\begin{quote}",
        "\\end{quote}",
        "{",
        "}",
        "\\item foo bar",
        "one\\ two three",
        "\\verb!raw { text!",
        "\\begin{verbatim}",
        "\\end{verbatim}",
    ];
    for width in [0, 1, 5, 15, 80] {
        let options = Options {
            line_width: Some(width),
            ..Options::default()
        };
        for first in lines {
            for second in lines {
                let source = format!("{first}\n{second}\n");
                let formatted = format_with_options(&source, &options);
                assert_eq!(format_with_options(&formatted, &options), formatted, "width {width}, source {source:?}");
            }
        }
    }
}
