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

- [ ] `net/http.rs`: `Request { method, url, headers, body, timeout }` → `Response { status, headers, body }` over WinHTTP (`WinHttpOpen` with `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`, `WinHttpConnect`, `WinHttpOpenRequest` with `WINHTTP_FLAG_SECURE` for https, send, receive, read headers and body). Connect, send and receive timeouts; one session shared by the worker. Errors say whether they may pass (`ERROR_WINHTTP_TIMEOUT`, `CANNOT_CONNECT`, `NAME_NOT_RESOLVED`, `CONNECTION_ERROR`) or not (bad URL, TLS failure).
- [ ] `net/test_server.rs` (tests only): binds `127.0.0.1:0`, answers each connection with the next scripted response (status, headers, body, optional delay) and records the request it got.
- [ ] `secrets.rs`: `CredWriteW` / `CredReadW` / `CredDeleteW` with `CRED_TYPE_GENERIC`, target `FastCapture/<backend>`, persisted per user. The key is never logged, never in an error message, never in a file.
- **Tests:** a GET and a POST against the test server (headers, body, status); a timeout; a refused connection is "may pass"; a secret round trip under a test target name, then deleted.

### Part 2: The note (pure + disk)

- [ ] `note/schema.rs`: the schema for `{title, summary, body, key_points[], tasks[], tags[], links[], home}` (§7.2), every object with `additionalProperties: false` and every field required (empty strings and arrays allowed), no length constraints (the API doesn't support them; `reply.rs` caps instead). `links` and `home` are in the schema from the start so M4 doesn't change it; in M2 the prompt tells the model to leave them empty and `reply.rs` drops them.
- [ ] `note/reply.rs`: parse the JSON; cap title 120 characters, summary 300, key points and tasks 12 each, tags 5; clean tags to FastPad's inline-tag rule (letters, digits, `_`, `-`, `/`, at least one letter); trim everything; an empty title falls back to the raw source's first words. Problems found are a `ReplyError` naming them, for the one retry the runner allows.
- [ ] `note/render.rs`: the note in §7.4's shape with M2's front matter: `type: capture`, `captured`, `capture_id`, `source: "[[raw/<stem>]]"`, `kind`, `tags`, `summary`, `status: inbox`, `model: <mode>/<model>`. Sections in order, empty ones left out: title, summary quote, body, `## Key points`, `## Tasks` (as `- [ ]`), `## Attachments` (the raw source's own links, rewritten relative to the note's folder). File name: the title sanitized, `.md`, ` 2`, ` 3`… on a clash.
- [ ] `note/write.rs`: writes into `capture_folder` (from `notebook.ini`, default `inbox`) with `save_atomic_new`; `replace` for Regenerate takes the old note's path and stamp, and refuses when the file changed since it was written (the user edited it).
- **Tests:** schema is valid JSON Schema with `additionalProperties: false` everywhere; replies with long fields, bad tags, missing fields, wrong types; the rendered note for a text-only and a text-plus-screenshot capture, compared as whole files; attachment links from `raw/` rewritten for `inbox/`; name clashes; an edited note isn't replaced.

### Part 3: The request

- [ ] `model/prompt.rs`: the writing rules (§7.3), stable text, plus the user turn: today's date and notebook name, `<your_note>…</your_note>`, `<source kind="link" url="…">…</source>` blocks for URLs (M3 adds page text), file attachments by name in `<attachment name="…"/>`, and the instruction that anything inside `<source>` is material to summarise, never instructions. Your text is the raw source's body without its link lines.
- [ ] `model/images.rs`: each image attachment (pasted PNG, dropped JPG/PNG/GIF/WebP/BMP) decoded with WIC, scaled to fit 1,568 px on the long side (`IWICBitmapScaler`, Fant), re-encoded as PNG, base64. At most 8 images; the rest are listed in `truncated:`. An image WIC can't decode is listed, not sent.
- [ ] `model/request.rs`: `NoteRequest { system, user_text, images, capture }` built from a job: reads the raw source, its front matter and its attachments through `capture::read`. Pure apart from reading files.
- **Tests:** the user turn for text, links, files and images (golden text); delimiters escaped when your text contains them; a 4000 × 3000 PNG scaled to 1568 × 1176; an undecodable image listed; the 9th image left out.

### Part 4: The backends

- [ ] `model/backend.rs`: the trait; `BackendError::{Retry { after: Option<Duration>, reason }, Pause(reason), Fail(reason), Invalid(ReplyError)}`.
- [ ] `model/claude.rs`: builds the request above from a `NoteRequest`; maps responses and errors as above; reads `usage` (input, cache read, output tokens) into the job's log line (counts only).
- [ ] `model/openai.rs`: builds the chat request; `response_format` fallback; maps errors the same way (401/403 pause, 404 model fails, 429 waits, 5xx and network retry).
- [ ] `model/local.rs`: probes `localhost:11434` (Ollama), `localhost:1234` (LM Studio) and Foundry Local (`foundry service status` for its port, only when `foundry` is on `PATH`) with `GET /v1/models`, 1 s timeout each; the loopback check (resolve with `GetAddrInfoW`, every address must be loopback); a vision probe (a one-pixel PNG, cached per endpoint and model for the session).
- **Tests:** against the test server: the exact Claude request body and headers (the key in `x-api-key`, never elsewhere); a good reply; `refusal`, `max_tokens`; 401, 402, 404, 429 with `retry-after: 7`, 529; the OpenAI request, a reply, the `response_format` fallback; model listing; the loopback check for `localhost`, `127.0.0.1`, `[::1]`, `192.168.1.5` and a name; a server without vision.

### Part 5: The runner

- [ ] `queue/job.rs`: jobs gain `note` (the written note's path and stamp), `regenerate` (bool) and `retry_at`; the JSON version goes to 2 and version-1 files still read.
- [ ] `runner/worker.rs`: one thread; wakes on a new job (an event the submit path sets), on a timer for `retry_at`, and on settings changes. For the oldest due job: `Working`, build the request, call the backend, on `Invalid` retry once with the problems appended to the user turn, write the note, `Done` with the note's path. `Retry` sets `retry_at` from `backoff.rs` (or `retry-after`) and leaves the job `Queued`; `Fail` marks it `Failed`; `Pause` stops the worker until settings change or the user picks Resume. A crash mid-job is recovered on start (M1's `recover`).
- [ ] Tray tooltip and menu: "Waiting for <backend>" while retrying, "Paused: <reason>" with **Resume** in the menu, "n notes to review". **Pause processing** in the menu (§6.3).
- [ ] No backend chosen yet: jobs stay queued, and the tray says "Choose a model in Settings".
- **Tests:** with a fake backend: oldest first; a retry waits and then succeeds; `Invalid` then good; `Invalid` twice fails; `Pause` stops later jobs; Regenerate replaces the note and an edited note makes it fail with "edited since it was written"; recovery after a kill mid-job.

### Part 6: Settings window

- [ ] `config/settings.rs`: keys `mode` (`local` / `cloud`, unset until chosen), `backend` (`claude`), `local_endpoint`, `local_model`, `claude_model` (default `claude-sonnet-5-5`), `cloud_accepted` (set when the first-use screen was accepted).
- [ ] `ui/settings.rs`: a dialog-like window (standard controls, keyboard and screen reader from Windows): Notebook (folder, Browse…), Mode (Local / Cloud). Local: endpoint (combo of detected servers, editable), model (combo from `/v1/models`), "Images: supported / not supported" from the vision probe. Cloud: backend (Claude API now; later backends appear here), model (Sonnet 5.5, Opus 5.5, Haiku 5.5), API key (password edit; "Saved in Windows Credential Manager"), **Test** (one tiny request, result shown inline). Start at sign-in and Record window titles as checkboxes. Save writes `fastcapture.ini` and the key; Cancel discards.
- [ ] The first time Cloud is saved, a screen lists what will be sent and to whom (§8.1) and needs **Send captures to Anthropic** to continue.
- [ ] The tray menu's Settings opens this window; **Open settings file** stays in the menu for hotkeys and everything else.
- **Tests:** settings keys parse and write; the window opens, fields load from settings, Save writes them (with a test credential target), the first-cloud screen is shown once; Test against the test server.

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
