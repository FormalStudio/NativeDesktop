//! Tier 2: a drag over a list marks the slot between rows it would drop into, on a real
//! `NSTableView`. The lower half of a row is the slot after it: the line sits between it and the
//! next, and the slot comes back with its index. The window renders to
//! `tests/goldens/<os>/list-drop-slot-light.png` (`ND_UPDATE_GOLDENS=1` rewrites it). Prints
//! `ND_LIST_DROP_SLOT_OK`.

mod harness;

fn main() {
    #[cfg(target_os = "macos")]
    harness::main("gate_m7::appkit_list_drop_slot", || app::run());
    #[cfg(not(target_os = "macos"))]
    harness::main("gate_m7::appkit_list_drop_slot", || {});
}

#[cfg(target_os = "macos")]
mod app {
    use std::path::Path;
    use std::time::Duration;

    use nd_backend_appkit::objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSTableView,
    };
    use nd_backend_appkit::{AppHost, AppKitBackend};
    use nd_ui::backend::Backend;
    use nd_ui::prelude::*;
    use nd_ui::{DragItem, DropSlot};
    use nd_virtual::{ListState, RowDropped, list};

    struct Slots {
        list: ListState<u32>,
    }

    impl Render for Slots {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                list("slots", &self.list, |row: u32, _, _| div().h(px(32.)).px(px(12.)).child(format!("Tab {row}")))
                    .accepts_drops([DragItem::FILES])
                    .on_drop(|_: &RowDropped<u32>, _, _| {}),
            )
        }
    }

    /// Per channel, out of 255, and the share of pixels allowed past it: antialiasing drifts.
    const CHANNEL_TOLERANCE: i16 = 24;
    const PIXEL_TOLERANCE: f64 = 0.004;

    fn golden(image: &image::RgbaImage, name: &str) -> Result<(), String> {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/goldens/{}/{name}.png", std::env::consts::OS));
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
            return Err(format!(
                "{}: size {:?}, golden {:?}",
                path.display(),
                image.dimensions(),
                expected.dimensions()
            ));
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

    fn settle(host: &AppHost) {
        let mut quiet = 0;
        for _ in 0..64 {
            host.platform().pump(Duration::from_millis(1), Duration::from_millis(30));
            let busy = host.update(|app| {
                app.drain_events();
                let busy = app.needs_frame();
                if busy {
                    app.frame();
                }
                busy
            });
            quiet = if busy { 0 } else { quiet + 1 };
            if quiet >= 3 {
                return;
            }
        }
    }

    pub fn run() -> ! {
        let mut opened = None;
        let host = AppHost::new(|app| {
            let handle = app.open_window(WindowOptions::main("slots").size(size(px(240.), px(140.))), |_, cx| {
                cx.new(|cx| {
                    let mut list = ListState::new(cx);
                    list.set_items(0..3, cx);
                    Slots { list }
                })
            });
            opened = Some(handle.id());
        });
        let window = opened.expect("window");
        let ns = host.ns_window(window).expect("window");
        ns.setAppearance(NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }).as_deref());
        settle(&host);

        let mut failures = 0;
        let mut check = |ok: bool, what: &str| {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
            failures += usize::from(!ok);
        };

        let mut backend: AppKitBackend = host.backend().clone();
        let node = *backend.lists().first().expect("a list");
        let item = DragItem::new(DragItem::FILES, "/tmp/a.txt");
        // The first row's top and bottom in window points from the top left, wherever the
        // table's insets put it.
        let (table, _) = backend.list_views(node).expect("list views");
        let table = table.downcast_ref::<NSTableView>().expect("a table");
        let row = table.convertRect_toView(table.rectOfRow(0), None);
        let height = ns.contentView().expect("content").frame().size.height;
        let (top, bottom) = ((height - row.origin.y - row.size.height) as f32, (height - row.origin.y) as f32);
        let quarter = (bottom - top) / 4.;
        let at = |x: f32, y: f32| Some(nd_core::Point { x, y });
        let slot = backend.drop_slot(node, at(40., bottom - quarter), &item);
        check(
            slot.as_ref().map(|s: &DropSlot| s.index) == Some(1),
            &format!("the lower half of the first row is the slot after it ({slot:?})"),
        );
        settle(&host);
        match backend.capture(window) {
            Ok(capture) => {
                let image = image::load_from_memory(&capture.png).expect("png").to_rgba8();
                if let Err(e) = golden(&image, "list-drop-slot-light") {
                    check(false, &e);
                }
            }
            Err(e) => check(false, &format!("capture: {e:?}")),
        }
        let slot = backend.drop_slot(node, at(40., top + quarter), &item);
        check(slot.map(|s| s.index) == Some(0), "the upper half of the first row is the slot before it");
        check(backend.drop_slot(node, None, &item).is_none(), "a drag that leaves hides the line");

        drop(host);
        if failures == 0 {
            println!("ND_LIST_DROP_SLOT_OK");
            std::process::exit(0);
        }
        std::process::exit(1);
    }
}
