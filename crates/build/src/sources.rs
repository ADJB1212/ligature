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
        let resolved_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let output_directory = output_directory
            .strip_prefix(root)
            .map_or_else(|_| output_directory.to_path_buf(), |relative| resolved_root.join(relative));
        let sources = ligature_project::search_files(&resolved_root, &output_directory, || false);
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
        self.complete && *self == Self::capture(root, output_directory)
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
}
