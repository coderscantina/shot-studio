# Fixtures

Test media is generated, never committed ([0007](../docs/decisions/0007-generated-fixtures-only.md)).

```
scripts/fixtures.sh [small|target|all] [--force]
```

Output goes to `fixtures/generated/` (git-ignored). Needs macOS (`say`), ffmpeg with libx265, ffprobe and awk. `small` takes about 1.5 min, `target` several minutes on an M1 Pro.

A rerun skips groups whose files all match `checksums.txt`. `--force` regenerates and logs `unchanged` or `changed` per file. The output is deterministic: x265 runs with one frame thread and no pool, muxing is bitexact with no metadata, and the speech comes from `say` with a fixed voice (Samantha). A different voice or macOS speech version changes the speech checksums.

## Layout

| Path | Content |
| --- | --- |
| `speech/*.wav`, `speech/*.txt` | Six clips, mono 48 kHz PCM, and the spoken text: `clean`, `retake`, `self-correction`, `filler`, `long-silence` (8 s), `mid-pause` (1.5 s) |
| `screen-30s.mp4` | 3840x2160, 60 fps, HEVC, fragmented MP4, 1 s GOP |
| `screen-30s.events.jsonl` | Cursor, clicks and keys of the screen clip |
| `camera-30s.mp4` | 1920x1080, 60 fps, HEVC, fragmented MP4, synthetic face |
| `target/screen.mp4`, `camera.mp4` | 17 min, same formats as above |
| `target/mic.mp4` | 17 min, mono 48 kHz AAC in fragmented MP4, narration from `say` |
| `target/events.jsonl` | Events for `target/screen.mp4` |
| `target/chapters.json` | Eight chapters as `{"title","start_us","end_us"}`, all tracks share this timeline |
| `checksums.txt` | sha256 of every output |

## Spec

`fixtures/spec/` holds the inputs:

- `clean.txt` and the other clip texts are exactly what `say` speaks. `[[slnc N]]` is a pause of N ms. The generated `speech/*.txt` is the same text without that markup, the ground truth for transcription tests.
- `screen-scenario.txt` is one 6 s loop of cursor moves, clicks and typing. Every screen segment and event log tiles it.
- `chapters.tsv` lists chapter length, accent color and title. `narration-N.txt` is spoken in chapter N after 0.8 s of silence, then padded with silence to the chapter end.

## Event log

JSON Lines, one event per line, sorted by time. `t_us` is media time in microseconds, from the first frame of the screen track.

```
{"t_us":416667,"kind":"move","x":1919,"y":1079}
{"t_us":1400000,"kind":"down","x":800,"y":500}
{"t_us":3000000,"kind":"key","key":"c"}
```

`kind` is `move`, `down`, `up` or `key`. `x` and `y` (screen pixels) are on `move`, `down` and `up`, `key` is only on `key`. A `move` is written whenever the cursor position changes between frames. The cursor in the video is drawn at the position of the latest `move` event for each frame, and clicks and typed keys appear on screen at their event time.

## Notes

- The mic track is AAC, so the container carries a 1024 sample encoder delay.
- The target narration is about 6 min of speech spread over 17 min,, padded with silence to each chapter end.
- The screen and camera are looped 6 s segments joined by stream copy, one segment variant per chapter for the screen. They look synthetic and are cheap to decode compared with real footage.
- Models are not fixtures: see `models/manifest.json`.
