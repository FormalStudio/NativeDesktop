//! Chrome-style pages on Windows, Views-hosted as on macOS: a native-parent browser has no
//! toolbar model and extension actions crash on it, so the page is born in a frameless CEF Views
//! window with Chrome's toolbar hidden. That window's HWND becomes a `WS_CHILD` of a container
//! of ours, and the container a child of the page's GPUI window: the page moves, resizes, hides
//! and clips by moving, resizing, hiding and cutting the region of the container. GPUI composes
//! its own content under child windows (fork patch 0022), so overlays are cut out of the
//! container ([`crate::gpui::shape`]).
//!
//! Containers live under a hidden parking window until the page is first drawn, since the Views
//! window exists before the page is in any window, and move into the GPUI window with
//! `SetParent`, also when the page moves to another window. Chromium's window goes back to the
//! parking window when the page closes, where Chromium closes it.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cef::*;
use nd_webview::extensions::Anchor;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, ClientToScreen, CombineRgn, CreateCompatibleDC, CreateDIBSection,
    CreateRectRgn, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW, HGDIOBJ, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromWindow, RGN_OR, ReleaseDC, SelectObject, SetWindowRgn,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, EnumThreadWindows, GWL_STYLE, GetWindowLongPtrW, GetWindowRect,
    HWND_TOP, IsChild, IsWindowVisible, PW_RENDERFULLCONTENT, RegisterClassW, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOWPOS,
    WM_NCDESTROY, WM_WINDOWPOSCHANGING, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    WS_EX_NOPARENTNOTIFY, WS_EX_TOOLWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
};
use windows::core::{BOOL, w};

use crate::gpui::shape::{self, R, Shape};
use crate::page::{self, CefPage};

/// How often an action click looks for its popup window.
const POPUP_POLL: Duration = Duration::from_millis(8);
/// The subclass id of a popup pinned under its anchor.
const PIN: usize = 1;

/// Every style is hosted here.
pub(crate) fn supported(style: nd_webview::CefStyle) -> nd_webview::CefStyle {
    style
}

/// A Views-hosted page has the toolbar model extension actions need.
pub(crate) fn extension_actions() -> Result<(), String> {
    Ok(())
}

/// The HWND GPUI draws `window` into.
pub(crate) fn parent(window: &nd_gpui::gpui::Window) -> Option<HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// Win32 has one window region for what shows and what takes the pointer, so a part that only
/// takes the pointer (a modal backdrop) is cut as well, and the snapshot shows there instead.
pub(crate) fn shape(frame: R, visible: Option<R>, overlays: &[(R, bool, bool)], scale: f32) -> Shape {
    let s = shape::shape(frame, visible, overlays, scale);
    let cut = s.cut || s.input != s.bounding;
    Shape { bounding: s.input.clone(), input: s.input, cut }
}

thread_local! {
    static PARKING: Cell<Option<HWND>> = const { Cell::new(None) };
    /// Pages created in `on_before_popup` for a Chrome-style opener, waiting for the BrowserView
    /// Chromium builds them (`get_delegate_for_popup_browser_view`, then
    /// `on_popup_browser_view_created`).
    static PENDING_POPUPS: RefCell<VecDeque<u64>> = RefCell::default();
}

/// The class of containers and of the parking window: nothing but `DefWindowProc`.
fn class() -> Result<windows::core::PCWSTR, String> {
    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let name = w!("nd-cef-page");
    let ok = *REGISTERED.get_or_init(|| {
        // SAFETY: registers a class whose procedure is DefWindowProcW; the name is static.
        unsafe {
            let instance = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(def_proc),
                hInstance: instance,
                lpszClassName: name,
                ..WNDCLASSW::default()
            };
            RegisterClassW(&class) != 0
        }
    });
    if ok { Ok(name) } else { Err(format!("RegisterClassW: {}", windows::core::Error::from_thread())) }
}

unsafe extern "system" fn def_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: the arguments are the ones Windows passed this window procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The hidden top-level window containers and closing Chromium windows wait under.
fn parking() -> Result<HWND, String> {
    if let Some(hwnd) = PARKING.with(Cell::get) {
        return Ok(hwnd);
    }
    let class = class()?;
    // SAFETY: a window of our own class, never shown; the strings are static.
    let hwnd =
        unsafe { CreateWindowExW(WS_EX_TOOLWINDOW, class, w!(""), WS_POPUP, 0, 0, 0, 0, None, None, None, None) }
            .map_err(|e| format!("the parking window: {e}"))?;
    PARKING.with(|p| p.set(Some(hwnd)));
    Ok(hwnd)
}

#[derive(Default)]
struct St {
    /// The GPUI window the container is in; `None` while parked.
    parent: Option<HWND>,
    shown: bool,
    frame: R,
    shape: Option<Shape>,
    /// The Views window, once Chromium made it, and the size it was given: it does not follow
    /// the container on its own.
    views: Option<HWND>,
    views_size: (i32, i32),
    closed: bool,
}

/// An extension popup on its way or on screen. Chromium anchors it to the hidden toolbar, so it
/// is pinned under the app's anchor whenever Chromium sizes or places it.
struct PopupTrack {
    id: String,
    anchor: Anchor,
    /// The top-level windows of this thread when the click ran; the popup is a new one.
    before: Vec<isize>,
    window: Option<HWND>,
    deadline: Instant,
    reported: bool,
}

pub(crate) struct Chrome {
    page: u64,
    container: HWND,
    browser_view: RefCell<Option<BrowserView>>,
    window: RefCell<Option<cef::Window>>,
    st: RefCell<St>,
    popup: RefCell<Option<PopupTrack>>,
}

impl Chrome {
    pub(crate) fn new(page: u64) -> Result<Rc<Self>, String> {
        let parking = parking()?;
        let class = class()?;
        // SAFETY: a child of the parking window, of our own class.
        let container = unsafe {
            CreateWindowExW(
                WS_EX_NOPARENTNOTIFY,
                class,
                w!(""),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                800,
                600,
                Some(parking),
                None,
                None,
                None,
            )
        }
        .map_err(|e| format!("the page's container window: {e}"))?;
        Ok(Rc::new(Self {
            page,
            container,
            browser_view: RefCell::default(),
            window: RefCell::default(),
            st: RefCell::default(),
            popup: RefCell::default(),
        }))
    }

    fn page(&self) -> Option<Rc<CefPage>> {
        page::get(self.page)
    }

    /// Creates the BrowserView and its Views window. The browser itself arrives through
    /// `on_after_created`.
    pub(crate) fn create(
        &self,
        mut client: Client,
        settings: &BrowserSettings,
        mut context: Option<RequestContext>,
    ) -> Result<(), String> {
        let mut delegate = NdBrowserView::new(self.page);
        let view = browser_view_create(
            Some(&mut client),
            Some(&CefString::from("")),
            Some(settings),
            None,
            context.as_mut(),
            Some(&mut delegate),
        )
        .ok_or("Chromium could not create the Chrome-style browser view")?;
        self.adopt(view)
    }

    /// This page is the popup a Chrome-style opener is about to create: Chromium builds its
    /// BrowserView and asks the opener's delegate for it.
    pub(crate) fn expect_popup(&self, _info: &mut WindowInfo) {
        PENDING_POPUPS.with(|p| p.borrow_mut().push_back(self.page));
    }

    /// Hosts `view` (made here, or a popup's that Chromium made) in a new Views window.
    fn adopt(&self, view: BrowserView) -> Result<(), String> {
        *self.browser_view.borrow_mut() = Some(view);
        let mut delegate = NdChromeWindow::new(self.page);
        // `on_window_created` runs before this returns.
        window_create_top_level(Some(&mut delegate)).ok_or("Chromium could not create the Chrome-style window")?;
        Ok(())
    }

    /// `on_window_created`: the BrowserView fills the frameless window, whose HWND becomes the
    /// container's child before it is first shown, so it is never a window of its own on the
    /// desktop.
    fn window_created(&self, window: &cef::Window) {
        *self.window.borrow_mut() = Some(window.clone());
        let _ = window.set_to_fill_layout();
        if let Some(browser_view) = self.browser_view.borrow().as_ref() {
            window.add_child_view(Some(&mut cef::View::from(browser_view)));
            match browser_view.chrome_toolbar() {
                Some(toolbar) => toolbar.set_visible(0),
                None => tracing::warn!("nd-cef: Chrome style has no toolbar to hide"),
            }
        }
        let hwnd = HWND(window.window_handle().0.cast());
        if hwnd.is_invalid() {
            tracing::warn!("nd-cef: the Chrome-style window reported no HWND");
            return;
        }
        let top_level = (WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX).0;
        // SAFETY: restyles and reparents a window of this thread that Chromium just created.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let child = (style & !top_level) | (WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS).0;
            SetWindowLongPtrW(hwnd, GWL_STYLE, child as isize);
            if let Err(e) = SetParent(hwnd, Some(self.container)) {
                tracing::warn!("nd-cef: parenting the Chrome-style window: {e}");
            }
        }
        {
            let mut st = self.st.borrow_mut();
            st.views = Some(hwnd);
            st.views_size = (0, 0);
        }
        window.show();
        let frame = self.st.borrow().frame;
        self.size_views(frame.w, frame.h);
    }

    pub(crate) fn browser_created(&self) {}

    /// Sizes Chromium's window to the container's `w` by `h`. Returns whether it changed.
    fn size_views(&self, w: i32, h: i32) -> bool {
        let views = {
            let mut st = self.st.borrow_mut();
            let Some(views) = st.views else { return false };
            if st.views_size == (w, h) || w <= 0 || h <= 0 {
                return false;
            }
            st.views_size = (w, h);
            views
        };
        // SAFETY: moves a child window of this thread inside its parent.
        let _ = unsafe { SetWindowPos(views, None, 0, 0, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
        true
    }

    /// The page is drawn at `frame` of GPUI window `parent` with `shape`. Returns whether the
    /// container or Chromium's window changed size, so a snapshot taken before is stale.
    pub(crate) fn place(&self, parent: HWND, frame: R, shape: Shape) -> bool {
        let (moved, mut resized, region, show) = {
            let mut st = self.st.borrow_mut();
            if st.closed {
                return false;
            }
            if st.parent != Some(parent) {
                // SAFETY: both are windows of this thread.
                if let Err(e) = unsafe { SetParent(self.container, Some(parent)) } {
                    tracing::warn!("nd-cef: moving the Chrome-style page into its window: {e}");
                }
                st.parent = Some(parent);
                st.frame = R::default();
                st.shape = None;
                st.shown = false;
            }
            let resized = st.frame.w != frame.w || st.frame.h != frame.h;
            let moved = st.frame != frame;
            st.frame = frame;
            let region = (st.shape.as_ref() != Some(&shape)).then(|| shape.bounding.clone());
            st.shape = Some(shape);
            (moved, resized, region, !std::mem::replace(&mut st.shown, true))
        };
        // SAFETY: all calls act on the container, a child window of this thread.
        unsafe {
            if moved {
                let _ = SetWindowPos(
                    self.container,
                    Some(HWND_TOP),
                    frame.x,
                    frame.y,
                    frame.w.max(1),
                    frame.h.max(1),
                    SWP_NOACTIVATE,
                );
            }
            if let Some(region) = region {
                set_region(self.container, frame, &region);
            }
            if show {
                let _ = ShowWindow(self.container, SW_SHOWNA);
            }
        }
        resized |= self.size_views(frame.w, frame.h);
        resized
    }

    /// The page is not drawn this frame: out of sight, its place kept.
    pub(crate) fn park(&self) {
        if std::mem::take(&mut self.st.borrow_mut().shown) {
            // SAFETY: hides a child window of this thread.
            let _ = unsafe { ShowWindow(self.container, SW_HIDE) };
        }
    }

    /// The keyboard goes to the page, or with `false` back to the GPUI window it is in.
    pub(crate) fn focus(&self, focused: bool) {
        let (views, parent, shown) = {
            let st = self.st.borrow();
            (st.views, st.parent, st.shown)
        };
        // SAFETY: focus moves between windows of this thread.
        unsafe {
            if focused {
                if let (true, Some(views)) = (shown, views) {
                    let _ = SetFocus(Some(views));
                }
            } else if let Some(parent) = parent {
                let current = GetFocus();
                if current == self.container || IsChild(self.container, current).as_bool() {
                    let _ = SetFocus(Some(parent));
                }
            }
        }
    }

    /// What the page shows now, as GPUI's BGRA, for the holes overlays cut into it.
    pub(crate) fn snapshot(&self) -> Option<(Vec<u8>, u32, u32)> {
        let (source, frame) = {
            let st = self.st.borrow();
            (st.views.unwrap_or(self.container), st.frame)
        };
        let (w, h) = (u32::try_from(frame.w).ok().filter(|w| *w > 0)?, u32::try_from(frame.h).ok().filter(|h| *h > 0)?);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: frame.w,
                // Negative: rows top to bottom, as GPUI's images are.
                biHeight: -frame.h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        // SAFETY: a memory DC and a DIB section of exactly `w * h` 32-bit pixels, released below;
        // PrintWindow draws a window of this thread into it.
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0);
            let pixels = match bitmap {
                Ok(bitmap) if !bits.is_null() => {
                    let old = SelectObject(dc, HGDIOBJ(bitmap.0));
                    let printed = PrintWindow(source, dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
                    let len = (w * h * 4) as usize;
                    let mut data = std::slice::from_raw_parts(bits.cast::<u8>(), len).to_vec();
                    SelectObject(dc, old);
                    let _ = DeleteObject(HGDIOBJ(bitmap.0));
                    printed.then(|| {
                        for pixel in data.as_chunks_mut::<4>().0 {
                            pixel[3] = 255;
                        }
                        data
                    })
                }
                _ => None,
            };
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            pixels.map(|p| (p, w, h))
        }
    }

    /// The page closed: Chromium's window waits under the parking window for Chromium to close
    /// it, and the container goes.
    pub(crate) fn teardown(&self) {
        let views = {
            let mut st = self.st.borrow_mut();
            if std::mem::replace(&mut st.closed, true) {
                return;
            }
            st.views
        };
        self.untrack_popup();
        // SAFETY: windows of this thread; the container has no child left when it goes.
        unsafe {
            if let (Some(views), Ok(parking)) = (views, parking()) {
                let _ = ShowWindow(views, SW_HIDE);
                let _ = SetParent(views, Some(parking));
            }
            let _ = DestroyWindow(self.container);
        }
    }

    /// The browser is gone (`on_before_close`): its window goes with it.
    pub(crate) fn browser_closed(&self) {
        self.teardown();
        self.browser_view.borrow_mut().take();
        if let Some(window) = self.window.borrow_mut().take() {
            window.close();
        }
    }

    /// An action click ran: the next top-level window of this thread to appear is its popup.
    pub(crate) fn track_popup(&self, id: &str, anchor: Anchor) {
        self.untrack_popup();
        *self.popup.borrow_mut() = Some(PopupTrack {
            id: id.to_owned(),
            anchor,
            before: thread_windows(),
            window: None,
            deadline: Instant::now() + crate::extensions::POPUP_SEARCH,
            reported: false,
        });
        self.poll_popup();
    }

    pub(crate) fn untrack_popup(&self) {
        let Some(track) = self.popup.borrow_mut().take() else { return };
        if let Some(window) = track.window {
            // SAFETY: removes our own subclass from a window of this thread.
            let _ = unsafe { RemoveWindowSubclass(window, Some(pin), PIN) };
        }
    }

    fn poll_popup(&self) {
        let id = self.page;
        crate::runtime::later(POPUP_POLL, move || {
            if let Some(chrome) = page::get(id).and_then(|p| p.view.chrome()) {
                chrome.find_popup();
            }
        });
    }

    fn find_popup(&self) {
        let parent = self.st.borrow().parent;
        let found = {
            let popup = self.popup.borrow();
            let Some(track) = popup.as_ref() else { return };
            if track.window.is_some() {
                return;
            }
            if Instant::now() > track.deadline {
                drop(popup);
                self.untrack_popup();
                return;
            }
            let parking = PARKING.with(Cell::get);
            thread_windows()
                .into_iter()
                .find(|w| {
                    !track.before.contains(w)
                        && Some(*w) != parent.map(|p| p.0 as isize)
                        && Some(*w) != parking.map(|p| p.0 as isize)
                })
                .map(|w| HWND(w as *mut c_void))
        };
        let Some(window) = found else { return self.poll_popup() };
        if let Some(track) = self.popup.borrow_mut().as_mut() {
            track.window = Some(window);
        }
        // SAFETY: subclasses a window of this thread; `pin` removes itself on WM_NCDESTROY.
        unsafe {
            let _ = SetWindowSubclass(window, Some(pin), PIN, self.page as usize);
            // No size or position of its own: `pin` puts it under the anchor.
            let _ = SetWindowPos(window, None, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.report_popup(window);
    }

    /// Where the popup goes at size `w` by `h` (device pixels, screen coordinates): its top edge
    /// on the anchor's bottom, its leading edge on the anchor's, kept on the monitor's work area
    /// (above the anchor when there is no room under it).
    fn popup_origin(&self, w: i32, h: i32) -> Option<(i32, i32)> {
        let parent = self.st.borrow().parent?;
        let anchor = self.popup.borrow().as_ref()?.anchor;
        let (origin, scale) = client_origin(parent);
        let px = |v: f64| (v * scale).round() as i32;
        let (ax, ay, aw, ah) = (origin.x + px(anchor.x), origin.y + px(anchor.y), px(anchor.width), px(anchor.height));
        let work = work_area(parent);
        let mut x = ax;
        if x + w > work.right {
            x = ax + aw - w;
        }
        x = x.max(work.left);
        let mut y = ay + ah;
        if y + h > work.bottom {
            y = ay - h;
        }
        Some((x, y))
    }

    /// Tells the page where its popup is, once, in the window's logical coordinates.
    fn report_popup(&self, window: HWND) {
        let Some(parent) = self.st.borrow().parent else { return };
        let id = {
            let mut popup = self.popup.borrow_mut();
            let Some(track) = popup.as_mut() else { return };
            if std::mem::replace(&mut track.reported, true) {
                return;
            }
            track.id.clone()
        };
        let mut rect = RECT::default();
        // SAFETY: reads the rectangle of a window of this thread.
        if unsafe { GetWindowRect(window, &mut rect) }.is_err() {
            return;
        }
        let (origin, scale) = client_origin(parent);
        let dip = |v: i32| f64::from(v) / scale;
        let bounds = Anchor::new(
            dip(rect.left - origin.x),
            dip(rect.top - origin.y),
            dip(rect.right - rect.left),
            dip(rect.bottom - rect.top),
        );
        if let Some(page) = self.page() {
            page.page.emit(nd_webview::ExtensionPopupOpened { id, bounds, page: None });
        }
    }
}

/// `hwnd`'s region from `rects` (container coordinates); the whole frame clears it.
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn set_region(hwnd: HWND, frame: R, rects: &[R]) {
    if rects == [R::new(0, 0, frame.w, frame.h)] {
        // SAFETY: per the caller.
        unsafe { SetWindowRgn(hwnd, None, true) };
        return;
    }
    // SAFETY: regions made here; SetWindowRgn owns the combined one on success.
    unsafe {
        let region = CreateRectRgn(0, 0, 0, 0);
        for r in rects {
            let piece = CreateRectRgn(r.x, r.y, r.x + r.w, r.y + r.h);
            CombineRgn(Some(region), Some(region), Some(piece), RGN_OR);
            let _ = DeleteObject(HGDIOBJ(piece.0));
        }
        if SetWindowRgn(hwnd, Some(region), true) == 0 {
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
    }
}

/// The screen position of `hwnd`'s client area and its scale (device pixels per logical one).
fn client_origin(hwnd: HWND) -> (POINT, f64) {
    let mut origin = POINT::default();
    // SAFETY: reads geometry of a window of this thread.
    let dpi = unsafe {
        let _ = ClientToScreen(hwnd, &mut origin);
        GetDpiForWindow(hwnd)
    };
    (origin, if dpi == 0 { 1. } else { f64::from(dpi) / 96. })
}

/// The work area of the monitor `hwnd` is on.
fn work_area(hwnd: HWND) -> RECT {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..MONITORINFO::default() };
    // SAFETY: fills `info`, whose size is set.
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let _ = GetMonitorInfoW(monitor, &mut info);
    }
    info.rcWork
}

/// The visible top-level windows of this thread.
pub(crate) fn thread_windows() -> Vec<isize> {
    unsafe extern "system" fn collect(hwnd: HWND, out: LPARAM) -> BOOL {
        // SAFETY: `out` is the vector `thread_windows` passed, alive for the call.
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                (*(out.0 as *mut Vec<isize>)).push(hwnd.0 as isize);
            }
        }
        BOOL(1)
    }
    let mut out: Vec<isize> = Vec::new();
    // SAFETY: the callback only runs during the call.
    let _ =
        unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(collect), LPARAM(&mut out as *mut Vec<isize> as isize)) };
    out
}

/// A pinned popup's window procedure: every move or resize Chromium makes lands under the
/// anchor of the page whose id is `page`.
unsafe extern "system" fn pin(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    page: usize,
) -> LRESULT {
    if msg == WM_WINDOWPOSCHANGING && lparam.0 != 0 {
        // SAFETY: WM_WINDOWPOSCHANGING's lParam is the WINDOWPOS about to be applied.
        let pos = unsafe { &mut *(lparam.0 as *mut WINDOWPOS) };
        let (w, h) = if pos.flags.contains(SWP_NOSIZE) {
            let mut rect = RECT::default();
            // SAFETY: reads the rectangle of the window being positioned.
            let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
            (rect.right - rect.left, rect.bottom - rect.top)
        } else {
            (pos.cx, pos.cy)
        };
        let chrome = page::get(page as u64).and_then(|p| p.view.chrome());
        if let Some((x, y)) = chrome.and_then(|c| c.popup_origin(w, h)) {
            pos.x = x;
            pos.y = y;
            pos.flags &= !SWP_NOMOVE;
        }
    }
    if msg == WM_NCDESTROY {
        // SAFETY: removes this subclass from the window being destroyed.
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(pin), PIN) };
    }
    // SAFETY: the arguments are the ones Windows passed this subclass.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

wrap_browser_view_delegate! {
    struct NdBrowserView {
        id: u64,
    }

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        fn browser_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::CHROME
        }

        /// A normal toolbar, hidden once the window exists: Chrome's extension container hangs off
        /// it, and with none (what a native-parent browser gets) extension actions crash.
        fn chrome_toolbar_type(&self, _browser_view: Option<&mut BrowserView>) -> ChromeToolbarType {
            ChromeToolbarType::NORMAL
        }

        fn delegate_for_popup_browser_view(
            &self,
            _browser_view: Option<&mut BrowserView>,
            _settings: Option<&BrowserSettings>,
            _client: Option<&mut Client>,
            is_devtools: std::os::raw::c_int,
        ) -> Option<BrowserViewDelegate> {
            if is_devtools != 0 {
                return None;
            }
            PENDING_POPUPS.with(|p| p.borrow().front().copied()).map(NdBrowserView::new)
        }

        /// A popup the opener's page adopted gets a window of its own, like any page; anything
        /// else (DevTools) gets Chromium's default window.
        fn on_popup_browser_view_created(
            &self,
            _browser_view: Option<&mut BrowserView>,
            popup_browser_view: Option<&mut BrowserView>,
            is_devtools: std::os::raw::c_int,
        ) -> std::os::raw::c_int {
            if is_devtools != 0 {
                return 0;
            }
            let (Some(popup), Some(child)) = (popup_browser_view, PENDING_POPUPS.with(|p| p.borrow_mut().pop_front()))
            else {
                return 0;
            };
            let Some(chrome) = page::get(child).and_then(|p| p.view.chrome()) else { return 0 };
            match chrome.adopt(popup.clone()) {
                Ok(()) => 1,
                Err(e) => {
                    tracing::warn!("nd-cef: {e}");
                    0
                }
            }
        }
    }
}

wrap_window_delegate! {
    struct NdChromeWindow {
        id: u64,
    }

    impl ViewDelegate {}

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut cef::Window>) {
            if let (Some(window), Some(chrome)) = (window, page::get(self.id).and_then(|p| p.view.chrome())) {
                chrome.window_created(window);
            }
        }

        fn window_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::CHROME
        }

        fn is_frameless(&self, _window: Option<&mut cef::Window>) -> std::os::raw::c_int {
            1
        }

        fn can_resize(&self, _window: Option<&mut cef::Window>) -> std::os::raw::c_int {
            0
        }

        fn can_maximize(&self, _window: Option<&mut cef::Window>) -> std::os::raw::c_int {
            0
        }

        fn can_minimize(&self, _window: Option<&mut cef::Window>) -> std::os::raw::c_int {
            0
        }

        /// The browser's close reaches its window here. One the app started (the page closing for
        /// good) proceeds; one the page asked for (`window.close()`, an approved `request_close`)
        /// goes to the app, which unmounts the page when it agrees.
        fn can_close(&self, _window: Option<&mut cef::Window>) -> std::os::raw::c_int {
            page::get(self.id).map_or(1, |p| std::os::raw::c_int::from(!p.do_close()))
        }

        fn initial_bounds(&self, _window: Option<&mut cef::Window>) -> Rect {
            Rect { x: 0, y: 0, width: 800, height: 600 }
        }
    }
}
