//! The tile grid, tier 1: a dragged tile takes the slot under the pointer and a drop there
//! reports the move; Escape puts it back and reports nothing; a tile of another grid dropped in
//! reports the slot it lands in.

use std::cell::RefCell;
use std::rc::Rc;

use nd_components::{TileMoved, tile, tile_grid};
use nd_ui::prelude::*;
use nd_ui::test_app::TestApp;
use nd_ui::{
    DivProps, DragEnded, DragEvent, Dropped, GesturePhase, Keystroke, NodeId, Point, Rect, Size, WindowHandle, WindowId,
};

type Log = Rc<RefCell<Vec<String>>>;

struct Grid {
    log: Log,
}

impl Render for Grid {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (moved, dropped, ended) = (self.log.clone(), self.log.clone(), self.log.clone());
        div().size_full().child(
            tile_grid("pinned")
                .tiles(["a", "b", "c", "d", "e", "f"].map(|id| tile(id, div().size_full())))
                .draggable_tiles("tab")
                .on_move(move |e: &TileMoved, _, _| moved.borrow_mut().push(format!("move {} {}", e.id, e.index)))
                .on_drop(move |e: &Dropped, _, _| {
                    let slot = e.slot.clone().expect("slot");
                    dropped.borrow_mut().push(format!("drop {} {}", e.item.id, slot.index))
                })
                .on_drag_end(move |e: &DragEnded, _, _| ended.borrow_mut().push(format!("end {:?}", e.outcome))),
        )
    }
}

/// Frames until the tiles' slides have run out: they animate on the frame clock.
fn settle(cx: &mut TestApp) {
    for _ in 0..6 {
        cx.drain_events();
        cx.advance_clock(1.);
        cx.frame();
    }
}

fn node(cx: &TestApp, test_id: &str) -> NodeId {
    let tree = cx.semantic_tree();
    tree.nodes().find(|(_, n)| n.test_id.as_deref() == Some(test_id)).map(|(_, n)| n.id).expect(test_id)
}

fn drag(cx: &mut TestApp, tile: NodeId, phase: GesturePhase, start: Point, to: Point) {
    let translation = Point { x: to.x - start.x, y: to.y - start.y };
    cx.events().emitter(tile).emit(DragEvent {
        phase,
        start,
        position: to,
        translation,
        velocity: Point::default(),
        modifiers: Default::default(),
    });
    settle(cx);
}

fn open() -> (TestApp, Log, WindowId) {
    let log: Log = Rc::default();
    let l = log.clone();
    let mut cx = TestApp::new();
    let w = cx.open_window(Size { width: 480., height: 400. }, move |_, cx| cx.new(|_| Grid { log: l })).id();
    cx.backend_mut().set_window_frame(w, Rect::new(0., 0., 480., 400.));
    // The grid learns its width from its first layout.
    settle(&mut cx);
    (cx, log, w)
}

/// Four columns of 114 pt with 8 pt gaps across 480 pt.
fn centre(slot: usize) -> Point {
    let (col, row) = ((slot % 4) as f32, (slot / 4) as f32);
    Point { x: col * 122. + 57., y: row * (114. * 0.74 + 8.) + 40. }
}

#[test]
fn a_tile_dropped_on_a_slot_reports_the_move() {
    let (mut cx, log, _) = open();
    let a = node(&cx, "a");
    let start = centre(0);
    drag(&mut cx, a, GesturePhase::Began, start, start);
    drag(&mut cx, a, GesturePhase::Changed, start, centre(2));
    drag(&mut cx, a, GesturePhase::Ended, start, centre(2));
    let log = log.borrow();
    assert_eq!(log.first().map(String::as_str), Some("move a 2"), "{log:?}");
}

#[test]
fn escape_puts_the_tile_back() {
    let (mut cx, log, w) = open();
    let b = node(&cx, "b");
    let start = centre(1);
    drag(&mut cx, b, GesturePhase::Began, start, start);
    drag(&mut cx, b, GesturePhase::Changed, start, centre(5));
    cx.dispatch_keystroke(w, &Keystroke::parse("escape").unwrap());
    settle(&mut cx);
    drag(&mut cx, b, GesturePhase::Ended, start, centre(5));
    assert_eq!(*log.borrow(), ["end Cancelled"]);
}

struct Pinned {
    ids: Vec<&'static str>,
}

impl Render for Pinned {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(tile_grid("pinned").tiles(self.ids.iter().map(|&id| tile(id, div().size_full()))))
    }
}

fn props(cx: &TestApp, id: NodeId) -> DivProps {
    cx.backend().props(id).and_then(|p| p.downcast_ref::<DivProps>()).cloned().expect("div")
}

/// Where the tile is drawn in the grid: its layout frame moved by its offset.
fn shown(cx: &TestApp, id: NodeId) -> (f32, f32) {
    let frame = cx.backend().frame(id).expect("laid out");
    let (dx, dy) = props(cx, id).transform.offset;
    (frame.origin.x + dx, frame.origin.y + dy)
}

fn open_pinned(ids: &[&'static str]) -> (TestApp, WindowHandle<Pinned>) {
    let mut cx = TestApp::new();
    let ids = ids.to_vec();
    let handle = cx.open_window(Size { width: 480., height: 400. }, move |_, cx| cx.new(|_| Pinned { ids }));
    settle(&mut cx);
    (cx, handle)
}

fn set_tiles(cx: &mut TestApp, handle: &WindowHandle<Pinned>, ids: &[&'static str]) {
    let ids = ids.to_vec();
    handle.root().update(cx, |p, cx| {
        p.ids = ids;
        cx.notify();
    });
    cx.frame();
}

#[test]
fn the_first_width_places_tiles_at_once() {
    let (cx, _) = open_pinned(&["a", "b"]);
    let b = node(&cx, "b");
    assert_eq!(shown(&cx, b), (122., 0.));
    assert_eq!(props(&cx, b).transform.offset, (0., 0.), "no glide on first show");
    assert_eq!(props(&cx, b).style.opacity, None, "no entrance on first show");
}

#[test]
fn tiles_sit_at_their_slots_in_layout() {
    // A backend hit-tests and hovers layout frames, so a tile placed by offset alone would take
    // its clicks at the grid's origin.
    let (cx, _) = open_pinned(&["a", "b", "c", "d", "e"]);
    let origin = |id| cx.backend().frame(node(&cx, id)).map(|f| (f.origin.x, f.origin.y));
    assert_eq!(origin("a"), Some((0., 0.)));
    assert_eq!(origin("b"), Some((122., 0.)));
    assert_eq!(origin("e"), Some((0., (114. * 0.74_f32).round() + 8.)));
}

#[test]
fn a_removed_tile_fades_where_it_stood_while_the_rest_slide_over() {
    let (mut cx, handle) = open_pinned(&["a", "b", "c"]);
    let (a, b) = (node(&cx, "a"), node(&cx, "b"));
    set_tiles(&mut cx, &handle, &["b", "c"]);
    assert!(cx.backend().is_live(a), "held while it fades");
    assert!(props(&cx, a).inert);
    assert_eq!(shown(&cx, a), (0., 0.), "where it stood");
    assert!((shown(&cx, b).0 - 122.).abs() < 0.5, "slides from its old slot");
    settle(&mut cx);
    assert!(!cx.backend().is_live(a), "gone once faded");
    assert_eq!(shown(&cx, b), (0., 0.));
}

#[test]
fn an_added_tile_fades_in_and_pushes_the_rest_along() {
    let (mut cx, handle) = open_pinned(&["b"]);
    let b = node(&cx, "b");
    set_tiles(&mut cx, &handle, &["a", "b"]);
    let a = node(&cx, "a");
    assert_eq!(props(&cx, a).style.opacity, Some(0.), "comes in from transparent");
    assert!(props(&cx, a).transform.scale < 1.);
    assert_eq!(shown(&cx, b), (0., 0.), "moves from where it was");
    settle(&mut cx);
    assert_eq!(props(&cx, a).style.opacity.unwrap_or(1.), 1.);
    assert_eq!(shown(&cx, b), (122., 0.));
}

#[test]
fn a_new_width_reflows_on_the_spring() {
    let (mut cx, handle) = open_pinned(&["a", "b"]);
    let b = node(&cx, "b");
    cx.resize_window(handle.id(), Size { width: 300., height: 400. });
    for _ in 0..3 {
        cx.drain_events();
        cx.frame();
    }
    assert!((shown(&cx, b).0 - 122.).abs() < 0.5, "no jump: {:?}", shown(&cx, b));
    assert!(cx.needs_frame(), "moving");
    settle(&mut cx);
    assert_eq!(shown(&cx, b), (77., 0.), "four 69 pt columns");
}

#[test]
fn a_released_tile_slides_home_from_the_pointer() {
    let (mut cx, _, _) = open();
    let b = node(&cx, "b");
    let start = centre(1);
    drag(&mut cx, b, GesturePhase::Began, start, start);
    drag(&mut cx, b, GesturePhase::Changed, start, Point { x: start.x, y: start.y + 60. });
    assert_eq!(shown(&cx, b), (122., 60.), "on the pointer");
    let item = nd_ui::DragItem::new("tab", "b");
    cx.events().emitter(b).emit(DragEnded { item, outcome: nd_ui::DragOutcome::Cancelled });
    cx.drain_events();
    cx.frame();
    let (x, y) = shown(&cx, b);
    assert!(x == 122. && y > 50., "leaves from where it was let go: {y}");
    settle(&mut cx);
    assert_eq!(shown(&cx, b), (122., 0.));
}

#[test]
fn under_reduce_motion_tiles_dissolve_into_new_slots() {
    let (mut cx, handle) = open_pinned(&["a", "b", "c"]);
    cx.set_reduce_motion(true);
    let (a, b) = (node(&cx, "a"), node(&cx, "b"));
    let grid = cx.backend().parent(b).expect("grid");
    let height = cx.backend().frame(grid).expect("laid out").size.height;
    set_tiles(&mut cx, &handle, &["b", "c"]);
    assert_eq!(props(&cx, a).transform.scale, 1., "a removed tile does not shrink");
    assert_eq!(shown(&cx, b), (122., 0.), "a moved tile holds its old slot");
    cx.advance_clock(0.05);
    cx.frame();
    assert_eq!(shown(&cx, b), (122., 0.), "while it fades out");
    assert!(props(&cx, b).style.opacity.unwrap() < 1.);
    assert!(props(&cx, a).style.opacity.unwrap() < 1., "the removed one fades");
    cx.advance_clock(0.05);
    cx.frame();
    assert_eq!(shown(&cx, b), (0., 0.), "then shows in the new one");
    assert!(props(&cx, b).style.opacity.unwrap() < 1., "fading in");
    assert_eq!(cx.backend().frame(grid).expect("laid out").size.height, height, "one row still");
    settle(&mut cx);
    assert_eq!(props(&cx, b).style.opacity.unwrap_or(1.), 1.);
    assert!(!cx.backend().is_live(a));
}
