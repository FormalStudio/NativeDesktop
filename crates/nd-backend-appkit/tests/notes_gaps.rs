//! Tier 2: what a notes app of cards needed on AppKit. A click lands on what is drawn under it,
//! offset and scaled elements and tile grid tiles included; text cut to one line ends in an
//! ellipsis and clamped text keeps its lines; corners of different radii round each corner; a
//! right click on a source tree row asks for that row's menu and leaves the selection alone.
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
    use nd_components::{source_node, source_tree, tile, tile_grid};
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
        c.check(one.height == short.height, &format!("truncate: one line tall ({} against {})", one.height, short.height));
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
        let outlined = unsafe { mixed.sublayers() }.is_some_and(|s| {
            s.iter().any(|l| l.downcast_ref::<objc2_quartz_core::CAShapeLayer>().is_some())
        });
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
        c.check(*asked.borrow() == ["trash"], &format!("row menu: a right click asks for the row's ({:?})", asked.borrow()));
        c.check(outline.selectedRow() == 0, "row menu: the selection stays on its row");
        let _ = outline.menuForEvent(&press(NSEventType::LeftMouseDown, NSEventModifierFlags::Control));
        rig.pump();
        c.check(asked.borrow().len() == 2, "row menu: a control-click asks too");
        rig.close(opened);
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
