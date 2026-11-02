//! The divider of a resizable sidebar or inspector on GPUI: a grab area over the pane's inner
//! edge, laid over the scaffold's panes, that the user drags. The pane draws at the dragged
//! width until the app's own `width` changes, and every move reports [`PaneResized`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use nd_components::{PANE_MAX_WIDTH, PANE_MIN_WIDTH, PaneProps, PaneResized, PaneRole};
use nd_core::{NodeId, Rect};
use nd_gpui::gpui::{
    self, AnyElement, DispatchPhase, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Styled, canvas, div, px,
};
use nd_ui::events::Emit;

use crate::backend::Wake;
use crate::render::Build;

/// A width the user dragged a pane to, and the app's width it was dragged under.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PaneWidth {
    width: f32,
    prop: Option<f32>,
}

/// A divider drag in progress: the pane, where the press began and the pane's width then.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PaneDrag {
    pane: NodeId,
    start: f32,
    width: f32,
}

#[derive(Default)]
pub(crate) struct PaneResizes {
    widths: HashMap<NodeId, PaneWidth>,
    drag: Option<PaneDrag>,
}

pub(crate) type PaneResizing = Rc<RefCell<PaneResizes>>;

impl PaneResizes {
    pub(crate) fn forget(&mut self, node: NodeId) {
        self.widths.remove(&node);
    }
}

/// Half the grab area: it reaches this far into the pane and as far over the content.
const GRAB: f32 = 4.;

/// The width range a drag keeps the pane in.
fn range(p: &PaneProps) -> (f32, f32) {
    let min = p.min_width.unwrap_or(PANE_MIN_WIDTH);
    (min, p.max_width.unwrap_or(PANE_MAX_WIDTH).max(min))
}

struct Grab {
    pane: NodeId,
    props: PaneProps,
    state: PaneResizing,
    bounds: Rc<RefCell<HashMap<NodeId, Rect>>>,
    emit: Emit,
    wake: Wake,
}

impl Grab {
    fn press(&self, x: f32) {
        let Some(b) = self.bounds.borrow().get(&self.pane).copied() else { return };
        self.state.borrow_mut().drag = Some(PaneDrag { pane: self.pane, start: x, width: b.size.width });
    }

    fn moved(&self, x: f32, window: &mut gpui::Window, cx: &mut gpui::App) {
        let mut state = self.state.borrow_mut();
        let Some(drag) = state.drag.filter(|d| d.pane == self.pane) else { return };
        let along = if self.props.role == PaneRole::Inspector { drag.start - x } else { x - drag.start };
        let (min, max) = range(&self.props);
        let width = (drag.width + along).clamp(min, max).round();
        let last = state.widths.get(&self.pane).filter(|w| w.prop == self.props.width).map(|w| w.width);
        if last == Some(width) {
            return;
        }
        state.widths.insert(self.pane, PaneWidth { width, prop: self.props.width });
        drop(state);
        self.emit.emit(PaneResized { width });
        (self.wake)(cx);
        window.refresh();
    }
}

impl Build<'_> {
    /// The width a drag gave pane `id`, while the app's width is the one it was dragged under.
    pub(crate) fn dragged_width(&self, id: NodeId, p: &PaneProps) -> Option<f32> {
        if !p.resizable {
            return None;
        }
        self.backend.pane_resizes.borrow().widths.get(&id).filter(|w| w.prop == p.width).map(|w| w.width)
    }

    /// The grab areas over the inner edges of the resizable panes among `panes` that show, in
    /// the coordinates of the row holding them, which starts at `scaffold`'s leading edge.
    pub(crate) fn pane_grabs(&self, scaffold: NodeId, panes: &[NodeId]) -> Vec<AnyElement> {
        let bounds = self.backend.bounds.borrow();
        let Some(row) = bounds.get(&scaffold).copied() else { return Vec::new() };
        let mut grabs = Vec::new();
        for pane in panes {
            let Some(node) = self.nodes.get(pane) else { continue };
            let Some(p) = node.props.downcast_ref::<PaneProps>() else { continue };
            let side = matches!(p.role, PaneRole::Sidebar | PaneRole::Inspector);
            let Some(b) = bounds.get(pane).copied().filter(|b| b.size.width > 0.) else { continue };
            if !p.resizable || !side || p.collapsed {
                continue;
            }
            let edge = if p.role == PaneRole::Sidebar { b.origin.x + b.size.width } else { b.origin.x };
            let grab = Rc::new(Grab {
                pane: *pane,
                props: p.clone(),
                state: self.backend.pane_resizes.clone(),
                bounds: self.backend.bounds.clone(),
                emit: self.backend.state.borrow().events.emitter(*pane),
                wake: self.wake.clone(),
            });
            let pressed = grab.clone();
            grabs.push(
                div()
                    .id(ElementId::Name(format!("nd-pane-divider-{}", pane.0).into()))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(edge - row.origin.x - GRAB))
                    .w(px(GRAB * 2.))
                    .occlude()
                    .cursor_col_resize()
                    .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        pressed.press(event.position.x.as_f32());
                    })
                    .into_any_element(),
            );
            // Window-wide while dragging, so the edge follows the pointer past the window.
            grabs.push(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let moved = grab.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Bubble && event.dragging() {
                                moved.moved(event.position.x.as_f32(), window, cx);
                            }
                        });
                        let released = grab.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, _| {
                            if phase == DispatchPhase::Bubble {
                                let mut state = released.state.borrow_mut();
                                if state.drag.is_some_and(|d| d.pane == released.pane) {
                                    state.drag = None;
                                }
                            }
                        });
                    },
                )
                .absolute()
                .size_0()
                .into_any_element(),
            );
        }
        grabs
    }
}
