# M4: Index, Links and Filing Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Each part ends with `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passing, and one commit. Code comments never cite the spec.

**Goal:** Notes join the notebook. FastCapture keeps a keyword index of the notebook. For each capture it picks up to 30 related notes as candidates and gives them to the model, which links the note to the ones that really relate and suggests a home: a folder, or a topic note. In the review window, **Enter** files the note there: it moves into the folder, or into the topic note's folder with a dated line appended to the topic note. **H** picks another home, **I** keeps the note in the inbox, and **Ctrl+Z** undoes the last file or discard.

**Spec:** `docs/specs/2026-10-18-fastcapture-design.md` §7.1, §7.2, §7.4 (`## Related`, `suggested_home`), §9, §10, §11 (`home_types`), §13, §15.

**Decided for M4:**

- **A topic note gets one dated line per filed note**, with FastPad's append rules: `- 2026-10-19 [[Consumer lag spikes during ledger batch]]: Kafka consumer lag on payments jumps to ~40k…` (open question 4).
- **Notes stay in `capture_folder`** (`inbox` by default), shared with FastPad's quick captures and told apart by `type: capture` (open question 5).
- **Search is a hand-written BM25 index**, with no new crates. It is built on FastPad's scanner, watcher and index cache, copied as M1 copied FastPad's notebook code. It matches words, not meaning (open question 6 stays open).

## What M4 does not do

- **Rewriting other notes' links** when a note is filed. A new inbox note rarely has links pointing at it, and wikilinks by name keep working after a move. Only the moved note's own relative links (its attachments) are rewritten, as §10.3 says.
- **The OpenAI-compatible cloud backend and token usage** (§8.3) are M5; **the Claude Code CLI backend** (§8.4) is M6.

## Architecture

```
src/
  index/
    scan.rs           the notebook's notes and folders (copied from FastPad: FileIdBothDirectoryInfo listing, skip rules)
    watch.rs          ReadDirectoryChangesW on the notebook, batched (copied from FastPad)
    words.rs          tokenizing: lowercase words, accents folded, numbers kept, a short stop list
    notes.rs          per note: path, title, aliases, tags, type, headings, term counts; built incrementally by size and write time; cached
    search.rs         BM25 over fields (title, aliases and tags weigh more), the top 30; the notebook's most used tags
    service.rs        one index per notebook on its own thread, kept current by the watcher; the worker asks it for candidates
  model/
    prompt.rs         (changed) <candidate_notes> and the notebook's tags in the user turn; the rules on links and homes
    request.rs        (changed) the query (your text, page titles, the start of the material) and the candidates
  note/
    reply.rs          (changed) links and the home kept only when they name a candidate, a home type or a folder
    render.rs         (changed) `## Related` in FastPad's link form; `suggested_home`
  file/
    home.rs           homes: folders, and notes whose `type:` is in `home_types` (topic, project, moc)
    move_note.rs      moving a note to a folder, its own relative links rewritten first
    topic.rs          the dated line appended to a topic note (FastPad's append rules)
    front.rs          `status: filed` / `kept`, `suggested_home` removed, nothing else touched
    undo.rs           what filing or discarding did, to put it back
  ui/
    review.rs         (changed) inbox notes with their suggested homes; Enter, H, I, Ctrl+Z
    home_picker.rs    the home picker: folders and home notes, filtered as you type
```

## Parts

### Part 1: Scanning and watching the notebook

- [x] `index/scan.rs`: FastPad's `library/scan.rs` copied (naming it on its first line): notes and folders under the notebook, with FastPad's skip rules (folders starting with `.`, `node_modules`, `target`, `bin`, `obj`), sizes and write times without opening files, 10,000 notes at most. `raw/`, the attachments folder and the templates folder are left out of the notes, passed in by the caller and matched without regard to case. Only `.md` and `.markdown` files are notes; file IDs and the online-only flag FastPad keeps are dropped.
- [x] `index/watch.rs`: FastPad's `library/watch.rs` copied: changes under the notebook, batched 250 ms after the last one, sent to a callback rather than a window, with no generation counter. Changes to `.fastpad\notebook.ini` and `.fastcapture\capture.ini` are kept, since they say where things go. Folders whose contents changed are reported too, as in FastPad.
- **Tests:** a scratch notebook: notes found, skipped folders, `raw/` left out, the limit; the watcher reports a write, a rename and a delete.

### Part 2: The index and search

- [x] `index/words.rs`: words as runs of letters and digits, lowercase, accents folded (`café` → `cafe`), at least 2 characters, a short English stop list (`the`, `and`, `of`…), `snake_case` and `kebab-case` split as well as kept whole. Also left out: runs over 64 characters, and `http`, `https`, `www`, `com`.
- [x] `index/notes.rs`: per note: its path, title (first `# ` heading, else the file name), aliases, tags (front matter and inline, FastPad's rules through `notebook::frontmatter`), `type:`, headings, and word counts per field, plus the first 300 characters of its text for snippets. Built by reading only notes whose size or write time changed, 2 MB per note at most. Cached as `%LocalAppData%\FastCapture\index\<folder key>.cache` (versioned, thrown away when it can't be read). Words are numbered in a vocabulary shared by the notes, so each count takes 8 bytes. The title is a `# ` heading only when nothing comes before it. Inline tags are read line by line, leaving out code blocks and code spans, since FastCapture has no Markdown parser. Named `notes.rs` rather than `index.rs`, which Clippy refuses inside `index/`.
- [x] `index/search.rs`: BM25 (k1 1.2, b 0.75) per field, weighted title 3, aliases 3, tags 2, headings 1.5, text 1; the top 30 for a query. The notebook's 50 most used tags. `Searcher` keeps per-word postings built from the index, so the service rebuilds it when the index changes.
- [x] `index/watch.rs` (from Part 1): `Watcher::start` now waits for the first read to be issued, so changes made right after it returns are no longer missed.
- **Tests:** words; an index of a scratch notebook, then one note edited, one added and one deleted, rebuilt incrementally; the cache round trip and a corrupt cache; ranking on a small fixed notebook (a title match beats a text match, a rare word beats a common one); 5,000 generated notes searched under 20 ms.

### Part 3: The index service

- [ ] `index/service.rs`: one index per notebook, built on its own thread after start-up (from the cache, then brought up to date), and kept current by the watcher. The worker asks it for candidates; while a notebook's index is still being built, the worker waits up to 10 s, then writes the note without candidates.
- [ ] The tray app starts the service for the notebooks it knows (the default and the recent ones, as they are used) and stops it on quit.
- **Tests:** a note added to the notebook is found within a second; a query while the build runs waits, then answers; a notebook that disappears is dropped.

### Part 4: Candidates for the model

- [ ] `model/request.rs`: the query is your text, the titles of read pages and the first 2,000 characters of the material; the top 30 notes (never the capture's own note) become candidates.
- [ ] `model/prompt.rs`: `<candidate_notes>` with each candidate's name as the model should give it, aliases, folder, tags, type and snippet, and the notebook's 50 most used tags, all inside escaped blocks. The rules change only where they already talk about candidates.
- [ ] `file/home.rs`: home candidates are the candidates whose `type:` is in `capture.ini`'s `home_types` (default `topic, project, moc`), and the folders that hold candidates.
- [ ] `note/reply.rs`: `Allowed` now holds the candidates, home notes and folders. A link is kept when it names a candidate, or is marked `new` (§7.2). The home is kept when it names a home note or a folder. Anything else is dropped, as now.
- [ ] `note/render.rs`: `## Related` links in the form FastPad's completion inserts (the name, or the path when the name is ambiguous, through FastPad's `wiki_target`, copied into `notebook::links`); `suggested_home: "[[Payments incidents]]"` for a note, `"Payments/Incidents/"` for a folder.
- **Tests:** golden user turns with candidates; links and homes kept or dropped; `## Related` with an ambiguous name; the capture's own note isn't a candidate.

### Part 5: Filing

- [ ] `file/front.rs`: changes `status:` and removes `suggested_home:`, leaving every other byte of the note as it was (line endings, other keys, order).
- [ ] `file/move_note.rs`: moves a note into a folder under a free name (` 2`, ` 3`… on a clash). Its own relative links are rewritten first so they keep reaching their files (FastPad's `edits_for_moves_with`, copied).
- [ ] `file/topic.rs`: the line `- <date> [[<note>]]: <summary>` appended to a topic note with FastPad's append rules (in its list, a blank line before it when needed, its line endings kept); the note moves into the topic note's folder.
- [ ] Writes use the atomic saver with a stamp check, retried up to three times when the file changed meanwhile, as FastPad's capture does. A topic note that can't be written leaves the note in the inbox with the error.
- [ ] `file/undo.rs`: each file or discard records what it did (moved from, moved to, the topic note's text before), so it can be put back while the file is still as FastCapture left it.
- **Tests:** front matter edits keep every other byte (CRLF, BOM, unknown keys); moves with attachments in a subfolder; clashes; a topic note ending with a list, with text, with no final line end, in CRLF; a topic note changed between read and write; undo of a move and of an append, and undo refused when the file changed since.

### Part 6: The review window

- [ ] The list shows FastCapture's notes with `status: inbox` in the capture folder (`type: capture`), newest first, with their suggested home under the title, plus queued, working and failed jobs (§10.1).
- [ ] **Enter** files to the suggested home; **H** opens `ui/home_picker.rs` (folders and home notes, filtered as you type, the suggestion first) and files there; **I** sets `status: kept` and takes the note off the list; **Ctrl+Z** undoes the last file or discard of this session. Enter opens the folder when there's no suggestion. The footer shows the keys that apply.
- [ ] A note filed, kept or undone updates its job (its path and stamp), so **R** still finds it.
- [ ] The tray icon's count is the notes waiting in the inbox.
- **Tests:** driven through messages: Enter files to a folder and to a topic note; H picks a home typed in the picker; I keeps; Ctrl+Z puts a filed note back; a failed append leaves the note in the list with the error.

### Part 7: End to end

- [ ] `runner/end_to_end.rs`: a notebook with notes; a capture whose text matches one of them goes through the worker; the request carries the candidates; the scripted reply links one candidate, invents another and suggests a topic note as home; the written note keeps the real link and the home; filing it appends the line to the topic note and moves the note.
- [ ] `examples/bench.rs`: the index of 10,000 generated notes built cold under 5 s and from the cache under 500 ms; a query under 20 ms (§14).
- [ ] Measure with `examples/live_note.rs` (with your approval) on a scratch copy of a notebook you choose: whether the model links sensibly and suggests a sensible home.

### Part 8: Polish and docs

- [ ] README: candidates, links and homes, filing and the review keys, `home_types`, what is sent (candidate snippets).
- [ ] Spec: §19 notes for M4; open questions 4, 5 and 6 updated.

## Testing approach

- **Pure:** words, BM25 ranking, candidates in the prompt, reply checking, front matter edits, topic lines, link rewriting.
- **Disk:** scratch notebooks for scanning, the index and its cache, moves and appends with stamp checks, undo.
- **Windows:** the watcher, the review window and the home picker driven through messages.
- **Live:** only `examples/live_note.rs`, run by hand with your approval.

## Order and dependencies

Parts 1 and 2 come first; 3 needs both; 4 needs 3; 5 is independent of 1–4; 6 needs 4 and 5; 7 and 8 come last.
