# Milestone 14 evidence: hardening from FilmCraft and EffectCraft

Source: `m14/integration`, built from `m13/integration`. Six streams (A to F) built in their own worktrees and merged here on 2026-11-08. Machine: Mike's Mac (M1 Pro), macOS 27.0. Fixtures from `scripts/fixtures.sh`, copied to `/tmp` for each run. Screenshots in `docs/evidence/m14/` (git-ignored).

## Checks on the merged branch

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace`: pass, except the known flaky watchdog test, which failed in one of three `shot-app` runs.
- Ignored GPU tests touched by this milestone: `tests/cutout.rs` (2) and the monitor golden test pass. In `join_reel`, dissolve passes at both edges. Zoom-through and morph fail at progress 0 and stay `#[ignore]`d with their numbers (see item 10).
- Fixed at integration: the cut-out tests used the old `golden::assert_match` signature (stream F added the frame width).

## By item

| Item | Stream | Shown by | Not verified |
| --- | --- | --- | --- |
| 1 Safe saves | A | `shot-fs` tests: a failed write keeps the old file; two lock holders both land; `shot caption-fix` waits for a held lock and both edits land | The app's lock under a real UI; read-only video folders (opening takes the lock, so they fail to open) |
| 2 Version guard | A | Version 99 refused for all four files; round trips stay byte-identical | |
| 3 Unsaved editor text | A | Nothing to build: every text field commits on Enter, blur or each change, and the script is edited in the user's editor | |
| 4 Word bounds | B | On the six `say` clips 21 of 99 words moved, by 208 ms on average, mostly ends. A filler cut went from 730 to 1280 ms to 730 to 970 ms. The silence cut in `long-silence` grew from 7.32 to 8.00 s. The ignored autocut fixture test passes | Autocut on the 17 min target project; a noisy real mic |
| 5 MCP contract | C | All 22 tools carry the four hints (test); unknown arguments name the accepted ones; a panicking test tool leaves the session serving. No tool runs long, so no progress or cancel | A real client session through the Claude bridge |
| 6 Confined paths | C | `confine.rs` tests: absolute, `..` and symlink escapes rejected | |
| 7 Keymap and reasons | D | All 164 bindings in `keymap.rs`, diffed against the old ones; the duplicate test fails when one is added. Six disabled conditions got a tooltip reason | The tooltips on screen |
| 8 Control socket | D | Debug app with studio, welcome and library windows: windows and commands listed, a dispatch refused in welcome and run in studio, typing reached only the library search, single-window screenshots (`control-typed-into-library.png`) | Click by element id (gpui has no clean way, not built); a socket left behind by a killed app |
| 9 Face cut-out | E | Golden test of the cut-out over the screen and of the fallback; a 12 s export with cutout on (`e-cutout-face-content.png`, `e-cutout-face.png`, `e-cutout-fallback.png`). About 38 s of mask work per minute of 1080p60 camera | A real face (Vision finds no person in the stand-in, so the evidence uses written masks); the app switches on screen |
| 10 Test hardening | F | Golden failures write `-actual.png` and `-diff.png`; Lottie warnings show under "Added sticker"; the parser deadline test catches and names a hang | Frame grid: the editor counts frames at 60 fps, so cut points miss the grid of other rates by up to 20 ms (test ignored with numbers). Zoom-through and morph do not start exactly on the outgoing frame |
| 11 Benchmark gate | F | `cargo xtask bench`, release, 6 s cut: edit-preview 8938 ms, glass 7506 ms, export 4944 ms. A half-time baseline fails with "export is 84.1% slower" | `shot-app --bench` hangs without a display, so it is not a workload; transcription is left out |
