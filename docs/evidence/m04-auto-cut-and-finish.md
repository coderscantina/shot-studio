# Milestone 04 evidence: auto cut and finish

Recorded 2026-11-07 on `task/m04-autocut`. Machine: MacBook Pro, Apple M1 Pro, 32 GB, macOS 27, rustc 1.99.1. Two other milestone agents shared the machine and the build cache during every measurement, so timings carry that load.

## Result

| Criterion | Evidence | Result |
| --- | --- | --- |
| On the `say` fixtures the known retake and self-correction are cut and the clean version is kept | `cargo test --release -p shot-analyze --test autocut -- --ignored`: the retake clip keeps "Welcome back. The first thing we do is create a new project from the tutorial preset.", the self-correction clip keeps "the build ran at 13.35, and it finished in two seconds." | Pass |
| Rough cut and transcript ready within 5 s of stop on the 17 min project with all rules on | `shot autocut --realtime` on a copy of `target-project` with `idle-screen on`: fed at recording speed for 1021 s, transcript and speech ready 0.08 s after the last sample, rough cut written 0.08 s after it | Pass, with the caveat below |
| Exported audio measures -14 LUFS integrated within 0.5 LU | ffmpeg `ebur128` on the exported 30 s project: -14.3 LUFS, peak -1.0 dBFS, with and without a music bed. The mic track went in at -16.6 LUFS | Pass |
| Every applied cut in `cut.kdl` names its rule, and every queue decision writes to `cut.kdl` | Rules write `remove ... by=<rule>` and `review ... by=<rule>` only. K and C pressed in the app turned a `review` into `ignore` and another into `remove ... by=filler` in `cut.kdl` (diff below) | Pass |

The 17 min take ends in silence, so its last VAD block and decode had little to do. Speech up to the stop costs one transcriber flush, measured in milestone 03 at 1.25 to 1.96 s on clips that end mid-speech, plus one VAD block of at most 30 s. The rules, the screen scan and the write take 0.01 s on the 17 min take from cached analysis.

## Rule counts per fixture

Opinion preset (tight pacing, confidence 85), each clip as one fresh take:

| Clip | Removed | Queued | Kept text |
| --- | --- | --- | --- |
| clean | nothing | nothing | the whole sentence |
| filler | filler 3 (1.45 s: um, uh, um), retake 1 (0.55 s: "the, " before "the settings") | filler 3: basically, you know, kind of | So, basically what we do here is, we open the file, and, you know, we kind of check the settings. |
| long-silence | silence 1 (7.32 s) | nothing | That was the setup. Now let's look at the result. |
| mid-pause | nothing | pause 1 (1.08 s, confidence 60 to 90 by length) | The renderer only redraws the chapters that changed. |
| retake | retake 1 (2.64 s) | nothing | Welcome back. The first thing we do is create a new project from the tutorial preset. |
| self-correction | misspoken 1 (2.04 s: "13.53, sorry,") | nothing | the build ran at 13.35, and it finished in two seconds. |

Whisper writes "The the first" as "The first", so the retake is caught as a restarted sentence, not as a doubled word.

17 min target take, all six rules on: silence 9 removed (723.3 s of 1020 s, the chapters are padded with silence), filler 2 removed (0.27 s) and 2 queued, 4 punch-in zooms `by=jump-cut`. 984 words, 23 speech ranges, no still stretches (the fixture screen never stops moving). Running the rules again on the result changes nothing.

Idle screen on a 12 s 4K clip encoded with VideoToolbox (3 s moving, 6 s frozen, 3 s moving) and 5 s of speech: one still stretch 3.0 to 9.0 s, the silent part removed as `remove from=4_980_000 to=9_000_000 by=idle-screen`. A still predicted frame is 555 bytes at 4K, a moving one about 90 KB; the encoder spends 4.7 KB and 1.9 KB on the two frames after each keyframe refining the picture, which is why stillness is judged per keyframe interval.

## Timings

| What | Number |
| --- | --- |
| 17 min take at recording speed, both detectors | 1021.2 s wall, 136.6 s CPU user, 2.0 GB peak footprint (the whisper model is 1.6 GB) |
| Ready after the last sample | 0.08 s for transcript and speech, 0.08 s for the written rough cut |
| Rough cut from cached analysis, 17 min | 0.01 s, including the scan of `screen.mp4`'s 2040 fragment headers |
| Voice chain, bed, meter and finish over 17 min of audio | 2.7 s per pass (release, `cargo test -p shot-audio --release -- --ignored`); export runs two passes |
| Export of the 30 s project (23.2 s cut, 53 chunks) | 15.3 s |

## Loudness

The 30 s test project is the five speech clips back to back with pink noise at about -50 dBFS mixed in, as AAC, over `screen-30s.mp4` and `camera-30s.mp4`.

| File | Integrated | Peak |
| --- | --- | --- |
| `media/t01/mic.mp4` | -16.6 LUFS | |
| export, voice only | -14.3 LUFS | -1.0 dBFS |
| export with `music "sources/bed.m4a" gain=-18 duck=-30` | -14.3 LUFS | -1.0 dBFS |

The bed, measured as the difference between the two exports, sits at -40 dBFS in the first half second before the voice keys the ducker and at -47 to -50 dBFS under speech.

## Queue decisions

K then C in the Cut screen, `cut.kdl` before and after:

```diff
-        keep from=22_370_000 to=25_880_000
+        keep from=22_370_000 to=24_220_000
+        remove from=24_220_000 to=24_920_000 by=filler
+        keep from=24_920_000 to=25_880_000
-        review from=20_250_000 to=20_570_000 by=filler
-        review from=24_220_000 to=24_920_000 by=filler
+        ignore from=20_250_000 to=20_570_000 by=filler
```

## Choices

- **VAD**: whisper.cpp's ggml Silero model, the one transcription loads, on its own thread fed by the same mic tap. It runs over 30 s blocks while recording; speech reaching into the last 2 s of a block is detected again with the next.
- **Denoise**: not DeepFilterNet. `deep_filter` last released in July 2022, `deep-filter-rt` is a week old and pulls `tract-onnx`. `shot-audio` uses a spectral denoiser (STFT with `realfft`, minimum-statistics noise floor, smoothed Wiener gain with a -15 dB floor), 12.6 dB less stationary noise in its test with the tone level unchanged. The briefing still names DeepFilterNet.
- **Idle screen**: the encoded frame sizes of `screen.mp4`, read from fragment headers, instead of decoding frames. The encoder's own frame diff costs nothing to read and works on a take that is still being written.
- **Pacing**: Loose keeps 0.5 s of silence on each voiced side of a silence or pause cut, Natural 0.25 s, Tight 0.05 s. Changing the pacing moves the edges of existing cuts in place.
- **Punch-ins**: 112% on the kept stretch after every second jump cut (at least 1 s), and on words 6 dB louder than the take's median word until the clause ends (at most one per 10 s). Rerunning replaces only `jump-cut` and `emphasis` zooms.
- **Fillers learned per voice**: each `ignore by=filler` of a phrase anywhere in the cut lowers that phrase's confidence by 20 points; three Keeps and the rule stops asking.
- **Switching a rule off** gives back what it removed and drops its open review items; its `ignore` nodes stay.
- **Captions**: the main cut's export writes `<out>.vtt` next to the MP4, Word pop with per-word timestamps, Clean as plain cues, none for Off. Caption fixes apply with the matched words' timing and keep trailing punctuation. `shot caption-fix --video <dir> <heard> [<said>]` adds, changes or removes a fix.

## Screenshots

1600x1000 in dark, git-ignored, in [m04/](m04/): `cut-queue.png` (the queue on the 30 s project), `cut-after-keep-cut.png` (after K and C).

Against `docs/design.md` and `docs/mockup/Review.dc.html`:

- No "Play around it" button: the Cut screen has no preview yet.
- The list hint reads ↑ ↓ instead of the mockup's J / K, since K keeps (design.md) and the list moves with the arrow keys.
- No Commands (⌘K) in the title bar, as on the other screens.
- Rule rows add "N to review" to the count when a rule has open items.
- The Raw stat is the length of the cut's material, not of every recorded take.
- Recording now lands on Cut instead of Edit; Continue to edit moves on.

## Not done

- **Burned-in captions are not drawn.** `captions::cues` and `captions::shown_at` give the cue and the word to highlight at any output time, but the compose shader has no RGBA overlay layer to draw text into. That layer belongs with the overlays of milestones 06 and 07 (`graph.rs`, `compose.wgsl`); derived cuts do not export before 07 anyway.
- **Misheard name fixes through Claude** are milestone 08's bridge; fixes are filled by hand or with `shot caption-fix`.
- **N markers** (chapter boundaries) are not consumed; A markers feed the retake rule.

## Not verified

- A real voice and room noise: every clip is `say` output. Emphasis zooms never fire on `say`, whose level is flat.
- The app's record path end to end: the recorder now feeds the speech detector and runs the rough cut after appending the take, but no take was recorded in this run (no free mic and display on the shared machine). The Cut screen was driven on a project cut with `shot autocut`.
- The ducker's level with no voice for long: the test cut has no gap longer than 0.1 s.
- Rule switch and pacing clicks in the app; the functions they call are covered by tests.
