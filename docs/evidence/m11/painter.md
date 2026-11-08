# Milestone 11: the preview on the painter

Branch `task/m11-painter`. Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while two other milestone agents built and exported on the same machine (load average 4 to 12), so timings may read high. Screenshots are in `docs/evidence/m11/` (git-ignored).

## What changed

- **The preview paints what export paints.** The Edit screen's engine draws every output frame with `shot_render::paint::Painter::paint` on the plan from `shot_render::export::preview_plan`, the code export renders each chunk with. Segments, the intro and outro, the end card, dips, dissolves, mask wipes, zoom-throughs and stingers draw the same in both. The engine's own `render` is gone.
- **`EditMap` carries the plan.** `EditMap::preview(dir, files, shown, view)` builds the export plan of the files and ties its segments to the blocks of the shown cut; `EditMap::plan` is that plan. Output frames, the length and chapter spans count the bookends: a chapter runs from its export marker (at the intro when one opens it) to the next. The transport, overview bar, transcript times, lanes and seeks follow. If the plan cannot be built (a missing bumper, a brand Lottie that does not render), the screen says why and shows the cut without bookends. The untrimmed view stays the bare recordings.
- **Sound** takes the voice from `TimelineAudio::of_plan`: silent under the bookends, blended over transitions as export blends it.
- **Lanes.** Transition marks come from the plan's transitions, so they sit on the edges export cuts at, including the dips into and out of the bookends. The Layout lane shows Intro and Outro blocks.
- **Two-sided transitions.** Within a second before a dissolve or mask wipe, the engine keeps 18 frames (300 ms) queued instead of 6, so the window's first frame, which opens the incoming side's decoder at its handle (up to 82 ms in `transitions.md`), drains the queue instead of dropping frames. A real prefetch of the second side needs a painter API (see "Needs from other tasks").
- **Callouts.** The painter returns no layout, so for a paused frame in a segment outside a transition the engine lays the frame out again (`compose` with its own `Assets` and the recordings' sizes, probed once) to map clicks to screen pixels. Frames rendered while playing carry no map.
- **`--bench-edit --glass`** draws its launch reel in a pass of its own over the painted frame (`Renderer::draw_over`), since the painter has no decoration hook; glass reads back what the painter drew. Each run now prints the bookends and transitions it played through and where frames were dropped.
- **Lottie import** declares the file's slots with `lottie::slots`, none required. A slot id that is no valid `brand.kdl` name is left out.
- **Chapter times.** `shot_ship::spoken::Bookends` and `shot_ship::lock::bookends(dir, project)` place chapter marks on the export's timeline: moved on by the bookends before them, and a chapter an intro opens marked where the intro starts, as export marks it. `describe` takes the bookends; `shot describe`, `ship::plan` (the Ship screen's description) and the MCP `describe` tool pass them. The MCP `describe` and `caption_fixes` tools now read the lock's timeline. When export cannot place the bookends, MCP `describe` still drafts and says the chapter times leave them out; `shot describe` and the ship plan fail with export's error. A brand without intro and outro skips the plan.

## The migrated copy

A scratch channel repo with the target project as `videos/target` (media symlinked), transcribed (983 words), analysed (1751 elements) and migrated, with `formats/opinion.kdl` from the example channel and this brand: an intro (3 s HEVC bumper with a tone, plus the generated title), an outro (4 s bumper stretched to a 6 s end card), the circle camera with a border and soft shadow, `on video-start { transition dip }` and `on chapter-start { transition dissolve length=800_000 }`; a second run adds `on chapter-start { transition mask-wipe length=800_000 }` to the video's `format.kdl`. Opinion format, so the intro plays after the hook: 8 chapters, 2 bookends, 9 transitions (7 two-sided), 56,814 frames (946.9 s).

A per-chapter override cannot set a transition (an override targets an element kind, `transition` only takes `chapter-start` and `video-start`), so dissolves and mask wipes ran as two configurations rather than mixed in one cut.

## Preview against export

`engine::tests::the_preview_draws_transitions_and_bookends_as_the_export_does` (ignored, needs the fixtures): a channel with three 4 s chapters of the fixture take, the intro after the hook (the 30 s screen clip with the title), an outro (the camera clip) under a 3 s end card, `video-start` dips, and dissolves or mask wipes on chapter starts. It exports the cut at 4K, then seeks the preview engine to 23 frames per configuration: the start, a quarter, the frames either side of the cut and three quarters of all four transitions (dip into the intro, the two-sided join out of it, the two-sided join between chapters, dip into the outro), a frame of the intro's title, and two of the end card. Each preview frame is compared with the export's frame averaged down to 1920x1080.

| | Mean difference per channel (0 to 255) |
| --- | --- |
| preview against the export's same frame, 46 samples | 0.63 to 3.05 |
| the same preview frame against the export's frame 8 later, inside the windows | 3.9 to 41 |

The test asserts under 4.0. **Passes** (174 s, both configurations). The remaining difference is the export's HEVC encoding at 4K and rasterizing text at another size.

## Editor bench

`shot-app --bench-edit <copy> --seconds 120`, 5 runs, each a 120 s stretch spread over the cut plus 60 seeks:

| Configuration | Result |
| --- | --- |
| dissolves, the project's effects | **PASS**: 1 dropped of 35,995 frames (run 5, load 7.7), 0 wrong seek frames, seek p50 14.0 ms, p95 57.7 ms, max 608.5 ms (the cold first seek), peak footprint 371 MB, app CPU 5 to 6.5% of one core |
| mask wipes, `--glass` | first run **FAIL**: 9 dropped (5 in run 1 starting at load 12.5, 4 in run 4); rerun once: **PASS**, 1 dropped at 746.2 s, nowhere near a transition (load 10.1), 0 wrong seek frames, seek p50 15.2 ms, p95 31.8 ms, max 134.1 ms, peak footprint 420 MB, app CPU 14 to 15.5% |

Every run played through at least one two-sided transition (run 1 also the dip into the intro, the intro and the mask wipe or dissolve out of it at 90.6 s; the others the joins at 219.5, 331.5, 594.7 and 712.8 s). No drop in either rerun or the dissolve run fell inside a transition window. The first glass run did not print drop positions yet, so where its drops fell is unknown.

## Chapter times

The mask-wipe configuration exported with `shot export` (1903 chunks, 56,814 frames in 873.5 s, 65 fps, while another agent exported at the same time), its markers read with `ffprobe -show_chapters`, against `shot describe` on the same folder:

| Chapter | Export marker | `shot describe` |
| --- | --- | --- |
| Intro | 0.0 s | 0:00 |
| Install and setup (opened by the intro) | 87.6 s | 1:27 |
| Project structure | 219.5 s | 3:39 |
| Recording a take | 331.5 s | 5:31 |
| Auto cut | 471.5 s | 7:51 |
| Review and notes | 594.7 s | 9:54 |
| Export | 712.8 s | 11:53 |
| Wrap up | 853.2 s | 14:13 |

Seven of eight agree to the second as YouTube reads them (rounded down). "Export" is marked at its first word, heard 0.2 s or more after the chapter's start, which crosses into the next second; describe keeps marking chapters at their first word by design. Without the bookends every chapter after the hook was 3.0 s early (the intro's length), "Install and setup" at the first word after the intro rather than at the intro.

## Screenshots

1600x1000 in dark, taken with a temporary start-up seek (removed before the commit), since synthetic clicks do not reach the window on the shared desktop:

- `painter-picture-wipe.png`: Picture at 03:39.36, inside the mask wipe into "Project structure": the lime-edged circle opening from the middle with the incoming face inside it, framed by the circle camera's border.
- `painter-timeline-wipe.png`: Timeline at the same frame: the "Mask wipe" mark on the edge at 03:39.5, the transport reading 15:46 with the bookends counted.
- `painter-picture-intro.png`: Picture at 01:28.83, inside the intro: the bumper with the video's title in kinetic type.
- `painter-picture-endcard.png`: Picture at 15:41.66: the end card over the outro.

Against the mockup: the mockup has no bookends or transitions, so the Intro and Outro blocks in the Layout lane (accent block style, like the lane's other blocks) are my choice. The Picture caption line under the preview shows the Text workspace's chapter, not the playhead's, so after the start-up seek it shows chapter 1's last line in every shot; a real seek through the screen moves the chapter too. The preview has no marker for "in the intro"; the Picture pane's "On screen now" is empty there.

## Checks

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass. `cargo test --workspace` passes (43 test binaries). On the way, the app's stall watchdog test (`metrics`) failed with this change: `dladdr` scans the binary's symbols, and with export linked into the app each sample took about 6 ms, so the watchdog took 6 samples in 42 ms instead of 10 or more. It now records return addresses while sampling and names them when the samples are read. New tests: `model` (an intro after the hook moves what follows and opens its chapter), `spoken` (bookends move marks and an intro opens its chapter), `place` (an imported Lottie declares its slots), and the ignored golden test above.

## Not verified

- **A real prefetch** of the incoming side: the deeper queue covers the 82 ms first frame in these runs, but the decoder still seeks at the window's first frame.
- **Callouts on the painter's layout**: the click map comes from a second layout of the paused frame, not from what the painter drew. They agree by construction (same `compose` inputs) but were not compared.
- **Sound**: nothing was listened to. The preview has the voice of `of_plan` but not export's bumper sound, brand transition sounds or music ducking under the bumpers (a known gap).
- Real clicks, keys and playback in the app window; the screenshots come from a start-up seek.
- A stinger or zoom-through in the app: covered by the painter's own tests and the transitions task's stinger bench, not run here.

## Needs from other tasks

- **shot-render**: (1) `Painter::prefetch(&plan, frame)` (or a lookahead in `paint`) that opens the incoming side's cursor at its handle before a two-sided window, so the engine's deeper queue can go; (2) `paint` returning the screen layer it drew (frame, crop, recording size), so the engine stops laying a paused frame out twice; (3) optionally a decoration hook, though the bench's own pass works.
- **shot-ship `check_cut`** reads chapter marks and the length without bookends, and the Ship screen's cut length too: pass `lock::bookends` there (not done: outside the describe path).
