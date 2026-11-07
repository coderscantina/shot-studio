# Milestone 06 evidence: product videos

Recorded 2026-11-07 on `task/m06-product`. Machine: MacBook Pro, Apple M1 Pro, 32 GB, macOS 27, rustc 1.99.1. Two other milestone agents built and measured on the same machine and target dir at the same time, so timings carry that noise. Screenshots and golden frames are in `m06/` (git-ignored).

## Result

| Criterion | Evidence | Result |
| --- | --- | --- |
| On the 30 s screen fixture, zooms land on every click and never cut off the cursor | `auto_zoom_lands_on_every_click_and_never_cuts_off_the_cursor` (ignored test, `crates/shot-render/tests/product.rs`): the product tour rule (`click typing scale=160`, smooth motion, window layout) writes 10 zooms for the 15 clicks. Every click is at full 160 % with the click point in view, and the smoothed cursor is inside the visible crop on all 1800 frames | Pass |
| Cursor effects change after recording without re-recording | The cursor, ripple and keystrokes are drawn from `events.jsonl` at render time. `the_cursor_follows_the_zoom_and_its_style_without_new_media` checks a size change on the same media; `a_cursor_style_change_re_renders_only_chunks_that_show_the_screen` changes `cursor` in `format.kdl` and re-exports | Pass |
| Code from git renders the real diff of a fixture repo commit range | `shot code-diff --code $SHOT_FIXTURES/demo-channel --range b8ff30b..9d5f5c1` wrote `sources/tighter-intro.diff` (the `tighter-intro` proposal's change to `cut.kdl`) and a `code-diff` overlay. The export types it out and highlights line 10 from 23 s: `m06/export-24.5.png` | Pass |
| Preview and export show the same frames within the golden-frame tolerance | `preview_and_export_show_the_same_frames`: five frames of the tour (zoom easing in, ripple with the typed run, a pan, a click) rendered through the preview path (1920x1080 `420f` BT.601) against the exported 4K HEVC frame halved to 1080p. Mean difference 1.09 to 1.33 code values, p99 17 to 22. Tolerance: mean under 2, p99 under 24 | Pass |

## How it works

- `shot_render::frame::compose` decides everything one output frame shows, and both the export workers and the preview engine call it. The render graph draws its `Scene` as before and its `Drawing` through a new vector pass: vello renders the shapes and parley the text into an RGBA texture on the graph's own wgpu device, and `vector.wgsl` composites it into the 4:2:0 target in the target's range and matrix. Each half renders only the rectangle it covers; the window decor under the layers is drawn once and reused while it stays the same.
- Zooms center on their `x y` and, with `zoom-motion smooth`, ease in and out over 450 ms (at most a third of the zoom) inside their own range, panning when one zoom starts where the previous ends. The crop is moved as little as needed to stay inside the source.
- The `window` layout places the screen 16:9 inside the padding and device bezel on the style's background, with corners, a blurred shadow and a MacBook or iPhone frame (`m06/export-macbook.png`). Capture records no window bounds, so the whole screen is the window.
- `events.jsonl` is in each take's media identity, so a new log re-renders.
- Auto zoom lives in `shot_render::autozoom` (pure) and `shot_render::rules` (runs it over a video folder). It writes `zoom ... by=click|typing|cursor` and replaces only those on a rerun.

## Measurements

- Export of the 30 s tour (window layout, smooth zooms, cursor 140 % with smoothing and ripples, all keystrokes, a 10 s code-diff): 1800 frames in 19.9 s, 90 fps. The same cut with cursor hidden, keystrokes off and no overlay: 19.0 s, 95 fps. The effects cost about 5 %.
- Cursor style change on a cut of the target project with 2 s of `face` and 2 s of `split`: the re-export renders 5 of 10 chunks, the five that show the screen, in 1.5 to 2.7 s against 3.5 to 4.1 s for the first export (two runs).
- On a product tour every chunk shows the screen, so a cursor change re-renders all of them, as decision 0016 says.

## Product demo screen

A fourth Edit workspace, Product, opens by default for `product-tour` videos (`m06/product.png`, 1600x1000). Differences from `docs/mockup/Product.dc.html`:

- Settings step to their next value on click instead of opening a picker. The trigger control is three toggles, not a single choice, because the rule takes several triggers.
- "Pick commits" is not built: the pane shows the code-diff overlay at the playhead and names `shot code-diff` for adding one.
- Zoom blocks use the zoom blue of `design.md` instead of the mockup's lime, and carry their provenance ("160% · click").
- No Voice and Music lanes; the mockup's zoom rectangle and cursor path drawn over the preview are not drawn.

## Not verified

- Clicking the settings in the running app. The screen was only rendered and screenshotted; every setting goes through the same `VideoFolder::write_format`, and auto zoom reruns through `rules::auto_zoom`, both exercised by the CLI runs.
- Preview frame rate with the vector pass. Export at 4K costs about 5 %, so the 1080p preview should stay at 60 fps, but `--bench` was not rerun with effects on.
- The highlight at a transcript phrase on real speech: `phrase_at` is unit-tested and wired into `shot code-diff --phrase`, but the screen fixture has no transcript.
- The fixture draws its own cursor into `screen.mp4`, so the frames show both. Real captures have no cursor.
