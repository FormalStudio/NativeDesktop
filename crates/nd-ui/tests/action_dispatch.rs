//! An action dispatched from inside an event handler (a button's click) reaches listeners that
//! need the window, which that handler holds, and they can move focus.

use std::cell::Cell;
use std::rc::Rc;

use nd_gpui::gpui::{self, actions};
use nd_ui::semantics::SemanticAction;
use nd_ui::test_app::TestApp;
use nd_ui::*;

actions!(dispatch_test, [Go]);

struct View {
    ran: Rc<Cell<u32>>,
}

impl Render for View {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ran = self.ran.clone();
        v_flex()
            .on_action(cx.listener(move |_, _: &Go, window, cx| {
                ran.set(ran.get() + 1);
                window.focus_id(cx, "field");
            }))
            .child(button("go", "Go").on_click(|_: &ClickEvent, window, cx| {
                cx.dispatch_action(window.id(), &Go);
            }))
            .child(div().id("field").focusable())
    }
}

fn node(cx: &TestApp, test_id: &str) -> NodeId {
    cx.semantic_tree().nodes().find(|(_, n)| n.test_id.as_deref() == Some(test_id)).map(|(_, n)| n.id).expect(test_id)
}

#[test]
fn a_click_can_dispatch_an_action_that_needs_the_window_and_focus() {
    let ran = Rc::new(Cell::new(0));
    let mut cx = TestApp::new();
    let window = cx.open_window(Size { width: 300., height: 200. }, |_, cx| cx.new(|_| View { ran: ran.clone() }));
    let go = node(&cx, "go");
    cx.perform(go, &SemanticAction::Click).expect("click");
    cx.run_until_idle();
    assert_eq!(ran.get(), 1, "the listener ran once the click handler returned");
    assert_eq!(cx.focused(window.id()), Some(node(&cx, "field")), "and its focus request landed");
}

struct Plain;

impl Render for Plain {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[test]
fn an_unhandled_sidebar_toggle_reaches_the_backend_as_a_window_command() {
    let mut cx = TestApp::new();
    let window = cx.open_window(Size { width: 300., height: 200. }, |_, cx| cx.new(|_| Plain));
    let handled = cx.dispatch_action(window.id(), &actions::ToggleSidebar);
    assert!(handled, "nd handles it when the app does not");
    assert_eq!(cx.backend().window_commands(), [(window.id(), backend::WindowCommand::ToggleSidebar)]);
}

#[test]
fn a_fullscreen_request_reaches_the_backend_as_a_window_command() {
    let mut cx = TestApp::new();
    let window = cx.open_window(Size { width: 300., height: 200. }, |_, cx| cx.new(|_| Plain));
    cx.set_window_fullscreen(window.id(), true);
    cx.set_window_fullscreen(window.id(), false);
    assert_eq!(
        cx.backend().window_commands(),
        [
            (window.id(), backend::WindowCommand::Fullscreen(true)),
            (window.id(), backend::WindowCommand::Fullscreen(false))
        ]
    );
}

#[test]
fn a_window_that_becomes_key_runs_its_activate_handler() {
    let mut cx = TestApp::new();
    let ran = Rc::new(Cell::new(0));
    let seen = ran.clone();
    let first = cx.open_window(Size { width: 300., height: 200. }, move |window, cx| {
        let seen = seen.clone();
        window.on_activate(move |_, _| seen.set(seen.get() + 1));
        cx.new(|_| Plain)
    });
    let second = cx.open_window(Size { width: 300., height: 200. }, |_, cx| cx.new(|_| Plain));
    cx.window_activated(first.id());
    cx.window_activated(second.id());
    assert_eq!(ran.get(), 1);
    assert_eq!(cx.active_window(), Some(second.id()));
}

#[test]
fn a_window_that_moves_runs_its_frame_handler_with_the_new_origin() {
    let mut cx = TestApp::new();
    let seen = Rc::new(Cell::new(None));
    let kept = seen.clone();
    let window = cx.open_window(Size { width: 300., height: 200. }, move |window, cx| {
        let kept = kept.clone();
        window.on_frame_changed(move |window, _| kept.set(window.origin()));
        cx.new(|_| Plain)
    });
    cx.window_frame_changed(window.id(), Some(nd_core::Point { x: 40., y: 60. }));
    assert_eq!(seen.get(), Some(nd_core::Point { x: 40., y: 60. }));
}

#[test]
fn a_window_keeps_its_place_only_while_its_title_bar_is_on_a_display() {
    let main = Rect::new(0., 0., 1512., 945.);
    let gone = Rect::new(1512., -400., 2560., 1415.);
    assert!(title_bar_on_a_display(Rect::new(100., 100., 1280., 800.), &[main]));
    assert!(title_bar_on_a_display(Rect::new(1800., 0., 1280., 800.), &[main, gone]));
    assert!(!title_bar_on_a_display(Rect::new(1800., 0., 1280., 800.), &[main]), "its display is gone");
    assert!(!title_bar_on_a_display(Rect::new(100., 930., 1280., 800.), &[main]), "the title bar is below the screen");
    assert!(!title_bar_on_a_display(Rect::new(100., -60., 1280., 800.), &[main]), "the title bar is above the screen");
    assert!(title_bar_on_a_display(Rect::new(-1200., 100., 1280., 800.), &[main]), "80 points still show");
    assert!(!title_bar_on_a_display(Rect::new(-1260., 100., 1280., 800.), &[main]), "20 points cannot be grabbed");
}
