//! Times the capture path against its budgets: the box from "open" to visible, a 4K screenshot
//! from clipboard pixels to PNG, Ctrl+Enter's filing of a capture holding it, the worker
//! building a model request from that capture, reading an article page and a 20-page PDF, and
//! the index of 10,000 notes: built cold, from its cache, and searched.
//! Run it as a release build: `cargo run --release --example bench`.

use fastcapture::capture::submit::submit;
use fastcapture::capture::{Attachment, Capture};
use fastcapture::extract::pdf::build::{self as pdf_build, Page};
use fastcapture::extract::{page, pdf};
use fastcapture::model::request;
use fastcapture::platform::png::{Pixels, encode_png};
use fastcapture::queue::Store;
use fastcapture::ui::capture_box::{self, Open, Sink};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;

struct Discard;

impl Sink for Discard {
    fn notebooks(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn file(&mut self, _capture: &Capture, _notebook: &Path) -> Result<(), String> {
        Ok(())
    }
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn report(name: &str, budget_ms: u128, samples: Vec<Duration>) {
    let median = median(samples);
    let verdict = if median.as_millis() <= budget_ms {
        "ok"
    } else {
        "OVER"
    };
    println!(
        "{name:<40} median {:>7.1} ms   budget {budget_ms} ms   {verdict}",
        median.as_secs_f64() * 1000.0
    );
}

fn main() {
    let temp = std::env::temp_dir().join(format!("fastcapture-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();

    // The box: from the hotkey's handler calling open() to the box shown and focused.
    let mut open_times = Vec::new();
    for _ in 0..15 {
        let started = Instant::now();
        let hwnd = capture_box::open(Open {
            sink: Box::new(Discard),
            notebook: temp.clone(),
            draft_dir: None,
            source_window: None,
            settings: None,
        })
        .expect("the box opens");
        open_times.push(started.elapsed());
        unsafe { DestroyWindow(hwnd) };
    }
    // The first open registers the window class and loads fonts; the hotkey's is like it.
    println!(
        "first box open: {:.1} ms",
        open_times[0].as_secs_f64() * 1000.0
    );
    report("hotkey to box visible", 100, open_times[1..].to_vec());

    // A 4K screenshot pasted as a bitmap becomes a PNG on a worker thread, not the UI thread:
    // the box shows "Preparing image…" meanwhile, so this has a looser budget.
    let (width, height) = (3840, 2160);
    let mut bgra = vec![0u8; (width * height * 4) as usize];
    // Gradients with noise in the low bits: harder to compress than a real screenshot, so the
    // timings are an upper bound.
    let mut seed: u32 = 0x9E37_79B9;
    for (index, pixel) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let noise = (seed & 0x0F) as u8;
        let x = (index as u32 % width) as u8;
        let y = (index as u32 / width) as u8;
        pixel.copy_from_slice(&[x ^ noise, y ^ noise, (x ^ y) ^ noise, 255]);
    }
    let pixels = Pixels {
        width,
        height,
        bgra,
    };
    let mut encode_times = Vec::new();
    let mut png = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        png = encode_png(&pixels).expect("encodes");
        encode_times.push(started.elapsed());
    }
    report("4K bitmap to PNG (worker thread)", 2000, encode_times);
    println!("    PNG size: {:.1} MB", png.len() as f64 / 1_048_576.0);

    // Ctrl+Enter: writing the attachment, the raw source and the job.
    let notebook = temp.join("Notes");
    let queue = temp.join("queue");
    std::fs::create_dir_all(&notebook).unwrap();
    let capture = Capture {
        text: "lag spikes during ledger batch??".to_owned(),
        attachments: vec![Attachment::Png(png)],
        source_window: None,
        source_url: None,
    };
    let mut submit_times = Vec::new();
    for _ in 0..10 {
        let started = Instant::now();
        submit(&notebook, &capture, Some(&queue), false).expect("files");
        submit_times.push(started.elapsed());
    }
    report("Ctrl+Enter with a 4K screenshot", 150, submit_times);

    // The worker building a model request for that capture: reading the raw source, scaling the
    // screenshot to the model's size and encoding it.
    let job = Store::new(queue).list().remove(0);
    let mut build_times = Vec::new();
    let mut image = None;
    for _ in 0..5 {
        let started = Instant::now();
        let request = request::build(&job, "2026-10-19").expect("builds");
        build_times.push(started.elapsed());
        image = request.images.into_iter().next();
    }
    report("request with a 4K screenshot (worker)", 500, build_times);
    if let Some(image) = image {
        println!(
            "    sent as {}×{} {}, {:.1} MB",
            image.width,
            image.height,
            image.media_type,
            image.data.len() as f64 / 1_048_576.0
        );
    }

    // A long article page, from HTML to its main content as Markdown (after download).
    let html = article();
    let mut page_times = Vec::new();
    let mut markdown = String::new();
    for _ in 0..10 {
        let started = Instant::now();
        markdown = page::extract(&html, "https://news.example/2026/10/lag").markdown;
        page_times.push(started.elapsed());
    }
    report("article page to Markdown", 300, page_times);
    println!(
        "    {:.0} KB of HTML, {:.0} KB of Markdown",
        html.len() as f64 / 1024.0,
        markdown.len() as f64 / 1024.0
    );

    // A 20-page text PDF read page by page.
    let lines: Vec<String> = (1..=45)
        .map(|n| {
            format!("Line {n} of the ledger batch review, with the numbers, names and decisions.")
        })
        .collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let pages: Vec<Page> = (0..20).map(|_| Page::Text(&lines)).collect();
    let bytes = pdf_build::pdf(&pages, false);
    let mut pdf_times = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        let read = pdf::read(&bytes, 20).expect("reads");
        assert_eq!(read.pages.len(), 20);
        pdf_times.push(started.elapsed());
    }
    report("20-page text PDF", 1000, pdf_times);

    index(&temp.join("notebook"));

    let _ = std::fs::remove_dir_all(&temp);
}

/// The index of 10,000 generated notes: built cold, built again from its cache, and searched.
fn index(notebook: &Path) {
    use fastcapture::index::notes::{self, NoteIndex};
    use fastcapture::index::scan;
    use fastcapture::index::search::{CANDIDATES, Searcher};
    const TOPICS: [&str; 16] = [
        "kafka", "ledger", "payments", "consumer", "invoice", "deploy", "budget", "garden",
        "recipe", "travel", "lag", "incident", "rust", "search", "hiring", "roadmap",
    ];
    println!("    writing 10,000 notes…");
    for number in 0..10_000usize {
        let folder = notebook.join(format!("area {}", number % 40));
        if number < 40 {
            std::fs::create_dir_all(&folder).unwrap();
        }
        let mut text = format!(
            "---\ntags: [{}, {}]\n---\n# Note {number} about {}\n\n",
            TOPICS[number % 16],
            TOPICS[(number / 16) % 16],
            TOPICS[(number * 7) % 16]
        );
        for word in 0..200usize {
            text.push_str(TOPICS[(number * 7 + word * 3) % 16]);
            text.push_str(&format!(" w{} ", (number * 31 + word * 17) % 5_000));
            if word % 20 == 19 {
                text.push_str("\n\n");
            }
        }
        std::fs::write(folder.join(format!("note {number}.md")), text).unwrap();
    }
    let cache = notebook.with_extension("cache");

    let started = Instant::now();
    let listed = scan::scan(notebook, &[], scan::NOTE_LIMIT).unwrap();
    let (index, _) = NoteIndex::build(notebook, &listed.entries, NoteIndex::default());
    let searcher = Searcher::new(index);
    let cold = started.elapsed();
    assert_eq!(searcher.index().notes.len(), 10_000);
    report("index of 10,000 notes, cold", 5000, vec![cold]);
    notes::write_cache(&cache, searcher.index()).unwrap();

    let mut warm = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let previous = notes::read_cache(&cache);
        let listed = scan::scan(notebook, &[], scan::NOTE_LIMIT).unwrap();
        let (index, changed) = NoteIndex::build(notebook, &listed.entries, previous);
        assert!(!changed);
        let rebuilt = Searcher::new(index);
        warm.push(started.elapsed());
        assert_eq!(rebuilt.index().notes.len(), 10_000);
    }
    report("index of 10,000 notes, from its cache", 500, warm);

    let query = "Consumer lag on the payments ledger after the deploy, incident w42 w1999 kafka \
                 budget. "
        .repeat(25);
    let mut searches = Vec::new();
    for _ in 0..10 {
        let started = Instant::now();
        let hits = searcher.search(&query, CANDIDATES);
        searches.push(started.elapsed());
        assert_eq!(hits.len(), CANDIDATES);
    }
    report("search of 10,000 notes, 2,000 chars", 20, searches);
}

/// A news page as sites serve them: navigation, a sidebar of links, 150 paragraphs of
/// article, comments and a footer.
fn article() -> String {
    let mut html = String::from(
        "<!doctype html><html><head><title>Lag</title>\
         <meta property=\"og:title\" content=\"Payments lag\"></head><body><header><nav><ul>",
    );
    for n in 0..100 {
        html.push_str(&format!(
            "<li><a href=\"/section/{n}\">Section {n}</a></li>"
        ));
    }
    html.push_str("</ul></nav></header><div class=\"layout\"><aside class=\"sidebar\"><ul>");
    for n in 0..60 {
        html.push_str(&format!(
            "<li><a href=\"/story/{n}\">Another story, number {n}</a></li>"
        ));
    }
    html.push_str("</ul></aside><main><article><h1>Payments lag</h1>");
    for n in 0..150 {
        html.push_str(&format!(
            "<p>Paragraph {n}: the nightly ledger batch wrote millions of records, and the \
             consumer group fell behind, so lag peaked, then recovered once the batch was done. \
             <a href=\"/ref/{n}\">A reference</a> and <em>some emphasis</em> keep it realistic.</p>"
        ));
        if n % 30 == 0 {
            html.push_str("<h2>A section</h2><ul><li>One point</li><li>Another point</li></ul>");
        }
    }
    html.push_str("</article></main></div><section class=\"comments\">");
    for n in 0..40 {
        html.push_str(&format!(
            "<div class=\"comment\"><p>Comment {n}, with an opinion or two.</p></div>"
        ));
    }
    html.push_str("</section><footer>© News</footer></body></html>");
    html
}
