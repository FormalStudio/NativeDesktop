# Linux look against libadwaita

Gaps between nd's drawn Linux look (`Preset::Adwaita`, nd-backend-gpui) and libadwaita 1.9, found by
putting nd's goldens next to real libadwaita widgets captured the same way. One line per gap; a tick
names how it was verified. References: `docs/parity/linux-hig/adwaita/<page>-<scheme>.png`, recorded
by `scripts/adwaita-reference/capture` on g815 (headless Hyprland at scale 2, fresh `HOME`, Adwaita
Sans 11). Stylesheet sources: [libadwaita 1.9.4 `src/stylesheet`](https://gitlab.gnome.org/GNOME/libadwaita/-/tree/1.9.4/src/stylesheet), [GNOME HIG](https://developer.gnome.org/hig/). nd's side: the Linux goldens in `crates/nd-backend-gpui/tests/goldens/linux/`.

## Colours and metrics

- [x] Baked Adwaita palette is libadwaita's own (window `#fafafb`/`#222226`, view, sidebar, popover, text at 80%, dim 55%) instead of the generic preset (forms, nav and layout goldens next to the reference pages, colours sampled)
- [x] GNOME accent colour (`org.gnome.desktop.interface accent-color`) drives the accent, with libadwaita's nine named colours (unit test `gnome_accent_names_map_to_libadwaitas_colours`)
- [x] Radii: buttons and entries 9, cards 12, popovers and dialogs 15 (forms and layout goldens next to the controls and dialog references)
- [x] Motion tokens: 200 ms state changes on ease-out-quad, 250 ms reveals (`preset.rs` values; no golden covers timing)

## Controls

- [x] Buttons: 10/15/30% text-colour fills, flat 0/7/16%, bold labels, 34 px icon buttons, 17 px text padding, no border (layout-alert golden next to dialog and controls references)
- [x] Suggested and destructive buttons: accent with a 10% light veil on hover and a 20% shade pressed; destructive as red text on a 15/20/35% red tint (layout-alert golden, `gpui_alert_destructive_paints_danger`)
- [x] Switch: 15/20/25% trough, white knob with libadwaita's knob shadow, accent checked trough with hover and press shades (forms golden next to controls reference)
- [x] Check button: 20 px box at radius 6, 15% inset ring that darkens on hover and fills 25% pressed (forms golden next to controls reference)
- [x] Check and radio labels sit 4 px from the indicator (`checkbutton { border-spacing: 4px }`), not the 11 px nd drew before (forms golden next to controls reference; [`_checks.scss`](https://gitlab.gnome.org/GNOME/libadwaita/-/blob/1.9.4/src/stylesheet/widgets/_checks.scss))
- [x] Slider: 4 px trough with hover state, knob with the 10% ring and drop shadow, white under the pointer (forms golden next to controls reference)
- [x] Progress bar: 8 px trough, 150 px minimum; spinner at 16 px over 1.2 s (forms golden next to controls reference)
- [x] Toggle group (segmented): 10% group fill, bold toggles, active toggle white (dark: 20% white) under the toast shadow, 7% hover (forms golden next to controls reference)
- [x] Menus and dropdown popovers: radius 15, 14% black edge and libadwaita's two shadows, 32 px rows, 10% hover and keyboard highlight (forms-menu golden next to the popover reference)
- [x] Overlay popovers open and close at once (GTK 4 has no popover animation); tooltips are 80% near-black with white text, radius 9 (`gpui_a_closed_popover_animates_out_then_is_gone`)
- [x] Focus: buttons are tab stops with a 2 px accent-50% outline right outside; rings on every control settle in over 200 ms; a click leaves keyboard focus alone (forms-menu golden shows the dropdown ring; settling is off in goldens, which run with reduced motion)
- [x] Text weights: bold and heavy text from a variable font (Adwaita Sans) draws at its weight instead of the 400 default instance (forms, nav and layout goldens: bold labels and headings, Fluent body text unchanged)
- [x] Links: standalone accent, underlined at rest (forms golden next to controls reference)
- [x] Coloured text uses libadwaita's standalone colours (destructive `#c30000` light, `#ff938c` dark; accent `#0461be`, `#81d0ff`) (unit test `standalone_colours_match_libadwaitas`, forms golden)

## Navigation and feedback

- [x] Navigation sidebar: neutral selection (10% of the text colour, text unchanged), 7% hover, 36 px rows at radius 9, 2 px apart, 8 px inset, no empty disclosure column (nav-sidebar golden next to app reference)
- [x] Toast: `#505053` pill with white bold title, regular action button and flat close button, 24 px above the window bottom, slides up from below over 300 ms (nav-sidebar golden next to app reference)
- [x] Banner: neutral strip with bold centred title and a regular button, instead of the accent container (nav-sidebar golden next to app reference)
- [x] Alert dialog: radius 18, `title-2` heading, 32/24 px padding, 9 px under the body and 12 px over the responses (21 px, nd drew 36), 44 px responses at radius 12, destructive as red tint, dark dimming at 50% (layout-alert goldens next to dialog references; [`_message-dialog.scss`](https://gitlab.gnome.org/GNOME/libadwaita/-/blob/1.9.4/src/stylesheet/widgets/_message-dialog.scss))
- [x] About dialog: AdwDialog's floating sheet (dialog colour, radius 15, libadwaita's three tight shadows, no grey edge), the version as a regular pill button with accent text at 3/18 px, the close button as a 24 px circle of the regular button fills in a 47 px header (layout-about goldens; [`_dialogs.scss`](https://gitlab.gnome.org/GNOME/libadwaita/-/blob/1.9.4/src/stylesheet/widgets/_dialogs.scss), [`adw-about-dialog.ui`](https://gitlab.gnome.org/GNOME/libadwaita/-/blob/1.9.4/src/adw-about-dialog.ui))
- [x] Search bar: GtkSearchBar's 6 px around the entry over a 1 px line of `--headerbar-shade-color` (12% light, 36% dark) (layout-search-open goldens; [`_toolbars.scss`](https://gitlab.gnome.org/GNOME/libadwaita/-/blob/1.9.4/src/stylesheet/widgets/_toolbars.scss))
- [x] Sidebar section headings: AdwSidebar's `.heading` (bold, body size, text colour) 12 px in from the pane and 12 px above and below, none above the first (nav-sidebar golden; [`_sidebars.scss`](https://gitlab.gnome.org/GNOME/libadwaita/-/blob/1.9.4/src/stylesheet/widgets/_sidebars.scss), `create_header` in [`adw-sidebar.c`](https://gitlab.gnome.org/GNOME/libadwaita/-/blob/1.9.4/src/adw-sidebar.c))
- [x] Backdrop: an inactive window's header bar content fades to 50% and its sidebar goes to the backdrop colour (halfway to the window's), over 200 ms ease-out; selections and labels elsewhere keep their colours as in libadwaita 1.9 (`gpui_an_inactive_window_takes_the_backdrop_look`, layout-chrome and layout-chrome-backdrop goldens)
- [x] Header bar: 47 px and flat, no line under it, as Nautilus' content header draws (layout-chrome golden next to a Nautilus capture)
- [x] Split header bars: with an open sidebar the toolbar splits as in AdwNavigationSplitView (Files, Settings). The sidebar runs the full height with its own header bar in the sidebar colour, holding the sidebar toggle and the leading items with the start window controls; the content header holds the title, the other items and the end window controls (the inspector's header takes them when it shows). A hidden or collapsed sidebar brings the single bar back (layout-chrome goldens next to Nautilus captures, light and dark, focused and backdrop)
- [x] Matugen surfaces: the sidebar and cards take `surface_container`, the colour FormalShell's GTK theme gives Nautilus' sidebar, instead of `surface_container_low` (unit test `formalshell_shape`)
- [ ] Window controls: neutral 10/15/30% circles (close not red) with symbolic minimize, maximize and close icons instead of text glyphs (built and unit-tested; not captured yet: headless goldens draw no window chrome, needs an `ndshell shot` of hello)

## Not matched

- Popover arrow: GTK points menus at their button; nd's popovers have no arrow yet.
- Toast width: AdwToast sizes its title wider than its text; nd sizes the toast to its content.
- Dialog spring (scale 0.8 to 1, damping 0.62) and toast exit (fade with scale 0.95): nd's dialogs and toasts leave at once.
- Kinetic scrolling and overshoot: libadwaita only applies them to touch input, which nd's GPUI backend does not receive on Linux yet.
- Body text is 14 px; Adwaita Sans 11 pt is 14.67 px at 96 dpi. Changing the scale moves every golden of every lane, so it waits for a coordinated pass.
- Contrast: libadwaita's suggested buttons put white on `#3584e4` (3.8:1) and dim text on a selected sidebar row (4.3:1). nd's appearance audit requires 4.5:1, so the suggested fill is `accent_fill` (darkened just enough), dim text on selected rows darkens slightly, and coloured text stays legible on its own 15% tint (dark destructive `#ff9b94` rather than `#ff938c`).
- Backdrop sidebar under a matugen palette: nd mixes sidebar and window 50/50, as libadwaita's own palette does; FormalShell's GTK theme sets `sidebar_backdrop_color` to the window colour, so Nautilus goes all the way.
- Content colour: Nautilus' content pane and its header bar sit on the view colour (`#ffffff`, FormalShell `#1c1b1a`); nd's content pane draws on the window colour (`#fafafb`, `#100f0f`) unless the app fills it. Header and body match either way, with no step between them.
- Sidebar header title: libadwaita's sidebar header bar carries the app's title (Files); nd puts the window title in the content header and leaves the sidebar header to the toggle and leading items.
- Dropdown arrow: libadwaita uses the filled `pan-down` triangle; Lucide has no filled triangle, nd keeps its chevron.
- About dialog contents: AdwAboutDialog shows a 128 px app icon, the developer name and the website as an AdwLinkRow in a boxed list; nd's about panel has no icon or developer field and keeps the website as a filled button.
- libadwaita 1.10 (GNOME 50) draws the popover edge as a 5% ring instead of 1.9's 14% border; nd and the reference captures follow 1.9.
