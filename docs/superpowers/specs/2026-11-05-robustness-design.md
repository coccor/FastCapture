# Robustness: no lost captures, no crash loops, diagnosable failures

Date: 2026-11-05
Status: draft design, awaiting review

## Why

FastCapture is heading for the Microsoft Store, and a Store user who hits a crash files a
one-star review, not a bug report. An audit of the code at `cafa489` found that saved
captures are safe, but:

- a PDF that panics the PDF parser kills the app, and the interrupted job is queued again
  on every start, so the app crashes in a loop;
- what is in an open capture box is lost when the app dies, and saving a draft can lose
  the previous one;
- filing moves a note to whatever folder its `suggested_home:` names, even outside the
  notebook;
- a model slower than its timeout is retried forever;
- a crash leaves nothing in the log, and the tests never run the build users get.

The findings, with file and line references, are in the notebook
(`C:\Projects\Notes\FastCapture\inbox`): "A capture can be lost only before it is saved,
or stranded after a crash", "A malformed PDF can crash FastCapture on every start, because
release builds abort on panic", "Transient model failures are retried forever, and a
folder home is not kept inside the notebook", "The test suite never runs the release
build, so panic = abort bugs pass CI" and "A crash leaves no trace in the log, and the log
holds note titles and URLs despite the README".

## What the user asked for

Make FastCapture robust before publishing it to the Store, in the order the audit ranked:
first what can make the app unusable or move files, then what can lose a capture, then the
outside world, the tests and diagnostics.

Out of scope: a local spool for unreachable notebooks (the box keeps a draft and says why
today), code signing, MSIX packaging, the GitHub issues opened on 2026-11-05 other than
#11 and #12, and any new feature.

## Decisions

Proposed; change them before approving.

1. **Panics are caught only where code asks for it.** Release builds switch to
   `panic = "unwind"`, so `catch_unwind` works again. The panic hook aborts the process
   after logging unless the panic is inside a guard (section 1.3), so a panic anywhere else
   still ends the app at once instead of leaving a dead thread behind. Rejected: a child
   process for PDFs (stronger isolation, much more code, a second process to package for
   the Store).
2. **The log stops holding anything from a capture**, as the README promises (section 5.3).
   Rejected: weakening the README.
3. **No spool.** An unreachable notebook keeps failing with a draft kept.

## 1. Stop the worst failures

### 1.1 A job interrupted twice fails

`Job` gains `interrupted: u32` (JSON `"interrupted"`, absent means 0, so old queue files
still read). `Store::recover` increments it for each `Working` job it finds. A job that
reaches 2 is set to `Failed` with `last_error`:

> FastCapture stopped while writing this note, twice. Press R to try again.

Otherwise it is queued again, as today. **R** in the review window resets `interrupted` to
0. A job that finishes, fails or is paused normally keeps its count; only `recover` and
**R** change it.

`attempts` isn't reused: it counts model tries, and a capture retried three times while
offline must not fail on its first crash.

### 1.2 The panic hook

`main` installs a hook with `std::panic::set_hook` before anything else. It writes one log
line:

```
panic: <message> at <file>:<line>:<column> (FastCapture <version>, thread <name or id>)
```

The message is the panic's payload when it is a `&str` or `String`, else `(no message)`.
Then, unless the panicking thread is inside a guard (1.3), it calls
`std::process::abort()`.

The hook logs only the payload and location. The `cfg!(test)` rule in `log::line` keeps
tests out of the real log.

### 1.3 Guards that survive a panic

`[profile.release]` sets `panic = "unwind"`. The cost in executable size is measured and
written in the PR. A thread-local `GUARDED: Cell<u32>` and one helper:

```rust
/// Runs `f`, turning a panic into `Err`. Only for code that reads untrusted input
/// (a PDF, a page, a document) and holds no state that a half-finished run could corrupt.
pub fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String>
```

`guarded` increments `GUARDED`, runs `f` under `catch_unwind(AssertUnwindSafe(..))`,
decrements it, and turns a payload into `Err("it couldn't be read")`. The hook still logs a
guarded panic, prefixed `panic (recovered):`, and doesn't abort.

`extract::pdf::read` uses `guarded` in place of its own `catch_unwind`. The `.docx` and
page extraction (`extract::docx`, `extract::page`) are wrapped too, since they parse
untrusted input.

A panic that unwinds out of an `extern "system"` window procedure already aborts in Rust,
after the hook has logged it, so window code needs no change.

### 1.4 The release smoke test

`tests/release_smoke.rs`, run in CI by a new step after the debug tests:

```
cargo build --locked --release
cargo test --locked --release --test release_smoke -- --test-threads=1
```

The test runs `target/release/fastcapture.exe` as a child process, through
`fastcapture extract <file>`. That is a hidden command that runs the same extraction a
capture gets and prints a one-line summary (kind, characters, pages, or the error). The
test checks:

- `tests/fixtures/bad/panics.pdf`, a PDF that makes `pdf_extract` panic: exit code 0,
  output says it couldn't be read;
- a truncated PDF, a `.docx` that isn't a zip, an HTML page of 5 MB of nested `<div>`s:
  exit code 0.

The fixture PDF comes from a short fuzz run of `pdf_extract::Document::load_mem` (section
4.5) or a published crasher from its issue tracker. The tests are `#[ignore]`d, so a plain
`cargo test` (which has no release exe) skips them, and CI's smoke step runs them with
`--ignored`. They find the exe at `FASTCAPTURE_EXE`, else
`target/release/fastcapture.exe`, and fail when it is missing.

`extract` is not listed in `--help`, and its output holds no document text, only counts.

### 1.5 Filing stays inside the notebook

`file::file_note` refuses a `Target::Folder` or `Target::Topic` path that is absolute, has
a drive or UNC prefix, or contains a `..` component. It also refuses one whose joined path,
normalized, is not under `root`. The error, shown in the review window:

> The suggested home "<home>" is outside the notebook, so the note stays in the inbox.

`ui::review::suggested_target` applies the same check before offering Enter. The home
picker only ever offers folders inside the notebook, so it is unaffected.

## 2. Never lose a capture

### 2.1 Drafts are saved atomically

`draft::save` writes the new draft into a sibling folder `draft.new` (cleared first), then
swaps it in: `draft` is renamed to `draft.old`, `draft.new` to `draft`, and `draft.old` is
removed. `draft::load` reads `draft`, and if it is missing, `draft.old`. So a crash at any
point leaves the old draft or the new one, never neither. Each file is written with
`fs::write` then `sync_all` on the file.

An empty capture still clears the draft.

### 2.2 Drafts are saved while you type

The box saves its draft 1.5 s after the last change: text typed or pasted, or an
attachment added or removed. (A draft doesn't record the notebook chosen with Ctrl+N, so
that is no change.) The edit gets no `EN_CHANGE` today, so the timer is reset from the
places that already see each change: the edit subclass's `WM_CHAR` and `WM_PASTE`,
`attach`, `remove_item`, and autocorrect's replacement. It also saves when Windows asks to end the session: the tray window
handles `WM_QUERYENDSESSION` (save, return TRUE) and `WM_ENDSESSION` (save if `wParam`).
The tray window forwards both to an open box with `SendMessageW`.

Saving a draft never shows an error in the box. A failure is logged as today.

Ctrl+Enter that succeeds still clears the draft. Esc still saves it.

### 2.3 Crash leftovers are swept

At start, after `recover`, a background sweep looks for `*.fastcapture-tmp` files (the
saver's temp files, `.<name>.<pid>.<n>.fastcapture-tmp`) in:

- each known notebook (`notebook` and `recent_notebook`): the raw folder, the attachments
  folders, the capture folder, and any folder the index scan saw holding one (the scan
  collects them as it walks);
- `%LocalAppData%\FastCapture\queue` and `index`.

It skips a temp whose `<pid>` is a running process, or that was modified in the last 10
minutes. For the rest:

- **Target missing, in an attachments folder:** deleted. Attachments are written before the
  raw source, so no raw source links it.
- **Target missing, anywhere else:** renamed to the target, with `MoveFileExW` without
  replace. A raw source restored this way is queued by the orphan scan that follows. A note
  restored this way is the copy `save_atomic` deliberately left when a replace deleted the
  destination but couldn't move the temp in.
- **Target present:** deleted.

Each restore and the number deleted are logged, by folder kind and count, not by name.

### 2.4 The orphan scan covers every known notebook

`enqueue_orphans` runs for `notebook` and each `recent_notebook` that exists, one after the
other on the same background thread.

### 2.5 Large attachments are streamed

`saver` gains `save_atomic_new_from(path, reader: &mut dyn Read)`. It copies into the temp
file with `io::copy` and an 64 KB buffer, then syncs and swaps as `save_atomic_new` does.
`capture::file::store` uses it for `Attachment::File`, opening the source with
`File::open`. Pasted PNGs keep the byte-slice path.

## 3. The outside world

### 3.1 Timeouts stop being retried forever

Today a timeout is only a WinHTTP error code inside a `Transient` message, and every
transient error is retried with no limit.

- `HttpError` gains `TimedOut(String)`, returned for `ERROR_WINHTTP_TIMEOUT` (and for the
  total deadline of 3.2). Code that matches `Transient` treats `TimedOut` the same way.
- `BackendError` gains `TimedOut(String)`. `from_http` maps `HttpError::TimedOut` to it.
  Claude Code's time limit (`RunError::TimedOut`) and Copilot's session deadline,
  start timeout and call timeout return it too.
- `Job` gains `timeouts: u32` (JSON `"timeouts"`, absent means 0). `process_with` treats
  `TimedOut` like `Retry { after: None }`, and increments `timeouts`. At 3 the job fails
  with:

  > The model didn't answer within <n> s, three times. Choose a faster model, or raise
  > local_timeout for a local one, then press R.

Other transient errors keep today's backoff with no limit. **R** resets `timeouts`.

A new `fastcapture.ini` key, `local_timeout`, is a local model's time limit per note in
seconds. The default is 300, today's fixed value (`OpenAi::local`), and values from 30 to
3600 are accepted; others are a settings warning and keep the default. The vision probe's
own 60 s limit (`model::local::reads_images`) is unchanged.

### 3.2 Requests have a total deadline

`net::http::Request::timeout` becomes the limit for the whole request, not each step.
`send` computes a deadline when it starts. Before each `WinHttpReadData` it sets the
remaining time with `WinHttpSetTimeouts` and fails with `ERROR_WINHTTP_TIMEOUT`'s transient
error once the deadline has passed. So a page fetch takes at most 10 s, as the README says.

### 3.3 The index watcher comes back

When a notebook's `Watcher` reports `failed`, `index::service::run` drops it and tries to
start a new one after 5 s, 30 s, 2 minutes, then every 10 minutes. It rescans after each
try, whether or not the watcher started, so changes made meanwhile are picked up. The
hourly wait (today a cache-save timer only) also rescans when no watcher is running.

### 3.4 CLI output is capped

`model::claude_code::run` reads stdout up to 32 MB (`net::http::MAX_RESPONSE_BYTES`).
Past that it ends the job object and returns an error: "Claude Code wrote more than 32 MB".
Stderr keeps its first 4 KB while reading and discards the rest, still draining the pipe,
as the Copilot CLI's stderr reader already does (`model::copilot::cli`).

## 4. Tests you can trust

### 4.1 Skipped autocorrect tests say so

GitHub's Windows Server runners have the en-US spell checker but no autocorrect list, so
the autocorrect tests (`platform::spell`, and seven in `ui::capture_box` through
`no_speller`) always skip in CI. Making a skip fail there would break CI, so instead:

- every skip prints the same marker, `skipped (no autocorrect list):` and the test's name;
- the CI test step counts the markers in its output and writes the count to the job
  summary, so "autocorrect untested in CI" is visible on every run;
- `docs/releasing.md` says to run `cargo test --lib autocorrect spell` on a desktop
  Windows before a release.

### 4.2 The pipe test uses its own pipe (#12)

`ipc::pipe::Server::create_at(name: &Path)` and `send_to(name: &Path, request)`, which
`create` and `send` call with `pipe_name()`. The test uses
`\\.\pipe\FastCapture-test-<pid>-<counter>`, so it passes while the tray app runs.
`names_are_scoped_to_the_session` is unchanged.

### 4.3 (dropped)

The clipboard tests already save and restore the clipboard
(`platform::clipboard::tests::Backup`).

### 4.4 UI tests serialize themselves (#11)

A `static UI_TESTS: Mutex<()>`, held by every test that creates a window (a helper,
`ui::test_support::lock()`, in each such test's first line). CI's test step drops
`--test-threads=1` once a plain `cargo test` passes three times in a row on the runner.

### 4.5 Fuzzing

`fuzz/`, a `cargo fuzz` project, nightly only and not in CI. Its targets are front matter
(`notebook::frontmatter`), page HTML to Markdown (`extract::page`), a model reply
(`note::reply`), the Copilot message decoder (`model::copilot::rpc::Decoder`) and PDF
(`extract::pdf`). Each crash found becomes a fixture and a unit test. `docs/fuzzing.md`
says how to run it.

### 4.6 Failure-path tests

- `saver` gains a test-only fault hook (`#[cfg(test)]`) that fails the n-th write, sync or
  swap. `capture::file` tests use it to check that a failure at each step leaves either
  the whole capture or nothing, and that `draft::save` always leaves a loadable draft.
- A worker test kills the worker mid-job (drops it with the job `Working`), runs `recover`
  twice, and checks the job ends `Failed` with the message from 1.1.

### 4.7 Arm64

A CI job on `windows-11-arm` runs the tests for `aarch64-pc-windows-msvc`.

## 5. Diagnostics and long runs

### 5.1 Symbols

`[profile.release]` adds `debug = "line-tables-only"`. The PR checks that the exe stays
the same size give or take a few KB, with `strip = true` still applied, and that
`fastcapture.pdb` is produced. The package job uploads the `.pdb` as a CI artifact and a
release asset named `FastCapture-<version>-windows-<arch>.pdb.zip`. It is not put in the
user's zip. `docs/releasing.md` gains the step.

### 5.2 A fuller start line

```
FastCapture <version> started: Windows <build> <arch>, mode <local|cloud|none>, backend <name>, notebook <local|network|onedrive|missing>
```

The notebook kind comes from `GetDriveTypeW` (remote means network) and whether the path
is under `%OneDrive%`, `%OneDriveConsumer%` or `%OneDriveCommercial%`. Never the path.

### 5.3 The log holds nothing from a capture

Many log lines carry paths and URLs indirectly, inside error messages, so the scrubbing is
in `log::line` itself:

- each `http://` or `https://` URL becomes its host (`https://example.com/a/b` →
  `example.com`);
- a path under a known notebook becomes `<notebook>\<first folder>\…` (`D:\Notes\inbox\Lag
  spikes.md` → `<notebook>\inbox\…`). The app tells the log its notebooks with
  `log::set_notebooks(&[PathBuf])` whenever settings are applied. Matching ignores case
  and `/` versus `\`;
- the rest of `%USERPROFILE%` becomes `%USERPROFILE%\…`, so paths outside a notebook keep
  their location but not their file names.

On top of that, call sites that log a name on its own change:

- `runner: wrote <path>` becomes `runner: wrote <capture_id>`, and
  `queue: finding <raw> in <notebook>` becomes `queue: finding <capture_id>`;
- `request: <attachment name>: <error>` becomes `request: attachment <n> (.<extension>):
  <error>`.

The review window and notifications still show full reasons; only the log is scrubbed.

### 5.4 One line per repeated retry

The worker logs a retry reason only when it differs from that job's previous one, and
otherwise counts it. When the reason changes or the job finishes, it logs
`runner: <capture_id>: <n> more retries, same reason`.

### 5.5 Copy diagnostics

The tray menu gains **Copy diagnostics** after **Open notebook folder**. It puts on the
clipboard:

```
FastCapture <version>, Windows <build> <arch>
Mode: <mode>, backend: <backend>, model: <model name>
Notebook: <kind>, notes indexed: <n>
Queue: <n> queued, <n> working, <n> failed, <n> done
Paused: <no | reason>

Last 100 log lines:
<lines>
```

and a notification says "Diagnostics copied. Paste them into your bug report." Keys,
endpoints and paths are never included; the model name is. `.github/ISSUE_TEMPLATE/bug.yml`
asks for them.

### 5.6 Sleep and resume

The tray window handles `WM_POWERBROADCAST` `PBT_APMRESUMEAUTOMATIC`. It wakes the worker
and calls a new `Indexes::refresh_all()`, which marks each open notebook dirty so it
rescans, and restarts a watcher that has stopped (3.3). The Copilot server is started for
each capture and stopped after it, so it needs nothing.

### 5.7 The soak run

`examples/soak.rs <hours>` starts the release exe against a scratch notebook and a fake
local model: `net::test_server`, made `#[doc(hidden)] pub` (as `extract::pdf::build`
is) and given a mode that answers OpenAI's chat completions with a fixed note. It runs
`fastcapture add` every 2 minutes with text, a PNG or a small PDF, in turn. Every minute it
writes `elapsed, handles, gdi, user, private bytes` (`GetProcessHandleCount`,
`GetGuiResources`, `GetProcessMemoryInfo`) to `soak.csv`. It ends with a verdict: flat, or
growing per hour. It is run by hand for 48 h before a Store submission, with a few sleep
and resume cycles and a move of the box between monitors with different scales.
`docs/releasing.md` gains the step.

## Order of work

One PR per line, in this order. Each one leaves `cargo fmt`, Clippy and the tests green.

1. 1.1 interrupted jobs fail; 1.2 panic hook
2. 1.3 guards and `panic = "unwind"`; 1.4 release smoke test with the PDF fixture
3. 1.5 filing stays inside the notebook
4. 4.1 skipped tests say so; 4.2 pipe test
5. 2.1 atomic drafts; 2.2 drafts while typing and at end of session
6. 2.3 sweep; 2.4 orphan scan for every notebook; 2.5 streamed attachments
7. 3.1 timeouts and `local_timeout`; 3.2 total deadline
8. 3.3 watcher re-arm; 3.4 CLI output cap
9. 4.4 UI test lock (#11); 4.6 failure-path tests; 4.7 Arm64
10. 5.1 symbols; 5.2 start line; 5.3 log scrubbing; 5.4 retry log
11. 5.5 Copy diagnostics; 5.6 sleep and resume
12. 4.5 fuzzing; 5.7 soak run

## Testing

Each section names its tests. Beyond those:

- `cargo test` with the default thread count passes after step 9.
- The release smoke test passes in CI from step 2 on.
- Before the Store submission, a 48 h soak run is flat for handles and GDI objects, and
  private bytes grow by less than 1 MB an hour after the first hour.

## README

- **A note, written for you:** a page is fetched "within 10 s" stays true (3.2).
- **Local or Cloud:** a local model gets 5 minutes per note, changed with
  `local_timeout`; a capture whose model times out three times fails and says so (3.1).
- **Make it yours:** the `local_timeout` row.
- **Where things go:** the log's description stays "never holds what you captured", now
  true (5.3).
- **Capture box:** what's in the box is kept as a draft while you type, so a crash or a
  restart doesn't lose it (2.2).
- **Tray menu** row: adds Copy diagnostics (5.5).
- **Building from source:** the release smoke test and `examples/soak.rs`.
