# FastCapture Design

Status: Draft design, pending review  
Date: 18 October 2026  
Related: FastPad's `docs/superpowers/specs/2026-10-16-second-brain-phase-1-design.md` ("the FastPad spec"), whose notebook conventions (§9.1), quick capture (§10) and attachments (§10.4) this product builds on.

## 1. Purpose

FastCapture is a Windows tray app that takes in what you come across during the day (a thought, a screenshot, a web page, a PDF), keeps the original, and has an AI model turn it into a well-written Markdown note in your notebook.

FastPad stays the editor. The two meet only through files in a notebook folder: FastCapture writes plain Markdown with YAML front matter and attachments, and FastPad, Obsidian, any other editor or an AI agent reads them. Neither app needs the other to be installed.

The lasting rules:

- **Capturing never waits for a model.** The original is on disk before any AI work starts, so nothing is lost when you're offline, a model is slow, or a request fails.
- **Originals are never changed.** Every note can be regenerated from its raw source, by FastCapture or by an agent.
- **One model provider at a time, chosen by you.** In Local mode nothing leaves the PC except page fetches for links you capture. In Cloud mode every model call goes to the provider you chose. The app never mixes the two or falls back from one to the other.
- **Notes are plain files that follow the notebook's conventions.** Everything FastCapture knows about a note is in the note's text, or in a per-PC cache it can rebuild.
- **Nothing is filed into your notebook's structure without you.** New notes land in the inbox. The AI suggests where each one belongs, and you accept the suggestion with one key.

Success looks like this: you press the hotkey over a Grafana dashboard, snip the spike and type "lag spikes during ledger batch??", then press Ctrl+Enter. The box is gone in under a second. A minute later the tray shows one note waiting. It has a proper title, a summary, the numbers read off the chart, a task, links to `[[Payments incidents]]` and `[[Ledger batch]]`, and the suggestion "file under Payments incidents". You press Enter, and the note is filed.

## 2. Decisions

| Topic | Decision |
|---|---|
| Platform | Windows 10 1809+ and Windows 11, x64 and ARM64. Windows only. |
| Stack | Rust + Win32 (`windows`/`windows-sys`), the same approach as FastPad. Own small controls, Direct2D/DirectWrite where needed. |
| Inputs in v1 | Text and clipboard (including rich text), screenshots and pasted images, links (web pages), files (PDF, Word `.docx`, text, logs and code). Voice and handwriting photos are later (§17). |
| Presence | A tray app that starts at sign-in, with a global capture hotkey. The tray icon shows how many notes are waiting for review. |
| Flow | Save raw now, refine in the background. The capture box closes at once; a queue does the model work. |
| On disk | The **raw source** goes to `raw/` and is never changed afterwards. The **written note** goes to the notebook's capture (inbox) folder and links back to its source. Images and files go to the notebook's attachments folder. |
| Note shape | A fixed structure (§7.2): front matter, title, summary, your text cleaned up in your own voice, key points, tasks, related links, attachments. |
| Filing | Notes land in the inbox with a `suggested_home`. The review window accepts it with one key, changes it, or leaves the note in the inbox. |
| Models | A single global **Mode** setting, Local or Cloud, never both. Local: an OpenAI-compatible server on this PC (Ollama, LM Studio, Foundry Local). Cloud: the Claude API, any OpenAI-compatible cloud endpoint, or (experimental) your own installed Claude Code CLI, which uses your Claude subscription. |
| Model output | The model returns JSON matching a schema. FastCapture checks it and writes the Markdown itself, so every note has the same shape, whatever the model. |
| Linking | A local keyword index of the notebook finds about 30 candidate notes for each capture. The model may link only to those, or suggest new notes, which are shown as such. |
| Web pages | FastCapture fetches the page itself (no cookies, no JavaScript), extracts the main text locally, and stores it in the raw source. |
| Conventions | Reads FastPad's `.fastpad\notebook.ini` (read-only) and Obsidian's attachment folder. FastCapture's own keys live in `.fastcapture\capture.ini` in the notebook. |
| Notebook | One capture notebook, set in Settings. The capture box can send a single capture to another notebook. |
| Review | A review window: list of new notes, a read-only preview, the raw source beside it. Editing opens the note in FastPad or the default `.md` editor. FastCapture has no editor. |
| Secrets | API keys in Windows Credential Manager. FastCapture never reads or stores Claude.ai credentials. |

Rejected alternatives:

- **Review in the capture box before saving.** Each capture would wait 5–60 seconds for a model, and a failure would lose the moment. Background refinement keeps capture instant.
- **Raw original at the bottom of the note, or the note rewritten in place.** Mixing the two makes the note longer for every reader, and rewriting in place loses the original. A separate, unchanged raw file is what agent-maintained vaults use (`raw/` and `wiki/`), and it makes "regenerate" safe.
- **Automatic filing.** An AI moving files around the notebook surprises the user and fights agents working in the same folders. A one-key accept is nearly as fast.
- **Local extraction with cloud writing, or a cloud fallback when local fails.** The user wants one clear answer to "where does my data go": the provider chosen in Settings, for everything.
- **Embedding the notebook for semantic search.** In Cloud mode every note would have to be sent to the cloud to be embedded, and Local mode would need a second model. Keyword search plus the model's judgement is enough to pick links.
- **A browser extension for pages.** Better content behind logins, but a second product to build and publish. Pasting the page's text with the link covers v1.
- **Writing FastCapture keys into `notebook.ini`.** FastPad warns about unknown keys there, and the apps would then have to be released in step.
- **Signing in with a Claude.ai account inside FastCapture.** Anthropic doesn't allow third-party apps to offer Claude.ai login or to use Free, Pro or Max plan credentials ([legal and compliance](https://code.claude.com/docs/en/legal-and-compliance), [Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)). Running the user's own Claude Code CLI is the supported way to use a subscription (§8.4).
- **C# + WinUI 3, or Tauri.** Both start slower and use more memory for an app that sits in the tray all day, and neither could reuse FastPad's notebook code.

## 3. Relation to FastPad

- **Same notebook conventions.** FastCapture reads `notebook.ini`'s `capture_folder`, `attachments_folder`, `daily_folder` and `daily_format`, with FastPad's defaults when the file is missing (`inbox`, `attachments`, `journal`, `YYYY-MM-DD`). It reads Obsidian's `attachmentFolderPath` the same way FastPad does (FastPad spec §9.1).
- **FastPad's quick capture stays.** It is the fast, AI-free way to jot a line into the inbox or daily note. FastCapture is for things worth turning into a note. They use different hotkeys (§5.1).
- **FastPad sees FastCapture's work as outside changes.** Its watcher marks new notes in the tree, its index picks up their `[[links]]` and tags, and the "Missing notes" list shows the new notes the model suggested.
- **Moving notes.** When FastCapture files a note (§10.3), FastPad sees a rename outside FastPad. Links to the note are wikilinks by name, so they keep resolving, and FastCapture rewrites the moved note's own relative attachment links before the move, as FastPad does.
- **Shared code.** v1 copies the small pure modules it needs from FastPad (date formats, front matter reading, `notebook.ini` parsing, file naming and attachment links) into FastCapture's own `notebook` module, with their tests. Extracting a shared crate is an open question (§18), because it would add a build dependency between the two repos.
- **Optional integration.** "Open in editor" uses `fastpad.exe <path>` when FastPad is installed, and otherwise the default handler for `.md`.

## 4. Goals and non-goals

### Goals

- From anywhere in Windows, capture text, the clipboard, a screenshot, a link or files in under a second, and get back to work.
- Every capture produces a raw source file at once, and a written note within a minute in Cloud mode (on typical local hardware, a few minutes in Local mode).
- Notes read as if written by a careful person: a clear title, a one-line summary, your words tidied, the important facts from the source, and real tasks.
- Notes link to the notes you already have, and suggest where they belong.
- Choose one model provider, local or cloud, and know that every model call goes there.
- Work offline: captures queue up and are processed when the model is reachable again.
- Regenerate any note from its raw source with one key.

### Non-goals (v1)

- Voice memos and transcription, photos of handwriting and whiteboards as a dedicated input, video and YouTube transcripts (§17).
- Editing notes. FastCapture opens them in an editor.
- Fetching pages that need a login or JavaScript. Paste the text with the link.
- Syncing, accounts or a server of our own.
- Processing notes that FastCapture didn't capture (for example FastPad's own quick captures in the inbox). See §18.
- Mixing local and cloud models, or falling back from one to the other.
- macOS or Linux.

## 5. Capturing

### 5.1 Ways in

| Entry point | Result |
|---|---|
| Global hotkey `capture_hotkey` (default **Ctrl+Alt+Space**) | Opens the capture box |
| Global hotkey `clipboard_hotkey` (default none) | Captures the clipboard as it is, with no box, and shows a short tray toast "Captured from clipboard" |
| Tray icon: left click | Opens the review window |
| Tray icon: right click | Menu: Capture…, Capture clipboard, Review (n), Pause processing, Settings, Quit |
| `fastcapture add ["text" \| -] [--url URL] [--attach PATH]... [--notebook PATH]` | Queues a capture with no window, for scripts and agents. Forwarded to the running instance over a named pipe, or handled by a short-lived process that writes the raw source and exits; the tray app processes it when it next runs. Exit code 0 when the raw source is written. |

- The default hotkey differs from FastPad's Ctrl+Alt+N, so both apps can run together. If `RegisterHotKey` fails, a tray notice says so and Settings offers another key.
- FastCapture starts at sign-in through a `HKCU\…\Run` entry, which the installer offers and Settings can turn off. One instance per user, guarded by a mutex, as FastPad does.

### 5.2 The capture box

Like FastPad's (FastPad spec §10.2), plus a kind indicator and a notebook picker:

```
┌ Capture → Work notes · Cloud (Claude) ──────────────────┐
│ lag spikes during ledger batch??                        │
│                                                         │
│ ┌──────┐ ┌──────────────┐                               │
│ │ ▓▓▓▓ │ │ ledger.log   │   + Snip (Ctrl+Shift+S)       │
│ └──────┘ └──────────────┘                               │
└ Screenshot + file · Ctrl+Enter save · Esc close ────────┘
```

- A multi-line edit control. What you type is **your note**: the model keeps it, tidied, and uses it as your intent for the source ("why did I capture this?").
- Attachments come in the same ways as in FastPad: Ctrl+V with an image or with files copied in Explorer, dropping files, the **Snip** button (Windows' own `ms-screenclip:` overlay, then the next clipboard image), and Ctrl+Shift+V to attach a clipboard image when the clipboard holds text too.
- **Links.** A line that is only an `http(s)://` URL is treated as a link to fetch (§6.3). Several URL lines are several links in one capture. A URL inside a sentence is kept as text.
- **Rich text.** Pasting HTML from a browser or Word (`HTML Format` on the clipboard) keeps it as Markdown in the raw source (headings, lists, links, tables), and the box shows the plain text. The page URL from the HTML clipboard's `SourceURL` is recorded as `source_url`.
- The footer shows what was detected ("Text", "Link", "Screenshot + file"…). The title shows the notebook and the mode, so you can see where a capture will go.
- **Ctrl+Enter** saves and closes. **Esc** closes and keeps a draft, as in FastPad. The box stays open when it loses focus.
- **Ctrl+N** opens a picker of recent notebooks for this capture only.
- Fully keyboard-operable, with an accessible name ("Capture to Work notes").

### 5.3 Where it came from

With `capture_source=true` (default `false`), the title of the window in front when the hotkey was pressed is recorded as `source_window` in the raw source, and given to the model as context. It is off by default for the reason FastPad gives: window titles can contain things you didn't mean to save.

## 6. Raw sources

### 6.1 The raw file

On Ctrl+Enter, FastCapture writes, in this order and all or nothing (as FastPad does):

1. the attachments, copied into the attachments folder (images from the clipboard as PNG, files copied and never moved, the 20 MB question as in FastPad);
2. the raw source, `raw/YYYY-MM-DD HHmm <first words>.md`, named with FastPad's `capture_stem` rules.

```markdown
---
type: raw
captured: 2026-10-19T10:30
kind: [text, screenshot, file]
capture_id: 01JAB3K9X7Q2M8
source_window: "Grafana – payments – Google Chrome"
---

lag spikes during ledger batch??

![](../attachments/2026-10-19%201030%20lag%20spikes.png)
[ledger.log](../attachments/ledger.log)
```

- `raw_folder` (in `capture.ini`, default `raw`) sets the folder.
- `capture_id` is a ULID. It ties the raw source, the queue entry and the written note together, and survives renames.
- For a **link**, the body holds the URL, your text, and below a `## Page` heading the extracted page as Markdown, with `url`, `fetched`, `page_title` and `site` in the front matter. If the fetch fails, the raw source says so (`fetch: failed (403)`) and the capture continues with whatever text you gave.
- For a **file**, the raw source links the copied file. The extracted text is not stored in the raw source: it can always be extracted again from the copy.
- After this, the raw file is never written by FastCapture again.

### 6.2 Extraction (local, no model)

These steps run on the PC in both modes, because no model is involved:

| Input | Extraction |
|---|---|
| Rich text | HTML clipboard → Markdown, with a small converter (headings, emphasis, lists, links, code, tables) |
| Web page | WinHTTP GET with the system proxy, a desktop user agent, no cookies, 10 s timeout, 5 MB limit, redirects followed (at most 5). HTML → main content with a readability-style scorer → Markdown. `og:title`, `og:site_name` and `article:published_time` are kept. |
| PDF | Text extracted page by page (a pure-Rust PDF text extractor). A page with almost no text is treated as scanned and rendered to PNG with `Windows.Data.Pdf`, to be read by the model as an image. |
| `.docx` | Paragraph and heading text from `word/document.xml` (zip + XML) |
| Text, logs, code | Read with FastPad's encoding detection. A long log is cut to its first and last 200 lines, marked where it was cut. |
| Other files | Name, type and size only. The note lists the file; nothing is read. |

Limits, so a single capture can't cost much: at most 60,000 characters of text, 20 PDF pages, and 8 images per model call. Images are scaled down to 1,568 pixels on the long side before sending. Anything cut is listed in the note's `truncated:` front matter key.

### 6.3 The queue

Each raw source becomes a job in a per-PC queue, `%LocalAppData%\FastCapture\queue\<capture_id>.json`, with its raw path, notebook, state (`queued`, `working`, `done`, `failed`), attempts and last error.

- The queue survives restarts. On start, jobs left `working` go back to `queued`.
- Jobs run one at a time per notebook, oldest first, on a worker thread.
- Failures that may pass (offline, timeout, HTTP 429/5xx, local server not running) are retried with backoff: 30 s, 2 min, 10 min, then every 30 min. Other failures (bad key, model missing, invalid output twice) mark the job `failed`, shown in the review list with the error and **Retry**.
- **Pause processing** (tray) stops new jobs. A paused queue and a missing model show in the tray tooltip.
- Raw sources in `raw/` with no job and no note (written by `fastcapture add` while the tray app was closed, or copied from another PC) get a job when the notebook is next scanned.

## 7. Writing the note

### 7.1 What the model gets

One request per capture, built from:

- **system prompt:** the writing rules (§7.3) and the JSON schema;
- **context:** today's date, the notebook's name, the 50 most used tags, and up to 30 **candidate notes** from the index (§9), each with its title, aliases, folder, tags and a 300-character snippet;
- **your note** (the text you typed), clearly marked as yours;
- **the source:** extracted text, page metadata, and images (screenshots, scanned PDF pages).

### 7.2 What the model returns

JSON, checked against a schema:

```json
{
  "title": "Consumer lag spikes during ledger batch",
  "summary": "Kafka consumer lag on payments jumps to ~40k when the nightly ledger batch runs.",
  "body": "Consumer lag spikes when the ledger batch runs. Seen on the Grafana payments board.",
  "key_points": ["Lag peaks at ~40k around 02:10, recovers by 02:35", "Only the `ledger-events` consumer group"],
  "tasks": ["Check batch size config in ledger job"],
  "tags": ["investigate", "payments"],
  "links": [{"note": "Payments incidents"}, {"note": "Ledger batch", "new": true}],
  "home": {"kind": "topic", "note": "Payments incidents", "reason": "Earlier lag incidents are collected there"}
}
```

FastCapture then:

- drops links to notes that aren't candidates and aren't marked `new`, and tags that aren't valid tags (FastPad spec §6.3);
- caps lengths (title 120 characters, summary 300, key points 12, tasks 12);
- asks once more, with the validation errors, when the JSON is invalid; a second failure marks the job `failed`.

The Claude API, OpenAI-compatible servers that support it, and Claude Code (`--json-schema`) all get the schema as structured output. For other servers the schema is in the prompt and the JSON is parsed from the reply.

### 7.3 Writing rules (system prompt summary)

- Keep the user's words and voice. Fix spelling, grammar and unclear wording; don't add opinions or change meaning.
- The title says what the note is about, in the user's terms. No dates in titles (the front matter has them).
- The summary is one sentence a reader could stop after.
- Key points are facts from the source (numbers, names, decisions, quotes with attribution), not padding. Empty is fine.
- Tasks only when the user or the source implies something to do. Empty is fine.
- Prefer the notebook's existing tags. At most 5.
- Link only to candidate notes that are really related. Suggest a new note only for a clear, recurring subject.
- Never invent facts that aren't in the user's text or the source. If an image can't be read, say so in the body.
- Write in the language of the user's text, or of the source when the user wrote nothing.

### 7.4 The written note

`<capture_folder>/<title>.md` (sanitized as in FastPad, ` 2`, ` 3`… on a clash):

```markdown
---
type: capture
captured: 2026-10-19T10:30
capture_id: 01JAB3K9X7Q2M8
source: "[[raw/2026-10-19 1030 lag spikes during ledger batch]]"
kind: [text, screenshot, file]
tags: [investigate, payments]
summary: Kafka consumer lag on payments jumps to ~40k when the nightly ledger batch runs.
suggested_home: "[[Payments incidents]]"
status: inbox
model: cloud/claude-sonnet-5-5
---

# Consumer lag spikes during ledger batch

> Kafka consumer lag on payments jumps to ~40k when the nightly ledger batch runs.

Consumer lag spikes when the ledger batch runs. Seen on the Grafana payments board.

## Key points

- Lag peaks at ~40k around 02:10, recovers by 02:35
- Only the `ledger-events` consumer group

## Tasks

- [ ] Check batch size config in ledger job

## Related

- [[Payments incidents]]
- [[Ledger batch]]

## Attachments

![](../attachments/2026-10-19%201030%20lag%20spikes.png)
[ledger.log](../attachments/ledger.log)
```

- Empty sections are left out. Links come in the form FastPad's completion would insert (the stem, or the path when the stem is ambiguous).
- For a link capture, `url` and `site` are in the front matter and the `## Source` section links the page.
- `model` records the mode and model name, so a reader knows what wrote the note.
- `status` is `inbox` until filed (§10), then `filed`.

## 8. Models

### 8.1 Modes

Settings has one **Mode** choice for the whole app:

| Mode | Backends | Data leaving the PC |
|---|---|---|
| **Local** | An OpenAI-compatible server on this PC | Only page fetches for captured links |
| **Cloud** | Claude API · OpenAI-compatible cloud endpoint · Claude Code CLI (experimental) | Each capture's text, extracted content, images and candidate note snippets, sent to the chosen provider |

- Every model call uses the chosen backend. There is no fallback to the other mode.
- Changing the mode affects jobs that haven't started. Existing notes are left as they are; **Regenerate** (§10.2) uses the current mode.
- The first time Cloud mode is chosen, a short screen lists what will be sent and to whom.

### 8.2 Local mode

- **Endpoint:** an OpenAI-compatible base URL. Settings detects Ollama (`localhost:11434`), LM Studio (`localhost:1234`) and Foundry Local (asked through its CLI for its port) and lists their models.
- **The endpoint must be on this PC:** the host must be `localhost` or resolve only to loopback addresses. Otherwise Local mode refuses it, so a remote server can't be used by mistake while the setting says Local.
- **Model:** one model for everything. Settings shows whether it accepts images (from the server's model info, or a one-pixel test request). Without vision, screenshots and scanned pages are attached to the note unread, and the note says "Image not read: the local model doesn't support images."
- Suggested models are listed in the README, not built in.
- Timeouts are longer (5 minutes per request) and jobs run one at a time.

### 8.3 Cloud mode: API backends

- **Claude API:** the Messages API with the user's key, a model picked from a list (default the current Sonnet), structured output, image blocks, and prompt caching on the system prompt and schema.
- **OpenAI-compatible:** a base URL, a key and a model name (OpenAI, OpenRouter, Azure OpenAI, and others). Uses the same client as Local mode, without the loopback check.
- Keys are stored in Windows Credential Manager (`FastCapture/<backend>`), never in files or logs.
- Settings shows the tokens used today and this month for API backends, as reported by the API. There is a monthly token cap (default none): when it's reached, the queue pauses with a tray notice.

### 8.4 Cloud mode: Claude Code CLI (experimental)

For users with a Claude Pro or Max plan who don't want to manage an API key. FastCapture runs **the user's own installed, unmodified Claude Code CLI**, which uses whatever login the user gave it. FastCapture never reads, copies or stores Claude.ai credentials.

- Each job runs `claude -p --output-format json --json-schema <schema> --tools "" --max-turns 1 --no-session-persistence --model <model> --append-system-prompt <rules>`, with the request on stdin as `--input-format stream-json` (image content blocks for images), and large inputs written to temporary files.
- `ANTHROPIC_API_KEY` is removed from the child's environment, because the CLI would otherwise use it instead of the subscription. Settings shows the result of `claude auth status`, so the user can see that `claude.ai` login is in use.
- Usage limits, auth failures and overload are read from the CLI's result and retry events. A usage limit pauses the queue until the reset time when the CLI reports one, and otherwise for 30 minutes.
- Marked **experimental** and **for personal use** in Settings. Anthropic's docs allow a user to sign in to the unmodified Claude Code program with their own subscription. They don't cover another app starting it, so this backend may have to change if the rules do. Image input over stream-json and the usage-limit error text need checking before this ships (§18).
- When FastCapture is distributed, each user uses their own Claude Code login. FastCapture never sends one user's requests through another user's account.

## 9. The notebook index

To pick candidate notes (§7.1) and homes (§10), FastCapture keeps its own keyword index of the capture notebook:

- **What:** every Markdown note's path, title (first `# ` heading or stem), aliases, tags, headings and text, with FastPad's skip rules (folders starting with `.`, `node_modules`, `target`, and so on). `raw/` and templates are left out.
- **How:** BM25 over words, with titles, aliases and tags weighted higher. Built on a worker thread after start-up, kept current with a `ReadDirectoryChangesW` watcher, and cached at `%LocalAppData%\FastCapture\index\<folder key>.cache`, which can be thrown away at any time.
- **Query:** your note text, the page title and the first 2,000 characters of the source. The top 30 results become candidates. Notes with a `type:` of `topic`, `project` or `moc`, and folders, are offered as possible homes.
- **Limits:** 10,000 notes and 2 MB per note, as in FastPad.

## 10. Review and filing

### 10.1 The review window

```
┌ FastCapture — Review (3) ─────────────────────────────────────────────┐
│ ● Consumer lag spikes during ledger…  │ # Consumer lag spikes during…  │
│   → Payments incidents        10:31   │ > Kafka consumer lag on…       │
│ ● Rust async cancellation (article)   │ ...                            │
│   → Learning/Rust             09:12   │──────── Raw source ────────────│
│ ⚠ Q3 planning.pdf: model timed out    │ lag spikes during ledger…      │
│                       Retry   08:40   │ [screenshot]                   │
├───────────────────────────────────────┴────────────────────────────────┤
│ Enter file → Payments incidents · H change home · E edit · R regenerate │
│ I keep in inbox · Del discard                                           │
└─────────────────────────────────────────────────────────────────────────┘
```

- The list shows notes with `status: inbox` written by FastCapture (`type: capture`) in the capture folder, newest first, plus queued, working and failed jobs.
- The preview is read-only Markdown (headings, lists, links, images) with the raw source below it.
- The tray icon shows the number of notes waiting. A toast when a note is ready is optional (`notify_ready`, default off).

### 10.2 Actions

| Key | Action |
|---|---|
| **Enter** | File to the suggested home (§10.3) |
| **H** | Change home: a picker of folders and home candidates, filtered as you type |
| **I** | Keep in inbox: set `status: kept` and remove it from the list |
| **E** | Open the note in FastPad or the default editor |
| **R** | Regenerate from the raw source with the current mode. The previous note is replaced; the raw source is untouched. |
| **Del** | Discard: the note goes to the Recycle Bin. Shift+Del also recycles the raw source and the attachments that only it uses. |
| **Ctrl+Z** | Undo the last file or discard in this session |

A note you've edited since FastCapture wrote it (its stamp changed) is never replaced by Regenerate without asking.

### 10.3 Filing

- **Home is a folder:** the note moves into it. Its own relative links (attachments) are rewritten first, so they keep working.
- **Home is a topic note:** the note moves into the topic note's folder, and an entry is appended to the end of the topic note, using FastPad's append rules (an entry inside a list, a blank line before it when needed):

  ```markdown
  - 2026-10-19 [[Consumer lag spikes during ledger batch]]: Kafka consumer lag on payments jumps to ~40k…
  ```

- In both cases, `status` becomes `filed` and `suggested_home` is removed. These are the only edits FastCapture makes to a note's front matter after writing it, and they change only those lines.
- Writes use an atomic saver with a stamp check, retried up to three times, as FastPad's capture does, so an open FastPad tab takes the change as an outside edit or an append merge.

## 11. Settings and files

**Per-PC settings:** `%AppData%\FastCapture\fastcapture.ini`: the notebook, mode, backend, endpoint and model per backend, hotkeys, `capture_source`, `notify_ready`, the token cap, start at sign-in. Keys are in Credential Manager.

**Per-notebook:** `.fastcapture\capture.ini`, created only when a value changes from its default:

```ini
version=1
raw_folder=raw
home_types=topic, project, moc
```

Read from the notebook, never written: `.fastpad\notebook.ini` (`capture_folder`, `attachments_folder`, `daily_folder`, `daily_format`) and `.obsidian\app.json` (`attachmentFolderPath`).

**Per-PC state:** `%LocalAppData%\FastCapture\`: `queue\`, `index\`, `draft\`, `log\` (diagnostics, never capture content).

## 12. Architecture

One process, `fastcapture.exe`:

| Part | Thread | Job |
|---|---|---|
| Shell | UI | Tray icon, hotkeys, capture box, review window, settings |
| Capture writer | UI, then I/O worker | Attachments and raw source, all or nothing |
| Extractors | Job worker | Rich text, web pages, PDF, docx, text (§6.2) |
| Index | Index worker + watcher | Keyword index of the notebook (§9) |
| Job runner | Job worker | Queue, prompt building, the backend call, validation, writing the note |
| Backends | Job worker | `LocalOpenAi`, `ClaudeApi`, `CloudOpenAi`, `ClaudeCodeCli` behind one trait: `fn complete(&self, request: &NoteRequest) -> Result<NoteJson, BackendError>` |
| Filing | UI | Moves, link rewrites, appends to topic notes |
| CLI | Short-lived process | `fastcapture add`, forwarded over a named pipe |

Dependencies are kept small: `serde`/`serde_json`, `pulldown-cmark` (as in FastPad), an HTML parser for extraction, a PDF text extractor, `zip` for docx. HTTP uses WinHTTP (system proxy and certificate store, no TLS crate). Images use WIC. `Windows.Data.Pdf` is called through WinRT only when a scanned page needs rendering.

## 13. Privacy and security

- Local mode sends nothing to any model outside the PC. Page fetches for captured links are the only network traffic, and they carry no cookies.
- Cloud mode sends only what §8.1 lists. Nothing is sent before a capture is saved, and nothing is sent for notes FastCapture didn't capture.
- Web page content and file text are passed to the model as **material to summarise, never as instructions**. They go inside delimited blocks, the system prompt says to ignore any instructions in them, and the model has no tools (the Claude Code backend runs with tools turned off). The worst a hostile page can do is write a misleading note, and the raw source is beside it in review.
- Window titles are recorded only when `capture_source=true`.
- Diagnostic logs never contain capture text, page content, prompts or keys.
- Links in notes are written only to notes in the index or as clearly new notes. FastCapture never writes outside the notebook, the attachments folder and its own app data.

## 14. Performance

- **Idle:** no CPU use, under 30 MB of memory, with the watcher thread blocked.
- **Hotkey to box visible:** under 100 ms.
- **Ctrl+Enter to box closed:** under 150 ms for text and a 4K screenshot (the PNG is encoded and written on a worker thread; the box closes once the copy is safe in the draft folder).
- **Index:** a cold build of 10,000 notes in under 5 s on a worker thread, warm from cache in under 500 ms. A query in under 20 ms.
- **Extraction:** a typical article page under 300 ms after download, a 20-page text PDF under 1 s.

## 15. Error handling

| Situation | Behavior |
|---|---|
| Raw source or attachment can't be written | The box stays open with the error, the draft is kept, nothing partial is left |
| Page fetch fails or the page has no main text | The raw source records it; the note is written from your text and says the page couldn't be read |
| Model unreachable (offline, server stopped) | Job stays queued, retried with backoff; tray tooltip says "Waiting for <backend>" |
| Bad key, model missing, auth failed | Queue pauses; one tray notice opens Settings |
| Rate limit or Claude Code usage limit | Queue pauses until the reset time, or for 30 minutes |
| Invalid JSON twice | Job `failed`, shown in review with Retry |
| Local model without vision | Images attached unread, and the note says so |
| Local endpoint isn't loopback | Refused in Settings with an explanation |
| `notebook.ini` or `capture.ini` bad value | Default used; one notice per file change |
| Note changed by the user before Regenerate or Filing | Regenerate asks first; filing re-reads and keeps their edits |
| Topic note can't be appended to (locked, missing) | The note stays in the inbox, with an error in the review list |
| `RegisterHotKey` fails | Tray notice; capture still works from the tray and `fastcapture add` |
| Token cap reached | Queue pauses with a notice |

## 16. Delivery

Six milestones. Each is usable on its own.

| # | Scope | Depends on |
|---|---|---|
| **M1** | Tray, hotkeys, capture box, attachments and snip, raw sources, queue, `fastcapture add`, notebook conventions, a review window that lists raw captures. No AI yet: a "note" is the raw source. | none |
| **M2** | Backend trait; Local (OpenAI-compatible) and Claude API backends; prompt, schema, validation, note writer for text and screenshots; Regenerate. | M1 |
| **M3** | Extraction: rich text, web pages, PDF (text and scanned), docx, text and logs. | M2 |
| **M4** | Notebook index, candidate notes and links, homes, filing (move and topic append), undo. | M2 |
| **M5** | OpenAI-compatible cloud backend, token usage display and cap. | M2 |
| **M6** | Claude Code CLI backend (experimental), after checking image input and limit errors. | M2 |

## 17. Later

- **Voice memos:** a hotkey to record, transcription with Whisper (`whisper.cpp` locally, or a cloud speech API in Cloud mode), then the same note pipeline.
- **Handwriting and whiteboard photos:** a "photo" kind with a prompt tuned for transcription and a cleaned-up image.
- **Daily note entry:** an optional line in today's daily note for each filed note.
- **Browser extension** for logged-in and JavaScript pages.
- **Processing FastPad's own captures** in the inbox folder on request.
- **Templates per kind** in the notebook, overriding the built-in note shape.

## 18. Open questions

1. **Default hotkey.** Ctrl+Alt+Space is free in most setups, but some IMEs and launchers use it. Win+Shift+C is the alternative.
2. **Shared crate with FastPad.** Copying the notebook modules is simple now. Extracting `notebook-conventions` as a crate both repos use would keep them in step, at the cost of a cross-repo dependency.
3. **Claude Code CLI checks** (before M6): image content blocks over `--input-format stream-json` in `-p` mode, the result and exit code when the subscription limit is reached, and whether Anthropic's rules for personal tools that start the CLI change.
4. **Topic note entry format.** A dated link line (§10.3) is the simplest. Some users will want the summary under a heading, or the tasks copied over.
5. **Sharing the capture folder with FastPad.** If a notebook's `capture_folder` is also where FastPad writes one-file quick captures, the two kinds of file share a folder. They're told apart by `type: capture`, but a separate `notes_folder` key in `capture.ini` may be clearer.
6. **Picking candidate notes by keyword only** may miss related notes that use different words. If it does, a small local embedding model in both modes could be added, sending nothing extra in Cloud mode.
