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
- **The environment** loses `COPILOT_GITHUB_TOKEN`, `GH_TOKEN` and `GITHUB_TOKEN` only if Part 1 shows they'd override the user's own `copilot login`; FastCapture never sets a token itself.
- **Usage is counted:** Copilot charges per token, so `assistant.usage`'s tokens go in the usage ledger and count toward the token cap (`metered()` is true). The log also has each note's AI credits (from `totalNanoAiu`). The note's `model:` is `cloud/copilot/<model>`.
- **Limits and sign-in:** `quota` and `rate_limit` errors pause until the reset time when one is given, else for 30 minutes; `authentication` pauses naming `copilot login`; `authorization` and `policy` pause saying the plan or the organization doesn't allow it; a model the plan doesn't offer pauses asking to choose another in Settings.
- **The CLI is found** at `copilot_path` when set, else `copilot.exe` then `copilot.cmd` on `PATH`, else where WinGet puts it (found in Part 1).

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

- [ ] Where WinGet put `copilot.exe`, `copilot --version`, `copilot --help`, and that `--headless --stdio` answers `ping` and `status.get` (the protocol version).
- [ ] `auth.getStatus` and `models.list`, free: what they return signed in, and whether the list is only the models your plan allows.
- [ ] A text request through `session.create` and the message call: the wire names, the events in order, `assistant.usage`, the time from start to note, and whether a `gh` login alone is enough.
- [ ] With `availableTools: []` (and with every tool in `excludedTools`): the tools the session reports. No request is needed if the session lists them.
- [ ] `systemMessage` with `mode: "replace"`: whether the rules are followed and Copilot's own prompt is gone (input tokens tell).
- [ ] `responseSchema`: whether the CLI takes it and where the structured reply appears.
- [ ] The M2 chart screenshot as a base64 blob: whether the model sees it.
- [ ] Signed out (by starting it with `COPILOT_HOME` pointed at an empty folder, so your login isn't touched): the error it gives. A used-up allowance can't be reached on purpose; it is taken from the SDK's error types.
- [ ] Whether `GH_TOKEN` or `GITHUB_TOKEN` in the environment wins over `copilot login`.
- [ ] Whether the CLI saves the session anywhere under `~/.copilot` after `session.delete`.

### Part 2: The protocol and the CLI

- [ ] `model/copilot/rpc.rs`: `Content-Length` framing both ways; numbered requests and their replies; notifications handed to the caller; a reader thread so the CLI can't block on a full pipe; a timeout per request.
- [ ] `model/copilot/cli.rs`: finding `copilot` (above), its version, `copilot --headless --no-auto-update --stdio` in the scratch folder, the cleaned environment, and the protocol version check.
- [ ] `model/claude_code/run.rs`: the job object and no-window start pulled out so both CLIs use them.
- **Tests:** framing (split and joined messages, a multi-byte body), a fake CLI (a small test program speaking the protocol from a script of replies), a timeout, the CLI ending early.

### Part 3: The backend

- [ ] `model/copilot/mod.rs`: create the session, send the message with images and schema, collect events until `session.idle`, read the note from the last `assistant.message` (structured or parsed with `reply::parse`), take usage from `assistant.usage`, delete the session, stop the CLI. Errors as decided above; a reply that isn't a note is asked for once more, as now.
- **Tests:** replies and errors from fixtures (success, each `errorType`, a reply that isn't JSON, the CLI ending mid-session); usage returned by `take_usage`.

### Part 4: Settings and choosing the backend

- [ ] `config/settings.rs`: `backend=copilot`, `copilot_model`, `copilot_path`, `copilot_accepted`.
- [ ] `runner/from_settings.rs`: builds it when chosen and accepted, saying what's missing.
- [ ] `ui/settings.rs`, `ui/settings_form.rs`: **GitHub Copilot (your Copilot plan, experimental)** as the fourth provider. Its rows: **Model** (from `models.list`, or typed), and a status line: "Copilot CLI 1.0.100 at C:\…\copilot.exe, signed in as coccor." or "…, not signed in: run copilot login." or "The Copilot CLI isn't installed: winget install GitHub.Copilot." The usage line and cap apply. The first-use screen says captures go to GitHub and on to the model's provider, use the plan's AI credits, and that the backend is experimental and for personal use.
- **Tests:** settings round trip; the form's checks; consent asked once; `build` with a fake CLI path.

### Part 5: End to end and live

- [ ] `runner/end_to_end.rs`: a capture with a screenshot written through the fake CLI: the messages it was given, the note written, the tokens in the ledger.
- [ ] `examples/live_note.rs`: `--copilot <model>`.
- [ ] Measure (with your approval): a text capture, a screenshot and a link on two models your plan offers; time per note, tokens and AI credits, against Claude Code's 4–8 s.

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
