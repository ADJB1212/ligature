use gpui::{
    AppContext, ClipboardItem, EntityInputHandler, Focusable, InputEvent, KeyDownEvent, Keystroke, MouseButton, ScrollDelta, ScrollWheelEvent,
    TestAppContext, VisualTestContext, point, px, size,
};
use ligature_buffer::Buffer;
use ligature_editor::{Editor, init};

fn shortcut(key: &str) -> String {
    format!("{}-{key}", if cfg!(target_os = "macos") { "cmd" } else { "ctrl" })
}

#[test]
fn native_typing_clipboard_shortcuts_and_history_work_in_focused_editor() {
    let mut cx = TestAppContext::single();
    cx.update(init);
    let window = cx.add_window(|_, cx| Editor::new(Buffer::from_text("old"), cx));
    let editor = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), &shortcut("a"));
    cx.simulate_input(window.into(), "e\u{301}👩🏽‍💻");
    assert_eq!(editor.read_with(&cx, |editor, _| editor.state().text().to_owned()), "e\u{301}👩🏽‍💻");
    cx.simulate_keystrokes(window.into(), &shortcut("a"));
    cx.simulate_keystrokes(window.into(), &shortcut("c"));
    cx.update(|cx| assert_eq!(cx.read_from_clipboard().unwrap().text().as_deref(), Some("e\u{301}👩🏽‍💻")));
    cx.simulate_keystrokes(window.into(), &shortcut("x"));
    assert!(editor.read_with(&cx, |editor, _| editor.state().text().is_empty()));
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("one\ntwo".to_owned())));
    cx.simulate_keystrokes(window.into(), &shortcut("v"));
    assert_eq!(editor.read_with(&cx, |editor, _| editor.state().text().to_owned()), "one\ntwo");
    cx.simulate_keystrokes(window.into(), &shortcut("z"));
    assert!(editor.read_with(&cx, |editor, _| editor.state().text().is_empty()));
    cx.simulate_keystrokes(window.into(), &shortcut("shift-z"));
    assert_eq!(editor.read_with(&cx, |editor, _| editor.state().text().to_owned()), "one\ntwo");
    cx.update_window(window.into(), |_, window, cx| assert!(editor.focus_handle(cx).is_focused(window)))
        .unwrap();
}

#[test]
fn ime_utf16_ranges_commit_cancel_and_report_caret_bounds() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, cx| Editor::new(Buffer::from_text("a😀z"), cx));
    let editor = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            editor.replace_and_mark_text_in_range(Some(1..3), "日本", Some(1..2), window, cx);
            assert_eq!(editor.marked_text_range(window, cx), Some(1..3));
            assert_eq!(editor.selected_text_range(false, window, cx).unwrap().range, 2..3);
            let mut actual = None;
            assert_eq!(editor.text_for_range(1..3, &mut actual, window, cx).as_deref(), Some("日本"));
            assert_eq!(actual, Some(1..3));
            editor.replace_text_in_range(None, "日本語", window, cx);
            assert_eq!(editor.state().text(), "a日本語z");
        });
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            let bounds = editor.bounds_for_range(4..4, Default::default(), window, cx).unwrap();
            assert!(bounds.size.height > px(0.0));
            assert_eq!(editor.character_index_for_point(bounds.origin, window, cx), Some(4));
            editor.update_state(|state| state.undo(), cx);
            assert_eq!(editor.state().text(), "a😀z");
            editor.replace_and_mark_text_in_range(Some(1..3), "draft", None, window, cx);
        });
        window.dispatch_event(
            KeyDownEvent {
                keystroke: Keystroke::parse("escape").unwrap(),
                is_held: false,
                prefer_character_input: false,
            }
            .to_platform_input(),
            cx,
        );
        assert_eq!(editor.read(cx).state().text(), "a😀z");
        assert!(!editor.read(cx).state().buffer().is_dirty());
    })
    .unwrap();
}

#[test]
fn mouse_hit_testing_selection_scroll_and_resize_use_current_layout() {
    let mut app = TestAppContext::single();
    let text = format!("\tabc e\u{301}👩🏽‍💻\n{}\n{}", "long ".repeat(80), "line\n".repeat(80));
    let window = app.add_window(|_, cx| Editor::new(Buffer::from_text(text), cx));
    let editor = window.root(&mut app).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), &app);
    cx.simulate_resize(size(px(400.0), px(200.0)));
    cx.update(|window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
    });
    let (first, last) = cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            let first = editor.bounds_for_range(1..1, Default::default(), window, cx).unwrap().origin;
            let last = editor.bounds_for_range(4..4, Default::default(), window, cx).unwrap().origin;
            (point(first.x, first.y + px(5.0)), point(last.x, last.y + px(5.0)))
        })
    });
    cx.simulate_mouse_down(first, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(last, MouseButton::Left, Default::default());
    cx.simulate_mouse_up(last, MouseButton::Left, Default::default());
    assert_eq!(editor.read_with(&cx, |editor, _| editor.state().selected_text().to_owned()), "abc");
    cx.simulate_mouse_down(first, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(point(px(500.0), first.y), MouseButton::Left, Default::default());
    cx.simulate_mouse_up(point(px(500.0), first.y), MouseButton::Left, Default::default());
    assert_eq!(
        editor.read_with(&cx, |editor, _| editor.state().selected_text().to_owned()),
        "abc e\u{301}👩🏽‍💻"
    );
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui::MouseDownEvent {
                button: MouseButton::Left,
                position: first,
                modifiers: Default::default(),
                click_count: 2,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
    });
    assert_eq!(editor.read_with(&cx, |editor, _| editor.state().selected_text().to_owned()), "abc");
    cx.simulate_mouse_up(first, MouseButton::Left, Default::default());
    cx.update(|window, cx| {
        window.dispatch_event(
            ScrollWheelEvent {
                position: first,
                delta: ScrollDelta::Pixels(point(px(0.0), px(-300.0))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            let top = editor.character_index_for_point(first, window, cx).unwrap();
            assert!(top > 20);
            editor.update_state(|state| state.move_cursor(ligature_editor::Movement::DocumentEnd, false), cx);
        });
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            let end = editor.state().text().encode_utf16().count();
            let bounds = editor.bounds_for_range(end..end, Default::default(), window, cx).unwrap();
            assert!(bounds.top() >= px(0.0));
            assert!(bounds.bottom() <= px(200.0));
        });
    });
    cx.simulate_resize(size(px(250.0), px(100.0)));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            editor.set_font(None, px(20.0), cx);
        });
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            let end = editor.state().text().encode_utf16().count();
            let bounds = editor.bounds_for_range(end..end, Default::default(), window, cx).unwrap();
            assert_eq!(bounds.size.height, px(30.0));
            assert!(bounds.bottom() <= px(100.0));
        });
    });
}

#[test]
fn horizontal_scroll_keeps_the_gutter_fixed_and_keyboard_movement_reveals_caret() {
    let mut app = TestAppContext::single();
    let window = app.add_window(|_, cx| Editor::new(Buffer::from_text("wide ".repeat(150)), cx));
    let editor = window.root(&mut app).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), &app);
    cx.simulate_resize(size(px(300.0), px(160.0)));
    cx.update(|window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            editor.update_state(|state| state.move_cursor(ligature_editor::Movement::DocumentEnd, false), cx);
        });
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            let end = editor.state().text().len();
            let bounds = editor.bounds_for_range(end..end, Default::default(), window, cx).unwrap();
            assert!(bounds.right() <= px(300.0));
            assert!(bounds.left() > px(200.0));
        });
        window.dispatch_event(
            ScrollWheelEvent {
                position: point(px(200.0), px(40.0)),
                delta: ScrollDelta::Pixels(point(px(200.0), px(0.0))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            let end = editor.state().text().len();
            let bounds = editor.bounds_for_range(end..end, Default::default(), window, cx).unwrap();
            assert!(bounds.left() > px(300.0));
            assert!(editor.character_index_for_point(point(px(10.0), px(5.0)), window, cx).unwrap() > 0);
        });
    });
    cx.simulate_keystrokes("left");
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            let caret = editor.state().caret();
            let bounds = editor.bounds_for_range(caret..caret, Default::default(), window, cx).unwrap();
            assert!(bounds.right() <= px(300.0));
        });
    });
    cx.simulate_keystrokes("home");
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        editor.update(cx, |editor, cx| {
            let bounds = editor.bounds_for_range(0..0, Default::default(), window, cx).unwrap();
            assert!(bounds.left() > px(10.0));
            assert!(bounds.left() < px(100.0));
        });
    });
}

#[test]
fn tab_newline_and_vertical_keys_use_editor_commands() {
    let mut app = TestAppContext::single();
    app.update(init);
    let window = app.add_window(|_, cx| Editor::new(Buffer::from_text("abcdef\nx\nabcdef"), cx));
    let editor = window.root(&mut app).unwrap();
    app.update_window(window.into(), |_, window, cx| {
        window.focus(&editor.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    app.simulate_keystrokes(window.into(), "right right right right right down down");
    assert_eq!(editor.read_with(&app, |editor, _| editor.state().caret()), 14);
    app.simulate_keystrokes(window.into(), "end tab enter");
    assert_eq!(
        editor.read_with(&app, |editor, _| editor.state().text().to_owned()),
        "abcdef\nx\nabcdef  \n"
    );
    app.update_window(window.into(), |_, window, cx| assert!(editor.focus_handle(cx).is_focused(window)))
        .unwrap();
}
