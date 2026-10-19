use std::ops::Range;

use super::EditorState;

pub(super) struct AutoCloser {
    range: Range<usize>,
    opening: &'static str,
}

impl EditorState {
    pub(super) fn type_delimiter(&mut self, text: &str) -> bool {
        if text.len() != 1 || !text.is_ascii() {
            return false;
        }
        let selection = self.selection();
        let caret = selection.start;
        let escaped = self.text()[..caret].bytes().rev().take_while(|&byte| byte == b'\\').count() % 2 == 1;

        if selection.is_empty() {
            if escaped
                && let Some(index) = self
                    .auto_closers
                    .iter()
                    .position(|closer| closer.range.start == caret && self.text()[closer.range.clone()].strip_prefix('\\') == Some(text))
            {
                let closer = self.auto_closers.remove(index);
                self.replace(caret - 1..caret, "", false);
                self.select(closer.range.end - 1, closer.range.end - 1);
                return true;
            }
            if text == "$"
                && !escaped
                && let Some(index) = self
                    .auto_closers
                    .iter()
                    .position(|closer| closer.range.start == caret && closer.opening == "$" && self.text()[..caret].ends_with('$'))
            {
                self.auto_closers.remove(index);
                self.replace(caret..caret + 1, "$$$", false);
                self.select(caret + 1, caret + 1);
                self.auto_closers.push(AutoCloser {
                    range: caret + 1..caret + 3,
                    opening: "$$",
                });
                return true;
            }
            if let Some(index) = self
                .auto_closers
                .iter()
                .position(|closer| closer.range.start == caret && text != "\\" && !escaped && self.text()[closer.range.clone()].starts_with(text))
            {
                let closer = &mut self.auto_closers[index];
                closer.range.start += text.len();
                if closer.range.is_empty() {
                    self.auto_closers.remove(index);
                }
                self.select(caret + text.len(), caret + text.len());
                return true;
            }
        }

        let (opening, closing) = match (text, escaped) {
            ("$", false) => ("$", "$"),
            ("{", false) => ("{", "}"),
            ("[", false) => ("[", "]"),
            ("(", false) => ("(", ")"),
            ("{", true) => ("\\{", "\\}"),
            ("[", true) => ("\\[", "\\]"),
            ("(", true) => ("\\(", "\\)"),
            _ => return false,
        };
        if selection.is_empty()
            && !self.auto_closers.iter().any(|closer| closer.range.start == caret)
            && self.text()[caret..]
                .chars()
                .next()
                .is_some_and(|next| !next.is_whitespace() && !matches!(next, ')' | ']' | '}' | '$' | ',' | '.' | ';' | ':' | '!' | '?'))
        {
            return false;
        }
        let insertion = format!("{text}{}{closing}", self.selected_text());
        let end = caret + text.len() + selection.len();
        let reversed = self.caret < self.anchor;
        self.replace(selection, &insertion, false);
        self.select(
            if reversed { end } else { caret + text.len() },
            if reversed { caret + text.len() } else { end },
        );
        self.auto_closers.push(AutoCloser {
            range: end..end + closing.len(),
            opening,
        });
        true
    }

    pub(super) fn adjust_auto_closers(&mut self, range: Range<usize>, inserted: usize) {
        self.auto_closers.retain_mut(|closer| {
            if range.end <= closer.range.start {
                closer.range.start = closer.range.start - range.len() + inserted;
                closer.range.end = closer.range.end - range.len() + inserted;
                true
            } else {
                range.start >= closer.range.end
            }
        });
    }

    pub(super) fn delete_empty_pair(&mut self) -> bool {
        let Some(index) = self
            .auto_closers
            .iter()
            .position(|closer| closer.range.start == self.caret && self.text()[..self.caret].ends_with(closer.opening))
        else {
            return false;
        };
        let closer = self.auto_closers.remove(index);
        self.replace(self.caret - closer.opening.len()..closer.range.end, "", false);
        true
    }
}
