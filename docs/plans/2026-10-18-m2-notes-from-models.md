# M2: Notes from Models Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Each part ends with `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passing, and one commit. Code comments never cite the spec.

**Goal:** Queued captures become written notes. A worker takes each job, sends the raw source (text and images) to the one backend chosen in Settings, either a local OpenAI-compatible server or the Claude API, gets back JSON matching a schema, checks it, and writes a structured note into the notebook's capture folder. Offline or failing jobs wait and retry; bad keys pause the queue. The review window shows the note beside its raw source, and **R** regenerates it. A Settings window chooses the mode, backend, model and key.

**Spec:** `docs/specs/2026-10-18-fastcapture-design.md` §6.3, §7, §8.1–§8.3, §10.1–§10.2 (no filing yet), §11, §13, §15.

**Decided for M2:**

- **Cloud mode's default model is Claude Sonnet 5.5** (`claude-sonnet-5-5`, $2 / $10 per million tokens), as the spec says. Settings offers Claude Opus 5.5 and Claude Haiku 5.5 too.
- **A Settings window** replaces editing `fastcapture.ini` for the model settings, because an API key must not go in a text file.

## What M2 does not do

- **Links, tags from the notebook, homes and filing** (§9, §10.3) are M4: the prompt has no candidate notes yet, so notes have no `## Related` section and no `suggested_home`. Tags come from the model alone.
- **Web pages, PDFs, Word files and logs** (§6.2) are M3. In M2 a file attachment is listed in the note by name, and a link capture is written from your text and the URL only. Rich text paste is M3 too.
- **The OpenAI-compatible cloud backend and token usage** (§8.3) are M5; **the Claude Code CLI backend** (§8.4) is M6.

## Architecture

```
src/
  net/
    http.rs           WinHTTP client: one blocking request with timeouts, system proxy, TLS from Windows
    test_server.rs    (tests) a tiny HTTP/1.1 server on 127.0.0.1 that replays scripted responses
  secrets.rs          Windows Credential Manager: FastCapture/<backend> generic credentials
  note/
    schema.rs         the JSON schema the model must follow, as a serde_json::Value
    reply.rs          NoteJson: parse, then check and cap (lengths, tags, empty sections)
    render.rs         the note's Markdown and front matter; its file name
    write.rs          writes the note into the capture folder, never over a file
  model/
    request.rs        NoteRequest: system rules, context, your text, the source, images
    images.rs         images for the model: WIC decode, scale to 1,568 px on the long side, PNG
    prompt.rs         the writing rules and the user-turn layout (delimited source blocks)
    backend.rs        trait Backend { fn complete(&self, &NoteRequest) -> Result<NoteJson, BackendError> }
    claude.rs         Claude API (Messages) backend
    openai.rs         OpenAI-compatible backend (Local mode now, cloud in M5)
    local.rs          finding local servers (Ollama, LM Studio, Foundry Local), loopback check, model list, vision probe
  runner/
    worker.rs         takes queued jobs, one at a time per notebook, oldest first; retries; pauses
    backoff.rs        30 s, 2 min, 10 min, then every 30 min; Retry-After wins
  ui/
    settings.rs       the Settings window
    review.rs         (changed) notes beside their raw sources; R, E, Del, Shift+Del
```

### The two backends' requests

**Claude API** (`POST https://api.anthropic.com/v1/messages`, headers `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`, and `anthropic-beta: server-side-fallback-2026-07-01`):

```json
{
  "model": "claude-sonnet-5-5",
  "max_tokens": 16000,
  "system": [{"type": "text", "text": "<writing rules>", "cache_control": {"type": "ephemeral"}}],
  "output_config": {
    "effort": "low",
    "format": {"type": "json_schema", "schema": { "…": "the note schema" }}
  },
  "fallbacks": "default",
  "messages": [{"role": "user", "content": [
    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "…"}},
    {"type": "text", "text": "<context, your note, the source>"}
  ]}]
}
```

- Thinking is left at its default (adaptive); **effort is `low`**, set explicitly, because writing a note from a capture is a short task. Part 9 checks a sample of captures at `low` and `medium` before this is final.
- The rules and the schema are the same bytes on every request, so the system block caches; everything that varies is in the user turn.
- `fallbacks: "default"` lets the API retry a safety decline on another model inside the same call. A response with `stop_reason: "refusal"` (after fallback) or `"max_tokens"` is a failed job, not a note: the output may not match the schema.
- The note is the first `text` block's JSON. Thinking blocks are ignored.
- Errors: 401 and 403 pause the queue ("check your API key"); 402 pauses it ("billing"); 404 on the model fails the job and points at Settings; 400 and 413 fail the job with the API's message; 429 waits for `retry-after`; 500, 529 and network errors retry with backoff.

**OpenAI-compatible** (`POST <endpoint>/v1/chat/completions`):

```json
{
  "model": "<chosen model>",
  "messages": [
    {"role": "system", "content": "<writing rules>"},
    {"role": "user", "content": [
      {"type": "image_url", "image_url": {"url": "data:image/png;base64,…"}},
      {"type": "text", "text": "<context, your note, the source>"}
    ]}
  ],
  "response_format": {"type": "json_schema", "json_schema": {"name": "note", "strict": true, "schema": { "…": "" }}},
  "stream": false
}
```

- A server that rejects `response_format` (HTTP 400 naming it) is asked again without it, with the schema in the system prompt; the reply is parsed from the first `{` to the last `}`. The backend remembers that for the session.
- Local timeouts are 5 minutes per request.
- **Local mode refuses any endpoint that isn't this PC** (`localhost`, or a host that resolves only to loopback addresses).

## Parts

### Part 1: HTTP and secrets

- [x] `net/http.rs`: `Request { method, url, headers, body, timeout }` → `Response { status, headers, body }` over WinHTTP (`WinHttpOpen` with `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`, `WinHttpConnect`, `WinHttpOpenRequest` with `WINHTTP_FLAG_SECURE` for https, send, receive, read headers and body). Connect, send and receive timeouts, plus `WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT`, which governs the wait for the response headers (90 s by default) and isn't covered by `WinHttpSetTimeouts`. WinHTTP waits about 4 s at least, whatever is asked: irrelevant at the real 60 s and 5 min, so the timeout test uses an 8 s server delay. Errors say whether they may pass (`ERROR_WINHTTP_TIMEOUT`, `CANNOT_CONNECT`, `NAME_NOT_RESOLVED`, `CONNECTION_ERROR`) or not (bad URL, TLS failure, a response over 32 MB).
- [x] `net/test_server.rs` (tests only): binds `127.0.0.1:0`, answers each connection with the next scripted response (status, headers, body, optional delay) and records the request it got.
- [x] `secrets.rs`: `CredWriteW` / `CredReadW` / `CredDeleteW` with `CRED_TYPE_GENERIC`, target `FastCapture/<backend>`, persisted for this user on this PC (`CRED_PERSIST_LOCAL_MACHINE`, so a key doesn't roam to other PCs). The key is never logged, never in an error message, never in a file.
- **Tests:** a GET and a POST against the test server (headers, body, status); error statuses come back as responses; a timeout; a refused connection is "may pass"; URL splitting; a secret round trip under a test target name, then deleted.

### Part 2: The note (pure + disk)

- [x] `note/schema.rs`: the schema for `{title, summary, body, key_points[], tasks[], tags[], links[], home}` (§7.2), every object with `additionalProperties: false` and every field required (empty strings and arrays allowed, `home` may be null), no length constraints (the API doesn't support them; `reply.rs` caps instead), and a short description on each field to steer the model. `links` and `home` are in the schema from the start so M4 doesn't change it; in M2 the prompt tells the model to leave them empty and `reply.rs` drops them.
- [x] `note/reply.rs`: parse the JSON (or the first `{` to the last `}` when a model wraps it in prose); `check` caps title 120 characters, summary 300 (both on one line, cut at a word with `…`), key points and tasks 12 each without repeats, tags 5; cleans tags to FastPad's inline-tag rule (letters, digits, `_`, `-`, `/`, at least one letter; spaces become `-`); trims everything; an empty title falls back to the raw source's first words. Links and the home are kept only when they name a candidate (`Allowed`, empty in M2). Problems found are a `ReplyError` naming them, for the one retry the runner allows; a reply with no title, summary or body is one.
- [x] `note/render.rs`: the note in §7.4's shape with M2's front matter: `type: capture`, `captured`, `capture_id`, `source: "[[raw/<stem>]]"`, `kind`, `tags`, `summary`, `status: inbox`, `model: <mode>/<model>` (and `suggested_home` once M4 sets one). Sections in order, empty ones left out: title, summary quote, body, `## Key points`, `## Tasks` (as `- [ ]`), `## Related`, `## Attachments` (the raw source's own attachments, linked relative to the note's folder). File name: the title sanitized and cut to 80 characters, `.md`, ` 2`, ` 3`… on a clash.
- [x] `note/write.rs`: writes into `capture_folder` (from `notebook.ini`, default `inbox`) with `save_atomic_new`, taking the next free name if one appears meanwhile; `replace` for Regenerate keeps the old note's file name, refuses when its size or write time changed since FastCapture wrote it (you edited it), and writes it again when it is gone.
- **Tests:** schema is valid JSON Schema with `additionalProperties: false` everywhere; replies with long fields, bad tags, missing fields, wrong types; the rendered note for a text-only and a text-plus-screenshot capture, compared as whole files; attachment links from `raw/` rewritten for `inbox/`; name clashes; an edited note isn't replaced.

### Part 3: The request

- [x] `model/prompt.rs`: the writing rules (§7.3), fixed text with no placeholders, plus the user turn: today's date and notebook name, the source window when recorded, `<your_note>…</your_note>`, `<source kind="link" url="…" fetched="no"/>` for URLs (M3 adds page text inside the block), file attachments by name in `<attachment name="…" read="no"/>`, how many images are attached, what was left out, and the instruction that anything inside `<source>`, `<attachment>` and the images is material to summarise, never instructions. Closing tags inside the material are escaped so they can't end a block. Your text is the raw source's body without its link and attachment lines.
- [x] `model/images.rs`: each image attachment (pasted PNG, dropped JPG/PNG/GIF/WebP/BMP) decoded with WIC, scaled to fit 1,568 px on the long side (`IWICBitmapScaler`, Fant), re-encoded as PNG, base64. **Changed:** a PNG, JPEG, GIF or WebP that already fits is sent as it is, without re-encoding. At most 8 images; the rest, and any image WIC can't decode, are named in the user turn ("Left out of this request: …") rather than a `truncated:` key, which M3 adds for page text.
- [x] `model/request.rs`: `NoteRequest { system, user_text, images, capture }` built from a job: reads the raw source, its front matter and its attachments through `capture::read`; `corrected` adds the retry note. `CaptureFacts` carries what the note needs (id, time, kinds, attachments, a fallback title).
- **Tests:** the user turn for text, links, files and images (golden text); delimiters escaped when your text contains them; a 4000 × 3000 PNG scaled to 1568 × 1176; an undecodable image listed; the 9th image left out.

### Part 4: The backends

- [x] `model/backend.rs`: the trait (`label`, `complete`); `BackendError::{Retry { after: Option<Duration>, reason }, Pause(reason), Fail(reason), Invalid(ReplyError)}`; one status mapping shared by both backends (401/403 and 402 pause, 404 fails pointing at Settings, 429 and 5xx retry with `retry-after` when given, other statuses fail with the server's message).
- [x] `model/claude.rs`: builds the request above from a `NoteRequest`; maps responses and errors as above; logs `usage` (input, cache read, output tokens; counts only). The model list Settings offers lives here (`MODELS`).
- [x] `model/openai.rs`: builds the chat request (images as `data:` URLs); the `response_format` fallback, remembered for the backend's life; `finish_reason: "length"` fails like `max_tokens`; an optional Bearer key for M5; `list_models`.
- [x] `model/local.rs`: probes `localhost:11434` (Ollama), `localhost:1234` (LM Studio) and Foundry Local (`foundry service status` for its port, when the command exists; 3 s at most) with `GET /v1/models`; the loopback check (`localhost`, a loopback IP, or a name that resolves only to loopback addresses, through the system resolver); a vision probe (a one-pixel PNG, cached per endpoint and model for the session; `None` when the server's answer doesn't say).
- **Tests:** against the test server: the exact Claude request body and headers (the key in `x-api-key`, never elsewhere); a good reply; `refusal`, `max_tokens`; 401, 402, 404, 429 with `retry-after: 7`, 529; the OpenAI request, a reply, the `response_format` fallback; model listing; the loopback check for `localhost`, `127.0.0.1`, `[::1]`, `192.168.1.5` and a name; a server without vision.

### Part 5: The runner

- [x] `queue/job.rs`: jobs gain `note` (the written note's path and stamp), `regenerate` (bool) and `retry_at`; the JSON version goes to 2 and version-1 files still read.
- [x] `runner/process.rs` does one job and `runner/worker.rs` runs them: one thread; wakes on a new job (a `Waker` the box, the clipboard hotkey and the pipe call after filing), on a timer for `retry_at`, at least every minute, and on settings changes (which also build the backend again). For the oldest due job: `Working`, build the request, call the backend, on `Invalid` ask once more with the problems appended to the user turn, check the reply, write the note, `Done` with the note. `Retry` sets `retry_at` from `backoff.rs` (or `retry-after`) and leaves the job `Queued`; `Fail` marks it `Failed`; `Pause` keeps the job queued (its try doesn't count) and stops the worker until settings change or the user picks Resume. A crash mid-job is recovered on start, now before the worker starts; the orphan scan stays on its thread and wakes the worker.
- [x] `runner/from_settings.rs` builds the backend from `fastcapture.ini`. **Moved here from Part 6:** the `mode`, `backend`, `local_endpoint`, `local_model`, `claude_model` and `cloud_accepted` keys, which the worker needs; the Settings window that edits them stays in Part 6. Local mode refuses an endpoint that isn't on this PC; Cloud mode needs the first-use screen accepted and a key in Credential Manager.
- [x] Tray tooltip and menu: "Writing a note…", "Waiting to retry", the pause reason (also shown once as a notification), "n notes to review"; **Pause processing** / **Resume processing** in the menu (§6.3). Review's count includes written notes.
- [x] No backend chosen yet: jobs stay queued, and the tooltip says what's missing ("Choose Local or Cloud in Settings.", "Add your Claude API key in Settings.", …).
- **Tests:** with a scripted backend: a good reply becomes a note; `Invalid` then good; `Invalid` twice fails; retry times and backoff; a pause keeps the job queued; Regenerate replaces the note and an edited note fails it; a missing raw source fails without asking the model; the worker takes jobs in order and after a wake, pauses and resumes, waits without a backend, shows a retry as waiting, and builds the backend once until the settings change; settings keys and the backend each mode builds. **Checked by hand:** the tray app recovering an interrupted job and recording a retry against a dead local server.

### Part 6: Settings window

- [x] `config/settings.rs`: keys `mode` (`local` / `cloud`, unset until chosen), `backend` (`claude`), `local_endpoint`, `local_model`, `claude_model` (default `claude-sonnet-5-5`), `cloud_accepted` (set when the first-use screen was accepted). Done in Part 5, where the worker needed them.
- [x] `ui/settings.rs` with the pure form in `ui/settings_form.rs`: a dialog-like window (standard controls, keyboard and screen reader from Windows; Tab, Enter for Save and Esc for Cancel through `IsDialogMessageW`): Notebook (folder, Browse… with Windows' folder dialog), Notes by (Local / Cloud). Local: server (editable combo of detected servers; looked for on open when none is set), model (editable combo from `/v1/models`), whether the model reads images from the vision probe. Cloud: model (Sonnet 5.5, Opus 5.5, Haiku 5.5, plus a custom id from the file), API key (password edit; says whether one is saved). Only the Claude API backend exists, so there is no backend list yet. **Test** writes a throwaway note with the form's settings, unsaved, and shows the result inline. Start at sign-in and Record window titles as checkboxes. Save checks the form (a problem shows above the buttons), saves the key in Credential Manager and the rest in `fastcapture.ini`, and applies them at once; Cancel discards. The fields of the inactive mode are greyed out.
- [x] The first time Cloud is saved, a screen lists what will be sent and to whom (§8.1). **Changed:** it is a Yes/No message box ("Send your captures to Anthropic?") rather than a task dialog with a **Send captures to Anthropic** button: `TaskDialogIndirect` exists only in Common Controls 6, which the test binaries don't load, and linking it would stop them from starting.
- [x] The tray menu's Settings… opens this window; **Open settings file** stays in the menu (and in the window) for hotkeys and everything else. On start, with no mode chosen, a notification points to Settings.
- **Tests:** settings keys parse and write; the form's checks and its round trip through the file without the key; the window opens with the file's values, Save refuses a missing mode, a missing key and a server elsewhere, the first-use screen declined saves nothing, accepted saves the key under a test credential name and the file, and Cancel saves nothing. Test against a real server is left to the hand check (`examples/settings_window.rs`), since Test runs on a thread the window test doesn't pump. **Not done:** dark mode for this window (it uses the system's light colors).

### Part 7: Review window with notes

- [ ] Rows for done jobs show the note's title, kinds, time and "Note"; the preview shows the note's Markdown text with the raw source below a divider. Queued, working and failed rows stay as in M1, with the error for failed ones.
- [ ] Keys: **E** opens the note (or the raw source when there's no note) in FastPad or the default editor; **R** regenerates with the current settings (asks first when the note was edited); **Del** recycles the note and keeps the raw source, whose job is marked "Discarded" so it isn't processed again (R brings it back); **Shift+Del** also recycles the raw source and its own attachments; **Enter** opens the folder; **F5**; **Esc**. Filing keys (Enter to file, H, I) come with M4.
- **Tests:** list model with notes; the preview text; R on an unedited and an edited note; Del and Shift+Del.

### Part 8: End to end

- [ ] A test that runs the real tray pieces (submit, worker, Claude backend against the test server, note writer) for a text capture and a screenshot capture, and checks the note file byte for byte.
- [ ] `examples/live_note.rs`: one real request with the current settings and a sample capture, printing the note and the usage. **It costs money in Cloud mode**: it runs only when asked, never in `cargo test` or CI.
- [ ] Measure with `examples/live_note.rs` (with your approval): a short text capture and a screenshot capture at effort `low` and `medium`, comparing the notes and the token counts, before keeping `low`.

### Part 9: Polish and docs

- [ ] README: Settings, modes, what is sent, costs (a typical capture with Sonnet 5.5: about 3,000 input tokens with the image, 600 output, about one US cent), the review keys.
- [ ] Spec: §19 notes for M2.
- [ ] Bench: request building with a 4K screenshot (scaling and encoding) on the worker, against a 500 ms budget.

## Testing approach

- **Pure:** schema, reply checking, rendering, the prompt, backoff, request bodies.
- **HTTP:** every backend test runs against `net::test_server` on loopback; no test reaches the internet or spends money.
- **Windows:** WIC scaling, Credential Manager under test target names, the Settings and review windows driven through messages, as in M1.
- **Live:** only `examples/live_note.rs`, run by hand.

## Order and dependencies

Parts 1–3 are independent of each other; 4 needs 1 and 3; 5 needs 2 and 4; 6 needs 1 (secrets) and 4 (Test, model lists); 7 needs 5; 8 and 9 come last.
