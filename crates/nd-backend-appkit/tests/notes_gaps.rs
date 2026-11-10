//! Tier 2: what a notes app of cards needed on AppKit. A click lands on what is drawn under it,
//! offset and scaled elements and tile grid tiles included; text cut to one line ends in an
//! ellipsis and clamped text keeps its lines; corners of different radii round each corner; a
//! right click on a source tree row asks for that row's menu and leaves the selection alone; a
//! control-click or right click on a card's content asks for the card's menu, not its tap, and
//! one on a list row, its label or its padding, asks for the row's; a card casts its shadow,
//! clipped or not, and dips while a press on it is held. A list that fits its rows in a
//! sidebar's footer shows all of them with no scroller, in a short window too.
//! Prints `ND_NOTES_GAPS_OK`.

mod harness;

fn main() {
    #[cfg(target_os = "macos")]
    harness::main("notes_gaps::appkit_mapping", || gaps::run());
    #[cfg(not(target_os = "macos"))]
    harness::main("notes_gaps::appkit_mapping", || {});
}

#[cfg(target_os = "macos")]
#[path = "forms"]
mod gaps {
    mod rig;

    use std::cell::RefCell;
    use std::rc::Rc;

    use nd_backend_appkit::objc2::rc::Retained;
    use nd_backend_appkit::objc2_app_kit::NSView;
    use nd_backend_appkit::objc2_foundation::NSPoint;
    use nd_components::{SubtreeOverrides, source_node, source_tree, tile, tile_grid};
    use nd_ui::prelude::*;

    pub use rig::{Checks, Rig};

    const LONG: &str = "A summary long enough to need more than two lines at this width, so the clamp \
                        has something to cut, and then some more words after that for good measure.";

    pub fn run() -> ! {
        let rig = Rig::new();
        let mut c = Checks(0);
        hits_where_drawn(&rig, &mut c);
        cut_text(&rig, &mut c);
        corners(&rig, &mut c);
        row_menus(&rig, &mut c);
        inner_menus(&rig, &mut c);
        list_row_menus(&rig, &mut c);
        shadows_and_presses(&rig, &mut c);
        hero_first_frame(&rig, &mut c);
        fitted_rows(&rig, &mut c);
        if c.0 == 0 {
            println!("ND_NOTES_GAPS_OK");
            std::process::exit(0);
        }
        println!("ND_NOTES_GAPS_FAILED {} check(s)", c.0);
        std::process::exit(1);
    }

    /// The view the window's hit test finds at `at`, in the content view's coordinates.
    fn hit(opened: &rig::Opened<impl Sized>, root: &NSView, at: NSPoint) -> Option<Retained<NSView>> {
        let content = opened.window.contentView().expect("content view");
        let in_window = root.convertPoint_toView(at, None);
        // SAFETY: main thread, a view in a live window.
        let point = unsafe { content.superview() }.map_or(in_window, |s| s.convertPoint_fromView(in_window, None));
        content.hitTest(point)
    }

    fn inside(view: Option<Retained<NSView>>, target: &NSView) -> bool {
        view.is_some_and(|v| v.isDescendantOf(target))
    }

    struct Moved;

    impl Render for Moved {
        fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("stage")
                .size_full()
                .child(div().id("offset").absolute().top_0().left_0().size(px(40.)).offset(px(200.), px(100.)))
                .child(div().id("small").absolute().top(px(300.)).left(px(0.)).size(px(100.)).scale(0.5))
                .child(
                    div().id("grid-box").absolute().top(px(400.)).left_0().w(px(400.)).child(
                        tile_grid("grid")
                            .max_columns(3)
                            .min_tile_width(100.)
                            .tiles(["a", "b", "c"].map(|id| tile(id, div().id(format!("card-{id}")).size_full()))),
                    ),
                )
        }
    }

    fn hits_where_drawn(rig: &Rig, c: &mut Checks) {
        let opened = rig.open("moved", |_| Moved);
        let win = opened.id();
        for _ in 0..4 {
            rig.pump();
        }
        let stage = rig.view(win, "stage");
        let offset = rig.view(win, "offset");
        c.check(inside(hit(&opened, &stage, NSPoint::new(220., 120.)), &offset), "offset: a click where it is drawn");
        c.check(!inside(hit(&opened, &stage, NSPoint::new(20., 20.)), &offset), "offset: none where it was laid out");
        let small = rig.view(win, "small");
        c.check(inside(hit(&opened, &stage, NSPoint::new(50., 350.)), &small), "scale: a click on its centre");
        c.check(!inside(hit(&opened, &stage, NSPoint::new(10., 310.)), &small), "scale: none on the corner it left");
        let (a, c_tile) = (rig.view(win, "card-a"), rig.view(win, "card-c"));
        let first = hit(&opened, &stage, NSPoint::new(30., 420.));
        c.check(inside(first.clone(), &a), "tile grid: the first tile takes a click on the first tile");
        c.check(!inside(first, &c_tile), "tile grid: the last tile does not");
        c.check(
            inside(hit(&opened, &stage, NSPoint::new(330., 420.)), &c_tile),
            "tile grid: the third tile takes a click on the third",
        );
        rig.close(opened);
    }

    struct Cut;

    impl Render for Cut {
        fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .p(px(12.))
                .gap(px(8.))
                .w(px(200.))
                .child(text("Short").id("short"))
                .child(text(LONG).id("one").truncate())
                .child(text(LONG).id("two").line_clamp(2).text_ellipsis())
                .child(text(LONG).id("all"))
        }
    }

    fn cut_text(rig: &Rig, c: &mut Checks) {
        let opened = rig.open("cut", |_| Cut);
        let win = opened.id();
        rig.pump();
        let frame = |id: &str| rig.view(win, id).frame().size;
        let (short, one, two, all) = (frame("short"), frame("one"), frame("two"), frame("all"));
        c.check(
            one.height == short.height,
            &format!("truncate: one line tall ({} against {})", one.height, short.height),
        );
        c.check(one.width == 176., &format!("truncate: as wide as the column ({})", one.width));
        c.check(
            two.height > one.height * 1.5 && two.height < one.height * 2.5,
            &format!("line clamp: two lines tall ({} against {})", two.height, one.height),
        );
        c.check(all.height > two.height, &format!("unclamped text is taller ({} > {})", all.height, two.height));
        rig.close(opened);
    }

    struct Corners;

    impl Render for Corners {
        fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let shape = |id: &str| div().id(id.to_owned()).size(px(80.)).bg(hsla(0.6, 0.6, 0.5, 1.));
            h_flex()
                .p(px(12.))
                .gap(px(8.))
                .child(shape("all").rounded(px(12.)))
                .child(shape("diagonal").rounded_tl(px(24.)).rounded_br(px(24.)))
                .child(
                    shape("mixed")
                        .rounded_tl(px(4.))
                        .rounded_tr(px(16.))
                        .rounded_br(px(32.))
                        .rounded_bl(px(8.))
                        .border_2()
                        .border_color(hsla(0., 0., 0., 1.)),
                )
        }
    }

    fn corners(rig: &Rig, c: &mut Checks) {
        use objc2_quartz_core::CACornerMask;
        let opened = rig.open("corners", |_| Corners);
        let win = opened.id();
        rig.pump();
        let layer = |id: &str| {
            let view = rig.view(win, id);
            view.displayIfNeeded();
            view.layer().expect("layer-backed")
        };
        let all = layer("all");
        c.check(all.cornerRadius() == 12. && all.mask().is_none(), "corners: one radius rounds the layer");
        let diagonal = layer("diagonal");
        let masked = diagonal.maskedCorners();
        c.check(
            diagonal.cornerRadius() == 24.
                && masked == CACornerMask::LayerMinXMinYCorner | CACornerMask::LayerMaxXMaxYCorner,
            &format!("corners: equal radii round only their corners ({masked:?})"),
        );
        let mixed = layer("mixed");
        // SAFETY: main thread; the array is read, not kept.
        let outlined = unsafe { mixed.sublayers() }
            .is_some_and(|s| s.iter().any(|l| l.downcast_ref::<objc2_quartz_core::CAShapeLayer>().is_some()));
        c.check(
            mixed.cornerRadius() == 0. && mixed.mask().is_some() && mixed.borderWidth() == 0. && outlined,
            "corners: different radii cut a mask, and the border follows it",
        );
        rig.close(opened);
    }

    struct Tree {
        asked: Rc<RefCell<Vec<String>>>,
    }

    impl Render for Tree {
        fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let asked = self.asked.clone();
            div().w(px(240.)).h(px(300.)).child(
                source_tree("tree")
                    .nodes([source_node("notes", "Notes"), source_node("trash", "Trash")])
                    .selected("notes")
                    .context_menu(move |id, menu, _, _| {
                        asked.borrow_mut().push(id.to_string());
                        menu
                    }),
            )
        }
    }

    fn row_menus(rig: &Rig, c: &mut Checks) {
        use nd_backend_appkit::objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSOutlineView};
        use nd_backend_appkit::objc2_foundation::NSProcessInfo;
        let asked = Rc::new(RefCell::new(Vec::new()));
        let log = asked.clone();
        let opened = rig.open("tree", move |_| Tree { asked: log });
        let win = opened.id();
        rig.pump();
        let outline = find::<NSOutlineView>(&rig.view(win, "tree")).expect("an outline view");
        let rect = outline.rectOfRow(1);
        let at = outline.convertPoint_toView(NSPoint::new(rect.origin.x + 20., rect.origin.y + 4.), None);
        let press = |kind, flags| {
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind,
                at,
                flags,
                NSProcessInfo::processInfo().systemUptime(),
                opened.window.windowNumber(),
                None,
                0,
                1,
                1.,
            )
            .expect("mouse event")
        };
        let shown = outline.menuForEvent(&press(NSEventType::RightMouseDown, NSEventModifierFlags::empty()));
        rig.pump();
        c.check(shown.is_none(), "row menu: AppKit shows no menu of its own");
        c.check(
            *asked.borrow() == ["trash"],
            &format!("row menu: a right click asks for the row's ({:?})", asked.borrow()),
        );
        c.check(outline.selectedRow() == 0, "row menu: the selection stays on its row");
        let _ = outline.menuForEvent(&press(NSEventType::LeftMouseDown, NSEventModifierFlags::Control));
        rig.pump();
        c.check(asked.borrow().len() == 2, "row menu: a control-click asks too");
        rig.close(opened);
    }

    struct MenuCard {
        asked: Rc<std::cell::Cell<usize>>,
        tapped: Rc<std::cell::Cell<usize>>,
    }

    impl Render for MenuCard {
        fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let (asked, tapped) = (self.asked.clone(), self.tapped.clone());
            div().id("tile").size(px(200.)).on_tap(move |_: &TapEvent, _, _| tapped.set(tapped.get() + 1)).child(
                div()
                    .id("face")
                    .size_full()
                    .context_menu(move |menu, _, _| {
                        asked.set(asked.get() + 1);
                        menu
                    })
                    .child(div().id("inner").size(px(100.)).bg(hsla(0., 0., 0.5, 1.)).child(text("Groceries"))),
            )
        }
    }

    /// A press on a card's content: the content has no menu of its own, so a control-click asks
    /// the card's, where the tile around would otherwise take it as a tap.
    fn inner_menus(rig: &Rig, c: &mut Checks) {
        use nd_backend_appkit::objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
        use nd_backend_appkit::objc2_foundation::NSProcessInfo;
        let (asked, tapped) = (Rc::new(std::cell::Cell::new(0)), Rc::new(std::cell::Cell::new(0)));
        let (a, t) = (asked.clone(), tapped.clone());
        let opened = rig.open("menu card", move |_| MenuCard { asked: a, tapped: t });
        let win = opened.id();
        rig.pump();
        let inner = rig.view(win, "inner");
        let at = inner.convertPoint_toView(NSPoint::new(10., 10.), None);
        let event = |kind, flags| {
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind,
                at,
                flags,
                NSProcessInfo::processInfo().systemUptime(),
                opened.window.windowNumber(),
                None,
                0,
                1,
                1.,
            )
            .expect("mouse event")
        };
        let control = NSEventModifierFlags::Control;
        inner.mouseDown(&event(NSEventType::LeftMouseDown, control));
        inner.mouseUp(&event(NSEventType::LeftMouseUp, control));
        settle(rig);
        c.check(
            asked.get() == 1 && tapped.get() == 0,
            &format!(
                "inner menu: a control-click asks for the card's menu ({} asked, {} tapped)",
                asked.get(),
                tapped.get()
            ),
        );
        inner.rightMouseDown(&event(NSEventType::RightMouseDown, NSEventModifierFlags::empty()));
        settle(rig);
        c.check(asked.get() == 2, &format!("inner menu: so does a right click ({} asked)", asked.get()));
        rig.close(opened);
    }

    /// Rows with a menu of their own, but for row 4, which has the column's around the list.
    struct MenuRows {
        state: nd_virtual::ListState<u64>,
        asked: Rc<RefCell<Vec<u64>>>,
        picked: Rc<RefCell<Vec<Vec<u64>>>>,
    }

    impl Render for MenuRows {
        fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let asked = self.asked.clone();
            let column = self.asked.clone();
            let picked = self.picked.clone();
            div()
                .size_full()
                .context_menu(move |menu, _, _| {
                    column.borrow_mut().push(0);
                    menu
                })
                .child(
                    nd_virtual::list("menu rows", &self.state, move |key: u64, _, _| {
                        let asked = asked.clone();
                        let row = div()
                            .id(format!("menu-row-{key}"))
                            .w_full()
                            .h(px(28.))
                            .px(px(8.))
                            .rounded(px(6.))
                            .hover(|s| s.bg(hsla(0., 0., 0.5, 0.2)))
                            .on_hover(|_: &bool, _, _| {});
                        let row = if key == 4 {
                            row
                        } else {
                            row.context_menu(move |menu, _, _| {
                                asked.borrow_mut().push(key);
                                menu
                            })
                        };
                        row.flex()
                            .flex_row()
                            .items_center()
                            .child(nd_components::icon(nd_icons::Icon::Globe, 16.))
                            .child(
                                nd_components::label(format!("Row {key}"))
                                    .id(format!("menu-label-{key}"))
                                    .truncate()
                                    .flex_1()
                                    .min_w_0(),
                            )
                            .child(
                                div()
                                    .w(px(24.))
                                    .flex_none()
                                    .child(nd_components::icon_button(format!("x-{key}"), nd_icons::Icon::X)),
                            )
                            .into_any_element()
                    })
                    .inset()
                    .selection(nd_virtual::Selection::single())
                    .on_selection_change(move |e: &nd_virtual::SelectionChanged<u64>, _, _| {
                        picked.borrow_mut().push(e.keys.clone())
                    }),
                )
        }
    }

    /// A right click on a virtual list row's label, the selected row's or another's, or on a
    /// row's padding, and a control-click on its padding, ask for that row's menu, not the
    /// column's around the list, and the label does not keep the click for itself.
    fn list_row_menus(rig: &Rig, c: &mut Checks) {
        use nd_backend_appkit::objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
        use nd_backend_appkit::objc2_foundation::{MainThreadMarker, NSProcessInfo};
        let asked = Rc::new(RefCell::new(Vec::new()));
        let picked = Rc::new(RefCell::new(Vec::new()));
        let (a, p) = (asked.clone(), picked.clone());
        let opened = rig.open("menu rows", move |cx| {
            let mut state = nd_virtual::ListState::new(cx).estimated_row_height(px(28.));
            state.set_items(vec![1, 2, 3, 4], cx);
            MenuRows { state, asked: a, picked: p }
        });
        settle(rig);
        // A row drawn again (its hover changed) keeps its menu.
        rig.update(&opened, |rows, cx| {
            rows.state.set_items_versioned(vec![(1, 0), (2, 1), (3, 0), (4, 0)], cx);
            rows.state.set_selected(vec![2], cx);
        });
        settle(rig);
        let send = |kind: NSEventType, flags: NSEventModifierFlags, row: &str, x: Option<f64>| {
            let row = rig.view(opened.id(), row);
            let b = row.bounds();
            let at = row.convertPoint_toView(NSPoint::new(x.unwrap_or(b.size.width / 2.), b.size.height / 2.), None);
            let event = |kind| {
                NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                    kind,
                    at,
                    flags,
                    NSProcessInfo::processInfo().systemUptime(),
                    opened.window.windowNumber(),
                    None,
                    0,
                    1,
                    1.,
                )
                .expect("mouse event")
            };
            // Through the app's queue, as hardware input comes: NSTableView hit-tests against
            // the current event, which a direct sendEvent does not set.
            let app = NSApplication::sharedApplication(MainThreadMarker::new().expect("main thread"));
            app.postEvent_atStart(&event(kind), false);
            if kind == NSEventType::LeftMouseDown {
                app.postEvent_atStart(&event(NSEventType::LeftMouseUp), false);
            }
            settle(rig);
        };
        let none = NSEventModifierFlags::empty();
        send(NSEventType::RightMouseDown, none, "menu-row-2", None);
        send(NSEventType::RightMouseDown, none, "menu-row-3", None);
        send(NSEventType::RightMouseDown, none, "menu-row-3", Some(4.));
        send(NSEventType::LeftMouseDown, NSEventModifierFlags::Control, "menu-row-1", Some(4.));
        c.check(
            *asked.borrow() == [2, 3, 3, 1],
            &format!("list row menu: each click asks its own row's menu, 2, 3, 3, 1 ({:?})", asked.borrow()),
        );
        c.check(
            picked.borrow().is_empty() && rig.update(&opened, |rows, _| rows.state.selected()) == [2],
            &format!("list row menu: a row's own menu leaves the selection alone ({:?})", picked.borrow()),
        );
        send(NSEventType::RightMouseDown, none, "menu-row-4", None);
        c.check(
            *asked.borrow() == [2, 3, 3, 1, 0] && *picked.borrow() == [vec![4]],
            &format!(
                "list row menu: the column's menu on a row selects it alone first ({:?}, {:?})",
                asked.borrow(),
                picked.borrow()
            ),
        );
        rig.close(opened);
    }

    struct Cards;

    impl Render for Cards {
        fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let card =
                |id: &str| div().id(id.to_owned()).size(px(80.)).rounded(px(10.)).bg(hsla(0., 0., 1., 1.)).shadow_md();
            h_flex()
                .p(px(20.))
                .gap(px(20.))
                .child(card("plain").active(|p| p.scale(0.9)))
                .child(card("clipped").overflow_hidden())
        }
    }

    fn shadows_and_presses(rig: &Rig, c: &mut Checks) {
        use nd_backend_appkit::objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
        use nd_backend_appkit::objc2_foundation::NSProcessInfo;
        let opened = rig.open("cards", |_| Cards);
        let win = opened.id();
        rig.pump();
        let (plain, clipped) = (rig.view(win, "plain"), rig.view(win, "clipped"));
        plain.displayIfNeeded();
        clipped.displayIfNeeded();
        let layer = plain.layer().expect("layer-backed");
        c.check(
            layer.shadowOpacity() > 0. && layer.shadowRadius() > 0. && layer.shadowPath().is_some(),
            "shadow: a card casts its own",
        );
        let clipped_layer = clipped.layer().expect("layer-backed");
        let parent = unsafe { clipped.superview() }.and_then(|s| s.layer()).expect("parent layer");
        // SAFETY: main thread; the array is read, not kept.
        let siblings: Vec<_> = unsafe { parent.sublayers() }.map(|s| s.to_vec()).unwrap_or_default();
        let at = siblings.iter().position(|l| std::ptr::eq(&**l, &*clipped_layer));
        let caster = at.and_then(|i| i.checked_sub(1)).map(|i| siblings[i].clone());
        c.check(clipped_layer.shadowOpacity() == 0., "shadow: a clipped card's own layer casts none");
        c.check(
            caster.as_ref().is_some_and(|l| {
                l.shadowOpacity() > 0. && l.mask().is_some() && l.frame().origin.x == clipped_layer.frame().origin.x
            }),
            "shadow: a layer under a clipped card casts it, cut out where the card is",
        );

        let at = plain.convertPoint_toView(NSPoint::new(40., 40.), None);
        let event = |kind| {
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind,
                at,
                NSEventModifierFlags::empty(),
                NSProcessInfo::processInfo().systemUptime(),
                opened.window.windowNumber(),
                None,
                0,
                1,
                1.,
            )
            .expect("mouse event")
        };
        let scale = || plain.layer().expect("layer").affineTransform().a;
        plain.mouseDown(&event(NSEventType::LeftMouseDown));
        settle(rig);
        c.check((scale() - 0.9).abs() < 1e-3, &format!("press: the card dips while held ({})", scale()));
        plain.mouseUp(&event(NSEventType::LeftMouseUp));
        settle(rig);
        c.check((scale() - 1.).abs() < 1e-3, &format!("press: and comes back on release ({})", scale()));
        rig.close(opened);
    }

    struct Hero {
        open: bool,
    }

    impl Render for Hero {
        fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("stage")
                .size_full()
                .relative()
                .child(div().id("card").absolute().top(px(300.)).left(px(200.)).size(px(80.)).bg(hsla(0., 0., 0.5, 1.)))
                .when(self.open, |d| {
                    d.child(
                        div()
                            .id("hero")
                            .absolute()
                            .top_0()
                            .left_0()
                            .w(px(400.))
                            .h(px(500.))
                            .bg(hsla(0., 0., 1., 1.))
                            .hero_from("card", false)
                            .child(text("A page growing out of its card")),
                    )
                })
        }
    }

    /// The frame a hero first shows in draws it on its source: AppKit resets the backing layer's
    /// transform when it syncs the geometry of a view lifted onto the frame view.
    fn hero_first_frame(rig: &Rig, c: &mut Checks) {
        let opened = rig.open("hero", |_| Hero { open: false });
        let win = opened.id();
        rig.pump();
        rig.update(&opened, |v, cx| {
            v.open = true;
            cx.notify();
        });
        // The clock stands still: the hero stays on its source once placed.
        for _ in 0..3 {
            rig.pump();
        }
        let hero = rig.view(win, "hero");
        opened.window.displayIfNeeded();
        let t = hero.layer().expect("layer-backed").affineTransform();
        c.check(
            (t.tx - 200.).abs() < 1. && (t.ty.abs() - 300.).abs() < 1.,
            &format!("hero: the first frame drawn keeps the offset onto its source ({}, {})", t.tx, t.ty),
        );
        rig.close(opened);
    }

    struct Footer {
        height: f32,
    }

    impl Render for Footer {
        fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            use nd_components::source_list;
            let rows = (0..30).map(|i| source_node(format!("note{i}"), format!("Note {i}")));
            v_flex()
                .w(px(240.))
                .h(px(self.height))
                .child(source_tree("tree").nodes(rows).flex_1().min_h_0())
                .child(source_list("trash").nodes([source_node("trash", "Trash").badge("1")]).fit_rows())
        }
    }

    /// The Trash row under a sidebar's list: the list is as tall as the row, so nothing scrolls
    /// or shows a scroller, and it stays whole while the list above it shrinks.
    fn fitted_rows(rig: &Rig, c: &mut Checks) {
        use nd_backend_appkit::objc2_app_kit::{NSOutlineView, NSScrollView};
        let opened = rig.open("footer", |_| Footer { height: 420. });
        let win = opened.id();
        for height in [420., 120.] {
            rig.update(&opened, |v, cx| {
                v.height = height;
                cx.notify();
            });
            rig.pump();
            let trash = rig.view(win, "trash");
            let scroll = find::<NSScrollView>(&trash).expect("a scroll view");
            let outline = find::<NSOutlineView>(&trash).expect("an outline view");
            let row = outline.rectOfRow(0);
            let visible = scroll.documentVisibleRect();
            c.check(!scroll.hasVerticalScroller(), &format!("fit rows at {height}: no vertical scroller"));
            c.check(
                (scroll.frame().size.height - (row.origin.y + row.size.height)).abs() < 1.,
                &format!("fit rows at {height}: as tall as its rows ({:?}, {row:?})", scroll.frame()),
            );
            c.check(
                visible.origin.y <= row.origin.y
                    && visible.origin.y + visible.size.height >= row.origin.y + row.size.height,
                &format!("fit rows at {height}: the whole row shows ({row:?} in {visible:?})"),
            );
            let tree = find::<NSScrollView>(&rig.view(win, "tree")).expect("the tree's scroll view");
            let (bottom, top) = {
                let t = tree.convertRect_toView(tree.bounds(), None);
                let f = scroll.convertRect_toView(scroll.bounds(), None);
                (t.origin.y, f.origin.y + f.size.height)
            };
            c.check(
                (bottom - top).abs() < 1.,
                &format!("fit rows at {height}: the list above ends where it starts ({bottom}, {top})"),
            );
            c.check(tree.autohidesScrollers(), &format!("fit rows at {height}: the tree's scroller hides when idle"));
        }
        rig.close(opened);
    }

    /// Frames on a clock that moves a tenth of a second each, past any fast animation.
    fn settle(rig: &Rig) {
        for _ in 0..6 {
            rig.app.borrow_mut().advance_clock(0.1);
            rig.pump();
        }
    }

    fn find<T: nd_backend_appkit::objc2::ClassType + nd_backend_appkit::objc2::DowncastTarget>(
        view: &NSView,
    ) -> Option<Retained<T>> {
        if let Some(t) = view.downcast_ref::<T>() {
            return Some(nd_backend_appkit::objc2::Message::retain(t));
        }
        view.subviews().iter().find_map(|v| find::<T>(&v))
    }
}
