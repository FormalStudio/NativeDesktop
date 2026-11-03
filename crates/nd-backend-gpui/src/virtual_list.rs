//! The GPUI presenter of `nd-virtual` lists, tables, grids and trees (ARCHITECTURE.md 7). Our own
//! element, not GPUI's `list`: GPUI's `ListState` trusts heights of rows outside the viewport
//! until `splice` (`gpui/src/elements/list.rs:1-4`), which is where "last rows disappearing"
//! comes from. Here every rendered row is measured at the list's width on every frame before the
//! model places anything, and the model never clamps against an estimate.
//!
//! The rows are the list node's children, rendered by the owner for the model's render range.
//! When the viewport needs rows that are not rendered, the presenter emits `ViewportChanged` and
//! wakes the host; the list's handler renders them in the next app frame, which runs before GPUI
//! draws again.

use nd_core::time::Instant;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;

use nd_core::{NodeId, Rect};
use nd_gpui::gpui::{
    self, AnyElement, App, AvailableSpace, Bounds, ContentMask, DispatchPhase, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Refineable, SharedString, Style, fill, point, px, size,
};
use nd_ui::events::Emit;
use nd_virtual::selection::{self, Extend, Nav};
use nd_virtual::{
    Align, ListHandle, ListKind, ListProps, RowActivated, RowDisclosed, RowId, RowMeta, RowTag, RowsSelected,
    Selection, ViewportChanged,
};

use crate::backend::Wake;

/// Points of indentation per tree level; the disclosure triangle sits in the last one.
pub(crate) const INDENT: f32 = 16.;
/// Height of a table's header row.
pub(crate) const HEADER: f32 = 24.;
/// A marquee held this close to the viewport's top or bottom edge, or past it, scrolls.
const EDGE: f32 = 24.;
/// Points a press may travel before it is a drag rather than a click.
const SLOP: f32 = 4.;

fn now() -> f64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// A list's presenter state, kept across frames by the backend.
#[derive(Default)]
pub struct Presenter {
    /// The router's offset for this list when the presenter last synced it with the model, in
    /// router coordinates (`0` is the model's lowest offset). What it moved since is the user's.
    router: Option<f32>,
    /// Rows painted in the last frame: their node and their rectangle in window coordinates.
    pub(crate) painted: Vec<(RowId, NodeId, Rect)>,
    /// The viewport in window coordinates, below a table's header.
    pub(crate) viewport: Rect,
    /// The rendered rows when it last asked the app for others, so it asks once per change.
    asked: Option<Range<usize>>,
    edges: (bool, bool),
    /// Where a Shift range grows from, and the row keys move from.
    anchor: Option<RowId>,
    cursor: Option<RowId>,
    /// The selection the presenter last reported, and the one the props last carried: props
    /// moving to anything but the report are the app's own change, which resets the anchor.
    reported: Vec<RowId>,
    seen: Vec<RowId>,
    marquee: Option<Marquee>,
    /// A plain press on a row of a multiple selection: it collapses the selection to that row
    /// on release, unless the press became a drag (the group moves together).
    pending: Option<(RowId, Point<Pixels>)>,
    /// A press on a row of a list whose rows drag: where, and the row's bounds.
    press: Option<(RowId, Point<Pixels>, Bounds<Pixels>)>,
    /// The rows being dragged out, drawn under the pointer.
    drag: Option<RowsDrag>,
    /// The row a drag over the list would drop on, outlined.
    pub(crate) drop_row: Option<RowId>,
}

struct RowsDrag {
    item: nd_ui::DragItem,
    rows: Vec<RowId>,
    /// The rows' files went to the platform's drag when the pointer left the window: the
    /// release then happens in another app, and the platform says when it is over.
    handed: bool,
    /// The pointer, and where it holds the pressed row from its top-left.
    at: Point<Pixels>,
    grab: Point<Pixels>,
    size: gpui::Size<Pixels>,
}

/// What a row's GPUI drag carries, so that GPUI can hand the files of the rows being dragged to
/// the platform when the pointer leaves the window (`List::drag_out_files`). nd's own drag
/// session runs alongside it and stays the drag inside the app.
struct FilesOut {
    node: NodeId,
    presenters: Presenters,
    files: Rc<HashMap<RowId, PathBuf>>,
}

impl FilesOut {
    /// GPUI starts the row's drag on any press past its own threshold, so a press that never
    /// became an nd row drag (past `SLOP`) offers nothing.
    fn payload(&self) -> Option<gpui::ExternalDragPayload> {
        let mut presenters = self.presenters.borrow_mut();
        let drag = presenters.get_mut(&self.node)?.drag.as_mut()?;
        let entries = files_out(&drag.rows, &self.files);
        if entries.is_empty() {
            return None;
        }
        drag.handed = true;
        Some(gpui::ExternalDragPayload::Files(gpui::FileDragPaths::new(entries)))
    }
}

/// The dragged rows' files in drag order, each with whether it is a directory.
fn files_out(rows: &[RowId], files: &HashMap<RowId, PathBuf>) -> Vec<(PathBuf, bool)> {
    rows.iter().filter_map(|r| files.get(r)).map(|p| (p.clone(), p.is_dir())).collect()
}

/// A rubber band dragged out from empty space.
struct Marquee {
    /// The press: x in window coordinates, y in content coordinates, so the band stays on the
    /// rows it started at while the list scrolls.
    from: (f32, f64),
    /// The pointer now, in window coordinates.
    to: (f32, f32),
    /// What was selected at the press, for a marquee that adds to it or toggles it.
    base: Vec<RowId>,
    extend: Extend,
    swept: Vec<RowId>,
}

/// The rows the band touches, in list order.
fn swept(handle: &ListHandle, view: Rect, m: &Marquee) -> Vec<RowId> {
    let core = handle.0.borrow();
    let top = core.scroll_top();
    let to = top + f64::from(m.to.1 - view.origin.y);
    let (y0, y1) = (m.from.1.min(to), m.from.1.max(to));
    let (a, b) = (m.from.0 - view.origin.x, m.to.0 - view.origin.x);
    let (x0, x1) = (f64::from(a.min(b)), f64::from(a.max(b)));
    let grid = core.grid();
    let lanes = core.lanes();
    let mut out = Vec::new();
    for (i, row) in core.ids().iter().enumerate() {
        let (rt, rh) = (core.top(i), grid.map_or(core.height(i), |g| g.item_height));
        if rt > y1 {
            break;
        }
        let (rx, rw) = match grid {
            Some(g) => ((i % lanes) as f64 * (g.item_width + g.gap), g.item_width),
            None => (0., f64::from(view.size.width)),
        };
        if rt + rh >= y0 && rx <= x1 && rx + rw >= x0 {
            out.push(*row);
        }
    }
    out
}

/// Points a held marquee scrolls by this frame: towards the edge it is near, faster the further
/// past it.
fn edge_speed(y: f32, view: Rect) -> f32 {
    let (top, bottom) = (view.origin.y + EDGE, view.origin.y + view.size.height - EDGE);
    let d = if y < top {
        y - top
    } else if y > bottom {
        y - bottom
    } else {
        0.
    };
    (d / 2.).clamp(-40., 40.)
}

pub(crate) type Presenters = Rc<RefCell<HashMap<NodeId, Presenter>>>;

pub(crate) struct Row {
    pub tag: RowTag,
    pub node: NodeId,
    pub element: AnyElement,
}

/// A tree row's hierarchy, from the model (the row's own tag can be a render behind).
fn meta(handle: &ListHandle, row: RowId) -> RowMeta {
    let core = handle.0.borrow();
    core.index_of(row).map(|i| core.meta(i)).unwrap_or_default()
}

/// Points a tree row is indented by: its depth plus the disclosure column.
fn tree_indent(kind: &ListKind, handle: &ListHandle, row: RowId) -> f32 {
    if *kind == ListKind::Tree { (f32::from(meta(handle, row).depth) + 1.) * INDENT } else { 0. }
}

pub(crate) struct VirtualList {
    pub node: NodeId,
    pub props: ListProps,
    pub rows: Vec<Row>,
    pub header: Option<AnyElement>,
    pub emit: Emit,
    pub wake: Wake,
    pub presenters: Presenters,
    pub gestures: crate::gestures::Gestures,
    pub window_id: nd_core::WindowId,
    pub selection_color: Hsla,
    /// The selection while the list has the keyboard: the accent's tint, as Finder and Files
    /// show a focused list's rows.
    pub focused_selection_color: Hsla,
    /// Under the row the pointer is over, in a list whose rows select or activate.
    pub hover_color: Hsla,
    pub sidebar: SidebarSelection,
    pub disclosure_color: Hsla,
    /// libadwaita's raised and undershoot shades at the scroll edges, under that look.
    pub edges: Option<(Hsla, Hsla)>,
    /// The list's keyboard focus: a press focuses it, keys move the selection while it holds it.
    pub focus: crate::forms::Sink,
    /// The marquee's band.
    pub accent: Hsla,
    /// The keyboard focus ring, 2 px wide.
    pub ring_color: Hsla,
    pub ring_radius: Pixels,
}

/// How an inset list (a sidebar's) draws its selection.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SidebarSelection {
    /// From the list's edges.
    pub inset: f32,
    pub radius: f32,
    /// WinUI's selection indicator at the leading edge, in this colour.
    pub pill: Option<Hsla>,
}

pub(crate) struct Prepainted {
    hitbox: Hitbox,
    /// Indices into `rows` that are painted, with their bounds.
    placed: Vec<(usize, Bounds<Pixels>)>,
    selected: Vec<Bounds<Pixels>>,
    disclosures: Vec<(Bounds<Pixels>, bool)>,
    header: Option<Bounds<Pixels>>,
    /// The router's offset and scrollable range this frame.
    scrolled: (f32, f32),
    /// Over each placed row of a list whose rows carry files out of the app, the element GPUI's
    /// own drag starts from: any row, since a drag from one carries the whole selection.
    handles: Vec<AnyElement>,
}

impl IntoElement for VirtualList {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

fn rect(b: Bounds<Pixels>) -> Rect {
    Rect::new(b.origin.x.as_f32(), b.origin.y.as_f32(), b.size.width.as_f32(), b.size.height.as_f32())
}

impl VirtualList {
    /// The marquee's band in window coordinates, cut to the viewport.
    fn band(&self, view: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
        let presenters = self.presenters.borrow();
        let m = presenters.get(&self.node)?.marquee.as_ref()?;
        let top = self.props.handle.0.borrow().scroll_top();
        let y = view.origin.y.as_f32() + (m.from.1 - top) as f32;
        let (x0, x1) = (m.from.0.min(m.to.0), m.from.0.max(m.to.0));
        let (y0, y1) = (y.min(m.to.1), y.max(m.to.1));
        let band = Bounds::from_corners(point(px(x0), px(y0)), point(px(x1), px(y1)));
        Some(band.intersect(&view))
    }

    /// Asks the app for the rows the model wants when they are not the ones rendered.
    fn ask_for_rows(&self, render: Range<usize>, edges: (bool, bool), cx: &mut App) {
        let rendered: HashSet<RowId> = self.rows.iter().map(|r| r.tag.row).collect();
        let core = self.props.handle.0.borrow();
        let wanted: Vec<RowId> = render.clone().filter_map(|i| core.row(i)).collect();
        drop(core);
        let mut presenters = self.presenters.borrow_mut();
        let p = presenters.entry(self.node).or_default();
        let missing = wanted.len() != rendered.len() || wanted.iter().any(|r| !rendered.contains(r));
        let edges_changed = p.edges != edges;
        if (missing && p.asked.as_ref() != Some(&render)) || edges_changed {
            p.asked = Some(render);
            p.edges = edges;
            drop(presenters);
            self.emit.emit(ViewportChanged { at_start: edges.0, at_end: edges.1 });
            (self.wake)(cx);
        }
    }
}

impl gpui::Element for VirtualList {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Integer(self.node.0))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.refine(&self.props.style);
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> Prepainted {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        window.set_focus_handle(&self.focus.focus, cx);
        let header_h = if self.header.is_some() { px(HEADER) } else { px(0.) };
        let header = self.header.as_mut().map(|h| {
            let hb = Bounds::new(bounds.origin, size(bounds.size.width, header_h));
            h.layout_as_root(
                size(AvailableSpace::Definite(hb.size.width), AvailableSpace::Definite(header_h)),
                window,
                cx,
            );
            h.prepaint_at(hb.origin, window, cx);
            hb
        });
        let view = Bounds::new(
            point(bounds.origin.x, bounds.origin.y + header_h),
            size(bounds.size.width, (bounds.size.height - header_h).max(px(0.))),
        );
        let width = view.size.width.as_f32();
        let handle = self.props.handle.clone();
        let grid = handle.0.borrow().grid();

        let viewport = nd_core::Size { width, height: view.size.height.as_f32() };
        {
            let mut core = handle.0.borrow_mut();
            core.set_viewport(f64::from(width), f64::from(viewport.height));
            core.set_time(now());
        }
        // The window's router owns scroll motion (momentum, rubber band, nested arbitration); the
        // model owns position. What the router moved since the last frame goes to the model as a
        // user scroll.
        let routed = self.gestures.list_offset(self.window_id, self.node);
        let last = self.presenters.borrow().get(&self.node).and_then(|p| p.router);
        if let (Some(routed), Some(last)) = (routed, last)
            && routed != last
        {
            handle.0.borrow_mut().scroll_by(f64::from(routed - last));
        }
        let held = self.presenters.borrow().get(&self.node).and_then(|p| p.marquee.as_ref().map(|m| m.to.1));
        let edge = held.map_or(0., |y| edge_speed(y, rect(view)));
        if edge != 0. {
            handle.0.borrow_mut().scroll_by(f64::from(edge));
            window.request_animation_frame();
        }
        // Rows are laid out only where it matters: in the viewport, or with a height the model
        // has not measured at this width. Overscan rows already measured are only kept ready.
        let kind = self.props.kind.clone();
        let by_row: HashMap<RowId, usize> = self.rows.iter().enumerate().map(|(i, r)| (r.tag.row, i)).collect();
        let mut laid: HashSet<usize> = HashSet::new();
        for _ in 0..4 {
            let need: Vec<usize> = {
                let core = handle.0.borrow();
                let visible: HashSet<RowId> = core.visible().iter().map(|p| p.row).collect();
                self.rows
                    .iter()
                    .enumerate()
                    .filter(|(i, r)| !laid.contains(i) && (visible.contains(&r.tag.row) || !core.is_fresh(r.tag.row)))
                    .map(|(i, _)| i)
                    .collect()
            };
            if need.is_empty() {
                break;
            }
            for i in need {
                let row = &mut self.rows[i];
                let indent = tree_indent(&kind, &handle, row.tag.row);
                let available = match grid {
                    Some(g) => size(
                        AvailableSpace::Definite(px(g.item_width as f32)),
                        AvailableSpace::Definite(px(g.item_height as f32)),
                    ),
                    None => size(AvailableSpace::Definite(px((width - indent).max(0.))), AvailableSpace::MinContent),
                };
                let measured = row.element.layout_as_root(available, window, cx);
                handle.0.borrow_mut().report_height(row.tag.row, f64::from(measured.height.as_f32()));
                laid.insert(i);
            }
        }
        let layout = handle.0.borrow_mut().layout();
        // Keep the router on the model's position (rows above changed height, a jump, new rows),
        // without stopping a fling; past an edge the difference is the rubber band, drawn as is.
        let (lo, hi) = handle.0.borrow().bounds();
        let range = (hi - lo) as f32;
        let model = (layout.scroll_top - lo) as f32;
        let synced = self.gestures.sync_list(self.window_id, self.node, &self.props.gestures, viewport, range, model);
        let overscroll = synced - synced.clamp(0., range.max(0.));
        self.presenters.borrow_mut().entry(self.node).or_default().router = Some(synced);
        self.ask_for_rows(layout.render.clone(), (layout.at_start, layout.at_end), cx);
        if handle.0.borrow().animating() || layout.unmeasured {
            window.request_animation_frame();
        }

        let selected: HashSet<RowId> = self.props.selected.iter().copied().collect();
        let files = (self.props.drag_kind.is_some() && !self.props.drag_files.is_empty())
            .then(|| Rc::new(self.props.drag_files.clone()));
        let mut handles = Vec::new();
        let mut placed = Vec::new();
        let mut painted = Vec::new();
        let mut sel = Vec::new();
        let mut disclosures = Vec::new();
        window.with_content_mask(Some(ContentMask { bounds: view }), |window| {
            for p in &layout.visible {
                let Some(&i) = by_row.get(&p.row) else { continue };
                let tag = meta(&handle, p.row);
                let indent = tree_indent(&kind, &handle, p.row);
                if !laid.contains(&i) {
                    let available =
                        size(AvailableSpace::Definite(px((width - indent).max(0.))), AvailableSpace::MinContent);
                    self.rows[i].element.layout_as_root(available, window, cx);
                }
                let x = match grid {
                    Some(g) => p.lane as f32 * (g.item_width + g.gap) as f32,
                    None => indent,
                };
                // Rounded to whole points like the rest of GPUI's layout, so rows tile exactly.
                let origin =
                    point(view.origin.x + px(x), view.origin.y + px((p.y - f64::from(overscroll)).round() as f32));
                let h = match grid {
                    Some(g) => g.item_height as f32,
                    None => p.height as f32,
                };
                let w = match grid {
                    Some(g) => g.item_width as f32,
                    None => width - indent,
                };
                let row_bounds = Bounds::new(origin, size(px(w), px(h)));
                self.rows[i].element.prepaint_at(origin, window, cx);
                let full = Bounds::new(point(view.origin.x, origin.y), size(view.size.width, px(h)));
                if selected.contains(&p.row) {
                    sel.push(if grid.is_some() { row_bounds } else { full });
                }
                if let Some(files) = &files {
                    use gpui::{
                        AppContext as _, InteractiveElement as _, StatefulInteractiveElement as _, Styled as _,
                    };
                    let b = if grid.is_some() { row_bounds } else { full };
                    let out = FilesOut { node: self.node, presenters: self.presenters.clone(), files: files.clone() };
                    let mut handle = gpui::div()
                        .id(ElementId::NamedInteger("drag-out".into(), p.row.0))
                        .w(b.size.width)
                        .h(b.size.height)
                        .on_drag(out, |_, _, _, cx| cx.new(|_| gpui::Empty))
                        .external_drag_payload(|out: &FilesOut, _, _| out.payload())
                        .into_any_element();
                    handle.layout_as_root(
                        size(AvailableSpace::Definite(b.size.width), AvailableSpace::Definite(b.size.height)),
                        window,
                        cx,
                    );
                    handle.prepaint_at(b.origin, window, cx);
                    handles.push(handle);
                }
                if tag.expandable {
                    let d = Bounds::new(point(origin.x - px(INDENT), origin.y), size(px(INDENT), px(h)));
                    disclosures.push((d, tag.expanded));
                }
                placed.push((i, row_bounds));
                painted.push((p.row, self.rows[i].node, rect(row_bounds)));
            }
        });
        let mut presenters = self.presenters.borrow_mut();
        let state = presenters.entry(self.node).or_default();
        state.painted = painted;
        state.viewport = rect(view);
        if state.seen != self.props.selected {
            state.seen = self.props.selected.clone();
            if state.reported != self.props.selected {
                state.reported = self.props.selected.clone();
                state.anchor = self.props.selected.last().copied();
                state.cursor = state.anchor;
            }
        }
        if edge != 0.
            && let Some(m) = state.marquee.as_mut()
        {
            let now = swept(&handle, rect(view), m);
            if now != m.swept {
                m.swept = now;
                let rows = selection::marquee(handle.0.borrow().ids(), &m.base, &m.swept, m.extend);
                state.reported = rows.clone();
                self.emit.emit(RowsSelected { rows, cursor: state.cursor });
                (self.wake)(cx);
            }
        }
        let preview = state.drag.as_ref().map(|d| (d.at - d.grab, d.size, d.item.ids.len()));
        drop(presenters);
        if let Some((origin, size, count)) = preview {
            use gpui::{ParentElement as _, Styled as _};
            let mut card = gpui::div()
                .w(size.width)
                .h(size.height)
                .rounded(px(6.))
                .border_1()
                .border_color(self.accent)
                .bg(self.selection_color.opacity(0.85))
                .opacity(0.9);
            if count > 1 {
                card = card.child(
                    gpui::div()
                        .absolute()
                        .top(px(-8.))
                        .right(px(-8.))
                        .min_w(px(20.))
                        .h(px(20.))
                        .px(px(5.))
                        .rounded_full()
                        .bg(self.accent)
                        .text_color(gpui::white())
                        .text_size(px(12.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(count.to_string()),
                );
            }
            let mut card = card.into_any_element();
            card.layout_as_root(gpui::AvailableSpace::min_size(), window, cx);
            window.defer_draw(card, origin, 2, None);
        }
        Prepainted { hitbox, placed, selected: sel, disclosures, header, scrolled: (synced, range), handles }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepainted: &mut Prepainted,
        window: &mut gpui::Window,
        cx: &mut App,
    ) {
        // Before the list's own listeners, so the list sees every move before GPUI's drag start
        // on a row stops it from reaching further.
        for handle in &mut prepainted.handles {
            handle.paint(window, cx);
        }
        let emit = self.emit.clone();
        let wake = self.wake.clone();
        // An unselectable row leaves the pointer and presses to its own content.
        let rows: Vec<(RowId, Bounds<Pixels>)> = prepainted
            .placed
            .iter()
            .filter(|(i, _)| self.rows[*i].tag.selectable)
            .map(|(i, b)| (self.rows[*i].tag.row, *b))
            .collect();
        let unselectable: HashSet<RowId> = prepainted
            .placed
            .iter()
            .filter(|(i, _)| !self.rows[*i].tag.selectable)
            .map(|(i, _)| self.rows[*i].tag.row)
            .collect();
        let disclosures: Vec<(RowId, Bounds<Pixels>, bool)> = prepainted
            .placed
            .iter()
            .map(|(i, b)| (self.rows[*i].tag.row, *b, meta(&self.props.handle, self.rows[*i].tag.row)))
            .filter(|(_, _, m)| m.expandable)
            .map(|(r, b, m)| (r, b, m.expanded))
            .collect();
        let selection = self.props.selection;
        let selected = self.props.selected.clone();
        let tree = self.props.kind == ListKind::Tree;
        let grid = self.props.kind == ListKind::Grid;
        let hitbox = prepainted.hitbox.clone();
        let view_rect = self.presenters.borrow().get(&self.node).map_or(rect(bounds), |p| p.viewport);
        // The row under the pointer gets the hover fill, as GTK's navigation lists and boxed
        // tables show it; a move onto another row repaints. A plain list of mixed rows (a page
        // of headers and shelves) leaves hover to the rows themselves.
        let hovers = selection != Selection::None
            && (self.props.inset || matches!(self.props.kind, ListKind::Table(_) | ListKind::Tree));
        // A grid's cell is its own box; a list row takes the press across the whole width, the
        // indent of a tree row included, as NSOutlineView's rows do.
        let row_at = move |rows: &[(RowId, Bounds<Pixels>)], at: Point<Pixels>| {
            rows.iter()
                .find(|(_, b)| if grid { b.contains(&at) } else { at.y >= b.top() && at.y < b.bottom() })
                .map(|(r, b)| (*r, *b))
        };
        let hovered = (hovers && hitbox.is_hovered(window)).then(|| row_at(&rows, window.mouse_position())).flatten();
        if hovers {
            let (rows, hitbox) = (rows.clone(), hitbox.clone());
            let shown = hovered.map(|(r, _)| r);
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let now = hitbox.is_hovered(window).then(|| row_at(&rows, event.position)).flatten().map(|(r, _)| r);
                if now != shown {
                    window.refresh();
                }
            });
        }
        let ctx = Pick {
            node: self.node,
            handle: self.props.handle.clone(),
            presenters: self.presenters.clone(),
            emit: emit.clone(),
            wake: wake.clone(),
            selection,
            selected: selected.clone(),
            unselectable: Rc::new(unselectable),
        };
        let all: Vec<(RowId, Bounds<Pixels>)> =
            prepainted.placed.iter().map(|(i, b)| (self.rows[*i].tag.row, *b)).collect();
        let marquees = matches!(self.props.kind, ListKind::Table(_) | ListKind::Grid) && selection != Selection::None;
        let drags = self.props.drag_kind.is_some();
        let drag_kind = self.props.drag_kind.clone();
        let drag_ids = Rc::new(self.props.drag_ids.clone());
        let focus = self.focus.clone();
        let pick = ctx.clone();
        // A press a row's own control took focus for (a text field renaming the row) is that
        // control's: the list neither selects, activates nor takes the keys.
        let before: Rc<RefCell<Option<gpui::FocusHandle>>> = Rc::default();
        let held = before.clone();
        window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture {
                *held.borrow_mut() = window.focused(cx);
            }
        });
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                return;
            }
            let now = window.focused(cx);
            if now.is_some() && now != before.borrow_mut().take() && now.as_ref() != Some(&focus.focus) {
                return;
            }
            let at = event.position;
            let in_view = at.y.as_f32() >= view_rect.origin.y;
            if event.button == MouseButton::Left && in_view {
                focus.pressed();
                focus.take_focus(window, cx);
            }
            if tree
                && event.button == MouseButton::Left
                && let Some((row, _, expanded)) = disclosures.iter().find(|(_, b, _)| {
                    at.y >= b.top() && at.y < b.bottom() && at.x < b.left() && at.x >= b.left() - px(INDENT)
                })
            {
                emit.emit(RowDisclosed { row: *row, expanded: !expanded });
                wake(cx);
                return;
            }
            let extend = Extend::from(crate::gestures::key_modifiers(&event.modifiers));
            let hit = row_at(&all, at).map(|(r, _)| r);
            match (event.button, hit) {
                (MouseButton::Left, Some(row)) if pick.unselectable.contains(&row) => {}
                (MouseButton::Left, Some(row)) if event.click_count >= 2 => {
                    emit.emit(RowActivated(row));
                    wake(cx);
                }
                (MouseButton::Left, Some(row)) if selection != Selection::None => {
                    if drags && let Some((_, b)) = all.iter().find(|(r, _)| *r == row) {
                        pick.with(|p| p.press = Some((row, at, *b)));
                    }
                    if selection == Selection::Multiple && extend == Extend::default() && selected.contains(&row) {
                        pick.with(|p| p.pending = Some((row, at)));
                        return;
                    }
                    pick.click(row, extend, cx);
                }
                // The context menu acts on what is selected: a right click outside the selection
                // selects that row alone first, as Finder and Nautilus do.
                (MouseButton::Right, Some(row))
                    if selection != Selection::None
                        && !selected.contains(&row)
                        && !pick.unselectable.contains(&row) =>
                {
                    pick.click(row, Extend::default(), cx);
                }
                (MouseButton::Left, None) if in_view && selection != Selection::None => {
                    let keep = selection == Selection::Multiple && extend != Extend::default();
                    if !keep && !selected.is_empty() {
                        pick.report(Vec::new(), None, cx);
                    }
                    if marquees && selection == Selection::Multiple {
                        let top = pick.handle.0.borrow().scroll_top();
                        let from = (at.x.as_f32(), top + f64::from(at.y.as_f32() - view_rect.origin.y));
                        let base = if keep { selected.clone() } else { Vec::new() };
                        let m = Marquee { from, to: (at.x.as_f32(), at.y.as_f32()), base, extend, swept: Vec::new() };
                        pick.with(|p| p.marquee = Some(m));
                    }
                }
                _ => {}
            }
        });
        let pick = ctx.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            if let Some(kind) = &drag_kind
                && pick.drag(kind, &drag_ids, event, cx)
            {
                window.refresh();
                return;
            }
            let at = (event.position.x.as_f32(), event.position.y.as_f32());
            let moved = pick.with(|p| {
                if p.pending.is_some_and(|(_, from)| {
                    (from.x.as_f32() - at.0).abs() > SLOP || (from.y.as_f32() - at.1).abs() > SLOP
                }) {
                    p.pending = None;
                }
                let m = p.marquee.as_mut()?;
                m.to = at;
                let now = swept(&pick.handle, p.viewport, m);
                (now != m.swept).then(|| {
                    m.swept = now;
                    selection::marquee(pick.handle.0.borrow().ids(), &m.base, &m.swept, m.extend)
                })
            });
            if let Some(rows) = moved.flatten() {
                let cursor = pick.with(|p| p.cursor).flatten();
                pick.report(rows, cursor, cx);
            }
            if pick.with(|p| p.marquee.is_some()) == Some(true) {
                window.refresh();
            }
        });
        let pick = ctx.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                return;
            }
            let (pending, marquee, drag) = pick
                .with(|p| (p.pending.take(), p.marquee.take(), (p.press.take(), p.drag.take()).1))
                .unwrap_or_default();
            if let Some(drag) = drag {
                let source =
                    crate::nav::drag_source(&drag.item, nd_ui::GesturePhase::Ended, event.position, &event.modifiers);
                pick.emit.emit(source);
                (pick.wake)(cx);
                cx.stop_propagation();
                window.refresh();
                return;
            }
            if let Some((row, _)) = pending {
                pick.click(row, Extend::default(), cx);
            }
            if marquee.is_some() {
                window.refresh();
            }
        });
        // The platform's drag ended (dropped in another app, or let go nowhere): the release
        // never reached this window, so the drag ends where the pointer left it. A drop back in
        // this window arrives as a release first and has already ended it.
        let pick = ctx.clone();
        window.on_mouse_event(move |event: &gpui::FileDropEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !matches!(event, gpui::FileDropEvent::Ended) {
                return;
            }
            let Some(drag) = pick.with(|p| p.drag.take_if(|d| d.handed)).flatten() else { return };
            pick.with(|p| p.press = None);
            let mut source =
                crate::nav::drag_source(&drag.item, nd_ui::GesturePhase::Ended, drag.at, &gpui::Modifiers::default());
            source.native = true;
            pick.emit.emit(source);
            (pick.wake)(cx);
            window.refresh();
        });
        if selection != Selection::None {
            let (pick, focus) = (ctx.clone(), self.focus.clone());
            let page = (view_rect.size.height / self.props.handle.0.borrow().estimate().max(1.) as f32) as usize;
            window.on_key_event(move |event: &KeyDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !focus.focus.is_focused(window) {
                    return;
                }
                if pick.key(event, tree, page.saturating_sub(1).max(1), cx) {
                    focus.keyboard();
                    cx.stop_propagation();
                    window.refresh();
                }
            });
        }

        if let (Some(header), Some(_)) = (self.header.as_mut(), prepainted.header) {
            header.paint(window, cx);
        }
        let view = {
            let p = self.presenters.borrow();
            p.get(&self.node).map(|p| p.viewport)
        };
        let view = view.map_or(bounds, |v| {
            Bounds::new(point(px(v.origin.x), px(v.origin.y)), size(px(v.size.width), px(v.size.height)))
        });
        let selected_rows: HashSet<RowId> = self.props.selected.iter().copied().collect();
        window.with_content_mask(Some(ContentMask { bounds: view }), |window| {
            if let Some((_, b)) = hovered.filter(|(r, _)| !selected_rows.contains(r)) {
                let full = Bounds::new(point(view.origin.x, b.origin.y), size(view.size.width, b.size.height));
                if self.props.inset {
                    let s = self.sidebar;
                    let inner = Bounds::new(
                        point(full.origin.x + px(s.inset), full.origin.y + px(1.)),
                        size(full.size.width - px(s.inset * 2.), full.size.height - px(2.)),
                    );
                    window.paint_quad(fill(inner, self.hover_color).corner_radii(px(s.radius)));
                } else {
                    window.paint_quad(fill(full, self.hover_color));
                }
            }
            for b in &prepainted.selected {
                if self.props.inset {
                    // A sidebar's selection: inset from the edges, rounded.
                    let s = self.sidebar;
                    let inner = Bounds::new(
                        point(b.origin.x + px(s.inset), b.origin.y + px(1.)),
                        size(b.size.width - px(s.inset * 2.), b.size.height - px(2.)),
                    );
                    window.paint_quad(fill(inner, self.selection_color).corner_radii(px(s.radius)));
                    if let Some(pill) = s.pill {
                        let h = px(16.).min(inner.size.height - px(8.));
                        let at = point(inner.origin.x, inner.origin.y + (inner.size.height - h) / 2.);
                        window.paint_quad(fill(Bounds::new(at, size(px(3.), h)), pill).corner_radii(px(1.5)));
                    }
                } else if self.focus.focus.is_focused(window) {
                    window.paint_quad(fill(*b, self.focused_selection_color));
                } else {
                    window.paint_quad(fill(*b, self.selection_color));
                }
            }
            for (b, expanded) in &prepainted.disclosures {
                paint_disclosure(window, *b, *expanded, self.disclosure_color);
            }
            for (i, _) in &prepainted.placed {
                self.rows[*i].element.paint(window, cx);
            }
            let target = self.presenters.borrow().get(&self.node).and_then(|p| p.drop_row);
            if let Some((_, b)) =
                target.and_then(|t| prepainted.placed.iter().find(|(i, _)| self.rows[*i].tag.row == t))
            {
                let b = if self.props.kind == ListKind::Grid {
                    *b
                } else {
                    Bounds::new(point(view.origin.x, b.origin.y), size(view.size.width, b.size.height))
                };
                let mut ring = gpui::outline(b, self.accent, gpui::BorderStyle::Solid).corner_radii(px(6.));
                ring.border_widths = (px(2.)).into();
                window.paint_quad(ring);
            }
            // The keyboard's row carries the ring only where the selection does not already mark
            // it (a Ctrl+arrow walk), as in Finder: a focused list's selection is its tint, and a
            // list with no such row draws no ring round its edge.
            let cursor = self.presenters.borrow().get(&self.node).and_then(|p| p.cursor);
            let placed = cursor
                .filter(|r| !selected_rows.contains(r))
                .and_then(|r| prepainted.placed.iter().find(|(i, _)| self.rows[*i].tag.row == r));
            if let Some((_, b)) = placed.filter(|_| self.focus.ring(window)) {
                let b = if self.props.kind == ListKind::Grid {
                    *b
                } else {
                    Bounds::new(point(view.origin.x, b.origin.y), size(view.size.width, b.size.height))
                };
                let b = if self.props.inset {
                    let s = self.sidebar;
                    Bounds::new(
                        point(b.origin.x + px(s.inset), b.origin.y),
                        size(b.size.width - px(s.inset * 2.), b.size.height),
                    )
                } else {
                    b
                };
                let mut ring =
                    gpui::outline(b, self.ring_color, gpui::BorderStyle::Solid).corner_radii(self.ring_radius);
                ring.border_widths = (px(2.)).into();
                window.paint_quad(ring);
            }
            if let Some(band) = self.band(view) {
                window.paint_quad(fill(band, self.accent.opacity(0.2)));
                window.paint_quad(gpui::outline(band, self.accent.opacity(0.8), gpui::BorderStyle::Solid));
            }
            if let Some(edges) = self.edges {
                let (offset, max) = prepainted.scrolled;
                crate::scrollbar::paint_adwaita_edges(window, view, edges, offset, max);
            }
        });
    }
}

/// What the presenter's input handlers share: the list, its presenter state and the way to
/// report a selection.
#[derive(Clone)]
struct Pick {
    node: NodeId,
    handle: ListHandle,
    presenters: Presenters,
    emit: Emit,
    wake: Wake,
    selection: Selection,
    selected: Vec<RowId>,
    unselectable: Rc<HashSet<RowId>>,
}

impl Pick {
    fn with<T>(&self, f: impl FnOnce(&mut Presenter) -> T) -> Option<T> {
        self.presenters.borrow_mut().get_mut(&self.node).map(f)
    }

    fn report(&self, rows: Vec<RowId>, cursor: Option<RowId>, cx: &mut App) {
        self.with(|p| p.reported = rows.clone());
        self.emit.emit(RowsSelected { rows, cursor });
        (self.wake)(cx);
    }

    fn click(&self, row: RowId, extend: Extend, cx: &mut App) {
        let anchor = self.with(|p| p.anchor).flatten();
        let picked = {
            let core = self.handle.0.borrow();
            selection::click(core.ids(), &self.selected, anchor, row, self.selection, extend)
        };
        self.with(|p| {
            p.anchor = picked.anchor;
            p.cursor = picked.cursor;
        });
        self.report(picked.selected, picked.cursor, cx);
    }

    /// A move with the button held: past the slop a press on a row drags the selection (or the
    /// row alone, selected first when it was not), and later moves carry the drag. True while
    /// a drag runs.
    fn drag(
        &self,
        kind: &SharedString,
        ids: &HashMap<RowId, SharedString>,
        event: &MouseMoveEvent,
        cx: &mut App,
    ) -> bool {
        let at = event.position;
        let running = self.with(|p| {
            let d = p.drag.as_mut()?;
            d.at = at;
            Some(d.item.clone())
        });
        if let Some(Some(item)) = running {
            self.emit.emit(crate::nav::drag_source(&item, nd_ui::GesturePhase::Changed, at, &event.modifiers));
            (self.wake)(cx);
            return true;
        }
        let Some(Some((row, from, bounds))) = self.with(|p| p.press) else { return false };
        if (at.x - from.x).abs().as_f32() <= SLOP && (at.y - from.y).abs().as_f32() <= SLOP {
            return false;
        }
        let rows = if self.selected.contains(&row) {
            self.selected.clone()
        } else {
            self.click(row, Extend::default(), cx);
            vec![row]
        };
        let item = nd_ui::DragItem::many(kind.clone(), rows.iter().filter_map(|r| ids.get(r).cloned()));
        let grab = point(from.x - bounds.origin.x, from.y - bounds.origin.y);
        let size = gpui::size(bounds.size.width.min(px(240.)), bounds.size.height.min(px(120.)));
        self.with(|p| {
            p.press = None;
            p.pending = None;
            p.marquee = None;
            p.drag = Some(RowsDrag {
                item: item.clone(),
                rows,
                handed: false,
                at,
                grab: point(grab.x.min(size.width), grab.y.min(size.height)),
                size,
            });
        });
        self.emit.emit(crate::nav::drag_source(&item, nd_ui::GesturePhase::Began, from, &event.modifiers));
        self.emit.emit(crate::nav::drag_source(&item, nd_ui::GesturePhase::Changed, at, &event.modifiers));
        (self.wake)(cx);
        true
    }

    /// A key while the list has focus; true when it was the list's.
    fn key(&self, event: &KeyDownEvent, tree: bool, page: usize, cx: &mut App) -> bool {
        let k = &event.keystroke;
        let m = k.modifiers;
        let (anchor, cursor) = self.with(|p| (p.anchor, p.cursor)).unwrap_or_default();
        let (ids, lanes) = {
            let core = self.handle.0.borrow();
            (core.ids().to_vec(), core.lanes())
        };
        if k.key == "a" && m.secondary() && !m.shift && !m.alt {
            if self.selection != Selection::Multiple {
                return false;
            }
            let rows: Vec<RowId> = ids.iter().copied().filter(|r| !self.unselectable.contains(r)).collect();
            self.report(rows, cursor, cx);
            return true;
        }
        if m.secondary() || m.alt {
            return false;
        }
        let at = cursor.and_then(|c| ids.iter().position(|r| *r == c));
        if (k.key == "enter" || k.key == "kp_enter") && !m.shift {
            let Some(row) = cursor else { return false };
            self.emit.emit(RowActivated(row));
            (self.wake)(cx);
            return true;
        }
        if tree
            && let (Some(i), Some(row)) = (at, cursor)
            && (k.key == "left" || k.key == "right")
        {
            let meta = self.handle.0.borrow().meta(i);
            let open = k.key == "right";
            if meta.expandable && meta.expanded != open {
                self.emit.emit(RowDisclosed { row, expanded: open });
                (self.wake)(cx);
                return true;
            }
            // Left on a collapsed row goes to its parent, right on an open one to its first
            // child, as in NSOutlineView.
            let target = if open {
                (meta.expandable && i + 1 < ids.len()).then_some(i + 1)
            } else {
                (0..i).rev().find(|j| self.handle.0.borrow().meta(*j).depth < meta.depth)
            };
            if let Some(t) = target {
                self.land(&ids, anchor, t, false, cx);
            }
            return true;
        }
        let nav = match k.key.as_str() {
            "up" => Nav::Up,
            "down" => Nav::Down,
            "left" => Nav::Left,
            "right" => Nav::Right,
            "home" => Nav::Home,
            "end" => Nav::End,
            "pageup" => Nav::PageUp,
            "pagedown" => Nav::PageDown,
            _ => return false,
        };
        let Some(t) = selection::step(ids.len(), lanes, at, nav, page, |i| !self.unselectable.contains(&ids[i])) else {
            return matches!(nav, Nav::Up | Nav::Down) || lanes > 1;
        };
        self.land(&ids, anchor, t, m.shift, cx);
        true
    }

    /// The cursor moved by a key to the row at `index`: select and scroll it into view.
    fn land(&self, ids: &[RowId], anchor: Option<RowId>, index: usize, range: bool, cx: &mut App) {
        let picked = selection::key(ids, anchor, ids[index], self.selection, range);
        self.handle.0.borrow_mut().scroll_to(ids[index], Align::Nearest, 0., false);
        self.with(|p| {
            p.anchor = picked.anchor;
            p.cursor = picked.cursor;
        });
        self.report(picked.selected, picked.cursor, cx);
    }
}

/// A small filled triangle: pointing right when collapsed, down when expanded.
fn paint_disclosure(window: &mut gpui::Window, b: Bounds<Pixels>, expanded: bool, color: Hsla) {
    let c = point(b.left() + b.size.width / 2., b.top() + px(INDENT / 2. + 2.));
    let s = px(4.);
    let mut path = gpui::Path::new(if expanded { point(c.x - s, c.y - s / 2.) } else { point(c.x - s / 2., c.y - s) });
    if expanded {
        path.line_to(point(c.x + s, c.y - s / 2.));
        path.line_to(point(c.x, c.y + s / 2.));
    } else {
        path.line_to(point(c.x + s / 2., c.y));
        path.line_to(point(c.x - s / 2., c.y + s));
    }
    window.paint_path(path, color);
}

/// A table's header: column titles, clicking a sortable one reports `SortChanged`.
pub(crate) fn header(
    columns: &[nd_virtual::Column],
    look: &crate::render::Look,
    text: gpui::Hsla,
    emit: Emit,
    wake: Wake,
) -> AnyElement {
    use gpui::{InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, div};
    let mut row =
        div().flex().flex_row().h(px(HEADER)).w_full().border_b_1().border_color(look.border).text_color(text);
    for (i, c) in columns.iter().enumerate() {
        let (emit, wake) = (emit.clone(), wake.clone());
        let (id, sortable, sort) = (c.id.clone(), c.sortable, c.sort);
        let arrow = match sort {
            Some(nd_virtual::SortDirection::Ascending) => " \u{25b2}",
            Some(nd_virtual::SortDirection::Descending) => " \u{25bc}",
            None => "",
        };
        row = row.child(
            div()
                .id(ElementId::Integer(i as u64))
                .w(px(c.width))
                .flex_shrink_0()
                .px(px(6.))
                .flex()
                .items_center()
                .overflow_hidden()
                .child(format!("{}{arrow}", c.title))
                .when(sortable, |d| {
                    d.on_click(move |_, _, cx| {
                        let direction = match sort {
                            Some(nd_virtual::SortDirection::Ascending) => nd_virtual::SortDirection::Descending,
                            _ => nd_virtual::SortDirection::Ascending,
                        };
                        emit.emit(nd_virtual::SortChanged { column: id.clone(), direction });
                        wake(cx);
                    })
                }),
        );
    }
    row.into_any_element()
}

use gpui::prelude::FluentBuilder;
