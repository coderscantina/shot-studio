# Milestone 03 evidence: render graph, layouts and incremental export

Measured on 2026-11-07, branch `task/m03-export`. Brief: [milestone 03](../../milestones/03-opinion-video.md) work items 5 and 6.

## Verdict

`shot export` turns `cut.kdl` into a 4K60 HEVC MP4 with AAC audio and chapters. It renders only segments whose content hash has no cached file, and it joins them by stream copy. Both time targets fail on this machine. The cause is the hardware HEVC encoder, which tops out at about 95 fps at 4K no matter how many sessions run:

| Target | Measured | Result |
| --- | --- | --- |
| Full export of the 17 min project under 5 min | 591 s (56,274 frames at 96 fps) | Fail, encoder bound |
| One chapter changed, re-export under 20 s | 23.9 s (2 of 26 segments, 1,722 frames) | Fail, encoder bound |
| Peak memory under 2 GB | 664 MB footprint (full), 385 MB (re-export) | Pass |
| No CPU copy of pixels | decode, render and encode share IOSurfaces | Pass |

## Setup

| | |
| --- | --- |
| Machine | MacBook Pro M1 Pro 32 GB, macOS 27, rustc 1.99.1. Other agents shared the machine (a preview bench was running during part of the work) |
| Fixture | `$SHOT_FIXTURES/target-project`, 0.5 s GOP fixtures. Exports ran on a copy of the folder with `media/` symlinked, so the cache did not land in the shared fixtures |
| Build | `cargo build --release -p shot-cli` |
| Command | `/usr/bin/time -l shot export --video <copy> --jobs 2` |

## Pipeline

1. `shot-core` evaluates the cut. `shot_render::plan` snaps each segment to the 60 fps grid from its source range alone, so a cached segment stays valid when an edit happens before it.
2. Up to `--jobs` workers each own a wgpu device and keep decoders open across segments. Per frame: decode (`shot-media` `Decoder`, IOSurface `420f`) → render graph into a buffer from the encoder's own pool (`420v`, IOSurface) → `VTCompressionSession`.
3. A segment is written to `.shot/cache/segments/<hash>.mp4` as fragmented MP4, one fragment per 0.5 s GOP, then renamed into place. An existing file is reused. Stale files are not deleted in 03.
4. Audio runs on its own thread while video renders. It reads the mic of each take through AVFoundation as 48 kHz PCM, fades 5 ms where the source jumps, and encodes one continuous AAC-LC stream (128 kbit/s, AudioToolbox). The cache holds video only. One AAC stream has no encoder delay or padding at the segment joins. Per-segment AAC would put 2112 samples of priming and up to 1023 samples of padding at every join, so it would click and drift. At 60 fps a frame is exactly 800 samples, so audio and video share one grid.
5. The join writes one init segment (`moov` with `mvex`, BT.709 `colr`, an `elst` for the AAC delay and a Nero `chpl` chapter list). Then it copies each segment's fragments with a new sequence number and shifted `tfdt`, and writes the audio for the same span after each one. No FFmpeg: the muxer is `shot_media::mux`, about 400 lines.

### Layouts

Typed `LayoutSpec` per layout in `crates/shot-render/src/layout.rs`, designed on a 3840x2160 canvas:

| Layout | Screen | Camera |
| --- | --- | --- |
| `face` | not shown | fills the canvas, cropped to cover |
| `content` | fills the canvas | not shown |
| `face-content` | fills the canvas | 864x486 inset, 72 px from the bottom right, 36 px corners |
| `split` | 2560x1440 panel | 1080x1440 panel, 48 px gap, 32 px corners, centered on `#0F1115` |

`zoom` punches in on the primary slot, which is the camera in `face` and the screen everywhere else. `screen`, `face-corner` and `window` are refused with an error naming the layout. A scene is a list of layers (source, frame, crop, corner radius), so overlays in 04 to 07 add layers.

### Render graph

One WGSL shader (`compose.wgsl`), two draws per layer: one into the luma plane, one into the half-size chroma plane. It samples the decoded planes in place, converts to RGB with the BT.709 matrix (the range is read from the pixel format, `420v` or `420f`), and computes rounded-corner coverage. It writes BT.709 YCbCr in the target's range. Blending in YCbCr equals blending in RGB because the conversion is affine, so no RGB intermediate texture is needed. Preview can use the same `Renderer::render` with a `BufferPool` of `420f` buffers, which is what gpui's surface element shows.

## Measurements

### Full export

```
exported main (937.9 s) to ./.shot/export/main.mp4
25 of 25 segments rendered, 56274 frames in 587.6 s (96 fps), joined in 3.5 s, 591.1 s total
      591.22 real        74.92 user        93.37 sys
           663881336  peak memory footprint
```

- CPU: 168 s over 591 s, 28 % of one core for `shot`. The encode runs in VideoToolbox's service process and is not counted.
- Output 5.8 GB: video 49.7 Mbit/s, audio 58 kbit/s (mostly silence). Cache 5.6 GB.

### Re-export after one chapter changed

A copy of `cut.kdl` with half a second deleted in "Auto cut", like deleting a word:

```diff
-        keep from=504_800_000 to=534_000_000
+        keep from=504_800_000 to=520_000_000
+        remove from=520_000_000 to=520_500_000 by=manual
+        keep from=520_500_000 to=534_000_000
```

```
exported main (937.4 s) to ./.shot/export/main.mp4
2 of 26 segments rendered, 1722 frames in 19.9 s (86 fps), joined in 4.0 s, 23.9 s total
       23.92 real         7.36 user         7.95 sys
           384713976  peak memory footprint
```

The rest of the output was reused. The render time is about 18 s of encoding (1,722 frames at 96 fps) plus 1.7 s of decoder open. The join is 4.0 s, mostly copying 5.8 GB.

### Where the time goes

Per-stage timing of one session, 600 frames of 4K, before speed priority was on: decode 0.27 s, render 1.3 s (including the GPU wait), encode 10.0 s.

Encoder throughput at 4K60, one segment per session:

| Setting | 1 session | 2 | 3 | 6 |
| --- | --- | --- | --- | --- |
| Quality mode, default | 49 fps | 49 | 49 | 47 |
| `PrioritizeEncodingSpeedOverQuality` | 92 fps | 90 | 89 | not run |

- The quality value (0.5 to 1.0) did not change speed.
- `RealTime` gives the same speedup as speed priority, and `MaximizePowerEfficiency=false` gives none.
- `ffmpeg hevc_videotoolbox -prio_speed 1` reaches the same 91 fps, with either `-q:v` or `-b:v`. `h264_videotoolbox` reaches 54 fps. HEVC at 1080p reaches 293 fps, so the engine is limited by pixel rate.

Speed priority costs little quality. PSNR against a quality 1.0 encode, 5 s of `split`:

| Setting | Size | PSNR |
| --- | --- | --- |
| 0.65, speed priority | 5.6 MB | 49.2 dB |
| 0.70, speed priority | 9.6 MB | 50.8 dB |
| 0.70, quality priority | 10.2 MB | 51.3 dB |
| 0.75, speed priority (shipped) | 13.0 MB | 51.5 dB |

Decode is not the limit on this project, but it would be on screen-heavy layouts:

- One 4K stream: 220 fps with the decoder's `420f` output, 743 fps with native `420v`.
- Three parallel streams: 352 fps aggregate for `420f`, 745 fps for `420v`.
- `Decoder::open` scans the whole file: 1.5 s for the 17 min screen track, 1.7 s for the camera.

Jobs: 1, 2 and 3 workers finish within 3 % of each other, since one session already saturates the engine. The default is 2. A second worker hides each segment's start, drain and decoder seek.

### Output checks (ffprobe)

```
stream|codec_name=hevc|profile=Main|codec_tag_string=hvc1|width=3840|height=2160|pix_fmt=yuv420p|color_range=tv|color_space=bt709|color_transfer=bt709|color_primaries=bt709|r_frame_rate=60/1|avg_frame_rate=60/1|start_time=0.000000|duration=937.900000
stream|codec_name=aac|profile=LC|sample_rate=48000|channels=1|start_time=0.000000|duration=937.962667
chapter|start_time=0.000000|end_time=87.600000|tag:title=Intro
chapter|start_time=87.600000|end_time=216.500000|tag:title=Install and setup
... eight chapters, the last ends at 937.96
```

- **Duration**: video is 937.900 s, equal to the timeline's 56,274 frames. ffprobe counts the 44 ms AAC priming in the audio stream's duration (937.963 s). The decoder skips it (`skip 2112 samples due to side data`).
- **Joins**: all 56,274 video packets are exactly 1/60 s apart and all 43,967 audio packets exactly 1024 samples apart. There are 1,884 keyframes. A full hardware decode pass (`ffmpeg -hwaccel videotoolbox -f null`) reports no errors.
- **Video mapping**: the output frame at 88.1 s, downscaled to 1080p, matches camera frame 97.3 s best (43.2 dB). Frames one or two away score 38.3 to 39.9 dB.
- **Audio sync**: cross-correlating the output with the source mic at four chapter starts gives the same offset every time: the output is 44.0 ms ahead of ffmpeg's decode of `mic.mp4`. The cause is the fixture. Its mic is AAC in fragmented MP4 without an edit list, so the 1024-sample encoder delay is not signalled. ffmpeg decodes it without skipping anything, which is 21.3 ms late. AVFoundation assumes Apple's default 2112 samples, which is 22.7 ms early. `afconvert` shows the same 44 ms between the two decoders. On this fixture the export's audio therefore leads by 22.7 ms. That is under the 45 ms lead people notice, and it is the input's error, not the join's. Capture written with AVAssetWriter signals the delay.

### Frames per layout

Git-ignored, in `docs/evidence/m03/`, from a short cut with every layout:

- `export-layout-face.png`: the synthetic face fills the frame, upscaled 2x from 1080p.
- `export-layout-content.png`: the screen recording fills the frame. Colors match the source bars.
- `export-layout-face-content.png`: the screen full frame, with the camera inset at the bottom right. Corners are rounded and antialiased.
- `export-layout-face-content-zoom-115.png`: the same moment inside a `zoom 115`. The screen is punched in around its center, the inset is unchanged.
- `export-layout-split.png`: the screen panel left, the camera cropped to a 3:4 panel right, both rounded, on the dark background.

## Options that keep the targets

Both targets need about twice the encode throughput, or fewer encoded frames.

- **Encode fewer frames on re-export (biggest lever)**: cache in chunks on the take's absolute 0.5 s GOP grid instead of whole segments. A word deletion would then re-render about a second of frames instead of the whole kept range, about 5 s total. This changes the cache key from the segment hash to a chunk key, which is a contract change.
- **Stream copy untouched source**: a `content` segment without zoom or overlays is the screen track itself, which is already 4K60 HEVC with a 0.5 s GOP. Copying its whole GOPs and re-encoding only the partial first GOP skips most encoding for screen-heavy videos. It needs a second sample entry, because the parameter sets differ. It does not help this face-heavy fixture.
- **Hardware with two encode engines** (M1 Max and later Max and Ultra chips) roughly halves the time.
- **Overlap the join with rendering**: copy cached segments while changed ones render. Saves up to 4 s on a re-export.
- **Relaxing the target** (for example 4K30 at about 295 s, or about 10 min for 4K60 on an M1 Pro) is Mike's call.

## What was not verified

- **Encoder CPU**: `VTEncoderXPCService` CPU was not sampled.
- **QuickTime and YouTube**: playback of the fragmented output was checked with ffmpeg only.
- **Preview wiring**: `Renderer` into gpui is not wired, since shot-app is not part of this task.
- **Derived cuts**: refused with an error. They need 9:16 and 1:1 layouts (milestone 07).
- **Long camera clips past the end of the media**: the last frame is held, but no test covers it.
