# Fuzzing

FastCapture reads things it didn't write: the front matter of notes in a notebook, web pages it
fetches, the models' replies, the Copilot CLI's messages and PDFs. A crafted or broken input must
never crash the app or hang it. Fuzzing feeds each of those parsers inputs mutated at random and
watches for panics, hangs and stack overflows.

## The targets

Each target is a function in `src/fuzzing.rs` that takes any bytes, runs one real parser on them
and throws the result away:

| Target         | Parser                                                       |
| -------------- | ------------------------------------------------------------ |
| `front_matter` | `notebook::frontmatter::parse`, on the bytes as lossy UTF-8  |
| `page`         | `extract::page::extract`, against `https://example.invalid/` |
| `reply`        | `note::reply::parse`                                         |
| `rpc`          | `model::copilot::rpc::Decoder`: push, then read messages     |
| `pdf`          | `extract::pdf::read_unguarded`, 20 pages                     |

The PDF target calls the parser without the guard that `extract::pdf::read` puts around it, so a
panic inside pdf-extract shows up. In the app such a panic is caught and the PDF is unreadable;
it is a bug in pdf-extract, not in FastCapture, and needs no fix unless the guard misses it. A
panic in any other target is a bug in FastCapture.

## Running it: `examples/fuzz.rs`

libFuzzer needs a nightly toolchain and cargo-fuzz, so the fuzzer here is an example that builds
on stable. It doesn't measure coverage; it starts from the files in `tests/fixtures` (and a few
inputs it builds, such as PDFs from `extract::pdf::build`), applies a few random mutations at a
time (bit flips, inserted tokens the parser cares about, repeated, spliced and cut ranges, huge
numbers) and runs the target on each mutant under `catch_unwind`.

Run it as a debug build, so integer overflow panics as well:

```
cargo run --example fuzz -- pdf 600          # one target for 10 minutes
cargo run --example fuzz -- all 600          # every target, 10 minutes each
cargo run --example fuzz -- reply 600 42     # with a fixed seed, to repeat a run
```

It prints each place that panicked once, and saves the input that did under
`target/fuzz/<target>/crash-<hash>.bin`. It exits 1 when anything panicked.

Two findings end the process instead of being caught:

- A **hang**: a run that takes over 20 seconds ends the fuzzer with `HANG` and exit code 3.
- A **stack overflow** kills the process outright.

In both cases the input being run is in `target/fuzz/<target>/current.bin`, written before each
run.

To look at a finding:

```
cargo run --example fuzz -- page replay target/fuzz/page/crash-0123456789abcdef.bin
cargo run --example fuzz -- page minimize target/fuzz/page/crash-0123456789abcdef.bin
```

`replay` runs one input and says whether it panics. `minimize` removes ever smaller chunks while
the input still panics in the same place and writes what is left next to it, as `.min`.

## Every crash becomes a fixture and a test

For each crash, hang or overflow:

1. Minimize it, and save it under `tests/fixtures/bad/` with a name that says what it is.
2. Add a unit test next to the parser that reads the fixture and reproduces the failure.
3. Fix the parser (for a PDF: make sure `extract::pdf::read`'s guard catches it), and see the
   test pass.

The fixtures under `tests/fixtures` are also the fuzzer's seeds, so each one makes later runs
start closer to the places that broke before.

## Moving to cargo-fuzz later

With a nightly toolchain, `cargo install cargo-fuzz` and `cargo +nightly fuzz init` make a
`fuzz/` crate outside the workspace. Each target there is a one-line call into the same
functions:

```rust
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| fastcapture::fuzzing::pdf(data));
```

Copy the files in `tests/fixtures` into `fuzz/corpus/<target>/` as seeds, then run
`cargo +nightly fuzz run pdf -- -max_total_time=600` and minimize with
`cargo +nightly fuzz tmin pdf <crash file>`. The rule above stays the same.

## What has been found

- `page` hung on `tests/fixtures/bad/nested.html`, 20,000 `<div>`s deep: over two minutes in a
  debug build and 36 seconds in a release build. The page scorer measured the text under every
  level again, and html5ever looks through every open element for each tag. The scorer now
  measures each node once, and `extract::dom::Dom::parse` reads a page that nests deeper than
  512 levels as text from there on.
- `page` panicked (capacity overflow), or ran out of memory, on a Word list paragraph with a
  huge `mso-list` level: the item was indented that many times. Levels past Word's nine are
  read as the ninth (`tests/fixtures/bad/word-list-level.html`).
- `rpc` panicked (add with overflow) on a `Content-Length` of `u64::MAX`. A length over 64 MB
  is now an error (`tests/fixtures/bad/rpc-huge-length.txt`).
- `pdf` panics in eight places inside pdf-extract and lopdf (missing objects, a missing
  `MediaBox`, `Font` or `XObject`, wrong types, an offset overflow). `extract::pdf::read`'s guard
  turns each into an unreadable PDF; the inputs are `tests/fixtures/bad/pdf-*.pdf`, checked by a
  unit test and, in a release build, by `tests/release_smoke.rs`.
- `front_matter` and `reply` found nothing.
