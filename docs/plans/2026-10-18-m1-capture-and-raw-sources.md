# M1: Capture and Raw Sources Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Each part ends with `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passing, and one commit.

**Goal:** FastCapture runs in the tray from sign-in. Ctrl+Alt+C anywhere opens a capture box. Ctrl+Enter writes the text, pasted, dropped or snipped images and files into the notebook as a raw source in `raw/` with its attachments, and queues a job. `fastcapture add` does the same from scripts and agents. The review window lists the captures. No AI yet: in M1 a capture's "note" is its raw source, and queued jobs stay queued until M2 adds a backend.

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

`windows` / `windows-sys` (pinned to FastPad's versions, `=0.62.2` / `=0.61.2`, with only the features used), `serde` + `serde_json` (queue jobs), `pulldown-cmark` `=0.13.4` (front matter options, the review preview in M2). No others in M1. Each part adds only the dependencies it uses; Part 1 has none.

## Deliberate scope choices for M1

- **No Settings window.** Tray › Settings opens `fastcapture.ini` in the default editor, made first with every key commented out at its default (as FastPad's `notebook.ini` does). A settings window comes with M2, which adds the mode and backend choices.
- **No rich text.** Pasted HTML is kept as plain text; HTML-to-Markdown is M3 (§6.2).
- **URL lines are recorded, not fetched.** The raw source lists them under `urls:`; fetching is M3.
- **The review window shows raw captures only.** It lists jobs with their state and previews the raw source as plain text. Filing, Regenerate and the note preview come with M2 and M4. Actions in M1: **E** open raw source in editor, **Del** recycle the capture (raw source and its own attachments), **Enter** open the notebook folder.
- **No `.fastcapture\capture.ini` writes.** It is read (`raw_folder`, `home_types`) with defaults; nothing creates it.

## Parts

### Part 1: Project skeleton

- [x] `cargo init --lib` plus `src/main.rs`; edition 2024; `rust-toolchain.toml` on the stable channel, as FastPad has it; `[profile.release]` with `lto = "thin"`, `panic = "abort"`, `strip = true`.
- [x] `#![windows_subsystem = "windows"]` in `main.rs`; an application manifest (per-monitor DPI v2, Common Controls v6, UTF-8 code page) embedded as `RT_MANIFEST` by a `build.rs` that runs `rc.exe`, adapted from FastPad's icon script.
- [x] `error.rs`; a diagnostic log in `%LocalAppData%\FastCapture\log\` that never receives capture text.
- [x] `.gitignore` (`/target`), `README.md` (what it is, build, the spec link), MIT `LICENSE` to match FastPad.
- [x] GitHub Actions workflow: `windows-latest`, fmt, clippy, test, release build for x64 and ARM64 (`aarch64-pc-windows-msvc`, build only).
- **Done when:** `cargo run` starts and exits cleanly, CI is green. (Local run verified; CI runs once the repo has a GitHub remote.)

### Part 2: Notebook conventions (pure)

- [x] Copy the `notebook/` modules (table above) with their tests; remove FastPad-only parts; make them compile with no FastPad types. (`frontmatter` without `add_alias`; `notebook_ini` without `starter_text`, whose text became a test fixture.)
- [x] `capture_ini.rs`: `raw_folder` (default `raw`, must stay inside the notebook, using `notebook_ini`'s `inside_path` rule), `home_types` (comma list, default `topic, project, moc`). Bad value → default plus a `Warning` with its line.
- [x] `notebook::Conventions::load(root)`: reads `.fastpad\notebook.ini`, `.fastcapture\capture.ini` and `.obsidian\app.json` (each optional), and gives `capture_folder`, `raw_folder`, `attachments_folder_for(note)`, `daily_folder`, `daily_format`, plus warnings naming their file. The settings files are read as UTF-8 or BOM-marked UTF-16 by a small reader in `notebook`; FastPad's full `encoding` module arrives with Part 3, for captured text files.
- **Tests:** copied tests pass; `capture_ini` defaults, bad values and paths that leave the notebook; `Conventions` with no files, FastPad's file only, Obsidian only, and both (notebook.ini wins).

### Part 3: Captures on disk (pure + disk, no window)

- [x] `capture/model.rs`, `capture/detect.rs`: a line that is only an `http(s)://` URL is a link; several are several links; a URL inside a sentence stays text. Kinds: text, link, screenshot (pasted or snipped image), image (dropped image file), file.
- [x] `capture/ulid.rs`: 48-bit ms time + 80 random bits (`BCryptGenRandom`, in `platform/random.rs`), Crockford base32, monotonic within a process.
- [x] `capture/raw.rs`: raw file name `YYYY-MM-DD HHmm <first words>.md` (`capture` when there's no text, the first URL's host when there are only links); raw source text as in spec §6.1, with `type: raw`, `captured`, `kind`, `capture_id`, `urls` (when any), `source_window` (when recorded, quoted and escaped), then your text (URL lines included) and one link line per attachment.
- [x] `capture/file.rs`: `file_capture(&Request) -> Result<Filed, FileError>`, where the request carries the conventions, the capture, its id, the time and `allow_large`. `FileError::TooLarge` (checked before anything is written) lets the box ask and `add` refuse. Attachments are copied first, then the raw source is written with `save_atomic_new` (never overwrites, ` 2`, ` 3`… on a clash). Any failure removes the files already written. `platform/saver.rs` and `platform/clock.rs` are copied from FastPad here.
- [x] `capture/draft.rs`: save, load and clear the box draft (text plus pasted images as PNGs plus dropped paths).
- **Tests:** detection examples; raw names (text, links only, empty); raw source text for each kind, CRLF-free LF output, escaping in `source_window`; links relative to `raw/` (`../attachments/…`) and with Obsidian's `./` setting; a missing attachment and an unwritable raw folder leave nothing behind; a name clash; draft round trip. FastPad's `encoding` module is not needed until M3 reads captured text files, so it isn't copied yet.

### Part 4: The queue

- [x] `queue/job.rs`: `Job` with `state: Queued | Working | Done | Failed`, `attempts`, `last_error`, `created`, serialized with `serde_json`. The JSON is converted by hand through `serde_json::Value` (versioned), so no derive macros are added.
- [x] `queue/store.rs`: one file per job, written atomically; `put`, `get`, `list`, `set_state`, `remove`; `recover()` on start turns `Working` back into `Queued`; `enqueue_orphans(conventions)` finds raw sources in `raw/` whose `capture_id` has no job (parsed with the front matter reader) and enqueues them. Capture ids must be 1–64 letters or digits before they name a file.
- [x] No runner yet: M2 adds the worker that takes jobs. M1 only writes and lists them.
- **Tests:** round trip; recovery; a corrupt job file is moved aside as `.bad` and logged, not fatal; orphan scan finds a raw source written while the app was closed and ignores ones with jobs.

### Part 5: Settings, command line and IPC

- [x] `config/keymap.rs`: parse and print `Ctrl+Alt+Space`, `Win+Shift+C`, function keys, letters, digits and named keys; empty means off. A global hotkey must have a modifier.
- [x] `config/settings.rs`: `fastcapture.ini` keys `notebook`, `recent_notebook` (one line per notebook, up to 10), `capture_hotkey` (default `Ctrl+Alt+C`; first planned as `Ctrl+Alt+Space`, which another app held on the development PC), `clipboard_hotkey` (default empty), `capture_source` (default `false`), `start_at_sign_in` (default `true`). Writes change only the lines they set, so unknown keys and comments are kept; `review_*`, `mode*` and `backend*` keys are reserved for later and never warned about. First run: `notebook` defaults to FastPad's most recent existing notebook from `%LocalAppData%\FastPad\folders.ini` (that is where FastPad keeps it, not `%AppData%`), else `Documents\FastPad` (the known folder), and is written once chosen.
- [x] `launch.rs`: no arguments → tray app; `add ["text" | -] [--url URL]... [--attach PATH]... [--notebook PATH]`; `--settings`; `--version`; `--help`. `--attach` and `--notebook` paths made absolute against the working folder. Errors → exit code 2 with a message on stderr (`platform/console.rs` attaches to the parent console with `AttachConsole(ATTACH_PARENT_PROCESS)` unless output is redirected).
- [x] `ipc/protocol.rs` and `ipc/pipe.rs`: pipe `\\.\pipe\FastCapture-<session>` as FastPad names its own (the server is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, and the client checks the server's user and session before sending, so a squatter gets nothing); ACL limited to the current user (FastPad's `security.rs`, copied); duplex, one `Add` request per connection, reply `Filed(raw path)` or `Failed { exit_code, message }`. Frames carry UTF-16, so any Windows path survives.
- [x] `add` flow (`add.rs`): when the tray app is running, forward and wait for its reply (5 s); otherwise file the capture itself (Part 3) and enqueue it (Part 4), then exit. Exit 0 on success, 1 when filing failed, 2 for bad arguments, text too long, a missing file or a file over 20 MB. `add::handle` is what the tray app will answer forwarded captures with.
- [x] `--settings` writes the starter `fastcapture.ini` when there is none and opens it (`platform/shell.rs`).
- **Tests:** keymap; settings parse, defaults, keeping comments and unknown keys; FastPad `folders.ini` pick; `launch` parsing for every form and the errors; frame round trip and the size limit.

### Part 6: Tray app shell

- [x] `platform/instance.rs`: mutex `Local\FastCapture` (`Local\` is already per session, as FastPad's is); a second tray start exits quietly. Arguments never reach a second tray start: `add` forwards over the pipe itself.
- [x] `ui/app.rs`: a hidden top-level tool window (not message-only: only top-level windows get `TaskbarCreated`); creates the tray icon, registers hotkeys, runs the pipe server on a thread that answers with `add::handle` and posts `WM_APP_QUEUE`, recovers the queue and runs the orphan scan on a worker after start-up. The app state lives in a thread-local, borrowed briefly per message, never while the menu's own message loop runs.
- [x] `platform/tray.rs`: one icon drawn in code (`src/icon.rs`, also embedded in the exe by `build.rs`): a blue square with a white mark, which reads on light and dark taskbars alike, so there are no theme variants. Tooltip "FastCapture → <notebook>" and the queued and failed counts. Right-click menu: Capture…, Capture clipboard, Review (n), Settings, Open notebook folder, Quit. Re-adds itself on `TaskbarCreated`, and removes itself before the window goes. **Until Parts 7 and 8:** Capture… and Review are greyed out, and a left click opens the raw folder.
- [x] `platform/hotkey.rs`: `RegisterHotKey` for `capture_hotkey` and `clipboard_hotkey`; failure → one tray balloon naming the key and `fastcapture.ini`, not repeated for the same key.
- [x] **Capture clipboard** (`ui/clipboard_capture.rs`, hotkey and menu): files the clipboard as a capture with no box (Explorer files first, then an image when there's no text, then text), then a balloon "Captured from clipboard" or the error. The window title is recorded for the hotkey (when `capture_source` is on), never for the menu, where the taskbar is in front. `capture/submit.rs` files and queues; `add` uses it too.
- [x] `platform/startup.rs`: keep the `Run` value in step with `start_at_sign_in` on every start and settings change.
- [x] Tray › Settings opens `fastcapture.ini` (created with commented defaults first); `platform/watch.rs` watches `%AppData%\FastCapture` and the app re-reads the file on `WM_APP_SETTINGS`. There is no re-read on focus: the tray app has no focus to get.
- **Done when:** the app sits in the tray at zero CPU, survives an Explorer restart, a second start doesn't make a second icon, and Capture clipboard writes a raw source. **Checked:** start, a second start exiting, `add` answered by the tray app over the pipe, a clean exit on `WM_CLOSE`. Capture clipboard is covered by tests through `from_clipboard` and `submit`, not by pressing the hotkey. Not checked by hand: the icon's look and an Explorer restart. **Found:** on this PC `Ctrl+Alt+Space` is already registered by another app (`ERROR_HOTKEY_ALREADY_REGISTERED`), which is spec open question 1. The default is now `Ctrl+Alt+C` (free here, as was `Ctrl+Shift+Space`; `Win+Shift+C` was taken too).

### Part 7: The capture box

- [x] Port FastPad's box layout and key logic (its first half is pure and tested) and its window code (`ui/capture_box.rs`), then change:
  - title "Capture → <notebook name>"; footer shows the detected kinds (Part 3) and the keys, and follows typing;
  - **Ctrl+N** opens a menu of `recent_notebooks` (the current one checked) for this capture only;
  - no target cycling (FastCapture always writes a raw source), and no Ctrl+Shift+Enter (there is no editor to open);
  - Ctrl+Enter files through `capture::submit`, enqueues, clears the draft and closes; Esc keeps the draft. The capture hotkey while the box is open brings it forward, or back from a snip.
- [x] Attachments as in FastPad: Ctrl+V image (when no text), Ctrl+Shift+V image anyway, Explorer files, file drops, Snip with `ms-screenclip:` and `AddClipboardFormatListener` (30 s timeout), named chips with remove buttons. **Changed:** drops arrive through `WM_DROPFILES` (`DragAcceptFiles`) instead of FastPad's OLE drop target, and the 20 MB question is a Yes/No message box instead of FastPad's own prompt.
- [x] `capture_source`: when on, the foreground window's title is read when the hotkey fires, before the box takes focus. The tray menu's Capture… records none, since the taskbar is in front.
- [x] Box opens on the monitor under the mouse, top-most, follows the Windows app light/dark setting (`ui/theme.rs`, a small stand-in for FastPad's palette and type ramp: Segoe UI, two palettes), per-monitor DPI. High-contrast colors are not handled yet.
- [x] Accessibility: the window's accessible name is "Capture to <notebook>"; everything works from the keyboard (Tab to the strip, arrows, Delete, Enter or Space on Snip). **Not done:** chips are drawn, not child buttons, as in FastPad, so a screen reader doesn't announce them one by one.
- **Tests:** the pure layout and key tests carried over and adapted; window tests for Ctrl+Enter, Esc and the draft coming back, a failure staying in the box, folders and the 20 MB question, and a snip ended by the hotkey; and `ui::app`'s end-to-end test: an image put on the clipboard with `SetClipboardData` is pasted into the real box, text typed, Ctrl+Enter, and the raw source, the PNG and the job are all there. **Checked by hand:** `Ctrl+Alt+C` opens the box over other apps (screenshot), Esc closes it.

### Part 8: The review window (M1 form)

- [x] `ui/review.rs`: a resizable window, remembered size and position in `fastcapture.ini` (`review_x`, `review_y`, `review_width`, `review_height`). Left: list of captures (raw sources with jobs) newest first, showing the first line (or the first file's name), kinds, time and job state. Right: the raw source as plain text, read-only, with a failed job's error above it. The list model is `ui/review_list.rs`, and reading a raw source back is `capture/read.rs`. Jobs whose raw source is gone are removed when the list loads.
- [x] Keys: **E** or a double-click opens the raw source in FastPad (found through its `App Paths` entry, per user then per machine; not `PATH`) or the default `.md` handler; **Del** sends the raw source and the attachments that only it links to the Recycle Bin (`SHFileOperationW` with `FOF_ALLOWUNDO`, which needs no COM, instead of `IFileOperation`) and removes the job; **Enter** opens the selected capture's raw folder (the notebook's when nothing is selected); **F5** refreshes; **Esc** closes. Tab, Enter and Esc go through `IsDialogMessageW` in the app's message loop.
- [x] The tray tooltip and the window title show the queued count, updated when a capture is filed or deleted. The tray icon's click and the menu's Review open the window.
- [x] Use a standard ListView (`SysListView32`) and a read-only multi-line edit, so keyboard and screen-reader support come from Windows. Columns share the list's width, so no horizontal scroll bar shows. In dark mode the list and edit take the dark colors; the column header stays light.
- **Tests:** list model (sorting, states, first lines, times) as pure code; reading raw sources back (titles, kinds, attachment links, links that leave the notebook); a window test that files two captures, sees them listed, opens one, and recycles each in turn: an image another raw source links to stays until that one goes too; the window's place round-trips through the settings. **Checked by hand:** the window with two captures (screenshots), through `examples/review_window.rs`, since the sandboxed shell can't reach the tray app's window to click its icon.

### Part 9: Polish, docs and a release build

- [x] Performance checks against spec §14, with `examples/bench.rs` (release build) rather than a hidden flag: hotkey to box visible 61 ms (budget 100), Ctrl+Enter with a 4K screenshot 34 ms (150), idle tray app 11.6 MB working set and no CPU (30 MB). **Found and fixed:** a pasted 4K *bitmap* took about 1.2 s to encode on the UI thread. It is now encoded on a worker thread behind a "Preparing image…" chip, and Ctrl+Enter files once it's ready. Images on the clipboard as `PNG` need no encoding.
- [x] README: hotkeys, the capture box's keys, `fastcapture add`, the review window, where files go, `fastcapture.ini` keys, what M1 does and doesn't do yet, building.
- [x] Spec: a "§19 Implementation notes" section with decisions made while building M1, as FastPad does.
- [x] A release zip (x64 and ARM64) with a SHA-256 file, from CI's `package` job. Checked locally for x64; CI hasn't run because the repo has no GitHub remote yet. An installer (and its "start at sign-in" task) is left for after M2.

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
