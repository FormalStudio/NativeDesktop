//! A list that selects is a focus target, as NSTableView is first responder: `window.focus`
//! reaches it, a press on it keeps nd focus there, and the bindings around it match.

use std::cell::RefCell;
use std::rc::Rc;

use nd_gpui::gpui;
use nd_ui::prelude::*;
use nd_ui::test_app::TestApp;
use nd_ui::{KeyBinding, Keystroke, NodeId};
use nd_virtual::{ListState, Selection, list};

gpui::actions!(listing, [Open]);

struct Listing {
    state: ListState<u32>,
    selection: Selection,
    opened: Rc<RefCell<u32>>,
}

impl Render for Listing {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let opened = self.opened.clone();
        div().key_context("Listing").on_action(move |_: &Open, _, _| *opened.borrow_mut() += 1).size_full().child(
            list("files", &self.state, |key: u32, _, _| div().id(format!("row-{key}")).h(px(20.)))
                .selection(self.selection),
        )
    }
}

fn open(selection: Selection) -> (TestApp, nd_ui::WindowId, Rc<RefCell<u32>>) {
    let mut cx = TestApp::new();
    cx.bind_keys([KeyBinding::new("space", Open, Some("Listing"))]);
    let opened = Rc::new(RefCell::new(0));
    let o = opened.clone();
    let handle = cx.open_window(nd_ui::Size { width: 300., height: 200. }, move |_, cx| {
        cx.new(|cx| {
            let mut state = ListState::new(cx);
            state.set_items(0..5, cx);
            Listing { state, selection, opened: o }
        })
    });
    cx.run_until_idle();
    (cx, handle.id(), opened)
}

fn list_node(cx: &TestApp) -> NodeId {
    let tree = cx.semantic_tree();
    tree.nodes().find(|(_, n)| n.test_id.as_deref() == Some("files")).map(|(_, n)| n.id).expect("files")
}

#[test]
fn window_focus_reaches_a_list_and_its_contexts_bindings_match() {
    let (mut cx, window, opened) = open(Selection::Multiple);
    assert!(cx.focus_id(window, "files"), "the list takes focus by its id");
    cx.run_until_idle();
    let files = list_node(&cx);
    assert_eq!(cx.focused(window), Some(files));
    assert_eq!(cx.backend().focused(), Some(files), "the backend follows");
    cx.dispatch_keystroke(window, &Keystroke::parse("space").unwrap());
    cx.run_until_idle();
    assert_eq!(*opened.borrow(), 1, "a binding of the list's context matches while it has focus");
}

#[test]
fn a_press_the_platform_reports_on_a_list_keeps_nd_focus_there() {
    let (mut cx, window, _) = open(Selection::Single);
    let files = list_node(&cx);
    assert!(cx.platform_focused(window, Some(files)), "the presenter's press focuses the list");
    assert_eq!(cx.focused(window), Some(files));
    assert!(cx.focus_order(window).contains(&files), "Tab reaches it");
}

#[test]
fn a_list_that_selects_nothing_takes_no_focus() {
    let (mut cx, window, _) = open(Selection::None);
    let files = list_node(&cx);
    assert!(!cx.focus_id(window, "files"));
    assert!(!cx.platform_focused(window, Some(files)));
}
