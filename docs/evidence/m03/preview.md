# M03 evidence: preview colors, dropped frames, seek and scrub

Measured on 2026-11-07 on the reference MacBook Pro (M1 Pro, 32 GB, macOS 27, on AC, lid closed) driving two Dell U2515H at 2560x1440, nominal 60 Hz, measured 59.95 Hz. Other agents were building and running hardware encodes the whole time: the one minute load average was 4 to 22 during the runs below. Fixture: `$SHOT_FIXTURES/screen-30s.mp4` (3840x2160 HEVC, 60 fps, BT.709 limited range, 0.5 s GOP, 60 keyframes). Work item 7 of [milestone 03](../../milestones/03-opinion-video.md), following the [spike](../m02-preview-spike.md).

## Bench command

```sh
export CARGO_TARGET_DIR=$HOME/.cache/shot-studio-target SHOT_FIXTURES=$HOME/.cache/shot-studio-fixtures
cargo build --release -p shot-app && $CARGO_TARGET_DIR/release/shot-app --bench $SHOT_FIXTURES/screen-30s.mp4
```

Five runs of 60 s playback plus 60 random seeks each, in one window, about 6.5 minutes. It prints one block per run and ends with a `PASS:` or `FAIL:` line; the exit code is 0 or 1. A run passes when no frame is dropped, late or missing, every seek shows its exact frame and seek p95 is under 100 ms. Flags are documented in `crates/shot-app/src/main.rs` (`--runs`, `--seconds`, `--seeks`, `--zoom`, `--wall-clock`).

## Results

| Criterion | Measured | Result |
| --- | --- | --- |
| 60 s of 4K60 playback drops no frames in five runs in a row | Final bench: 4 of 5 runs drop nothing, run 1 drops 1 frame at a 33.4 ms render gap during load 10.7. The bench before it: 5 of 5 clean at load 4 to 22. A 10 minute run: 2 drops | Fail on this loaded machine; rerun idle |
| Seek p95 under 100 ms | 300 seeks per bench. Final bench p50 33.6 ms, p95 66.7 ms, max 66.9 ms, worst run p95 66.7 ms. Every seek showed its exact frame. One earlier bench had one run at p95 100.1 ms while the load average was 22 | Pass |
| BT.709 colors | Screenshot of eight bars within 2 of the BT.709 values per channel, was off by up to 33 | Pass |
| Mouse scrub | 15 to 23 frames per second during a drag, screen behind the mouse p50 33 to 50 ms. The spike's code showed 1 to 3 frames per second | Works, see below |

### Final bench

`shot-app --bench $SHOT_FIXTURES/screen-30s.mp4`, 18:22 to 18:28, load average 8.8 at start and 1.8 at the end:

| Run | Presented | Dropped | Render interval p99 / max | Main thread CPU per frame p99 / max | Seek p50 / p95 / max |
| --- | --- | --- | --- | --- | --- |
| 1 | 3599 / 3600 | 1 | 17.0 / 33.4 ms | 2.2 / 2.6 ms | 49.9 / 50.2 / 66.8 ms |
| 2 | 3600 / 3600 | 0 | 17.4 / 22.2 ms | 2.3 / 3.0 ms | 49.9 / 66.7 / 66.8 ms |
| 3 | 3600 / 3600 | 0 | 17.3 / 24.3 ms | 2.3 / 2.5 ms | 33.4 / 66.7 / 66.9 ms |
| 4 | 3600 / 3600 | 0 | 17.1 / 24.2 ms | 2.3 / 3.2 ms | 33.4 / 66.7 / 66.8 ms |
| 5 | 3600 / 3600 | 0 | 17.1 / 21.3 ms | 2.3 / 2.7 ms | 49.9 / 66.7 / 66.8 ms |

No run had late frames or underruns. Clock rate 0.99918 (locked to the 59.95 Hz display). App CPU 11.1 to 11.4 % of a core, `VTDecoderXPCService` 5.5 to 5.7 %. Peak footprint 68 MB, peak resident 113 MB. Seeks by frames decoded past the keyframe: 0 to 9 mean 27.6 ms, 10 to 19 mean 43.1 ms, 20 to 29 mean 55.2 ms.

The one gap, as the bench printed it:

```
gap at 6.12 s: 33.4 ms, main thread cpu 0.8 ms, load 10.7, 4 samples: 3 idle, 0 blocked, 1 running. Main thread idle in its run loop, the display link called late
```

## Dropped frames

### How the bench finds the cause

Every render interval over 25 ms is printed with what the main thread did during it:

- **Main thread CPU** in the interval, from `thread_info`. Our render plus gpui's layout, paint and Metal submit take 0.7 ms per frame at p50 and 2.3 ms at p99. `Preview::render` itself takes 0.6 ms at p99.
- **Watchdog samples**: a thread polls every millisecond. Once no render happened for 18 ms, it suspends the main thread for a few microseconds, reads its scheduler state and walks its frame pointers, then names the functions with `dladdr`. A gap is classed as main thread idle in its run loop (the display link did not call), busy in a function, blocked in a function, or runnable but not on a core.

### What it found

| Cause | In our code? | Fix |
| --- | --- | --- |
| The display link calls a refresh late while the main thread sits idle in its run loop | No. The window server or GPU did not hand out the next drawable in time. Seen twice, at load averages 4.7 and 10.7 from other agents | None in our code, see recommendations |
| The main thread stalled 24 ms without using CPU, seen once before the fixes below (spike code with this bench) | Possibly: the decode thread ran at default QoS and could be held off a core while it held the player lock the main thread takes every render | Decode thread at user-interactive QoS |
| gpui caps the frame rate at 16.667 ms under serious thermal pressure, and with its 1 ms tolerance defers a render that arrives 1 ms early, which costs a frame. Latent: `pmset -g therm` recorded no thermal warning today | Yes, a gpui setting we left at its default | `thermal_frame_interval: None` and `low_power_frame_interval: None` on the preview window |
| Each seek leaked its `AVAssetReader` and buffers: the decode thread has no autorelease pool. Peak footprint was 217 MB after 60 seeks and 904 MB after 300 | Yes. Not a frame drop cause, found by the bench | `Decoder` drains a pool around `open`, `next_frame` and `seek`. Peak footprint is now 68 MB after 300 seeks. The spike's 228 to 276 MB were mostly this leak |

Allocation and lock checks: the render path allocates only the status line and gpui's element tree, and the player lock is held for microseconds by either side. Main thread CPU per frame stays under 3.2 ms in every run, so render work is far from the 16.7 ms budget.

The thermal cap and the QoS change are reasoned fixes: the gaps are too rare (about one per 5 minutes) to show a before and after difference in the time available. After the changes, the final bench had one drop, a late display link call with an idle main thread. A 10 minute run before the watchdog moved to user-interactive QoS had 2 drops: one at a 25.1 ms interval the watchdog did not sample (its timer ran late too, so likely machine load), one without any interval over 25 ms.

### Tried and dropped

Snapping each render's time to the refresh grid, so a render called late still picks the frame of its refresh. It changed nothing: `FramePacer` already puts frame boundaries half a refresh from each render, which is the same tolerance. A unit test with renders 7 ms late passed with and without it.

## Colors

gpui-pre 0.3.11's `surface_fragment` (`gpui-pre-apple/src/shaders.metal`) decodes full-range YCbCr with the BT.601 matrix (`R = Y + 1.402 (Cr - 0.5)`, `G = Y - 0.3441 (Cb - 0.5) - 0.7141 (Cr - 0.5)`, `B = Y + 1.772 (Cb - 0.5)`) and accepts only `420f`. Our sources are BT.709 limited range.

The decoder already outputs `420v`, which VideoToolbox converts to `420f` with the hardware pixel transfer. `Decoder::open_with(path, Matrix::Bt601)` sets `kVTDecompressionPropertyKey_PixelTransferProperties` to `{DestinationYCbCrMatrix: ITU_R_601_4}`, so the same transfer also re-encodes the colors into BT.601. gpui's shader then yields the BT.709 RGB. Frames are tagged `ITU_R_601_4`, stay IOSurface-backed `420f`, and the decoder service CPU stayed at 5 to 6 % of a core (7 % in the spike), so the conversion runs on the scaler hardware, not the CPU. `Decoder::open` keeps the source matrix, so export sees unchanged frames. `Player` always uses `Matrix::Bt601`.

Check: a 3840x2160 HEVC clip of eight bars (75 % white, yellow, cyan, green, magenta, red, blue, 25 % gray) encoded as BT.709 limited range. The expected RGB is the BT.709 decode of the YCbCr values ffmpeg reads back from the file. The screenshot (`screencapture -l <window>`, gpui's layer has no color space so values pass through) was read at the center of each bar:

| Bar | Expected BT.709 | Before | After |
| --- | --- | --- | --- |
| White 75 % | 191 191 191 | 192 190 192 | 192 190 192 |
| Yellow | 191 191 0 | 190 203 8 | 192 190 0 |
| Cyan | 0 191 190 | 16 210 190 | 0 191 192 |
| Green | 0 190 1 | 15 223 8 | 1 190 1 |
| Magenta | 191 0 192 | 178 0 187 | 192 0 193 |
| Red | 191 0 1 | 176 0 3 | 192 0 1 |
| Blue | 0 0 191 | 2 0 185 | 0 0 193 |
| Gray 25 % | 64 64 64 | 65 63 65 | 65 63 65 |
| Worst channel | | 33 | 2 |

The 2x zoom pass keeps the colors: the zoomed bars read within 2 of their values too. Screenshots, git-ignored: `docs/evidence/m03/preview-colors.png` and `preview-colors-before.png`.

## Seek

Seeks decode from the keyframe before the target and show the exact frame. With the 0.5 s GOP at most 29 frames lie past a keyframe, and latency grows about 1.3 ms per frame on top of roughly 25 ms of fixed cost. Measured values are multiples of a refresh because a finished seek waits for the next render.

Because p95 is 66.7 ms with margin, the bench seeks keep showing only the exact frame. Showing a nearer frame first is used for scrubbing only, below. The one run over 100 ms (p95 100.1 ms, max 233.6 ms) ran at load average 22 while other agents ran builds and an export.

## Mouse scrub

The window now has a timeline strip under the video with the playhead. A press on it starts a scrub, and the drag continues anywhere in the window. Driven by a CGEvent helper (scratch, deleted) posting real mouse down, drag and up events at 16 ms steps, release build, load average 1.8:

| Drag | Frames shown | Screen behind the mouse p50 / p95 / max |
| --- | --- | --- |
| 1520 px right in 2 s (fast) | 53, 23 per second | 50.0 / 66.8 / 92.3 ms |
| 1520 px left in 1 s (fast, backwards) | 25, 21 per second | 50.0 / 83.4 / 133.4 ms |
| 200 px in 2 s | 54, 23 per second | 33.4 / 100.1 / 133.4 ms |
| 30 px in 2 s (frame by frame) | 35, 15 per second | 36.3 / 66.1 / 66.2 ms |

"Screen behind the mouse" runs from the first seek the screen has not caught up with to the next frame on screen. Each drag ends on the exact frame under the mouse. Screenshot after the last drag, git-ignored: `docs/evidence/m03/preview-scrubbed.png`.

With the spike's code the same drags showed 1 to 3 frames per second, with waits up to 1.9 s: every mouse move cancelled the seek still decoding and dropped its frame, so a moving mouse starved the screen. Now:

- A running seek finishes, and its frame stands in while the newest seek decodes (`Player::take_next` says which frame is the latest).
- When the mouse enters another keyframe interval while the decoder is busy, the scrub seeks to that keyframe first, which decodes nothing past it, and to the exact frame once the decoder catches up.
- A move that lands on the frame already requested starts no seek.

## Changes

- `crates/shot-media/src/videotoolbox.rs`: `Matrix`, `Decoder::open_with`, `Frame::ycbcr_matrix`, `DecodeError::PixelTransfer`, autorelease pools. `Decoder::open` and the rest of the API are unchanged.
- `crates/shot-media/src/player.rs`: BT.601 output, seeks run to the end and stand in, `take_next` returns `(Frame, bool)`, `seeking()`, decode thread at user-interactive QoS.
- `crates/shot-app`: bench runs and verdict, render timing and the stall watchdog (`metrics.rs`), gpui throttles off, timeline strip and scrub logic.
- Tests: `crates/shot-media/tests/videotoolbox.rs` (ignored, hardware: frame matrix tags), `metrics.rs` (watchdog names a sleeping and a spinning thread, symbol demangling).

## Not verified

- **Idle machine**: every number here ran next to other agents' builds and hardware encodes. The acceptance run belongs on an idle machine.
- **Why the display link calls late**: the watchdog proves the main thread was idle, not what held the drawable. Metal System Trace during a gap would show it; not recorded.
- **Presented frames**: counted from frame timestamps at render time. gpui does not expose drawable present times.
- **Built-in 120 Hz display**: the lid was closed. There a missed refresh costs no frame.
- **The QoS and thermal cap fixes** are not shown to remove a gap, see above.
- **Scrub by hand**: driven by synthetic CGEvents, not a person.

## Recommendations

- Rerun the bench on an idle machine. If late display link calls remain, record Metal System Trace during a run.
- If they remain on an idle machine, a dedicated `CAMetalLayer` for the video presented with `presentAtTime` from the decode side takes the main thread and gpui's frame callback off the video path.
- Scrub could continue a running decode forward instead of restarting at the keyframe when the mouse moves ahead inside one keyframe interval, and cache recent frames for backward drags.
