//! Tier 2: the drawn command palette, toasts and source tree on the GPUI backend inside GPUI's
//! headless app, with input through the window's own key path; and the goldens of the drawn
//! look, light and dark, rendered by GPUI's headless renderer from the same view builders.
//!
//! `ND_UPDATE_GOLDENS=1` rewrites the goldens. Goldens are per OS (text is shaped by the
//! platform's text system): `tests/goldens/<os>/nav-<view>-<scheme>.png`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use nd_backend_gpui::GpuiHost;
use nd_backend_gpui::nav::{Marks, Paint, banner_view, palette_overlay, source_tree_view, theme_for, toast_view};
use nd_components::*;
use nd_core::semantic::Role;
use nd_gpui::gpui::{self, HeadlessAppContext};
use nd_theme::Scheme;
use nd_ui::prelude::*;
use nd_ui::semantics::SyntheticInput;
use nd_ui::{Keystroke, NodeId, SemanticNode, WindowId};

fn headless() -> HeadlessAppContext {
    let text = nd_gpui::gpui_platform::current_platform(true).text_system();
    HeadlessAppContext::with_platform(text, Arc::new(()), nd_gpui::gpui_platform::current_headless_renderer)
}

fn find<'a>(node: &'a SemanticNode, f: &impl Fn(&SemanticNode) -> bool) -> Option<&'a SemanticNode> {
    if f(node) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, f))
}

fn by_test_id(host: &GpuiHost, window: WindowId, id: &str) -> SemanticNode {
    host.read(|app| {
        let tree = app.semantic_window(window).expect("window");
        find(&tree.root, &|n| n.test_id.as_deref() == Some(id)).cloned().expect(id)
    })
}

fn settle(cx: &mut HeadlessAppContext, host: &GpuiHost) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|cx| {
            host.pump(cx);
            host.draw(cx);
        });
    }
    cx.run_until_parked();
}

#[derive(Default)]
struct Log {
    queries: Vec<String>,
    activated: Vec<String>,
    cancels: u32,
    dismissed: Vec<(String, DismissReason)>,
}

const NOTES: [&str; 3] = ["Trip to Lisbon", "Groceries", "Trivia night"];

fn items(query: &str) -> Vec<PaletteItem> {
    let q = query.to_lowercase();
    NOTES
        .iter()
        .filter(|t| t.to_lowercase().contains(&q))
        .map(|t| {
            let item = palette_item(*t, *t).icon(Icon::FileText).subtitle("Notes").hint("Open");
            if !q.is_empty() && t.to_lowercase().starts_with(&q) { item.completion(*t) } else { item }
        })
        .collect()
}

struct Nav {
    log: Rc<RefCell<Log>>,
    open: bool,
    query: SharedString,
    toasts: Vec<Toast>,
}

impl Render for Nav {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                command_palette("palette")
                    .open(self.open)
                    .placeholder("Search notes")
                    .query(self.query.clone())
                    .items(items(&self.query))
                    .on_query_change(cx.listener(|this, e: &PaletteQueryChanged, _, cx| {
                        this.log.borrow_mut().queries.push(e.text.to_string());
                        this.query = e.text.clone();
                        cx.notify();
                    }))
                    .on_activate(cx.listener(|this, e: &PaletteActivated, _, cx| {
                        this.log.borrow_mut().activated.push(e.id.to_string());
                        this.open = false;
                        cx.notify();
                    }))
                    .on_cancel(cx.listener(|this, _: &PaletteCancelled, _, cx| {
                        this.log.borrow_mut().cancels += 1;
                        this.open = false;
                        cx.notify();
                    })),
            )
            .child(toast_overlay("toasts").toasts(self.toasts.clone()).on_dismiss(cx.listener(
                |this, e: &ToastDismissed, _, cx| {
                    this.log.borrow_mut().dismissed.push((e.id.to_string(), e.reason));
                    this.toasts.retain(|t| t.id != e.id);
                    cx.notify();
                },
            )))
    }
}

struct Opened {
    cx: HeadlessAppContext,
    host: GpuiHost,
    window: WindowId,
    log: Rc<RefCell<Log>>,
    view: Option<Entity<Nav>>,
}

fn open(nav: impl FnOnce(Rc<RefCell<Log>>) -> Nav + 'static) -> Opened {
    let mut cx = headless();
    let log: Rc<RefCell<Log>> = Rc::default();
    let l = log.clone();
    type Slot = Rc<RefCell<Option<(WindowId, Entity<Nav>)>>>;
    let opened: Slot = Rc::default();
    let o = opened.clone();
    let host = cx.update(|cx| {
        GpuiHost::builder().quit_when_empty(false).start(cx, move |app| {
            let handle =
                app.open_window(WindowOptions::main("nav").size(size(px(800.), px(560.))), |_, cx| cx.new(|_| nav(l)));
            *o.borrow_mut() = Some((handle.id(), handle.root().clone()));
        })
    });
    settle(&mut cx, &host);
    let (window, view) = opened.borrow_mut().take().expect("window");
    Opened { cx, host, window, log, view: Some(view) }
}

fn press(o: &mut Opened, key: &'static str) {
    let keystroke = Keystroke::parse(key).expect("keystroke");
    let window = o.window;
    o.cx.update(|cx| o.host.update(cx, |app| app.synthesize(window, &SyntheticInput::Key(keystroke)))).expect("posted");
    settle(&mut o.cx, &o.host);
}

fn palette_node(o: &Opened) -> SemanticNode {
    by_test_id(&o.host, o.window, "palette")
}

mod gate_m6 {
    use super::*;

    pub fn gpui_palette_types_completes_and_activates() {
        let mut o = open(|log| Nav { log, open: true, query: "".into(), toasts: Vec::new() });
        let palette = palette_node(&o);
        assert_eq!((palette.role, palette.name.as_str()), (Role::Dialog, "Search notes"));
        assert_eq!(palette.expanded, Some(true), "open from its first render");
        for key in ["t", "r", "i"] {
            press(&mut o, key);
        }
        assert_eq!(o.log.borrow().queries, ["t", "tr", "tri"], "each keystroke reaches the app through the window");
        assert_eq!(palette_node(&o).value.as_deref(), Some("trip to Lisbon"), "the first row's completion shows");
        press(&mut o, "tab");
        assert_eq!(o.log.borrow().queries.last().map(String::as_str), Some("trip to Lisbon"), "Tab accepts it");
        press(&mut o, "enter");
        assert_eq!(o.log.borrow().activated, ["Trip to Lisbon"]);
        assert_eq!(palette_node(&o).expanded, Some(false), "the app closed it");
        assert_eq!(o.log.borrow().cancels, 0, "an app-driven close is no cancel");

        let view = o.view.clone().expect("view");
        o.cx.update(|cx| {
            o.host.update(cx, |app| {
                view.update(app, |this, cx| {
                    this.open = true;
                    this.query = "".into();
                    cx.notify();
                })
            })
        });
        settle(&mut o.cx, &o.host);
        press(&mut o, "down");
        press(&mut o, "escape");
        assert_eq!(o.log.borrow().cancels, 1, "Esc cancels once");
        assert_eq!(palette_node(&o).expanded, Some(false));
    }

    pub fn gpui_toasts_time_out_in_order() {
        let mut o = open(|log| Nav {
            log,
            open: false,
            query: "".into(),
            toasts: vec![
                toast("saved", "Saved").timeout(Duration::from_millis(500)),
                toast("synced", "Synced").timeout(Duration::from_millis(500)),
            ],
        });
        assert_eq!(by_test_id(&o.host, o.window, "toasts").name, "Saved");
        o.cx.advance_clock(Duration::from_millis(600));
        settle(&mut o.cx, &o.host);
        assert_eq!(o.log.borrow().dismissed, [("saved".to_string(), DismissReason::Timeout)]);
        assert_eq!(by_test_id(&o.host, o.window, "toasts").name, "Synced", "the queue moved on");
        o.cx.advance_clock(Duration::from_millis(600));
        settle(&mut o.cx, &o.host);
        assert_eq!(o.log.borrow().dismissed.len(), 2);
        assert_eq!(by_test_id(&o.host, o.window, "toasts").name, "");
    }

    /// WinUI's icon rail: a row with artwork keeps its cover there, as it does expanded.
    pub fn gpui_rail_rows_keep_their_artwork() {
        use gpui::AppContext as _;
        if nd_gpui::gpui_platform::current_headless_renderer().ok().flatten().is_none() {
            println!("skipped: GPUI has no headless renderer on this OS");
            return;
        }
        let mut cx = headless();
        let handle =
            cx.open_window(gpui::size(gpui::px(48.), gpui::px(120.)), |_, cx| cx.new(|_| Rail)).expect("window");
        cx.run_until_parked();
        let shot = cx.capture_screenshot(handle.into()).expect("screenshot");
        let cover = shot.pixels().filter(|p| p[0] > 200 && p[1] < 60 && p[2] > 200).count();
        assert!(cover >= 200, "the rail draws the row's cover ({cover} cover pixels)");
    }

    /// A row's capacity meter fills its track in proportion, in the accent.
    pub fn gpui_source_row_meter_fills_in_proportion() {
        use gpui::AppContext as _;
        if nd_gpui::gpui_platform::current_headless_renderer().ok().flatten().is_none() {
            println!("skipped: GPUI has no headless renderer on this OS");
            return;
        }
        let accent = |fraction: f32| {
            let mut cx = headless();
            let handle = cx
                .open_window(gpui::size(gpui::px(220.), gpui::px(60.)), move |_, cx| cx.new(|_| Meter(fraction)))
                .expect("window");
            cx.run_until_parked();
            let shot = cx.capture_screenshot(handle.into()).expect("screenshot");
            let a = meter_theme().accent.to_rgb();
            let near = |c: u8, f: f32| (f32::from(c) - f * 255.).abs() < 24.;
            shot.pixels().filter(|p| near(p[0], a.r) && near(p[1], a.g) && near(p[2], a.b)).count()
        };
        let (low, high) = (accent(0.2), accent(0.8));
        assert!(low > 0, "a fifth full still shows");
        assert!(high > low * 3, "four fifths fill about four times the track: {high} against {low}");
    }

    /// Every meter in a source list is as long as the others and starts under its title,
    /// whatever the title's and the badge's widths.
    pub fn gpui_source_row_meters_share_one_width() {
        use gpui::AppContext as _;
        if nd_gpui::gpui_platform::current_headless_renderer().ok().flatten().is_none() {
            println!("skipped: GPUI has no headless renderer on this OS");
            return;
        }
        let mut cx = headless();
        let handle =
            cx.open_window(gpui::size(gpui::px(260.), gpui::px(120.)), |_, cx| cx.new(|_| Meters)).expect("window");
        cx.run_until_parked();
        let shot = cx.capture_screenshot(handle.into()).expect("screenshot");
        let a = meter_theme().accent.to_rgb();
        let near = |c: u8, f: f32| (f32::from(c) - f * 255.).abs() < 24.;
        // Each row's accent span: the leftmost and rightmost accent pixel in its half.
        let half = shot.height() / 2;
        let span = |rows: std::ops::Range<u32>| {
            let xs: Vec<u32> = rows
                .flat_map(|y| (0..shot.width()).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let p = shot.get_pixel(x, y);
                    near(p[0], a.r) && near(p[1], a.g) && near(p[2], a.b)
                })
                .map(|(x, _)| x)
                .collect();
            (*xs.iter().min().expect("a meter"), *xs.iter().max().expect("a meter"))
        };
        let (long, short) = (span(0..half), span(half..shot.height()));
        assert!(long.0.abs_diff(short.0) <= 1, "both meters start under the title: {long:?} and {short:?}");
        assert!(long.1.abs_diff(short.1) <= 1, "both meters are one length: {long:?} and {short:?}");
    }

    /// A caption over a meter grows its row past the one-line pitch, with the meter clear of
    /// the row's bottom edge, so it never runs into the next row.
    /// A right click on a row selects it and opens the menu built for its id; a middle click
    /// reports the row and leaves the selection where it was.
    pub fn gpui_source_rows_take_menus_and_middle_clicks() {
        use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput};
        use nd_ui::backend::Backend as _;
        let mut cx = headless();
        let log: Rc<RefCell<Log>> = Rc::default();
        let l = log.clone();
        let opened: Rc<RefCell<Option<WindowId>>> = Rc::default();
        let o = opened.clone();
        let host = cx.update(|cx| {
            GpuiHost::builder().quit_when_empty(false).start(cx, move |app| {
                let backend = app.backend_as::<nd_backend_gpui::GpuiBackend>().expect("GPUI backend").clone();
                app.set_global(Chrome(nd_backend_gpui::GpuiChrome::new(backend)));
                let handle = app.open_window(WindowOptions::main("sources").size(size(px(300.), px(300.))), |_, cx| {
                    cx.new(|_| Sources { log: l, selected: "a".into() })
                });
                *o.borrow_mut() = Some(handle.id());
            })
        });
        settle(&mut cx, &host);
        let window = opened.borrow_mut().take().expect("window");
        let tree = by_test_id(&host, window, "sources").bounds.expect("tree bounds");
        // libadwaita's 36 pt rows, 2 pt apart, under the list's 8 pt padding.
        let row = |k: f32| gpui::point(px(tree.origin.x + 60.), px(tree.origin.y + 8. + 38. * k + 18.));
        let click = |cx: &mut HeadlessAppContext, button: MouseButton, position: gpui::Point<Pixels>| {
            let modifiers = Modifiers::default();
            let handle = host.window_handle(window).expect("gpui window");
            for input in [
                PlatformInput::MouseDown(MouseDownEvent {
                    button,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }),
                PlatformInput::MouseUp(MouseUpEvent { button, position, modifiers, click_count: 1 }),
            ] {
                cx.update(|cx| handle.update(cx, |_, window, cx| window.dispatch_event(input, cx))).expect("window");
            }
            settle(cx, &host);
        };
        click(&mut cx, MouseButton::Right, row(1.));
        assert_eq!(log.borrow().activated, ["select b", "menu b"], "the row is selected before its menu asks");
        assert!(host.backend().menu_open(window), "the right click opens the row's menu");
        let paints = host.backend().paints(window).expect("paints");
        assert!(paints.iter().any(|p| p.label == "Open b"), "the menu is the one built for b");
        let escape = Keystroke::parse("escape").expect("escape");
        cx.update(|cx| host.update(cx, |app| app.synthesize(window, &SyntheticInput::Key(escape)))).expect("posted");
        settle(&mut cx, &host);

        log.borrow_mut().activated.clear();
        click(&mut cx, MouseButton::Middle, row(2.));
        assert_eq!(log.borrow().activated, ["middle c"], "a middle click reports its row and selects nothing");
        assert!(!host.backend().menu_open(window));
    }

    pub fn gpui_source_row_fits_a_caption_over_a_meter() {
        use gpui::AppContext as _;
        if nd_gpui::gpui_platform::current_headless_renderer().ok().flatten().is_none() {
            println!("skipped: GPUI has no headless renderer on this OS");
            return;
        }
        let mut cx = headless();
        let handle =
            cx.open_window(gpui::size(gpui::px(260.), gpui::px(140.)), |_, cx| cx.new(|_| Volumes)).expect("window");
        cx.run_until_parked();
        let shot = cx.capture_screenshot(handle.into()).expect("screenshot");
        let scale = shot.width() as f32 / 260.;
        let theme = meter_theme();
        let bg = theme.surface.sidebar.to_rgb();
        let a = theme.accent.to_rgb();
        let near = |c: u8, f: f32, by: f32| (f32::from(c) - f * 255.).abs() < by;
        // The selected row's fill down a column past the text, and the meter's accent rows.
        let x = shot.width() - (12. * scale) as u32;
        let band: Vec<u32> = (0..shot.height())
            .filter(|&y| {
                let p = shot.get_pixel(x, y);
                !(near(p[0], bg.r, 3.) && near(p[1], bg.g, 3.) && near(p[2], bg.b, 3.))
            })
            .collect();
        let (top, bottom) = (*band.first().expect("a selected row"), *band.last().expect("a selected row"));
        let meter: Vec<u32> = (0..shot.height())
            .filter(|&y| {
                (0..shot.width()).any(|x| {
                    let p = shot.get_pixel(x, y);
                    near(p[0], a.r, 24.) && near(p[1], a.g, 24.) && near(p[2], a.b, 24.)
                })
            })
            .collect();
        let meter_bottom = *meter.last().expect("a meter");
        assert!((bottom - top) as f32 / scale > 40., "the row is taller than one line: {top}..{bottom}");
        assert!(
            (bottom as f32 - meter_bottom as f32) / scale >= 4.,
            "the meter sits clear of the row's bottom: meter ends {meter_bottom}, row {bottom}"
        );
    }

    /// A source list taller than its pane scrolls: every row keeps its height, so a caption
    /// over a meter never lands on the meter of the row above.
    pub fn gpui_source_list_overflow_scrolls_instead_of_squashing() {
        use gpui::AppContext as _;
        if nd_gpui::gpui_platform::current_headless_renderer().ok().flatten().is_none() {
            println!("skipped: GPUI has no headless renderer on this OS");
            return;
        }
        let mut cx = headless();
        let handle =
            cx.open_window(gpui::size(gpui::px(260.), gpui::px(200.)), |_, cx| cx.new(|_| Crowded)).expect("window");
        cx.run_until_parked();
        let shot = cx.capture_screenshot(handle.into()).expect("screenshot");
        let scale = shot.width() as f32 / 260.;
        let a = meter_theme().accent.to_rgb();
        let near = |c: u8, f: f32| (f32::from(c) - f * 255.).abs() < 24.;
        let accent_row = |y: u32| {
            (0..shot.width()).any(|x| {
                let p = shot.get_pixel(x, y);
                near(p[0], a.r) && near(p[1], a.g) && near(p[2], a.b)
            })
        };
        let tops: Vec<u32> = (0..shot.height()).filter(|&y| accent_row(y) && (y == 0 || !accent_row(y - 1))).collect();
        assert!(tops.len() >= 2, "at least two meters in view: {tops:?}");
        assert!(tops.len() < 8, "the rows past the pane are scrolled out of view, not squeezed in: {tops:?}");
        for pair in tops.windows(2) {
            let pitch = (pair[1] - pair[0]) as f32 / scale;
            assert!(pitch > 44., "a row with a caption over its meter keeps its height: {pitch} between {pair:?}");
        }
    }

    pub fn gpui_nav_goldens() {
        // GPUI renders headless only on macOS (Metal) and Linux (wgpu); Windows captures arrive
        // with the Fluent goldens (M3).
        if nd_gpui::gpui_platform::current_headless_renderer().ok().flatten().is_none() {
            println!("skipped: GPUI has no headless renderer on this OS");
            return;
        }
        let mut failed = Vec::new();
        for scheme in [Scheme::Light, Scheme::Dark] {
            for view in [GoldenView::Sidebar, GoldenView::Palette] {
                if let Err(e) = golden(view, scheme) {
                    failed.push(e);
                }
            }
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }
}

#[derive(Clone, Copy, Debug)]
enum GoldenView {
    /// The source tree, a banner and a toast.
    Sidebar,
    /// The palette with a completion, over the same window.
    Palette,
}

struct Golden {
    view: GoldenView,
    scheme: Scheme,
}

fn golden_tree() -> SourceTreeProps {
    SourceTreeProps {
        nodes: vec![
            source_section("lib", "Library"),
            source_node("inbox", "Inbox").parent("lib").icon(Icon::Inbox).badge("12"),
            source_node("work", "Work").parent("lib").icon(Icon::Folder).expanded(true).actions(["close"]),
            source_node("q3", "Q3 planning").parent("work").caption("Edited today"),
            source_node("trips", "Trips").parent("work"),
            source_section("tags", "Tags"),
            source_node("starred", "Starred").parent("tags").icon(Icon::Star),
        ],
        actions: vec![source_action("close", Icon::Trash).label("Close")],
        selected: Some("q3".into()),
        action_visibility: ActionVisibility::Always,
        empty: None,
        drag_kind: None,
        accepts: Vec::new(),
        drop_onto: false,
        row_menus: false,
        style: Default::default(),
    }
}

impl gpui::Render for Golden {
    fn render(&mut self, window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::{IntoElement, ParentElement, Styled};
        let theme = theme_for(self.scheme);
        let viewport = window.viewport_size();
        let banner = BannerProps {
            title: "An update is ready to install".into(),
            action: Some("Restart".into()),
            revealed: true,
            style: Default::default(),
        };
        let toast = toast("deleted", "Note deleted").action("Undo");
        let mut root = gpui::div()
            .size_full()
            .flex()
            .flex_row()
            .bg(theme.surface.base)
            .text_color(theme.text.primary)
            .child(gpui::div().w(gpui::px(240.)).h_full().bg(theme.surface.sidebar).child(source_tree_view(
                NodeId(1),
                &golden_tree(),
                None,
                false,
                None,
                &Default::default(),
                Paint { theme: &theme, marks: &Marks::default() },
            )))
            .child(
                gpui::div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(banner_view(NodeId(2), &banner, &theme, None, &Marks::default()))
                    .child(gpui::div().p(gpui::px(24.)).child("Q3 planning")),
            )
            .child(toast_view(Some(&toast), &theme, viewport, false, None, &Marks::default()));
        if let GoldenView::Palette = self.view {
            let mut model = PaletteModel::default();
            model.set_items(&items("tri"));
            model.present();
            model.edited("tri");
            model.set_items(&items("tri"));
            root = root.child(palette_overlay(
                &model,
                Some("Search notes"),
                &theme,
                viewport,
                None,
                None,
                &Marks::default(),
            ));
        }
        root.into_any_element()
    }
}

/// A collapsed Fluent sidebar holding one playlist row with a magenta cover.
fn meter_theme() -> nd_theme::Theme {
    use nd_theme::{Metrics, Palette, Preset, Theme};
    let preset = Preset::Adwaita;
    Theme::derive(preset, Scheme::Light, Palette::baked_for(preset, Scheme::Light), Metrics::preset(preset), 1.)
}

/// Two volumes at the same fill: a long title with a badge, and a short one without.
struct Meters;

impl gpui::Render for Meters {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::{IntoElement, ParentElement, Styled};
        let theme = meter_theme();
        let props = SourceTreeProps {
            nodes: vec![
                source_node("disk", "Macintosh HD").badge("12 GB free").meter(0.5),
                source_node("usb", "USB").meter(0.5),
            ],
            selected: None,
            ..golden_tree()
        };
        gpui::div()
            .size_full()
            .bg(theme.surface.sidebar)
            .child(source_tree_view(
                NodeId(1),
                &props,
                None,
                false,
                None,
                &Default::default(),
                Paint { theme: &theme, marks: &Marks::default() },
            ))
            .into_any_element()
    }
}

gpui::actions!(test, [OpenRow]);

struct Sources {
    log: Rc<RefCell<Log>>,
    selected: SharedString,
}

impl Render for Sources {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (menu_log, middle_log) = (self.log.clone(), self.log.clone());
        source_tree("sources")
            .items([source_item("a", "Alpha"), source_item("b", "Beta"), source_item("c", "Gamma")])
            .selected(self.selected.clone())
            .on_select(cx.listener(|this, e: &SourceSelected, _, cx| {
                this.log.borrow_mut().activated.push(format!("select {}", e.id));
                this.selected = e.id.clone();
                cx.notify();
            }))
            .context_menu(move |id, menu, _, _| {
                menu_log.borrow_mut().activated.push(format!("menu {id}"));
                menu.item(format!("Open {id}"), OpenRow)
            })
            .on_middle_click(move |e: &SourceMiddleClicked, _, _| {
                middle_log.borrow_mut().activated.push(format!("middle {}", e.id));
            })
    }
}

/// A selected volume with its free space as a caption over its meter, and one more row.
struct Volumes;

impl gpui::Render for Volumes {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::{IntoElement, ParentElement, Styled};
        let theme = meter_theme();
        let props = SourceTreeProps {
            nodes: vec![
                source_node("disk", "Data").caption("120 GB free of 500 GB").meter(0.5),
                source_node("usb", "USB"),
            ],
            selected: Some("disk".into()),
            ..golden_tree()
        };
        gpui::div()
            .size_full()
            .bg(theme.surface.sidebar)
            .child(source_tree_view(
                NodeId(1),
                &props,
                None,
                false,
                None,
                &Default::default(),
                Paint { theme: &theme, marks: &Marks::default() },
            ))
            .into_any_element()
    }
}

/// Eight volumes with a caption over a meter, more than a short pane holds.
struct Crowded;

impl gpui::Render for Crowded {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::{IntoElement, ParentElement, Styled};
        let theme = meter_theme();
        let props = SourceTreeProps {
            nodes: (0..8)
                .map(|i| source_node(format!("disk{i}"), format!("Disk {i}")).caption("120 GB free").meter(0.5))
                .collect(),
            selected: None,
            ..golden_tree()
        };
        gpui::div()
            .size_full()
            .bg(theme.surface.sidebar)
            .child(source_tree_view(
                NodeId(1),
                &props,
                None,
                false,
                None,
                &Default::default(),
                Paint { theme: &theme, marks: &Marks::default() },
            ))
            .into_any_element()
    }
}

/// One source row with a meter `.0` full.
struct Meter(f32);

impl gpui::Render for Meter {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::{IntoElement, ParentElement, Styled};
        let theme = meter_theme();
        let props = SourceTreeProps {
            nodes: vec![source_node("disk", "Macintosh HD").meter(self.0)],
            selected: None,
            ..golden_tree()
        };
        gpui::div()
            .size_full()
            .bg(theme.surface.sidebar)
            .child(source_tree_view(
                NodeId(1),
                &props,
                None,
                false,
                None,
                &Default::default(),
                Paint { theme: &theme, marks: &Marks::default() },
            ))
            .into_any_element()
    }
}

struct Rail;

impl gpui::Render for Rail {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::{IntoElement, ParentElement, Styled};
        use nd_theme::{Metrics, Palette, Preset, Theme};
        let preset = Preset::Fluent;
        let theme = Theme::derive(
            preset,
            Scheme::Light,
            Palette::baked_for(preset, Scheme::Light),
            Metrics::preset(preset),
            1.,
        );
        let props = SourceTreeProps {
            nodes: vec![
                source_node("gym", "GYM")
                    .icon(Icon::ListMusic)
                    .artwork(ImageSource::Bytes { key: "gym".into(), bytes: Arc::from(&b""[..]) }),
            ],
            ..golden_tree()
        };
        let magenta = image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 255, 255]));
        let art = std::collections::HashMap::from([(
            gpui::SharedString::from("gym"),
            Arc::new(gpui::RenderImage::new(vec![image::Frame::new(magenta)])),
        )]);
        gpui::div()
            .size_full()
            .bg(theme.surface.sidebar)
            .child(source_tree_view(
                NodeId(1),
                &props,
                None,
                true,
                None,
                &art,
                Paint { theme: &theme, marks: &Marks::default() },
            ))
            .into_any_element()
    }
}

fn golden_path(view: GoldenView, scheme: Scheme) -> PathBuf {
    let view = match view {
        GoldenView::Sidebar => "sidebar",
        GoldenView::Palette => "palette",
    };
    let scheme = if scheme.is_dark() { "dark" } else { "light" };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/goldens/{}/nav-{view}-{scheme}.png", std::env::consts::OS))
}

/// Per channel, out of 255, and the share of pixels allowed past it: antialiasing drifts.
const CHANNEL_TOLERANCE: i16 = 24;
const PIXEL_TOLERANCE: f64 = 0.004;

fn golden(view: GoldenView, scheme: Scheme) -> Result<(), String> {
    use gpui::AppContext as _;
    let mut cx = headless();
    let handle = cx
        .open_window(gpui::size(gpui::px(800.), gpui::px(520.)), |_, cx| cx.new(|_| Golden { view, scheme }))
        .map_err(|e| e.to_string())?;
    cx.run_until_parked();
    let image = cx.capture_screenshot(handle.into()).map_err(|e| e.to_string())?;
    let path = golden_path(view, scheme);
    if std::env::var_os("ND_UPDATE_GOLDENS").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().expect("dir")).map_err(|e| e.to_string())?;
        image.save(&path).map_err(|e| e.to_string())?;
        println!("recorded {}", path.display());
        return Ok(());
    }
    if !path.exists() {
        return Err(format!("{} is missing (record it with ND_UPDATE_GOLDENS=1)", path.display()));
    }
    let expected = image::open(&path).map_err(|e| e.to_string())?.to_rgba8();
    let actual_path = path.with_extension("actual.png");
    if expected.dimensions() != image.dimensions() {
        let _ = image.save(&actual_path);
        return Err(format!("{}: size {:?}, golden {:?}", path.display(), image.dimensions(), expected.dimensions()));
    }
    let off = expected
        .pixels()
        .zip(image.pixels())
        .filter(|(a, b)| a.0.iter().zip(b.0.iter()).any(|(x, y)| (*x as i16 - *y as i16).abs() > CHANNEL_TOLERANCE))
        .count();
    let share = off as f64 / (image.width() * image.height()).max(1) as f64;
    if share > PIXEL_TOLERANCE {
        let _ = image.save(&actual_path);
        return Err(format!(
            "{}: {:.2}% of pixels differ (see {})",
            path.display(),
            share * 100.,
            actual_path.display()
        ));
    }
    let _ = std::fs::remove_file(&actual_path);
    println!("ok   {} ({:.3}% off)", path.display(), share * 100.);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tests: Vec<(&str, fn())> = vec![
        ("gate_m6::gpui_palette_types_completes_and_activates", gate_m6::gpui_palette_types_completes_and_activates),
        ("gate_m6::gpui_toasts_time_out_in_order", gate_m6::gpui_toasts_time_out_in_order),
        ("gate_m6::gpui_rail_rows_keep_their_artwork", gate_m6::gpui_rail_rows_keep_their_artwork),
        ("gate_m6::gpui_source_row_meter_fills_in_proportion", gate_m6::gpui_source_row_meter_fills_in_proportion),
        ("gate_m6::gpui_source_row_meters_share_one_width", gate_m6::gpui_source_row_meters_share_one_width),
        ("gate_m6::gpui_source_row_fits_a_caption_over_a_meter", gate_m6::gpui_source_row_fits_a_caption_over_a_meter),
        (
            "gate_m6::gpui_source_rows_take_menus_and_middle_clicks",
            gate_m6::gpui_source_rows_take_menus_and_middle_clicks,
        ),
        (
            "gate_m6::gpui_source_list_overflow_scrolls_instead_of_squashing",
            gate_m6::gpui_source_list_overflow_scrolls_instead_of_squashing,
        ),
        ("gate_m6::gpui_nav_goldens", gate_m6::gpui_nav_goldens),
    ];
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("--list") {
        if !has("--ignored") {
            for (name, _) in &tests {
                println!("{name}: test");
            }
        }
        return;
    }
    if has("--ignored") {
        return;
    }
    let exact = has("--exact");
    let mut skip = false;
    let filters: Vec<&String> = args
        .iter()
        .filter(|a| {
            if std::mem::take(&mut skip) {
                return false;
            }
            if matches!(a.as_str(), "--format" | "--test-threads" | "--color" | "--skip" | "-Z") {
                skip = true;
            }
            !a.starts_with('-')
        })
        .collect();
    let mut failed = 0;
    for (name, test) in &tests {
        let selected =
            filters.is_empty() || filters.iter().any(|f| if exact { *f == name } else { name.contains(f.as_str()) });
        if !selected {
            continue;
        }
        match std::panic::catch_unwind(test) {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        std::process::exit(101);
    }
}
