# FastCapture

A Windows tray app that captures what you come across (a thought, a screenshot, a web page, a file), keeps the original, and has an AI model, local or cloud, turn it into a well-written Markdown note in your notebook.

FastCapture writes plain Markdown files with YAML front matter and attachments into a notebook folder. [FastPad](https://github.com/coccor/FastPad), Obsidian, any other editor or an AI agent reads them. Neither app needs the other.

**Status:** milestone M3, extraction. Everything you capture is saved, the pages it links to and the documents attached to it are read, and the model you choose writes it up as a note. Links between notes and filing come in a later milestone. See the [design](docs/specs/2026-10-18-fastcapture-design.md) and the plans for [M1](docs/plans/2026-10-18-m1-capture-and-raw-sources.md), [M2](docs/plans/2026-10-18-m2-notes-from-models.md) and [M3](docs/plans/2026-10-19-m3-extraction.md).

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
| **Ctrl+V** | Pastes text, or attaches an image (when the clipboard holds no text) or files copied in Explorer. Text copied from a browser or Word with headings, lists, links, tables or code is pasted as Markdown. |
| **Ctrl+Alt+V** | Pastes the plain text, even when the clipboard holds formatted text |
| **Ctrl+Shift+V** | Attaches the clipboard's image even when it also holds text |
| **Ctrl+Shift+S** or **+ Snip** | Hides the box and starts Windows' snipping tool; the snip is attached when the box comes back |
| Drop files on the box | Attaches them (copied, never moved; files over 20 MB ask first) |
| **Ctrl+N** | Sends this capture to another recent notebook |
| **Tab** | Moves to the attachments: arrows to choose, **Delete** to remove |
| **Ctrl+Enter** | Saves and closes |
| **Esc** | Closes, keeping what you wrote as a draft for next time |

A line holding only an `http(s)://` address is recorded as a link. The footer shows what the capture holds ("Text + screenshot"). When pasted text came from a web page, the browser's note of that page is kept with it (`source_url`). The clipboard hotkey pastes the same way.

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

## What is read

Before a note is written, FastCapture reads what you captured, on this PC, with no model:

| Captured | What the model gets |
|---|---|
| Your text | All of it |
| A link | The page's main content as Markdown (navigation, ads, comments and footers left out), with its title, site and date. Fetched once, as a browser would but without cookies, within 10 s and 5 MB; at most 3 links per capture. |
| A PDF | The text of each page. A page with almost no text (a scan) is rendered and sent as an image. |
| A Word `.docx` | Its text, with headings, lists and tables |
| Text, logs, code | Their text, in any common encoding. A log over 400 lines keeps its first and last 200. |
| Screenshots and images | The images, scaled to at most 1,568 pixels on the long side |
| Other files | Their name, type and size |

Limits, so one capture can't cost much: 60,000 characters of material (your text doesn't count), 20 PDF pages and 8 images. Whatever is cut is named in the note's `truncated:` front matter. A page that can't be fetched (a 403, a site that needs JavaScript or a login) is noted, and the note is written from your text; a page that can't be reached yet (offline) is tried again later, three times at most.

## Where things go

In the notebook (FastPad's conventions, read from `.fastpad\notebook.ini` and Obsidian's `.obsidian\app.json` when they exist; FastCapture never writes them):

- `inbox\<title>.md` (`notebook.ini`'s `capture_folder`): the note the model wrote, with the capture's facts in front matter (`captured`, `capture_id`, `source` linking the raw source, `kind`, `url` and `site` for a capture of one link, `tags`, `summary`, `truncated` when something was cut, `status: inbox`, `model`), then the title, summary, body, key points, tasks, the links captured (`## Source`) and attachments.
- `raw\YYYY-MM-DD HHmm <first words>.md`: each capture's raw source, never changed after it is written. Its front matter holds `type: raw`, `captured`, `kind`, `capture_id`, any `urls`, `source_url` for pasted web text and, when enabled, `source_window`.
- `raw\<raw source's name>.page.md` (`.page 2.md`, …): each link's page as it was fetched, kept beside its raw source so the note can be written again without fetching it. A failed fetch is recorded there too, and replaced when **R** fetches it again.
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

For each capture: the writing rules, what you typed, what was read from its links and files (see [What is read](#what-is-read)), today's date, the notebook's name, the page pasted text came from and, when `capture_source` is on, the window title. Screenshots, images and scanned PDF pages go too, up to 8 per capture. In Local mode all of this stays on this PC.

### What Cloud costs

You pay Anthropic per token. A capture is the rules (about 1,500 tokens, read from the cache after the first capture at a tenth of the price), a few hundred tokens of your text, and about 1,600 more for a full-screen screenshot. The note itself is 200–400 tokens. Roughly, per capture:

| Model | Text only | With a screenshot |
|---|---|---|
| Claude Sonnet 5.5 (default) | about 0.4 US cents | about 0.8 cents |
| Claude Opus 5.5 | about 0.8 cents | about 1.5 cents |
| Claude Haiku 5.5 | under 0.05 cents | under 0.1 cents |

A link or a document adds its text: about one token per four characters, so a typical article (6,000 characters) adds about 1,500 tokens and the 60,000-character limit about 15,000.

Measured on Haiku 5.5, besides the cached rules: a short text capture used 103 input and 225 output tokens; the same with a 1200×600 chart, 1,037 and 345; a Wikipedia article, 2,475 and 504; a 2-page PDF guide, 1,504 and 648. Prices change; see Anthropic's pricing page.

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

In both modes, FastCapture fetches the pages of links you capture: one GET each, with no cookies, so it sees what anyone signed out sees. In Local mode those fetches are its only connections beyond this PC. In Cloud mode it also sends what is listed under [What is sent](#what-is-sent) to Anthropic's API over HTTPS, one request per capture, and nothing else. Page content and file text are given to the model as material to summarise, never as instructions. The log records token counts, fetch failures and errors, never what you captured or your key.

## Building

Requires Windows 10 1809 or later, the stable Rust toolchain with the MSVC target, and the Windows 10/11 SDK (for `rc.exe`, which embeds the manifest and icon).

```
cargo build --release
cargo test -- --test-threads=1
cargo run --release --example bench
```

`examples/bench.rs` times the box opening, a 4K screenshot's encoding, Ctrl+Enter, building a model request, an article page and a 20-page PDF against their budgets. `examples/review_window.rs <queue folder> [<notebook>]` and `examples/settings_window.rs` open those windows on their own.

`cargo run --release --example live_note -- [--dry-run] [--effort low|medium|high] [--attach PATH]… ["text"]` writes one real note with your settings into a scratch notebook and prints it with the time and the tokens used; links in the text are fetched. **In Cloud mode it costs money**, except with `--dry-run`, which prints what would be sent and stops. The tests never reach the internet.

Third-party crates and their licenses are listed in [LICENSES.md](LICENSES.md).

## License

MIT
