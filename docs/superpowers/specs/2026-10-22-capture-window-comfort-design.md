# Capture window comfort: resizing, room to write, autocorrect

Date: 2026-10-22
Status: approved design, awaiting spec review

## Why

The capture box is 520 px wide with a 92 px tall edit, at 96 DPI, and cannot be resized.
Writing more than a line or two in it feels claustrophobic. Typos made in a hurry go
straight into the capture.

## What the user asked for

- The capture box can be resized.
- It is more comfortable to write in: a larger default size, it remembers the size you
  gave it, and the text does not touch the edit's edges.
- English autocorrect that silently fixes common typos as you type ("teh" → "the").
  No red underlines and no suggestion menus.

Out of scope: a larger or configurable font, spell-check underlines, languages other
than English, remembering the box's position.

## 1. Resizing and size

### Behavior

- Every edge and corner resizes the box. The resize band is `frame::resize_border(dpi)`
  wide, the same as the review and settings windows. The title strip still moves the box;
  the close button is still a button.
- No maximize button. The box stays a `WS_POPUP` hotkey window.
- Sizes at 96 DPI, scaled with `scale(v, dpi)`:
  - default: 720 × 420
  - minimum: 420 × 240 (`WM_GETMINMAXINFO`)
  - maximum: the work area of the box's monitor
- When a move or resize ends (`WM_EXITSIZEMOVE`), the box saves its size as
  `capture_width` and `capture_height` in `fastcapture.ini` in physical pixels, using
  `SettingsFile::set` and `save` as `review.rs` does. It saves only the size.
- When it opens, the box uses the saved size if both numbers are there and at least the
  minimum, otherwise the default. Then it clamps the size to the work area of the monitor
  under the mouse and centers it there a third of the way down, as `place` does today.
- A DPI change while the box is open (`WM_DPICHANGED`) takes the suggested rectangle,
  rebuilds the fonts and lays out again, so the box keeps its size relative to the text.

### Layout

`layout` takes the window's client size instead of computing it:

```rust
pub fn layout(dpi: u32, size: (i32, i32), chip_widths: &[i32], has_error: bool, snip_width: i32) -> Layout
```

From the top down: title, edit, chips, error, footer. Chips, error and footer are placed
from the bottom of the window up. The edit gets whatever height is left, with a floor of
two lines' worth (about 40 px at 96 DPI). Chips still wrap at the window's width.

`relayout` no longer resizes the window. It only moves the edit and repaints. `WM_SIZE`
calls `relayout`.

Adding attachments or an error therefore shrinks the edit instead of growing the window.
The minimum size leaves room for the edit floor plus two rows of chips and an error. When
there are more chips than that, the edit stays at its floor and the last rows of chips are
clipped. The footer still says how many attachments there are, and Tab still reaches all of
them.

## 2. Writing comfort

- `EM_SETMARGINS` gives the edit 8 px left and right margins at 96 DPI, scaled.
- The edit's rectangle starts 6 px (scaled) below the title strip, so the first line has
  room above it.
- The font stays `TextStyle::Body`.

## 3. Autocorrect

### Engine: `src/platform/spell.rs`

A wrapper around the Windows Spell Checking API (`ISpellCheckerFactory`, `ISpellChecker`,
Windows 8 and later), using the `windows` crate with the added `Win32_Globalization`
feature. No new crate.

```rust
pub struct Speller { /* ISpellChecker for "en-US" */ }

impl Speller {
    /// None when COM fails or "en-US" is not supported on this PC.
    pub fn english() -> Option<Speller>;
    /// The sure replacement for `word`, if Windows has one.
    pub fn replacement(&self, word: &str) -> Option<String>;
}
```

`replacement` calls `ISpellChecker::Check(word)`. It returns `Some` only when the first
error covers the whole word and its `CorrectiveAction` is `CORRECTIVE_ACTION_REPLACE`,
which is Windows' autocorrect list. `GET_SUGGESTIONS` and `DELETE` return `None`. The
`Speller` is created once on the UI thread when the first box opens, and kept for the life
of the app. The UI thread does not initialize COM today, so `english()` calls
`CoInitializeEx(COINIT_APARTMENTTHREADED)` first, as `platform/shell.rs` does, and keeps
COM initialized while the `Speller` lives. If the call fails, `english()` returns `None`.

### When it runs

- In the edit's subclass (`edit_proc`), on `WM_CHAR` for a word boundary: space, Enter,
  Tab when it is inserted, and `. , ; : ! ? ) ] " '`. The check runs before the character
  goes to the edit.
- `word_before(text, caret) -> Option<Range<usize>>` is a pure function. It returns the run
  of letters and apostrophes that ends at the caret. It returns `None` when:
  - the run is shorter than 2 characters,
  - the run is preceded by a character that makes it part of a token: a digit, `_`, `/`,
    `\`, `@`, `.`, `:`, `-` or `#`,
  - the run is inside a URL: the line's text from the last whitespace up to the caret
    contains `://` or starts with `www.`.
- When `Speller::replacement` returns a replacement, the box selects the word and replaces
  it with `EM_REPLACESEL`, matching its case: all-caps stays all-caps and a leading
  capital stays capital. Then the boundary character goes through as usual.
- Text inserted by paste, `insert_text`, a restored draft or `set_edit_text` is never
  checked, because those do not arrive as `WM_CHAR`.

### Undo

The box remembers the last fix: where it is, the original word and the replacement. The
memory is cleared by any key other than the boundary character just typed, any click, any
paste and any change of selection. Ctrl+Z while the memory is set puts the original word
back, keeps the boundary character, and clears the memory. Otherwise Ctrl+Z goes to the
edit as it does today.

### Settings

- `fastcapture.ini`: `autocorrect=on`, the default, or `off`, parsed with `parse_bool`, with
  a commented line in the written template.
- Settings window: a checkbox, "Fix common typos as I type (English)".
- When `Speller::english()` returns `None`, autocorrect does nothing and the log gets one
  line saying why. The setting is left as it is.

## Testing

- `layout`: the edit fills what is left at several sizes, chips wrap at the window's width,
  the edit never goes below its floor, and the results scale with DPI.
- Size handling: a saved size is used when valid, ignored when missing or too small,
  clamped to the work area, and saved on `WM_EXITSIZEMOVE`.
- `word_before`: plain words, words after punctuation, apostrophes ("dont"), digits, paths,
  URLs, emails, `#tags`, one-letter words and the caret at the start of the text.
- Case matching: "teh" → "the", "Teh" → "The", "TEH" → "THE".
- Integration, run on the UI thread like the existing box tests:
  - typing "teh " gives "the ", and Ctrl+Z then gives "teh ";
  - pasting "teh " is left alone;
  - with `autocorrect=off`, nothing changes.
  These tests skip themselves when `Speller::english()` returns `None`.
- The existing capture box tests keep passing. The ones that assert on `layout`'s width
  and height move to the new signature.

## README

Document resizing (the box remembers its size) and the autocorrect setting in "The capture
box" section.
