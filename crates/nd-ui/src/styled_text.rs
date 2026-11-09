//! Styled editing: a text area whose paragraphs the app styles as the user types (a live
//! markdown preview). The app's [`TextStyler`] styles one paragraph at a time and hands a small
//! state to the next one (inside a code fence or not), so an edit restyles the paragraphs it
//! touched and goes on only while that state differs from what the next paragraph was styled
//! with. [`ParagraphCache`] is that bookkeeping, shared by every backend: AppKit counts its
//! lengths in UTF-16 units, the drawn editor in bytes.
//!
//! A paragraph is the text between two line breaks, the unit AppKit's paragraph styles apply to.

use std::any::TypeId;
use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;

use nd_gpui::gpui::Hsla;

pub use crate::{SpanStyle, TextColor};

/// A run's background, by theme token.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextFill {
    /// The text's own ink at a low alpha: a code span's well, legible on any surface.
    Subtle,
    /// A colour the app resolved itself, from `cx.theme()`.
    Custom(Hsla),
}

/// How a paragraph is set. Lengths are in ems (multiples of the area's font size), so they
/// follow its size; the default is the area's own setting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParagraphStyle {
    /// Line height as a multiple of the paragraph's font size.
    pub line_height: Option<f32>,
    pub space_before: f32,
    pub space_after: f32,
    /// Indent of the paragraph's first line.
    pub first_indent: f32,
    /// Indent of its wrapped lines: past a list marker, so wrapped text lines up with the item's.
    pub head_indent: f32,
}

/// What a styler makes of one paragraph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyledParagraph {
    /// Byte ranges into the paragraph, in order and not overlapping; text outside them keeps
    /// the area's style.
    pub runs: Vec<(Range<usize>, SpanStyle)>,
    pub paragraph: ParagraphStyle,
    /// Handed to the next paragraph's [`TextStyler::style`]: `0` for the first.
    pub state: u32,
}

impl StyledParagraph {
    /// The largest run scale: the drawn editor sets a paragraph at one size.
    pub fn scale(&self) -> f32 {
        self.runs.iter().map(|(_, s)| s.scale()).fold(1., f32::max)
    }
}

/// A key the area would otherwise handle itself, offered to the styler first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditKey {
    Return,
    Tab,
    /// Shift-Tab.
    Backtab,
    /// Command-B (Control-B off macOS).
    Bold,
    /// Command-I (Control-I off macOS).
    Italic,
}

/// An edit a styler makes for a key, inside the caret's paragraph. Byte offsets into the
/// paragraph: `range` is replaced by `text`, then `selection` (in the paragraph after the edit)
/// is selected. It is one undo step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

/// Styles a text area's paragraphs. Any `Fn(&str, u32) -> StyledParagraph` is one.
pub trait TextStyler: 'static {
    /// Styles `paragraph` (without its line break), given the state the paragraph before it
    /// handed on.
    fn style(&self, paragraph: &str, state: u32) -> StyledParagraph;

    /// An edit to make for `key` instead of the area's own, given the caret's paragraph, the
    /// selection inside it (byte offsets) and the state it was styled with. `None` leaves the
    /// key to the area. Only asked while the selection sits inside one paragraph.
    fn edit(&self, key: EditKey, paragraph: &str, selection: Range<usize>, state: u32) -> Option<TextEdit> {
        let _ = (key, paragraph, selection, state);
        None
    }
}

impl<F: Fn(&str, u32) -> StyledParagraph + 'static> TextStyler for F {
    fn style(&self, paragraph: &str, state: u32) -> StyledParagraph {
        self(paragraph, state)
    }
}

/// A [`TextStyler`] as a text area's prop. Two stylers of the same type are equal, so a view
/// that builds its closure again each render does not restyle the document; switching to a
/// styler of another type restyles all of it.
#[derive(Clone)]
pub struct Styler {
    inner: Rc<dyn TextStyler>,
    kind: TypeId,
}

impl Styler {
    pub fn new<S: TextStyler>(styler: S) -> Self {
        Self { inner: Rc::new(styler), kind: TypeId::of::<S>() }
    }

    pub fn style(&self, paragraph: &str, state: u32) -> StyledParagraph {
        self.inner.style(paragraph, state)
    }

    pub fn edit(&self, key: EditKey, paragraph: &str, selection: Range<usize>, state: u32) -> Option<TextEdit> {
        self.inner.edit(key, paragraph, selection, state)
    }
}

impl PartialEq for Styler {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl std::fmt::Debug for Styler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Styler")
    }
}

/// One paragraph as the cache keeps it.
#[derive(Debug)]
pub struct Paragraph<L> {
    /// In the backend's units, without the line break.
    pub len: usize,
    /// The state the styler was given for it.
    pub(crate) state: u32,
    pub styled: StyledParagraph,
    /// What the backend made of it (a shaped layout, or that its attributes are applied);
    /// cleared each time the paragraph is restyled.
    pub layout: Option<L>,
}

impl<L> Paragraph<L> {
    /// The state the styler was given for it.
    pub fn state(&self) -> u32 {
        self.state
    }
}

/// Every paragraph of a styled text with its style, kept in step with edits.
#[derive(Debug)]
pub struct ParagraphCache<L> {
    paragraphs: Vec<Paragraph<L>>,
    /// Calls to the styler since the cache was made, for tests and the perf note.
    styled: u64,
}

impl<L> Default for ParagraphCache<L> {
    fn default() -> Self {
        Self { paragraphs: Vec::new(), styled: 0 }
    }
}

impl<L> ParagraphCache<L> {
    pub fn paragraphs(&self) -> &[Paragraph<L>] {
        &self.paragraphs
    }

    pub fn paragraphs_mut(&mut self) -> &mut [Paragraph<L>] {
        &mut self.paragraphs
    }

    /// How many paragraphs the styler has styled in all.
    pub fn styled_count(&self) -> u64 {
        self.styled
    }

    /// Forgets every paragraph; the next [`ParagraphCache::reset`] styles them all again.
    pub fn clear(&mut self) {
        self.paragraphs.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.paragraphs.is_empty()
    }

    /// Styles every paragraph of `text` afresh. `units` measures a paragraph.
    pub fn reset(&mut self, text: &str, styler: &Styler, units: impl Fn(&str) -> usize) {
        self.paragraphs.clear();
        let mut state = 0;
        for line in text.split('\n') {
            let styled = styler.style(line, state);
            self.styled += 1;
            self.paragraphs.push(Paragraph { len: units(line), state, styled: styled.clone(), layout: None });
            state = styled.state;
        }
    }

    /// The paragraph holding `offset` and where it starts. An offset on a line break belongs
    /// to the paragraph the break ends.
    pub fn locate(&self, offset: usize) -> (usize, usize) {
        let mut start = 0;
        for (index, p) in self.paragraphs.iter().enumerate() {
            if offset <= start + p.len {
                return (index, start);
            }
            start += p.len + 1;
        }
        let last = self.paragraphs.len().saturating_sub(1);
        (last, start.saturating_sub(self.paragraphs.last().map_or(0, |p| p.len + 1)))
    }

    /// Where paragraph `index` starts.
    pub fn start_of(&self, index: usize) -> usize {
        self.paragraphs[..index.min(self.paragraphs.len())].iter().map(|p| p.len + 1).sum()
    }

    /// An edit replaced `old` (in the text before it) with `new_len` units. `text` reads a
    /// range of the text after it. Restyles the paragraphs the edit touched, then the ones
    /// after for as long as the state handed on differs from what they were styled with, and
    /// returns the indices it restyled.
    pub fn edit<'a>(
        &mut self,
        old: Range<usize>,
        new_len: usize,
        styler: &Styler,
        units: impl Fn(&str) -> usize,
        mut text: impl FnMut(Range<usize>) -> Cow<'a, str>,
    ) -> Range<usize> {
        if self.paragraphs.is_empty() {
            let all = text(0..new_len);
            self.reset(&all, styler, units);
            return 0..self.paragraphs.len();
        }
        let (first, start) = self.locate(old.start);
        let (last, last_start) = self.locate(old.end);
        let old_end = last_start + self.paragraphs[last].len;
        let new_end = old_end + new_len - (old.end - old.start);
        let region = text(start..new_end);
        let mut state = self.paragraphs[first].state;
        let mut fresh = Vec::new();
        for line in region.split('\n') {
            let styled = styler.style(line, state);
            self.styled += 1;
            let next = styled.state;
            fresh.push(Paragraph { len: units(line), state, styled, layout: None });
            state = next;
        }
        let count = fresh.len();
        self.paragraphs.splice(first..=last, fresh);
        let mut index = first + count;
        let mut at = new_end + 1;
        while let Some(p) = self.paragraphs.get_mut(index) {
            if p.state == state {
                break;
            }
            let line = text(at..at + p.len);
            let styled = styler.style(&line, state);
            self.styled += 1;
            at += p.len + 1;
            p.state = state;
            state = styled.state;
            p.styled = styled;
            p.layout = None;
            index += 1;
        }
        first..index
    }
}

/// The byte range of `old` that changed to make `new`, and the length of its replacement:
/// what an app's write of the whole text amounts to as one edit.
pub fn changed_range(old: &str, new: &str) -> (Range<usize>, usize) {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let room = a.len().min(b.len()) - prefix;
    let mut suffix = a.iter().rev().zip(b.iter().rev()).take(room).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(a.len() - suffix) || !new.is_char_boundary(b.len() - suffix) {
        suffix -= 1;
    }
    (prefix..a.len() - suffix, b.len() - suffix - prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines between ``` fences are code (state 1); a line starting `#` is a heading.
    fn fenced(line: &str, state: u32) -> StyledParagraph {
        let fence = line.starts_with("```");
        let code = state == 1 || fence;
        let mut style = SpanStyle::default();
        style.mono = code;
        style.bold = !code && line.starts_with('#');
        StyledParagraph {
            runs: vec![(0..line.len(), style)],
            state: if fence { 1 - state } else { state },
            ..Default::default()
        }
    }

    fn cache(text: &str) -> (ParagraphCache<()>, Styler) {
        let styler = Styler::new(fenced);
        let mut cache = ParagraphCache::default();
        cache.reset(text, &styler, str::len);
        (cache, styler)
    }

    fn mono(cache: &ParagraphCache<()>) -> Vec<bool> {
        cache.paragraphs().iter().map(|p| p.styled.runs[0].1.mono).collect()
    }

    /// Applies `new` over `range` of `text` and the same edit to the cache.
    fn apply(
        cache: &mut ParagraphCache<()>,
        styler: &Styler,
        text: &mut String,
        range: Range<usize>,
        new: &str,
    ) -> Range<usize> {
        text.replace_range(range.clone(), new);
        let after = text.clone();
        cache.edit(range, new.len(), styler, str::len, |r| Cow::Owned(after[r].to_owned()))
    }

    #[test]
    fn a_keystroke_restyles_only_its_paragraph() {
        let mut text = String::from("# a\nb\nc\nd");
        let (mut cache, styler) = cache(&text);
        let before = cache.styled_count();
        let touched = apply(&mut cache, &styler, &mut text, 5..5, "x");
        assert_eq!(touched, 1..2);
        assert_eq!(cache.styled_count() - before, 1);
        assert_eq!(cache.paragraphs()[1].len, 2);
        assert_eq!(cache.start_of(2), 7);
    }

    #[test]
    fn opening_a_fence_restyles_until_the_state_settles() {
        let mut text = String::from("a\nb\n```\nc");
        let (mut cache, styler) = cache(&text);
        assert_eq!(mono(&cache), [false, false, true, true]);
        // A new fence before `b` turns `b` into code and the old fence into a closing one.
        let touched = apply(&mut cache, &styler, &mut text, 2..2, "```\n");
        assert_eq!(text, "a\n```\nb\n```\nc");
        assert_eq!(mono(&cache), [false, true, true, true, false]);
        assert_eq!(touched, 1..5);
    }

    #[test]
    fn joining_and_splitting_paragraphs_keeps_lengths() {
        let mut text = String::from("one\ntwo\nthree");
        let (mut cache, styler) = cache(&text);
        apply(&mut cache, &styler, &mut text, 3..4, "");
        assert_eq!(cache.paragraphs().iter().map(|p| p.len).collect::<Vec<_>>(), [6, 5]);
        apply(&mut cache, &styler, &mut text, 2..2, "\n\n");
        assert_eq!(cache.paragraphs().iter().map(|p| p.len).collect::<Vec<_>>(), [2, 0, 4, 5]);
        assert_eq!(cache.locate(3), (1, 3));
        assert_eq!(cache.locate(text.len()), (3, 9));
    }

    #[test]
    fn a_write_of_the_whole_text_is_the_change_inside_it() {
        assert_eq!(changed_range("hello world", "hello brave world"), (6..6, 6));
        assert_eq!(changed_range("abc", "abc"), (3..3, 0));
        assert_eq!(changed_range("aé", "aè"), (1..3, 2));
        assert_eq!(changed_range("aaa", "aa"), (2..3, 0));
    }
}
