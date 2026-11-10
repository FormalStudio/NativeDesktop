//! The picture a lifted drag carries (`Div::lift_on_drag`), drawn by its window over everything
//! but menus. It is the source's subtree as it was when the drag lifted, so it can still land
//! after a drop removed the source: rebuilt each frame from that snapshot, a little larger than
//! the source while it follows the pointer, then shrinking into the target that took it or
//! settling back on the source. Under Reduce Motion it only fades. A picture is never tracked,
//! so the source's bounds stay where layout put it. A plain drag leaving its window shows no
//! picture here: GPUI draws nothing outside a window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use nd_core::{NodeId, Rect, WindowId};
use nd_gpui::gpui::{
    self, AnyElement, InteractiveElement, IntoElement, ParentElement, Styled, anchored, deferred, div, point, px,
};
use nd_ui::DivProps;
use nd_ui::DragPreview;

use crate::backend::Node;

/// How much larger a lifted picture rides than its source, as Finder's does.
const LIFT: f32 = 1.04;
/// How small a picture dropped into a target gets before it is gone.
const INTO: f32 = 0.5;
const POP: f64 = 0.15;
const SETTLE: f64 = 0.25;

struct Picture {
    root: NodeId,
    nodes: HashMap<NodeId, Node>,
    /// In global points, as the drag reported it.
    frame: Rect,
    reduce: bool,
    /// The frame time it was first drawn at.
    shown: Option<f64>,
    landing: Option<Landing>,
}

struct Landing {
    to: Rect,
    into: bool,
    /// The frame time it was first drawn landing.
    at: Option<f64>,
}

/// Each window's picture.
#[derive(Clone, Default)]
pub(crate) struct Pictures(Rc<RefCell<HashMap<WindowId, Picture>>>);

fn slowdown() -> f64 {
    std::env::var("ND_ANIMATION_SLOWDOWN").ok().and_then(|v| v.parse::<f64>().ok()).filter(|v| *v >= 1.).unwrap_or(1.)
}

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Pictures {
    /// `Backend::drag_preview`. `subtree` snapshots the source the first time it is asked.
    pub fn update(
        &self,
        window: WindowId,
        root: NodeId,
        preview: DragPreview,
        reduce: bool,
        subtree: impl FnOnce() -> HashMap<NodeId, Node>,
    ) {
        let mut map = self.0.borrow_mut();
        match preview {
            DragPreview::At { frame, lifted: true } => match map.get_mut(&window) {
                Some(p) if p.root == root && p.landing.is_none() => p.frame = frame,
                _ => {
                    let nodes = settled(root, subtree());
                    map.insert(window, Picture { root, nodes, frame, reduce, shown: None, landing: None });
                }
            },
            DragPreview::At { lifted: false, .. } => {}
            DragPreview::Land { frame, into } => {
                if let Some(p) = map.get_mut(&window) {
                    p.landing = Some(Landing { to: frame, into, at: None });
                }
            }
            DragPreview::Gone => {
                map.remove(&window);
            }
        }
    }

    /// The window's picture over its content at `origin` (the window's global origin) on the
    /// frame drawn for `now` (seconds on the window's frame clock), or `None`; the bool asks
    /// for another frame while it animates.
    pub fn element(
        &self,
        window: WindowId,
        origin: gpui::Point<gpui::Pixels>,
        now: f64,
        build: impl FnOnce(&HashMap<NodeId, Node>, NodeId) -> AnyElement,
    ) -> Option<(AnyElement, bool)> {
        let mut map = self.0.borrow_mut();
        let p = map.get_mut(&window)?;
        let since = *p.shown.get_or_insert(now);
        if let Some(l) = p.landing.as_mut() {
            l.at.get_or_insert(now);
        }
        let (pop, settle) = (POP * slowdown(), SETTLE * slowdown());
        let shown = ((now - since) / pop).clamp(0., 1.) as f32;
        let (frame, scale, opacity, more) = match &p.landing {
            None => {
                let (scale, opacity) = if p.reduce { (1., shown) } else { (mix(1., LIFT, ease_out(shown)), 1.) };
                (p.frame, scale, opacity, shown < 1.)
            }
            Some(l) => {
                let t = ((now - l.at.unwrap_or(now)) / settle).clamp(0., 1.) as f32;
                if t >= 1. {
                    map.remove(&window);
                    return None;
                }
                let e = ease_out(t);
                let fade = if l.into || p.reduce { 1. - e } else { 1. };
                if p.reduce {
                    (p.frame, 1., fade, true)
                } else {
                    let to = l.to;
                    let (cx, cy) = (to.origin.x + to.size.width / 2., to.origin.y + to.size.height / 2.);
                    let (fx, fy) =
                        (p.frame.origin.x + p.frame.size.width / 2., p.frame.origin.y + p.frame.size.height / 2.);
                    let (w, h) = (p.frame.size.width, p.frame.size.height);
                    // The picture keeps its size; the scale carries it to the target's.
                    let end = if l.into { INTO * (to.size.width / w.max(1.)) } else { to.size.width / w.max(1.) };
                    let x = mix(fx, cx, e) - w / 2.;
                    let y = mix(fy, cy, e) - h / 2.;
                    (Rect::new(x, y, w, h), mix(LIFT, end, e), fade, true)
                }
            }
        };
        let p = map.get(&window)?;
        let content = build(&p.nodes, p.root);
        drop(map);
        let sized = div()
            .id("nd-drag-picture")
            .w(px(frame.size.width))
            .h(px(frame.size.height))
            .opacity(opacity)
            .shadow_lg()
            .child(content)
            .into_any_element();
        let scaled = if scale == 1. { sized } else { crate::gestures::scale(sized, scale) };
        let at = point(px(frame.origin.x) - origin.x, px(frame.origin.y) - origin.y);
        let layer = anchored().position(at).child(crate::render::Inert(scaled));
        Some((deferred(layer).with_priority(crate::overlay::DRAG_PRIORITY).into_any_element(), more))
    }
}

/// `nodes` with the root drawn as it stands alone: no position or offset of its own, no
/// opacity a dragged source dims to.
fn settled(root: NodeId, mut nodes: HashMap<NodeId, Node>) -> HashMap<NodeId, Node> {
    if let Some(node) = nodes.get_mut(&root)
        && let Some(props) = node.props.downcast_ref::<DivProps>()
    {
        let mut props = props.clone();
        props.style.position = Some(gpui::Position::Relative);
        props.style.inset = Default::default();
        props.style.margin = Default::default();
        props.style.opacity = None;
        props.style.size.width = Some(gpui::relative(1.).into());
        props.style.size.height = Some(gpui::relative(1.).into());
        props.transform = Default::default();
        node.props = Rc::new(props);
        node.parent = None;
    }
    nodes
}
