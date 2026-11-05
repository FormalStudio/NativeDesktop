//! Watches one theme file (`theme.json`, `theme.toml`) and hands each new, parseable version to
//! a callback on the watcher's thread. The parent directory is watched, not the file: atomic
//! rename writers replace the inode and a watch on the file would die with it. When the path is
//! a symlink, the directory of the file it points at is watched too, and the watch follows the
//! link when it is retargeted. Only writes count (a closed write, a create, a rename in, a
//! remove): the watcher's own reads raise open and close events on the same directory, and
//! counting those had every app read its palette again forever. The first write of a burst is
//! read at once (a recolour lands in one frame); writes in the next [`DEBOUNCE`] are read once
//! at its end. A file that fails to parse (half written, mid-edit) is skipped, so the last good
//! theme stays. Nothing runs between writes, except a retry every [`POLL`] while a watched
//! directory is gone.

// Modification times are std's type, and a browser tab has no theme files to watch.
#![allow(clippy::disallowed_types)]

use nd_core::time::{Duration, Instant};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use notify::event::{AccessKind, AccessMode, EventKind, ModifyKind};
use notify::{RecursiveMode, Watcher};

pub const DEBOUNCE: Duration = Duration::from_millis(50);
pub const POLL: Duration = Duration::from_secs(1);

/// Stops the watch when dropped.
pub struct ThemeFileWatcher {
    stop: Option<mpsc::Sender<Msg>>,
    thread: Option<thread::JoinHandle<()>>,
}

enum Msg {
    /// A write under a watched directory, with the paths it names. `rescan` is the OS saying
    /// it dropped events.
    Wrote {
        paths: Vec<PathBuf>,
        rescan: bool,
    },
    Stop,
}

/// Whether an event changes a file's contents or which file a name points at. Opens, reads and
/// metadata changes do not. On Linux every write ends in a close-write, so the modify events
/// before it are left out and the file is read once it is whole.
fn is_write(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Modify(ModifyKind::Data(_)) => !cfg!(any(target_os = "linux", target_os = "android")),
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_) => true,
        EventKind::Any | EventKind::Other => true,
    }
}

/// The directories to watch and the file paths inside them that are the theme: the path's own
/// directory and, when the path is a symlink, the directory of the file it resolves to.
#[derive(Clone, PartialEq, Eq)]
struct Targets {
    dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
}

/// The directory is canonical because FSEvents names paths by their real location (a temp dir
/// under `/var` arrives as `/private/var`), and events are matched against these paths.
fn targets(path: &Path, dir: &Path) -> Targets {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut t = Targets { dirs: vec![dir.clone()], files: Vec::new() };
    if let Some(name) = path.file_name() {
        t.files.push(dir.join(name));
    }
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if let Some(real) = is_link.then(|| std::fs::canonicalize(path).ok()).flatten() {
        if let Some(real_dir) = real.parent().filter(|d| !t.dirs.iter().any(|w| w == d)) {
            t.dirs.push(real_dir.to_path_buf());
        }
        t.files.push(real);
    }
    t
}

impl ThemeFileWatcher {
    /// Reads `path` now (when it exists) and on every change after. `parse` turns its text
    /// into a theme; `on_change` gets each parsed version that differs from the last.
    pub fn spawn<T, E>(
        path: impl Into<PathBuf>,
        parse: impl Fn(&str) -> Result<T, E> + Send + 'static,
        on_change: impl Fn(T) + Send + 'static,
    ) -> std::io::Result<ThemeFileWatcher>
    where
        T: Send + 'static,
        E: std::fmt::Display,
    {
        let path: PathBuf = path.into();
        let dir =
            path.parent().filter(|d| !d.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        std::fs::create_dir_all(&dir)?;
        let (tx, rx) = mpsc::channel();
        let events = tx.clone();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            let rescan = event.need_rescan();
            if rescan || is_write(&event.kind) {
                let _ = events.send(Msg::Wrote { paths: event.paths, rescan });
            }
        })
        .map_err(std::io::Error::other)?;
        let mut watching = targets(&path, &dir);
        for d in &watching.dirs {
            watcher.watch(d, RecursiveMode::NonRecursive).map_err(std::io::Error::other)?;
        }

        let thread = thread::Builder::new().name("nd-theme-watch".into()).spawn(move || {
            let mut last_text: Option<String> = None;
            let read = |last_text: &mut Option<String>| {
                let Ok(text) = std::fs::read_to_string(&path) else { return };
                if last_text.as_deref() == Some(text.as_str()) {
                    return;
                }
                match parse(&text) {
                    Ok(theme) => {
                        *last_text = Some(text);
                        on_change(theme);
                    }
                    Err(e) => eprintln!("nd: {} kept the last theme: {e}", path.display()),
                }
            };
            // Points the watch at the directories the path resolves to now. False while one is
            // missing, which leaves a retry every POLL until it is back.
            let mut rearm = |watching: &mut Targets, force: bool| -> bool {
                let next = targets(&path, &dir);
                if !force && next == *watching {
                    return true;
                }
                // Each watch or unwatch restarts the FSEvents stream (hundreds of ms on a busy
                // Mac), so only the directories that changed are touched, unless forced.
                for d in watching.dirs.iter().filter(|d| force || !next.dirs.contains(d)) {
                    let _ = watcher.unwatch(d);
                }
                let ok = next
                    .dirs
                    .iter()
                    .filter(|d| force || !watching.dirs.contains(d))
                    .all(|d| watcher.watch(d, RecursiveMode::NonRecursive).is_ok());
                *watching = next;
                ok
            };
            read(&mut last_text);
            // The end of the current burst, and whether a write landed after its first read.
            let mut burst: Option<Instant> = None;
            let mut pending = false;
            let mut broken = false;
            loop {
                let wait = match (burst, broken) {
                    (Some(end), _) => Some(end.saturating_duration_since(Instant::now())),
                    (None, true) => Some(POLL),
                    (None, false) => None,
                };
                let msg = match wait {
                    Some(wait) => rx.recv_timeout(wait),
                    None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                };
                match msg {
                    Ok(Msg::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Ok(Msg::Wrote { paths, rescan }) => {
                        let on_dir = paths.iter().any(|p| watching.dirs.contains(p));
                        let on_file = paths.iter().any(|p| watching.files.contains(p));
                        if !(rescan || on_dir || on_file) {
                            continue;
                        }
                        // A retargeted link or a directory event moves the watch, which restarts
                        // the FSEvents stream and can take a second, so the new file is read
                        // first, even inside a burst. A write to a directory that only then gets
                        // its watch would go unseen, so the burst ends with one more read.
                        let moved = on_dir || (on_file && targets(&path, &dir) != watching);
                        if burst.is_some() && !moved {
                            pending = true;
                        } else {
                            read(&mut last_text);
                            burst = Some(Instant::now() + DEBOUNCE);
                        }
                        if moved {
                            broken = !rearm(&mut watching, on_dir);
                            pending = true;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if burst.is_some_and(|end| Instant::now() >= end) {
                            burst = None;
                            if std::mem::take(&mut pending) {
                                read(&mut last_text);
                            }
                        } else if broken && burst.is_none() {
                            broken = !rearm(&mut watching, true);
                            if !broken {
                                read(&mut last_text);
                            }
                        }
                    }
                }
            }
        })?;
        Ok(ThemeFileWatcher { stop: Some(tx), thread: Some(thread) })
    }
}

impl Drop for ThemeFileWatcher {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(Msg::Stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
