//! Tier 2: what a notes app of cards needed on AppKit. A click lands on what is drawn under it,
//! offset and scaled elements and tile grid tiles included; text cut to one line ends in an
//! ellipsis and clamped text keeps its lines. Prints `ND_NOTES_GAPS_OK`.

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

    use nd_backend_appkit::objc2::rc::Retained;
    use nd_backend_appkit::objc2_app_kit::NSView;
    use nd_backend_appkit::objc2_foundation::NSPoint;
    use nd_components::{tile, tile_grid};
    use nd_ui::prelude::*;

    pub use rig::{Checks, Rig};

    const LONG: &str = "A summary long enough to need more than two lines at this width, so the clamp \
                        has something to cut, and then some more words after that for good measure.";

    pub fn run() -> ! {
        let rig = Rig::new();
        let mut c = Checks(0);
        hits_where_drawn(&rig, &mut c);
        cut_text(&rig, &mut c);
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
}
