//! Live preview on AppKit (TextKit 1). The styler's concealed runs carry `NDConceal` on the
//! text storage and a paragraph's block carries `NDBlock`. Outside the revealed range (the
//! selection's paragraphs, while the area has focus) the layout manager's delegate makes
//! concealed glyphs control glyphs with no advance, except a mark's first, which is whitespace
//! [`MARK_WIDTH`] ems wide; the text view then draws the blocks and marks behind the text. The storage keeps
//! the source, so the caret, selection, copy and undo all work on the Markdown.

use std::ptr::NonNull;

use nd_ui::{Conceal, MARK_WIDTH, TextBlock};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBezierPath, NSColor, NSControlCharacterAction, NSFont, NSFontAttributeName, NSGlyphProperty, NSLayoutManager,
    NSLineCapStyle, NSLineJoinStyle, NSTextView,
};
use objc2_core_graphics::CGGlyph;
use objc2_foundation::{NSAttributedString, NSNumber, NSPoint, NSRange, NSRect, NSSize, NSString};

thread_local! {
    static CONCEAL: Retained<NSString> = NSString::from_str("NDConceal");
    static BLOCK: Retained<NSString> = NSString::from_str("NDBlock");
}

/// A task box's side and corner, and a bullet's diameter, in ems.
const TASK_BOX: f64 = 1.0625;
const TASK_RADIUS: f64 = 0.3; // nd-lint: allow, a share of the em, and AppKit has no task box
const BULLET: f64 = 0.34;

pub(crate) fn conceal_key() -> Retained<NSString> {
    CONCEAL.with(Clone::clone)
}

pub(crate) fn block_key() -> Retained<NSString> {
    BLOCK.with(Clone::clone)
}

pub(crate) fn conceal_value(conceal: Conceal) -> Retained<NSNumber> {
    NSNumber::new_isize(match conceal {
        Conceal::Hide => 1,
        Conceal::Bullet => 2,
        Conceal::Task(false) => 3,
        Conceal::Task(true) => 4,
    })
}

pub(crate) fn block_value(block: TextBlock) -> Retained<NSNumber> {
    NSNumber::new_isize(match block {
        TextBlock::Code => 1,
        TextBlock::Rule => 2,
        TextBlock::Quote(depth) => 10 + isize::from(depth),
    })
}

fn number(value: Option<Retained<AnyObject>>) -> Option<isize> {
    Some(value?.downcast::<NSNumber>().ok()?.integerValue())
}

/// The conceal at `index` and the range of its run.
fn conceal_at(text: &NSAttributedString, index: usize) -> Option<(Conceal, NSRange)> {
    if index >= text.length() {
        return None;
    }
    let mut run = NSRange::new(0, 0);
    let value = CONCEAL.with(|key| unsafe { text.attribute_atIndex_effectiveRange(key, index, &mut run) });
    let conceal = match number(value)? {
        1 => Conceal::Hide,
        2 => Conceal::Bullet,
        3 => Conceal::Task(false),
        4 => Conceal::Task(true),
        _ => return None,
    };
    Some((conceal, run))
}

fn block_at(text: &NSAttributedString, index: usize, within: NSRange) -> Option<(TextBlock, NSRange)> {
    let mut run = NSRange::new(0, 0);
    let value =
        BLOCK.with(|key| unsafe { text.attribute_atIndex_longestEffectiveRange_inRange(key, index, &mut run, within) });
    let block = match number(value)? {
        1 => TextBlock::Code,
        2 => TextBlock::Rule,
        n @ 11.. => TextBlock::Quote(u8::try_from(n - 10).unwrap_or(u8::MAX)),
        _ => return None,
    };
    Some((block, run))
}

fn contains(range: NSRange, index: usize) -> bool {
    index >= range.location && index < range.location + range.length
}

/// A mark (bullet or task box) starts at `index`: the glyph that stands in for its run.
fn mark_start(text: &NSAttributedString, index: usize) -> Option<Conceal> {
    let (conceal, _) = conceal_at(text, index).filter(|(c, _)| *c != Conceal::Hide)?;
    let first = index == 0 || conceal_at(text, index - 1).map(|(c, _)| c) != Some(conceal);
    first.then_some(conceal)
}

/// `layoutManager:shouldGenerateGlyphs:..`: concealed glyphs outside `revealed` become
/// control glyphs. Returns how many glyphs it set, or 0 to leave them be.
///
/// # Safety
///
/// The pointers are the layout manager's, each `range.length` long.
pub(crate) unsafe fn generate(
    layout: &NSLayoutManager,
    glyphs: NonNull<CGGlyph>,
    props: NonNull<NSGlyphProperty>,
    chars: NonNull<usize>,
    font: &NSFont,
    range: NSRange,
    revealed: NSRange,
) -> usize {
    let Some(storage) = (unsafe { layout.textStorage() }) else { return 0 };
    let count = range.length;
    // SAFETY: the layout manager hands `count` character indexes and properties.
    let (indexes, given) = unsafe {
        (std::slice::from_raw_parts(chars.as_ptr(), count), std::slice::from_raw_parts(props.as_ptr(), count))
    };
    let mut set: Option<Vec<NSGlyphProperty>> = None;
    let mut i = 0;
    while i < count {
        let index = indexes[i];
        if conceal_at(&storage, index).is_none() {
            // Nothing concealed until the run ends: skip its glyphs.
            let mut plain = NSRange::new(index, 1);
            let _ = CONCEAL.with(|key| unsafe {
                storage.attribute_atIndex_effectiveRange(key, index.min(storage.length().saturating_sub(1)), &mut plain)
            });
            let end = (plain.location + plain.length).max(index + 1);
            while i < count && indexes[i] < end {
                i += 1;
            }
            continue;
        }
        if !contains(revealed, index) {
            // Null glyphs that open a paragraph make the typesetter lay it out as a
            // continuation (head indent, no paragraph spacing), so concealed glyphs are
            // control glyphs that `action` gives no advance.
            set.get_or_insert_with(|| given.to_vec())[i] = NSGlyphProperty::ControlCharacter;
        }
        i += 1;
    }
    let Some(set) = set else { return 0 };
    // SAFETY: `set` holds `count` properties for the same glyphs and characters.
    unsafe {
        layout.setGlyphs_properties_characterIndexes_font_forGlyphRange(
            glyphs,
            NonNull::new_unchecked(set.as_ptr().cast_mut()),
            chars,
            font,
            range,
        );
    }
    count
}

/// `layoutManager:shouldUseAction:forControlCharacterAtIndex:`: outside `revealed` a mark
/// takes room as whitespace and other concealed glyphs none; every other control character
/// keeps its action.
pub(crate) fn action(
    layout: &NSLayoutManager,
    action: NSControlCharacterAction,
    index: usize,
    revealed: NSRange,
) -> NSControlCharacterAction {
    let Some(storage) = (unsafe { layout.textStorage() }) else { return action };
    if contains(revealed, index) {
        return action;
    }
    if mark_start(&storage, index).is_some() {
        NSControlCharacterAction::Whitespace
    } else if conceal_at(&storage, index).is_some() {
        NSControlCharacterAction::ZeroAdvancement
    } else {
        action
    }
}

fn font_at(text: &NSAttributedString, index: usize) -> Option<Retained<NSFont>> {
    let value = unsafe { text.attribute_atIndex_effectiveRange(NSFontAttributeName, index, std::ptr::null_mut()) };
    value?.downcast::<NSFont>().ok()
}

/// `layoutManager:boundingBoxForControlGlyphAtIndex:..`: a mark's room, [`MARK_WIDTH`] ems.
pub(crate) fn mark_box(layout: &NSLayoutManager, index: usize, proposed: NSRect, at: NSPoint) -> NSRect {
    let size = unsafe { layout.textStorage() }
        .filter(|s| index < s.length())
        .and_then(|s| font_at(&s, index))
        .map_or(13., |f| f.pointSize());
    NSRect::new(NSPoint::new(at.x, 0.), NSSize::new(f64::from(MARK_WIDTH) * size, proposed.size.height))
}

/// Where the mark at `index` draws, in the view: its room's left edge and the y of the middle
/// of its line's capitals, and the em it sizes from.
fn mark_place(
    view: &NSTextView,
    layout: &NSLayoutManager,
    storage: &NSAttributedString,
    index: usize,
) -> (NSPoint, f64) {
    let origin = view.textContainerOrigin();
    let glyph = layout.glyphIndexForCharacterAtIndex(index);
    let line = unsafe { layout.lineFragmentRectForGlyphAtIndex_effectiveRange(glyph, std::ptr::null_mut()) };
    let at = layout.locationForGlyphAtIndex(glyph);
    let font = font_at(storage, index).or_else(|| view.font());
    let (em, cap) = font.map_or((13., 9.), |f| (f.pointSize(), f.capHeight()));
    let x = origin.x + line.origin.x + at.x;
    let y = origin.y + line.origin.y + at.y - cap / 2.;
    (NSPoint::new(x, y), em)
}

fn task_rect(centre: NSPoint, em: f64) -> NSRect {
    let side = (TASK_BOX * em).round();
    NSRect::new(NSPoint::new(centre.x, (centre.y - side / 2.).round()), NSSize::new(side, side))
}

/// The task box under `point` (in the view) outside `revealed`: its run.
pub(crate) fn task_at(view: &NSTextView, point: NSPoint, revealed: NSRange) -> Option<NSRange> {
    let layout = unsafe { view.layoutManager() }?;
    let container = unsafe { view.textContainer() }?;
    let storage = unsafe { view.textStorage() }?;
    let origin = view.textContainerOrigin();
    let local = NSPoint::new(point.x - origin.x, point.y - origin.y);
    let glyph = layout.glyphIndexForPoint_inTextContainer(local, &container);
    let index =
        unsafe { layout.characterRangeForGlyphRange_actualGlyphRange(NSRange::new(glyph, 1), std::ptr::null_mut()) }
            .location;
    // The point can resolve to the glyph either side of the box.
    for index in [index, index.saturating_sub(1), index + 1] {
        if contains(revealed, index) {
            continue;
        }
        let Some(Conceal::Task(_)) = mark_start(&storage, index) else { continue };
        let (at, em) = mark_place(view, &layout, &storage, index);
        let hit = task_rect(at, em);
        let slop = 4.;
        let hit = NSRect::new(
            NSPoint::new(hit.origin.x - slop, hit.origin.y - slop),
            NSSize::new(hit.size.width + 2. * slop, hit.size.height + 2. * slop),
        );
        let inside = point.x >= hit.origin.x
            && point.x <= hit.origin.x + hit.size.width
            && point.y >= hit.origin.y
            && point.y <= hit.origin.y + hit.size.height;
        if inside {
            return conceal_at(&storage, index).map(|(_, run)| run);
        }
    }
    None
}

/// Draws the blocks and marks of the text in `dirty` behind it: code wells, quote bars, rules
/// and, outside `revealed`, bullets and task boxes.
pub(crate) fn draw(view: &NSTextView, dirty: NSRect, revealed: NSRange) {
    let (Some(layout), Some(container), Some(storage)) =
        (unsafe { view.layoutManager() }, unsafe { view.textContainer() }, unsafe { view.textStorage() })
    else {
        return;
    };
    let total = storage.length();
    if total == 0 {
        return;
    }
    let origin = view.textContainerOrigin();
    let local = NSRect::new(NSPoint::new(dirty.origin.x - origin.x, dirty.origin.y - origin.y), dirty.size);
    let glyphs = layout.glyphRangeForBoundingRectWithoutAdditionalLayout_inTextContainer(local, &container);
    let chars = unsafe { layout.characterRangeForGlyphRange_actualGlyphRange(glyphs, std::ptr::null_mut()) };
    let string = storage.string();
    let visible = string.paragraphRangeForRange(NSRange::new(chars.location.min(total), chars.length));
    let end = (visible.location + visible.length).min(total);
    let em = view.font().map_or(13., |f| f.pointSize());
    let padding = container.lineFragmentPadding();
    let width = container.containerSize().width;
    let left = origin.x + padding;
    let right = origin.x + width - padding;
    let whole = NSRange::new(0, total);
    let line_of = |index: usize| {
        let glyph = layout.glyphIndexForCharacterAtIndex(index.min(total - 1));
        let rect = unsafe { layout.lineFragmentRectForGlyphAtIndex_effectiveRange(glyph, std::ptr::null_mut()) };
        NSRect::new(NSPoint::new(rect.origin.x + origin.x, rect.origin.y + origin.y), rect.size)
    };
    let mut at = visible.location;
    while at < end {
        let Some((block, run)) = block_at(&storage, at, whole) else {
            let mut next = NSRange::new(at, 1);
            let _ = BLOCK.with(|key| unsafe {
                storage.attribute_atIndex_longestEffectiveRange_inRange(key, at, &mut next, whole)
            });
            at = (next.location + next.length).max(at + 1);
            continue;
        };
        let last = (run.location + run.length).saturating_sub(1).min(total - 1);
        let (top, bottom) = (line_of(run.location), line_of(last));
        let (y0, y1) = (top.origin.y, bottom.origin.y + bottom.size.height);
        match block {
            TextBlock::Code => {
                let rect = NSRect::new(NSPoint::new(left, y0), NSSize::new(right - left, y1 - y0));
                let radius = (em * 0.375).round();
                let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(inset(rect, 0.5), radius, radius);
                NSColor::labelColor().colorWithAlphaComponent(0.035).setFill();
                path.fill();
                NSColor::labelColor().colorWithAlphaComponent(0.08).setStroke();
                path.setLineWidth(1.);
                path.stroke();
            }
            TextBlock::Quote(depth) => {
                NSColor::labelColor().colorWithAlphaComponent(0.25).setFill();
                for level in 0..depth {
                    let x = left + em * f64::from(level);
                    let bar = NSRect::new(NSPoint::new(x, y0), NSSize::new(3., y1 - y0));
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bar, 1.5, 1.5).fill();
                }
            }
            TextBlock::Rule => {
                // Each rule paragraph on its own: one may be revealed and the next not.
                let mut index = run.location;
                while index <= last {
                    let paragraph = string.paragraphRangeForRange(NSRange::new(index, 0));
                    if !contains(revealed, index) {
                        let line = line_of(index);
                        let y = (line.origin.y + line.size.height / 2.).floor() + 0.5;
                        NSColor::separatorColor().setFill();
                        NSBezierPath::fillRect(NSRect::new(NSPoint::new(left, y - 0.5), NSSize::new(right - left, 1.)));
                    }
                    index = paragraph.location + paragraph.length.max(1);
                }
            }
        }
        at = (run.location + run.length).max(at + 1);
    }
    let mut at = visible.location;
    while at < end {
        let Some((conceal, run)) = conceal_at(&storage, at) else {
            at += 1;
            continue;
        };
        if conceal != Conceal::Hide && !contains(revealed, at) && mark_start(&storage, at).is_some() {
            let (place, em) = mark_place(view, &layout, &storage, at);
            draw_mark(conceal, place, em);
        }
        at = (run.location + run.length).max(at + 1);
    }
}

fn inset(rect: NSRect, by: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.origin.x + by, rect.origin.y + by),
        NSSize::new((rect.size.width - 2. * by).max(0.), (rect.size.height - 2. * by).max(0.)),
    )
}

fn draw_mark(conceal: Conceal, at: NSPoint, em: f64) {
    match conceal {
        Conceal::Bullet => {
            let d = BULLET * em;
            let dot = NSRect::new(NSPoint::new(at.x + 0.45 * em - d / 2., at.y - d / 2.), NSSize::new(d, d));
            NSColor::textColor().setFill();
            NSBezierPath::bezierPathWithOvalInRect(dot).fill();
        }
        Conceal::Task(done) => {
            let rect = task_rect(at, em);
            let radius = TASK_RADIUS * em;
            if done {
                NSColor::controlAccentColor().setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius).fill();
                let (x, y, w) = (rect.origin.x, rect.origin.y, rect.size.width);
                let tick = NSBezierPath::bezierPath();
                tick.moveToPoint(NSPoint::new(x + w * 0.26, y + w * 0.52));
                tick.lineToPoint(NSPoint::new(x + w * 0.43, y + w * 0.69));
                tick.lineToPoint(NSPoint::new(x + w * 0.75, y + w * 0.33));
                tick.setLineWidth(0.11 * em);
                tick.setLineCapStyle(NSLineCapStyle::Round);
                tick.setLineJoinStyle(NSLineJoinStyle::Round);
                NSColor::whiteColor().setStroke();
                tick.stroke();
            } else {
                let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(inset(rect, 0.5), radius, radius);
                path.setLineWidth(1.);
                NSColor::labelColor().colorWithAlphaComponent(0.3).setStroke();
                path.stroke();
            }
        }
        Conceal::Hide => {}
    }
}
