# FastCapture

A Windows tray app that captures what you come across (a thought, a screenshot, a web page, a file), keeps the original, and has an AI model, local or cloud, turn it into a well-written Markdown note in your notebook.

FastCapture writes plain Markdown files with YAML front matter and attachments into a notebook folder. [FastPad](https://github.com/coccor/FastPad), Obsidian, any other editor or an AI agent reads them. Neither app needs the other.

**Status:** milestone M2, notes from models. Everything you capture is saved, then written up as a note by the model you choose. Reading web pages and documents, links between notes and filing come in later milestones. See the [design](docs/specs/2026-10-18-fastcapture-design.md), the [M1 plan](docs/plans/2026-10-18-m1-capture-and-raw-sources.md) and the [M2 plan](docs/plans/2026-10-18-m2-notes-from-models.md).

## Capturing

FastCapture runs in the notification area and starts when you sign in.

| Key or action | What it does |
|---|---|
| **Ctrl+Alt+C** (anywhere) | Opens the capture box |
| Tray icon, click | Opens the review window |
| Tray icon, right click | Capture…, Capture clipboard, Review, Pause processing, Settings…, Open settings file, Open notebook folder, Quit |
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

Lists your captures, newest first: what each became (Queued, Writing, Note, Failed…) with the note, or the raw source, beside it. A capture that failed shows why.

| Key | |
|---|---|
| **E** or double-click | Opens the note, or the raw source when there is no note, in FastPad when it is installed, else in your default `.md` editor |
| **R** | Writes the note again with the current settings. If you edited the note since, it asks first. |
| **Del** | Sends the note to the Recycle Bin and keeps the capture; with no note, the capture goes too |
| **Shift+Del** | Sends the note, the capture and the attachments only it links to, to the Recycle Bin |
| **Enter** | Opens the folder |
| **F5** | Refreshes |
| **Esc** | Closes |

## Where things go

In the notebook (FastPad's conventions, read from `.fastpad\notebook.ini` and Obsidian's `.obsidian\app.json` when they exist; FastCapture never writes them):

- `inbox\<title>.md` (`notebook.ini`'s `capture_folder`): the note the model wrote, with the capture's facts in front matter (`captured`, `capture_id`, `source` linking the raw source, `kind`, `tags`, `summary`, `status: inbox`, `model`), then the title, summary, body, key points, tasks and attachments.
- `raw\YYYY-MM-DD HHmm <first words>.md`: each capture's raw source, never changed after it is written. Its front matter holds `type: raw`, `captured`, `kind`, `capture_id`, any `urls` and, when enabled, `source_window`.
- `attachments\`: pasted images (as PNG) and copied files, linked from the raw source.
- `.fastcapture\capture.ini` (optional): `raw_folder` (default `raw`) and `home_types`.

On this PC:

- `%AppData%\FastCapture\fastcapture.ini`: settings (below).
- `%LocalAppData%\FastCapture\queue\`: one file per capture waiting to be processed.
- `%LocalAppData%\FastCapture\draft\`: the capture box's draft.
- `%LocalAppData%\FastCapture\log\fastcapture.log`: diagnostics. It never holds what you captured.

## Settings

Tray menu › **Settings…** chooses how notes are written and the everyday options. **Open settings file** opens `fastcapture.ini` for the rest; changes apply when you save it.

### Local or Cloud

Notes are written by one model, never both:

- **A model on this PC (Local):** an OpenAI-compatible server running here, such as Ollama, LM Studio or Foundry Local. **Find servers** looks for them; choose a model it lists. Only addresses on this PC are accepted, so nothing leaves it. If the model doesn't read images, screenshots are attached to notes unread.
- **Claude (Cloud):** the Claude API with your own API key, from [console.anthropic.com](https://console.anthropic.com) › API Keys. The key is saved in Windows Credential Manager (`FastCapture/claude`), never in a file or the log. Choosing Cloud asks you to confirm once that captures will be sent to Anthropic.

**Test** writes a note from a sample capture with what you chose. Until you choose, captures wait in the queue; nothing is lost. **Pause processing** in the tray menu holds them too. A rejected key or a billing problem pauses processing with a notification; a network error or an overloaded API is retried after 30 seconds, 2 minutes, 10 minutes, then every 30 minutes.

### What is sent

For each capture: the writing rules, what you typed, link addresses (not the pages yet), attachment names (not their contents yet), today's date, the notebook's name and, when `capture_source` is on, the window title. Screenshots and images go too, scaled to at most 1,568 pixels on the long side, up to 8 per capture. In Local mode all of this stays on this PC.

### What Cloud costs

You pay Anthropic per token. A capture is the rules (about 1,500 tokens, read from the cache after the first capture at a tenth of the price), a few hundred tokens of your text, and about 1,600 more for a full-screen screenshot. The note itself is 200–400 tokens. Roughly, per capture:

| Model | Text only | With a screenshot |
|---|---|---|
| Claude Sonnet 5.5 (default) | about 0.4 US cents | about 0.8 cents |
| Claude Opus 5.5 | about 0.8 cents | about 1.5 cents |
| Claude Haiku 5.5 | under 0.05 cents | under 0.1 cents |

Measured on Haiku 5.5: a short text capture used 103 input and 225 output tokens, and the same with a 1200×600 chart 1,037 and 345, besides the cached rules. Prices change; see Anthropic's pricing page.

### The settings file

| Key | Default | |
|---|---|---|
| `notebook` | FastPad's most recent notebook, else `Documents\FastPad` | The folder captures go to |
| `capture_hotkey` | `Ctrl+Alt+C` | Ctrl, Shift, Alt and Win with a letter, digit, F1–F24 or a named key (Space, Enter, Insert…). Empty turns it off. |
| `clipboard_hotkey` | (none) | Captures the clipboard with no box |
| `capture_source` | `false` | Records the title of the window in front when you capture. Off by default because titles can hold private things. |
| `start_at_sign_in` | `true` | Starts FastCapture when you sign in |
| `recent_notebook` | | One line per notebook offered by Ctrl+N |
| `mode` | (none) | `local` or `cloud`, set by Settings |
| `local_endpoint`, `local_model` | | The local server's address and model |
| `claude_model` | `claude-sonnet-5-5` | `claude-opus-5-5` and `claude-haiku-5-5` work too |
| `cloud_accepted` | `false` | Set when you confirm Cloud |

If a hotkey is already taken by another app, a notification says so; pick another key here.

## Privacy

In Local mode FastCapture makes no connections beyond this PC. In Cloud mode it sends what is listed under [What is sent](#what-is-sent) to Anthropic's API over HTTPS, one request per capture, and nothing else. The log records token counts and errors, never what you captured or your key.

## Building

Requires Windows 10 1809 or later, the stable Rust toolchain with the MSVC target, and the Windows 10/11 SDK (for `rc.exe`, which embeds the manifest and icon).

```
cargo build --release
cargo test -- --test-threads=1
cargo run --release --example bench
```

`examples/bench.rs` times the box opening, a 4K screenshot's encoding, Ctrl+Enter and building a model request against their budgets. `examples/review_window.rs <queue folder> [<notebook>]` and `examples/settings_window.rs` open those windows on their own.

`cargo run --release --example live_note -- [--effort low|medium|high] [--image PATH] ["text"]` writes one real note with your settings into a scratch notebook and prints it with the time and the tokens used. **In Cloud mode it costs money.** The tests never reach the internet.

## License

MIT
