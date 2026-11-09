+++
title = "Motion"
description = "Platform timings for every animation, exits and layout glides, hover groups that reveal actions, shared element transitions, presentations, and the lyrics depth of field."
+++

Every animation in nd goes through one set of platform timings. The theme's preset carries
`motion` (fast, normal and slow durations, the platform's curve and its spring), and the host
copies it into the app's `MotionTokens`. An element asks for a pace rather than a duration, and
gets the platform's timing for it:

| Pace | Use | macOS | GNOME | Windows |
| --- | --- | --- | --- | --- |
| `Animation::fast()` | hover, press, small state changes | 150 ms | 200 ms | 83 ms |
| `Animation::normal()` | something coming in or going out | 250 ms | 250 ms | 167 ms |
| `Animation::slow()` | larger moves | 400 ms | 400 ms | 250 ms |
| `Animation::natural()` | interruptible moves, the platform spring | response 0.35 | response 0.2 | response 0.3 |

The curve is `CAMediaTimingFunction`'s default on macOS, ease-out-cubic on GNOME and WinUI's
entrance curve on Windows. With reduce motion on nothing travels: see
[Reduce motion](#reduce-motion). `.scale(..)` scales an element about
its centre on both backends. An element's offset and scale move its hit area with it, so a click
lands on what is drawn under the pointer. Hover follows the layout box on AppKit.

```rust
div()
    .opacity(if shown { 1. } else { 0. })
    .animate(Animation::normal())
```

`Animation::timed(..)` and `Animation::spring(..)` still take explicit values, for motion that
has a reason to differ from the platform. `Animation::spring(response, damping)` takes Apple's
terms; `Animation::spring_physics(stiffness, damping)` takes a hand-written integrator's k and c
with a mass of 1, so a spec written as "k 420, c 31" carries over as it stands.
`ND_ANIMATION_SLOWDOWN=8` stretches every animation eight times, paced or given explicitly, to
watch one frame by frame. `ND_REDUCE_MOTION=1` (or `0`) stands in for the system's reduce motion setting, so either
mode can be watched without changing it.

## Reduce motion

With the user's reduce motion setting on, nd follows Apple's guidance: movement is replaced by
dissolves, never by jumps. The rule lives in the motion layer, so every element, component and
backend gets it alike:

- A paced animation or a spring becomes the platform's fast duration (150 ms on macOS) on an
  ease-out. Fades keep their timing; only travel goes.
- A move that would show, more than 12 points (`DISSOLVE_DISTANCE`) or a tenth of a scale,
  dissolves: the element fades out where it was over half the fade, switches to its new offset,
  scale, size or layout place while it shows nothing, and fades in there over the whole fade.
  A further move while it is still fading out keeps it at the old place and joins the same
  dissolve.
- A smaller move (a hover lift, a press dip, a few points of reflow while a window resizes)
  takes its new place at once.
- An entrance or exit whose pose travels only fades, from or to the pose's opacity, or from
  or to nothing when the pose has none.
- A shared element fades in where it lands and fades out where it is when it goes back, so a
  presentation crossfades over its base.

Size an element by what it draws: a container that only holds its children (a grid's height,
say) should not animate its size under reduce motion, or the dissolve takes every child with
it. `cx.reduce_motion()` says whether the setting is on.

## Entering and leaving

`.enter_from(|p| ..)` gives a freshly mounted element a pose to come in from, and
`.exit_to(|p| ..)` one to leave to. When the element drops out of its parent's output, the tree
keeps its node drawn where it stood while it animates to the exit pose, then drops it. In the
meantime it takes no pointer input, is out of the accessibility tree and out of the layout flow,
so its siblings close the gap at once. Rendered again with the same key before it is gone, it
turns back from where it is, with the velocity it had.

```rust
div()
    .id(note.id.clone())
    .enter_from(|p| p.opacity(0.).offset(px(0.), px(-6.)))
    .exit_to(|p| p.opacity(0.).scale(0.96))
```

Both run on `.animate`'s animation, a short ease-out without one. The pose's values are the
element's own: an offset in the pose replaces the element's offset rather than adding to it.
With reduce motion on, an entrance or exit only fades.

## Layout animation

`.animate_layout(animation)` makes an element glide when layout moves it within its parent: a
sibling inserted or removed before it, a grid reflowing for a new width. The tree compares each
frame with the last one and runs the element's offset from the old place to its own, so the
glide adds to any offset the element sets itself, and a new move mid-flight turns it from where
it is. Sizes given in points animate as with `.animate`, which takes the same animation when it
is not set. Nothing glides on the first layout after mounting, and with reduce motion on, a
move that shows dissolves.

```rust
v_flex().children(notes.iter().map(|note| {
    div()
        .id(note.id.clone())
        .animate_layout(Animation::spring_physics(420., 31.))
        .exit_to(|p| p.opacity(0.))
        .child(label(note.title.clone()))
}))
```

`tile_grid` uses all three: tiles move to new slots on `.reflow(animation)` (the platform spring
by default), new tiles fade and grow in, removed ones fade and shrink where they stood, and the
grid's height follows. The first width it learns places the tiles at once. Tiles sit at their
slots in layout and glide with `animate_layout`, so they hover where they are drawn too. Under
reduce motion tiles changing slots dissolve, new and removed ones only fade, and the grid's
height jumps.

## Hover groups

A card or a row is a hover group. While the pointer is anywhere over it, descendants that ask
for `.group_hover(name, ..)` take their hover style, easing in and out on `Animation::fast()`.
`.reveal_on_hover(name)` is the common case: hidden at rest, faded in on hover.

```rust
div()
    .group("card")
    .child(artwork)
    .child(div().reveal_on_hover("card").child(icon_button("card.more", Icon::Ellipsis)))
```

The group name is looked up among the group's own descendants, so every card can use the same
name. Revealed controls stay in the accessibility tree at rest, so a screen reader reaches them
without a pointer.

### Actions over artwork

`artwork_actions` draws Music's hover: the artwork darkens and Play and More fade in over its
bottom corners. Put it in a box with the artwork that clips to the artwork's corners. On a row,
`.compact()` puts the Play glyph alone in the middle of the small artwork, and
`hover_menu_button` adds More at the end of the row.

```rust
let art = div()
    .relative()
    .rounded_lg()
    .overflow_hidden()
    .child(image(cover).size(px(168.)))
    .child(
        artwork_actions("album.actions", "card")
            .on_play(move |_, cx| play(cx))
            .menu(move |menu, _, cx| album_menu(menu, cx)),
    );
v_flex().group("card").child(art).child(label(title))
```

## Press feedback

`.active(|p| p.scale(0.97))` gives an element a pose to take while a press on it is held: its
opacity, offset or scale, easing on `Animation::fast()` or the element's own `.animate(..)`. It
goes back when the button comes up, or as soon as the press moves 4 pt and becomes a drag, so a
draggable card does not stay dipped. The press still clicks: a tap handler on the same element
runs on release.

## Source lists

On the GPUI backend a `source_list` or `source_tree` animates its rows. A folder's
rows open as a drawer: its edge travels on a cubic ease-out over the fast to normal timing,
longer the more rows it shows (150 to 250 ms on macOS), each row fades in and rises 4 pt as the
edge passes it, the rows below slide down with it, and the disclosure turns. Closing plays the
same back at 0.8 of the time and removes the rows once the drawer has shut; a click mid-flight
reverses from where it is. The selection is one pill under the rows that slides to a newly
selected row on a stiff spring (about 80 ms), and the hover fill fades in fast and out slower.
With reduce motion nothing moves: the rows fade in place and the pill jumps.

## Shared element transitions

`.hero_from(id, away)` makes an element travel from the frame of the element keyed `id` when it
mounts, and back onto that frame while `away` is true. It runs on the platform spring, so a new
target mid-flight turns it around from where it is. Its size animates when given in points, so
give it a box of its final size and place it absolutely in that box, and nothing else moves:

```rust
div().size(px(480.)).child(
    div()
        .absolute()
        .top_0()
        .left_0()
        .size(px(480.))
        .hero_from("player.bar.art", closing)
        .child(cover.size_full()),
)
```

The element shows nothing on its first frame, while its own frame is laid out. Then it sits on
the source for the fast pace and travels. With reduce motion on, it fades in where it lands, and
going back it fades out where it is.

## Presentations

`presentation` shows a view over a region: an expanded player over the page, a detail over its
list. The view keeps a `Presentation` handle and opens or closes it. The base stays mounted, so
it keeps its scroll position, and is hidden while the presentation covers it.

The motion is built around a hero inside the content, `away` set from `is_closing()`. Opening,
the backdrop fades in on the platform spring while the hero travels out of its source, and the
rest of the content, marked `.reveal_with(&state)`, fades in once the hero has landed
(`Pace::Settled`). Closing runs backwards: the revealed content fades first, then the hero
travels home while the backdrop fades out with it (`Pace::Exit`). The hero is never half
transparent and nothing draws over it in flight. Closing mid-open turns everything around from
where it is. With reduce motion on, the backdrop, the hero and the revealed content fade
together, a crossfade from the base to the presentation and back.

```rust
// In the view: self.expanded = Presentation::new();
self.expanded.set_open(true);

presentation("now-playing", &self.expanded)
    .base(page)
    .backdrop(image(cover).size_full().blur(px(64.)))
    .content(self.now_playing.clone())
    .bg(cx.theme().surface.base)
```

`image(..).blur(radius)` decodes the picture at eight pixels per radius, blurs it with each edge
pixel standing in for what lies past it, and draws it a little past its box on every side, so the
wash keeps the picture's colours right up to the box's edges on every backend. Lay a scrim of a
surface token over it for text.

## Player layout

`player_layout` lays out an expanded player for the room it is given. From 760 points wide the
artwork and the details sit in a column on the left and the panel (lyrics, the queue) on the
right; narrower, a header row holds a 64 point artwork beside the details, above the panel. The
artwork gets its side in points from the layout, so it never leaves its column and can be a hero.

```rust
// In the view: self.layout = PlayerLayoutState::new();
player_layout("now-playing", &self.layout)
    .artwork(move |side| cover.size(px(side)))
    .details(v_flex().child(label(title)).child(label(artist)))
    .panel(lyrics)
```

## Lyrics depth of field

`lyrics_view` keeps the line being sung sharp and whole, and fades and blurs the others more the
further they are from it (`DEPTH_OPACITY`, `DEPTH_BLUR`). As the line advances, each row eases
to its new depth on the slow pace. While the user scrolls, every row is sharp and whole; resync
brings the depth back. The app only supplies the lines and the clock.

On macOS each row's layer carries a Core Image gaussian blur whose radius animates. GPUI has no
blur filter for text, so on Linux and Windows a blurred row is drawn as faint copies on a ring
of the blur's radius, which add up to the row's opacity with soft edges. Only rows on screen get
the copies.
