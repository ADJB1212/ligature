use std::path::Path;

use ligature_project::{IncludeGraph, Project, directory, eligible_search_path, include_paths, searchable};

#[test]
fn opening_requires_an_existing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file.tex");
    std::fs::write(&file, "").unwrap();
    assert!(Project::open(&file).unwrap_err().contains("not a project directory"));
    assert!(Project::open(&dir.path().join("missing")).unwrap_err().contains("Could not open project"));
    let project = Project::open(dir.path()).unwrap();
    assert_eq!(project.root, std::fs::canonicalize(dir.path()).unwrap());
    assert!(project.documents.is_empty() && project.root_document.is_none());
}

#[test]
fn relative_paths_are_normalized_and_confined_to_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::open(dir.path()).unwrap();
    let root = project.root.clone();
    assert_eq!(project.relative(Path::new("a/../b/./c.tex")), Some("b/c.tex".into()));
    assert_eq!(project.relative(&root.join("sub/../x.tex")), Some("x.tex".into()));
    assert_eq!(project.relative(&root), Some("".into()));
    assert_eq!(project.relative(Path::new("../x.tex")), None);
    assert_eq!(project.relative(Path::new("a/../../x.tex")), None);
    assert_eq!(project.relative(Path::new("/definitely/elsewhere.tex")), None);
    let sibling = root.with_file_name(format!("{}-sibling", root.file_name().unwrap().to_str().unwrap()));
    assert_eq!(project.relative(&sibling.join("x.tex")), None);
}

#[test]
fn documents_outside_the_project_are_not_added_and_duplicates_collapse() {
    let dir = tempfile::tempdir().unwrap();
    let mut project = Project::open(dir.path()).unwrap();
    project.add_document(Path::new("a.tex"));
    project.add_document(&project.root.join("./a.tex"));
    project.add_document(Path::new("../outside.tex"));
    assert_eq!(project.documents.iter().collect::<Vec<_>>(), [Path::new("a.tex")]);
}

#[test]
fn root_document_selection_accepts_uppercase_extensions_and_nested_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/MAIN.TeX"), "").unwrap();
    let mut project = Project::open(dir.path()).unwrap();
    project.set_root_document(Path::new("src/MAIN.TeX")).unwrap();
    assert_eq!(project.root_document, Some("src/MAIN.TeX".into()));
    let absolute = project.root.join("src/../src/MAIN.TeX");
    project.set_root_document(&absolute).unwrap();
    assert_eq!(project.root_document, Some("src/MAIN.TeX".into()));
}

#[test]
fn searchable_extensions_are_case_insensitive_and_exclude_binaries() {
    for name in [
        "a.tex", "a.TEX", "a.Bib", "a.sty", "a.cls", "a.bst", "a.ltx", "a.dtx", "a.ins", "a.txt", "a.cfg", "a.def", "a.tikz",
    ] {
        assert!(searchable(Path::new(name)), "{name}");
    }
    for name in ["a.pdf", "a.png", "a", "tex", ".tex", "a.tex.bak", "a.synctex.gz", "a.aux"] {
        assert!(!searchable(Path::new(name)), "{name}");
    }
}

#[test]
fn eligibility_excludes_generated_directories_the_output_directory_and_outside_paths() {
    let root = Path::new("/project");
    let output = Path::new("/project/artifacts");
    assert!(eligible_search_path(root, Path::new("/project/main.tex"), output));
    assert!(eligible_search_path(root, Path::new("/project/deep/er/main.tex"), output));
    for excluded in [
        "/project/artifacts/main.tex",
        "/project/artifacts/deep/main.tex",
        "/project/.git/main.tex",
        "/project/target/x.tex",
        "/project/build/x.tex",
        "/project/out/x.tex",
        "/project/.latexmk/x.tex",
        "/project/.ligature-cache/x.tex",
        "/project/sub/build/x.tex",
        "/project/main.pdf",
        "/elsewhere/main.tex",
    ] {
        assert!(!eligible_search_path(root, Path::new(excluded), output), "{excluded}");
    }
}

#[test]
fn directory_listings_hide_generated_entries_and_sort_folders_first() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    for folder in [
        "zeta",
        "alpha",
        ".git",
        "target",
        "build",
        "out",
        ".latexmk",
        ".ligature-cache",
        "artifacts",
    ] {
        std::fs::create_dir(root.join(folder)).unwrap();
    }
    for file in ["b.tex", "a.tex", ".ligature.json", ".ligature-tmp", "visible.pdf"] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let listing = directory(&root, &root.join("artifacts"));
    assert!(listing.error.is_none());
    let names: Vec<_> = listing
        .entries
        .iter()
        .map(|entry| entry.path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(names, ["alpha", "zeta", "a.tex", "b.tex", "visible.pdf"]);
    assert!(listing.entries[0].is_directory && !listing.entries[2].is_directory);
}

#[test]
fn include_paths_handle_empty_text_and_odd_whitespace() {
    assert!(include_paths("").is_empty());
    assert_eq!(include_paths("\\input{ a b }\\include{\tc\t}"), ["a b", "c"]);
    assert!(include_paths("\\input{a#1}\\input{#}").is_empty());
}

#[test]
fn include_graphs_add_the_default_extension_and_keep_the_requested_text() {
    let graph = IncludeGraph::load_with(Path::new("/p/main.tex"), |path| match path.to_str().unwrap() {
        "/p/main.tex" => Ok("\\input{a}\\input{b.sty}\\input{dir/c.v2}".into()),
        "/p/a.tex" | "/p/b.sty" => Ok(String::new()),
        _ => Err("missing".into()),
    });
    let includes = &graph.files[Path::new("/p/main.tex")];
    assert_eq!(
        includes.iter().map(|include| include.requested.as_str()).collect::<Vec<_>>(),
        ["a", "b.sty", "dir/c.v2"]
    );
    assert_eq!(includes.iter().map(|include| include.resolved).collect::<Vec<_>>(), [true, true, false]);
    assert_eq!(graph.errors.len(), 1);
}

#[test]
fn a_file_that_includes_itself_is_visited_once() {
    let graph = IncludeGraph::load_with(Path::new("/p/main.tex"), |_| Ok("\\input{main}".into()));
    assert_eq!(graph.files.len(), 1);
    assert!(graph.files[Path::new("/p/main.tex")][0].resolved);
}
