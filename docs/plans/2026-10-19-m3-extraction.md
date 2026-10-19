# M3: Extraction Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Each part ends with `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passing, and one commit. Code comments never cite the spec.

**Goal:** The model reads what you captured, not just its name. Rich text pasted into the box keeps its structure as Markdown. A link's web page is fetched, its main content is extracted and kept beside the raw source, and the note is written from it. PDFs (text and scanned), Word `.docx` files, and text, logs and code are read when the note is written. All of this runs on this PC in both modes, within limits that keep one capture cheap.

**Spec:** `docs/specs/2026-10-18-fastcapture-design.md` §5.2 (rich text), §6.1 (links), §6.2, §7.1, §7.4 (`url`, `site`, `## Source`, `truncated:`), §13, §14, §15.

**Decided for M3:**

- **A link's page is fetched by the worker, not at Ctrl+Enter.** Ctrl+Enter stays instant. Before asking the model, the worker fetches the page and writes it once to a sidecar beside the raw source, `raw/<stem>.page.md` (front matter `url`, `fetched`, `page_title`, `site`, `published`, or `fetch: failed (403)`; then the page as Markdown). The raw source and its sidecar are never written again; Regenerate reuses the sidecar. This replaces the spec's page inside the raw source (§6.1).
- **HTML is parsed with `html5ever`** (=0.40.1, about 20 crates), with a small arena DOM of our own (its `rcdom` is unofficial and unsupported).
- **PDF text comes from `pdf-extract`** (=0.12.1). It brings about 70 crates, most of them for encrypted PDFs through `lopdf`. Pages with almost no text are rendered with `Windows.Data.Pdf` and sent as images.
- **`.docx` is read with `zip`** (=9.0.5, `deflate` only) and a small scanner over `word/document.xml`, with no XML crate.
- **`encoding_rs`** (already pulled in by `pdf-extract`) decodes pages that aren't UTF-8.
- **Rich text paste puts the Markdown in the box.** What you see is what is saved, and you can edit it. This differs from the spec's "the box shows the plain text" (§5.2), which would need a hidden copy that goes stale as you edit.

## What M3 does not do

- **Links, homes and filing** (§9, §10) are M4.
- **JavaScript-rendered or logged-in pages** are read as served; a page with no main text is recorded as such (§17: a browser extension later).
- **Other file types** (`.xlsx`, `.pptx`, images inside `.docx`) are listed by name, type and size, as now.

## Architecture

```
src/
  extract/
    mod.rs            Extracted { text, images, truncated }, the 60,000-character and 20-page budget shared by a capture's sources
    text.rs           text, logs and code: encoding detection (copied from FastPad), binary sniffing, long logs cut to their first and last 200 lines
    dom.rs            html5ever into a small arena DOM (TreeSink), with element, text and attribute access
    markdown.rs       DOM → Markdown: headings, emphasis, lists, links, code, blockquotes, tables, images as links
    page.rs           main content (a readability-style scorer), og:title, og:site_name, article:published_time
    pdf.rs            pdf-extract per page; scanned pages rendered with Windows.Data.Pdf to PNG
    docx.rs           zip → word/document.xml → paragraphs, headings, lists, tables as text
  net/
    http.rs           (changed) redirects (at most 5), no cookies, a body limit per request, charset from Content-Type
    fetch.rs          a page GET: desktop user agent, 10 s, 5 MB, HTML only, decoded with the page's charset
  capture/
    sidecar.rs        raw/<stem>.page.md: write once, read back, recognised and skipped wherever raw sources are listed
  platform/
    clipboard.rs      (changed) `HTML Format`: its header, the fragment, SourceURL
  model/
    prompt.rs         (changed) page and file text inside their blocks; what was cut
    request.rs        (changed) builds the sources through extract/, within the budget
  note/
    render.rs         (changed) url, site, truncated in front matter; ## Source
  runner/
    process.rs        (changed) fetches a link's page before building the request
```

## Parts

### Part 1: Text files and the budget

- [x] `extract/text.rs`: decoding adapted from FastPad's `file/encoding.rs`, naming it on its first line. FastPad handles BOMs and UTF-8 only, and its `sniff.rs` matches image signatures, so the rest is new: UTF-16 without a BOM (by its zero bytes), Windows-1252 as the fallback, and binary detection (a NUL, or over 10% control characters, in the first 8 KB). Logs (`.log`, `.out`, `.err`, or `log` as a word of the name) longer than 400 lines keep their first and last 200 with a `… n lines cut …` line between. Code (by extension, the list FastPad uses) goes in a fenced block with its language.
- [x] `extract/mod.rs`: `Budget` hands out characters (60,000 per capture) and PDF pages (20) to sources in capture order. Anything cut is recorded by name for `truncated:`. Your own text is never cut by the budget.
- **Tests:** each encoding; a binary file refused; a 1,000-line log cut to 401 lines; a code file fenced; the budget across three sources, with the third cut and named.

### Part 2: HTML to Markdown

- [x] `Cargo.toml`: `html5ever = "=0.40.1"`.
- [x] `extract/dom.rs`: an arena `Dom` filled through html5ever's `TreeSink` (elements, text, comments dropped, attributes kept), with `children`, `text_of`, `attr` and a walk.
- [x] `extract/markdown.rs`: block and inline conversion: `h1`–`h6`, `p`, `br`, `strong`/`b`, `em`/`i`, `code`, `pre` (fenced, language from `class="language-…"`), `ul`/`ol` (nested, numbered), `a` (relative links resolved against a base URL), `blockquote`, `table` (a Markdown table when every row has the same cell count, else rows as lines), `img` as `![alt](src)` link text only, `hr`. `script`, `style`, `nav`, `noscript`, form controls and hidden elements are dropped. **Changed:** a `form` is kept, because some sites wrap whole pages in one; a table whose cells hold blocks is read as layout, its cells as blocks; a link whose text is its URL becomes `<url>`. Whitespace collapsed as browsers do; Markdown characters in text escaped.
- **Tests:** golden Markdown for small fragments of each element; nested lists; a table with a colspan falls back to lines; entities and whitespace; malformed HTML (unclosed tags, stray `</div>`) converts like a browser would show it.

### Part 3: Rich text paste

- [x] `platform/clipboard.rs`: reads `HTML Format` (UTF-8 with a header: `StartFragment`/`EndFragment` offsets, `SourceURL`), returns the fragment and the source URL.
- [x] Capture box: Ctrl+V with HTML on the clipboard inserts the fragment's Markdown (Part 2) instead of the plain text, when the HTML holds more than plain paragraphs (headings, lists, links, tables, code); otherwise plain text as now. The source URL is kept with the capture and written as `source_url` in the raw source's front matter. **Ctrl+Alt+V** pastes plain text. Also: Word's list paragraphs (`mso-list` styles) become Markdown lists, keeping their numbers; the source URL is kept whenever HTML with one is pasted, rich or not (only `http(s)` URLs), and the draft keeps it.
- [x] Clipboard capture (the no-box hotkey) does the same.
- **Tests:** the header parsed (offsets in bytes, a missing `SourceURL`); a fragment from Chrome, Edge and Word (fixtures) converted; plain paragraphs stay plain; `source_url` in the raw source; through the box driven by messages, Ctrl+V and Ctrl+Alt+V with a fixture on the clipboard (saved and restored by `Backup`).

### Part 4: Web pages

- [ ] `net/http.rs`: per-request options for automatic redirects (`WINHTTP_OPTION_REDIRECT_POLICY`, at most 5 via `WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS`), cookies off (`WINHTTP_DISABLE_COOKIES`), and a body limit (the API keeps 32 MB, pages get 5 MB). The final URL after redirects is returned.
- [ ] `Cargo.toml`: `encoding_rs = "=0.8.42"`.
- [ ] `net/fetch.rs`: GET with a desktop browser user agent and `Accept: text/html`, 10 s; `text/html` and `application/xhtml+xml` only (a PDF link is fetched as a file in Part 5's way); charset from `Content-Type`, else `<meta charset>`, else UTF-8, through `encoding_rs`.
- [ ] `extract/page.rs`: the main content by a readability-style score (paragraph text length and commas, minus link density, class and id hints like `article`, `content`, `comment`, `sidebar`), its Markdown through Part 2 resolved against the final URL, and `og:title` (else `<title>`), `og:site_name` (else the host), `article:published_time`. Under 200 characters of main text counts as "no main text".
- [ ] `capture/sidecar.rs`: `raw/<stem>.page.md` written once with `save_atomic_new`; read back for the request. Every listing of raw sources (review window, orphan scan, `capture::read`) skips `*.page.md`; Shift+Del recycles the sidecar with its raw source.
- [ ] `runner/process.rs`: for a capture with links and no sidecar, fetch each link (at most 3 per capture) before building the request. A failure that may pass (offline, timeout, DNS) retries the job with the usual backoff and writes nothing. An HTTP error, a non-HTML page or no main text is recorded in the sidecar (`fetch: failed (403)`, `fetch: not a page (application/zip)`, `fetch: no main text`), and the note is written from your text. **R** on a capture whose sidecar records a failure fetches again into a new sidecar, `<stem>.page 2.md`, and the newest is used.
- **Tests:** against the test server: a redirect chain of 5 followed and of 6 refused; no `Cookie` header sent; a 6 MB page refused; a Windows-1252 page decoded; a 403 and a `application/zip` recorded; a timeout retried. Pure: main content picked from fixtures of a news article, a blog post and a docs page (Markdown compared as golden files); metadata; a page of links only has no main text. Sidecar written once, skipped by the listings, recycled with its raw source.

### Part 5: PDF

- [ ] `Cargo.toml`: `pdf-extract = "=0.12.1"`; `windows` features `Data_Pdf`, `Foundation`, `Storage`, `Storage_Streams`.
- [ ] `extract/pdf.rs`: text page by page from the copied attachment (`pdf_extract::extract_text_from_mem_by_pages`), whitespace tidied, each page headed `Page n`. A page with under 40 characters of text is scanned: it is rendered with `Windows.Data.Pdf` (`PdfDocument::LoadFromFileAsync`, `PdfPage::RenderToStreamAsync` at a width of 1,568 px) to PNG and goes to the model as an image, within the 8-image limit. Pages past the budget's 20 are cut and named. An encrypted PDF that needs a password, or one that fails to parse, is listed as unreadable. pdf-extract runs inside `catch_unwind`, since a malformed file can make it panic.
- **Tests:** fixtures made for the tests (a two-page text PDF, a scanned page, a 25-page PDF, a password-protected PDF, a corrupt file): text per page, the scanned page as a 1,568 px PNG, pages 21–25 cut, the protected and corrupt ones listed.

### Part 6: Word

- [ ] `Cargo.toml`: `zip = { version = "=9.0.5", default-features = false, features = ["deflate"] }`.
- [ ] `extract/docx.rs`: `word/document.xml` scanned for `w:p` (paragraphs), `w:pStyle` `Heading1`–`Heading6` and `Title` (as `#` headings), `w:numPr` (list items), `w:t`, `w:tab`, `w:br`, and `w:tbl` (rows as `|`-separated lines). XML entities decoded. A file over 50 MB uncompressed, or one that isn't a zip, is listed as unreadable.
- **Tests:** a fixture `.docx` with headings, a list, a table and a tab; a zip with no `word/document.xml`; a zip bomb refused by the size check.

### Part 7: The request and the note

- [ ] `model/request.rs`: sources are built through `extract/` in capture order within the budget: each link's sidecar page, then each file attachment by type (text, PDF, `.docx`, else name, type and size), then images (attachments, then scanned PDF pages, 8 at most). A file that can't be read is listed with why.
- [ ] `model/prompt.rs`: `<source kind="link" url="…" title="…" site="…" published="…">page Markdown</source>`, `<source kind="link" url="…" fetched="no" reason="…"/>`, `<attachment name="…" kind="pdf" pages="12">text</attachment>`, and "Cut to fit: …" for what the budget cut. The rules gain one line: for a link capture, the note is about the page, in light of what you wrote.
- [ ] `note/render.rs`: for a capture with one link, `url` and `site` in the front matter and a `## Source` section linking the page with its title; with several, `## Source` lists them. `truncated: [<name>, …]` when the budget cut anything.
- **Tests:** golden user turns for a link with a page, a failed fetch, a PDF with a scanned page, a `.docx`, a log and a cut capture; closing tags inside page text escaped; the note's front matter and `## Source` for one and for two links; `truncated:` written.

### Part 8: End to end

- [ ] `runner/end_to_end.rs`: a link capture through the worker: the page from the test server becomes a sidecar, the request carries its Markdown, and the note is checked byte for byte. A PDF capture likewise.
- [ ] `examples/bench.rs`: an article page from HTML to Markdown under 300 ms after download, a 20-page text PDF under 1 s (§14).
- [ ] Measure with `examples/live_note.rs` (with your approval): a real article link and a real PDF, comparing the notes and the token counts.

### Part 9: Polish and docs

- [ ] README: what is read from each kind of capture, the limits, rich text paste and Ctrl+Alt+V, page fetches in Local mode (the only traffic, no cookies), the sidecar file.
- [ ] Spec: §19 notes for M3.
- [ ] A new `LICENSES.md` listing the third-party crates and their licenses, which must be MIT, Apache-2.0 or similarly permissive (checked with `cargo tree` before each crate is added).

## Testing approach

- **Pure:** encodings, log cutting, the budget, HTML to Markdown, main content scoring, `.docx` scanning, prompts, the note.
- **Fixtures:** small files written for the tests (pages, PDFs, `.docx`, clipboard HTML), checked in under `tests/fixtures/`; no copyrighted pages.
- **HTTP:** fetches run against `net::test_server` on loopback; no test reaches the internet.
- **Windows:** `Windows.Data.Pdf` rendering, the clipboard's `HTML Format` under `Backup`.
- **Live:** only `examples/live_note.rs`, run by hand with your approval.

## Order and dependencies

Parts 1, 2, 5 and 6 are independent of each other; 3 and 4 need 2; 7 needs 1, 4, 5 and 6; 8 and 9 come last.
