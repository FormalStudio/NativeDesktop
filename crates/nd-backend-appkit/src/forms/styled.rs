//! A styled `text_area` on AppKit: the app's styler applied to the text view's `NSTextStorage`
//! one paragraph at a time. The storage's delegate keeps the paragraph cache in step with every
//! character edit (lengths in UTF-16 units); the paragraphs an edit restyled are set once the
//! edit is over, inside one `beginEditing`/`endEditing`, as attribute-only changes the undo
//! manager never sees. Nothing is set while an input method composes; the paragraphs wait for
//! the composition to end.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

use nd_ui::styled_text::ParagraphCache;
use nd_ui::{EditKey, SpanStyle, StyledParagraph, Styler, TextColor, TextFill};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBackgroundColorAttributeName, NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
    NSLayoutManager, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSStrikethroughStyleAttributeName,
    NSTextStorage, NSTextView, NSUnderlineStyleAttributeName,
};
use objc2_foundation::{NSAttributedStringKey, NSDictionary, NSNumber, NSRange};

use super::preview;
use crate::look::{Color, SystemText};
use crate::rich::{RunLook, run_font};

fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The byte offset `units` UTF-16 units into `text`.
fn byte_at(text: &str, units: usize) -> usize {
    let mut count = 0;
    for (index, ch) in text.char_indices() {
        if count >= units {
            return index;
        }
        count += ch.len_utf16();
    }
    text.len()
}

fn units_at(text: &str, byte: usize) -> usize {
    if text.is_ascii() { byte } else { units(&text[..byte.min(text.len())]) }
}

fn substring(storage: &NSTextStorage, range: Range<usize>) -> String {
    let string = storage.string();
    let range = NSRange::new(range.start, range.end.saturating_sub(range.start));
    string.substringWithRange(range).to_string()
}

/// What the runs are set on: the area's font and colour, and what it has resolved so far.
struct Look {
    font: Retained<NSFont>,
    color: Retained<NSColor>,
    fonts: HashMap<(u32, u32, bool, bool), Retained<NSFont>>,
    colors: HashMap<u8, Retained<NSColor>>,
    /// The natural line height of each font, which `lineHeightMultiple` multiplies.
    heights: HashMap<u32, f64>,
    measure: Retained<NSLayoutManager>,
}

impl Look {
    fn font(&mut self, style: &SpanStyle) -> Retained<NSFont> {
        let scale = style.scale();
        let weight = style.weight(400.);
        if scale == 1. && weight == 400. && !style.italic && !style.mono {
            return self.font.clone();
        }
        let size = self.font.pointSize() as f32 * scale;
        let key = (size.to_bits(), weight.to_bits(), style.italic, style.mono);
        self.fonts
            .entry(key)
            .or_insert_with(|| {
                let run = RunLook {
                    text: String::new(),
                    color: None,
                    size,
                    weight,
                    italic: style.italic,
                    mono: style.mono,
                    underline: false,
                    strikethrough: false,
                    link: None,
                };
                run_font(&run, false, None)
            })
            .clone()
    }

    fn color(&mut self, color: TextColor) -> Retained<NSColor> {
        let (slot, system) = match color {
            TextColor::Primary => (0, None),
            TextColor::Secondary => (1, Some(SystemText::Secondary)),
            TextColor::Tertiary => (2, Some(SystemText::Tertiary)),
            TextColor::Accent => (3, Some(SystemText::Accent)),
            TextColor::Danger => (4, Some(SystemText::Danger)),
            TextColor::Success => (5, Some(SystemText::Success)),
            TextColor::Warning => (6, Some(SystemText::Warning)),
            TextColor::Custom(c) => return Color::from(c).ns_color(),
        };
        self.colors
            .entry(slot)
            .or_insert_with(|| system.map_or_else(NSColor::textColor, |s| Color::Legible(s).ns_color()))
            .clone()
    }

    fn fill(fill: TextFill) -> Retained<NSColor> {
        match fill {
            TextFill::Subtle => NSColor::quaternaryLabelColor(),
            TextFill::Custom(c) => Color::from(c).ns_color(),
        }
    }

    /// `font`'s natural line height over its size.
    fn natural(&mut self, font: &NSFont) -> f64 {
        let size = font.pointSize();
        let measure = &self.measure;
        let height =
            *self.heights.entry((size as f32).to_bits()).or_insert_with(|| measure.defaultLineHeightForFont(font));
        height / size.max(1.)
    }
}

/// A text area's styler and the paragraphs it has styled.
pub(crate) struct Styles {
    styler: Styler,
    /// A paragraph's layout is `Some` once its attributes are on the storage.
    cache: ParagraphCache<()>,
    look: Look,
}

impl Styles {
    pub fn new(styler: Styler, font: Retained<NSFont>, color: Retained<NSColor>) -> Self {
        let look = Look {
            font,
            color,
            fonts: HashMap::new(),
            colors: HashMap::new(),
            heights: HashMap::new(),
            measure: NSLayoutManager::new(),
        };
        Self { styler, cache: ParagraphCache::default(), look }
    }

    pub fn styler(&self) -> &Styler {
        &self.styler
    }

    /// The area's font or colour changed: every paragraph is set again.
    pub fn set_base(&mut self, font: Retained<NSFont>, color: Retained<NSColor>) {
        self.look.font = font;
        self.look.color = color;
        self.look.fonts.clear();
        self.look.heights.clear();
        for p in self.cache.paragraphs_mut() {
            p.layout = None;
        }
    }

    /// A character edit the storage processed: `range` is the edited range after it, `delta`
    /// the change in length.
    pub fn edited(&mut self, storage: &NSTextStorage, range: NSRange, delta: isize) {
        let old_len = (range.length as isize - delta).max(0) as usize;
        let old = range.location..range.location + old_len;
        if self.cache.is_empty() {
            let all = storage.string().to_string();
            self.cache.reset(&all, &self.styler, units);
            return;
        }
        self.cache.edit(old, range.length, &self.styler, units, |r| Cow::Owned(substring(storage, r)));
    }

    /// Sets the attributes of every paragraph restyled since the last call, then the typing
    /// attributes at the caret.
    pub fn apply(&mut self, view: &NSTextView, storage: &NSTextStorage) {
        if self.cache.is_empty() {
            let all = storage.string().to_string();
            self.cache.reset(&all, &self.styler, units);
        }
        let total = storage.length();
        let mut start = 0;
        let mut began = false;
        let count = self.cache.paragraphs().len();
        let Self { cache, look, .. } = self;
        for (index, p) in cache.paragraphs_mut().iter_mut().enumerate() {
            if p.layout.is_none() && start + p.len <= total {
                if !began {
                    storage.beginEditing();
                    began = true;
                }
                let newline = usize::from(index + 1 < count && start + p.len < total);
                set_paragraph(storage, look, start, p.len, newline, &p.styled);
                p.layout = Some(());
            }
            start += p.len + 1;
        }
        if began {
            storage.endEditing();
        }
        self.typing_attributes(view, storage);
    }

    /// What typing at the caret sets text in: the character before it, inside its paragraph.
    fn typing_attributes(&self, view: &NSTextView, storage: &NSTextStorage) {
        let selection = view.selectedRange();
        if selection.length > 0 || storage.length() == 0 {
            return;
        }
        let string = storage.string();
        let at = selection.location;
        let index = if at > 0 && string.characterAtIndex(at - 1) != u16::from(b'\n') {
            at - 1
        } else if at < storage.length() {
            at
        } else {
            return;
        };
        let attributes = unsafe { storage.attributesAtIndex_effectiveRange(index, std::ptr::null_mut()) };
        // Typed text takes the run's look but not its concealment, which the restyle decides.
        let attributes = objc2_foundation::NSMutableCopying::mutableCopy(&*attributes);
        attributes.removeObjectForKey(&preview::conceal_key());
        attributes.removeObjectForKey(&preview::block_key());
        unsafe { view.setTypingAttributes(&attributes) };
    }

    /// The edit the styler makes for `key` at `selection`: the range to replace, its new text,
    /// and the selection after, all in UTF-16 units.
    pub fn edit_for(
        &self,
        storage: &NSTextStorage,
        selection: NSRange,
        key: EditKey,
    ) -> Option<(NSRange, String, NSRange)> {
        if self.cache.is_empty() {
            return None;
        }
        let (index, start) = self.cache.locate(selection.location);
        let p = &self.cache.paragraphs()[index];
        if selection.location + selection.length > start + p.len {
            return None;
        }
        let text = substring(storage, start..start + p.len);
        let a = byte_at(&text, selection.location - start);
        let b = byte_at(&text, selection.location + selection.length - start);
        let edit = self.styler.edit(key, &text, a..b, p.state())?;
        let range = edit.range.start.min(text.len())..edit.range.end.min(text.len());
        let replace = NSRange::new(start + units_at(&text, range.start), units(&text[range.clone()]));
        let mut after = text.clone();
        after.replace_range(range, &edit.text);
        let (a, b) = (edit.selection.start.min(after.len()), edit.selection.end.min(after.len()));
        let (a, b) = (units_at(&after, a), units_at(&after, b));
        Some((replace, edit.text, NSRange::new(start + a, b.saturating_sub(a))))
    }
}

fn set_paragraph(
    storage: &NSTextStorage,
    look: &mut Look,
    start: usize,
    len: usize,
    newline: usize,
    styled: &StyledParagraph,
) {
    let ps = &styled.paragraph;
    let size = look.font.pointSize();
    let style = NSMutableParagraphStyle::new();
    if let Some(multiple) = ps.line_height {
        // `lineHeightMultiple` multiplies the font's natural height; the styler's multiple is
        // of the font size. The paragraph's largest run sets the line.
        let largest = styled.runs.iter().max_by(|a, b| a.1.scale().total_cmp(&b.1.scale()));
        let font = match largest {
            Some((_, s)) => look.font(s),
            None => look.font.clone(),
        };
        let natural = look.natural(&font);
        style.setLineHeightMultiple(f64::from(multiple) / natural.max(0.1));
    }
    style.setParagraphSpacingBefore(f64::from(ps.space_before) * size);
    style.setParagraphSpacing(f64::from(ps.space_after) * size);
    style.setFirstLineHeadIndent(f64::from(ps.first_indent) * size);
    style.setHeadIndent(f64::from(ps.head_indent) * size);
    let (font_key, color_key, paragraph_key): (&NSAttributedStringKey, &NSAttributedStringKey, &NSAttributedStringKey) =
        unsafe { (NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName) };
    let base: [&AnyObject; 3] = [&look.font, &look.color, &style];
    let attrs = NSDictionary::from_slices(&[font_key, color_key, paragraph_key], &base);
    unsafe { storage.setAttributes_range(Some(&attrs), NSRange::new(start, len + newline)) };
    if let Some(block) = ps.block {
        // Over the line break too, so a run of code paragraphs reads as one well.
        let value = preview::block_value(block);
        unsafe { storage.addAttribute_value_range(&preview::block_key(), &value, NSRange::new(start, len + newline)) };
    }
    if styled.runs.is_empty() {
        return;
    }
    let text = substring(storage, start..start + len);
    let one = NSNumber::new_isize(1);
    let conceal_key = preview::conceal_key();
    for (range, span) in &styled.runs {
        if *span == SpanStyle::default() || range.start >= range.end || range.end > text.len() {
            continue;
        }
        let mut keys: Vec<&NSAttributedStringKey> = Vec::new();
        let mut values: Vec<Retained<AnyObject>> = Vec::new();
        let font = look.font(span);
        if !std::ptr::eq(&*font, &*look.font) {
            keys.push(font_key);
            values.push(Retained::into_super(Retained::into_super(font)));
        }
        if let Some(color) = span.color() {
            keys.push(color_key);
            values.push(Retained::into_super(Retained::into_super(look.color(color))));
        }
        if let Some(fill) = span.background {
            keys.push(unsafe { NSBackgroundColorAttributeName });
            values.push(Retained::into_super(Retained::into_super(Look::fill(fill))));
        }
        if span.underlined() {
            keys.push(unsafe { NSUnderlineStyleAttributeName });
            values.push(Retained::into_super(Retained::into_super(Retained::into_super(one.clone()))));
        }
        if span.strikethrough {
            keys.push(unsafe { NSStrikethroughStyleAttributeName });
            values.push(Retained::into_super(Retained::into_super(Retained::into_super(one.clone()))));
        }
        if let Some(conceal) = span.conceal {
            keys.push(&conceal_key);
            values.push(Retained::into_super(Retained::into_super(Retained::into_super(preview::conceal_value(
                conceal,
            )))));
        }
        if keys.is_empty() {
            continue;
        }
        let values: Vec<&AnyObject> = values.iter().map(|v| &**v).collect();
        let attrs = NSDictionary::from_slices(&keys, &values);
        let a = units_at(&text, range.start);
        let b = units_at(&text, range.end);
        unsafe { storage.addAttributes_range(&attrs, NSRange::new(start + a, b - a)) };
    }
}

/// Whether `view` is composing with an input method.
pub(crate) fn composing(view: &NSTextView) -> bool {
    unsafe { msg_send![view, hasMarkedText] }
}
