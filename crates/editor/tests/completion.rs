use gpui::{AppContext, Focusable, TestAppContext};
use ligature_buffer::Buffer;
use ligature_completion::{Candidate, Edit, Provider};
use ligature_editor::{Editor, init};

fn candidate(range: std::ops::Range<usize>, insertion: &str) -> Candidate {
    Candidate {
        label: "section".into(),
        detail: String::new(),
        filter: "section".into(),
        sort: "section".into(),
        edit: Edit {
            range,
            text: insertion.into(),
        },
        additional: Vec::new(),
        snippet: false,
        provider: Provider::Server,
    }
}

#[test]
fn typing_completes_locally_and_acceptance_is_one_undo_transaction() {
    let mut cx = TestAppContext::single();
    cx.update(init);
    let window = cx.add_window(|_, cx| Editor::new(Buffer::new(), cx));
    let editor = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_input(window.into(), "\\sec");
    assert!(editor.read_with(&cx, |editor, _| editor.completion_target().is_some()));
    cx.simulate_keystrokes(window.into(), "enter");
    assert_eq!(editor.read_with(&cx, |editor, _| editor.state().text().to_owned()), "\\section");
    editor.update(&mut cx, |editor, cx| {
        editor.update_state(|state| state.undo(), cx);
        assert_eq!(editor.state().text(), "\\sec");
    });
}

#[test]
fn additional_edits_and_snippet_stops_apply_together_and_undo_together() {
    let mut cx = TestAppContext::single();
    cx.update(init);
    let window = cx.add_window(|_, cx| Editor::new(Buffer::from_text("head\n\\sec"), cx));
    let editor = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        editor.update(cx, |editor, cx| {
            editor.update_state(|state| state.move_to(state.text().len(), false), cx);
        });
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "ctrl-space");
    editor.update(&mut cx, |editor, cx| {
        let target = editor.completion_target().unwrap();
        let mut candidate = candidate(6..9, "section{${1:Title}}$0");
        candidate.snippet = true;
        candidate.additional.push(Edit {
            range: 0..0,
            text: "% added\n".into(),
        });
        assert!(editor.publish_completion(target, vec![candidate], cx));
        assert!(editor.accept_completion(cx));
        assert_eq!(editor.state().text(), "% added\nhead\n\\section{Title}");
        assert_eq!(editor.state().selected_text(), "Title");
    });
    cx.simulate_input(window.into(), "X");
    cx.simulate_keystrokes(window.into(), "tab");
    editor.read_with(&cx, |editor, _| {
        assert!(editor.state().selection().is_empty());
        assert_eq!(editor.state().caret(), editor.state().text().len());
    });
    editor.update(&mut cx, |editor, cx| {
        editor.update_state(|state| state.undo(), cx);
        assert_eq!(editor.state().text(), "% added\nhead\n\\section{Title}");
        editor.update_state(|state| state.undo(), cx);
        assert_eq!(editor.state().text(), "head\n\\sec");
    });
}

#[test]
fn delayed_results_are_rejected_after_edits_movement_dismissal_and_buffer_replacement() {
    let mut cx = TestAppContext::single();
    cx.update(init);
    let window = cx.add_window(|_, cx| Editor::new(Buffer::from_text("\\sec"), cx));
    let editor = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        editor.update(cx, |editor, cx| {
            editor.update_state(|state| state.move_to(4, false), cx);
        });
        window.draw(cx).clear(cx);
    })
    .unwrap();
    for action in ["edit", "move", "dismiss", "replace"] {
        editor.update(&mut cx, |editor, cx| {
            editor.set_buffer(Buffer::from_text("\\sec"), cx);
            editor.update_state(|state| state.move_to(4, false), cx);
        });
        cx.simulate_keystrokes(window.into(), "ctrl-space");
        let target = editor.read_with(&cx, |editor, _| editor.completion_target().unwrap());
        editor.update(&mut cx, |editor, cx| {
            match action {
                "edit" => {
                    editor.update_state(|state| state.insert("t", true), cx);
                }
                "move" => {
                    editor.update_state(|state| state.move_to(0, false), cx);
                }
                "dismiss" => editor.dismiss_assistance(),
                "replace" => editor.set_buffer(Buffer::from_text("\\sec"), cx),
                _ => unreachable!(),
            }
            assert!(!editor.publish_completion(target, vec![candidate(1..4, "section")], cx));
        });
    }
}

#[test]
fn completion_keyboard_navigation_escape_and_focus_loss_preserve_text() {
    let mut cx = TestAppContext::single();
    cx.update(init);
    let window = cx.add_window(|_, cx| Editor::new(Buffer::new(), cx));
    let editor = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_input(window.into(), "\\s");
    cx.simulate_keystrokes(window.into(), "down");
    cx.simulate_keystrokes(window.into(), "escape");
    assert!(editor.read_with(&cx, |editor, _| editor.completion_target().is_none()));
    assert_eq!(editor.read_with(&cx, |editor, _| editor.state().text().to_owned()), "\\s");
    cx.simulate_keystrokes(window.into(), "ctrl-space");
    cx.update_window(window.into(), |_, window, cx| {
        window.blur();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    assert!(editor.read_with(&cx, |editor, _| editor.completion_target().is_none()));
}

#[test]
fn server_and_build_diagnostic_messages_coexist_and_stale_messages_disappear() {
    use ligature_diagnostics::{Diagnostic, Provider, Severity, SourceRange};
    let mut cx = TestAppContext::single();
    cx.update(init);
    let window = cx.add_window(|_, cx| Editor::new(Buffer::from_text("broken"), cx));
    let editor = window.root(&mut cx).unwrap();
    editor.update(&mut cx, |editor, cx| {
        let diagnostic = |provider, message: &str| Diagnostic {
            severity: Severity::Error,
            provider,
            message: message.into(),
            path: None,
            range: Some(SourceRange::Lines { start: 1, end: 1 }),
        };
        let build = diagnostic(Provider::Build(Default::default()), "build problem");
        editor.set_diagnostics([&build], false, cx);
        editor.set_server_diagnostics(vec![diagnostic(Provider::LanguageServer("Texlab".into()), "server problem")], cx);
        assert_eq!(editor.diagnostic_messages(0), ["build problem", "server problem"]);
        editor.set_server_diagnostics(Vec::new(), cx);
        assert_eq!(editor.diagnostic_messages(0), ["build problem"]);
        editor.update_state(|state| state.insert("edited", false), cx);
        assert!(editor.diagnostic_messages(0).is_empty());
    });
}

#[test]
fn texlab_environment_snippet_mirrors_names_and_undo_restores_both_occurrences() {
    let mut cx = TestAppContext::single();
    cx.update(init);
    let window = cx.add_window(|_, cx| Editor::new(Buffer::from_text("\\be"), cx));
    let editor = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        editor.update(cx, |editor, cx| {
            editor.update_state(|state| state.move_to(3, false), cx);
        });
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "ctrl-space");
    editor.update(&mut cx, |editor, cx| {
        let mut candidate = candidate(1..3, "begin{$1}\n\t$0\n\\end{$1}");
        candidate.label = "begin".into();
        candidate.filter = "begin".into();
        candidate.sort = "begin".into();
        candidate.snippet = true;
        editor.publish_completion(editor.completion_target().unwrap(), vec![candidate], cx);
        editor.accept_completion(cx);
    });
    cx.simulate_input(window.into(), "align");
    editor.read_with(&cx, |editor, _| assert_eq!(editor.state().text(), "\\begin{align}\n\t\n\\end{align}"));
    cx.simulate_keystrokes(window.into(), "backspace");
    editor.read_with(&cx, |editor, _| assert_eq!(editor.state().text(), "\\begin{alig}\n\t\n\\end{alig}"));
    editor.update(&mut cx, |editor, cx| {
        editor.update_state(|state| state.undo(), cx);
        assert_eq!(editor.state().text(), "\\begin{align}\n\t\n\\end{align}");
    });
}

#[test]
fn hover_results_are_rejected_after_target_changes_and_escape() {
    let mut cx = TestAppContext::single();
    cx.update(init);
    let window = cx.add_window(|_, cx| Editor::new(Buffer::from_text("text"), cx));
    let editor = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |editor, cx| editor.set_hover_available(true, cx));
        window.focus(&editor.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "shift-f1");
    let target = editor.read_with(&cx, |editor, _| editor.hover_target().unwrap());
    editor.update(&mut cx, |editor, cx| {
        assert!(editor.publish_hover(target, Some("hover information".into()), cx));
        editor.update_state(|state| state.move_to(1, false), cx);
        assert!(!editor.publish_hover(target, Some("obsolete".into()), cx));
    });
    cx.simulate_keystrokes(window.into(), "shift-f1");
    let target = editor.read_with(&cx, |editor, _| editor.hover_target().unwrap());
    cx.simulate_keystrokes(window.into(), "escape");
    editor.update(&mut cx, |editor, cx| {
        assert!(!editor.publish_hover(target, Some("obsolete".into()), cx));
    });
}

#[test]
fn server_publication_preserves_the_keyboard_selected_local_candidate() {
    let mut cx = TestAppContext::single();
    cx.update(init);
    let window = cx.add_window(|_, cx| Editor::new(Buffer::new(), cx));
    let editor = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_input(window.into(), "\\s");
    cx.simulate_keystrokes(window.into(), "down");
    editor.update(&mut cx, |editor, cx| {
        let target = editor.completion_target().unwrap();
        editor.publish_completion(target, vec![candidate(1..2, "section")], cx);
    });
    cx.simulate_keystrokes(window.into(), "enter");
    assert_eq!(editor.read_with(&cx, |editor, _| editor.state().text().to_owned()), "\\sigma");
}

#[test]
fn replacing_a_nested_snippet_default_keeps_later_stops_within_the_document() {
    let mut cx = TestAppContext::single();
    cx.update(init);
    let window = cx.add_window(|_, cx| Editor::new(Buffer::from_text("\\sec"), cx));
    let editor = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        editor.update(cx, |editor, cx| {
            editor.update_state(|state| state.move_to(4, false), cx);
        });
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "ctrl-space");
    editor.update(&mut cx, |editor, cx| {
        let mut candidate = candidate(1..4, "section{${1:${2:Inner}}}$0");
        candidate.snippet = true;
        editor.publish_completion(editor.completion_target().unwrap(), vec![candidate], cx);
        editor.accept_completion(cx);
    });
    cx.simulate_input(window.into(), "X");
    cx.simulate_keystrokes(window.into(), "tab");
    cx.simulate_input(window.into(), "Y");
    assert_eq!(editor.read_with(&cx, |editor, _| editor.state().text().to_owned()), "\\section{XY}");
}
