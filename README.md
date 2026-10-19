# FastCapture

A Windows tray app that captures what you come across (a thought, a screenshot, a web page, a file), keeps the original, and has an AI model, local or cloud, turn it into a well-written Markdown note in your notebook.

FastCapture writes plain Markdown files with YAML front matter and attachments into a notebook folder. [FastPad](https://github.com/coccor/FastPad), Obsidian, any other editor or an AI agent reads them. Neither app needs the other.

**Status:** milestone M5, the OpenAI-compatible cloud backend and token usage. Everything you capture is saved, the pages it links to and the documents attached to it are read, and the model you choose (on this PC, the Claude API, or any OpenAI-compatible API) writes it up as a note that links the notes it relates to and suggests where it belongs; one key files it there. Settings shows the tokens cloud notes used, and a monthly cap stops them. See the [design](docs/specs/2026-10-18-fastcapture-design.md) and the plans for [M1](docs/plans/2026-10-18-m1-capture-and-raw-sources.md), [M2](docs/plans/2026-10-18-m2-notes-from-models.md), [M3](docs/plans/2026-10-19-m3-extraction.md), [M4](docs/plans/2026-10-19-m4-index-links-filing.md) and [M5](docs/plans/2026-10-19-m5-cloud-openai-usage.md).

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

Lists the notes waiting in your inbox, newest first, each with the home the model suggested (`→ Payments incidents`), and the captures still queued, being written or failed. A capture that failed shows why. The note, or the raw source, is shown beside the list. Notes you file, keep or discard leave the list.

| Key | |
|---|---|
| **Enter** | Files the note in its suggested home (see [Filing](#filing)); with no suggestion, opens the folder |
| **H** | Picks another home: folders and home notes, filtered as you type, the suggestion first. **Enter** files there, **Esc** closes. |
| **I** | Keeps the note in the inbox (`status: kept`) and takes it off the list |
| **Ctrl+Z** | Undoes the last file, keep or discard in this window, unless the files changed since |
| **E** or double-click | Opens the note, or the raw source when there is no note, in FastPad when it is installed, else in your default `.md` editor |
| **R** | Writes the note again with the current settings. If you edited the note since, it asks first. |
| **Del** | Sends the note to the Recycle Bin and keeps the capture; with no note, the capture goes too |
| **Shift+Del** | Sends the note, the capture and the attachments only it links to, to the Recycle Bin |
| **F5** | Refreshes |
| **Esc** | Closes |

The footer shows the keys that apply to the selected row. The tray icon's tooltip counts the notes waiting in the inbox.

## Links and homes

FastCapture keeps a keyword index of the notebook (on this PC, kept current as notes change). For each capture it looks up the notes your text, the pages' titles and the start of the material share the most words with, and offers the 30 best to the model as candidates. The model links the ones the capture really relates to under `## Related`, in the form FastPad's link completion uses, and may link a clearly new subject as a new note. Links to notes that weren't offered are dropped.

The model also suggests a **home**, kept in `suggested_home:`: a **home note** among the candidates, one whose `type:` is in `capture.ini`'s `home_types` (`topic`, `project` and `moc` by default), or a **folder** holding candidates. Search matches words, not meaning: a note that says the same thing in other words won't be found.

### Filing

- **Into a folder:** the note moves there, under a free name (` 2`, ` 3`… when the name is taken). Its links to its attachments are rewritten so they keep working.
- **Under a home note:** the note moves into the home note's folder, and a line is added at the end of the home note, in its list when it ends with one:

  ```markdown
  - 2026-10-19 [[Consumer lag spikes during ledger batch]]: Kafka consumer lag on payments jumps to ~40k…
  ```

- Either way, `status:` becomes `filed` and `suggested_home:` goes; nothing else in the note's front matter changes. A home note that can't be written (it's locked, or gone) leaves the note in the inbox with the reason. Files are written atomically and read again if they change meanwhile, so an open FastPad tab takes the change as an outside edit.

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

- `inbox\<title>.md` (`notebook.ini`'s `capture_folder`): the note the model wrote, with the capture's facts in front matter (`captured`, `capture_id`, `source` linking the raw source, `kind`, `url` and `site` for a capture of one link, `tags`, `summary`, `suggested_home`, `truncated` when something was cut, `status: inbox`, `model`), then the title, summary, body, key points, tasks, related notes (`## Related`), the links captured (`## Source`) and attachments. FastPad's own quick captures may share this folder; FastCapture's notes are the ones with `type: capture`. Filed notes move to their home.
- `raw\YYYY-MM-DD HHmm <first words>.md`: each capture's raw source, never changed after it is written. Its front matter holds `type: raw`, `captured`, `kind`, `capture_id`, any `urls`, `source_url` for pasted web text and, when enabled, `source_window`.
- `raw\<raw source's name>.page.md` (`.page 2.md`, …): each link's page as it was fetched, kept beside its raw source so the note can be written again without fetching it. A failed fetch is recorded there too, and replaced when **R** fetches it again.
- `attachments\`: pasted images (as PNG) and copied files, linked from the raw source.
- `.fastcapture\capture.ini` (optional): `raw_folder` (default `raw`) and `home_types` (default `topic, project, moc`): the `type:` values that make a note a home.

On this PC:

- `%AppData%\FastCapture\fastcapture.ini`: settings (below).
- `%LocalAppData%\FastCapture\queue\`: one file per capture waiting to be processed.
- `%LocalAppData%\FastCapture\draft\`: the capture box's draft.
- `%LocalAppData%\FastCapture\index\<folder key>.cache`: the notebook's index, so it doesn't read every note at each start. It can be deleted at any time.
- `%LocalAppData%\FastCapture\usage.txt`: the tokens cloud notes used, one line per day and model, as the APIs reported them. The last 13 months are kept.
- `%LocalAppData%\FastCapture\log\fastcapture.log`: diagnostics. It never holds what you captured.

## Settings

Tray menu › **Settings…** chooses how notes are written and the everyday options. **Open settings file** opens `fastcapture.ini` for the rest; changes apply when you save it.

### Local or Cloud

Notes are written by one model, never both:

- **A model on this PC (Local):** an OpenAI-compatible server running here, such as Ollama, LM Studio or Foundry Local. **Find servers** looks for them; choose a model it lists. Only addresses on this PC are accepted, so nothing leaves it. If the model doesn't read images, screenshots are attached to notes unread.
- **A cloud API (Cloud)**, with your own API key, from one of two providers:
  - **Claude API:** from [console.anthropic.com](https://console.anthropic.com) › API Keys; choose Sonnet, Opus or Haiku.
  - **OpenAI-compatible API:** any service that answers OpenAI's chat completions: give its base address, a model and your key. **List models** asks the service for its models; you can also type one. For example, `https://api.openai.com/v1` (OpenAI), `https://openrouter.ai/api/v1` (OpenRouter, with models such as `openai/gpt-5-mini`), or `https://<resource>.openai.azure.com/openai/v1` (Azure OpenAI, whose model is your deployment's name). The address must be `https://`. The model must support structured output; a model that can't read images fails captures with screenshots and says so.

  Each key is saved in Windows Credential Manager (`FastCapture/claude`, `FastCapture/openai`), never in a file or the log. Choosing Cloud asks you to confirm what will be sent, and to whom: once for Anthropic, and again whenever the OpenAI-compatible address moves to another host.

**Test** writes a note from a sample capture with what you chose. Until you choose, captures wait in the queue; nothing is lost. **Pause processing** in the tray menu holds them too. A rejected key or a billing problem pauses processing with a notification; a network error or an overloaded API is retried after 30 seconds, 2 minutes, 10 minutes, then every 30 minutes.

### What is sent

For each capture: the writing rules, what you typed, what was read from its links and files (see [What is read](#what-is-read)), today's date, the notebook's name, the page pasted text came from and, when `capture_source` is on, the window title. Screenshots, images and scanned PDF pages go too, up to 8 per capture. So do the candidate notes: for each of up to 30, its name, folder, aliases, tags, type and the first 300 characters of its text; the folders they are in; and the notebook's 50 most used tags. Nothing else from your notebook is sent. In Local mode all of this stays on this PC.

### What Cloud costs

You pay the provider per token. With the Claude API you pay Anthropic. A capture is the rules (about 1,500 tokens, read from the cache after the first capture at a tenth of the price), a few hundred tokens of your text, and about 1,600 more for a full-screen screenshot. The note itself is 200–400 tokens. Roughly, per capture:

| Model | Text only | With a screenshot |
|---|---|---|
| Claude Sonnet 5.5 (default) | about 0.4 US cents | about 0.8 cents |
| Claude Opus 5.5 | about 0.8 cents | about 1.5 cents |
| Claude Haiku 5.5 | under 0.05 cents | under 0.1 cents |

A link or a document adds its text: about one token per four characters, so a typical article (6,000 characters) adds about 1,500 tokens and the 60,000-character limit about 15,000. Candidate notes add about 100 tokens each, so 30 of them about 3,000.

Measured on Haiku 5.5, besides the cached rules: a short text capture used 103 input and 225 output tokens; the same with a 1200×600 chart, 1,037 and 345; a Wikipedia article, 2,475 and 504; a 2-page PDF guide, 1,504 and 648; a short capture with 7 candidate notes, 900 and 391. Prices change; see Anthropic's pricing page.

An OpenAI-compatible model reads the same request, so it takes a similar number of tokens, give or take how its tokenizer counts; the price is the provider's. Whether the rules are read from a cache, and how cheaply, is up to the provider too.

### Tokens used and the cap

Settings shows the tokens cloud notes used today and this month, added up from what each API reported: input (cached included) and output, for both providers together. A capture asked twice (when the first reply wasn't a usable note) counts both requests. Local models are free and aren't counted.

**Token cap** (empty for none) is the most tokens cloud notes may use in a month, like `2M` or `500k`. Cached tokens count in full, so it stops early rather than late. Once the month reaches it, processing pauses with a notification and captures wait in the queue; raising the cap, or the next month, lets them run. A note already being written is finished, so a month can go over by one capture.

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
| `backend` | `claude` | Cloud's provider: `claude` or `openai` (any OpenAI-compatible API) |
| `claude_model` | `claude-sonnet-5-5` | `claude-opus-5-5` and `claude-haiku-5-5` work too |
| `cloud_accepted` | `false` | Set when you confirm Cloud for Anthropic |
| `openai_endpoint`, `openai_model` | | The OpenAI-compatible API's `https://` base address and model |
| `openai_accepted` | | The host you confirmed Cloud for |
| `token_cap` | (none) | The most tokens cloud notes may use in a month: a number, or with `k` or `M` |

If a hotkey is already taken by another app, a notification says so; pick another key here.

## Privacy

In both modes, FastCapture fetches the pages of links you capture: one GET each, with no cookies, so it sees what anyone signed out sees. In Local mode those fetches are its only connections beyond this PC. In Cloud mode it also sends what is listed under [What is sent](#what-is-sent) to the API you chose (Anthropic's, or the OpenAI-compatible one) over HTTPS, one request per capture, and nothing else; the notebook's index stays on this PC, and only the candidates' short descriptions go with a capture. Page content, file text and candidate notes are given to the model as material, never as instructions. The log records token counts, fetch failures and errors, never what you captured or your key.

## Building

Requires Windows 10 1809 or later, the stable Rust toolchain with the MSVC target, and the Windows 10/11 SDK (for `rc.exe`, which embeds the manifest and icon).

```
cargo build --release
cargo test -- --test-threads=1
cargo run --release --example bench
```

`examples/bench.rs` times the box opening, a 4K screenshot's encoding, Ctrl+Enter, building a model request, an article page, a 20-page PDF, and the index of 10,000 notes (built, rebuilt from its cache, searched) against their budgets. `examples/review_window.rs <queue folder> [<notebook>]` and `examples/settings_window.rs` open those windows on their own.

`cargo run --release --example live_note -- [--dry-run] [--effort low|medium|high] [--notebook PATH] [--attach PATH]… ["text"]` writes one real note with your settings (either cloud provider; `--effort` is for the Claude API only) into a scratch notebook and prints it with the time and the tokens used; links in the text are fetched, and `--notebook` offers that notebook's notes as candidates (it is only read). **In Cloud mode it costs money**, except with `--dry-run`, which prints what would be sent and stops. The tests never reach the internet.

Third-party crates and their licenses are listed in [LICENSES.md](LICENSES.md).

## License

MIT
