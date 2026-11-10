//! The page in a GPUI window, the same on Linux and Windows: the element that draws Chromium's
//! off-screen frames and forwards input to it, the message pump on GPUI's main loop, dialogs and
//! input conversion. A Chrome-style page is a native child window of the platform's
//! (`platform::chrome`), which the element places and shapes around overlays ([`shape`]).

pub(crate) mod dialogs;
pub(crate) mod keys;
pub(crate) mod pump;
pub(crate) mod shape;
pub(crate) mod view;
