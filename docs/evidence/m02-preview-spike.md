# Milestone 02 evidence: preview spike

Measured on 2026-11-07. Brief: [milestone 02](../milestones/02-preview-spike.md). Decision under test: [0002](../decisions/0002-gpui-and-zero-copy-preview.md).

## Verdict

gpui shows a 4K60 HEVC VideoToolbox decode with no CPU copy of frame pixels. Two of the four criteria fail as written, both by small margins:

- **Seeks**: 5 to 9 of 60 random seeks take 100 ms or more. The cause is the 1 s GOP times the hardware decoder's speed, not gpui. With a 0.5 s GOP every seek is under 67 ms.
- **Playback**: 2 of 5 runs drop nothing. The other 3 drop 1 to 3 frames, each time when a render misses one display refresh on this shared, busy machine. Decode and pacing kept up in every run.

Per the brief this gates milestone 03 until Mike decides. Decision 0002 is left as is.

## Setup

| | |
| --- | --- |
| Source | branch `task/m02-spike`, commit `083163c` (code), on top of `09eea48` |
| Machine | MacBook Pro, Apple M1 Pro, 32 GB, lid closed, on AC power |
| Display | Dell U2515H 2560x1440 at 1x scale, nominal 60.00 Hz, measured 59.95 Hz. The built-in 120 Hz panel was not in use |
| OS and tools | macOS 27.0.1 (26A434), Xcode 27.2 (27B52), rustc 1.99.1 |
| Crates | gpui-pre 0.3.11 (Zed f816e1f) with gpui-pre-platform features `font-kit` and `runtime_shaders`, wgpu 30.0.1, objc2 0.6.5 with the 0.3.2 framework crates |
| Fixture | `fixtures/generated/screen-30s.mp4`, sha256 `1fd7a17b…aff426e0a` (matches `checksums.txt`): 3840x2160, 60 fps, HEVC Main, BT.709 limited range, fragmented MP4, 30 keyframes (1 s GOP), no B-frames, 32 MB |
| Window | 1600x1000, video drawn into 1600x900 with `ObjectFit::Contain` |
| Load | Other agents and desktop apps were running. WindowServer used about 33 % of a core during the profile |

## Commands

```sh
export CARGO_TARGET_DIR=$HOME/.cache/shot-studio-target
cargo build --release -p shot-app
F=fixtures/generated/screen-30s.mp4
$CARGO_TARGET_DIR/release/shot-app --bench $F               # 60 s playback, then 60 random seeks
$CARGO_TARGET_DIR/release/shot-app --bench --zoom $F        # same with the wgpu punch-in on every frame
$CARGO_TARGET_DIR/release/shot-app --bench --wall-clock $F  # pacing without the display lock
$CARGO_TARGET_DIR/release/shot-app $F                       # interactive: space, z, drag to scrub
```

The bench opens the window paused and presses play after 1 s. Seeks use a fixed seed, so every run seeks to the same 60 places. The bench waits 3 renders between seeks, roughly a fast scrub.

## Results

Each default row is one full run of `--bench`.

| Run | Presented / expected | Dropped | Render gaps over 25 ms | Seek p50 / p95 / max | Seeks at 100 ms or more | App CPU | Decoder service CPU | Peak footprint |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| default 1 | 3600 / 3600 | 0 | 0 | 66.6 / 100.1 / 116.7 ms | 6 | 13.9 % | 6.9 % | 228 MB |
| default 2 | 3599 / 3600 | 1 | 1 (33.5 ms) | 64.8 / 100.0 / 100.1 ms | 6 | 14.1 % | 7.0 % | 231 MB |
| default 3 | 3600 / 3600 | 0 | 0 | 50.2 / 100.0 / 100.1 ms | 5 | 14.0 % | 7.0 % | 234 MB |
| default 4 | 3599 / 3600 | 1 | 1 (33.4 ms) | 66.5 / 100.1 / 100.2 ms | 5 | 13.5 % | 6.6 % | 233 MB |
| default 5 | 3597 / 3600 | 3 | 2 (max 31.3 ms) | 66.6 / 100.1 / 100.2 ms | 7 | 13.4 % | 6.6 % | 228 MB |
| `--zoom` | 3600 / 3600 | 0 | 0 | 66.6 / 100.1 / 116.7 ms | 9 | 21.6 % | 6.5 % | 276 MB |
| `--wall-clock` | 3583 / 3601 | 18 | 3 | 10 seeks only | 0 | 13.9 % | 6.9 % | 77 MB |
| 0.5 s GOP copy | 3598 / 3600 | 2 | 1 (33.2 ms) | 49.9 / 66.6 / 66.9 ms | 0 | 14.0 % | 6.9 % | 209 MB |

CPU is percent of one core over the 60 s of playback. The decoder service is `VTDecoderXPCService`, where VideoToolbox runs hardware decode out of process. Footprint is Activity Monitor's "Memory" for the app; peak resident size was 264 to 275 MB.

Seek latency by how many frames the seek decodes past its keyframe, default run 1:

| Frames past keyframe | Seeks | Mean | Max |
| --- | --- | --- | --- |
| 0 to 19 | 23 | 38.3 ms | 50.1 ms |
| 20 to 39 | 19 | 65.8 ms | 83.5 ms |
| 40 to 59 | 18 | 89.9 ms | 116.7 ms |

## Acceptance criteria

| Criterion | Measured | Result |
| --- | --- | --- |
| 60 s of 4K60 playback drops no frames | 0 drops in 2 of 5 runs and in the zoom run. 1, 1 and 3 drops in the others, each at a render gap of 31 to 34 ms. No underruns and no late frames in any run | Fail as written |
| A random seek shows a frame in under 100 ms ([0010](../decisions/0010-acceptance-thresholds.md), proposed) | p50 50 to 67 ms, p95 100.0 to 100.1 ms, max 100.1 to 116.7 ms, 5 to 9 of 60 at or over 100 ms. Every seek showed the exact target frame | Fail |
| A profile shows no CPU copy of frame pixels between decode and display | No pixel read-back on the path. Time Profiler shows no lock, map or pixel conversion on the CPU in either process | Pass |
| App memory stays under 2 GB while playing | Peak footprint 228 to 276 MB | Pass |

## How the frames travel

1. A passthrough `AVAssetReaderTrackOutput` (output settings nil) hands over compressed HEVC samples.
2. A `VTDecompressionSession` created with `RequireHardwareAcceleratedVideoDecoder` decodes them asynchronously. `UsingHardwareAcceleratedVideoDecoder` reads back true. The output attributes ask for `420f` with IOSurface properties and Metal compatibility. gpui's surface shader accepts only `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` and rejects anything else.
3. A decode thread keeps up to 8 frames queued while playing, 1 while paused. The UI thread picks due frames with `FramePacer`.
4. gpui's `surface()` element hands the `CVPixelBuffer` to its Metal renderer. It wraps both planes as textures through `CVMetalTextureCache` and converts YCbCr to RGB in the fragment shader.
5. With `--zoom`, the decode thread wraps both IOSurface planes as Metal textures (`newTextureWithDescriptor:iosurface:plane:`), imports them into wgpu (`texture_from_raw`, `create_texture_from_hal`), and renders the zoomed planes into a new IOSurface buffer from a `CVPixelBufferPool`. gpui shows that buffer.

## How no-copy was verified

- **Code path**: nothing on the path calls `CVPixelBufferLockBaseAddress`, `CVPixelBufferGetBaseAddress` or any other pixel accessor. Every buffer reports an IOSurface (`CVPixelBufferGetIOSurface`). gpui samples the IOSurface planes as Metal textures; the zoom pass reads and writes IOSurfaces on the GPU.
- **Time Profiler**: `xctrace record --template 'Time Profiler' --all-processes`, 25 s in the middle of a bench run. No `LockBaseAddress`, `vImage`, IOSurface lock or texture upload in either process.
  - App: all `memmove`/`memcpy` together took 30 ms over 25 s. Callers are gpui element arenas, taffy layout, IOSurface mach-port lookups and `CMBlockBufferCopyDataBytes`. That last one copies the compressed bitstream, about 18 KB per frame. Copying 4K frames at 60 fps would take about 1.8 s of CPU in that window. The main thread used 0.93 s.
  - Decoder service: 18 ms of copies, from bitstream and XPC serialization.
- **The range conversion**: the decoder produces video range (`420v`). Asking for `420f` makes VideoToolbox run `VTPixelTransferSession` → `figIOSurfaceAcceleratedPixelTransfer_TransferM2` → `IOSurfaceAcceleratorTransformSurface`, and the thread waits in `mach_msg` the whole time. That is the M2 scaler hardware converting one IOSurface into another, not the CPU. `sample` of the live service shows the same call tree.
- **CPU totals**: about 14 % of a core for the app and 7 % for the decoder service. A CPU path moving 4K 4:2:0 frames at 60 fps would show far more.

## Why the two criteria fail

### Seeks

Latency grows linearly with the frames decoded past the keyframe: about 1.3 ms per 4K frame, on top of about 25 ms of fixed cost. The fixed cost is reader start, the target frame's range conversion and the wait for the next render. A decode-only probe without the window measured the same slope: seeks to a keyframe 3.6 to 7.5 ms, seeks 59 frames past it 88 to 98 ms.

Measured values are multiples of 16.7 ms: 50.0, 66.6, 83.5, 100.1, 116.7. A finished seek waits for the next 60 Hz render, which adds up to one refresh. So a seek deep in a 1 s GOP lands just over 100 ms. Decoding only one frame ahead while paused did not change the numbers, so the bench's back-to-back seeks are not the cause.

### Playback

Every dropped frame lines up with a render gap of 31 to 34 ms, meaning gpui did not render for one refresh. At 60 Hz a missed render costs the frame it would have shown. The cause is outside our code as far as I can tell:

- The main thread used 3.7 % of a core in the profile.
- Decode never fell behind: 0 underruns, 0 late frames, and the queue held 6 frames at every drop.
- The gaps hit at random times: 33.4 s, 27.0 s, 9.3 s and 54.3 s.

The profile window did not contain a gap, so the blocking call is not identified.

Pacing itself caused drops before `FramePacer`. The monitor runs at 59.95 Hz against 60.000 fps media. A clock anchored at a render then puts frame boundaries on refreshes, and the phase drifts through a boundary every 20 s. The `--wall-clock` run shows it: 18 drops. `FramePacer` picks frames half a refresh ahead of the clock and pulls the clock 1/32 of the phase error per shown frame. Playback then follows the display 0.08 % slow (clock rate 0.99918) and the drift drops are gone. A unit test simulates 10 minutes at 59.95 Hz with jitter and missed renders: each missed render costs exactly one frame and nothing more. At 120 Hz a missed render costs no frame at all.

## Alternatives that keep the targets

- **Record with a 0.5 s GOP.** Measured on a re-encode of the fixture (hevc_videotoolbox, `-g 30`, 9 Mbit/s, scratch file only): every seek under 67 ms, p95 66.6 ms. Short-GOP HEVC is already the recording plan. It costs some bitrate and needs a fixture generator change (milestone 01's script).
- **Show a nearer frame first.** Show the keyframe or the last decoded frame at once, then the exact frame when it is ready. Seeks that decode fewer than 20 frames already average 37 to 40 ms end to end. This is a different reading of "shows a frame", so it is Mike's call.
- **Decode native `420v` and expand the range in our own wgpu pass.** Hardware decode runs at 742 fps instead of 217 fps without the M2 scaler step. This saves about 3 ms per seek and gives headroom for multi-track decode. The render graph needs a pass anyway.
- **For missed refreshes:** rerun on the built-in 120 Hz display with the machine idle, where one missed render does not cost a frame. If drops remain, profile with Metal System Trace during a gap. The fallback is a dedicated `CAMetalLayer` for the preview, presented from the decode side, which takes the main thread off the video path.

## What was not verified

- **Built-in ProMotion display**: the lid was closed, so 120 Hz behaviour is covered only by the unit test.
- **The missed-refresh cause**: the profile window missed the gaps.
- **Metal System Trace**: not recorded. Presentation was counted from frame timestamps at render time, not from drawable present times, which gpui does not expose.
- **Shader build**: precompiled gpui shaders could not be built. The Xcode Metal Toolchain component is missing (`xcodebuild -downloadComponent MetalToolchain`), so the app uses gpui's `runtime_shaders`. They compile the same source at startup. Per-frame rendering should not differ, but it was not compared.
- **Color**: gpui's surface shader uses a BT.601 matrix on full range, while the fixture is BT.709. Colors are slightly off; this was not measured.
- **The 420f/420v comparison**: it came from a throwaway decode probe (since deleted) with the output format switched. It cannot be rerun from this commit.
- **Mouse scrub**: driven by synthetic CGEvents only (a 40-step drag), not by hand.

## Files

- Screenshots and a 10 s recording, git-ignored, in `docs/evidence/m02/`: `preview-playing.png`, `preview-zoom-2x.png`, `preview-scrubbed.png`, `preview-play-and-scrub.mov`.
- Code: `crates/shot-media/src/{videotoolbox,player,timing}.rs`, `crates/shot-app/src/{preview,zoom,bench,metrics}.rs`.

## For milestone 03

Reuse the shot-media `Decoder`, `Player` and `FramePacer`, and gpui's `surface()` element. Reuse the zoom stage's IOSurface-to-wgpu import (`texture_from_raw` plus `create_texture_from_hal`) as the way shot-render reads and writes frames.

Carry these over:

- The preview window sets `inactive_frame_interval: None`. gpui's default caps an unfocused window at about 30 fps.
- gpui-platform needs `font-kit` for any text.
- `Window::refresh_interval` reports the nominal rate (60.000 Hz on a 59.95 Hz monitor). Lock to the display, don't trust the number.
- The keyframe index scans the whole file at open: 130 ms for 30 s. For 17 minute recordings, build it from the fMP4 fragment index or `AVSampleCursor`.
