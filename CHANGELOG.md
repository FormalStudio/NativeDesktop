# Changelog

Releases are git tags on this repository. `nd upgrade` prints the entries between an app's tag
and the one it moves to.

## Unreleased

- Container transforms. A hero now carries its source's look to its own as it flies: corner
  radius, border width and colour and fill interpolate with its frame on the platform spring, so
  a card's ring thins away as it grows into a page instead of staying as an outline. Elements
  inside a hero can follow its flight with `.hero_part(HeroPart::Source | Destination)`: the
  source's content keeps the source's size and fades out over the first 35% of the flight, the
  destination's keeps its final size and fades in from 20% to 60%, so the container is never
  empty, and closing (or turning around mid-flight) plays it back. In `presentation`,
  `.hero_content()` marks a card's content inside the container and `.reveal_with(&state)`
  inside a container is its destination; beside a shared element it fades as before. A hero
  holding parts goes home at once on close instead of waiting for its surroundings to fade.
  Reduce motion crossfades the container over its base without travel.
- Reduce motion dissolves instead of jumping, as Apple's guidance asks. A move that would show
  (more than 12 points of offset, size or layout, or a tenth of a scale) fades out where it was
  and back in where it lands; a smaller one (a hover lift, a press dip) takes its place at once;
  an entrance or exit whose pose travels fades from or to nothing; a shared element fades in
  where it lands and out where it is, so a `presentation` crossfades over its base. Paced
  animations and springs run on the platform's fast duration on an ease-out. `tile_grid` tiles
  changing slots dissolve and the grid's height jumps. AppKit panels and the GPUI backend's
  popovers, menus and panels keep their fade (GPUI drops the rise). The GPUI backend now tells
  `glides` under reduce motion too, so a dissolve starts where the element was.
- `ND_REDUCE_MOTION=1` or `0` stands in for the system's reduce motion setting on every
  backend, to watch either mode without changing it.
- A motion started by a key press or drag step after the window sat idle starts now. AppKit
  ran those frames on the last frame's clock, and kept the display link's last tick after
  pausing, so a fade begun from idle was already over on the next tick and looked instant.
- `on_animation_end` waits for the node to be at rest after its frame renders: a value that
  settles in the frame a render starts it again (the next leg of a crossfade) no longer reports
  an end.
- A shared element with no source on screen rises into place as it fades in, and fades out
  where it is going back.
- `ND_ANIMATION_SLOWDOWN` stretches explicit animations and springs too, not only paced ones.
- Menu roles `CloseWindow` (Close, Cmd-W, in File), `Find` (the standard Find submenu, wired to
  the text view's find bar) and `Bold` and `Italic` (Cmd-B and Cmd-I through the focused text
  area). The AppKit Edit menu includes Find, and text areas use the find bar.
- The settings window has a fixed size on macOS, as the system's do.
- `AnimationEnded` is in the prelude.
- `Icon::FolderPlus`.
- `markdown_editor` previews live, as Obsidian and Bear do: outside the paragraphs the selection
  touches (while the area has focus), `#`, emphasis delimiters, backticks and a link's brackets
  and target take no room, a bullet shows as a dot, `- [ ]` and `- [x]` as a box a click ticks
  or clears (one undo step, the selection kept), `>` as a bar, `---` as a hairline and a fenced
  block as a rounded monospace well. The text stays the Markdown, so the caret, selection, copy
  and undo work on the source, and a keystroke restyles one paragraph. Spans carry the new
  `SpanStyle::conceal` and paragraphs `ParagraphStyle::block`; `ConcealedText` maps offsets
  between the source and what shows, and `EditKey::Toggle` asks a styler for a box's edit.
  AppKit makes concealed glyphs zero-advance control glyphs in its layout manager's delegate and
  draws the marks and blocks behind the text; GPUI shapes the shown text and maps hit testing,
  carets and selections back. Markdown takes quiet.md's metrics: 16pt body on a 1.625 leading,
  headings from 1.625em down, and code at 0.8125em.
- `scaffold().content_min_width(points)` keeps the content at least that wide beside the
  sidebar: on macOS the window stops narrowing there while the sidebar shows, and opening the
  sidebar in a narrow window widens it.
- On AppKit a control-click on an element's content raises the menu of the nearest element
  around it that has one: the content's own press went to the gestures around it, so a
  control-click on a card's text opened the card instead of its menu. A control-click on a link
  in text no longer follows the link.
- A context menu leaves out a submenu with nothing in it, and the separators that leaves doubled
  or at an edge: AppKit drew its row with a chevron that opened nothing on hover, as a note's
  Move To did with no folder to move to. A menu left with nothing opens nothing, on every backend.
- On GPUI a window too narrow for its sidebar beside 360 points of content folds the sidebar
  away, as libadwaita's split views collapse at a breakpoint and AppKit's split view collapses
  its sidebar. The sidebar toggle then slides it in over the content, and a press on the
  content beside it closes it. The breakpoint is the window's width against the sidebar's
  `min_w`, so dragging the sidebar wider never folds it.
- A right click on a `source_tree` or `source_list` row on GPUI opens its menu without selecting
  the row: selecting a sidebar row navigates, and the sidebars of Files and Explorer open the menu
  in place, as Finder's does.
- A `source_action` with no `label` draws its icon on GPUI, as a flat button in the row's suffix,
  instead of its tooltip as text.
- `BoxShadow` is in the prelude, so an app can pass shadows of its own to `.shadow(vec![..])`
  instead of only the `shadow_sm()` to `shadow_2xl()` presets.
- `nd::test` exports `DivProps`, so an app's tier 1 test can read a div's look from the
  recording backend (`cx.backend().props(node)`), as it reads `ButtonProps` and `TextProps`.
- On AppKit a hero's first frame draws it on its source: AppKit put the view's layer transform
  back to identity when it synced the geometry of the hero lifted onto the frame view, so for one
  frame the hero showed at its own place, the card's size in the corner of the page it grows into.
  Any offset or scaled view takes its transform again when its layer updates.
- On AppKit `.on_resize` inside a scaffold pane or split pane reports the size the pane gives it
  from its first report: nd places nothing in a pane AppKit has not sized yet, where the first
  layout reported a width and height of 0 and started layout glides from that empty box.
- `.active(|p| p.scale(0.97))` gives a `div` a pose to take while a press on it is held, eased on
  the fast timing, and dropped when the button comes up or the press becomes a drag. Backends
  report presses as `PressChanged`, and `DivProps` gained `active`. AppKit casts `box_shadow`:
  the outer shadow with the widest blur, along the view's corners, from a layer under the view
  when the view clips. The gallery's cards cast a shadow and dip when pressed.
- A `source_tree` or `source_list` on AppKit raises its rows' menus: a right click or a
  control-click on a row asks for that row's `.context_menu`, leaving the selection as it is, as
  Finder's sidebar does, and a middle click reports `SourceMiddleClicked`. A context menu with no
  items opens nothing. The gallery's sidebar has row menus.
- AppKit draws each corner of a `div` at its own radius: corners of one radius (or none) round
  with the layer's masked corners, and corners of different radii cut a shape mask, with the
  border stroked along it. Only the top-left radius applied before.
- AppKit text takes `truncate()` and `line_clamp(n)`: a truncated line ends in an ellipsis
  (`text_ellipsis_start` and `text_ellipsis_middle` cut where they say), and clamped text keeps
  its first `n` lines, the last ending in an ellipsis with `text_ellipsis()`, as on GPUI. Text
  draws again when its size changes, so a label laid out narrow first no longer stays cut there.
- On AppKit a click lands on what is drawn under the pointer: an element moved by `.offset` or
  scaled by `.scale` (a press dip, a layout glide, a hero) takes clicks where it is drawn, not
  where layout put it. A `tile_grid` places its tiles at their slots in layout and glides them
  with `animate_layout`, so a click on the first card no longer opens the last. An element that
  gains `animate_layout` after mounting glides from its next move, not the layout that sees it.
  The gallery has a Tile grid page.
- On the GPUI backend `animate_layout` starts its glide in the frame that moves the element, from
  where it was (it showed at its new place for one frame first), and `exit_to` and
  `animate_layout` work inside a scroll view, where the element vanished or jumped.
  `Backend::layout_frame(node)` lets a backend that lays out itself report a node's frame within
  its parent, and `DivProps` gained `glides`.
- On the GPUI backend an element leaving with `exit_to` takes no pointer input: nothing in it is
  hit, hovered or shows a cursor or tooltip while it animates out.
- `text_area(..).borderless()` (and `markdown_editor(..).borderless()`) drops the field chrome
  for an area that is the page, a document editor filling its window: no bezel, outline, focus
  ring or fill, with the text where a bordered area puts it. On AppKit the scroll, clip and text
  views draw no background; the drawn area keeps a clear border of the look's width.
  `TextAreaProps` gained `borderless`.
- `Div::exit_to(|p| ..)`: an element that drops out of its parent's output stays drawn where it
  stood, out of the layout flow, inert and out of the accessibility tree, while it animates to the
  pose, then goes. The same key back before then turns it around from where it is. Reduce motion
  keeps only the fade. `DivProps` gained `inert`.
- `Div::animate_layout(animation)`: an element that layout moves within its parent glides from
  its old place, on top of its own offset, and turns mid-flight from where it is. Not on its first
  layout; reduce motion jumps.
- `Animation::spring_physics(stiffness, damping)` builds a spring from an integrator's k and c.
- `tile_grid` animates reflows, inserts and removals on `.reflow(animation)`, the platform spring
  by default, and no longer jumps when its width changes. `TILE_SLIDE` is gone.

- `.scale(..)` draws on the GPUI backend: the element and everything in it (fills, borders,
  shadows, text, images, paths and clips) scales about its centre, and its hit area follows what
  is drawn, so press dips and hero transitions shrink and grow as on AppKit. GPUI gained
  `Window::with_element_scale` in nd's fork queue. Text and icons are sampled from their unscaled
  raster while scaled.
- A drawn `source_tree` or `source_list` animates: a folder's rows open as a drawer over the
  theme's fast to normal timing (150 to 250 ms on the macOS look, longer the more rows it shows,
  on a cubic ease-out), each row fading in and rising 4 pt as the edge passes it, while the rows
  below slide down and the disclosure turns; closing plays it back at 0.8 of the time before the
  rows go. The selection slides from row to row as one pill on a stiff spring, and the hover fill
  fades in fast and out slower. With reduce motion the rows only fade and the pill jumps. The
  disclosure is now a drawn triangle, and Fluent's chevron turns instead of swapping (except
  Segoe Fluent Icons on Windows, which swaps at halfway).
- `text_area(..).styles(styler)` styles the text as the user types: the styler gets one paragraph
  and the state the paragraph before handed on, and returns runs (`SpanStyle` with size, weight,
  italic, monospace, colour token, the new `background: Option<TextFill>`, underline,
  strikethrough) and a `ParagraphStyle` (line height, spacing before and after, first-line and
  head indents, in ems). An edit restyles the paragraphs it touched and goes on only while that
  state changes, so a keystroke in a 1M-character document styles one paragraph. AppKit sets the
  attributes on the `NSTextStorage` outside the undo manager; the drawn editor shapes each
  paragraph on its own and only near the viewport. A styler's `edit` may take Return, Tab,
  Shift-Tab and Command-B/I. `.max_measure(points)` keeps the text in a centred column.
- `markdown_editor(id, &state)` (and `Markdown` for `.styles`): live-preview markdown with the
  syntax dimmed in place, headings H1 to H3 scaled, emphasis, strikethrough, code spans and
  fences on a code well, quotes, lists and tasks (a ticked one dimmed and struck), links and
  rules. Return continues a list, task or quote, lifts an empty nested item and clears an empty
  top-level one, Tab and Shift-Tab nest list items, Command-B and Command-I wrap the selection.
- A styled text area keeps Tab and Shift-Tab (`KeyRoles::takes_tab`); Control-Tab moves focus out
  of it. Every text area now keeps Return, which used to press the window's default button.
- The drawn text area takes the app's `.text_size()`, and has undo and redo (Command-Z,
  Shift-Command-Z, Control-Y off macOS), typing within 700 ms folded into one step. AppKit's
  text area takes `.text_size()` too. `TextFieldState::with_text` starts a state with text.
- `WindowKind::Popup` opens a flyout for a tray or menu-bar icon: above other windows, without a
  title row, window controls or taskbar entry, at a fixed size, placed with `origin`. On Windows
  it is a rounded Acrylic tool window; on macOS a floating window with its buttons hidden.
- `PlayerBar::without_song()` leaves the artwork and titles out and keeps the seek bar, the
  transport and the item's controls, in every layout: a bar kept over a now-playing view that
  shows the song itself. The pill no longer loses its seek line that way.
- Drawn menus on the GPUI backend (context menus, `menu_button`) show each row's key binding at
  its trailing edge, read from the app's bindings: "Ctrl+Shift+T", with Super for the platform
  key on Linux. Only Fluent showed them before.
- A drawn `form_row`'s help wraps to the room its control leaves, so a long help line under a
  settings row no longer pushes the control past the window's edge.
- `List::drag_out_files(|key| Option<PathBuf>)`: rows dragged by `drag_rows` out of the app's
  windows carry their files to the app they land in (a browser, a chat app) as a `text/uri-list`,
  on the GPUI backend under Wayland (`Feature::DragFilesOut`). Inside the app the drag is the same
  `drag_rows` drag. `ListProps` gained `drag_files`.
- `List::drag_rows` takes `|key| Option<SharedString>`, and a row whose id is `None` does not
  drag. A press on a row that drags selects it on release, so `on_select` never fires for the
  start of a drag, and an unselected row drags alone without being selected. AppKit tables drag
  their rows in AppKit's dragging session to nd's targets, as source lists already did.
- `.on_scroll_zoom(|e: &ScrollZoomEvent, window, cx| ..)`: a wheel or two-finger scroll with the
  secondary key held (Command on macOS, Control elsewhere) over the element zooms it instead of
  scrolling anything. `e.steps` is in wheel notches, positive to zoom in, and `e.modifiers` the
  keys held. `ScrollInput` gained `modifiers` and `GestureSet` gained `scroll_zoom`.
- A scaffold's inspector on the libadwaita look gets no header bar of its own when the window
  draws no controls over it (a tiling compositor): it runs to the top instead of under an empty
  band.
- `WindowOptions::child_of(ForeignParent)` opens a window over a window of another app, and
  `.modal()` blocks that window's input while it is open: what a file chooser portal backend
  needs to sit on the app that asked. `ForeignParent::from_portal` reads the portal's
  `parent_window` (`wayland:<handle>` or `x11:<xid>`). On Linux the GPUI backend imports the
  handle over xdg-foreign v2 and marks the window modal over xdg-dialog v1 (WM_TRANSIENT_FOR and
  `_NET_WM_STATE_MODAL` on X11); other platforms open it on its own.
- `ndshell::hyprland::Options::xwayland` starts the headless Hyprland with XWayland from its
  config, for a tier 3 test with an X11 client: `Hyprland::x11_display()` names the display and
  `client_env()` carries it as `DISPLAY`.
- `Window::focus_if_lost(id)` focuses `id` after the frame only when nothing mounted holds focus,
  for a view that swaps one list for another without taking the keys out of a text field.
- A scaffold's sidebar on the libadwaita look gets no header bar of its own when nothing sits
  over it and the window draws no controls there (a tiling compositor): it runs to the top
  instead of under an empty band.
- `toolbar().flexible_item(..)` takes the room the header leaves and gives it back first when the
  window narrows, so the items after it stay clear of the inspector on the GPUI backend.
  `ToolbarItemProps` gained `flexible`.
- A focused list, table or grid on the GPUI backend marks its selection with `theme.selection`
  and draws no focus ring round a selected row or round the list itself; the ring shows only on
  a keyboard row the selection does not cover.
- `text.accent` keeps a 1.6:1 step of lightness from `text.primary`, still 4.5:1 on every surface:
  a pastel matugen accent on a dark scheme no longer reads as body text in links, mentions and
  search matches.
- A source list taller than its pane scrolls on the GPUI backend: rows and section headers keep
  their height instead of shrinking into each other.
- `window.scroll_into_view(cx, id)` and `App::scroll_into_view(window, id)` scroll every scroll
  view around an element, horizontal ones included, until it shows. Automation's
  `scrollIntoView` reaches scroll views on the GPUI backend and works on macOS through
  `scrollRectToVisible:`.
- A list, table, grid or tree with a selection is a focus target: `window.focus(id)` reaches it,
  a press keeps nd focus on it, Tab stops on it and bindings of the contexts around it match while
  it has focus. On macOS
  nd focus makes the table or collection view first responder.
- `toolbar().persistent_search(search_field(..))` keeps the field in the header on Linux
  instead of behind the search toggle and bar. `ToolbarItemProps` gained `persistent`.
- `VirtualInput::middle_click`, and `VirtualInput::hold(&[Modifier])` with `release()` for a
  Shift-click or a Control drag in tier 3. `ND_DIAG_WINDOW=1` prints each press as
  `nd: press <button> <modifiers>`.
- Rich text on the GPUI backend takes the text size of the box around it, or the body size, as
  `text` does, instead of 16 px.
- `sidebar().resizable()` and `inspector().resizable()` drag by their inner edge on the GPUI
  backend, between `min_w` and `max_w` (`PANE_MIN_WIDTH` and `PANE_MAX_WIDTH` where unset).
  `Pane::on_resize` reports `PaneResized { width }` on every backend (the split view's divider
  on macOS), and a changed `.width(..)` moves the divider. `PaneProps` gained `resizable`.
- `tab_strip(..).context_menu(|id, menu, window, cx| ..)` and `tab_view(..).context_menu(..)`:
  a menu per tab on the GPUI backend, the tab left unselected. A middle click on a closable
  document tab reports `TabCloseRequested`. `TabStripProps` gained `tab_menus`, and the drawn
  `tab_bar::Bar` gained `menus`.
- `source_tree(..).context_menu(|id, menu, window, cx| ..)` builds a menu per row, the row
  selected first, and `.on_middle_click` reports `SourceMiddleClicked { id }`, on the GPUI
  backend. `SourceTreeProps` gained `row_menus`.
- `source_item(..).caption(..)`, and a source row with a caption over a meter grows to hold both
  on the GPUI backend instead of running into the next row. `SourceItem` gained `caption`.
- Taps, double taps and drags carry the keyboard modifiers held (`TapEvent::modifiers`, a
  `KeyModifiers` with `secondary()`). `.on_middle_click` reports a middle click and
  `.on_navigate` the mouse's back and forward buttons (and the three-finger swipe on macOS).
  `TapEvent` and `DragEvent` gained a field: build a `TapEvent` with `TapEvent::new`.
- Lists, tables and grids on the GPUI backend select as AppKit does: Shift ranges from the
  anchor, Shift with Control adds the range, a click on empty space clears, the arrow keys, Home,
  End and Page Up/Down move and extend, Control+A (Command+A on macOS) selects all, a marquee from empty space in a
  table or grid, and a right click selects the row under it (on macOS too).
  `List::on_selection_change` sees every change, an emptied selection included. `SelectEvent::key`
  is the row the click landed on. `RowsSelected` is now `{ rows, cursor }`
  (`RowsSelected::new(rows)` for the old shape).
- `segmented(..).icons([..])`: icon segments, each option's title its accessible name and
  tooltip (`NSSegmentedControl` images with segment tooltips on macOS). `SelectProps` gained
  `icons`.
- `source_node(..).meter(f)` and `source_item(..).meter(f)`: a capacity bar under the title on
  the GPUI backend, none on macOS, as long as the row's text column whatever the title, badge
  and actions (which share the title's line). `SourceNode` and `SourceItem` gained `meter`.
- Inline rename: `text_field(..).select_on_focus(range)`, `.on_end_editing` (`TextEditingEnded`
  on Return or focus loss) and `.on_cancel` (`TextCancelled` on Escape). A press inside a list
  row's own control no longer selects or activates the row. `TextFieldProps` gained
  `select_on_focus`.
- Drag and drop on the GPUI backend: `List::drag_rows(kind, id)` drags the selection with a
  count badge, `List::accepts_drops`, `.drop_on` and `.on_drop` (`RowDropped`) drop on a row,
  `source_list(..).drops_onto_rows()` and `tab_strip(..).accepts_drops(..)` name the item or tab
  in `DropSlot::on` (on macOS too), and files dragged in from outside reach any target taking
  `DragItem::FILES`, with `FileDrop` still getting the drops no target takes. `DragOver`,
  `Dropped` and `DragSource` carry the modifiers held. `DragItem` gained `ids`, `DropSlot` gained
  `on`, and `DragSource`, `DragOver`, `Dropped` and `SourceTreeProps` gained fields.
- The theme file watcher counts writes only. It read its own reads of FormalShell's
  `theme.json` as changes, so every app re-read the palette about 20 times a second while idle
  (9 CPU ticks a minute for an idle `hello`, now 0). It follows a symlinked file to its target
  and no longer polls.
- `set_items_versioned` and `set_items` on 100k rows: a reorder takes 3.6 ms instead of 31 ms,
  the same rows 0.8 ms instead of 15 ms and 1% of versions bumped 0.8 ms instead of 34 ms
  (release, g815's efficiency cores). The diff, the scroll anchor and per-row re-renders are
  unchanged.
- `tab_strip` with `TabStyle::Strip` on GPUI backends: the strip is the header's surface and the
  selected tab takes the content's surface and joins the content below it, with the bottom line
  under the other tabs only. A group window's own tab bar keeps libadwaita's look.

## v0.6.1

- Video plays in the GPUI backend: Media Foundation on Windows, GStreamer on Linux (loaded at run
  time). A file that can't play opens in the system player.
- An emoji picker for text fields: the system panel on Windows and macOS, and a built-in picker
  with search, categories, skin tones and recents on Linux.
- Scrolling: Wayland touchpad scrolls report their start and end (GPUI patch 0010), wheel input
  eases on Windows, and only macOS rubber-bands at the edges. A scroll goes to the innermost
  scrollable under the pointer.
- Motion follows each platform: Fluent durations and curves on Windows, hover and press fades,
  popover and panel entrances and exits, and a page transition between views. The OS setting
  that turns animations off is respected on Windows and GNOME.
- Fixes: taps and scrolls reached nothing under an empty overlay, circles drew no content on
  macOS, avatars and images swallowed clicks, double-clicks in scroll views never counted, and
  the web build panicked on `std::time` (now caught by a wasm clippy pass in `nd docs check`).
- The `nd docs` web check waits for the first frame and prints the page console when an embed
  stays blank.

## v0.6.0

The first release of the Rust rewrite. Tags up to v0.5.9 are the Zig framework, kept on branch
`zig`; nothing carries over from it.

- Apps are Rust crates on the `nd` crate. Windows draw through AppKit on macOS and through a
  patched GPUI elsewhere (Linux on Wayland and X11, Windows, and WebAssembly in a browser).
- Components: navigation, forms, media, tabs, panes, dialogs, virtualized lists, tables, grids and
  trees, Lucide and SF Symbols icons, the libghostty-vt terminal, web views (WKWebView, and CEF on
  macOS and Linux).
- Settings in TOML, theme tokens with live matugen reload, IPC, SQLite, plugins, and system APIs:
  dialogs, clipboard, notifications, keychain, tray, media keys and audio.
- Diagnostics: crash and hang bundles with the UI tree, `nd diagnose`, an automation socket and
  an MCP server.
- The `nd` CLI: `new`, `upgrade`, `check`, `test` (three test tiers), `build`, `serve`,
  `package` (macOS .app, .pkg and .dmg, Linux AppImage, Windows Setup.exe), `release` (signed
  delta updates from a self-hosted feed), `docs` and `doctor`.
- `nd new` depends on nd by git tag and points the GPUI patch block at the same tag, with nd's
  `Cargo.lock` as the starting lockfile. `nd upgrade` moves both to a newer tag.
- Known gaps: the Linux AppImage is untested on Linux, Setup.exe registers no file associations
  or URL schemes, and web views on Windows report a missing engine.
