//! The Windows side: where `libcef.dll` and its resources are, and Chrome-style pages as child
//! windows of the GPUI window. The off-screen element, the pump and the dialogs are the ones
//! Linux uses (`crate::gpui`); GPUI's own message loop dispatches the messages of Chromium's
//! windows, which live on the same thread.

pub(crate) mod chrome;
pub(crate) mod install_prompt;
pub(crate) mod keys;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cef::*;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::core::w;

pub(crate) use crate::gpui::{dialogs, pump, view};

/// What Chromium reads at startup besides `libcef.dll`, all from the same directory.
const RESOURCES: [&str; 5] = ["chrome_elf.dll", "icudtl.dat", "resources.pak", "v8_context_snapshot.bin", "locales"];

/// The directory `libcef.dll` was loaded from: the executable imports it, so it is loaded by
/// the time any code runs.
fn library_dir() -> Option<PathBuf> {
    // SAFETY: looks up a module this process already loaded; the handle is not freed.
    let module = unsafe { GetModuleHandleW(w!("libcef.dll")) }.ok()?;
    let mut buf = vec![0u16; 32 * 1024];
    // SAFETY: writes at most `buf.len()` UTF-16 units of the module's path into `buf`.
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    if len == 0 || len >= buf.len() {
        return None;
    }
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
    path.parent().map(Path::to_owned)
}

/// Where Chromium's resources are: `ND_CEF_DIR`, else beside `libcef.dll`.
fn runtime_dir() -> Result<PathBuf, String> {
    let dir = std::env::var_os("ND_CEF_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(library_dir)
        .ok_or("libcef.dll is not loaded")?;
    match RESOURCES.iter().find(|name| !dir.join(name).exists()) {
        Some(missing) => Err(format!(
            "the Chromium runtime at {} is incomplete ({missing} is missing): ship the whole CEF distribution \
             beside libcef.dll, or point ND_CEF_DIR at it",
            dir.display()
        )),
        None => Ok(dir),
    }
}

/// Checks the runtime and picks the API version; `libcef.dll` itself is loaded with the
/// executable. Every wrapper object reports the version set here.
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

/// The DevTools pipe's switch is `pipe.rs`'s; Windows needs no other.
pub(crate) fn command_line(_line: &mut CommandLine) {}

/// Runs `f` on a later turn of the main loop, outside whatever CEF callback is on the stack.
pub(crate) fn run_later(f: impl FnOnce() + 'static) {
    pump::later(Box::new(f));
}

/// Chromium prints through the system print dialog on Windows (`disable-print-preview`), owned
/// by the page's window, so it needs no handler of the app's.
pub(crate) fn print_handler() -> Option<PrintHandler> {
    None
}

/// Runs `f` on the main thread, from any thread.
pub(crate) fn on_main(f: impl FnOnce() + Send + 'static) {
    pump::send(pump::Msg::Run(Box::new(f)));
}

/// Chromium's windows get their messages from GPUI's loop, so the pump runs Chromium only when
/// it asks to be.
pub(crate) fn pump_fallback() -> Option<Duration> {
    None
}

pub(crate) fn after_work() {}
