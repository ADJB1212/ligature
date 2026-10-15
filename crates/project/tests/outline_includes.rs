use std::path::Path;

use ligature_project::SemanticIndex;

#[test]
fn outline_preserves_include_order_and_skips_repeated_missing_and_cyclic_sources() {
    let index = SemanticIndex::load_with(Path::new("/virtual/main.tex"), |path| {
        match path.file_name().and_then(|name| name.to_str()) {
            Some("main.tex") => Ok("\\section{Before}\\input{child}\\input{missing}\\input{child}\\section{After}".into()),
            Some("child.tex") => Ok("\\subsection{Child}\\input{main}".into()),
            _ => Err("missing source".into()),
        }
    });
    let titles: Vec<_> = index.outline.iter().map(|entry| entry.title.as_str()).collect();
    assert_eq!(titles, ["Before", "Child", "After"]);
    assert_eq!(index.outline[1].parent, Some(0));
    assert_eq!(index.errors.len(), 1);
}
