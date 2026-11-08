# Milestone 11, work items 7, 8 and 9: transitions, intro and outro, camera styling

Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while other milestone agents were building on the same machine (load average 3 to 9). Frames are in `docs/evidence/m11/` (git-ignored).

## What was built

All in `crates/shot-render`, plus `RENDERER_VERSION` 5 in shot-core (the compose shader changed) and `resvg` 0.46 in the workspace (the version gpui already builds) for SVG logos.

- **Joins** (`join.rs`, pure): `ExportPlan::with_joins(&Joins)` places the brand's intro and outro and resolves the lock's `transition` instances to output windows. The plan now lists its `pieces` (segments and bookends in output order), `bookends`, `transitions` and the `end_card` range. A `chapter-start` transition joins its chapter to the piece before it, which may be the intro; the `video-start` transition joins the bookends (into the intro, out of it where the next chapter has none, into the outro). Dissolve, dip, mask wipe and zoom-through are centered on their edge, half their length on each side, shortened to the pieces they join. A stinger starts its peak before the edge, so the cut is under its peak, and plays its Lottie's length. A cut draws nothing.
- **Handles**: a dissolve and a mask wipe show both sides. The outgoing take runs on past its kept range and the incoming one starts early, like an editor's center-on-cut, holding the media's first frame where a take has nothing earlier.
- **Chunks** stay inside one piece, so every chunk is still one encoder session starting on a keyframe and joins by stream copy as before. `join::key_joins` extends the key of each chunk a window overlaps: the kind, window and edge relative to the chunk, the brand fields it draws (stinger hash, background, accent and wipe shape), and for two-sided kinds the key of the other side over exactly the frames it shows, computed like a chunk key over the handle frames. A bookend's chunks are keyed by what it draws (bumper file identity, title, theme, logo, logo reveal) and their frames in it, never by the video around it. Chunks under the end card add its length and their place in it.
- **Painter** (`paint.rs`): one `Painter::paint(&Renderer, &ExportPlan, frame, &target)` draws any output frame for export and preview: segments (the old export loop, cutaways included), bookends, the end card, and transitions. It keeps two decoder cursors per file and picks the one that decodes on, so both sides of a transition in one take decode forward without seeking back and forth. Export's worker now renders every chunk through it; jobs and the join run over pieces.
- **Renderer**: layers have an opacity, a border inside the edge and a drop shadow (an analytic blurred rounded rectangle) drawn in the compose shader over a quad grown to hold the shadow, and `Renderer::draw_over` draws a frame onto what a target holds. Dissolve and mask wipe draw the incoming side into a scratch buffer of the target's format and composite it as one layer.
- **Bookends** (`bookend.rs`): a bumper (video, decoded and covering the frame, or a Lottie), a generated title (the brand's `logo-reveal` Lottie when it declares one, otherwise the logo popping in on the preset's springs, then the title in the preset's kinetic type and an accent rule), both, or none. The intro shows the video's title, the outro only the logo. The intro plays after the hook in opinion and reaction, before the video in product tour and tutorial. A title without a bumper plays 3 s. The outro is stretched to the end card's length. Chapter markers start at the intro that opens a chapter.
- **End card**: drawn over the last `end-card length` of the output, in a pass of its own over the frame so the brand's logo sits on its backdrop above the headline. Logos are SVG (rasterized once by resvg) or PNG.
- **Camera** (`LayoutSpec::with_camera`): the brand's shape, corners, border (in the accent) and shadow (falling away from the scene's light) on the camera in `face-content` and `split`. A circle in a corner over the screen keeps to that corner, a panel beside it is centered. `face` fills the canvas and has nothing to frame. The camera's frame enters the key of those layouts' chunks only.
- **Audio** (`TimelineAudio::of_plan`): the voice placed by the plan, silent under bookends. A dip fades out to the cut and in after it; every other transition crossfades on equal-power curves over its middle with the takes' handles, a stinger over a quarter second around its cut. Joins inside one take's continuous material are left alone. The brand's `transition` sounds play per phase (bare at the window's start, `-travel` and `-impact` peaking on the cut, `-settle` at the window's end). A video bumper's own sound plays as a cue, and the music ducks under bumper ranges.
- **For the lanes task**: `lottie::slots(bytes) -> Result<Vec<(String, SlotKind)>, LottieError>` lists a file's slots by id, refusing a file velato cannot render.

## Frames per transition and preset

`join_reel::frames_per_transition_per_preset` writes `transitions-<preset>.png`: one row per transition of a 6-chapter reel over the fixture take (intro after the hook, outro with a 2 s end card), at a quarter of the window, the frame before the cut, the cut and three quarters; the last row is the intro's title and the outro's end card. `transitions-<row>-<col>.png` are the editorial frames at 1920x1080, `camera-<shape>-<layout>.png` the camera frames.

| Transition | What the frames show |
| --- | --- |
| dip (hook into the intro) | the hook with its framed camera inset, then the brand background, opaque on both sides of the cut, then the bumper with the logo popping in |
| dissolve (intro into a split chapter) | the bumper's face with the split layout ghosting in, about half and half at the cut |
| mask wipe | a rounded rectangle in the camera's shape opening from the middle, edged in the lime accent, the incoming side inside it |
| zoom-through | the outgoing screen punched in and smeared towards the cut, the incoming face punched in and settling out, motion blurred both ways |
| dip | face, background, background, face |
| stinger | the accent card covering the cut with the dark disc growing on it, then fading off over the incoming side |
| into the outro (dip) | the last chapter dipping to the background, the end card's backdrop coming up |

The presets differ only in the bookend title and the end card (kinetic type and springs); the transitions themselves draw the same in every preset. These frames are for Mike to judge; nothing here is scored.

## Golden frames

`join_reel::transitions_bookends_and_the_camera_frame_the_same_reached_by_playing_or_by_seeking`: the reel (7 transitions, intro with bumper and title, outro with end card, circle camera with border and shadow, 1,035 frames) played through one painter at 1280x720, then 80 seeks, every other one into a transition window, alternating between the playing painter and renderer and a fresh pair. **Passes, 8 runs in a row.**

It and the templates' golden test found one defect of mine on the way, fixed: the compose shader first derived each vertex's texture position from its interpolated canvas position, which is not exactly the frame's corner. The templates' test then failed at the same launch frame every run (frame 351: a layer sampled a hair off, and differently once a sheet texture was reused), and this test failed once in six. Texture positions now come from the quad's corners, exactly 0 and 1 where the quad is the frame, so a layer without a shadow draws bit for bit as before; both tests pass.

## Export

A copy of the target project as `videos/target` of a scratch channel repo (media symlinked), analysed and migrated with `shot migrate`, then a brand with an intro (3 s bumper with a tone, plus the generated title), an outro (4 s bumper with a tone, stretched to a 12 s end card), the circle camera, a stinger, the logo, `on video-start { transition dip; music ... }` and `on chapter-start { transition dissolve }`, built with `shot style build`: 8 chapters, the intro after the hook, a dissolve at every chapter start, dips into and out of the bookends.

| Export | Result |
| --- | --- |
| fresh cache | 1915 chunks, 57,174 frames (952.9 s) in 588.6 s render, 1.6 s join: **97 fps, 1.61x realtime** |
| after moving one chapter's start by 0.5 s | **3 of 1914 chunks** rendered (72 frames, 25.6 s): the previous chapter's last chunk, which shows the moved side's handle, and the two chunks of the new start under the window |
| loudness of the first export (ffmpeg `ebur128`) | **-14.0 LUFS integrated, -1.3 dBTP**, with bumper sound, transition sounds (`swoosh`, `impact`) and a music cue ducked under the bumpers |
| chapter markers | 0.0, 87.6 (the intro, opening chapter 2), 219.5, 331.5, 471.5, 594.7, 712.8, 853.2 |

## Preview

- `shot-app --bench-edit <unmigrated copy> --glass` with the app as it is on this branch (the engine does not call the painter yet): **PASS**, 0 dropped of 17,995 frames, 0 wrong seek frames, seek p50 14.0 ms, p95 31.4 ms, max 523.7 ms (the cold first seek), peak footprint 340 MB, load 3 to 8.
- The same bench with a **temporary, uncommitted patch** that routes the engine through `Painter` and `preview_plan`, on the migrated copy with `on chapter-start { transition stinger }`, the intro, outro and circle camera, `--seconds 120` so every run crosses a stinger (run 1 also the dip into the intro and the intro itself): **PASS**, 0 dropped of 35,995 frames (1 lost to a late bench tick), 0 wrong seek frames, seek p50 12.7 ms, p95 28.1 ms, max 96.4 ms, peak footprint 341 MB, app CPU about 5% of one core. `--glass` has no hook in the painter, so this run drew the project's own effects.
- `join_reel::preview_frames_across_transitions_fit_the_frame_budget`, decode, compose and draw at 1920x1080 per window frame, playing in from a second before:

| Transition | p50 | max |
| --- | --- | --- |
| dip into the intro (opens the bumper's decoder) | 3.4 ms | 27.3 ms |
| dissolve | 5.3 ms | 82.2 ms |
| mask wipe | 5.3 ms | 42.3 ms |
| zoom-through | 3.1 ms | 9.7 ms |
| dip | 3.0 ms | 7.3 ms |
| stinger | 3.5 ms | 13.8 ms |

The two-sided maximum is the window's first frame, where the incoming side's cursor seeks to its handle: one frame of 82 ms against the engine's 6 frames (100 ms) of lead. It held in this test but was not measured in the app; prefetching the other side a second ahead would remove it.

## Not verified

- **The preview in the app.** The Edit screen's engine still draws through its own `render` and an `EditMap` without bookends, so transitions, bookends and the end card do not show in the app. The stinger bench ran a temporary patch (not committed) that routes the engine through the painter. See "Needs from other tasks".
- **A dissolve or mask wipe in the app's preview**: the stinger bench covers one-sided transitions only.
- The two exports ran before the shader fix above, which moves layer sampling by a hair; render cost is the same.
- **A video stinger.** Decision 0023 makes the stinger a Lottie and rejects alpha video; `transition stinger=` takes a Lottie only.
- **Nothing was listened to.** The crossfades, dips, bumper tones and transition sounds were measured, not heard.
- Lottie bumpers, the `logo-reveal` template and PNG logos are covered by code paths but not by a run; the reel used a video bumper and an SVG logo.
- Bumpers are cached per video folder; decision 0021's "rendered once per file and reused by every video" would need a channel-level chunk cache. The keys already allow it.

## Needs from other tasks

- **App (preview)**: keep an `ExportPlan` from `shot_render::export::preview_plan(dir, &folder.project)` next to the `EditMap`, and in `engine.rs` draw each frame with one `shot_render::paint::Painter` (`Painter::new(video_dir, effects)` once per plan, then `painter.paint(&renderer, &plan, frame, &target)`), replacing `render`. The `EditMap` must then count bookend frames for the playhead, timecodes and sound; `TimelineAudio::of_plan(&plan, track)` gives the voice in the same output frames. The bench's `--glass` decoration has no hook in the painter yet.
- **Ship**: `shot describe` places chapters from the timeline without bookends, so with an intro after the hook every later chapter is early by the intro's length. It should read chapter starts from `preview_plan`'s `chapters`.
- **Contract**: the bookend rules above (which transition joins a bookend, where the intro goes per format, what each title shows, the end card's own pass, handles) are not in `docs/contracts/project-files.md` yet; the chunk-hash paragraph already names "both sides of the transition" and the camera style.
- **Derived cuts (item 12)**: derived plans are built without joins, so Shorts skip bookends, stingers and the end card and keep hard joins; I did not run a Short.
