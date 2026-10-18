# FastCapture

A Windows tray app that captures what you come across (a thought, a screenshot, a web page, a PDF), keeps the original, and has an AI model, local or cloud, turn it into a well-written Markdown note in your notebook.

FastCapture writes plain Markdown files with YAML front matter and attachments into a notebook folder. [FastPad](https://github.com/coccor/FastPad), Obsidian, any other editor or an AI agent reads them. Neither app needs the other.

**Status:** early development. Milestone M1 (capture and raw sources, no AI yet) is being built.

- Design: [docs/specs/2026-10-18-fastcapture-design.md](docs/specs/2026-10-18-fastcapture-design.md)
- M1 plan: [docs/plans/2026-10-18-m1-capture-and-raw-sources.md](docs/plans/2026-10-18-m1-capture-and-raw-sources.md)

## Building

Requires Windows 10 1809 or later, the stable Rust toolchain with the MSVC target, and the Windows 10/11 SDK (for `rc.exe`, which embeds the application manifest).

```
cargo build --release
cargo test -- --test-threads=1
```

## License

MIT
