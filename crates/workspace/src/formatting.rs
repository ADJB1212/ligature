use std::path::Path;

use gpui::Context;
use ligature_formatter::{Indentation, Options};

use crate::Workspace;

#[cfg(test)]
#[path = "../tests/unit/formatting.rs"]
mod tests;

impl Workspace {
    pub(super) fn format_document(&mut self, cx: &mut Context<Self>) {
        let Some(document) = &self.document else { return };
        let path = document.path.clone();
        self.format_editor(path.as_deref(), cx);
    }

    pub(super) fn format_editor(&mut self, path: Option<&Path>, cx: &mut Context<Self>) {
        if path.is_some_and(|path| {
            !path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| {
                ["tex", "sty", "cls", "ltx"]
                    .iter()
                    .any(|supported| extension.eq_ignore_ascii_case(supported))
            })
        }) {
            return;
        }
        let options = Options {
            indentation: match self.settings.indentation.style {
                ligature_settings::Indentation::Spaces => Indentation::Spaces(self.settings.indentation.width),
                ligature_settings::Indentation::Tabs => Indentation::Tabs,
            },
            ..Options::default()
        };
        self.editor.update(cx, |editor, cx| {
            editor.dismiss_assistance();
            editor.update_state(
                |state| {
                    state.commit_composition();
                    let formatted = ligature_formatter::format_with_options(state.text(), &options);
                    let start = ligature_syntax::common_prefix(state.text(), &formatted);
                    if start == state.text().len() && start == formatted.len() {
                        return;
                    }
                    let suffix = ligature_syntax::common_suffix(&state.text()[start..], &formatted[start..]);
                    let old_end = state.text().len() - suffix;
                    let new_end = formatted.len() - suffix;
                    let map = |offset: usize| {
                        let mut mapped = if offset <= start {
                            offset
                        } else if offset >= old_end {
                            new_end + offset - old_end
                        } else {
                            offset.min(new_end)
                        };
                        while !formatted.is_char_boundary(mapped) {
                            mapped -= 1;
                        }
                        mapped
                    };
                    let anchor = map(state.anchor());
                    let caret = map(state.caret());
                    state.replace(start..old_end, &formatted[start..new_end], false);
                    state.select(anchor, caret);
                },
                cx,
            );
        });
    }
}
