//! Live-preview markdown in a text area: `markdown_editor(id, &state)`, or
//! `text_area(id).styles(Markdown)`. The syntax stays in the text, dimmed to the tertiary text
//! colour, and headings, emphasis, code, quotes, lists, tasks, links and rules take their look
//! around it. Return continues a list, task or quote and clears an empty one, Tab and Shift-Tab
//! nest list items, and Command-B and Command-I wrap the selection.
//!
//! The scanner reads one line at a time, so an edit restyles its own paragraph. A code fence is
//! the one construct that reaches past its line; it rides in the state handed to the next
//! paragraph. Setext headings (a line underlined with `===`) read as text and a rule.

use std::ops::Range;

use nd_ui::prelude::*;
use nd_ui::{EditKey, ParagraphStyle, SpanStyle, StyledParagraph, TextEdit, TextFill, TextStyler};

use crate::forms::{TextArea, TextFieldState, text_area};

/// The markdown styler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Markdown;

/// Body line height, as a multiple of the font size.
pub const BODY_LEADING: f32 = 1.6;
/// The column a markdown editor keeps its text to, in points.
pub const MEASURE: f32 = 820.;
/// The body font size a markdown editor starts at, in points.
pub const BODY_SIZE: f32 = 15.;

/// Heading sizes as a multiple of the body's, H1 to H6.
const HEADING_SCALE: [f32; 6] = [1.75, 1.4, 1.2, 1.05, 1., 1.];

/// Inside a fence opened with backticks, or with tildes.
const IN_BACKTICKS: u32 = 1;
const IN_TILDES: u32 = 2;

/// A text area bound to `state` that styles its text as markdown: a centred column at most
/// [`MEASURE`] points wide, body text at [`BODY_SIZE`] on a [`BODY_LEADING`] line.
pub fn markdown_editor(id: impl Into<SharedString>, state: &Entity<TextFieldState>) -> TextArea {
    text_area(id).bind(state).styles(Markdown).max_measure(MEASURE).text_size(px(BODY_SIZE))
}

impl TextStyler for Markdown {
    fn style(&self, line: &str, state: u32) -> StyledParagraph {
        style_line(line, state)
    }

    fn edit(&self, key: EditKey, line: &str, selection: Range<usize>, state: u32) -> Option<TextEdit> {
        if state != 0 {
            return None;
        }
        match key {
            EditKey::Return => continue_line(line, selection),
            EditKey::Tab => indent(line, selection),
            EditKey::Backtab => outdent(line, selection),
            EditKey::Bold => Some(wrap(line, selection, 2)),
            EditKey::Italic => Some(wrap(line, selection, 1)),
        }
    }
}

// Per-byte marks, turned into runs at the end.
const DIM: u16 = 1;
const BOLD: u16 = 1 << 1;
const ITALIC: u16 = 1 << 2;
const STRIKE: u16 = 1 << 3;
const MONO: u16 = 1 << 4;
const FILL: u16 = 1 << 5;
const LINK: u16 = 1 << 6;
/// Taken by a code span, a link target or an escape: emphasis does not look inside.
const OPAQUE: u16 = 1 << 7;
const MARKER: u16 = 1 << 8;
const CHECK: u16 = 1 << 9;

struct Marks {
    flags: Vec<u16>,
}

impl Marks {
    fn new(len: usize) -> Self {
        Self { flags: vec![0; len] }
    }

    fn set(&mut self, range: Range<usize>, flag: u16) {
        for f in &mut self.flags[range] {
            *f |= flag;
        }
    }

    fn opaque(&self, at: usize) -> bool {
        self.flags[at] & OPAQUE != 0
    }
}

/// What a line opens with: indent, quote markers, a list marker and a task box.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Prefix {
    /// Quote markers (`> > `) after the indent.
    quote: Range<usize>,
    /// The list marker with its space (`- `, `12. `), after any quote.
    marker: Range<usize>,
    /// The task box with its space (`[ ] `), after the marker, and whether it is ticked.
    task: Option<(Range<usize>, bool)>,
    /// Where the line's own text starts.
    end: usize,
}

impl Prefix {
    fn read(line: &str) -> Self {
        let b = line.as_bytes();
        let mut at = b.iter().take_while(|c| **c == b' ' || **c == b'\t').count();
        let quote_start = at;
        while b.get(at) == Some(&b'>') {
            at += 1;
            if b.get(at) == Some(&b' ') {
                at += 1;
            }
            while b.get(at) == Some(&b' ') && b.get(at + 1) == Some(&b'>') {
                at += 1;
            }
        }
        let quote = quote_start..at;
        if !quote.is_empty() {
            at += b[at..].iter().take_while(|c| **c == b' ').count();
        }
        let marker_start = at;
        let marker_end = match b.get(at) {
            Some(b'-' | b'*' | b'+') if matches!(b.get(at + 1), Some(b' ') | None) && !is_rule(&line[at..]) => {
                Some((at + 2).min(b.len()))
            }
            Some(c) if c.is_ascii_digit() => {
                let digits = b[at..].iter().take_while(|c| c.is_ascii_digit()).count();
                let delim = at + digits;
                (digits <= 9
                    && matches!(b.get(delim), Some(b'.' | b')'))
                    && matches!(b.get(delim + 1), Some(b' ') | None))
                .then(|| (delim + 2).min(b.len()))
            }
            _ => None,
        };
        let Some(marker_end) = marker_end else {
            return Self { end: quote.end, quote, marker: marker_start..marker_start, task: None };
        };
        let marker = marker_start..marker_end;
        let bullet = matches!(b[marker_start], b'-' | b'*' | b'+');
        let task = (bullet && b.get(marker_end) == Some(&b'[') && b.get(marker_end + 2) == Some(&b']'))
            .then(|| match b.get(marker_end + 1) {
                Some(b' ') => Some(false),
                Some(b'x' | b'X') => Some(true),
                _ => None,
            })
            .flatten()
            .filter(|_| matches!(b.get(marker_end + 3), Some(b' ') | None))
            .map(|checked| (marker_end..(marker_end + 4).min(b.len()), checked));
        let end = task.as_ref().map_or(marker_end, |(r, _)| r.end);
        Self { quote, marker, task, end }
    }

    /// The prefix a new line after this one opens with: the next number, an open task box.
    fn continuation(&self, line: &str) -> String {
        let mut out = String::new();
        out.push_str(&line[..self.quote.start]);
        if !self.quote.is_empty() {
            let depth = line[self.quote.clone()].matches('>').count();
            out.push_str(&"> ".repeat(depth));
        }
        out.push_str(&line[self.quote.end..self.marker.start]);
        let marker = line[self.marker.clone()].trim_end();
        match marker.strip_suffix(['.', ')']) {
            Some(number) => {
                let next = number.parse::<u64>().map_or(1, |n| n + 1);
                out.push_str(&format!("{next}{} ", &marker[number.len()..]));
            }
            None if !marker.is_empty() => {
                out.push_str(marker);
                out.push(' ');
            }
            None => {}
        }
        if self.task.is_some() {
            out.push_str("[ ] ");
        }
        out
    }
}

/// `---`, `***` or `___`, three or more, spaces allowed between.
fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(first) = trimmed.chars().next().filter(|c| matches!(c, '-' | '*' | '_')) else { return false };
    trimmed.chars().all(|c| c == first || c == ' ') && trimmed.chars().filter(|c| *c == first).count() >= 3
}

/// The fence a line opens or closes, if it is one: its character.
fn fence(line: &str) -> Option<u8> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    [b'`', b'~'].into_iter().find(|c| trimmed.len() >= 3 && trimmed.as_bytes()[..3].iter().all(|b| b == c))
}

/// About how wide `text` sets in the system font, in ems: where a list item's wrapped lines
/// line up.
fn ems(text: &str) -> f32 {
    text.chars()
        .map(|c| match c {
            ' ' | '.' => 0.28,
            '\t' => 1.12,
            '-' | '[' | ']' => 0.36,
            ')' => 0.33,
            '*' => 0.45,
            '0'..='9' | '+' | '>' => 0.6,
            _ => 0.55,
        })
        .sum()
}

fn style_line(line: &str, state: u32) -> StyledParagraph {
    let body = ParagraphStyle { line_height: Some(BODY_LEADING), ..ParagraphStyle::default() };
    let len = line.len();
    let mut marks = Marks::new(len);
    let fenced = fence(line);
    if state != 0 || fenced.is_some() {
        // Inside a fence, or a fence line: monospaced on the code well.
        let closes = match (state, fenced) {
            (IN_BACKTICKS, Some(b'`')) | (IN_TILDES, Some(b'~')) => true,
            (0, _) => false,
            _ => {
                marks.set(0..len, MONO | FILL);
                return finish(line, marks, Line::default(), code_paragraph(), state);
            }
        };
        marks.set(0..len, MONO | FILL | DIM);
        let next = match (closes, fenced) {
            (true, _) => 0,
            (false, Some(b'~')) => IN_TILDES,
            _ => IN_BACKTICKS,
        };
        return finish(line, marks, Line::default(), code_paragraph(), next);
    }
    if is_rule(line) {
        marks.set(0..len, DIM | STRIKE);
        let paragraph = ParagraphStyle { space_before: 0.3, space_after: 0.3, ..body };
        return finish(line, marks, Line::default(), paragraph, 0);
    }
    if let Some(level) = heading(line) {
        let hashes = line.find('#').unwrap_or(0) + level;
        let marker_end = (hashes + 1).min(len);
        marks.set(0..marker_end, DIM);
        inline(line, marker_end, &mut marks);
        let look = Line { bold: true, scale: HEADING_SCALE[level - 1], ..Line::default() };
        let paragraph = ParagraphStyle {
            line_height: Some(1.3),
            space_before: if level <= 2 { 0.6 } else { 0.4 },
            space_after: 0.2,
            ..ParagraphStyle::default()
        };
        return finish(line, marks, look, paragraph, 0);
    }
    let prefix = Prefix::read(line);
    let mut look = Line::default();
    let mut paragraph = body;
    if !prefix.quote.is_empty() {
        marks.set(prefix.quote.clone(), DIM);
        look.secondary = true;
        let depth = line[prefix.quote.clone()].matches('>').count() as f32;
        paragraph.first_indent = 0.8 * depth;
        paragraph.head_indent = paragraph.first_indent + ems(&line[prefix.quote.clone()]);
    }
    if !prefix.marker.is_empty() {
        marks.set(prefix.marker.clone(), MARKER);
        paragraph.head_indent = paragraph.first_indent + ems(&line[prefix.quote.start..prefix.end]);
        if let Some((task, checked)) = &prefix.task {
            marks.set(task.clone(), CHECK);
            if *checked {
                look.done = prefix.end;
            }
        }
    }
    inline(line, prefix.end, &mut marks);
    finish(line, marks, look, paragraph, 0)
}

fn code_paragraph() -> ParagraphStyle {
    ParagraphStyle { line_height: Some(1.45), ..ParagraphStyle::default() }
}

/// `#` to `######` and a space (or nothing) after up to three spaces: the level.
fn heading(line: &str) -> Option<usize> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let level = trimmed.bytes().take_while(|b| *b == b'#').count();
    ((1..=6).contains(&level) && matches!(trimmed.as_bytes().get(level), Some(b' ') | None)).then_some(level)
}

/// What a whole line adds to every run in it.
#[derive(Clone, Copy, Default)]
struct Line {
    bold: bool,
    scale: f32,
    /// A quote: secondary text.
    secondary: bool,
    /// A ticked task: its text from here on is dimmed and struck through.
    done: usize,
}

fn finish(line: &str, marks: Marks, look: Line, paragraph: ParagraphStyle, state: u32) -> StyledParagraph {
    let mut runs: Vec<(Range<usize>, SpanStyle)> = Vec::new();
    let mut start = 0;
    let mut current = run_style(marks.flags.first().copied().unwrap_or(0), 0, &look);
    for at in 1..=line.len() {
        let next = (at < line.len()).then(|| run_style(marks.flags[at], at, &look));
        if next.as_ref() == Some(&current) {
            continue;
        }
        if current != SpanStyle::default() && start < at {
            match runs.last_mut() {
                Some((range, style)) if range.end == start && *style == current => range.end = at,
                _ => runs.push((start..at, current.clone())),
            }
        }
        if let Some(next) = next {
            current = next;
            start = at;
        }
    }
    StyledParagraph { runs, paragraph, state }
}

fn run_style(flags: u16, at: usize, look: &Line) -> SpanStyle {
    let done = look.done > 0 && at >= look.done;
    let color = if flags & DIM != 0 {
        Some(TextColor::Tertiary)
    } else if flags & CHECK != 0 || flags & LINK != 0 {
        Some(TextColor::Accent)
    } else if flags & MARKER != 0 || look.secondary {
        Some(TextColor::Secondary)
    } else if done {
        Some(TextColor::Tertiary)
    } else {
        None
    };
    SpanStyle {
        bold: look.bold || flags & BOLD != 0,
        italic: flags & ITALIC != 0,
        mono: flags & MONO != 0,
        strikethrough: flags & STRIKE != 0 || (done && flags & DIM == 0),
        color,
        scale: (look.scale > 0. && look.scale != 1.).then_some(look.scale),
        background: (flags & FILL != 0).then_some(TextFill::Subtle),
        ..SpanStyle::default()
    }
}

/// Marks the inline syntax of `line` from `from` on: escapes, code spans, links, URLs, then
/// emphasis and strikethrough outside them.
fn inline(line: &str, from: usize, marks: &mut Marks) {
    let b = line.as_bytes();
    let mut at = from;
    while at < b.len() {
        match b[at] {
            b'\\' if b.get(at + 1).is_some_and(u8::is_ascii_punctuation) => {
                marks.set(at..at + 1, DIM | OPAQUE);
                marks.set(at + 1..at + 2, OPAQUE);
                at += 2;
            }
            b'`' => {
                let ticks = b[at..].iter().take_while(|c| **c == b'`').count();
                let close = find_ticks(b, at + ticks, ticks);
                match close {
                    Some(close) => {
                        marks.set(at..at + ticks, DIM | OPAQUE | MONO | FILL);
                        marks.set(at + ticks..close, OPAQUE | MONO | FILL);
                        marks.set(close..close + ticks, DIM | OPAQUE | MONO | FILL);
                        at = close + ticks;
                    }
                    None => at += ticks,
                }
            }
            _ => at += 1,
        }
    }
    links(line, from, marks);
    emphasis(line, from, marks);
}

/// The next run of exactly `ticks` backticks from `from`.
fn find_ticks(b: &[u8], from: usize, ticks: usize) -> Option<usize> {
    let mut at = from;
    while at < b.len() {
        if b[at] == b'`' {
            let run = b[at..].iter().take_while(|c| **c == b'`').count();
            if run == ticks {
                return Some(at);
            }
            at += run;
        } else {
            at += 1;
        }
    }
    None
}

/// `[text](target)`, `<https://..>` and bare `http(s)://` URLs.
fn links(line: &str, from: usize, marks: &mut Marks) {
    let b = line.as_bytes();
    let mut at = from;
    while at < b.len() {
        if marks.opaque(at) {
            at += 1;
            continue;
        }
        if b[at] == b'[' {
            let close = (at + 1..b.len()).find(|i| b[*i] == b']' && !marks.opaque(*i));
            if let Some(close) = close
                && b.get(close + 1) == Some(&b'(')
                && let Some(end) = (close + 2..b.len()).find(|i| b[*i] == b')')
            {
                marks.set(at..at + 1, DIM);
                marks.set(at + 1..close, LINK);
                marks.set(close..end + 1, DIM | OPAQUE);
                at = end + 1;
                continue;
            }
        }
        if b[at] == b'<'
            && line[at + 1..].starts_with("http")
            && let Some(end) = line[at..].find('>').map(|e| at + e)
            && !line[at..end].contains(' ')
        {
            marks.set(at..at + 1, DIM | OPAQUE);
            marks.set(at + 1..end, LINK | OPAQUE);
            marks.set(end..end + 1, DIM | OPAQUE);
            at = end + 1;
            continue;
        }
        let starts_word = at == 0 || !b[at - 1].is_ascii_alphanumeric();
        if starts_word && (line[at..].starts_with("https://") || line[at..].starts_with("http://")) {
            let mut end = line[at..].find(char::is_whitespace).map_or(b.len(), |e| at + e);
            // Punctuation closing a sentence is not part of the URL.
            while end > at && matches!(b[end - 1], b'.' | b',' | b';' | b':' | b'!' | b'?' | b')') {
                end -= 1;
            }
            marks.set(at..end, LINK | OPAQUE);
            at = end.max(at + 1);
            continue;
        }
        at += 1;
    }
}

/// A run of `*`, `_` or `~` that may open or close emphasis.
struct Delim {
    ch: u8,
    start: usize,
    len: usize,
    open: bool,
    close: bool,
}

/// CommonMark's delimiter runs, cut down: `*` and `_` pair into emphasis (one) and strong
/// (two), `~~` into strikethrough; `_` needs a word boundary outside.
fn emphasis(line: &str, from: usize, marks: &mut Marks) {
    let b = line.as_bytes();
    let class = |i: Option<usize>| -> u8 {
        // 0 whitespace or edge, 1 punctuation, 2 other.
        match i.and_then(|i| line[i..].chars().next()) {
            None => 0,
            Some(c) if c.is_whitespace() => 0,
            Some(c) if c.is_ascii_punctuation() => 1,
            Some(_) => 2,
        }
    };
    let mut delims: Vec<Delim> = Vec::new();
    let mut at = from;
    while at < b.len() {
        let ch = b[at];
        if !matches!(ch, b'*' | b'_' | b'~') || marks.opaque(at) {
            at += 1;
            continue;
        }
        let len = b[at..].iter().take_while(|c| **c == ch).count();
        let before = class(line[..at].char_indices().next_back().map(|(i, _)| i));
        let after = class((at + len < b.len()).then_some(at + len));
        let left = after != 0 && (after == 2 || before != 2);
        let right = before != 0 && (before == 2 || after != 2);
        let (open, close) = match ch {
            b'_' => (left && (!right || before == 1), right && (!left || after == 1)),
            b'~' if len != 2 => (false, false),
            _ => (left, right),
        };
        if open || close {
            delims.push(Delim { ch, start: at, len, open, close });
        }
        at += len;
    }
    let mut closer = 0;
    while closer < delims.len() {
        if !delims[closer].close || delims[closer].len == 0 {
            closer += 1;
            continue;
        }
        let ch = delims[closer].ch;
        let Some(opener) = (0..closer).rev().find(|i| delims[*i].open && delims[*i].ch == ch && delims[*i].len > 0)
        else {
            closer += 1;
            continue;
        };
        let used = if ch == b'~' || (delims[opener].len >= 2 && delims[closer].len >= 2) { 2 } else { 1 };
        let open_end = delims[opener].start + delims[opener].len;
        let close_start = delims[closer].start;
        let flag = match (ch, used) {
            (b'~', _) => STRIKE,
            (_, 2) => BOLD,
            _ => ITALIC,
        };
        marks.set(open_end - used..open_end, DIM);
        marks.set(open_end..close_start, flag);
        marks.set(close_start..close_start + used, DIM);
        delims[opener].len -= used;
        delims[closer].start += used;
        delims[closer].len -= used;
        // Delimiters between the pair can no longer pair across it.
        for d in &mut delims[opener + 1..closer] {
            d.len = 0;
        }
    }
}

/// Return: a list, task or quote line goes on with its marker; an empty one loses it.
fn continue_line(line: &str, selection: Range<usize>) -> Option<TextEdit> {
    let prefix = Prefix::read(line);
    if (prefix.quote.is_empty() && prefix.marker.is_empty()) || selection.start < prefix.end {
        return None;
    }
    if line[prefix.end..].trim().is_empty() {
        // An empty nested item lifts a level; at the top an empty item ends the list and an
        // empty quote line ends the quote.
        let indent = if prefix.quote.is_empty() { prefix.marker.start } else { prefix.marker.start - prefix.quote.end };
        if !prefix.marker.is_empty() && indent > 0 {
            return outdent(line, selection);
        }
        let keep = if prefix.marker.is_empty() { String::new() } else { line[..prefix.quote.end].to_owned() };
        let caret = keep.len();
        return Some(TextEdit { range: 0..line.len(), text: keep, selection: caret..caret });
    }
    let text = format!("\n{}", prefix.continuation(line));
    let caret = selection.start + text.len();
    Some(TextEdit { range: selection, text, selection: caret..caret })
}

/// The spaces one nesting level takes: the marker's width, so a child lines up with its
/// parent's text.
fn nesting(line: &str, prefix: &Prefix) -> usize {
    line[prefix.marker.clone()].trim_end().len() + 1
}

fn indent(line: &str, selection: Range<usize>) -> Option<TextEdit> {
    let prefix = Prefix::read(line);
    if prefix.marker.is_empty() {
        return None;
    }
    let at = prefix.marker.start;
    let text = " ".repeat(nesting(line, &prefix));
    let shift = |o: usize| if o >= at { o + text.len() } else { o };
    let selection = shift(selection.start)..shift(selection.end);
    Some(TextEdit { range: at..at, text, selection })
}

fn outdent(line: &str, selection: Range<usize>) -> Option<TextEdit> {
    let prefix = Prefix::read(line);
    if prefix.marker.is_empty() {
        return None;
    }
    let at = prefix.marker.start;
    let space = if prefix.quote.is_empty() { &line[..at] } else { &line[prefix.quote.end..at] };
    let removed = if space.ends_with('\t') {
        1
    } else {
        space.bytes().rev().take_while(|c| *c == b' ').count().min(nesting(line, &prefix))
    };
    let from = at - removed;
    let shift = |o: usize| if o >= at { o - removed } else { o.min(from) };
    let selection = shift(selection.start)..shift(selection.end);
    // Nothing to take away still takes the key, so Shift-Tab does not leave the editor.
    Some(TextEdit { range: from..at, text: String::new(), selection })
}

/// Command-B (`**`, `width` 2) or Command-I (`*`, 1): wraps the selection, or unwraps it when
/// the markers already sit round it; with nothing selected, the caret lands between a new pair.
fn wrap(line: &str, selection: Range<usize>, width: usize) -> TextEdit {
    let b = line.as_bytes();
    let before = b[..selection.start].iter().rev().take_while(|c| **c == b'*').count();
    let after = b[selection.end..].iter().take_while(|c| **c == b'*').count();
    let around = before.min(after);
    let has = if width == 2 { around >= 2 } else { around % 2 == 1 };
    if has && !selection.is_empty() {
        let range = selection.start - width..selection.end + width;
        let text = line[selection.clone()].to_owned();
        let selection = selection.start - width..selection.end - width;
        return TextEdit { range, text, selection };
    }
    let marker = "*".repeat(width);
    let text = format!("{marker}{}{marker}", &line[selection.clone()]);
    let selection = selection.start + width..selection.end + width;
    TextEdit { range: selection.start - width..selection.end - width, text, selection }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(line: &str) -> Vec<(&str, SpanStyle)> {
        style_line(line, 0).runs.into_iter().map(|(r, s)| (&line[r], s)).collect()
    }

    fn find<'a>(line: &'a str, part: &str) -> SpanStyle {
        let start = line.find(part).unwrap_or_else(|| panic!("{part} in {line}"));
        let styled = style_line(line, 0);
        styled.runs.iter().find(|(r, _)| r.start <= start && start < r.end).map(|(_, s)| s.clone()).unwrap_or_default()
    }

    fn dim() -> SpanStyle {
        SpanStyle { color: Some(TextColor::Tertiary), ..SpanStyle::default() }
    }

    #[test]
    fn headings_scale_by_level_and_dim_their_hashes() {
        let h1 = runs("# Title");
        assert_eq!(h1[0].0, "# ");
        assert_eq!(h1[0].1.color, Some(TextColor::Tertiary));
        assert_eq!((h1[1].0, h1[1].1.bold, h1[1].1.scale), ("Title", true, Some(1.75)));
        assert_eq!(find("### Three", "Three").scale, Some(1.2));
        assert_eq!(style_line("## Two", 0).paragraph.line_height, Some(1.3));
        assert!(runs("#hashtag").is_empty(), "no space after the hash is text");
    }

    #[test]
    fn emphasis_strong_strike_and_code_mark_their_insides() {
        let line = "a **bold** and *it* and ~~gone~~ and `co*de*`";
        assert!(find(line, "bold").bold);
        assert_eq!(find(line, "**bold"), dim());
        assert!(find(line, "it*").italic);
        assert!(find(line, "gone").strikethrough);
        let code = find(line, "co*de");
        assert!(code.mono && !code.italic);
        assert_eq!(code.background, Some(TextFill::Subtle));
        let both = "***both***";
        let s = find(both, "both");
        assert!(s.bold && s.italic);
        assert!(runs("2 * 3 * 4").is_empty(), "spaced stars are not emphasis");
        assert!(runs("snake_case_name").is_empty(), "underscores inside a word are not emphasis");
        assert!(runs(r"\*not\*").iter().all(|(_, s)| !s.italic));
    }

    #[test]
    fn links_take_the_accent_and_dim_their_target() {
        let line = "see [docs](https://nd.dev/a_b_c) or https://x.dev.";
        assert_eq!(find(line, "docs").color, Some(TextColor::Accent));
        assert_eq!(find(line, "](https").color, Some(TextColor::Tertiary));
        assert!(!find(line, "b_c").italic);
        assert_eq!(find(line, "https://x.dev").color, Some(TextColor::Accent));
        let period = style_line(line, 0).runs.iter().any(|(r, _)| r.contains(&(line.len() - 1)));
        assert!(!period, "a trailing period is not the URL's");
    }

    #[test]
    fn fences_carry_code_across_lines_until_they_close() {
        let open = style_line("```rust", 0);
        assert_eq!(open.state, IN_BACKTICKS);
        assert!(open.runs.iter().all(|(_, s)| s.mono && s.color == Some(TextColor::Tertiary)));
        let body = style_line("let *x* = 1;", IN_BACKTICKS);
        assert_eq!(body.state, IN_BACKTICKS);
        assert_eq!(body.runs.len(), 1);
        assert!(body.runs[0].1.mono && !body.runs[0].1.italic);
        assert_eq!(body.runs[0].1.background, Some(TextFill::Subtle));
        assert_eq!(style_line("~~~", IN_BACKTICKS).state, IN_BACKTICKS, "a tilde fence does not close backticks");
        assert_eq!(style_line("```", IN_BACKTICKS).state, 0);
    }

    #[test]
    fn tasks_quotes_lists_and_rules() {
        let open = "- [ ] buy milk";
        assert_eq!(find(open, "- ").color, Some(TextColor::Secondary));
        assert_eq!(find(open, "[ ]").color, Some(TextColor::Accent));
        assert_eq!(find(open, "buy"), SpanStyle::default());
        let done = "- [x] buy milk";
        let text = find(done, "buy");
        assert!(text.strikethrough && text.color == Some(TextColor::Tertiary));
        assert!(!find(done, "[x]").strikethrough);

        let quote = style_line("> quoted", 0);
        assert!(quote.paragraph.first_indent > 0.);
        assert_eq!(find("> quoted", ">").color, Some(TextColor::Tertiary));
        assert_eq!(find("> quoted", "quoted").color, Some(TextColor::Secondary));

        let item = style_line("12. twelve", 0);
        assert!(item.paragraph.head_indent > item.paragraph.first_indent);
        let rule = find("---", "---");
        assert!(rule.strikethrough && rule.color == Some(TextColor::Tertiary));
        assert!(find("- item", "item") == SpanStyle::default());
        assert_eq!(style_line("plain", 0).paragraph.line_height, Some(BODY_LEADING));
    }

    fn apply(line: &str, edit: &TextEdit) -> String {
        let mut out = line.to_owned();
        out.replace_range(edit.range.clone(), &edit.text);
        out
    }

    fn key(line: &str, key: EditKey, caret: usize) -> Option<(String, Range<usize>)> {
        Markdown.edit(key, line, caret..caret, 0).map(|e| (apply(line, &e), e.selection))
    }

    #[test]
    fn return_continues_lists_tasks_and_quotes() {
        assert_eq!(key("- one", EditKey::Return, 5), Some(("- one\n- ".into(), 8..8)));
        assert_eq!(key("  * one", EditKey::Return, 7), Some(("  * one\n  * ".into(), 12..12)));
        assert_eq!(key("9. nine", EditKey::Return, 7), Some(("9. nine\n10. ".into(), 12..12)));
        assert_eq!(key("- [x] done", EditKey::Return, 10), Some(("- [x] done\n- [ ] ".into(), 17..17)));
        assert_eq!(key("> said", EditKey::Return, 6), Some(("> said\n> ".into(), 9..9)));
        assert_eq!(key("- split here", EditKey::Return, 7), Some(("- split\n-  here".into(), 10..10)));
        assert_eq!(key("plain", EditKey::Return, 5), None);
        assert_eq!(key("- one", EditKey::Return, 1), None, "inside the marker the key is the area's");
        assert_eq!(Markdown.edit(EditKey::Return, "- one", 2..2, IN_BACKTICKS), None, "code is plain");
    }

    #[test]
    fn return_on_an_empty_marker_lifts_or_clears_it() {
        assert_eq!(key("- ", EditKey::Return, 2), Some((String::new(), 0..0)));
        assert_eq!(key("- [ ] ", EditKey::Return, 6), Some((String::new(), 0..0)));
        assert_eq!(key("> ", EditKey::Return, 2), Some((String::new(), 0..0)));
        assert_eq!(key("> - ", EditKey::Return, 4), Some(("> ".into(), 2..2)));
        assert_eq!(key("  - ", EditKey::Return, 4), Some(("- ".into(), 2..2)), "a nested item lifts a level");
        assert_eq!(key("    - [ ] ", EditKey::Return, 10), Some(("  - [ ] ".into(), 8..8)));
    }

    #[test]
    fn tab_nests_list_items_and_shift_tab_lifts_them() {
        assert_eq!(key("- item", EditKey::Tab, 6), Some(("  - item".into(), 8..8)));
        assert_eq!(key("1. item", EditKey::Tab, 3), Some(("   1. item".into(), 6..6)));
        assert_eq!(key("    - item", EditKey::Backtab, 10), Some(("  - item".into(), 8..8)));
        assert_eq!(key("- item", EditKey::Backtab, 6), Some(("- item".into(), 6..6)));
        assert_eq!(key("plain", EditKey::Tab, 0), None);
    }

    #[test]
    fn command_b_and_i_wrap_and_unwrap() {
        let bold = Markdown.edit(EditKey::Bold, "a word", 2..6, 0).unwrap_or_else(|| panic!("bold"));
        assert_eq!((apply("a word", &bold), bold.selection.clone()), ("a **word**".into(), 4..8));
        let back = Markdown.edit(EditKey::Bold, "a **word**", 4..8, 0).unwrap_or_else(|| panic!("unbold"));
        assert_eq!((apply("a **word**", &back), back.selection), ("a word".into(), 2..6));
        let italic = Markdown.edit(EditKey::Italic, "a **word**", 4..8, 0).unwrap_or_else(|| panic!("italic"));
        assert_eq!(apply("a **word**", &italic), "a ***word***");
        assert_eq!(key("ab", EditKey::Italic, 1), Some(("a**b".into(), 2..2)));
    }
}
