//! What apps receive. Each type is delivered to the node's handler through the event queue, never
//! from inside a native callback.

use nd_core::Point;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GesturePhase {
    Began,
    Changed,
    Ended,
    /// The gesture stopped without completing: another recognizer won, the OS took the pointer,
    /// or a swipe fell short. Undo whatever the earlier phases previewed.
    Cancelled,
}

/// The keyboard modifiers held when a pointer event happened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    /// Command on macOS, the Windows or Super key elsewhere.
    pub platform: bool,
}

impl KeyModifiers {
    /// The platform's "add to the selection, open in a new tab" key: Command on macOS, Control
    /// elsewhere.
    pub fn secondary(&self) -> bool {
        if cfg!(target_os = "macos") { self.platform } else { self.control }
    }

    pub fn any(&self) -> bool {
        self.shift || self.control || self.alt || self.platform
    }
}

/// A primary-button press released in place. `count` is 2 for the second press of a double tap.
/// `modifiers` are the ones held at the press.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapEvent {
    pub position: Point,
    pub count: u32,
    pub modifiers: KeyModifiers,
}

impl TapEvent {
    pub fn new(position: Point, count: u32) -> Self {
        Self { position, count, modifiers: KeyModifiers::default() }
    }
}

/// The second press of a double tap, for `.on_double_tap`. The router reports it as a
/// [`TapEvent`] with `count: 2`; `nd-ui` delivers it as this type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DoubleTapEvent {
    pub position: Point,
    pub modifiers: KeyModifiers,
}

/// A press of a pointer button other than the primary and secondary ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerButton {
    Middle,
    /// The mouse's back button (button 4 on X11 and Wayland, `buttonNumber` 3 on macOS).
    Back,
    Forward,
}

/// A middle-button press released in place (`.on_middle_click`): open in a new tab, paste a
/// selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiddleClickEvent {
    pub position: Point,
    pub modifiers: KeyModifiers,
}

/// The user asked to go back or forward (`.on_navigate`): the mouse's back and forward buttons,
/// or on macOS a three-finger swipe (`swipeWithEvent:`). Delivered to the innermost node under
/// the pointer that listens, so a handler on a window's root hears every one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigateEvent {
    pub direction: SwipeDirection,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LongPressEvent {
    pub position: Point,
    pub phase: GesturePhase,
}

/// A pointer drag (`.on_drag`). `translation` is from the press point; `velocity` is in points
/// per second, set on `Ended` for a fling. `modifiers` are the ones held at this event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragEvent {
    pub phase: GesturePhase,
    pub start: Point,
    pub position: Point,
    pub translation: Point,
    pub velocity: Point,
    pub modifiers: KeyModifiers,
}

/// A trackpad pinch. `scale` multiplies: `zoom *= event.scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinchEvent {
    pub position: Point,
    pub scale: f32,
    pub phase: GesturePhase,
}

/// A trackpad rotation, macOS only. `degrees` is this event's change, counterclockwise positive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotateEvent {
    pub position: Point,
    pub degrees: f32,
    pub phase: GesturePhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwipeDirection {
    /// Fingers moved right (left in right-to-left layouts is the backend's to flip).
    Back,
    Forward,
}

/// A two-finger horizontal page swipe (`.on_swipe_page`). `progress` runs 0 to 1 while the
/// fingers track; `Ended` means the swipe committed, `Cancelled` that it fell back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwipePageEvent {
    pub direction: SwipeDirection,
    pub progress: f32,
    pub phase: GesturePhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowEdge {
    Leading,
    Trailing,
}

/// A row's swipe-to-reveal state. `offset` is the row content's horizontal displacement
/// (positive reveals the leading edge). `open` is the edge left revealed once the row settles;
/// `full` means a full swipe committed the edge's first action.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowSwipeEvent {
    pub offset: f32,
    pub phase: GesturePhase,
    pub open: Option<RowEdge>,
    pub full: bool,
}

/// A deep press on a Force Touch trackpad, macOS only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForceClickEvent {
    pub position: Point,
}

/// Two-finger double tap, macOS only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmartMagnifyEvent {
    pub position: Point,
}

/// The wheel or two fingers scrolled with the secondary key held (Control, Command on macOS),
/// over a node with `.on_scroll_zoom`: the zoom a browser or file manager does there, and the
/// view under the pointer does not scroll. `steps` counts wheel notches, fractions of one from a
/// trackpad; positive where the same scroll would move the content towards its start, which
/// zooms in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollZoomEvent {
    pub position: Point,
    pub steps: f32,
    pub modifiers: KeyModifiers,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GestureEvent {
    Tap(TapEvent),
    LongPress(LongPressEvent),
    Drag(DragEvent),
    Pinch(PinchEvent),
    Rotate(RotateEvent),
    SwipePage(SwipePageEvent),
    RowSwipe(RowSwipeEvent),
    ForceClick(ForceClickEvent),
    SmartMagnify(SmartMagnifyEvent),
    ScrollZoom(ScrollZoomEvent),
    MiddleClick(MiddleClickEvent),
    Navigate(NavigateEvent),
}
