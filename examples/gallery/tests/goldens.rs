//! Tier 2 goldens of the navigation and feedback components in the macOS look, light and
//! dark: the gallery window on the real AppKit backend, captured as the window server
//! composites it (`screencapture -l`, or ScreenCaptureKit over the window and its overlay windows
//! so the palette's and toasts' glass is in it) and compared
//! with `tests/goldens/macos-<view>-<scheme>@<scale>x.png`. The views: the source list with a
//! banner and a toast, the command palette open over it, and the tabs, split view and status
//! page pages. Holds the ndshell machine lock
//! (activation and capture share one window server). Needs Screen Recording for the process
//! that runs the test (`ndshell grant` on CI).
//!
//! `ND_UPDATE_GOLDENS=1` rewrites the goldens. A mismatch writes `<golden>.actual.png` beside
//! the golden. Prints `ND_GALLERY_GOLDENS_OK`.

#[cfg_attr(not(target_os = "macos"), allow(clippy::needless_return))]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = "gate_m6::gallery_goldens";
    if args.iter().any(|a| a == "--list") {
        if !args.iter().any(|a| a == "--ignored") && cfg!(target_os = "macos") {
            println!("{name}: test");
        }
        return;
    }
    let filters: Vec<&String> = args.iter().skip(1).filter(|a| !a.starts_with('-')).collect();
    if args.iter().any(|a| a == "--ignored")
        || !(filters.is_empty() || filters.iter().any(|f| name.contains(f.as_str())))
    {
        return;
    }
    #[cfg(target_os = "macos")]
    goldens::run();
}

#[cfg(target_os = "macos")]
mod goldens {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use nd::appkit::objc2::rc::Retained;
    use nd::appkit::objc2_app_kit::NSWindow;
    use nd::theme::Appearance;

    /// Per channel, out of 255: antialiasing and subpixel positioning drift by a few levels.
    const CHANNEL_TOLERANCE: i16 = 24;
    /// Share of pixels allowed past the channel tolerance.
    const PIXEL_TOLERANCE: f64 = 0.004;

    fn decode(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let mut decoder = png::Decoder::new(png_bytes);
        decoder.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
        let mut reader = decoder.read_info().expect("png header");
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("png frame");
        buf.truncate(info.buffer_size());
        let channels = info.color_type.samples();
        // Normalise to RGBA.
        let rgba = match channels {
            4 => buf,
            2 => buf.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
            _ => panic!("unexpected png layout {:?}", info.color_type),
        };
        (info.width, info.height, rgba)
    }

    /// Fraction of pixels that differ past the channel tolerance; `None` when sizes differ.
    fn difference(a: &[u8], b: &[u8]) -> f64 {
        let off = a
            .chunks(4)
            .zip(b.chunks(4))
            .filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| (*x as i16 - *y as i16).abs() > CHANNEL_TOLERANCE))
            .count();
        off as f64 / (a.len() / 4).max(1) as f64
    }

    fn golden(view: &str, scheme: &str, scale: u32) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/goldens/macos-{view}-{scheme}@{scale}x.png"))
    }

    fn settle(host: &nd::appkit::AppHost) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            host.platform().pump(Duration::from_millis(30), Duration::from_millis(300));
            let busy = host.update(|app| {
                app.frame();
                app.needs_frame()
            });
            if !busy || Instant::now() > deadline {
                break;
            }
        }
        // A few more turns so deferred work (toolbar items, traffic lights) lands.
        host.platform().pump(Duration::from_millis(100), Duration::from_millis(400));
    }

    /// The overlay windows over `window` (ARCHITECTURE.md 6.3): the palette and toasts draw in
    /// child panels covering its frame.
    fn overlays(window: &nd::appkit::NdWindow) -> Vec<Retained<NSWindow>> {
        let children = window.childWindows().map(|c| c.to_vec()).unwrap_or_default();
        children.into_iter().filter(|c| c.isVisible()).collect()
    }

    /// The window, or the palette's overlay over it, has the keyboard in the active app. A key
    /// overlay panel leaves its owner main, which keeps the owner's active chrome.
    fn is_key(host: &nd::appkit::AppHost, window: &nd::appkit::NdWindow) -> bool {
        let key = window.isKeyWindow() || overlays(window).iter().any(|o| o.isKeyWindow());
        key && host.platform().app().isActive()
    }

    fn make_key(host: &nd::appkit::AppHost, window: &nd::appkit::NdWindow) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !is_key(host, window) && Instant::now() < deadline {
            host.platform().show(window);
            for overlay in overlays(window).iter().filter(|o| o.canBecomeKeyWindow()) {
                overlay.makeKeyWindow();
            }
            host.platform().pump(Duration::from_millis(30), Duration::from_millis(150));
        }
        // Long enough for the active-state redraw to land and for the overlay scrollers AppKit
        // flashes when a window with overflowing content becomes key to fade out again.
        host.platform().pump(Duration::from_millis(1200), Duration::from_millis(1500));
        is_key(host, window)
    }

    /// The window as the window server composites it (glass included), without its shadow. With
    /// overlay windows over it (the palette and toasts, ARCHITECTURE.md 6.3) the capture is the
    /// window and its overlays composited together over its frame, since their glass samples the
    /// window below and a capture of either window alone would not show that. Only those windows
    /// are in it, so an alert another process raises on top stays out.
    fn screenshot(window: &nd::appkit::NdWindow) -> Result<Vec<u8>, String> {
        let overlays = overlays(window);
        if overlays.is_empty() {
            return capture(&format!("-l{}", window.windowNumber()));
        }
        let mut numbers = vec![window.windowNumber() as u32];
        numbers.extend(overlays.iter().map(|o| o.windowNumber() as u32));
        let path = std::env::temp_dir().join(format!("nd-gallery-{}.png", std::process::id()));
        std::fs::write(&path, sck::capture(&numbers)?).map_err(|e| e.to_string())?;
        to_srgb(&path)
    }

    /// One window as the window server composites it, without its shadow.
    fn capture(what: &str) -> Result<Vec<u8>, String> {
        let path = std::env::temp_dir().join(format!("nd-gallery-{}.png", std::process::id()));
        let status = std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", "-o", what])
            .arg(&path)
            .status()
            .map_err(|e| format!("screencapture: {e}"))?;
        std::fs::metadata(&path).map_err(|e| format!("screencapture wrote nothing ({status}): {e}"))?;
        to_srgb(&path)
    }

    /// Reads the capture at `path` matched to sRGB and removes the file.
    fn to_srgb(path: &Path) -> Result<Vec<u8>, String> {
        // Captures come in the main display's colour space (P3 on the built-in panel, near sRGB
        // on a headless display), so each one is matched to sRGB before it is compared.
        let sips = std::process::Command::new("/usr/bin/sips")
            .args(["--matchTo", "/System/Library/ColorSync/Profiles/sRGB Profile.icc"])
            .arg(path)
            .output()
            .map_err(|e| format!("sips: {e}"))?;
        if !sips.status.success() {
            return Err(format!("sips: {}", String::from_utf8_lossy(&sips.stderr)));
        }
        let png = std::fs::read(path).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(path);
        Ok(png)
    }

    /// ScreenCaptureKit stills of a set of windows composited together.
    mod sck {
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        use block2::RcBlock;
        use nd::appkit::objc2::AnyThread;
        use nd::appkit::objc2::rc::Retained;
        use nd::appkit::objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
        use nd::appkit::objc2_foundation::{NSArray, NSDate, NSDictionary, NSError, NSPoint, NSRect, NSRunLoop};
        use objc2_core_graphics::CGImage;
        use objc2_screen_capture_kit::{
            SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
        };

        struct SendBox<T>(T);
        // SAFETY: only moves a retained object from a completion handler to the main thread,
        // which uses it alone afterwards.
        unsafe impl<T> Send for SendBox<T> {}

        /// Runs the main run loop until `slot` fills, since ScreenCaptureKit completes there.
        fn wait<T>(slot: &Mutex<Option<T>>, what: &str) -> Result<T, String> {
            let end = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(value) = slot.lock().unwrap().take() {
                    return Ok(value);
                }
                if Instant::now() > end {
                    return Err(format!("ScreenCaptureKit gave no {what} (Screen Recording permission?)"));
                }
                let until = NSDate::dateWithTimeIntervalSinceNow(0.02);
                NSRunLoop::currentRunLoop().runUntilDate(&until);
            }
        }

        /// The windows `numbers` composited as the window server shows them, cropped to the
        /// frame of the first, at the display's pixel scale. A PNG in the display's colour space.
        pub fn capture(numbers: &[u32]) -> Result<Vec<u8>, String> {
            let found = Arc::new(Mutex::new(None::<Result<SendBox<Retained<SCShareableContent>>, String>>));
            let done = found.clone();
            let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
                let content = unsafe { Retained::retain(content) };
                *done.lock().unwrap() = Some(content.map(SendBox).ok_or_else(|| {
                    unsafe { error.as_ref() }.map_or("no content".into(), |e| e.localizedDescription().to_string())
                }));
            });
            unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
            let content = wait(&found, "shareable content")??.0;
            let all = unsafe { content.windows() };
            let mut windows = Vec::new();
            for number in numbers {
                let w = all.iter().find(|w| unsafe { w.windowID() } == *number);
                windows.push(w.ok_or_else(|| format!("window {number} is not shareable"))?);
            }
            let frame = unsafe { windows[0].frame() };
            let centre = (frame.origin.x + frame.size.width / 2., frame.origin.y + frame.size.height / 2.);
            let display = unsafe { content.displays() }
                .iter()
                .find(|d| {
                    let f = unsafe { d.frame() };
                    (f.origin.x..f.origin.x + f.size.width).contains(&centre.0)
                        && (f.origin.y..f.origin.y + f.size.height).contains(&centre.1)
                })
                .ok_or("the window is on no display")?;
            let included = NSArray::from_retained_slice(&windows);
            let filter = unsafe {
                SCContentFilter::initWithDisplay_includingWindows(SCContentFilter::alloc(), &display, &included)
            };
            let scale = unsafe { filter.pointPixelScale() } as f64;
            let origin = unsafe { display.frame() }.origin;
            let config = unsafe { SCStreamConfiguration::new() };
            unsafe {
                config.setSourceRect(NSRect::new(
                    NSPoint::new(frame.origin.x - origin.x, frame.origin.y - origin.y),
                    frame.size,
                ));
                config.setWidth((frame.size.width * scale).round() as usize);
                config.setHeight((frame.size.height * scale).round() as usize);
                config.setScalesToFit(false);
                config.setShowsCursor(false);
            }
            let shot = Arc::new(Mutex::new(None::<Result<SendBox<Retained<CGImage>>, String>>));
            let done = shot.clone();
            let block = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
                let image = unsafe { Retained::retain(image) };
                *done.lock().unwrap() = Some(image.map(SendBox).ok_or_else(|| {
                    unsafe { error.as_ref() }.map_or("no image".into(), |e| e.localizedDescription().to_string())
                }));
            });
            unsafe {
                SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                    &filter,
                    &config,
                    Some(&block),
                )
            };
            let image = wait(&shot, "screenshot")??.0;
            let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &image);
            let png =
                unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
                    .ok_or("PNG encoding failed")?;
            Ok(png.to_vec())
        }
    }

    fn check(host: &nd::appkit::AppHost, view: &str, appearance: Appearance, scheme: &str) -> Result<(), String> {
        let page = match view {
            "tabs" => gallery::Page::Tabs,
            "split" => gallery::Page::Split,
            "status" => gallery::Page::StatusPage,
            _ => gallery::Page::SourceList,
        };
        let start = gallery::Start {
            page,
            palette: (view == "palette").then(|| "to".into()),
            toast: page == gallery::Page::SourceList,
            appearance: Some(appearance),
            ..gallery::Start::default()
        };
        let handle = host.update(|cx| gallery::open(cx, start));
        settle(host);
        // The toast slides in over 220 ms.
        host.platform().pump(Duration::from_millis(400), Duration::from_millis(600));
        settle(host);
        let window = host.ns_window(handle.id()).expect("window");
        // AppKit draws an inactive window's controls differently, and parallel tests take focus
        // from each other: capture only while this window is key in the active app.
        let mut capture = None;
        for _ in 0..10 {
            if !make_key(host, &window) {
                continue;
            }
            let shot = screenshot(&window)?;
            if is_key(host, &window) {
                capture = Some(shot);
                break;
            }
        }
        let capture = capture.ok_or_else(|| format!("{view} {scheme}: the window never stayed key for a capture"))?;
        let scale = window.backingScaleFactor().round() as u32;
        host.update(|cx| cx.close_window(handle.id()));
        settle(host);
        let path = golden(view, scheme, scale);
        if std::env::var_os("ND_UPDATE_GOLDENS").is_some_and(|v| v == "1") || !path.exists() {
            std::fs::write(&path, &capture).map_err(|e| e.to_string())?;
            println!("recorded {}", path.display());
            return Ok(());
        }
        let expected = std::fs::read(&path).map_err(|e| e.to_string())?;
        let (ew, eh, e) = decode(&expected);
        let (aw, ah, a) = decode(&capture);
        let actual_path = path.with_extension("actual.png");
        if (ew, eh) != (aw, ah) {
            let _ = std::fs::write(&actual_path, &capture);
            return Err(format!("{view} {scheme}: size {aw}x{ah}, golden {ew}x{eh}"));
        }
        let diff = difference(&e, &a);
        if diff > PIXEL_TOLERANCE {
            let _ = std::fs::write(&actual_path, &capture);
            return Err(format!(
                "{view} {scheme}: {:.2}% of pixels differ (see {})",
                diff * 100.,
                actual_path.display()
            ));
        }
        let _ = std::fs::remove_file(&actual_path);
        println!("ok   {view} {scheme} matches {} ({:.3}% off)", path.display(), diff * 100.);
        Ok(())
    }

    pub fn run() -> ! {
        // Activation and the capture share the window server with every other real-window run.
        let _lock = ndshell::lock::window_test("gate_m6::gallery_goldens");
        // Isolated from the user's theme files and data.
        let data = std::env::temp_dir().join(format!("nd-gallery-goldens-{}", std::process::id()));
        unsafe {
            std::env::set_var("ND_APP_DATA_DIR", &data);
            std::env::set_var("ND_THEME_TOML", "");
            std::env::set_var("ND_THEME_JSON", "");
        }
        let host = gallery::app().host(|_cx| {});
        let mut failed = 0;
        for view in ["sidebar", "palette", "tabs", "split", "status"] {
            for (appearance, scheme) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
                if let Err(e) = check(&host, view, appearance, scheme) {
                    println!("FAIL {e}");
                    failed += 1;
                }
            }
        }
        let _ = std::fs::remove_dir_all(&data);
        if failed == 0 {
            println!("ND_GALLERY_GOLDENS_OK");
            std::process::exit(0);
        }
        std::process::exit(1);
    }
}
