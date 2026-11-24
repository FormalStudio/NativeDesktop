use std::{
    fs::File,
    io::{ErrorKind, Write},
    os::fd::{AsRawFd, BorrowedFd, OwnedFd},
};

use calloop::{LoopHandle, PostAction};
use filedescriptor::Pipe;
use strum::IntoEnumIterator;
use wayland_client::{Connection, protocol::wl_data_offer::WlDataOffer};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::ExtDataControlOfferV1;
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1;

use crate::linux::platform::{PIPE_READ_TIMEOUT, read_fd_with_timeout};
use gpui::{ClipboardEntry, ClipboardItem, Image, ImageFormat, hash};

/// Text mime types that we'll offer to other programs.
pub(crate) const TEXT_MIME_TYPES: [&str; 3] =
    ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
pub(crate) const FILE_LIST_MIME_TYPE: &str = "text/uri-list";

/// Text mime types that we'll accept from other programs, in order of
/// preference. Plain `text/plain` names no charset; it is read as UTF-8, as
/// the sources that offer only it (wl-copy, Wine, older toolkits) write it.
pub(crate) const ALLOWED_TEXT_MIME_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

/// What the desktop's clipboard selection holds, as its offer's mime types tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    /// No selection: it was cleared, or its owner went away.
    Empty,
    /// This client's own copy.
    Own,
    /// Another client's copy, with text.
    Text,
    /// Another client's copy, without text.
    Other,
}

impl SelectionKind {
    const ALL: [Self; 4] = [Self::Empty, Self::Own, Self::Text, Self::Other];

    pub(crate) fn of(mime_types: Option<&[String]>, self_mime: &str) -> Self {
        let Some(mime_types) = mime_types else {
            return Self::Empty;
        };
        if mime_types.iter().any(|t| t == self_mime) {
            Self::Own
        } else if mime_types
            .iter()
            .any(|t| ALLOWED_TEXT_MIME_TYPES.contains(&t.as_str()))
        {
            Self::Text
        } else {
            Self::Other
        }
    }
}

/// The count of selection changes, times four, plus the current kind's index.
static SELECTION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many times the desktop's clipboard selection has changed since the
/// client connected, and what it holds now. A new copy changes it even when it
/// holds the same text. Only the offer is looked at: no content is read.
pub fn selection() -> (u64, SelectionKind) {
    let value = SELECTION.load(std::sync::atomic::Ordering::Acquire);
    (value >> 2, SelectionKind::ALL[(value & 3) as usize])
}

/// A change of the desktop's selection to one holding `kind`.
pub(crate) fn selection_changed(kind: SelectionKind) {
    let _ = SELECTION.fetch_update(
        std::sync::atomic::Ordering::AcqRel,
        std::sync::atomic::Ordering::Acquire,
        |value| Some((((value >> 2) + 1) << 2) | kind as u64),
    );
}

/// The selection count of the offer the last read took its content from.
static READ_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The count `selection` gave for the selection the last clipboard read took
/// its content from. A read blocks without dispatching events, so the count can
/// lag a newer selection; a reader matches this against the count it acted on
/// rather than against `selection`.
pub fn read_serial() -> u64 {
    READ_SERIAL.load(std::sync::atomic::Ordering::Acquire)
}

/// The kind of the selection `selection` last reported.
pub(crate) fn selection_kind() -> SelectionKind {
    selection().1
}

pub(crate) struct Clipboard {
    connection: Connection,
    loop_handle: LoopHandle<'static, ()>,
    self_mime: String,

    // Internal clipboard
    contents: Option<ClipboardItem>,
    primary_contents: Option<ClipboardItem>,

    // External clipboard
    cached_read: Option<ClipboardItem>,
    current_offer: Option<DataOffer<WlDataOffer>>,
    /// With data control bound, its offer of the selection (None when there is no
    /// selection). Reads take it over the data device's, which follows the focus.
    control_offer: Option<Option<DataOffer<ExtDataControlOfferV1>>>,
    /// The selection count the control offer was counted under.
    control_serial: u64,
    cached_primary_read: Option<ClipboardItem>,
    current_primary_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>,
}

/// What an offer of the selection holds: this client's own copy, or another's content.
enum Read {
    Own,
    Foreign(ClipboardItem),
}

pub(crate) trait ReceiveData {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>);
}

impl ReceiveData for WlDataOffer {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

impl ReceiveData for ZwpPrimarySelectionOfferV1 {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

impl ReceiveData for ExtDataControlOfferV1 {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

#[derive(Clone, Debug)]
/// Wrapper for `WlDataOffer` and `ZwpPrimarySelectionOfferV1`, used to help track mime types.
pub(crate) struct DataOffer<T: ReceiveData> {
    pub inner: T,
    mime_types: Vec<String>,
}

impl<T: ReceiveData> DataOffer<T> {
    pub fn new(offer: T) -> Self {
        Self {
            inner: offer,
            mime_types: Vec::new(),
        }
    }

    pub fn with_mime_types(offer: T, mime_types: Vec<String>) -> Self {
        Self {
            inner: offer,
            mime_types,
        }
    }

    pub(crate) fn mime_types(&self) -> &[String] {
        &self.mime_types
    }

    pub fn add_mime_type(&mut self, mime_type: String) {
        self.mime_types.push(mime_type)
    }

    fn has_mime_type(&self, mime_type: &str) -> bool {
        self.mime_types.iter().any(|t| t == mime_type)
    }

    pub(crate) fn read_bytes(&self, connection: &Connection, mime_type: &str) -> Option<Vec<u8>> {
        let pipe = Pipe::new().unwrap();
        self.inner.receive_data(mime_type.to_string(), unsafe {
            BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
        });
        let fd = pipe.read;
        drop(pipe.write);

        connection.flush().unwrap();

        match read_fd_with_timeout(fd, PIPE_READ_TIMEOUT) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                log::error!("error reading clipboard pipe: {err:?}");
                None
            }
        }
    }

    fn read(&self, self_mime: &str, connection: &Connection) -> Option<Read> {
        if self.has_mime_type(self_mime) {
            return Some(Read::Own);
        }
        self.read_text(connection)
            .or_else(|| self.read_image(connection))
            .map(Read::Foreign)
    }

    fn read_text(&self, connection: &Connection) -> Option<ClipboardItem> {
        let mime_type = ALLOWED_TEXT_MIME_TYPES
            .iter()
            .find(|&&allowed| self.has_mime_type(allowed))?;
        let bytes = self.read_bytes(connection, mime_type)?;
        let text_content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(e) => {
                log::error!("Failed to convert clipboard content to UTF-8: {}", e);
                return None;
            }
        };

        // Normalize the text to unix line endings, otherwise
        // copying from eg: firefox inserts a lot of blank
        // lines, and that is super annoying.
        let result = text_content.replace("\r\n", "\n");
        Some(ClipboardItem::new_string(result))
    }

    fn read_image(&self, connection: &Connection) -> Option<ClipboardItem> {
        for format in ImageFormat::iter() {
            let mime_type = format.mime_type();
            if !self.has_mime_type(mime_type) {
                continue;
            }

            if let Some(bytes) = self.read_bytes(connection, mime_type) {
                let id = hash(&bytes);
                return Some(ClipboardItem {
                    entries: vec![ClipboardEntry::Image(Image { format, bytes, id })],
                });
            }
        }
        None
    }
}

impl Clipboard {
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn new(connection: Connection, loop_handle: LoopHandle<'static, ()>) -> Self {
        Self {
            connection,
            loop_handle,
            self_mime: format!("pid/{}", std::process::id()),

            contents: None,
            primary_contents: None,

            cached_read: None,
            current_offer: None,
            control_offer: None,
            control_serial: 0,
            cached_primary_read: None,
            current_primary_offer: None,
        }
    }

    pub fn set(&mut self, item: ClipboardItem) {
        self.contents = Some(item);
    }

    pub fn set_primary(&mut self, item: ClipboardItem) {
        self.primary_contents = Some(item);
    }

    pub fn offer_kind(&self, data_offer: Option<&DataOffer<WlDataOffer>>) -> SelectionKind {
        SelectionKind::of(data_offer.map(|offer| offer.mime_types.as_slice()), &self.self_mime)
    }

    pub fn set_offer(&mut self, data_offer: Option<DataOffer<WlDataOffer>>) {
        if self.control_offer.is_none() {
            self.cached_read = None;
        }
        self.current_offer = data_offer;
    }

    /// The data control device's offer of a new selection, `Some(None)` for none, or
    /// None once that device is gone. Set after the selection is counted.
    pub fn set_control_offer(&mut self, offer: Option<Option<DataOffer<ExtDataControlOfferV1>>>) {
        self.cached_read = None;
        self.control_serial = selection().0;
        if let Some(Some(previous)) = std::mem::replace(&mut self.control_offer, offer) {
            previous.inner.destroy();
        }
    }

    pub fn set_primary_offer(&mut self, data_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>) {
        self.cached_primary_read = None;
        self.current_primary_offer = data_offer;
    }

    pub fn self_mime(&self) -> String {
        self.self_mime.clone()
    }

    pub fn send(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self.contents.as_ref().and_then(|contents| contents.text()) {
            self.send_bytes(fd, text.as_bytes().to_owned());
        }
    }

    pub fn send_primary(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self
            .primary_contents
            .as_ref()
            .and_then(|contents| contents.text())
        {
            self.send_bytes(fd, text.as_bytes().to_owned());
        }
    }

    pub fn read(&mut self) -> Option<ClipboardItem> {
        // Without data control the offer read is the selection counted last.
        let serial = if self.control_offer.is_some() { self.control_serial } else { selection().0 };
        READ_SERIAL.store(serial, std::sync::atomic::Ordering::Release);
        if let Some(cached) = self.cached_read.clone() {
            return Some(cached);
        }
        let read = match &self.control_offer {
            Some(offer) => offer.as_ref()?.read(&self.self_mime, &self.connection),
            None => self.current_offer.as_ref()?.read(&self.self_mime, &self.connection),
        };
        match read? {
            Read::Own => self.contents.clone(),
            Read::Foreign(item) => {
                self.cached_read = Some(item.clone());
                Some(item)
            }
        }
    }

    pub fn read_primary(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_primary_offer.as_ref()?;
        if let Some(cached) = self.cached_primary_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.primary_contents.clone();
        }

        let item = offer
            .read_text(&self.connection)
            .or_else(|| offer.read_image(&self.connection))?;

        self.cached_primary_read = Some(item.clone());
        Some(item)
    }

    pub fn send_bytes(&self, fd: OwnedFd, bytes: Vec<u8>) {
        let mut written = 0;
        self.loop_handle
            .insert_source(
                calloop::generic::Generic::new(
                    File::from(fd),
                    calloop::Interest::WRITE,
                    calloop::Mode::Level,
                ),
                move |_, file, _| {
                    let file = unsafe { file.get_mut() };
                    loop {
                        match file.write(&bytes[written..]) {
                            Ok(n) if written + n == bytes.len() => {
                                written += n;
                                break Ok(PostAction::Remove);
                            }
                            Ok(n) => written += n,
                            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                                break Ok(PostAction::Continue);
                            }
                            Err(_) => break Ok(PostAction::Remove),
                        }
                    }
                },
            )
            .unwrap();
    }
}
