# Milestone 03 evidence: chunked cache and incremental re-export

Measured on 2026-11-07, branch `task/m03-exportperf`. Follows [export.md](export.md), which found the re-export at 23.9 s against a 20 s target.

## Verdict

Deleting a word in one chapter and exporting again now takes 6 to 7 s and renders 2 chunks of about 40 frames. The full export stays where it was, at the hardware encoder's cap.

| Target | Before | Now | Result |
| --- | --- | --- | --- |
| One chapter changed, re-export under 20 s | 23.9 s (2 of 26 segments, 1,722 frames) | 6.2 to 7.0 s for a word deletion (2 chunks, 13 to 43 frames) | Pass |
| Full export of the 17 min project under 5 min | 591 s | 588.5 s (56,274 frames at 97 fps) | Fail, encoder bound, Mike's call |
| Peak memory under 2 GB | 664 MB (full) | 362 MB (full), 296 MB (re-export) | Pass |

What is left of a re-export is writing the 5.8 GB output file. Rendering two chunks and encoding the whole cut's audio run at the same time as that write.

## Setup

| | |
| --- | --- |
| Machine | MacBook Pro M1 Pro 32 GB, macOS 27, rustc 1.99.1. Other agents shared the machine, so numbers are from runs 15 to 20 s apart, after the previous export's writeback settled |
| Fixture | `$SHOT_FIXTURES/target-project`, copied to a scratch folder with `media/` symlinked |
| Build | `cargo build --release -p shot-cli` |
| Command | `/usr/bin/time -l shot export --video <copy>` (default 3 jobs; the full export used `--jobs 2` like export.md) |

## What changed

1. **Chunks.** `shot_render::plan` cuts each segment on its take's own grid of 0.5 s windows (30 frames at 60 fps, the recordings' keyframe interval) and at the segment's edges. A chunk is cached as `.shot/cache/chunks/<hash>.mp4`, one fragment that starts with an IDR frame. Its key is `Output::chunk_hash` in `shot-core`: renderer version, aspect, captions, take id, media identity, frame rate, the chunk's frames of the take, layout, the zooms cut to the chunk and the overlays overlapping it. Output position is not in it, so a word deletion re-keys only the chunk before and the chunk after the cut. `RENDERER_VERSION` is 2, so old segment files never match.
2. **Encoder sessions.** Consecutive missing chunks of one segment render in one `VTCompressionSession`, which forces a keyframe at each chunk start (`HevcEncoder::encode_keyframe`). The writer fails the export if a chunk does not start with a keyframe. In every chunk checked, the first sample is an IDR (`IDR_N_LP`), so chunks from different sessions join without decoder state carried across.
3. **Media identity.** The export stats `screen.mp4` and `camera.mp4` of each take (size and modification time) into a `MediaId` and passes it to the pure planning. A take recorded again under a reused id gets new keys. The mic is not part of it because audio is not cached.
4. **Decoder.** `Decoder::open_native` outputs the decoder's own `420v`, with no pixel transfer. Export uses it; the render graph already reads either range. `Decoder::open` and `open_with` now build the keyframe index from the `moov` and `moof` headers (`mux::video_sample_times`), and scan the samples only when a file is not fragmented, has an edit list other than one plain edit, or its first sample disagrees with what AVFoundation reports.
5. **Overlapped join.** The join starts with rendering and audio. It copies chunks in output order as soon as each is cached, and takes the AAC packets from the encoder thread as they come out (`AacEncoder::take`). A failure in any thread stops the others.

## Measurements

### Full export

```
exported main (937.9 s) to .../proj/.shot/export/main.mp4
1885 of 1885 chunks rendered, 56274 frames in 582.7 s (97 fps), joined in 5.9 s, 588.5 s total
      588.93 real        62.58 user        80.17 sys
           361547000  peak memory footprint
```

Before: 591.1 s and 664 MB. Forcing a keyframe every chunk adds none beyond the 0.5 s interval the encoder already used. The chunk cache is 5.6 GB, like the segment cache was. This run used the sequential join; the overlapped join hides its 5.9 s behind rendering.

### Re-export after a word deletion

A 0.31 s word deleted in "Auto cut":

```diff
-        keep from=504_800_000 to=534_000_000
+        keep from=504_800_000 to=529_830_000
+        remove from=529_830_000 to=530_140_000 by=manual
+        keep from=530_140_000 to=534_000_000
```

```
2 of 1885 chunks rendered, 42 frames in 4.0 s (11 fps), joined in 2.7 s, 6.7 s total
        6.74 real         2.36 user         2.84 sys
           296191080  peak memory footprint
```

The two chunks are frames 31,770 to 31,790 and 31,808 to 31,830: the ends of the half-second windows that hold the new cut edges. "Rendered in 4.0 s" includes audio. In this mode it is the time until the last chunk and the audio are done, slowed by the join's disk writes, which run at the same time. "Joined in 2.7 s" is how much longer the join ran after that.

Three more word deletions, at different points in "Auto cut", alternated with a build that joins after rendering:

| Build | Runs (s) |
| --- | --- |
| Join after render and audio | 8.5, 7.6, 7.5 |
| Join alongside them (shipped) | 7.0, 6.2, 6.7 |

### Re-export after the keep range change from export.md

The same diff as export.md (half a second removed at 520.0 s):

```
0 of 1884 chunks rendered, 0 frames in 2.2 s (0 fps), joined in 5.0 s, 7.2 s total
        7.63 real         2.22 user         2.69 sys
```

Both new edges, 520.0 s and 520.5 s, fall on the 0.5 s grid, so every chunk already exists. Only the join runs, and 2.2 s of it is spent waiting on the audio.

### Re-export after a layout switch

`layout face-content from=520_000_000 to=530_000_000` added in "Auto cut":

```
20 of 1885 chunks rendered, 600 frames in 9.8 s (61 fps), joined in 2.9 s, 12.6 s total
```

### Where a re-export spends its time

- **Join: 4 to 9 s** for the 5.8 GB output. Timing the phases showed 0.2 to 1.3 s reading the 1,885 chunk headers, 5.2 to 7.3 s copying, and under 0.2 s for the final `fsync`. The copy is disk-bound: a positional copy of the chunk files with 1, 4 and 8 threads took 4.1, 5.9 and 8.2 s (and 8.5 s again with 1 thread). Back-to-back exports are slower because the previous 5.8 GB is still being written back.
- **Audio: 2.2 s** to read and encode 937 s of AAC. It is now hidden behind the join.
- **Decoder open: 17 to 155 ms** per 17 min track with the fragment index (8 to 221 ms of that is the index), down from 1.5 to 1.7 s. During an overlapped re-export, opening rises to about 3 s because the join's writes saturate the disk.

## Output checks (ffprobe, the re-export after a word deletion)

```
stream|codec_name=hevc|profile=Main|width=3840|height=2160|pix_fmt=yuv420p|color_range=tv|color_space=bt709|r_frame_rate=60/1|start_time=0.000000|duration=937.616667
stream|codec_name=aac|profile=LC|start_time=0.000000|duration=937.664000
```

- **Duration**: 56,257 video packets, equal to the plan's 56,257 frames (17 frames fewer than the full export, for the 0.29 s word). The audio ends at 937.62 s after the priming.
- **Joins**: every video packet is exactly 1,000 ticks (1/60 s) after the one before, and every audio packet exactly 1,024 samples. Decode times only increase. There are 1,885 keyframes, one per chunk and no others.
- **Full decode**: `ffmpeg -hwaccel videotoolbox -f null` decodes the whole file with no errors (7 min, at the same time as other work).
- **Audio sync**: cross-correlation against `mic.mp4` gives 44.0 ms at 499.7 s (before the edit), 595.0 s and 715.0 s (after it). export.md measured the same 44.0 ms and traced it to the fixture, which does not signal its AAC delay. The join adds no offset.
- **Video mapping**: output frame 35,700 (595.0 s, after the edit) matches camera frame 39,143 best (35.3 dB, neighbours 28.7 and 32.6 dB). The plan maps it to exactly that frame.

## What was not verified

- **Fragment index on capture output**: `mux::video_sample_times` matches the full scan on the ffmpeg-written fixtures. Files written by `AVAssetWriter` in `shot-capture` were not tried, because recording needs Screen Recording permission. If their layout differs, the first-sample check or the parser's refusals fall back to the scan, which is correct but slow.
- **Re-recorded take end to end**: the media id changes the keys (unit test), but no export ran against a take recorded again. The shared fixture must not be modified.
- **Stale chunks** are never deleted, like segments before them. After this run the cache held 1,902 chunks (6.1 GB) for 1,885 in use. The old `.shot/cache/segments/` folder is not cleaned up either.
