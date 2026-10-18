//! Times the capture path against M1's budgets: the box from "open" to visible, a 4K screenshot
//! from clipboard pixels to PNG, and Ctrl+Enter's filing of a capture holding it. Run it as a
//! release build: `cargo run --release --example bench`.

use fastcapture::capture::submit::submit;
use fastcapture::capture::{Attachment, Capture};
use fastcapture::platform::png::{Pixels, encode_png};
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
    };
    let mut submit_times = Vec::new();
    for _ in 0..10 {
        let started = Instant::now();
        submit(&notebook, &capture, Some(&queue), false).expect("files");
        submit_times.push(started.elapsed());
    }
    report("Ctrl+Enter with a 4K screenshot", 150, submit_times);

    let _ = std::fs::remove_dir_all(&temp);
}
