//! Element animation on the frame clock and hover, tier 1: presented values reach the backend
//! as props on each frame, a new target mid-flight continues from where the motion is, and
//! reduce motion dissolves instead of moving.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use nd_gpui::gpui::{SharedString, px};
use nd_ui::test_app::TestApp;
use nd_ui::*;

const WINDOW: Size = Size { width: 400., height: 300. };
const FRAME: f64 = 1. / 60.;

struct Card {
    shown: bool,
    rows: usize,
    ended: Rc<RefCell<u32>>,
}

impl Render for Card {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ended = self.ended.clone();
        v_flex()
            .child(
                div()
                    .id("card")
                    .w(px(if self.shown { 200. } else { 100. }))
                    .h(px(40.))
                    .opacity(if self.shown { 1. } else { 0. })
                    .offset(px(0.), px(if self.shown { 0. } else { 20. }))
                    .animate(Animation::linear(Duration::from_millis(200)))
                    .on_animation_end(move |_, _, _| *ended.borrow_mut() += 1),
            )
            .children((0..self.rows).map(|i| {
                div()
                    .id(SharedString::from(format!("row{i}")))
                    .h(px(10.))
                    .enter_from(|p| p.opacity(0.).offset(px(0.), px(8.)))
            }))
    }
}

fn props(cx: &TestApp, id: NodeId) -> DivProps {
    cx.backend().props(id).and_then(|p| p.downcast_ref::<DivProps>()).cloned().expect("div")
}

fn node(cx: &TestApp, test_id: &str) -> NodeId {
    let tree = cx.semantic_tree();
    tree.nodes().find(|(_, n)| n.test_id.as_deref() == Some(test_id)).map(|(_, n)| n.id).expect(test_id)
}

fn step(cx: &mut TestApp, seconds: f64) {
    cx.advance_clock(seconds);
    cx.frame();
}

fn open(cx: &mut TestApp, ended: Rc<RefCell<u32>>) -> WindowHandle<Card> {
    cx.open_window(WINDOW, move |_, cx| cx.new(|_| Card { shown: false, rows: 0, ended }))
}

#[test]
fn a_changed_target_animates_on_the_frame_clock_and_reports_its_end() {
    let ended = Rc::new(RefCell::new(0));
    let mut cx = TestApp::new();
    let handle = open(&mut cx, ended.clone());
    let card = node(&cx, "card");
    assert_eq!(props(&cx, card).style.opacity, Some(0.), "a first render shows its values at once");
    assert!(!cx.needs_frame(), "nothing moves, nothing draws");

    handle.root().update(&mut cx, |c, cx| {
        c.shown = true;
        cx.notify();
    });
    cx.frame();
    assert!(cx.needs_frame(), "a motion in flight asks for frames");
    step(&mut cx, 0.1);
    let mid = props(&cx, card);
    assert!((mid.style.opacity.unwrap() - 0.5).abs() < 0.02, "halfway at 100 ms: {:?}", mid.style.opacity);
    assert!((mid.transform.offset.1 - 10.).abs() < 0.5);
    let width = cx.backend().frame(card).unwrap().size.width;
    assert!((width - 150.).abs() < 2., "the size animates through layout: {width}");

    for _ in 0..12 {
        step(&mut cx, FRAME);
    }
    cx.run_until_idle();
    let done = props(&cx, card);
    assert_eq!(done.style.opacity, Some(1.));
    assert_eq!(done.transform.offset, (0., 0.));
    assert_eq!(cx.backend().frame(card).unwrap().size.width, 200.);
    assert!(!cx.needs_frame(), "a settled window is idle");
    assert_eq!(*ended.borrow(), 1);
}

#[test]
fn a_motion_restarted_in_the_frame_it_settles_reports_one_end() {
    let ended = Rc::new(RefCell::new(0));
    let mut cx = TestApp::new();
    let handle = open(&mut cx, ended.clone());
    handle.root().update(&mut cx, |c, cx| {
        c.shown = true;
        cx.notify();
    });
    cx.frame();
    cx.advance_clock(0.3);
    handle.root().update(&mut cx, |c, cx| {
        c.shown = false;
        cx.notify();
    });
    cx.frame();
    step(&mut cx, FRAME);
    assert_eq!(*ended.borrow(), 0, "the first leg ended as the second began: not idle yet");
    step(&mut cx, 0.3);
    cx.run_until_idle();
    assert_eq!(*ended.borrow(), 1);
}

#[test]
fn a_reversal_mid_flight_continues_from_where_it_is() {
    let mut cx = TestApp::new();
    let handle = open(&mut cx, Rc::default());
    let card = node(&cx, "card");
    handle.root().update(&mut cx, |c, cx| {
        c.shown = true;
        cx.notify();
    });
    cx.frame();
    step(&mut cx, 0.05);
    let at = props(&cx, card).style.opacity.unwrap();
    handle.root().update(&mut cx, |c, cx| {
        c.shown = false;
        cx.notify();
    });
    cx.frame();
    let after = props(&cx, card).style.opacity.unwrap();
    assert!((after - at).abs() < 0.02, "no jump back to either end: {at} then {after}");
    step(&mut cx, 0.1);
    assert!(props(&cx, card).style.opacity.unwrap() < after, "heading back down");
}

#[test]
fn mounted_rows_enter_from_their_pose() {
    let mut cx = TestApp::new();
    let handle = open(&mut cx, Rc::default());
    handle.root().update(&mut cx, |c, cx| {
        c.rows = 1;
        cx.notify();
    });
    cx.frame();
    let row = node(&cx, "row0");
    let first = props(&cx, row);
    assert_eq!(first.style.opacity, Some(0.), "a new row starts transparent");
    assert_eq!(first.transform.offset.1, 8.);
    for _ in 0..30 {
        step(&mut cx, FRAME);
    }
    let last = props(&cx, row);
    assert_eq!(last.style.opacity, Some(1.));
    assert_eq!(last.transform.offset.1, 0.);
    handle.root().update(&mut cx, |c, cx| {
        c.rows = 2;
        cx.notify();
    });
    cx.frame();
    assert_eq!(props(&cx, row).style.opacity.unwrap_or(1.), 1., "a row already mounted does not enter again");
}

#[test]
fn reduce_motion_dissolves_a_move_and_a_resize() {
    let mut cx = TestApp::new();
    cx.set_reduce_motion(true);
    let handle = open(&mut cx, Rc::default());
    let card = node(&cx, "card");
    handle.root().update(&mut cx, |c, cx| {
        c.shown = true;
        cx.notify();
    });
    cx.frame();
    step(&mut cx, 0.05);
    let out = props(&cx, card);
    assert_eq!(out.transform.offset.1, 20., "nothing travels: it holds its old place");
    assert_eq!(cx.backend().frame(card).unwrap().size.width, 100., "and its old size");
    step(&mut cx, 0.1);
    let back = props(&cx, card);
    assert_eq!(back.transform.offset.1, 0., "then shows at the new one");
    assert_eq!(cx.backend().frame(card).unwrap().size.width, 200.);
    let o = back.style.opacity.unwrap();
    assert!(o > 0. && o < 1., "fading in: {o}");
}

struct Hoverable {
    log: Rc<RefCell<Vec<bool>>>,
}

impl Render for Hoverable {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let log = self.log.clone();
        div()
            .id("row")
            .hover(|s| s.bg(nd_gpui::gpui::red()))
            .on_hover(move |hovered, _, _| log.borrow_mut().push(*hovered))
    }
}

#[test]
fn hover_style_reaches_the_backend_and_on_hover_hears_enter_and_exit() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut cx = TestApp::new();
    let l = log.clone();
    cx.open_window(WINDOW, move |_, cx| cx.new(|_| Hoverable { log: l }));
    let row = node(&cx, "row");
    let p = props(&cx, row);
    assert!(p.tracks_hover);
    assert!(p.hover.as_ref().and_then(|h| h.background.as_ref()).is_some(), "the backend applies it itself");
    cx.emit(row, HoverChanged { hovered: true });
    cx.emit(row, HoverChanged { hovered: false });
    cx.run_until_idle();
    assert_eq!(*log.borrow(), [true, false]);
}

struct Tile;

impl Render for Tile {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tile")
            .group("tile")
            .size(px(100.))
            .child(div().id("play").size(px(20.)).reveal_on_hover("tile"))
            .child(div().id("other").size(px(20.)).reveal_on_hover("row"))
    }
}

fn hover(cx: &mut TestApp, node: NodeId, hovered: bool) {
    cx.events().emitter(node).emit(HoverChanged { hovered });
    cx.drain_events();
    cx.frame();
}

#[test]
fn a_hover_group_fades_its_reveals_in_and_out_on_the_platform_timing() {
    let mut cx = TestApp::new();
    cx.open_window(WINDOW, |_, cx| cx.new(|_| Tile));
    let (tile, play, other) = (node(&cx, "tile"), node(&cx, "play"), node(&cx, "other"));
    assert!(props(&cx, tile).tracks_hover, "a group hears the pointer");
    assert_eq!(props(&cx, play).style.opacity, Some(0.));
    assert!(!cx.needs_frame());

    hover(&mut cx, tile, true);
    let fast = cx.motion().fast.as_secs_f64();
    step(&mut cx, fast / 2.);
    let mid = props(&cx, play).style.opacity.unwrap();
    assert!(mid > 0.05 && mid < 0.95, "fading in: {mid}");
    assert_eq!(props(&cx, other).style.opacity, Some(0.), "another group's reveal stays hidden");
    step(&mut cx, fast);
    assert_eq!(props(&cx, play).style.opacity, Some(1.));
    assert!(!cx.needs_frame(), "idle once shown");

    hover(&mut cx, tile, false);
    step(&mut cx, fast / 2.);
    assert!(props(&cx, play).style.opacity.unwrap() < 1.);
    step(&mut cx, fast);
    assert_eq!(props(&cx, play).style.opacity, Some(0.));
}

#[test]
fn a_themed_animation_follows_the_apps_motion_tokens() {
    let mut cx = TestApp::new();
    cx.set_motion(MotionTokens { fast: Duration::from_millis(400), curve: Curve::Linear, ..MotionTokens::default() });
    cx.open_window(WINDOW, |_, cx| cx.new(|_| Tile));
    let (tile, play) = (node(&cx, "tile"), node(&cx, "play"));
    hover(&mut cx, tile, true);
    step(&mut cx, 0.1);
    let at = props(&cx, play).style.opacity.unwrap();
    assert!((at - 0.25).abs() < 0.03, "a quarter of 400 ms, linear: {at}");
}

struct Pressable;

impl Render for Pressable {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(div().id("card").size(px(100.)).active(|p| p.scale(0.9)))
    }
}

fn press(cx: &mut TestApp, node: NodeId, pressed: bool) {
    cx.events().emitter(node).emit(PressChanged { pressed });
    cx.drain_events();
    cx.frame();
}

#[test]
fn an_active_pose_dips_while_a_press_is_held_and_comes_back_on_release() {
    let mut cx = TestApp::new();
    cx.open_window(WINDOW, |_, cx| cx.new(|_| Pressable));
    let card = node(&cx, "card");
    assert_eq!(props(&cx, card).active, Some(Pose::default().scale(0.9)), "the backend knows to report presses");
    assert_eq!(props(&cx, card).transform.scale, 1.);

    press(&mut cx, card, true);
    let fast = cx.motion().fast.as_secs_f64();
    step(&mut cx, fast / 2.);
    let mid = props(&cx, card).transform.scale;
    assert!(mid < 0.99 && mid > 0.91, "dipping on the platform timing: {mid}");
    step(&mut cx, fast);
    assert_eq!(props(&cx, card).transform.scale, 0.9);
    assert!(!cx.needs_frame(), "held still while pressed");

    press(&mut cx, card, false);
    step(&mut cx, fast / 2.);
    assert!(props(&cx, card).transform.scale < 1.);
    step(&mut cx, fast);
    assert_eq!(props(&cx, card).transform.scale, 1., "back on release");
}

struct Presented {
    open: bool,
    away: bool,
    landed: Rc<RefCell<u32>>,
}

impl Render for Presented {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let landed = self.landed.clone();
        div().size_full().child(div().id("thumb").absolute().left(px(10.)).top(px(250.)).size(px(40.))).when(
            self.open,
            |root| {
                root.child(
                    div().absolute().left(px(100.)).top(px(20.)).size(px(200.)).child(
                        div()
                            .id("cover")
                            .absolute()
                            .size(px(200.))
                            .hero_from("thumb", self.away)
                            .animate(Animation::linear(Duration::from_millis(200)))
                            .on_animation_end(move |_, _, _| *landed.borrow_mut() += 1),
                    ),
                )
            },
        )
    }
}

#[test]
fn a_hero_comes_in_from_its_source_frame_and_goes_back_to_it() {
    let mut cx = TestApp::new();
    let handle =
        cx.open_window(WINDOW, |_, cx| cx.new(|_| Presented { open: false, away: false, landed: Rc::default() }));
    handle.root().update(&mut cx, |p, cx| {
        p.open = true;
        cx.notify();
    });
    cx.frame();
    let cover = node(&cx, "cover");
    assert_eq!(props(&cx, cover).style.opacity, Some(0.), "hidden until its frame is known");
    assert!(cx.needs_frame());
    cx.frame();
    let start = props(&cx, cover);
    assert_eq!(start.style.opacity, Some(1.));
    assert!((start.transform.offset.0 - -90.).abs() < 0.5, "on the thumb: {:?}", start.transform.offset);
    assert!((start.transform.offset.1 - 230.).abs() < 0.5);
    assert!((cx.backend().frame(cover).unwrap().size.width - 40.).abs() < 0.5, "at the thumb's size");
    step(&mut cx, 0.25);
    cx.run_until_idle();
    assert_eq!(props(&cx, cover).transform.offset, (0., 0.));
    assert_eq!(cx.backend().frame(cover).unwrap().size.width, 200.);

    handle.root().update(&mut cx, |p, cx| {
        p.away = true;
        cx.notify();
    });
    cx.frame();
    cx.frame();
    step(&mut cx, 0.1);
    assert_eq!(props(&cx, cover).transform.offset, (0., 0.), "waits while what surrounds it fades");
    step(&mut cx, 0.3);
    let back = props(&cx, cover);
    assert!((back.transform.offset.0 - -90.).abs() < 0.5, "back on the thumb: {:?}", back.transform.offset);
    assert!((cx.backend().frame(cover).unwrap().size.width - 40.).abs() < 0.5);
}

/// Frames on a ticking clock until nothing moves.
fn settle(cx: &mut TestApp) {
    for _ in 0..240 {
        cx.flush_effects();
        cx.drain_events();
        if !cx.needs_frame() && cx.events().is_empty() {
            return;
        }
        step(cx, FRAME);
    }
    panic!("still moving after four seconds");
}

/// A hero whose source is not on screen: the presented view opened from somewhere else.
struct Unsourced {
    open: bool,
    away: bool,
    landed: Rc<RefCell<u32>>,
}

impl Render for Unsourced {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let landed = self.landed.clone();
        div().size_full().when(self.open, |root| {
            root.child(
                div()
                    .id("cover")
                    .absolute()
                    .left(px(100.))
                    .top(px(20.))
                    .size(px(200.))
                    .hero_from("nowhere", self.away)
                    .animate(Animation::linear(Duration::from_millis(200)))
                    .on_animation_end(move |_, _, _| *landed.borrow_mut() += 1),
            )
        })
    }
}

#[test]
fn a_hero_with_no_source_rises_and_fades_in_and_fades_out_going_back() {
    let mut cx = TestApp::new();
    let landed = Rc::new(RefCell::new(0));
    let handle = cx.open_window(WINDOW, {
        let landed = landed.clone();
        move |_, cx| cx.new(|_| Unsourced { open: false, away: false, landed })
    });
    handle.root().update(&mut cx, |p, cx| {
        p.open = true;
        cx.notify();
    });
    cx.frame();
    let cover = node(&cx, "cover");
    step(&mut cx, FRAME);
    step(&mut cx, 0.1);
    let mid = props(&cx, cover);
    let o = mid.style.opacity.unwrap();
    assert!(o > 0. && o < 1., "fading in: {o}");
    assert!(mid.transform.offset.1 > 0. && mid.transform.offset.1 < 16., "rising: {:?}", mid.transform.offset);
    settle(&mut cx);
    assert_eq!(props(&cx, cover).transform.offset, (0., 0.));
    *landed.borrow_mut() = 0;
    handle.root().update(&mut cx, |p, cx| {
        p.away = true;
        cx.notify();
    });
    cx.frame();
    step(&mut cx, FRAME);
    step(&mut cx, 0.1);
    let o = props(&cx, cover).style.opacity.unwrap();
    assert!(o > 0. && o < 1., "fading out where it is: {o}");
    assert_eq!(props(&cx, cover).transform.offset, (0., 0.));
    settle(&mut cx);
    assert_eq!(props(&cx, cover).style.opacity, Some(0.));
    assert_eq!(*landed.borrow(), 1, "lands when the fade does");
}

#[test]
fn reduce_motion_fades_a_hero_in_where_it_lands() {
    let mut cx = TestApp::new();
    cx.set_reduce_motion(true);
    let handle =
        cx.open_window(WINDOW, |_, cx| cx.new(|_| Presented { open: false, away: false, landed: Rc::default() }));
    handle.root().update(&mut cx, |p, cx| {
        p.open = true;
        cx.notify();
    });
    cx.frame();
    let cover = node(&cx, "cover");
    assert_eq!(props(&cx, cover).style.opacity, Some(0.), "comes in from nothing");
    step(&mut cx, 0.1);
    let mid = props(&cx, cover);
    let o = mid.style.opacity.unwrap();
    assert!(o > 0. && o < 1., "fading in: {o}");
    assert_eq!(mid.transform.offset, (0., 0.), "in place");
    assert_eq!(mid.transform.scale, 1.);
    assert_eq!(cx.backend().frame(cover).unwrap().size.width, 200., "at its own size");
    settle(&mut cx);
    assert_eq!(props(&cx, cover).style.opacity.unwrap_or(1.), 1.);
}

#[test]
fn reduce_motion_fades_a_hero_going_back_where_it_is_and_lands_once() {
    let mut cx = TestApp::new();
    cx.set_reduce_motion(true);
    let landed = Rc::new(RefCell::new(0));
    let handle = cx.open_window(WINDOW, {
        let landed = landed.clone();
        move |_, cx| cx.new(|_| Presented { open: false, away: false, landed })
    });
    handle.root().update(&mut cx, |p, cx| {
        p.open = true;
        cx.notify();
    });
    settle(&mut cx);
    let cover = node(&cx, "cover");
    *landed.borrow_mut() = 0;
    handle.root().update(&mut cx, |p, cx| {
        p.away = true;
        cx.notify();
    });
    cx.frame();
    let mut last = 1.;
    for frame in 0..30 {
        step(&mut cx, FRAME);
        let shown = props(&cx, cover);
        let o = shown.style.opacity.unwrap_or(1.);
        assert!(o <= last + 1e-6, "frame {frame} brightens: {last} then {o}");
        assert_eq!(shown.transform.offset, (0., 0.), "no travel");
        assert_eq!(cx.backend().frame(cover).unwrap().size.width, 200., "never shrinks onto the thumb");
        last = o;
    }
    settle(&mut cx);
    assert_eq!(props(&cx, cover).style.opacity, Some(0.), "faded out");
    assert_eq!(*landed.borrow(), 1, "a presentation waiting on it can close");
    handle.root().update(&mut cx, |_, cx| cx.notify());
    settle(&mut cx);
    assert_eq!(*landed.borrow(), 1, "lands once, not on every render");
    assert_eq!(props(&cx, cover).style.opacity, Some(0.), "stays hidden");
}

struct Notes {
    ids: Vec<&'static str>,
    log: Rc<RefCell<Vec<&'static str>>>,
}

impl Render for Notes {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().children(self.ids.iter().map(|&id| {
            let log = self.log.clone();
            div()
                .id(id)
                .h(px(20.))
                .w(px(100.))
                .animate(Animation::linear(Duration::from_millis(200)))
                .animate_layout(Animation::linear(Duration::from_millis(200)))
                .exit_to(|p| p.opacity(0.).scale(0.5))
                .on_tap(move |_, _, _| log.borrow_mut().push(id))
        }))
    }
}

fn notes(cx: &mut TestApp, ids: &[&'static str]) -> (WindowHandle<Notes>, Rc<RefCell<Vec<&'static str>>>) {
    let log: Rc<RefCell<Vec<&'static str>>> = Rc::default();
    let (ids, l) = (ids.to_vec(), log.clone());
    (cx.open_window(WINDOW, move |_, cx| cx.new(|_| Notes { ids, log: l })), log)
}

fn set_ids(cx: &mut TestApp, handle: &WindowHandle<Notes>, ids: &[&'static str]) {
    let ids = ids.to_vec();
    handle.root().update(cx, |n, cx| {
        n.ids = ids;
        cx.notify();
    });
    cx.frame();
}

fn tap(cx: &mut TestApp, node: NodeId) {
    cx.events().emitter(node).emit(TapEvent::new(Point::default(), 1));
    cx.drain_events();
}

/// Where the node shows: its laid-out frame plus its offset.
fn shown_y(cx: &TestApp, id: NodeId) -> f32 {
    cx.backend().frame(id).unwrap().origin.y + props(cx, id).transform.offset.1
}

#[test]
fn an_exiting_element_stays_drawn_where_it_was_until_its_exit_settles() {
    let mut cx = TestApp::new();
    let (handle, log) = notes(&mut cx, &["a", "b", "c"]);
    let (a, b, c) = (node(&cx, "a"), node(&cx, "b"), node(&cx, "c"));
    let list = cx.backend().parent(a).unwrap();
    set_ids(&mut cx, &handle, &["a", "c"]);
    assert!(cx.backend().is_live(b), "held while it exits");
    assert_eq!(cx.backend().children(list), [a, b, c], "drawn in its place among the others");
    assert_eq!(cx.backend().frame(b).unwrap().origin.y, 20., "where it stood");
    assert!(props(&cx, b).inert, "takes no input");
    let tree = cx.semantic_tree();
    assert!(tree.nodes().all(|(_, n)| n.test_id.as_deref() != Some("b")), "out of the accessibility tree");
    tap(&mut cx, b);
    assert!(log.borrow().is_empty(), "a tap on it is not heard");

    step(&mut cx, 0.1);
    let mid = props(&cx, b);
    assert!((mid.style.opacity.unwrap() - 0.5).abs() < 0.02, "fading: {:?}", mid.style.opacity);
    assert!((mid.transform.scale - 0.75).abs() < 0.02, "shrinking: {}", mid.transform.scale);
    assert!(cx.backend().is_live(b));
    step(&mut cx, 0.15);
    cx.run_until_idle();
    assert!(!cx.backend().is_live(b), "dropped once settled");
    assert_eq!(cx.backend().children(list), [a, c]);
    assert!(!cx.needs_frame());
}

#[test]
fn a_key_back_mid_exit_turns_around_from_where_it_is() {
    let mut cx = TestApp::new();
    let (handle, log) = notes(&mut cx, &["a", "b"]);
    let b = node(&cx, "b");
    set_ids(&mut cx, &handle, &["a"]);
    step(&mut cx, 0.1);
    let at = props(&cx, b).style.opacity.unwrap();
    set_ids(&mut cx, &handle, &["a", "b"]);
    assert_eq!(node(&cx, "b"), b, "the same node, not a new one");
    let back = props(&cx, b);
    assert!(!back.inert);
    assert!((back.style.opacity.unwrap() - at).abs() < 0.02, "no jump: {at} then {:?}", back.style.opacity);
    step(&mut cx, 0.05);
    assert!(props(&cx, b).style.opacity.unwrap() > at, "heading back in");
    step(&mut cx, 0.3);
    cx.run_until_idle();
    assert_eq!(props(&cx, b).style.opacity, Some(1.));
    assert_eq!(props(&cx, b).transform.scale, 1.);
    assert!(cx.backend().is_live(b), "kept");
    tap(&mut cx, b);
    assert_eq!(*log.borrow(), ["b"], "takes input again");
}

#[test]
fn siblings_glide_into_the_gap_and_turn_mid_flight() {
    let mut cx = TestApp::new();
    let (handle, _) = notes(&mut cx, &["a", "b", "c"]);
    let c = node(&cx, "c");
    assert_eq!(props(&cx, c).transform.offset, (0., 0.), "no glide on first mount");
    assert!(!cx.needs_frame());

    set_ids(&mut cx, &handle, &["a", "c"]);
    assert_eq!(cx.backend().frame(c).unwrap().origin.y, 20., "laid out in the gap at once");
    assert_eq!(shown_y(&cx, c), 40., "but drawn where it was");
    step(&mut cx, 0.1);
    let mid = shown_y(&cx, c);
    assert!((mid - 30.).abs() < 0.5, "halfway: {mid}");

    set_ids(&mut cx, &handle, &["c"]);
    assert!((shown_y(&cx, c) - mid).abs() < 0.5, "turns from where it is: {mid} then {}", shown_y(&cx, c));
    step(&mut cx, 0.25);
    cx.run_until_idle();
    assert_eq!(cx.backend().frame(c).unwrap().origin.y, 0.);
    assert_eq!(props(&cx, c).transform.offset, (0., 0.));
}

#[test]
fn an_inserted_element_pushes_its_siblings_down_smoothly() {
    let mut cx = TestApp::new();
    let (handle, _) = notes(&mut cx, &["b"]);
    let b = node(&cx, "b");
    set_ids(&mut cx, &handle, &["a", "b"]);
    assert_eq!(shown_y(&cx, b), 0., "starts where it was");
    assert_eq!(props(&cx, node(&cx, "a")).transform.offset, (0., 0.), "the new one does not glide");
    step(&mut cx, 0.1);
    assert!((shown_y(&cx, b) - 10.).abs() < 0.5, "{}", shown_y(&cx, b));
}

#[test]
fn reduce_motion_dissolves_layout_and_only_fades_an_exit() {
    let mut cx = TestApp::new();
    cx.set_reduce_motion(true);
    let (handle, _) = notes(&mut cx, &["a", "b", "c"]);
    let (b, c) = (node(&cx, "b"), node(&cx, "c"));
    set_ids(&mut cx, &handle, &["a", "c"]);
    assert_eq!(shown_y(&cx, c), 40., "the sibling holds its place");
    step(&mut cx, 0.05);
    assert_eq!(shown_y(&cx, c), 40.);
    assert!(props(&cx, c).style.opacity.unwrap() < 1., "while it fades out");
    let exit = props(&cx, b);
    assert!(exit.style.opacity.unwrap() < 1., "the exit still fades");
    assert_eq!(exit.transform.scale, 1., "but does not shrink");
    step(&mut cx, 0.1);
    assert_eq!(shown_y(&cx, c), 20., "then shows at its new place");
    let o = props(&cx, c).style.opacity.unwrap();
    assert!(o > 0. && o < 1., "fading in: {o}");
    settle(&mut cx);
    assert_eq!(props(&cx, c).style.opacity.unwrap_or(1.), 1.);
}

struct LayoutOnly {
    top: bool,
}

impl Render for LayoutOnly {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().when(self.top, |d| d.child(div().id("top").h(px(30.)))).child(
            div()
                .id("moving")
                .h(px(10.))
                .offset(px(5.), px(0.))
                .animate_layout(Animation::linear(Duration::from_millis(100))),
        )
    }
}

#[test]
fn a_layout_glide_composes_with_the_elements_own_offset() {
    let mut cx = TestApp::new();
    let handle = cx.open_window(WINDOW, |_, cx| cx.new(|_| LayoutOnly { top: false }));
    let moving = node(&cx, "moving");
    handle.root().update(&mut cx, |v, cx| {
        v.top = true;
        cx.notify();
    });
    cx.frame();
    assert_eq!(props(&cx, moving).transform.offset, (5., -30.));
    step(&mut cx, 0.05);
    assert!((props(&cx, moving).transform.offset.1 - -15.).abs() < 0.5);
    step(&mut cx, 0.1);
    cx.run_until_idle();
    assert_eq!(props(&cx, moving).transform.offset, (5., 0.), "lands on its own offset");
}

#[test]
fn a_backend_that_lays_out_as_it_paints_glides_from_the_painted_frames() {
    let mut cx = TestApp::with_backend(nd_ui::test_backend::TestBackend::new().without_frames());
    let handle = cx.open_window(WINDOW, |_, cx| cx.new(|_| LayoutOnly { top: false }));
    let moving = node(&cx, "moving");
    assert!(props(&cx, moving).glides, "the backend is told it glides");
    let paint = |cx: &mut TestApp, y: f32| cx.backend_mut().set_layout_frame(moving, Rect::new(0., y, 400., 10.));
    paint(&mut cx, 0.);
    cx.run_until_idle();
    handle.root().update(&mut cx, |v, cx| {
        v.top = true;
        cx.notify();
    });
    cx.frame();
    paint(&mut cx, 30.);
    assert!(cx.needs_frame(), "reads the frames the backend paints next");
    cx.frame();
    assert_eq!(props(&cx, moving).transform.offset, (5., -30.), "back where it was painted");
    step(&mut cx, 0.2);
    cx.run_until_idle();
    assert_eq!(props(&cx, moving).transform.offset, (5., 0.));
}

struct ScrolledNotes {
    ids: Vec<&'static str>,
}

impl Render for ScrolledNotes {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().id("scroller").h(px(40.)).overflow_y_scroll().children(self.ids.iter().map(|&id| {
            div()
                .id(id)
                .h(px(20.))
                .flex_none()
                .animate(Animation::linear(Duration::from_millis(200)))
                .animate_layout(Animation::linear(Duration::from_millis(200)))
                .exit_to(|p| p.opacity(0.))
        }))
    }
}

#[test]
fn a_backend_that_lays_out_itself_holds_an_exit_inside_a_scroll_view() {
    let mut cx = TestApp::with_backend(nd_ui::test_backend::TestBackend::new().without_frames());
    let handle = cx.open_window(WINDOW, |_, cx| cx.new(|_| ScrolledNotes { ids: vec!["a", "b", "c"] }));
    let (b, c) = (node(&cx, "b"), node(&cx, "c"));
    cx.backend_mut().set_layout_frame(b, Rect::new(0., 20., 400., 20.));
    cx.backend_mut().set_layout_frame(c, Rect::new(0., 40., 400., 20.));
    cx.run_until_idle();
    handle.root().update(&mut cx, |n, cx| {
        n.ids = vec!["a", "c"];
        cx.notify();
    });
    cx.frame();
    assert!(cx.backend().is_live(b), "held while it exits");
    assert!(props(&cx, b).inert, "takes no input");
    cx.backend_mut().set_layout_frame(c, Rect::new(0., 20., 400., 20.));
    cx.frame();
    assert_eq!(props(&cx, c).transform.offset, (0., 20.), "the row below glides up from where it was");
    step(&mut cx, 0.25);
    cx.run_until_idle();
    assert!(!cx.backend().is_live(b), "dropped once settled");
    assert_eq!(props(&cx, c).transform.offset, (0., 0.));
}

#[test]
fn under_reduce_motion_the_backend_still_holds_a_moved_node_back() {
    let mut cx = TestApp::with_backend(nd_ui::test_backend::TestBackend::new().without_frames());
    cx.set_reduce_motion(true);
    cx.open_window(WINDOW, |_, cx| cx.new(|_| LayoutOnly { top: false }));
    assert!(props(&cx, node(&cx, "moving")).glides, "a dissolve starts where it was");
}
