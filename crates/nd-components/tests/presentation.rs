//! Tier 1 behaviour of a presentation on `TestBackend`: what each frame draws while it closes.

mod common;

use common::*;
use nd_components::*;
use nd_ui::prelude::*;
use nd_ui::{DivProps, NodeId, Size};

const FRAME: f64 = 1. / 60.;

struct Shown {
    expanded: Presentation,
}

fn view(s: &Shown, _cx: &mut Context<Host<Shown>>) -> AnyElement {
    let page = div().size_full().child(div().id("thumb").absolute().left(px(10.)).top(px(580.)).size(px(48.)));
    let content = div()
        .size_full()
        .child(div().id("cover").absolute().size(px(400.)).hero_with(&s.expanded, "thumb"))
        .child(div().id("title").h(px(20.)).reveal_with(&s.expanded));
    presentation("now-playing", &s.expanded).base(page).content(content).into_any_element()
}

fn set_open(rig: &mut Rig<Shown>, open: bool) {
    rig.win.root().clone().update(&mut rig.cx, |h, cx| {
        h.s.expanded.set_open(open);
        cx.notify();
    });
}

#[test]
fn reduce_motion_crossfades_the_hero_and_never_brightens_it_after_close() {
    let mut rig = Rig::sized(Size { width: 480., height: 640. }, |_| Shown { expanded: Presentation::new() }, view);
    rig.cx.set_reduce_motion(true);
    set_open(&mut rig, true);
    while rig.cx.needs_frame() {
        rig.cx.advance_clock(FRAME);
        rig.cx.frame();
    }
    let cover = rig.node("cover");
    let open = rig.props::<DivProps>("cover");
    assert_eq!(open.style.opacity.unwrap_or(1.), 1., "faded in");
    assert_eq!(open.transform.offset, (0., 0.), "in place");
    set_open(&mut rig, false);
    let mut frames = 0;
    let mut last = 1.;
    while rig.state().expanded.is_shown() {
        rig.cx.frame();
        if let Some(props) = rig.cx.backend().props(cover) {
            let div = props.downcast_ref::<DivProps>().expect("div");
            let opacity = div.style.opacity.unwrap_or(1.);
            assert!(opacity <= last + 1e-6, "frame {frames} after close brightens the cover: {last} then {opacity}");
            assert_eq!(div.transform.offset, (0., 0.), "frame {frames}: no travel");
            last = opacity;
        }
        rig.cx.advance_clock(FRAME);
        frames += 1;
        assert!(frames < 120, "closes");
    }
    assert!(last < 1e-3, "faded out before the presentation closed: {last}");
    rig.cx.frame();
    assert!(rig.cx.backend().props(cover).is_none(), "torn down");
}

/// The view as an app might draw it: the presentation moves into a wrapper as it closes, and
/// the content may stop drawing its hero.
struct Moving {
    expanded: Presentation,
    hero: bool,
}

fn moving(s: &Moving, _cx: &mut Context<Host<Moving>>) -> AnyElement {
    let page = div().size_full().child(div().id("thumb").absolute().left(px(10.)).top(px(580.)).size(px(48.)));
    let content = div()
        .size_full()
        .when(s.hero || s.expanded.is_open(), |c| {
            c.child(div().id("cover").absolute().size(px(400.)).hero_with(&s.expanded, "thumb"))
        })
        .child(div().id("title").h(px(20.)).reveal_with(&s.expanded));
    let shown = presentation("now-playing", &s.expanded).base(page).content(content).into_any_element();
    if s.expanded.is_open() { shown } else { div().size_full().child(shown).into_any_element() }
}

fn closes(rig: &mut Rig<Moving>) {
    rig.win.root().clone().update(&mut rig.cx, |h, cx| {
        h.s.expanded.set_open(true);
        cx.notify();
    });
    while rig.cx.needs_frame() {
        rig.cx.advance_clock(FRAME);
        rig.cx.frame();
    }
    rig.win.root().clone().update(&mut rig.cx, |h, cx| {
        h.s.expanded.set_open(false);
        cx.notify();
    });
    let mut frames = 0;
    while rig.state().expanded.is_shown() {
        rig.cx.frame();
        rig.cx.advance_clock(FRAME);
        frames += 1;
        assert!(frames < 240, "still closing after {frames} frames");
    }
}

#[test]
fn a_presentation_mounted_again_as_it_closes_still_closes() {
    let mut rig = Rig::sized(
        Size { width: 480., height: 640. },
        |_| Moving { expanded: Presentation::new(), hero: true },
        moving,
    );
    closes(&mut rig);
}

#[test]
fn a_close_whose_hero_never_heads_back_ends_with_the_fade() {
    let mut rig = Rig::sized(
        Size { width: 480., height: 640. },
        |_| Moving { expanded: Presentation::new(), hero: false },
        moving,
    );
    closes(&mut rig);
}

/// A card opening into its detail: the presentation's content is a container hero holding the
/// card's face and the detail.
struct Card {
    detail: Presentation,
}

const CARD: (f32, f32, f32, f32) = (40., 300., 120., 100.);

fn card(s: &Card, _cx: &mut Context<Host<Card>>) -> AnyElement {
    let (x, y, w, h) = CARD;
    let page = div().size_full().child(
        div()
            .id("card")
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .rounded(px(20.))
            .border(px(5.))
            .border_color(hsla(0., 0., 0.5, 1.))
            .bg(hsla(0., 0., 1., 1.)),
    );
    let container = div()
        .id("container")
        .absolute()
        .top_0()
        .left_0()
        .w(px(480.))
        .h(px(640.))
        .bg(hsla(0., 0., 1., 1.))
        .overflow_hidden()
        .hero_with(&s.detail, "card")
        .child(div().id("face").p(px(16.)).child(div().h(px(20.))).hero_content())
        .child(div().id("detail").absolute().top_0().left_0().size_full().reveal_with(&s.detail));
    presentation("sheet", &s.detail).base(page).content(container).into_any_element()
}

fn card_rig(reduce: bool) -> Rig<Card> {
    let mut rig = Rig::sized(Size { width: 480., height: 640. }, |_| Card { detail: Presentation::new() }, card);
    rig.cx.set_reduce_motion(reduce);
    rig
}

fn set_detail(rig: &mut Rig<Card>, open: bool) {
    rig.win.root().clone().update(&mut rig.cx, |h, cx| {
        h.s.detail.set_open(open);
        cx.notify();
    });
}

/// What one frame of the container transform draws.
#[derive(Debug, Clone, Copy)]
struct Shot {
    opacity: f32,
    offset: (f32, f32),
    width: f32,
    radius: f32,
    border: f32,
    face: f32,
    detail: f32,
    face_width: f32,
    detail_width: f32,
}

/// The container, the card's content and the detail, once all three are mounted.
fn parts(rig: &Rig<Card>) -> Option<[NodeId; 3]> {
    let tree = rig.cx.semantic_tree();
    let find = |id: &str| tree.nodes().find(|(_, n)| n.test_id.as_deref() == Some(id)).map(|(_, n)| n.id);
    Some([find("container")?, find("face")?, find("detail")?])
}

fn shot(rig: &Rig<Card>, [container, face, detail]: [NodeId; 3]) -> Option<Shot> {
    let backend = rig.cx.backend();
    let div = |n: NodeId| backend.props(n).and_then(|p| p.downcast_ref::<DivProps>().cloned());
    let points = |l: Option<AbsoluteLength>| match l {
        Some(AbsoluteLength::Pixels(p)) => p.as_f32(),
        _ => 0.,
    };
    let width = |n: NodeId| backend.frame(n).map_or(0., |f| f.size.width);
    let shell = div(container)?;
    Some(Shot {
        opacity: shell.style.opacity.unwrap_or(1.),
        offset: shell.transform.offset,
        width: width(container),
        radius: points(shell.style.corner_radii.top_left),
        border: points(shell.style.border_widths.top),
        face: div(face)?.style.opacity.unwrap_or(1.),
        detail: div(detail)?.style.opacity.unwrap_or(1.),
        face_width: width(face),
        detail_width: width(detail),
    })
}

/// Frames until `until` holds and nothing moves, each one recorded while the container shows.
fn frames(rig: &mut Rig<Card>, count: usize, until: impl Fn(&Card) -> bool) -> Vec<Shot> {
    let mut shots = Vec::new();
    let mut ids = parts(rig);
    for _ in 0..count {
        rig.cx.advance_clock(FRAME);
        rig.cx.frame();
        ids = ids.or_else(|| parts(rig));
        shots.extend(ids.and_then(|ids| shot(rig, ids)).filter(|s| s.opacity > 0.));
        if until(rig.state()) && !rig.cx.needs_frame() {
            return shots;
        }
    }
    assert!(count < 300, "still moving after five seconds");
    shots
}

/// Frames until the presentation stops moving with `until` holding.
fn run(rig: &mut Rig<Card>, until: impl Fn(&Card) -> bool) -> Vec<Shot> {
    frames(rig, 300, until)
}

fn assert_never_empty(shots: &[Shot]) {
    for (i, s) in shots.iter().enumerate() {
        assert!(s.face.max(s.detail) > 0.19, "frame {i} shows neither the card nor the detail: {s:?}");
        if s.width > CARD.2 + 1. && s.width < 479. {
            assert!((s.face_width - (CARD.2 - 10.)).abs() < 0.5, "frame {i}: the card's content reflowed: {s:?}");
            assert!((s.detail_width - 480.).abs() < 0.5, "frame {i}: the detail reflowed: {s:?}");
        }
    }
}

#[test]
fn a_container_grows_out_of_its_card_with_the_cards_content_then_the_detail() {
    let mut rig = card_rig(false);
    set_detail(&mut rig, true);
    let shots = run(&mut rig, |s| s.detail.is_open());
    let first = shots.iter().find(|s| s.width > 0. && s.width < 200.).copied().expect("a frame on the card");
    assert!((first.width - CARD.2).abs() < 2., "starts on the card: {first:?}");
    assert!(first.radius > 18. && first.border > 4.5, "in the card's look: {first:?}");
    assert!(first.face > 0.95 && first.detail < 0.05, "showing the card's content: {first:?}");
    assert_never_empty(&shots);
    let faded = shots.iter().position(|s| s.face < 0.01).expect("the card's content fades out");
    let shown = shots.iter().position(|s| s.detail > 0.99).expect("the detail fades in");
    assert!(faded < shown, "the card's content goes before the detail is all in");
    let halfway = CARD.2 + 0.55 * (480. - CARD.2);
    let late = shots.iter().filter(|s| s.width > halfway).map(|s| s.border).fold(0f32, f32::max);
    assert!(late < 0.05, "the card's ring is gone by halfway, no outline lingers: {late}");
    let borders: Vec<f32> = shots.iter().map(|s| s.border).collect();
    assert!(borders.windows(2).all(|w| w[1] <= w[0] + 0.3), "the border only thins as it grows: {borders:?}");
    let last = *shots.last().expect("frames");
    assert_eq!((last.width, last.radius, last.border), (480., 0., 0.), "in its own look at home: {last:?}");
    assert_eq!((last.face, last.detail), (0., 1.));
}

#[test]
fn a_container_closes_back_into_its_card_in_reverse() {
    let mut rig = card_rig(false);
    set_detail(&mut rig, true);
    run(&mut rig, |s| s.detail.is_open());
    set_detail(&mut rig, false);
    let shots = run(&mut rig, |s| !s.detail.is_shown());
    assert_never_empty(&shots);
    let halfway = CARD.2 + 0.55 * (480. - CARD.2);
    let early = shots.iter().filter(|s| s.width > halfway).map(|s| s.border).fold(0f32, f32::max);
    assert!(early < 0.05, "the card's ring comes back only in the second half: {early}");
    let gone = shots.iter().position(|s| s.detail < 0.01).expect("the detail fades out");
    let back = shots.iter().position(|s| s.face > 0.99).expect("the card's content comes back");
    assert!(gone < back, "the detail goes before the card's content is all back");
    let landed = shots.iter().rev().find(|s| s.width > 0.).copied().expect("a frame before it closed");
    assert!((landed.width - CARD.2).abs() < 2., "lands on the card: {landed:?}");
    assert!(landed.border > 4.5 && landed.radius > 18., "in the card's look again: {landed:?}");
    assert!(rig.cx.backend().props(rig.node("card")).is_some(), "the card is still there");
}

#[test]
fn a_container_turns_around_mid_flight_both_ways() {
    let mut rig = card_rig(false);
    set_detail(&mut rig, true);
    let mut shots = frames(&mut rig, 12, |_| false);
    let mid = *shots.last().expect("frames");
    assert!(mid.width > CARD.2 + 10. && mid.width < 470., "mid-flight: {mid:?}");
    set_detail(&mut rig, false);
    shots.extend(frames(&mut rig, 8, |_| false));
    let widths: Vec<f32> = shots.iter().map(|s| s.width).collect();
    let jumps = widths.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
    assert!(jumps < 80., "turns around from where it is: {widths:?}");
    let back = *shots.last().expect("frames");
    assert!(back.width < mid.width, "heading back to the card: {back:?} after {mid:?}");
    set_detail(&mut rig, true);
    let rest = run(&mut rig, |s| s.detail.is_open());
    shots.extend(rest);
    assert_never_empty(&shots);
    let last = *shots.last().expect("frames");
    assert_eq!((last.width, last.border, last.face, last.detail), (480., 0., 0., 1.), "open again: {last:?}");
}

#[test]
fn under_reduce_motion_a_container_crossfades_without_travel() {
    let mut rig = card_rig(true);
    set_detail(&mut rig, true);
    let opened = run(&mut rig, |s| s.detail.is_open());
    set_detail(&mut rig, false);
    let closed = run(&mut rig, |s| !s.detail.is_shown());
    for (i, s) in opened.iter().chain(&closed).enumerate() {
        assert!(s.width == 0. || (s.width - 480.).abs() < 0.5, "frame {i} travels: {s:?}");
        assert_eq!(s.offset, (0., 0.), "frame {i} moves: {s:?}");
        assert_eq!((s.face, s.detail, s.border), (0., 1., 0.), "frame {i}: the detail, in the container's look");
    }
    let rising = opened.windows(2).all(|w| w[1].opacity >= w[0].opacity - 1e-6);
    let falling = closed.windows(2).all(|w| w[1].opacity <= w[0].opacity + 1e-6);
    assert!(rising && falling, "fades in, then out: {opened:?} {closed:?}");
    assert!(opened.iter().any(|s| s.opacity > 0.05 && s.opacity < 0.95), "a crossfade, not a cut");
    assert!(closed.iter().any(|s| s.opacity > 0.05 && s.opacity < 0.95), "a crossfade back");
}

/// A note over pages of cards, as a notes app draws it: the page under the note is keyed by
/// where the window is and rises in as the one it replaces fades out where it stood. The note
/// grows out of its card and goes back into it, or into its folder's card on a page without it.
struct Pages {
    detail: Presentation,
    page: Page,
    /// Where the note's card stands on the folder page.
    card_at: (f32, f32),
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    /// The note's folder: the note's card.
    Folder,
    /// Home: the folder's card.
    Home,
    /// Neither.
    Empty,
}

const FOLDER: (f32, f32, f32, f32) = (240., 120., 140., 110.);

fn pages(s: &Pages, _cx: &mut Context<Host<Pages>>) -> AnyElement {
    let (key, tile) = match s.page {
        Page::Folder => (
            "page:folder",
            Some(
                div()
                    .id("card")
                    .absolute()
                    .left(px(s.card_at.0))
                    .top(px(s.card_at.1))
                    .w(px(CARD.2))
                    .h(px(CARD.3))
                    .rounded(px(20.))
                    .border(px(5.))
                    .border_color(hsla(0., 0., 0.5, 1.))
                    .bg(hsla(0., 0., 1., 1.)),
            ),
        ),
        Page::Home => (
            "page:home",
            Some(
                div()
                    .id("folder")
                    .absolute()
                    .left(px(FOLDER.0))
                    .top(px(FOLDER.1))
                    .w(px(FOLDER.2))
                    .h(px(FOLDER.3))
                    .rounded(px(12.)),
            ),
        ),
        Page::Empty => ("page:empty", None),
    };
    let page = div()
        .id(key)
        .size_full()
        .animate(Animation::natural())
        // The window opens with its page in place: only a page after the first comes in.
        .when(s.detail.is_shown(), |d| d.enter_from(|p| p.opacity(0.).offset(px(0.), px(8.))))
        .exit_to(|p| p.opacity(0.))
        .children(tile);
    let container = div()
        .id("container")
        .absolute()
        .top_0()
        .left_0()
        .w(px(480.))
        .h(px(640.))
        .bg(hsla(0., 0., 1., 1.))
        .overflow_hidden()
        .hero_with(&s.detail, "card")
        .hero_fallback("folder")
        .child(div().id("face").p(px(16.)).child(div().h(px(20.))).hero_content())
        .child(div().id("detail").absolute().top_0().left_0().size_full().reveal_with(&s.detail));
    presentation("sheet", &s.detail).base(div().size_full().child(page)).content(container).into_any_element()
}

/// One frame of a note going back: where the container stands in the window and how it shows.
#[derive(Debug, Clone, Copy)]
struct Back {
    x: f32,
    y: f32,
    width: f32,
    opacity: f32,
    scale: f32,
    face: f32,
    detail: f32,
}

fn back_shot(rig: &Rig<Pages>, [container, face, detail]: [NodeId; 3]) -> Option<Back> {
    let backend = rig.cx.backend();
    let div = |n: NodeId| backend.props(n).and_then(|p| p.downcast_ref::<DivProps>().cloned());
    let shell = div(container)?;
    let frame = backend.window_frame_of(container)?;
    Some(Back {
        x: frame.origin.x + shell.transform.offset.0,
        y: frame.origin.y + shell.transform.offset.1,
        width: frame.size.width,
        opacity: shell.style.opacity.unwrap_or(1.),
        scale: shell.transform.scale,
        face: div(face)?.style.opacity.unwrap_or(1.),
        detail: div(detail)?.style.opacity.unwrap_or(1.),
    })
}

fn pages_parts(rig: &Rig<Pages>) -> Option<[NodeId; 3]> {
    let tree = rig.cx.semantic_tree();
    let find = |id: &str| tree.nodes().find(|(_, n)| n.test_id.as_deref() == Some(id)).map(|(_, n)| n.id);
    Some([find("container")?, find("face")?, find("detail")?])
}

/// `count` frames, or until `until` holds and nothing moves, each one recorded while the
/// container is mounted.
fn back_frames(rig: &mut Rig<Pages>, count: usize, until: impl Fn(&Pages) -> bool) -> Vec<Back> {
    let mut shots = Vec::new();
    let mut ids = pages_parts(rig);
    for _ in 0..count {
        rig.cx.advance_clock(FRAME);
        rig.cx.frame();
        ids = ids.or_else(|| pages_parts(rig));
        shots.extend(ids.and_then(|ids| back_shot(rig, ids)));
        if until(rig.state()) && !rig.cx.needs_frame() {
            return shots;
        }
    }
    assert!(count < 300, "still moving after five seconds");
    shots
}

/// A note open over its folder's page.
fn opened() -> Rig<Pages> {
    let mut rig = Rig::sized(
        Size { width: 480., height: 640. },
        |_| Pages { detail: Presentation::new(), page: Page::Folder, card_at: (CARD.0, CARD.1) },
        pages,
    );
    rig.win.root().clone().update(&mut rig.cx, |h, cx| {
        h.s.detail.set_open(true);
        cx.notify();
    });
    back_frames(&mut rig, 300, |s| s.detail.is_open());
    rig
}

/// Closes the note and shows `page` in the same render, as leaving it for another page does.
fn leave_for(rig: &mut Rig<Pages>, page: Page) {
    rig.win.root().clone().update(&mut rig.cx, |h, cx| {
        h.s.detail.set_open(false);
        h.s.page = page;
        cx.notify();
    });
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.
}

#[test]
fn a_note_closing_onto_its_page_lands_on_its_cards_frame_as_it_is_then() {
    let mut rig = opened();
    // The card has moved since the note opened (the grid laid out again under it).
    rig.win.root().clone().update(&mut rig.cx, |h, cx| {
        h.s.detail.set_open(false);
        h.s.card_at = (300., 60.);
        cx.notify();
    });
    let mut shots = back_frames(&mut rig, 6, |_| false);
    // And it moves again in flight: the container follows it.
    rig.win.root().clone().update(&mut rig.cx, |h, cx| {
        h.s.card_at = (200., 420.);
        cx.notify();
    });
    shots.extend(back_frames(&mut rig, 300, |s| !s.detail.is_shown()));
    let landed = shots.iter().rev().find(|s| s.opacity > 0.).copied().expect("frames");
    assert!(near(landed.x, 200.) && near(landed.y, 420.) && near(landed.width, CARD.2), "on the card: {landed:?}");
    assert!(landed.face > 0.99 && landed.detail < 0.01, "showing the card's content: {landed:?}");
    let stale = shots.iter().any(|s| near(s.x, CARD.0) && near(s.y, CARD.1) && near(s.width, CARD.2));
    assert!(!stale, "never on the frame the card had when the note opened");
    let xs: Vec<f32> = shots.iter().map(|s| s.x).collect();
    assert!(xs.windows(2).all(|w| (w[1] - w[0]).abs() < 80.), "follows without a jump: {xs:?}");
}

#[test]
fn a_note_closing_onto_a_page_with_only_its_folder_shrinks_into_the_folder() {
    let mut rig = opened();
    leave_for(&mut rig, Page::Home);
    let shots = back_frames(&mut rig, 300, |s| !s.detail.is_shown());
    let on_card = shots.iter().any(|s| near(s.x, CARD.0) && near(s.y, CARD.1) && near(s.width, CARD.2));
    assert!(!on_card, "never flies to the card on the page going out");
    let landed = shots.iter().rev().find(|s| s.width < 470.).copied().expect("a frame in flight");
    assert!(near(landed.x, FOLDER.0) && near(landed.y, FOLDER.1) && near(landed.width, FOLDER.2), "{landed:?}");
    for (i, s) in shots.iter().enumerate() {
        assert_eq!(s.face, 0., "frame {i}: the note's card content does not belong in the folder: {s:?}");
    }
    assert!(landed.detail < 0.01 && landed.opacity < 0.05, "faded as it landed: {landed:?}");
    let early = shots.iter().filter(|s| s.width > 300.).map(|s| s.opacity).fold(1f32, f32::min);
    assert!(early > 0.99, "solid while it is still large, fading only near the folder: {early}");
    let widths: Vec<f32> = shots.iter().map(|s| s.width).collect();
    assert!(widths.windows(2).all(|w| w[1] <= w[0] + 0.5), "only shrinks: {widths:?}");
}

#[test]
fn a_note_closing_onto_a_page_with_neither_fades_and_shrinks_in_place() {
    let mut rig = opened();
    leave_for(&mut rig, Page::Empty);
    let shots = back_frames(&mut rig, 300, |s| !s.detail.is_shown());
    assert!(!shots.is_empty(), "frames");
    for (i, s) in shots.iter().enumerate() {
        assert!(near(s.x, 0.) && near(s.y, 0.) && near(s.width, 480.), "frame {i} travels: {s:?}");
    }
    let opacities: Vec<f32> = shots.iter().map(|s| s.opacity).collect();
    assert!(opacities.windows(2).all(|w| w[1] <= w[0] + 1e-4), "only fades: {opacities:?}");
    let last = *shots.last().expect("frames");
    assert!(last.opacity < 0.02 && last.scale < 0.97 && last.scale > 0.9, "faded, a little smaller: {last:?}");
    assert!(shots.iter().all(|s| s.face == 0.), "no card content without a card");
}

#[test]
fn a_note_reopened_while_it_closes_turns_around_from_where_it_is() {
    let mut rig = opened();
    leave_for(&mut rig, Page::Home);
    let mut shots = back_frames(&mut rig, 10, |_| false);
    let mid = *shots.last().expect("frames");
    assert!(mid.width < 470. && mid.width > FOLDER.2 + 10., "mid-flight: {mid:?}");
    rig.win.root().clone().update(&mut rig.cx, |h, cx| {
        h.s.detail.set_open(true);
        h.s.page = Page::Folder;
        cx.notify();
    });
    shots.extend(back_frames(&mut rig, 300, |s| s.detail.is_open()));
    let widths: Vec<f32> = shots.iter().map(|s| s.width).collect();
    let jumps = widths.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
    assert!(jumps < 80., "turns around from where it is: {widths:?}");
    let last = *shots.last().expect("frames");
    assert!(near(last.x, 0.) && near(last.y, 0.) && near(last.width, 480.), "home again: {last:?}");
    assert_eq!((last.opacity, last.scale, last.face, last.detail), (1., 1., 0., 1.), "open again: {last:?}");
    // Closed now onto the card, it lands there in the card's look.
    leave_for(&mut rig, Page::Folder);
    let back = back_frames(&mut rig, 300, |s| !s.detail.is_shown());
    let landed = back.iter().rev().find(|s| s.opacity > 0.).copied().expect("frames");
    assert!(near(landed.x, CARD.0) && near(landed.y, CARD.1) && landed.face > 0.99, "on the card: {landed:?}");
}
