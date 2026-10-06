use gpui::{
    AppContext, Context, Entity, EntityInputHandler, Focusable, InputEvent, IntoElement, KeyUpEvent, Keystroke, MouseButton, Render, TestAppContext,
    VisualTestContext, Window, div, point, prelude::*, px, size,
};
use ligature_ui::{Button, SplitPane, TextInput, focus_next, focus_previous, init};

struct Controls {
    input: Entity<TextInput>,
    clicks: usize,
}

impl Render for Controls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("LigatureControls")
            .on_action(focus_next)
            .on_action(focus_previous)
            .child(self.input.clone())
            .child(Button::new("enabled", "Enabled").on_click(cx.listener(|this, _, _, cx| {
                this.clicks += 1;
                cx.notify();
            })))
            .child(Button::new("disabled", "Disabled").disabled(true).on_click(cx.listener(|this, _, _, _| {
                this.clicks += 100;
            })))
    }
}

#[test]
fn keyboard_focus_skips_disabled_buttons_and_activation_fires_once() {
    let mut cx = TestAppContext::single();
    cx.update(init);
    let input = cx.new(|cx| TextInput::new("Name", cx));
    let window = cx.add_window({
        let input = input.clone();
        move |_, _| Controls { input, clicks: 0 }
    });
    let root = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&input.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "tab");
    for key in ["enter", "space"] {
        cx.simulate_keystrokes(window.into(), key);
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_event(
                KeyUpEvent {
                    keystroke: Keystroke::parse(key).unwrap(),
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
    }
    assert_eq!(root.read_with(&cx, |view, _| view.clicks), 2);
    cx.simulate_keystrokes(window.into(), "tab");
    cx.update_window(window.into(), |_, window, cx| {
        assert!(input.focus_handle(cx).is_focused(window));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "shift-tab enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_event(
            KeyUpEvent {
                keystroke: Keystroke::parse("enter").unwrap(),
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    assert_eq!(root.read_with(&cx, |view, _| view.clicks), 3);
}

#[test]
fn focused_input_accepts_native_text_and_unicode_deletion() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, cx| TextInput::new("Name", cx));
    let input = window.root(&mut cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.focus(&input.focus_handle(cx), cx);
        window.draw(cx).clear(cx);
        input.update(cx, |input, cx| {
            input.replace_text_in_range(None, "a😀é", window, cx);
        });
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "left backspace");
    assert_eq!(input.read_with(&cx, |input, _| input.text().to_owned()), "aé");
    cx.simulate_input(window.into(), "b");
    assert_eq!(input.read_with(&cx, |input, _| input.text().to_owned()), "abé");
}

#[test]
fn dragging_boundary_resizes_panes_and_stops_on_release() {
    let mut cx = TestAppContext::single();
    let first = cx.new(|cx| TextInput::new("Editor", cx));
    let second = cx.new(|cx| TextInput::new("Preview", cx));
    let window = cx.add_window(move |_, _| SplitPane::new(first.into(), second.into()));
    let mut cx = VisualTestContext::from_window(window.into(), &cx);
    cx.simulate_resize(size(px(1000.0), px(600.0)));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let boundary = cx.debug_bounds("pane-boundary").unwrap();
    let start = boundary.center();
    cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(point(start.x + px(10.0), start.y), MouseButton::Left, Default::default());
    cx.simulate_mouse_move(point(px(700.0), start.y), MouseButton::Left, Default::default());
    cx.simulate_mouse_up(point(px(700.0), start.y), MouseButton::Left, Default::default());
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let resized = cx.debug_bounds("pane-boundary").unwrap();
    assert!(resized.left() > boundary.left() + px(100.0));
    cx.simulate_mouse_move(point(px(300.0), start.y), None, Default::default());
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert_eq!(cx.debug_bounds("pane-boundary").unwrap(), resized);
}
