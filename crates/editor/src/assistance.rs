use super::*;
use ligature_completion::{Candidate, Target};

actions!(ligature_editor, [Complete, ShowHover]);

#[derive(Default)]
pub(super) struct Assistance {
    serial: u64,
    completion: Option<Target>,
    hover: Option<Target>,
    candidates: Vec<Candidate>,
    selected: usize,
    hover_text: Option<String>,
    labels: Vec<String>,
    citations: Vec<String>,
    last: Option<(ligature_buffer::DocumentId, ligature_buffer::Revision, usize)>,
    stops: Vec<Range<usize>>,
    mirrors: Vec<(usize, Range<usize>)>,
    stop: usize,
    snippet_text: String,
    server_diagnostics: Vec<ligature_diagnostics::Diagnostic>,
    server_ranges: Vec<(Range<usize>, ligature_diagnostics::Severity)>,
    server_revision: Option<ligature_buffer::Revision>,
    pub(super) build_messages: Vec<(Range<usize>, String)>,
    hover_enabled: bool,
}

impl Editor {
    pub(super) fn server_diagnostic_ranges(&self) -> &[(Range<usize>, ligature_diagnostics::Severity)] {
        if self.assistance.server_revision != Some(self.state.buffer().revision()) {
            return &[];
        }
        &self.assistance.server_ranges
    }
    fn target(&mut self) -> Target {
        self.assistance.serial += 1;
        Target {
            document: self.state.buffer().id(),
            revision: self.state.buffer().revision(),
            caret: self.state.caret(),
            serial: self.assistance.serial,
        }
    }

    pub fn target_current(&self, target: Target) -> bool {
        target.document == self.state.buffer().id()
            && target.revision == self.state.buffer().revision()
            && target.caret == self.state.caret()
            && self.state.marked_range().is_none()
    }

    pub fn completion_target(&self) -> Option<Target> {
        self.assistance
            .completion
            .filter(|target| self.target_current(*target) && self.state.selection().is_empty())
    }

    pub fn hover_target(&self) -> Option<Target> {
        self.assistance.hover.filter(|target| self.target_current(*target))
    }

    pub fn set_hover_available(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.assistance.hover_enabled != enabled {
            self.assistance.hover_enabled = enabled;
            cx.notify();
        }
    }

    pub fn hover_available(&self) -> bool {
        !self.read_only && (self.assistance.hover_enabled || !self.diagnostic_messages(self.state.caret()).is_empty())
    }

    pub(super) fn suppress_opaque_assistance(&mut self) {
        if self.syntax_snapshot().is_some_and(|snapshot| {
            snapshot.tokens.iter().any(|token| {
                matches!(token.category, ligature_syntax::Category::Comment | ligature_syntax::Category::Verbatim)
                    && token.range.start < self.state.caret()
                    && token.range.end >= self.state.caret()
            })
        }) {
            self.dismiss_assistance();
        }
    }

    pub fn set_completion_keys(&mut self, labels: Vec<String>, citations: Vec<String>) {
        self.assistance.labels = labels;
        self.assistance.citations = citations;
    }

    pub fn set_server_diagnostics(&mut self, diagnostics: Vec<ligature_diagnostics::Diagnostic>, cx: &mut Context<Self>) {
        let revision = self.state.buffer().revision();
        if self.assistance.server_diagnostics != diagnostics || self.assistance.server_revision != Some(revision) {
            self.assistance.server_ranges = diagnostics
                .iter()
                .filter_map(|diagnostic| Some((self.diagnostic_byte_range(diagnostic)?, diagnostic.severity)))
                .collect();
            self.assistance.server_diagnostics = diagnostics;
            self.assistance.server_revision = Some(revision);
            cx.notify();
        }
    }

    pub fn diagnostic_messages(&self, offset: usize) -> Vec<String> {
        let mut messages = Vec::new();
        if self.diagnostic_revision == Some(self.state.buffer().revision()) {
            messages.extend(
                self.assistance
                    .build_messages
                    .iter()
                    .filter(|(range, _)| range.contains(&offset) || range.end == offset)
                    .map(|(_, message)| message.clone()),
            );
        }
        if self.assistance.server_revision == Some(self.state.buffer().revision()) {
            messages.extend(
                self.assistance
                    .server_diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic
                            .range
                            .as_ref()
                            .and_then(|range| range.byte_range(self.state.text()))
                            .is_some_and(|range| range.contains(&offset) || range.end == offset)
                    })
                    .map(|diagnostic| diagnostic.message.clone()),
            );
        }
        messages
    }

    pub fn publish_completion(&mut self, target: Target, candidates: Vec<Candidate>, cx: &mut Context<Self>) -> bool {
        if self.completion_target() != Some(target) {
            return false;
        }
        let prefix = ligature_completion::context(self.state.text(), target.caret).map_or(String::new(), |context| context.prefix);
        let selected = self
            .assistance
            .candidates
            .get(self.assistance.selected)
            .map(|candidate| candidate.label.clone());
        self.assistance.candidates.extend(candidates);
        self.assistance.candidates = ligature_completion::rank(std::mem::take(&mut self.assistance.candidates), &prefix);
        self.assistance.selected = selected
            .and_then(|label| self.assistance.candidates.iter().position(|candidate| candidate.label == label))
            .unwrap_or(0);
        cx.notify();
        true
    }

    pub fn publish_hover(&mut self, target: Target, text: Option<String>, cx: &mut Context<Self>) -> bool {
        if self.hover_target() != Some(target) {
            return false;
        }
        let mut messages = self.diagnostic_messages(target.caret);
        if let Some(text) = text {
            messages.push(text);
        }
        self.assistance.hover_text = (!messages.is_empty()).then(|| messages.join("\n\n"));
        cx.notify();
        true
    }

    pub fn dismiss_assistance(&mut self) {
        self.assistance.completion = None;
        self.assistance.hover = None;
        self.assistance.candidates.clear();
        self.assistance.hover_text = None;
    }

    fn request_completion(&mut self, explicit: bool, cx: &mut Context<Self>) {
        self.dismiss_assistance();
        if self.read_only
            || self.language == ligature_syntax::Language::Plain
            || self.state.marked_range().is_some()
            || !self.state.selection().is_empty()
        {
            return;
        }
        if self.syntax_snapshot().is_some_and(|snapshot| {
            snapshot.tokens.iter().any(|token| {
                matches!(token.category, ligature_syntax::Category::Comment | ligature_syntax::Category::Verbatim)
                    && token.range.start < self.state.caret()
                    && token.range.end >= self.state.caret()
            })
        }) {
            return;
        }
        let context = ligature_completion::context(self.state.text(), self.state.caret());
        if !explicit && context.is_none() && self.language != ligature_syntax::Language::Bibliography {
            return;
        }
        if let Some(context) = context {
            self.assistance.candidates = ligature_completion::local(
                &context,
                self.assistance.labels.iter().map(String::as_str),
                self.assistance.citations.iter().map(String::as_str),
            );
        }
        let target = self.target();
        self.assistance.completion = Some(target);
        self.assistance.selected = 0;
        cx.notify();
    }

    pub(super) fn complete(&mut self, _: &Complete, _: &mut Window, cx: &mut Context<Self>) {
        self.request_completion(true, cx);
    }

    pub(super) fn show_hover(&mut self, _: &ShowHover, _: &mut Window, cx: &mut Context<Self>) {
        if !self.hover_available() || self.state.marked_range().is_some() {
            return;
        }
        self.dismiss_assistance();
        let target = self.target();
        self.assistance.hover = Some(target);
        self.publish_hover(target, None, cx);
    }

    pub(super) fn refresh_assistance(&mut self, cx: &mut Context<Self>) {
        let version = (self.state.buffer().id(), self.state.buffer().revision(), self.state.caret());
        if self.assistance.last == Some(version) && self.state.marked_range().is_none() {
            return;
        }
        let edited = self.assistance.last.is_some_and(|last| last.0 == version.0 && last.1 != version.1);
        self.assistance.last = Some(version);
        self.dismiss_assistance();
        if !self.assistance.stops.is_empty() {
            if edited && self.adjust_snippet() {
                return;
            }
            self.assistance.stops.clear();
        }
        if edited {
            self.request_completion(false, cx);
        }
    }

    fn adjust_snippet(&mut self) -> bool {
        let active = self.assistance.stops[self.assistance.stop].clone();
        let old = &self.assistance.snippet_text;
        let new = self.state.text();
        if old.get(active.clone()).is_none() {
            return false;
        }
        if !new.starts_with(&old[..active.start]) || !new.ends_with(&old[active.end..]) {
            return false;
        }
        let delta = new.len() as isize - old.len() as isize;
        self.assistance.stops[self.assistance.stop].end = active.end.saturating_add_signed(delta);
        for (index, range) in self.assistance.stops.iter_mut().enumerate() {
            if index == self.assistance.stop {
                continue;
            }
            if range.start >= active.end && range.start != active.start {
                range.start = range.start.saturating_add_signed(delta);
                range.end = range.end.saturating_add_signed(delta);
            } else if range.start >= active.start && range.end <= active.end {
                let end = active.end.saturating_add_signed(delta);
                *range = end..end;
            } else if range.start <= active.start && range.end >= active.end {
                range.end = range.end.saturating_add_signed(delta);
            }
        }
        for (_, range) in &mut self.assistance.mirrors {
            if range.start >= active.end {
                range.start = range.start.saturating_add_signed(delta);
                range.end = range.end.saturating_add_signed(delta);
            }
        }
        self.assistance.snippet_text = new.into();
        true
    }

    pub(super) fn replace_snippet(&mut self, selection: Range<usize>, insertion: &str) -> bool {
        if self.state.marked_range().is_some() || self.assistance.stops.is_empty() {
            return false;
        }
        let active = self.assistance.stops[self.assistance.stop].clone();
        if selection.start < active.start
            || selection.end > active.end
            || !self.assistance.mirrors.iter().any(|(index, _)| *index == self.assistance.stop)
        {
            return false;
        }
        let Some(original) = self.state.text().get(active.clone()) else {
            return false;
        };
        let mut value = original.to_owned();
        value.replace_range(selection.start - active.start..selection.end - active.start, insertion);
        let mut candidate = Candidate {
            label: String::new(),
            detail: String::new(),
            filter: String::new(),
            sort: String::new(),
            edit: ligature_completion::Edit {
                range: active.clone(),
                text: value.clone(),
            },
            additional: self
                .assistance
                .mirrors
                .iter()
                .filter(|(index, _)| *index == self.assistance.stop)
                .map(|(_, range)| ligature_completion::Edit {
                    range: range.clone(),
                    text: value.clone(),
                })
                .collect(),
            snippet: false,
            provider: ligature_completion::Provider::Local,
        };
        let Ok(plan) = ligature_completion::plan(self.state.text(), &candidate) else {
            return false;
        };
        candidate.additional.push(candidate.edit);
        candidate.additional.sort_by_key(|edit| edit.range.start);
        let shift = |range: &mut Range<usize>| {
            let mut delta = 0_isize;
            for edit in &candidate.additional {
                if *range == edit.range {
                    range.start = range.start.saturating_add_signed(delta);
                    range.end = range.start + value.len();
                    return;
                }
                if edit.range.end <= range.start {
                    delta += value.len() as isize - edit.range.len() as isize;
                }
            }
            range.start = range.start.saturating_add_signed(delta);
            range.end = range.end.saturating_add_signed(delta);
        };
        for range in &mut self.assistance.stops {
            shift(range);
        }
        for (_, range) in &mut self.assistance.mirrors {
            shift(range);
        }
        let caret = self.assistance.stops[self.assistance.stop].start + selection.start - active.start + insertion.len();
        self.state.replace(plan.edit.range, &plan.edit.text, false);
        self.state.move_to(caret, false);
        self.assistance.last = Some((self.state.buffer().id(), self.state.buffer().revision(), self.state.caret()));
        self.assistance.snippet_text = self.state.text().into();
        true
    }

    pub fn accept_completion(&mut self, cx: &mut Context<Self>) -> bool {
        if self.completion_target().is_none() {
            self.dismiss_assistance();
            return false;
        }
        let Some(candidate) = self.assistance.candidates.get(self.assistance.selected) else {
            return false;
        };
        let Ok(plan) = ligature_completion::plan(self.state.text(), candidate) else {
            self.dismiss_assistance();
            return false;
        };
        self.state.replace(plan.edit.range, &plan.edit.text, false);
        self.assistance.stops = plan.placeholders;
        self.assistance.mirrors = plan.mirrors;
        self.assistance.stop = 0;
        if let Some(range) = self.assistance.stops.first() {
            self.state.select(range.start, range.end);
        } else {
            self.state.move_to(plan.caret, false);
        }
        self.assistance.snippet_text = self.state.text().into();
        self.assistance.last = Some((self.state.buffer().id(), self.state.buffer().revision(), self.state.caret()));
        self.dismiss_assistance();
        self.preferred_x = None;
        self.reveal_caret = true;
        self.refresh_search();
        self.queue_parse(cx);
        cx.notify();
        true
    }

    pub(super) fn assistance_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt || self.state.marked_range().is_some() {
            return false;
        }
        if matches!(key, "backspace" | "delete") && !self.assistance.stops.is_empty() {
            let mut range = self.state.selection();
            if range.is_empty() {
                if key == "backspace" {
                    range.start = self.state.text()[..range.start]
                        .grapheme_indices(true)
                        .next_back()
                        .map_or(0, |(offset, _)| offset);
                } else {
                    range.end += self.state.text()[range.end..].graphemes(true).next().map_or(0, str::len);
                }
            }
            if self.replace_snippet(range, "") {
                self.changed(window, cx);
                cx.stop_propagation();
                return true;
            }
        }
        if key == "escape" && (self.assistance.completion.is_some() || self.assistance.hover.is_some() || !self.assistance.stops.is_empty()) {
            self.dismiss_assistance();
            self.assistance.stops.clear();
        } else if !self.assistance.candidates.is_empty() && self.completion_target().is_some() {
            match key {
                "up" => {
                    self.assistance.selected = (self.assistance.selected + self.assistance.candidates.len() - 1) % self.assistance.candidates.len()
                }
                "down" => self.assistance.selected = (self.assistance.selected + 1) % self.assistance.candidates.len(),
                "enter" | "tab" => {
                    self.accept_completion(cx);
                    window.focus(&self.focus, cx);
                }
                _ => return false,
            }
        } else if key == "tab" && !self.assistance.stops.is_empty() {
            if modifiers.shift {
                self.assistance.stop = self.assistance.stop.saturating_sub(1);
            } else {
                self.assistance.stop += 1;
            }
            if let Some(range) = self.assistance.stops.get(self.assistance.stop) {
                self.state.select(range.start, range.end);
            } else {
                self.assistance.stops.clear();
            }
            self.assistance.last = Some((self.state.buffer().id(), self.state.buffer().revision(), self.state.caret()));
            self.reveal_caret = true;
        } else {
            return false;
        }
        cx.stop_propagation();
        cx.notify();
        true
    }

    pub(super) fn render_assistance(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let line = self.state.line_index(self.state.caret());
        let x = self.gutter + self.shape(line, window).x_for_byte(self.state.caret()) - self.scroll.x;
        let y = self.line_height * (line + 1) as f32 - self.scroll.y;
        let height = if self.completion_target().is_some() {
            px(24.0) * self.assistance.candidates.len().min(8) as f32 + px(2.0)
        } else {
            px(220.0)
        };
        let top = if y + height <= self.bounds.size.height {
            y
        } else {
            (y - self.line_height - height).max(px(0.0))
        };
        let mut popup = div()
            .absolute()
            .left(x.max(px(0.0)).min((self.bounds.size.width - px(320.0)).max(px(0.0))))
            .top(top)
            .w(px(320.0))
            .max_h(height.min(self.bounds.size.height))
            .overflow_hidden()
            .bg(colors().background)
            .text_color(colors().text)
            .text_size(typography::control_size())
            .line_height(px(24.0))
            .cursor(CursorStyle::Arrow);
        if self.completion_target().is_some() && !self.assistance.candidates.is_empty() {
            popup = popup.border_1().border_color(colors().border).flex().flex_col();
            let start = self.assistance.selected.saturating_sub(7);
            for (index, candidate) in self.assistance.candidates.iter().enumerate().skip(start).take(8) {
                popup = popup.child(
                    div()
                        .id(("completion", index))
                        .px(px(8.0))
                        .overflow_hidden()
                        .when(index == self.assistance.selected, |row| row.bg(colors().selection))
                        .child(candidate.label.clone())
                        .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
                        .on_click(cx.listener(move |editor, _, window, cx| {
                            editor.assistance.selected = index;
                            editor.accept_completion(cx);
                            window.focus(&editor.focus, cx);
                        })),
                );
            }
        } else if self.hover_target().is_some()
            && let Some(text) = &self.assistance.hover_text
        {
            popup = popup.border_1().border_color(colors().border).p(px(8.0)).child(text.clone());
        }
        popup
    }
}
