//! A drawn text area's paragraphs: each shaped on its own, in its styled runs, and the shape
//! kept until the paragraph, its style or the column changes. Only paragraphs near the
//! viewport (and the caret's) are shaped at all; the rest count at their last measured height,
//! or an estimate, so a long document lays out in the time its visible part takes. When a
//! paragraph above the viewport's top edge changes height as it is shaped, the scroll offset
//! moves by the same amount, so the text in view stays put.

use std::borrow::Cow;
use std::ops::Range;

use nd_components::rich::text_color;
use nd_gpui::gpui::{
    App, Bounds, Font, FontStyle, FontWeight, Hsla, Pixels, Point, SharedString, StrikethroughStyle, TextAlign,
    TextRun, UnderlineStyle, Window, WrappedLine, point, px,
};
use nd_theme::Theme;
use nd_theme::color::with_alpha;
use nd_ui::styled_text::{ParagraphCache, Styler};
use nd_ui::{SpanStyle, StyledParagraph, TextColor, TextFill};

/// How far past the viewport paragraphs are shaped, so a scroll shows shaped text.
const OVERDRAW: Pixels = px(600.);

/// The theme colours a styled run can name.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Palette {
    primary: Hsla,
    secondary: Hsla,
    tertiary: Hsla,
    accent: Hsla,
    danger: Hsla,
    success: Hsla,
    warning: Hsla,
    subtle: Hsla,
    mono: SharedString,
}

impl Palette {
    pub fn new(theme: &Theme) -> Self {
        let c = |t| text_color(theme, t, None);
        Self {
            primary: c(TextColor::Primary),
            secondary: c(TextColor::Secondary),
            tertiary: c(TextColor::Tertiary),
            accent: c(TextColor::Accent),
            danger: c(TextColor::Danger),
            success: c(TextColor::Success),
            warning: c(TextColor::Warning),
            subtle: with_alpha(theme.text.primary, 0.07),
            mono: theme.fonts.mono.family.clone(),
        }
    }

    fn color(&self, color: TextColor) -> Hsla {
        match color {
            TextColor::Primary => self.primary,
            TextColor::Secondary => self.secondary,
            TextColor::Tertiary => self.tertiary,
            TextColor::Accent => self.accent,
            TextColor::Danger => self.danger,
            TextColor::Success => self.success,
            TextColor::Warning => self.warning,
            TextColor::Custom(c) => c,
        }
    }

    fn fill(&self, fill: TextFill) -> Hsla {
        match fill {
            TextFill::Subtle => self.subtle,
            TextFill::Custom(c) => c,
        }
    }
}

/// What a paragraph's shape depends on besides its text and style.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    wrap: Pixels,
    size: Pixels,
    font: Font,
    color: Hsla,
    palette: Palette,
    /// Composition and the redrawn selection, in the paragraph.
    marked: Option<Range<usize>>,
    recolor: Option<(Range<usize>, Hsla)>,
}

pub(crate) struct Shaped {
    line: WrappedLine,
    key: Key,
    height: Pixels,
}

/// One paragraph's place in the last layout, relative to the text origin.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Placed {
    pub start: usize,
    pub len: usize,
    pub top: Pixels,
    /// With the spacing before and after.
    pub height: Pixels,
    pub before: Pixels,
    /// Left edge of the text: the column's, plus the first-line indent.
    pub x: Pixels,
    pub line_height: Pixels,
    pub size: Pixels,
    /// Height of the text alone.
    pub text_height: Pixels,
}

/// What a layout pass sets text in.
pub(crate) struct Frame<'a> {
    /// The text column: its left edge and width, relative to the text origin.
    pub column: (Pixels, Pixels),
    pub size: Pixels,
    pub line_height: Pixels,
    pub font: Font,
    pub color: Hsla,
    pub palette: &'a Palette,
    pub viewport: Pixels,
    /// An offset whose paragraph is shaped wherever it is: the caret's.
    pub caret: usize,
    pub marked: Option<Range<usize>>,
    pub recolor: Option<(Range<usize>, Hsla)>,
}

pub(crate) struct Doc {
    cache: ParagraphCache<Shaped>,
    styler: Styler,
    placed: Vec<Placed>,
    pub content: Pixels,
}

fn plain(_: &str, _: u32) -> StyledParagraph {
    StyledParagraph::default()
}

impl Doc {
    pub fn new() -> Self {
        Self { cache: ParagraphCache::default(), styler: Styler::new(plain), placed: Vec::new(), content: px(0.) }
    }

    /// Takes the area's styler; a different one restyles everything at the next layout.
    pub fn set_styler(&mut self, styler: Option<&nd_ui::Styler>) {
        let styler = styler.cloned().unwrap_or_else(|| Styler::new(plain));
        if styler != self.styler {
            self.cache.clear();
        }
        self.styler = styler;
    }

    pub fn styler(&self) -> &Styler {
        &self.styler
    }

    /// Styles every paragraph of `text` unless they already are.
    fn ensure(&mut self, text: &str) {
        if self.cache.is_empty() {
            self.cache.reset(text, &self.styler, str::len);
        }
    }

    /// `old` (bytes of the text before) became `new_len` bytes of `text`.
    pub fn edit(&mut self, text: &str, old: Range<usize>, new_len: usize) {
        if self.cache.is_empty() {
            return;
        }
        self.cache.edit(old, new_len, &self.styler, str::len, |r| Cow::Borrowed(&text[r]));
    }

    /// How many paragraphs the styler has styled, for the perf test.
    #[cfg(test)]
    pub fn styled_count(&self) -> u64 {
        self.cache.styled_count()
    }

    /// The paragraph holding `offset`, its text and the state it was styled with.
    pub fn paragraph_at<'t>(&self, text: &'t str, offset: usize) -> Option<(usize, &'t str, u32)> {
        if self.cache.is_empty() {
            return None;
        }
        let (index, start) = self.cache.locate(offset);
        let p = &self.cache.paragraphs()[index];
        Some((start, &text[start..start + p.len], p.state()))
    }

    /// Lays out every paragraph, shaping the ones near `scroll` (which it may move to keep
    /// the text in view still).
    pub fn layout(&mut self, text: &str, f: &Frame<'_>, scroll: &mut Pixels, window: &mut Window) {
        self.ensure(text);
        self.placed.clear();
        let (left, width) = f.column;
        let mut top = px(0.);
        let mut start = 0;
        for p in self.cache.paragraphs_mut() {
            let len = p.len;
            let scale = p.styled.scale();
            let size = f.size * scale;
            let ps = &p.styled.paragraph;
            let line_height = match ps.line_height {
                Some(m) => size * m,
                None => f.line_height * scale,
            };
            let before = f.size * ps.space_before;
            let after = f.size * ps.space_after;
            let indent = f.size * ps.first_indent;
            let wrap = (width - indent).max(px(1.));
            let stale = p.layout.as_ref().map_or_else(
                || {
                    let per_line = (wrap / (size * 0.5)).max(1.);
                    line_height * (len as f32 / per_line).ceil().max(1.)
                },
                |s| s.height,
            );
            let lo = *scroll - OVERDRAW;
            let hi = *scroll + f.viewport + OVERDRAW;
            let wanted = (top < hi && top + before + stale + after > lo) || (start..=start + len).contains(&f.caret);
            let mut text_height = stale;
            if wanted {
                let local = |r: &Range<usize>| {
                    (r.start <= start + len && r.end >= start)
                        .then(|| r.start.max(start) - start..r.end.min(start + len) - start)
                };
                let key = Key {
                    wrap,
                    size,
                    font: f.font.clone(),
                    color: f.color,
                    palette: f.palette.clone(),
                    marked: f.marked.as_ref().and_then(local),
                    recolor: f.recolor.as_ref().and_then(|(r, c)| Some((local(r)?, *c))),
                };
                if p.layout.as_ref().is_none_or(|s| s.key != key) {
                    let shaped = shape(&text[start..start + len], &p.styled, key, line_height, window);
                    if top < *scroll {
                        *scroll += shaped.height - stale;
                    }
                    p.layout = Some(shaped);
                }
                text_height = p.layout.as_ref().map_or(stale, |s| s.height);
            }
            let height = before + text_height + after;
            self.placed.push(Placed {
                start,
                len,
                top,
                height,
                before,
                x: left + indent,
                line_height,
                size,
                text_height,
            });
            top += height;
            start += len + 1;
        }
        self.content = top;
    }

    fn index_of(&self, offset: usize) -> usize {
        self.placed.partition_point(|p| p.start + p.len < offset).min(self.placed.len().saturating_sub(1))
    }

    fn line(&self, index: usize) -> Option<&WrappedLine> {
        self.cache.paragraphs().get(index)?.layout.as_ref().map(|s| &s.line)
    }

    /// Where `offset` sits relative to the text origin (top of its line), its line height and
    /// font size.
    pub fn caret(&self, offset: usize) -> Option<(Point<Pixels>, Pixels, Pixels)> {
        let index = self.index_of(offset);
        let p = self.placed.get(index)?;
        let local = self
            .line(index)
            .and_then(|l| l.position_for_index(offset.saturating_sub(p.start).min(p.len), p.line_height))
            .unwrap_or_default();
        Some((point(p.x + local.x, p.top + p.before + local.y), p.line_height, p.size))
    }

    /// The offset nearest `at`, relative to the text origin.
    pub fn offset_at(&self, at: Point<Pixels>) -> usize {
        let index = self.placed.partition_point(|p| p.top + p.height <= at.y).min(self.placed.len().saturating_sub(1));
        let Some(p) = self.placed.get(index) else { return 0 };
        let Some(line) = self.line(index) else { return p.start };
        let y = (at.y - p.top - p.before).clamp(px(0.), (p.text_height - px(1.)).max(px(0.)));
        let x = (at.x - p.x).max(px(0.));
        p.start + line.closest_index_for_position(point(x, y), p.line_height).unwrap_or_else(|i| i).min(p.len)
    }

    /// One line up (`-1`) or down (`1`) from `offset`, keeping its x. `None` past either end.
    pub fn vertical(&self, offset: usize, direction: i32) -> Option<usize> {
        let index = self.index_of(offset);
        let p = self.placed.get(index)?;
        let (at, line_height, _) = self.caret(offset)?;
        let local = at.y - p.top - p.before;
        let y = if direction > 0 {
            if local + line_height < p.text_height - px(0.5) {
                at.y + line_height * 1.5
            } else {
                let next = self.placed.get(index + 1)?;
                next.top + next.before + next.line_height / 2.
            }
        } else if local > px(0.5) {
            at.y - line_height / 2.
        } else {
            let previous = self.placed.get(index.checked_sub(1)?)?;
            previous.top + previous.before + previous.text_height - previous.line_height / 2.
        };
        Some(self.offset_at(point(at.x, y)))
    }

    /// The selection's rects relative to the text origin, for paragraphs between `view`'s
    /// edges; lines it runs past end at `right`.
    pub fn selection(&self, range: Range<usize>, right: Pixels, view: Range<Pixels>) -> Vec<Bounds<Pixels>> {
        let mut out = Vec::new();
        if range.is_empty() || self.placed.is_empty() {
            return out;
        }
        for index in self.index_of(range.start)..=self.index_of(range.end) {
            let p = self.placed[index];
            if p.top + p.height < view.start || p.top > view.end {
                continue;
            }
            let a = range.start.max(p.start) - p.start;
            let b = range.end.min(p.start + p.len) - p.start;
            let past = range.end > p.start + p.len;
            let lh = p.line_height;
            let y0 = p.top + p.before;
            let pos = |i: usize| self.line(index).and_then(|l| l.position_for_index(i, lh)).unwrap_or_default();
            let (pa, pb) = (pos(a), pos(b));
            let end_x = |pb: Point<Pixels>| if past { right } else { p.x + pb.x };
            if pa.y == pb.y {
                out.push(Bounds::from_corners(point(p.x + pa.x, y0 + pa.y), point(end_x(pb), y0 + pa.y + lh)));
                continue;
            }
            out.push(Bounds::from_corners(point(p.x + pa.x, y0 + pa.y), point(right, y0 + pa.y + lh)));
            if pb.y > pa.y + lh {
                out.push(Bounds::from_corners(point(p.x, y0 + pa.y + lh), point(right, y0 + pb.y)));
            }
            out.push(Bounds::from_corners(point(p.x, y0 + pb.y), point(end_x(pb), y0 + pb.y + lh)));
        }
        out
    }

    fn visible(&self, view: &Range<Pixels>) -> impl Iterator<Item = (Placed, &WrappedLine)> {
        let first = self.placed.partition_point(|p| p.top + p.height <= view.start);
        let end = view.end;
        self.placed[first..]
            .iter()
            .enumerate()
            .take_while(move |(_, p)| p.top < end)
            .filter_map(move |(i, p)| Some((*p, self.line(first + i)?)))
    }

    /// Run backgrounds (a code span's well) of the paragraphs in `view`.
    pub fn paint_backgrounds(&self, origin: Point<Pixels>, view: Range<Pixels>, window: &mut Window, cx: &mut App) {
        for (p, line) in self.visible(&view) {
            let at = origin + point(p.x, p.top + p.before);
            let _ = line.paint_background(at, p.line_height, TextAlign::Left, None, window, cx);
        }
    }

    pub fn paint_text(&self, origin: Point<Pixels>, view: Range<Pixels>, window: &mut Window, cx: &mut App) {
        for (p, line) in self.visible(&view) {
            let at = origin + point(p.x, p.top + p.before);
            let _ = line.paint(at, p.line_height, TextAlign::Left, None, window, cx);
        }
    }
}

fn shape(text: &str, styled: &StyledParagraph, key: Key, line_height: Pixels, window: &mut Window) -> Shaped {
    let runs = runs(text.len(), styled, &key);
    let line = window
        .text_system()
        .shape_text(SharedString::from(text.to_owned()), key.size, &runs, Some(key.wrap), None)
        .ok()
        .and_then(|lines| lines.into_iter().next())
        .unwrap_or_default();
    let height = line.size(line_height).height.max(line_height);
    Shaped { line, key, height }
}

/// The paragraph's runs as text runs covering all `len` bytes, with composition underlined
/// and a redrawn selection recoloured.
fn runs(len: usize, styled: &StyledParagraph, key: &Key) -> Vec<TextRun> {
    let plain = SpanStyle::default();
    let mut spans: Vec<(Range<usize>, &SpanStyle)> = Vec::new();
    let mut at = 0;
    for (range, style) in &styled.runs {
        let range = range.start.min(len)..range.end.min(len);
        if range.start > at {
            spans.push((at..range.start, &plain));
        }
        if range.end > range.start.max(at) {
            spans.push((range.start.max(at)..range.end, style));
            at = range.end;
        }
    }
    if at < len {
        spans.push((at..len, &plain));
    }
    let mut cuts: Vec<usize> = spans.iter().map(|(r, _)| r.start).collect();
    for r in key.marked.iter().chain(key.recolor.as_ref().map(|(r, _)| r)) {
        cuts.extend([r.start, r.end]);
    }
    cuts.push(len);
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    let mut span = 0;
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a >= b || a >= len {
            continue;
        }
        while span + 1 < spans.len() && spans[span].0.end <= a {
            span += 1;
        }
        let style = spans.get(span).map_or(&plain, |(_, s)| *s);
        let marked = key.marked.as_ref().is_some_and(|m| m.start <= a && b <= m.end);
        let recolor = key.recolor.as_ref().filter(|(r, _)| r.start <= a && b <= r.end).map(|(_, c)| *c);
        out.push(text_run(b - a, style, key, marked, recolor));
    }
    out
}

fn text_run(len: usize, style: &SpanStyle, key: &Key, marked: bool, recolor: Option<Hsla>) -> TextRun {
    let mut font = key.font.clone();
    if style.mono {
        font.family = key.palette.mono.clone();
    }
    font.weight = FontWeight(style.weight(font.weight.0));
    if style.italic {
        font.style = FontStyle::Italic;
    }
    let color = recolor.unwrap_or_else(|| style.color().map_or(key.color, |c| key.palette.color(c)));
    TextRun {
        len,
        font,
        color,
        background_color: style.background.map(|f| key.palette.fill(f)),
        underline: (style.underlined() || marked).then_some(UnderlineStyle {
            color: Some(color),
            thickness: px(1.),
            wavy: false,
        }),
        strikethrough: style.strikethrough.then_some(StrikethroughStyle { thickness: px(1.), color: Some(color) }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_in_a_million_characters_restyles_and_reshapes_one_paragraph() {
        let chapter = "# Heading\n\nBody with **bold** and `code`.\n- [ ] task\n```\ncode\n```\n";
        let mut text = chapter.repeat(1_000_000 / chapter.len() + 1);
        let mut doc = Doc::new();
        doc.set_styler(Some(&Styler::new(nd_components::Markdown)));
        doc.ensure(&text);
        // Every paragraph counts as shaped, as after a full layout.
        for p in doc.cache.paragraphs_mut() {
            p.layout = Some(Shaped { line: WrappedLine::default(), key: key(), height: px(20.) });
        }
        let at = text.len() / 2 + text[text.len() / 2..].find("Body").unwrap_or(0) + 4;
        let before = doc.styled_count();
        text.insert(at, 'x');
        doc.edit(&text, at..at, 1);
        assert_eq!(doc.styled_count() - before, 1);
        let unshaped = doc.cache.paragraphs().iter().filter(|p| p.layout.is_none()).count();
        assert_eq!(unshaped, 1, "only the edited paragraph is shaped again");
        let (start, paragraph, _) = doc.paragraph_at(&text, at).unwrap_or((0, "", 0));
        assert!(paragraph.starts_with("Bodyx") && start < at);
    }

    #[test]
    fn the_same_styler_type_keeps_the_cache_and_another_clears_it() {
        let mut doc = Doc::new();
        doc.set_styler(Some(&Styler::new(nd_components::Markdown)));
        doc.ensure("# a\nb");
        doc.set_styler(Some(&Styler::new(nd_components::Markdown)));
        assert!(!doc.cache.is_empty());
        doc.set_styler(None);
        assert!(doc.cache.is_empty());
    }

    fn key() -> Key {
        Key {
            wrap: px(100.),
            size: px(15.),
            font: Font::default(),
            color: Hsla::default(),
            palette: Palette {
                primary: Hsla::default(),
                secondary: Hsla::default(),
                tertiary: Hsla::default(),
                accent: Hsla::default(),
                danger: Hsla::default(),
                success: Hsla::default(),
                warning: Hsla::default(),
                subtle: Hsla::default(),
                mono: SharedString::default(),
            },
            marked: None,
            recolor: None,
        }
    }

    #[test]
    fn runs_cover_the_paragraph_and_overlay_composition() {
        let styled = StyledParagraph {
            runs: vec![(2..4, SpanStyle { bold: true, ..SpanStyle::default() })],
            ..StyledParagraph::default()
        };
        let mut k = key();
        k.marked = Some(3..6);
        let runs = runs(8, &styled, &k);
        let lens: Vec<usize> = runs.iter().map(|r| r.len).collect();
        assert_eq!(lens, [2, 1, 1, 2, 2]);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 8);
        assert!(runs[2].underline.is_some() && runs[2].font.weight == FontWeight(700.));
        assert!(runs[0].underline.is_none());
    }
}
