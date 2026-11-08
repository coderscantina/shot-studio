# Fixtures

Test media is generated, never committed ([0007](../docs/decisions/0007-generated-fixtures-only.md)).

```
scripts/fixtures.sh [small|target|demo|all] [--force]
```

Output goes to `$SHOT_FIXTURES`, by default `fixtures/generated/` (git-ignored). Agents working in a git worktree set `SHOT_FIXTURES=$HOME/.cache/shot-studio-fixtures`, so media is generated once and survives removing the worktree. Needs macOS (`say`), ffmpeg with libx265, ffprobe and awk. `small` takes about 1.5 min, `target` several minutes on an M1 Pro, `demo` a second.

`demo` always rebuilds the repo, since it is cheap. A rerun skips media groups whose files all match `checksums.txt`. `--force` regenerates and logs `unchanged` or `changed` per file. The output is deterministic: x265 runs with one frame thread and no pool, muxing is bitexact with no metadata, and the speech comes from `say` with a fixed voice (Samantha). A different voice or macOS speech version changes the speech checksums.

## Layout

| Path | Content |
| --- | --- |
| `speech/*.wav`, `speech/*.txt` | Six clips, mono 48 kHz PCM, and the spoken text: `clean`, `retake`, `self-correction`, `filler`, `long-silence` (8 s), `mid-pause` (1.5 s) |
| `screen-30s.mp4` | 3840x2160, 60 fps, HEVC, fragmented MP4, 0.5 s GOP (30 frames) |
| `screen-30s.events.jsonl` | Cursor, clicks and keys of the screen clip |
| `camera-30s.mp4` | 1920x1080, 60 fps, HEVC, fragmented MP4, synthetic face |
| `target/screen.mp4`, `camera.mp4` | 17 min, same formats as above |
| `target/mic.mp4` | 17 min, mono 48 kHz AAC in fragmented MP4, narration from `say` |
| `target/events.jsonl` | Events for `target/screen.mp4` |
| `target/chapters.json` | Eight chapters as `{"title","start_us","end_us"}`, all tracks share this timeline |
| `target-project/` | Copy of `fixtures/projects/target/` (video.kdl, format.kdl, script.md, cut.kdl) with the target tracks hard-linked into `media/t01/` as `screen.mp4`, `camera.mp4`, `mic.mp4`, `events.jsonl` |
| `demo-channel/` | Git repo built from `fixtures/projects/demo-channel/`, see below |
| `demo-channel.refs` | Every ref of the demo repo with its sha |
| `checksums.txt` | sha256 of every output |

## Projects

`fixtures/projects/` is committed text, all KDL in canonical form (a `shot-core` test checks this).

- `target/` is the 8 chapter video folder for the 17 min media. `cut.kdl` tiles each chapter of `chapters.json` with kept and removed ranges of take `t01`, including silence, pause, filler, retake, misspoken and manual removals.
- `demo-channel/` holds the final files of seven videos. The generator starts a repo on `main` with the formats and every video as an idea, then walks each non-idea video up its columns on `video/<slug>` with one `🚚 move <slug> to <status>` commit per step. `script.md` appears at `scripting`, an empty `format.kdl` (the video's layer, nothing differs from the channel's format) with `cut.lock` at `ready`, and `cut.kdl` with its `cut.lock` at `editing`. Videos start migrated, as the app creates them: `cut.kdl` holds only a person's removals, and each `cut.lock` (`empty-cut.lock` before a cut exists) is what `shot style build` writes for those files; a `shot-app` test checks this. `tailwind-without-the-config` is shipped and tagged, `proposals/<slug>/<name>/` becomes the open branch `proposal/<slug>/<name>`.
- Authors, committers and dates are fixed (12 h apart from 2026-10-05), so every sha is the same on every run. `demo-channel.refs` is the proof.

## Spec

`fixtures/spec/` holds the inputs:

- `clean.txt` and the other clip texts are exactly what `say` speaks. `[[slnc N]]` is a pause of N ms. The generated `speech/*.txt` is the same text without that markup, the ground truth for transcription tests.
- `screen-scenario.txt` is one 6 s loop of cursor moves, clicks and typing. Every screen segment and event log tiles it.
- `chapters.tsv` lists chapter length, accent color and title. `narration-N.txt` is spoken in chapter N after 0.8 s of silence, then padded with silence to the chapter end.

## Event log

JSON Lines as in [the contract](../docs/contracts/project-files.md#cursor-and-key-event-log), sorted by time. `t` is media time in microseconds, from the first frame of the screen track.

```
{"t":416667,"kind":"move","x":1919,"y":1079}
{"t":1400000,"kind":"down","x":800,"y":500,"button":"left"}
{"t":3000000,"kind":"key","key":"c","mods":[]}
```

The generator writes `move`, `down`, `up` (always `left`) and `key` (never with modifiers, a space is `space`). A `move` is written whenever the cursor position changes between frames. The cursor in the video is drawn at the position of the latest `move` event for each frame, and clicks and typed keys appear on screen at their event time.

## Notes

- The mic track is AAC, so the container carries a 1024 sample encoder delay.
- The target narration is about 6 min of speech spread over 17 min,, padded with silence to each chapter end.
- The screen and camera are looped 6 s segments joined by stream copy, one segment variant per chapter for the screen. They look synthetic and are cheap to decode compared with real footage.
- Models are not fixtures: see `models/manifest.json`.
