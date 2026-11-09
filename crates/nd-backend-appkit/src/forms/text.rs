//! `text_field`, `secure_field` and `search_field` as `NSTextField`, `NSSecureTextField` and
//! `NSSearchField`; `text_area` as an `NSTextView` in an `NSScrollView` wearing a text field's
//! rounded bezel; `combo_box` as `NSComboBox`. One delegate per control reports edits, Return
//! and combo picks.
//!
//! Field icons on plain and secure fields are borderless SF Symbol buttons inside the field's
//! bezel, and the field's cell draws and edits its text in the rect between them. A search
//! field puts them on its own search and cancel buttons instead.
//!
//! Text is native-owned while the user edits: a value the app feeds back equals what the field
//! holds and sets nothing, and a different value written during IME composition waits until
//! the composition or the editing session ends (ARCHITECTURE.md 3.13).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use nd_components::forms::{
    ComboBoxProps, FieldIcon, FieldIconClicked, FieldStyle, FilesDropped, IconEdge, ImagesPasted, PastedImage,
    SelectChanged, TextAreaProps, TextCancelled, TextChanged, TextEditingEnded, TextFieldProps, TextSubmitted,
};
use nd_core::Size;
use nd_ui::backend::Props;
use nd_ui::events::Synced;
use nd_ui::prelude::SharedString;
use nd_ui::semantics::{ActionError, Probe, SemanticAction};
use nd_ui::{EditKey, Styler};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSActionCell, NSAppearanceCustomization, NSApplication, NSAttributedStringNSExtendedStringDrawing,
    NSAttributedStringNSStringDrawing, NSBitmapImageFileType, NSBitmapImageRep, NSBorderType, NSButton, NSCell,
    NSCellImagePosition, NSColor, NSComboBox, NSComboBoxDelegate, NSControl, NSControlSize,
    NSControlTextEditingDelegate, NSEventModifierFlags, NSFocusRingType, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSPasteboard, NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeTIFF, NSResponder, NSScrollView, NSScroller, NSSearchField, NSSearchFieldCell,
    NSSearchFieldDelegate, NSSecureTextField, NSSecureTextFieldCell, NSStandardKeyBindingResponding, NSText,
    NSTextDelegate, NSTextField, NSTextFieldBezelStyle, NSTextFieldCell, NSTextFieldDelegate, NSTextStorage,
    NSTextStorageDelegate, NSTextStorageEditActions, NSTextView, NSTextViewDelegate, NSView,
};
use objc2_app_kit::{
    NSControlCharacterAction, NSGlyphProperty, NSLayoutManager, NSLayoutManagerDelegate, NSTextContainer,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSDictionary, NSInteger, NSNotification, NSObject, NSObjectProtocol,
    NSPoint, NSRange, NSRect, NSSize, NSString, NSURL,
};

use super::preview;
use super::styled::{Styles, composing};
use super::{Ctx, FormControl, NdControlTarget, apply_common, props, set_accessibility_label};
use crate::events::defer;

#[derive(Default)]
pub struct CellInsets {
    leading: Cell<f64>,
    trailing: Cell<f64>,
}

impl CellInsets {
    fn inset(&self, mut r: NSRect) -> NSRect {
        let (l, t) = (self.leading.get(), self.trailing.get());
        r.origin.x += l;
        r.size.width = (r.size.width - l - t).max(0.);
        r
    }
}

// The field editor is placed in `drawingRectForBounds:` too, so this one override keeps typed
// text clear of the icons as well as drawn text.
macro_rules! inset_cell {
    ($name:ident, $($super:ident),+) => {
        define_class!(
            // SAFETY: the cell subclass only overrides `drawingRectForBounds:`, calling super, and
            // its ivars are plain `Cell<f64>`s, so a bitwise `copyWithZone:` stays sound.
            #[unsafe(super($($super),+))]
            #[thread_kind = MainThreadOnly]
            #[ivars = CellInsets]
            struct $name;

            impl $name {
                #[unsafe(method(drawingRectForBounds:))]
                fn drawing_rect(&self, bounds: NSRect) -> NSRect {
                    let r: NSRect = unsafe { msg_send![super(self), drawingRectForBounds: bounds] };
                    self.ivars().inset(r)
                }
            }
        );

        impl $name {
            fn new(mtm: MainThreadMarker) -> Retained<Self> {
                let this = Self::alloc(mtm).set_ivars(CellInsets::default());
                unsafe { msg_send![super(this), initTextCell: &*NSString::new()] }
            }
        }
    };
}

inset_cell!(NdFieldCell, NSTextFieldCell, NSActionCell, NSCell, NSObject);
inset_cell!(NdSecureFieldCell, NSSecureTextFieldCell, NSTextFieldCell, NSActionCell, NSCell, NSObject);

#[derive(Default)]
pub struct FieldIcons {
    buttons: RefCell<[Option<Retained<NSButton>>; 2]>,
}

/// Keeps the icon buttons on their edges and vertically centred: they are placed again
/// whenever the field's size changes or it lays out, so a host that resizes the field after
/// mount (a SwiftUI form row) cannot leave them stale.
fn place_icons(field: &NSTextField, icons: &FieldIcons) {
    let buttons = icons.buttons.borrow();
    let bounds = field.bounds();
    let leading = buttons[0].as_ref().map_or(0., |b| b.frame().size.width);
    // The cell's text rect starts after the leading icon; what is left is the bezel's own inset.
    let margin = field.cell().map_or(0., |c| c.drawingRectForBounds(bounds).origin.x - bounds.origin.x - leading);
    for (slot, button) in buttons.iter().enumerate() {
        let Some(button) = button else { continue };
        let size = button.frame().size;
        let x = if slot == 0 {
            bounds.origin.x + margin
        } else {
            bounds.origin.x + bounds.size.width - margin - size.width
        };
        let y = bounds.origin.y + ((bounds.size.height - size.height) / 2.).round();
        button.setFrameOrigin(NSPoint::new(x, y));
    }
}

macro_rules! icon_field {
    ($name:ident, $($super:ident),+) => {
        define_class!(
            // SAFETY: the subclass only overrides `resizeSubviewsWithOldSize:` and `layout`,
            // calling super first, and does not implement Drop.
            #[unsafe(super($($super),+))]
            #[thread_kind = MainThreadOnly]
            #[ivars = FieldIcons]
            struct $name;

            impl $name {
                #[unsafe(method(resizeSubviewsWithOldSize:))]
                fn resize_subviews(&self, old: NSSize) {
                    let _: () = unsafe { msg_send![super(self), resizeSubviewsWithOldSize: old] };
                    place_icons(self, self.ivars());
                }

                #[unsafe(method(layout))]
                fn layout(&self) {
                    let _: () = unsafe { msg_send![super(self), layout] };
                    place_icons(self, self.ivars());
                }
            }
        );

        impl $name {
            fn new(mtm: MainThreadMarker) -> Retained<Self> {
                let this = Self::alloc(mtm).set_ivars(FieldIcons::default());
                unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
            }
        }
    };
}

icon_field!(NdIconField, NSTextField, NSControl, NSView, NSResponder, NSObject);
icon_field!(NdSecureIconField, NSSecureTextField, NSTextField, NSControl, NSView, NSResponder, NSObject);

enum Source {
    Field(Retained<NSTextField>),
    Area(Retained<NSTextView>),
}

/// What the control and its delegate share: the text slot and how to read and write the
/// native text.
struct Core {
    cx: Ctx,
    source: Source,
    search: bool,
    /// Return submits and Shift- or Option-Return breaks the line (a composer's text area).
    submit_on_return: Cell<bool>,
    text: RefCell<Synced<SharedString>>,
    pending: RefCell<Option<SharedString>>,
    options: RefCell<Vec<SharedString>>,
    /// Escape abandoned the edit: the end of editing that follows is no commit.
    cancelled: Cell<bool>,
    /// A text area's styler and the paragraphs it styled.
    styles: RefCell<Option<Styles>>,
    /// The paragraphs live preview shows unconcealed: the selection's, while the area has
    /// focus. Empty otherwise.
    revealed: Cell<NSRange>,
}

impl Core {
    fn new(cx: Ctx, source: Source, search: bool) -> Rc<Self> {
        Rc::new(Self {
            cx,
            source,
            search,
            submit_on_return: Cell::new(false),
            text: RefCell::default(),
            pending: RefCell::default(),
            options: RefCell::default(),
            cancelled: Cell::new(false),
            styles: RefCell::default(),
            revealed: Cell::new(NSRange::new(0, 0)),
        })
    }

    fn area(&self) -> Option<&NSTextView> {
        match &self.source {
            Source::Area(t) => Some(t),
            Source::Field(_) => None,
        }
    }

    /// Sets the paragraphs edits restyled, once no input method is composing.
    fn paint_styles(&self) {
        let Some(view) = self.area() else { return };
        if self.styles.borrow().is_none() || composing(view) {
            return;
        }
        let Some(storage) = (unsafe { view.textStorage() }) else { return };
        if let Some(styles) = self.styles.borrow_mut().as_mut() {
            styles.apply(view, &storage);
        }
    }

    /// The styler's edit for `key` at the selection, made as one undo step. False when it
    /// makes none and the key is the text view's.
    fn styled_key(&self, key: EditKey) -> bool {
        let Some(view) = self.area() else { return false };
        if !view.isEditable() || composing(view) {
            return false;
        }
        let Some(storage) = (unsafe { view.textStorage() }) else { return false };
        let edit = self.styles.borrow().as_ref().and_then(|s| s.edit_for(&storage, view.selectedRange(), key));
        let Some((range, text, selection)) = edit else { return false };
        if range.length > 0 || !text.is_empty() {
            let text = NSString::from_str(&text);
            view.breakUndoCoalescing();
            if view.shouldChangeTextInRange_replacementString(range, Some(&text)) {
                storage.replaceCharactersInRange_withString(range, &text);
                view.didChangeText();
            }
            view.breakUndoCoalescing();
        }
        view.setSelectedRange(selection);
        view.scrollRangeToVisible(selection);
        true
    }

    /// Reveals the selection's paragraphs while the area has `focus` and conceals the ones it
    /// left: their glyphs are generated again.
    fn reveal(&self, focus: bool) {
        let Some(view) = self.area() else { return };
        if self.styles.borrow().is_none() {
            return;
        }
        let string = view.string();
        let total = string.length();
        let next = if focus { string.paragraphRangeForRange(view.selectedRange()) } else { NSRange::new(0, 0) };
        let previous = self.revealed.replace(next);
        if previous == next {
            return;
        }
        let Some(layout) = (unsafe { view.layoutManager() }) else { return };
        for range in [previous, next] {
            let start = range.location.min(total);
            let range = NSRange::new(start, (range.location + range.length).min(total) - start);
            if range.length == 0 {
                continue;
            }
            unsafe {
                layout.invalidateGlyphsForCharacterRange_changeInLength_actualCharacterRange(
                    range,
                    0,
                    std::ptr::null_mut(),
                );
                layout.invalidateLayoutForCharacterRange_actualCharacterRange(range, std::ptr::null_mut());
            }
            layout.invalidateDisplayForCharacterRange(range);
        }
    }

    fn focused(&self) -> bool {
        self.area().is_some_and(|view| {
            view.window().and_then(|w| w.firstResponder()).is_some_and(|r| {
                std::ptr::eq(&*r as *const NSResponder as *const u8, view as *const NSTextView as *const u8)
            })
        })
    }

    /// A click on the task box over `run`: the styler's toggle, as one undo step, with the
    /// selection left where it was.
    fn toggle_task(&self, run: NSRange) -> bool {
        let Some(view) = self.area() else { return false };
        if !view.isEditable() || composing(view) {
            return false;
        }
        let Some(storage) = (unsafe { view.textStorage() }) else { return false };
        let edit = self.styles.borrow().as_ref().and_then(|s| s.edit_for(&storage, run, EditKey::Toggle));
        let Some((range, text, _)) = edit else { return false };
        let selection = view.selectedRange();
        let text = NSString::from_str(&text);
        view.breakUndoCoalescing();
        if view.shouldChangeTextInRange_replacementString(range, Some(&text)) {
            storage.replaceCharactersInRange_withString(range, &text);
            view.didChangeText();
        }
        view.breakUndoCoalescing();
        view.setSelectedRange(selection);
        true
    }

    /// The text view the user types into: the field editor while a field is being edited.
    fn editor(&self) -> Option<Retained<NSText>> {
        match &self.source {
            Source::Field(f) => f.currentEditor(),
            Source::Area(t) => Some(Retained::into_super(t.clone())),
        }
    }

    fn current(&self) -> String {
        match &self.source {
            Source::Field(f) => f.currentEditor().map_or_else(|| f.stringValue(), |e| e.string()).to_string(),
            Source::Area(t) => t.string().to_string(),
        }
    }

    fn composing(&self) -> bool {
        self.editor().is_some_and(|e| unsafe { msg_send![&*e, hasMarkedText] })
    }

    /// Writes `text` into the control without echoing; equal text is left alone so the caret
    /// and selection stay where the user put them.
    fn set_native(&self, text: &str) {
        if self.current() == text {
            return;
        }
        let ns = NSString::from_str(text);
        self.cx.emit.programmatic(|| match &self.source {
            Source::Field(f) => match f.currentEditor() {
                Some(editor) => editor.setString(&ns),
                None => f.setStringValue(&ns),
            },
            Source::Area(t) => t.setString(&ns),
        });
        self.paint_styles();
    }

    /// The app's text, applied unless the user is composing.
    fn apply(&self, text: &SharedString) {
        if self.text.borrow().value() == Some(text) {
            self.pending.take();
            return;
        }
        // Cloned out so a delegate the setter fires can still borrow the slot.
        let mut synced = self.text.borrow().clone();
        synced.apply(text, |v| {
            if self.composing() {
                *self.pending.borrow_mut() = Some(v.clone());
                return self.current().into();
            }
            self.pending.take();
            self.set_native(v);
            v.clone()
        });
        *self.text.borrow_mut() = synced;
    }

    /// A user edit: report, emit, wake. With composition over, an app write that waited wins.
    fn edited(&self) {
        self.paint_styles();
        if !self.composing()
            && let Some(text) = self.pending.take()
        {
            self.set_native(&text);
            self.text.borrow_mut().report(text);
            return;
        }
        let text: SharedString = self.current().into();
        if self.text.borrow().value() == Some(&text) {
            return;
        }
        self.cancelled.set(false);
        self.text.borrow_mut().report(text.clone());
        self.cx.send(TextChanged { text });
    }

    /// Editing ended. A waiting app write lands once the field editor is gone, since the
    /// field copies the editor's text back as editing ends.
    fn ended(self: &Rc<Self>) {
        if self.pending.borrow().is_none() {
            return;
        }
        let core = self.clone();
        defer(move || {
            if let Some(text) = core.pending.take() {
                core.set_native(&text);
                core.text.borrow_mut().report(text);
            }
        });
    }

    fn submitted(&self) {
        self.cx.send(TextSubmitted { text: self.current().into() });
    }

    fn picked(&self, index: isize) {
        let Some(text) = usize::try_from(index).ok().and_then(|i| self.options.borrow().get(i).cloned()) else {
            return;
        };
        self.text.borrow_mut().report(text.clone());
        self.cx.emit.emit(SelectChanged { index: index as usize });
        self.cx.send(TextChanged { text });
    }

    /// Automation's `SetValue`: the native text changes, then the user-edit path runs.
    fn replace(&self, text: &str) {
        self.pending.take();
        let ns = NSString::from_str(text);
        match &self.source {
            Source::Field(f) => match f.currentEditor() {
                Some(editor) => editor.setString(&ns),
                None => f.setStringValue(&ns),
            },
            Source::Area(t) => t.setString(&ns),
        }
        self.edited();
    }

    /// Automation's `Type`: into the editor the way a keyboard would when the control is
    /// being edited, else appended to its text.
    fn type_text(&self, text: &str) {
        let editing = match &self.source {
            Source::Field(f) => f.currentEditor(),
            Source::Area(t) => t
                .window()
                .and_then(|w| w.firstResponder())
                .is_some_and(|r| std::ptr::eq(&*r as *const _ as *const NSTextView, &**t))
                .then(|| Retained::into_super(t.clone())),
        };
        match editing {
            Some(editor) => {
                let ns = NSString::from_str(text);
                let _: () = unsafe { msg_send![&*editor, insertText: &*ns] };
                self.edited();
            }
            None => self.replace(&format!("{}{text}", self.current())),
        }
    }
}

pub struct DelegateIvars {
    core: Rc<Core>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this type does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    struct NdTextDelegate;

    unsafe impl NSObjectProtocol for NdTextDelegate {}

    unsafe impl NSControlTextEditingDelegate for NdTextDelegate {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _n: &NSNotification) {
            self.ivars().core.edited();
        }

        #[unsafe(method(controlTextDidEndEditing:))]
        fn control_text_did_end_editing(&self, _n: &NSNotification) {
            let core = &self.ivars().core;
            if !core.cancelled.replace(false) {
                core.cx.send(TextEditingEnded { text: core.current().into() });
            }
            core.ended();
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, _text_view: &NSTextView, command: Sel) -> bool {
            let core = &self.ivars().core;
            // A search field submits through its own action.
            if command == sel!(insertNewline:) && !core.search {
                core.submitted();
            }
            // Escape: AppKit goes on up the responder chain (a sheet's cancel button).
            if command == sel!(cancelOperation:) {
                core.cancelled.set(true);
                core.cx.send(TextCancelled);
            }
            false
        }
    }

    unsafe impl NSTextFieldDelegate for NdTextDelegate {}
    unsafe impl NSSearchFieldDelegate for NdTextDelegate {}

    unsafe impl NSComboBoxDelegate for NdTextDelegate {
        #[unsafe(method(comboBoxSelectionDidChange:))]
        fn combo_selection_did_change(&self, n: &NSNotification) {
            let Some(combo) = n.object().and_then(|o| o.downcast::<NSComboBox>().ok()) else { return };
            self.ivars().core.picked(combo.indexOfSelectedItem());
        }
    }

    unsafe impl NSTextDelegate for NdTextDelegate {
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, _n: &NSNotification) {
            self.ivars().core.edited();
        }

        #[unsafe(method(textDidEndEditing:))]
        fn text_did_end_editing(&self, _n: &NSNotification) {
            self.ivars().core.ended();
        }
    }

    unsafe impl NSTextStorageDelegate for NdTextDelegate {
        // Keeps the paragraph cache in step with every character edit; the attributes wait
        // for the edit to end (`Core::paint_styles`).
        #[unsafe(method(textStorage:didProcessEditing:range:changeInLength:))]
        fn did_process_editing(
            &self,
            storage: &NSTextStorage,
            mask: NSTextStorageEditActions,
            range: NSRange,
            delta: NSInteger,
        ) {
            self.storage_edited(storage, mask, range, delta);
        }
    }

    unsafe impl NSLayoutManagerDelegate for NdTextDelegate {
        #[unsafe(method(layoutManager:shouldGenerateGlyphs:properties:characterIndexes:font:forGlyphRange:))]
        unsafe fn should_generate_glyphs(
            &self,
            layout: &NSLayoutManager,
            glyphs: std::ptr::NonNull<objc2_core_graphics::CGGlyph>,
            properties: std::ptr::NonNull<NSGlyphProperty>,
            characters: std::ptr::NonNull<usize>,
            font: &NSFont,
            range: NSRange,
        ) -> usize {
            let revealed = self.ivars().core.revealed.get();
            // SAFETY: the pointers come from the layout manager for `range`.
            unsafe { preview::generate(layout, glyphs, properties, characters, font, range, revealed) }
        }

        #[unsafe(method(layoutManager:shouldUseAction:forControlCharacterAtIndex:))]
        fn should_use_action(
            &self,
            layout: &NSLayoutManager,
            action: NSControlCharacterAction,
            index: usize,
        ) -> NSControlCharacterAction {
            preview::action(layout, action, index, self.ivars().core.revealed.get())
        }

        #[unsafe(method(layoutManager:boundingBoxForControlGlyphAtIndex:forTextContainer:proposedLineFragment:glyphPosition:characterIndex:))]
        fn control_glyph_box(
            &self,
            layout: &NSLayoutManager,
            _glyph: usize,
            _container: &NSTextContainer,
            proposed: NSRect,
            position: NSPoint,
            index: usize,
        ) -> NSRect {
            preview::mark_box(layout, index, proposed, position)
        }
    }

    unsafe impl NSTextViewDelegate for NdTextDelegate {
        #[unsafe(method(textViewDidChangeSelection:))]
        fn text_view_did_change_selection(&self, _n: &NSNotification) {
            let core = &self.ivars().core;
            core.reveal(core.focused());
        }

        #[unsafe(method(textView:doCommandBySelector:))]
        fn text_view_do_command(&self, text_view: &NSTextView, command: Sel) -> bool {
            let core = &self.ivars().core;
            let key = if command == sel!(insertNewline:) && !core.submit_on_return.get() {
                Some(EditKey::Return)
            } else if command == sel!(insertTab:) {
                Some(EditKey::Tab)
            } else if command == sel!(insertBacktab:) {
                Some(EditKey::Backtab)
            } else {
                None
            };
            if let Some(key) = key {
                core.styled_key(key)
            } else if command != sel!(insertNewline:) || !core.submit_on_return.get() {
                false
            } else {
                let mtm = MainThreadMarker::from(self);
                let flags = NSApplication::sharedApplication(mtm).currentEvent().map(|e| e.modifierFlags());
                let line_break = flags.is_some_and(|f| {
                    f.contains(NSEventModifierFlags::Shift) || f.contains(NSEventModifierFlags::Option)
                });
                if line_break {
                    unsafe { text_view.insertNewlineIgnoringFieldEditor(None) };
                } else {
                    core.submitted();
                }
                true
            }
        }
    }
);

impl NdTextDelegate {
    fn storage_edited(&self, storage: &NSTextStorage, mask: NSTextStorageEditActions, range: NSRange, delta: isize) {
        if !mask.contains(NSTextStorageEditActions::EditedCharacters) {
            return;
        }
        let core = &self.ivars().core;
        if let Some(styles) = core.styles.borrow_mut().as_mut() {
            styles.edited(storage, range, delta);
        }
        // The revealed paragraphs move with an edit before them and grow with one inside.
        let revealed = core.revealed.get();
        let old_end = (range.location + range.length) as isize - delta;
        let shifted = if (old_end as usize) <= revealed.location && range.location < revealed.location {
            NSRange::new((revealed.location as isize + delta).max(0) as usize, revealed.length)
        } else if range.location < revealed.location + revealed.length.max(1) {
            NSRange::new(revealed.location, (revealed.length as isize + delta).max(0) as usize)
        } else {
            revealed
        };
        core.revealed.set(shifted);
    }

    fn new(mtm: MainThreadMarker, core: Rc<Core>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { core });
        unsafe { msg_send![super(this), init] }
    }
}

fn attributes(font: &NSFont, color: Option<&NSColor>) -> Retained<NSDictionary<NSString, AnyObject>> {
    let mut keys: Vec<&NSString> = vec![unsafe { NSFontAttributeName }];
    let mut values: Vec<&AnyObject> = vec![font];
    if let Some(color) = color {
        keys.push(unsafe { NSForegroundColorAttributeName });
        values.push(color);
    }
    NSDictionary::from_slices(&keys, &values)
}

fn text_width(text: &str, font: Option<&NSFont>) -> f64 {
    let Some(font) = font else { return 0. };
    let s = unsafe { NSAttributedString::new_with_attributes(&NSString::from_str(text), &attributes(font, None)) };
    s.size().width.ceil()
}

/// A borderless SF Symbol button sitting inside a field's bezel.
struct IconButton {
    button: Retained<NSButton>,
    _target: Retained<NdControlTarget>,
}

impl IconButton {
    fn new(cx: &Ctx, icon: &FieldIcon, edge: IconEdge) -> Self {
        let send = cx.clone();
        let target = NdControlTarget::new(cx.mtm, move |_| send.send(FieldIconClicked { edge }));
        let button = NSButton::new(cx.mtm);
        button.setBordered(false);
        button.setImagePosition(NSCellImagePosition::ImageOnly);
        button.setRefusesFirstResponder(true);
        target.wire(&button);
        let this = Self { button, _target: target };
        this.set(icon);
        this
    }

    fn set(&self, icon: &FieldIcon) {
        self.button.setImage(crate::icons::symbol(icon.icon.lucide()).as_deref());
        self.button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        let label = NSString::from_str(&icon.label);
        self.button.setAccessibilityLabel(Some(&label));
        self.button.setToolTip(Some(&label));
        self.button.sizeToFit();
    }

    fn width(&self) -> f64 {
        self.button.frame().size.width
    }
}

enum IconField {
    Plain(Retained<NdIconField>),
    Secure(Retained<NdSecureIconField>),
}

impl IconField {
    fn ivars(&self) -> &FieldIcons {
        match self {
            Self::Plain(f) => f.ivars(),
            Self::Secure(f) => f.ivars(),
        }
    }
}

enum Insets {
    Plain(Retained<NdFieldCell>),
    Secure(Retained<NdSecureFieldCell>),
}

impl Insets {
    fn ivars(&self) -> &CellInsets {
        match self {
            Self::Plain(c) => c.ivars(),
            Self::Secure(c) => c.ivars(),
        }
    }
}

pub(crate) struct TextFieldControl {
    field: Retained<NSTextField>,
    core: Rc<Core>,
    delegate: Retained<NdTextDelegate>,
    icons: [Option<IconButton>; 2],
    icon_field: Option<IconField>,
    insets: Option<Insets>,
    search_target: Option<Retained<NdControlTarget>>,
    icon_targets: Vec<Retained<NdControlTarget>>,
    props: TextFieldProps,
}

fn has_icons(p: &TextFieldProps) -> bool {
    p.leading.is_some() || p.trailing.is_some()
}

impl TextFieldControl {
    pub fn new(cx: Ctx, props: &TextFieldProps) -> Self {
        let mtm = cx.mtm;
        let icons = props.field != FieldStyle::Search && has_icons(props);
        let (field, icon_field): (Retained<NSTextField>, _) = match props.field {
            FieldStyle::Plain if icons => {
                let f = NdIconField::new(mtm);
                (Retained::into_super(f.clone()), Some(IconField::Plain(f)))
            }
            FieldStyle::Secure if icons => {
                let f = NdSecureIconField::new(mtm);
                (Retained::into_super(Retained::into_super(f.clone())), Some(IconField::Secure(f)))
            }
            FieldStyle::Plain => (NSTextField::new(mtm), None),
            FieldStyle::Secure => (Retained::into_super(NSSecureTextField::new(mtm)), None),
            FieldStyle::Search => (Retained::into_super(NSSearchField::new(mtm)), None),
        };
        let core = Core::new(cx.clone(), Source::Field(field.clone()), props.field == FieldStyle::Search);
        let delegate = NdTextDelegate::new(mtm, core.clone());
        let mut this = Self {
            field,
            core,
            delegate,
            icons: [None, None],
            icon_field,
            insets: None,
            search_target: None,
            icon_targets: Vec::new(),
            props: props.clone(),
        };
        if icons {
            this.adopt_inset_cell(mtm, props.field == FieldStyle::Secure);
        }
        let field = &this.field;
        if props.field != FieldStyle::Search {
            field.setBezeled(true);
            field.setBezelStyle(NSTextFieldBezelStyle::RoundedBezel);
        }
        field.setUsesSingleLineMode(true);
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*this.delegate))) };
        if let Some(search) = field.downcast_ref::<NSSearchField>() {
            search.setSendsWholeSearchString(true);
            let core = this.core.clone();
            let target = NdControlTarget::new(mtm, move |_| {
                core.edited();
                core.submitted();
            });
            target.wire(search);
            this.search_target = Some(target);
        }
        this.apply(None, props);
        this
    }

    /// Swaps in a cell that keeps its text clear of the icons, keeping the stock cell's font.
    fn adopt_inset_cell(&mut self, mtm: MainThreadMarker, secure: bool) {
        let font = self.field.font();
        let insets =
            if secure { Insets::Secure(NdSecureFieldCell::new(mtm)) } else { Insets::Plain(NdFieldCell::new(mtm)) };
        let cell: &NSCell = match &insets {
            Insets::Plain(c) => c,
            Insets::Secure(c) => c,
        };
        self.field.setCell(Some(cell));
        self.field.setFont(font.as_deref());
        self.field.setEditable(true);
        self.field.setSelectable(true);
        cell.setScrollable(true);
        cell.setWraps(false);
        self.insets = Some(insets);
    }

    fn apply(&mut self, old: Option<&TextFieldProps>, new: &TextFieldProps) {
        let field = &self.field;
        if old.is_none_or(|o| o.placeholder != new.placeholder) {
            field.setPlaceholderString(new.placeholder.as_deref().map(NSString::from_str).as_deref());
        }
        if old.is_none_or(|o| o.large != new.large) {
            let large = if crate::glass::has_liquid_glass() { NSControlSize::ExtraLarge } else { NSControlSize::Large };
            field.setControlSize(if new.large { large } else { NSControlSize::Regular });
        }
        if old.is_none_or(|o| o.editable != new.editable) {
            field.setEditable(new.editable);
            field.setSelectable(true);
        }
        apply_common(Some(field), field, old.map(|o| (o.enabled, &o.tooltip)), new.enabled, &new.tooltip);
        if old.is_none_or(|o| o.label != new.label) {
            set_accessibility_label(field, new.label.as_deref());
        }
        if old.is_none_or(|o| o.leading != new.leading || o.trailing != new.trailing) {
            self.apply_icons(new);
        }
        self.core.apply(&new.text);
    }

    fn apply_icons(&mut self, p: &TextFieldProps) {
        if let Some(search) = self.field.downcast_ref::<NSSearchField>() {
            self.icon_targets.clear();
            let Some(cell) = search.cell().and_then(|c| c.downcast::<NSSearchFieldCell>().ok()) else { return };
            let buttons = [
                (cell.searchButtonCell(), &p.leading, IconEdge::Leading),
                (cell.cancelButtonCell(), &p.trailing, IconEdge::Trailing),
            ];
            for (button, icon, edge) in buttons {
                let (Some(button), Some(icon)) = (button, icon) else { continue };
                let send = self.core.cx.clone();
                let target = NdControlTarget::new(self.core.cx.mtm, move |_| send.send(FieldIconClicked { edge }));
                button.setImage(crate::icons::symbol(icon.icon.lucide()).as_deref());
                unsafe {
                    button.setTarget(Some(&target));
                    button.setAction(Some(sel!(changed:)));
                }
                self.icon_targets.push(target);
            }
            return;
        }
        let Some(insets) = &self.insets else { return };
        let cx = self.core.cx.clone();
        for (slot, icon, edge) in [(0, &p.leading, IconEdge::Leading), (1, &p.trailing, IconEdge::Trailing)] {
            match (icon, &self.icons[slot]) {
                (Some(icon), Some(b)) => b.set(icon),
                (Some(icon), None) => {
                    let b = IconButton::new(&cx, icon, edge);
                    self.field.addSubview(&b.button);
                    self.icons[slot] = Some(b);
                }
                (None, Some(b)) => {
                    b.button.removeFromSuperview();
                    self.icons[slot] = None;
                }
                (None, None) => {}
            }
        }
        let widths = self.icons.each_ref().map(|b| b.as_ref().map_or(0., IconButton::width));
        insets.ivars().leading.set(widths[0]);
        insets.ivars().trailing.set(widths[1]);
        self.place_icons();
    }

    fn place_icons(&self) {
        let Some(icon_field) = &self.icon_field else { return };
        *icon_field.ivars().buttons.borrow_mut() = self.icons.each_ref().map(|b| b.as_ref().map(|b| b.button.clone()));
        place_icons(&self.field, icon_field.ivars());
    }
}

impl Drop for TextFieldControl {
    fn drop(&mut self) {
        unsafe { self.field.setDelegate(None) };
    }
}

impl FormControl for TextFieldControl {
    fn view(&self) -> &NSView {
        &self.field
    }

    fn focused(&self) {
        let (Some(r), Some(editor)) = (self.props.select_on_focus.clone(), self.field.currentEditor()) else { return };
        let text = self.core.current();
        let utf16 = |n: usize| text.chars().take(n).map(char::len_utf16).sum::<usize>();
        let (start, end) = (utf16(r.start), utf16(r.end.max(r.start)));
        editor.setSelectedRange(objc2_foundation::NSRange::new(start, end - start));
    }

    fn update(&mut self, old: &Props, new: &Props) -> bool {
        let (old, new): (&TextFieldProps, &TextFieldProps) = (props(old), props(new));
        if old.field != new.field || (new.field != FieldStyle::Search && has_icons(old) != has_icons(new)) {
            return false;
        }
        self.apply(Some(old), new);
        self.props = new.clone();
        true
    }

    fn measure(&self, _width: Option<f32>) -> Size {
        field_size(&self.field, self.props.text.is_empty(), self.props.placeholder.as_deref(), self.icon_widths())
    }

    fn probe(&self, probe: &mut Probe) {
        probe.enabled = Some(self.field.isEnabled());
        if self.props.field != FieldStyle::Secure {
            probe.value = Some(self.core.current());
        }
        probe.placeholder = self.field.placeholderString().map(|p| p.to_string());
    }

    fn perform(&self, action: &SemanticAction) -> Option<Result<(), ActionError>> {
        perform_text(&self.core, self.props.enabled, self.props.editable, action)
    }
}

impl TextFieldControl {
    fn icon_widths(&self) -> f64 {
        self.insets.as_ref().map_or(0., |i| i.ivars().leading.get() + i.ivars().trailing.get())
    }
}

fn field_size(field: &NSTextField, empty: bool, placeholder: Option<&str>, extra: f64) -> Size {
    let cell = field.cell().map_or(NSSize::new(0., 0.), |c| c.cellSize());
    let placeholder = placeholder.map_or(0., |p| text_width(p, field.font().as_deref()));
    let width = if empty { cell.width + placeholder } else { cell.width.max(placeholder) };
    Size { width: (width + extra).ceil() as f32, height: cell.height.ceil() as f32 }
}

fn perform_text(
    core: &Core,
    enabled: bool,
    editable: bool,
    action: &SemanticAction,
) -> Option<Result<(), ActionError>> {
    if !matches!(action, SemanticAction::SetValue(_) | SemanticAction::Type(_)) {
        return None;
    }
    if !enabled {
        return Some(Err(ActionError::NotActionable("the control is disabled".into())));
    }
    if !editable {
        return Some(Err(ActionError::NotActionable("the field is read-only".into())));
    }
    match action {
        SemanticAction::SetValue(v) => core.replace(v),
        SemanticAction::Type(t) => core.type_text(t),
        _ => unreachable!(),
    }
    Some(Ok(()))
}

pub(crate) struct ComboBoxControl {
    combo: Retained<NSComboBox>,
    core: Rc<Core>,
    _delegate: Retained<NdTextDelegate>,
    props: ComboBoxProps,
}

impl ComboBoxControl {
    pub fn new(cx: Ctx, props: &ComboBoxProps) -> Self {
        let mtm = cx.mtm;
        let combo = NSComboBox::new(mtm);
        let field: Retained<NSTextField> = Retained::into_super(combo.clone());
        let core = Core::new(cx, Source::Field(field), false);
        let delegate = NdTextDelegate::new(mtm, core.clone());
        unsafe { combo.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        combo.setUsesSingleLineMode(true);
        let this = Self { combo, core, _delegate: delegate, props: props.clone() };
        this.apply(None, props);
        this
    }

    fn apply(&self, old: Option<&ComboBoxProps>, new: &ComboBoxProps) {
        let combo = &self.combo;
        if old.is_none_or(|o| o.options != new.options) {
            combo.removeAllItems();
            let items: Vec<Retained<NSString>> = new.options.iter().map(|o| NSString::from_str(o)).collect();
            let items: Retained<NSArray> = unsafe { Retained::cast_unchecked(NSArray::from_retained_slice(&items)) };
            unsafe { combo.addItemsWithObjectValues(&items) };
            *self.core.options.borrow_mut() = new.options.clone();
        }
        if old.is_none_or(|o| o.placeholder != new.placeholder) {
            combo.setPlaceholderString(new.placeholder.as_deref().map(NSString::from_str).as_deref());
        }
        combo.setCompletes(new.completes);
        if old.is_none_or(|o| o.editable != new.editable) {
            combo.setEditable(new.editable);
            combo.setSelectable(true);
        }
        apply_common(Some(combo), combo, old.map(|o| (o.enabled, &o.tooltip)), new.enabled, &new.tooltip);
        if old.is_none_or(|o| o.label != new.label) {
            set_accessibility_label(combo, new.label.as_deref());
        }
        self.core.apply(&new.text);
    }
}

impl Drop for ComboBoxControl {
    fn drop(&mut self) {
        unsafe { self.combo.setDelegate(None) };
    }
}

impl FormControl for ComboBoxControl {
    fn view(&self) -> &NSView {
        &self.combo
    }

    fn update(&mut self, old: &Props, new: &Props) -> bool {
        let (old, new): (&ComboBoxProps, &ComboBoxProps) = (props(old), props(new));
        self.apply(Some(old), new);
        self.props = new.clone();
        true
    }

    fn measure(&self, _width: Option<f32>) -> Size {
        let widest = self.props.options.iter().map(|o| text_width(o, self.combo.font().as_deref())).fold(0., f64::max);
        field_size(&self.combo, self.props.text.is_empty(), self.props.placeholder.as_deref(), 0.).max_width(widest)
    }

    fn probe(&self, probe: &mut Probe) {
        probe.enabled = Some(self.combo.isEnabled());
        probe.value = Some(self.core.current());
        probe.placeholder = self.combo.placeholderString().map(|p| p.to_string());
    }

    fn perform(&self, action: &SemanticAction) -> Option<Result<(), ActionError>> {
        perform_text(&self.core, self.props.enabled, self.props.editable, action)
    }
}

trait MaxWidth {
    fn max_width(self, content: f64) -> Self;
}

impl MaxWidth for Size {
    /// Widens a combo box so its widest option fits beside the pop-up button.
    fn max_width(mut self, content: f64) -> Self {
        self.width = self.width.max((content as f32) + self.height);
        self
    }
}

pub struct AreaIvars {
    core: RefCell<Option<std::rc::Weak<Core>>>,
    media: Cell<bool>,
}

define_class!(
    // SAFETY: NSTextView has no subclassing requirements beyond calling super in the overrides
    // below, and this type does not implement Drop.
    #[unsafe(super(NSTextView, NSText, NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[ivars = AreaIvars]
    struct NdAreaView;

    impl NdAreaView {
        #[unsafe(method_id(readablePasteboardTypes))]
        fn readable_types(&self) -> Retained<NSArray<NSPasteboardType>> {
            let own: Retained<NSArray<NSPasteboardType>> = unsafe { msg_send![super(self), readablePasteboardTypes] };
            self.with_media(own)
        }

        #[unsafe(method_id(acceptableDragTypes))]
        fn drag_types(&self) -> Retained<NSArray<NSPasteboardType>> {
            let own: Retained<NSArray<NSPasteboardType>> = unsafe { msg_send![super(self), acceptableDragTypes] };
            self.with_media(own)
        }

        #[unsafe(method(readSelectionFromPasteboard:type:))]
        fn read_typed(&self, pboard: &NSPasteboard, kind: &NSPasteboardType) -> bool {
            self.take_media(pboard) || unsafe { msg_send![super(self), readSelectionFromPasteboard: pboard, type: kind] }
        }

        #[unsafe(method(readSelectionFromPasteboard:))]
        fn read(&self, pboard: &NSPasteboard) -> bool {
            self.take_media(pboard) || unsafe { msg_send![super(self), readSelectionFromPasteboard: pboard] }
        }

        // Command-B and Command-I go to the styler before the menus, while the area has focus.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &objc2_app_kit::NSEvent) -> bool {
            self.styled_shortcut(event)
                || unsafe { msg_send![super(self), performKeyEquivalent: event] }
        }

        // Live preview's code wells, quote bars, rules, bullets and task boxes sit behind the
        // text.
        #[unsafe(method(drawViewBackgroundInRect:))]
        fn draw_view_background(&self, rect: NSRect) {
            let _: () = unsafe { msg_send![super(self), drawViewBackgroundInRect: rect] };
            if let Some(core) = self.core().filter(|c| c.styles.borrow().is_some()) {
                preview::draw(self, rect, core.revealed.get());
            }
        }

        // A click on a concealed task box ticks it instead of placing the caret.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &objc2_app_kit::NSEvent) {
            if self.click_task(event) {
                return;
            }
            let _: () = unsafe { msg_send![super(self), mouseDown: event] };
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            let became: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if became && let Some(core) = self.core() {
                core.reveal(true);
            }
            became
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            let resigned: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if resigned && let Some(core) = self.core() {
                core.reveal(false);
            }
            resigned
        }
    }
);

#[derive(Default)]
pub struct AreaScrollIvars {
    bezel: RefCell<Option<Retained<NSTextField>>>,
    /// The capsule's corner radius, drawn on the layer at every tile: the layer may not exist
    /// yet when the shape is set (a view not in a window).
    capsule: Cell<Option<f64>>,
    /// Points the clip view leaves free at the trailing edge (`trailing_room`).
    trailing: Cell<f64>,
    /// The text view's inset from its shape, and the widest its text runs (`max_measure`):
    /// wider, the horizontal inset grows to centre a column that wide.
    inset: Cell<NSSize>,
    measure: Cell<Option<f64>>,
}

define_class!(
    // SAFETY: the subclass overrides `tile` (calling super first) and the focus ring mask
    // methods (falling back to super), and does not implement Drop.
    #[unsafe(super(NSScrollView, NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[ivars = AreaScrollIvars]
    struct NdAreaScroll;

    impl NdAreaScroll {
        // AppKit asks the scroll view for the ring while its text view is first responder, so
        // the system focus ring goes round the rounded bezel, as it does round a text field.
        #[unsafe(method(focusRingMaskBounds))]
        fn focus_ring_mask_bounds(&self) -> NSRect {
            match self.bezel() {
                Some(bezel) => bezel.frame(),
                None => unsafe { msg_send![super(self), focusRingMaskBounds] },
            }
        }

        #[unsafe(method(drawFocusRingMask))]
        fn draw_focus_ring_mask(&self) {
            let Some((frame, cell)) = self.bezel().and_then(|b| Some((b.frame(), b.cell()?))) else {
                return unsafe { msg_send![super(self), drawFocusRingMask] };
            };
            // The cell draws its rounded mask only while it shows first responder status, which
            // the inert bezel never has on its own.
            cell.setShowsFirstResponder(true);
            cell.drawFocusRingMaskWithFrame_inView(frame, self);
            cell.setShowsFirstResponder(false);
        }

        #[unsafe(method(tile))]
        fn tile(&self) {
            let _: () = unsafe { msg_send![super(self), tile] };
            if let Some(bezel) = self.ivars().bezel.borrow().as_ref() {
                bezel.setFrame(self.bounds());
            }
            let trailing = self.ivars().trailing.get();
            if trailing > 0. {
                let clip = self.contentView();
                let mut frame = clip.frame();
                frame.size.width = (frame.size.width - trailing).max(0.);
                clip.setFrame(frame);
            }
            self.place_column();
            self.draw_capsule();
        }
    }
);

impl NdAreaScroll {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AreaScrollIvars::default());
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    /// The rounded bezel AppKit draws for a text field, behind the scrolled text: an empty,
    /// inert `NSTextField`, so the bezel, its fill and its disabled look are the system's own
    /// on every release. `None` takes it away.
    fn set_bezel(&self, bezel: Option<Retained<NSTextField>>) {
        if let Some(old) = self.ivars().bezel.replace(bezel.clone()) {
            old.removeFromSuperview();
        }
        let clip = self.contentView();
        clip.setDrawsBackground(bezel.is_none());
        self.setFocusRingType(if bezel.is_some() { NSFocusRingType::Exterior } else { NSFocusRingType::Default });
        if let Some(text) = self.documentView().and_then(|v| v.downcast::<NSTextView>().ok()) {
            text.setDrawsBackground(bezel.is_none());
        }
        if let Some(bezel) = bezel {
            self.addSubview_positioned_relativeTo(&bezel, objc2_app_kit::NSWindowOrderingMode::Below, Some(&clip));
            bezel.setFrame(self.bounds());
        }
    }

    fn bezel(&self) -> Option<Retained<NSTextField>> {
        self.ivars().bezel.borrow().clone()
    }

    /// Sets the text view's inset: the shape's, widened to centre the measure.
    fn place_column(&self) {
        let Some(text) = self.documentView().and_then(|v| v.downcast::<NSTextView>().ok()) else { return };
        let mut inset = self.ivars().inset.get();
        if let Some(measure) = self.ivars().measure.get() {
            let padding = unsafe { text.textContainer() }.map_or(0., |c| c.lineFragmentPadding());
            let width = self.contentView().frame().size.width;
            inset.width = inset.width.max(((width - measure) / 2. - padding).floor());
        }
        if text.textContainerInset() != inset {
            text.setTextContainerInset(inset);
        }
    }

    fn set_inset(&self, inset: NSSize) {
        self.ivars().inset.set(inset);
        self.place_column();
    }

    /// Lays the clip view out again (the trailing room changed).
    fn tile_now(&self) {
        let _: () = unsafe { msg_send![self, tile] };
    }

    /// Sets the capsule's radius (`None` for none) and draws it.
    fn set_capsule(&self, radius: Option<f64>) {
        self.ivars().capsule.set(radius);
        self.setWantsLayer(radius.is_some());
        self.draw_capsule();
    }

    /// The capsule: rounded corners and a hairline in the separator colour, over no fill, so the
    /// field sits on the bar like Messages'.
    fn draw_capsule(&self) {
        let Some(radius) = self.ivars().capsule.get() else { return };
        let Some(layer) = self.layer() else { return };
        layer.setCornerRadius(radius.min(self.bounds().size.height / 2.));
        layer.setMasksToBounds(true);
        layer.setBorderWidth(1. / self.window().map_or(2., |w| w.backingScaleFactor()));
        let resolve = block2::RcBlock::new(|| layer.setBorderColor(Some(&NSColor::separatorColor().CGColor())));
        self.effectiveAppearance().performAsCurrentDrawingAppearance(&resolve);
    }
}

/// An empty rounded-bezel text field that never takes focus or clicks.
fn inert_bezel(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let bezel = NSTextField::new(mtm);
    bezel.setBezeled(true);
    bezel.setBezelStyle(NSTextFieldBezelStyle::RoundedBezel);
    bezel.setEditable(false);
    bezel.setSelectable(false);
    bezel.setRefusesFirstResponder(true);
    bezel.setAccessibilityElement(false);
    bezel
}

fn media_types() -> Vec<&'static NSPasteboardType> {
    unsafe { vec![NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeTIFF] }
}

impl NdAreaView {
    fn core(&self) -> Option<Rc<Core>> {
        self.ivars().core.borrow().as_ref().and_then(std::rc::Weak::upgrade)
    }

    fn click_task(&self, event: &objc2_app_kit::NSEvent) -> bool {
        let plain = event.modifierFlags()
            & (NSEventModifierFlags::Command | NSEventModifierFlags::Shift | NSEventModifierFlags::Option)
            == NSEventModifierFlags::empty();
        if event.clickCount() != 1 || !plain {
            return false;
        }
        let Some(core) = self.core().filter(|c| c.styles.borrow().is_some()) else { return false };
        let at = self.convertPoint_fromView(event.locationInWindow(), None);
        let Some(run) = preview::task_at(self, at, core.revealed.get()) else { return false };
        core.toggle_task(run)
    }

    fn styled_shortcut(&self, event: &objc2_app_kit::NSEvent) -> bool {
        let focused = self
            .window()
            .and_then(|w| w.firstResponder())
            .is_some_and(|r| std::ptr::eq(&*r as *const NSResponder as *const u8, self as *const Self as *const u8));
        let keys = NSEventModifierFlags::Command
            | NSEventModifierFlags::Shift
            | NSEventModifierFlags::Option
            | NSEventModifierFlags::Control;
        if !focused || event.modifierFlags() & keys != NSEventModifierFlags::Command {
            return false;
        }
        let key = match event.charactersIgnoringModifiers().map(|c| c.to_string()).as_deref() {
            Some("b") => EditKey::Bold,
            Some("i") => EditKey::Italic,
            _ => return false,
        };
        let Some(core) = self.ivars().core.borrow().as_ref().and_then(std::rc::Weak::upgrade) else { return false };
        core.styled_key(key)
    }

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AreaIvars { core: RefCell::new(None), media: Cell::new(false) });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    /// The pasteboard types the area reads: its own, and files and images when it takes media.
    fn with_media(&self, own: Retained<NSArray<NSPasteboardType>>) -> Retained<NSArray<NSPasteboardType>> {
        if !self.ivars().media.get() {
            return own;
        }
        let mut types: Vec<Retained<NSPasteboardType>> = media_types().into_iter().map(|t| t.retain()).collect();
        types.extend(own.iter());
        NSArray::from_retained_slice(&types)
    }

    /// Files and images on `pboard` become the area's media events instead of text.
    fn take_media(&self, pboard: &NSPasteboard) -> bool {
        if !self.ivars().media.get() {
            return false;
        }
        let Some(core) = self.ivars().core.borrow().as_ref().and_then(std::rc::Weak::upgrade) else { return false };
        let mut paths = Vec::new();
        for item in pboard.pasteboardItems().iter().flat_map(|items| items.iter().collect::<Vec<_>>()) {
            let url = item.stringForType(unsafe { NSPasteboardTypeFileURL });
            if let Some(url) = url.and_then(|u| NSURL::URLWithString(&u)).and_then(|u| u.path()) {
                paths.push(std::path::PathBuf::from(url.to_string()));
            }
        }
        if !paths.is_empty() {
            core.cx.send(FilesDropped { paths });
            return true;
        }
        let png = pboard.dataForType(unsafe { NSPasteboardTypePNG }).map(|d| d.to_vec());
        let png = png.or_else(|| {
            let tiff = pboard.dataForType(unsafe { NSPasteboardTypeTIFF })?;
            let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &tiff)?;
            let data =
                unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
            Some(data.to_vec())
        });
        match png {
            Some(bytes) => {
                let image = PastedImage { bytes: std::sync::Arc::from(bytes), mime: "image/png".into() };
                core.cx.send(ImagesPasted { images: vec![image] });
                true
            }
            None => false,
        }
    }
}

/// The text container inset inside the rounded bezel: with the line fragment padding, the
/// text starts where a rounded field's does, and its first line sits as far below the top
/// edge as a field's single line.
const BEZEL_INSET: (f64, f64) = (2., 4.);

pub(crate) struct TextAreaControl {
    scroll: Retained<NdAreaScroll>,
    text_view: Retained<NdAreaView>,
    core: Rc<Core>,
    _delegate: Retained<NdTextDelegate>,
    enabled: Cell<bool>,
    props: TextAreaProps,
}

impl TextAreaControl {
    pub fn new(cx: Ctx, props: &TextAreaProps) -> Self {
        let mtm = cx.mtm;
        let scroll = NdAreaScroll::new(mtm);
        scroll.setHasVerticalScroller(true);
        // Left automatic, AppKit insets a text area by the toolbar's height when its window's
        // content runs under the bars, though the field sits nowhere near them.
        scroll.setAutomaticallyAdjustsContentInsets(false);
        scroll.setAutohidesScrollers(true);
        let text_view = NdAreaView::new(mtm);
        text_view.setVerticallyResizable(true);
        text_view.setHorizontallyResizable(false);
        text_view.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
        text_view.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        if let Some(container) = unsafe { text_view.textContainer() } {
            container.setWidthTracksTextView(true);
            container.setContainerSize(NSSize::new(0., f64::MAX));
        }
        scroll.setDocumentView(Some(&text_view));
        text_view.setRichText(false);
        text_view.setAllowsUndo(true);
        text_view.setFont(Some(&NSFont::systemFontOfSize(NSFont::systemFontSize())));
        let core = Core::new(cx, Source::Area(Retained::into_super(text_view.clone())), false);
        *text_view.ivars().core.borrow_mut() = Some(Rc::downgrade(&core));
        let delegate = NdTextDelegate::new(mtm, core.clone());
        text_view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // Measuring reads the layout manager, which moves the view to TextKit 1 for good; doing
        // it now leaves the storage the styling delegate sits on in place from here on.
        if let Some(layout) = unsafe { text_view.layoutManager() } {
            layout.setAllowsNonContiguousLayout(true);
            layout.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
        if let Some(storage) = unsafe { text_view.textStorage() } {
            storage.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
        let this =
            Self { scroll, text_view, core, _delegate: delegate, enabled: Cell::new(true), props: props.clone() };
        this.apply(None, props);
        this
    }

    fn apply(&self, old: Option<&TextAreaProps>, new: &TextAreaProps) {
        let tv = &self.text_view;
        let font_changed = old.is_none_or(|o| o.style.text != new.style.text);
        if font_changed {
            let size = NSFont::systemFontSize();
            let font = crate::look::ns_font(&new.style, size).unwrap_or_else(|| NSFont::systemFontOfSize(size));
            tv.setFont(Some(&font));
        }
        if font_changed || old.is_none_or(|o| o.placeholder != new.placeholder) {
            self.set_placeholder(new.placeholder.as_deref());
        }
        let restyle = font_changed || old.is_none_or(|o| o.enabled != new.enabled || o.styles != new.styles);
        if old.is_none_or(|o| o.editable != new.editable || o.enabled != new.enabled) {
            tv.setEditable(new.editable && new.enabled);
            tv.setSelectable(new.enabled);
            let color = if new.enabled { NSColor::textColor() } else { NSColor::disabledControlTextColor() };
            tv.setTextColor(Some(&color));
            self.enabled.set(new.enabled);
            if let Some(bezel) = self.scroll.bezel() {
                bezel.setEnabled(new.enabled);
            }
        }
        apply_common(None, tv, old.map(|o| (o.enabled, &o.tooltip)), new.enabled, &new.tooltip);
        if old.is_none_or(|o| o.label != new.label) {
            set_accessibility_label(tv, new.label.as_deref());
        }
        self.core.submit_on_return.set(new.submit_on_return);
        if old.is_none_or(|o| o.trailing_room != new.trailing_room) {
            self.scroll.ivars().trailing.set(f64::from(new.trailing_room));
            self.scroll.setNeedsLayout(true);
            self.scroll.tile_now();
        }
        tv.ivars().media.set(new.accepts_media);
        if old.is_none_or(|o| o.style.corner_radii != new.style.corner_radii || o.borderless != new.borderless) {
            self.apply_shape(new);
        }
        if old.is_none_or(|o| o.max_measure != new.max_measure) {
            self.scroll.ivars().measure.set(new.max_measure.map(f64::from));
            self.scroll.place_column();
        }
        if restyle {
            self.set_styler(new.styles.as_ref(), new.enabled);
        }
        if old.is_some_and(|o| o.emoji_picker != new.emoji_picker) {
            let tv = tv.clone();
            defer(move || {
                if let Some(window) = tv.window() {
                    window.makeFirstResponder(Some(&tv));
                }
                NSApplication::sharedApplication(MainThreadMarker::from(&*tv)).orderFrontCharacterPalette(None);
            });
        }
        self.core.apply(&new.text);
        self.core.paint_styles();
    }

    /// Takes `styler` (or none), and sets every paragraph again in the area's font and colour.
    fn set_styler(&self, styler: Option<&Styler>, enabled: bool) {
        let tv = &self.text_view;
        let font = tv.font().unwrap_or_else(|| NSFont::systemFontOfSize(NSFont::systemFontSize()));
        let color = if enabled { NSColor::textColor() } else { NSColor::disabledControlTextColor() };
        let mut styles = self.core.styles.borrow_mut();
        let Some(styler) = styler else {
            if styles.take().is_some()
                && let Some(storage) = unsafe { tv.textStorage() }
            {
                // The runs' attributes go with the styler.
                let keys: [&objc2_foundation::NSAttributedStringKey; 2] =
                    unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
                let values: [&AnyObject; 2] = [&font, &color];
                let attrs = NSDictionary::from_slices(&keys, &values);
                unsafe { storage.setAttributes_range(Some(&attrs), NSRange::new(0, storage.length())) };
            }
            return;
        };
        match styles.as_mut() {
            Some(current) if current.styler() == styler => current.set_base(font, color),
            _ => *styles = Some(Styles::new(styler.clone(), font, color)),
        }
    }

    /// By default the area wears the rounded bezel of AppKit's text fields, so it matches the
    /// single-line fields beside it, and the system focus ring follows that bezel. A style with
    /// rounded corners draws it as a capsule instead: a hairline in the system separator colour,
    /// the text kept clear of the curve. A zero radius asks for the square scroll view bezel.
    /// A borderless area draws none of these and no fill, its text where the bezel's would be.
    fn apply_shape(&self, props: &TextAreaProps) {
        let radius = props.style.corner_radii.top_left.map(crate::look::length);
        let mtm = self.core.cx.mtm;
        self.scroll.set_capsule(None);
        self.scroll.setDrawsBackground(true);
        if props.borderless {
            self.scroll.set_bezel(None);
            self.scroll.setBorderType(NSBorderType::NoBorder);
            self.scroll.setDrawsBackground(false);
            self.scroll.contentView().setDrawsBackground(false);
            self.text_view.setDrawsBackground(false);
            self.scroll.setFocusRingType(NSFocusRingType::None);
            self.scroll.set_inset(NSSize::new(BEZEL_INSET.0, BEZEL_INSET.1));
            return;
        }
        let Some(radius) = radius else {
            self.scroll.setBorderType(NSBorderType::NoBorder);
            let bezel = inert_bezel(mtm);
            bezel.setEnabled(self.enabled.get());
            self.scroll.set_bezel(Some(bezel));
            // The text keeps clear of the bezel's edge and corners, as a field's text does.
            self.scroll.set_inset(NSSize::new(BEZEL_INSET.0, BEZEL_INSET.1));
            return;
        };
        self.scroll.set_bezel(None);
        if radius <= 0. {
            self.scroll.setBorderType(NSBorderType::BezelBorder);
            self.scroll.set_inset(NSSize::new(0., 0.));
            return;
        }
        self.scroll.setBorderType(NSBorderType::NoBorder);
        self.scroll.setDrawsBackground(false);
        self.scroll.contentView().setDrawsBackground(false);
        self.text_view.setDrawsBackground(false);
        self.scroll.set_capsule(Some(f64::from(radius)));
        let inset = (f64::from(radius) * 0.4).round();
        // One line fills the capsule's height (twice its radius), the text centred in it.
        let vertical = (f64::from(radius) - self.line_height() / 2.).max(6.);
        self.scroll.set_inset(NSSize::new(inset, vertical));
    }

    /// NSTextView draws `placeholderAttributedString` while empty; it is not in the public
    /// headers, so it is set only where the text view answers to it.
    fn set_placeholder(&self, placeholder: Option<&str>) {
        let tv = &self.text_view;
        tv.setAccessibilityPlaceholderValue(placeholder.map(NSString::from_str).as_deref());
        let setter = sel!(setPlaceholderAttributedString:);
        if !tv.respondsToSelector(setter) {
            return;
        }
        let attributed = match (placeholder, tv.font()) {
            (Some(p), Some(font)) => Some(unsafe {
                NSAttributedString::new_with_attributes(
                    &NSString::from_str(p),
                    &attributes(&font, Some(&NSColor::placeholderTextColor())),
                )
            }),
            _ => None,
        };
        let _: () = unsafe { msg_send![&**tv, setPlaceholderAttributedString: attributed.as_deref()] };
    }

    fn line_height(&self) -> f64 {
        let tv = &self.text_view;
        match (unsafe { tv.layoutManager() }, tv.font()) {
            (Some(lm), Some(font)) => lm.defaultLineHeightForFont(&font),
            _ => 0.,
        }
    }
}

impl Drop for TextAreaControl {
    fn drop(&mut self) {
        self.text_view.setDelegate(None);
        if let Some(storage) = unsafe { self.text_view.textStorage() } {
            storage.setDelegate(None);
        }
        if let Some(layout) = unsafe { self.text_view.layoutManager() } {
            layout.setDelegate(None);
        }
    }
}

impl FormControl for TextAreaControl {
    fn view(&self) -> &NSView {
        &self.scroll
    }

    fn focus_view(&self) -> &NSView {
        &self.text_view
    }

    fn appearance_changed(&self) {
        self.scroll.draw_capsule();
    }

    fn update(&mut self, old: &Props, new: &Props) -> bool {
        let (old, new): (&TextAreaProps, &TextAreaProps) = (props(old), props(new));
        self.apply(Some(old), new);
        self.props = new.clone();
        true
    }

    fn measure(&self, width: Option<f32>) -> Size {
        let tv = &self.text_view;
        let inset = tv.textContainerInset();
        let padding = unsafe { tv.textContainer() }.map_or(0., |c| c.lineFragmentPadding());
        let placeholder = self.props.placeholder.as_deref().map_or(0., |p| text_width(p, tv.font().as_deref()));
        let line = self.line_height().max(1.);
        let lines = match self.props.max_lines {
            Some(max) => {
                let text_width = width.map_or(tv.frame().size.width, f64::from)
                    - 2. * (inset.width + padding)
                    - if width.is_some() { f64::from(self.props.trailing_room) } else { 0. };
                let text = self.core.current();
                let font = tv.font();
                let used = font.as_deref().map_or(line, |f| {
                    let s = unsafe {
                        NSAttributedString::new_with_attributes(
                            &NSString::from_str(&format!("{text} ")),
                            &attributes(f, None),
                        )
                    };
                    let r = s.boundingRectWithSize_options_context(
                        NSSize::new(text_width.max(1.), f64::MAX),
                        objc2_app_kit::NSStringDrawingOptions::UsesLineFragmentOrigin,
                        None,
                    );
                    r.size.height
                });
                ((used / line).round() as u16).clamp(self.props.min_lines.max(1), max.max(1))
            }
            None => self.props.min_lines.max(1),
        };
        let content =
            NSSize::new(placeholder + 2. * (inset.width + padding), f64::from(lines) * line + 2. * inset.height);
        let frame = unsafe {
            NSScrollView::frameSizeForContentSize_horizontalScrollerClass_verticalScrollerClass_borderType_controlSize_scrollerStyle(
                content,
                None,
                None,
                self.scroll.borderType(),
                NSControlSize::Regular,
                NSScroller::preferredScrollerStyle(self.core.cx.mtm),
                self.core.cx.mtm,
            )
        };
        Size { width: width.unwrap_or(frame.width.ceil() as f32), height: frame.height.ceil() as f32 }
    }

    fn probe(&self, probe: &mut Probe) {
        probe.enabled = Some(self.enabled.get());
        probe.value = Some(self.core.current());
        probe.placeholder = self.props.placeholder.as_ref().map(|p| p.to_string());
    }

    fn perform(&self, action: &SemanticAction) -> Option<Result<(), ActionError>> {
        perform_text(&self.core, self.props.enabled, self.props.editable, action)
    }
}
