//! The theme file watcher on the real file system: a direct write, a half-written file that
//! must keep the last theme, and an atomic rename that replaces the inode.

use std::sync::mpsc;
use std::time::Duration;

use nd_theme::{MatugenTheme, ThemeFileWatcher};

fn json(primary: &str) -> String {
    format!(r##"{{"colors":{{"primary":{{"light":"{primary}","dark":"{primary}"}}}}}}"##)
}

fn accent(theme: &MatugenTheme) -> String {
    nd_theme::color::to_hex(theme.palettes.light.accent.expect("accent"))
}

#[test]
fn reads_writes_skips_half_files_and_survives_renames() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("theme.json");
    std::fs::write(&path, json("#111111")).unwrap();
    let (tx, rx) = mpsc::channel();
    let _watcher = ThemeFileWatcher::spawn(&path, MatugenTheme::parse, move |t| tx.send(accent(&t)).unwrap()).unwrap();
    let next = || rx.recv_timeout(Duration::from_secs(3)).expect("a theme within 3 s");

    assert_eq!(next(), "#111111", "the file is read at start");

    std::fs::write(&path, json("#222222")).unwrap();
    assert_eq!(next(), "#222222", "a write is picked up");

    std::fs::write(&path, r##"{"colors":{"primary":{"light":"#33"##).unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(400)).is_err(), "a half-written file keeps the last theme");

    let tmp = dir.path().join("theme.json.tmp");
    std::fs::write(&tmp, json("#444444")).unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    assert_eq!(next(), "#444444", "an atomic rename is picked up");

    std::fs::write(&path, json("#555555")).unwrap();
    assert_eq!(next(), "#555555", "the watch survived the rename");
}

/// The opens of `name` under `dir` that another watch on the same directory sees.
#[cfg(target_os = "linux")]
fn count_opens(
    dir: &std::path::Path,
    name: &'static str,
) -> (notify::RecommendedWatcher, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use notify::Watcher;
    use notify::event::{AccessKind, EventKind};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let opens = std::sync::Arc::new(AtomicUsize::new(0));
    let seen = opens.clone();
    let mut watch = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        let Ok(e) = e else { return };
        let ours = e.paths.iter().any(|p| p.file_name().is_some_and(|n| n == name));
        if ours && matches!(e.kind, EventKind::Access(AccessKind::Open(_))) {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    })
    .unwrap();
    watch.watch(dir, notify::RecursiveMode::NonRecursive).unwrap();
    (watch, opens)
}

#[cfg(target_os = "linux")]
#[test]
fn reading_the_file_reads_nothing_again_and_a_write_does() {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("theme.json");
    std::fs::write(&path, json("#111111")).unwrap();
    let (tx, rx) = mpsc::channel();
    let _watcher = ThemeFileWatcher::spawn(&path, MatugenTheme::parse, move |t| tx.send(accent(&t)).unwrap()).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "#111111");

    let (_watch, opens) = count_opens(dir.path(), "theme.json");
    for _ in 0..5 {
        std::fs::read_to_string(&path).unwrap();
    }
    std::thread::sleep(Duration::from_millis(300));
    opens.store(0, Ordering::SeqCst);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(opens.load(Ordering::SeqCst), 0, "nothing opens the file while nobody writes it");

    std::fs::write(&path, json("#222222")).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "#222222", "a write is read");
    std::thread::sleep(Duration::from_millis(300));
    opens.store(0, Ordering::SeqCst);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(opens.load(Ordering::SeqCst), 0, "the read after a write raises no read after it");
}

#[cfg(unix)]
#[test]
fn a_symlinked_file_is_read_through_writes_and_retargets() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let shell = dir.path().join("shell");
    std::fs::create_dir_all(&shell).unwrap();
    std::fs::write(a.join("one.json"), json("#111111")).unwrap();
    std::fs::write(b.join("two.json"), json("#333333")).unwrap();
    let path = shell.join("theme.json");
    std::os::unix::fs::symlink(a.join("one.json"), &path).unwrap();
    let (tx, rx) = mpsc::channel();
    let _watcher = ThemeFileWatcher::spawn(&path, MatugenTheme::parse, move |t| tx.send(accent(&t)).unwrap()).unwrap();
    // Following a retargeted link restarts the FSEvents stream, which takes seconds on a busy Mac.
    let next = || rx.recv_timeout(Duration::from_secs(10)).expect("a theme within 10 s");
    assert_eq!(next(), "#111111");

    std::fs::write(a.join("one.json"), json("#222222")).unwrap();
    assert_eq!(next(), "#222222", "a write to the link's target is read");

    let tmp = shell.join("theme.json.new");
    std::os::unix::fs::symlink(b.join("two.json"), &tmp).unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    assert_eq!(next(), "#333333", "a retargeted link is read");

    std::fs::write(b.join("two.json"), json("#444444")).unwrap();
    assert_eq!(next(), "#444444", "the watch followed the link to its new target");
    std::fs::write(a.join("one.json"), json("#555555")).unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(400)).is_err(), "the old target is no longer the theme");
}
