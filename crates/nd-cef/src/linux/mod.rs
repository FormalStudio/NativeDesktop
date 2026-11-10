//! The Linux side: where `libcef.so` and its resources are, the switches Chromium needs on a
//! Wayland or X11 desktop, Chrome-style pages as X11 child windows, and the GLib work Chrome
//! style needs pumped. The element, the pump and the dialogs are shared with Windows
//! (`crate::gpui`).

pub(crate) mod chrome;
pub(crate) mod install_prompt;
pub(crate) mod keys;
mod print;

use std::ffi::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

pub(crate) use crate::gpui::{dialogs, pump, view};

use cef::*;

/// What Chromium reads at startup besides `libcef.so`, all from the same directory.
const RESOURCES: [&str; 4] = ["icudtl.dat", "resources.pak", "v8_context_snapshot.bin", "locales"];

/// The directory `libcef.so` was loaded from: the executable links it, so it is mapped by the
/// time any code runs.
fn library_dir() -> Option<PathBuf> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    maps.lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .find(|path| path.ends_with("/libcef.so"))
        .and_then(|path| Path::new(path).parent().map(Path::to_owned))
}

/// Where Chromium's resources are: `ND_CEF_DIR`, else beside `libcef.so`.
fn runtime_dir() -> Result<PathBuf, String> {
    let dir = std::env::var_os("ND_CEF_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(library_dir)
        .ok_or("libcef.so is not loaded")?;
    match RESOURCES.iter().find(|name| !dir.join(name).exists()) {
        Some(missing) => Err(format!(
            "the Chromium runtime at {} is incomplete ({missing} is missing): ship the whole CEF distribution \
             beside libcef.so, or point ND_CEF_DIR at it",
            dir.display()
        )),
        None => Ok(dir),
    }
}

/// Checks the runtime and picks the API version; `libcef.so` itself is loaded with the
/// executable. Every wrapper object reports the version set here, and Chromium aborts on the
/// first one if none was.
pub(crate) fn load() -> Result<(), String> {
    runtime_dir()?;
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    Ok(())
}

/// The resources, and the app's own executable as every subprocess.
pub(crate) fn settings() -> Result<Settings, String> {
    let dir = runtime_dir()?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let s = |p: &Path| CefString::from(p.to_string_lossy().as_ref());
    Ok(Settings {
        browser_subprocess_path: s(&exe),
        resources_dir_path: s(&dir),
        locales_dir_path: s(&dir.join("locales")),
        ..Settings::default()
    })
}

/// Switches for the browser process. Off-screen pages need no window of Chromium's, but Ozone
/// still connects to a display server: the one the app runs on, or none.
pub(crate) fn command_line(line: &mut CommandLine) {
    let platform = std::env::var("ND_CEF_OZONE").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| {
        let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        if nd_backend_gpui::on_x11() {
            "x11".into()
        } else if set("WAYLAND_DISPLAY") {
            "wayland".into()
        } else if set("DISPLAY") {
            "x11".into()
        } else {
            "headless".into()
        }
    });
    line.append_switch_with_value(Some(&CefString::from("ozone-platform")), Some(&CefString::from(platform.as_str())));
}

/// Runs `f` on a later turn of the main loop, outside whatever CEF callback is on the stack.
pub(crate) fn run_later(f: impl FnOnce() + 'static) {
    pump::later(Box::new(f));
}

/// Printing through the desktop portal: Chromium has no print dialog of its own on Linux.
pub(crate) fn print_handler() -> Option<PrintHandler> {
    Some(print::handler())
}

/// Runs `f` on the main thread, from any thread.
pub(crate) fn on_main(f: impl FnOnce() + Send + 'static) {
    pump::send(pump::Msg::Run(Box::new(f)));
}

/// How often the pump runs Chromium while a Chrome-style page is open: Chromium reads its X
/// events only when polled.
pub(crate) fn pump_fallback() -> Option<Duration> {
    (chrome::live() > 0).then_some(Duration::from_millis(4))
}

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// After each `do_message_loop_work`, while a Chrome-style page is open: dispatches what is
/// ready on glib's default context. Chromium's UI loop on Linux is glib's, and a Chrome-style
/// page's X connection is a source on that context, which CEF leaves to the app's GTK loop under
/// `external_message_pump`; GPUI runs none. glib comes in with libcef.
pub(crate) fn after_work() {
    if chrome::live() == 0 {
        return;
    }
    type Iteration = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
    static ITERATION: OnceLock<Option<Iteration>> = OnceLock::new();
    let iteration = ITERATION.get_or_init(|| {
        // SAFETY: a lookup in the loaded objects; the symbol has this signature.
        let f = unsafe { dlsym(std::ptr::null_mut(), c"g_main_context_iteration".as_ptr()) };
        (!f.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, Iteration>(f) })
    });
    let Some(iteration) = iteration else { return };
    // A bound, so a source that is always ready cannot hold the main thread.
    for _ in 0..64 {
        // SAFETY: the default context (null), without blocking, on the thread Chromium runs on.
        if unsafe { iteration(std::ptr::null_mut(), 0) } == 0 {
            break;
        }
    }
}
