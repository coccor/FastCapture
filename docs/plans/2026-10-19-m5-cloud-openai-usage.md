# M5: OpenAI-Compatible Cloud Backend and Token Usage Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Each part ends with `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passing, and one commit. Code comments never cite the spec.

**Goal:** Cloud mode gets a second API backend: any OpenAI-compatible endpoint (OpenAI, OpenRouter, Azure OpenAI and others) with a base URL, a key and a model name. Every cloud request's tokens, as the API reports them, are added to a small ledger on this PC. Settings shows the tokens used today and this month, and takes a monthly token cap: when the month's tokens reach it, the queue pauses with one tray notice.

**Spec:** `docs/specs/2026-10-18-fastcapture-design.md` §8.1 (the first-use screen), §8.3, §11 (per-PC settings), §13, §15 (token cap reached).

**Decided for M5:**

- **The OpenAI-compatible backend is the Local client with a key**, as §8.3 says: `POST <base>/v1/chat/completions`, the schema as `response_format` (strict), images as data URLs, and the schema-in-the-prompt fallback for servers that refuse `response_format`. Its base URL must be `https://`, since the key goes with every request. A base URL ending in `/v1` is taken as it is (`https://openrouter.ai/api/v1` → `https://openrouter.ai/api/v1/chat/completions`).
- **Azure OpenAI gets its key as `api-key`**, not `Authorization: Bearer`, for hosts ending in `.openai.azure.com` or `.services.ai.azure.com`, with the v1 API's base URL (`https://<resource>.openai.azure.com/openai/v1`). Older deployment URLs with `api-version` aren't supported.
- **Tokens are counted for cloud backends only**, as reported: input (including cache reads and writes) and output. Local servers are free and aren't counted. A capture asked twice (an unusable first reply) counts both requests.
- **The cap counts every token**, cached ones included, so it errs on the side of stopping early. It is one cap for the month across both cloud backends. A job already running finishes, so the month can go over by one capture. The pause lifts when the cap is raised or removed in Settings, or when the month changes.
- **The first-use screen is asked per provider**: once for Anthropic (`cloud_accepted`, as now), and again whenever the OpenAI-compatible host changes (`openai_accepted=<host>`), naming the host.
- **No vision probe in the cloud**, since it would cost a request. A model that refuses images fails the job with "<model> doesn't read images. Choose a model that does in Settings."

## What M5 does not do

- **Prices.** Settings shows tokens, not money; prices differ by provider and model and change.
- **Usage from the provider's own billing pages**, or counts for requests made outside FastCapture.
- **Older Azure OpenAI deployment URLs** (`/openai/deployments/<name>/chat/completions?api-version=…`).
- **The Claude Code CLI backend** (§8.4) is M6.

## Architecture

```
src/
  model/
    backend.rs        (changed) `Usage` moves here; `Backend::take_usage` returns the tokens used since it was last asked
    claude.rs         (changed) its usage through `take_usage`, added up across the second ask
    openai.rs         (changed) `OpenAi::cloud`: https only, the key as Bearer or `api-key`, 120 s; `usage` read from replies; `list_models` with a key
  runner/
    usage.rs          the ledger: tokens per day and model in %LocalAppData%\FastCapture\usage.txt; today and this month
    from_settings.rs  (changed) Cloud mode builds Claude or the OpenAI-compatible backend; the factory also gives the cap
    worker.rs         (changed) records each job's tokens; pauses before a cloud job when the cap is reached
  config/
    settings.rs       (changed) `backend=openai`, `openai_endpoint`, `openai_model`, `openai_accepted`, `token_cap`
  ui/
    settings.rs       (changed) the cloud provider choice, the OpenAI-compatible fields, List models, tokens used, the cap
    settings_form.rs  (changed) the new fields, their checks, per-provider consent
```

## Parts

### Part 1: Usage from every API backend

- [x] `model/backend.rs`: `Usage { input, cache_read, cache_write, output }` moves here, with `total()` and `add`. `Backend::take_usage(&self) -> Option<Usage>` (default `None`) returns what the backend's requests used since it was last asked, so the worker gets both requests of a capture asked twice. A `Meter` holds it for each backend.
- [x] `model/claude.rs`: keeps adding into its usage instead of replacing `last_usage`; `examples/live_note.rs` reads it through `take_usage`.
- [x] `model/openai.rs`: reads `usage` from replies (`prompt_tokens`, `completion_tokens`, `prompt_tokens_details.cached_tokens` as cache reads, so `input` is the rest), logs it as Claude does, and returns it through `take_usage`. A reply without `usage` counts nothing.
- **Tests:** Claude and OpenAI-compatible replies with and without usage; two requests add up; taking it empties it.

### Part 2: The OpenAI-compatible cloud backend

- [x] `config/settings.rs`: `backend=openai`, `openai_endpoint` (an `https://` URL), `openai_model`, `openai_accepted` (the host the first-use screen was accepted for). `openai_` keys are no longer just reserved. The Settings form keeps the file's `backend` until Part 5 lets you choose it.
- [x] `model/openai.rs`: `OpenAi::cloud(endpoint, model, key)`: refuses a base URL that isn't `https://`; 120 s per request; the key as `Authorization: Bearer`, or `api-key` for Azure hosts; errors name the host ("api.openai.com refused the API key…"). A message with `refusal` fails the job ("The model declined to write a note for this capture."). A 400 whose message names images fails it with the advice above. `list_models(endpoint, key, timeout)` sends the key the same way.
- [x] `runner/from_settings.rs`: Cloud mode with `backend=openai` needs the endpoint, the model, `openai_accepted` matching the endpoint's host, and the key in `FastCapture/openai`, and says which is missing. Labels are `cloud/<model>`.
- **Tests:** a scripted server as the cloud endpoint: the Bearer key and the Azure `api-key`; the base URL with and without `/v1`; `http://` refused; refusal, the images message and the status mapping (401 pauses, 429 retries after `retry-after`); the model list with a key; settings round trip; `build` naming what is missing.

### Part 3: The usage ledger

- [x] `runner/usage.rs`: `%LocalAppData%\FastCapture\usage.txt`, one line per day and model: `2026-10-19 cloud/claude-haiku-5-5 input=900 cache_read=1605 cache_write=0 output=391`. `Ledger::add(day, label, usage)` updates the day's line and saves atomically (the atomic saver); `today(day)` and `month(day)` add up the lines. Lines older than 13 months are dropped when it is saved. A line it can't read is kept as it is and counts nothing; a file it can't read counts as empty, logged once.
- [x] `runner/worker.rs`: after each job, a cloud backend's `take_usage` is added to the ledger under the backend's label and today's date, whether the job was written, retried or failed. `Worker::start` takes the ledger's path; a cloud backend is one whose label starts `cloud/`.
- **Tests:** add and total across days and models; a new month starts at zero; old lines dropped; a corrupt line kept; the worker records a written job and a job asked twice.

### Part 4: The token cap

- [x] `config/settings.rs`: `token_cap`: empty or `0` for none (the default), else a whole number, with `k` or `M` allowed (`2M`, `500k`); a bad value warns and means none.
- [x] `runner/worker.rs`: the factory gives the cap with the backend. Before a cloud job, when this month's tokens have reached the cap, the queue pauses with "This month's token cap (2,000,000) is reached. Raise it in Settings, or wait for next month." The tray shows it once, as other pauses. The pause is checked again when the settings change, and at the usual idle check, so a new month resumes on its own. Local mode ignores the cap. The cap is checked only when a cloud job is due, from `Factory::token_cap`, which reads the settings file each time. **Resume processing** in the tray menu checks it again, so it stays paused while the cap is reached.
- **Tests:** the cap's values; a job waits once the month reaches the cap and goes on after the cap is raised; a new month lifts it; Local mode isn't capped.

### Part 5: Settings

- [x] `ui/settings.rs`, `ui/settings_form.rs`: "Notes by" becomes **A model on this PC (Local)** / **A cloud API (Cloud)**. Cloud has a provider combo, **Claude API** or **OpenAI-compatible**: Claude shows its model list and key as now; OpenAI-compatible shows **Address** (base URL), **Model** (a combo you can type into, filled by **List models**) and its own **API key**, saved as `FastCapture/openai`. **Test** sends the usual tiny note request to whichever is chosen. The two providers share the Address, Model and API key rows; only the chosen one's fields show, Claude's address is shown read-only, and a key typed for one provider is cleared when you switch to the other.
- [x] Below them: "Tokens used: 12,400 today, 210,000 this month, as the APIs reported them." and **Monthly token cap** (a box; empty for none), with the line saying when the cap is reached.
- [x] The first-use screen names the provider: Anthropic's Claude API, or the OpenAI-compatible host. It is asked again when that host changes. It now says "short descriptions of related notes" rather than "later, the titles of related notes", since M4 sends them.
- **Tests:** the form's checks (no address, not https, no model, no key); the round trip through the file, never the key; consent asked once per host; the window driven through messages saves an OpenAI-compatible setup with its key in Credential Manager (a test credential name), and shows the ledger's numbers.

### Part 6: End to end

- [x] `runner/end_to_end.rs`: a capture written by a scripted OpenAI-compatible server, with its tokens in the ledger; the cap reached pauses the next job, and raising it lets it run.
- [x] `examples/live_note.rs` uses whichever cloud backend the settings choose, and prints its tokens the same way for both. `--effort` applies to the Claude API only.
- [ ] Measure with `examples/live_note.rs` (with your approval, with a key you add in Settings yourself) on a provider you choose: one text capture, one screenshot, one link; tokens and time against the Claude numbers from M2–M4.

  **Skipped for now** (2026-10-19), at your choice: no OpenAI-compatible key is set up yet. The backend is tested against scripted servers only.

### Part 7: Polish and docs

- [x] README: the OpenAI-compatible backend (OpenAI, OpenRouter, Azure base URLs), what is sent and to whom, tokens used, the cap.
- [x] Spec: §19 notes for M5.

## Testing approach

- **Pure:** usage parsing, the ledger's text, the cap's values, the form's checks.
- **Scripted servers:** the OpenAI-compatible backend's requests, headers and errors, with no internet.
- **Disk:** the ledger file in a scratch folder; the worker with a scripted backend.
- **Windows:** the Settings window driven through messages.
- **Live:** only `examples/live_note.rs`, run by hand with your approval.

## Order and dependencies

Part 1 comes first; 2 and 3 need it; 4 needs 3; 5 needs 2–4; 6 and 7 come last.
