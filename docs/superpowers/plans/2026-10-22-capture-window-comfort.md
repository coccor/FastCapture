# Capture Window Comfort Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the capture box resizable, larger by default, size-remembering and padded, and add silent English autocorrect for typed words.

**Architecture:** The box's `layout` takes the window's client size instead of computing it. The text area absorbs whatever height the chips, error and footer leave. Resize edges reuse `frame.rs`'s edge test, and the size is saved to `fastcapture.ini` the way `review.rs` saves its place. Autocorrect is a pure decision module (`src/ui/autocorrect.rs`) fed by a thin wrapper around Windows' `ISpellChecker` (`src/platform/spell.rs`), called from the edit's existing subclass on word-boundary characters.

**Tech Stack:** Rust 2024, `windows-sys` 0.61 for Win32 UI, `windows` 0.62 for COM (`ISpellCheckerFactory`).

**Spec:** `docs/superpowers/specs/2026-10-22-capture-window-comfort-design.md`

## Global Constraints

- No new crates. Add only the `Win32_Globalization` feature to the `windows` dependency in `Cargo.toml`.
- All pixel sizes are at 96 DPI and go through `scale(v, dpi)`: default 720 × 420, minimum 420 × 240, text margins 8 left and right, 6 above the edit, edit floor 40.
- `fastcapture.ini` keys: `capture_width`, `capture_height` (physical pixels), `autocorrect` (`on` by default, read with `parse_bool`).
- Settings checkbox text, exactly: `Fix common typos as I type (English)`.
- Autocorrect language: `en-US`. Replace only on `CORRECTIVE_ACTION_REPLACE`.
- CI runs `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings` and `cargo test --locked -- --test-threads=1`. All three must pass after every task.
- Tests that need the `en-US` checker return early, with an `eprintln!` saying they were skipped, when `spell::with_english` returns `None`.
- Text positions sent to the edit (`EM_SETSEL`) are UTF-16 code-unit offsets.

## Review Focus

- **A saved size bigger than the screen the box opens on** (a smaller second monitor, or a different scale): the box must fit the work area, not run off-screen. Test `a_saved_size_is_clamped_to_the_work_area` in Task 4.
- **Typing a boundary with the caret in the middle of the text:** only the word before the caret is checked and replaced, and the text after it is untouched. Test `a_word_in_the_middle_is_fixed_in_place` in Task 5.
- **Non-ASCII text before the typo** ("café", emoji with surrogate pairs): the replacement must land on the typo, not shifted by mis-counted UTF-16 units. Test `fixes_land_after_non_ascii_text` in Task 5.
- **Ctrl+Z after typing more:** once you've typed another key, Ctrl+Z must not resurrect an old typo over newer text. Test `ctrl_z_after_more_typing_does_not_bring_the_typo_back` in Task 5.
- **Many attachments in a minimum-size box:** chips must never be drawn over the error line or footer. Test `chips_that_do_not_fit_are_clipped_not_overlapping` in Task 3.

---

### Task 1: The English spell checker wrapper

**Files:**
- Modify: `Cargo.toml` (add `"Win32_Globalization"` to the `windows` features, alphabetically)
- Create: `src/platform/spell.rs`
- Modify: `src/platform/mod.rs` (add `pub mod spell;`)

**Interfaces:**
- Produces:
  - `pub struct Speller` (holds an `ISpellChecker`; `!Send`)
  - `impl Speller { pub fn replacement(&self, word: &str) -> Option<String> }`
  - `pub fn with_english<R>(f: impl FnOnce(&Speller) -> R) -> Option<R>`: runs `f` with this thread's `en-US` speller. The speller is created lazily in a `thread_local!` the first time. `None` if COM, the factory or `en-US` is unavailable. In that case the reason is logged once with `crate::log::line("spell: …")` and later calls return `None` without retrying.

- [ ] **Step 1: Write the failing tests** in `src/platform/spell.rs`'s `#[cfg(test)] mod tests`

```rust
#[test]
fn windows_autocorrects_teh_to_the() {
    let Some(fix) = with_english(|s| s.replacement("teh")) else {
        eprintln!("skipped: no en-US spell checker");
        return;
    };
    assert_eq!(fix.as_deref(), Some("the"));
}

#[test]
fn correct_words_and_mere_misspellings_are_left_alone() {
    let Some((right, unsure)) =
        with_english(|s| (s.replacement("the"), s.replacement("qwzxv")))
    else {
        eprintln!("skipped: no en-US spell checker");
        return;
    };
    assert_eq!(right, None);
    assert_eq!(unsure, None, "GET_SUGGESTIONS is not a sure fix");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib platform::spell -- --test-threads=1`
Expected: compile error, `with_english` not found.

- [ ] **Step 3: Implement `spell.rs`**

`with_english` first calls `CoInitializeEx(None, COINIT_APARTMENTTHREADED)` once per thread, as `platform/shell.rs` does. `S_FALSE` and `RPC_E_CHANGED_MODE` (already initialized) count as fine. Never call `CoUninitialize`, because the speller lives as long as the thread. Then: `CoCreateInstance::<_, ISpellCheckerFactory>(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER)`, `IsSupported(w!("en-US"))`, then `CreateSpellChecker(w!("en-US"))`.

`replacement(word)`: call `Check(word)`, take the first `ISpellingError` from the enumerator, and return `Some(Replacement())` only when `StartIndex() == 0`, `Length()` equals the word's UTF-16 length and `CorrectiveAction() == CORRECTIVE_ACTION_REPLACE`. Every COM error returns `None`.

**Stop condition:** if `windows_autocorrects_teh_to_the` runs (doesn't skip) on this PC and still fails, stop and report. The spec's autocorrect design rests on Windows returning `REPLACE` for its autocorrect list.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib platform::spell -- --test-threads=1`
Expected: 2 passed, and no "skipped" lines on a desktop with English installed.

- [ ] **Step 5: Run clippy, then commit**

```bash
cargo clippy --locked --all-targets --all-features -- -D warnings
git add Cargo.toml Cargo.lock src/platform/spell.rs src/platform/mod.rs
git commit -m "Wrap Windows' English spell checker for sure typo fixes"
```

---

### Task 2: The autocorrect decision rules

Pure logic, with no window and no COM.

**Files:**
- Create: `src/ui/autocorrect.rs`
- Modify: `src/ui/mod.rs` (add `pub mod autocorrect;` in alphabetical order)

**Interfaces:**
- Produces:
  - `pub fn is_boundary(ch: u32) -> bool`: true for space (0x20), Enter (0x0D) and `. , ; : ! ? ) ] " '`
  - `pub fn word_before(before: &str) -> Option<&str>`: `before` is the edit's text up to the caret; returns the eligible word, which is a suffix of `before`
  - `pub fn match_case(original: &str, replacement: &str) -> String`
  - `pub struct Correction { pub start: usize, pub original: String, pub replacement: String }`: `start` is the word's UTF-16 offset in the text
  - `pub fn correction(before: &str, lookup: impl FnOnce(&str) -> Option<String>) -> Option<Correction>`: `word_before`, then `lookup` on the lowercase word, then `match_case`; returns `None` when the replacement equals the word

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_word_before_the_caret_is_a_run_of_letters_and_apostrophes() {
    assert_eq!(word_before("I think teh"), Some("teh"));
    assert_eq!(word_before("(teh"), Some("teh"));
    assert_eq!(word_before("dont"), Some("dont"));
    assert_eq!(word_before("I"), None, "one letter");
    assert_eq!(word_before(""), None);
    assert_eq!(word_before("teh "), None, "the caret is after a space");
}

#[test]
fn tokens_paths_urls_and_tags_are_not_words() {
    for before in [
        "v2teh", "my_teh", r"C:\teh", "src/teh", "me@teh", "file.teh", "a:teh", "re-teh",
        "#teh", "https://example.com/teh", "www.example.com/teh",
    ] {
        assert_eq!(word_before(before), None, "{before}");
    }
}

#[test]
fn a_fix_keeps_the_typed_case() {
    assert_eq!(match_case("teh", "the"), "the");
    assert_eq!(match_case("Teh", "the"), "The");
    assert_eq!(match_case("TEH", "the"), "THE");
}

#[test]
fn a_correction_names_the_word_by_its_utf16_offset() {
    let fix = correction("café 😀 Teh", |w| (w == "teh").then(|| "the".into())).unwrap();
    assert_eq!(fix.start, "café 😀 ".encode_utf16().count());
    assert_eq!((fix.original.as_str(), fix.replacement.as_str()), ("Teh", "The"));
    assert!(correction("the", |_| Some("the".into())).is_none());
    assert!(correction("qwzxv", |_| None).is_none());
}

#[test]
fn boundaries_are_space_enter_and_closing_punctuation() {
    for ch in " \r.,;:!?)]\"'".chars() {
        assert!(is_boundary(ch as u32), "{ch:?}");
    }
    for ch in "a1-_/\t\n".chars() {
        assert!(!is_boundary(ch as u32), "{ch:?}");
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib ui::autocorrect`
Expected: compile errors, functions not found.

- [ ] **Step 3: Implement the five items** with the signatures above

The URL rule looks at `before` from its last whitespace character: the token contains `://` or starts with `www.` (case-insensitive). A word counts as letters when it has `char::is_alphabetic` characters and `'`, with a minimum of 2 characters. The character just before the run must not be a digit or one of `_ / \ @ . : - #`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib ui::autocorrect`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add src/ui/autocorrect.rs src/ui/mod.rs
git commit -m "Decide which typed word autocorrect may fix, and how"
```

---

### Task 3: A resizable box whose text area fills it

**Files:**
- Modify: `src/ui/frame.rs:130-170` (extract the edge test)
- Modify: `src/ui/capture_box.rs` (constants at :56-65, `layout` at :142-212, `open` at :344-445, `relayout` at :517-561, `box_proc` at :1239-1365, and the `draw` and `clicked` chip loops at :1088 and :1193)
- Test: `src/ui/capture_box.rs` tests module, `src/ui/frame.rs` tests module

**Interfaces:**
- Produces:
  - `frame::edge(x: i32, y: i32, width: i32, height: i32, border: i32) -> Option<Target>`: the resize part of `Layout::hit`, which `hit` now calls
  - `capture_box::layout(dpi: u32, size: (i32, i32), chip_widths: &[i32], has_error: bool, snip_width: i32) -> Layout`. `size` is the client size, and `Layout.width` and `height` echo it. A chip that doesn't fit is `RECT::default()` (empty), so `chips.len() == chip_widths.len()` always.
  - constants `DEFAULT_SIZE_AT_96_DPI: (i32, i32) = (720, 420)`, `MIN_SIZE_AT_96_DPI: (i32, i32) = (420, 240)`, `EDIT_FLOOR_AT_96_DPI = 40`, `EDIT_TOP_GAP_AT_96_DPI = 6`, `EDIT_MARGIN_AT_96_DPI = 8`. These replace `WIDTH_AT_96_DPI` and `EDIT_AT_96_DPI`.

- [ ] **Step 1: Write the failing tests.** Replace `the_strip_appears_only_with_attachments_and_wraps` with:

```rust
#[test]
fn the_edit_fills_what_the_strip_error_and_footer_leave() {
    let plain = layout(96, (720, 420), &[], false, 160);
    assert_eq!((plain.width, plain.height), (720, 420));
    assert_eq!(plain.edit.top, 34 + 6);
    assert_eq!(plain.footer.bottom, 420 - 4);
    assert_eq!(plain.edit.bottom + 8, plain.footer.top);
    let with = layout(96, (520, 420), &[150, 150, 150], true, 160);
    assert_eq!(with.chips[0].top, with.chips[1].top);
    assert!(with.chips[2].top > with.chips[1].top, "the third wraps at 520");
    assert!(with.edit.bottom < plain.edit.bottom, "the edit gave the chips room");
    assert!(with.error.unwrap().top >= with.chips[2].bottom);
    assert_eq!(layout(192, (1440, 840), &[], false, 320).edit.top, 80);
}

#[test]
fn chips_that_do_not_fit_are_clipped_not_overlapping() {
    let tight = layout(96, (420, 240), &[200; 12], true, 160);
    assert_eq!(tight.chips.len(), 12);
    assert_eq!(tight.edit.bottom - tight.edit.top, 40, "the edit keeps its floor");
    let error = tight.error.unwrap();
    for chip in tight.chips.iter().filter(|c| c.right > c.left) {
        assert!(chip.bottom <= error.top, "{chip:?}");
    }
    assert!(tight.chips.iter().any(|c| c.right == c.left), "some were clipped");
}
```

In `frame.rs` tests:

```rust
#[test]
fn edges_resize_and_the_middle_does_not() {
    assert_eq!(edge(0, 0, 500, 300, 6), Some(Target::TopLeft));
    assert_eq!(edge(499, 150, 500, 300, 6), Some(Target::Right));
    assert_eq!(edge(250, 299, 500, 300, 6), Some(Target::Bottom));
    assert_eq!(edge(250, 150, 500, 300, 6), None);
}
```

Add a window test in the capture box module:

```rust
#[test]
fn the_box_resizes_from_its_edges_and_keeps_a_minimum() {
    let drafts = temp("resize");
    let (hwnd, _) = open_box(&drafts, None);
    let mut frame: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut frame) };
    let at = |x, y| unsafe {
        SendMessageW(hwnd, WM_NCHITTEST, 0, ((y as u32) << 16 | (x as u32 & 0xFFFF)) as isize)
    };
    assert_eq!(at(frame.right - 1, frame.bottom - 1), HTBOTTOMRIGHT as isize);
    let mut info: MINMAXINFO = unsafe { std::mem::zeroed() };
    unsafe { SendMessageW(hwnd, WM_GETMINMAXINFO, 0, &raw mut info as isize) };
    let dpi = state(hwnd).unwrap().dpi;
    assert_eq!(info.ptMinTrackSize.x, scale(420, dpi));
    unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, scale(900, dpi), scale(600, dpi),
        SWP_NOMOVE | SWP_NOZORDER) };
    let capture = state(hwnd).unwrap();
    assert_eq!(capture.layout.width, scale(900, dpi), "WM_SIZE laid it out again");
    unsafe { DestroyWindow(hwnd) };
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib ui::capture_box ui::frame -- --test-threads=1`
Expected: compile errors (the new `layout` signature, and `edge`).

- [ ] **Step 3: Implement**

- `frame::edge`: move the border `match` out of `Layout::hit` into the new function. `hit` calls it when `border` is `Some`.
- `layout`: the footer is pinned to `height - s(4)` and the error line sits directly above it. Wrap the chips into rows at `width - pad` as today, giving `rows`. The block of chips sits above the error (or the footer), with `s(8)` gaps. `edit.bottom = max(edit.top + floor, chips_top - s(8))`, or `footer/error top - s(8)` when there are no chips. If the edit hits its floor, the chips flow down from `edit.bottom + s(8)`, and any chip whose bottom passes the error's (or footer's) top becomes `RECT::default()`.
- `open`: create the window at the default size scaled. Task 4 replaces this with the saved size. Send `EM_SETMARGINS` (`EC_LEFTMARGIN | EC_RIGHTMARGIN`, both `scale(8, dpi)`) to the edit.
- `relayout`: compute `layout` from `GetClientRect`, move the edit, and stop calling `SetWindowPos` on the box.
- `box_proc`:
  - `WM_NCHITTEST` returns `frame::edge(...)` with `frame::resize_border(dpi)` first, then title or close as today.
  - `WM_GETMINMAXINFO` sets `ptMinTrackSize` to the minimum size scaled, and `ptMaxTrackSize` to the size of the work area of the box's monitor (`MonitorFromWindow`).
  - `WM_SIZE` calls `relayout`.
- `draw` and `clicked` skip empty chip rects.
- Update `place` to use the window's current size instead of `layout.width`/`height`, so it only positions the box.

- [ ] **Step 4: Run all tests to verify they pass**

Run: `cargo test --locked -- --test-threads=1`
Expected: all pass, including the existing box tests and `src/ui/app.rs`'s capture test.

- [ ] **Step 5: Look at it, then commit**

Run `cargo run --example look` if it shows the capture box; otherwise run the app. Resize from each edge and corner, add three attachments, and check that the text area shrinks and nothing overlaps.

```bash
cargo fmt --all && cargo clippy --locked --all-targets --all-features -- -D warnings
git add src/ui/frame.rs src/ui/capture_box.rs
git commit -m "The capture box resizes from its edges and its text area fills it"
```

---

### Task 4: The box remembers its size and follows DPI changes

**Files:**
- Modify: `src/ui/capture_box.rs` (`Open`, `open`, `place`, `box_proc`, tests' `open_box`)
- Modify: `src/ui/app.rs:612-621` and `:845-855` (pass `settings`)

**Interfaces:**
- Consumes: `DEFAULT_SIZE_AT_96_DPI` and `MIN_SIZE_AT_96_DPI` from Task 3.
- Produces:
  - `Open.settings: Option<PathBuf>`: the path of `fastcapture.ini`; `None` keeps no size
  - `pub fn initial_size(saved: Option<(i32, i32)>, dpi: u32, work: RECT) -> (i32, i32)`
  - private `fn saved_size(settings: Option<&Path>) -> Option<(i32, i32)>` and `fn save_size(hwnd: HWND, settings: Option<&Path>)`, matching `review.rs`'s `saved_place` and `save_place` (`SettingsFile::load`, `value`, `set`, `save`; errors go to `crate::log::line("capture box: …")`)

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_saved_size_is_used_when_it_is_big_enough() {
    let work = RECT { left: 0, top: 0, right: 1920, bottom: 1040 };
    assert_eq!(initial_size(None, 96, work), (720, 420));
    assert_eq!(initial_size(None, 144, work), (1080, 630));
    assert_eq!(initial_size(Some((900, 500)), 96, work), (900, 500));
    assert_eq!(initial_size(Some((300, 500)), 96, work), (720, 420), "too narrow");
}

#[test]
fn a_saved_size_is_clamped_to_the_work_area() {
    let small = RECT { left: 1920, top: 0, right: 3200, bottom: 760 };
    assert_eq!(initial_size(Some((1800, 1000)), 96, small), (1280, 760));
}

#[test]
fn a_resized_box_reopens_at_its_size() {
    let drafts = temp("size");
    let ini = drafts.join("fastcapture.ini");
    std::fs::create_dir_all(&drafts).unwrap();
    let (hwnd, _) = open_box_with(&drafts, Some(&ini));
    let dpi = state(hwnd).unwrap().dpi;
    unsafe {
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, scale(800, dpi), scale(500, dpi),
            SWP_NOMOVE | SWP_NOZORDER);
        SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0);
        DestroyWindow(hwnd);
    }
    let file = SettingsFile::load(&ini);
    assert_eq!(file.value("capture_width"), Some(scale(800, dpi).to_string().as_str()));
    let (hwnd, _) = open_box_with(&drafts, Some(&ini));
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut rect) };
    assert_eq!((rect.right - rect.left, rect.bottom - rect.top), (scale(800, dpi), scale(500, dpi)));
    unsafe { DestroyWindow(hwnd) };
}

#[test]
fn a_dpi_change_rescales_the_box() {
    let drafts = temp("dpi");
    let (hwnd, _) = open_box(&drafts, None);
    let suggested = RECT { left: 0, top: 0, right: 1080, bottom: 630 };
    unsafe { SendMessageW(hwnd, WM_DPICHANGED, (144 << 16 | 144) as usize, &raw const suggested as isize) };
    let capture = state(hwnd).unwrap();
    assert_eq!(capture.dpi, 144);
    assert_eq!(capture.layout.edit.top, scale(34 + 6, 144));
    unsafe { DestroyWindow(hwnd) };
}
```

`open_box_with(draft_dir, settings: Option<&Path>)` is the tests' `open_box` with a settings path. `open_box` calls it with `None`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib ui::capture_box -- --test-threads=1`
Expected: compile errors (`initial_size`, `Open.settings`).

- [ ] **Step 3: Implement**

- `initial_size`: uses `saved` when both values are at least the minimum scaled, otherwise the default scaled. Then it takes `min` with the work area's width and height.
- `place` reads the work area of the monitor under the cursor, calls `initial_size(saved_size(...), dpi, work)`, and centers the box a third of the way down as today.
- `box_proc`:
  - `WM_EXITSIZEMOVE` calls `save_size`. It saves only the size: `capture_width` and `capture_height` from `GetWindowRect`.
  - `WM_DPICHANGED` stores the new dpi, rebuilds `title_font` and `body_font` (deleting the old ones), re-sends `WM_SETFONT` and `EM_SETMARGINS` to the edit, applies the suggested rect with `SetWindowPos`, and calls `relayout`.
- `app.rs`: both `capture_box::Open` sites pass `settings`. The tray path uses `with_app(|app| app.context.settings.clone())`, and the test at :845 passes `None`.

- [ ] **Step 4: Run all tests to verify they pass**

Run: `cargo test --locked -- --test-threads=1`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --locked --all-targets --all-features -- -D warnings
git add src/ui/capture_box.rs src/ui/app.rs
git commit -m "The capture box remembers its size and follows a change of scale"
```

---

### Task 5: Autocorrect in the box, with its setting

**Files:**
- Modify: `src/config/settings.rs` (`Settings.autocorrect: bool`, default `true`, the `"autocorrect"` parse arm, and the starter text lines `"# Fix common typos as you type (English)."` and `"# autocorrect=on"`)
- Modify: `src/ui/settings_form.rs` (`Form.autocorrect`, `from_settings`, `apply` writes `autocorrect` with `bool_text`)
- Modify: `src/ui/settings.rs` (an `ID_AUTOCORRECT` checkbox under `source` at y=446, 360×28; add 32 to every y from 446 to 552 inclusive and to `HEIGHT`; wire it like `controls.source` at :1118 and :1159)
- Modify: `src/ui/capture_box.rs` (`Open.autocorrect: bool`, the `CaptureBox.autocorrect` and `last_fix` fields, `edit_proc`)
- Modify: `src/ui/app.rs` (pass `app.settings.autocorrect`)
- Modify: `README.md` ("The capture box" section)

**Interfaces:**
- Consumes: `spell::with_english` and `Speller::replacement` (Task 1); `autocorrect::{is_boundary, correction, Correction}` (Task 2).
- Produces: `Open.autocorrect: bool`; `#[cfg(test)] pub(crate) fn type_for_test(hwnd: HWND, text: &str)`, which sends `WM_CHAR` for each UTF-16 unit to the edit with `SendMessageW`, so it goes through `edit_proc`.

- [ ] **Step 1: Write the failing tests**

In `settings.rs`, extend `the_starter_file_means_the_defaults` with `assert!(settings.autocorrect);`, and add:

```rust
#[test]
fn autocorrect_can_be_turned_off() {
    assert!(!parse("autocorrect=off\n").0.autocorrect);
}
```

In `settings_form.rs`'s round-trip test, set `cloud.autocorrect = false;` and assert `!settings.autocorrect` after `parse`.

In `capture_box.rs` (each test begins with `if spell::with_english(|_| ()).is_none() { eprintln!("skipped: no en-US spell checker"); return; }`. `open_box` passes `autocorrect: true`, and `open_box_off` passes `false`):

```rust
#[test]
fn a_typed_typo_is_fixed_and_ctrl_z_brings_it_back() {
    let (hwnd, _) = open_box(&temp("fix"), None);
    type_for_test(hwnd, "I think teh ");
    let edit = state(hwnd).unwrap().edit;
    assert_eq!(edit_text(edit), "I think the ");
    press_ctrl_z(edit);
    assert_eq!(edit_text(edit), "I think teh ");
    unsafe { DestroyWindow(hwnd) };
}

#[test]
fn a_word_in_the_middle_is_fixed_in_place() {
    let (hwnd, _) = open_box(&temp("middle"), None);
    let edit = state(hwnd).unwrap().edit;
    set_edit_text(edit, "say teh world");
    unsafe { SendMessageW(edit, EM_SETSEL, 7, 7) };
    type_for_test(hwnd, ",");
    assert_eq!(edit_text(edit), "say the, world");
    unsafe { DestroyWindow(hwnd) };
}

#[test]
fn fixes_land_after_non_ascii_text() {
    let (hwnd, _) = open_box(&temp("unicode"), None);
    type_for_test(hwnd, "café 😀 teh.");
    assert_eq!(edit_text(state(hwnd).unwrap().edit), "café 😀 the.");
    unsafe { DestroyWindow(hwnd) };
}

#[test]
fn ctrl_z_after_more_typing_does_not_bring_the_typo_back() {
    let (hwnd, _) = open_box(&temp("later"), None);
    let edit = state(hwnd).unwrap().edit;
    type_for_test(hwnd, "teh ");
    key_down(edit, u16::from(b'X'));
    type_for_test(hwnd, "x");
    press_ctrl_z(edit);
    assert!(!edit_text(edit).contains("teh"), "{}", edit_text(edit));
    unsafe { DestroyWindow(hwnd) };
}

#[test]
fn pasted_restored_and_switched_off_text_is_left_alone() {
    let drafts = temp("alone");
    let (hwnd, _) = open_box(&drafts, None);
    insert_text(hwnd, "teh ");
    assert_eq!(edit_text(state(hwnd).unwrap().edit), "teh ");
    unsafe { DestroyWindow(hwnd) };
    let (hwnd, _) = open_box_off(&drafts);
    type_for_test(hwnd, "teh ");
    assert_eq!(edit_text(state(hwnd).unwrap().edit), "teh ");
    unsafe { DestroyWindow(hwnd) };
}
```

`press_ctrl_z(edit)` and `key_down(edit, key)` are test helpers: they send `WM_KEYDOWN` (with the Ctrl state set through `SetKeyboardState` for Ctrl+Z) followed by the matching `WM_CHAR` (0x1A for Ctrl+Z), as a real keyboard would.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked -- --test-threads=1 autocorrect`
Expected: compile errors (`Settings.autocorrect`, `Open.autocorrect`, `type_for_test`).

- [ ] **Step 3: Implement**

- Settings, form and checkbox, as listed under **Files**.
- `edit_proc`, `WM_CHAR`, when `capture.autocorrect && is_boundary(ch)`:
  1. Read the caret with `EM_GETSEL`. Only act on an empty selection.
  2. Take the edit's text up to the caret in UTF-16 and convert it with `String::from_utf16_lossy`.
  3. Call `correction(before, |w| spell::with_english(|s| s.replacement(w)).flatten())`.
  4. On `Some(fix)`: `EM_SETSEL(fix.start, caret)`, then `EM_REPLACESEL(0, replacement)`. Pass `0` (not undoable) so the edit's own one-level undo can never bring the typo back; the box's Ctrl+Z is the only way back, and store `last_fix = Some(fix)`. Then fall through to `DefSubclassProc`, so the boundary character is inserted.
- `last_fix` is cleared by:
  - `WM_KEYDOWN` of any key except `VK_CONTROL`, `VK_SHIFT`, `VK_MENU` and Ctrl+Z,
  - `WM_LBUTTONDOWN`,
  - `WM_PASTE`.
- Ctrl+Z (`WM_KEYDOWN` with `'Z'` and Ctrl) with `last_fix` set:
  1. Check that the text at `fix.start` still reads `replacement` followed by one character, and that the caret is right after that character.
  2. If so, select `fix.start..fix.start + replacement_utf16_len` and `EM_REPLACESEL(0, original)`, put the caret back after the boundary character, clear `last_fix`, and set a `swallow_undo_char` flag so the following `WM_CHAR` 0x1A is dropped (the same pattern as `swallow_paste_char`).
  3. Otherwise fall through to the edit's own undo.
- README: in "The capture box", add a sentence saying it resizes from its edges and reopens at the size you left it, and a sentence on autocorrect and the Settings checkbox that turns it off.

- [ ] **Step 4: Run all tests to verify they pass**

Run: `cargo test --locked -- --test-threads=1`
Expected: all pass, with no "skipped" lines on this PC.

- [ ] **Step 5: Try it, then commit**

In the running app, type `I think teh ` (it should become `the `) and press Ctrl+Z (it should go back to `teh `). Untick the checkbox in Settings, open a new box and type `teh ` again (it should stay as typed).

```bash
cargo fmt --all && cargo clippy --locked --all-targets --all-features -- -D warnings
git add src/config/settings.rs src/ui/settings_form.rs src/ui/settings.rs src/ui/capture_box.rs src/ui/app.rs README.md
git commit -m "Fix common English typos as you type in the capture box"
```
