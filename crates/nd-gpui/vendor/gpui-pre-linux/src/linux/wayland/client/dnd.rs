//! Drag and drop run by the embedding application. Once it sets a handler, the
//! data device's drags reach it rather than GPUI's file drop: every offer with
//! its mime types and actions, wherever it comes from, and the events of the
//! drags it starts itself with any mime types, actions and icon. A drag
//! between two windows of this client rides the same protocol as one from
//! another client.

use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use wayland_client::Proxy;
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::{wl_buffer, wl_data_offer, wl_data_source, wl_shm, wl_surface};
use wayland_protocols::wp::viewporter::client::wp_viewport;

use super::{DataSourceKind, WaylandClientStatePtr};
use crate::linux::wayland::clipboard::DataOffer;
use crate::linux::wayland::serial::SerialKind;

/// wl_data_device_manager.dnd_action bits.
pub const DND_COPY: u32 = 1;
pub const DND_MOVE: u32 = 2;
pub const DND_ASK: u32 = 4;

/// What the data device reports about a drag, to the handler.
#[derive(Clone, Debug)]
pub enum DndEvent {
    /// A drag entered one of this client's windows, named by its wl_surface
    /// as raw-window-handle gives it. The position is in logical pixels from
    /// the surface's top left.
    Enter {
        surface: usize,
        x: f64,
        y: f64,
        mime_types: Vec<String>,
        source_actions: u32,
    },
    Motion {
        x: f64,
        y: f64,
    },
    /// The drag left the window without a drop.
    Leave,
    /// The pointer was released over the window: the offer stays readable
    /// until `finish_drop` or `end_drop`.
    Drop,
    /// The source changed the actions it offers.
    SourceActions(u32),
    /// This client's drag: the mime type the destination accepts, if any.
    Target(Option<String>),
    /// This client's drag: the action the compositor chose.
    Action(u32),
    /// This client's drag: the pointer was released over a destination that
    /// takes it.
    DropPerformed,
    /// This client's drag: the destination is done with it.
    Finished,
    /// This client's drag ended without a drop, or the destination refused it.
    Cancelled,
}

/// The image that follows the pointer during a drag: premultiplied ARGB8888
/// rows, `width` by `height` device pixels at `scale`, with its top left at
/// (`x`, `y`) logical pixels from the pointer.
pub struct DragIcon {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub scale: f64,
    pub x: i32,
    pub y: i32,
}

type Handler = Box<dyn FnMut(DndEvent)>;

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
    static CLIENT: RefCell<Option<WaylandClientStatePtr>> = const { RefCell::new(None) };
}

/// Routes this thread's Wayland drags to `handler`. It runs on the thread
/// that runs GPUI, while no client state is borrowed, so it may call the
/// other functions here.
pub fn set_dnd_handler(handler: impl FnMut(DndEvent) + 'static) {
    HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
}

pub(crate) fn has_handler() -> bool {
    HANDLER.with(|h| h.borrow().is_some())
}

pub(crate) fn set_client(client: WaylandClientStatePtr) {
    CLIENT.with(|c| *c.borrow_mut() = Some(client));
}

/// Calls the handler, taken out for the call so that a drag it starts or
/// ends does not reenter it.
pub(crate) fn emit(event: DndEvent) {
    let Some(mut handler) = HANDLER.with(|h| h.borrow_mut().take()) else {
        return;
    };
    handler(event);
    HANDLER.with(|h| {
        let mut slot = h.borrow_mut();
        if slot.is_none() {
            *slot = Some(handler);
        }
    });
}

/// The drag over one of this client's windows, from its enter to its end.
pub(crate) struct DropState {
    pub offer: DataOffer<wl_data_offer::WlDataOffer>,
    pub serial: u32,
    pub dropped: bool,
}

/// This client's own drag, kept until it finishes or is cancelled.
pub(crate) struct SourceState {
    pub source: wl_data_source::WlDataSource,
    pub offers: Vec<(String, Vec<u8>)>,
    pub icon: Option<(wl_surface::WlSurface, wl_buffer::WlBuffer, Option<wp_viewport::WpViewport>)>,
}

impl SourceState {
    pub fn bytes(&self, mime_type: &str) -> Option<Vec<u8>> {
        self.offers.iter().find(|(m, _)| m == mime_type).map(|(_, b)| b.clone())
    }

    pub fn destroy(self) {
        self.source.destroy();
        if let Some((surface, buffer, viewport)) = self.icon {
            if let Some(viewport) = viewport {
                viewport.destroy();
            }
            surface.destroy();
            buffer.destroy();
        }
    }
}

#[derive(Default)]
pub(crate) struct DndState {
    pub drop: Option<DropState>,
    pub source: Option<SourceState>,
    /// The actions of the newest offer, which the source sends before the
    /// offer enters a window.
    pub offered_actions: Option<(wayland_client::backend::ObjectId, u32)>,
}

fn with_client<R>(f: impl FnOnce(&WaylandClientStatePtr) -> Option<R>) -> Option<R> {
    CLIENT.with(|c| c.borrow().as_ref().and_then(f))
}

/// Starts a drag from the window whose wl_surface is `surface`, with the
/// pointer press that began it as the grab, offering `offers` (mime type and
/// content) and `actions`. False when the window is not this client's or the
/// compositor has no data device.
pub fn start_drag(surface: usize, offers: Vec<(String, Vec<u8>)>, actions: u32, icon: Option<DragIcon>) -> bool {
    with_client(|ptr| {
        let client = ptr.get_client();
        let mut state = client.borrow_mut();
        let origin = state
            .windows
            .values()
            .map(|w| w.surface())
            .find(|s| s.id().as_ptr() as usize == surface)?;
        let manager = state.globals.data_device_manager.clone()?;
        let device = state.data_device.clone()?;
        if let Some(old) = state.dnd.source.take() {
            old.destroy();
        }
        let qh = state.globals.qh.clone();
        let source = manager.create_data_source(&qh, DataSourceKind::Drag);
        for (mime_type, _) in &offers {
            source.offer(mime_type.clone());
        }
        source.set_actions(DndAction::from_bits_truncate(actions));
        let icon_surface = icon.as_ref().map(|_| state.globals.compositor.create_surface(&qh, ()));
        let serial = state.serial_tracker.get(SerialKind::MousePress);
        device.start_drag(Some(&source), &origin, icon_surface.as_ref(), serial.as_raw());
        let icon = icon.zip(icon_surface).and_then(|(icon, surface)| {
            let buffer = icon_buffer(&state.globals.shm, &qh, &icon)?;
            let viewport = state.globals.viewporter.as_ref().map(|v| v.get_viewport(&surface, &qh, ()));
            // Before wl_surface 5 the offset rides on attach.
            if surface.version() >= wl_surface::REQ_OFFSET_SINCE {
                surface.attach(Some(&buffer), 0, 0);
                surface.offset(icon.x, icon.y);
            } else {
                surface.attach(Some(&buffer), icon.x, icon.y);
            }
            let (w, h) = (icon.width as f64 / icon.scale, icon.height as f64 / icon.scale);
            match &viewport {
                Some(viewport) => viewport.set_destination(w.round().max(1.0) as i32, h.round().max(1.0) as i32),
                None => surface.set_buffer_scale(icon.scale.round().max(1.0) as i32),
            }
            surface.damage_buffer(0, 0, icon.width as i32, icon.height as i32);
            surface.commit();
            Some((surface, buffer, viewport))
        });
        state.dnd.source = Some(SourceState { source, offers, icon });
        let _ = state.clipboard.connection().flush();
        Some(true)
    })
    .unwrap_or(false)
}

fn icon_buffer(
    shm: &wl_shm::WlShm,
    qh: &wayland_client::QueueHandle<WaylandClientStatePtr>,
    icon: &DragIcon,
) -> Option<wl_buffer::WlBuffer> {
    let stride = icon.width as usize * 4;
    let size = stride * icon.height as usize;
    if size == 0 || icon.pixels.len() < size {
        return None;
    }
    // Safety: memfd_create returns a new descriptor this function owns, or -1.
    let fd = unsafe { libc::memfd_create(c"gpui-drag-icon".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let mut file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    file.write_all(&icon.pixels[..size]).ok()?;
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        icon.width as i32,
        icon.height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();
    Some(buffer)
}

/// Answers the drag over this client's windows: the actions the window under
/// the pointer takes, the one it prefers, and the mime type it would read
/// (None refuses the drop there).
pub fn set_drop_actions(actions: u32, preferred: u32, mime_type: Option<String>) {
    with_client(|ptr| {
        let client = ptr.get_client();
        let state = client.borrow();
        let drop = state.dnd.drop.as_ref()?;
        drop.offer.inner.accept(drop.serial, mime_type);
        if drop.offer.inner.version() >= wl_data_offer::REQ_SET_ACTIONS_SINCE {
            drop.offer.inner.set_actions(
                DndAction::from_bits_truncate(actions),
                DndAction::from_bits_truncate(preferred),
            );
        }
        let _ = state.clipboard.connection().flush();
        Some(())
    });
}

/// The content of the drag over this client's windows as `mime_type`. It
/// blocks until the source has written it, without dispatching events: the
/// source must be another client.
pub fn read_drop(mime_type: &str) -> Option<Vec<u8>> {
    with_client(|ptr| {
        let client = ptr.get_client();
        let state = client.borrow();
        let drop = state.dnd.drop.as_ref()?;
        drop.offer.read_bytes(state.clipboard.connection(), mime_type)
    })
}

/// Tells the source the drop is done, after its content was read.
pub fn finish_drop() {
    end(true);
}

/// Lets the drag go without finishing it: the drop was refused, or it left.
pub fn end_drop() {
    end(false);
}

fn end(finish: bool) {
    with_client(|ptr| {
        let client = ptr.get_client();
        let mut state = client.borrow_mut();
        let drop = state.dnd.drop.take()?;
        if finish && drop.dropped && drop.offer.inner.version() >= wl_data_offer::REQ_FINISH_SINCE {
            drop.offer.inner.finish();
        }
        drop.offer.inner.destroy();
        state.data_offers.retain(|o| o.inner.id() != drop.offer.inner.id());
        let _ = state.clipboard.connection().flush();
        Some(())
    });
}
