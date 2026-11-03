//! Tier 2: drag and drop onto the parts of a window in GPUI's headless app. Rows dragged out of
//! a table carry the selection, and their files once they leave the window; files dragged in
//! from outside land on a table row, a source list item or a tab, each named in what the app
//! receives, and skip `on_file_drop`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use nd_backend_gpui::GpuiHost;
use nd_components::{TabStyle, source_item, source_list, tab_strip};
use nd_gpui::gpui::{
    ExternalPaths, FileDropEvent, HeadlessAppContext, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput, point,
};
use nd_ui::prelude::*;
use nd_ui::{DragItem, DragOver, Dropped, WindowId};
use nd_virtual::{ListState, RowDropped, Selection, column, table};

type Log = Rc<RefCell<Vec<String>>>;

struct Targets {
    list: ListState<u32>,
    log: Log,
    /// The source list item a drag over it would land on.
    over: Over,
}

type Over = Rc<RefCell<Option<SharedString>>>;

impl Render for Targets {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (tabs, places, rows) = (self.log.clone(), self.log.clone(), self.log.clone());
        let over = self.over.clone();
        let dropped = |e: &Dropped| format!("{:?} {:?}", e.slot.as_ref().and_then(|s| s.on.clone()), e.item.ids);
        v_flex()
            .size_full()
            .child(
                tab_strip("tabs")
                    .presentation(TabStyle::Strip)
                    .tab("one", "One")
                    .tab("two", "Two")
                    .selected("one")
                    .accepts_drops([DragItem::FILES])
                    .on_drop(move |e: &Dropped, _, _| tabs.borrow_mut().push(format!("tab {}", dropped(e)))),
            )
            .child(
                h_flex()
                    .flex_1()
                    .child(
                        div().id("sidebar").w(px(160.)).h_full().child(
                            source_list("places")
                                .section("Places", [source_item("home", "Home"), source_item("disk", "Disk")])
                                .drops_onto_rows()
                                .accepts_drops([DragItem::FILES])
                                .on_drag_over(move |e: &DragOver, _, _| {
                                    *over.borrow_mut() = e.slot.as_ref().and_then(|s| s.on.clone());
                                })
                                .on_drop(move |e: &Dropped, _, _| {
                                    places.borrow_mut().push(format!("place {}", dropped(e)))
                                }),
                        ),
                    )
                    .child(
                        table("files", &self.list, vec![column("name", "Name").width(300.)], |row: u32, _, _, _| {
                            div().h(px(24.)).child(format!("file-{row}"))
                        })
                        .selection(Selection::Multiple)
                        .drag_rows("file", |k| format!("file-{k}").into())
                        .drag_out_files(|k| (k % 4 != 3).then(|| PathBuf::from(format!("/tmp/file-{k}"))))
                        .accepts_drops(["file", DragItem::FILES])
                        .drop_on(|k| k % 2 == 0)
                        .on_drop(move |e: &RowDropped<u32>, _, _| {
                            rows.borrow_mut().push(format!("row {:?} {:?} alt {}", e.key, e.item.ids, e.modifiers.alt))
                        }),
                    ),
            )
    }
}

fn headless() -> HeadlessAppContext {
    let text = nd_gpui::gpui_platform::current_platform(true).text_system();
    HeadlessAppContext::with_platform(text, Arc::new(()), nd_gpui::gpui_platform::current_headless_renderer)
}

fn frames(cx: &mut HeadlessAppContext, host: &GpuiHost, n: usize) {
    for _ in 0..n {
        cx.run_until_parked();
        cx.update(|cx| {
            host.pump(cx);
            host.draw(cx);
        });
        std::thread::sleep(Duration::from_millis(4));
    }
}

type Fallback = Rc<RefCell<Vec<Vec<PathBuf>>>>;

struct Rig {
    cx: HeadlessAppContext,
    host: GpuiHost,
    window: WindowId,
    list: ListState<u32>,
    log: Log,
    fallback: Fallback,
    over: Over,
}

fn open() -> Rig {
    let mut cx = headless();
    let log: Log = Rc::default();
    let l = log.clone();
    let over: Over = Rc::default();
    let o = over.clone();
    let mut opened = None;
    let host = cx.update(|cx| {
        GpuiHost::builder().quit_when_empty(false).start(cx, |app| {
            let handle = app.open_window(WindowOptions::main("drops").size(size(px(500.), px(400.))), |_, cx| {
                cx.new(|cx| {
                    let mut list = ListState::new(cx);
                    list.set_items(0..8, cx);
                    Targets { list, log: l, over: o }
                })
            });
            opened = Some((handle.id(), handle.root().clone()));
        })
    });
    let (window, view) = opened.expect("window");
    let fallback: Fallback = Rc::default();
    let f = fallback.clone();
    host.backend().on_file_drop(move |_, paths, _| f.borrow_mut().push(paths));
    frames(&mut cx, &host, 6);
    let list = host.read(|app| view.read(app).list.clone());
    Rig { cx, host, window, list, log, fallback, over }
}

impl Rig {
    fn dispatch(&mut self, events: Vec<PlatformInput>) {
        let handle = self.host.window_handle(self.window).expect("gpui window");
        for e in events {
            self.cx.update(|cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    window.dispatch_event(e, cx);
                });
            });
            frames(&mut self.cx, &self.host, 2);
        }
    }

    /// The bounds of the node with `test_id`, or of the first node named `name` inside it.
    fn bounds(&self, test_id: &str, name: Option<&str>) -> nd_core::Rect {
        self.host.read(|app| {
            let tree = app.semantic_window(self.window).expect("window");
            let node = tree.root.iter().find(|n| n.test_id.as_deref() == Some(test_id)).expect(test_id);
            let node = match name {
                Some(name) => node.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("{name} in {node:#?}")),
                None => node,
            };
            node.bounds.expect("laid out")
        })
    }

    fn files(&mut self, paths: &[&str], to: (f32, f32)) {
        let paths = ExternalPaths(paths.iter().map(PathBuf::from).collect());
        let position = point(px(to.0), px(to.1));
        self.dispatch(vec![
            PlatformInput::FileDrop(FileDropEvent::Entered { position: point(px(2.), px(2.)), paths }),
            PlatformInput::FileDrop(FileDropEvent::Pending { position }),
            PlatformInput::FileDrop(FileDropEvent::Submit { position }),
        ]);
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.borrow_mut())
    }
}

/// Rows 24 tall under a 24 tall header.
fn row_y(table: nd_core::Rect, row: u32) -> f32 {
    table.origin.y + 24. + 24. * row as f32 + 12.
}

fn files_land_on_rows_items_and_tabs() {
    let mut r = open();
    let table = r.bounds("files", None);
    let x = table.origin.x + 40.;
    r.files(&["/tmp/a.txt", "/tmp/b.txt"], (x, row_y(table, 2)));
    assert_eq!(r.take(), [r#"row Some(2) ["/tmp/a.txt", "/tmp/b.txt"] alt false"#]);
    r.files(&["/tmp/a.txt"], (x, row_y(table, 3)));
    assert_eq!(r.take(), [r#"row None ["/tmp/a.txt"] alt false"#], "row 3 takes nothing on itself");

    // Source list rows have no semantic nodes: find Disk's band by moving down the list.
    let places = r.bounds("places", None);
    let x = places.origin.x + 30.;
    let paths = ExternalPaths(vec![PathBuf::from("/tmp/c.txt")].into());
    r.dispatch(vec![PlatformInput::FileDrop(FileDropEvent::Entered {
        position: point(px(x), px(places.origin.y)),
        paths,
    })]);
    let mut band = Vec::new();
    for step in 0..60 {
        let y = places.origin.y + 2. * step as f32;
        r.dispatch(vec![PlatformInput::FileDrop(FileDropEvent::Pending { position: point(px(x), px(y)) })]);
        if r.over.borrow().as_deref() == Some("disk") {
            band.push(y);
        }
    }
    r.dispatch(vec![PlatformInput::FileDrop(FileDropEvent::Exited)]);
    let (first, last) = (*band.first().expect("a band lands on Disk"), *band.last().expect("band"));
    assert!(last - first < 24., "only the row's middle lands on it: {band:?}");
    r.files(&["/tmp/c.txt"], (x, (first + last) / 2.));
    assert_eq!(r.take(), [r#"place Some("disk") ["/tmp/c.txt"]"#]);
    r.files(&["/tmp/c.txt"], (x, last + 6.));
    assert_eq!(r.take(), [r#"place None ["/tmp/c.txt"]"#], "past the middle it lands between rows");

    let strip = r.bounds("tabs", None);
    r.files(&["/tmp/d.txt"], (strip.origin.x + strip.size.width * 0.75, strip.origin.y + strip.size.height / 2.));
    assert_eq!(r.take(), [r#"tab Some("two") ["/tmp/d.txt"]"#]);
    assert!(r.fallback.borrow().is_empty(), "a drop a target took skips on_file_drop");
}

fn rows_drag_out_with_the_selection() {
    let mut r = open();
    let table = r.bounds("files", None);
    let x = table.origin.x + 40.;
    let press = |y: f32, modifiers: Modifiers| {
        let position = point(px(x), px(y));
        vec![
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position, modifiers, click_count: 1 }),
        ]
    };
    r.dispatch(press(row_y(table, 1), Modifiers::none()));
    r.dispatch(press(row_y(table, 5), Modifiers { shift: true, ..Modifiers::none() }));
    assert_eq!(r.list.selected(), vec![1, 2, 3, 4, 5]);

    let alt = Modifiers { alt: true, ..Modifiers::none() };
    let moved = |y: f32, modifiers: Modifiers| {
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x), px(y)),
            pressed_button: Some(MouseButton::Left),
            modifiers,
        })
    };
    let at = point(px(x), px(row_y(table, 6)));
    r.dispatch(vec![
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x), px(row_y(table, 3))),
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        }),
        moved(row_y(table, 3) + 12., Modifiers::none()),
        moved(row_y(table, 6), alt),
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: alt,
            click_count: 1,
        }),
    ]);
    assert_eq!(
        r.take(),
        [r#"row Some(6) ["file-1", "file-2", "file-3", "file-4", "file-5"] alt true"#],
        "pressing a selected row drags the whole selection"
    );
    assert_eq!(r.list.selected(), vec![1, 2, 3, 4, 5], "the drag changed no selection");

    r.dispatch(vec![
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x), px(row_y(table, 7))),
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        }),
        moved(row_y(table, 7) - 12., Modifiers::none()),
        moved(row_y(table, 0), Modifiers::none()),
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(x), px(row_y(table, 0))),
            modifiers: Modifiers::none(),
            click_count: 1,
        }),
    ]);
    assert_eq!(r.take(), [r#"row Some(0) ["file-7"] alt false"#]);
    assert_eq!(r.list.selected(), vec![7], "an unselected row is selected and drags alone");
}

/// A row drag that leaves the window offers the rows' files to the platform's drag, which then
/// holds the pointer: nd's drag runs on until the platform says its drag is over (a drop in
/// another app), and ends there with nothing in the app taking it. A drag that never left the
/// window offered nothing, so that end leaves it alone.
fn rows_carry_files_out_of_the_window() {
    let mut r = open();
    let table = r.bounds("files", None);
    let x = table.origin.x + 40.;
    let dragging = |r: &Rig| r.host.read(|app| app.is_dragging());
    let down = |y: f32| {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x), px(y)),
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        })
    };
    let moved = |x: f32, y: f32| {
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x), px(y)),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::none(),
        })
    };
    let up = |x: f32, y: f32| {
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(x), px(y)),
            modifiers: Modifiers::none(),
            click_count: 1,
        })
    };

    r.dispatch(vec![down(row_y(table, 1)), moved(x, row_y(table, 1) + 12.)]);
    assert!(dragging(&r), "the row drags");
    r.dispatch(vec![PlatformInput::FileDrop(FileDropEvent::Ended)]);
    assert!(dragging(&r), "a drag still inside the window handed nothing to the platform");
    r.dispatch(vec![up(x, row_y(table, 4))]);
    assert_eq!(r.take(), [r#"row Some(4) ["file-1"] alt false"#], "and drops inside the app as before");

    r.dispatch(vec![down(row_y(table, 2)), moved(x, row_y(table, 2) + 12.), moved(-20., row_y(table, 2))]);
    assert!(dragging(&r), "the drag runs on while the platform holds the pointer");
    r.dispatch(vec![PlatformInput::FileDrop(FileDropEvent::Ended)]);
    assert!(!dragging(&r), "the platform's drag ending ends it");
    assert!(r.take().is_empty(), "nothing in the app took it");
    // The test window refuses the platform drag, so GPUI still holds its own until a release.
    r.dispatch(vec![up(-20., row_y(table, 2))]);

    // Row 3 carries no file: leaving the window offers nothing, and the drag is the app's alone.
    r.dispatch(vec![down(row_y(table, 3)), moved(x, row_y(table, 3) + 12.), moved(-20., row_y(table, 3))]);
    r.dispatch(vec![PlatformInput::FileDrop(FileDropEvent::Ended)]);
    assert!(dragging(&r), "a drag with no files was never the platform's");
    r.dispatch(vec![up(-20., row_y(table, 3))]);
    assert!(!dragging(&r));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tests: Vec<(&str, fn())> = vec![
        ("gpui_drops::files_land_on_rows_items_and_tabs", files_land_on_rows_items_and_tabs),
        ("gpui_drops::rows_drag_out_with_the_selection", rows_drag_out_with_the_selection),
        ("gpui_drops::rows_carry_files_out_of_the_window", rows_carry_files_out_of_the_window),
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
    let filters: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let mut failed = 0;
    for (name, test) in &tests {
        if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        match std::panic::catch_unwind(*test) {
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
