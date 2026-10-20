# M7: GitHub Copilot Backend Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Each part ends with `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passing, and one commit. Code comments never cite the spec.

**Goal:** Cloud mode gets a fourth provider, **GitHub Copilot (experimental)**, for people with a Copilot plan who would rather use it than an API key. FastCapture runs the user's own installed, unmodified Copilot CLI and talks to it the way the Copilot SDK does, one session per note, with no tools. Usage comes out of the user's Copilot allowance (GitHub AI Credits); when it runs out or a rate limit is hit, the queue pauses.

**Spec:** `docs/specs/2026-10-18-fastcapture-design.md` §8.1, §8.5, §13, §15, §18.7.

**Found before planning (2026-10-20, free):**

- **On this PC:** the Copilot CLI isn't installed (no `copilot` on `PATH`; `~/.copilot` holds only another tool's logs). `gh` 2.102.0 is installed and signed in, which the CLI accepts as a last-resort login.
- **The CLI** is installed with `winget install GitHub.Copilot` (or `npm install -g @github/copilot`, which needs Node 22) and is at 1.0.100 (2026-10-19). It signs in with `copilot login` (an OAuth token kept in Windows Credential Manager) or takes `COPILOT_GITHUB_TOKEN`, `GH_TOKEN` or `GITHUB_TOKEN`; classic `ghp_` tokens aren't accepted.
- **`copilot -p`** has `--model`, `--attachment` for images, `--output-format=json`, `--available-tools`, `--no-custom-instructions` and `--disable-builtin-mcps`, but **no system-prompt flag, no reply schema, and no way to stop it saving the session**.
- **The SDK** starts `copilot --headless --no-auto-update --stdio` and speaks JSON-RPC 2.0 with `Content-Length` framing (protocol version 3). Requests include `ping`, `status.get`, `auth.getStatus`, `models.list` and `session.create`; the CLI sends `session.event` notifications. A session takes `model`, `systemMessage` with `mode: "replace"`, `availableTools` / `excludedTools`, and per message `attachments` (`{type: "blob", data, mimeType}`, base64) and `responseSchema` (preview). Events: `assistant.message`, `assistant.usage` (`inputTokens`, `outputTokens`, `cacheReadTokens`, `cacheWriteTokens`, `copilotUsage.totalNanoAiu`), `session.idle`, `session.error` (`errorType`: `authentication`, `authorization`, `policy`, `quota`, `rate_limit`, `context_limit`; `errorCode` such as `quota_exceeded`, `rate_limited`, `user_weekly_rate_limited`, `billing_not_configured`). There is no written protocol spec, only the SDKs' generated types.
- **The Rust crate** `github-copilot-sdk` 1.0.23 brings tokio, tokio-stream, tokio-util, async-trait, tracing, parking_lot, regex, uuid, futures-util and more, and by default downloads its own copy of the CLI at build time.
- **Billing:** since 2026-06-01 Copilot charges AI Credits per token at each model's rate (1 credit = $0.01; Pro has 1,500 a month, Pro+ 7,000). Annual Pro and Pro+ plans keep premium requests until they renew. Copilot Free has a small allowance with automatic model choice only.
- **Terms:** the SDK is "available to all existing GitHub Copilot subscribers, including Copilot Free for personal use" (GitHub changelog, 2026-06-02). Nothing found says whether another desktop app may start the CLI for a user's own plan, as with Claude Code.
- **GitHub Models** was retired on 2026-07-30, as §8.5 says.

**Decided for M7:**

- **FastCapture speaks the SDK's protocol itself**, over the CLI's stdio, with plain threads like the rest of FastCapture: no crate, no async runtime. `copilot -p` can't take the writing rules as a system prompt or a reply schema, and the crate would bring tokio and a dozen crates for one request per note. The wire names are taken from the SDK's generated types and checked live in Part 1; the protocol version the CLI reports is checked on every start, and an unknown one pauses the queue asking to update FastCapture.
- **One CLI process per note at first**, started in an empty folder of FastCapture's (`%LocalAppData%\FastCapture\copilot\`) inside a job object, with no window and a 5-minute timeout, as for Claude Code. If Part 1 finds start-up slow, the process is kept for a few minutes between notes instead.
- **Each note is one session:** the model from Settings, `systemMessage` replacing Copilot's own prompt with the writing rules, `availableTools: []` (or every tool excluded, whichever Part 1 shows really leaves none), custom instructions off, images as base64 blobs, the schema as `responseSchema` when the CLI takes it and in the rules otherwise. The reply is parsed and checked as any other. The session is deleted afterwards.
- **The environment is left as it is:** Part 1 found a `GH_TOKEN` doesn't override `copilot login` in headless mode. FastCapture never sets a token itself.
- **Usage is counted:** Copilot charges per token (Part 1: every note reports its tokens and AI units), so `assistant.usage`'s tokens go in the usage ledger and count toward the token cap (`metered()` is true). The log also has each note's AI credits (from `totalNanoAiu`). The note's `model:` is `cloud/copilot/<model>`. (Part 1: on Copilot Free the allowance is counted in requests, 200 chat requests a month, so the tokens are FastCapture's own count and the cap is a soft guard there; Settings also shows the requests left, from `account.getQuota`.)
- **Limits and sign-in:** `quota` and `rate_limit` errors pause until the reset time when one is given, else for 30 minutes; `authentication` pauses naming `copilot login`; `authorization` and `policy` pause saying the plan or the organization doesn't allow it; a model the plan doesn't offer pauses asking to choose another in Settings.
- **The CLI is found** at `copilot_path` when set, else `copilot.exe` then `copilot.cmd` on `PATH`. A `copilot.cmd` from npm is followed to the `copilot.exe` in its package (`node_modules@githubcopilot
ode_modules@githubcopilot-win32-x64`), which is started directly. WinGet's location wasn't seen (Part 1 had an npm install), so it is looked for on `PATH` only.

## What M7 does not do

- **Installing, updating or signing in to the CLI.** The user does those (`winget install GitHub.Copilot`, then `copilot login`). FastCapture never reads, copies or stores GitHub credentials, and starts the CLI with `--no-auto-update`.
- **Showing the month's allowance.** GitHub's billing page shows it; Settings shows the tokens and credits FastCapture has used, as for the cloud APIs.
- **Copilot through an organization's OAuth app** or a GitHub App: the user's own login only.

## Architecture

```
src/
  model/
    copilot/
      mod.rs          the backend: the session, the message, the reply, errors; label cloud/copilot/<model>
      cli.rs          finding copilot.exe, its version, the command line
      rpc.rs          JSON-RPC 2.0 over stdio with Content-Length framing: requests, replies, notifications
    claude_code/
      run.rs          (changed) the job object and no-window start shared with the Copilot CLI
  runner/
    from_settings.rs  (changed) `backend=copilot`
  config/
    settings.rs       (changed) `copilot_model`, `copilot_path`, `copilot_accepted`
  ui/
    settings.rs       (changed) the fourth provider: model list, version, sign-in state
    settings_form.rs  (changed) its checks and its first-use screen
```

## Parts

### Part 1: Checks on the real CLI (live, with your approval)

These need the Copilot CLI installed (`winget install GitHub.Copilot`) and signed in (`copilot login`), both by you. The checks that send a prompt use your Copilot allowance: a few short requests, likely under 20 AI credits ($0.20) on Pro. Each finding is written here before the code depends on it.

- [x] Where `copilot.exe` is, `copilot --version`, `copilot --help`, and that `--headless --stdio` answers `ping` and `status.get` (the protocol version).

  **Found (2026-10-20, free):** you installed it with npm (`@github/copilot` 1.0.100, on Node 22.16), not WinGet. npm's `%AppData%\npm\copilot.cmd` only runs a Node launcher, which starts the real program: `%AppData%\npm\node_modules\@github\copilot\node_modules\@github\copilot-win32-x64\copilot.exe` (170 MB). FastCapture starts that `copilot.exe` directly when it finds the `.cmd`, so there is no `cmd.exe` or Node in between. `copilot --version` prints "GitHub Copilot CLI 1.0.100." `--headless` and `--stdio` aren't in `--help` but work. The handshake is `connect` (the SDK's own first call; `ping` is the fallback for older CLIs), which answers `{"ok":true,"protocolVersion":3,"version":"1.0.100"}` about 0.9–1.7 s after start. `ping` and `status.get` give the same protocol version. A whole start, handshake, session and stop took 2–4 s.
- [x] `auth.getStatus` and `models.list`, free: what they return signed in, and whether the list is only the models your plan allows.

  **Found:** `auth.getStatus` gives `{"isAuthenticated":true,"authType":"user","login":"coccor","statusMessage":"coccor"}`. Your plan is **Copilot Free** (`account.getCurrentAuth`: `access_type_sku: "free_limited_copilot"`). `account.getQuota` counts **requests**, not credits: `chat` 200 a month (2 used), `completions` 2,000, `premium_interactions` 0, each with `remainingPercentage`, `hasQuota` and `tokenBasedBilling: true`; it resets on 2026-11-01. `models.list` is only the plan's models: here just `{"id":"auto","name":"Auto"}`. So on Free the model row offers only **Auto**, and a note costs one chat request whatever its size.
- [x] A text request through `session.create` and the message call: the wire names, the events in order, `assistant.usage`, the time from start to note, and whether a `gh` login alone is enough.

  **Found (2026-10-20, 3 requests on Copilot Free, with FastCapture's own rules and the sample lag text):** the calls are `connect`, `session.create`, `session.send` (`{sessionId, prompt, attachments?, responseFormat?}`, which answers `{messageId}` at once), then `session.event` notifications until `session.idle`, then `session.delete`. The events that matter, in order: `session.usage_info` (`systemTokens`, `conversationTokens`, `toolDefinitionsTokens: 0`), `assistant.usage`, `assistant.message` (`content`, the reply's text; `toolRequests: []`), `session.idle`. No requests came from the CLI. `model` left out means Auto, which chose **GPT-6 Luna** (`isAuto: true`, `reasoningEffort: "medium"`). `assistant.usage` has `inputTokens`, `outputTokens`, `cacheReadTokens`, `cacheWriteTokens`, `reasoningTokens`, `finishReason`, `copilotUsage.totalNanoAiu` and `tokenDetails` (the rate per million tokens per kind: input 10 AI units, output 50, cache read 1, cache write 12.5, in nano-units). The text note took 924 input and 184 output tokens, 0.018 AI units, **7.4 s** from start (connect 0.6 s, the request 5.0 s). The quota snapshot, in `assistant.usage` and `account.getQuota`, kept its 05:13 timestamp and still showed 2 chat requests used afterwards, so it isn't fresh enough to show per note. A `gh` login alone lists no models (above), so it isn't enough.
- [x] With `availableTools: []` (and with every tool in `excludedTools`): the tools the session reports. No request is needed if the session lists them.

  **Found:** `session.create` with `availableTools: []` and then `session.tools.initializeAndValidate`: `session.tools.getCurrentMetadata` reports `{"tools":[]}`. The same session with the defaults reports `powershell` and the rest of the coding agent's tools, so the empty list really turns them off. `excludedTools` isn't needed. Creating a session sends no prompt and costs nothing. Also set: `enableConfigDiscovery: false`, `enableSkills: false`, `enableSessionStore: false`, `infiniteSessions: {enabled: false}`, `requestPermission: false`, `streaming: false` and `workingDirectory` (the scratch folder). The CLI may send requests of its own (`userInput.request`, `hooks.invoke` and others), which FastCapture refuses; none came.
- [x] `systemMessage` with `mode: "replace"`: whether the rules are followed and Copilot's own prompt is gone (input tokens tell).

  **Found:** it is gone. `systemTokens` was 842 with the rules and the schema in them (3,734 characters) and 484 with the rules alone (2,163 characters), so nothing of Copilot's own prompt is left. The notes followed the rules: the user's words, a one-sentence summary, the facts as key points, "Check the batch size." as the task, `home: null`.
- [x] `responseSchema`: whether the CLI takes it and where the structured reply appears.

  **Found:** `responseFormat: {type: "json_schema", jsonSchema: {name: "note", strict: true, schema}}` with FastCapture's note schema is taken (its `anyOf` with `null` included). The reply is the note's JSON as `assistant.message`'s `content`, with no separate field, so it is parsed with `reply::parse` like any other. With the schema in the rules instead, the reply was the same valid JSON. FastCapture sends `responseFormat` and keeps the rules' fallback for a CLI that refuses it.
- [x] The M2 chart screenshot as a base64 blob: whether the model sees it.

  **Found:** it does: `attachments: [{type: "blob", data, mimeType: "image/png", displayName}]`, and the note read the chart exactly (`ledger-events`, the batch at 02:05, the 39.8k peak at 02:12, back near baseline by 02:30). 1,760 input tokens (1,757 written to a 30-minute cache), 297 output, 0.037 AI units, **9.1 s** from start (the request 6.6 s).
- [x] Signed out (by starting it with `COPILOT_HOME` pointed at an empty folder, so your login isn't touched): the error it gives. A used-up allowance can't be reached on purpose; it is taken from the SDK's error types.

  **Found:** an empty `COPILOT_HOME` isn't signed out: the CLI falls back to the `gh` login (`authType: "gh-cli"`, "coccor (via gh)"), even with `GH_CONFIG_DIR` pointed at an empty folder, since `gh` keeps its token in Windows Credential Manager. **Through `gh` alone, `models.list` is empty**, so a `gh` login isn't enough. A truly signed-out CLI wasn't reached without signing `gh` out, which wasn't done. So FastCapture counts the CLI usable only when `isAuthenticated` is true **and** `models.list` isn't empty; otherwise Settings and the pause say "run copilot login". An empty `COPILOT_HOME` gets a new `config.json` and `logs`, so FastCapture never points it anywhere but the user's own.
- [x] Whether `GH_TOKEN` or `GITHUB_TOKEN` in the environment wins over `copilot login`.

  **Found:** in headless mode it doesn't: with a made-up `GH_TOKEN`, `auth.getStatus` was still your `copilot login` (`authType: "user"`) and `models.list` still offered `auto`. FastCapture leaves the environment as it is, so a user who has only a token keeps it working. `COPILOT_GITHUB_TOKEN` wasn't tried.
- [x] Whether the CLI saves the session anywhere under `~/.copilot` after `session.delete`.

  **Found:** with `enableSessionStore: false` there is no `workspacePath` and nothing in `session-state`. After `session.delete` (and `session.list` empty) only an empty lock file per session is left in `session-state\.session-operation-locks`, and an empty log per process in `logs` with `--log-level none`. They hold names, not content, and FastCapture leaves them. With the defaults the CLI makes a `session-state\<id>` folder.

### Part 2: The protocol and the CLI

- [x] `model/copilot/rpc.rs`: `Content-Length` framing both ways; numbered requests and their replies; notifications handed to the caller; a reader thread so the CLI can't block on a full pipe; a timeout per request. Requests from the CLI are refused with JSON-RPC's "method not found", as the real CLI refuses an unknown one; a call made after the CLI has ended fails at once.
- [x] `model/copilot/cli.rs`: finding `copilot` (above), its version, `copilot --headless --no-auto-update --stdio --log-level none` in the scratch folder (`%LocalAppData%\FastCapture\copilot\`), and the protocol version check. The version comes from `connect`'s answer, so `copilot --version` isn't run; `ping` stands in for a CLI without `connect`; any protocol but 3 is refused, naming both. The environment is left as it is (Part 1). Checked once against the real CLI, free: found through npm's shim, connected in 1.7 s, signed in, `auto` listed.
- [x] `model/claude_code/run.rs`: the job object and no-window start pulled out into `platform/job.rs` so both CLIs use them.
- **Tests:** framing (split and joined messages, a multi-byte body), a fake CLI (a small test program speaking the protocol from a script of replies), a timeout, the CLI ending early. The fake is a PowerShell script behind a `copilot.cmd`, answering each method from a file of canned replies and keeping what it was sent; the connection itself is tested over in-memory pipes.

### Part 3: The backend

- [x] `model/copilot/mod.rs`: create the session, send the message with images and schema, collect events until `session.idle`, read the note from the last `assistant.message` (structured or parsed with `reply::parse`), take usage from `assistant.usage`, delete the session, stop the CLI. Errors as decided above; a reply that isn't a note is asked for once more, as now. Before the session, `auth.getStatus` is asked, so a signed-out CLI pauses before anything is sent. A CLI that refuses `responseFormat` gets the message again with the schema after the text. A `session.error` has no reset time, so a used-up allowance or an account-wide rate limit (`user_weekly_rate_limited`, `user_global_rate_limited`) pauses for 30 minutes; other rate limits retry. A session.create error naming the model pauses asking for another. The CLI ending mid-note, or the 5-minute limit (the session is aborted), retries the job. The session is deleted however it ended. Each note's model, tokens, AI units and time go in the log.
- **Tests:** replies and errors from fixtures (success, each `errorType`, a reply that isn't JSON, the CLI ending mid-session); usage returned by `take_usage`. Whole notes through the fake CLI: a note with an image, a signed-out CLI, a used-up allowance. The CLI ending mid-session is covered by the connection's own tests.

### Part 4: Settings and choosing the backend

- [x] `config/settings.rs`: `backend=copilot`, `copilot_model` (default `auto`), `copilot_path`, `copilot_accepted`.
- [x] `runner/from_settings.rs`: builds it when chosen and accepted, saying what's missing.
- [x] `ui/settings.rs`, `ui/settings_form.rs`: **GitHub Copilot (your Copilot plan, experimental)** as the fourth provider. Its rows: **Model** (from `models.list`, or typed), and a status line: "Copilot CLI 1.0.100 at C:\…\copilot.exe, signed in as coccor." or "…, not signed in: run copilot login." or "The Copilot CLI isn't installed: winget install GitHub.Copilot." The usage line and cap apply. The first-use screen says captures go to GitHub and on to the model's provider, use the plan's AI credits, and that the backend is experimental and for personal use. As built: the status line puts the account first, "Copilot CLI 1.0.100, signed in as coccor; 198 of 200 chat requests left this month. At C:\Users\corne\AppData\Roaming\npm\copilot.cmd.", naming npm's `copilot.cmd` rather than the program deep in its package, which didn't fit; the chat requests come from `account.getQuota` when the plan counts them. It is found off the UI thread when Copilot is shown, and fills the model list with `models.list`, keeping the model chosen. A `gh`-only login says it offers no models and to run `copilot login`. The address and key rows are hidden; `copilot_path` is only in the file. Checked by a screenshot against the real CLI, which sends no prompt. Found on the way: in M6, a few long strings had lost their line continuations, leaving runs of spaces in the Claude Code first-use screen and its "claude update" pause; both are fixed, and a test now checks every first-use screen for doubled spaces.
- **Tests:** settings round trip; the form's checks; consent asked once; `build` with a fake CLI path; the status line from fixtures and the fake CLI; the Settings window with the fake CLI (status, model list, saved without a key).

### Part 5: End to end and live

- [x] `runner/end_to_end.rs`: a capture with a screenshot written through the fake CLI: the messages it was given, the note written, the tokens in the ledger.
- [x] `examples/live_note.rs`: `--copilot <model>`, whatever the settings say. A dry run found the real CLI and built the request, sending nothing.
- [x] Measure (with your approval): a text capture, a screenshot and a link on two models your plan offers; time per note, tokens and AI credits, against Claude Code's 4–8 s.

  **Measured 2026-10-20** with `live_note --copilot auto` on Copilot CLI 1.0.100, signed in with Copilot Free, which offers only Auto; Auto chose GPT-6 Luna each time. 3 of the plan's 200 monthly chat requests. Times include starting the CLI; input counts what the CLI reported, cache writes included.

  | Capture | Time | Input (written to the cache) | Output | AI units |
  | --- | --- | --- | --- | --- |
  | The sample text | 7.8 s | 894 (0) | 276 | 0.023 |
  | The M2 chart screenshot | 9.3 s | 1,789 (1,786) | 427 | 0.044 |
  | A Wikipedia link (6,138 characters read) | 9.8 s | 2,237 (2,234) | 383 | 0.047 |

  All three wrote good notes: the chart was read exactly (`ledger-events`, the 0.8k baseline, the batch at 02:05, the 39.8k peak at 02:12, back by about 02:30), and the link note is drawn from the article. A note takes 2–3 s longer than through Claude Code (4.4–7.5 s), mostly the CLI starting (0.6–1.7 s) and GPT-6 Luna's reasoning at medium effort (68–121 reasoning tokens); no reads from the cache were seen, since each note is a new session. FastCapture counts these tokens in the ledger and toward the cap; on Free each note is one chat request whatever its size.

### Part 6: Polish and docs

- [ ] README: the Copilot provider, what it needs, what is sent and to whom, credits, experimental.
- [ ] Spec: §8.5 brought in line with what was found; §18.7 answered; §19 notes for M7.

## Testing approach

- **Pure:** framing, the session and message requests, reading events and errors.
- **Processes:** a fake CLI built as a test helper; no test runs the real CLI.
- **Windows:** the Settings window driven through messages.
- **Live:** only Part 1's checks and `examples/live_note.rs`, run by hand with your approval.

## Order and dependencies

Part 1 comes first, since the protocol's wire names and the tool, schema and image behaviour shape everything after it; 3 needs 2; 4 needs 3; 5 and 6 come last. If Part 1 finds a session can't run with no tools, M7 stops there and the spec says why.
