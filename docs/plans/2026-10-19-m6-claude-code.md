# M6: Claude Code CLI Backend Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Each part ends with `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passing, and one commit. Code comments never cite the spec.

**Goal:** Cloud mode gets a third provider, **Claude Code (experimental)**, for people with a Claude Pro or Max plan who don't want an API key. FastCapture runs the user's own installed, unmodified `claude` command once per note, with no tools, and reads the note from its JSON result. Usage comes out of the user's plan; when the plan's limit is reached, the queue pauses until the reset time.

**Spec:** `docs/specs/2026-10-18-fastcapture-design.md` §8.1, §8.4, §13, §15, §18.3.

**Found before planning (2026-10-19, free):** Claude Code is installed at `%USERPROFILE%\.local\bin\claude.exe`, which isn't on `PATH`, and is version **2.0.42**. Its `--help` has `-p`, `--output-format json|stream-json`, `--input-format stream-json`, `--tools ""`, `--system-prompt`, `--model`, `--strict-mcp-config` and `--setting-sources`, but not `--json-schema`, `--max-turns`, `--no-session-persistence` or `claude auth status`, which §8.4 assumed. Newer versions may have them; updating is your call (`claude update`).

**Then, after you signed in (2026-10-19):** signing in updated Claude Code to **2.1.303**, which has `--json-schema`, `--no-session-persistence`, `--effort`, `--bare` and `claude auth status` (JSON: `loggedIn`, `authMethod: "claude.ai"`, `subscriptionType`), but no `--max-turns`. Part 1 changes the decisions below where marked.

**Decided for M6:**

- **The command is built from what the installed CLI offers**, read once per session from `claude --help` and `claude --version`: `claude -p --output-format stream-json --verbose --input-format stream-json --tools "" --strict-mcp-config --setting-sources "" --model <model> --system-prompt <rules>`, plus `--json-schema <schema>` and `--no-session-persistence` when the CLI lists them. Without `--json-schema`, the schema goes in the system prompt, as for OpenAI-compatible servers that refuse `response_format`. (Changed by Part 1: stream-json input needs stream-json output; `--max-turns 1` isn't used, since structured output takes two turns; no settings at all are loaded.)
- **Not `--bare`**: it would skip hooks and `CLAUDE.md` in one flag, but it never reads the plan's login, only an API key.
- **`--system-prompt` replaces Claude Code's own prompt** rather than appending to it (§8.4 said append): the coding agent's prompt is long, says nothing useful about notes, and would be paid for on every capture.
- **It runs in an empty scratch folder** (`%LocalAppData%\FastCapture\claude-code\`), so no project's `CLAUDE.md` or settings are read, with `--setting-sources ""`, so not even the user's own settings and hooks are loaded. Every `ANTHROPIC_*` variable, `CLAUDECODE` and every `CLAUDE_CODE_*` variable are removed from its environment, so it uses the plan's login and a host session can't redirect it. It runs with no console window and is killed after 5 minutes.
- **The CLI is found** at `claude_code_path` when set, else on `PATH` (`claude.exe`, then `claude.cmd`), else at `%USERPROFILE%\.local\bin\claude.exe`, where the native installer puts it.
- **Usage isn't counted in the ledger or the cap**: it isn't billed per token, and the plan has its own limits. The note's `model:` is `cloud/claude-code/<model>`.
- **A usage limit pauses the whole queue until the reset time** the CLI reports (a `rate_limit_event`'s `resetsAt`, else the time in the error text), or for 30 minutes, then resumes on its own. This needs a timed pause in the worker (`BackendError::PauseUntil`), which the Claude API's rate limits don't use.
- **Models are Claude Code's aliases**, `sonnet` (the default), `opus` and `haiku`; `claude_code_model` takes any model name the CLI accepts.

## What M6 does not do

- **Signing in.** The user signs in to Claude Code themselves (`claude`, then `/login`). FastCapture never reads, copies or stores Claude.ai credentials, and never starts an interactive session.
- **Updating Claude Code**, or installing it.
- **Counting the plan's usage**: Claude Code's own `/usage` shows it.
- **The GitHub Copilot backend** is M7.

## Architecture

```
src/
  model/
    claude_code/
      mod.rs          the backend: the stream-json request, the JSON result, errors; label cloud/claude-code/<model>
      cli.rs          finding claude.exe, its version and flags (from --help), the command line
      run.rs          running it: stdin, stdout, no window, a scratch folder, a cleaned environment, a timeout
    backend.rs        (changed) `BackendError::PauseUntil`
  runner/
    worker.rs         (changed) a timed pause that lifts itself
    from_settings.rs  (changed) `backend=claude_code`
  config/
    settings.rs       (changed) `claude_code_model`, `claude_code_path`, `claude_code_accepted`
  ui/
    settings.rs       (changed) the third provider: model, where the CLI is and its version, sign-in state
    settings_form.rs  (changed) its checks and its first-use screen
```

## Parts

### Part 1: Checks on the real CLI (live, with your approval)

These run the installed `claude` against your Claude plan: a few short requests, no API bill. Each finding is written here before the code depends on it.

- [x] Whether to update first: you chose to keep 2.0.42 as it is.
- [x] A text request with the command above: the shape of the `json` result (`result`, `is_error`, `subtype`, `usage`, `total_cost_usd`, `session_id`), the exit code, and the time it takes.

  **Found:** exit 0 in 3 s (1.1 s of it the API) on haiku. The `json` result has `type: "result"`, `subtype`, `is_error`, `result` (the reply's text), `stop_reason`, `num_turns`, `usage` (`input_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`, `output_tokens`), `total_cost_usd` (what it would cost at API prices), `modelUsage` keyed by the full model id, `api_error_status` and `terminal_reason`. The system prompt was written to the cache (677 tokens, 1-hour cache).
- [x] An image as a base64 `image` block in a `stream-json` user message: whether the model sees it (§18.3).

  **Found:** it does. The M2 chart screenshot (`{"type":"user","message":{"role":"user","content":[{"type":"image","source":{"type":"base64",…}},{"type":"text",…}]}}` on stdin) was described exactly: the group, the 0.8k baseline, the 39.8k peak at 02:12, the recovery by 02:30. `--input-format stream-json` refuses to run without `--output-format stream-json` ("requires output-format=stream-json"), so the reply is a stream of JSON lines: `system` (`init`, with the session's tools and model), `assistant`, `user`, `rate_limit_event`, and last a `result` line shaped like the `json` result above. 4 s in all; 2,137 input tokens written to the cache, 248 output.
- [x] `--json-schema`, if the CLI has it: where the structured reply appears in the result.

  **Found:** the CLI adds a `StructuredOutput` tool; the model calls it, so the session takes 2 turns (`stop_reason: "tool_use"`, `num_turns: 2`), and the `result` line carries the reply as `structured_output` (an object) as well as `result` (its JSON text).
  - **The `rate_limit_event`** reports the plan's windows: `status` (`allowed`), `rateLimitType` (`five_hour`), `resetsAt` (Unix seconds), and `unifiedWindows.five_hour` / `seven_day` with `utilization` (0.25 here) and `resetsAt`. When a limit is reached, its `resetsAt` is the time to pause until.
- [x] Signed out: found without trying, since the installed 2.0.42 isn't signed in (2026-10-19). With stdin closed it answers in 5–7 s, exit code 1, with `{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login","total_cost_usd":0,"usage":{…all 0}}` on stdout and nothing on stderr. So `is_error` decides, not `subtype` or the exit code alone. Other versions say "Not logged in · Please run /login".
  - **stdin must be closed or written:** with the prompt as an argument and stdin left open, `-p` waited more than 3 minutes for input. FastCapture always writes the stream-json message and closes stdin.
  - **The environment:** a process started from inside Claude Code inherits `CLAUDECODE`, `CLAUDE_CODE_*` and `ANTHROPIC_BASE_URL`; the tray app normally has none, but FastCapture removes every `ANTHROPIC_*` variable and `CLAUDECODE`/`CLAUDE_CODE_*` too, so a host session can't redirect it.
  - **The Claude desktop app bundles its own Claude Code** (`%AppData%\Claude\claude-code\<version>\…\claude.exe`, 2.1.299 here). FastCapture doesn't use it: it isn't the user's installed CLI, and its login belongs to the app.
- [x] A usage limit can't be reached on purpose, and no official example of its `-p` result was found. Reports show `is_error: true` with a `result` naming the limit, in forms such as "Claude AI usage limit reached|<unix time>" (older versions), "5-hour limit reached ∙ resets 5pm" and `grace_daily_limit_reached`. FastCapture treats an error `result` that mentions a usage, daily, weekly or 5-hour limit as a limit; it reads a Unix time after `|`, or a "resets <time>" clock time (the next one to come), and otherwise waits 30 minutes. The whole message is logged the first time.
- [x] Whether `--tools ""` really leaves no tools: the `init` line lists the session's tools, and with `--tools ""` and `--json-schema` it was only `["StructuredOutput"]`, with no MCP servers. No request was needed to ask the model. The 55 skills it lists can't run without tools.

### Part 2: Finding and running the CLI

- [ ] `model/claude_code/cli.rs`: where `claude` is (above); `claude --version` and `claude --help` read once per session with a 10 s timeout; which optional flags exist; the command line for a model, the rules and the schema. Arguments are quoted for `CreateProcessW`; a `.cmd` is run through `cmd.exe /d /c` with Rust's own batch-file escaping.
- [ ] `model/claude_code/run.rs`: runs it with stdin and stdout piped and stderr kept (first 4 KB), `CREATE_NO_WINDOW`, the scratch folder as working directory, the cleaned environment, and the timeout; reads stdout on its own thread so a large reply can't block it.
- **Tests:** a fake CLI (a `.cmd` that prints a fixed reply or a fixed help text): flags found or missing, the command line with and without `--json-schema`, the environment without the `ANTHROPIC_*` and `CLAUDE*` variables, a timeout kills it, stderr kept.

### Part 3: The backend

- [ ] `model/claude_code/mod.rs`: the stream-json user message (images as base64 `image` blocks, then the text), the stream's last `result` line read into a note (from `structured_output` when `--json-schema` was used, else from `result` with `reply::parse`), `is_error` deciding failure rather than `subtype` or the exit code, the last `rate_limit_event` kept for its reset time, and errors: not found, not signed in (pauses, naming `claude` › `/login`), a usage limit (`PauseUntil` the reset time, else 30 minutes), overloaded or offline (retry), a reply that isn't a note (asked once more, as now), anything else fails the job with the CLI's own message.
- [ ] `model/backend.rs`: `BackendError::PauseUntil { until, reason }`.
- [ ] `runner/worker.rs`: a timed pause: the status says why and until when; it lifts itself at that time, or when the settings change or you resume.
- **Tests:** results parsed from fixtures (success, `is_error`, the limit text with and without a reset time, signed out); the worker pauses until a time and goes on after it.

### Part 4: Settings and choosing the backend

- [ ] `config/settings.rs`: `backend=claude_code`, `claude_code_model` (default `sonnet`), `claude_code_path`, `claude_code_accepted`.
- [ ] `runner/from_settings.rs`: builds it when chosen and accepted, saying what's missing (no CLI found, not confirmed).
- [ ] `ui/settings.rs`, `ui/settings_form.rs`: **Claude Code (your Claude plan, experimental)** as the third provider. Its rows: **Model** (sonnet, opus, haiku, or typed), and a status line from `claude --version` and `claude auth status`: "Claude Code 2.1.303 at C:\…\claude.exe, signed in with your Claude account (Max)." or "…, not signed in: run claude, then /login." or "Claude Code isn't installed or isn't on PATH." No key. **Test** runs one test note through it. The first-use screen says captures go to Anthropic through Claude Code, use the plan's limits, and that the backend is experimental and for personal use.
- **Tests:** settings round trip; the form's checks; consent asked once for Claude Code; `build` with a fake CLI path.

### Part 5: End to end and live

- [ ] `runner/end_to_end.rs`: a capture with a screenshot written through the fake CLI: the stream-json it was given, the note written, nothing in the ledger.
- [ ] `examples/live_note.rs` works with it.
- [ ] Measure with `examples/live_note.rs` (with your approval): a text capture, a screenshot and a link on `haiku` and `sonnet`; the time per note (the CLI's start-up included) against the Claude API's 2–4 s.

### Part 6: Polish and docs

- [ ] README: the Claude Code provider, what it needs (an installed, signed-in Claude Code), what is sent and to whom, limits, experimental.
- [ ] Spec: §8.4 brought in line with what was found; §18.3 answered; §19 notes for M6.

## Testing approach

- **Pure:** the command line, the stream-json message, parsing results and errors.
- **Processes:** a fake `claude.cmd` in a scratch folder; no test runs the real CLI.
- **Windows:** the Settings window driven through messages.
- **Live:** only Part 1's checks and `examples/live_note.rs`, run by hand with your approval.

## Order and dependencies

Part 1 comes first, since its findings shape Parts 2 and 3; 3 needs 2; 4 needs 3; 5 and 6 come last.
