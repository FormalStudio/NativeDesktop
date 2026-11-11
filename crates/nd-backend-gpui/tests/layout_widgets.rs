//! Tier 2: the drawn layout and window widgets on the GPUI backend inside GPUI's headless app:
//! the split's divider drag, `visibility: Hidden`, the status page, drawn alerts and the about
//! panel, with input through the window's own pointer and key paths; a window's chrome active
//! and inactive; and their goldens, light and dark, captured from the host's own window.
//!
//! `ND_UPDATE_GOLDENS=1` rewrites the goldens: `tests/goldens/<os>/layout-<view>-<scheme>.png`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use nd_backend_gpui::{GpuiBackend, GpuiChrome, GpuiHost, OsHooks};
use nd_components::services::{self, Chrome};
use nd_components::split::Axis;
use nd_components::*;
use nd_gpui::gpui::HeadlessAppContext;
use nd_theme::{GlassVariant, OsTheme, Preset, Scheme, Theme, ThemeRegistry};
use nd_ui::backend::{Backend, WindowCommand};
use nd_ui::prelude::*;
use nd_ui::semantics::{MouseButton, PaintKind, PointerPhase, SyntheticInput};
use nd_ui::{Keystroke, Point, SemanticNode, TitlebarInsets, WindowId};

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

fn find<'a>(node: &'a SemanticNode, f: &impl Fn(&SemanticNode) -> bool) -> Option<&'a SemanticNode> {
    if f(node) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, f))
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Split,
    /// A resizable sidebar and inspector, their widths the app's.
    Resizable,
    /// Rich text in rows too narrow for it, and under a parent that sizes its text.
    RichRows,
    Hidden,
    Status,
    /// Nothing but a label, for the dialogs.
    Plain,
    /// A toolbar: its own title or none, and an app item before the flexible space or none.
    Bar {
        title: Option<&'static str>,
        leading_item: bool,
    },
    /// A toolbar with a centred item that takes taps, a short leading run and a long trailing one.
    Centered,
    /// A window's chrome: a titled toolbar over a sidebar with a source list and the content.
    Chrome,
    /// A sidebar whose list overflows, with Trash pinned in its footer.
    Footer,
    /// The same with a list that fits, its first row selected.
    FittingFooter,
    /// Notes' toolbar: the sidebar toggle, a button, a flexible space and a search field.
    Search,
    /// The same with the search field kept in the header.
    PersistentSearch,
    /// A path bar too long for the header between a sidebar and an inspector, as a flexible
    /// item before two buttons.
    FlexibleItem,
    /// `glass()` pills, regular and clear, over black, white, red and green stripes they blur.
    Glass,
    /// A settings window: a toolbar of tabs over a padded body holding a form longer than the
    /// window, its rows holding text fields, steppers, combo boxes and pickers.
    Settings,
}

#[derive(Default)]
struct Log {
    ratios: Vec<f32>,
    clicks: Vec<&'static str>,
    answers: Vec<usize>,
    widths: Vec<(&'static str, f32)>,
    /// What the scaffold's content reported to `.on_resize`.
    content_sizes: Vec<(f32, f32)>,
}

struct Scene {
    kind: Kind,
    log: Rc<RefCell<Log>>,
    ratio: f32,
    /// The app keeps its ratio whatever the user drags.
    refuse: bool,
    search: Option<Entity<TextFieldState>>,
    widths: (f32, f32),
}

impl Render for Scene {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = v_flex().size_full();
        match self.kind {
            Kind::Split => root.child(
                div().w_full().h(px(300.)).child(
                    split("split", Axis::Horizontal)
                        .label("Panes")
                        .ratio(self.ratio)
                        .first_pane(split_pane().min_size(px(120.)).child(text("Notes")))
                        .second_pane(split_pane().min_size(px(200.)).child(text("Editor")))
                        .on_resize(cx.listener(|this, e: &SplitResized, _, cx| {
                            this.log.borrow_mut().ratios.push(e.ratio);
                            if !this.refuse {
                                this.ratio = e.ratio;
                            }
                            cx.notify();
                        })),
                ),
            ),
            Kind::Hidden => root.child(
                div()
                    .relative()
                    .w(px(300.))
                    .h(px(200.))
                    .child(button("under", "Under").on_click(cx.listener(|this, _, _, _| {
                        this.log.borrow_mut().clicks.push("under");
                    })))
                    .child(div().id("page").absolute().top_0().left_0().size_full().invisible().child(
                        button("over", "Over").on_click(cx.listener(|this, _, _, _| {
                            this.log.borrow_mut().clicks.push("over");
                        })),
                    )),
            ),
            Kind::Status => root.child(
                status_page("No notes")
                    .id("status")
                    .icon(Icon::Search)
                    .description("Notes you create show up here. Try another search, or start a new note.")
                    .child(button("new", "New Note")),
            ),
            Kind::Plain => root.child(text("Inbox")),
            Kind::Glass => {
                let stripes = [0x000000, 0xffffff, 0xff0000, 0xffffff, 0x000000, 0x00cc00, 0xffffff, 0x000000];
                let pill =
                    |variant| glass().variant(variant).rounded_full().px_4().py_2().w(px(220.)).child(text("On glass"));
                root.child(
                    div()
                        .relative()
                        .size_full()
                        .flex()
                        .children(stripes.map(|c| div().flex_1().h_full().bg(rgb(c))))
                        .child(
                            v_flex()
                                .absolute()
                                .top(px(120.))
                                .left(px(240.))
                                .gap(px(40.))
                                .child(pill(GlassVariant::Regular))
                                .child(pill(GlassVariant::Clear)),
                        ),
                )
            }
            Kind::RichRows => {
                let name = || {
                    rich_text([
                        span("quarterly report "),
                        span("draft").semibold(),
                        span(" with every appendix and the notes"),
                    ])
                };
                root.child(
                    h_flex()
                        .id("row")
                        .w(px(200.))
                        .gap(px(4.))
                        .child(div().w(px(16.)).h(px(16.)))
                        .child(name().id("rich").truncate())
                        .child(text("tail").id("tail")),
                )
                .child(div().text_size(px(20.)).child(rich_text([span("big "), span("type").bold()]).id("big")))
            }
            Kind::Centered => {
                let log = self.log.clone();
                let bar = toolbar()
                    .sidebar_toggle()
                    .item(button("new", "New"))
                    .flexible_space()
                    .centered_item(div().id("who").w(px(120.)).h(px(28.)).on_tap(move |_, _, _| {
                        log.borrow_mut().clicks.push("who");
                    }))
                    .flexible_space()
                    .item(button("catch-up", "Catch Me Up"))
                    .item(button("call", "Call"));
                return scaffold().toolbar(bar).content(text("Body")).into_any_element();
            }
            Kind::Search | Kind::PersistentSearch => {
                let search = self.search.get_or_insert_with(|| cx.new(|_| TextFieldState::default())).clone();
                let field = search_field("find", &search).w(px(220.));
                let bar = toolbar().title("Notes").sidebar_toggle().item(button("new", "New")).flexible_space();
                let bar =
                    if matches!(self.kind, Kind::Search) { bar.item(field) } else { bar.persistent_search(field) };
                return scaffold()
                    .toolbar(bar)
                    .sidebar(sidebar().min_w(px(220.)).child(text("Folders")))
                    .content(text("Body"))
                    .into_any_element();
            }
            Kind::FlexibleItem => {
                let path = div()
                    .id("path")
                    .min_w(px(80.))
                    .flex_1()
                    .overflow_hidden()
                    .child(text("Home / Projects / NativeDesktop / crates / nd-backend-gpui / tests / fixtures"));
                let bar = toolbar()
                    .hide_title()
                    .navigation(button("back", "Back"))
                    .flexible_item(path)
                    .item(button("new", "New"))
                    .item(button("info", "Info"));
                return scaffold()
                    .toolbar(bar)
                    .sidebar(sidebar().width(px(220.)).child(div().id("folders").child(text("Folders"))))
                    .content(text("Body"))
                    .inspector(inspector().width(px(260.)).child(div().id("details").child(text("Info"))))
                    .into_any_element();
            }
            Kind::Resizable => {
                let (sidebar_w, inspector_w) = self.widths;
                return scaffold()
                    .sidebar(sidebar().resizable().width(px(sidebar_w)).child(text("Folders")).on_resize(cx.listener(
                        |this, e: &PaneResized, _, cx| {
                            this.log.borrow_mut().widths.push(("sidebar", e.width));
                            if !this.refuse {
                                this.widths.0 = e.width;
                            }
                            cx.notify();
                        },
                    )))
                    .content(div().size_full().child(text("Body")).on_resize({
                        let log = self.log.clone();
                        move |e, _, _| log.borrow_mut().content_sizes.push((e.width, e.height))
                    }))
                    .inspector(
                        inspector().resizable().width(px(inspector_w)).min_w(px(200.)).child(text("Info")).on_resize(
                            cx.listener(|this, e: &PaneResized, _, cx| {
                                this.log.borrow_mut().widths.push(("inspector", e.width));
                                this.widths.1 = e.width;
                                cx.notify();
                            }),
                        ),
                    )
                    .into_any_element();
            }
            Kind::Chrome => {
                let folders = source_list("folders")
                    .section(
                        "Library",
                        [
                            source_item("inbox", "Inbox").icon(Icon::Inbox).badge("12"),
                            source_item("work", "Work").icon(Icon::Folder),
                        ],
                    )
                    .selected(SharedString::from("inbox"));
                return scaffold()
                    .toolbar(
                        toolbar().title("Notes").sidebar_toggle().flexible_space().item(button("search", "Search")),
                    )
                    .sidebar(sidebar().min_w(px(220.)).child(folders))
                    .content(text("Body"))
                    .into_any_element();
            }
            Kind::Footer | Kind::FittingFooter => {
                let rows = (0..if self.kind == Kind::Footer { 40 } else { 3 })
                    .map(|i| source_node(format!("note{i}"), format!("Note {i}")));
                return scaffold()
                    .sidebar(
                        sidebar()
                            .child(
                                source_tree("tree")
                                    .nodes(rows)
                                    .selected(SharedString::from("note0"))
                                    .flex_1()
                                    .min_h_0(),
                            )
                            .footer(source_list("trash").nodes([source_node("trash", "Trash").badge("1")]).fit_rows()),
                    )
                    .content(text("Body"))
                    .into_any_element();
            }
            Kind::Settings => {
                let tabs = toolbar()
                    .tab("general", "General", Icon::Settings)
                    .tab("assistant", "Assistant", Icon::Lightbulb)
                    .tab("account", "Account", Icon::User)
                    .selected_tab("assistant");
                let sections = (0..6).map(|i| {
                    form_section()
                        .title(format!("Section {i}"))
                        .child(
                            form_row("Folder")
                                .help("Where the notes go.")
                                .child(text_field(format!("text{i}")).w(px(220.))),
                        )
                        .child(form_row("Minutes").child(stepper(format!("stepper{i}"), 5.).range(0.0..=60.)))
                        .child(form_row("Model").child(combo_box(format!("combo{i}"), ["One", "Two"]).w(px(170.))))
                        .child(
                            form_row("Theme")
                                .id(format!("last{i}"))
                                .child(select(format!("select{i}"), ["System", "Dark"])),
                        )
                });
                return div()
                    .size_full()
                    .child(tabs)
                    .child(div().id("body").size_full().p(px(24.)).child(form().children(sections)))
                    .into_any_element();
            }
            Kind::Bar { title, leading_item } => {
                let bar = toolbar().sidebar_toggle();
                let bar = match title {
                    Some(title) => bar.title(title),
                    None => bar,
                };
                let bar = if leading_item { bar.item(button("new", "New")) } else { bar };
                return scaffold()
                    .toolbar(bar.flexible_space().item(button("search", "Search")))
                    .content(text("Body"))
                    .into_any_element();
            }
        }
        .into_any_element()
    }
}

struct Opened {
    cx: HeadlessAppContext,
    host: GpuiHost,
    window: WindowId,
    log: Rc<RefCell<Log>>,
    view: Entity<Scene>,
}

fn registry(scheme: Scheme) -> ThemeRegistry {
    registry_in(Preset::Adwaita, scheme)
}

fn registry_in(preset: Preset, scheme: Scheme) -> ThemeRegistry {
    // The GPUI look on every OS, in the OS's own fonts, as `nd` sets it up at launch.
    let mut registry = ThemeRegistry::new(preset);
    let os = OsTheme { scheme: Some(scheme), fonts: nd_theme::fonts::resolve(), ..registry.os().clone() };
    registry.set_os(os);
    registry
}

fn open_with(kind: Kind, scheme: Scheme) -> Opened {
    open_in(kind, Preset::Adwaita, scheme)
}

/// Window controls drawn at the end of the title row, as a floating window's under client-side
/// decorations.
struct EndControls;

impl OsHooks for EndControls {
    fn titlebar_insets(&self, _: WindowId, _: &nd_gpui::gpui::Window) -> Option<TitlebarInsets> {
        Some(TitlebarInsets { leading: 0., trailing: 80., height: 46. })
    }
}

fn open_in(kind: Kind, preset: Preset, scheme: Scheme) -> Opened {
    open_hooked(kind, preset, scheme, false, 800.)
}

fn open_hooked(kind: Kind, preset: Preset, scheme: Scheme, end_controls: bool, width: f32) -> Opened {
    let mut cx = headless();
    let log: Rc<RefCell<Log>> = Rc::default();
    let l = log.clone();
    let mut opened = None;
    let host = cx.update(|cx| {
        let builder = GpuiHost::builder().quit_when_empty(false);
        let builder = if end_controls { builder.hooks(EndControls) } else { builder };
        builder.start(cx, |app| {
            let backend = app.backend_as::<GpuiBackend>().expect("GPUI backend").clone();
            backend.set_theme(registry_in(preset, scheme));
            app.set_global(Chrome(GpuiChrome::new(backend)));
            let options = WindowOptions::main("layout").size(size(px(width), px(560.)));
            let options = match kind {
                Kind::Bar { .. } => options.title("Notes"),
                Kind::Settings => options.title("Settings"),
                _ => options,
            };
            let handle = app.open_window(options, |_, cx| {
                cx.new(|_| Scene { kind, log: l, ratio: 0.5, refuse: false, search: None, widths: (220., 260.) })
            });
            opened = Some((handle.id(), handle.root().clone()));
        })
    });
    settle(&mut cx, &host);
    let (window, view) = opened.expect("window");
    Opened { cx, host, window, log, view }
}

fn open(kind: Kind) -> Opened {
    open_with(kind, Scheme::Light)
}

impl Opened {
    fn node(&self, id: &str) -> SemanticNode {
        self.host.read(|app| {
            let tree = app.semantic_window(self.window).expect("window");
            find(&tree.root, &|n| n.test_id.as_deref() == Some(id)).cloned().unwrap_or_else(|| panic!("no {id}"))
        })
    }

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

    fn keys(&mut self, keys: &[&'static str]) {
        for key in keys {
            self.input(vec![SyntheticInput::Key(Keystroke::parse(*key).expect("keystroke"))]);
        }
    }

    fn click(&mut self, at: Point) {
        let inputs = [PointerPhase::Down, PointerPhase::Up]
            .into_iter()
            .map(|phase| SyntheticInput::Pointer { phase, at, button: MouseButton::Left, click_count: 1 })
            .collect();
        self.input(inputs);
    }

    /// The divider's centre: the first pane's trailing edge plus half the hairline.
    fn divider(&self) -> Point {
        let first = self.node("first").bounds.expect("first pane laid out");
        Point { x: first.origin.x + first.size.width + 0.5, y: first.origin.y + first.size.height / 2. }
    }

    fn drag_divider_to(&mut self, x: f32) {
        let from = self.divider();
        self.input(vec![SyntheticInput::Drag { from, to: Point { x, y: from.y }, steps: 8 }]);
    }

    fn ratios(&self) -> Vec<f32> {
        std::mem::take(&mut self.log.borrow_mut().ratios)
    }

    fn alert(&mut self, alert: Alert) {
        let (window, log) = (self.window, self.log.clone());
        self.cx.update(|cx| {
            self.host.update(cx, |app| {
                services::alert(app, window, alert, move |r, _| log.borrow_mut().answers.push(r.0));
            })
        });
        settle(&mut self.cx, &self.host);
    }

    fn about(&mut self, about: AboutPanel) {
        let window = self.window;
        self.cx.update(|cx| self.host.update(cx, |app| services::about(app, Some(window), about)));
        settle(&mut self.cx, &self.host);
    }

    fn showing(&self) -> Option<String> {
        self.host.backend().showing_dialog(self.window).map(|s| s.to_string())
    }

    fn theme(&self) -> Theme {
        registry(Scheme::Light).app_theme().clone()
    }

    /// Makes the window the active one, as the compositor does when it takes focus.
    fn activate(&mut self) {
        let handle = self.host.window_handle(self.window).expect("gpui window");
        self.cx.update(|cx| {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        });
        self.past_backdrop_fade();
    }

    /// Past libadwaita's 200 ms backdrop transition.
    fn past_backdrop_fade(&mut self) {
        for _ in 0..6 {
            std::thread::sleep(std::time::Duration::from_millis(60));
            settle(&mut self.cx, &self.host);
        }
    }

    fn is_active(&mut self) -> bool {
        let handle = self.host.window_handle(self.window).expect("gpui window");
        self.cx.update(|cx| handle.update(cx, |_, window, _| window.is_window_active())).expect("window")
    }

    /// Another app window takes focus, leaving this one inactive.
    fn deactivate(&mut self) {
        let other = self.cx.update(|cx| {
            let mut other = None;
            self.host.update(cx, |app| {
                let handle = app.open_window(WindowOptions::main("other").size(size(px(200.), px(100.))), |_, cx| {
                    cx.new(|_| Scene {
                        kind: Kind::Plain,
                        log: Rc::default(),
                        ratio: 0.5,
                        refuse: false,
                        search: None,
                        widths: (220., 260.),
                    })
                });
                other = Some(handle.id());
            });
            other
        });
        settle(&mut self.cx, &self.host);
        let handle = self.host.window_handle(other.expect("other window")).expect("gpui window");
        self.cx.update(|cx| {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        });
        self.past_backdrop_fade();
    }

    fn toggle_sidebar(&mut self) {
        let window = self.window;
        self.cx.update(|cx| self.host.update(cx, |app| app.window_command(window, WindowCommand::ToggleSidebar)));
        self.past_slide();
    }

    /// Past a pane's slide in or out.
    fn past_slide(&mut self) {
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(60));
            settle(&mut self.cx, &self.host);
        }
    }

    fn pane_fill(&self) -> Hsla {
        let paints = self.host.backend().paints(self.window).expect("paints");
        paints.iter().find(|p| p.label == "pane").expect("the sidebar's fill").color
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.002
}

mod gate_m6 {
    use super::*;

    pub fn gpui_split_drag_reports_ratio_within_min_sizes() {
        let mut o = open(Kind::Split);
        let split = o.node("split").bounds.expect("split");
        assert_eq!(split.size.width, 800., "the split fills its row");
        let available = split.size.width - 1.;
        let start = o.divider();
        assert!(close((start.x - 0.5) / available, 0.5), "opens at the ratio: divider at {}", start.x);

        o.drag_divider_to(500.);
        let ratios = o.ratios();
        assert!(ratios.len() > 1, "reports while dragging: {ratios:?}");
        let last = *ratios.last().expect("a ratio");
        assert!(close(last, 499.5 / available), "{last} for a divider at 500");
        let mut dedup = ratios.clone();
        dedup.dedup();
        assert_eq!(dedup, ratios, "only real changes are reported");
        assert!(close((o.divider().x - 0.5) / available, last), "the app fed the ratio back");

        o.drag_divider_to(790.);
        let last = *o.ratios().last().expect("a ratio");
        assert!(close(last, (available - 200.) / available), "the second pane keeps its 200 pt: {last}");
        o.drag_divider_to(5.);
        let last = *o.ratios().last().expect("a ratio");
        assert!(close(last, 120. / available), "the first pane keeps its 120 pt: {last}");
        let first = o.node("first").bounds.expect("first");
        assert!((first.size.width - 120.).abs() < 1., "drawn at the clamped size: {}", first.size.width);
    }

    pub fn gpui_split_echo_and_refusal() {
        let mut o = open(Kind::Split);
        o.drag_divider_to(450.);
        assert!(!o.ratios().is_empty());
        for _ in 0..3 {
            settle(&mut o.cx, &o.host);
        }
        assert_eq!(o.ratios(), Vec::<f32>::new(), "feeding the ratio back reports nothing");
        let view = o.view.clone();
        o.cx.update(|cx| {
            o.host.update(cx, |app| {
                view.update(app, |this, cx| {
                    this.ratio = 0.3;
                    cx.notify();
                })
            })
        });
        settle(&mut o.cx, &o.host);
        assert_eq!(o.ratios(), Vec::<f32>::new(), "an app-set ratio reports nothing");
        assert!(close((o.divider().x - 0.5) / 799., 0.3), "drawn at the app's ratio");

        o.cx.update(|cx| o.host.update(cx, |app| view.update(app, |this, _| this.refuse = true)));
        o.drag_divider_to(600.);
        assert!(!o.ratios().is_empty(), "the drag reported");
        assert!(close((o.divider().x - 0.5) / 799., 0.3), "a refused ratio snaps back");
    }

    pub fn gpui_split_hidden_subtree_is_not_hit_or_painted() {
        let mut o = open(Kind::Hidden);
        let under = o.node("under").bounds.expect("under");
        assert!(!o.node("over").visible, "the hidden page's button is not visible");
        o.click(Point { x: under.origin.x + under.size.width / 2., y: under.origin.y + under.size.height / 2. });
        assert_eq!(o.log.borrow().clicks, ["under"], "the click passes the hidden page to what lies under it");
        let paints = o.host.backend().paints(o.window).expect("paints");
        assert!(paints.iter().any(|p| p.label == "Under"), "the visible button painted");
        assert!(!paints.iter().any(|p| p.label == "Over"), "nothing in the hidden page painted");
    }

    /// The centred item sits at the window's middle whatever sits beside it, and takes taps.
    pub fn gpui_toolbar_centres_its_centred_item_on_the_window() {
        let mut o = open(Kind::Centered);
        let who = o.node("who").bounds.expect("who");
        let centre = who.origin.x + who.size.width / 2.;
        assert!((centre - 400.).abs() < 1., "centred on the 800 pt window, not between its neighbours: {centre}");
        o.click(Point { x: centre, y: who.origin.y + who.size.height / 2. });
        assert_eq!(o.log.borrow().clicks, ["who"], "a tap on the centred item reaches it");
    }

    pub fn gpui_status_page_lays_out_centred() {
        let o = open(Kind::Status);
        let status = o.node("status").bounds.expect("status");
        let centre = status.origin.x + status.size.width / 2.;
        assert!((centre - 400.).abs() < 1., "centred across the window: {centre}");
        assert!(status.size.width <= 600., "no wider than the readable width: {}", status.size.width);
        let button = o.node("new").bounds.expect("action");
        assert!(button.origin.y >= status.origin.y + status.size.height, "the action sits under the header");
        let paints = o.host.backend().paints(o.window).expect("paints");
        let title = paints.iter().find(|p| p.label == "No notes").expect("title painted");
        assert!(
            matches!(title.kind, PaintKind::Text { size, .. } if size == if cfg!(target_os = "macos") { 22. } else { 20. }),
            "title-1: {:?}",
            title.kind
        );
        assert!(paints.iter().any(|p| p.label == "status page icon"), "the icon slot painted");
    }

    /// The status page's dimmed icon is one shape tinted once: where the search glass's handle
    /// crosses its ring, the pixels are no brighter than the rest of the stroke.
    pub fn gpui_a_translucent_icon_covers_its_overlaps_once() {
        if nd_gpui::gpui_platform::current_headless_renderer().ok().flatten().is_none() {
            println!("skipped: GPUI has no headless renderer on this OS");
            return;
        }
        let mut o = open_with(Kind::Status, Scheme::Dark);
        let status = o.node("status").bounds.expect("status");
        let handle = o.host.window_handle(o.window).expect("window");
        let image = o.cx.capture_screenshot(handle).expect("capture");
        let scale = image.width() as f32 / 800.;
        let luma = |x: u32, y: u32| {
            let p = image.get_pixel(x, y).0;
            (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3
        };
        let (left, right) = (((400. - 72.) * scale) as u32, ((400. + 72.) * scale) as u32);
        let top = (status.origin.y * scale) as u32;
        let background = luma(left, top);
        // The icon's rows: from the first row that shows it to the first blank one after.
        let mut histogram = [0u32; 256];
        let mut seen = false;
        for y in top..image.height() {
            let row: Vec<u32> = (left..right).map(|x| luma(x, y)).filter(|l| l.abs_diff(background) > 12).collect();
            if row.is_empty() && seen {
                break;
            }
            seen |= !row.is_empty();
            for l in row {
                histogram[l as usize] += 1;
            }
        }
        let stroke = (0..256).max_by_key(|l| histogram[*l]).expect("icon pixels");
        let brightest = (0..256).rev().find(|l| histogram[*l] > 0).expect("icon pixels");
        assert!(stroke > background as usize, "the dimmed icon drew lighter than the dark page");
        assert!(
            brightest <= stroke + 6,
            "overlapping strokes drew brighter ({brightest}) than the stroke ({stroke}) on {background}"
        );
    }

    pub fn gpui_toolbar_shows_a_title_in_an_empty_leading_run() {
        let titles = |kind| {
            let o = open(kind);
            let paints = o.host.backend().paints(o.window).expect("paints");
            ["Notes", "Inbox"].into_iter().filter(|t| paints.iter().any(|p| p.label == *t)).collect::<Vec<_>>()
        };
        assert_eq!(titles(Kind::Bar { title: None, leading_item: false }), ["Notes"], "the window's title by default");
        assert!(titles(Kind::Bar { title: None, leading_item: true }).is_empty(), "not over the app's own items");
        assert_eq!(titles(Kind::Bar { title: Some("Inbox"), leading_item: true }), ["Inbox"], "the toolbar's own");
        assert!(titles(Kind::Bar { title: Some(""), leading_item: false }).is_empty(), "an empty title hides it");
    }

    pub fn gpui_alert_keys_answer_once() {
        let mut o = open(Kind::Plain);
        o.alert(Alert::confirm("Delete the note?", "Delete").message("You cannot undo this."));
        assert_eq!(o.showing().as_deref(), Some("Delete the note?"));
        o.keys(&["tab", "enter"]);
        assert_eq!(o.log.borrow().answers, [1], "Tab moved to Cancel, Return pressed it");
        assert_eq!(o.showing(), None);
        o.keys(&["enter"]);
        assert_eq!(o.log.borrow().answers, [1], "answered once");

        o.alert(Alert::new("Save changes?").cancel("Cancel").button("Save"));
        o.keys(&["enter"]);
        assert_eq!(o.log.borrow().answers, [1, 1], "Return presses the default button");

        o.alert(Alert::new("Save changes?").button("Save").cancel("Cancel").button("Discard"));
        o.keys(&["shift-tab", "shift-tab", "escape"]);
        assert_eq!(o.log.borrow().answers, [1, 1, 1], "Esc answers the cancel button");

        o.alert(Alert::new("Copied"));
        o.keys(&["escape"]);
        assert_eq!(o.log.borrow().answers, [1, 1, 1, 0], "an alert with no buttons answers its OK");
        assert_eq!(o.showing(), None);
    }

    pub fn gpui_alert_destructive_paints_danger() {
        let mut o = open(Kind::Plain);
        o.alert(Alert::confirm("Delete the note?", "Delete"));
        let theme = o.theme();
        let paints = o.host.backend().paints(o.window).expect("paints");
        let fill = paints.iter().find(|p| p.label == "destructive button").expect("destructive button");
        // libadwaita draws a destructive response as its red over a 15% tint of it.
        let red = if theme.preset == Preset::Adwaita {
            nd_theme::color::with_alpha(theme.text.danger, 0.15)
        } else {
            theme.danger
        };
        assert_eq!(fill.color, red);
        assert!(paints.iter().any(|p| p.label == "dialog scrim" && p.color == theme.scrim));
        o.keys(&["escape"]);
        assert_eq!(o.log.borrow().answers, [1]);
        let paints = o.host.backend().paints(o.window).expect("paints");
        assert!(!paints.iter().any(|p| p.label == "dialog scrim"), "the scrim went with the alert");
    }

    pub fn gpui_alert_queues_and_blocks_input() {
        let mut o = open(Kind::Split);
        o.alert(Alert::new("First").button("OK"));
        o.alert(Alert::new("Second").button("OK"));
        assert_eq!(o.showing().as_deref(), Some("First"), "one alert per window");
        o.drag_divider_to(600.);
        assert_eq!(o.ratios(), Vec::<f32>::new(), "the scrim takes the drag");
        o.keys(&["enter"]);
        assert_eq!(o.showing().as_deref(), Some("Second"), "the next one shows");
        o.keys(&["enter"]);
        assert_eq!(o.log.borrow().answers, [0, 0]);
        o.drag_divider_to(600.);
        assert!(!o.ratios().is_empty(), "input reaches the window again");
    }

    pub fn gpui_alert_modal_covers_every_window() {
        let mut o = open(Kind::Plain);
        let log = o.log.clone();
        let second = o.cx.update(|cx| {
            o.host.update(cx, |app| {
                app.open_window(WindowOptions::main("second").size(size(px(400.), px(300.))), |_, cx| {
                    cx.new(|_| Scene {
                        kind: Kind::Split,
                        log,
                        ratio: 0.5,
                        refuse: false,
                        search: None,
                        widths: (220., 260.),
                    })
                })
                .id()
            })
        });
        settle(&mut o.cx, &o.host);
        o.alert(Alert::new("Quit Notes?").button("Quit").cancel("Cancel").modal());
        assert_eq!(o.showing().as_deref(), Some("Quit Notes?"));
        assert_eq!(o.host.backend().showing_dialog(second), None, "the card shows on the asking window only");
        let paints = o.host.backend().paints(second).expect("paints");
        assert!(paints.iter().any(|p| p.label == "dialog scrim"), "the other window is covered");
        let escape = Keystroke::parse("escape").expect("keystroke");
        o.cx.update(|cx| {
            o.host.update(cx, |app| app.synthesize(second, &SyntheticInput::Key(escape)).expect("posted"))
        });
        settle(&mut o.cx, &o.host);
        assert_eq!(o.log.borrow().answers, [1], "a key in the covered window reaches the modal alert");
        let paints = o.host.backend().paints(second).expect("paints");
        assert!(!paints.iter().any(|p| p.label == "dialog scrim"), "every scrim went with it");
    }

    pub fn gpui_alert_about_opens_and_closes() {
        let mut o = open(Kind::Plain);
        o.about(AboutPanel::new().name("Notes").version("1.2").copyright("© 2026 CanaryCoders"));
        assert_eq!(o.showing().as_deref(), Some("Notes"));
        let paints = o.host.backend().paints(o.window).expect("paints");
        assert!(paints.iter().any(|p| p.label == "1.2"), "the version painted");
        o.keys(&["enter"]);
        assert_eq!(o.showing().as_deref(), Some("Notes"), "Return leaves it open");
        o.keys(&["escape"]);
        assert_eq!(o.showing(), None, "Esc closes it");
    }

    /// libadwaita's header bar centres its title between the start and end widgets, so the
    /// content's header starts with its first button, 6 px in (the `layout-search` golden
    /// shows the title in the middle).
    pub fn gpui_header_bar_centres_its_title() {
        let o = open(Kind::Search);
        let content = o.node("nd-content").bounds.expect("content laid out");
        let new = o.node("new").bounds.expect("new laid out");
        assert!(
            (new.origin.x - content.origin.x - 6.).abs() < 1.,
            "the first start widget sits 6 px in, as the header bar's box padding puts it: {}",
            new.origin.x - content.origin.x
        );
    }

    /// GNOME's search pattern: the header holds a search toggle, and the search bar with the
    /// field slides out under it; Esc closes it again.
    pub fn gpui_toolbar_search_is_a_toggle_and_a_search_bar_on_linux() {
        let mut o = open(Kind::Search);
        assert!(o.node("find").bounds.is_none(), "the field waits in a closed search bar");
        let toggle = o.node("nd-toolbar-item-1").bounds.expect("search toggle laid out");
        assert!(toggle.origin.y < 47. && (toggle.size.width - 34.).abs() < 1., "a 34 px header button: {toggle:?}");
        o.click(Point { x: toggle.origin.x + 17., y: toggle.origin.y + 17. });
        let find = o.node("find").bounds.expect("the search bar opened");
        let content = o.node("nd-content").bounds.expect("content laid out");
        assert!(find.origin.y >= 47., "under the header bar: {find:?}");
        let centre = find.origin.x + find.size.width / 2.;
        assert!((centre - (content.origin.x + content.size.width / 2.)).abs() < 1., "centred in the bar: {find:?}");
        assert!(o.node("find").focused, "opening the bar focuses the field");
        o.keys(&["escape"]);
        assert!(o.node("find").bounds.is_none(), "Esc closes the search bar");
    }

    /// A persistent search field sits in the header at its own width, with no toggle and no
    /// search bar, and takes focus and typing there.
    pub fn gpui_persistent_toolbar_search_stays_in_the_header() {
        let mut o = open(Kind::PersistentSearch);
        let find = o.node("find").bounds.expect("the field is in the header");
        let content = o.node("nd-content").bounds.expect("content laid out");
        assert!(find.origin.y < 47. && find.origin.x >= content.origin.x, "in the content's header: {find:?}");
        assert!((find.size.width - 220.).abs() < 1., "at the app's width: {find:?}");
        o.click(Point { x: find.origin.x + 40., y: find.origin.y + find.size.height / 2. });
        assert!(o.node("find").focused, "a click focuses it");
        o.keys(&["a", "escape"]);
        let after = o.node("find").bounds.expect("Esc closes nothing");
        assert_eq!(after.origin.y, find.origin.y, "still in the header");
    }

    /// A flexible item gives its room back when the header is short: the buttons after it stay
    /// in the content's header, clear of the inspector, and the item keeps its least width. With
    /// nothing over the sidebar, the sidebar has no header of its own.
    pub fn gpui_flexible_toolbar_item_gives_way_in_a_short_header() {
        let o = open(Kind::FlexibleItem);
        let inspector = o.node("nd-inspector").bounds.expect("inspector laid out");
        let info = o.node("info").bounds.expect("the last button is in the header");
        assert!(
            info.origin.x + info.size.width <= inspector.origin.x + 0.5,
            "the last button {info:?} runs under the inspector {inspector:?}"
        );
        let path = o.node("path").bounds.expect("path laid out");
        assert!(path.size.width >= 79.5, "the item keeps its least width: {path:?}");
        assert!(path.size.width < 300., "the item gave its room back: {path:?}");
        // No item and no window controls over the sidebar: no empty header band above it.
        let folders = o.node("folders").bounds.expect("sidebar laid out");
        assert!(folders.origin.y < 1., "the sidebar runs to the top: {folders:?}");
    }

    /// The inspector's header holds nothing but the window's end controls: with none there it
    /// is not drawn and the inspector runs to the top, and with them it keeps their row clear.
    pub fn gpui_inspector_header_only_under_window_controls() {
        let o = open(Kind::FlexibleItem);
        let details = o.node("details").bounds.expect("inspector laid out");
        assert!(details.origin.y < 1., "no empty band over the inspector: {details:?}");
        let o = open_hooked(Kind::FlexibleItem, Preset::Adwaita, Scheme::Light, true, 800.);
        let details = o.node("details").bounds.expect("inspector laid out");
        assert!(details.origin.y >= 46., "the inspector's content clears the controls' row: {details:?}");
        let inspector = o.node("nd-inspector").bounds.expect("inspector laid out");
        assert!(inspector.origin.y < 1., "the pane itself still starts at the top: {inspector:?}");
    }

    /// WinUI's title bar: the search box is the bar's content, centred and stretched to the
    /// Gallery's 580 px where the window has room.
    pub fn gpui_toolbar_search_fills_the_title_bar_on_windows() {
        let o = open_in(Kind::Search, Preset::Fluent, Scheme::Light);
        let find = o.node("find").bounds.expect("search laid out");
        let new = o.node("new").bounds.expect("new laid out");
        assert!(find.origin.y < 48., "in the title bar: {find:?}");
        assert!(find.size.width > 220., "stretched past the app's width: {find:?}");
        assert!(find.size.width <= 580.5, "no wider than 580: {find:?}");
        assert!(find.origin.x > new.origin.x + new.size.width, "after the leading items: {find:?}");
    }

    /// Rich text takes the text size around it as `text` does, and in a row too narrow for it
    /// shrinks to the room left, as truncated plain text does.
    pub fn gpui_rich_text_inherits_size_and_shrinks() {
        let o = open(Kind::RichRows);
        let paints = o.host.backend().paints(o.window).expect("paints");
        let size_of = |label: &str| {
            paints
                .iter()
                .find_map(|p| match p.kind {
                    PaintKind::Text { size, .. } if p.label.trim() == label => Some(size),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no run {label}"))
        };
        let body = o.theme().text_styles[nd_theme::TextStyle::Body].size.as_f32();
        assert_eq!(size_of("draft"), body, "a row's rich text is the body size, as its plain text is");
        assert_eq!(size_of("type"), 20., "rich text takes its parent's size");
        let row = o.node("row").bounds.expect("row");
        let rich = o.node("rich").bounds.expect("rich");
        let tail = o.node("tail").bounds.expect("tail");
        assert!(rich.origin.x + rich.size.width <= tail.origin.x, "the runs shrink to the room left: {rich:?}");
        assert!(
            tail.origin.x + tail.size.width <= row.origin.x + row.size.width + 0.5,
            "the text after it stays in the row: {tail:?} in {row:?}"
        );
        let line = o.node("big").bounds.expect("big").size.height;
        assert!((20. ..32.).contains(&line), "one line at its own size: {line}");
    }

    /// A list that fits its rows in the sidebar's footer is as tall as its row and the list's
    /// padding, pinned at the sidebar's foot under a list that overflows.
    pub fn gpui_footer_list_fits_its_rows() {
        let o = open(Kind::Footer);
        let sidebar = o.node("nd-sidebar").bounds.expect("sidebar");
        let tree = o.node("tree").bounds.expect("tree");
        let trash = o.node("trash").bounds.expect("trash");
        let theme = o.theme();
        let fits = 36. + 2. * theme.spacing.sm.as_f32();
        assert_eq!(trash.size.height, fits, "one 36 px row and the list's padding");
        assert_eq!(trash.origin.y + trash.size.height, sidebar.origin.y + sidebar.size.height, "at the foot");
        assert_eq!(tree.origin.y + tree.size.height, trash.origin.y, "the list above ends where it starts");
    }

    /// A sidebar list whose rows fit has nothing to scroll: a wheel over it leaves its rows where
    /// they are, the selection pill under them included.
    pub fn gpui_fitting_list_does_not_scroll() {
        use nd_gpui::gpui::{
            Modifiers, MouseMoveEvent, PlatformInput, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point,
        };
        if nd_gpui::gpui_platform::current_headless_renderer().ok().flatten().is_none() {
            println!("skipped: GPUI has no headless renderer on this OS");
            return;
        }
        let mut o = open(Kind::FittingFooter);
        let tree = o.node("tree").bounds.expect("tree");
        let handle = o.host.window_handle(o.window).expect("gpui window");
        let at = point(px(tree.origin.x + 40.), px(tree.origin.y + 40.));
        let hover = MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() };
        o.cx.update(|cx| {
            let _ = handle.update(cx, |_, window, cx| window.dispatch_event(PlatformInput::MouseMove(hover), cx));
        });
        settle(&mut o.cx, &o.host);
        let before = o.cx.capture_screenshot(handle).expect("capture");
        for _ in 0..6 {
            let event = ScrollEvent {
                position: at,
                delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
                source: ScrollSource::Pointer,
            };
            o.cx.update(|cx| {
                let _ = handle.update(cx, |_, window, cx| window.dispatch_event(PlatformInput::Scroll(event), cx));
            });
            settle(&mut o.cx, &o.host);
        }
        let after = o.cx.capture_screenshot(handle).expect("capture");
        let scale = before.width() as f32 / 800.;
        let (w, h) = ((tree.size.width * scale) as u32, (tree.size.height * scale) as u32);
        // The selection may still be finishing its fade in, a step or two of colour.
        let differs = |x, y| {
            let (a, b) = (before.get_pixel(x, y).0, after.get_pixel(x, y).0);
            a.iter().zip(b).any(|(a, b)| a.abs_diff(b) > 8)
        };
        let moved = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| differs(x, y)).count();
        assert_eq!(moved, 0, "the rows and the selection stay put");
    }

    /// Wheels the window at `at`, `steps` times by `dy` points (negative scrolls down).
    fn wheel(o: &mut Opened, at: Point, dy: f32, steps: usize) {
        use nd_gpui::gpui::{
            Modifiers, MouseMoveEvent, PlatformInput, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point,
        };
        let handle = o.host.window_handle(o.window).expect("gpui window");
        let at = point(px(at.x), px(at.y));
        let hover = MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() };
        o.cx.update(|cx| {
            let _ = handle.update(cx, |_, window, cx| window.dispatch_event(PlatformInput::MouseMove(hover), cx));
        });
        settle(&mut o.cx, &o.host);
        for _ in 0..steps {
            let event = ScrollEvent {
                position: at,
                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
                source: ScrollSource::Pointer,
            };
            o.cx.update(|cx| {
                let _ = handle.update(cx, |_, window, cx| window.dispatch_event(PlatformInput::Scroll(event), cx));
            });
            settle(&mut o.cx, &o.host);
        }
    }

    /// A wheel over any control in a long form scrolls the form: a control's buttons take
    /// presses, never the wheel.
    pub fn gpui_wheel_over_a_form_control_scrolls_the_form() {
        let mut o = open(Kind::Settings);
        let row = |o: &Opened| o.node("last0").bounds.expect("row laid out").origin.y;
        for id in ["stepper0", "combo0", "text0", "select0"] {
            let control = o.node(id).bounds.expect("control laid out");
            // The trailing end: a stepper's arrows, a combo box's toggle.
            let at =
                Point { x: control.origin.x + control.size.width - 6., y: control.origin.y + control.size.height / 4. };
            let before = row(&o);
            wheel(&mut o, at, -20., 1);
            let after = row(&o);
            assert!(after < before, "a wheel over {id} scrolls the form: {before} -> {after}");
            wheel(&mut o, at, 20., 1);
        }
    }

    /// A form under a window's toolbar of tabs scrolls to both of its ends: the last row comes
    /// fully into the window, and the first one back.
    pub fn gpui_form_under_toolbar_tabs_scrolls_to_both_ends() {
        let mut o = open(Kind::Settings);
        let body = o.node("body").bounds.expect("body laid out");
        let first = o.node("last0").bounds.expect("first section laid out").origin.y;
        let at = Point { x: body.origin.x + 30., y: body.origin.y + 60. };
        wheel(&mut o, at, -200., 30);
        let last = o.node("last5").bounds.expect("last row laid out");
        assert!(
            last.origin.y + last.size.height <= 560. - 24.,
            "the last row shows whole above the body's padding: ends at {}",
            last.origin.y + last.size.height
        );
        assert!(body.origin.y + body.size.height <= 560.5, "the body fits under the tabs: {body:?}");
        wheel(&mut o, at, 200., 30);
        assert_eq!(o.node("last0").bounds.expect("laid out").origin.y, first, "back at the top");
    }

    /// A toolbar's selected tab is marked on every look, and the window's title never sits
    /// among the tabs: libadwaita's view switcher and a macOS preferences toolbar take the
    /// title's place in the middle of the row, a WinUI title bar keeps its caption ahead of them.
    pub fn gpui_toolbar_tabs_mark_the_selected_one() {
        for preset in [Preset::Adwaita, Preset::Fluent, Preset::MacOs, Preset::Plain] {
            let o = open_in(Kind::Settings, preset, Scheme::Light);
            let paints = o.host.backend().paints(o.window).expect("paints");
            let marked: Vec<_> = paints.iter().filter(|p| p.label == "selected tab").collect();
            let selected = o.node("nd-tab-assistant");
            assert_eq!(marked.len(), 1, "{preset:?}: one tab is marked");
            assert_eq!(marked[0].node, Some(selected.id), "{preset:?}: the selected one");
            assert!(marked[0].color.a > 0., "{preset:?}: the mark shows");
            let general = o.node("nd-tab-general").bounds.expect("tab laid out");
            let account = o.node("nd-tab-account").bounds.expect("tab laid out");
            let title = paints.iter().any(|p| p.label == "Settings");
            match preset {
                Preset::Fluent => {
                    assert!(title, "the caption stays in a WinUI title bar");
                    assert!(general.origin.x > 60., "the tabs follow the caption: {general:?}");
                }
                _ => assert!(!title, "{preset:?}: the tabs take the title's place"),
            }
            if matches!(preset, Preset::Adwaita | Preset::MacOs) {
                let middle = (general.origin.x + account.origin.x + account.size.width) / 2.;
                assert!((middle - 400.).abs() < 1., "{preset:?}: the tabs sit in the middle: {middle}");
            }
        }
    }

    /// A resizable sidebar's and inspector's inner edges drag, inside their widths' range; the
    /// app's width wins once it changes, and a width the app ignores still holds.
    pub fn gpui_side_panes_resize_by_their_edges() {
        let mut o = open(Kind::Resizable);
        let width = |o: &Opened, id: &str| o.node(id).bounds.expect("laid out").size.width;
        let drag = |o: &mut Opened, from: f32, to: f32| {
            let y = 200.;
            o.input(vec![SyntheticInput::Drag { from: Point { x: from, y }, to: Point { x: to, y }, steps: 8 }]);
        };
        assert_eq!(width(&o, "nd-sidebar"), 220.);
        let sizes = o.log.borrow().content_sizes.clone();
        assert!(
            sizes.first().is_some_and(|(w, h)| *w > 100. && *h > 100.) && sizes.iter().all(|(w, h)| *w > 0. && *h > 0.),
            "the content reports its real size from the first layout: {sizes:?}"
        );
        drag(&mut o, 220., 300.);
        assert_eq!(o.log.borrow().widths.last(), Some(&("sidebar", 300.)), "the drag reports as it moves");
        assert!(o.log.borrow().widths.len() > 1, "more than once: {:?}", o.log.borrow().widths);
        assert_eq!(width(&o, "nd-sidebar"), 300.);
        drag(&mut o, 300., 20.);
        assert_eq!(width(&o, "nd-sidebar"), PANE_MIN_WIDTH, "no narrower than the default minimum");
        drag(&mut o, PANE_MIN_WIDTH, 790.);
        assert_eq!(width(&o, "nd-sidebar"), PANE_MAX_WIDTH, "no wider than the default maximum");

        let inspector = o.node("nd-inspector").bounds.expect("inspector");
        assert_eq!(inspector.size.width, 260.);
        drag(&mut o, inspector.origin.x, inspector.origin.x + 100.);
        assert_eq!(width(&o, "nd-inspector"), 200., "the inspector grows leftwards and keeps its min_w");
        assert_eq!(o.log.borrow().widths.last(), Some(&("inspector", 200.)));

        let view = o.view.clone();
        o.cx.update(|cx| {
            o.host.update(cx, |app| {
                view.update(app, |this, cx| {
                    this.widths.0 = 240.;
                    this.refuse = true;
                    cx.notify();
                })
            })
        });
        settle(&mut o.cx, &o.host);
        assert_eq!(width(&o, "nd-sidebar"), 240., "the app's width replaces the dragged one");
        drag(&mut o, 240., 280.);
        assert_eq!(width(&o, "nd-sidebar"), 280., "a width the app does not take back still holds");
    }

    /// libadwaita's split view: an open sidebar runs the full height under its own header bar,
    /// and the content's header bar holds the title and the trailing items.
    pub fn gpui_toolbar_splits_into_a_header_bar_per_pane() {
        let o = open(Kind::Chrome);
        let sidebar = o.node("nd-sidebar").bounds.expect("sidebar laid out");
        let content = o.node("nd-content").bounds.expect("content laid out");
        let search = o.node("search").bounds.expect("search laid out");
        assert_eq!(sidebar.origin.y, 0., "the sidebar starts at the window's top");
        assert_eq!(content.origin.y, 0., "the content's header bar is its own");
        assert!(search.origin.x >= content.origin.x && search.origin.y < 47., "search heads the content");
    }

    /// libadwaita's collapsed split view: a window too narrow for the sidebar beside the content
    /// folds it away, the toggle lays it over the content, and a press on the content beside it
    /// takes it away again.
    pub fn gpui_narrow_window_folds_the_sidebar_over_the_content() {
        let mut o = open_hooked(Kind::Chrome, Preset::Adwaita, Scheme::Light, false, 520.);
        let content = o.node("nd-content").bounds.expect("content laid out");
        assert_eq!((content.origin.x, content.size.width), (0., 520.), "the content takes the whole width");
        assert!(!o.node("folders").visible, "the folded sidebar shows nothing");
        o.toggle_sidebar();
        let folders = o.node("folders");
        assert!(folders.visible, "the toggle lays the sidebar over the content");
        assert_eq!(folders.bounds.expect("folders laid out").origin.x, 0., "at the window's leading edge");
        assert_eq!(o.node("nd-content").bounds.expect("content").size.width, 520., "the content keeps its width");
        o.click(Point { x: 480., y: 300. });
        o.past_slide();
        assert!(!o.node("folders").visible, "a press on the content closes it");
        assert!(o.log.borrow().clicks.is_empty(), "and goes no further");
    }

    /// libadwaita's backdrop state: an inactive window's sidebar goes halfway to the window's
    /// colour, and it comes back once the window is active again.
    pub fn gpui_an_inactive_window_takes_the_backdrop_look() {
        let mut o = open(Kind::Chrome);
        o.activate();
        assert!(o.is_active());
        let theme = o.theme();
        assert_eq!(o.pane_fill(), theme.surface.sidebar, "active: the sidebar colour");
        o.deactivate();
        assert!(!o.is_active());
        assert_eq!(o.pane_fill(), theme.sidebar_backdrop(), "inactive: the backdrop sidebar");
        o.activate();
        assert_eq!(o.pane_fill(), theme.surface.sidebar, "active again");
    }

    pub fn gpui_layout_goldens() {
        // GPUI renders headless only on macOS (Metal) and Linux (wgpu); Windows captures arrive
        // with the Fluent goldens (M3).
        if nd_gpui::gpui_platform::current_headless_renderer().ok().flatten().is_none() {
            println!("skipped: GPUI has no headless renderer on this OS");
            return;
        }
        let mut failed = Vec::new();
        for scheme in [Scheme::Light, Scheme::Dark] {
            for view in [
                "split",
                "status",
                "glass",
                "alert",
                "about",
                "chrome",
                "chrome-backdrop",
                "search",
                "search-open",
                "settings",
            ] {
                if let Err(e) = golden(view, scheme) {
                    failed.push(e);
                }
            }
            // The Fluent look as `ND_PRESET=fluent` draws it off Windows.
            {
                for view in [
                    "alert-fluent",
                    "about-fluent",
                    "chrome-fluent",
                    "chrome-backdrop-fluent",
                    "search-fluent",
                    "settings-fluent",
                ] {
                    if let Err(e) = golden(view, scheme) {
                        failed.push(e);
                    }
                }
            }
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }
}

fn golden_path(view: &str, scheme: Scheme) -> PathBuf {
    let scheme = if scheme.is_dark() { "dark" } else { "light" };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/goldens/{}/layout-{view}-{scheme}.png", std::env::consts::OS))
}

/// Per channel, out of 255, and the share of pixels allowed past it: antialiasing drifts.
const CHANNEL_TOLERANCE: i16 = 24;
const PIXEL_TOLERANCE: f64 = 0.004;

fn golden(view: &str, scheme: Scheme) -> Result<(), String> {
    let kind = match view {
        "split" => Kind::Split,
        "status" => Kind::Status,
        "glass" => Kind::Glass,
        v if v.starts_with("chrome") => Kind::Chrome,
        v if v.starts_with("search") => Kind::Search,
        v if v.starts_with("settings") => Kind::Settings,
        _ => Kind::Plain,
    };
    let (shown, preset) = match view.strip_suffix("-fluent") {
        Some(shown) => (shown, Preset::Fluent),
        None => (view, Preset::Adwaita),
    };
    let mut o = open_in(kind, preset, scheme);
    match shown {
        "chrome" | "search" | "search-open" | "settings" => o.activate(),
        "chrome-backdrop" => o.deactivate(),
        "alert" => o.alert(
            Alert::confirm("Delete the note?", "Delete").message("The note and its attachments are removed for good."),
        ),
        "about" => o.about(
            AboutPanel::new()
                .name("Notes")
                .version("1.2.0")
                .credits("Plain notes that sync everywhere.")
                .website("https://example.com/notes")
                .copyright("© 2026 CanaryCoders"),
        ),
        _ => {}
    }
    if shown == "search-open" {
        let toggle = o.node("nd-toolbar-item-1").bounds.ok_or("no search toggle")?;
        o.click(Point { x: toggle.origin.x + toggle.size.width / 2., y: toggle.origin.y + toggle.size.height / 2. });
    }
    let handle = o.host.window_handle(o.window).ok_or("no window")?;
    let image = o.cx.capture_screenshot(handle).map_err(|e| e.to_string())?;
    let path = golden_path(view, scheme);
    if std::env::var_os("ND_UPDATE_GOLDENS").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().expect("dir")).map_err(|e| e.to_string())?;
        image.save(&path).map_err(|e| e.to_string())?;
        println!("recorded {}", path.display());
        return Ok(());
    }
    if !path.exists() {
        let _ = image.save(path.with_extension("actual.png"));
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
        (
            "gate_m6::gpui_split_drag_reports_ratio_within_min_sizes",
            gate_m6::gpui_split_drag_reports_ratio_within_min_sizes,
        ),
        ("gate_m6::gpui_split_echo_and_refusal", gate_m6::gpui_split_echo_and_refusal),
        ("gate_m6::gpui_side_panes_resize_by_their_edges", gate_m6::gpui_side_panes_resize_by_their_edges),
        ("gate_m6::gpui_footer_list_fits_its_rows", gate_m6::gpui_footer_list_fits_its_rows),
        ("gate_m6::gpui_fitting_list_does_not_scroll", gate_m6::gpui_fitting_list_does_not_scroll),
        (
            "gate_m6::gpui_wheel_over_a_form_control_scrolls_the_form",
            gate_m6::gpui_wheel_over_a_form_control_scrolls_the_form,
        ),
        ("gate_m6::gpui_toolbar_tabs_mark_the_selected_one", gate_m6::gpui_toolbar_tabs_mark_the_selected_one),
        (
            "gate_m6::gpui_form_under_toolbar_tabs_scrolls_to_both_ends",
            gate_m6::gpui_form_under_toolbar_tabs_scrolls_to_both_ends,
        ),
        ("gate_m6::gpui_rich_text_inherits_size_and_shrinks", gate_m6::gpui_rich_text_inherits_size_and_shrinks),
        (
            "gate_m6::gpui_persistent_toolbar_search_stays_in_the_header",
            gate_m6::gpui_persistent_toolbar_search_stays_in_the_header,
        ),
        (
            "gate_m6::gpui_flexible_toolbar_item_gives_way_in_a_short_header",
            gate_m6::gpui_flexible_toolbar_item_gives_way_in_a_short_header,
        ),
        (
            "gate_m6::gpui_inspector_header_only_under_window_controls",
            gate_m6::gpui_inspector_header_only_under_window_controls,
        ),
        (
            "gate_m6::gpui_split_hidden_subtree_is_not_hit_or_painted",
            gate_m6::gpui_split_hidden_subtree_is_not_hit_or_painted,
        ),
        ("gate_m6::gpui_status_page_lays_out_centred", gate_m6::gpui_status_page_lays_out_centred),
        (
            "gate_m6::gpui_a_translucent_icon_covers_its_overlaps_once",
            gate_m6::gpui_a_translucent_icon_covers_its_overlaps_once,
        ),
        (
            "gate_m6::gpui_toolbar_centres_its_centred_item_on_the_window",
            gate_m6::gpui_toolbar_centres_its_centred_item_on_the_window,
        ),
        (
            "gate_m6::gpui_toolbar_shows_a_title_in_an_empty_leading_run",
            gate_m6::gpui_toolbar_shows_a_title_in_an_empty_leading_run,
        ),
        ("gate_m6::gpui_alert_keys_answer_once", gate_m6::gpui_alert_keys_answer_once),
        ("gate_m6::gpui_alert_destructive_paints_danger", gate_m6::gpui_alert_destructive_paints_danger),
        ("gate_m6::gpui_alert_queues_and_blocks_input", gate_m6::gpui_alert_queues_and_blocks_input),
        ("gate_m6::gpui_alert_modal_covers_every_window", gate_m6::gpui_alert_modal_covers_every_window),
        ("gate_m6::gpui_alert_about_opens_and_closes", gate_m6::gpui_alert_about_opens_and_closes),
        ("gate_m6::gpui_header_bar_centres_its_title", gate_m6::gpui_header_bar_centres_its_title),
        (
            "gate_m6::gpui_toolbar_search_is_a_toggle_and_a_search_bar_on_linux",
            gate_m6::gpui_toolbar_search_is_a_toggle_and_a_search_bar_on_linux,
        ),
        (
            "gate_m6::gpui_toolbar_search_fills_the_title_bar_on_windows",
            gate_m6::gpui_toolbar_search_fills_the_title_bar_on_windows,
        ),
        (
            "gate_m6::gpui_toolbar_splits_into_a_header_bar_per_pane",
            gate_m6::gpui_toolbar_splits_into_a_header_bar_per_pane,
        ),
        (
            "gate_m6::gpui_narrow_window_folds_the_sidebar_over_the_content",
            gate_m6::gpui_narrow_window_folds_the_sidebar_over_the_content,
        ),
        (
            "gate_m6::gpui_an_inactive_window_takes_the_backdrop_look",
            gate_m6::gpui_an_inactive_window_takes_the_backdrop_look,
        ),
        ("gate_m6::gpui_layout_goldens", gate_m6::gpui_layout_goldens),
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
