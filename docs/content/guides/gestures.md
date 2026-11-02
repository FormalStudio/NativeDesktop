+++
title = "Gestures"
description = "Tap, double tap, long press, and drag; trackpad pinch, rotate, and page swipes; momentum scrolling, haptics, and the focus rules that go with them."
+++

Gestures are handlers on elements, such as `.on_tap(..)` and `.on_pinch(..)`. Where the OS has
native recognition (macOS), the framework uses it. Elsewhere,
[`nd-gesture`](../../../crates/nd-gesture/src/lib.rs) does the recognition. Either way, your
handler gets the same event type, always on the UI thread and never from inside a native callback.

<!-- docs:example notes -->

## Pointer gestures

| Handler | Event | Notes |
| --- | --- | --- |
| `.on_tap` | `TapEvent { position, count, modifiers }` | A primary-button press released in place |
| `.on_double_tap` | `DoubleTapEvent { position, modifiers }` | When it's set, a single tap waits out the double-click interval |
| `.on_long_press` | `LongPressEvent { position, phase }` | Fires after 0.5 s without movement |
| `.on_drag` | `DragEvent` | Any direction. For one axis, use `.on_drag_axis(axis, ..)` |
| `.on_middle_click` | `MiddleClickEvent { position, modifiers }` | The middle button released in place |
| `.on_navigate` | `NavigateEvent { direction }` | The mouse's back and forward buttons, and the three-finger swipe on macOS |

A press is a tap until it moves more than 4 points. After that, a drag can claim it. Positions
are in window coordinates.

`modifiers` is a `KeyModifiers { shift, control, alt, platform }` with the keys held at the
press (a drag's at each event). `modifiers.secondary()` is Command on macOS and Control
elsewhere: the key that adds to a selection or opens a link in a new tab.

`.on_navigate` goes to the innermost element under the pointer that has one, so a handler on the
window's root hears every back and forward press in that window. `direction` is
`SwipeDirection::Back` or `Forward`, the same type as a page swipe's.

Phased events carry a `GesturePhase`: `Began`, `Changed`, `Ended`, or `Cancelled`. Treat
`Cancelled` as undo. It means that another recognizer won the press, the OS took the pointer, or
a swipe fell short, so remove whatever you previewed on `Began` and `Changed`. A drag's `Ended`
carries `velocity` in points per second, for a fling.

<!-- docs:snippet examples/scroll/src/gestures.rs#handlers -->

Wire one handler per gesture:

<!-- docs:snippet examples/scroll/src/gestures.rs#wiring -->

This `Pad` lives in the scroll example so that CI compiles it. The benchmark window doesn't open
it.

### Nested recognizers

When elements are nested, their handlers don't both fire for one press. The innermost recognizer
that claims the press wins, and ties go to the innermost one. An inner tap and an outer drag
never both fire for one press: the tap claims it on release, and the drag claims it once the
pointer passes the slop.

## Drag and drop

`.draggable(DragItem::new(kind, id))` makes an element a drag source, and
`.accepts_drops([kind])` makes any element a target. A target hears `DragOver` as the drag
enters, moves and leaves, and `Dropped` on the release. Both carry the position, the target's
`slot` and the `modifiers` held, which is how a target tells a copy from a move. The source
hears `DragEnded` with the outcome. `DragItem::many(kind, ids)` carries several ids, as a list
dragging its selection does.

On the GPUI backend, files dragged in from a file manager are a `DragItem` of kind
`DragItem::FILES` whose `ids` are the paths, so a target taking `DragItem::FILES` takes them with
the same hover and the same `Dropped`. A drop that no target takes still reaches `FileDrop` (see
[system](system.md)), which is the only place files dropped on macOS arrive for now.

Lists, source lists and tab strips take drops on their parts. A list's drop names the row under
the pointer (see [virtualization](virtualization.md)). `source_list(..).drops_onto_rows()` lands
a drag over the middle of an item on that item, with the item outlined and `slot.on` naming it;
above and below the middle it lands between items as before. `tab_strip(..).accepts_drops(..)`
names the tab under the pointer in `slot.on`, outlined on the GPUI backend.

Dragging files out of the app to a file manager isn't supported. GPUI's Wayland backend can
start an outbound file drag, but only from its own element drags, which nd's drag sessions don't
use; it needs a patch in the GPUI fork.

## Trackpad gestures

`.on_pinch` delivers `PinchEvent { scale, phase, position }`. `scale` is a multiplier, so the
handler is `zoom *= event.scale`. The notes editor zooms its text this way and plays a haptic when
the zoom crosses 100%:

<!-- docs:snippet examples/notes/src/editor.rs#pinch -->

- `.on_rotate` gives `RotateEvent { degrees }`, the change since the last event. Counterclockwise
  is positive.
- `.on_swipe_page` gives `SwipePageEvent { direction, progress, phase }` for a two-finger
  horizontal swipe between pages, `Back` or `Forward`. `progress` runs from 0 to 1 while the
  fingers track. `Ended` means that the swipe committed, and `Cancelled` means that it fell back.
  On macOS, it follows the user's **Swipe between pages** setting.
- `.on_force_click` is a deep press on a Force Touch trackpad. `.on_smart_magnify` is the
  two-finger double tap.

Where these events arrive:

| | macOS | Linux, Windows, web |
| --- | --- | --- |
| Pinch | `NSResponder` magnify events | Forwarded from GPUI where its platform reports one |
| Rotate, force click, smart magnify | Yes | Never delivered |
| Page swipe | `trackSwipeEvent` | Not delivered yet |

Windows DirectManipulation and Wayland swipe and hold gestures need patches to the GPUI fork
that haven't landed. The framework never promises finger counts that the compositor owns, such
as Hyprland's `gesture =` rules. Put the same action behind a button or a key as well.

## Scrolling

`div().overflow_y_scroll()` (and `overflow_x_scroll`, `overflow_scroll`) makes a scroll view.
On macOS, it's an `NSScrollView` with native momentum and elastic overscroll. On the GPUI
backends, the framework provides both:

- Momentum decays exponentially at Apple's rate of 0.998 per millisecond. The framework evaluates
  it in closed form, so the path is identical at 60 Hz and 240 Hz. Wayland has no kinetic
  scrolling and GPUI ignores `momentumPhase`, so the framework owns momentum.
- The rubber band uses `x * d * c / (d + c * x)` with `c = 0.55`, and a critically damped spring
  brings the content back.

Lists scroll the same way. For details, see [virtualization](virtualization.md).

### Who gets a scroll

A two-finger scroll locks to an axis after 8 points. After the lock, the innermost consumer of
that axis wins: a scroller with room, a row swipe, or a page swipe. So a row swipe inside a
vertical list never fights the list, and a horizontal scroller inside a vertical one keeps its
own axis.

A row of lists wider than the window, as a column browser lays them out, is an `h_flex` with
`overflow_x_scroll()` around fixed-width, `flex_shrink_0()` columns. A sideways wheel or swipe
scrolls the row and an upright one scrolls the list under the pointer.

### Scrolling into view

`window.scroll_into_view(cx, "column-3")` (or `cx.scroll_into_view(window_id, id)`) scrolls
every scroll view around the element given that id until it shows, the nearest edge first, and
a list to the row holding it. It returns false when no element has the id or nothing around it
scrolls. A list mounts only the rows near its viewport, so reach an unmounted row with the
list's `scroll_to` or `jump_to` first. Automation's `scrollIntoView` does the same. On macOS
it's `scrollRectToVisible:` on the element's view.

### Row actions

Swipe actions on list rows come from the list (`list(..).row_actions(|key, cx| ..)`) instead of
a gesture handler. macOS builds them from the table's delegate, and the API is shaped so that a
misuse doesn't compile. For the example, see
[virtualization](virtualization.md#selection-activation-and-row-actions).

## Haptics

`cx.haptic(Haptic::Alignment)` plays a trackpad haptic timed to the frame being drawn.

| Haptic | Use |
| --- | --- |
| `Alignment` | Something snapped, such as a guide, a detent, or a zoom reaching 100% |
| `LevelChange` | A discrete threshold, such as a full-swipe point or a stepper |
| `Generic` | Anything else |

macOS has no custom waveforms, so these values map onto the three `NSHapticFeedbackManager`
patterns. The call does nothing on other operating systems and on trackpads without Force Touch,
so you can call it without checking.

## Overlays and gestures

Gesture hit-testing starts inside the topmost overlay, or hits nothing. A tap on a popover never
reaches a handler underneath it. For details, see [overlays](overlays.md#input-never-leaks).

## Focus and keyboard

The framework applies these rules for you:

- Return activates the window's default button, which is the first one in tree order inside the
  innermost focus scope. Losing focus never activates anything. A multi-line editor keeps Return
  for itself with `.takes_return()`.
- Tab moves through focusable elements in tree order, or in `.tab_index(n)` order.
  `.tab_stop(false)` leaves an element out of the Tab order, and code can still focus it.
- A focus scope saves focus when it mounts and gives it back when it unmounts. A trapping scope
  keeps Tab inside it. Popovers, panels, and sheets are scopes.
- Escape closes the innermost overlay, unless something inside handled the key first.
- Focus rings show for keyboard focus and not for pointer focus.
- Declared accelerators run before an embedded page sees the key, so a web view can't swallow
  Cmd-T.

Text fields are `NSTextField` and `NSTextView` on macOS and GPUI's text input on other platforms,
so IME composition goes through each platform's own path.
