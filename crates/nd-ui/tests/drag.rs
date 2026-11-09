//! Drag and drop, tier 1: a drag from a `draggable` div finds the innermost target taking its
//! kind in whichever window is under the pointer, reports entry, moves and exit to targets and
//! the outcome to the source; Escape and a release outside end it; widget sources drive the
//! same session; a source that lifts has its picture follow the pointer from where it was
//! grabbed and land in the target or back home; a spring-loaded target opens once the drag
//! has stayed over it for the delay; `.on_resize` reports laid-out sizes.

use std::cell::RefCell;
use std::rc::Rc;

use nd_gpui::gpui::{SharedString, px};
use nd_ui::test_app::TestApp;
use nd_ui::*;

type Log = Rc<RefCell<Vec<String>>>;

struct Side {
    log: Log,
    name: &'static str,
    source: bool,
}

impl Render for Side {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (over, drop, end, inner, size) =
            (self.log.clone(), self.log.clone(), self.log.clone(), self.log.clone(), self.log.clone());
        let name = self.name;
        v_flex()
            .size_full()
            .child(div().id("spacer").h(px(40.)).w_full().when(self.source, |d| {
                d.child(
                    div()
                        .id("chip")
                        .w(px(40.))
                        .h(px(40.))
                        .draggable(DragItem::new("tab", "t1"))
                        .on_drag_end(move |e: &DragEnded, _, _| end.borrow_mut().push(format!("end {:?}", e.outcome))),
                )
            }))
            .child(
                div()
                    .id(SharedString::from(format!("{name}-list")))
                    .w_full()
                    .h(px(200.))
                    .accepts_drops(["tab", DragItem::FILES])
                    .on_drag_over(move |e: &DragOver, _, _| {
                        over.borrow_mut().push(format!("{name} {:?} {},{}", e.phase, e.local.x, e.local.y))
                    })
                    .on_drop(move |e: &Dropped, _, _| {
                        let line = if e.item.kind == DragItem::FILES {
                            format!("{name} files {:?} alt {}", e.item.ids, e.modifiers.alt)
                        } else {
                            format!("{name} drop {} {},{}", e.item.id, e.local.x, e.local.y)
                        };
                        drop.borrow_mut().push(line)
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("{name}-tiles")))
                            .w(px(100.))
                            .h(px(50.))
                            .accepts_drops(["tile"])
                            .on_drop(move |_: &Dropped, _, _| inner.borrow_mut().push(format!("{name} tiles drop"))),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{name}-meter")))
                    .w_full()
                    .h(px(10.))
                    .on_resize(move |e: &Resized, _, _| size.borrow_mut().push(format!("{name} size {}", e.width))),
            )
    }
}

fn node(cx: &TestApp, test_id: &str) -> NodeId {
    let tree = cx.semantic_tree();
    tree.nodes().find(|(_, n)| n.test_id.as_deref() == Some(test_id)).map(|(_, n)| n.id).expect(test_id)
}

const SIZE: Size = Size { width: 400., height: 300. };

/// Two windows side by side, 100 points apart: `a` holds the chip.
fn open() -> (TestApp, Log, WindowId, WindowId) {
    let log: Log = Rc::default();
    let mut cx = TestApp::new();
    let (la, lb) = (log.clone(), log.clone());
    let a = cx.open_window(SIZE, move |_, cx| cx.new(|_| Side { log: la, name: "a", source: true })).id();
    let b = cx.open_window(SIZE, move |_, cx| cx.new(|_| Side { log: lb, name: "b", source: false })).id();
    cx.backend_mut().set_window_frame(a, Rect::new(0., 0., 400., 300.));
    cx.backend_mut().set_window_frame(b, Rect::new(500., 0., 400., 300.));
    log.borrow_mut().clear();
    (cx, log, a, b)
}

fn drag(cx: &mut TestApp, chip: NodeId, phase: GesturePhase, x: f32, y: f32) {
    let p = Point { x, y };
    cx.emit(
        chip,
        DragEvent {
            phase,
            start: Point { x: 20., y: 20. },
            position: p,
            translation: p,
            velocity: p,
            modifiers: Default::default(),
        },
    );
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.borrow_mut())
}

#[test]
fn a_drag_crosses_windows_to_the_target_under_the_pointer() {
    let (mut cx, log, _, b) = open();
    let chip = node(&cx, "chip");
    drag(&mut cx, chip, GesturePhase::Began, 20., 20.);
    assert!(take(&log).is_empty(), "nothing under the chip takes tabs");
    drag(&mut cx, chip, GesturePhase::Changed, 200., 140.);
    assert_eq!(take(&log), ["a Entered 200,100"]);
    drag(&mut cx, chip, GesturePhase::Changed, 210., 150.);
    assert_eq!(take(&log), ["a Moved 210,110"]);
    // Over the inner target, which takes tiles only: the list keeps the drag.
    drag(&mut cx, chip, GesturePhase::Changed, 30., 60.);
    assert_eq!(take(&log), ["a Moved 30,20"]);
    // 550 in a's coordinates is 50 into b.
    drag(&mut cx, chip, GesturePhase::Changed, 550., 60.);
    assert_eq!(take(&log), ["a Exited 0,0", "b Entered 50,20"]);
    drag(&mut cx, chip, GesturePhase::Ended, 560., 60.);
    assert_eq!(take(&log), ["b Moved 60,20", "b drop t1 60,20", format!("end Dropped {{ window: {b:?} }}").as_str()]);
    assert!(!cx.is_dragging());
}

#[test]
fn escape_cancels_and_a_release_off_every_target_reports_where() {
    let (mut cx, log, a, _) = open();
    let chip = node(&cx, "chip");
    drag(&mut cx, chip, GesturePhase::Began, 20., 20.);
    drag(&mut cx, chip, GesturePhase::Changed, 200., 140.);
    take(&log);
    let escape = Keystroke::parse("escape").unwrap();
    assert_eq!(cx.dispatch_keystroke(a, &escape), nd_ui::dispatch::KeyDispatch::Handled);
    cx.run_until_idle();
    assert_eq!(take(&log), ["a Exited 0,0", "end Cancelled"]);
    // The rest of the gesture belongs to no drag.
    drag(&mut cx, chip, GesturePhase::Ended, 200., 140.);
    assert!(take(&log).is_empty());

    drag(&mut cx, chip, GesturePhase::Began, 20., 20.);
    drag(&mut cx, chip, GesturePhase::Ended, 300., 290.);
    assert_eq!(take(&log), ["end Cancelled"], "inside a window, nothing there takes it");

    drag(&mut cx, chip, GesturePhase::Began, 20., 20.);
    drag(&mut cx, chip, GesturePhase::Ended, 450., 500.);
    assert_eq!(take(&log), ["end Outside { screen: Point { x: 450.0, y: 500.0 } }"]);
}

#[test]
fn a_drag_outside_its_window_is_previewed_until_it_comes_back() {
    let (mut cx, _log, _, _) = open();
    let chip = node(&cx, "chip");
    drag(&mut cx, chip, GesturePhase::Began, 20., 20.);
    drag(&mut cx, chip, GesturePhase::Changed, 200., 140.);
    assert!(cx.backend().previews().is_empty(), "inside its window the source draws itself");
    drag(&mut cx, chip, GesturePhase::Changed, 450., 140.);
    drag(&mut cx, chip, GesturePhase::Changed, 600., 140.);
    drag(&mut cx, chip, GesturePhase::Changed, 100., 140.);
    let shown: Vec<_> = cx
        .backend()
        .previews()
        .iter()
        .map(|(_, p)| match p {
            DragPreview::At { frame, lifted: false } => Some(frame.origin.x + frame.size.width / 2.),
            _ => None,
        })
        .collect();
    assert_eq!(shown, [Some(450.), Some(600.), None], "centred under the pointer, then gone");
}

#[test]
fn a_widget_source_drives_the_same_session() {
    let (mut cx, log, _, b) = open();
    let list = node(&cx, "a-list");
    let item = DragItem::new("tab", "row-7");
    let source = |phase, at| DragSource { item: item.clone(), phase, at, native: false, modifiers: Default::default() };
    cx.emit(list, source(GesturePhase::Began, DragPoint::Window(Point { x: 10., y: 60. })));
    cx.emit(list, source(GesturePhase::Changed, DragPoint::Window(Point { x: 10., y: 61. })));
    assert_eq!(take(&log), ["a Entered 10,21"], "a source list is a target of its own rows");
    cx.emit(list, source(GesturePhase::Ended, DragPoint::Screen(Point { x: 520., y: 100. })));
    assert_eq!(take(&log), ["a Exited 0,0", "b Entered 20,60", "b drop row-7 20,60"]);
    let _ = b;
}

#[test]
fn a_backend_owned_drag_hit_tests_the_window_it_reports() {
    let (mut cx, log, a, b) = open();
    let chip = node(&cx, "chip");
    cx.begin_drag(a, chip, DragItem::new("tab", "t1"), true);
    cx.drag_moved(b, Point { x: 30., y: 70. });
    cx.drain_events();
    assert_eq!(take(&log), ["b Entered 30,30"]);
    cx.drag_exited();
    cx.drain_events();
    assert_eq!(take(&log), ["b Exited 0,0"]);
    cx.drag_finished(Point { x: 450., y: 20. });
    cx.drain_events();
    assert_eq!(take(&log), ["end Outside { screen: Point { x: 450.0, y: 20.0 } }"]);
    assert!(cx.backend().previews().is_empty(), "the backend draws its own drags");
}

#[test]
fn files_from_outside_reach_a_target_with_the_keys_held() {
    let (mut cx, log, _, b) = open();
    cx.begin_external_drag(b, DragItem::many(DragItem::FILES, ["/tmp/a.txt", "/tmp/b.txt"]));
    cx.drag_moved(b, Point { x: 30., y: 70. });
    cx.drain_events();
    assert_eq!(take(&log), ["b Entered 30,30"]);
    cx.set_drag_modifiers(KeyModifiers { alt: true, ..Default::default() });
    assert!(cx.drag_released(b, Point { x: 30., y: 70. }));
    cx.drain_events();
    assert_eq!(take(&log), ["b Moved 30,30", r#"b files ["/tmp/a.txt", "/tmp/b.txt"] alt true"#]);

    cx.begin_external_drag(b, DragItem::many(DragItem::FILES, ["/tmp/c.txt"]));
    assert!(!cx.drag_released(b, Point { x: 30., y: 5. }), "nothing above the list takes files");
    assert!(!cx.is_dragging());
}

#[test]
fn resizes_are_reported_once_per_size() {
    let log: Log = Rc::default();
    let l = log.clone();
    let mut cx = TestApp::new();
    let w = cx.open_window(SIZE, move |_, cx| cx.new(|_| Side { log: l, name: "c", source: false })).id();
    let sizes = |log: &Log| take(log).into_iter().filter(|l| l.contains("size")).collect::<Vec<_>>();
    assert_eq!(sizes(&log), ["c size 400"], "the first layout reports");
    cx.refresh_window(w);
    cx.run_until_idle();
    assert!(sizes(&log).is_empty(), "the same size is not reported again");
    cx.resize_window(w, Size { width: 500., height: 300. });
    cx.run_until_idle();
    assert_eq!(sizes(&log), ["c size 500"]);
}

struct Folders {
    log: Log,
    /// Whether the card is there: a spring-loaded folder can replace the page it was on.
    card: Rc<std::cell::Cell<bool>>,
}

impl Render for Folders {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let folder = |id: &'static str, x: f32| {
            let (over, dropped, sprung) = (self.log.clone(), self.log.clone(), self.log.clone());
            div()
                .id(id)
                .absolute()
                .left(px(x))
                .top(px(100.))
                .w(px(80.))
                .h(px(60.))
                .draggable(DragItem::new("note", id))
                .lift_on_drag()
                .accepts_drops(["note"])
                .on_drag_over(move |e: &DragOver, _, _| {
                    if e.phase != DragOverPhase::Moved {
                        over.borrow_mut().push(format!("{id} {:?}", e.phase))
                    }
                })
                .on_drop(move |e: &Dropped, _, _| dropped.borrow_mut().push(format!("{} into {id}", e.item.id)))
                .on_spring_load(move |e: &SpringLoaded, _, _| {
                    sprung.borrow_mut().push(format!("{} opens {id}", e.item.id))
                })
        };
        let end = self.log.clone();
        div()
            .size_full()
            .relative()
            .when(self.card.get(), |d| {
                d.child(
                    div()
                        .id("card")
                        .absolute()
                        .left(px(10.))
                        .top(px(10.))
                        .w(px(80.))
                        .h(px(60.))
                        .draggable(DragItem::new("note", "card"))
                        .lift_on_drag()
                        .on_drag_end(move |e: &DragEnded, _, _| end.borrow_mut().push(format!("end {:?}", e.outcome))),
                )
            })
            .child(folder("recipes", 200.))
            .child(folder("journal", 300.))
    }
}

fn open_folders() -> (TestApp, Log, WindowId) {
    let (cx, log, w, _) = open_folders_with_card();
    (cx, log, w)
}

fn open_folders_with_card() -> (TestApp, Log, WindowId, Rc<std::cell::Cell<bool>>) {
    let log: Log = Rc::default();
    let l = log.clone();
    let card = Rc::new(std::cell::Cell::new(true));
    let c = card.clone();
    let mut cx = TestApp::new();
    let w = cx.open_window(SIZE, move |_, cx| cx.new(|_| Folders { log: l, card: c })).id();
    cx.backend_mut().set_window_frame(w, Rect::new(100., 50., 400., 300.));
    (cx, log, w, card)
}

/// A drag of `source` pressed at `start`, now at `to`, in window coordinates.
fn drag_from(cx: &mut TestApp, source: NodeId, phase: GesturePhase, start: Point, to: Point) {
    let translation = Point { x: to.x - start.x, y: to.y - start.y };
    cx.emit(
        source,
        DragEvent { phase, start, position: to, translation, velocity: translation, modifiers: Default::default() },
    );
}

fn frames(cx: &mut TestApp, seconds: f64) {
    for _ in 0..(seconds / 0.1).round() as usize {
        cx.advance_clock(0.1);
        cx.frame();
    }
}

#[test]
fn a_lifted_picture_keeps_its_grab_and_shrinks_into_the_target() {
    let (mut cx, log, w) = open_folders();
    let card = node(&cx, "card");
    // Grabbed 15 by 20 into the card.
    let start = Point { x: 25., y: 30. };
    drag_from(&mut cx, card, GesturePhase::Began, start, Point { x: 30., y: 30. });
    drag_from(&mut cx, card, GesturePhase::Changed, start, Point { x: 240., y: 130. });
    let at = |p: &DragPreview| match p {
        DragPreview::At { frame, lifted: true } => Some((frame.origin.x, frame.origin.y)),
        _ => None,
    };
    let followed: Vec<_> = cx.backend().previews().iter().filter_map(|(_, p)| at(p)).collect();
    // The window sits at 100, 50 on screen.
    assert_eq!(followed, [(115., 60.), (325., 160.)], "the grab point stays under the pointer");
    drag_from(&mut cx, card, GesturePhase::Ended, start, Point { x: 240., y: 130. });
    cx.drain_events();
    let landed = cx.backend().previews().last().map(|(_, p)| *p);
    assert_eq!(
        landed,
        Some(DragPreview::Land { frame: Rect::new(300., 150., 80., 60.), into: true }),
        "into the folder's frame on screen"
    );
    assert_eq!(
        take(&log),
        ["recipes Entered", "card into recipes", format!("end Dropped {{ window: {w:?} }}").as_str()]
    );
}

#[test]
fn a_cancelled_lift_settles_back_on_its_source() {
    let (mut cx, log, w) = open_folders();
    let card = node(&cx, "card");
    let start = Point { x: 25., y: 30. };
    drag_from(&mut cx, card, GesturePhase::Began, start, Point { x: 30., y: 30. });
    drag_from(&mut cx, card, GesturePhase::Changed, start, Point { x: 150., y: 250. });
    cx.dispatch_keystroke(w, &Keystroke::parse("escape").unwrap());
    cx.run_until_idle();
    let landed = cx.backend().previews().last().map(|(_, p)| *p);
    assert_eq!(landed, Some(DragPreview::Land { frame: Rect::new(110., 60., 80., 60.), into: false }), "home");
    assert_eq!(take(&log), ["end Cancelled"]);
}

#[test]
fn a_spring_loaded_target_opens_after_the_delay_and_not_before() {
    let (mut cx, log, _) = open_folders();
    let card = node(&cx, "card");
    let start = Point { x: 25., y: 30. };
    drag_from(&mut cx, card, GesturePhase::Began, start, Point { x: 30., y: 30. });
    drag_from(&mut cx, card, GesturePhase::Changed, start, Point { x: 240., y: 130. });
    cx.drain_events();
    assert_eq!(take(&log), ["recipes Entered"]);
    frames(&mut cx, SPRING_LOAD_DELAY - 0.2);
    drag_from(&mut cx, card, GesturePhase::Changed, start, Point { x: 250., y: 135. });
    cx.drain_events();
    assert!(take(&log).is_empty(), "moving inside the folder keeps its time; not open yet");
    assert!(cx.needs_frame(), "the clock runs while it waits");
    frames(&mut cx, 0.3);
    assert_eq!(take(&log), ["card opens recipes"]);
    frames(&mut cx, 2.);
    assert!(take(&log).is_empty(), "once per stay");
    assert!(!cx.needs_frame());
    // Leaving for another folder starts over there.
    drag_from(&mut cx, card, GesturePhase::Changed, start, Point { x: 340., y: 130. });
    cx.drain_events();
    frames(&mut cx, SPRING_LOAD_DELAY - 0.1);
    assert_eq!(take(&log), ["recipes Exited", "journal Entered"]);
    frames(&mut cx, 0.2);
    assert_eq!(take(&log), ["card opens journal"]);
}

#[test]
fn nothing_takes_a_drag_onto_itself() {
    let (mut cx, log, _) = open_folders();
    let recipes = node(&cx, "recipes");
    let start = Point { x: 240., y: 130. };
    drag_from(&mut cx, recipes, GesturePhase::Began, start, Point { x: 245., y: 130. });
    drag_from(&mut cx, recipes, GesturePhase::Changed, start, Point { x: 250., y: 140. });
    frames(&mut cx, 1.);
    assert!(take(&log).is_empty(), "the folder under its own drag neither lights up nor opens");
    drag_from(&mut cx, recipes, GesturePhase::Changed, start, Point { x: 340., y: 130. });
    cx.drain_events();
    assert_eq!(take(&log), ["journal Entered"]);
}

#[test]
fn the_window_carries_a_drag_on_once_its_source_is_gone() {
    let (mut cx, log, w, card) = open_folders_with_card();
    let source = node(&cx, "card");
    let start = Point { x: 25., y: 30. };
    drag_from(&mut cx, source, GesturePhase::Began, start, Point { x: 30., y: 30. });
    drag_from(&mut cx, source, GesturePhase::Changed, start, Point { x: 60., y: 40. });
    cx.drag_pointer(w, Point { x: 240., y: 130. }, false);
    cx.drain_events();
    assert!(take(&log).is_empty(), "the source's gesture reports while it is there");
    card.set(false);
    cx.refresh_window(w);
    cx.run_until_idle();
    assert!(!cx.backend().is_live(source), "a spring-loaded page took it away");
    cx.drag_pointer(w, Point { x: 340., y: 130. }, false);
    cx.drag_pointer(w, Point { x: 340., y: 130. }, true);
    cx.drain_events();
    assert_eq!(take(&log), ["journal Entered", "card into journal"]);
    let landed = cx.backend().previews().last().map(|(_, p)| *p);
    assert!(matches!(landed, Some(DragPreview::Land { into: true, .. })), "{landed:?}");
}

#[test]
fn a_release_queued_after_the_source_left_still_ends_its_drag() {
    let (mut cx, log, w, card) = open_folders_with_card();
    let source = node(&cx, "card");
    let start = Point { x: 25., y: 30. };
    drag_from(&mut cx, source, GesturePhase::Began, start, Point { x: 30., y: 30. });
    drag_from(&mut cx, source, GesturePhase::Changed, start, Point { x: 340., y: 130. });
    card.set(false);
    cx.refresh_window(w);
    cx.run_until_idle();
    assert!(!cx.backend().is_live(source));
    drag_from(&mut cx, source, GesturePhase::Ended, start, Point { x: 340., y: 130. });
    cx.drain_events();
    assert!(!cx.is_dragging());
    assert_eq!(take(&log), ["journal Entered", "card into journal"]);
}

#[test]
fn a_window_idle_for_a_while_does_not_spring_a_target_at_once() {
    let (mut cx, log, _) = open_folders();
    let card = node(&cx, "card");
    let start = Point { x: 25., y: 30. };
    drag_from(&mut cx, card, GesturePhase::Began, start, Point { x: 30., y: 30. });
    // Nothing animates, so no frame runs while the clock goes on.
    cx.advance_clock(5.);
    drag_from(&mut cx, card, GesturePhase::Changed, start, Point { x: 240., y: 130. });
    cx.drain_events();
    frames(&mut cx, 0.1);
    assert_eq!(take(&log), ["recipes Entered"], "the stay starts at the drag's first frame over it");
    frames(&mut cx, SPRING_LOAD_DELAY);
    assert_eq!(take(&log), ["card opens recipes"]);
}
