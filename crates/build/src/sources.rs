use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSnapshot {
    files: BTreeMap<PathBuf, [u8; 32]>,
    complete: bool,
}

impl SourceSnapshot {
    pub fn capture(root: &Path, output_directory: &Path) -> Self {
        let sources = Self::source_paths(root, output_directory);
        let mut complete = sources.errors.is_empty();
        let files = sources
            .paths
            .into_iter()
            .filter_map(|path| match std::fs::read(&path) {
                Ok(bytes) => Some((path, Self::digest(&bytes))),
                Err(_) => {
                    complete = false;
                    None
                }
            })
            .collect();
        Self { files, complete }
    }

    pub fn matches_disk(&self, root: &Path, output_directory: &Path) -> bool {
        if !self.complete {
            return false;
        }
        let sources = Self::source_paths(root, output_directory);
        sources.errors.is_empty()
            && sources.paths.iter().eq(self.files.keys())
            && self
                .files
                .iter()
                .all(|(path, digest)| std::fs::read(path).is_ok_and(|bytes| Self::digest(&bytes) == *digest))
    }

    pub fn matches_text(&self, path: &Path, text: &str) -> bool {
        let digest = self.files.get(path).or_else(|| {
            let resolved = std::fs::canonicalize(path).ok()?;
            self.files.get(&resolved)
        });
        digest.is_some_and(|digest| *digest == Self::digest(text.as_bytes()))
    }

    fn digest(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
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
