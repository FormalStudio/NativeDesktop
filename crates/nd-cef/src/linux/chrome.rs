//! Chrome-style pages on Linux: a windowed browser in an X11 child window of the page's GPUI
//! window, which therefore runs on X11 (XWayland under a Wayland compositor:
//! `nd_backend_gpui::prefer_x11`). Chromium parents its own X window to a container of ours, so
//! the page moves, resizes, hides and clips by moving, resizing, unmapping and shaping the
//! container.
//!
//! The X server stacks a child window above everything its parent draws: the container's
//! bounding and input shapes are the two halves of [`crate::gpui::shape`].
//!
//! The container is created parked under the root window, unmapped, since Chromium needs its
//! parent before the page is in any window; it moves into the GPUI window when the page is first
//! drawn, and back to the root when the page closes, so Chromium's window outlives neither the
//! page nor its own close.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use cef::{BrowserSettings, CefString, Client, RequestContext, RuntimeStyle, WindowInfo, browser_host_create_browser};
use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    ClipOrdering, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, ImageFormat, InputFocus, Rectangle,
    StackMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub(crate) use crate::gpui::shape::shape;
use crate::gpui::shape::{R, Shape};

/// Chrome style parents Chromium's window to the page's, which takes an X11 window: in a Wayland
/// window the page is off-screen instead.
pub(crate) fn supported(style: nd_webview::CefStyle) -> nd_webview::CefStyle {
    if style == nd_webview::CefStyle::Chrome && !nd_backend_gpui::on_x11() {
        tracing::warn!(
            "nd-cef: Chrome style needs the window on X11; this page is off-screen. Set \
             `[webview.cef] style = \"chrome\"` for the app, which puts its windows on XWayland"
        );
        return nd_webview::CefStyle::Osr;
    }
    style
}

/// Whether Chromium can run an extension action on a page here: it needs the toolbar model a
/// Views-hosted browser has, and a page in a native parent window has none (Chromium
/// dereferences the missing toolbar).
pub(crate) fn extension_actions() -> Result<(), String> {
    Err("extension actions on Linux need a Views-hosted page, which nd does not create yet".into())
}

/// The X11 window GPUI draws `window` into.
pub(crate) fn parent(window: &nd_gpui::gpui::Window) -> Option<Window> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok(),
        _ => None,
    }
}

thread_local! {
    static CONN: RefCell<Option<Rc<X>>> = const { RefCell::new(None) };
    /// Containers of closed pages, until Chromium's browser in each is gone.
    static RELEASED: RefCell<HashMap<u64, Window>> = RefCell::default();
    static LIVE: Cell<usize> = const { Cell::new(0) };
}

/// nd-cef's own connection to the X server the app's windows are on.
struct X {
    conn: RustConnection,
    root: Window,
    depth: u8,
    visual: u32,
    colormap: u32,
    white: u32,
}

fn x() -> Result<Rc<X>, String> {
    CONN.with(|c| {
        if let Some(x) = c.borrow().as_ref() {
            return Ok(x.clone());
        }
        let (conn, screen) = RustConnection::connect(None).map_err(|e| format!("X11: {e}"))?;
        let s = &conn.setup().roots[screen];
        let x = Rc::new(X {
            root: s.root,
            depth: s.root_depth,
            visual: s.root_visual,
            colormap: s.default_colormap,
            white: s.white_pixel,
            conn,
        });
        *c.borrow_mut() = Some(x.clone());
        Ok(x)
    })
}

impl X {
    /// Sends what is queued, and drops the errors of requests nobody waits on (a focus change
    /// to a window that just unmapped).
    fn flush(&self) {
        let _ = self.conn.flush();
        while let Ok(Some(_)) = self.conn.poll_for_event() {}
    }
}

/// Chrome-style pages open: Chromium's X events then need its loop run often (`pump`).
pub(crate) fn live() -> usize {
    LIVE.with(Cell::get)
}

#[derive(Default)]
struct St {
    /// The GPUI window the container is in; the root while parked.
    parent: Window,
    mapped: bool,
    frame: R,
    shape: Option<Shape>,
    /// Chromium's window and the size it was given: it only exists once the browser does, and
    /// does not follow the container on its own.
    browser: Option<(Window, i32, i32)>,
}

pub(crate) struct Chrome {
    page: u64,
    x: Rc<X>,
    container: Window,
    st: RefCell<St>,
}

impl Chrome {
    pub(crate) fn track_popup(&self, _id: &str, _anchor: nd_webview::extensions::Anchor) {}

    pub(crate) fn untrack_popup(&self) {}

    pub(crate) fn new(page: u64) -> Result<Rc<Self>, String> {
        let x = x()?;
        let container = x.conn.generate_id().map_err(|e| format!("X11: {e}"))?;
        // The screen's own visual, not the parent's: a GPUI window can be 32-bit ARGB, and
        // Chromium's GL surface cannot be parented under one.
        let aux =
            CreateWindowAux::new().background_pixel(x.white).border_pixel(0).colormap(x.colormap).override_redirect(1);
        x.conn
            .create_window(x.depth, container, x.root, 0, 0, 800, 600, 0, WindowClass::INPUT_OUTPUT, x.visual, &aux)
            .map_err(|e| format!("X11: {e}"))?;
        x.conn.sync().map_err(|e| format!("X11: {e}"))?;
        LIVE.with(|l| l.set(l.get() + 1));
        Ok(Rc::new(Self { page, x: x.clone(), container, st: RefCell::new(St { parent: x.root, ..St::default() }) }))
    }

    /// Creates the page's browser, windowed in the container.
    pub(crate) fn create(
        &self,
        mut client: Client,
        settings: &BrowserSettings,
        mut context: Option<RequestContext>,
    ) -> Result<(), String> {
        // Asynchronous: a windowed browser's creation waits on Chromium's loop, which only runs
        // once this returns. The browser arrives through `on_after_created`.
        let created = browser_host_create_browser(
            Some(&self.window_info()),
            Some(&mut client),
            Some(&CefString::from("")),
            Some(settings),
            None,
            context.as_mut(),
        );
        if created == 0 {
            return Err("Chromium could not create the browser".into());
        }
        Ok(())
    }

    /// This page is the popup a Chrome-style opener is about to create, in the container.
    pub(crate) fn expect_popup(&self, info: &mut WindowInfo) {
        *info = self.window_info();
    }

    pub(crate) fn browser_created(&self) {}

    pub(crate) fn browser_closed(&self) {}

    /// How Chromium creates the page's browser: windowed, in the container, Chrome style.
    fn window_info(&self) -> WindowInfo {
        WindowInfo {
            parent_window: u64::from(self.container) as _,
            bounds: cef::Rect { x: 0, y: 0, width: 800, height: 600 },
            runtime_style: RuntimeStyle::CHROME,
            ..WindowInfo::default()
        }
    }

    fn browser_window(&self) -> Option<Window> {
        let host = crate::page::get(self.page)?.host()?;
        u32::try_from(cef::ImplBrowserHost::window_handle(&host)).ok().filter(|w| *w != 0)
    }

    /// The page is drawn at `frame` of GPUI window `parent` with `shape`. Returns whether the
    /// container or Chromium's window changed size, so a snapshot taken before is stale.
    pub(crate) fn place(&self, parent: Window, frame: R, shape: Shape) -> bool {
        let conn = &self.x.conn;
        let mut st = self.st.borrow_mut();
        if st.parent != parent {
            let _ = conn.reparent_window(self.container, parent, frame.x as i16, frame.y as i16);
            st.parent = parent;
            st.frame = R::default();
            st.mapped = false;
        }
        let mut resized = st.frame.w != frame.w || st.frame.h != frame.h;
        if st.frame != frame {
            let aux = ConfigureWindowAux::new()
                .x(frame.x)
                .y(frame.y)
                .width(frame.w.max(1) as u32)
                .height(frame.h.max(1) as u32)
                .stack_mode(StackMode::ABOVE);
            let _ = conn.configure_window(self.container, &aux);
            st.frame = frame;
        }
        let browser = self.browser_window().map(|w| (w, frame.w, frame.h));
        if let Some((window, w, h)) = browser
            && st.browser != browser
        {
            let aux = ConfigureWindowAux::new().x(0).y(0).width(w.max(1) as u32).height(h.max(1) as u32);
            let _ = conn.configure_window(window, &aux);
            st.browser = browser;
            resized = true;
        }
        if st.shape.as_ref() != Some(&shape) {
            let rects = |rs: &[R]| -> Vec<Rectangle> {
                rs.iter()
                    .map(|r| Rectangle { x: r.x as i16, y: r.y as i16, width: r.w as u16, height: r.h as u16 })
                    .collect()
            };
            let whole = [R::new(0, 0, frame.w, frame.h)];
            for (kind, region) in [(SK::BOUNDING, &shape.bounding), (SK::INPUT, &shape.input)] {
                let region = if region.as_slice() == whole { None } else { Some(rects(region)) };
                let _ = match region {
                    None => conn.shape_mask(SO::SET, kind, self.container, 0, 0, x11rb::NONE),
                    Some(rs) => conn.shape_rectangles(SO::SET, kind, ClipOrdering::UNSORTED, self.container, 0, 0, &rs),
                };
            }
            st.shape = Some(shape);
        }
        if !st.mapped {
            let _ = conn.map_window(self.container);
            st.mapped = true;
        }
        drop(st);
        self.x.flush();
        resized
    }

    /// The page is not drawn this frame: out of sight, its place kept.
    pub(crate) fn park(&self) {
        let mut st = self.st.borrow_mut();
        if std::mem::take(&mut st.mapped) {
            let _ = self.x.conn.unmap_window(self.container);
            self.x.flush();
        }
    }

    /// The keyboard goes to the page, or with `false` back to the GPUI window it is in.
    pub(crate) fn focus(&self, focused: bool) {
        let st = self.st.borrow();
        let target = if focused { self.browser_window() } else { Some(st.parent).filter(|w| *w != self.x.root) };
        if let (true, Some(target)) = (st.mapped || !focused, target) {
            let _ = self.x.conn.set_input_focus(InputFocus::PARENT, target, x11rb::CURRENT_TIME);
            self.x.flush();
        }
    }

    /// What the page shows now, as GPUI's BGRA, for the holes overlays cut into it.
    pub(crate) fn snapshot(&self) -> Option<(Vec<u8>, u32, u32)> {
        let frame = self.st.borrow().frame;
        let (w, h) = (u16::try_from(frame.w).ok()?, u16::try_from(frame.h).ok()?);
        let source = self.browser_window().unwrap_or(self.container);
        let reply = self.x.conn.get_image(ImageFormat::Z_PIXMAP, source, 0, 0, w, h, !0).ok()?.reply().ok()?;
        let mut data = reply.data;
        if data.len() != usize::from(w) * usize::from(h) * 4 {
            return None;
        }
        for pixel in data.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        Some((data, u32::from(w), u32::from(h)))
    }

    /// The page closed: the container leaves the window for the root, unmapped, until
    /// Chromium's browser in it is gone ([`released`]).
    pub(crate) fn teardown(&self) {
        self.park();
        let _ = self.x.conn.reparent_window(self.container, self.x.root, 0, 0);
        self.x.flush();
        RELEASED.with(|r| r.borrow_mut().insert(self.page, self.container));
    }
}

/// Page `id`'s browser is gone: its container goes too.
pub(crate) fn released(id: u64) {
    let Some(container) = RELEASED.with(|r| r.borrow_mut().remove(&id)) else { return };
    LIVE.with(|l| l.set(l.get().saturating_sub(1)));
    if let Ok(x) = x() {
        let _ = x.conn.destroy_window(container);
        x.flush();
    }
}
