//! The primitive views behind retained nodes: [`NdRootView`] (a window's content root),
//! [`NdLayerView`] (`div`) and [`NdTextView`] (`text`). All are flipped, so frames from `nd-ui`
//! (top-left origin) apply unchanged.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use nd_components::ContextMenuRequested;
use nd_core::Size;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibilityStaticTextRole, NSAttributedStringNSExtendedStringDrawing, NSBackgroundStyle, NSEvent,
    NSEventModifierFlags, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSResponder,
    NSStringDrawingOptions, NSTableRowView, NSView, NSViewLayerContentsRedrawPolicy,
};
use objc2_foundation::{MainThreadMarker, NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize, NSString};

use nd_gesture::PointerPhase;

use crate::gestures::GestureHost;
use crate::look::{Color, TextLook, ViewLook};

pub(crate) type LayoutHook = Rc<dyn Fn(Size)>;

/// How far a press may move, in points, before it is a drag and its `.active` pose lets go.
const PRESS_SLOP: f64 = 4.;

#[derive(Default)]
pub struct RootIvars {
    on_layout: RefCell<Option<LayoutHook>>,
    content: OnceCell<Retained<NdLayerView>>,
    /// Content covers the whole window, title bar included: a scaffold (whose split view
    /// insets its panes itself) or a custom title bar.
    full_size: Cell<bool>,
    /// Lies over a scaffold's split view: a click that lands on no nd view of its own falls
    /// through to the panes below.
    passes_through: Cell<bool>,
    /// Containers that pass clicks through with it (the scaffold's host view).
    pass_views: RefCell<Vec<Retained<NSView>>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond calling super in the overrides
    // below, and this type does not implement Drop.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RootIvars]
    pub struct NdRootView;

    impl NdRootView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            if self.ivars().passes_through.get() && hit.as_ref().is_some_and(|h| self.passes(h)) { None } else { hit }
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.setNeedsLayout(true);
        }

        // AppKit runs this in the display cycle, before drawing and inside the same Core
        // Animation transaction, including on every frame of a live resize. Laying out here is
        // what makes the presented frame the frame laid out.
        #[unsafe(method(layout))]
        fn layout(&self) {
            let _: () = unsafe { msg_send![super(self), layout] };
            let safe = self.safe_rect();
            let content = self.ivars().content.get().expect("content view");
            if content.frame() != safe {
                content.setFrame(safe);
            }
            let hook = self.ivars().on_layout.borrow().clone();
            if let Some(hook) = hook {
                hook(Size { width: safe.size.width as f32, height: safe.size.height as f32 });
            }
        }
    }
);

impl NdRootView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RootIvars::default());
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setWantsLayer(true);
        let content = NdLayerView::new(mtm, &ViewLook::default());
        this.addSubview(&content);
        let _ = this.ivars().content.set(content);
        this
    }

    /// The view the window's root node is: the content view minus the title bar band, so
    /// app content never sits under the OS title bar.
    pub(crate) fn content(&self) -> &NdLayerView {
        self.ivars().content.get().expect("content view")
    }

    /// Lets clicks that hit only the root, its content view or `via` reach the views below.
    /// `None` makes the root take its clicks again.
    pub(crate) fn set_passes_through(&self, via: Option<&NSView>) {
        self.ivars().passes_through.set(via.is_some());
        *self.ivars().pass_views.borrow_mut() = via.map(|v| v.retain()).into_iter().collect();
    }

    /// `view` is the root, its content view, or a pass-through container.
    fn passes(&self, view: &NSView) -> bool {
        let ptr: *const NSView = view;
        let content = self.ivars().content.get().map(|c| Retained::as_ptr(c).cast::<NSView>());
        std::ptr::eq(ptr, (self as *const Self).cast())
            || content.is_some_and(|c| std::ptr::eq(ptr, c))
            || self.ivars().pass_views.borrow().iter().any(|v| std::ptr::eq(ptr, Retained::as_ptr(v)))
            || view.downcast_ref::<NdLayerView>().is_some_and(|l| l.ivars().passes.get())
    }

    pub(crate) fn set_full_size(&self, full: bool) {
        if self.ivars().full_size.replace(full) != full {
            self.setNeedsLayout(true);
        }
    }

    /// The window's `contentLayoutRect` in this (full-size, flipped) view.
    fn safe_rect(&self) -> NSRect {
        let bounds = self.bounds();
        if self.ivars().full_size.get() {
            return bounds;
        }
        let Some(window) = self.window() else { return bounds };
        let layout = window.contentLayoutRect();
        let top = (bounds.size.height - (layout.origin.y + layout.size.height)).max(0.);
        NSRect::new(NSPoint::new(0., top), NSSize::new(bounds.size.width, (bounds.size.height - top).max(0.)))
    }

    pub(crate) fn set_layout_hook(&self, hook: Option<LayoutHook>) {
        *self.ivars().on_layout.borrow_mut() = hook;
        self.setNeedsLayout(true);
    }
}

pub struct LayerIvars {
    props: RefCell<ViewLook>,
    /// Set once nd focuses the node; a plain `div` refuses first responder.
    focusable: Cell<bool>,
    /// Multiplies the corner radius: the theme's radius scale at this node.
    radius_scale: Cell<f32>,
    /// A press on the view's own area moves the window (a title bar).
    drag: Cell<bool>,
    /// A press on the view's own area falls through the root to the panes below (the
    /// scaffold's overlay pane, which covers the whole window).
    passes: Cell<bool>,
    /// Right clicks report a context menu request to the node.
    menu: RefCell<Option<(nd_ui::events::Emit, crate::events::Wake)>>,
    /// Painted when the style sets no background: the window background, under a subtree
    /// scope that flips the appearance, so text inside reads on it.
    surface: Cell<Option<Color>>,
    /// The gestures the node listens for.
    gestures: RefCell<Option<GestureHost>>,
    /// The last Force Touch stage seen, so a force click fires once per press.
    stage: Cell<isize>,
    /// Pointer enter and exit are reported here (`.on_hover`).
    hover_emit: RefCell<Option<(nd_ui::events::Emit, crate::events::Wake)>>,
    /// The pointer is over the view, which paints its hover look then.
    hovered: Cell<bool>,
    /// Presses are reported here (`.active`).
    press_emit: RefCell<Option<(nd_ui::events::Emit, crate::events::Wake)>>,
    /// Where a held press went down, in the window, until it comes up or turns into a drag.
    press_at: Cell<Option<NSPoint>>,
    /// The list around this view that took the press too and waits for its drags and release.
    list_press: RefCell<Option<Retained<NSView>>>,
    tracking: RefCell<Option<Retained<objc2_app_kit::NSTrackingArea>>>,
    /// Where a hero in flight lives in the tree while it is drawn on the window's frame view.
    lifted: RefCell<Option<Home>>,
    /// The border of a shape Core Animation cannot round (corners of different radii), drawn
    /// along the mask that cuts the layer to it.
    outline: RefCell<Option<Retained<objc2_quartz_core::CAShapeLayer>>>,
    /// Casts the shadow of a view whose layer clips (`overflow_hidden()` or a corner mask), which
    /// would cut its own shadow away: a layer under it in the superview's layer, holding its
    /// frame, transform and opacity.
    caster: RefCell<Option<Retained<objc2_quartz_core::CALayer>>>,
}

/// A lifted hero's place in the tree: its parent, and the sibling above it.
struct Home {
    parent: Retained<NSView>,
    above: Option<Retained<NSView>>,
}

impl Default for LayerIvars {
    fn default() -> Self {
        Self {
            props: RefCell::default(),
            focusable: Cell::new(false),
            radius_scale: Cell::new(1.),
            drag: Cell::new(false),
            passes: Cell::new(false),
            menu: RefCell::new(None),
            surface: Cell::new(None),
            gestures: RefCell::new(None),
            stage: Cell::new(0),
            hover_emit: RefCell::new(None),
            hovered: Cell::new(false),
            press_emit: RefCell::new(None),
            press_at: Cell::new(None),
            list_press: RefCell::default(),
            tracking: RefCell::new(None),
            lifted: RefCell::new(None),
            outline: RefCell::new(None),
            caster: RefCell::new(None),
        }
    }
}

define_class!(
    // SAFETY: as for NdRootView.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[ivars = LayerIvars]
    pub struct NdLayerView;

    impl NdLayerView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            self.ivars().focusable.get()
        }

        // AppKit hit-tests frames, never the layer transform that draws an offset or scale, so
        // the point is taken back through it first: a click lands on what is drawn under it.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            match self.untransformed(point) {
                Some(point) if !self.ivars().props.borrow().inert => unsafe { msg_send![super(self), hitTest: point] },
                _ => None,
            }
        }

        // Content under a transparent title bar must not move the window unless it is a drag
        // region: a click on a card there is a click.
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if event.modifierFlags().contains(NSEventModifierFlags::Control) && self.request_menu(event) {
                return;
            }
            if self.ivars().drag.get() && let Some(window) = self.window() {
                if event.clickCount() == 2 {
                    crate::titlebar::double_click(&window);
                } else {
                    window.performWindowDragWithEvent(event);
                }
                return;
            }
            self.press(Some(event.locationInWindow()));
            if crate::gestures::mouse(self, event, PointerPhase::Down) {
                self.press_in_list(event);
                return;
            }
            let _: () = unsafe { msg_send![super(self), mouseDown: event] };
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if let Some(at) = self.ivars().press_at.get() {
                let to = event.locationInWindow();
                if (to.x - at.x).hypot(to.y - at.y) > PRESS_SLOP {
                    self.press(None);
                }
            }
            if !crate::gestures::mouse(self, event, PointerPhase::Move) {
                let _: () = unsafe { msg_send![super(self), mouseDragged: event] };
            } else if let Some(list) = self.ivars().list_press.borrow().clone() {
                let _: () = unsafe { msg_send![&*list, mouseDragged: event] };
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.press(None);
            let list = self.ivars().list_press.take();
            if !crate::gestures::mouse(self, event, PointerPhase::Up) {
                let _: () = unsafe { msg_send![super(self), mouseUp: event] };
            } else if let Some(list) = list {
                let _: () = unsafe { msg_send![&*list, mouseUp: event] };
            }
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            if !crate::gestures::other_mouse_down(self, self.ivars().gestures.borrow().as_ref(), event) {
                let _: () = unsafe { msg_send![super(self), otherMouseDown: event] };
            }
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            let host = self.ivars().gestures.borrow().clone();
            if !crate::gestures::other_mouse_up(self, host.as_ref(), event) {
                let _: () = unsafe { msg_send![super(self), otherMouseUp: event] };
            }
        }

        #[unsafe(method(swipeWithEvent:))]
        fn swipe_with_event(&self, event: &NSEvent) {
            let host = self.ivars().gestures.borrow().clone();
            if !crate::gestures::swipe(host.as_ref(), event) {
                let _: () = unsafe { msg_send![super(self), swipeWithEvent: event] };
            }
        }

        #[unsafe(method(magnifyWithEvent:))]
        fn magnify_with_event(&self, event: &NSEvent) {
            if !crate::gestures::magnify(self, self.ivars().gestures.borrow().as_ref(), event) {
                let _: () = unsafe { msg_send![super(self), magnifyWithEvent: event] };
            }
        }

        #[unsafe(method(rotateWithEvent:))]
        fn rotate_with_event(&self, event: &NSEvent) {
            if !crate::gestures::rotate(self, self.ivars().gestures.borrow().as_ref(), event) {
                let _: () = unsafe { msg_send![super(self), rotateWithEvent: event] };
            }
        }

        #[unsafe(method(smartMagnifyWithEvent:))]
        fn smart_magnify_with_event(&self, event: &NSEvent) {
            if !crate::gestures::smart_magnify(self, self.ivars().gestures.borrow().as_ref(), event) {
                let _: () = unsafe { msg_send![super(self), smartMagnifyWithEvent: event] };
            }
        }

        #[unsafe(method(pressureChangeWithEvent:))]
        fn pressure_change_with_event(&self, event: &NSEvent) {
            let mut stage = self.ivars().stage.get();
            let handled = crate::gestures::pressure(self, self.ivars().gestures.borrow().as_ref(), event, &mut stage);
            self.ivars().stage.set(stage);
            if !handled {
                let _: () = unsafe { msg_send![super(self), pressureChangeWithEvent: event] };
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let host = self.ivars().gestures.borrow().clone();
            if !crate::gestures::scroll_wheel(host.as_ref(), event) {
                let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
            }
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            if !self.request_menu(event) {
                let _: () = unsafe { msg_send![super(self), rightMouseDown: event] };
            }
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            if let Some(old) = self.ivars().tracking.borrow_mut().take() {
                self.removeTrackingArea(&old);
            }
            if self.tracks_hover() {
                use objc2::AnyThread;
                use objc2_app_kit::{NSTrackingArea, NSTrackingAreaOptions};
                let options =
                    NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::ActiveInKeyWindow;
                // Not InVisibleRect: inside a scroll view's document, visibleRect can reach past
                // the view, and a card lit up while the pointer was over its neighbour.
                let area = unsafe {
                    NSTrackingArea::initWithRect_options_owner_userInfo(
                        NSTrackingArea::alloc(),
                        self.hover_rect(),
                        options,
                        Some(self),
                        None,
                    )
                };
                self.addTrackingArea(&area);
                *self.ivars().tracking.borrow_mut() = Some(area);
            }
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
        }

        #[unsafe(method(viewDidMoveToSuperview))]
        fn view_did_move_to_superview(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidMoveToSuperview] };
            self.place_caster();
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.set_hovered(true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.set_hovered(false);
        }

        // nd frames the view in its parent's space, which a lifted hero has left.
        #[unsafe(method(setFrame:))]
        fn set_frame(&self, frame: NSRect) {
            let frame = match (&*self.ivars().lifted.borrow(), unsafe { self.superview() }) {
                (Some(home), Some(host)) => host.convertRect_fromView(frame, Some(&home.parent)),
                _ => frame,
            };
            let _: () = unsafe { msg_send![super(self), setFrame: frame] };
            self.place_caster();
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.place_caster();
            if !self.ivars().props.borrow().transform.is_identity() {
                self.apply_transform();
            }
            // A `rounded_full()` radius is clamped to the size, so it follows the size.
            if self.ivars().props.borrow().corner_radii.iter().any(|r| *r > 0.) {
                self.setNeedsDisplay(true);
            }
        }

        // Runs under the view's effective appearance, so system colours resolve for it here and
        // again whenever the appearance changes.
        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            let Some(layer) = self.layer() else { return };
            // AppKit puts the backing layer's transform back to identity when it syncs the layer's
            // geometry in the display pass, as it does for a hero just lifted onto the frame view:
            // that frame would draw it where layout put it, not where its offset moves it.
            if !self.ivars().props.borrow().transform.is_identity() {
                self.apply_transform();
            }
            let props = self.current_look();
            let background = props.background.or(self.ivars().surface.get()).map(|c| c.ns_color().CGColor());
            layer.setBackgroundColor(background.as_deref());
            // `rounded_full()` asks for an unbounded radius; Core Animation draws a layer with one
            // past half its size, and everything in it, as nothing.
            let size = self.bounds().size;
            let scale = self.ivars().radius_scale.get();
            let radii = props
                .corner_radii
                .map(|r| f64::from(r * scale).min(size.width.min(size.height) / 2.).max(0.));
            let radius = radii.iter().copied().fold(0., f64::max);
            layer.setMasksToBounds(props.clips);
            let border = props.border.map(|(width, color)| (f64::from(width), color.ns_color().CGColor()));
            let masked = !radii.iter().all(|r| *r == 0. || *r == radius);
            let shadow = props.shadow.map(|shadow| {
                let spread = f64::from(shadow.spread);
                let path = objc2_core_graphics::CGMutablePath::new();
                let rect = NSRect::new(
                    NSPoint::new(-spread, -spread),
                    NSSize::new(size.width + 2. * spread, size.height + 2. * spread),
                );
                crate::media::chat::rounded(&path, rect, radii.map(|r| if r > 0. { r + spread } else { r }));
                (shadow, path)
            });
            if props.clips || masked {
                shade(&layer, None);
                self.cast(shadow, size);
            } else {
                shade(&layer, shadow.as_ref());
                self.cast(None, size);
            }
            // Core Animation rounds the corners it is told to by one radius; any other shape is a
            // mask, which cuts what is inside it too.
            if !masked {
                layer.setCornerRadius(radius);
                layer.setMaskedCorners(corner_mask(radii));
                // SAFETY: clearing the mask; no other layer uses it.
                unsafe { layer.setMask(None) };
                self.set_outline(None, &layer);
                match &border {
                    Some((width, color)) => {
                        layer.setBorderWidth(*width);
                        layer.setBorderColor(Some(color));
                    }
                    None => layer.setBorderWidth(0.),
                }
            } else {
                layer.setCornerRadius(0.);
                layer.setBorderWidth(0.);
                let bounds = NSRect::new(NSPoint::ZERO, size);
                let path = objc2_core_graphics::CGMutablePath::new();
                crate::media::chat::rounded(&path, bounds, radii);
                let mask = objc2_quartz_core::CAShapeLayer::new();
                mask.setFrame(bounds);
                mask.setPath(Some(&path));
                // SAFETY: the mask is a fresh layer used by no other layer.
                unsafe { layer.setMask(Some(&mask)) };
                self.set_outline(border.map(|(width, color)| (width, color, bounds, radii)), &layer);
            }
        }
    }
);

/// Casts `shadow` (its look and the shape it falls from) from `layer`, or none.
fn shade(
    layer: &objc2_quartz_core::CALayer,
    shadow: Option<&(crate::look::Shadow, objc2_core_foundation::CFRetained<objc2_core_graphics::CGMutablePath>)>,
) {
    let Some((shadow, path)) = shadow else {
        layer.setShadowOpacity(0.);
        layer.setShadowPath(None);
        return;
    };
    layer.setShadowColor(Some(&shadow.color.ns_color().CGColor()));
    layer.setShadowOpacity(1.);
    // A CSS blur radius is twice the standard deviation of its blur; Core Animation's radius is
    // about the deviation.
    layer.setShadowRadius(f64::from(shadow.blur) / 2.);
    layer.setShadowOffset(NSSize::new(f64::from(shadow.offset.0), f64::from(shadow.offset.1)));
    layer.setShadowPath(Some(path));
}

/// The corners with a radius, all four when none has one (so a later radius rounds them all).
fn corner_mask(radii: [f64; 4]) -> objc2_quartz_core::CACornerMask {
    use objc2_quartz_core::CACornerMask;
    // Each layer of a flipped view keeps its y axis pointing down, so min Y is the top.
    let corners = [
        CACornerMask::LayerMinXMinYCorner,
        CACornerMask::LayerMaxXMinYCorner,
        CACornerMask::LayerMaxXMaxYCorner,
        CACornerMask::LayerMinXMaxYCorner,
    ];
    let mask = radii.iter().zip(corners).filter(|(r, _)| **r > 0.).fold(CACornerMask::empty(), |m, (_, c)| m | c);
    if mask.is_empty() { corners.into_iter().fold(CACornerMask::empty(), |m, c| m | c) } else { mask }
}

impl NdLayerView {
    pub(crate) fn new(mtm: MainThreadMarker, props: &ViewLook) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(LayerIvars::default());
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setWantsLayer(true);
        this.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::OnSetNeedsDisplay);
        this.set_props(props);
        this
    }

    /// What the view paints, the scope surface included.
    pub(crate) fn look(&self) -> ViewLook {
        let mut look = self.current_look();
        look.background = look.background.or(self.ivars().surface.get());
        look
    }

    pub(crate) fn set_surface(&self, surface: Option<Color>) {
        if self.ivars().surface.replace(surface) != surface {
            self.setNeedsDisplay(true);
        }
    }

    pub(crate) fn set_focusable(&self) {
        self.ivars().focusable.set(true);
    }

    pub(crate) fn set_radius_scale(&self, scale: f32) {
        if self.ivars().radius_scale.replace(scale) != scale {
            self.setNeedsDisplay(true);
        }
    }

    pub(crate) fn set_drag_region(&self, drag: bool) {
        self.ivars().drag.set(drag);
    }

    pub(crate) fn set_passes_clicks(&self) {
        self.ivars().passes.set(true);
    }

    /// Right clicks (and control-clicks) on this view emit [`ContextMenuRequested`] through
    /// `emit`; `None` gives them back to AppKit.
    pub(crate) fn set_menu(&self, menu: Option<(nd_ui::events::Emit, crate::events::Wake)>) {
        *self.ivars().menu.borrow_mut() = menu;
    }

    /// The gestures this view's node listens for; `None` when it listens for none.
    pub(crate) fn set_gestures(&self, host: Option<GestureHost>) {
        *self.ivars().gestures.borrow_mut() = host;
    }

    pub(crate) fn gesture_host(&self) -> Option<GestureHost> {
        self.ivars().gestures.borrow().clone()
    }

    /// Asks the nearest view from this one outwards that has a menu for it. A right click
    /// reaches that view through the responder chain, but a control-click is a left press,
    /// which the gestures of the innermost view (a card's text) would take as a tap.
    fn request_menu(&self, event: &NSEvent) -> bool {
        raise_menu_from(Retained::into_super(self.retain()), event)
    }

    /// Opens this view's menu, if it has one. `row` is the table row the click came out of when
    /// the menu is around the list rather than the row's own.
    fn raise_menu(&self, event: &NSEvent, row: Option<(&objc2_app_kit::NSTableView, isize)>) -> bool {
        let menu = self.ivars().menu.borrow();
        let Some((emit, wake)) = menu.as_ref() else { return false };
        let at = self.convertPoint_fromView(event.locationInWindow(), None);
        if let Some((table, row)) = row {
            select_row_alone(table, row);
        }
        emit.emit(ContextMenuRequested { node: emit.node(), x: at.x as f32, y: at.y as f32 });
        wake.wake();
        true
    }

    pub(crate) fn set_props(&self, props: &ViewLook) {
        // AppKit skips a hidden view and its subviews in hit testing and the key view loop, and
        // moves first responder out of it.
        if self.isHidden() != props.hidden {
            self.setHidden(props.hidden);
        }
        let had_hover = self.tracks_hover();
        let moved = self.ivars().props.borrow().transform != props.transform;
        if self.ivars().props.borrow().inert != props.inert {
            objc2_app_kit::NSAccessibility::setAccessibilityHidden(&**self, props.inert);
        }
        *self.ivars().props.borrow_mut() = props.clone();
        self.setAlphaValue(self.current_look().opacity.into());
        if moved {
            self.lift(props.hero && !props.transform.is_identity());
            self.apply_transform();
        }
        self.place_caster();
        if had_hover != self.tracks_hover() {
            self.updateTrackingAreas();
        }
        self.setNeedsDisplay(true);
    }

    /// Reports pointer enter and exit through `emit` (`.on_hover`); `None` stops.
    pub(crate) fn set_hover_target(&self, target: Option<(nd_ui::events::Emit, crate::events::Wake)>) {
        let had = self.tracks_hover();
        *self.ivars().hover_emit.borrow_mut() = target;
        if had != self.tracks_hover() {
            self.updateTrackingAreas();
        }
    }

    /// Reports presses through `emit` (`.active`); `None` stops.
    pub(crate) fn set_press_target(&self, target: Option<(nd_ui::events::Emit, crate::events::Wake)>) {
        if target.is_none() {
            self.press(None);
        }
        *self.ivars().press_emit.borrow_mut() = target;
    }

    /// A press went down at `at` (in the window), or the one held ended.
    /// A press a pointer gesture took, on a view in a table's row or a grid's item: the list
    /// still selects, drags and activates on it. The list can track the mouse to the release
    /// inside its mouseDown, and then no mouseUp reaches this view, so the gesture is finished
    /// here with the event that tracking stopped on.
    fn press_in_list(&self, down: &NSEvent) {
        let mut cursor = unsafe { self.superview() };
        let list = loop {
            let Some(v) = cursor else { return };
            if v.downcast_ref::<objc2_app_kit::NSTableView>().is_some()
                || v.downcast_ref::<objc2_app_kit::NSCollectionView>().is_some()
            {
                break v;
            }
            cursor = unsafe { v.superview() };
        };
        let _: () = unsafe { msg_send![&*list, mouseDown: down] };
        let mtm = MainThreadMarker::from(self);
        let last = objc2_app_kit::NSApplication::sharedApplication(mtm).currentEvent();
        let phase = match last.as_deref().map(NSEvent::r#type) {
            // The list did not track: its drags and release come to this view, which hands
            // them on.
            Some(objc2_app_kit::NSEventType::LeftMouseDown) => {
                *self.ivars().list_press.borrow_mut() = Some(list);
                return;
            }
            Some(objc2_app_kit::NSEventType::LeftMouseUp) => PointerPhase::Up,
            _ => PointerPhase::Cancel,
        };
        self.press(None);
        crate::gestures::mouse(self, last.as_deref().unwrap_or(down), phase);
    }

    fn press(&self, at: Option<NSPoint>) {
        let ivars = self.ivars();
        let Some((emit, wake)) = ivars.press_emit.borrow().clone() else { return };
        if ivars.press_at.replace(at).is_some() == at.is_some() {
            return;
        }
        emit.emit(nd_ui::PressChanged { pressed: at.is_some() });
        wake.wake();
    }

    /// The part of the view on screen, where the pointer hovers it.
    fn hover_rect(&self) -> NSRect {
        let (b, v) = (self.bounds(), self.visibleRect());
        let x0 = b.origin.x.max(v.origin.x);
        let y0 = b.origin.y.max(v.origin.y);
        let x1 = (b.origin.x + b.size.width).min(v.origin.x + v.size.width);
        let y1 = (b.origin.y + b.size.height).min(v.origin.y + v.size.height);
        NSRect::new(NSPoint::new(x0, y0), NSSize::new((x1 - x0).max(0.), (y1 - y0).max(0.)))
    }

    /// Drops the hover look when the pointer is no longer over the view, for a moment AppKit sent
    /// no `mouseExited:` (a drag session suppresses them).
    pub(crate) fn settle_hover(&self) {
        if !self.ivars().hovered.get() {
            return;
        }
        let Some(window) = self.window() else { return };
        let at = self.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
        if !self.mouse_inRect(at, self.hover_rect()) {
            self.set_hovered(false);
        }
    }

    fn tracks_hover(&self) -> bool {
        self.ivars().props.borrow().hover.is_some() || self.ivars().hover_emit.borrow().is_some()
    }

    fn set_hovered(&self, hovered: bool) {
        if self.ivars().hovered.replace(hovered) == hovered {
            return;
        }
        if self.ivars().props.borrow().hover.is_some() {
            self.setAlphaValue(self.current_look().opacity.into());
            self.place_caster();
            self.setNeedsDisplay(true);
        }
        if let Some((emit, wake)) = self.ivars().hover_emit.borrow().as_ref() {
            emit.emit(nd_ui::HoverChanged { hovered });
            wake.wake();
        }
    }

    /// The look in effect: the hover look while the pointer is over the view.
    fn current_look(&self) -> ViewLook {
        let props = self.ivars().props.borrow();
        match (&props.hover, self.ivars().hovered.get()) {
            (Some(hover), true) => (**hover).clone(),
            _ => props.clone(),
        }
    }

    /// Takes a lifted hero off the frame view, for a node going away.
    pub(crate) fn land(&self) {
        if self.ivars().lifted.borrow_mut().take().is_some() {
            self.removeFromSuperview();
        }
    }

    /// Moves a hero in flight onto the window's frame view, and back into its place once it has
    /// landed. On the content view a Liquid Glass bar it lands in would still draw over it.
    fn lift(&self, up: bool) {
        if up == self.ivars().lifted.borrow().is_some() {
            return;
        }
        if up {
            let Some(home) = (unsafe { self.superview() }) else { return };
            let Some(content) = self.window().and_then(|w| w.contentView()) else { return };
            let Some(host) = (unsafe { content.superview() }) else { return };
            let siblings = home.subviews();
            let above = siblings
                .iter()
                .position(|v| std::ptr::eq(&*v, &**self as &NSView))
                .and_then(|i| siblings.iter().nth(i + 1));
            let frame = host.convertRect_fromView(self.frame(), Some(&home));
            self.removeFromSuperview();
            *self.ivars().lifted.borrow_mut() = Some(Home { parent: home, above });
            host.addSubview_positioned_relativeTo(self, objc2_app_kit::NSWindowOrderingMode::Above, None);
            let _: () = unsafe { msg_send![super(self), setFrame: frame] };
        } else {
            let Some(Home { parent: home, above }) = self.ivars().lifted.borrow_mut().take() else { return };
            let frame = unsafe { self.superview() }
                .map_or(self.frame(), |host| home.convertRect_fromView(self.frame(), Some(&host)));
            self.removeFromSuperview();
            match above.filter(|v| unsafe { v.superview() }.is_some_and(|p| p == home)) {
                Some(above) => home.addSubview_positioned_relativeTo(
                    self,
                    objc2_app_kit::NSWindowOrderingMode::Below,
                    Some(&above),
                ),
                None => home.addSubview(self),
            }
            let _: () = unsafe { msg_send![super(self), setFrame: frame] };
        }
    }

    /// Draws `border` (width, colour, bounds and radii) as a stroke inside the shape the mask
    /// cuts; `None` takes it away.
    fn set_outline(
        &self,
        border: Option<(f64, Retained<objc2_core_graphics::CGColor>, NSRect, [f64; 4])>,
        layer: &objc2_quartz_core::CALayer,
    ) {
        let Some((width, color, bounds, radii)) = border else {
            if let Some(outline) = self.ivars().outline.borrow_mut().take() {
                outline.removeFromSuperlayer();
            }
            return;
        };
        let outline =
            self.ivars().outline.borrow_mut().get_or_insert_with(objc2_quartz_core::CAShapeLayer::new).clone();
        // The stroke is centred on its path: inset by half the width, it lies inside the mask.
        let half = width / 2.;
        let inner = NSRect::new(
            NSPoint::new(half, half),
            NSSize::new((bounds.size.width - width).max(0.), (bounds.size.height - width).max(0.)),
        );
        let path = objc2_core_graphics::CGMutablePath::new();
        crate::media::chat::rounded(&path, inner, radii.map(|r| (r - half).max(0.)));
        outline.setFrame(bounds);
        outline.setPath(Some(&path));
        outline.setFillColor(None);
        outline.setStrokeColor(Some(&color));
        outline.setLineWidth(width);
        if outline.superlayer().is_none() {
            layer.addSublayer(&outline);
        }
    }

    /// The layer's transform: the offset, and the scale about the view's centre (the layer's
    /// anchor is its origin, as AppKit sets it for a layer-backed view).
    fn apply_transform(&self) {
        let Some(layer) = self.layer() else { return };
        let (s, tx, ty) = self.affine();
        layer.setAffineTransform(objc2_core_foundation::CGAffineTransform { a: s, b: 0., c: 0., d: s, tx, ty });
        self.place_caster();
    }

    /// Casts `shadow` from the caster under the view, of a view `size` big; `None` takes the
    /// caster away. The caster is cut out where the view is, so nothing of it shows through a
    /// translucent view, and a caster AppKit reorders above its view still only shades around it.
    fn cast(
        &self,
        shadow: Option<(crate::look::Shadow, objc2_core_foundation::CFRetained<objc2_core_graphics::CGMutablePath>)>,
        size: NSSize,
    ) {
        let Some(shadow) = shadow else {
            if let Some(caster) = self.ivars().caster.borrow_mut().take() {
                caster.removeFromSuperlayer();
            }
            return;
        };
        let caster = self.ivars().caster.borrow_mut().get_or_insert_with(objc2_quartz_core::CALayer::new).clone();
        shade(&caster, Some(&shadow));
        let look = shadow.0;
        let reach = f64::from(look.blur + look.spread.abs() + look.offset.0.abs().max(look.offset.1.abs())) * 2.;
        let cut = objc2_core_graphics::CGMutablePath::new();
        let around =
            NSRect::new(NSPoint::new(-reach, -reach), NSSize::new(size.width + 2. * reach, size.height + 2. * reach));
        // SAFETY: a null transform adds the rect as given.
        unsafe { objc2_core_graphics::CGMutablePath::add_rect(Some(&cut), std::ptr::null(), around) };
        crate::media::chat::rounded(&cut, NSRect::new(NSPoint::ZERO, size), self.shown_radii(size));
        let mask = objc2_quartz_core::CAShapeLayer::new();
        mask.setPath(Some(&cut));
        // SAFETY: a constant Core Animation exports.
        mask.setFillRule(unsafe { objc2_quartz_core::kCAFillRuleEvenOdd });
        // SAFETY: the mask is a fresh layer used by no other layer.
        unsafe { caster.setMask(Some(&mask)) };
        self.place_caster();
    }

    /// The corner radii the view is drawn with at `size`.
    fn shown_radii(&self, size: NSSize) -> [f64; 4] {
        let scale = self.ivars().radius_scale.get();
        self.current_look().corner_radii.map(|r| f64::from(r * scale).min(size.width.min(size.height) / 2.).max(0.))
    }

    /// Puts the caster under the view, on its frame, transform and opacity, in one step with
    /// animation off: Core Animation would otherwise ease every change behind the view.
    fn place_caster(&self) {
        let Some(caster) = self.ivars().caster.borrow().clone() else { return };
        let (Some(layer), Some(parent)) = (self.layer(), unsafe { self.superview() }.and_then(|s| s.layer())) else {
            caster.removeFromSuperlayer();
            return;
        };
        objc2_quartz_core::CATransaction::begin();
        objc2_quartz_core::CATransaction::setDisableActions(true);
        caster.setBounds(layer.bounds());
        caster.setAnchorPoint(layer.anchorPoint());
        caster.setPosition(layer.position());
        caster.setAffineTransform(layer.affineTransform());
        caster.setOpacity(layer.opacity());
        caster.setHidden(layer.isHidden());
        caster.setGeometryFlipped(layer.isGeometryFlipped());
        if caster.superlayer().is_none_or(|s| !std::ptr::eq(&*s, &*parent)) {
            caster.removeFromSuperlayer();
            parent.insertSublayer_below(&caster, Some(&layer));
        }
        objc2_quartz_core::CATransaction::commit();
    }

    /// The scale and translation of the layer's transform, in the superview's space.
    fn affine(&self) -> (f64, f64, f64) {
        let t = self.ivars().props.borrow().transform;
        let size = self.bounds().size;
        let s = f64::from(t.scale);
        // A lifted hero sits on the window's frame view, whose y axis points up.
        let down = if unsafe { self.superview() }.is_none_or(|p| p.isFlipped()) { 1. } else { -1. };
        (
            s,
            f64::from(t.offset.0) + size.width * (1. - s) / 2.,
            down * f64::from(t.offset.1) + size.height * (1. - s) / 2.,
        )
    }

    /// `point`, in the superview's space, where it falls on the view's frame before the layer
    /// transform moves and scales what the frame shows. `None` at a scale of zero, where
    /// nothing is drawn.
    fn untransformed(&self, point: NSPoint) -> Option<NSPoint> {
        if self.ivars().props.borrow().transform.is_identity() {
            return Some(point);
        }
        let (s, tx, ty) = self.affine();
        if s.abs() < f64::EPSILON {
            return None;
        }
        let o = self.frame().origin;
        Some(NSPoint::new(o.x + (point.x - o.x - tx) / s, o.y + (point.y - o.y - ty) / s))
    }
}

#[derive(Default)]
pub struct TextIvars {
    props: RefCell<Option<TextLook>>,
    /// The family the theme sets text in, in place of the system font.
    family: RefCell<Option<String>>,
    /// Where a click on a link run is reported, when the text has links.
    links: RefCell<Option<(nd_ui::events::Emit, crate::events::Wake)>>,
}

define_class!(
    // SAFETY: as for NdRootView.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TextIvars]
    pub struct NdTextView;

    impl NdTextView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let Some(props) = self.drawn_look() else { return };
            let text = attributed(&props, self.family().as_deref());
            match props.clamp {
                Some(clamp) => {
                    let Some(laid) = crate::rich::lay_out(&text, self.bounds().size.width, Some(clamp)) else {
                        return;
                    };
                    let glyphs = laid.layout.glyphRangeForTextContainer(&laid.container);
                    laid.layout.drawBackgroundForGlyphRange_atPoint(glyphs, NSPoint::ZERO);
                    laid.layout.drawGlyphsForGlyphRange_atPoint(glyphs, NSPoint::ZERO);
                }
                None => text.drawWithRect_options_context(self.bounds(), draw_options(), None),
            }
        }

        // The text wraps, cuts and clamps at the view's width: a new size draws it again.
        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            let old = self.frame().size;
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            if old != size {
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            // A control-click goes on to the views around, which raise their menu.
            let control = event.modifierFlags().contains(NSEventModifierFlags::Control);
            if control || !self.click_link(event) {
                let _: () = unsafe { msg_send![super(self), mouseDown: event] };
            }
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let props = self.ivars().props.borrow();
            let Some(props) = props.as_ref().filter(|p| p.has_links()) else { return };
            let text = attributed(props, self.family().as_deref());
            for rect in crate::rich::link_rects(&props.runs, &text, self.bounds().size.width) {
                self.addCursorRect_cursor(rect, &objc2_app_kit::NSCursor::pointingHandCursor());
            }
        }

        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> bool {
            true
        }

        #[unsafe(method_id(accessibilityRole))]
        fn accessibility_role(&self) -> Option<Retained<NSString>> {
            Some(unsafe { NSAccessibilityStaticTextRole }.retain())
        }

        #[unsafe(method_id(accessibilityValue))]
        fn accessibility_value(&self) -> Option<Retained<AnyObject>> {
            let props = self.ivars().props.borrow();
            props.as_ref().map(|p| Retained::into_super(Retained::into_super(NSString::from_str(&p.text))))
        }
    }
);

impl NdTextView {
    pub(crate) fn new(mtm: MainThreadMarker, props: &TextLook) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TextIvars::default());
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setWantsLayer(true);
        this.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::OnSetNeedsDisplay);
        this.set_props(props);
        this
    }

    pub(crate) fn look(&self) -> Option<TextLook> {
        self.ivars().props.borrow().clone()
    }

    /// The look as it is drawn: on a table row's emphasized selection the label tones give way
    /// to the selected text colour.
    pub(crate) fn drawn_look(&self) -> Option<TextLook> {
        let mut look = self.look()?;
        if on_emphasized_row(self) {
            look.color = look.color.on_selection();
            for run in &mut look.runs {
                run.color = run.color.map(crate::look::Color::on_selection);
            }
        }
        Some(look)
    }

    pub(crate) fn set_props(&self, props: &TextLook) {
        self.setAlphaValue(props.opacity.into());
        let links = props.has_links();
        *self.ivars().props.borrow_mut() = Some(props.clone());
        if links && let Some(window) = self.window() {
            window.invalidateCursorRectsForView(self);
        }
        self.setNeedsDisplay(true);
    }

    fn family(&self) -> Option<String> {
        self.ivars().family.borrow().clone()
    }

    /// Sets the text in `family` (a theme's font override); `None` is the system font.
    pub(crate) fn set_family(&self, family: Option<&str>) {
        let family = family.map(str::to_owned);
        if *self.ivars().family.borrow() != family {
            *self.ivars().family.borrow_mut() = family;
            self.setNeedsDisplay(true);
        }
    }

    /// Clicks on link runs report [`nd_ui::LinkClicked`] through `emit`.
    pub(crate) fn set_link_target(&self, target: Option<(nd_ui::events::Emit, crate::events::Wake)>) {
        *self.ivars().links.borrow_mut() = target;
    }

    fn click_link(&self, event: &NSEvent) -> bool {
        let Some(props) = self.ivars().props.borrow().clone().filter(TextLook::has_links) else { return false };
        let at = self.convertPoint_fromView(event.locationInWindow(), None);
        let text = attributed(&props, self.family().as_deref());
        let Some(link) = crate::rich::link_at(&text, self.bounds().size.width, at) else {
            return false;
        };
        if let Some((emit, wake)) = self.ivars().links.borrow().as_ref() {
            emit.emit(nd_ui::LinkClicked { link: link.clone().into() });
            wake.wake();
        }
        if props.opens_links {
            crate::rich::open(&link);
        }
        true
    }

    /// Measures in `family` when the caller knows the theme's (`Some`), else in the view's own.
    pub(crate) fn measure_in(&self, width: Option<f32>, family: Option<Option<&str>>) -> Size {
        let Some(props) = self.ivars().props.borrow().clone() else { return Size::default() };
        match family {
            Some(family) => measure_text(&props, family, width),
            None => measure_text(&props, self.family().as_deref(), width),
        }
    }
}

/// Whether `view` sits in a table row that draws the emphasized selection behind it.
fn on_emphasized_row(view: &NSView) -> bool {
    let mut at = unsafe { view.superview() };
    while let Some(v) = at {
        if let Some(row) = v.downcast_ref::<NSTableRowView>() {
            return row.interiorBackgroundStyle() == NSBackgroundStyle::Emphasized;
        }
        at = unsafe { v.superview() };
    }
    false
}

/// Draws the texts under `view` again: their colours follow the row's selection.
pub(crate) fn redraw_texts(view: &NSView) {
    for sub in view.subviews().iter() {
        if sub.downcast_ref::<NdTextView>().is_some() {
            sub.setNeedsDisplay(true);
        } else {
            redraw_texts(&sub);
        }
    }
}

pub(crate) fn measure_text(props: &TextLook, family: Option<&str>, width: Option<f32>) -> Size {
    let text = attributed(props, family);
    let size = match props.clamp {
        // NSStringDrawing has no line limit: TextKit lays the cut text out as it is drawn.
        Some(clamp) => crate::rich::lay_out(&text, width.map_or(1e7, f64::from), Some(clamp))
            .map_or(NSSize::ZERO, |laid| laid.layout.usedRectForTextContainer(&laid.container).size),
        None => {
            let bound = NSSize::new(width.map_or(f64::MAX, f64::from), f64::MAX);
            text.boundingRectWithSize_options_context(bound, draw_options(), None).size
        }
    };
    Size { width: size.width.ceil() as f32, height: size.height.ceil() as f32 }
}

fn draw_options() -> NSStringDrawingOptions {
    NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::UsesFontLeading
}

fn attributed(props: &TextLook, family: Option<&str>) -> Retained<NSAttributedString> {
    if !props.runs.is_empty() {
        return crate::rich::attributed(&props.runs, props.tabular_nums, &props.color.ns_color(), family);
    }
    let size = props.size.map_or_else(NSFont::systemFontSize, f64::from);
    let weight = props.weight.map_or(400., |w| w.0);
    let font = if let Some(font) = family.and_then(|f| crate::rich::family_font(f, size, weight, false)) {
        font
    } else if props.tabular_nums {
        NSFont::monospacedDigitSystemFontOfSize_weight(size, props.ns_weight())
    } else {
        NSFont::systemFontOfSize_weight(size, props.ns_weight())
    };
    let color = props.color.ns_color();
    let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
    let values: [&AnyObject; 2] = [font.as_ref(), color.as_ref()];
    let attrs = NSDictionary::from_slices(&keys, &values);
    unsafe { NSAttributedString::new_with_attributes(&NSString::from_str(&props.text), &attrs) }
}

pub(crate) fn ns_rect(rect: nd_core::Rect) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.origin.x.into(), rect.origin.y.into()),
        NSSize::new(rect.size.width.into(), rect.size.height.into()),
    )
}

/// Asks the nearest view from `view` outwards that has a menu for it.
pub(crate) fn raise_menu_from(view: Retained<NSView>, event: &NSEvent) -> bool {
    let mut row: Option<(Retained<objc2_app_kit::NSTableView>, isize)> = None;
    let mut below: Option<Retained<NSView>> = None;
    let mut cursor = Some(view);
    while let Some(v) = cursor {
        let passed = row.as_ref().map(|(table, row)| (&**table, *row));
        if v.downcast_ref::<NdLayerView>().is_some_and(|layer| layer.raise_menu(event, passed)) {
            return true;
        }
        if row.is_none()
            && let (Ok(table), Some(child)) = (v.clone().downcast::<objc2_app_kit::NSTableView>(), below.as_deref())
        {
            let at = table.rowForView(child);
            if at >= 0 {
                row = Some((table, at));
            }
        }
        // SAFETY: main thread, a view in a live window; the superview is retained before use.
        cursor = unsafe { v.superview() };
        below = Some(v);
    }
    false
}

/// A list's menu opened on one of its rows acts on the selection, so a row outside it is
/// selected alone first, as in Finder. NSTableView itself only outlines the clicked row. A row's
/// own menu acts on its row and leaves the selection alone, as a browser's tab strip does.
fn select_row_alone(table: &objc2_app_kit::NSTableView, row: isize) {
    if !table.isRowSelected(row) {
        let set = objc2_foundation::NSIndexSet::indexSetWithIndex(row as usize);
        table.selectRowIndexes_byExtendingSelection(&set, false);
    }
}
