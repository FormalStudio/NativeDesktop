//! Chromium's browser-wide DevTools session, over `--remote-debugging-pipe`: the one session
//! that may drive what belongs to no page (targets, extensions). A page's own session
//! (`devtools.rs`) cannot: Chromium answers its `Extensions` and `Target.attachToBrowserTarget`
//! calls with "not allowed".
//!
//! On macOS and Linux Chromium reads the pipe's requests from fd 3 and writes replies to fd 4, of
//! its own process, which is this one. Both are claimed in [`reserve`], the first thing the app
//! runs, before anything else can take those numbers; descriptors a launcher left there move up.
//! A run with no session (socket activation owns 3 and 4) says so from the calls that need it.
//! On Windows the CRT that would map fds 3 and 4 is libcef's own, so Chromium's ends are two
//! anonymous pipes named by handle value in `--remote-debugging-io-pipes`; the browser process
//! is this one, so the handles need no inheritance. Messages are JSON, each ended by a NUL. A
//! reader thread collects Chromium's and posts a task to the main thread, where replies and
//! events are dispatched.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::fd::{FromRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use cef::*;
use serde_json::{Value, json};

#[cfg(unix)]
const CHROME_READS: i32 = 3;
#[cfg(unix)]
const CHROME_WRITES: i32 = 4;

type Reply = Box<dyn FnOnce(Result<Value, String>)>;
type Listener = Rc<dyn Fn(&str, &Value, Option<&str>)>;

/// Our ends: what we write Chromium reads (fd 3, or its read handle), what Chromium writes (fd 4,
/// or its write handle) we read.
struct Ends {
    to_chrome: std::fs::File,
    from_chrome: std::fs::File,
}

static RESERVED: OnceLock<Mutex<Option<Ends>>> = OnceLock::new();
static INBOX: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());
/// `--remote-debugging-io-pipes`: Chromium's read and write handles.
#[cfg(windows)]
static IO_PIPES: OnceLock<String> = OnceLock::new();

thread_local! {
    static WRITER: RefCell<Option<std::fs::File>> = const { RefCell::new(None) };
    static NEXT: Cell<i64> = const { Cell::new(0) };
    static PENDING: RefCell<HashMap<i64, Reply>> = RefCell::default();
    static LISTENERS: RefCell<Vec<Listener>> = const { RefCell::new(Vec::new()) };
    static BROKEN: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[cfg(unix)]
fn is_open(fd: i32) -> bool {
    // SAFETY: F_GETFD only reads the descriptor's flags.
    unsafe { libc::fcntl(fd, libc::F_GETFD) != -1 }
}

/// A pipe whose descriptors sit above 3 and 4, so placing Chromium's ends there cannot close
/// one of them. Both are close-on-exec.
#[cfg(unix)]
fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors pipe(2) writes.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut high = [-1; 2];
    for (i, fd) in fds.into_iter().enumerate() {
        // SAFETY: duplicates a descriptor this process owns, then closes the original.
        unsafe {
            high[i] = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10);
            libc::close(fd);
        }
    }
    if high.contains(&-1) {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both were just created and are owned by nobody else.
    Ok(unsafe { (OwnedFd::from_raw_fd(high[0]), OwnedFd::from_raw_fd(high[1])) })
}

#[cfg(unix)]
fn cloexec(fd: i32) {
    // SAFETY: sets a flag on a descriptor this process owns.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
}

/// Claims fds 3 and 4 for Chromium's pipe. Runs once, first thing in the browser process.
#[cfg(unix)]
pub(crate) fn reserve() {
    RESERVED.get_or_init(|| {
        if std::env::var_os("LISTEN_FDS").is_some() && (is_open(CHROME_READS) || is_open(CHROME_WRITES)) {
            tracing::warn!(
                "nd-cef: fds 3 and 4 are socket-activation descriptors, so extensions are unavailable in this run"
            );
            return Mutex::new(None);
        }
        // A launcher can leak descriptors into the app (cargo's jobserver does). Whatever sits at 3
        // or 4 stays open at a higher number, so nothing holding it loses it.
        for fd in [CHROME_READS, CHROME_WRITES] {
            if is_open(fd) {
                // SAFETY: duplicates a descriptor this process holds; the copy is never closed.
                let moved = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) };
                if moved == -1 {
                    tracing::warn!(
                        "nd-cef: fd {fd} is taken and cannot move, so extensions are unavailable in this run"
                    );
                    return Mutex::new(None);
                }
                tracing::debug!("nd-cef: fd {fd} was taken at launch; it is now fd {moved}");
            }
        }
        let claim = || -> std::io::Result<Ends> {
            let (chrome_reads, to_chrome) = pipe()?;
            let (from_chrome, chrome_writes) = pipe()?;
            // SAFETY: duplicates our descriptors onto the two free numbers Chromium expects.
            unsafe {
                if libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&chrome_reads), CHROME_READS) == -1
                    || libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&chrome_writes), CHROME_WRITES) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            // Children (other than Chromium's own, which get an explicit descriptor map) must not
            // hold the pipe open.
            for fd in [CHROME_READS, CHROME_WRITES] {
                cloexec(fd);
            }
            Ok(Ends { to_chrome: std::fs::File::from(to_chrome), from_chrome: std::fs::File::from(from_chrome) })
        };
        match claim() {
            Ok(ends) => Mutex::new(Some(ends)),
            Err(e) => {
                tracing::warn!("nd-cef: the DevTools pipe: {e}");
                Mutex::new(None)
            }
        }
    });
}

/// Makes Chromium's two pipes. Runs once, first thing in the browser process, before
/// `CefInitialize` reads the switch.
#[cfg(windows)]
pub(crate) fn reserve() {
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Pipes::CreatePipe;
    RESERVED.get_or_init(|| {
        let pipe = || -> windows::core::Result<(HANDLE, HANDLE)> {
            let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
            // SAFETY: CreatePipe writes the two handles it creates.
            unsafe { CreatePipe(&mut read, &mut write, None, 1 << 20)? };
            Ok((read, write))
        };
        match pipe().and_then(|to| pipe().map(|from| (to, from))) {
            Ok(((chrome_reads, to_chrome), (from_chrome, chrome_writes))) => {
                // Chromium's ends stay open for the life of the process: Chromium owns them.
                let _ = IO_PIPES.set(format!("{},{}", chrome_reads.0 as usize, chrome_writes.0 as usize));
                // SAFETY: our ends were just created and are owned by nobody else.
                let ends = unsafe {
                    Ends {
                        to_chrome: std::fs::File::from_raw_handle(to_chrome.0),
                        from_chrome: std::fs::File::from_raw_handle(from_chrome.0),
                    }
                };
                Mutex::new(Some(ends))
            }
            Err(e) => {
                tracing::warn!("nd-cef: the DevTools pipe: {e}");
                Mutex::new(None)
            }
        }
    });
}

fn reserved() -> bool {
    RESERVED.get().is_some_and(|r| r.lock().is_ok_and(|r| r.is_some()))
}

/// The browser process's switches for the pipe. `AutomationControlled` is what the pipe would
/// otherwise turn on: `navigator.webdriver`, which sites read as a bot.
pub(crate) fn switches(line: &mut CommandLine) {
    if !reserved() {
        return;
    }
    line.append_switch(Some(&CefString::from("remote-debugging-pipe")));
    #[cfg(windows)]
    if let Some(pipes) = IO_PIPES.get() {
        line.append_switch_with_value(
            Some(&CefString::from("remote-debugging-io-pipes")),
            Some(&CefString::from(pipes.as_str())),
        );
    }
    line.append_switch(Some(&CefString::from("enable-unsafe-extension-debugging")));
    let name = CefString::from("disable-blink-features");
    let existing = crate::text(&line.switch_value(Some(&name)));
    let value = if existing.is_empty() {
        "AutomationControlled".to_owned()
    } else {
        format!("{existing},AutomationControlled")
    };
    line.append_switch_with_value(Some(&name), Some(&CefString::from(value.as_str())));
}

/// Starts reading Chromium's side, once Chromium is up.
pub(crate) fn start() {
    let Some(ends) = RESERVED.get().and_then(|r| r.lock().ok()?.take()) else { return };
    WRITER.with(|w| *w.borrow_mut() = Some(ends.to_chrome));
    let mut reader = ends.from_chrome;
    let spawned = std::thread::Builder::new().name("nd-cef-devtools-pipe".into()).spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        let mut message = Vec::new();
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            let mut any = false;
            for &b in &buf[..n] {
                if b == 0 {
                    if let Ok(mut inbox) = INBOX.lock() {
                        inbox.push_back(std::mem::take(&mut message));
                    }
                    any = true;
                } else {
                    message.push(b);
                }
            }
            if any {
                post_task(ThreadId::UI, Some(&mut Drain::new()));
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("nd-cef: the DevTools pipe reader: {e}");
    }
}

wrap_task! {
    struct Drain;

    impl Task {
        fn execute(&self) {
            drain();
        }
    }
}

fn drain() {
    loop {
        let Some(bytes) = INBOX.lock().ok().and_then(|mut i| i.pop_front()) else { return };
        let Ok(message) = serde_json::from_slice::<Value>(&bytes) else { continue };
        if let Some(id) = message["id"].as_i64() {
            let Some(reply) = PENDING.with(|p| p.borrow_mut().remove(&id)) else { continue };
            match message.get("error") {
                Some(error) => reply(Err(error["message"].as_str().map_or_else(|| error.to_string(), str::to_owned))),
                None => reply(Ok(message["result"].clone())),
            }
        } else if let Some(method) = message["method"].as_str() {
            let session = message["sessionId"].as_str();
            let listeners = LISTENERS.with(|l| l.borrow().clone());
            for listener in listeners {
                listener(method, &message["params"], session);
            }
        }
    }
}

/// Whether the session exists: `None`, or why not.
pub(crate) fn unavailable() -> Option<String> {
    if let Some(broken) = BROKEN.with(|b| b.borrow().clone()) {
        return Some(broken);
    }
    let up = WRITER.with(|w| w.borrow().is_some());
    (!up).then(|| "Chromium's browser DevTools session is not running (see the nd-cef warning at launch)".into())
}

/// Sends `method` on the browser's session, or on the flat `session` of a target attached
/// through it.
pub(crate) fn call(
    method: &str,
    params: Value,
    session: Option<&str>,
    reply: impl FnOnce(Result<Value, String>) + 'static,
) {
    if let Some(why) = unavailable() {
        reply(Err(why));
        return;
    }
    let id = NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let mut message = json!({ "id": id, "method": method, "params": params });
    if let Some(session) = session {
        message["sessionId"] = json!(session);
    }
    let mut bytes = message.to_string().into_bytes();
    bytes.push(0);
    PENDING.with(|p| p.borrow_mut().insert(id, Box::new(reply)));
    let written = WRITER.with(|w| w.borrow_mut().as_mut().map(|w| w.write_all(&bytes)));
    if let Some(Err(e)) = written {
        let why = format!("the DevTools pipe broke: {e}");
        BROKEN.with(|b| *b.borrow_mut() = Some(why.clone()));
        if let Some(reply) = PENDING.with(|p| p.borrow_mut().remove(&id)) {
            reply(Err(why));
        }
    }
}

/// Hears every event on the browser's session and the sessions attached through it.
pub(crate) fn listen(f: impl Fn(&str, &Value, Option<&str>) + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Rc::new(f)));
}
