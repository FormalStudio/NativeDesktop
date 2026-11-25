//! The system's on-device language model (Foundation Models), through `NDSwift`
//! (`TextModel.swift`). It is absent when the app does not link `NDSwift`, the system predates
//! the framework, or Apple Intelligence is off or still downloading its model.

use std::ffi::{CStr, CString, c_char, c_void};

use crate::swift::lookup;

type Available = unsafe extern "C" fn() -> bool;
type Done = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char);
type Respond = unsafe extern "C" fn(*const c_char, *const c_char, *mut c_void, Done);

/// Runs once with the answer or why there is none.
pub type Reply = Box<dyn FnOnce(Result<String, String>) + Send>;

pub fn available() -> bool {
    let Some(f) = lookup("nd_swift_text_model_available") else { return false };
    // SAFETY: NDSwift's `@_cdecl` function with this signature.
    unsafe { std::mem::transmute::<*mut c_void, Available>(f)() }
}

/// One answer from a fresh session. Returns at once; `reply` runs on a thread of Swift's.
pub fn respond(instructions: &str, prompt: &str, reply: Reply) {
    let Some(f) = lookup("nd_swift_text_model_respond") else {
        return reply(Err("NDSwift is not linked".into()));
    };
    let (Ok(instructions), Ok(prompt)) = (CString::new(instructions), CString::new(prompt)) else {
        return reply(Err("the text holds a NUL".into()));
    };
    let context = Box::into_raw(Box::new(reply)).cast::<c_void>();
    // SAFETY: NDSwift's `@_cdecl` function with this signature. It copies both strings before
    // it returns and calls `done` exactly once with `context`.
    unsafe { std::mem::transmute::<*mut c_void, Respond>(f)(instructions.as_ptr(), prompt.as_ptr(), context, done) }
}

unsafe extern "C" fn done(context: *mut c_void, answer: *const c_char, error: *const c_char) {
    // SAFETY: `context` is the box `respond` leaked, handed back once.
    let reply = unsafe { Box::from_raw(context.cast::<Reply>()) };
    // SAFETY: Swift passes NUL-terminated strings that live for this call.
    let text = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    reply(if answer.is_null() { Err(text(error)) } else { Ok(text(answer)) });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_ndswift_there_is_no_model_and_the_answer_says_why() {
        assert!(!available());
        let (tx, rx) = std::sync::mpsc::channel();
        respond("Answer in one word.", "Sky colour?", Box::new(move |answer| tx.send(answer).unwrap()));
        assert_eq!(rx.recv().unwrap(), Err("NDSwift is not linked".to_owned()));
    }
}
