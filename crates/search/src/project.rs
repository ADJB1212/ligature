use crate::{Cancellation, Match, Query, ReplacementEdit, Search, describe_matches_cancellable, replacement_edit_cancellable};
use ligature_buffer::{DocumentId, Revision};
use ligature_fs::FileState;
use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub const MAX_PROJECT_MATCHES: usize = 10_000;
pub const MAX_PROJECT_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PROJECT_ERRORS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferVersion {
    pub document: DocumentId,
    pub revision: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSnapshot {
    pub text: String,
    pub buffer: Option<BufferVersion>,
    pub disk: FileState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileResults {
    pub path: PathBuf,
    pub source: SourceSnapshot,
    pub matches: Vec<Match>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectResults {
    pub query: Query,
    pub files: Vec<FileResults>,
    pub errors: BTreeMap<PathBuf, String>,
    pub canceled: bool,
    pub truncated: bool,
}
impl ProjectResults {
    pub fn count(&self) -> usize {
        self.files.iter().map(|file| file.matches.len()).sum()
    }
    pub fn preview(&self, replacement: &str) -> ReplacementPreview {
        self.preview_cancellable(replacement, &Cancellation::default()).unwrap_or_default()
    }
    pub fn preview_cancellable(&self, replacement: &str, cancellation: &Cancellation) -> Option<ReplacementPreview> {
        if self.truncated || self.canceled {
            return None;
        }
        let bytes = self.files.iter().try_fold(0usize, |retained, file| {
            let removed: usize = file.matches.iter().map(|matched| matched.range.len()).sum();
            let added = replacement.len().checked_mul(file.matches.len())?;
            let after = file.source.text.len().checked_sub(removed)?.checked_add(added)?;
            retained.checked_add(after.checked_mul(2)?)
        })?;
        if bytes > MAX_PROJECT_SOURCE_BYTES {
            return None;
        }
        let files = self
            .files
            .iter()
            .take_while(|_| !cancellation.is_canceled())
            .filter_map(|file| {
                let ranges = file.matches.iter().map(|matched| matched.range.clone());
                let edit = replacement_edit_cancellable(&file.source.text, ranges, replacement, cancellation)?;
                let mut after = String::with_capacity(file.source.text.len() - edit.range.len() + edit.text.len());
                after.push_str(&file.source.text[..edit.range.start]);
                after.push_str(&edit.text);
                after.push_str(&file.source.text[edit.range.end..]);
                let (mut old_offset, mut new_offset) = (0, 0);
                let ranges = file
                    .matches
                    .iter()
                    .take_while(|_| !cancellation.is_canceled())
                    .map(|matched| {
                        let start = new_offset + matched.range.start - old_offset;
                        let range = start..start + replacement.len();
                        old_offset = matched.range.end;
                        new_offset = range.end;
                        range
                    })
                    .collect();
                let after_matches = describe_matches_cancellable(&after, ranges, cancellation);
                if cancellation.is_canceled() {
                    return None;
                }
                Some(FileReplacement {
                    file: file.clone(),
                    edit,
                    after,
                    after_matches,
                })
            })
            .collect();
        if cancellation.is_canceled() {
            None
        } else {
            Some(ReplacementPreview { files })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileReplacement {
    pub file: FileResults,
    pub edit: ReplacementEdit,
    pub after: String,
    pub after_matches: Vec<Match>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplacementPreview {
    pub files: Vec<FileReplacement>,
}

pub fn search_project(
    root: &Path, output_directory: &Path, open: &BTreeMap<PathBuf, SourceSnapshot>, query: Query, cancellation: &Cancellation,
) -> Result<ProjectResults, String> {
    search_project_with(root, output_directory, open, query, cancellation, |path| {
        ligature_fs::read(path)
            .map(|file| SourceSnapshot {
                disk: FileState::Present(file.text.as_bytes().to_vec()),
                text: file.text,
                buffer: None,
            })
            .map_err(|error| error.to_string())
    })
}

fn search_project_with(
    root: &Path, output_directory: &Path, open: &BTreeMap<PathBuf, SourceSnapshot>, query: Query, cancellation: &Cancellation,
    mut read: impl FnMut(&Path) -> Result<SourceSnapshot, String>,
) -> Result<ProjectResults, String> {
    let search = Search::new(&query)?;
    let mut results = ProjectResults {
        query,
        ..ProjectResults::default()
    };
    if results.query.text.is_empty() {
        return Ok(results);
    }
    let mut files = ligature_project::search_files(root, output_directory, || cancellation.is_canceled());
    for path in open.keys() {
        if ligature_project::eligible_search_path(root, path, output_directory) {
            files.paths.insert(path.clone());
        }
    }
    results.truncated = files.errors.len() > MAX_PROJECT_ERRORS;
    results.errors = files.errors.into_iter().take(MAX_PROJECT_ERRORS).collect();
    let (mut retained_bytes, mut retained_matches) = (0, 0);
    for path in files.paths {
        if cancellation.is_canceled() {
            break;
        }
        let source = if let Some(source) = open.get(&path) {
            Cow::Borrowed(source)
        } else {
            match read(&path) {
                Ok(source) => Cow::Owned(source),
                Err(error) => {
                    if results.errors.len() < MAX_PROJECT_ERRORS {
                        results.errors.insert(path, error);
                    } else {
                        results.truncated = true;
                    }
                    continue;
                }
            }
        };
        let mut found = search.find_limited(&source.text, cancellation, MAX_PROJECT_MATCHES - retained_matches + 1);
        if !found.is_empty() {
            let bytes = source.text.len()
                + match &source.disk {
                    FileState::Present(bytes) => bytes.len(),
                    _ => 0,
                };
            if bytes > MAX_PROJECT_SOURCE_BYTES - retained_bytes {
                results.truncated = true;
                break;
            }
            if found.len() > MAX_PROJECT_MATCHES - retained_matches {
                results.truncated = true;
                found.truncate(MAX_PROJECT_MATCHES - retained_matches);
            }
            if found.is_empty() {
                break;
            }
            retained_bytes += bytes;
            retained_matches += found.len();
            let matches = describe_matches_cancellable(&source.text, found, cancellation);
            results.files.push(FileResults {
                path,
                source: source.into_owned(),
                matches,
            });
            if results.truncated {
                break;
            }
        }
    }
    if cancellation.is_canceled() {
        results.canceled = true;
        results.files.clear();
    }
    Ok(results)
}

pub fn apply_disk_replacement(replacement: &FileReplacement) -> Result<(), String> {
    if replacement.file.source.buffer.is_some() {
        return Err("The open document must be replaced in its buffer".into());
    }
    ligature_fs::write_atomic_checked(&replacement.file.path, &replacement.after, Some(&replacement.file.source.disk))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_project_results_cannot_produce_partial_replacements() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let source = SourceSnapshot {
            text: "x".repeat(MAX_PROJECT_MATCHES + 1),
            disk: FileState::Missing,
            buffer: None,
        };
        let results = search_project(
            &root,
            &root.join("build"),
            &BTreeMap::from([(root.join("main.tex"), source)]),
            Query {
                text: "x".into(),
                ..Query::default()
            },
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(results.count(), MAX_PROJECT_MATCHES);
        assert!(results.truncated);
        assert!(results.preview_cancellable("y", &Cancellation::default()).is_none());
        assert!(results.preview("y").files.is_empty());
    }

    #[test]
    fn project_source_and_replacement_preview_bytes_are_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let query = Query {
            text: "x".into(),
            ..Query::default()
        };
        let source = SourceSnapshot {
            text: "x".into(),
            disk: FileState::Present(vec![b'x'; MAX_PROJECT_SOURCE_BYTES]),
            buffer: None,
        };
        let results = search_project(
            &root,
            &root.join("build"),
            &BTreeMap::from([(root.join("main.tex"), source)]),
            query.clone(),
            &Cancellation::default(),
        )
        .unwrap();
        assert!(results.truncated);
        assert!(results.files.is_empty());
        let source = SourceSnapshot {
            text: "xx".into(),
            disk: FileState::Missing,
            buffer: None,
        };
        let results = search_project(
            &root,
            &root.join("build"),
            &BTreeMap::from([(root.join("main.tex"), source)]),
            query,
            &Cancellation::default(),
        )
        .unwrap();
        assert!(!results.truncated);
        assert!(
            results
                .preview_cancellable(&"y".repeat(MAX_PROJECT_SOURCE_BYTES / 2), &Cancellation::default())
                .is_none()
        );
    }
    use crate::describe_matches;
    use ligature_buffer::Buffer;

    #[test]
    fn replacement_preview_preserves_unicode_prefix_suffix_and_positions() {
        let text = "猫 cat\ncat 😀";
        let results = ProjectResults {
            files: vec![FileResults {
                path: "main.tex".into(),
                source: SourceSnapshot {
                    text: text.into(),
                    buffer: None,
                    disk: FileState::Present(text.as_bytes().to_vec()),
                },
                matches: describe_matches(text, vec![4..7, 8..11]),
            }],
            ..ProjectResults::default()
        };
        for replacement in ["", "é", "猫猫猫"] {
            let preview = results.preview(replacement);
            let file = &preview.files[0];
            assert_eq!(file.after, format!("猫 {replacement}\n{replacement} 😀"));
            assert_eq!(file.after_matches[0].range, 4..4 + replacement.len());
            assert_eq!(file.after_matches[1].range, 5 + replacement.len()..5 + 2 * replacement.len());
            assert_eq!((file.after_matches[1].line, file.after_matches[1].column), (2, 1));
        }
    }

    #[test]
    fn project_search_uses_unsaved_text_and_excludes_generated_and_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("build")).unwrap();
        std::fs::write(root.join("build/generated.tex"), "needle").unwrap();
        std::fs::write(root.join("main.tex"), "disk").unwrap();
        std::fs::write(root.join("output.pdf"), "needle").unwrap();
        let buffer = Buffer::from_text("unsaved needle");
        let source = SourceSnapshot {
            text: buffer.text().into(),
            buffer: Some(BufferVersion {
                document: buffer.id(),
                revision: buffer.revision(),
            }),
            disk: FileState::Present(b"disk".to_vec()),
        };
        let query = Query {
            text: "needle".into(),
            ..Query::default()
        };
        let results = search_project(
            &root,
            &root.join("build"),
            &BTreeMap::from([(root.join("main.tex"), source)]),
            query,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(results.count(), 1);
        assert_eq!(results.files[0].matches[0].column, 9);
        assert_eq!(results.files[0].matches[0].excerpt, "unsaved needle");
    }
    #[test]
    fn cancellation_discards_partial_results_and_preview_checks_disk_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let first = root.join("first.tex");
        let second = root.join("second.tex");
        std::fs::write(&first, "cat").unwrap();
        std::fs::write(&second, "cat").unwrap();
        let query = Query {
            text: "cat".into(),
            ..Query::default()
        };
        let results = search_project(&root, &root.join("build"), &BTreeMap::new(), query.clone(), &Cancellation::default()).unwrap();
        let preview = results.preview("catcat");
        std::fs::write(&first, "changed").unwrap();
        assert!(apply_disk_replacement(&preview.files[0]).is_err());
        apply_disk_replacement(&preview.files[1]).unwrap();
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "changed");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "catcat");
        let canceled = Cancellation::default();
        canceled.cancel();
        let results = search_project(&root, &root.join("build"), &BTreeMap::new(), query, &canceled).unwrap();
        assert!(results.canceled);
        assert!(results.files.is_empty());
    }

    #[test]
    fn canceling_after_a_read_stops_remaining_files_and_discards_results() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("one.tex"), "cat").unwrap();
        std::fs::write(root.join("two.tex"), "cat").unwrap();
        let cancellation = Cancellation::default();
        let mut reads = 0;
        let results = search_project_with(
            &root,
            &root.join("build"),
            &BTreeMap::new(),
            Query {
                text: "cat".into(),
                ..Query::default()
            },
            &cancellation,
            |_| {
                reads += 1;
                cancellation.cancel();
                Ok(SourceSnapshot {
                    text: "cat".into(),
                    buffer: None,
                    disk: FileState::Present(b"cat".to_vec()),
                })
            },
        )
        .unwrap();
        assert_eq!(reads, 1);
        assert!(results.canceled);
        assert!(results.files.is_empty());
    }
}
