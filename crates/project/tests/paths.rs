use std::path::Path;

use ligature_project::{Project, include_paths, normalize, search_files};

#[test]
fn normalization_preserves_relative_parents_and_stops_at_absolute_root() {
    for (input, expected) in [
        ("../../chapter/../main.tex", "../../main.tex"),
        ("./chapters/./one/../../main.tex", "main.tex"),
        ("/../../project/../main.tex", "/main.tex"),
        ("a/../../main.tex", "../main.tex"),
    ] {
        assert_eq!(normalize(Path::new(input)), Path::new(expected));
    }
}

#[test]
fn invalid_root_documents_preserve_the_previous_selection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.TEX"), "").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("folder.tex")).unwrap();
    let mut project = Project::open(dir.path()).unwrap();
    project.set_root_document(Path::new("main.TEX")).unwrap();
    for invalid in ["notes.txt", "folder.tex", "missing.tex", "../outside.tex"] {
        assert!(project.set_root_document(Path::new(invalid)).is_err());
        assert_eq!(project.root_document.as_deref(), Some(Path::new("main.TEX")));
    }
}

#[cfg(unix)]
#[test]
fn root_document_symlinks_cannot_escape_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("main.tex");
    std::fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("alias.tex")).unwrap();
    let mut project = Project::open(dir.path()).unwrap();
    assert!(project.set_root_document(Path::new("alias.tex")).is_err());
    assert_eq!(project.root_document, None);
}

#[test]
fn source_enumeration_excludes_generated_trees_and_respects_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    for folder in ["chapters", "custom-output", ".git", "target", ".ligature-cache"] {
        std::fs::create_dir(root.join(folder)).unwrap();
        std::fs::write(root.join(folder).join("source.tex"), "").unwrap();
    }
    std::fs::write(root.join("references.BIB"), "").unwrap();
    std::fs::write(root.join("preview.pdf"), "").unwrap();
    let output = root.join("custom-output");
    let files = search_files(&root, &output, || false);
    assert!(files.errors.is_empty());
    assert_eq!(files.paths, [root.join("chapters/source.tex"), root.join("references.BIB")].into());
    let canceled = search_files(&root, &output, || true);
    assert!(canceled.paths.is_empty());
    assert!(canceled.errors.is_empty());
}

#[test]
fn include_parser_accepts_literal_paths_and_ignores_macros_and_comments() {
    let text = concat!(
        "\\input chapter.tex % \\include{ignored}\n",
        "\\subfile{chapters/猫} \\include{ chapter two }\n",
        "\\input{\\chapter} \\input{#1} \\input{}\n",
        "\\inputting{ignored} \\input{unterminated\n",
        "\\% \\input{after-escaped-percent}\n",
    );
    assert_eq!(
        include_paths(text),
        vec!["chapter.tex", "chapters/猫", "chapter two", "after-escaped-percent"]
    );
}
