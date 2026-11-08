# Milestone 13 evidence: takes, scenes and studio tools

Source: `m13/integration`, built from `polish/council-round1`. Seven streams (A to G) built in their own worktrees and merged here on 2026-11-08. Machine: Mike's Mac, macOS 27.0, release build of `shot-app`. Fixtures: `target-project` and `demo-channel` from `scripts/fixtures.sh`, copied to `/tmp` for each run. Screenshots in `docs/evidence/m13/` (git-ignored), named by stream letter, `int-` for checks on the merged build.

## Checks on the merged branch

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace --no-fail-fast`: every test passes. The known flaky `the_watchdog_names_where_a_stalled_thread_waits_or_runs` passed in one full run and failed in the next, as in the known gaps.
- The merged app on a copy of the target project: Text shows effect chips and underlines next to the other streams' work (`int-open.png`), Picture shows the Camera section (`int-takes.png` was first taken there by mistake), Cut's Takes lines up 33 beats of t01 (`int-takes.png`), the Library lists sections and overlays (`int-library-overlays.png`). On a copy of the demo channel the theme window opens with ⌘⇧B and its Hand-held slider writes `camera handheld=50` to `brand/brand.kdl` (`int-theme.png`, `int-theme-camera.png`).

## By item

| Item | Stream | Shown in | Not verified |
| --- | --- | --- | --- |
| 1, 2 Takes view and compose | A | `a-takes-1600.png`, `a-compare-a.png`, `a-use-marked.png`, `a-assembled.png`, `a-takes-1280.png` | A real recording landing on Takes, a migrated video, a video with prompter beat markers |
| 3 Scene switches | B | `b-switch-popover.png`, `b-export-switches.png`, `b-export-custom-morph.png` | Switch sounds by ear, 1280x800 |
| 4, 8 Library and search | E | `e-sound-search.png`, `e-meme-search.png`, `e-download-progress.png`, `e-placed-meme.png` | Audition by ear, the offline state in the app, export with an image pop |
| 5, 6 Theme and overlay editor | F | `f-theme-*.png`, `f-generator.png`, `f-overlay-editor-*.png`, `f-preset-placed.png` | File pickers, New theme and Delete, Lottie presets |
| 7 Zoom framing and ramps | C | `c-framings.png`, `c-ramp-in-sequence.png`, `c-zoom-inspector.png` | An export run; the ramp ease reads shorter than its number |
| 9, 11, 16 Text effects, captions, Delete | D | `d-effects-text.png`, `d-layout-chip-menu.png`, `d-captions-style.png`, `d-short-karaoke.png` | Style-rule effects and sound chips on a migrated video with media, Vision finding words on screen |
| 10 Layout builder | B | `b-builder-new.png`, `b-builder-snap.png`, `b-lane-menu.png` | Renaming a layout |
| 12 Hand-held camera | C | `c-handheld-30-sequence.png`, `c-handheld-80-sequence.png`, `int-theme-camera.png` | An export run |
| 13 Quiet prompter settings | G | `g-prompter-closed.png`, `g-prompter-open.png` | |
| 14 Input switches | G | `g-record-camera-off.png`, `g-edit-take-without-camera.png` | Takes with only the mic, or screen off with system audio on; a full export with a missing track |
| 15 Thumbnail studio | G | `g-thumbnail-studio.png`, `g-ship-designed-thumbnail.png` | Face cutout and expression candidates on a real face (Vision does not see the fixture's face as a person) |

## Fixed at integration

- L picks a layout for the selection in Text (D), so the layout builder (B) moved to ⇧L.
- Undo keeps both the video's brand layer (C) and the channel's brand file (B).
- The chunk key of the camera combines channel layouts (B) and hand-held strength (C).
- A take without a camera or screen (G) falls back on both sides of a scene switch (B).
- "Use this" in Takes (A) keeps a layout's switches only on edges it does not move (B).
- Switching themes (F) moved channel layouts (B) into the old theme's file, so a video using one could not export. Layouts now stay with the channel; the theme store's test covers it.
- The theme window's preview draws sample captions in the brand's caption look (D), and its Camera section sets the hand-held strength (C).
- The Library (E) lists the channel's overlay presets (F), places them, and opens presets and templates in the overlay editor.
- Text's layout menus use the same list as the Layout lane, channel layouts included.
- The Takes header no longer reads "1 take takes".
