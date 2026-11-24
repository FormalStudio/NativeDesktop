#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

pub use linux::{current_platform, linux_platform};
#[cfg(feature = "wayland")]
pub use linux::{SelectionKind, dnd, read_serial, selection, set_toplevel_tag};
