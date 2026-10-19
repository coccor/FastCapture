# M6: Claude Code CLI Backend Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Each part ends with `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passing, and one commit. Code comments never cite the spec.

**Goal:** Cloud mode gets a third provider, **Claude Code (experimental)**, for people with a Claude Pro or Max plan who don't want an API key. FastCapture runs the user's own installed, unmodified `claude` command once per note, with no tools, and reads the note from its JSON result. Usage comes out of the user's plan; when the plan's limit is reached, the queue pauses until the reset time.

**Spec:** `docs/specs/2026-10-18-fastcapture-design.md` §8.1, §8.4, §13, §15, §18.3.

**Found before planning (2026-10-19, free):** Claude Code is installed at `%USERPROFILE%\.local\bin\claude.exe`, which isn't on `PATH`, and is version **2.0.42**. Its `--help` has `-p`, `--output-format json|stream-json`, `--input-format stream-json`, `--tools ""`, `--system-prompt`, `--model`, `--strict-mcp-config` and `--setting-sources`, but not `--json-schema`, `--max-turns`, `--no-session-persistence` or `claude auth status`, which §8.4 assumed. Newer versions may have them; updating is your call (`claude update`).

**Decided for M6:**

- **The command is built from what the installed CLI offers**, read once per session from `claude --help` and `claude --version`: `claude -p --output-format json --input-format stream-json --tools "" --strict-mcp-config --model <model> --system-prompt <rules>`, plus `--json-schema <schema>`, `--max-turns 1` and `--no-session-persistence` when the CLI lists them. Without `--json-schema`, the schema goes in the system prompt, as for OpenAI-compatible servers that refuse `response_format`.
- **`--system-prompt` replaces Claude Code's own prompt** rather than appending to it (§8.4 said append): the coding agent's prompt is long, says nothing useful about notes, and would be paid for on every capture.
- **It runs in an empty scratch folder** (`%LocalAppData%\FastCapture\claude-code\`), so no project's `CLAUDE.md` or settings are read, with `--setting-sources` limited to the user's own settings when the CLI offers it. `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` are removed from its environment, so it uses the plan's login. It runs with no console window and is killed after 5 minutes.
- **The CLI is found** at `claude_code_path` when set, else on `PATH` (`claude.exe`, then `claude.cmd`), else at `%USERPROFILE%\.local\bin\claude.exe`, where the native installer puts it.
- **Usage isn't counted in the ledger or the cap**: it isn't billed per token, and the plan has its own limits. The note's `model:` is `cloud/claude-code/<model>`.
- **A usage limit pauses the whole queue until the reset time** the CLI reports, or for 30 minutes, then resumes on its own. This needs a timed pause in the worker (`BackendError::PauseUntil`), which the Claude API's rate limits don't use.
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

- [ ] Whether to update first: compare `--help` after `claude update` (only if you choose to update).
- [ ] A text request with the command above: the shape of the `json` result (`result`, `is_error`, `subtype`, `usage`, `total_cost_usd`, `session_id`), the exit code, and the time it takes.
- [ ] An image as a base64 `image` block in a `stream-json` user message: whether the model sees it (§18.3).
- [ ] `--json-schema`, if the CLI has it: where the structured reply appears in the result.
- [ ] Signed out, using a scratch `CLAUDE_CONFIG_DIR` (no login, nothing sent): the message and exit code.
- [ ] A usage limit can't be reached on purpose: its wording is taken from Claude Code's docs and issues, matched loosely, and logged in full the first time it is seen.
- [ ] Whether `--tools ""` really leaves no tools, by asking the model to list files.

### Part 2: Finding and running the CLI

- [ ] `model/claude_code/cli.rs`: where `claude` is (above); `claude --version` and `claude --help` read once per session with a 10 s timeout; which optional flags exist; the command line for a model, the rules and the schema. Arguments are quoted for `CreateProcessW`; a `.cmd` is run through `cmd.exe /d /c` with Rust's own batch-file escaping.
- [ ] `model/claude_code/run.rs`: runs it with stdin and stdout piped and stderr kept (first 4 KB), `CREATE_NO_WINDOW`, the scratch folder as working directory, the cleaned environment, and the timeout; reads stdout on its own thread so a large reply can't block it.
- **Tests:** a fake CLI (a `.cmd` that prints a fixed reply or a fixed help text): flags found or missing, the command line with and without `--json-schema`, the environment without the API key variables, a timeout kills it, stderr kept.

### Part 3: The backend

- [ ] `model/claude_code/mod.rs`: the stream-json user message (images as base64 `image` blocks, then the text), the JSON result read into a note (from the structured output when `--json-schema` was used, else from `result` with `reply::parse`), and errors: not found, not signed in (pauses, naming `claude` › `/login`), a usage limit (`PauseUntil` the reset time, else 30 minutes), overloaded or offline (retry), a reply that isn't a note (asked once more, as now), anything else fails the job with the CLI's own message.
- [ ] `model/backend.rs`: `BackendError::PauseUntil { until, reason }`.
- [ ] `runner/worker.rs`: a timed pause: the status says why and until when; it lifts itself at that time, or when the settings change or you resume.
- **Tests:** results parsed from fixtures (success, `is_error`, the limit text with and without a reset time, signed out); the worker pauses until a time and goes on after it.

### Part 4: Settings and choosing the backend

- [ ] `config/settings.rs`: `backend=claude_code`, `claude_code_model` (default `sonnet`), `claude_code_path`, `claude_code_accepted`.
- [ ] `runner/from_settings.rs`: builds it when chosen and accepted, saying what's missing (no CLI found, not confirmed).
- [ ] `ui/settings.rs`, `ui/settings_form.rs`: **Claude Code (your Claude plan, experimental)** as the third provider. Its rows: **Model** (sonnet, opus, haiku, or typed), and a status line: "Claude Code 2.0.42 found at C:\…\claude.exe." or "Claude Code isn't installed or isn't on PATH." No key. **Test** runs one test note through it. The first-use screen says captures go to Anthropic through Claude Code, use the plan's limits, and that the backend is experimental and for personal use.
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
