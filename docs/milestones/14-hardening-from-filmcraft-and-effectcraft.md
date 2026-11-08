# Milestone 14: Hardening from FilmCraft and EffectCraft

## Outcome

Mike asked on 2026-11-08 to bring in what FilmCraft and EffectCraft (storytold, both pure Rust) do better, without feature creep. Most of it is reliability: project files that survive crashes and concurrent writers, sharper word timings for auto cut, MCP tools that tell an agent what they do, an app an agent can drive one window at a time, and tests and benchmarks that catch regressions. One item is visible: a cut-out face cam in video.

## Dependencies and ownership

Builds on `m13/integration`. Six workstreams run in parallel, each in its own worktree and task branch, merged into `m14/integration`. Shared files (`crates/shot-core/src/*.rs` model files, `docs/contracts/project-files.md`, workspace `Cargo.toml`, `crates/shot-app/src/main.rs`, `studio.rs`) take small additive changes only. Write call sites that stream A replaces in another stream's crate are one-line changes. Conflicts are resolved at integration.

| Stream | Items | Owns |
| --- | --- | --- |
| A files | 1, 2, 3 | the atomic write and writer lock helper, every project file write site, the `version` node in `shot-core`, the recovery of unsaved editor text in `shot-app` |
| B word bounds | 4 | `shot-analyze/src/transcribe.rs` and a new word-bounds module |
| C MCP contract | 5, 6 | `crates/shot-mcp` |
| D app control | 7, 8 | a new keymap and control module in `crates/shot-app`, disabled reasons on existing buttons |
| E face cut-out | 9 | person masks in `shot-analyze`, the camera slot in `shot-render`, the camera node in `shot-core` and the contract |
| F tests and bench | 10, 11 | `shot-render/src/golden.rs`, `lottie.rs` warnings, transition tests, a test deadline helper, `xtask` bench, a frame-grid property test in `shot-core` |

## Work

1. **Safe saves.** Every write of a project file (`cut.kdl`, `format.kdl`, `video.kdl`, `script.md`, `cut.lock`, brand and theme files, credit and layout files) goes through one helper: a uniquely named temp file in the same folder, flushed to disk, renamed over the target, then the folder flushed. The app, `shot` and the MCP server take an advisory lock on the video folder while they read, change and write, so two writers wait for each other instead of losing an edit.
2. **Version guard.** `cut.kdl`, `format.kdl`, `video.kdl` and `brand.kdl` carry a `version` node. A file without one is version 1. A file from a newer version is refused with a message that names both versions, instead of half-read and then overwritten without the fields this build doesn't know.
3. **Recover unsaved editor text.** Edits are written to disk as they happen, so most work is safe already. Editors that hold text in memory before writing it (find them, the script editor is a candidate) keep a draft beside the file about a second after each change and offer it back after a crash.
4. **Word bounds from the audio.** After whisper, each word's start and end are re-measured against 10 ms audio levels: a quiet stretch of 200 ms or more inside a word ends it, quiet at the edges is trimmed, no word gets shorter than 60 ms. Pauses then belong to the gap, not to the word before it, and silence and filler cuts land closer to the speech.
5. **MCP tool contract.** Every tool carries the MCP hints (read-only, destructive, idempotent, open world), and a test enforces that each tool sets them. An unknown argument is an error that names the accepted ones. A panicking tool returns an error and the session keeps running. Long-running tools report progress and stop on cancel.
6. **Confined paths.** Every path an MCP tool takes stays inside the channel repo or the video folder: no absolute paths, no `..`, symlinks resolved before the check.
7. **One keymap, commands that explain themselves.** The app's key bindings live in one list, and a test fails when two bindings in the same context share a key or a binding names no action. Buttons that can be disabled say why in their tooltip.
8. **Control channel.** A running app listens on a Unix socket of its own (`$TMPDIR/shot-app-<pid>.sock`, user only). One JSON request per line: list windows, list commands of a window with their keys and whether they can run now, dispatch a command to one window, type text, and save a screenshot of one window. A malformed line closes the connection. Agents then drive one instance and one window instead of sending keys to whichever window has focus.
9. **Face cut-out.** The camera slot can cut out the person (Apple Vision person segmentation, per frame, masks cached beside the take like other analysis), and draw them over the screen or the brand background. Frames with a low-confidence mask fall back to the plain camera. Set on the brand's camera node and per video, drawn the same in preview and export.
10. **Test hardening.** A failing golden test writes the actual frame and a difference image (x8) under `target/golden-failures/`. Lottie import returns warnings for what velato skips, and the app shows them on import. Transition tests check that progress 0 is exactly the outgoing frame and 1 exactly the incoming one. A deadline helper fails a test that runs too long and names the input it was on, used by the parser tests over broken files. A property test checks that cut points the editor writes land on the frame grid of the take's rate.
11. **Benchmark gate.** `cargo xtask bench` runs fixed workloads on the generated fixtures (preview frames, chunk export, transcription), writes JSON lines with wall time and peak memory, compares with `--compare <file>` and fails over `--fail-over <pct>`.

## Acceptance criteria

- A killed writer never leaves an empty or partial project file, and two concurrent `shot` writes to one video both land (test with two processes).
- A `cut.kdl` with `version 99` is refused with both versions named; files without `version` still open and the round trip stays byte-identical for them.
- On the `say` fixture, word ends move off trailing silence and the auto-cut result changes only where pauses were absorbed (before and after numbers in the evidence).
- An MCP client lists every tool with all four hints; an unknown argument and a `..` path each come back as errors; a panicking test tool leaves the session usable.
- The keymap test passes and fails when a duplicate is added. The control socket lists, dispatches and screenshots one window of two open ones.
- A face cut-out renders over the screen in preview and export with matching golden frames, and the camera falls back on a frame without a person.
- A golden failure leaves the two images. `cargo xtask bench --compare` fails on an injected slowdown.

## Evidence

`docs/evidence/m14-hardening.md`, screenshots in `docs/evidence/m14/`.
