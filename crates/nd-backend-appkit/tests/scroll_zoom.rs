//! Tier 2: a scroll with Command held over an element with `.on_scroll_zoom` is that element's
//! zoom, taken before the `NSScrollView` under the pointer scrolls; without Command it scrolls.
//! The events go straight to the window, as AppKit hands it one read from the window server.

mod harness;

fn main() {
    #[cfg(target_os = "macos")]
    harness::main("gate_m4::appkit_scroll_zoom", mac::run);
    #[cfg(not(target_os = "macos"))]
    harness::main("gate_m4::appkit_scroll_zoom", || {});
}

#[cfg(target_os = "macos")]
mod mac {
    use std::time::Duration;

    use nd_backend_appkit::AppHost;
    use nd_backend_appkit::objc2_app_kit::{NSEvent, NSScreen};
    use nd_backend_appkit::objc2_foundation::MainThreadMarker;
    use nd_ui::prelude::*;
    use nd_ui::{ScrollZoomEvent, WindowId};
    use ndshell::input::{self, GesturePhase as Hid};
    use objc2_core_graphics::{CGEvent, CGEventFlags};

    #[derive(Default)]
    struct Zoomable {
        zooms: Vec<(f32, bool)>,
    }

    impl Render for Zoomable {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("zoom")
                .size_full()
                .on_scroll_zoom(cx.listener(|this, e: &ScrollZoomEvent, _, _| {
                    this.zooms.push((e.steps, e.modifiers.platform));
                }))
                .child(
                    v_flex()
                        .id("scroller")
                        .w(px(200.))
                        .h(px(300.))
                        .overflow_y_scroll()
                        .children((0..50).map(|i| div().id(format!("row-{i}")).w_full().h(px(40.)).flex_shrink_0())),
                )
        }
    }

    pub fn run() {
        let mut window_id = WindowId(0);
        let mut root = None;
        let host = AppHost::new(|app| {
            let handle = app.open_window(WindowOptions::main("zoom").size(size(px(400.), px(400.))), |_, cx| {
                cx.new(|_| Zoomable::default())
            });
            window_id = handle.id();
            root = Some(handle.root().clone());
        });
        let root = root.expect("root view");
        let pump = |host: &AppHost| {
            for _ in 0..10 {
                host.platform().pump(Duration::from_millis(1), Duration::from_millis(20));
                host.update(|app| {
                    if app.needs_frame() {
                        app.frame();
                    }
                });
            }
        };
        pump(&host);
        let window = host.ns_window(window_id).expect("window");
        let views = host.backend().scroll_views();
        let (_, scroll) = views.first().expect("the scroller is an NSScrollView");
        let clip = scroll.contentView();

        let mtm = MainThreadMarker::new().expect("main thread");
        let primary = NSScreen::screens(mtm).firstObject().expect("a screen").frame();
        let frame = window.frame();
        // Sent straight to the window, the event has no window of its own, so AppKit reads its
        // location as the point in the window: 100 in from the left and down from the top, over
        // the scroller. CGEvent locations are global with a top-left origin on the primary display.
        let over = (100., primary.size.height - (frame.size.height - 100.));
        let wheel = |dy: i32, phase: Hid, flags: CGEventFlags| {
            let event = input::scroll_event(over, 0, dy, phase).expect("scroll event");
            CGEvent::set_flags(Some(&event), flags);
            let ns = NSEvent::eventWithCGEvent(&event).expect("NSEvent");
            window.sendEvent(&ns);
            pump(&host);
        };

        wheel(40, Hid::Changed, CGEventFlags::MaskCommand);
        wheel(-10, Hid::Changed, CGEventFlags::MaskCommand);
        let zooms = host.update(|app| root.read(app).zooms.clone());
        assert_eq!(zooms, [(2., true), (-0.5, true)], "Command and two fingers zoom");
        assert_eq!(clip.bounds().origin.y, 0., "and nothing scrolled");

        wheel(0, Hid::Began, CGEventFlags::empty());
        for _ in 0..4 {
            wheel(-60, Hid::Changed, CGEventFlags::empty());
        }
        wheel(0, Hid::Ended, CGEventFlags::empty());
        let y = clip.bounds().origin.y;
        assert!(y > 0., "without Command the fingers scroll ({y})");
        assert_eq!(host.update(|app| root.read(app).zooms.len()), 2);

        window.close();
        println!("ND_APPKIT_SCROLL_ZOOM_OK");
        std::process::exit(0);
    }
}
