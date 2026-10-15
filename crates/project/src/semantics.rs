use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use ligature_buffer::Buffer;
use ligature_syntax::{Language, Snapshot};

use crate::normalize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineEntry {
    pub title: String,
    pub level: usize,
    pub parent: Option<usize>,
    pub location: Location,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub text: Arc<str>,
    pub document: Arc<ligature_tex::Document>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SemanticIndex {
    pub sources: BTreeMap<PathBuf, Source>,
    pub bibliography: ligature_bibliography::Index,
    pub bibliography_sources: BTreeMap<PathBuf, Arc<str>>,
    pub labels: BTreeMap<String, Vec<Location>>,
    pub unresolved_references: Vec<(String, Location)>,
    pub unresolved_citations: Vec<(String, Location)>,
    pub outline: Vec<OutlineEntry>,
    pub errors: BTreeMap<PathBuf, String>,
}

impl SemanticIndex {
    pub fn load_with(root: &Path, read: impl FnMut(&Path) -> Result<String, String>) -> Self {
        Self::refresh_with(root, &Self::default(), read)
    }

    pub fn refresh_with<T>(root: &Path, previous: &Self, read: impl FnMut(&Path) -> Result<T, String>) -> Self
    where
        T: AsRef<str> + Into<Arc<str>>,
    {
        Self::refresh_cancellable(root, previous, read, || false)
    }

    pub fn refresh_cancellable<T>(
        root: &Path, previous: &Self, mut read: impl FnMut(&Path) -> Result<T, String>, mut canceled: impl FnMut() -> bool,
    ) -> Self
    where
        T: AsRef<str> + Into<Arc<str>>,
    {
        let mut index = Self::default();
        let root = canonical(root);
        let mut pending = vec![root.clone()];
        let mut visited = BTreeSet::new();
        let mut bibliography_files = BTreeSet::new();
        let mut resolved_includes = BTreeMap::new();
        let mut previous_bibliographies: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (key, definitions) in &previous.bibliography.entries {
            for definition in definitions {
                previous_bibliographies.entry(&definition.path).or_default().push((key, definition));
            }
        }
        while let Some(path) = pending.pop() {
            if canceled() {
                return Self::default();
            }
            if !visited.insert(path.clone()) {
                continue;
            }
            let text = match read(&path) {
                Ok(text) => text,
                Err(error) => {
                    index.errors.insert(path, error);
                    continue;
                }
            };
            let source = if let Some(source) = previous
                .sources
                .get(&path)
                .filter(|source| std::ptr::eq(source.text.as_ref(), text.as_ref()) || source.text.as_ref() == text.as_ref())
            {
                source.clone()
            } else {
                let buffer = Buffer::new();
                let text = text.into();
                let snapshot = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, Arc::clone(&text));
                Source {
                    text,
                    document: Arc::new(ligature_tex::parse(&snapshot)),
                }
            };
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            let includes: Vec<_> = source
                .document
                .includes
                .iter()
                .map(|include| resource(parent, &include.name, "tex"))
                .collect();
            pending.extend(includes.iter().rev().cloned());
            resolved_includes.insert(path.clone(), includes);
            for bibliography in &source.document.bibliographies {
                bibliography_files.insert(resource(parent, &bibliography.name, "bib"));
            }
            for label in &source.document.labels {
                index.labels.entry(label.name.clone()).or_default().push(Location {
                    path: path.clone(),
                    range: label.range.clone(),
                });
            }
            index.sources.insert(path, source);
        }
        for path in bibliography_files {
            if canceled() {
                return Self::default();
            }
            match read(&path) {
                Ok(text) => {
                    let unchanged = previous
                        .bibliography_sources
                        .get(&path)
                        .filter(|old| std::ptr::eq(old.as_ref(), text.as_ref()) || old.as_ref() == text.as_ref());
                    if unchanged.is_some() {
                        for &(key, definition) in previous_bibliographies.get(&path).into_iter().flatten() {
                            index.bibliography.entries.entry(key.clone()).or_default().push(definition.clone());
                        }
                    } else {
                        index.bibliography.insert(path.clone(), &ligature_bibliography::parse(text.as_ref()));
                    }
                    index.bibliography_sources.insert(path, unchanged.cloned().unwrap_or_else(|| text.into()));
                }
                Err(error) => {
                    index.errors.insert(path, error);
                }
            }
        }
        for (path, source) in &index.sources {
            if canceled() {
                return Self::default();
            }
            for reference in &source.document.references {
                if index.definitions(&reference.name).is_empty() {
                    index.unresolved_references.push((
                        reference.name.clone(),
                        Location {
                            path: path.clone(),
                            range: reference.range.clone(),
                        },
                    ));
                }
            }
            for citation in &source.document.citations {
                if citation.name != "*" && index.bibliography.resolve(&citation.name).is_empty() {
                    index.unresolved_citations.push((
                        citation.name.clone(),
                        Location {
                            path: path.clone(),
                            range: citation.range.clone(),
                        },
                    ));
                }
            }
        }
        index.outline = outline(&root, &index.sources, &resolved_includes);
        index
    }

    pub fn definitions(&self, key: &str) -> &[Location] {
        self.labels.get(key).map_or(&[], Vec::as_slice)
    }

    pub fn duplicate_labels(&self) -> impl Iterator<Item = (&str, &[Location])> {
        self.labels
            .iter()
            .filter(|(_, locations)| locations.len() > 1)
            .map(|(key, locations)| (key.as_str(), locations.as_slice()))
    }

    pub fn definition_at(&self, path: &Path, offset: usize) -> Vec<Location> {
        let Some(source) = self.sources.get(path) else { return Vec::new() };
        if let Some(reference) = source.document.references.iter().find(|key| key.range.contains(&offset)) {
            return self.definitions(&reference.name).to_vec();
        }
        if let Some(citation) = source.document.citations.iter().find(|key| key.range.contains(&offset)) {
            return self
                .bibliography
                .resolve(&citation.name)
                .iter()
                .map(|definition| Location {
                    path: definition.path.clone(),
                    range: definition.entry.key_range.clone(),
                })
                .collect();
        }
        Vec::new()
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize(path))
}

fn resource(parent: &Path, name: &str, extension: &str) -> PathBuf {
    let mut path = parent.join(name);
    if path.extension().is_none() {
        path.set_extension(extension);
    }
    canonical(&path)
}

fn outline(root: &Path, sources: &BTreeMap<PathBuf, Source>, resolved_includes: &BTreeMap<PathBuf, Vec<PathBuf>>) -> Vec<OutlineEntry> {
    enum Event<'a> {
        File(PathBuf),
        Heading(PathBuf, &'a ligature_tex::Heading),
    }
    let mut result: Vec<OutlineEntry> = Vec::new();
    let mut headings: Vec<usize> = Vec::new();
    let mut visited = BTreeSet::new();
    let mut pending = vec![Event::File(root.to_owned())];
    while let Some(event) = pending.pop() {
        match event {
            Event::File(path) => {
                if !visited.insert(path.clone()) {
                    continue;
                }
                let Some(source) = sources.get(&path) else { continue };
                let mut source_headings = source.document.headings.iter().rev().peekable();
                let mut includes = source.document.includes.iter().zip(&resolved_includes[&path]).rev().peekable();
                pending.reserve(source_headings.len() + includes.len());
                while source_headings.peek().is_some() || includes.peek().is_some() {
                    if source_headings
                        .peek()
                        .is_some_and(|heading| includes.peek().is_none_or(|(include, _)| heading.range.start > include.range.start))
                    {
                        pending.push(Event::Heading(path.clone(), source_headings.next().unwrap()));
                    } else {
                        pending.push(Event::File(includes.next().unwrap().1.clone()));
                    }
                }
            }
            Event::Heading(path, heading) => {
                while headings.last().is_some_and(|&entry| result[entry].level >= heading.level) {
                    headings.pop();
                }
                let parent = headings.last().copied();
                headings.push(result.len());
                result.push(OutlineEntry {
                    title: heading.title.clone(),
                    level: heading.level,
                    parent,
                    location: Location {
                        path,
                        range: heading.range.clone(),
                    },
                });
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn canceling_semantic_refresh_stops_reads_and_discards_partial_indexes() {
        let canceled = std::cell::Cell::new(false);
        let reads = std::cell::Cell::new(0);
        let index = super::SemanticIndex::refresh_cancellable(
            std::path::Path::new("/p/main.tex"),
            &super::SemanticIndex::default(),
            |_| {
                reads.set(reads.get() + 1);
                canceled.set(true);
                Ok::<_, String>("\\input{child}\\label{first}".to_owned())
            },
            || canceled.get(),
        );
        assert_eq!(reads.get(), 1);
        assert_eq!(index, super::SemanticIndex::default());
    }
    use super::*;

    fn files() -> BTreeMap<PathBuf, String> {
        [
            (
                "/project/main.tex",
                "\\section{Main}\\input{child}\\ref{child}\\ref{missing}\\cite{book,absent}\\bibliography{one,two}",
            ),
            (
                "/project/child.tex",
                "\\subsection{Child}\\label{child}\\label{duplicate}\\label{duplicate}\\input{main}",
            ),
            ("/project/one.bib", "@book{book,title={One},year=2026}"),
            ("/project/two.bib", "@article{book,title={Two}}"),
        ]
        .into_iter()
        .map(|(path, text)| (PathBuf::from(path), text.to_owned()))
        .collect()
    }

    #[test]
    fn resolves_included_labels_citations_duplicates_and_cycles() {
        let files = files();
        let index = SemanticIndex::load_with(Path::new("/project/main.tex"), |path| {
            files.get(path).cloned().ok_or_else(|| "missing".into())
        });
        assert_eq!(index.sources.len(), 2);
        assert_eq!(index.definitions("child").len(), 1);
        assert_eq!(index.duplicate_labels().next().unwrap().0, "duplicate");
        assert_eq!(index.unresolved_references[0].0, "missing");
        assert_eq!(index.unresolved_citations[0].0, "absent");
        assert_eq!(index.bibliography.resolve("book").len(), 2);
        assert_eq!(index.outline[1].parent, Some(0));
        let main = Path::new("/project/main.tex");
        let offset = files[main].find("ref{child}").unwrap() + 4;
        assert_eq!(index.definition_at(main, offset)[0].path, Path::new("/project/child.tex"));
    }

    #[test]
    fn refreshing_replaces_removed_symbols_and_changed_bibliographies() {
        let mut files = files();
        let root = Path::new("/project/main.tex");
        let initial = SemanticIndex::load_with(root, |path| files.get(path).cloned().ok_or_else(|| "missing".into()));
        files.insert(PathBuf::from("/project/child.tex"), "\\section{Changed}".into());
        files.remove(Path::new("/project/two.bib"));
        let refreshed = SemanticIndex::refresh_with(root, &initial, |path| files.get(path).cloned().ok_or_else(|| "missing".into()));
        assert!(refreshed.definitions("child").is_empty());
        assert_eq!(refreshed.bibliography.resolve("book").len(), 1);
        assert_eq!(refreshed.unresolved_references.len(), 2);
        assert_eq!(refreshed.errors.len(), 1);
        assert!(Arc::ptr_eq(&initial.sources[root].text, &refreshed.sources[root].text));
        assert!(Arc::ptr_eq(&initial.sources[root].document, &refreshed.sources[root].document));
    }

    #[test]
    fn unchanged_bibliographies_preserve_duplicate_order_and_share_source_text() {
        let files = files();
        let root = Path::new("/project/main.tex");
        let initial = SemanticIndex::load_with(root, |path| files.get(path).cloned().ok_or_else(|| "missing".into()));
        let refreshed = SemanticIndex::refresh_with(root, &initial, |path| files.get(path).cloned().ok_or_else(|| "missing".into()));
        assert_eq!(initial, refreshed);
        for (path, source) in &initial.sources {
            assert!(Arc::ptr_eq(&source.document, &refreshed.sources[path].document));
        }
        for (path, text) in &initial.bibliography_sources {
            assert!(Arc::ptr_eq(text, &refreshed.bibliography_sources[path]));
        }
        assert_eq!(refreshed.bibliography.resolve("book")[0].entry.title(), Some("One"));
        assert_eq!(refreshed.bibliography.resolve("book")[1].entry.title(), Some("Two"));
    }

    #[test]
    fn outline_follows_include_positions_between_headings() {
        let files: BTreeMap<PathBuf, String> = [
            ("/project/main.tex", "\\section{First}\\input{child}\\section{Last}"),
            ("/project/child.tex", "\\subsection{Child}"),
        ]
        .into_iter()
        .map(|(path, text)| (PathBuf::from(path), text.into()))
        .collect();
        let index = SemanticIndex::load_with(Path::new("/project/main.tex"), |path| {
            files.get(path).cloned().ok_or_else(|| "missing".into())
        });
        assert_eq!(
            index.outline.iter().map(|entry| entry.title.as_str()).collect::<Vec<_>>(),
            ["First", "Child", "Last"]
        );
        assert_eq!(index.outline[1].parent, Some(0));
        assert_eq!(index.outline[2].parent, None);
    }
}
