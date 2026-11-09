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

/// How a run shows while the selection is outside its paragraph: live preview hides markup
/// there and shows it again where the caret is. The text keeps the run either way, so caret
/// movement, selection and copy see the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conceal {
    /// Not drawn and takes no room.
    Hide,
    /// A list bullet, [`MARK_WIDTH`] ems wide, drawn for the run's first character; the rest
    /// is hidden.
    Bullet,
    /// A task box, ticked or not, drawn like [`Conceal::Bullet`]. Clicking it asks the styler
    /// for [`EditKey::Toggle`] over the run.
    Task(bool),
}

/// The room a [`Conceal::Bullet`] or [`Conceal::Task`] mark takes, in ems.
pub const MARK_WIDTH: f32 = 1.5;

/// What a backend draws behind a paragraph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextBlock {
    /// A quote `depth` levels deep: a bar at the left of each level, each one em in.
    Quote(u8),
    /// Code: consecutive code paragraphs share one rounded well across the column.
    Code,
    /// A thematic break: a hairline across the column, while the paragraph is concealed.
    Rule,
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
    pub block: Option<TextBlock>,
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
    /// The size the drawn editor sets this paragraph of `len` bytes at, as a multiple of the
    /// area's: its largest run's, and at least the area's where some text has no run.
    pub fn scale(&self, len: usize) -> f32 {
        let largest = self.runs.iter().map(|(_, s)| s.scale()).fold(f32::MIN, f32::max);
        let mut end = 0;
        let gapless = self.runs.iter().all(|(r, _)| {
            let joined = r.start <= end;
            end = end.max(r.end);
            joined
        });
        if gapless && end >= len && !self.runs.is_empty() { largest } else { largest.max(1.) }
    }

    /// Whether any run hides or replaces its text while concealed.
    pub fn conceals(&self) -> bool {
        self.runs.iter().any(|(_, s)| s.conceal.is_some())
    }
}

/// A paragraph as live preview shows it while concealed: the text left once hidden runs are
/// gone and marks stand in for theirs, and where each piece came from. Offsets are bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConcealedText {
    pub text: String,
    pieces: Vec<Piece>,
    /// The source paragraph's length.
    len: usize,
}

/// A stretch of [`ConcealedText::text`] and the source it shows: the same bytes, or a mark
/// standing in for a run.
#[derive(Clone, Debug, PartialEq)]
struct Piece {
    shown: Range<usize>,
    source: Range<usize>,
    mark: Option<Conceal>,
}

impl ConcealedText {
    /// `paragraph` with its concealed runs taken out; each mark shows as `placeholder`, which
    /// the backend sizes to [`MARK_WIDTH`] ems and draws the mark over.
    pub fn new(paragraph: &str, styled: &StyledParagraph, placeholder: &str) -> Self {
        let mut out = Self { len: paragraph.len(), ..Self::default() };
        let mut at = 0;
        for (range, style) in &styled.runs {
            let Some(conceal) = style.conceal else { continue };
            let range = range.start.min(paragraph.len())..range.end.min(paragraph.len());
            if range.start < at || range.is_empty() {
                continue;
            }
            out.push(&paragraph[at..range.start], at..range.start, None);
            if conceal != Conceal::Hide {
                out.push(placeholder, range.clone(), Some(conceal));
            }
            at = range.end;
        }
        out.push(&paragraph[at..], at..paragraph.len(), None);
        out
    }

    fn push(&mut self, text: &str, source: Range<usize>, mark: Option<Conceal>) {
        if text.is_empty() {
            return;
        }
        let shown = self.text.len()..self.text.len() + text.len();
        self.text.push_str(text);
        self.pieces.push(Piece { shown, source, mark });
    }

    /// Every piece in order: its range in [`ConcealedText::text`], the source it shows, and the
    /// mark it is, if one.
    pub fn pieces(&self) -> impl Iterator<Item = (Range<usize>, Range<usize>, Option<Conceal>)> + '_ {
        self.pieces.iter().map(|p| (p.shown.clone(), p.source.clone(), p.mark))
    }

    /// Each mark's offset in [`ConcealedText::text`], its source run and what it is.
    pub fn marks(&self) -> impl Iterator<Item = (usize, Range<usize>, Conceal)> + '_ {
        self.pieces.iter().filter_map(|p| Some((p.shown.start, p.source.clone(), p.mark?)))
    }

    /// The source offset shown at `shown`: the start of a mark is its run's start, past it
    /// the run's end.
    pub fn source(&self, shown: usize) -> usize {
        let Some(p) = self.pieces.iter().find(|p| shown < p.shown.end) else {
            return self.pieces.last().map_or(self.len, |p| p.source.end);
        };
        match p.mark {
            _ if shown <= p.shown.start => p.source.start,
            Some(_) => p.source.end,
            None => p.source.start + (shown - p.shown.start),
        }
    }

    /// Where source offset `source` shows: a hidden offset shows where the text after it does.
    pub fn shown(&self, source: usize) -> usize {
        let Some(p) = self.pieces.iter().find(|p| source < p.source.end) else { return self.text.len() };
        match p.mark {
            _ if source <= p.source.start => p.shown.start,
            Some(_) => p.shown.start,
            None => p.shown.start + (source - p.source.start),
        }
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
    /// A click on a [`Conceal::Task`] mark; the selection is the mark's run.
    Toggle,
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
    fn a_paragraph_sets_at_its_largest_run_and_shrinks_only_when_runs_cover_it() {
        let run = |range: Range<usize>, scale: f32| (range, SpanStyle { scale: Some(scale), ..Default::default() });
        let small = StyledParagraph { runs: vec![run(0..4, 0.8)], ..Default::default() };
        assert_eq!((small.scale(4), small.scale(6)), (0.8, 1.), "unstyled text keeps the area's size");
        let mixed = StyledParagraph { runs: vec![run(0..2, 0.8), run(2..4, 1.5)], ..Default::default() };
        assert_eq!(mixed.scale(4), 1.5);
        assert_eq!(StyledParagraph::default().scale(0), 1.);
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
