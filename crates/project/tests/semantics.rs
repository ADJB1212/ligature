use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use ligature_project::SemanticIndex;

struct Files(BTreeMap<PathBuf, String>);

impl Files {
    fn new(files: &[(&str, &str)]) -> Self {
        Self(files.iter().map(|(path, text)| (PathBuf::from(path), (*text).to_owned())).collect())
    }

    fn index(&self, root: &str) -> SemanticIndex {
        SemanticIndex::load_with(Path::new(root), |path| self.0.get(path).cloned().ok_or_else(|| "missing".into()))
    }

    fn refresh(&self, root: &str, previous: &SemanticIndex) -> SemanticIndex {
        SemanticIndex::refresh_with(Path::new(root), previous, |path| {
            self.0.get(path).cloned().ok_or_else(|| "missing".into())
        })
    }
}

fn titles(index: &SemanticIndex) -> Vec<&str> {
    index.outline.iter().map(|entry| entry.title.as_str()).collect()
}

#[test]
fn an_unreadable_root_yields_an_empty_index_with_one_error() {
    let index = Files::new(&[]).index("/project/main.tex");
    assert!(index.sources.is_empty());
    assert!(index.outline.is_empty());
    assert_eq!(index.errors.keys().collect::<Vec<_>>(), [Path::new("/project/main.tex")]);
}

#[test]
fn missing_includes_are_recorded_without_stopping_the_walk() {
    let files = Files::new(&[
        ("/project/main.tex", "\\input{gone}\\input{here}\\label{a}"),
        ("/project/here.tex", "\\label{b}"),
    ]);
    let index = files.index("/project/main.tex");
    assert_eq!(index.sources.len(), 2);
    assert_eq!(index.errors.len(), 1);
    assert!(index.errors.contains_key(Path::new("/project/gone.tex")));
    assert_eq!(index.definitions("b").len(), 1);
}

#[test]
fn explicit_extensions_are_not_extended_and_missing_ones_default_by_kind() {
    let files = Files::new(&[
        (
            "/project/main.tex",
            "\\input{a.tex}\\bibliography{refs}\\addbibresource{extra.bib}\\cite{r,e}",
        ),
        ("/project/a.tex", ""),
        ("/project/refs.bib", "@book{r}"),
        ("/project/extra.bib", "@book{e}"),
    ]);
    let index = files.index("/project/main.tex");
    assert!(index.errors.is_empty(), "{:?}", index.errors);
    assert!(index.unresolved_citations.is_empty());
}

#[test]
fn includes_in_subdirectories_resolve_relative_to_the_including_file() {
    let files = Files::new(&[
        ("/project/main.tex", "\\input{chapters/one}"),
        ("/project/chapters/one.tex", "\\input{../shared/macros}\\input{two}\\label{one}"),
        ("/project/chapters/two.tex", "\\label{two}"),
        ("/project/shared/macros.tex", "\\label{macros}"),
    ]);
    let index = files.index("/project/main.tex");
    assert!(index.errors.is_empty(), "{:?}", index.errors);
    for label in ["one", "two", "macros"] {
        assert_eq!(index.definitions(label).len(), 1, "{label}");
    }
}

#[test]
fn references_to_labels_in_files_that_are_not_included_are_unresolved() {
    let files = Files::new(&[
        ("/project/main.tex", "\\ref{outside}\\ref{inside}\\label{inside}"),
        ("/project/orphan.tex", "\\label{outside}"),
    ]);
    let index = files.index("/project/main.tex");
    assert_eq!(index.unresolved_references.len(), 1);
    assert_eq!(index.unresolved_references[0].0, "outside");
}

#[test]
fn unresolved_symbol_locations_slice_to_the_offending_key() {
    let text = "intro \\ref{missing} \\cite{ghost}";
    let files = Files::new(&[("/project/main.tex", text)]);
    let index = files.index("/project/main.tex");
    let (name, location) = &index.unresolved_references[0];
    assert_eq!(&text[location.range.clone()], name);
    let (name, location) = &index.unresolved_citations[0];
    assert_eq!(&text[location.range.clone()], name);
    assert_eq!(location.path, Path::new("/project/main.tex"));
}

#[test]
fn nocite_star_is_never_unresolved_but_other_keys_are_case_sensitive() {
    let files = Files::new(&[
        ("/project/main.tex", "\\nocite{*}\\cite{Book,book}\\bibliography{refs}"),
        ("/project/refs.bib", "@book{book}"),
    ]);
    let index = files.index("/project/main.tex");
    assert_eq!(
        index.unresolved_citations.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        ["Book"]
    );
}

#[test]
fn missing_bibliography_files_are_errors_and_leave_citations_unresolved() {
    let files = Files::new(&[("/project/main.tex", "\\cite{a}\\bibliography{refs}")]);
    let index = files.index("/project/main.tex");
    assert!(index.errors.contains_key(Path::new("/project/refs.bib")));
    assert_eq!(index.unresolved_citations.len(), 1);
}

#[test]
fn bibliography_files_declared_twice_are_indexed_once() {
    let files = Files::new(&[
        ("/project/main.tex", "\\bibliography{refs}\\input{child}"),
        ("/project/child.tex", "\\addbibresource{refs.bib}"),
        ("/project/refs.bib", "@book{k}"),
    ]);
    let index = files.index("/project/main.tex");
    assert_eq!(index.bibliography.resolve("k").len(), 1);
    assert_eq!(index.duplicate_labels().count(), 0);
}

#[test]
fn duplicate_labels_report_every_definition_across_files() {
    let files = Files::new(&[("/project/main.tex", "\\label{x}\\input{b}"), ("/project/b.tex", "\\label{x}\\label{y}")]);
    let index = files.index("/project/main.tex");
    let duplicates: Vec<_> = index.duplicate_labels().collect();
    assert_eq!(duplicates.len(), 1);
    assert_eq!(duplicates[0].0, "x");
    assert_eq!(duplicates[0].1.len(), 2);
}

#[test]
fn definition_at_resolves_references_and_citations_and_rejects_everything_else() {
    let text = "\\label{target}\\ref{target} \\cite{entry}\\bibliography{refs}";
    let files = Files::new(&[("/project/main.tex", text), ("/project/refs.bib", "@book{entry, title={T}}")]);
    let index = files.index("/project/main.tex");
    let main = Path::new("/project/main.tex");
    let reference = text.find("ref{target}").unwrap() + 4;
    let found = index.definition_at(main, reference);
    assert_eq!(found.len(), 1);
    assert_eq!(&text[found[0].range.clone()], "target");
    let citation = text.find("entry").unwrap();
    let found = index.definition_at(main, citation);
    assert_eq!(found[0].path, Path::new("/project/refs.bib"));
    assert_eq!(found[0].range, 6..11);
    assert!(index.definition_at(main, citation + "entry".len()).is_empty());
    assert!(index.definition_at(main, citation - 1).is_empty());
    assert!(index.definition_at(main, 0).is_empty());
    assert!(index.definition_at(Path::new("/project/other.tex"), reference).is_empty());
    assert!(index.definition_at(main, usize::MAX).is_empty());
}

#[test]
fn definition_at_on_an_unresolved_key_is_empty() {
    let text = "\\ref{nowhere}";
    let index = Files::new(&[("/project/main.tex", text)]).index("/project/main.tex");
    assert!(index.definition_at(Path::new("/project/main.tex"), 6).is_empty());
}

#[test]
fn outline_interleaves_included_headings_and_resets_parents_across_files() {
    let files = Files::new(&[
        ("/project/main.tex", "\\section{A}\\input{c}\\subsection{D}"),
        ("/project/c.tex", "\\subsection{B}\\section{C}"),
    ]);
    let index = files.index("/project/main.tex");
    assert_eq!(titles(&index), ["A", "B", "C", "D"]);
    let parents: Vec<_> = index.outline.iter().map(|entry| entry.parent).collect();
    assert_eq!(parents, [None, Some(0), None, Some(2)]);
    assert_eq!(index.outline[1].location.path, Path::new("/project/c.tex"));
    assert_eq!(index.outline[3].location.path, Path::new("/project/main.tex"));
}

#[test]
fn files_included_twice_or_cyclically_contribute_their_headings_once() {
    let files = Files::new(&[
        ("/project/main.tex", "\\input{c}\\input{c}\\section{Main}"),
        ("/project/c.tex", "\\section{Child}\\input{main}"),
    ]);
    let index = files.index("/project/main.tex");
    assert_eq!(titles(&index), ["Child", "Main"]);
}

#[test]
fn outline_locations_slice_to_the_heading_command() {
    let text = "pre \\section*{Title} post";
    let index = Files::new(&[("/project/main.tex", text)]).index("/project/main.tex");
    assert_eq!(&text[index.outline[0].location.range.clone()], "\\section*{Title}");
}

#[test]
fn refresh_reuses_unchanged_sources_and_reparses_changed_ones() {
    let mut files = Files::new(&[("/project/main.tex", "\\input{c}\\label{m}"), ("/project/c.tex", "\\label{c}")]);
    let first = files.index("/project/main.tex");
    files.0.insert("/project/c.tex".into(), "\\label{c2}".into());
    let second = files.refresh("/project/main.tex", &first);
    assert!(std::sync::Arc::ptr_eq(
        &first.sources[Path::new("/project/main.tex")].text,
        &second.sources[Path::new("/project/main.tex")].text
    ));
    assert!(!std::sync::Arc::ptr_eq(
        &first.sources[Path::new("/project/c.tex")].text,
        &second.sources[Path::new("/project/c.tex")].text
    ));
    assert!(second.definitions("c").is_empty());
    assert_eq!(second.definitions("c2").len(), 1);
}

#[test]
fn refresh_after_a_bibliography_edit_replaces_its_entries() {
    let mut files = Files::new(&[
        ("/project/main.tex", "\\cite{old}\\bibliography{refs}"),
        ("/project/refs.bib", "@book{old}"),
    ]);
    let first = files.index("/project/main.tex");
    assert!(first.unresolved_citations.is_empty());
    files.0.insert("/project/refs.bib".into(), "@book{new}".into());
    let second = files.refresh("/project/main.tex", &first);
    assert_eq!(second.unresolved_citations.len(), 1);
    assert_eq!(second.bibliography.resolve("new").len(), 1);
    assert!(second.bibliography.resolve("old").is_empty());
}

#[test]
fn refresh_preserves_unchanged_bibliography_entries_from_multiple_files() {
    let files = Files::new(&[
        ("/project/main.tex", "\\bibliography{a,b}"),
        ("/project/a.bib", "@book{shared}@book{only_a}"),
        ("/project/b.bib", "@book{shared}"),
    ]);
    let first = files.index("/project/main.tex");
    let second = files.refresh("/project/main.tex", &first);
    assert_eq!(second.bibliography, first.bibliography);
    assert_eq!(second.bibliography.resolve("shared").len(), 2);
}

#[test]
fn refresh_drops_files_that_are_no_longer_included_and_records_new_errors() {
    let mut files = Files::new(&[("/project/main.tex", "\\input{a}"), ("/project/a.tex", "\\label{a}")]);
    let first = files.index("/project/main.tex");
    files.0.insert("/project/main.tex".into(), "\\input{b}".into());
    let second = files.refresh("/project/main.tex", &first);
    assert!(!second.sources.contains_key(Path::new("/project/a.tex")));
    assert!(second.definitions("a").is_empty());
    assert!(second.errors.contains_key(Path::new("/project/b.tex")));
}
