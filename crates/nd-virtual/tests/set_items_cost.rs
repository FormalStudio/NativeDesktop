//! What `set_items_versioned` costs on 100k rows: the first set, the same rows again, a reorder
//! (a sort change) and 1% of the versions bumped. Prints each; a release build asserts the
//! budgets (`cargo test --release -p nd-virtual --test set_items_cost -- --nocapture`).

#![allow(clippy::disallowed_types)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use nd_ui::prelude::*;
use nd_ui::test_app::TestApp;
use nd_virtual::{ListState, list};

const ROWS: u64 = 100_000;
/// A reorder of 100k rows, on e1504g's class of core.
const REORDER_BUDGET: Duration = Duration::from_millis(8);

struct Owner {
    state: ListState<u64>,
}

impl Render for Owner {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        list("rows", &self.state, |key: u64, _, _| div().id(format!("row-{key}")).h(px(20.)))
    }
}

/// A fixed shuffle: what a sort on another column looks like to the list.
fn shuffled(n: u64) -> Vec<u64> {
    let mut keys: Vec<u64> = (0..n).collect();
    let mut s = 0x9e37_79b9_7f4a_7c15_u64;
    for i in (1..keys.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        keys.swap(i, (s % (i as u64 + 1)) as usize);
    }
    keys
}

/// The fastest of five runs of `set`, each after `reset` put the list back.
fn best(cx: &mut TestApp, state: &mut ListState<u64>, reset: &[(u64, u64)], set: &[(u64, u64)]) -> Duration {
    (0..5)
        .map(|_| {
            state.set_items_versioned(reset.iter().copied(), cx);
            cx.run_until_idle();
            let t = Instant::now();
            state.set_items_versioned(set.iter().copied(), cx);
            let took = t.elapsed();
            cx.run_until_idle();
            took
        })
        .min()
        .unwrap()
}

#[test]
fn set_items_versioned_on_100k_rows() {
    let mut cx = TestApp::new();
    let slot: Rc<RefCell<Option<ListState<u64>>>> = Rc::default();
    let s = slot.clone();
    let _window = cx.open_window(nd_ui::Size { width: 400., height: 600. }, move |_, cx| {
        cx.new(|cx| {
            let state = ListState::new(cx);
            *s.borrow_mut() = Some(state.clone());
            Owner { state }
        })
    });
    let mut state = slot.borrow().clone().unwrap();

    let sorted: Vec<(u64, u64)> = (0..ROWS).map(|k| (k, 0)).collect();
    let reordered: Vec<(u64, u64)> = shuffled(ROWS).into_iter().map(|k| (k, 0)).collect();
    let bumped: Vec<(u64, u64)> = (0..ROWS).map(|k| (k, u64::from(k % 100 == 0))).collect();
    let empty: Vec<(u64, u64)> = Vec::new();

    let first = best(&mut cx, &mut state, &empty, &sorted);
    let same = best(&mut cx, &mut state, &sorted, &sorted);
    let reorder = best(&mut cx, &mut state, &sorted, &reordered);
    let changed = best(&mut cx, &mut state, &sorted, &bumped);
    assert_eq!(state.len(), ROWS as usize);
    eprintln!(
        "set_items_versioned 100k: first {first:?}, unchanged {same:?}, reorder {reorder:?}, 1% changed {changed:?}"
    );
    if !cfg!(debug_assertions) {
        assert!(reorder < REORDER_BUDGET, "a reorder of 100k rows took {reorder:?}");
        assert!(same < REORDER_BUDGET && changed < REORDER_BUDGET, "unchanged {same:?}, 1% changed {changed:?}");
    }
}
