# M03 evidence: preview sound and preview colors

Checked on 2026-11-07 on the reference MacBook Pro (M1 Pro, 32 GB, macOS 27), release build, lid closed, two Dell U2515H at 59.95 Hz, with other agents building on the machine. Branch `task/m03-audio`. Closes three items of "Open before closing": preview colors, the silent preview and the frame cursor the UI asked for.

## What changed

- **Colors**: the render graph reads each buffer's YCbCr matrix tag (`surface::Matrix`, BT.709 when untagged) and decodes and encodes with it. `BufferPool::new` takes a matrix and tags its buffers. The Edit preview renders into `420f` BT.601 buffers, the encoding gpui's surface shader decodes, and now decodes with `Decoder::open_native` like export, so both draw from the same frames.
- **Frame cursor**: `shot_render::cursor::FrameCursor` (`goto(time)`) replaces export's cursor and the app's copy of it. Zoom lookup is one function, `plan::zoom_at`, used by `PlannedSegment` and the app's `Span`.
- **Timeline audio**: `shot_render::audio::TimelineAudio` reads any range of an output's mic audio, cut and faded over 5 ms where the source jumps. Export encodes it, the preview plays it, so both cut the same samples. A fade read in pieces equals the fade read at once (unit test).
- **Audio output**: `shot_media::speaker::Speaker` plays mono 48 kHz on the default output through CoreAudio's default output unit (both channels), fed from a lock-free single-producer ring. Each render callback stamps the host time its buffer reaches the device. `SpeakerClock::audible(ahead)` returns the sample heard at now plus `ahead`, adding the device's reported latency (unit, device, safety offset and stream latency: 17.4 ms on this machine). When the ring runs dry the clock holds.
- **Playback**: `shot-app/src/sound.rs` feeds the speaker from a thread, 200 ms ahead. Sound is the master clock: while it plays, the Edit screen shows the frame of the sound heard one refresh from now. A seek, Edited/Untrimmed or an edit stops the device, drops the queue and restarts at the new frame with a 5 ms fade-in; the picture holds until the device plays again. Sound starts together with the first queued frame. Without an output device or with an unreadable mic the screen paces by wall clock as before and shows "no sound: ..." in the error strip.
- **Frame picking against the sound**: `shot_media::timing::ClockFollower`. Picking frames straight from the audio clock skipped 40 frames in 51 s of playback, in clusters whenever the 59.95 Hz display's phase drifted onto a frame boundary. The follower shifts the frame grid by up to 0.6 frames so boundaries stay away from renders, and wraps once per frame the display rate owes. A 10 minute simulation at 59.95 Hz with 2 ms jitter skips 29 frames (30 are owed) and repeats none; picking straight from the clock skips over 100.

## Colors

The same frame (take t01 at 100 s) in each layout, exported with `shot export` and shown paused in the Picture workspace, screenshot with `screencapture -l`. Both images were reduced to 8x8 pixel blocks; the export's reference is its YCbCr through the exact BT.709 limited range formula. Flat blocks (all four neighbours within 3 in the reference) remove registration error at edges.

| Layout | Flat blocks | Max channel difference p50 / p95 / max |
| --- | --- | --- |
| Face | 4020 | 1 / 2 / 3 |
| Face + content | 5071 | 1 / 2 / 4 |
| Split | 5305 | 1 / 2 / 4 |
| Content | 5475 | 1 / 1 / 3 |

The block maxima of 3 and 4 are in the camera's noisy blue background. A flat 30x30 px corner of it reads (58, 73, 106) in the preview, and the exact BT.709 decode of the source's and the export's YCbCr there (78, 144, 120 in both) is (58, 73, 106). The background fill reads (16, 16, 21) in the preview and (15, 16, 21) in the export.

Note for later checks: ffmpeg's default RGB conversion (swscale) is itself off by up to 5 per channel on these frames, so it cannot serve as the reference.

## A/V sync

`shot-app` on a copy of `$SHOT_FIXTURES/target-project` (media linked, default layout set to Content), driven by synthetic key and mouse events: play from the start, then a seek on the overview bar while playing, Untrimmed, Edited, and a second seek, about 10 s each. A temporary log (removed) recorded, for every frame newly shown, the output frame heard one refresh later by the sound clock.

| Stretch | Frames shown | Skipped | Shown twice | Frame start minus sound heard at display, min / p50 / max |
| --- | --- | --- | --- | --- |
| Play from 0 | 1211 | 2 | 0 | -20.8 / -10.3 / -1.8 ms |
| Seek while playing | 607 | 1 | 0 | -21.2 / -13.0 / 0 ms |
| Untrimmed | 601 | 1 | 0 | -19.3 / -13.7 / 0 ms |
| Edited again | 597 | 1 | 0 | -21.3 / -14.5 / 0 ms |
| Seek while playing | 602 | 0 | 0 | -22.7 / -13.7 / 0 ms |

A frame lasts 16.7 ms, so a value between -16.7 and 0 ms means the sound heard is inside the frame on screen. The median is 10 to 15 ms into the frame; at worst the sound is 6 ms past its end. The skips are the one frame per 20 s that a 59.95 Hz display owes 60 fps sound. Seeks and view switches restart in sync: the picture waits for the device.

What these numbers rest on: the display latency is taken as one refresh (`VIDEO_AHEAD`, 16.7 ms), not measured, and the audio latency is what CoreAudio reports for the device. An end-to-end check failed: ScreenCaptureKit's system audio recorded silence even for `afplay` at the machine's current (muted) output volume, so the acoustic onset could not be put against captured frames without changing the system volume, which I did not do. The pushed audio is not silent: 857 of 2884 pushed blocks carried speech, peak RMS 0.29.

## Bench

`shot-app --bench $SHOT_FIXTURES/screen-30s.mp4`, release, 20:31 to 20:37, load average 3.8 at start and 1.7 at the end: PASS, 5 of 5 runs. 1 dropped frame in 300 s (run 2, limit 1), no late frames or underruns, 300 seeks p50 33.5 ms, p95 66.7 ms, max 67.0 ms, every seek on its exact frame. Same as [preview.md](preview.md): the bench path (`Player`) is unchanged.

## Black preview of new takes

The coordinator reported that a take recorded in the app previews black. The decoder is not the cause: every recorded take in the session's demo folder (t02 to t05, AVAssetWriter fMP4 at timescale 600, screen 2560x1440 at 60 fps, camera 1920x1080 at 20 fps starting at 6.7 ms) opens with the fragment index, seeks to 0, 1, 5 and 18 s and decodes forward with `Decoder::open` and `open_native`. The camera files themselves are black: ffmpeg reads a maximum luma of 23 (of 255) in all four, and the screen files have content. The opinion format's default layout is Face, which shows only the camera, so the new chapter is black in preview and would be black in export. With the lid closed the built-in camera sees nothing, and the default camera is "USB3 Video", a capture device. Not a preview bug; no code change for it. Worth a guard in Record: warn when the camera delivers black frames.

## Checks

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`: pass.
- Ignored, run by hand: `shot-render --test export` (re-export renders only changed chunks, after moving export onto `FrameCursor` and `TimelineAudio`), `shot-media --test speaker` (the clock follows the device and holds when the ring is empty).
- New unit tests: BT.601 full range round trip through gpui's matrix, fade continuity and fade edges of timeline audio, the ring, the stamp history, the clock follower.

## Not verified

- **By ear**: I cannot listen. Whether cuts are free of clicks rests on the shared 5 ms fades and the fade-in after a restart, not on listening. Pause stops the device mid-waveform without a fade, which can click.
- **Acoustic A/V offset**, see above.
- **Output device changes** while the app runs: the default output unit follows the new device, but the latency is read once at start.
- **120 Hz displays**: `VIDEO_AHEAD` assumes 60 Hz.
