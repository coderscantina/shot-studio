# Shot Studio

<div align="center">

**Record and edit developer videos like a software project.** A native macOS app written in Rust with gpui, with a `shot` CLI and MCP server on the same core. Everything runs locally.

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![macOS 26+](https://img.shields.io/badge/macOS-26%2B-lightgrey?logo=apple)](https://www.apple.com/macos/)

[Website](https://shot.app) · [Contributing](CONTRIBUTING.md) · [Development handoff](docs/README.md)

</div>

## What it does

- **The rough cut is ready when you press stop.** Transcription and analysis run while you record, so there is no processing step.
- **The edit is a plain text file in git.** `cut.kdl` holds ranges, layouts and effects, so you can read it in a diff and an agent can edit it.
- **4K export beats realtime, and re-exports only redo what changed.** Segments render in parallel and get cached by content.

Claude does the judgement work, such as script drafts, unsure cuts and Shorts picks. The heavy lifting stays on your Mac. Raw audio and video never leave the machine.

## Status

In development. There is no release yet. The plan and milestones live in [docs/README.md](docs/README.md).

## Build

Run `cargo build` from the repo root. The app needs macOS 26 or newer on Apple Silicon.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) before you open a pull request. Start with [docs/README.md](docs/README.md) if you want the design context.

## Security

Report vulnerabilities privately, see [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE), Coder's Cantina.

---

<div align="center">
  <p>Maintained by <a href="https://github.com/badmike">badmike</a></p>
</div>
