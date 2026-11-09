+++
title = "Styled text editing"
description = "A text area the app styles paragraph by paragraph as the user types: the styler trait, the markdown editor, key edits and the centred measure."
+++

A `text_area` can style its own text as it changes. The app hands it a styler, the styler looks
at one paragraph at a time, and the backend sets what it returns on the native text: the
`NSTextStorage` on macOS, the drawn editor's text runs elsewhere. Styling never moves the
caret, the selection or the scroll position, and it never adds an undo step.

## The markdown editor

`markdown_editor(id, &state)` is a text area bound to a `TextFieldState` with the markdown
styler on it, a column at most 820 points wide and 15 point body text on a 1.6 line.

<!-- docs:snippet examples/gallery/src/forms.rs#markdown -->

The syntax stays in the text, dimmed to the tertiary text colour. Headings scale by level,
emphasis, strikethrough and code spans take their look between their markers, fences set
their lines in the monospaced font on a code well, quotes indent in the secondary colour, and a
ticked task (`- [x]`) is dimmed and struck through. Links take the accent.

Keys it takes:

| Key | In the editor |
|---|---|
| Return | Continues a list, task or quote line with the next marker. On an empty nested item it lifts the item a level; on an empty top-level marker it clears the marker. |
| Tab, Shift-Tab | Nests a list item one level, or lifts it one. Off a list line Tab inserts a tab. Control-Tab and Control-Shift-Tab move focus out of the editor. |
| Command-B, Command-I | Wraps the selection in `**` or `*`, or unwraps it. With nothing selected the caret lands between a new pair. |

The scanner reads lines on their own. A setext heading (a line underlined with `===`) reads as
a line of text and a rule.

## Writing a styler

A styler is anything that implements `nd::styled_text::TextStyler`, and any
`Fn(&str, u32) -> StyledParagraph` does:

```rust
fn style(&self, paragraph: &str, state: u32) -> StyledParagraph;
```

`paragraph` is the text between two line breaks. `StyledParagraph` holds the runs (byte ranges
into the paragraph, each with a `SpanStyle`), a `ParagraphStyle`, and a `state` for the next
paragraph. The first paragraph gets `0`.

The state is what makes styling incremental. After an edit the backend restyles the paragraphs
the edit touched, then keeps going only while the state it hands on differs from what the next
paragraph was styled with. Markdown keeps "inside a code fence" there, so typing in a body line
styles that line alone, and closing a fence restyles up to the fence that ends the block. An
unclosed fence turns the rest of the document into code, as markdown reads it, and restyles all
of it.

`SpanStyle` sets size (`scale`, a multiple of the area's size), weight, italic, monospace, a
colour token, a `background` (`TextFill::Subtle` is the text's ink at a low alpha),
underline and strikethrough. `ParagraphStyle` sets the line height as a multiple of the font
size, the spacing before and after, and the first-line and head indents. Its lengths are in
ems of the area's font size, so they follow `.text_size()`.

Two stylers of the same type are equal. A view that builds its closure again on every render
does not restyle anything; switching to a styler of another type restyles the whole text.

`TextStyler::edit` is optional. The area asks it before handling Return, Tab, Shift-Tab and
Command-B/I, with the caret's paragraph and the selection inside it. A `TextEdit` it returns
replaces a range of the paragraph and sets the selection, as one undo step; `None` leaves the
key to the area.

## Measure

`.max_measure(points)` keeps the text at most that wide, centred in the area, with the room
either side as margin. A document stays readable in a wide window without the app laying out
its own column.

## Borderless

`.borderless()` drops the field chrome: no bezel, outline, focus ring or fill, so the text sits
on the window behind it. Use it for an area that is the page, a document editor filling its
window, rather than a field in a form. The text keeps the inset it has inside the bezel, and
`markdown_editor(..)` takes it too: `markdown_editor("doc", &state).borderless()`.

## How the backends set it

On macOS the text view's storage delegate keeps a paragraph cache in step with every character
edit, in UTF-16 units. Once the edit is over, the restyled paragraphs are set inside one
`beginEditing`/`endEditing` as attribute changes, which the undo manager never records. While an
input method composes, nothing is set; the paragraphs wait for the composition to end. The
typing attributes follow the character before the caret.

The drawn editor (Linux, Windows, the web, and macOS with `ND_BACKEND=gpui`) shapes each
paragraph on its own, keeps the shape until the paragraph, its style or the column width
changes, and shapes only the paragraphs near the viewport and the caret's. The others count at
their last height or an estimate. A paragraph is set at one size, the largest of its runs, and
its wrapped lines start at its first-line indent.
