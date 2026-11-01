//! What clicks, keys and a marquee select on presenters that draw the list, against what
//! NSTableView and NSCollectionView select for the same input, and what the app's handlers see.

use std::cell::RefCell;
use std::rc::Rc;

use nd_ui::prelude::*;
use nd_ui::test_app::TestApp;
use nd_virtual::selection::{Extend, Nav, Picked, click, key, marquee, step};
use nd_virtual::{ListState, RowId, RowsSelected, SelectEvent, Selection, SelectionChanged, list};

fn ids(n: u64) -> Vec<RowId> {
    (0..n).map(RowId).collect()
}

const PLAIN: Extend = Extend { range: false, toggle: false };
const SHIFT: Extend = Extend { range: true, toggle: false };
const TOGGLE: Extend = Extend { range: false, toggle: true };
const BOTH: Extend = Extend { range: true, toggle: true };

#[test]
fn a_click_selects_one_and_moves_the_anchor() {
    let rows = ids(6);
    let p = click(&rows, &[RowId(1), RowId(2)], Some(RowId(1)), RowId(4), Selection::Multiple, PLAIN);
    assert_eq!(p, Picked { selected: vec![RowId(4)], anchor: Some(RowId(4)), cursor: Some(RowId(4)) });
}

#[test]
fn shift_selects_the_range_from_the_anchor_either_way() {
    let rows = ids(8);
    let down = click(&rows, &[RowId(2)], Some(RowId(2)), RowId(5), Selection::Multiple, SHIFT);
    assert_eq!(down.selected, ids(6)[2..].to_vec());
    assert_eq!((down.anchor, down.cursor), (Some(RowId(2)), Some(RowId(5))), "the anchor stays");
    // A second Shift click replaces the range from the same anchor.
    let up = click(&rows, &down.selected, down.anchor, RowId(0), Selection::Multiple, SHIFT);
    assert_eq!(up.selected, ids(3));
}

#[test]
fn the_secondary_key_toggles_and_with_shift_adds_the_range() {
    let rows = ids(10);
    let one = click(&rows, &[RowId(1)], Some(RowId(1)), RowId(4), Selection::Multiple, TOGGLE);
    assert_eq!(one.selected, vec![RowId(1), RowId(4)]);
    let off = click(&rows, &one.selected, one.anchor, RowId(1), Selection::Multiple, TOGGLE);
    assert_eq!(off.selected, vec![RowId(4)]);
    let added = click(&rows, &[RowId(1), RowId(4)], Some(RowId(4)), RowId(6), Selection::Multiple, BOTH);
    assert_eq!(added.selected, vec![RowId(1), RowId(4), RowId(5), RowId(6)]);
}

#[test]
fn single_selection_ignores_the_modifiers() {
    let rows = ids(4);
    let p = click(&rows, &[RowId(0)], Some(RowId(0)), RowId(3), Selection::Single, BOTH);
    assert_eq!(p.selected, vec![RowId(3)]);
}

#[test]
fn keys_step_by_row_and_by_lane_and_skip_unselectable_rows() {
    let any = |_: usize| true;
    assert_eq!(step(10, 1, Some(3), Nav::Down, 4, any), Some(4));
    assert_eq!(step(10, 1, Some(0), Nav::Up, 4, any), None, "the first row stays");
    assert_eq!(step(10, 1, None, Nav::Down, 4, any), Some(0), "no cursor: down starts at the top");
    assert_eq!(step(10, 1, Some(3), Nav::Left, 4, any), None, "a list has no lanes");
    assert_eq!(step(10, 1, Some(3), Nav::End, 4, any), Some(9));
    assert_eq!(step(10, 1, Some(8), Nav::Home, 4, any), Some(0));
    assert_eq!(step(10, 1, Some(2), Nav::PageDown, 4, any), Some(6));
    assert_eq!(step(10, 1, Some(8), Nav::PageDown, 4, any), Some(9));
    assert_eq!(step(10, 1, Some(2), Nav::PageUp, 4, any), Some(0));
    // A grid of 4 lanes: up and down move a whole line, left and right one cell.
    assert_eq!(step(10, 4, Some(5), Nav::Down, 2, any), Some(9));
    assert_eq!(step(10, 4, Some(6), Nav::Down, 2, any), None, "no cell below on the short last line");
    assert_eq!(step(10, 4, Some(5), Nav::Up, 2, any), Some(1));
    assert_eq!(step(10, 4, Some(5), Nav::Right, 2, any), Some(6));
    assert_eq!(step(10, 4, Some(4), Nav::Left, 2, any), Some(3));
    // Row 4 is a section header.
    let rows = |i: usize| i != 4;
    assert_eq!(step(10, 1, Some(3), Nav::Down, 4, rows), Some(5));
    assert_eq!(step(10, 1, Some(5), Nav::Up, 4, rows), Some(3));
}

#[test]
fn shift_with_a_key_extends_from_the_anchor() {
    let rows = ids(6);
    let p = key(&rows, Some(RowId(2)), RowId(4), Selection::Multiple, true);
    assert_eq!(p.selected, ids(5)[2..].to_vec());
    assert_eq!(p.anchor, Some(RowId(2)));
    let plain = key(&rows, Some(RowId(2)), RowId(4), Selection::Multiple, false);
    assert_eq!(plain.selected, vec![RowId(4)]);
    assert_eq!(plain.anchor, Some(RowId(4)));
}

#[test]
fn a_marquee_replaces_adds_or_toggles() {
    let rows = ids(6);
    let base = [RowId(0), RowId(2)];
    let swept = [RowId(2), RowId(3)];
    assert_eq!(marquee(&rows, &base, &swept, PLAIN), vec![RowId(2), RowId(3)]);
    assert_eq!(marquee(&rows, &base, &swept, SHIFT), vec![RowId(0), RowId(2), RowId(3)]);
    assert_eq!(marquee(&rows, &base, &swept, TOGGLE), vec![RowId(0), RowId(3)]);
}

struct Files {
    state: ListState<u64>,
    seen: Rc<RefCell<Vec<SelectionChanged<u64>>>>,
    picked: Rc<RefCell<Vec<SelectEvent<u64>>>>,
}

impl Render for Files {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (seen, picked) = (self.seen.clone(), self.picked.clone());
        list("files", &self.state, |key: u64, _, _| div().id(format!("row-{key}")).h(px(20.)))
            .selection(Selection::Multiple)
            .selectable(|key, _| *key != 3)
            .on_selection_change(move |e, _, _| seen.borrow_mut().push(e.clone()))
            .on_select(move |e, _, _| picked.borrow_mut().push(e.clone()))
    }
}

#[test]
fn the_app_sees_the_cursor_an_emptied_selection_and_no_unselectable_row() {
    let mut cx = TestApp::new();
    let (seen, picked) = (Rc::new(RefCell::new(Vec::new())), Rc::new(RefCell::new(Vec::new())));
    let (s, p) = (seen.clone(), picked.clone());
    let handle = cx.open_window(nd_ui::Size { width: 300., height: 200. }, move |_, cx| {
        cx.new(|cx| {
            let mut state = ListState::new(cx);
            state.set_items(0..6, cx);
            Files { state, seen: s, picked: p }
        })
    });
    let window = handle.id();
    let root = cx.semantic_window(window).expect("window").root;
    let node = root.iter().find(|n| n.test_id.as_deref() == Some("files")).expect("the list node").id;
    let state = handle.root().read(&cx).state.clone();
    let row = |k: u64| state.row_of(&k).expect("row");

    // A Shift range from row 4 up to row 1 that crosses the unselectable row 3.
    cx.emit(node, RowsSelected { rows: (1..=4).map(row).collect(), cursor: Some(row(1)) });
    assert_eq!(state.selected(), vec![1, 2, 4]);
    let last = picked.borrow().last().cloned().expect("on_select");
    assert_eq!(last.key, 1, "the key is the row the click landed on, not the last in order");

    cx.emit(node, RowsSelected { rows: Vec::new(), cursor: None });
    assert!(state.selected().is_empty());
    assert_eq!(seen.borrow().last(), Some(&SelectionChanged { keys: Vec::new(), cursor: None }));
    assert_eq!(picked.borrow().len(), 1, "on_select has no key to report for an empty selection");
}
