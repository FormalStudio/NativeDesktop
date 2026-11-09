//! The drawn source tree's motion: a folder's rows open and close as a drawer (cubic ease-out
//! over the theme's fast to normal timing, longer the more rows it travels, closing at 0.8 of
//! that and reversing from where it is), the selection pill slides to a newly selected row on a
//! stiff spring, and the hover wash fades in fast and out slower. With reduce motion nothing
//! moves: rows fade in place and the pill jumps.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use nd_components::{SourceIndex, SourceNode};
use nd_gesture::{Spring, SpringMotion};
use nd_gpui::gpui::{self, Bounds, ContentMask, Pixels};
use nd_ui::Curve;

/// Cubic ease-out, `1 - (1 - t)^3`.
const DRAWER_CURVE: Curve = Curve::Bezier(0.33, 1., 0.68, 1.);
/// Seconds a drawer adds per row it travels: `0.13 + 0.012 * rows` seconds on the macOS timings.
const PER_ROW: f64 = 0.012;
const CLOSE: f64 = 0.8;
/// A row rises this far as it fades in at the drawer's edge.
pub(crate) const RISE: f32 = 4.;
/// The share of a row pitch the edge travels while a row fades in.
const REVEAL_SPAN: f32 = 0.7;
/// Springs given as stiffness k and damping c (unit mass), in Apple's terms: response `2 pi / sqrt(k)`, damping `c / (2 sqrt(k))`. The pill: k 2500, c 100.
const PILL: Spring = Spring { response: 0.1257, damping: 1. };
/// The hover wash coming in: k 1100, c 66.
const HOVER_IN: Spring = Spring { response: 0.1894, damping: 0.995 };
/// The hover wash going out: k 300, c 35.
const HOVER_OUT: Spring = Spring { response: 0.3628, damping: 1.01 };

/// A tree's motion state, kept with the drawn tree across frames.
#[derive(Clone, Default)]
pub struct TreeMotion(Rc<RefCell<State>>);

#[derive(Default)]
struct State {
    /// Each expandable row's `expanded` as last drawn.
    seen: HashMap<gpui::SharedString, bool>,
    drawers: HashMap<gpui::SharedString, Drawer>,
    /// Each drawer's height when fully open, as last laid out.
    heights: HashMap<gpui::SharedString, f32>,
    hover: HashMap<gpui::SharedString, SpringMotion>,
    pill: Option<Pill>,
    /// Each row as laid out this frame.
    rows: HashMap<gpui::SharedString, Row>,
}

#[derive(Clone, Copy, Debug)]
struct Drawer {
    open: bool,
    from: f32,
    start: f64,
    duration: f64,
}

impl Drawer {
    fn t(&self, now: f64) -> f32 {
        if self.duration <= 0. { 1. } else { ((now - self.start) / self.duration).clamp(0., 1.) as f32 }
    }

    /// How far open, 0 to 1.
    fn shown(&self, now: f64) -> f32 {
        let to = if self.open { 1. } else { 0. };
        self.from + (to - self.from) * DRAWER_CURVE.at(self.t(now))
    }
}

struct Pill {
    id: gpui::SharedString,
    /// The slide from the last row: the pill's offset from its row, going to zero.
    dy: Option<SpringMotion>,
    dh: Option<SpringMotion>,
    /// Where the pill was painted last, from the list's origin: top and height.
    drawn: Option<(f32, f32)>,
}

#[derive(Clone, Copy)]
struct Row {
    bounds: Bounds<Pixels>,
    clip: ContentMask<Pixels>,
    /// How far the row has faded in.
    shown: f32,
    /// The list's origin in the same frame.
    list: Option<gpui::Point<Pixels>>,
}

/// One frame of a tree's motion: what the builder reads while drawing the rows.
#[derive(Clone)]
pub struct TreeFrame {
    motion: TreeMotion,
    now: f64,
    reduce: bool,
}

/// The theme timings a drawer runs between.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    pub fast: f64,
    pub normal: f64,
}

impl TreeMotion {
    /// The frame at `now`, after taking in the rows `nodes` asks for: a row whose `expanded`
    /// flipped since the last frame starts its drawer and a newly selected row starts the pill
    /// sliding over from the last one.
    pub fn frame(&self, nodes: &[SourceNode], selected: Option<&gpui::SharedString>, now: f64, reduce: bool) -> TreeFrame {
        let frame = TreeFrame { motion: self.clone(), now, reduce };
        frame.sync(nodes, selected);
        frame
    }

    /// Whether anything is still moving at `now`, so the window needs another frame.
    pub fn moving(&self, now: f64) -> bool {
        let s = self.0.borrow();
        s.drawers.values().any(|d| d.t(now) < 1.)
            || s.hover.values().any(|m| !at_rest(m, now))
            || s.pill.as_ref().is_some_and(|p| p.dy.iter().chain(p.dh.iter()).any(|m| !at_rest(m, now)))
    }
}

/// Within a five-hundredth of the target and nearly still: invisible for an opacity, and well
/// under a point for the pill's offsets.
fn at_rest(m: &SpringMotion, now: f64) -> bool {
    let (x, v) = m.sample(now);
    (x - m.target()).abs() < 0.002 && v.abs() < 0.02
}

impl TreeFrame {
    pub(crate) fn reduce(&self) -> bool {
        self.reduce
    }

    fn sync(&self, nodes: &[SourceNode], selected: Option<&gpui::SharedString>) {
        let now = self.now;
        let mut s = self.motion.0.borrow_mut();
        let ids: std::collections::HashSet<&gpui::SharedString> = nodes.iter().map(|n| &n.id).collect();
        s.drawers.retain(|id, d| ids.contains(id) && d.t(now) < 1.);
        s.seen.retain(|id, _| ids.contains(id));
        s.heights.retain(|id, _| ids.contains(id));
        s.hover.retain(|id, _| ids.contains(id));
        // The pill slides from where it was painted to the new row as that row was laid out.
        let reduce = self.reduce;
        let next = match (s.pill.take(), selected) {
            (Some(p), Some(id)) if p.id == *id => Some(p),
            (Some(p), Some(id)) => {
                let to = s.rows.get(id).and_then(|r| Some((r.bounds.origin.y - r.list?.y).as_f32()).zip(Some(r.bounds.size.height.as_f32())));
                let (dy, dh) = match (p.drawn, to) {
                    (Some((y, h)), Some((to_y, to_h))) if !reduce => {
                        let v = |m: &Option<SpringMotion>| m.map_or(0., |m| m.sample(now).1);
                        (
                            Some(PILL.start(f64::from(y - to_y), 0., v(&p.dy), now)),
                            Some(PILL.start(f64::from(h - to_h), 0., v(&p.dh), now)),
                        )
                    }
                    _ => (None, None),
                };
                Some(Pill { id: id.clone(), dy, dh, drawn: p.drawn })
            }
            (None, Some(id)) => Some(Pill { id: id.clone(), dy: None, dh: None, drawn: None }),
            (_, None) => None,
        };
        s.pill = next;
        s.rows.clear();
    }

    /// Takes in expandable row `id`'s `expanded`: a change since the last frame starts its
    /// drawer, timed by the `rows` it shows when open.
    pub(crate) fn toggle(&self, id: &gpui::SharedString, expanded: bool, rows: usize, timing: Timing) {
        let now = self.now;
        let mut s = self.motion.0.borrow_mut();
        let was = s.seen.insert(id.clone(), expanded);
        if was != Some(!expanded) {
            return;
        }
        let k = f64::from(crate::reveal::slowdown());
        let open = (timing.fast - 0.02 + PER_ROW * rows as f64).clamp(timing.fast, timing.normal.max(timing.fast));
        let duration = if expanded { open } else { open * CLOSE } * k;
        let from = s.drawers.get(id).map_or(if expanded { 0. } else { 1. }, |d| d.shown(now));
        s.drawers.insert(id.clone(), Drawer { open: expanded, from, start: now, duration });
    }

    /// Whether `id`'s drawer is still moving: its rows stay drawn while it closes.
    pub(crate) fn in_flight(&self, id: &gpui::SharedString) -> bool {
        self.motion.0.borrow().drawers.contains_key(id)
    }

    /// How far open `id`'s drawer is, when it is moving.
    pub(crate) fn drawer(&self, id: &gpui::SharedString) -> Option<f32> {
        self.motion.0.borrow().drawers.get(id).map(|d| d.shown(self.now))
    }

    /// How far the disclosure of `id` has turned towards open, 0 to 1.
    pub(crate) fn turn(&self, id: &gpui::SharedString, expanded: bool) -> f32 {
        let rest = if expanded { 1. } else { 0. };
        if self.reduce { rest } else { self.drawer(id).unwrap_or(rest) }
    }

    /// The fully open height of `id`'s drawer as last laid out.
    pub(crate) fn height(&self, id: &gpui::SharedString) -> Option<f32> {
        self.motion.0.borrow().heights.get(id).copied()
    }

    pub(crate) fn set_height(&self, id: gpui::SharedString, height: f32) {
        self.motion.0.borrow_mut().heights.insert(id, height);
    }

    /// The hover wash on `id`, 0 to 1, heading for `hovered`.
    pub(crate) fn hover(&self, id: &gpui::SharedString, hovered: bool) -> f32 {
        let now = self.now;
        let target = if hovered { 1. } else { 0. };
        let mut s = self.motion.0.borrow_mut();
        let spring = if hovered { HOVER_IN } else { HOVER_OUT };
        let current = s.hover.get(id).copied();
        match current {
            None if !hovered => return 0.,
            None => {
                s.hover.insert(id.clone(), spring.start(0., 1., 0., now));
            }
            Some(m) if m.target() != target => {
                let (x, v) = m.sample(now);
                s.hover.insert(id.clone(), spring.start(x, target, v, now));
            }
            Some(_) => {}
        }
        let m = s.hover[id];
        if !hovered && at_rest(&m, now) {
            s.hover.remove(id);
            return 0.;
        }
        (m.sample(now).0 as f32).clamp(0., 1.)
    }

    /// Records where row `id` was laid out this frame, inside `clip`, faded in by `shown`.
    pub(crate) fn place(&self, id: gpui::SharedString, bounds: Bounds<Pixels>, clip: ContentMask<Pixels>, shown: f32) {
        self.motion.0.borrow_mut().rows.insert(id, Row { bounds, clip, shown, list: None });
    }

    /// Paints the selection pill under the rows of a list laid out at `list`: on the selected
    /// row, plus what is left of its slide from the last one.
    pub(crate) fn paint_pill(
        &self,
        list: Bounds<Pixels>,
        fill: gpui::Hsla,
        radius: Pixels,
        window: &mut gpui::Window,
    ) {
        let now = self.now;
        let mut s = self.motion.0.borrow_mut();
        for row in s.rows.values_mut() {
            row.list = Some(list.origin);
        }
        let s = &mut *s;
        let Some(pill) = s.pill.as_mut() else { return };
        let Some(row) = s.rows.get(&pill.id).copied() else {
            pill.drawn = None;
            return;
        };
        let offset = |m: &Option<SpringMotion>| m.map_or(0., |m| m.sample(now).0 as f32);
        let (dy, dh) = (offset(&pill.dy), offset(&pill.dh));
        let top = (row.bounds.origin.y - list.origin.y).as_f32() + dy;
        let height = (row.bounds.size.height.as_f32() + dh).max(0.);
        pill.drawn = Some((top, height));
        let bounds = Bounds {
            origin: gpui::point(row.bounds.origin.x, list.origin.y + gpui::px(top)),
            size: gpui::size(row.bounds.size.width, gpui::px(height)),
        };
        let fill = nd_theme::color::with_alpha(fill, fill.a * row.shown);
        window.with_content_mask(Some(row.clip), |window| {
            window.paint_quad(gpui::fill(bounds, fill).corner_radii(radius));
        });
    }
}

/// The rows a drawer under row `i` shows when open: its descendants under expanded rows.
pub(crate) fn drawer_rows(index: &SourceIndex, nodes: &[SourceNode], i: usize) -> usize {
    index.children[i].iter().map(|&c| 1 + if nodes[c].expanded { drawer_rows(index, nodes, c) } else { 0 }).sum()
}

/// How far a row `k` rows into a drawer has faded in, with the drawer's edge `edge` points
/// down it: smoothstep over most of a pitch as the edge passes the row's top.
pub(crate) fn reveal(edge: f32, top: f32, pitch: f32) -> f32 {
    let t = ((edge - top) / (REVEAL_SPAN * pitch).max(1.)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nd_components::source_node;

    const TIMING: Timing = Timing { fast: 0.15, normal: 0.25 };

    fn tree(expanded: bool) -> Vec<SourceNode> {
        vec![
            source_node("work", "Work").expanded(expanded),
            source_node("q3", "Q3").parent("work"),
            source_node("trips", "Trips").parent("work"),
        ]
    }

    #[test]
    fn a_drawer_opens_on_its_timing_and_closes_faster_from_where_it_got_to() {
        let motion = TreeMotion::default();
        let f = motion.frame(&tree(false), None, 0., false);
        f.toggle(&"work".into(), false, 2, TIMING);
        assert!(!f.in_flight(&"work".into()), "first sight does not animate");
        let f = motion.frame(&tree(true), None, 1., false);
        f.toggle(&"work".into(), true, 2, TIMING);
        assert!(f.drawer(&"work".into()).expect("opening") < 1e-4, "starts shut");
        let mid = motion.frame(&tree(true), None, 1.05, false);
        let at = mid.drawer(&"work".into()).expect("open");
        assert!(at > 0.5, "eases out: most of the way a third in ({at})");
        assert!(motion.moving(1.05));
        let f = motion.frame(&tree(false), None, 1.05, false);
        f.toggle(&"work".into(), false, 2, TIMING);
        assert!((f.drawer(&"work".into()).expect("closing") - at).abs() < 1e-4, "reverses from its edge");
        // Two rows open over 0.13 + 2 * 0.012 s.
        let close = 0.154 * CLOSE;
        assert!(motion.moving(1.05 + close - 0.01));
        assert!(!motion.moving(1.05 + close + 0.001), "closes at 0.8 of the open time");
        let f = motion.frame(&tree(false), None, 2., false);
        assert!(!f.in_flight(&"work".into()));
    }

    #[test]
    fn longer_drawers_take_longer_within_the_theme_timings() {
        let motion = TreeMotion::default();
        let duration = |rows: usize| {
            let f = motion.frame(&tree(false), None, 0., false);
            f.toggle(&"work".into(), false, rows, TIMING);
            let f = motion.frame(&tree(true), None, 0., false);
            f.toggle(&"work".into(), true, rows, TIMING);
            let d = motion.0.borrow().drawers["work"].duration;
            motion.0.borrow_mut().drawers.clear();
            d
        };
        assert_eq!(duration(1), 0.15);
        assert!((duration(10) - 0.25).abs() < 1e-9, "{}", duration(10));
        assert!(duration(5) > 0.15 && duration(5) < 0.25);
        assert_eq!(duration(40), 0.25);
    }

    #[test]
    fn hover_fades_in_faster_than_it_fades_out() {
        let motion = TreeMotion::default();
        let id: gpui::SharedString = "q3".into();
        let f = motion.frame(&tree(true), None, 0., false);
        assert_eq!(f.hover(&id, false), 0.);
        f.hover(&id, true);
        let f = motion.frame(&tree(true), None, 0.05, false);
        let rise = f.hover(&id, true);
        let f = motion.frame(&tree(true), None, 1., false);
        assert!((f.hover(&id, true) - 1.).abs() < 0.01);
        f.hover(&id, false);
        let f = motion.frame(&tree(true), None, 1.05, false);
        let fall = 1. - f.hover(&id, false);
        assert!(rise > fall * 1.5, "in {rise}, out {fall}");
        let f = motion.frame(&tree(true), None, 3., false);
        assert_eq!(f.hover(&id, false), 0.);
        assert!(!motion.moving(3.));
    }

    #[test]
    fn rows_fade_in_as_the_edge_passes_them() {
        assert_eq!(reveal(0., 10., 30.), 0.);
        assert_eq!(reveal(40., 10., 30.), 1.);
        let half = reveal(10. + 0.35 * 30., 10., 30.);
        assert!((half - 0.5).abs() < 1e-4);
    }
}
