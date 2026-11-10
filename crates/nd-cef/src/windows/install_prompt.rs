//! Chromium's "Add <name>?" prompt for a Web Store install the app has already confirmed, as on
//! macOS (`mac/install_prompt.rs`): while an install is armed, the first new top-level window of
//! this thread big enough to be a dialog is taken as the prompt. It is cloaked, which hides it
//! and keeps clicks off it while Views still counts it as shown, and its accept button is
//! invoked through UI Automation. A thread CBT hook takes it when it activates, before its first
//! frame is composed. A prompt still up after [`HAND_BACK`] is shown for the user to answer.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationInvokePattern, TreeScope_Descendants, UIA_ButtonControlTypeId,
    UIA_ControlTypePropertyId, UIA_InvokePatternId,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetWindowRect, HCBT_ACTIVATE, HHOOK, IsWindow, IsWindowVisible,
    SetForegroundWindow, SetWindowsHookExW, UnhookWindowsHookEx, WH_CBT,
};
use windows::core::BOOL;

/// Views keeps the accept button disabled for its first half second on screen, and ignores a
/// click within a double-click interval of the dialog showing.
const FIRST_PRESS: Duration = Duration::from_millis(1200);
const HAND_BACK: Duration = Duration::from_secs(8);
const TICK: Duration = Duration::from_millis(8);
/// In DIPs.
const MIN_WIDTH: f64 = 200.;
const MIN_HEIGHT: f64 = 80.;

struct Armed {
    until: Instant,
    before: Vec<isize>,
    active_before: isize,
    prompt: Option<(isize, Instant)>,
    last_press: Option<Instant>,
    hook: Option<HHOOK>,
    generation: u64,
}

thread_local! {
    static ARMED: RefCell<Option<Armed>> = const { RefCell::new(None) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// The next prompt within `wait` is answered unseen.
pub(crate) fn arm(wait: Duration) {
    disarm();
    let generation = GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    // SAFETY: a hook on this thread only; `disarm` removes it.
    let hook = unsafe { SetWindowsHookExW(WH_CBT, Some(cbt), None, GetCurrentThreadId()) }
        .inspect_err(|e| tracing::debug!("nd-cef: install prompt hook: {e}"))
        .ok();
    ARMED.with(|a| {
        *a.borrow_mut() = Some(Armed {
            until: Instant::now() + wait,
            before: super::chrome::thread_windows(),
            // SAFETY: no arguments.
            active_before: unsafe { GetForegroundWindow() }.0 as isize,
            prompt: None,
            last_press: None,
            hook,
            generation,
        })
    });
    tick(generation);
    tracing::debug!("nd-cef: install prompt armed");
}

/// Stops watching; a prompt already taken and still up is handed back to the user.
pub(crate) fn disarm() {
    let Some(armed) = ARMED.with(|a| a.borrow_mut().take()) else { return };
    if let Some(hook) = armed.hook {
        // SAFETY: the hook `arm` set on this thread.
        let _ = unsafe { UnhookWindowsHookEx(hook) };
    }
    let Some((prompt, _)) = armed.prompt else { return };
    let prompt = hwnd(prompt);
    // SAFETY: checks and activates windows of this thread; a destroyed one is not visible.
    unsafe {
        if IsWindowVisible(prompt).as_bool() {
            hand_back(prompt);
        } else if armed.active_before != 0 && IsWindow(Some(hwnd(armed.active_before))).as_bool() {
            let _ = SetForegroundWindow(hwnd(armed.active_before));
        }
    }
}

fn hwnd(h: isize) -> HWND {
    HWND(h as *mut c_void)
}

fn hand_back(prompt: HWND) {
    tracing::warn!("nd-cef: Chromium's install prompt stayed up; it is shown for the user to answer");
    cloak(prompt, false);
    // SAFETY: activates a window of this thread.
    let _ = unsafe { SetForegroundWindow(prompt) };
}

fn cloak(window: HWND, on: bool) {
    let value = BOOL::from(on);
    // SAFETY: DWMWA_CLOAK takes a BOOL, passed with its size; the window is this process's.
    let set = unsafe {
        DwmSetWindowAttribute(window, DWMWA_CLOAK, &value as *const BOOL as *const c_void, size_of::<BOOL>() as u32)
    };
    if let Err(e) = set {
        tracing::debug!("nd-cef: cloaking the install prompt: {e}");
    }
}

fn tick(generation: u64) {
    crate::runtime::later(TICK, move || {
        if ARMED.with(|a| a.borrow().as_ref().is_some_and(|a| a.generation == generation)) {
            step();
            tick(generation);
        }
    });
}

/// Activation comes inside the `ShowWindow` that puts the dialog up, before it paints.
unsafe extern "system" fn cbt(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HCBT_ACTIVATE as i32 {
        take(wparam.0 as isize, false);
    }
    // SAFETY: passes the hook's own arguments on.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Takes `window` as the prompt if none is taken yet and it is new and big enough; `visible`
/// also asks that it be shown.
fn take(window: isize, visible: bool) -> bool {
    ARMED.with(|a| {
        // Windows calls the hook from inside the calls this makes.
        let Ok(mut a) = a.try_borrow_mut() else { return false };
        let Some(armed) = a.as_mut() else { return false };
        if armed.prompt.is_some() || armed.before.contains(&window) || !is_dialog(hwnd(window), visible) {
            return false;
        }
        cloak(hwnd(window), true);
        tracing::debug!("nd-cef: install prompt taken");
        armed.prompt = Some((window, Instant::now()));
        true
    })
}

fn is_dialog(window: HWND, visible: bool) -> bool {
    let mut rect = RECT::default();
    // SAFETY: reads the state of a window of this thread.
    unsafe {
        if (visible && !IsWindowVisible(window).as_bool()) || GetWindowRect(window, &mut rect).is_err() {
            return false;
        }
        let scale = f64::from(GetDpiForWindow(window).max(96)) / 96.;
        f64::from(rect.right - rect.left) / scale >= MIN_WIDTH
            && f64::from(rect.bottom - rect.top) / scale >= MIN_HEIGHT
    }
}

fn step() {
    enum Next {
        Wait,
        Find,
        Press(isize),
        Done,
    }
    let next = ARMED.with(|a| {
        let Ok(mut a) = a.try_borrow_mut() else { return Next::Wait };
        let Some(armed) = a.as_mut() else { return Next::Wait };
        let now = Instant::now();
        let Some((window, taken)) = armed.prompt else {
            return if now > armed.until { Next::Done } else { Next::Find };
        };
        if now.duration_since(taken) > HAND_BACK {
            return Next::Done;
        }
        // Views hides the dialog and shows it again while it sets up, so a hidden prompt is not
        // an answered one: the install's own result disarms this.
        // SAFETY: reads the state of a window of this thread.
        if !unsafe { IsWindowVisible(hwnd(window)) }.as_bool() {
            return Next::Wait;
        }
        // Views takes a click within a double-click interval of the last one as a repeat, and
        // ignores it and every one after it that comes as fast.
        // SAFETY: no arguments.
        let every = Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() })) + Duration::from_millis(300);
        if now.duration_since(taken) >= FIRST_PRESS && armed.last_press.is_none_or(|t| now.duration_since(t) >= every) {
            armed.last_press = Some(now);
            Next::Press(window)
        } else {
            Next::Wait
        }
    });
    match next {
        Next::Wait => {}
        Next::Find => {
            if let Some(window) = super::chrome::thread_windows().into_iter().find(|w| take(*w, true)) {
                tracing::debug!("nd-cef: install prompt found by polling ({window:#x})");
            }
        }
        // UI Automation asks the dialog's provider through messages to this thread, so it is
        // driven from another one while this thread keeps pumping.
        Next::Press(window) => {
            std::thread::spawn(move || press_accept(window));
        }
        Next::Done => disarm(),
    }
}

/// Views puts a dialog's accept button at the leading end of its bottom row on Windows.
fn press_accept(window: isize) {
    // SAFETY: this thread joins the multithreaded apartment for the calls below and leaves it
    // after every interface they returned is dropped.
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return;
        }
        if let Err(e) = invoke_accept(hwnd(window)) {
            tracing::debug!("nd-cef: install prompt accept: {e}");
        }
        CoUninitialize();
    }
}

/// # Safety
/// COM is initialised on this thread.
unsafe fn invoke_accept(window: HWND) -> windows::core::Result<()> {
    // SAFETY: plain UI Automation client calls; the caller initialised COM.
    unsafe {
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let root = uia.ElementFromHandle(window)?;
        let condition =
            uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_ButtonControlTypeId.0))?;
        let found = root.FindAll(TreeScope_Descendants, &condition)?;
        let mut buttons = Vec::new();
        for i in 0..found.Length()? {
            let button = found.GetElement(i)?;
            let rect = button.CurrentBoundingRectangle()?;
            let enabled = button.CurrentIsEnabled()?.as_bool();
            tracing::debug!("nd-cef: install prompt button {:?} enabled {enabled} at {rect:?}", button.CurrentName());
            buttons.push((button, rect, enabled));
        }
        let bottom = buttons.iter().map(|(_, r, _)| r.top).max().unwrap_or_default();
        let accept = buttons.iter().filter(|(_, r, _)| (r.top - bottom).abs() < 4).min_by_key(|(_, r, _)| r.left);
        let Some((accept, _, enabled)) = accept else {
            tracing::debug!("nd-cef: the install prompt shows no buttons yet");
            return Ok(());
        };
        if *enabled {
            accept.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?.Invoke()?;
            tracing::debug!("nd-cef: install prompt accept pressed ({} buttons)", buttons.len());
        }
        Ok(())
    }
}
