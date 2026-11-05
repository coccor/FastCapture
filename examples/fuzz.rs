//! A fuzzer for the parsers that read what FastCapture didn't write, for a toolchain without
//! cargo-fuzz: it mutates the files in `tests/fixtures` at random and runs a target from
//! `fastcapture::fuzzing` on each mutant, catching panics.
//!
//! Run it as a debug build, so integer overflow panics too:
//!
//! - `cargo run --example fuzz -- <target> <seconds> [seed]` fuzzes for that long;
//! - `cargo run --example fuzz -- <target> replay <file>` runs one input;
//! - `cargo run --example fuzz -- <target> minimize <file>` shrinks an input that panics.
//!
//! The targets are `front_matter`, `page`, `reply`, `rpc` and `pdf`, or `all` to fuzz each in
//! turn for the seconds given. A panicking input is saved under `target/fuzz/<target>/`. The
//! input being run is written to `target/fuzz/<target>/current.bin` first, so one that kills
//! the process (a stack overflow) or hangs is still on disk.

use fastcapture::extract::pdf::build::{self as pdf_build, Page};
use fastcapture::fuzzing::{TARGETS, Target};
use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// A run that takes longer than this is a hang.
const HANG: Duration = Duration::from_secs(20);
/// Mutants grow no bigger than this.
const MAX_LEN: usize = 256 * 1024;

/// Where the last panic happened, set by the panic hook.
static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);
/// Milliseconds since `START` when the current run began, or 0 between runs.
static RUN_STARTED: AtomicU64 = AtomicU64::new(0);

/// xorshift64*: enough randomness for choosing mutations.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number below `n`, which must not be 0.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: fuzz <target|all> <seconds> [seed] | fuzz <target> replay|minimize <file>";
    let (Some(name), Some(second)) = (args.first(), args.get(1)) else {
        eprintln!("{usage}");
        std::process::exit(2);
    };
    // A replay prints the panic as Rust does, with a backtrace under `RUST_BACKTRACE=1`.
    install_hook(second == "replay");
    let names: Vec<&str> = if name == "all" {
        TARGETS.iter().map(|(name, _)| *name).collect()
    } else {
        vec![name.as_str()]
    };
    let targets: Vec<(&str, Target)> = names
        .iter()
        .map(|name| {
            *TARGETS
                .iter()
                .find(|(known, _)| known == name)
                .unwrap_or_else(|| {
                    eprintln!("fuzz: no target {name}; the targets are {}", target_names());
                    std::process::exit(2);
                })
        })
        .collect();
    match (second.as_str(), args.get(2)) {
        ("replay", Some(file)) => replay(targets[0], Path::new(file)),
        ("minimize", Some(file)) => minimize(targets[0], Path::new(file)),
        (seconds, seed) => {
            let Ok(seconds) = seconds.parse::<u64>() else {
                eprintln!("{usage}");
                std::process::exit(2);
            };
            let seed = seed
                .and_then(|seed| seed.parse::<u64>().ok())
                .unwrap_or_else(|| {
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(1)
                })
                | 1;
            watch_for_hangs();
            let mut found = 0;
            for target in targets {
                found += fuzz(target, Duration::from_secs(seconds), seed);
            }
            std::process::exit(if found == 0 { 0 } else { 1 });
        }
    }
}

fn target_names() -> String {
    TARGETS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A hook that remembers where a panic happened, and prints it only when `print` is set: the
/// fuzzer expects many panics and reports each place once.
fn install_hook(print: bool) {
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if print {
            default(info);
        }
        let place = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "somewhere".to_owned());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        if let Ok(mut last) = LAST_PANIC.lock() {
            *last = Some(format!("{place}: {message}"));
        }
    }));
}

fn start() -> Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Ends the process when one run takes longer than `HANG`; its input is in `current.bin`.
fn watch_for_hangs() {
    start();
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let began = RUN_STARTED.load(Ordering::SeqCst);
            if began != 0 && start().elapsed().as_millis() as u64 > began + HANG.as_millis() as u64
            {
                eprintln!(
                    "fuzz: HANG: a run took over {}s; its input is current.bin",
                    HANG.as_secs()
                );
                std::process::exit(3);
            }
        }
    });
}

fn out_dir(target: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("fuzz")
        .join(target);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Runs `target` on `data`, and where it panicked if it did.
fn run(target: Target, data: &[u8]) -> Option<String> {
    if let Ok(mut last) = LAST_PANIC.lock() {
        *last = None;
    }
    RUN_STARTED.store(start().elapsed().as_millis() as u64 + 1, Ordering::SeqCst);
    let result = panic::catch_unwind(AssertUnwindSafe(|| target(data)));
    RUN_STARTED.store(0, Ordering::SeqCst);
    result.err().map(|_| {
        LAST_PANIC
            .lock()
            .ok()
            .and_then(|last| last.clone())
            .unwrap_or_else(|| "a panic".to_owned())
    })
}

/// Fuzzes `target` for `time`, and how many places it panicked in.
fn fuzz((name, target): (&str, Target), time: Duration, seed: u64) -> usize {
    let corpus = seeds(name);
    let dictionary = dictionary(name);
    let dir = out_dir(name);
    let current = dir.join("current.bin");
    println!(
        "fuzz: {name} for {}s from {} seeds, seed {seed}",
        time.as_secs(),
        corpus.len()
    );
    for input in &corpus {
        if let Some(place) = run(target, input) {
            println!("fuzz: {name}: a seed panics at {place}");
        }
    }
    let mut rng = Rng(seed);
    let mut places = HashSet::new();
    let mut runs: u64 = 0;
    let began = Instant::now();
    let mut last: Vec<u8> = corpus[0].clone();
    while began.elapsed() < time {
        // Mostly from a seed; sometimes on from the last mutant, to reach further.
        let mut input = if rng.chance(4) {
            last.clone()
        } else {
            corpus[rng.below(corpus.len())].clone()
        };
        for _ in 0..1 + rng.below(8) {
            mutate(&mut input, &corpus, &dictionary, &mut rng);
        }
        input.truncate(MAX_LEN);
        let _ = std::fs::write(&current, &input);
        runs += 1;
        if let Some(place) = run(target, &input)
            && places.insert(place.clone())
        {
            let file = dir.join(format!("crash-{:016x}.bin", hash(&input)));
            let _ = std::fs::write(&file, &input);
            println!(
                "fuzz: {name}: PANIC at {place}\n      input: {}",
                file.display()
            );
        }
        last = input;
    }
    let _ = std::fs::remove_file(&current);
    println!(
        "fuzz: {name}: {runs} runs in {}s, {} panic place(s)",
        began.elapsed().as_secs(),
        places.len()
    );
    places.len()
}

fn replay((name, target): (&str, Target), file: &Path) {
    let data = std::fs::read(file).unwrap_or_else(|error| {
        eprintln!("fuzz: {}: {error}", file.display());
        std::process::exit(2);
    });
    match run(target, &data) {
        Some(place) => {
            println!("fuzz: {name}: PANIC at {place}");
            std::process::exit(1);
        }
        None => println!("fuzz: {name}: no panic"),
    }
}

/// Removes ever smaller chunks of a panicking input while it still panics in the same place,
/// and writes what is left next to it as `.min`.
fn minimize((name, target): (&str, Target), file: &Path) {
    let mut data = std::fs::read(file).unwrap_or_else(|error| {
        eprintln!("fuzz: {}: {error}", file.display());
        std::process::exit(2);
    });
    let Some(place) = run(target, &data) else {
        println!("fuzz: {name}: it doesn't panic");
        std::process::exit(1);
    };
    let mut chunk = data.len().next_power_of_two();
    while chunk >= 1 {
        let mut at = 0;
        while at < data.len() {
            let mut smaller = data.clone();
            smaller.drain(at..(at + chunk).min(data.len()));
            if run(target, &smaller).as_deref() == Some(place.as_str()) {
                data = smaller;
            } else {
                at += chunk;
            }
        }
        chunk /= 2;
    }
    let out = file.with_extension("min");
    let _ = std::fs::write(&out, &data);
    println!(
        "fuzz: {name}: {} bytes still panic at {place}\n      input: {}",
        data.len(),
        out.display()
    );
}

fn hash(data: &[u8]) -> u64 {
    // FNV-1a.
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The inputs mutants start from: the fixtures that fit the target, and a few built here.
fn seeds(target: &str) -> Vec<Vec<u8>> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let wanted: &[&str] = match target {
        "front_matter" => &["md"],
        "page" => &["html"],
        "reply" => &["json"],
        "rpc" => &["txt"],
        "pdf" => &["pdf"],
        _ => &[],
    };
    let mut seeds = Vec::new();
    let mut stack = vec![fixtures];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| wanted.contains(&e))
                && let Ok(bytes) = std::fs::read(&path)
            {
                seeds.push(bytes);
            }
        }
    }
    match target {
        "front_matter" => {
            seeds.push(
                b"\xef\xbb\xbf---\r\ntitle: CRLF\r\ntags:\r\n- one\r\n- two\r\n---\r\nbody\r\n"
                    .to_vec(),
            );
            seeds
                .push(b"---\nlist: [a, [b, c], 'd, e', \"f\\\"g\"]\nx: |\n  block\n...\n".to_vec());
        }
        "reply" => {
            seeds.push(br#"{"title":"t","summary":"s","body":"b"}"#.to_vec());
        }
        "rpc" => {
            use fastcapture::model::copilot::rpc::frame;
            use serde_json::json;
            let mut stream = frame(&json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}));
            stream.extend(frame(&json!({
                "jsonrpc": "2.0",
                "method": "session.event",
                "params": {"type": "assistant.message", "data": {"content": "a note"}}
            })));
            stream.extend(
                b"Content-Length: 2\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{}".iter(),
            );
            seeds.push(stream);
        }
        "pdf" => {
            seeds.push(pdf_build::pdf(
                &[
                    Page::Text(&["Hello from the first page", "and a second line"]),
                    Page::Picture,
                ],
                false,
            ));
            seeds.push(pdf_build::pdf(&[Page::Text(&["Locked text here"])], true));
        }
        _ => {}
    }
    if seeds.is_empty() {
        seeds.push(Vec::new());
    }
    seeds
}

/// Tokens that mean something to the target, for mutations to insert.
fn dictionary(target: &str) -> Vec<&'static [u8]> {
    let common: &[&[u8]] = &[b"\n", b"\r\n", b"\0", b"\xef\xbb\xbf", b"\xff", b"\xc3"];
    let own: &[&[u8]] = match target {
        "front_matter" => &[
            b"---",
            b"...",
            b": ",
            b"- ",
            b"[",
            b"]",
            b",",
            b"\"",
            b"'",
            b"\\",
            b"\\u",
            b"#",
            b"|",
            b">",
            b"  ",
            b"\t",
            b"tags: ",
            b"aliases: ",
        ],
        "page" => &[
            b"<div>",
            b"</div>",
            b"<p>",
            b"<article>",
            b"<table>",
            b"<tr><td>",
            b"<pre><code>",
            b"<ul><li>",
            b"<h1>",
            b"<a href=\"",
            b"<img src=\"",
            b"<meta property=\"og:title\" content=\"",
            b"<title>",
            b"<script>",
            b"<!--",
            b"&amp;",
            b"&#x",
            b"<blockquote>",
            b"<br>",
        ],
        "reply" => &[
            b"{",
            b"}",
            b"[",
            b"]",
            b"\"",
            b":",
            b",",
            b"null",
            b"true",
            b"\"title\"",
            b"\"links\"",
            b"\"home\"",
            b"\\ud83e",
            b"\\u0000",
            b"1e999",
        ],
        "rpc" => &[
            b"Content-Length: ",
            b"\r\n\r\n",
            b"18446744073709551615",
            b"99999999999999999999",
            b"0",
            b"{}",
            b"{\"jsonrpc\":\"2.0\"}",
            b":",
        ],
        "pdf" => &[
            b"obj",
            b"endobj",
            b" 0 R",
            b"<<",
            b">>",
            b"/Type",
            b"/Pages",
            b"/Kids",
            b"/Count",
            b"/Length",
            b"stream\n",
            b"endstream",
            b"xref",
            b"trailer",
            b"startxref",
            b"/Filter",
            b"/FlateDecode",
            b"/Font",
            b"/Encoding",
            b"/ToUnicode",
            b"(",
            b")",
            b"Tj",
            b"TJ",
            b"BT",
            b"ET",
            b"-1",
            b"4294967295",
            b"99999999999",
        ],
        _ => &[],
    };
    common.iter().chain(own).copied().collect()
}

fn mutate(input: &mut Vec<u8>, corpus: &[Vec<u8>], dictionary: &[&[u8]], rng: &mut Rng) {
    let len = input.len();
    match rng.below(9) {
        // Flip a bit.
        0 if len > 0 => {
            let at = rng.below(len);
            input[at] ^= 1 << rng.below(8);
        }
        // Set a byte to something that often matters.
        1 if len > 0 => {
            let at = rng.below(len);
            input[at] = [0, 0xff, 0x7f, 0x80, b'\n', b'0', b'9', b'-', b' ', b'<'][rng.below(10)];
        }
        // Insert random bytes.
        2 => {
            let at = rng.below(len + 1);
            let bytes: Vec<u8> = (0..1 + rng.below(8)).map(|_| rng.next() as u8).collect();
            input.splice(at..at, bytes);
        }
        // Delete a range.
        3 if len > 0 => {
            let at = rng.below(len);
            let end = (at + 1 + rng.below(64.min(len - at))).min(len);
            input.drain(at..end);
        }
        // Repeat a range, many times now and then, to nest and to grow.
        4 if len > 0 => {
            let at = rng.below(len);
            let end = (at + 1 + rng.below(256.min(len - at))).min(len);
            let piece = input[at..end].to_vec();
            let times = if rng.chance(8) { 1 + rng.below(500) } else { 1 };
            let to = rng.below(len + 1);
            input.splice(to..to, piece.repeat(times));
        }
        // Splice in a piece of another seed.
        5 => {
            let other = &corpus[rng.below(corpus.len())];
            if !other.is_empty() {
                let at = rng.below(other.len());
                let end = (at + 1 + rng.below(512.min(other.len() - at))).min(other.len());
                let to = rng.below(len + 1);
                input.splice(to..to, other[at..end].iter().copied());
            }
        }
        // Cut it short.
        6 if len > 0 => input.truncate(rng.below(len)),
        // Change a number: digits, made larger, negative or huge.
        7 if len > 0 => {
            if let Some(at) = (0..16)
                .map(|_| rng.below(len))
                .find(|&i| input[i].is_ascii_digit())
            {
                let number: &[u8] = [
                    &b"0"[..],
                    b"-1",
                    b"65535",
                    b"4294967296",
                    b"18446744073709551615",
                ][rng.below(5)];
                input.splice(at..at + 1, number.iter().copied());
            }
        }
        // Insert a token.
        _ => {
            let token = dictionary[rng.below(dictionary.len())];
            let at = rng.below(len + 1);
            input.splice(at..at, token.iter().copied());
        }
    }
}
