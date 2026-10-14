use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use ligature_project::Project;

fn discover(project: &mut Project, active: Option<&Path>, text: &BTreeMap<PathBuf, String>) {
    project.discover_root_document(&project.root.join("artifacts"), active, |path| {
        text.get(path)
            .cloned()
            .map_or_else(|| std::fs::read_to_string(path).map_err(|error| error.to_string()), Ok)
    });
}

#[test]
fn discovery_prefers_main_then_shallow_paths_with_stable_ties() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("src")).unwrap();
    for path in ["z.tex", "a.tex", "src/MAIN.TeX"] {
        std::fs::write(directory.path().join(path), "\\documentclass{article}").unwrap();
    }
    let mut project = Project::open(directory.path()).unwrap();
    discover(&mut project, None, &BTreeMap::new());
    assert_eq!(project.root_document.as_deref(), Some(Path::new("src/MAIN.TeX")));
    std::fs::remove_file(project.root.join("src/MAIN.TeX")).unwrap();
    discover(&mut project, None, &BTreeMap::new());
    assert_eq!(project.root_document.as_deref(), Some(Path::new("a.tex")));
}

#[test]
fn active_standalone_text_overrides_configured_root() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("main.tex"), "\\documentclass{article}").unwrap();
    std::fs::write(directory.path().join("other.tex"), "fragment").unwrap();
    let mut project = Project::open(directory.path()).unwrap();
    project.set_root_document(Path::new("main.tex")).unwrap();
    let active = project.root.join("other.tex");
    let text = BTreeMap::from([(active.clone(), "\\documentclass[12pt]{book}".into())]);
    discover(&mut project, Some(&active), &text);
    assert_eq!(project.root_document.as_deref(), Some(Path::new("other.tex")));
}

#[test]
fn active_chapter_finds_its_main_through_unsaved_transitive_includes() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("chapters")).unwrap();
    std::fs::write(directory.path().join("main.tex"), "\\documentclass{article}").unwrap();
    std::fs::write(directory.path().join("thesis.tex"), "\\documentclass{book}").unwrap();
    std::fs::write(directory.path().join("chapters/one.tex"), "\\input{two}").unwrap();
    std::fs::write(directory.path().join("chapters/two.tex"), "chapter").unwrap();
    let mut project = Project::open(directory.path()).unwrap();
    let active = project.root.join("chapters/two.tex");
    let text = BTreeMap::from([(project.root.join("thesis.tex"), "\\documentclass{book}\\input{chapters/one}".into())]);
    discover(&mut project, Some(&active), &text);
    assert_eq!(project.root_document.as_deref(), Some(Path::new("thesis.tex")));
}

#[test]
fn comments_verbatim_and_escaped_commands_do_not_make_a_main_document() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("real.tex"), "\\documentclass{book}").unwrap();
    std::fs::write(
        directory.path().join("main.tex"),
        "% \\documentclass{article}\n\\verb|\\documentclass|\n\\\\documentclass",
    )
    .unwrap();
    let mut project = Project::open(directory.path()).unwrap();
    let active = project.root.join("main.tex");
    discover(&mut project, Some(&active), &BTreeMap::new());
    assert_eq!(project.root_document.as_deref(), Some(Path::new("real.tex")));
}

#[test]
fn existing_configured_root_is_a_fallback_for_fragments() {
    let directory = tempfile::tempdir().unwrap();
    for path in ["main.tex", "preferred.tex", "chapter.tex"] {
        std::fs::write(directory.path().join(path), "fragment").unwrap();
    }
    let mut project = Project::open(directory.path()).unwrap();
    project.set_root_document(Path::new("preferred.tex")).unwrap();
    let active = project.root.join("chapter.tex");
    discover(&mut project, Some(&active), &BTreeMap::new());
    assert_eq!(project.root_document.as_deref(), Some(Path::new("preferred.tex")));
}

#[test]
fn generated_sources_and_outside_symlinks_are_excluded() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("artifacts")).unwrap();
    std::fs::create_dir(directory.path().join("target")).unwrap();
    for path in ["artifacts/main.tex", "target/main.tex"] {
        std::fs::write(directory.path().join(path), "\\documentclass{article}").unwrap();
    }
    std::fs::write(outside.path().join("main.tex"), "\\documentclass{article}").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path().join("main.tex"), directory.path().join("alias.tex")).unwrap();
    let mut project = Project::open(directory.path()).unwrap();
    let active = project.root.join("artifacts/main.tex");
    discover(&mut project, Some(&active), &BTreeMap::new());
    assert!(project.root_document.is_none());
}

#[test]
fn a_single_tex_file_can_build_without_a_documentclass() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("plain.tex"), "Plain TeX\\bye").unwrap();
    let mut project = Project::open(directory.path()).unwrap();
    discover(&mut project, None, &BTreeMap::new());
    assert_eq!(project.root_document.as_deref(), Some(Path::new("plain.tex")));
}

#[test]
fn multiple_fragments_without_a_main_leave_the_root_unset() {
    let directory = tempfile::tempdir().unwrap();
    for path in ["one.tex", "two.tex"] {
        std::fs::write(directory.path().join(path), "fragment").unwrap();
    }
    let mut project = Project::open(directory.path()).unwrap();
    discover(&mut project, None, &BTreeMap::new());
    assert!(project.root_document.is_none());
}
