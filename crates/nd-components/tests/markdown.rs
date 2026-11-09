//! Tier 1: the markdown styler over a long document, through the paragraph cache every
//! backend keeps. A keystroke restyles its own paragraph; opening a fence restyles up to the
//! fence that now closes it, and no further.

use std::borrow::Cow;
use std::time::Instant;

use nd_components::Markdown;
use nd_ui::styled_text::ParagraphCache;
use nd_ui::{SpanStyle, Styler};

const CHAPTER: &str = "# Chapter

Some **bold** words, some *italic* ones, a `code span` and a [link](https://nd.dev).

- [ ] a task to do
- [x] a task done
  1. nested
> a quote that goes on for a while so the line is not short at all

```rust
fn main() { let x = 1; }
```
---
";

/// About a million characters of markdown.
fn document() -> String {
    CHAPTER.repeat(1_000_000 / CHAPTER.len() + 1)
}

/// The paragraph is set as code from its first character.
fn mono(cache: &ParagraphCache<()>, index: usize) -> bool {
    cache.paragraphs()[index].styled.runs.first().is_some_and(|(r, s): &(_, SpanStyle)| r.start == 0 && s.mono)
}

#[test]
fn a_keystroke_in_a_million_characters_restyles_one_paragraph() {
    let mut text = document();
    assert!(text.len() >= 1_000_000);
    let styler = Styler::new(Markdown);
    let mut cache = ParagraphCache::<()>::default();
    let started = Instant::now();
    cache.reset(&text, &styler, str::len);
    let full = started.elapsed();
    let paragraphs = cache.paragraphs().len();

    // A letter typed into a body line halfway down.
    let line = text[text.len() / 2..].find("Some ").map(|i| text.len() / 2 + i + 5).unwrap_or(0);
    let before = cache.styled_count();
    text.insert(line, 'x');
    let started = Instant::now();
    let touched = cache.edit(line..line, 1, &styler, str::len, |r| Cow::Borrowed(&text[r]));
    let keystroke = started.elapsed();
    assert_eq!(touched.len(), 1, "only the edited paragraph");
    assert_eq!(cache.styled_count() - before, 1);
    assert_eq!(cache.paragraphs().len(), paragraphs);

    // Backspace joining two lines restyles the one paragraph left.
    let join = text[line..].find('\n').map_or(0, |i| line + i);
    let before = cache.styled_count();
    text.remove(join);
    let touched = cache.edit(join..join + 1, 0, &styler, str::len, |r| Cow::Borrowed(&text[r]));
    assert_eq!(touched.len(), 1);
    assert_eq!(cache.styled_count() - before, 1);
    assert_eq!(cache.paragraphs().len(), paragraphs - 1);

    eprintln!(
        "{} chars, {paragraphs} paragraphs: styling all {full:?}, restyling after a keystroke {keystroke:?}",
        text.len()
    );
}

#[test]
fn a_fenced_block_restyles_only_up_to_its_closing_fence() {
    let mut text = document();
    let styler = Styler::new(Markdown);
    let mut cache = ParagraphCache::<()>::default();
    cache.reset(&text, &styler, str::len);
    let at = CHAPTER.len() + text[CHAPTER.len()..].find("Some").unwrap_or(0);
    let (first, _) = cache.locate(at);
    assert!(!mono(&cache, first));
    let before = cache.styled_count();
    let block = "```\nlet *x* = 1;\n```\n";
    text.insert_str(at, block);
    let touched = cache.edit(at..at, block.len(), &styler, str::len, |r| Cow::Borrowed(&text[r]));
    assert!(mono(&cache, first + 1), "the line inside the fence is code");
    assert!(!mono(&cache, first + 3), "the line after the closing fence is not");
    assert_eq!(touched, first..first + 4);
    assert_eq!(cache.styled_count() - before, 4);
}

#[test]
fn an_unclosed_fence_turns_the_rest_into_code_as_markdown_reads_it() {
    let mut text = String::from("a\n```\nb\n```\nc");
    let styler = Styler::new(Markdown);
    let mut cache = ParagraphCache::<()>::default();
    cache.reset(&text, &styler, str::len);
    text.insert_str(0, "```\n");
    let touched = cache.edit(0..0, 4, &styler, str::len, |r| Cow::Borrowed(&text[r]));
    assert_eq!(touched, 0..6, "every fence after it flips");
    assert!(mono(&cache, 5), "`c` now sits in the block the old closing fence opens");
}
