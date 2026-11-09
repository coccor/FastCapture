# FastMind backlog

FastCapture becomes **FastMind**: one always-running app (one tray icon) whose modules each do
one thing a mind does. FastPad stays a separate editor. Both work on the same plain Markdown
notebook folder.

| Module | What it does | Status |
|---|---|---|
| **Learn** | One key from anywhere saves a thought, screenshot, link or file; the model writes the note and suggests its home | Exists today as FastCapture |
| **Remember** | One key from anywhere searches the notebook or answers a question from it, citing notes | New |
| **Dream** | Overnight, while the PC is idle: links, merges and tidies notes, as proposals you approve | New |
| **Practice** | Morning digest, weekly review, tasks, optional flashcards from notes | New |
| **Remind** | Brings a note back at a time ("Friday") or in a situation ("next time I open this repo") | New |
| **Forget** | Proposes archiving old, unused or finished notes; ranks fresh notes higher | New |
| **Focus** | Holds captures, reminders and notifications while you work | New |

## Principles

- **One resident process.** Every module lives in `fastmind.exe`. A new module never adds a tray
  icon or a startup entry.
- **Modules, not plugins.** Modules are crates compiled in and switched on or off in Settings. No
  runtime-loaded code and no public plugin interface. Third-party plugins, if ever, are a separate
  layer (see Later).
- **Offline by default.** Local model unless the user chooses Cloud, with consent per provider and
  a token cap. Dream gets its own consent and its own nightly budget.
- **Plain Markdown, any editor.** No database the user depends on; indexes can always be rebuilt.
- **Propose, never silently rewrite.** Background work (Dream, Forget) writes proposals. The user
  accepts or rejects them, and every accepted change can be undone.
- **Risky work in workers.** Parsing files, fetching pages and calling models run in child
  processes, so a bad PDF can't take down the hotkeys.
- **Small and instant.** Disabled modules cost nothing: no hotkey, no index, no thread.

## M0 Rename to FastMind

There are no FastCapture users yet, so there's no migration: rename everything outright.

- [ ] Rename the GitHub repo `coccor/FastCapture` to `coccor/FastMind` (GitHub keeps redirects)
- [ ] Crate `fastcapture` to `fastmind`; executable `fastmind.exe`
- [ ] Settings file `fastmind.ini`; AppData folders; the instance pipe name; temp folder prefixes
- [ ] The per-notebook folder `.fastcapture/` (holds `capture.ini`) becomes `.fastmind/`
- [ ] Credential Manager entries `FastMind/claude`, `FastMind/openai`
- [ ] Worker folders (`fastcapture-claude-code`, `fastcapture-copilot`) and the test
      environment variable `FASTCAPTURE_SKIPPED`
- [ ] User-facing strings ("FastCapture could not save…")
- [ ] CLI: `fastmind learn "…"` replaces `fastcapture add`; `--attach`, `--url`, `--notebook`
      and `-` keep their meaning
- [ ] Agent skill `skills/fastcapture-notes` to `skills/fastmind-notes`
- [ ] Release artifacts `FastMind-x.y.z-windows-{x64,arm64}.zip` (+ `.sha256`, `.pdb.zip`);
      update `docs/releasing.md`
- [ ] README rewritten around FastMind and its modules; screenshots regenerated
- [ ] FastPad: README and "open in" links point at FastMind
- [ ] First winget and Scoop packages published as `coccor.FastMind` (never publish
      `coccor.FastCapture`)

## M1 Host and workspace

Split the single crate into a Cargo workspace so modules share code instead of copying it.

- [ ] Workspace crates: `host` (tray, hotkeys, settings shell, module registry), `notebook`
      (front matter, wikilinks, tags, files), `index`, `model` (providers, token cap), `extract`,
      `net`, `ui` (theme, controls), `platform`, and `learn` (today's capture, queue, review)
- [ ] `Module` trait: id, hotkeys, tray items, settings page, start, stop, commands; modules get
      shared services through a `HostContext`
- [ ] One hotkey registry: every module declares its keys; the host detects clashes and says so
- [ ] Tray menu with a section per enabled module; tooltip summarizes them ("3 notes in inbox")
- [ ] Settings window with a tab per module plus General (theme, notebook, model, token cap)
- [ ] Modules switched on and off in `fastmind.ini` and Settings; a disabled module starts nothing
- [ ] Extraction and fetching moved into worker processes; the host survives a worker crash and
      fails just that capture
- [ ] Update path: winget/Scoop upgrade can stop and restart the host without the user quitting it

## M2 Remember

- [ ] Hotkey (proposed `Ctrl+Alt+R`, check clashes) opens a search box over any window
- [ ] Instant search by title, alias, tag and full text, with typo tolerance; reuses the
      notebook index
- [ ] Search by meaning with local embeddings, kept current as notes change
- [ ] Ask a question and get an answer built from notes, each claim linked to its source note;
      says plainly when the notebook has nothing on it
- [ ] Open the result in FastPad (or the default `.md` editor) at the right line
- [ ] Related-notes panel for what you're working on (window title, open file, repo)
- [ ] Learn uses Remember's meaning search to suggest related notes and homes
- [ ] CLI `fastmind remember "…"` with JSON output for scripts
- [ ] MCP server so coding agents can search and read the notebook (read-only first)
- [ ] Timeline: "what was I working on in March?" from captures and note dates

## M3 Dream

- [ ] Scheduler: runs when the PC is idle, on mains power, inside a time window; stops when the
      user comes back
- [ ] Incremental: only notes changed since the last run, plus a small rotating sample of old ones
- [ ] Proposals store and a "while you were away" list in the morning, each accepted or rejected
      with one key
- [ ] Proposes links between notes about the same thing, missed by word search
- [ ] Flags duplicates and contradictions (newer note disagrees with older one), proposes merges
- [ ] Drafts a topic note when several loose notes share a subject with no home note
- [ ] Refreshes summaries in home notes (topic, project, MOC) when notes under them change
- [ ] Housekeeping: broken `[[links]]`, attachments nothing links to, notes stuck in the inbox
- [ ] Change log for every accepted proposal, with undo
- [ ] Own consent for Cloud and a nightly token budget; local model recommended

## M4 Practice

- [ ] Morning digest: inbox to file, tasks due or going stale, what Dream found, notes from a
      year ago today
- [ ] Tasks: every `- [ ]` across the notebook in one list; ticking it writes back to the note
- [ ] Weekly review: clear the inbox, go through open tasks, see the most-captured topics; the
      model drafts a "this week" note
- [ ] Optional flashcards from notes (`Q:: … A::`, or `review: spaced` on a note), scheduled
      with FSRS

## M5 Remind

- [ ] Time reminders on a note or task ("Friday 9:00", "in 2 weeks")
- [ ] Situation reminders: next time a given app, repo, folder or window title is active
- [ ] Set from the Learn box, from Remember results, from FastPad, and from front matter
      (`remind:`) so agents and other editors can set them too
- [ ] Shown as a Windows notification that opens the note

## M6 Forget

- [ ] Staleness score from age, last opened, links in and out, and status
- [ ] Proposes archiving (to an archive folder, never deleting); confirmed in Practice
- [ ] Remember ranks fresh, often-used notes higher and archived ones lower

## M7 Focus

- [ ] Focus on and off from the tray or a hotkey; follows Windows' Do Not Disturb when it's on
- [ ] While on: Learn still saves but processes quietly; Remind and Practice hold notifications
- [ ] When it ends: a short summary of what was held

## FastPad integration (FastPad repo)

- [ ] IPC protocol between FastPad and FastMind: open a note at a line, list related notes,
      run a module command
- [ ] FastMind commands appear in FastPad's command palette when FastMind is running
      ("Remember: ask the notebook…", "Learn: capture selection")
- [ ] FastPad sidebar panel for related notes and the inbox, fed by FastMind
- [ ] FastPad's quick capture (`Ctrl+Alt+N`) hands off to Learn when FastMind is running

## Later

- [ ] Voice notes in Learn: hold a key and speak, transcribed on this PC
- [ ] Prepare: notes for an upcoming meeting, from a calendar read on this PC
- [ ] Phone capture: a share target that drops captures into the notebook's capture folder
      through a sync folder
- [ ] Third-party plugins in a WASM sandbox, with a narrow interface and no network by default

## Open questions

- **Learn or Capture** as the module name. Learn matches the other verbs; Capture is clearer.
- **Default hotkeys** for each module, checked against common apps.
- **Embeddings runtime** for Remember: which local model, and how it's shipped without growing the
  download much.
- **Name checks still to do:** Microsoft Store and trademarks (USPTO, EUIPO, classes 9 and 42).
  GitHub, winget and Scoop are clear; `fastmind.com`, `.app` and `.dev` are taken, so the home
  page stays on GitHub or a family domain.

## Decisions (2026-11-09)

- **One host app, not several tray apps.** A new product must never add another tray icon.
- **FastPad stays separate.** It keeps its offline, ~60 ms, under-3 MB promises and quits when
  closed; the resident work lives in FastMind.
- **Modules compiled in, not plugins.** Only first-party code, no plugin interface to keep stable.
- **FastMind over FastBrain and FastMemory.** FastBrain searches lead to brain-training games and
  a crowded "second brain" field; FastMemory clashes with an AI-memory project of that name and
  with RAM tools on Windows.
- **Where FastMind stands out** against Mem, Obsidian plugins, Tentacle, Basic Memory and others:
  native and small on Windows, works with any editor on plain files, background changes are
  proposed and undoable, Forget, and the same notebook shared with coding agents.
