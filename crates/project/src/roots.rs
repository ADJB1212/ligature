use std::{collections::BTreeMap, path::Path};

use ligature_syntax::{Category, Language, Snapshot};

use crate::{IncludeGraph, Project, eligible_search_path, search_files};

impl Project {
    /// An active standalone document takes precedence over the existing configured root.
    pub fn discover_root_document(&mut self, output_directory: &Path, active: Option<&Path>, mut read: impl FnMut(&Path) -> Result<String, String>) {
        let active = active.filter(|path| eligible_search_path(&self.root, path, output_directory) && tex_file(path));
        let mut text = BTreeMap::new();
        if let Some(path) = active
            && let Ok(source) = read(path)
        {
            let standalone = has_document_class(&source);
            text.insert(path.to_owned(), source);
            if standalone && self.set_root_document(path).is_ok() {
                return;
            }
        }
        if let Some(path) = self.root_document.clone()
            && self.set_root_document(&path).is_ok()
        {
            return;
        }
        self.root_document = None;
        let files = search_files(&self.root, output_directory, || false);
        let files: Vec<_> = files.paths.into_iter().filter(|path| tex_file(path)).collect();
        for path in &files {
            if !text.contains_key(path)
                && let Ok(source) = read(path)
            {
                text.insert(path.clone(), source);
            }
        }
        let mut candidates: Vec<_> = files
            .iter()
            .filter(|path| text.get(*path).is_some_and(|source| has_document_class(source)))
            .collect();
        candidates.sort_by_key(|path| {
            (
                !path.file_name().is_some_and(|name| name.eq_ignore_ascii_case("main.tex")),
                path.components().count(),
                *path,
            )
        });
        let containing = active.and_then(|active| {
            candidates.iter().copied().find(|path| {
                IncludeGraph::load_with(path, |path| text.get(path).cloned().map_or_else(|| read(path), Ok))
                    .files
                    .contains_key(active)
            })
        });
        let root = containing.or_else(|| candidates.first().copied()).or_else(|| {
            files
                .iter()
                .find(|path| path.file_name().is_some_and(|name| name.eq_ignore_ascii_case("main.tex")))
                .or_else(|| (files.len() == 1).then(|| &files[0]))
        });
        if let Some(path) = root {
            let _ = self.set_root_document(path);
        }
    }
}

fn tex_file(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
}

fn has_document_class(text: &str) -> bool {
    let buffer = ligature_buffer::Buffer::new();
    let snapshot = Snapshot::parse(buffer.id(), buffer.revision(), Language::Tex, text.into());
    snapshot
        .tokens
        .iter()
        .any(|token| token.category == Category::Command && &snapshot.text[token.range.clone()] == "\\documentclass")
}
