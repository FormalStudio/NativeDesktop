//! Tier 2: drag and drop onto the parts of a window in GPUI's headless app. Rows dragged out of
//! a table carry the selection, and their files once they leave the window; files dragged in
//! from outside land on a table row, a source list item or a tab, each named in what the app
//! receives, and skip `on_file_drop`. A drag over a list marks the slot between rows it would drop
//! into: `tests/goldens/<os>/list-drop-slot-light.png` (`ND_UPDATE_GOLDENS=1` rewrites it).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use nd_backend_gpui::{GpuiBackend, GpuiHost};
use nd_components::{TabStyle, TileMoved, source_item, source_list, tab_strip, tile, tile_grid};
use nd_gpui::gpui::{
    ExternalPaths, FileDropEvent, HeadlessAppContext, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput, point,
};
use nd_theme::{OsTheme, Preset, Scheme, ThemeRegistry};
use nd_ui::prelude::*;
use nd_ui::{DragItem, DragOver, Dropped, WindowId};
use nd_virtual::{ListState, RowDropped, Selection, column, list, table};

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
                        .drag_rows("file", |k| Some(format!("file-{k}").into()))
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
    let (down, up) = {
        let mut both = press(row_y(table, 1), Modifiers::none());
        let up = both.pop().expect("release");
        (both, up)
    };
    r.dispatch(down);
    assert!(r.list.selected().is_empty(), "a press on a row that drags selects nothing yet");
    r.dispatch(vec![up]);
    assert_eq!(r.list.selected(), vec![1], "the release selects it");
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
    assert_eq!(r.list.selected(), vec![1, 2, 3, 4, 5], "an unselected row drags alone and leaves the selection");
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

/// Three rows in a sidebar's list that takes files.
struct Slots {
    list: ListState<u32>,
    log: Log,
}

impl Render for Slots {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let log = self.log.clone();
        div().size_full().child(
            list("slots", &self.list, |row: u32, _, _| div().h(px(32.)).px(px(12.)).child(format!("Tab {row}")))
                .inset()
                .accepts_drops([DragItem::FILES])
                .on_drop(move |e: &RowDropped<u32>, _, _| {
                    log.borrow_mut().push(format!("{:?} at {}", e.key, e.index));
                }),
        )
    }
}

/// Per channel, out of 255, and the share of pixels allowed past it: antialiasing drifts.
const CHANNEL_TOLERANCE: i16 = 24;
const PIXEL_TOLERANCE: f64 = 0.004;

fn golden(image: &image::RgbaImage, name: &str) -> Result<(), String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/goldens/{}/{name}.png", std::env::consts::OS));
    if std::env::var_os("ND_UPDATE_GOLDENS").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().expect("dir")).map_err(|e| e.to_string())?;
        image.save(&path).map_err(|e| e.to_string())?;
        println!("recorded {}", path.display());
        return Ok(());
    }
    let actual_path = path.with_extension("actual.png");
    if !path.exists() {
        let _ = image.save(&actual_path);
        return Err(format!("{} is missing (record it with ND_UPDATE_GOLDENS=1)", path.display()));
    }
    let expected = image::open(&path).map_err(|e| e.to_string())?.to_rgba8();
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

/// The lower half of a row is the slot after it: the line sits between it and the next, and the
/// drop reports that slot with the row it was over.
fn drag_over_a_list_marks_the_slot() {
    let mut cx = headless();
    let log: Log = Rc::default();
    let l = log.clone();
    let mut opened = None;
    let host = cx.update(|cx| {
        GpuiHost::builder().quit_when_empty(false).start(cx, |app| {
            let backend = app.backend_as::<GpuiBackend>().expect("GPUI backend").clone();
            let mut registry = ThemeRegistry::new(Preset::Adwaita);
            let os =
                OsTheme { scheme: Some(Scheme::Light), fonts: nd_theme::fonts::resolve(), ..registry.os().clone() };
            registry.set_os(os);
            backend.set_theme(registry);
            let handle = app.open_window(WindowOptions::main("slots").size(size(px(240.), px(140.))), |_, cx| {
                cx.new(|cx| {
                    let mut list = ListState::new(cx);
                    list.set_items(0..3, cx);
                    Slots { list, log: l }
                })
            });
            opened = Some((handle.id(), handle.root().clone()));
        })
    });
    frames(&mut cx, &host, 6);
    let (window, view) = opened.expect("window");
    let list = host.read(|app| view.read(app).list.clone());
    let mut r = Rig { cx, host, window, list, log, fallback: Rc::default(), over: Rc::default() };
    let slots = r.bounds("slots", None);
    let (x, y) = (slots.origin.x + 40., slots.origin.y + 24.);
    let paths = ExternalPaths(vec![PathBuf::from("/tmp/a.txt")].into());
    r.dispatch(vec![
        PlatformInput::FileDrop(FileDropEvent::Entered { position: point(px(x), px(y)), paths }),
        PlatformInput::FileDrop(FileDropEvent::Pending { position: point(px(x), px(y)) }),
    ]);
    if nd_gpui::gpui_platform::current_headless_renderer().ok().flatten().is_some() {
        let handle = r.host.window_handle(r.window).expect("gpui window");
        let image = r.cx.capture_screenshot(handle).expect("capture");
        if let Err(e) = golden(&image, "list-drop-slot-light") {
            panic!("{e}");
        }
    }
    r.dispatch(vec![PlatformInput::FileDrop(FileDropEvent::Submit { position: point(px(x), px(y)) })]);
    assert_eq!(r.take(), ["Some(0) at 1"]);
    r.files(&["/tmp/a.txt"], (x, slots.origin.y + 40.));
    assert_eq!(r.take(), ["Some(1) at 1"], "the upper half of a row is the slot before it");
    r.files(&["/tmp/a.txt"], (x, slots.origin.y + 120.));
    assert_eq!(r.take(), ["None at 3"], "past the last row");
}

struct Pins {
    order: Vec<&'static str>,
}

impl Render for Pins {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tiles = self.order.iter().map(|id| {
            let face = div()
                .id(format!("pin-{id}"))
                .role(nd_core::semantic::Role::Button, *id)
                .size_full()
                .active(|pose| pose.scale(0.96))
                .on_tap(|_: &TapEvent, _, _| {});
            tile(*id, face)
        });
        div().size_full().child(tile_grid("pinned").max_columns(3).tiles(tiles).draggable_tiles("tab").on_move(
            cx.listener(|this, e: &TileMoved, _, cx| {
                let at = this.order.iter().position(|i| *i == e.id.as_ref()).expect("a tile of the grid");
                let id = this.order.remove(at);
                this.order.insert(e.index.min(this.order.len()), id);
                cx.notify();
            }),
        ))
    }
}

/// A tile held again right where the last drop let it go, as soon as it lands: the press is the
/// second of a double click on Wayland, and the tile under the pointer still makes room.
fn a_tile_lifts_again_where_it_was_dropped() {
    let mut cx = headless();
    let mut opened = None;
    let host = cx.update(|cx| {
        GpuiHost::builder().quit_when_empty(false).start(cx, |app| {
            let handle = app.open_window(WindowOptions::main("pins").size(size(px(300.), px(160.))), |_, cx| {
                cx.new(|_| Pins { order: vec!["a", "b", "c"] })
            });
            opened = Some(handle.id());
        })
    });
    frames(&mut cx, &host, 8);
    let window = opened.expect("window");
    let dispatch = |cx: &mut HeadlessAppContext, events: Vec<PlatformInput>| {
        let handle = host.window_handle(window).expect("gpui window");
        for e in events {
            cx.update(|cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    window.dispatch_event(e, cx);
                });
            });
            frames(cx, &host, 1);
        }
        // The tiles slide on the frame clock; let them land.
        frames(cx, &host, 120);
    };
    let centre = |id: &str| {
        host.read(|app| {
            let tree = app.semantic_window(window).expect("window");
            let id = format!("pin-{id}");
            let b =
                tree.root.iter().find(|n| n.test_id.as_deref() == Some(&id)).and_then(|n| n.bounds).expect("a tile");
            (b.origin.x + b.size.width / 2., b.origin.y + b.size.height / 2.)
        })
    };
    let slots: Vec<f32> = ["a", "b", "c"].iter().map(|id| centre(id).0).collect();
    let lift = |cx: &mut HeadlessAppContext, from: (f32, f32), to: (f32, f32), clicks: usize| {
        let mut events = vec![PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(from.0), px(from.1)),
            modifiers: Modifiers::none(),
            click_count: clicks,
            first_mouse: false,
        })];
        for step in 1..=12 {
            let f = step as f32 / 12.;
            events.push(PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(from.0 + (to.0 - from.0) * f), px(from.1 + (to.1 - from.1) * f)),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::none(),
            }));
        }
        dispatch(cx, events);
    };
    let release = |cx: &mut HeadlessAppContext, at: (f32, f32), clicks: usize| {
        dispatch(
            cx,
            vec![PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(at.0), px(at.1)),
                modifiers: Modifiers::none(),
                click_count: clicks,
            })],
        );
    };

    let (c, a) = (centre("c"), centre("a"));
    lift(&mut cx, c, a, 1);
    assert_eq!((centre("a").0, centre("b").0), (slots[1], slots[2]), "a and b made room for c");
    release(&mut cx, a, 1);
    assert_eq!(centre("c").0, slots[0], "the drop kept c first");

    let b = centre("b");
    lift(&mut cx, a, b, 2);
    assert_eq!(centre("b").0, slots[1], "b made room for c held a second time");
    release(&mut cx, b, 2);
    assert_eq!(["a", "b", "c"].map(|id| centre(id).0), [slots[0], slots[1], slots[2]], "the second drop put c last");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tests: Vec<(&str, fn())> = vec![
        ("gpui_drops::files_land_on_rows_items_and_tabs", files_land_on_rows_items_and_tabs),
        ("gpui_drops::rows_drag_out_with_the_selection", rows_drag_out_with_the_selection),
        ("gpui_drops::rows_carry_files_out_of_the_window", rows_carry_files_out_of_the_window),
        ("gpui_drops::drag_over_a_list_marks_the_slot", drag_over_a_list_marks_the_slot),
        ("gpui_drops::a_tile_lifts_again_where_it_was_dropped", a_tile_lifts_again_where_it_was_dropped),
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
