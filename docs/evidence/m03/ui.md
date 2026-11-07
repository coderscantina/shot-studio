# M03 evidence: the Record and Edit screens

Work item 8 of [milestone 03](../../milestones/03-opinion-video.md). Checked on 2026-11-07 on the reference MacBook Pro (M1 Pro, 32 GB, macOS 27), debug build, with other agents building and exporting at the same time.

## What it does

`shot-app <video-folder>` opens a video folder on its Edit screen, with Record one click away in the title bar. `shot-app [flags] <video.mp4>` still opens the preview spike, so `--bench` works as before.

- **Record**: tracks with device pickers (display, camera, mic, system audio), a live mic level, the transcript as stable words arrive from the mic tap through `Transcriber`, elapsed time, start and stop on the button or Space. Start fetches the whisper and VAD models first when they are missing (progress in the stage), loads the transcriber, then starts the session. Stop finishes the take, writes the transcript to the cache, appends a chapter `Take tNN` that keeps the whole take to `cut.kdl` and opens it in Edit. Capture errors, permissions included, show as an alert above the stage with capture's own message.
- **Edit**: three workspaces on ⌥1 to ⌥3 or the title bar switcher, the transport and the Edited/Untrimmed switch under the preview in each, and Export in the title bar.
  - **Text**: chapters with their edited length, the chapter's transcript as a document with a rail of timecode and layout per paragraph, short auto cuts as a faint seam, longer and manual cuts as a red length chip. Clicking a chip opens it to show the removed words struck through. Untrimmed shows every word, removed ones struck through. Click, drag and shift-click select words; ← and → move, with shift they extend. The selection toolbar sits under the paragraph that holds the end of the selection, 8 px clear of the text. Backspace cuts (`Cut::delete_words`), R restores (`Cut::restore`), 1 to 4 set Face, Face + content, Split or Content over the selection, or over the stretch under the playhead without one (`Cut::set_layout`, and `clear_layout` for the format's default). The inspector shows the selection and what is on screen at the playhead.
  - **Picture**: layout picker with 1 to 4, the preview at up to 1040 pt wide, three rolling transcript lines, what is on screen now (layout, zoom, overlays) and next (layout change, zoom), the overview bar.
  - **Timeline**: preview and the change list (removals by rule and by you with their count and length, zooms, layout changes), the overview bar, a ruler and the Layout, Zoom, Words and Voice lanes over 15 s around the playhead. Words in the lane select like in Text.
  - **Overview bar**: chapter blocks, cut ticks, a frame around the chapter on show (Text, Picture) or the lane window (Timeline), the lime playhead. Click or drag to seek.
  - **Preview**: a render thread plans the cut with `shot_render::plan`, decodes screen and camera forward and draws every frame with `Renderer::render` and `LayoutSpec::scene` into 1920x1080 `420f` buffers that gpui's surface element shows. Layouts, crops and zooms are the export's. A take without a camera draws its layouts without the camera slot.
  - **Export** runs `shot_render::export::export` for `main` into `.shot/export/main.mp4` on a thread, with progress and the result in the title bar.
- **No hidden edit state**: every edit goes through a `shot-core` function, is written with `Cut::to_kdl` and reloaded. The screen keeps only the workspace, the chapter on show, the selection, opened chips and the playhead. Edits keep the playhead on the same media.
- **Design**: tokens from `docs/design.md` in `src/theme.rs` (dark only, lime accent, red for recording and cuts, panels by shade, 8 to 14 px corners), Geist and Geist Mono embedded (OFL, `assets/fonts/LICENSE-GEIST.txt`), Hugeicons stroke rounded 1.5 px embedded as SVG (MIT, `assets/icons/LICENSE-HUGEICONS.md`), gpui-component's theme restyled with the tokens for its buttons and progress bars. Minimum window 1280x800.

## Files

`crates/shot-app/src/`: `studio.rs` (window, title bar, screens), `record.rs`, `edit/mod.rs` (state, actions, playback, export), `edit/text.rs`, `edit/picture.rs`, `edit/timeline.rs`, `edit/parts.rs` (preview, transport, overview, switcher), `engine.rs` (preview render thread), `model.rs` (pure view model), `folder.rs` (project files and transcripts), `peaks.rs` (Voice lane), `theme.rs`, `ui.rs`. Assets in `crates/shot-app/assets/`.

## Checks

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`: pass. 8 new unit tests in `model.rs` and `folder.rs`: block mapping of output frames, Untrimmed, removed words behind their cut, paragraph breaks, selection range, layout keys and the default clearing, overview mapping and timecodes, appending a take.
- `shot-app --bench --runs 1 --seconds 5 --seeks 5 screen-30s.mp4` (debug): PASS.

## Run on the target project

A copy of `$SHOT_FIXTURES/target-project` in the session scratchpad (text files copied, `media/t01` linked, a git repo for diffs), transcribed with `shot transcribe` (983 words).

Backspace on "boring parts" in Intro, then R on the opened chip:

```diff
     take t01 {
         remove from=0 to=800_000 by=silence
-        keep from=800_000 to=31_400_000
+        keep from=800_000 to=11_640_000
+        remove from=11_640_000 to=12_250_000 by=manual
+        keep from=12_250_000 to=31_400_000
         remove from=31_400_000 to=33_000_000 by=pause
```

R restored the file byte for byte. Key 3 over 21 selected words added `layout split from=3_960_000 to=10_540_000` and the preview switched to the split layout; key 1 (the opinion default) removed the line again.

Recording with camera, mic and system audio: the transcript appeared while `say` spoke into the room, Space stopped. `cut.kdl` held the new `Take t02` chapter 2.3 s after stop (a 42 s take), and Edit opened on it. A second take, t03, went the same way.

Export from the app of a one-chapter copy (Intro, 87.7 s): `.shot/export/main.mp4`, 3840x2160 HEVC 60 fps plus AAC, 87.66 s, 566 MB, sha256 `10d3ac6a6022f850…`, 57 s including segment renders, debug build of the app and shot-render under load.

## Screenshots

1600x1000 in dark, git-ignored, in this folder: `record-idle.png`, `record-recording.png`, `edit-text.png`, `edit-picture.png`, `edit-timeline.png`.

Against `docs/design.md` and the mockup canvas:

- **Title bar**: only Record and Edit in the nav, the other screens come with their milestones. No Commands (⌘K) and no workspace menu (saved workspaces, open-in per format, reset). Export added at the right.
- **Text**: no beat list under the chapters, no inline zoom or b-roll tags, no proposals. The selection toolbar offers Cut, Restore and the layout keys only; Highlight, Zoom and B-roll come later. It sits left-aligned under the paragraph rather than under the selected words. The inspector shows the selection and the playhead's layout, zoom and caption style instead of the highlight inspector. The ⌫ glyph in key hints draws small in the system font, since Geist Mono has no key symbols.
- **Picture**: no caption panel or draggable caption, captions are later. The layers list comes from the cut, so it shows layout, zooms and overlays only.
- **Timeline**: Layout, Zoom, Words and Voice lanes; no B-roll, Highlight or Music lanes, no proposal card and no H/Z selection toolbar. Words are packed into two rows by time instead of phrases. A hovered cut does not open. The lane window is a fixed 15 s with no zoom keys. The preview pane is 653 px wide instead of 740 so the transport fits under it.
- **Record**: no live screen preview and no prompter pane (milestone 05); the right pane is the live transcript instead. No beat hotkeys (A, N, M, Z, B) and no layout keys during recording.

## Not verified

- **Model download progress**: both models were present, so the download path in Record was not exercised.
- **Permission errors**: camera, mic and screen were granted in this session, so the alert was not seen with a real permission error.
- **Preview frame pacing**: playback through the render graph was watched, not measured. The bench still measures the spike's direct path only.
- **Preview colors**: the render graph writes BT.709 YCbCr, while gpui's surface shader decodes BT.601 (see `Matrix::Bt601` in `shot-media`), so preview colors are slightly off against export. Not measured.
- **Full 17 min export from the app**: only the one-chapter copy was exported, to keep the shared machine and disk free.

## Needs from other tasks

- `shot-render`: a BT.601 (or matrix-selectable) target for preview buffers, so the preview shows export's colors; and the frame cursor export uses, so `engine.rs` stops duplicating it.
- `shot-capture`: a screen frame tap (latest frame as an IOSurface) for the Record screen's live preview.
- `shot-core`: `Cut::append_take(take, duration)`; `folder.rs` builds the chapter itself for now.
- `shot-media`: audio output, the preview plays silent.
- `AGENTS.md` Demo section (Core): the app now runs with `cargo run -p shot-app -- <video-folder>`.
