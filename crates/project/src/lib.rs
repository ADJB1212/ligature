//! TeX project membership, root-document selection, and relationships between project files.

use ligature_fs::DirectoryEntry;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub root_document: Option<PathBuf>,
    pub documents: BTreeSet<PathBuf>,
}
impl Project {
    pub fn open(root: &Path) -> Result<Self, String> {
        let root = std::fs::canonicalize(root).map_err(|error| format!("Could not open project '{}': {error}", root.display()))?;
        if !root.is_dir() {
            return Err(format!("'{}' is not a project directory", root.display()));
        }
        Ok(Self {
            root,
            root_document: None,
            documents: BTreeSet::new(),
        })
    }
    pub fn relative(&self, path: &Path) -> Option<PathBuf> {
        let path = normalize(&if path.is_absolute() { path.to_owned() } else { self.root.join(path) });
        path.strip_prefix(&self.root).ok().map(Path::to_owned)
    }
    pub fn add_document(&mut self, path: &Path) {
        if let Some(path) = self.relative(path) {
            self.documents.insert(path);
        }
    }
    pub fn set_root_document(&mut self, path: &Path) -> Result<(), String> {
        let relative = self.relative(path).ok_or("The root document must belong to this project")?;
        let absolute = self.root.join(&relative);
        let resolved = std::fs::canonicalize(&absolute).map_err(|error| error.to_string())?;
        if !resolved.starts_with(&self.root)
            || !resolved.is_file()
            || relative.extension().is_none_or(|extension| !extension.eq_ignore_ascii_case("tex"))
        {
            return Err("The root document must be an existing TeX file within this project".into());
        }
        self.root_document = Some(relative);
        Ok(())
    }
}

pub fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if result.file_name().is_some_and(|name| name != "..") => {
                result.pop();
            }
            Component::ParentDir if result.has_root() => {}
            component => result.push(component.as_os_str()),
        }
    }
    result
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Directory {
    pub entries: Vec<DirectoryEntry>,
    pub error: Option<String>,
}
pub fn directory(path: &Path, output_directory: &Path) -> Directory {
    match ligature_fs::read_directory(path) {
        Ok(mut entries) => {
            entries.retain(|entry| {
                let name = entry.path.file_name().and_then(|name| name.to_str()).unwrap_or("");
                !excluded_name(name) && entry.path != output_directory
            });
            Directory { entries, error: None }
        }
        Err(error) => Directory {
            entries: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

fn excluded_name(name: &str) -> bool {
    matches!(name, ".git" | "target" | "build" | "out" | ".latexmk" | ".ligature.json") || name.starts_with(".ligature-")
}

pub fn eligible_search_path(root: &Path, path: &Path, output_directory: &Path) -> bool {
    searchable(path)
        && !path.starts_with(output_directory)
        && path
            .strip_prefix(root)
            .is_ok_and(|path| path.components().all(|part| !excluded_name(part.as_os_str().to_str().unwrap_or(""))))
}

#[derive(Debug, Default)]
pub struct SearchFiles {
    pub paths: BTreeSet<PathBuf>,
    pub errors: BTreeMap<PathBuf, String>,
}

pub fn search_files(root: &Path, output_directory: &Path, canceled: impl Fn() -> bool) -> SearchFiles {
    let mut files = SearchFiles::default();
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        if canceled() {
            break;
        }
        let entries = directory(&path, output_directory);
        if let Some(error) = entries.error {
            files.errors.insert(path, error);
        }
        for entry in entries.entries {
            if canceled() {
                return files;
            }
            if entry.is_directory {
                pending.push(entry.path);
            } else if searchable(&entry.path) {
                match std::fs::canonicalize(&entry.path) {
                    Ok(path) if eligible_search_path(root, &path, output_directory) => {
                        files.paths.insert(path);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        files.errors.insert(entry.path, error.to_string());
                    }
                }
            }
        }
    }
    files
}

pub fn searchable(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| {
        ["tex", "bib", "sty", "cls", "bst", "ltx", "dtx", "ins", "txt", "cfg", "def", "tikz"]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Include {
    pub requested: String,
    pub path: PathBuf,
    pub resolved: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IncludeGraph {
    pub files: BTreeMap<PathBuf, Vec<Include>>,
    pub errors: BTreeMap<PathBuf, String>,
}
impl IncludeGraph {
    pub fn load(root: &Path) -> Self {
        Self::load_with(root, |path| {
            ligature_fs::read(path).map(|file| file.text).map_err(|error| error.to_string())
        })
    }
    pub fn load_with(root: &Path, mut read: impl FnMut(&Path) -> Result<String, String>) -> Self {
        let mut graph = Self::default();
        let mut pending = vec![std::fs::canonicalize(root).unwrap_or_else(|_| normalize(root))];
        let mut visited = BTreeSet::new();
        while let Some(source) = pending.pop() {
            if !visited.insert(source.clone()) {
                continue;
            }
            let text = match read(&source) {
                Ok(text) => text,
                Err(error) => {
                    graph.errors.insert(source, error);
                    continue;
                }
            };
            let includes = include_paths(&text)
                .into_iter()
                .map(|requested| {
                    let mut path = source.parent().unwrap_or_else(|| Path::new(".")).join(&requested);
                    if path.extension().is_none() {
                        path.set_extension("tex");
                    }
                    let path = std::fs::canonicalize(&path).unwrap_or_else(|_| normalize(&path));
                    pending.push(path.clone());
                    Include {
                        requested,
                        path,
                        resolved: false,
                    }
                })
                .collect();
            graph.files.insert(source, includes);
        }
        let found: BTreeSet<_> = graph.files.keys().cloned().collect();
        for includes in graph.files.values_mut() {
            for include in includes {
                include.resolved = found.contains(&include.path);
            }
        }
        graph
    }
}

pub fn include_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in text.lines() {
        let bytes = line.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                break;
            }
            if bytes[index] != b'\\' {
                index += 1;
                continue;
            }
            index += 1;
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
                index += 1;
            }
            let command = &line[start..index];
            if !matches!(command, "input" | "include" | "subfile") {
                if start == index {
                    index = (index + 1).min(bytes.len());
                }
                continue;
            }
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            let braced = bytes.get(index) == Some(&b'{');
            if braced {
                index += 1;
            }
            let start = index;
            while index < bytes.len()
                && if braced {
                    bytes[index] != b'}'
                } else {
                    !bytes[index].is_ascii_whitespace() && bytes[index] != b'%'
                }
            {
                index += 1;
            }
            if braced && bytes.get(index) != Some(&b'}') {
                continue;
            }
            let path = line[start..index].trim();
            if !path.is_empty() && !path.contains(['\\', '{', '}', '#']) {
                paths.push(path.to_owned());
            }
            if braced {
                index += 1;
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cyclic_and_missing_includes_are_recorded_without_looping() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main.tex");
        std::fs::write(&root, "\\input{chapter one}\n% \\input{ignored}\n\\include{missing}").unwrap();
        std::fs::write(dir.path().join("chapter one.tex"), "\\input{./main.tex}").unwrap();
        let graph = IncludeGraph::load(&root);
        let root = std::fs::canonicalize(root).unwrap();
        assert_eq!(graph.files.len(), 2);
        assert_eq!(graph.files[&root].len(), 2);
        assert!(graph.files[&root][0].resolved);
        assert!(!graph.files[&root][1].resolved);
        assert_eq!(graph.errors.len(), 1);
    }
    #[test]
    fn includes_resolve_relative_to_each_source() {
        let graph = IncludeGraph::load_with(Path::new("/project/main.tex"), |path| match path.to_str().unwrap() {
            "/project/main.tex" => Ok("\\input{sub/one}".into()),
            "/project/sub/one.tex" => Ok("\\input{../two}".into()),
            "/project/two.tex" => Ok(String::new()),
            _ => Err("missing".into()),
        });
        assert_eq!(graph.files.len(), 3);
        assert!(graph.errors.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_include_cycles_use_canonical_file_identity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.tex"), "\\input{alias/main}").unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("alias")).unwrap();
        let graph = IncludeGraph::load(&dir.path().join("main.tex"));
        assert_eq!(graph.files.len(), 1);
        assert!(graph.files.values().next().unwrap()[0].resolved);
    }
    #[test]
    fn membership_normalizes_paths_and_tree_excludes_generated_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("build")).unwrap();
        std::fs::write(dir.path().join("main file.tex"), "").unwrap();
        let mut project = Project::open(dir.path()).unwrap();
        project.add_document(&project.root.join("sub/../main file.tex"));
        assert!(project.documents.contains(Path::new("main file.tex")));
        assert_eq!(project.relative(Path::new("../outside.tex")), None);
        assert_eq!(directory(&project.root, &project.root.join("build")).entries.len(), 1);
        assert!(directory(&project.root.join("missing"), &project.root.join("build")).error.is_some());
        project.set_root_document(Path::new("main file.tex")).unwrap();
        assert_eq!(project.root_document, Some("main file.tex".into()));
    }
}
