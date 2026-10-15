use ligature_fs::Signature;
use rustc_hash::FxHasher;
use std::{
    collections::{BTreeMap, HashMap},
    hash::Hasher,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Content digests of project sources. File signatures observed alongside a
/// matching digest let repeated disk comparisons skip rereading unchanged files.
#[derive(Debug)]
pub struct SourceSnapshot {
    files: BTreeMap<PathBuf, u64>,
    complete: bool,
    verified: Mutex<HashMap<PathBuf, Signature>>,
}

impl Clone for SourceSnapshot {
    fn clone(&self) -> Self {
        Self {
            files: self.files.clone(),
            complete: self.complete,
            verified: Mutex::new(self.verified.lock().unwrap().clone()),
        }
    }
}

impl PartialEq for SourceSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.files == other.files && self.complete == other.complete
    }
}

impl Eq for SourceSnapshot {}

impl SourceSnapshot {
    pub fn capture(root: &Path, output_directory: &Path) -> Self {
        let sources = Self::source_paths(root, output_directory);
        let mut complete = sources.errors.is_empty();
        let mut verified = HashMap::new();
        let files = sources
            .paths
            .into_iter()
            .filter_map(|path| {
                let signature = Signature::of(&path).ok();
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        if let Some(signature) = signature {
                            verified.insert(path.clone(), signature);
                        }
                        Some((path, Self::digest(&bytes)))
                    }
                    Err(_) => {
                        complete = false;
                        None
                    }
                }
            })
            .collect();
        Self {
            files,
            complete,
            verified: Mutex::new(verified),
        }
    }

    pub fn matches_disk(&self, root: &Path, output_directory: &Path) -> bool {
        if !self.complete {
            return false;
        }
        let sources = Self::source_paths(root, output_directory);
        if !sources.errors.is_empty() || !sources.paths.iter().eq(self.files.keys()) {
            return false;
        }
        let mut verified = self.verified.lock().unwrap();
        self.files.iter().all(|(path, digest)| Self::file_matches(&mut verified, path, *digest))
    }

    /// Whether the captured file at `path` still has its captured contents on disk.
    pub fn matches_disk_file(&self, path: &Path) -> bool {
        let Some((path, digest)) = self.entry(path) else { return false };
        Self::file_matches(&mut self.verified.lock().unwrap(), path, *digest)
    }

    pub fn matches_text(&self, path: &Path, text: &str) -> bool {
        self.entry(path).is_some_and(|(_, digest)| *digest == Self::digest(text.as_bytes()))
    }

    fn entry(&self, path: &Path) -> Option<(&Path, &u64)> {
        self.files
            .get_key_value(path)
            .or_else(|| {
                let resolved = std::fs::canonicalize(path).ok()?;
                self.files.get_key_value(&resolved)
            })
            .map(|(path, digest)| (path.as_path(), digest))
    }

    fn file_matches(verified: &mut HashMap<PathBuf, Signature>, path: &Path, digest: u64) -> bool {
        let Ok(signature) = Signature::of(path) else { return false };
        if verified.get(path) == Some(&signature) {
            return true;
        }
        let matches = std::fs::read(path).is_ok_and(|bytes| Self::digest(&bytes) == digest);
        if matches {
            verified.insert(path.to_owned(), signature);
        } else {
            verified.remove(path);
        }
        matches
    }

    fn digest(bytes: &[u8]) -> u64 {
        let mut hasher = FxHasher::default();
        hasher.write(bytes);
        hasher.finish()
    }

    fn source_paths(root: &Path, output_directory: &Path) -> ligature_project::SearchFiles {
        let resolved_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let output_directory = output_directory
            .strip_prefix(root)
            .map_or_else(|_| output_directory.to_path_buf(), |relative| resolved_root.join(relative));
        ligature_project::search_files(&resolved_root, &output_directory, || false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_edits_saves_additions_and_deleted_sources() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let path = root.join("main.tex");
        let output = root.join("build");
        std::fs::write(&path, "old").unwrap();
        let snapshot = SourceSnapshot::capture(root, &output);
        assert!(snapshot.matches_disk(root, &output));
        assert!(!snapshot.matches_text(&path, "new"));
        std::fs::write(&path, "new").unwrap();
        assert!(!snapshot.matches_disk(root, &output));
        std::fs::write(&path, "old").unwrap();
        std::fs::write(root.join("included.tex"), "added").unwrap();
        assert!(!snapshot.matches_disk(root, &output));
        std::fs::remove_file(path).unwrap();
        assert!(!snapshot.matches_disk(root, &output));
    }

    #[test]
    fn disk_comparison_preserves_capture_semantics_for_file_sets_and_failures() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let output = root.join("output");
        std::fs::create_dir(&output).unwrap();
        let first = root.join("first.tex");
        let second = root.join("second.bib");
        std::fs::write(&first, "original").unwrap();
        std::fs::write(&second, "bibliography").unwrap();
        let snapshot = SourceSnapshot::capture(root, &output);
        let assert_matches_capture = || {
            assert_eq!(snapshot.matches_disk(root, &output), snapshot == SourceSnapshot::capture(root, &output));
        };
        assert_matches_capture();
        std::fs::write(output.join("ignored.tex"), "ignored").unwrap();
        assert_matches_capture();
        std::fs::write(&first, "changed").unwrap();
        assert_matches_capture();
        std::fs::write(&first, "original").unwrap();
        std::fs::rename(&second, root.join("renamed.bib")).unwrap();
        assert_matches_capture();
        let incomplete = SourceSnapshot::capture(&root.join("missing"), &output);
        assert!(!incomplete.complete);
        assert!(!incomplete.matches_disk(&root.join("missing"), &output));
    }
}
