//! Tier 2: row drags and the edge-revealed sidebar on the GPUI backend inside GPUI's headless
//! app. A press on a row that drags turns into an nd drag past a few points, the list it is over
//! reports the slot under the pointer and takes the drop; a collapsed `edge_reveal()` sidebar
//! slides in when the pointer touches the leading edge and away 250 ms after it leaves.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use nd_backend_gpui::GpuiHost;
use nd_backend_gpui::nav::theme_for;
use nd_components::*;
use nd_gpui::gpui::{self, HeadlessAppContext};
use nd_theme::Scheme;
use nd_ui::prelude::*;
use nd_ui::semantics::{MouseButton, PointerPhase, SyntheticInput};
use nd_ui::{Point, WindowId};

type Log = Rc<RefCell<Vec<String>>>;

fn headless() -> HeadlessAppContext {
    let text = nd_gpui::gpui_platform::current_platform(true).text_system();
    HeadlessAppContext::with_platform(text, Arc::new(()), nd_gpui::gpui_platform::current_headless_renderer)
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

struct Rig {
    cx: HeadlessAppContext,
    host: GpuiHost,
    window: WindowId,
}

fn open<V: Render>(view: impl FnOnce() -> V + 'static) -> Rig {
    let mut cx = headless();
    let out: Rc<RefCell<Option<WindowId>>> = Rc::default();
    let id = out.clone();
    let host = cx.update(|cx| {
        GpuiHost::builder().quit_when_empty(false).start(cx, move |app| {
            let options = WindowOptions::main("drag").size(gpui::size(px(800.), px(600.)));
            let w = app.open_window(options, move |_, cx| cx.new(|_| view())).id();
            *id.borrow_mut() = Some(w);
        })
    });
    settle(&mut cx, &host);
    let window = out.borrow().expect("window");
    Rig { cx, host, window }
}

impl Rig {
    fn input(&mut self, inputs: Vec<SyntheticInput>) {
        let window = self.window;
        self.cx.update(|cx| {
            self.host.update(cx, |app| {
                for input in &inputs {
                    app.synthesize(window, input).expect("posted");
                }
            })
        });
        settle(&mut self.cx, &self.host);
    }

    fn move_to(&mut self, at: Point) {
        self.input(vec![SyntheticInput::Pointer {
            phase: PointerPhase::Move,
            at,
            button: MouseButton::Left,
            click_count: 0,
        }]);
    }

    fn bounds(&self, test_id: &str) -> Option<nd_ui::Rect> {
        let window = self.window;
        self.host.read(|app| {
            let tree = app.semantic_window(window)?;
            tree.root.iter().find(|n| n.test_id.as_deref() == Some(test_id)).and_then(|n| n.bounds)
        })
    }
}

struct Tabs {
    log: Log,
}

impl Render for Tabs {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (drop, end) = (self.log.clone(), self.log.clone());
        div().size_full().child(
            source_list("tabs")
                .nodes([source_node("t1", "One"), source_node("t2", "Two"), source_node("t3", "Three")])
                .draggable_rows("tab")
                .accepts_drops(["tab"])
                .on_drop(move |e: &Dropped, _, _| {
                    let slot = e.slot.clone().expect("a list drop has a slot");
                    drop.borrow_mut().push(format!("drop {} at {:?} {}", e.item.id, slot.parent, slot.index));
                })
                .on_drag_end(move |e: &DragEnded, _, _| end.borrow_mut().push(format!("end {:?}", e.outcome))),
        )
    }
}

pub fn gpui_rows_drag_to_a_slot() {
    let log: Log = Rc::default();
    let l = log.clone();
    let mut r = open(move || Tabs { log: l });
    let theme = theme_for(Scheme::Light);
    let (pad, row) = (theme.spacing.sm.as_f32(), theme.control.regular.as_f32() + 1.);
    let centre = |i: f32| Point { x: 60., y: pad + row * i + row / 2. };
    let below_three = Point { x: 60., y: centre(2.).y + row / 4. };
    r.input(vec![SyntheticInput::Drag { from: centre(0.), to: below_three, steps: 12 }]);
    let window = r.window;
    assert_eq!(
        *log.borrow(),
        ["drop t1 at None 3".to_owned(), format!("end Dropped {{ window: {window:?} }}")],
        "dropped after the last row"
    );
    log.borrow_mut().clear();
    r.input(vec![SyntheticInput::Drag {
        from: centre(2.),
        to: Point { x: 60., y: centre(0.).y - row / 4. },
        steps: 12,
    }]);
    assert_eq!(log.borrow()[0], "drop t3 at None 0", "dropped before the first row");
    log.borrow_mut().clear();
    r.input(vec![SyntheticInput::Drag { from: centre(1.), to: Point { x: 61., y: centre(1.).y + 1. }, steps: 2 }]);
    assert!(log.borrow().is_empty(), "a press that barely moves is a click, not a drag");
}

struct Hidden {
    log: Log,
}

impl Render for Hidden {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let log = self.log.clone();
        scaffold()
            .sidebar(
                sidebar()
                    .width(px(240.))
                    .collapsed(true)
                    .edge_reveal()
                    .on_reveal_change(move |e: &SidebarRevealed, _, _| {
                        log.borrow_mut().push(format!("revealed {}", e.revealed))
                    })
                    .child(div().id("in-sidebar").w_full().h(px(40.))),
            )
            .content(div().id("page").size_full())
    }
}

pub fn gpui_a_hidden_sidebar_reveals_from_the_edge() {
    let log: Log = Rc::default();
    let l = log.clone();
    let mut r = open(move || Hidden { log: l });
    let page = r.bounds("page").expect("page laid out");
    assert_eq!(page.origin.x, 0., "the hidden sidebar takes no room");
    // The headless pointer starts at the window's corner, on the edge: away from it first.
    for y in [300., 301.] {
        r.move_to(Point { x: 300., y });
        std::thread::sleep(Duration::from_millis(300));
        settle(&mut r.cx, &r.host);
    }
    log.borrow_mut().clear();
    r.move_to(Point { x: 300., y: 302. });
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
    r.move_to(Point { x: 2., y: 300. });
    assert_eq!(*log.borrow(), ["revealed true"]);
    std::thread::sleep(Duration::from_millis(260));
    settle(&mut r.cx, &r.host);
    let inside = r.bounds("in-sidebar").expect("the sidebar's content shows");
    assert_eq!(inside.origin.x, 9., "the panel floats 8 in from the edge, inside its border");
    assert_eq!(r.bounds("page").expect("page").size.width, page.size.width, "the page keeps its size");
    r.move_to(Point { x: 100., y: 300. });
    std::thread::sleep(Duration::from_millis(300));
    settle(&mut r.cx, &r.host);
    assert_eq!(*log.borrow(), ["revealed true"], "the pointer over the panel keeps it");
    r.move_to(Point { x: 600., y: 300. });
    std::thread::sleep(Duration::from_millis(300));
    settle(&mut r.cx, &r.host);
    assert_eq!(*log.borrow(), ["revealed true", "revealed false"], "250 ms after the pointer left");
    std::thread::sleep(Duration::from_millis(200));
    settle(&mut r.cx, &r.host);
    let hidden = r.host.read(|app| {
        let tree = app.semantic_window(r.window)?;
        tree.root.iter().find(|n| n.test_id.as_deref() == Some("in-sidebar")).map(|n| n.visible)
    });
    assert_eq!(hidden, Some(false), "gone, the panel's content keeps no bounds from when it showed");
}

/// A sidebar the app docks and undocks, with the edge reveal while undocked.
struct Docking {
    docked: bool,
}

impl Render for Docking {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        scaffold()
            .sidebar(
                sidebar()
                    .width(px(240.))
                    .collapsed(!self.docked)
                    .edge_reveal()
                    .child(div().id("in-sidebar").w_full().h(px(40.))),
            )
            .content(div().id("page").size_full())
            .overlay(div())
    }
}

/// The page's left edge and width on each frame for a while after `docked` is set.
fn dock(r: &mut Rig, view: &Entity<Docking>, docked: bool) -> Vec<(f32, f32, Option<nd_ui::Rect>)> {
    r.cx.update(|cx| r.host.update(cx, |app| view.update(app, |v, cx| (v.docked = docked, cx.notify()).1)));
    let mut seen = Vec::new();
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(16));
        settle(&mut r.cx, &r.host);
        let page = r.bounds("page").expect("page laid out");
        seen.push((page.origin.x, page.size.width, r.bounds("in-sidebar")));
    }
    seen
}

fn open_docking() -> (Rig, Entity<Docking>) {
    unsafe { std::env::set_var("ND_ANIMATION_SLOWDOWN", "4") };
    let mut cx = headless();
    let mut opened = None;
    let host = cx.update(|cx| {
        GpuiHost::builder().quit_when_empty(false).start(cx, |app| {
            let options = WindowOptions::main("dock").size(gpui::size(px(800.), px(600.)));
            let handle = app.open_window(options, |_, cx| cx.new(|_| Docking { docked: true }));
            opened = Some((handle.id(), handle.root().clone()));
        })
    });
    settle(&mut cx, &host);
    let (window, view) = opened.expect("window");
    (Rig { cx, host, window }, view)
}

/// An edge-reveal sidebar slides when the app docks or undocks it, the page beside it laid out
/// at one of its two widths on every frame, and only once it is shut does the edge take over.
pub fn gpui_an_edge_reveal_sidebar_slides_when_it_docks_and_undocks() {
    let (mut r, view) = open_docking();
    for (docked, x, width) in [(false, 0., 800.), (true, 240., 560.)] {
        let seen = dock(&mut r, &view, docked);
        assert!(seen.iter().any(|(x, ..)| *x > 1. && *x < 239.), "the page's edge follows the sidebar: {seen:?}");
        assert!(
            seen.iter().all(|(_, w, _)| *w == 800. || *w == 560.),
            "the page is never laid out between its two widths: {seen:?}"
        );
        assert_eq!(seen.last().map(|s| (s.0, s.1)), Some((x, width)), "rests docked {docked}");
    }
}

/// Docking from the floating panel keeps the sidebar whole where it is while the page's edge
/// moves out to it, instead of the panel going and the sidebar sliding in from the edge.
pub fn gpui_a_floating_sidebar_docks_where_it_is() {
    let (mut r, view) = open_docking();
    dock(&mut r, &view, false);
    r.move_to(Point { x: 300., y: 300. });
    r.move_to(Point { x: 2., y: 300. });
    std::thread::sleep(Duration::from_millis(900));
    settle(&mut r.cx, &r.host);
    assert!(r.bounds("in-sidebar").is_some_and(|b| b.origin.x > 0.), "the floating panel is in");
    let seen = dock(&mut r, &view, true);
    assert!(seen.iter().any(|(x, ..)| *x > 1. && *x < 239.), "the page's edge moves out: {seen:?}");
    for (x, _, sidebar) in &seen {
        let sidebar = sidebar.expect("the sidebar shows throughout");
        assert!(sidebar.size.width >= 230., "whole, never clipped to a share: {seen:?}");
        assert!(sidebar.origin.x >= 0. && sidebar.origin.x <= 9., "where the panel was: {seen:?}");
        assert!(*x <= 240.);
    }
    assert_eq!(seen.last().map(|s| (s.0, s.1)), Some((240., 560.)), "docked beside the page");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tests: Vec<(&str, fn())> = vec![
        ("gpui_rows_drag_to_a_slot", gpui_rows_drag_to_a_slot),
        ("gpui_a_hidden_sidebar_reveals_from_the_edge", gpui_a_hidden_sidebar_reveals_from_the_edge),
        (
            "gpui_an_edge_reveal_sidebar_slides_when_it_docks_and_undocks",
            gpui_an_edge_reveal_sidebar_slides_when_it_docks_and_undocks,
        ),
        ("gpui_a_floating_sidebar_docks_where_it_is", gpui_a_floating_sidebar_docks_where_it_is),
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
