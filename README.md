# FastCapture

A Windows tray app that captures what you come across (a thought, a screenshot, a web page, a file), keeps the original, and will have an AI model, local or cloud, turn it into a well-written Markdown note in your notebook.

FastCapture writes plain Markdown files with YAML front matter and attachments into a notebook folder. [FastPad](https://github.com/coccor/FastPad), Obsidian, any other editor or an AI agent reads them. Neither app needs the other.

**Status:** milestone M1, capture and raw sources. Everything you capture is saved and queued; turning captures into notes with a model arrives in M2. See the [design](docs/specs/2026-10-18-fastcapture-design.md) and the [M1 plan](docs/plans/2026-10-18-m1-capture-and-raw-sources.md).

## Capturing

FastCapture runs in the notification area and starts when you sign in.

| Key or action | What it does |
|---|---|
| **Ctrl+Alt+C** (anywhere) | Opens the capture box |
| Tray icon, click | Opens the review window |
| Tray icon, right click | Capture…, Capture clipboard, Review, Settings, Open notebook folder, Quit |
| `clipboard_hotkey` (none by default) | Captures the clipboard as it is, with no box |

### The capture box

Type what the capture is about, then add what you want to keep with it:

| Key | |
|---|---|
| **Ctrl+V** | Pastes text, or attaches an image (when the clipboard holds no text) or files copied in Explorer |
| **Ctrl+Shift+V** | Attaches the clipboard's image even when it also holds text |
| **Ctrl+Shift+S** or **+ Snip** | Hides the box and starts Windows' snipping tool; the snip is attached when the box comes back |
| Drop files on the box | Attaches them (copied, never moved; files over 20 MB ask first) |
| **Ctrl+N** | Sends this capture to another recent notebook |
| **Tab** | Moves to the attachments: arrows to choose, **Delete** to remove |
| **Ctrl+Enter** | Saves and closes |
| **Esc** | Closes, keeping what you wrote as a draft for next time |

A line holding only an `http(s)://` address is recorded as a link. The footer shows what the capture holds ("Text + screenshot").

### `fastcapture add`

The same from scripts and agents, with no window:

```
fastcapture add "lag spikes during ledger batch??" --attach C:\logs\ledger.log
fastcapture add --url https://example.com/article "why this matters"
echo "from a pipe" | fastcapture add -
fastcapture add "elsewhere" --notebook D:\Work
```

It prints the raw source's path. Exit codes: 0 saved, 1 saving failed, 2 bad arguments, text over 1 MB, a missing file or a file over 20 MB. When the tray app is running, the capture goes through it; otherwise `add` saves it and the tray app finds it on its next start.

### The review window

Lists the captures waiting to become notes, newest first, with the raw source beside them.

| Key | |
|---|---|
| **E** or double-click | Opens the raw source in FastPad when it is installed, else in your default `.md` editor |
| **Del** | Sends the capture, and the attachments only it links to, to the Recycle Bin |
| **Enter** | Opens the folder |
| **F5** | Refreshes |
| **Esc** | Closes |

## Where things go

In the notebook (FastPad's conventions, read from `.fastpad\notebook.ini` and Obsidian's `.obsidian\app.json` when they exist; FastCapture never writes them):

- `raw\YYYY-MM-DD HHmm <first words>.md`: each capture's raw source, never changed after it is written. Its front matter holds `type: raw`, `captured`, `kind`, `capture_id`, any `urls` and, when enabled, `source_window`.
- `attachments\`: pasted images (as PNG) and copied files, linked from the raw source.
- `.fastcapture\capture.ini` (optional): `raw_folder` (default `raw`) and `home_types`.

On this PC:

- `%AppData%\FastCapture\fastcapture.ini`: settings (below).
- `%LocalAppData%\FastCapture\queue\`: one file per capture waiting to be processed.
- `%LocalAppData%\FastCapture\draft\`: the capture box's draft.
- `%LocalAppData%\FastCapture\log\fastcapture.log`: diagnostics. It never holds what you captured.

## Settings

Tray menu › Settings opens `fastcapture.ini`. Changes apply when you save it.

| Key | Default | |
|---|---|---|
| `notebook` | FastPad's most recent notebook, else `Documents\FastPad` | The folder captures go to |
| `capture_hotkey` | `Ctrl+Alt+C` | Ctrl, Shift, Alt and Win with a letter, digit, F1–F24 or a named key (Space, Enter, Insert…). Empty turns it off. |
| `clipboard_hotkey` | (none) | Captures the clipboard with no box |
| `capture_source` | `false` | Records the title of the window in front when you capture. Off by default because titles can hold private things. |
| `start_at_sign_in` | `true` | Starts FastCapture when you sign in |
| `recent_notebook` | | One line per notebook offered by Ctrl+N |

If a hotkey is already taken by another app, a notification says so; pick another key here.

## Privacy

M1 makes no network connections and sends nothing anywhere. Model processing, local or cloud as you choose, arrives in M2.

## Building

Requires Windows 10 1809 or later, the stable Rust toolchain with the MSVC target, and the Windows 10/11 SDK (for `rc.exe`, which embeds the manifest and icon).

```
cargo build --release
cargo test -- --test-threads=1
cargo run --release --example bench
```

`examples/bench.rs` times the box opening, a 4K screenshot's encoding and Ctrl+Enter against M1's budgets. `examples/review_window.rs <queue folder> [<notebook>]` opens the review window on its own.

## License

MIT
