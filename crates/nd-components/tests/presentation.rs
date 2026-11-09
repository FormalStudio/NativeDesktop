//! Tier 1 behaviour of a presentation on `TestBackend`: what each frame draws while it closes.

mod common;

use common::*;
use nd_components::*;
use nd_ui::prelude::*;
use nd_ui::{DivProps, Size};

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
