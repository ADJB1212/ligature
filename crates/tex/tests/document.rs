use ligature_buffer::Buffer;
use ligature_syntax::{Language, Snapshot};
use ligature_tex::{Document, Key, parse};

fn document(text: &str) -> Document {
    let buffer = Buffer::from_text(text);
    parse(&Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, text.into()))
}

fn names(keys: &[Key]) -> Vec<&str> {
    keys.iter().map(|key| key.name.as_str()).collect()
}

#[test]
fn empty_and_plain_text_have_no_structure() {
    for text in ["", "plain words\nonly", "% \\section{x}", "\\"] {
        assert_eq!(document(text), Document::default(), "{text:?}");
    }
}

#[test]
fn every_sectioning_command_maps_to_its_level() {
    let text = "\\part{p}\\chapter{c}\\section{s}\\subsection{ss}\\subsubsection{sss}\\paragraph{pa}\\subparagraph{sp}";
    let document = document(text);
    assert_eq!(
        document.headings.iter().map(|heading| heading.level).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4, 5, 6]
    );
    assert_eq!(
        document.headings.iter().map(|heading| heading.parent).collect::<Vec<_>>(),
        [None, Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)]
    );
}

#[test]
fn heading_parents_pop_to_the_nearest_shallower_heading() {
    let document = document("\\section{A}\\subsection{B}\\subsubsection{C}\\subsection{D}\\section{E}\\subsection{F}\\chapter{G}\\section{H}");
    let parents: Vec<_> = document.headings.iter().map(|heading| heading.parent).collect();
    assert_eq!(parents, [None, Some(0), Some(1), Some(0), None, Some(4), None, Some(6)]);
}

#[test]
fn skipped_levels_still_attach_to_the_closest_ancestor() {
    let document = document("\\chapter{A}\\subsubsection{Deep}");
    assert_eq!(document.headings[1].parent, Some(0));
}

#[test]
fn heading_ranges_cover_the_command_through_the_closing_brace() {
    let text = "intro \\section*[short]{Long {title}} after";
    let document = document(text);
    assert_eq!(&text[document.headings[0].range.clone()], "\\section*[short]{Long {title}}");
    assert_eq!(&text[document.headings[0].title_range.clone()], "Long {title}");
}

#[test]
fn heading_titles_drop_comments_and_collapse_lines() {
    let document = document("\\section{Alpha % trailing } brace\n  Beta\n\n  Gamma 100\\% }\nafter");
    assert_eq!(document.headings[0].title, "Alpha Beta Gamma 100\\%");
}

#[test]
fn commands_with_longer_names_do_not_match_structural_commands() {
    let document = document("\\sectionmark{x}\\labelled{y}\\inputs{z}\\citeless{w}\\beginning{v}\\refresh{u}");
    assert_eq!(document, Document::default());
}

#[test]
fn reference_commands_are_all_recognized() {
    let text = "\\ref{a}\\pageref{b}\\eqref{c}\\autoref{d}\\cref{e}\\Cref{f}\\vref{g}\\Vref{h}\\nameref{i}";
    assert_eq!(names(&document(text).references), ["a", "b", "c", "d", "e", "f", "g", "h", "i"]);
}

#[test]
fn citation_variants_including_multicites_are_recognized() {
    let text = "\\cite{a}\\Citep{b}\\citet*{c}\\nocite{*}\\textcites{d}{e,f}\\autocite[p. 1]{g}\\footcite{h}\\smartcites{i}";
    assert_eq!(names(&document(text).citations), ["a", "b", "c", "*", "d", "e", "f", "g", "h", "i"]);
}

#[test]
fn multicite_collection_stops_at_non_argument_text() {
    let document = document("\\parencites{a} {b}text{not a key}\\cite{c}");
    assert_eq!(names(&document.citations), ["a", "b", "c"]);
}

#[test]
fn key_lists_trim_whitespace_skip_empties_and_keep_exact_ranges() {
    let text = "\\cite{ one ,, two ,\n three,}";
    let document = document(text);
    assert_eq!(names(&document.citations), ["one", "two", "three"]);
    for key in &document.citations {
        assert_eq!(&text[key.range.clone()], key.name);
    }
}

#[test]
fn keys_containing_macros_or_special_characters_are_ignored() {
    let document = document("\\label{\\foo}\\label{a#1}\\label{b{c}}\\cite{ok,\\bad,ok2}\\label{50%}\n\\label{fine}");
    assert!(names(&document.labels).contains(&"fine"));
    assert_eq!(names(&document.labels).len(), 1);
    assert_eq!(names(&document.citations), ["ok", "ok2"]);
}

#[test]
fn unicode_keys_have_byte_accurate_ranges() {
    let text = "日本 \\label{章:一} \\ref{章:一}";
    let document = document(text);
    assert_eq!(&text[document.labels[0].range.clone()], "章:一");
    assert_eq!(&text[document.references[0].range.clone()], "章:一");
}

#[test]
fn bibliography_declarations_accept_every_supported_command() {
    let text = "\\bibliography{a,b}\\addbibresource{c.bib}\\addglobalbib{d}\\addsectionbib{e}";
    assert_eq!(names(&document(text).bibliographies), ["a", "b", "c.bib", "d", "e"]);
}

#[test]
fn include_commands_accept_braced_and_bare_forms() {
    let document = document("\\input{a}\\include{b/c}\\subfile{d e}\n\\input bare.tex % tail\n\\input\tspaced\\relax");
    assert_eq!(names(&document.includes), ["a", "b/c", "d e", "bare.tex", "spaced"]);
}

#[test]
fn unterminated_declarations_are_not_recorded_but_unterminated_labels_are_recovered_to_end_of_line() {
    let document = document("\\input{open\n\\bibliography{open\n\\addbibresource{open\n\\label{recovered\nnext");
    assert!(document.includes.is_empty());
    assert!(document.bibliographies.is_empty());
    assert_eq!(names(&document.labels), ["recovered"]);
}

#[test]
fn many_unclosed_arguments_recover_independently_at_line_boundaries() {
    let text = "\\label{recovered\n".repeat(10_000);
    let document = document(&text);
    assert_eq!(document.labels.len(), 10_000);
    for label in document.labels {
        assert_eq!(label.name, "recovered");
        assert_eq!(&text[label.range], "recovered");
    }
}

#[test]
fn argument_boundaries_ignore_escaped_comment_and_verbatim_braces() {
    let text = "\\section{Outer \\ref{inner} \\{ % }\n\\verb|}| \\begin{verbatim}}\\end{verbatim} tail}";
    let document = document(text);
    assert_eq!(document.headings[0].range, 0..text.len());
    assert_eq!(names(&document.references), ["inner"]);
}

#[test]
fn mismatched_environment_closures_discard_inner_openings() {
    let document = document("\\begin{outer}\\begin{inner}\\begin{inner}\\end{missing}\\end{outer}\\end{inner}\\begin{inner}\\end{inner}");
    assert!(document.environments[0].end.is_some());
    assert!(document.environments[1].end.is_none());
    assert!(document.environments[2].end.is_none());
    assert_eq!(document.environments[3].parent, None);
    assert!(document.environments[3].end.is_some());
}

#[test]
fn nested_environments_record_parents_and_matching_end_ranges() {
    let text = "\\begin{document}\\begin{figure*}\\begin{center}x\\end{center}\\end{figure*}\\end{document}";
    let document = document(text);
    assert_eq!(names_of_environments(&document), ["document", "figure*", "center"]);
    assert_eq!(
        document.environments.iter().map(|environment| environment.parent).collect::<Vec<_>>(),
        [None, Some(0), Some(1)]
    );
    for environment in &document.environments {
        let end = environment.end.clone().unwrap();
        assert!(text[end].starts_with("\\end{"));
        assert!(environment.begin.end <= text.len());
    }
}

fn names_of_environments(document: &Document) -> Vec<&str> {
    document.environments.iter().map(|environment| environment.name.as_str()).collect()
}

#[test]
fn stray_end_commands_are_ignored_and_close_the_nearest_matching_begin() {
    let document = document("\\end{ghost}\\begin{a}\\begin{b}\\begin{a}\\end{a}\\end{a}");
    assert_eq!(names_of_environments(&document), ["a", "b", "a"]);
    assert!(document.environments[2].end.is_some());
    assert!(document.environments[0].end.is_some());
    assert!(document.environments[1].end.is_none());
}

#[test]
fn environment_names_are_trimmed() {
    let document = document("\\begin{ spaced }\\end{spaced}");
    assert_eq!(names_of_environments(&document), ["spaced"]);
    assert!(document.environments[0].end.is_some());
}

#[test]
fn verbatim_like_environments_hide_their_contents_but_are_recorded() {
    let text =
        "\\begin{lstlisting}\\section{x}\\label{y}\\end{lstlisting}\\begin{minted}\\cite{z}\\end{minted}\\begin{comment}\\ref{w}\\end{comment}";
    let document = document(text);
    assert_eq!(names_of_environments(&document), ["lstlisting", "minted", "comment"]);
    assert!(document.headings.is_empty());
    assert!(document.labels.is_empty());
    assert!(document.citations.is_empty());
    assert!(document.references.is_empty());
}

#[test]
fn starred_commands_and_optional_arguments_before_the_key_are_skipped() {
    let document = document("\\cref*{a}\\citep[see][p.~3]{b}\\section*[x]{Y}\\label {c}");
    assert_eq!(names(&document.references), ["a"]);
    assert_eq!(names(&document.citations), ["b"]);
    assert_eq!(document.headings[0].title, "Y");
    assert_eq!(names(&document.labels), ["c"]);
}

#[test]
fn optional_arguments_with_nested_brackets_and_comments_do_not_derail_parsing() {
    let document = document("\\citep[a [nested] % ] fake\n b]{key}");
    assert_eq!(names(&document.citations), ["key"]);
}

#[test]
fn unterminated_optional_arguments_yield_no_symbol_and_do_not_panic() {
    let document = document("\\cite[never closed {key}\n\\label{after}");
    assert!(document.citations.is_empty());
    assert_eq!(names(&document.labels), ["after"]);
}

#[test]
fn every_prefix_of_a_dense_document_yields_valid_ranges() {
    let text = "\\section*[é]{日本 % c\n x}\\label{l,m}\\begin{a}\\cite[p]{k}\\parencites[a]{b}[c]{d,e}\\end{a}\\input a\\verb|\\label{z}|\n";
    for end in text.char_indices().map(|(index, _)| index).chain([text.len()]) {
        let prefix = &text[..end];
        let document = document(prefix);
        let keys = [
            &document.labels,
            &document.references,
            &document.citations,
            &document.bibliographies,
            &document.includes,
        ];
        for key in keys.into_iter().flatten() {
            assert_eq!(prefix.get(key.range.clone()), Some(key.name.as_str()), "{end}");
        }
        for heading in &document.headings {
            assert!(prefix.get(heading.range.clone()).is_some());
            assert!(prefix.get(heading.title_range.clone()).is_some());
            assert!(heading.parent.is_none_or(|parent| parent < document.headings.len()));
        }
        for environment in &document.environments {
            assert!(prefix.get(environment.begin.clone()).is_some());
            assert!(environment.end.as_ref().is_none_or(|range| prefix.get(range.clone()).is_some()));
        }
    }
}
