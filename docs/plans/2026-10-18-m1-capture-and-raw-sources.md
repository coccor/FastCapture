# M1: Capture and Raw Sources Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Each part ends with `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passing, and one commit.

**Goal:** FastCapture runs in the tray from sign-in. Ctrl+Alt+Space anywhere opens a capture box. Ctrl+Enter writes the text, pasted, dropped or snipped images and files into the notebook as a raw source in `raw/` with its attachments, and queues a job. `fastcapture add` does the same from scripts and agents. The review window lists the captures. No AI yet: in M1 a capture's "note" is its raw source, and queued jobs stay queued until M2 adds a backend.

**Spec:** `docs/specs/2026-10-18-fastcapture-design.md` §5, §6.1, §6.3, §10.1 (list only), §11, §12, §14, §15 (the rows for capture, conventions and hotkeys).

## Architecture

One crate, `fastcapture`, with a library (everything testable) and a thin `main.rs`.

```
src/
  main.rs            entry: parse the command line, then tray app or `add`
  lib.rs
  error.rs           Error enum, Result alias
  launch.rs          command line: none (tray), `add …`, `--settings`, `--version`
  notebook/          pure notebook conventions, copied from FastPad (see below)
    dates.rs  frontmatter.rs  title.rs  links.rs  notebook_ini.rs  capture_ini.rs  obsidian.rs
  capture/
    model.rs         Capture { text, urls, attachments, source_window, notebook }, kinds
    detect.rs        URL lines, kinds for the footer ("Text", "Link", "Screenshot + file")
    raw.rs           raw source text (front matter + body), raw file name, attachment links
    file.rs          writes attachments then the raw source, all or nothing
    draft.rs         box draft in %LocalAppData%\FastCapture\draft
    ulid.rs          capture_id
  queue/
    job.rs           Job { capture_id, raw, notebook, state, attempts, last_error }, JSON
    store.rs         %LocalAppData%\FastCapture\queue\<id>.json, recovery on start, orphan scan
  config/
    settings.rs      %AppData%\FastCapture\fastcapture.ini: notebook, recent notebooks, hotkeys,
                     capture_source, start_at_sign_in (mode/backend keys reserved for M2)
    keymap.rs        "Ctrl+Alt+Space" <-> (modifiers, vk)
  ipc/
    protocol.rs      Add frame: text, urls, attachment paths, notebook (length-prefixed UTF-16)
    pipe.rs          named pipe server (tray app) and client (`add`), per-user name and ACL
  platform/          copied or adapted from FastPad
    clipboard.rs  png.rs  clock.rs  saver.rs  encoding.rs  ole_drop.rs
    hotkey.rs        RegisterHotKey wrapper
    tray.rs          Shell_NotifyIconW, badge count in the tooltip, menu
    startup.rs       HKCU\Software\Microsoft\Windows\CurrentVersion\Run entry
    instance.rs      per-user mutex
    shell.rs         ShellExecute for ms-screenclip: and "open in editor"
  ui/
    app.rs           hidden message window: tray, hotkeys, pipe messages, owns the windows
    capture_box.rs   the box (§5.2); pure layout and key logic in the first half, as in FastPad
    review.rs        review window: list of raw captures and jobs, read-only text preview
```

### Copied from FastPad

Copied from `C:\Projects\Fastpad` (read-only) with their tests, then trimmed to what FastCapture uses. Each copied file starts with a line naming its source, so the two can be compared later (spec §18.2).

| FastCapture | From FastPad | What's kept |
|---|---|---|
| `notebook/dates.rs` | `src/library/dates.rs` | `Date`, `Time`, `format`, `parse`, `is_daily_format` |
| `notebook/frontmatter.rs` | `src/library/frontmatter.rs` | the reader (used by the review list) |
| `notebook/title.rs` | `src/library/title.rs` | `sanitize_stem`, `free_name`, `is_raster_image_extension` (drop `Language`) |
| `notebook/links.rs` | `src/library/links.rs` | `relative_target` and its percent-encoding only |
| `notebook/notebook_ini.rs` | `src/library/notebook_ini.rs` | `parse`, `NotebookSettings` (read-only: no `starter_text`) |
| `notebook/obsidian.rs` | `src/library/capture.rs` | `obsidian_attachments`, `attachments_folder` |
| `capture/raw.rs` (parts) | `src/library/capture.rs` | `first_words`, `capture_stem`, `unique_name`, `link_lines` |
| `capture/file.rs` (parts) | `src/library/capture_file.rs` | `store`, `save_attachments`, `remove_attachments`, `is_image` |
| `platform/clipboard.rs` | `src/platform/clipboard.rs` | `has_text`, `has_image`, `has_files`, `files`, `image_png` |
| `platform/png.rs` | `src/platform/png.rs` | as is |
| `platform/clock.rs` | `src/platform/clock.rs` | as is |
| `platform/saver.rs` | `src/file/saver.rs` | `save_atomic`, `save_atomic_new`, `is_already_exists` |
| `platform/encoding.rs` | `src/file/encoding.rs` | `decode` (for reading text files and `notebook.ini`) |
| `platform/ole_drop.rs` | `src/platform/ole_drop.rs` | as is |
| `ipc/pipe.rs` (parts) | `src/ipc/{client,server,security}.rs` | pipe naming, ACL, framing pattern |
| `ui/capture_box.rs` (parts) | `src/window/capture_box.rs` | edit control, attachment chips, Snip and clipboard listener, drafts |

### Dependencies

`windows` / `windows-sys` (pinned to FastPad's versions, `=0.62.2` / `=0.61.2`, with only the features used), `serde` + `serde_json` (queue jobs), `pulldown-cmark` `=0.13.4` (front matter options, the review preview in M2). No others in M1. `cargo deny`-style review of the lock file in Part 1.

## Deliberate scope choices for M1

- **No Settings window.** Tray › Settings opens `fastcapture.ini` in the default editor, made first with every key commented out at its default (as FastPad's `notebook.ini` does). A settings window comes with M2, which adds the mode and backend choices.
- **No rich text.** Pasted HTML is kept as plain text; HTML-to-Markdown is M3 (§6.2).
- **URL lines are recorded, not fetched.** The raw source lists them under `urls:`; fetching is M3.
- **The review window shows raw captures only.** It lists jobs with their state and previews the raw source as plain text. Filing, Regenerate and the note preview come with M2 and M4. Actions in M1: **E** open raw source in editor, **Del** recycle the capture (raw source and its own attachments), **Enter** open the notebook folder.
- **No `.fastcapture\capture.ini` writes.** It is read (`raw_folder`, `home_types`) with defaults; nothing creates it.

## Parts

### Part 1: Project skeleton

- [ ] `cargo init --lib` plus `src/main.rs`; edition 2024; `rust-toolchain.toml` pinned to the installed stable (1.99); `[profile.release]` with `lto = "thin"`, `panic = "abort"`, `strip = true`.
- [ ] `#![windows_subsystem = "windows"]` in `main.rs`; an application manifest (per-monitor DPI v2, Common Controls v6, UTF-8 code page) embedded through a `build.rs` with a `.rc` file.
- [ ] `error.rs`; a diagnostic log in `%LocalAppData%\FastCapture\log\` that never receives capture text.
- [ ] `.gitignore` (`/target`), `README.md` (what it is, build, the spec link), MIT `LICENSE` to match FastPad.
- [ ] GitHub Actions workflow: `windows-latest`, fmt, clippy, test, release build for x64 and ARM64 (`aarch64-pc-windows-msvc`, build only).
- **Done when:** `cargo run` starts and exits cleanly, CI is green.

### Part 2: Notebook conventions (pure)

- [ ] Copy the `notebook/` modules (table above) with their tests; remove FastPad-only parts; make them compile with no FastPad types.
- [ ] `capture_ini.rs`: `raw_folder` (default `raw`, must stay inside the notebook, using `notebook_ini`'s `inside_path` rule), `home_types` (comma list, default `topic, project, moc`). Bad value → default plus a `Warning` with its line.
- [ ] `notebook::Conventions::load(root)`: reads `.fastpad\notebook.ini`, `.fastcapture\capture.ini` and `.obsidian\app.json` (each optional), and gives `capture_folder`, `raw_folder`, `attachments_folder_for(note)`, `daily_folder`, `daily_format`, plus warnings.
- **Tests:** copied tests pass; `capture_ini` defaults, bad values and paths that leave the notebook; `Conventions` with no files, FastPad's file only, Obsidian only, and both (notebook.ini wins).

### Part 3: Captures on disk (pure + disk, no window)

- [ ] `capture/model.rs`, `capture/detect.rs`: a line that is only an `http(s)://` URL is a link; several are several links; a URL inside a sentence stays text. Kinds: text, link, screenshot (pasted or snipped image), image (dropped image file), file.
- [ ] `capture/ulid.rs`: 48-bit ms time + 80 random bits (`BCryptGenRandom`), Crockford base32, monotonic within a process.
- [ ] `capture/raw.rs`: raw file name `YYYY-MM-DD HHmm <first words>.md` (`capture` when there's no text, the first URL's host when there are only links); raw source text as in spec §6.1, with `type: raw`, `captured`, `kind`, `capture_id`, `urls` (when any), `source_window` (when recorded, quoted and escaped), then your text, the URL lines and one link line per attachment.
- [ ] `capture/file.rs`: `file_capture(&Capture, &Conventions, now) -> Result<Filed, String>`: copies attachments into the attachments folder (20 MB check returned as `NeedsConfirm` for the box, refused for `add`), then writes the raw source with `save_atomic_new` (never overwrites, ` 2`, ` 3`… on a clash). Any failure removes the files already written.
- [ ] `capture/draft.rs`: save, load and clear the box draft (text plus pasted images as PNGs plus dropped paths).
- **Tests:** detection examples; raw names (text, links only, empty); raw source text for each kind, CRLF-free LF output, escaping in `source_window`; links relative to `raw/` (`../attachments/…`) and with Obsidian's `./` setting; a read-only attachments folder leaves nothing behind; a name clash; draft round trip.

### Part 4: The queue

- [ ] `queue/job.rs`: `Job` with `state: Queued | Working | Done | Failed`, `attempts`, `last_error`, `created`, serialized with `serde_json`.
- [ ] `queue/store.rs`: one file per job, written atomically; `enqueue`, `list`, `set_state`, `remove`; `recover()` on start turns `Working` back into `Queued`; `orphans(root, conventions)` finds raw sources in `raw/` whose `capture_id` has no job (parsed with the front matter reader) and enqueues them.
- [ ] No runner yet: M2 adds the worker that takes jobs. M1 only writes and lists them.
- **Tests:** round trip; recovery; a corrupt job file is moved aside as `.bad` and logged, not fatal; orphan scan finds a raw source written while the app was closed and ignores ones with jobs.

### Part 5: Settings, command line and IPC

- [ ] `config/keymap.rs`: parse and print `Ctrl+Alt+Space`, `Win+Shift+C`, function keys, letters and digits; empty means off.
- [ ] `config/settings.rs`: `fastcapture.ini` keys `notebook`, `recent_notebooks` (up to 10), `capture_hotkey` (default `Ctrl+Alt+Space`), `clipboard_hotkey` (default empty), `capture_source` (default `false`), `start_at_sign_in` (default `true`). Unknown keys kept on write; comments kept. First run: `notebook` defaults to FastPad's most recent notebook from `%AppData%\FastPad\folders.ini` when that file exists, else `Documents\FastPad`, and is written once chosen.
- [ ] `launch.rs`: no arguments → tray app; `add ["text" | -] [--url URL]... [--attach PATH]... [--notebook PATH]`; `--settings`; `--version`. `--attach` paths made absolute against the working folder. Errors → exit code 2 with a message on stderr (attach to the parent console with `AttachConsole(ATTACH_PARENT_PROCESS)`).
- [ ] `ipc/protocol.rs` and `ipc/pipe.rs`: pipe `\\.\pipe\FastCapture-<user SID>`, ACL limited to the current user (FastPad's `security.rs` pattern), one `Add` frame up to 1 MB of text, reply `Ok(raw path)` or `Err(message)`.
- [ ] `add` flow: when the tray app is running, forward and wait for its reply (5 s); otherwise file the capture itself (Part 3) and enqueue it (Part 4), then exit. Exit 0 on success, 1 when filing failed, 2 for bad arguments or text too long.
- **Tests:** keymap; settings parse, defaults, keeping comments and unknown keys; FastPad `folders.ini` pick; `launch` parsing for every form and the errors; frame round trip and the size limit.

### Part 6: Tray app shell

- [ ] `platform/instance.rs`: per-user mutex `Local\FastCapture-<SID>`; a second tray start exits quietly (or, with arguments, forwards them).
- [ ] `ui/app.rs`: hidden message-only window; creates the tray icon, registers hotkeys, runs the pipe server on a thread that posts `WM_APP_ADD` with a boxed request, recovers the queue and runs the orphan scan on a worker after start-up.
- [ ] `platform/tray.rs`: icon (light and dark variants following the taskbar theme), tooltip "FastCapture: n captures queued", left click → review window, right-click menu: Capture…, Capture clipboard, Review (n), Settings, Open notebook folder, Quit. Re-adds itself on `TaskbarCreated` (Explorer restart).
- [ ] `platform/hotkey.rs`: `RegisterHotKey` for `capture_hotkey` and `clipboard_hotkey`; failure → one tray balloon naming the key and `fastcapture.ini`.
- [ ] **Capture clipboard** (hotkey and menu): files the clipboard as a capture with no box (text, image, or Explorer files), then a balloon "Captured from clipboard" or the error.
- [ ] `platform/startup.rs`: keep the `Run` value in step with `start_at_sign_in` on every start.
- [ ] Tray › Settings opens `fastcapture.ini` (created with commented defaults first); the file is re-read when the tray app next gets focus or on `WM_APP_SETTINGS_CHANGED` from a directory watch on `%AppData%\FastCapture`.
- **Done when:** the app sits in the tray at zero CPU, survives an Explorer restart, a second start doesn't make a second icon, and Capture clipboard writes a raw source.

### Part 7: The capture box

- [ ] Port FastPad's box layout and key logic (its first half is pure and tested) and its window code, then change:
  - title "Capture → <notebook name>"; footer shows the detected kinds (Part 3) and the keys;
  - **Ctrl+N** opens a small list of `recent_notebooks` for this capture only;
  - no target cycling (FastCapture always writes a raw source);
  - Ctrl+Enter files through Part 3, enqueues, clears the draft and closes; Esc keeps the draft.
- [ ] Attachments exactly as in FastPad: Ctrl+V image (when no text), Ctrl+Shift+V image anyway, Explorer files, OLE drop, Snip with `ms-screenclip:` and `AddClipboardFormatListener` (30 s timeout), named chips with remove buttons.
- [ ] `capture_source`: when on, the foreground window's title is read when the hotkey fires, before the box takes focus.
- [ ] Box opens on the monitor under the mouse, top-most, follows the system light/dark theme, per-monitor DPI.
- [ ] Accessibility: the window's accessible name is "Capture to <notebook>"; chips are buttons with names; everything works from the keyboard.
- **Tests:** the pure layout and key tests carried over and adapted; a window test that opens the box, sets text, sends Ctrl+Enter and finds the raw source and the job; an image put on the clipboard with `SetClipboardData` and pasted is saved as PNG and linked.

### Part 8: The review window (M1 form)

- [ ] `ui/review.rs`: a resizable window, remembered size and position in `fastcapture.ini`. Left: list of captures (raw sources with jobs) newest first, showing first words, kinds, time and job state (`Queued`, and in later milestones the rest). Right: the raw source as plain text, read-only.
- [ ] Keys: **E** opens the raw source in FastPad (`fastpad.exe` found through `App Paths` or `PATH`) or the default `.md` handler; **Del** sends the raw source and the attachments that only it links to the Recycle Bin (`IFileOperation` with `FOFX_RECYCLEONDELETE`) and removes the job; **Enter** opens the notebook folder; **Esc** closes.
- [ ] The tray tooltip and the window title show the queued count, updated when a capture is filed or deleted.
- [ ] Use a standard ListView (`WC_LISTVIEW`) and a read-only multi-line edit, so keyboard and screen-reader support come from Windows.
- **Tests:** list model (sorting, states, first words) as pure code; a window test that files two captures and sees them listed; Del recycles only attachments no other raw source links to.

### Part 9: Polish, docs and a release build

- [ ] Performance checks against spec §14: hotkey to box visible under 100 ms, Ctrl+Enter to closed under 150 ms with a 4K screenshot (PNG encoded on a worker after the draft is safe), idle memory under 30 MB. A small `--bench` hidden flag or an integration test that times them.
- [ ] README: install, hotkeys, `fastcapture add`, `fastcapture.ini` keys, what M1 does and doesn't do yet.
- [ ] Spec: add a "§19 Implementation notes" section with decisions made while building M1, as FastPad does.
- [ ] A release zip (x64 and ARM64) from CI. An installer (and its "start at sign-in" task) is left for after M2.

## Testing approach

- **Pure modules** (`notebook`, `capture` text, `queue`, `config`, `launch`, `ipc::protocol`, box and review layout): ordinary unit tests, no Windows calls.
- **Disk:** tests in a temporary notebook folder with a scratch `LOCALAPPDATA` and `APPDATA` (environment set per test process; integration tests run with `--test-threads=1`).
- **Windows:** window tests in `tests/` drive the box and the review window through messages (`WM_SETTEXT`, `WM_KEYDOWN` for Ctrl+Enter) and the clipboard, as FastPad's window tests do.

## Not in M1 (by milestone)

- **M2:** settings window; Local and Claude API backends; job runner; written notes; Regenerate; review preview of notes.
- **M3:** rich text, web page fetch and extraction, PDF, docx, logs.
- **M4:** notebook index, links, homes, filing and undo.
- **M5:** OpenAI-compatible cloud backend, token usage and cap.
- **M6:** Claude Code CLI backend.
