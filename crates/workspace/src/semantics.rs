use std::{borrow::Cow, collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use gpui::{Context, IntoElement, Task, Window, div, prelude::*, px};
use ligature_buffer::{DocumentId, Revision};
use ligature_project::{Location, SemanticIndex};
use ligature_theme::{colors, spacing};
use ligature_ui::{Button, caption, inline_error};

use crate::{Workspace, lifecycle::Intent};

#[derive(Default)]
pub(super) struct SemanticUi {
    pub visible: bool,
    pub index: Arc<SemanticIndex>,
    task: Option<Task<()>>,
    open_sources: BTreeMap<PathBuf, (DocumentId, Revision, Arc<str>)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[test]
    fn semantic_versions_track_edits_paths_and_document_replacement_but_ignore_selection() {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(Workspace::new);
        let workspace = window.root(&mut cx).unwrap();
        workspace.update(&mut cx, |workspace, cx| {
            let initial = workspace.semantic_version(cx);
            workspace
                .editor
                .update(cx, |editor, cx| editor.update_state(|state| state.select_all(), cx));
            assert!(workspace.semantic_version(cx) == initial);
            workspace
                .editor
                .update(cx, |editor, cx| editor.update_state(|state| state.insert("changed", false), cx));
            assert!(workspace.semantic_version(cx) != initial);
            let edited = workspace.semantic_version(cx);
            workspace.document.as_mut().unwrap().path = Some("/project/saved.tex".into());
            assert!(workspace.semantic_version(cx) != edited);
            let saved = workspace.semantic_version(cx);
            workspace
                .editor
                .update(cx, |editor, cx| editor.set_buffer(ligature_buffer::Buffer::from_text("changed"), cx));
            assert!(workspace.semantic_version(cx) != saved);
        });
    }

    #[test]
    fn semantic_inputs_share_unchanged_buffers_and_drop_closed_sources() {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(Workspace::new);
        let workspace = window.root(&mut cx).unwrap();
        workspace.update(&mut cx, |workspace, cx| {
            let (root, initial) = workspace.semantic_inputs(cx).unwrap();
            let (_, unchanged) = workspace.semantic_inputs(cx).unwrap();
            assert!(Arc::ptr_eq(&initial[&root], &unchanged[&root]));
            workspace.editor.update(cx, |editor, cx| {
                editor.update_state(|state| state.insert("changed", false), cx);
            });
            let (_, edited) = workspace.semantic_inputs(cx).unwrap();
            assert!(!Arc::ptr_eq(&initial[&root], &edited[&root]));
            assert_eq!(edited[&root].as_ref(), workspace.editor.read(cx).state().buffer().text());
            workspace.editor.update(cx, |editor, cx| {
                editor.set_buffer(ligature_buffer::Buffer::from_text(edited[&root].as_ref()), cx);
            });
            let (replacement_root, replaced) = workspace.semantic_inputs(cx).unwrap();
            assert!(!Arc::ptr_eq(&edited[&root], &replaced[&replacement_root]));
            assert_eq!(edited[&root], replaced[&replacement_root]);
            assert!(!workspace.semantic_ui.open_sources.contains_key(&root));
            workspace.document.as_mut().unwrap().path = Some("/project/saved.tex".into());
            let (saved, _) = workspace.semantic_inputs(cx).unwrap();
            assert_ne!(replacement_root, saved);
            assert!(!workspace.semantic_ui.open_sources.contains_key(&replacement_root));
            let document = workspace.document.take();
            assert!(workspace.semantic_inputs(cx).is_none());
            assert!(workspace.semantic_ui.open_sources.is_empty());
            workspace.document = document;
        });
    }
}

#[derive(PartialEq, Eq)]
struct SemanticVersion {
    root: PathBuf,
    documents: Vec<(Option<PathBuf>, DocumentId, Revision)>,
}

impl Workspace {
    fn semantic_path(&self, cx: &gpui::App) -> PathBuf {
        self.document
            .as_ref()
            .and_then(|document| document.path.clone())
            .unwrap_or_else(|| std::env::temp_dir().join(format!("ligature-outline-{:?}.tex", self.editor.read(cx).state().buffer().id())))
    }

    fn semantic_inputs(&mut self, cx: &gpui::App) -> Option<(PathBuf, BTreeMap<PathBuf, Arc<str>>)> {
        if self.document.is_none() {
            self.semantic_ui.open_sources.clear();
            return None;
        }
        let active = self.semantic_path(cx);
        let root = self
            .project
            .as_ref()
            .and_then(|project| project.root_document.as_ref().map(|path| project.root.join(path)))
            .unwrap_or_else(|| active.clone());
        let mut text = BTreeMap::new();
        let mut snapshot = |path: PathBuf, editor: &ligature_editor::Editor| {
            let buffer = editor.state().buffer();
            let version = (buffer.id(), buffer.revision());
            let source = self
                .semantic_ui
                .open_sources
                .entry(path.clone())
                .or_insert_with(|| (version.0, version.1, Arc::from(buffer.text())));
            if (source.0, source.1) != version {
                *source = (version.0, version.1, Arc::from(buffer.text()));
            }
            text.insert(path, Arc::clone(&source.2));
        };
        snapshot(active, self.editor.read(cx));
        for tab in self.tabs.iter().flatten() {
            if let Some(path) = &tab.document.path {
                snapshot(path.clone(), tab.editor.read(cx));
            }
        }
        self.semantic_ui.open_sources.retain(|path, _| text.contains_key(path));
        Some((root, text))
    }

    fn semantic_version(&self, cx: &gpui::App) -> Option<SemanticVersion> {
        self.document.as_ref()?;
        let root = self
            .project
            .as_ref()
            .and_then(|project| project.root_document.as_ref().map(|path| project.root.join(path)))
            .unwrap_or_else(|| self.semantic_path(cx));
        let documents = self
            .tabs
            .iter()
            .map(|tab| {
                let (document, editor) = tab
                    .as_ref()
                    .map_or((self.document.as_ref(), &self.editor), |tab| (Some(&tab.document), &tab.editor));
                let buffer = editor.read(cx).state().buffer();
                (document.and_then(|document| document.path.clone()), buffer.id(), buffer.revision())
            })
            .collect();
        Some(SemanticVersion { root, documents })
    }

    pub(super) fn queue_semantics(&mut self, cx: &mut Context<Self>) {
        self.semantic_ui.task = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(inputs) = this.update(cx, |workspace, cx| {
                    workspace
                        .semantic_inputs(cx)
                        .map(|(root, text)| (root, text, workspace.semantic_version(cx), Arc::clone(&workspace.semantic_ui.index)))
                }) else {
                    break;
                };
                if let Some((root, text, version, previous)) = inputs {
                    let index = cx
                        .background_executor()
                        .spawn(async move {
                            SemanticIndex::refresh_with(&root, &previous, |path| {
                                text.get(path).map_or_else(
                                    || {
                                        ligature_fs::read(path)
                                            .map(|file| Cow::Owned(file.text))
                                            .map_err(|error| error.to_string())
                                    },
                                    |text| Ok(Cow::Borrowed(text.as_ref())),
                                )
                            })
                        })
                        .await;
                    if this
                        .update(cx, |workspace, cx| {
                            if workspace.semantic_version(cx) == version && workspace.semantic_ui.index.as_ref() != &index {
                                workspace.semantic_ui.index = Arc::new(index);
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                cx.background_executor().timer(Duration::from_millis(500)).await;
            }
        }));
    }

    pub(super) fn navigate_semantic(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        let original = self
            .semantic_ui
            .index
            .sources
            .get(&location.path)
            .map(|source| Arc::clone(&source.text))
            .or_else(|| self.semantic_ui.index.bibliography_sources.get(&location.path).cloned());
        let Some(original) = original else { return };
        if self.document.as_ref().is_some_and(|document| document.path.is_none()) && location.path == self.semantic_path(cx) {
            if self.editor.read(cx).state().text() == original.as_ref() {
                self.editor.update(cx, |editor, cx| {
                    editor.update_state(|state| state.select(location.range.start, location.range.end), cx)
                });
                self.focus_editor(window, cx);
            }
        } else {
            self.start(Intent::OpenSemantic { location, original }, window, cx);
        }
    }

    pub(super) fn go_to_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.document.is_none() {
            return;
        }
        let path = self.semantic_path(cx);
        let state = self.editor.read(cx).state();
        if self
            .semantic_ui
            .index
            .sources
            .get(&path)
            .is_none_or(|source| source.text.as_ref() != state.text())
        {
            self.error = Some("The reference index is updating. Try again when the outline refreshes.".into());
            self.queue_semantics(cx);
        } else {
            let definitions = self.semantic_ui.index.definition_at(&path, state.caret());
            match definitions.as_slice() {
                [location] => self.navigate_semantic(location.clone(), window, cx),
                [] => self.error = Some("No definition is available at the cursor.".into()),
                _ => {
                    self.semantic_ui.visible = true;
                    self.error = Some("This key has multiple definitions. Select a definition in the outline.".into());
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_outline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let index = &self.semantic_ui.index;
        let current = self.semantic_path(cx);
        let state = self.editor.read(cx).state();
        let fresh = index.sources.get(&current).is_some_and(|source| source.text.as_ref() == state.text());
        let mut rows = Vec::new();
        if fresh {
            for (row, entry) in index.outline.iter().enumerate() {
                let mut depth = 0;
                let mut parent = entry.parent;
                while let Some(index) = parent {
                    depth += 1;
                    parent = self.semantic_ui.index.outline[index].parent;
                }
                let location = entry.location.clone();
                rows.push(
                    div().pl(px(depth as f32 * 12.0)).child(
                        Button::new(("outline-entry", row), entry.title.clone())
                            .quiet()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |workspace, _, window, cx| workspace.navigate_semantic(location.clone(), window, cx))),
                    ),
                );
            }
        } else if let Some(document) = self.editor.read(cx).structure() {
            for (row, entry) in document.headings.iter().enumerate() {
                let range = entry.range.clone();
                let version = (state.buffer().id(), state.buffer().revision());
                rows.push(
                    div().pl(px(entry.level.saturating_sub(2) as f32 * 12.0)).child(
                        Button::new(("outline-local", row), entry.title.clone())
                            .quiet()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |workspace, _, window, cx| {
                                let state = workspace.editor.read(cx).state();
                                if (state.buffer().id(), state.buffer().revision()) == version && state.marked_range().is_none() {
                                    workspace
                                        .editor
                                        .update(cx, |editor, cx| editor.update_state(|state| state.select(range.start, range.end), cx));
                                    workspace.focus_editor(window, cx);
                                }
                            })),
                    ),
                );
            }
        }
        let mut definitions = Vec::new();
        if fresh {
            for (key, locations) in index.duplicate_labels() {
                for location in locations {
                    definitions.push((
                        format!("Label {key}: {}", location.path.file_name().unwrap_or_default().to_string_lossy()),
                        location.clone(),
                    ));
                }
            }
            for (key, entries) in index.bibliography.duplicates() {
                for definition in entries {
                    definitions.push((
                        format!("Citation {key}: {}", definition.path.file_name().unwrap_or_default().to_string_lossy()),
                        Location {
                            path: definition.path.clone(),
                            range: definition.entry.key_range.clone(),
                        },
                    ));
                }
            }
        }
        div()
            .id("document-outline")
            .w(px(220.0))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .px(spacing::medium())
            .py(spacing::small())
            .bg(colors().background)
            .child(caption("OUTLINE"))
            .when(!fresh, |view| view.child(caption("Updating outline...")))
            .children(rows)
            .when(fresh, |view| {
                view.child(caption(format!(
                    "{} unresolved references, {} unresolved citations",
                    index.unresolved_references.len(),
                    index.unresolved_citations.len()
                )))
                .children(
                    index
                        .unresolved_references
                        .iter()
                        .chain(&index.unresolved_citations)
                        .enumerate()
                        .map(|(row, (key, location))| {
                            let location = location.clone();
                            Button::new(("unresolved-key", row), format!("Unresolved: {key}"))
                                .quiet()
                                .disabled(self.busy)
                                .on_click(cx.listener(move |workspace, _, window, cx| workspace.navigate_semantic(location.clone(), window, cx)))
                        }),
                )
            })
            .when(!definitions.is_empty(), |view| view.child(caption("DUPLICATE DEFINITIONS")))
            .children(definitions.into_iter().enumerate().map(|(row, (label, location))| {
                Button::new(("duplicate-definition", row), label)
                    .quiet()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |workspace, _, window, cx| workspace.navigate_semantic(location.clone(), window, cx)))
            }))
            .children(
                index
                    .errors
                    .iter()
                    .map(|(path, error)| inline_error(format!("{}: {error}", path.display()))),
            )
    }
}
