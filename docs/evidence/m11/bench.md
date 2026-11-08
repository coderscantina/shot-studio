# Milestone 11: preview bench on the editor engine

Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while two other milestone agents were building, testing and rendering on the same machine. Load average during the runs was 4 to 19 (the machine has 10 cores). Treat every number as pessimistic.

## What was built

`shot-app --bench-edit <video-folder> [--glass] [--runs N] [--seconds N] [--seeks N]`, in `crates/shot-app/src/edit_bench.rs`. `--bench` is unchanged and still benches the milestone 02 spike player.

- It opens the folder like the Edit screen (`VideoFolder::open`, `EditMap::of(project, View::Edited)`, `Plan { map, media, effects }`) and drives the Edit screen's own `Engine`: the render thread that calls `compose` and `Renderer::draw` through `shot-render` at 1920x1080 with real decode. It touches only what the Edit screen calls (`Engine::{spawn, set_plan, seek, set_playing, take_due, take_next, front, error}`), so it keeps working when `shot-render` moves to `cut.lock`.
- It runs headless. A 60 Hz ticker takes frames the way `EditScreen::advance` does, without sound and without the display, so the gpui hand-off and the display link are not measured. Each run plays a different stretch of the edit (run N starts at N/runs of the timeline), then does random seeks.
- Rule, the same as `--bench`: dropped frames are judged over all played time, one per 5 minutes; seek p95 under 100 ms; every seek must show its exact frame. Memory is the peak footprint and resident size.
- A dropped frame is a frame number the ticker skipped because a newer one was due. Frames skipped on a tick that came over 25 ms after the previous one are counted apart ("lost to bench ticks"), because the bench thread was not scheduled, not the engine. A late frame is shown after its due tick. A starved tick found the queue empty.
- Seek latency runs from `Engine::seek` to the frame being ready in the queue (the display hand-off is excluded; `--bench` includes it).
- `--glass` draws a launch preset reel over every frame through a new `Engine::set_decoration` hook (nothing else sets it): a glass card with a soft shadow, tilt and parallax, a kinetic headline, a motion blurred badge and word pop captions, 6 s of every 12 s. It uses the same calls as `reel::decorate` in shot-render, which is test-only. Overlay templates do not use the motion presets yet (item 5), so this is the only way to put glass and motion blur through the engine today.

## Runs

Both on a temp copy of `$SHOT_FIXTURES/target-project` (the media is a symlink, the fixture is not written): 15.6 min edit, 56,274 frames, 5 runs of 60 s playback and 60 seeks each.

**Plain copy** (1 overlay, 1 zoom, as the fixture has it). Load average 5 to 14.

| | |
| --- | --- |
| dropped | 0 of 17,992 frames played (limit 1), 0 late, 0 starved |
| seeks | 300: p50 13.2 ms, p95 36.2 ms, max 508.7 ms (the first seek of run 1) |
| memory | footprint 292 MB, resident 102 MB |
| cpu | app 5 to 8% of one core, VTDecoderXPCService 2 to 3% |
| verdict | **PASS** |

**Heavy copy**: `zoom-motion smooth`, 34 zooms, 18 lower-third overlays, plus `--glass`. Load average 9 to 18.

| | first run | rerun |
| --- | --- | --- |
| dropped | 29 (all in run 1, with 54 late, 27 starved) | 5 (4 in run 4, 1 in run 3) |
| seeks | p50 15.9 ms, p95 76.8 ms, max 674 ms | p50 14.9 ms, p95 62.1 ms, max 373 ms |
| worst run p95 | 127 ms (run 3) | 132 ms (run 3) |
| memory | footprint 330 MB, resident 117 MB | footprint 332 MB, resident 106 MB |
| cpu | app 12 to 16%, VTDecoderXPCService 2 to 5% | app 11 to 16% |
| verdict | **FAIL** | **FAIL** |

## Reading the heavy result

- The engine keeps up on most runs: 6 of 10 heavy runs played 60 s with no drop at all, with seek p50 at 14 ms. The drops come in bursts (run 1: 27 starved ticks, run 4: 4), and the slow seeks cluster in run 3, where the load average was 12 to 18 in both reruns. That pattern is what machine contention looks like, but I cannot prove it from these runs: I did not get a quiet machine, so the heavy set neither passes nor fails with confidence.
- The plain set passed with the same load, so the effect set costs real headroom: app CPU roughly doubles (5% to 12 to 16%) and the render thread has to finish a frame in 16.7 ms with six frames of buffer, which a burst of load can use up. The earlier item 4 timing (launch p99 5.3 ms for compose and draw alone) leaves room on an idle machine.
- The worst seek is the first one after playback (cold decoder position after a jump): 509 ms and 674 ms in two first runs. One of 300, so it does not move p95, but it is a visible hitch on the first click.

## Not verified

- A run on a quiet machine. The heavy set should be repeated when no one else is building or rendering; `shot-app --bench-edit <copy> --glass` is the whole command.
- The display hand-off, gpui's paint and sound-clock pacing (the Edit screen follows the audio device clock). `--bench` covers the first two for the spike player only.
- Glass on the real overlay templates: they do not draw it before item 5.
- A ticker artifact I found on the way: spinning the last 1.5 ms before each tick (to cut sleep jitter) made things worse, with 3 to 5% of frames dropped, probably because macOS demotes a spinning thread. The shipped ticker sleeps.

## Needs from other tasks

When `shot-render` moves onto `cut.lock`, `compose`/`Shot`/`Look` may change shape; `Engine::render` in `engine.rs` is the one place calling them, and `launch_reel` in `edit_bench.rs` needs `Frame.drawing` with `Sheet`, `SoftShadow` and `Shape::{Kinetic, Caption, Label}` to stay public.
