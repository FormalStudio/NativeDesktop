//! Drag and drop between elements, in one window or across the app's windows (ARCHITECTURE.md
//! 6.1). A source is a `div` with [`Div::draggable`](crate::Div::draggable), or a widget whose
//! parts drag (a source list's rows) and reports them as [`DragSource`]. A target names the
//! kinds it takes with `accepts_drops`. `nd-ui` owns the session: it finds the window and the
//! innermost accepting target under the pointer from the windows' screen frames and the
//! backend's bounds, and reports [`DragOver`] and [`Dropped`] to the target and [`DragEnded`]
//! to the source. A backend may own the pointer for the length of a drag (AppKit's dragging
//! session draws the image across windows); it then reports where the pointer is per window.
//!
//! A source that lifts (`.lift_on_drag()`) has the backend carry a picture of it under the
//! pointer for the whole drag ([`DragPreview`]). A target that springs (`.on_spring_load`)
//! hears [`SpringLoaded`] once a drag has stayed over it, or over one place in it, for
//! [`SPRING_LOAD_DELAY`]: the frames of a drag over such a target keep the clock running.

use nd_core::{NodeId, Point, Rect, Size, WindowId};
use nd_gesture::{GesturePhase, KeyModifiers};
use nd_gpui::gpui::SharedString;

use crate::App;

/// What is dragged: an app-defined kind targets filter on, and the dragged things' ids. `id` is
/// the first of `ids`, the one a single drag carries.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DragItem {
    pub kind: SharedString,
    pub id: SharedString,
    pub ids: Vec<SharedString>,
}

impl DragItem {
    /// The kind of files dragged in from outside the app (a file manager's), each id a path.
    pub const FILES: &str = "files";

    pub fn new(kind: impl Into<SharedString>, id: impl Into<SharedString>) -> Self {
        let id = id.into();
        Self { kind: kind.into(), id: id.clone(), ids: vec![id] }
    }

    /// Several things dragged together, such as the selected rows of a list.
    pub fn many(kind: impl Into<SharedString>, ids: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        let ids: Vec<SharedString> = ids.into_iter().map(Into::into).collect();
        Self { kind: kind.into(), id: ids.first().cloned().unwrap_or_default(), ids }
    }
}

/// Where a widget that places drops itself (a source list, a list, a tab strip) would put one:
/// the index among the children of `parent`, or among the roots without one. `on` is the item
/// the drop lands on rather than between (a folder row, a tab), by its id in the widget.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropSlot {
    pub parent: Option<SharedString>,
    pub index: usize,
    pub on: Option<SharedString>,
}

impl DropSlot {
    /// Between items: at `index` among `parent`'s children.
    pub fn at(parent: Option<SharedString>, index: usize) -> Self {
        Self { parent, index, on: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragOverPhase {
    Entered,
    Moved,
    Exited,
}

/// A drag the target accepts is over it.
#[derive(Clone, Debug, PartialEq)]
pub struct DragOver {
    pub item: DragItem,
    pub phase: DragOverPhase,
    /// In the target window's coordinates.
    pub position: Point,
    /// From the target's top-left.
    pub local: Point,
    pub source_window: WindowId,
    pub slot: Option<DropSlot>,
    /// Held now: the platform's copy key (Control, Option on macOS) asks for a copy, Shift for a
    /// move.
    pub modifiers: KeyModifiers,
}

/// The pointer was released over the target.
#[derive(Clone, Debug, PartialEq)]
pub struct Dropped {
    pub item: DragItem,
    pub position: Point,
    pub local: Point,
    pub source_window: WindowId,
    pub slot: Option<DropSlot>,
    /// Held at the release.
    pub modifiers: KeyModifiers,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragOutcome {
    /// A target in `window` took it.
    Dropped { window: WindowId },
    /// Escape, or a release over a window where nothing takes it.
    Cancelled,
    /// Released outside every window of the app, at `screen` (global points, top-left origin).
    Outside { screen: Point },
}

/// A drag stayed over a target that springs for [`SPRING_LOAD_DELAY`]: open what is under
/// it (a folder) so the drag can go on inside. `slot` is where in a widget target it is, as
/// [`DragOver`] has it.
#[derive(Clone, Debug, PartialEq)]
pub struct SpringLoaded {
    pub item: DragItem,
    pub slot: Option<DropSlot>,
}

/// How long a drag stays over a spring-loaded target before it opens, in seconds.
pub const SPRING_LOAD_DELAY: f64 = 0.7;

/// What the picture a drag carries does, for [`Backend::drag_preview`](crate::backend::Backend::drag_preview).
/// Frames are in global screen points, top-left origin, at the source's own size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragPreview {
    /// Under the pointer with its frame at `frame`. `lifted`: the source asked to lift, so the
    /// picture rises a little larger, casting a shadow, and shows over its own window too.
    /// Otherwise it is a plain picture of a drag outside its window.
    At { frame: Rect, lifted: bool },
    /// The drag is over: the picture shrinks into `frame` and fades (`into`, a drop on a
    /// target), or settles back onto it (Escape, or a release where nothing takes it), then
    /// goes.
    Land { frame: Rect, into: bool },
    /// Gone at once.
    Gone,
}

/// Reported to the source once its drag is over.
#[derive(Clone, Debug, PartialEq)]
pub struct DragEnded {
    pub item: DragItem,
    pub outcome: DragOutcome,
}

/// Where a pointer reported by a widget source is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragPoint {
    /// In the coordinates of the source's window.
    Window(Point),
    /// Global screen points, top-left origin.
    Screen(Point),
}

/// A backend reports a drag from a part of a widget (a source list row) to the widget's node.
/// `native` says the backend owns the pointer meanwhile: moves then arrive through
/// [`App::drag_moved`] from whichever window the pointer is over, and the release through
/// [`App::drag_released`].
#[derive(Clone, Debug, PartialEq)]
pub struct DragSource {
    pub item: DragItem,
    pub phase: GesturePhase,
    pub at: DragPoint,
    pub native: bool,
    pub modifiers: KeyModifiers,
}

/// What a node does in drags: the item it drags as, and the kinds it takes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DragSpec {
    pub item: Option<DragItem>,
    pub accepts: Vec<SharedString>,
    /// Its drag carries a lifted picture of it.
    pub lift: bool,
    /// It hears [`SpringLoaded`].
    pub springs: bool,
}

/// Where a lifted source was grabbed, from its top-left, and its size.
#[derive(Clone, Copy, Debug)]
struct Lift {
    grab: Point,
    size: Size,
}

/// The target a drag is over and where in it, since when: for spring loading.
#[derive(Clone, Debug)]
struct Dwell {
    node: NodeId,
    on: Option<SharedString>,
    /// The first frame after the drag came over it. The clock only runs on frames, so the time
    /// when the drag arrived can be the last frame of an idle window, long gone.
    since: Option<f64>,
    sprung: bool,
}

pub(crate) struct Session {
    item: DragItem,
    source_window: WindowId,
    source: NodeId,
    native: bool,
    over: Option<(WindowId, NodeId)>,
    /// Where the target over said the drop would land.
    slot: Option<DropSlot>,
    /// Where the pointer last was, in screen points.
    screen: Option<Point>,
    /// The source is shown as a picture following the pointer outside its window.
    previewing: bool,
    modifiers: KeyModifiers,
    lift: Option<Lift>,
    dwell: Option<Dwell>,
    /// Where the pointer last was over a target, in that target window's coordinates: the
    /// frames of a drag over a target that springs look again from there, since what is under
    /// a still pointer changes when a folder opens.
    last: Option<(WindowId, Point)>,
}

fn contains(r: &Rect, p: Point) -> bool {
    p.x >= r.origin.x && p.y >= r.origin.y && p.x < r.origin.x + r.size.width && p.y < r.origin.y + r.size.height
}

impl App {
    /// Starts a drag of `item` from `source` in `window`. A drag already running is cancelled.
    pub fn begin_drag(&mut self, window: WindowId, source: NodeId, item: DragItem, native: bool) {
        if self.drag.is_some() {
            self.cancel_drag();
        }
        self.drag = Some(Session {
            item,
            source_window: window,
            source,
            native,
            over: None,
            slot: None,
            screen: None,
            previewing: false,
            modifiers: KeyModifiers::default(),
            lift: None,
            dwell: None,
            last: None,
        });
    }

    /// The running drag carries a lifted picture of its source, grabbed at `at` (the press, in
    /// the source window's coordinates).
    fn lift(&mut self, at: Point) {
        let Some(session) = self.drag.as_ref() else { return };
        let Some(bounds) = self.backend.probe(session.source).bounds else { return };
        let grab = Point { x: at.x - bounds.origin.x, y: at.y - bounds.origin.y };
        if let Some(session) = self.drag.as_mut() {
            session.lift = Some(Lift { grab, size: bounds.size });
        }
    }

    /// Whether a drag's frames have work: a spring-loaded target waiting out its delay.
    pub(crate) fn drag_needs_frame(&self) -> bool {
        self.drag.as_ref().is_some_and(|s| s.dwell.as_ref().is_some_and(|d| !d.sprung))
    }

    /// A frame of the running drag: looks again at what is under a still pointer, and opens a
    /// spring-loaded target the drag has stayed over long enough.
    pub(crate) fn drag_frame(&mut self) {
        let Some(session) = self.drag.as_ref() else { return };
        if let Some((window, at)) = session.last {
            let hit = self.target_at(window, at).map(|(n, local)| (window, n, at, local));
            if hit.map(|h| h.1) != self.drag.as_ref().and_then(|s| s.over).map(|o| o.1) {
                self.hover(hit);
            }
        }
        let now = self.now();
        let Some(session) = self.drag.as_mut() else { return };
        let Some(dwell) = session.dwell.as_mut().filter(|d| !d.sprung) else { return };
        if now - *dwell.since.get_or_insert(now) < SPRING_LOAD_DELAY {
            return;
        }
        dwell.sprung = true;
        let node = dwell.node;
        let sprung = SpringLoaded { item: session.item.clone(), slot: session.slot.clone() };
        self.events().push(node, Box::new(sprung));
    }

    /// The keys held during the running drag, for a backend that owns its pointer.
    pub fn set_drag_modifiers(&mut self, modifiers: KeyModifiers) {
        if let Some(session) = self.drag.as_mut() {
            session.modifiers = modifiers;
        }
    }

    /// A drag from outside the app (files from a file manager) entered `window`. Targets that
    /// accept `item.kind` see it as any drag; [`DragEnded`] goes to the window's root.
    pub fn begin_external_drag(&mut self, window: WindowId, item: DragItem) {
        let Some(root) = self.windows.get(&window).and_then(|w| w.tree.as_ref()).map(|t| t.root_node()) else {
            return;
        };
        self.begin_drag(window, root, item, false);
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether the running drag is over a target that takes it.
    pub fn drag_has_target(&self) -> bool {
        self.drag.as_ref().is_some_and(|s| s.over.is_some())
    }

    /// Escape ends the running drag, unless the backend owns it (its session takes the key).
    pub(crate) fn drag_escapes(&self) -> bool {
        self.drag.as_ref().is_some_and(|s| !s.native)
    }

    /// The pointer of the running drag is at `at` in `window`'s coordinates.
    pub fn drag_moved(&mut self, window: WindowId, at: Point) {
        let Some(screen) = self.window_frame(window).map(|f| Point { x: f.origin.x + at.x, y: f.origin.y + at.y })
        else {
            return;
        };
        let Some(session) = self.drag.as_mut() else { return };
        session.screen = Some(screen);
        let target = if session.native { Some((window, at)) } else { self.window_at(screen, window) };
        let hit = target.and_then(|(w, p)| self.target_at(w, p).map(|(n, local)| (w, n, p, local)));
        if let Some(session) = self.drag.as_mut() {
            session.last = target;
        }
        self.hover(hit);
        self.preview(target.map(|(w, _)| w), screen);
    }

    /// The window saw the pointer of the running drag move, or its button go up (`released`),
    /// at `at` in its coordinates. A source's own gesture reports its drag; this carries the drag
    /// on once the source has left the tree (a spring-loaded folder replaced the page it was
    /// on), since no gesture outlives its element. While the source is there it does nothing.
    pub fn drag_pointer(&mut self, window: WindowId, at: Point, released: bool) {
        let Some(session) = self.drag.as_ref().filter(|s| !s.native) else { return };
        let (source, source_window) = (session.source, session.source_window);
        let gone = self.windows.get(&source_window).and_then(|w| w.tree.as_ref()).is_none_or(|t| t.gone(source));
        if !gone {
            return;
        }
        if released {
            self.drag_released(window, at);
        } else {
            self.drag_moved(window, at);
        }
    }

    /// The pointer of a drag the backend owns left every window.
    pub fn drag_exited(&mut self) {
        self.hover(None);
    }

    /// The pointer of the running drag was released at `at` in `window`'s coordinates. Returns
    /// whether a target took it.
    pub fn drag_released(&mut self, window: WindowId, at: Point) -> bool {
        self.drag_moved(window, at);
        let Some(session) = self.drag.take() else { return false };
        let screen = session.screen.unwrap_or(at);
        if let Some((w, node)) = session.over {
            self.land(&session, Some((w, node)), screen);
            let _ = self.backend.drop_slot(node, None, &session.item);
            let position =
                self.window_frame(w).map_or(at, |f| Point { x: screen.x - f.origin.x, y: screen.y - f.origin.y });
            let local = self.local(node, position);
            let dropped = Dropped {
                item: session.item.clone(),
                position,
                local,
                source_window: session.source_window,
                slot: session.slot.clone(),
                modifiers: session.modifiers,
            };
            self.events().push(node, Box::new(dropped));
            self.end(session, DragOutcome::Dropped { window: w });
            return true;
        }
        self.land(&session, None, screen);
        let outcome = if self.window_at(screen, window).is_some() {
            DragOutcome::Cancelled
        } else {
            DragOutcome::Outside { screen }
        };
        self.end(session, outcome);
        false
    }

    /// A drag the backend owns ended at `screen` without a target taking it (a release outside
    /// the app's windows, or Escape).
    pub fn drag_finished(&mut self, screen: Point) {
        let Some(session) = self.drag.take() else { return };
        if let Some((_, node)) = session.over {
            let _ = self.backend.drop_slot(node, None, &session.item);
        }
        self.end_preview(&session);
        let inside = self.window_ids().any(|w| self.window_frame(w).is_some_and(|f| contains(&f, screen)));
        let outcome = if inside { DragOutcome::Cancelled } else { DragOutcome::Outside { screen } };
        self.end(session, outcome);
    }

    /// Escape: the drag ends where it started.
    pub fn cancel_drag(&mut self) {
        self.hover(None);
        let Some(session) = self.drag.take() else { return };
        let screen = session.screen.unwrap_or_default();
        self.land(&session, None, screen);
        self.end(session, DragOutcome::Cancelled);
    }

    /// The drag is over: a lifted picture shrinks into the target it was dropped on (`onto`),
    /// or settles back on its source; a plain one outside its window goes.
    fn land(&mut self, session: &Session, onto: Option<(WindowId, NodeId)>, screen: Point) {
        let Some(lift) = session.lift else {
            self.end_preview(session);
            return;
        };
        let on_screen = |app: &App, w: WindowId, r: Rect| {
            app.window_frame(w).map(|f| Rect {
                origin: Point { x: f.origin.x + r.origin.x, y: f.origin.y + r.origin.y },
                size: r.size,
            })
        };
        let (frame, into) = match onto {
            // A widget that places drops (a list) takes it at its row under the pointer, so it
            // shrinks where it is; an element takes it whole.
            Some((w, node)) => {
                let bounds = self.backend.probe(node).bounds.filter(|_| session.slot.is_none());
                let at =
                    Rect { origin: Point { x: screen.x - lift.grab.x, y: screen.y - lift.grab.y }, size: lift.size };
                (bounds.and_then(|b| on_screen(self, w, b)).unwrap_or(at), true)
            }
            None => {
                let home =
                    self.backend.probe(session.source).bounds.and_then(|b| on_screen(self, session.source_window, b));
                let at =
                    Rect { origin: Point { x: screen.x - lift.grab.x, y: screen.y - lift.grab.y }, size: lift.size };
                (home.unwrap_or(at), home.is_none())
            }
        };
        let reduce = self.reduce_motion();
        self.backend.drag_preview(session.source_window, session.source, DragPreview::Land { frame, into }, reduce);
    }

    fn end(&mut self, session: Session, outcome: DragOutcome) {
        self.events().push(session.source, Box::new(DragEnded { item: session.item, outcome }));
    }

    /// Moves the hover to `hit` (window, target, position, local), reporting the exit from the
    /// old target and the entry or move over the new one.
    fn hover(&mut self, hit: Option<(WindowId, NodeId, Point, Point)>) {
        let Some(session) = self.drag.as_mut() else { return };
        let item = session.item.clone();
        let source_window = session.source_window;
        let modifiers = session.modifiers;
        let old = session.over;
        session.over = hit.map(|(w, n, _, _)| (w, n));
        session.slot = None;
        if hit.is_none() {
            session.dwell = None;
        }
        if let Some((_, node)) = old
            && old != hit.map(|(w, n, _, _)| (w, n))
        {
            let _ = self.backend.drop_slot(node, None, &item);
            self.events().push(
                node,
                Box::new(DragOver {
                    item: item.clone(),
                    phase: DragOverPhase::Exited,
                    position: Point::default(),
                    local: Point::default(),
                    source_window,
                    slot: None,
                    modifiers,
                }),
            );
        }
        let springs = hit.is_some_and(|(w, n, _, _)| {
            self.windows.get(&w).and_then(|s| s.tree.as_ref()).is_some_and(|t| t.springs(n))
        });
        if let Some((w, node, position, local)) = hit {
            let phase = if old == Some((w, node)) { DragOverPhase::Moved } else { DragOverPhase::Entered };
            let slot = self.backend.drop_slot(node, Some(position), &item);
            if let Some(session) = self.drag.as_mut() {
                session.slot = slot.clone();
                let on = slot.as_ref().and_then(|s| s.on.clone());
                let same = session.dwell.as_ref().is_some_and(|d| d.node == node && d.on == on);
                if !springs {
                    session.dwell = None;
                } else if !same {
                    session.dwell = Some(Dwell { node, on, since: None, sprung: false });
                }
            }
            self.events()
                .push(node, Box::new(DragOver { item, phase, position, local, source_window, slot, modifiers }));
        }
    }

    /// Shows the source as a picture under the pointer while the pointer is outside its window,
    /// for a drag the backend does not draw itself.
    fn preview(&mut self, over: Option<WindowId>, screen: Point) {
        let reduce = self.reduce_motion();
        let Some(session) = self.drag.as_mut() else { return };
        if session.native {
            return;
        }
        let (w, n) = (session.source_window, session.source);
        if let Some(lift) = session.lift {
            session.previewing = true;
            let frame =
                Rect { origin: Point { x: screen.x - lift.grab.x, y: screen.y - lift.grab.y }, size: lift.size };
            self.backend.drag_preview(w, n, DragPreview::At { frame, lifted: true }, reduce);
            return;
        }
        let outside = over != Some(session.source_window);
        if outside || session.previewing {
            session.previewing = outside;
            let preview = if outside {
                let size = self.backend.probe(n).bounds.map_or(Size::default(), |b| b.size);
                let origin = Point { x: screen.x - size.width / 2., y: screen.y - size.height / 2. };
                DragPreview::At { frame: Rect { origin, size }, lifted: false }
            } else {
                DragPreview::Gone
            };
            self.backend.drag_preview(w, n, preview, reduce);
        }
    }

    fn end_preview(&mut self, session: &Session) {
        if session.previewing {
            let reduce = self.reduce_motion();
            self.backend.drag_preview(session.source_window, session.source, DragPreview::Gone, reduce);
        }
    }

    fn window_frame(&self, window: WindowId) -> Option<Rect> {
        self.backend.window_state(window).and_then(|s| s.frame)
    }

    /// The window under `screen` and the point in its coordinates. `prefer` wins where windows
    /// overlap, since nd does not know their stacking order.
    fn window_at(&self, screen: Point, prefer: WindowId) -> Option<(WindowId, Point)> {
        let at = |w: WindowId| {
            let state = self.backend.window_state(w)?;
            let frame = state.frame.filter(|f| state.visible && contains(f, screen))?;
            Some((w, Point { x: screen.x - frame.origin.x, y: screen.y - frame.origin.y }))
        };
        at(prefer).or_else(|| self.window_ids().filter(|w| *w != prefer).find_map(at))
    }

    fn local(&self, node: NodeId, at: Point) -> Point {
        let origin = self.backend.probe(node).bounds.map_or(Point::default(), |b| b.origin);
        Point { x: at.x - origin.x, y: at.y - origin.y }
    }

    /// The innermost node in `window` that takes the running drag's kind and is under `at`,
    /// with `at` from its top-left.
    fn target_at(&self, window: WindowId, at: Point) -> Option<(NodeId, Point)> {
        let session = self.drag.as_ref()?;
        let kind = &session.item.kind;
        let tree = self.windows.get(&window)?.tree.as_ref()?;
        // A list is a target of its own rows; an element is no target of its own drag.
        let source =
            (session.source_window == window && tree.drag_item(session.source).is_some()).then_some(session.source);
        tree.drop_targets(kind)
            .into_iter()
            .filter(|n| source.is_none_or(|s| !tree.within(*n, s)))
            .filter_map(|n| {
                let probe = self.backend.probe(n);
                let b = probe.bounds.filter(|b| probe.visible != Some(false) && contains(b, at))?;
                Some((n, b))
            })
            .min_by(|a, b| {
                let area = |r: &Rect| r.size.width * r.size.height;
                area(&a.1).total_cmp(&area(&b.1))
            })
            .map(|(n, b)| (n, Point { x: at.x - b.origin.x, y: at.y - b.origin.y }))
    }

    /// Runs the session for a drag event on a `draggable` node, or a widget's [`DragSource`].
    /// Returns whether the event belonged to drag and drop.
    pub(crate) fn drag_event(&mut self, node: NodeId, event: &dyn std::any::Any) -> bool {
        // The source's last events can arrive after it left the tree (its page went in the same
        // batch); they still end its drag.
        let source = self.drag.as_ref().filter(|s| s.source == node).map(|s| s.source_window);
        let window = self
            .windows
            .iter()
            .find(|(_, s)| s.tree.as_ref().is_some_and(|t| t.contains(node)))
            .map(|(w, _)| *w)
            .or(source);
        let Some(window) = window else { return false };
        if let Some(source) = event.downcast_ref::<DragSource>() {
            if source.phase != GesturePhase::Began {
                self.set_drag_modifiers(source.modifiers);
            }
            let at = |app: &App| match source.at {
                DragPoint::Window(p) => Some(p),
                DragPoint::Screen(s) => {
                    app.window_frame(window).map(|f| Point { x: s.x - f.origin.x, y: s.y - f.origin.y })
                }
            };
            match source.phase {
                GesturePhase::Began => {
                    self.begin_drag(window, node, source.item.clone(), source.native);
                    self.set_drag_modifiers(source.modifiers);
                }
                GesturePhase::Changed if !source.native => {
                    if let Some(p) = at(self) {
                        self.drag_moved(window, p);
                    }
                }
                GesturePhase::Changed => {}
                GesturePhase::Ended | GesturePhase::Cancelled if source.native => {
                    if self.drag.as_ref().is_some_and(|s| s.source == node) {
                        let screen = match source.at {
                            DragPoint::Screen(s) => s,
                            DragPoint::Window(p) => self
                                .window_frame(window)
                                .map_or(p, |f| Point { x: f.origin.x + p.x, y: f.origin.y + p.y }),
                        };
                        self.drag_finished(screen);
                    }
                }
                GesturePhase::Ended => {
                    if let Some(p) = at(self) {
                        self.drag_released(window, p);
                    }
                }
                GesturePhase::Cancelled => self.cancel_drag(),
            }
            return true;
        }
        let Some(drag) = event.downcast_ref::<nd_gesture::DragEvent>() else { return false };
        let item = self.windows.get(&window).and_then(|w| w.tree.as_ref()).and_then(|t| t.drag_item(node));
        let Some(item) = item.or_else(|| source.and(self.drag.as_ref().map(|s| s.item.clone()))) else {
            return false;
        };
        if drag.phase != GesturePhase::Began && self.drag.as_ref().is_some_and(|s| s.source == node) {
            self.set_drag_modifiers(drag.modifiers);
        }
        match drag.phase {
            GesturePhase::Began => {
                self.begin_drag(window, node, item, false);
                self.set_drag_modifiers(drag.modifiers);
                if self.windows[&window].tree.as_ref().is_some_and(|t| t.drag_lifts(node)) {
                    self.lift(drag.start);
                }
                self.drag_moved(window, drag.position);
            }
            GesturePhase::Changed => {
                if self.drag.as_ref().is_some_and(|s| s.source == node) {
                    self.drag_moved(window, drag.position);
                }
            }
            GesturePhase::Ended => {
                if self.drag.as_ref().is_some_and(|s| s.source == node) {
                    self.drag_released(window, drag.position);
                }
            }
            GesturePhase::Cancelled => {
                if self.drag.as_ref().is_some_and(|s| s.source == node) {
                    self.cancel_drag();
                }
            }
        }
        // The node's own `.on_drag` still sees the gesture.
        false
    }
}
