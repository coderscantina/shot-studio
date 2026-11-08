# Milestone 13: Takes, scenes and studio tools

## Outcome

Mike's change requests of 2026-11-08, after using the council polish branch. A video can be composed from the best parts of several takes, a take can switch scenes with real transitions, and the channel's look, overlays, sounds, memes, captions and thumbnails each get a tool of their own instead of a file to hand-edit. Each tool should set the bar for its category, not reach parity.

## Dependencies and ownership

Builds on `polish/council-round1`. Seven workstreams run in parallel, each in its own worktree and task branch, merged into `m13/integration`. A workstream owns the files listed for it. Shared files (`crates/shot-core/src/cut.rs`, `format.rs`, `brand.rs`, `docs/contracts/project-files.md`, `crates/shot-app/src/studio.rs`, `ui.rs`, `theme.rs`, workspace `Cargo.toml`) take small additive changes only: a new node, a new field, a new row, a new nav entry. No reshuffles, renames or reformatting of code another stream may touch. Conflicts are resolved at integration.

| Stream | Items | Owns |
| --- | --- | --- |
| A takes | 1, 2 | `crates/shot-app/src/takes/` (new), the Cut screen's entry to it, take functions in `shot-core/src/edit.rs` |
| B scenes | 3, 10 | `shot-render/src/layout.rs` and scene transitions, custom layouts in `shot-core/src/format.rs` and `brand.rs`, `crates/shot-app/src/layouts/` (new) |
| C zoom and camera | 7, 12 | `shot-render/src/zoom.rs`, `autozoom.rs`, camera motion, `crates/shot-app/src/zoom.rs` |
| D text and captions | 9, 11, 16 | `crates/shot-app/src/edit/text.rs`, `edit/picture.rs` caption parts, `shot-render/src/captions.rs`, caption style in core, every `backspace` binding |
| E library | 4, 8 | `crates/shot-library` (new crate), `crates/shot-app/src/library/` (new) |
| F brand and overlays | 5, 6 | `crates/shot-app/src/brand/` (new), overlay editing in `crates/shot-app/src/edit/overlays.rs` |
| G record and ship | 13, 14, 15 | `crates/shot-app/src/record.rs`, `prompter/`, `shot-capture`, `crates/shot-app/src/ship.rs` thumbnail parts, `shot-ship/src/thumbnails.rs` |

## Work

1. **Takes view.** For each script beat (or, without a script, each sentence group), every take's attempt side by side: transcript text, duration, waveform, a thumbnail, play. Rate an attempt (1 to 5 stars and a circle for the pick), mark a part of one by selecting words, and compare two attempts back to back (A/B play, aligned text diff). Ratings and picks live in `cut.kdl`.
2. **Compose from takes.** "Use this" puts an attempt, or a marked part of one, into the main cut in place of what holds that beat now. "Assemble picks" builds the cut from the circled attempts in script order. Today every recording appends a chapter that keeps the whole take and nothing chooses between takes; that is the gap Mike hit in Cut.
3. **Scene switches.** A layout change inside a take can carry a transition: a spring morph of the slots between the two layouts (the default), a dissolve, a zoom-through or a cut, with a length and an optional sound. Preview and export draw it the same way.
4. **SFX and meme search.** Search free, licensed sounds and images online from the app, audition, download into the channel with license and attribution recorded, and place as a sound, a `pop`, or a `b-roll` cutaway. Provider: Openverse (no key, CC licensed), see the decision record this stream adds.
5. **Channel theme editor.** Edit `brand/brand.kdl` in a form with a live preview frame: colors, fonts, logo, camera frame, motion preset and feel controls, sounds, intro, outro and end card. Several saved themes per channel, one active.
6. **Overlay editor.** Create and edit overlays as reusable presets (lower third, link card, callout, pop, chapter band and brand Lottie templates) with every parameter as a control and a live preview, and place them from the Edit screen.
7. **Zoom framing and ramps.** A zoom frames `auto` (cursor, click or the layout's center as today), the center, or one of four quadrants, and ramps in and out over set lengths instead of switching. Set in the zoom inspector.
8. **Library.** One place for memes, overlays, effects, sounds and music: the bundled set, the channel's own and downloaded items, with search, preview, tags and license, and drag or "Place" into the edit.
9. **Text editor effects.** Underline and highlight words in the transcript with the effect they carry (zoom, highlight, sound, pop), in the lane hues from the timeline, and add or change an effect from the selection. Layout switches show inline where they start and can be changed or removed there.
10. **Layout builder.** Define a channel layout by placing screen and camera slots on a 16:9 canvas (position, size, corners, crop, order), saved in the brand and offered next to the built-in layouts.
11. **Burned-in captions.** Edit caption text in place (through caption fixes), and style captions: font, size, weight, colors, highlight color, position, words per line, case. Saved in the brand with a per-video override.
12. **Talking head handheld.** A camera effect that looks humanly hand-held: low-frequency drift with small rotation, configurable strength, never seasick. Off by default.
13. **Quiet prompter settings.** Daily controls stay visible (mode, speed, text size, mirror); everything else moves behind one "More settings" disclosure that remembers being closed.
14. **Input switches.** Turn screen, camera, mic and system audio on or off per recording on the Record screen; a take records only the tracks that are on, and everything downstream copes with a missing track.
15. **Thumbnail studio.** Pick a frame (candidates or any moment), cut out or frame the face, add headline text in brand fonts and colors, background, arrows or emoji, compare variants, and export the thumbnail Ship uploads.
16. **Delete key.** Every place Backspace removes or deletes something also takes Delete (forward delete).

## Acceptance criteria

- Each item works in the real app on a copy of the target project or the demo channel, checked by real clicks and keys, with a screenshot at 1600x1000 in `docs/evidence/m13/`.
- New `cut.kdl`, `format.kdl` and `brand.kdl` nodes are in the contract with an example and a parse and write-back test.
- fmt, workspace clippy and `cargo test --workspace` pass.

## Evidence and handoff

Report in `docs/evidence/m13-takes-scenes-and-studio-tools.md`, screenshots in `docs/evidence/m13/`.
