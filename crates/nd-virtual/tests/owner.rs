//! A `ListState` driven the way views often drive it: keys handed over from `render`.

use std::cell::Cell;
use std::rc::Rc;

use nd_ui::prelude::*;
use nd_ui::test_app::TestApp;
use nd_virtual::{ListState, list};

struct Owner {
    state: ListState<u64>,
    renders: Rc<Cell<u32>>,
}

impl Render for Owner {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        self.state.set_items_versioned((0..3).map(|k| (k, 0)), cx);
        list("rows", &self.state, |key: u64, _, _| div().id(format!("row-{key}")).h(px(10.)))
    }
}

#[test]
fn the_same_keys_from_render_settle_instead_of_rendering_again() {
    let renders = Rc::new(Cell::new(0));
    let mut cx = TestApp::new();
    let r = renders.clone();
    let _handle = cx.open_window(nd_ui::Size { width: 300., height: 200. }, move |_, cx| {
        cx.new(|cx| Owner { state: ListState::new(cx), renders: r })
    });
    // Before the fix every hand-over bumped the model's generation and re-rendered the owner:
    // `run_until_idle` panicked after 100 frames.
    cx.run_until_idle();
    assert!(renders.get() <= 3, "the owner rendered {} times", renders.get());
}

struct Rows {
    state: ListState<u64>,
}

impl Render for Rows {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        list("rows", &self.state, |key: u64, _, _| div().id(format!("row-{key}")).h(px(10.)))
    }
}

#[test]
fn a_reorder_renders_no_row_again_and_a_new_version_renders_its_row() {
    let mut cx = TestApp::new();
    let slot: Rc<std::cell::RefCell<Option<ListState<u64>>>> = Rc::default();
    let s = slot.clone();
    let _handle = cx.open_window(nd_ui::Size { width: 300., height: 200. }, move |_, cx| {
        cx.new(|cx| {
            let state = ListState::new(cx);
            *s.borrow_mut() = Some(state.clone());
            Rows { state }
        })
    });
    let mut state = slot.borrow().clone().unwrap();
    state.set_items_versioned((0..10).map(|k| (k, 0)), &mut cx);
    cx.run_until_idle();
    let renders = |cx: &TestApp, state: &ListState<u64>| -> Vec<u64> {
        (0..10).map(|k| state.row_view(&k).map_or(0, |v| cx.render_count(v))).collect()
    };
    let shown = state.rendered();
    assert!(shown.len() >= 2, "rows rendered: {shown:?}");
    let (first, second) = (shown[0], shown[1]);
    let before = renders(&cx, &state);
    let swapped: Vec<u64> = [second, first].into_iter().chain((0..10).filter(|k| !shown[..2].contains(k))).collect();

    state.set_items_versioned(swapped.iter().map(|&k| (k, 0)), &mut cx);
    cx.run_until_idle();
    assert_eq!(state.items(), swapped);
    assert_eq!(renders(&cx, &state), before, "a reorder renders no row again");

    state.set_items_versioned(swapped.iter().map(|&k| (k, u64::from(k == second))), &mut cx);
    cx.run_until_idle();
    let after = renders(&cx, &state);
    assert_eq!(after[first as usize], before[first as usize], "an unchanged version renders nothing");
    assert_eq!(after[second as usize], before[second as usize] + 1, "a new version renders its row once");

    let row = state.row_of(&3).unwrap();
    state.set_items_versioned((5..10).map(|k| (k, 0)), &mut cx);
    cx.run_until_idle();
    assert_eq!(state.row_of(&3), None);
    assert_eq!(state.key_of(state.row_of(&7).unwrap()), Some(7));
    state.set_items_versioned((0..10).map(|k| (k, 0)), &mut cx);
    assert_ne!(state.row_of(&3), Some(row), "a key that left and came back is a new row");
}
