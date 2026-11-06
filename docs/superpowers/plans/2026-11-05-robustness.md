# Robustness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** No crash loops, no lost captures, no notes filed outside the notebook, no endless retries, tests that cover the release build, and failures a user can report.

**Architecture:** Mostly small changes in place. New code: `src/crash.rs` (panic hook and `guarded`), `src/capture/sweep.rs` (crash leftovers), `src/platform/process.rs` (is a pid running), `src/ui/test_support.rs` (UI test lock), `src/ui/diagnostics.rs` (the Copy diagnostics text), `tests/release_smoke.rs`, `fuzz/`, `examples/soak.rs`. Job JSON gains two optional counters, `interrupted` and `timeouts`. `HttpError` and `BackendError` gain a `TimedOut` variant.

**Tech Stack:** Rust 2024, `windows-sys` 0.61 (adds `Win32_System_ProcessStatus` in Task 26 only), `cargo-fuzz` on nightly for Task 25 only.

**Spec:** `docs/superpowers/specs/2026-11-05-robustness-design.md`

## Global Constraints

- Work on a branch from `main` (`git switch -c robustness main`), not on `winget`. Copy the spec and this plan onto it first.
- No new crates in `fastcapture` itself. `fuzz/` is its own crate, outside the workspace.
- Code comments never cite the spec, this plan, section numbers or `docs/superpowers` paths. Comments explain the code on its own terms.
- CI runs `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings` and `cargo test --locked -- --test-threads=1` (until Task 17 drops the flag). All three pass after every task.
- Job JSON stays `version: 2`. New fields are optional and default to 0, so queue files written before this plan still read, and files written after it still read in 0.1.2 (unknown fields are ignored).
- User-visible wording is exactly as given in the tasks.
- Every log line stays free of capture content. After Task 20, `log::line` scrubs on its own; until then, new log lines carry ids and counts, never paths, URLs or titles.
- Release-only behaviour is tested by `tests/release_smoke.rs` (Task 4), never by changing what debug builds do.

## Review Focus

- **Crash counting is separate from model tries.** A capture retried three times while offline, then interrupted once, is queued again, not failed. Test `a_job_interrupted_twice_fails_but_once_is_queued_again` (Task 1).
- **Only guarded code survives a panic.** After `panic = "unwind"`, a panic outside `crash::guarded` must still end the app. Test `the_hook_aborts_unless_guarded` (Task 3) and the smoke test (Task 4).
- **Containment covers every escape.** `..`, `\\server\share`, `C:\x`, a drive-relative `C:x`, `\x` and `\\?\` prefixes are all refused. Test `homes_outside_the_notebook_are_refused` (Task 5).
- **A draft save never leaves neither draft.** Test `a_failed_save_keeps_the_previous_draft` (Task 8).
- **The sweep never deletes the only copy of a note.** A temp file outside an attachments folder whose target is missing is restored, not deleted. One owned by a running process is skipped. Test `the_sweep_restores_deletes_and_skips_by_the_rules` (Task 10).
- **Every match on `HttpError::Transient` also handles `TimedOut`.** Task 13; check with `rg "HttpError::Transient"`.
- **The total deadline doesn't cut off a large download that keeps flowing within its time.** Test `a_dripping_server_hits_the_total_deadline` and the existing body-size tests (Task 14).
- **Log scrubbing keeps the cause.** `"FastCapture could not save D:\Notes\raw\x.md: Access is denied."` becomes `"... <notebook>\raw\…: Access is denied."`. Test `notebook_paths_keep_their_first_folder_and_the_error` (Task 20).

---

## PR 1: Crash loops

### Task 1: A job interrupted twice fails

**Files:**
- Modify: `src/queue/job.rs` (struct at :46-67, `queued` :71, `finding` :88, `to_json` :102-125, `from_json` :128-177)
- Modify: `src/queue/store.rs` (`recover` at :118-132, test at :275-289)
- Modify: `src/ui/app.rs` (`start_worker`, the `recover` call and log at ~:245)
- Modify: `src/ui/review.rs` (`regenerate_selected` at :891-938)

**Interfaces:**
- Produces:
  - `Job.interrupted: u32`, JSON `"interrupted"`, default 0
  - `pub const INTERRUPTED_TWICE: &str = "FastCapture stopped while writing this note, twice. Press R to try again.";` in `queue/store.rs`
  - `pub struct Recovered { pub queued: usize, pub failed: usize }` and `Store::recover(&self) -> Recovered`

- [ ] **Step 1: Write the failing tests**

In `src/queue/store.rs` tests, replace `interrupted_jobs_are_queued_again` with:

```rust
#[test]
fn a_job_interrupted_twice_fails_but_once_is_queued_again() {
    let temp = Temp::new("recover");
    let store = Store::new(temp.0.clone());
    let mut offline = job("A", "1");
    offline.attempts = 3; // three model tries while offline: not crashes
    store.put(&offline).unwrap();
    store.put(&job("B", "2")).unwrap();
    store.put(&job("C", "3")).unwrap();
    store.set_state("A", State::Working, None).unwrap();
    store.set_state("B", State::Working, None).unwrap();
    store.set_state("C", State::Failed, Some("bad key".into())).unwrap();

    assert_eq!(store.recover(), Recovered { queued: 2, failed: 0 });
    assert_eq!(store.get("A").unwrap().state, State::Queued);
    assert_eq!(store.get("A").unwrap().interrupted, 1);

    store.set_state("B", State::Working, None).unwrap();
    assert_eq!(store.recover(), Recovered { queued: 0, failed: 1 });
    let b = store.get("B").unwrap();
    assert_eq!((b.state, b.last_error.as_deref()), (State::Failed, Some(INTERRUPTED_TWICE)));
    assert_eq!(store.get("C").unwrap().state, State::Failed);
}
```

In `src/queue/job.rs` tests, extend `a_job_round_trips_through_json` with `job.interrupted = 1;` before the round trip, and add to `version_1_files_still_read`: `assert_eq!(job.interrupted, 0);`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib queue -- --test-threads=1`
Expected: compile errors: no field `interrupted`, no `Recovered`.

- [ ] **Step 3: Implement**

- `Job`: add `pub interrupted: u32` after `attempts`. Set it to 0 in `queued` and `finding`. Write it in `to_json`; read it in `from_json` exactly as `attempts` is (`as_u64`, `u32::try_from`, `unwrap_or(0)`). Fix any `Job { .. }` literal the compiler points to.
- `recover`: for each `Working` job, `interrupted += 1`. At 2 or more, state is `Failed` and `last_error` is `INTERRUPTED_TWICE`; otherwise state is `Queued` and `last_error` is kept. Write each with `put`, not `set_state`, so `attempts` doesn't change. Count each outcome.
- `app.rs`: log `queue: {queued} interrupted jobs queued again` when `queued > 0`, and `queue: {failed} jobs failed after two interruptions` when `failed > 0`.
- `regenerate_selected`: add `job.interrupted = 0;` next to `job.attempts = 0;`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib queue -- --test-threads=1` and `cargo test --lib ui::review -- --test-threads=1`
Expected: all pass.

- [ ] **Step 5: Clippy, then commit**

```bash
cargo clippy --locked --all-targets --all-features -- -D warnings
git add src/queue src/ui/app.rs src/ui/review.rs
git commit -m "Fail a capture that FastCapture stopped writing twice, instead of crashing on it again"
```

---

### Task 2: The panic hook

**Files:**
- Create: `src/crash.rs`
- Modify: `src/lib.rs` (add `pub mod crash;` alphabetically at :4-24; call `crash::install()` first thing in `run` at :32)

**Interfaces:**
- Produces:
  - `pub fn install()`: sets the hook once
  - `fn line(message: Option<&str>, location: Option<(&str, u32, u32)>, thread: &str, recovered: bool) -> String`: the log line, pure

- [ ] **Step 1: Write the failing test** in `src/crash.rs`'s tests

```rust
#[test]
fn the_line_names_the_panic_where_and_which_version() {
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(
        line(Some("index out of range"), Some(("src\\extract\\pdf.rs", 12, 5)), "worker", false),
        format!("panic: index out of range at src\\extract\\pdf.rs:12:5 (FastCapture {version}, thread worker)")
    );
    assert_eq!(
        line(None, None, "main", true),
        format!("panic (recovered): (no message) at (unknown) (FastCapture {version}, thread main)")
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib crash`
Expected: compile error, module `crash` not found.

- [ ] **Step 3: Implement**

`install` calls `std::panic::set_hook(Box::new(|info| { ... }))`:
- The message is `info.payload()` downcast to `&str`, else `String`, else `None`.
- The location is `info.location()`, as `(file, line, column)`.
- The thread is `std::thread::current().name()`, else its `id()` formatted with `{:?}`.
- Call `crate::log::line(&line(..., recovered))`, where `recovered` is `false` for now.
- Then `std::process::abort()`.

Task 3 makes the abort conditional. The hook is installed only in `run`, never in tests, because tests panic on purpose.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib crash`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add src/crash.rs src/lib.rs
git commit -m "Log a panic's message, place and version before FastCapture stops"
```

---

### Task 3: Guards that survive a panic

**Files:**
- Modify: `Cargo.toml` (`[profile.release]`: `panic = "unwind"`)
- Modify: `src/crash.rs`
- Modify: `src/extract/pdf.rs` (:57-63), `src/extract/docx.rs` (`read` at :13)
- Modify: `src/runner/pages.rs` (:58, the `page::extract` call)

**Interfaces:**
- Produces:
  - `pub fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String>`: a panic in `f` becomes `Err("it couldn't be read")`
  - `pub(crate) fn is_guarded() -> bool`: whether this thread is inside `guarded` now
  - `fn should_abort(guarded: bool) -> bool`: `!guarded`, so the decision can be tested

- [ ] **Step 1: Write the failing tests** in `src/crash.rs`

```rust
#[test]
fn a_guarded_panic_is_an_error_and_the_guard_is_left() {
    assert_eq!(guarded(|| 7), Ok(7));
    let result: Result<(), String> = guarded(|| panic!("bad pdf"));
    assert_eq!(result, Err("it couldn't be read".to_owned()));
    assert!(!is_guarded(), "the guard ends with the call, panic or not");
    let nested = guarded(|| guarded(|| is_guarded()).unwrap() && is_guarded());
    assert_eq!(nested, Ok(true));
}

#[test]
fn the_hook_aborts_unless_guarded() {
    assert!(should_abort(false));
    assert!(!should_abort(true));
}
```

In `src/extract/docx.rs` tests, add a test that `read` of 16 bytes of `0xFF` returns `Err` (it isn't a zip).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib crash extract`
Expected: compile errors: `guarded`, `is_guarded` and `should_abort` not found.

- [ ] **Step 3: Implement**

- `thread_local! { static GUARDED: Cell<u32> = const { Cell::new(0) }; }`. A small drop guard decrements it, so it is decremented even when `f` panics.
- `guarded` increments it, runs `std::panic::catch_unwind(AssertUnwindSafe(f))`, and maps `Err(_)` to the message.
- The hook (from Task 2):
  - computes `recovered = is_guarded()` and logs with it;
  - calls `abort()` only if `should_abort(recovered)`.
- `pdf::read`: replace its `catch_unwind` with `crate::crash::guarded(|| read_unguarded(bytes, max_pages)).and_then(|result| result)`.
- `docx::read`: wrap its body the same way.
- `pages.rs:58`: `crate::crash::guarded(|| page::extract(&html, &page_url))`. On `Err`, treat the page the way a page that isn't HTML is treated today, with the reason `"the page couldn't be read"`.
- `Cargo.toml`: `panic = "unwind"`. Build release before and after (`cargo build --release`) and write both sizes of `target/release/fastcapture.exe` in the commit message body.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib -- --test-threads=1`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/crash.rs src/extract src/runner/pages.rs
git commit -m "Let a PDF, Word file or page that breaks its parser fail alone, not the app"
```

---

### Task 4: The release smoke test

**Files:**
- Modify: `src/launch.rs` (`Launch` at :16-23, `parse` at :52-79; `USAGE` unchanged)
- Modify: `src/lib.rs` (`run`'s match at :43-49)
- Modify: `src/model/request.rs` (beside the private `read_file` at :481-541)
- Create: `tests/release_smoke.rs`, `tests/fixtures/bad/` (`panics.pdf`, `truncated.pdf`, `not-a-zip.docx`, `nested.html`)
- Modify: `.github/workflows/ci.yml` (`windows` job)

**Interfaces:**
- Produces:
  - `Launch::Extract(PathBuf)`, parsed from `fastcapture extract <file>` (one argument, relative to `cwd`), not listed in `USAGE`
  - `pub fn describe_file(name: &str, bytes: &[u8]) -> String` in `model::request`: one line, no document text. One of:
    - `read <kind>: <n> characters[, <p> pages]`
    - `unreadable: <reason>`
    - `other: <kind>, <size>`
  - For `.html` and `.htm` it runs `crash::guarded(|| page::extract(text, "https://example.invalid/"))` and says `page: <n> characters`.
- `run`: `Launch::Extract(path)` reads the file and prints `describe_file`, returning 0. It returns 1 with `fastcapture: <path>: <error>` on stderr when the file can't be read.

- [ ] **Step 1: Find or make the fixtures**

- `truncated.pdf`: the first 60% of `pdf::build::pdf(&[Page::Text(&["hello"])], false)`.
- `not-a-zip.docx`: 64 bytes of `0xFF`.
- `nested.html`: `<div>` repeated 200,000 times, then `x`.
- `panics.pdf`: a PDF that makes `pdf_extract::Document::load_mem` or text extraction panic. Search `pdf-extract`'s issue tracker for a reported crasher, or mutate built PDFs with a throwaway example (not committed). The example flips a few random bytes in `pdf::build::pdf(...)`'s output and calls `read_unguarded` under `catch_unwind` in a debug build until one panics, then saves that input.

**Stop condition:** if no panicking PDF turns up within 30 minutes, ship the other three fixtures, leave a `// TODO` naming the missing case in the test, and report it.

- [ ] **Step 2: Write the failing tests**

`tests/release_smoke.rs`:

```rust
//! Runs the release fastcapture.exe on inputs that break parsers. Release builds differ from
//! test builds in how panics end, so this is the only place that behaviour is checked.
//! `cargo build --release`, then `cargo test --release --test release_smoke -- --ignored`.

use std::path::PathBuf;
use std::process::Command;

fn exe() -> PathBuf {
    std::env::var_os("FASTCAPTURE_EXE").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(r"target\release\fastcapture.exe")
    })
}

fn extract(fixture: &str) -> (Option<i32>, String) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(r"tests\fixtures\bad").join(fixture);
    let exe = exe();
    assert!(exe.is_file(), "build it first: {}", exe.display());
    let output = Command::new(exe).arg("extract").arg(path).output().unwrap();
    (output.status.code(), String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
#[ignore = "needs the release exe"]
fn a_pdf_that_panics_its_parser_is_unreadable_not_fatal() {
    let (code, out) = extract("panics.pdf");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.starts_with("unreadable:"), "{out}");
}

#[test]
#[ignore = "needs the release exe"]
fn other_broken_inputs_end_cleanly() {
    for fixture in ["truncated.pdf", "not-a-zip.docx", "nested.html"] {
        let (code, out) = extract(fixture);
        assert_eq!(code, Some(0), "{fixture}: {out}");
    }
}
```

In `src/launch.rs` tests: `parse(&args(&["extract", "a.pdf"]), &cwd())` gives `Launch::Extract(r"C:\work\a.pdf".into())`, `extract` with no file or two files is an error, and `USAGE` doesn't contain `extract`.

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo build --release; cargo test --release --test release_smoke -- --ignored`
Expected: fails. `extract` is an unknown command (exit code 2).

- [ ] **Step 4: Implement** `Launch::Extract`, `describe_file` and the dispatch. `describe_file` calls `read_file` with `Budget::default()` and a scratch `scanned` vector, and formats its `FileMaterial`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo build --release; cargo test --release --test release_smoke -- --ignored` and `cargo test --lib launch`
Expected: all pass. With Task 3's `panic = "unwind"` temporarily reverted to `abort`, `a_pdf_that_panics_its_parser_is_unreadable_not_fatal` fails. Check this once, then put `unwind` back.

- [ ] **Step 6: CI**

In the `windows` job, after the test step:

```yaml
      - name: Release smoke test
        run: |
          cargo build --locked --release
          cargo test --locked --release --test release_smoke -- --ignored --test-threads=1
```

- [ ] **Step 7: Commit**

```bash
git add src/launch.rs src/lib.rs src/model/request.rs tests .github/workflows/ci.yml
git commit -m "Test the release exe on files that break parsers"
```

---

## PR 2: Filing stays inside the notebook

### Task 5: Refuse homes outside the notebook

**Files:**
- Modify: `src/file/home.rs` (add the check)
- Modify: `src/file/mod.rs` (`file_note` at :30-55)
- Modify: `src/ui/review.rs` (`suggested_target` at :643-662)

**Interfaces:**
- Produces: `pub fn within(relative: &Path) -> bool` in `file::home`. It is true when every component is `Normal` (no `Prefix`, `RootDir`, `ParentDir`; `CurDir` is allowed), and the path isn't empty after `CurDir`s are dropped. An empty path, meaning the notebook's root, is true.
- `file_note` returns `Err(format!("The suggested home \"{}\" is outside the notebook, so the note stays in the inbox.", folder.display()))` before touching any file, when `!within(&folder)`.

- [ ] **Step 1: Write the failing tests**

In `src/file/home.rs` tests:

```rust
#[test]
fn homes_outside_the_notebook_are_refused() {
    for inside in ["", "Payments", r"Payments\Incidents", r".\Payments"] {
        assert!(within(Path::new(inside)), "{inside}");
    }
    for outside in [r"..\Desktop", r"Payments\..\..\x", r"C:\x", r"C:x", r"\x",
                    r"\\server\share\x", r"\\?\C:\x"] {
        assert!(!within(Path::new(outside)), "{outside}");
    }
}
```

In `src/file/mod.rs` tests, using the `notebook(label)` helper:

```rust
#[test]
fn a_home_outside_the_notebook_leaves_the_note_where_it_is() {
    let scratch = notebook("file-outside");
    let error = file_note(&scratch.0, Path::new(r"inbox\Lag spikes.md"),
        &Target::Folder(PathBuf::from(r"..\Desktop")), "2026-10-19", &stem).unwrap_err();
    assert!(error.contains("outside the notebook"), "{error}");
    assert!(scratch.0.join(r"inbox\Lag spikes.md").exists());
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib file`
Expected: compile error, `within` not found.

- [ ] **Step 3: Implement** `within`, the check at the top of `file_note` (for both `Target`s, on the folder computed at :37-40), and the same check in `suggested_target` before returning a `Target::Folder`. There it returns `Err` with the same message, which `enter()` already shows with `show_problem`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib file ui::review -- --test-threads=1`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/file src/ui/review.rs
git commit -m "Never file a note outside its notebook, whatever its front matter says"
```

---

## PR 3: Tests you can trust (quick ones)

### Task 6: Skipped autocorrect tests are counted

**Files:**
- Modify: `src/platform/spell.rs` (tests at :90-113)
- Modify: `src/ui/capture_box.rs` (`no_speller` at :2165-2173)
- Modify: `.github/workflows/ci.yml`, `docs/releasing.md`

**Interfaces:**
- Produces: `#[cfg(test)] pub(crate) fn skipped(test: &str, why: &str)` in `platform::spell`. It prints `skipped ({why}): {test}` to stderr, and appends `{test}\n` to the file named by `FASTCAPTURE_SKIPPED` when that variable is set. libtest hides a passing test's stderr, so the file is what CI counts.

- [ ] **Step 1: Implement**

- The three `eprintln!` skips in `spell.rs` become `skipped("<test name>", "no en-US spell checker")` or `skipped(.., "no autocorrect list")`.
- `no_speller` takes the calling test's name, `no_speller("a_typed_typo_is_fixed_and_ctrl_z_brings_it_back")`, and calls `skipped`. Update its seven callers.

- [ ] **Step 2: CI**

The test step becomes:

```yaml
      - name: Unit and Windows integration tests (serial)
        shell: pwsh
        run: |
          $env:FASTCAPTURE_SKIPPED = "$env:RUNNER_TEMP\skipped.txt"
          cargo test --locked -- --test-threads=1
          $code = $LASTEXITCODE
          $count = if (Test-Path $env:FASTCAPTURE_SKIPPED) { (Get-Content $env:FASTCAPTURE_SKIPPED).Count } else { 0 }
          "Tests skipped for lack of Windows' autocorrect list: $count" >> $env:GITHUB_STEP_SUMMARY
          exit $code
```

- [ ] **Step 3: `docs/releasing.md`**: in section 5, before building, add a step saying to run `cargo test --lib spell autocorrect capture_box -- --test-threads=1` on a desktop Windows with English installed. The output must have no `skipped` lines, because CI's runners always skip these.

- [ ] **Step 4: Verify**: `$env:FASTCAPTURE_SKIPPED="$env:TEMP\s.txt"; cargo test --lib spell -- --test-threads=1` on this PC writes nothing (the list is present). Then push the branch and check the job summary shows a count.

- [ ] **Step 5: Commit**

```bash
git add src/platform/spell.rs src/ui/capture_box.rs .github/workflows/ci.yml docs/releasing.md
git commit -m "Count the autocorrect tests CI can't run, in every run's summary"
```

---

### Task 7: The pipe test uses its own pipe (#12)

**Files:**
- Modify: `src/ipc/pipe.rs` (`create` at :71, `wake` at :118, `send` at :153, test at :227-260)

**Interfaces:**
- Produces: `Server::create_at(name: &Path) -> Result<Self>`, `pub fn wake_at(name: &Path)`, `pub fn send_to(name: &Path, request: &Request) -> Result<Reply, SendError>`. `create`, `wake` and `send` call them with `pipe_name()?`.

- [ ] **Step 1: Change the test**: `a_request_gets_the_servers_reply_and_a_second_server_is_refused` uses `let name = PathBuf::from(format!(r"\\.\pipe\FastCapture-test-{}-{}", std::process::id(), line!()));` with `create_at`, `send_to` and `wake_at`. Its doc comment drops the "a running tray app would make this test fail" sentence.
- [ ] **Step 2: Run it to verify it fails**: `cargo test --lib ipc::pipe` gives a compile error.
- [ ] **Step 3: Implement** the three functions by moving the bodies of `create`, `wake` and `send` into them.
- [ ] **Step 4: Verify** with FastCapture's tray app running: `cargo test --lib ipc -- --test-threads=1` passes.
- [ ] **Step 5: Commit**: `git commit -am "Give the pipe test a pipe of its own, so it passes while FastCapture runs"`

---

## PR 4: Drafts

### Task 8: Drafts are saved atomically

**Files:**
- Modify: `src/capture/draft.rs` (`save` :20-45, `load` :49-78, `clear` :81-83)

**Interfaces:**
- Unchanged signatures. The draft is in `dir`. A save writes `<dir>.new` (the same name with `.new`), then moves `dir` to `<dir>.old`, `<dir>.new` to `dir`, and deletes `<dir>.old`. `load` reads `dir`, else `<dir>.old`. `clear` removes all three.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_failed_save_keeps_the_previous_draft() {
    let dir = dir("keep");
    save(&dir, &text("first")).unwrap();
    // A file where the new draft's folder goes makes the save fail before it swaps.
    fs::write(sibling(&dir, "new"), b"in the way").unwrap();
    assert!(save(&dir, &text("second")).is_err());
    assert_eq!(load(&dir).unwrap().text, "first");
    let _ = fs::remove_file(sibling(&dir, "new"));
    clear(&dir);
}

#[test]
fn a_save_interrupted_after_the_old_draft_moved_still_loads_it() {
    let dir = dir("moved");
    save(&dir, &text("first")).unwrap();
    fs::rename(&dir, sibling(&dir, "old")).unwrap();
    assert_eq!(load(&dir).unwrap().text, "first");
    clear(&dir);
    assert!(!sibling(&dir, "old").exists());
}

#[test]
fn a_save_leaves_no_sibling_folders() {
    let dir = dir("tidy");
    save(&dir, &text("one")).unwrap();
    save(&dir, &text("two")).unwrap();
    assert!(!sibling(&dir, "new").exists() && !sibling(&dir, "old").exists());
    assert_eq!(load(&dir).unwrap().text, "two");
    clear(&dir);
}
```

The helpers are `fn text(s: &str) -> Capture` (text only), and `fn sibling(dir, ext)`, which is the private function the implementation uses.

- [ ] **Step 2: Run them to verify they fail**: `cargo test --lib capture::draft` fails on `sibling` (not found) and then on the assertions.
- [ ] **Step 3: Implement**
  - `fn sibling(dir: &Path, suffix: &str) -> PathBuf`: `dir` with `.{suffix}` added to its file name.
  - `save`: remove `.new`; `create_dir` it (failing if a file is in the way); write each file with `File::create`, `write_all` and `sync_all`, text last; if `dir` exists, remove `.old` and rename `dir` to `.old`; rename `.new` to `dir`; remove `.old`.
  - An empty capture calls `clear`.
  - On any error, remove `.new` and return the error.
- [ ] **Step 4: Run the tests to verify they pass**: `cargo test --lib capture::draft` passes, and so does `cargo test --lib ui::capture_box -- --test-threads=1`.
- [ ] **Step 5: Commit**: `git commit -am "Save the capture box's draft so a failed save keeps the one before"`

---

### Task 9: Drafts while typing and at the end of the session

**Files:**
- Modify: `src/ui/capture_box.rs` (constants at :86-92; `edit_proc` WM_CHAR :1276 and WM_PASTE :1307; `attach` :847-869; `remove_item` :1027-1034; autocorrect's replacement; `close` :1198-1206; WM_TIMER :1781; WM_DESTROY :1789)
- Modify: `src/ui/app.rs` (`window_proc` at :769-851)

**Interfaces:**
- Produces:
  - `const DRAFT_TIMER: usize = 0x4350_4446;` and `const DRAFT_DELAY_MS: u32 = 1_500;`
  - `pub const WM_SAVE_DRAFT: u32 = WM_APP + 0x62;`, which saves the draft and returns 0
  - `fn draft_changed(hwnd: HWND)`: `SetTimer(hwnd, DRAFT_TIMER, DRAFT_DELAY_MS, None)` (which resets a running one), and sets `capture.draft_pending = true`
  - `fn save_draft(hwnd: HWND)`: `close`'s draft save without `DestroyWindow`; clears `draft_pending`
  - `CaptureBox.draft_pending: bool`

- [ ] **Step 1: Write the failing tests** in `capture_box.rs` tests, using `open_box` and `temp`:

```rust
#[test]
fn typing_schedules_a_draft_and_the_timer_saves_it() {
    let drafts = temp("draft-timer");
    let (hwnd, _) = open_box(&drafts, None);
    type_for_test(hwnd, "half a thought");
    assert!(state(hwnd).unwrap().draft_pending);
    unsafe { SendMessageW(hwnd, WM_TIMER, DRAFT_TIMER, 0) };
    assert_eq!(draft::load(&drafts).unwrap().text, "half a thought");
    assert!(!state(hwnd).unwrap().draft_pending);
    unsafe { DestroyWindow(hwnd) };
    draft::clear(&drafts);
}

#[test]
fn the_end_of_the_session_saves_the_draft() {
    let drafts = temp("draft-session");
    let (hwnd, _) = open_box(&drafts, None);
    type_for_test(hwnd, "before the restart");
    unsafe { SendMessageW(hwnd, WM_SAVE_DRAFT, 0, 0) };
    assert_eq!(draft::load(&drafts).unwrap().text, "before the restart");
    unsafe { DestroyWindow(hwnd) };
    draft::clear(&drafts);
}
```

Add `attach` and `remove_item` cases to `typing_schedules_a_draft_and_the_timer_saves_it`, or write a third test using the existing attach test at :2561-2575.

- [ ] **Step 2: Run them to verify they fail**: `cargo test --lib ui::capture_box -- --test-threads=1` gives compile errors.
- [ ] **Step 3: Implement**
  - Call `draft_changed(GetParent(edit))` at the end of the `WM_CHAR` and `WM_PASTE` handling, and `draft_changed(hwnd)` in `attach`, `remove_item` and after an autocorrect replacement.
  - `WM_TIMER if wparam == DRAFT_TIMER`: `KillTimer`, then `save_draft`.
  - `WM_SAVE_DRAFT`: `save_draft`.
  - `WM_DESTROY` also kills `DRAFT_TIMER`.
  - A successful `submit` already clears the draft; kill the timer there before `DestroyWindow`, so nothing re-saves it.
  - In `app.rs`'s `window_proc`:
    - `WM_QUERYENDSESSION`: if `capture_box::find()` is `Some(b)`, `SendMessageW(b, WM_SAVE_DRAFT, 0, 0)`; return 1.
    - `WM_ENDSESSION` with `wparam != 0`: the same, and return 0.
- [ ] **Step 4: Run the tests to verify they pass**: `cargo test --lib ui -- --test-threads=1` passes.
- [ ] **Step 5: Commit**: `git commit -am "Keep the capture box's draft while you type and when Windows signs out"`

---

## PR 5: Crash leftovers, every notebook, big files

### Task 10: The sweep

**Files:**
- Create: `src/platform/process.rs`, `src/capture/sweep.rs`
- Modify: `src/platform/mod.rs`, `src/capture/mod.rs` (module lines)
- Modify: `src/index/scan.rs` (`Scan` :36-43, filter :158-186)
- Modify: `src/ui/app.rs` (`start_orphan_scan` :279-299)

**Interfaces:**
- Produces:
  - `platform::process::running(pid: u32) -> bool`: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` and `GetExitCodeProcess == STILL_ACTIVE`. Any failure counts as not running. The current process counts as running.
  - `capture::sweep::temp_target(path: &Path) -> Option<(PathBuf, u32)>`: for `.<name>.<pid>.<n>.fastcapture-tmp`, the sibling `<name>` and the pid
  - `pub enum Action { Restore, Delete, Skip }`
  - `pub fn decide(target_exists: bool, in_attachments: bool, owner_running: bool, age: Duration) -> Action`:
    - `Skip` if `owner_running || age < 10 min`;
    - else `Delete` if `target_exists || in_attachments`;
    - else `Restore`.
  - `pub struct Swept { pub restored: usize, pub deleted: usize }`
  - `pub fn sweep(folders: &[(PathBuf, bool)]) -> Swept`: each folder, not recursive, with `true` for attachments folders. Restoring uses `MoveFileExW` without replace; a failure is logged and counted as neither.
  - `pub fn folders_of(conventions: &Conventions) -> Vec<(PathBuf, bool)>`:
    - the raw folder;
    - the capture folder;
    - the attachments folder for a note in the raw folder (`attachments_folder_for(&raw.join("x.md"))`), marked `true`;
    - each absolute, deduplicated.
  - `Scan.temps: Vec<PathBuf>`: `*.fastcapture-tmp` files seen in walked folders, relative

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn temp_names_give_their_target_and_owner() {
    assert_eq!(temp_target(Path::new(r"D:\n\raw\.2026-11-05 1200 lag.md.4242.7.fastcapture-tmp")),
        Some((PathBuf::from(r"D:\n\raw\2026-11-05 1200 lag.md"), 4242)));
    assert_eq!(temp_target(Path::new(r"D:\n\raw\lag.md")), None);
    assert_eq!(temp_target(Path::new(r"D:\n\raw\.x.notapid.1.fastcapture-tmp")), None);
}

#[test]
fn the_sweep_restores_deletes_and_skips_by_the_rules() {
    let old = Duration::from_secs(3600);
    assert!(matches!(decide(false, false, false, old), Action::Restore));
    assert!(matches!(decide(false, true, false, old), Action::Delete));
    assert!(matches!(decide(true, false, false, old), Action::Delete));
    assert!(matches!(decide(false, false, true, old), Action::Skip));
    assert!(matches!(decide(false, false, false, Duration::from_secs(60)), Action::Skip));
}

#[test]
fn a_whole_raw_source_left_as_a_temp_comes_back() {
    let scratch = Scratch::new("sweep");
    // A pid that isn't running: process ids are multiples of 4, so 3 never is.
    scratch.file(r"raw\.a.md.3.0.fastcapture-tmp", "---\ntype: raw\n---\nhello");
    scratch.file(r"attachments\.b.png.3.0.fastcapture-tmp", "partial");
    age(&scratch.0, Duration::from_secs(3600)); // sets each file's mtime back
    let swept = sweep(&[(scratch.0.join("raw"), false), (scratch.0.join("attachments"), true)]);
    assert_eq!((swept.restored, swept.deleted), (1, 1));
    assert!(scratch.0.join(r"raw\a.md").exists());
    assert!(!scratch.0.join(r"attachments\.b.png.3.0.fastcapture-tmp").exists());
}
```

The test helper `age` uses `File::set_modified`. Add a `running` test in `process.rs`: `running(std::process::id())` is true, and `running(3)` is false.

- [ ] **Step 2: Run them to verify they fail**: `cargo test --lib capture::sweep platform::process` gives compile errors.
- [ ] **Step 3: Implement**
  - The functions above.
  - In `scan.rs`, the `else` branch after `is_note` pushes the file to `temps` when its name ends with `.fastcapture-tmp`.
  - The index service passes nothing for now: `Scan.temps` is read by the startup sweep only, via its own `scan` call in Step 4.
- [ ] **Step 4: Wire it** in `start_orphan_scan`'s thread, before the orphan scan, for each notebook (Task 11 widens the list):
  - `sweep(&folders_of(&conventions))`;
  - then `sweep` over each distinct parent folder of `scan(&root, &[], NOTE_LIMIT)?.temps`, all marked `false`;
  - then the queue folder and `local()\index`, also `false`.
  - Log `sweep: {restored} restored, {deleted} removed` when either is nonzero.
- [ ] **Step 5: Run the tests to verify they pass**: `cargo test --lib -- --test-threads=1` passes.
- [ ] **Step 6: Commit**: `git add src/platform src/capture src/index/scan.rs src/ui/app.rs && git commit -m "Put back captures a crash left half-saved, and clear the rest"`

---

### Task 11: The orphan scan covers every known notebook

**Files:** Modify: `src/ui/app.rs` (`start_orphan_scan` :279-299)

- [ ] **Step 1: Implement**
  - The notebooks are `app.notebook` followed by `app.settings.recent_notebooks`, deduplicated case-insensitively. Keep only those that are directories.
  - For each one, in turn, on the existing thread: the sweep (Task 10), then `enqueue_orphans`.
  - Add the counts and log the total as today.
- [ ] **Step 2: Test**: extend `the_box_files_a_pasted_image_and_text_into_the_notebook_and_queue`'s module with a test that builds two scratch notebooks, writes a raw source with no job into the second, and calls the scan function. That function is extracted from the thread body as `fn scan_notebooks(queue: &Path, notebooks: &[PathBuf]) -> usize`. Then check the job exists.
- [ ] **Step 3: Run**: `cargo test --lib ui::app -- --test-threads=1` passes.
- [ ] **Step 4: Commit**: `git commit -am "Find captures without a job in every notebook FastCapture knows"`

---

### Task 12: Large attachments are streamed

**Files:** Modify: `src/platform/saver.rs` (`write_then_swap` :80-105), `src/capture/file.rs` (`store` :142-176)

**Interfaces:**
- Produces: `pub fn save_atomic_new_from(path: &Path, source: &mut dyn Read) -> Result<()>`. It works like `save_atomic_new`, but copies from `source` with `std::io::copy` into the temp file.
- `write_then_swap` takes `write: &mut dyn FnMut(&mut File) -> std::io::Result<()>` in place of `bytes`. `save_atomic` and `save_atomic_new` pass `|file| file.write_all(bytes)`.

- [ ] **Step 1: Failing test** in `saver.rs`: `save_atomic_new_from` with a `std::io::repeat(7).take(3 * 1024 * 1024)` reader creates a 3 MB file of 7s and leaves no temp sibling (use `ExistingFile`'s folder).
- [ ] **Step 2: Implement**
  - In `store`, for `Attachment::File`, don't read the bytes up front.
  - Inside the `free_name` loop, open the source with `File::open` on each try and call `save_atomic_new_from`. Opening it again makes a name clash restart from byte 0.
  - `Attachment::Png` keeps `save_atomic_new`.
- [ ] **Step 3: Run**: `cargo test --lib platform::saver capture -- --test-threads=1` passes.
- [ ] **Step 4: Commit**: `git commit -am "Copy attached files in pieces instead of reading them whole"`

---

## PR 6: Timeouts and deadlines

### Task 13: Timeouts stop being retried forever

**Files:**
- Modify: `src/net/http.rs` (`HttpError` :33-38, `failed` :152-160, `TRANSIENT` :25-30, tests :565-597)
- Modify: `src/model/backend.rs` (`BackendError` :51-68, `Display` :70-80, `from_http` :99-108)
- Modify: `src/runner/pages.rs` (:43-48), `src/runner/process.rs` (`process_with` :38-87)
- Modify: `src/model/claude_code/mod.rs` (:382), `src/model/copilot/mod.rs` (:109, :453, :481)
- Modify: `src/queue/job.rs`, `src/ui/review.rs` (`regenerate_selected`)
- Modify: `src/config/settings.rs` (`Settings` :21-62, defaults :119-148, `parse` :193-315), `src/model/openai.rs` (`local` :60-62), `src/runner/from_settings.rs` (:14-37), `src/ui/settings.rs` (:1430)
- Modify: `README.md` (Local or Cloud; Make it yours)

**Interfaces:**
- Produces:
  - `HttpError::TimedOut(String)`, returned by `failed` for `ERROR_WINHTTP_TIMEOUT`, which leaves the `TRANSIENT` list
  - `BackendError::TimedOut(String)`, whose `Display` is its reason
  - `Job.timeouts: u32`, JSON `"timeouts"`, default 0
  - `pub const TIMEOUTS_BEFORE_FAILING: u32 = 3;` in `runner::process`
  - `Settings.local_timeout: Duration`, default `openai::LOCAL_TIMEOUT` (`Duration::from_secs(300)`); ini key `local_timeout`, whole seconds, 30 to 3600
  - `OpenAi::local(endpoint: &str, model: &str, timeout: Duration)`
- `process_with`, `Err(BackendError::TimedOut(reason))`: `job.timeouts += 1`.
  - Below the limit: exactly as `Retry { after: None, reason }`.
  - At the limit: `State::Failed` with `format!("{reason} That happened three times. Choose a faster model, or raise local_timeout for a local one, then press R.")`, and `Outcome::Failed` with that text.

- [ ] **Step 1: Write the failing tests**
  - `http.rs`: in `a_slow_server_times_out_and_a_closed_port_may_pass`, `slow` is now `Err(HttpError::TimedOut(_))`. The closed port stays `Transient`.
  - `backend.rs`: `from_http(HttpError::TimedOut("x".into()), "Claude")` is `BackendError::TimedOut(_)`.
  - `process.rs` tests (with `Scripted` and `Temp`):

```rust
#[test]
fn a_model_that_times_out_three_times_fails_with_what_to_do() {
    let temp = Temp::new("timeouts");
    let store = temp.store();
    let job = temp.capture("slow");
    let timed_out = || Err(BackendError::TimedOut("The local model didn't finish within 5 minutes.".into()));
    let backend = Scripted::new(vec![timed_out(), timed_out(), timed_out()]);
    let mut job = job;
    for round in 1..=2 {
        assert!(matches!(process(&store, job.clone(), &backend, 0, "d"), Outcome::Retry { .. }), "{round}");
        job = store.get(&job.capture_id).unwrap();
        assert_eq!(job.timeouts, round);
    }
    let Outcome::Failed(reason) = process(&store, job.clone(), &backend, 0, "d") else { panic!() };
    assert!(reason.contains("three times") && reason.contains("local_timeout"), "{reason}");
}
```

  - `settings.rs` tests: `local_timeout=600` gives 600 s; `local_timeout=10`, `=abc` and `=99999` each give a warning and 300 s; a missing key gives 300 s.

- [ ] **Step 2: Run them to verify they fail**: `cargo test --lib net model runner config -- --test-threads=1` gives compile errors.
- [ ] **Step 3: Implement** the interfaces.
  - `rg "HttpError::Transient"` lists every match. Each one also matches `TimedOut`; in `pages.rs:43` that's `Err(HttpError::Transient(reason) | HttpError::TimedOut(reason)) if ...`.
  - Claude Code's `RunError::TimedOut`, Copilot's session deadline (:109), `StartError::TimedOut` (:453) and `RpcError::TimedOut` (:481) return `BackendError::TimedOut` with their current reasons.
  - `regenerate_selected` resets `timeouts`.
  - `from_settings` passes `settings.local_timeout`; Settings' Test button passes `openai::LOCAL_TIMEOUT`.
- [ ] **Step 4: README**
  - **Local or Cloud**, the Local bullet gains: "A note may take 5 minutes; `local_timeout` in the settings file changes that."
  - The retry paragraph gains: "A model that runs out of time three times on one capture fails it, saying so; press **R** to try again."
  - **Make it yours** gains a row: `local_timeout` | `300` | Seconds a model on this PC may take per note, 30 to 3600.
- [ ] **Step 5: Run**: `cargo test --lib -- --test-threads=1` passes.
- [ ] **Step 6: Commit**: `git commit -am "Fail a capture whose model runs out of time three times, and make a local model's time settable"`

---

### Task 14: Requests have a total deadline

**Files:** Modify: `src/net/http.rs` (`send_limited` :197-325), `src/net/test_server.rs` (`Scripted` :9-16, the writer at :124)

**Interfaces:**
- Produces:
  - `Scripted::dripping(self, every: Duration) -> Self`: writes the body one byte at a time, `every` apart, after the headers
  - `send_limited` fails with `HttpError::TimedOut(format!("{what} took longer than {} s", timeout.as_secs()))` once `Instant::now()` passes `started + request.timeout`. It checks before `WinHttpSendRequest`, after `WinHttpReceiveResponse`, and before each `WinHttpQueryDataAvailable`. Before each read it also sets `WINHTTP_OPTION_RECEIVE_TIMEOUT` to the time left, at least 1 ms.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn a_dripping_server_hits_the_total_deadline() {
    let body = "x".repeat(200);
    let server = TestServer::start(vec![
        Scripted::json(200, &body).dripping(Duration::from_millis(100)), // 20 s in all
    ]);
    let started = std::time::Instant::now();
    let result = Client::new("FastCapture-test").unwrap().send(&Request {
        method: "GET", url: &server.url(), headers: Vec::new(), body: &[],
        timeout: Duration::from_secs(2),
    });
    assert!(matches!(result, Err(HttpError::TimedOut(_))), "{result:?}");
    // WinHTTP waits about 4 s at least on loopback, whatever is asked.
    assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
}
```

- [ ] **Step 2: Run it to verify it fails**: `cargo test --lib net::http -- --test-threads=1` fails: no `dripping`, then a `TimedOut` after about 20 s, or success.
- [ ] **Step 3: Implement**. The body-size tests and `a_slow_server_times_out_and_a_closed_port_may_pass` must still pass.
- [ ] **Step 4: Commit**: `git commit -am "Give every request one deadline, so a server that drips bytes can't hold the queue"`

---

### Task 15: The index watcher comes back

**Files:** Modify: `src/index/service.rs` (`State` :34-40, `run` :180-259, `Indexes` :69-)

**Interfaces:**
- Produces:
  - `State.watch_failed: bool`, set by the watcher callback when `changes.failed`; the callback stops ignoring its argument
  - `const REWATCH_DELAYS: [Duration; 4]`: 5 s, 30 s, 2 min, 10 min (then 10 min each time); `#[cfg(test)]` shortens them all to 50 ms
  - `pub fn refresh_all(&self)` on `Indexes`: marks every open notebook dirty, and asks a notebook with no watcher to try again now
- `run` keeps `watcher: Option<Watcher>` and `rewatch_at: Option<Instant>`.
  - On `watch_failed`, it drops the watcher, schedules `rewatch_at` from the delays, and rescans.
  - The wait's timeout is the earliest of the cache-save time, `rewatch_at` and one hour.
  - When `rewatch_at` passes, it calls `Watcher::start` (success clears the schedule, failure schedules the next delay), then rescans either way.
  - With no watcher, the hourly wake rescans too.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn a_notebook_whose_watch_failed_is_watched_again() {
    let scratch = Scratch::new("rewatch");
    scratch.file("a.md", "# Alpha");
    let indexes = Indexes::new(None);
    indexes.open(&scratch.0);
    assert!(eventually(&indexes, &scratch.0, Duration::from_secs(2), |s| finds(s, "alpha", "a.md")));
    indexes.fail_watch_for_test(&scratch.0); // #[cfg(test)]: what a failed batch does
    scratch.file("b.md", "# Bravo");
    assert!(eventually(&indexes, &scratch.0, Duration::from_secs(2), |s| finds(s, "bravo", "b.md")));
    scratch.file("c.md", "# Charlie"); // seen only if the new watcher works
    assert!(eventually(&indexes, &scratch.0, Duration::from_secs(2), |s| finds(s, "charlie", "c.md")));
}
```

- [ ] **Step 2: Run it to verify it fails**: `cargo test --lib index::service -- --test-threads=1`
- [ ] **Step 3: Implement** the interfaces, with `fail_watch_for_test` as `#[cfg(test)] pub(crate) fn` doing `notebook.change(|s| { s.watch_failed = true; s.dirty = true; })`.
- [ ] **Step 4: Run**: `cargo test --lib index -- --test-threads=1` passes.
- [ ] **Step 5: Commit**: `git commit -am "Watch a notebook again after its watch stops, and let the app ask for a rescan"`

---

### Task 16: Claude Code's output is capped

**Files:** Modify: `src/model/claude_code/run.rs` (`run` :36-113, `RunError` :26-32), `src/model/claude_code/mod.rs` (~:370-386)

**Interfaces:**
- Produces:
  - `RunError::TooMuchOutput`
  - `pub fn run_capped(..same as run.., stdout_cap: usize) -> Result<Output, RunError>`; `run` calls it with `crate::net::http::MAX_RESPONSE_BYTES`
  - The stdout reader reads 64 KB chunks. Past the cap it sets an `Arc<AtomicBool>` and keeps draining without storing. The wait loop sees the flag, ends the job (or kills the child) and returns `TooMuchOutput`.
  - The stderr reader keeps only `STDERR_KEPT` bytes while reading, as `copilot/cli.rs:151-161` does.
  - `mod.rs` maps `TooMuchOutput` to `BackendError::Fail("Claude Code wrote more than 32 MB, so its reply was dropped.".into())`.

- [ ] **Step 1: Failing test**: a `Temp::script("loud.cmd", &["@for /l %%i in (1,1,2000) do @echo 0123456789012345678901234567890123456789"])` run through `run_capped(..., 1024)` gives `Err(RunError::TooMuchOutput)` in under 5 s.
- [ ] **Step 2: Implement**, then run `cargo test --lib model::claude_code -- --test-threads=1`.
- [ ] **Step 3: Commit**: `git commit -am "Stop reading Claude Code's output past 32 MB"`

---

## PR 7: The rest of the test work

### Task 17: UI tests serialize themselves (#11)

**Files:**
- Create: `src/ui/test_support.rs` (`#[cfg(test)]`)
- Modify: `src/ui/mod.rs`, and the test modules of `ui::app`, `ui::capture_box` (both `tests` and `preparing_tests`), `ui::controls`, `ui::frame`, `ui::home_picker`, `ui::popup`, `ui::review`, `ui::settings`, `ui::theme`, `ui::clipboard_capture`, `platform::clipboard`
- Modify: `.github/workflows/ci.yml`, `README.md` (Building from source), the note "FastCapture's tests pass only on one thread…" in the notebook (add a dated section once this lands)

**Interfaces:**
- Produces: `pub(crate) fn lock() -> std::sync::MutexGuard<'static, ()>`, a `static UI: Mutex<()>`, recovering from poisoning with `unwrap_or_else(PoisonError::into_inner)`.

- [ ] **Step 1: Add `let _ui = crate::ui::test_support::lock();` as the first line of every test in those modules.** The clipboard ones share the same lock: they and the box tests both use the real clipboard. So do the theme ones, because `theme::STATE` is process-wide.
- [ ] **Step 2: Run `cargo test --lib` (default threads) three times in a row.** Expected: all pass each time.
  **Stop condition:** if a test still fails only in parallel, find what it shares (a temp folder name, a global, a window class) and fix that. If it can't be fixed in an hour, leave `--test-threads=1` in CI, list the test in the PR, and report it.
- [ ] **Step 3: CI**: drop `-- --test-threads=1` from the test step (keep it in the smoke step). README's `cargo test -- --test-threads=1` becomes `cargo test`.
- [ ] **Step 4: Commit**: `git commit -am "Let UI tests take turns, so cargo test passes on any number of threads"`

---

### Task 18: Failure-path tests for saving a capture

**Files:** Modify: `src/platform/saver.rs`, `src/capture/file.rs` tests

**Interfaces:**
- Produces: `#[cfg(test)] pub(crate) fn fail_after(steps: u32)`. This thread's next saver operations succeed `steps` times, then the following one (a write, a sync or a swap, counted together) fails with `ERROR_DISK_FULL`, then everything succeeds again. `write_then_swap` counts each of its three steps.

- [ ] **Step 1: Test**: for a capture with text and two file attachments, for `n` in `0..9`:
  - `fail_after(n)`, then `file_capture(...)`;
  - either it succeeds and the raw source and both attachments exist, or it fails and `raw\` and `attachments\` hold no files at all, temps included.
- [ ] **Step 2: Implement** the hook; run `cargo test --lib capture platform::saver -- --test-threads=1`.
- [ ] **Step 3: Commit**: `git commit -am "Test that a capture failing at any save step leaves all of it or none"`

---

### Task 19: Arm64 tests in CI

**Files:** Modify: `.github/workflows/ci.yml`

- [ ] **Step 1:** add a job:

```yaml
  windows-arm64:
    runs-on: windows-11-arm
    steps:
      - uses: actions/checkout@v4
      - name: Unit and Windows integration tests
        run: cargo test --locked
```

- [ ] **Step 2:** push and check it runs. **Stop condition:** if the runner isn't available to the repository, remove the job and say so in the PR.
- [ ] **Step 3: Commit**: `git commit -am "Run the tests on Windows on Arm too"`

---

## PR 8: The log

### Task 20: The log holds nothing from a capture

**Files:** Modify: `src/log.rs`, `src/ui/app.rs` (`apply_settings` :303-370), `src/runner/worker.rs` (:342, :377-381), `src/model/request.rs` (:147, :173, :509)

**Interfaces:**
- Produces:
  - `pub fn set_notebooks(roots: &[PathBuf])`, kept in a `static NOTEBOOKS: RwLock<Vec<String>>`, lowercase with `/` turned into `\`
  - `fn scrub(message: &str, notebooks: &[String], profile: Option<&str>) -> String`, pure; `line` applies it with the stored notebooks and `%USERPROFILE%`
- The rules, in order:
  1. Each `http://` or `https://` URL, up to whitespace or a quote, becomes its host.
  2. A path starting with a notebook root becomes `<notebook>\<first component>\…`. It is `<notebook>` alone for the root itself, and `<notebook>\<component>` when nothing follows. The path ends at whitespace followed by an uppercase letter, at `:` followed by a space, at a quote, or at the end.
  3. The same for `%USERPROFILE%`, as `%USERPROFILE%\<first component>\…`.
- Call sites:
  - `runner: wrote {capture_id}`. Clone the id before `process_with` takes the job.
  - `queue: finding {capture_id}`.
  - `request: attachment {n} (.{extension}): {error}`, where `n` is the 1-based index in the capture.

- [ ] **Step 1: Failing tests** in `log.rs`:

```rust
#[test]
fn urls_become_their_host() {
    assert_eq!(scrub("The page at https://example.com/a/b?c couldn't be fetched yet: failed (403)", &[], None),
        "The page at example.com couldn't be fetched yet: failed (403)");
}

#[test]
fn notebook_paths_keep_their_first_folder_and_the_error() {
    let notebooks = vec![r"d:\notes".to_owned()];
    assert_eq!(scrub(r"FastCapture could not save D:\Notes\raw\2026 lag spikes.md: Access is denied.", &notebooks, None),
        r"FastCapture could not save <notebook>\raw\…: Access is denied.");
    assert_eq!(scrub("D:/Notes", &notebooks, None), "<notebook>");
}

#[test]
fn other_paths_in_the_profile_lose_their_names() {
    assert_eq!(scrub(r"usage: C:\Users\ana\AppData\Local\FastCapture\usage.txt couldn't be read: x",
        &[], Some(r"c:\users\ana")),
        r"usage: %USERPROFILE%\AppData\… couldn't be read: x");
}
```

- [ ] **Step 2: Implement**. `apply_settings` calls `crate::log::set_notebooks` with the notebook and `recent_notebooks`.
- [ ] **Step 3: Run**: `cargo test --lib log runner model -- --test-threads=1` passes.
- [ ] **Step 4: Commit**: `git commit -am "Keep note titles, paths and URLs out of the log, as the README promises"`

---

### Task 21: A fuller start line, and one line per repeated retry

**Files:**
- Create: `src/platform/system.rs` (moving `os_build` from `src/ui/theme/faces.rs:52`, made `pub fn windows_build() -> Option<u32>`; `faces.rs` calls it)
- Modify: `src/ui/app.rs` (the start line at :115 moves after `apply_settings` at :138), `src/runner/worker.rs` (:337-361)

**Interfaces:**
- Produces:
  - `pub fn notebook_kind(path: &Path) -> &'static str` in `platform::system`: `missing`, `network`, `onedrive` or `local`, from `kind_of(exists, drive_type, under_onedrive)`, which is pure and tested
    - `GetDriveTypeW` on the path's root: `DRIVE_REMOTE` is network.
    - OneDrive: the path is under `%OneDrive%`, `%OneDriveConsumer%` or `%OneDriveCommercial%`.
  - The start line: `FastCapture {version} started: Windows {build} {arch}, mode {local|cloud|none}, backend {key}, notebook {kind}`, with `arch` from `std::env::consts::ARCH` and `backend` from `CloudBackend::key()`
  - In `worker::run`, `last_retry: HashMap<String, (String, u32)>`, keyed by capture id:
    - a `Retry` whose reason equals the stored one only adds 1;
    - otherwise it logs `runner: {capture_id}: {n} more retries, same reason` if `n > 0`, then logs the new reason and stores it.
    - `Written` and `Failed` flush and remove the entry.

- [ ] **Step 1: Tests**: `kind_of` covers each case. A worker test with a `Scripted` backend returning the same `Retry` 3 times then a note checks the log lines through a `#[cfg(test)]` capture of `log::line`: make `line` push to a thread-local `Vec<String>` under `cfg(test)` instead of returning early. Then check that two lines appear, not four.
- [ ] **Step 2: Implement**; run `cargo test --lib -- --test-threads=1`.
- [ ] **Step 3: Commit**: `git commit -am "Say at start which Windows, mode and notebook kind FastCapture runs with, and log a repeated retry once"`

---

### Task 22: Symbols for every release

**Files:** Modify: `Cargo.toml`, `.github/workflows/ci.yml` (`package` job), `docs/releasing.md` (section 6)

- [ ] **Step 1:** add `debug = "line-tables-only"` to `[profile.release]`. Build release and compare the exe's size with Task 3's.
  **Stop condition:** if it grows by more than 64 KB, or no `target/release/fastcapture.pdb` appears, try `split-debuginfo = "packed"`, then report rather than ship a bigger exe.
- [ ] **Step 2:** in "Portable zip", add:

```powershell
Compress-Archive -Path "target/${{ matrix.target }}/release/fastcapture.pdb" -DestinationPath "dist/$name.pdb.zip" -Force
```

and add `dist/*.pdb.zip` to the upload's `path`. The user's zip is unchanged.
- [ ] **Step 3:** `docs/releasing.md` section 6: attach both `.pdb.zip` files to the GitHub release.
- [ ] **Step 4: Commit**: `git commit -am "Keep each release's symbols, so its crash dumps can be read"`

---

## PR 9: Copy diagnostics, and sleep and resume

### Task 23: Copy diagnostics

**Files:**
- Create: `src/ui/diagnostics.rs`, `.github/ISSUE_TEMPLATE/bug.yml`
- Modify: `src/ui/mod.rs`, `src/log.rs` (`tail`), `src/ui/app.rs` (`MENU_DIAGNOSTICS = 9` after :52, `show_menu` :657-767, its dispatch at :741-766)
- Modify: `README.md` (the tray menu row)

**Interfaces:**
- Produces:
  - `pub struct Facts { pub version: &'static str, pub build: Option<u32>, pub arch: &'static str, pub mode: &'static str, pub backend: &'static str, pub model: String, pub notebook_kind: &'static str, pub notes: Option<usize>, pub queue: [usize; 4], pub paused: Option<String> }`, where `queue` is queued, working, failed and done
  - `pub fn report(facts: &Facts, log_tail: &str) -> String`, the text in the spec, pure
  - `pub fn tail(lines: usize) -> String` in `log`: the last `lines` lines of `fastcapture.old.log` plus `fastcapture.log`, oldest first
- The menu gains "Copy &diagnostics" after "Open &notebook folder".
  - Its handler builds `Facts`. The counts come from `Store::list()`; the notes from `INDEXES` `snapshot(root, ZERO)` as `searcher.index().notes.len()`; paused from the worker's status.
  - It calls `clipboard::set_text(window, &report(..))`, then `tray.notify("Diagnostics copied", "Paste them into your bug report.", false)`.

- [ ] **Step 1: Test** `report`: with sample facts, the text has each line in order, and holds no `\` or `:/` path or URL. Even with a model name like `openai/gpt-5-mini`, check that only that name appears.
- [ ] **Step 2: Implement**; add the issue template asking for the pasted diagnostics, what was done and what happened.
- [ ] **Step 3: README**: the tray menu row lists "Copy diagnostics" after "Open notebook folder". Add a line under the FAQ's bug paragraph: "Tray › **Copy diagnostics** puts what a bug report needs on the clipboard, with nothing you captured."
- [ ] **Step 4: Commit**: `git commit -am "Copy what a bug report needs from the tray menu"`

---

### Task 24: Sleep and resume

**Files:** Modify: `src/ui/app.rs` (`window_proc`)

- [ ] **Step 1: Implement**: `WM_POWERBROADCAST` with `wparam == PBT_APMRESUMEAUTOMATIC` calls `wake_worker()` and `INDEXES.get().map(Indexes::refresh_all)`, then returns 1. If `PBT_APMRESUMEAUTOMATIC` isn't in `windows-sys`'s `Win32_UI_WindowsAndMessaging`, define `const PBT_APMRESUMEAUTOMATIC: usize = 0x12;`.
- [ ] **Step 2: Test**: in `ui::app` tests, `SendMessageW(window, WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC, 0)` returns 1 and doesn't panic. The behaviour itself is covered by Task 15's `refresh_all` and the soak run.
- [ ] **Step 3: Commit**: `git commit -am "Look at the queue and the notebooks again after the PC wakes"`

---

## PR 10: Fuzzing and the soak run

### Task 25: Fuzzing

**Files:**
- Create: `src/fuzzing.rs` (`#[doc(hidden)] pub mod fuzzing;` in `lib.rs`), `fuzz/Cargo.toml`, `fuzz/fuzz_targets/{front_matter,page,reply,rpc,pdf}.rs`, `docs/fuzzing.md`

**Interfaces:**
- Produces: in `fastcapture::fuzzing`, one `pub fn <target>(data: &[u8])` each. Each calls the real parser and discards the result:
  - `notebook::frontmatter`'s reader on lossy UTF-8;
  - `extract::page::extract(text, "https://example.invalid/")`;
  - `note::reply`'s parser;
  - `model::copilot::rpc::Decoder` (push, then drain `message()`);
  - `extract::pdf::read_unguarded(data, 20)`.

- [ ] **Step 1:** `cargo install cargo-fuzz`, then `cargo +nightly fuzz init`, and write the five targets.
  **Stop condition:** if libFuzzer won't build on Windows MSVC here, write `examples/fuzz.rs <target> <seconds>` instead. It mutates the fixtures in `tests/fixtures` at random and runs each target under `catch_unwind` in a debug build. Say so in `docs/fuzzing.md`.
- [ ] **Step 2:** run each target for 10 minutes. For each crash: save it under `tests/fixtures/bad/`, add a unit test reproducing it, and fix it (or guard it, for PDF).
- [ ] **Step 3:** `docs/fuzzing.md`: how to install, run and minimize, and the rule that each crash becomes a fixture and a test.
- [ ] **Step 4: Commit**: `git commit -am "Fuzz the parsers that read what FastCapture didn't write"`

---

### Task 26: The soak run

**Files:**
- Modify: `src/net/mod.rs` (`test_server` becomes `#[doc(hidden)] pub mod test_server;`, no longer `cfg(test)`), `src/net/test_server.rs` (add `TestServer::answering(reply: Scripted)`, which gives the same reply to every request)
- Modify: `Cargo.toml` (`windows-sys` feature `Win32_System_ProcessStatus`)
- Create: `examples/soak.rs`
- Modify: `README.md` (Building from source), `docs/releasing.md` (before section 5)

**Interfaces:**
- `examples/soak.rs <fastcapture.exe> <hours>`:
  - Precondition: the user's FastCapture is quit, because one tray app runs per session. The example checks with `ipc::pipe::send` and refuses with that message.
  - It makes a scratch folder holding `appdata\FastCapture\fastcapture.ini` (`mode=local`, `local_endpoint=<server url>`, `local_model=fake`, `notebook=<scratch>\notebook`) and `localappdata\`.
  - It starts a `TestServer::answering(...)` with a chat-completions reply holding a fixed note.
  - It starts the exe with `APPDATA` and `LOCALAPPDATA` pointing at the scratch folders.
  - Every 2 minutes it runs `<exe> add` with text, `--attach` of a PNG, or a PDF built with `pdf::build`, in turn.
  - Every minute it appends `elapsed_s,handles,gdi,user,private_bytes` to `soak.csv`, read with `GetProcessHandleCount`, `GetGuiResources` and `GetProcessMemoryInfo`.
  - At the end it prints each counter's change per hour after the first hour, and `flat` or `GROWING` against these limits: handles and GDI 0, private bytes 1 MB an hour.

- [ ] **Step 1: Implement**; run it for 10 minutes to check it works (`cargo run --release --example soak -- target\release\fastcapture.exe 0.17`).
- [ ] **Step 2: Docs**:
  - README's Building from source gains a sentence on `examples/soak.rs`.
  - `docs/releasing.md` gains a step before building a Store submission: a 48 h soak, with a few sleep and resume cycles and the box moved between monitors with different scales, must end `flat`.
- [ ] **Step 3: Commit**: `git commit -am "Add a soak run that watches FastCapture's handles and memory for days"`

---

## After the last task

- Run the whole suite, Clippy, the smoke test and a 48 h soak.
- Update the notebook. Add a dated section to each of the five audit notes saying what was fixed and in which commit, and one to "FastCapture's tests pass only on one thread…" once Task 17 lands.
- The README changes are made in the tasks above. Releasing still follows `docs/releasing.md`, including regenerating the README images.
