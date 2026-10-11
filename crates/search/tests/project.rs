use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use ligature_buffer::Buffer;
use ligature_fs::FileState;
use ligature_search::{
    BufferVersion, Cancellation, FileReplacement, Options, ProjectResults, Query, SourceSnapshot, apply_disk_replacement, search_project,
};

fn query(text: &str) -> Query {
    Query {
        text: text.into(),
        options: Options::default(),
    }
}

fn root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    (dir, root)
}

fn run(root: &Path, open: &BTreeMap<PathBuf, SourceSnapshot>, query: Query) -> ProjectResults {
    search_project(root, &root.join("build"), open, query, &Cancellation::default()).unwrap()
}

fn open_buffer(text: &str, disk: &str) -> SourceSnapshot {
    let buffer = Buffer::from_text(text);
    SourceSnapshot {
        text: text.into(),
        buffer: Some(BufferVersion {
            document: buffer.id(),
            revision: buffer.revision(),
        }),
        disk: FileState::Present(disk.as_bytes().to_vec()),
    }
}

fn paths(results: &ProjectResults) -> Vec<&Path> {
    results.files.iter().map(|file| file.path.as_path()).collect()
}

#[test]
fn empty_queries_find_nothing_and_are_not_canceled() {
    let (_dir, root) = root();
    std::fs::write(root.join("a.tex"), "text").unwrap();
    let results = run(&root, &BTreeMap::new(), query(""));
    assert!(results.files.is_empty() && !results.canceled && results.errors.is_empty());
    assert_eq!(results.count(), 0);
}

#[test]
fn results_are_ordered_by_path_and_counts_sum_matches() {
    let (_dir, root) = root();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("b.tex"), "cat cat").unwrap();
    std::fs::write(root.join("a.tex"), "cat").unwrap();
    std::fs::write(root.join("sub/c.tex"), "no\ncat").unwrap();
    std::fs::write(root.join("none.tex"), "dog").unwrap();
    let results = run(&root, &BTreeMap::new(), query("cat"));
    assert_eq!(paths(&results), [root.join("a.tex"), root.join("b.tex"), root.join("sub/c.tex")]);
    assert_eq!(results.count(), 4);
    assert_eq!(results.files[2].matches[0].line, 2);
}

#[test]
fn case_and_whole_word_options_apply_to_every_file() {
    let (_dir, root) = root();
    std::fs::write(root.join("a.tex"), "Cat concat CAT cat").unwrap();
    let all = run(&root, &BTreeMap::new(), query("cat"));
    assert_eq!(all.count(), 4);
    let sensitive = Query {
        options: Options {
            case_sensitive: true,
            whole_word: false,
        },
        ..query("cat")
    };
    assert_eq!(run(&root, &BTreeMap::new(), sensitive).count(), 2);
    let words = Query {
        options: Options {
            case_sensitive: false,
            whole_word: true,
        },
        ..query("cat")
    };
    assert_eq!(run(&root, &BTreeMap::new(), words).count(), 3);
}

#[test]
fn queries_are_literal_even_when_they_look_like_patterns() {
    let (_dir, root) = root();
    std::fs::write(root.join("a.tex"), "a.b a+b (x) [y] \\section{z}").unwrap();
    for needle in [".", "a+b", "(x)", "[y]", "\\section{z}"] {
        assert_eq!(run(&root, &BTreeMap::new(), query(needle)).count(), 1, "{needle}");
    }
}

#[test]
fn open_buffers_override_disk_and_files_that_only_exist_in_memory_are_searched() {
    let (_dir, root) = root();
    std::fs::write(root.join("saved.tex"), "needle on disk").unwrap();
    let open = BTreeMap::from([
        (root.join("saved.tex"), open_buffer("edited text", "needle on disk")),
        (root.join("unsaved.tex"), open_buffer("a needle", "")),
    ]);
    let results = run(&root, &open, query("needle"));
    assert_eq!(paths(&results), [root.join("unsaved.tex")]);
    assert_eq!(results.files[0].source.buffer, open[&root.join("unsaved.tex")].buffer);
}

#[test]
fn open_buffers_in_excluded_locations_are_not_searched() {
    let (_dir, root) = root();
    let outside = tempfile::tempdir().unwrap();
    let open: BTreeMap<_, _> = [
        root.join("build/x.tex"),
        root.join(".git/x.tex"),
        root.join("image.png"),
        std::fs::canonicalize(outside.path()).unwrap().join("x.tex"),
    ]
    .into_iter()
    .map(|path| (path, open_buffer("needle", "")))
    .collect();
    assert!(run(&root, &open, query("needle")).files.is_empty());
}

#[test]
fn unreadable_files_are_reported_while_other_files_are_still_searched() {
    let (_dir, root) = root();
    std::fs::write(root.join("good.tex"), "needle").unwrap();
    std::fs::write(root.join("binary.tex"), [0xff, 0xfe, 0xfd]).unwrap();
    let results = run(&root, &BTreeMap::new(), query("needle"));
    assert_eq!(paths(&results), [root.join("good.tex")]);
    assert_eq!(results.errors.len(), 1);
    assert!(results.errors[&root.join("binary.tex")].contains("unsupported encoding"));
}

#[test]
fn match_positions_use_scalar_columns_and_each_file_keeps_its_snapshot() {
    let (_dir, root) = root();
    std::fs::write(root.join("a.tex"), "日本 needle\r\nnext needle").unwrap();
    let results = run(&root, &BTreeMap::new(), query("needle"));
    let matches = &results.files[0].matches;
    assert_eq!((matches[0].line, matches[0].column), (1, 4));
    assert_eq!((matches[1].line, matches[1].column), (2, 6));
    assert_eq!(matches[0].excerpt, "日本 needle");
    assert_eq!(
        results.files[0].source.disk,
        FileState::Present("日本 needle\r\nnext needle".as_bytes().to_vec())
    );
}

#[test]
fn invalid_queries_never_fail_construction() {
    let (_dir, root) = root();
    assert!(
        search_project(
            &root,
            &root.join("build"),
            &BTreeMap::new(),
            query("(unclosed["),
            &Cancellation::default()
        )
        .is_ok()
    );
}

fn single_file_results(root: &Path, text: &str, needle: &str) -> ProjectResults {
    std::fs::write(root.join("a.tex"), text).unwrap();
    run(root, &BTreeMap::new(), query(needle))
}

#[test]
fn previews_track_replaced_positions_across_lines_and_multiline_replacements() {
    let (_dir, root) = root();
    let results = single_file_results(&root, "cat\nx cat\ncat", "cat");
    let preview = results.preview("a\nb");
    let file = &preview.files[0];
    assert_eq!(file.after, "a\nb\nx a\nb\na\nb");
    assert_eq!(file.edit.count, 3);
    let positions: Vec<_> = file.after_matches.iter().map(|found| (found.line, found.column)).collect();
    assert_eq!(positions, [(1, 1), (3, 3), (5, 1)]);
    for found in &file.after_matches {
        assert_eq!(&file.after[found.range.clone()], "a\nb");
    }
}

#[test]
fn adjacent_matches_are_replaced_independently() {
    let (_dir, root) = root();
    let results = single_file_results(&root, "aaaa", "aa");
    let preview = results.preview("b");
    assert_eq!(preview.files[0].after, "bb");
    assert_eq!(preview.files[0].after_matches.len(), 2);
}

#[test]
fn previews_cover_every_file_and_skip_nothing() {
    let (_dir, root) = root();
    std::fs::write(root.join("a.tex"), "x").unwrap();
    std::fs::write(root.join("b.tex"), "x x").unwrap();
    let results = run(&root, &BTreeMap::new(), query("x"));
    let preview = results.preview("yy");
    assert_eq!(preview.files.iter().map(|file| file.after.as_str()).collect::<Vec<_>>(), ["yy", "yy yy"]);
    assert_eq!(preview.files.iter().map(|file| file.edit.count).sum::<usize>(), results.count());
}

#[test]
fn a_canceled_preview_produces_nothing() {
    let (_dir, root) = root();
    let results = single_file_results(&root, "x", "x");
    let cancellation = Cancellation::default();
    cancellation.cancel();
    assert!(results.preview_cancellable("y", &cancellation).is_none());
    assert!(results.preview_cancellable("y", &Cancellation::default()).is_some());
}

#[test]
fn empty_results_preview_to_an_empty_replacement_set() {
    assert!(ProjectResults::default().preview("anything").files.is_empty());
    assert_eq!(ProjectResults::default().count(), 0);
}

fn first_replacement(root: &Path, text: &str) -> FileReplacement {
    let results = single_file_results(root, text, "old");
    results.preview("new").files.into_iter().next().unwrap()
}

#[test]
fn applying_a_replacement_writes_atomically_and_preserves_unmatched_text() {
    let (_dir, root) = root();
    let replacement = first_replacement(&root, "keep old keep\r\nold\n");
    apply_disk_replacement(&replacement).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("a.tex")).unwrap(), "keep new keep\r\nnew\n");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn applying_a_replacement_fails_if_the_file_changed_was_deleted_or_is_open_in_a_buffer() {
    let (_dir, root) = root();
    let replacement = first_replacement(&root, "old");
    std::fs::write(root.join("a.tex"), "old but changed").unwrap();
    assert!(apply_disk_replacement(&replacement).is_err());
    assert_eq!(std::fs::read_to_string(root.join("a.tex")).unwrap(), "old but changed");
    std::fs::remove_file(root.join("a.tex")).unwrap();
    assert!(apply_disk_replacement(&replacement).is_err());
    assert!(!root.join("a.tex").exists());
    let mut buffered = replacement;
    buffered.file.source = open_buffer("old", "old");
    std::fs::write(root.join("a.tex"), "old").unwrap();
    let error = apply_disk_replacement(&buffered).unwrap_err();
    assert!(error.contains("open document"));
    assert_eq!(std::fs::read_to_string(root.join("a.tex")).unwrap(), "old");
}
