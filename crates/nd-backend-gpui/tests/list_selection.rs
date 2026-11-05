//! Selection on the GPUI presenter under real pointer and key input, matching what NSTableView
//! and NSCollectionView select on macOS: Shift ranges, secondary-key toggles, a click on empty
//! space clearing, keys moving and extending, select all, a marquee from empty space, a right
//! click selecting the row under it, and a middle click reaching the row's own handler.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use nd_backend_gpui::GpuiHost;
use nd_components::forms::{TextCancelled, TextChanged, TextEditingEnded, text_field};
use nd_gpui::gpui::{
    HeadlessAppContext, KeyDownEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PlatformInput, point,
};
use nd_ui::WindowId;
use nd_ui::prelude::*;
use nd_virtual::{ListState, Selection, column, grid, table};

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

fn dispatch(cx: &mut HeadlessAppContext, host: &GpuiHost, window: WindowId, events: Vec<PlatformInput>) {
    let handle = host.window_handle(window).expect("gpui window");
    cx.update(|cx| {
        let _ = handle.update(cx, |_, window, cx| {
            for e in events {
                window.dispatch_event(e, cx);
            }
        });
    });
    frames(cx, host, 3);
}

fn press(button: MouseButton, x: f32, y: f32, modifiers: Modifiers) -> Vec<PlatformInput> {
    let position = point(px(x), px(y));
    vec![
        PlatformInput::MouseDown(MouseDownEvent { button, position, modifiers, click_count: 1, first_mouse: false }),
        PlatformInput::MouseUp(MouseUpEvent { button, position, modifiers, click_count: 1 }),
    ]
}

fn moved(x: f32, y: f32) -> PlatformInput {
    PlatformInput::MouseMove(MouseMoveEvent {
        position: point(px(x), px(y)),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    })
}

fn key(k: &'static str) -> Vec<PlatformInput> {
    let keystroke = Keystroke::parse(k).expect("keystroke");
    vec![PlatformInput::KeyDown(KeyDownEvent { keystroke, is_held: false, prefer_character_input: false })]
}

const SHIFT: Modifiers = Modifiers { shift: true, control: false, alt: false, platform: false, function: false };

fn secondary() -> Modifiers {
    if cfg!(target_os = "macos") { Modifiers::command() } else { Modifiers::control() }
}

fn open<V: Render>(
    build: impl FnOnce(&mut Context<V>) -> V + 'static,
) -> (HeadlessAppContext, GpuiHost, WindowId, Entity<V>) {
    let mut cx = headless();
    let mut opened = None;
    let host = cx.update(|cx| {
        GpuiHost::builder().quit_when_empty(false).start(cx, |app| {
            let handle =
                app.open_window(WindowOptions::main("selection").size(size(px(300.), px(300.))), |_, cx| cx.new(build));
            opened = Some((handle.id(), handle.root().clone()));
        })
    });
    let (window, view) = opened.expect("window");
    frames(&mut cx, &host, 6);
    (cx, host, window, view)
}

/// Rows 24 tall under a 24 tall header; row `i` is centred at `y(i)`.
struct Files {
    list: ListState<u32>,
    middle: Rc<RefCell<Vec<u32>>>,
}

fn y(row: u32) -> f32 {
    24. + 24. * row as f32 + 12.
}

impl Render for Files {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let middle = self.middle.clone();
        table("files", &self.list, vec![column("name", "Name").width(300.)], move |row: u32, _, _, _| {
            let middle = middle.clone();
            div().id(format!("row-{row}")).h(px(24.)).child(format!("file-{row}")).on_middle_click(move |_, _, _| {
                middle.borrow_mut().push(row);
            })
        })
        .selection(Selection::Multiple)
    }
}

type Middle = Rc<RefCell<Vec<u32>>>;

fn files(n: u32) -> (HeadlessAppContext, GpuiHost, WindowId, ListState<u32>, Middle) {
    let middle = Rc::new(RefCell::new(Vec::new()));
    let m = middle.clone();
    let (cx, host, window, view) = open(move |cx| {
        let mut list = ListState::new(cx);
        list.set_items(0..n, cx);
        Files { list, middle: m }
    });
    let list = host.read(|app| view.read(app).list.clone());
    (cx, host, window, list, middle)
}

pub fn shift_and_the_secondary_key_select_like_appkit() {
    let (mut cx, host, window, list, _) = files(10);
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(2), Modifiers::none()));
    assert_eq!(list.selected(), vec![2]);
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(5), SHIFT));
    assert_eq!(list.selected(), vec![2, 3, 4, 5], "shift selects the range from the anchor");
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(0), SHIFT));
    assert_eq!(list.selected(), vec![0, 1, 2], "a second shift click re-ranges from the same anchor");
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(7), secondary()));
    assert_eq!(list.selected(), vec![0, 1, 2, 7], "the secondary key adds one row");
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(9), secondary() | SHIFT));
    assert_eq!(list.selected(), vec![0, 1, 2, 7, 8, 9], "with shift it adds the range from the new anchor");
    // A plain press on a row of the selection keeps the group until the release, then selects
    // that row alone.
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(8), Modifiers::none()));
    assert_eq!(list.selected(), vec![8]);
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., 290., Modifiers::none()));
    assert_eq!(list.selected(), Vec::<u32>::new(), "a click on empty space clears the selection");
}

pub fn keys_move_extend_and_select_all() {
    let (mut cx, host, window, list, _) = files(20);
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(3), Modifiers::none()));
    dispatch(&mut cx, &host, window, key("down"));
    assert_eq!(list.selected(), vec![4]);
    dispatch(&mut cx, &host, window, key("shift-down"));
    dispatch(&mut cx, &host, window, key("shift-down"));
    assert_eq!(list.selected(), vec![4, 5, 6], "shift extends from the anchor");
    dispatch(&mut cx, &host, window, key("home"));
    assert_eq!(list.selected(), vec![0]);
    dispatch(&mut cx, &host, window, key("end"));
    assert_eq!(list.selected(), vec![19]);
    let (viewport, rows) = host.backend().painted_rows(*host.backend().lists().first().expect("list")).expect("rows");
    let last = list.row_of(&19).expect("row");
    let (_, b) = rows.iter().find(|(r, _)| *r == last).expect("the last row is painted after End");
    assert!(
        b.origin.y + b.size.height <= viewport.origin.y + viewport.size.height + 0.5,
        "End scrolled the last row into view: {b:?} in {viewport:?}"
    );
    dispatch(&mut cx, &host, window, key(if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" }));
    assert_eq!(list.selected(), (0..20).collect::<Vec<_>>());
}

/// A table whose rows drag their files out of the window, as a file manager's do.
struct Carrying {
    list: ListState<u32>,
}

impl Render for Carrying {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        table("files", &self.list, vec![column("name", "Name").width(300.)], |row: u32, _, _, _| {
            div().h(px(24.)).child(format!("file-{row}"))
        })
        .selection(Selection::Multiple)
        .drag_rows("files", |row: &u32| Some(format!("file-{row}").into()))
        .drag_out_files(|row: &u32| Some(std::path::PathBuf::from(format!("/tmp/file-{row}"))))
    }
}

/// Each row's drag-out handle is a child the list paints; the keys still reach the list.
pub fn keys_reach_a_list_whose_rows_drag_files_out() {
    let (mut cx, host, window, view) = open(|cx| {
        let mut list = ListState::new(cx);
        list.set_items(0..10, cx);
        Carrying { list }
    });
    let list = host.read(|app| view.read(app).list.clone());
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(3), Modifiers::none()));
    assert_eq!(list.selected(), vec![3]);
    dispatch(&mut cx, &host, window, key("down"));
    assert_eq!(list.selected(), vec![4], "Down moves the selection");
    dispatch(&mut cx, &host, window, key("shift-down"));
    assert_eq!(list.selected(), vec![4, 5], "Shift+Down extends it");
}

/// A list is a focus target: `window.focus` reaches it and keys move its selection, and a press
/// keeps nd focus on it. Its selection is the focus mark, as in Finder: a row the keyboard
/// selected looks the same as one a press selected, with no ring round it or the list.
pub fn a_list_takes_focus_by_id_and_its_selection_marks_it() {
    let (mut cx, host, window, list, _) = files(10);
    assert!(cx.update(|cx| host.update(cx, |app| app.focus_id(window, "files"))), "the list takes focus by id");
    frames(&mut cx, &host, 3);
    let node = *host.backend().lists().first().expect("list");
    assert_eq!(host.read(|app| app.focused(window)), Some(node));
    dispatch(&mut cx, &host, window, key("down"));
    assert_eq!(list.selected(), vec![0], "keys reach the list focused from code");

    let shot = |cx: &mut HeadlessAppContext| {
        let handle = host.window_handle(window).expect("gpui window");
        cx.capture_screenshot(handle).expect("screenshot")
    };
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(4), Modifiers::none()));
    assert_eq!(host.read(|app| app.focused(window)), Some(node), "a press keeps nd focus on the list");
    let pointer = shot(&mut cx);
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(3), Modifiers::none()));
    dispatch(&mut cx, &host, window, key("down"));
    assert_eq!(list.selected(), vec![4]);
    let keyboard = shot(&mut cx);
    let differ = keyboard.enumerate_pixels().filter(|(x, y, p)| *p != pointer.get_pixel(*x, *y)).count();
    assert_eq!(differ, 0, "the keyboard's row is drawn as the pointer's, with no ring");
}

pub fn a_marquee_from_empty_space_selects_what_it_sweeps() {
    let (mut cx, host, window, view) = open(|cx| {
        let mut list = ListState::new(cx);
        list.set_items(0..6, cx);
        Grid { list }
    });
    let list = host.read(|app| view.read(app).list.clone());
    // Two lanes of 100x60 cells with an 8pt gap: the gap at x 100..108 is empty space.
    let down = PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: point(px(104.), px(4.)),
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    dispatch(&mut cx, &host, window, vec![down, moved(150., 90.)]);
    assert_eq!(list.selected(), vec![1, 3], "the band from the gap into the second lane over two lines");
    dispatch(&mut cx, &host, window, vec![moved(50., 90.)]);
    assert_eq!(list.selected(), vec![0, 2], "the band follows the pointer back across the gap");
    let up = PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position: point(px(50.), px(90.)),
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    dispatch(&mut cx, &host, window, vec![up, moved(150., 200.)]);
    assert_eq!(list.selected(), vec![0, 2], "a release ends the marquee");
}

/// A marquee held past the bottom edge scrolls the grid and keeps sweeping.
pub fn a_marquee_past_the_edge_scrolls() {
    let (mut cx, host, window, view) = open(|cx| {
        let mut list = ListState::new(cx);
        list.set_items(0..60, cx);
        Grid { list }
    });
    let list = host.read(|app| view.read(app).list.clone());
    let down = PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: point(px(104.), px(4.)),
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    dispatch(&mut cx, &host, window, vec![down, moved(150., 320.)]);
    frames(&mut cx, &host, 30);
    let swept = list.selected();
    assert!(swept.iter().all(|k| k % 2 == 1), "the second lane only: {swept:?}");
    assert!(swept.iter().any(|k| *k > 9), "the band reached cells below the first viewport: {swept:?}");
    let (viewport, rows) = host.backend().painted_rows(*host.backend().lists().first().expect("list")).expect("rows");
    let first = list.row_of(&0).expect("row");
    assert!(
        rows.iter().all(|(r, b)| *r != first || b.origin.y < viewport.origin.y),
        "the grid scrolled under the held band"
    );
}

struct Grid {
    list: ListState<u32>,
}

impl Render for Grid {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        grid("tiles", &self.list, (px(100.), px(60.)), |row: u32, _, _| div().size_full().child(format!("tile-{row}")))
            .selection(Selection::Multiple)
    }
}

pub fn a_right_click_selects_the_row_and_a_middle_click_reaches_it() {
    let (mut cx, host, window, list, middle) = files(10);
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(1), Modifiers::none()));
    dispatch(&mut cx, &host, window, press(MouseButton::Left, 100., y(2), SHIFT));
    dispatch(&mut cx, &host, window, press(MouseButton::Right, 100., y(2), Modifiers::none()));
    assert_eq!(list.selected(), vec![1, 2], "a right click inside the selection keeps it");
    dispatch(&mut cx, &host, window, press(MouseButton::Right, 100., y(6), Modifiers::none()));
    assert_eq!(list.selected(), vec![6], "a right click outside it selects that row alone");
    dispatch(&mut cx, &host, window, press(MouseButton::Middle, 100., y(4), Modifiers::none()));
    assert_eq!(*middle.borrow(), vec![4]);
    assert_eq!(list.selected(), vec![6], "a middle click does not select");
}

type Log = Rc<RefCell<Vec<String>>>;

/// A table whose row 2 is being renamed: a text field in its cell, as a file manager opens one.
struct Rename {
    list: ListState<u32>,
    log: Log,
}

impl Render for Rename {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let log = self.log.clone();
        table("files", &self.list, vec![column("name", "Name").width(300.)], move |row: u32, _, _, _| {
            if row != 2 {
                return div().h(px(24.)).child(format!("file-{row}")).into_any_element();
            }
            let (changed, ended, cancelled) = (log.clone(), log.clone(), log.clone());
            text_field("rename")
                .text("notes.txt")
                .select_on_focus(0..5)
                .on_change(move |e: &TextChanged, _, _| changed.borrow_mut().push(format!("text:{}", e.text)))
                .on_end_editing(move |e: &TextEditingEnded, _, _| ended.borrow_mut().push(format!("end:{}", e.text)))
                .on_cancel(move |_: &TextCancelled, _, _| cancelled.borrow_mut().push("cancel".into()))
                .into_any_element()
        })
    }
}

fn keys(cx: &mut HeadlessAppContext, host: &GpuiHost, window: WindowId, keys: &[&'static str]) {
    cx.update(|cx| {
        host.update(cx, |app| {
            for k in keys {
                let key = nd_ui::semantics::SyntheticInput::Key(Keystroke::parse(*k).expect("keystroke"));
                app.synthesize(window, &key).expect("posted");
            }
        })
    });
    frames(cx, host, 3);
}

/// Inline rename in a table cell: focused from code with the stem selected, typing replaces
/// the stem, Return commits once, Escape abandons without a commit on the blur after it, and
/// focus leaving commits.
pub fn a_cell_field_renames_inline() {
    let log: Log = Rc::default();
    let l = log.clone();
    let (mut cx, host, window, _) = open(move |cx| {
        let mut list = ListState::new(cx);
        list.set_items(0..5, cx);
        Rename { list, log: l }
    });
    let focus = |cx: &mut HeadlessAppContext| {
        assert!(cx.update(|cx| host.update(cx, |app| app.focus_id(window, "rename"))), "the cell field takes focus");
        frames(cx, &host, 3);
    };
    let blur = |cx: &mut HeadlessAppContext| {
        cx.update(|cx| host.update(cx, |app| app.blur(window)));
        frames(cx, &host, 3);
    };
    focus(&mut cx);
    keys(&mut cx, &host, window, &["x"]);
    assert_eq!(std::mem::take(&mut *log.borrow_mut()), ["text:x.txt"], "typing replaced the selected stem");
    keys(&mut cx, &host, window, &["enter"]);
    assert_eq!(std::mem::take(&mut *log.borrow_mut()), ["end:x.txt"]);
    blur(&mut cx);
    assert!(log.borrow().is_empty(), "Return already ended the edit: {:?}", log.borrow());

    focus(&mut cx);
    keys(&mut cx, &host, window, &["escape"]);
    blur(&mut cx);
    assert_eq!(std::mem::take(&mut *log.borrow_mut()), ["cancel"], "no commit after Escape");

    focus(&mut cx);
    keys(&mut cx, &host, window, &["end", "s"]);
    blur(&mut cx);
    assert_eq!(std::mem::take(&mut *log.borrow_mut()), ["text:x.txts", "end:x.txts"], "focus leaving commits");

    // A click into the field edits it rather than handing the keys to the list.
    let field = host.read(|app| {
        let tree = app.semantic_window(window).expect("window");
        tree.root.iter().find(|n| n.test_id.as_deref() == Some("rename")).and_then(|n| n.bounds).expect("field")
    });
    let (x, y) = (field.origin.x + field.size.width - 4., field.origin.y + field.size.height / 2.);
    dispatch(&mut cx, &host, window, press(MouseButton::Left, x, y, Modifiers::none()));
    keys(&mut cx, &host, window, &["end", "2"]);
    assert_eq!(std::mem::take(&mut *log.borrow_mut()), ["text:x.txts2"]);
}

fn main() {
    let tests: [(&str, fn()); 8] = [
        ("gpui_selection::keys_reach_a_list_whose_rows_drag_files_out", keys_reach_a_list_whose_rows_drag_files_out),
        ("gpui_selection::a_cell_field_renames_inline", a_cell_field_renames_inline),
        (
            "gpui_selection::a_list_takes_focus_by_id_and_its_selection_marks_it",
            a_list_takes_focus_by_id_and_its_selection_marks_it,
        ),
        ("gpui_selection::a_marquee_past_the_edge_scrolls", a_marquee_past_the_edge_scrolls),
        (
            "gpui_selection::shift_and_the_secondary_key_select_like_appkit",
            shift_and_the_secondary_key_select_like_appkit,
        ),
        ("gpui_selection::keys_move_extend_and_select_all", keys_move_extend_and_select_all),
        (
            "gpui_selection::a_marquee_from_empty_space_selects_what_it_sweeps",
            a_marquee_from_empty_space_selects_what_it_sweeps,
        ),
        (
            "gpui_selection::a_right_click_selects_the_row_and_a_middle_click_reaches_it",
            a_right_click_selects_the_row_and_a_middle_click_reaches_it,
        ),
    ];
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        if !args.iter().any(|a| a == "--ignored") {
            for (n, _) in &tests {
                println!("{n}: test");
            }
        }
        return;
    }
    if args.iter().any(|a| a == "--ignored") {
        return;
    }
    let filters: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let mut failed = 0;
    for (name, test) in tests {
        if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
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
