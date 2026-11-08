# Milestone 12: audio speed for watch builds

The watch criterion failed at 31.4 to 32.8 s because every export spent about 22 s finishing the whole cut's audio, and the join waited for it ([watch.md](watch.md#where-the-time-goes)). This task made the audio finish in seconds, let the join copy the video while it does, and made `shot check` faster and cancellable by a save.

Branch `task/m12-audioperf`. Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while another milestone agent built and measured on the same machine. The disk ran full twice during the runs (another agent's files, then mine); the affected runs are marked.

## Verdict

| Criterion | Result |
| --- | --- |
| `shot build --watch` re-exports one changed chapter of the 17 min fixture in under 20 s after a save | **PASS, 7.8 to 13.5 s** from the write to `built` in 5 of 6 saves, 11.5 s for a save during a check. One save took 65.1 s, of which the build was 9.7 s: the build started 55 s after the write, cause not found (see [below](#the-outlier)) |

## Setup

Two scratch channels, both with the target project's media linked and its transcript and 1751 elements copied from the audio follow-ups' analysed copy:

- **Demo**: the demo channel with the target project as `videos/full-recording-session`, migrated (10 overrides, 27 manual cuts, a lock of 2 instances), as the watch task set it up. Its first build: 1885 and 59 chunks in 589.4 s, then a 42.7 s check.
- **Dense**: the audio follow-ups' channel, with music, beat-synced entrances and 1638 sound effects. It is the worst case for the limiter and the encoded true peak.

## What changed

- **Limiter** (`shot-audio/src/limiter.rs`): the 4x interpolation runs on blocks of 1024 samples, eight outputs at a time with all three phases in NEON registers, and the gain stage keeps its rings in fixed arrays: no allocation per sample. Each value is summed in tap order without fused multiply-adds, so the output is **bit-identical** to the old limiter; a scratch test compared both over 300 000 samples split into uneven pieces. 17 min: 1.8 s became 0.8 s per pass (interpolation 0.23 s, the serial gain stage 0.35 s).
- **EQ** (`eq.rs`): its three biquads run in one loop, so their dependency chains overlap. Same arithmetic, 0.63 s became 0.23 s for 17 min.
- **One mix, fewer passes** (`shot-render/src/voice.rs`): `voice::Mixed` runs the chain and score once into memory and measures it. Trials then only gain, limit and measure those samples, with the loudness meter on a second thread (`Meter::loudness_only`, no true-peak oversampling). The first trial on target is the result, so the old separate final pass is gone. A second encode starts from the gain the first search found. Memory: the export's peak footprint went from 61 MB to 418 MB on the 17 min cut.
- **Codec headroom 1.0 to 1.5 dB** (`loudness.rs`): the limiter holds -2.5 dBTP before AAC. On the dense channel the first encode peaked at -1.08 dBTP with 1.0 dB, over the -1.2 check, so every export encoded twice; with 1.5 dB it peaks at -1.53 and encodes once. Loudness is unchanged (the trials correct for it). **This changes the sound slightly**, see [comparison](#did-the-sound-change); it is the one choice here I would ask Mike about.
- **Join** (`export.rs`): the audio's format goes to the join at once. After each video fragment the join writes room for the audio packets that start before it ends (their count follows from timing alone), sized for the largest packet AAC allows (768 bytes), and fills it in place once the stream is encoded; unused room stays zeros at the end of each `mdat`, about 19 MB (0.3 %) of a 6.4 GB export. So the 6 s copy runs while the audio finishes. ffmpeg and AVFoundation read the result: 57174 video and 44670 audio packets, decoded audio identical to the same stream joined the old way.
- **`shot check`** (`shot-ship/src/media.rs`): frames are sampled on the keyframe at or before each half second, so each sample decodes one frame instead of up to 30; six decoders seek side by side; the audio is measured while the frames are read. `check_cut_until` takes a cancel function.
- **Watch** (`shot-cli/src/build.rs`): a change that matters sets a flag, and the running check stops at its next sample with `check stopped: the files changed`, then the next build starts.

Not done: caching the finished audio per chapter. Loudness is integrated over the whole cut and the chain's state and the music run across chapter edges, so a chapter's finished samples change when any chapter does. A cache of the whole cut's encoded audio by its inputs would help only builds where nothing audible changed; at about 6 s for all the audio it did not seem worth the key's complexity.

## Measurements

### Export with every chunk cached

| Channel | Before | After |
| --- | --- | --- |
| Demo | 32.1 to 35.1 s (audio 26.4 to 29.2 s, then join 5.7 to 5.9 s) | **6.5 to 7.4 s** (audio 5.4 to 6.6 s, then 0.3 to 1.7 s) |
| Dense | 59.3 s (audio 51.4 s with two encodes, join 7.5 s) | **8.7 to 10.0 s** (audio 7.4 to 7.5 s, one encode, then 0.9 to 2.2 s) |
| Demo, the Short | | 0.4 s |

Where the dense channel's audio goes now: mix 1.9 s, three trials of 0.44 s, AAC encode 2.75 to 3.4 s, decode and true-peak check 0.8 s. The encode is now the largest part.

### Watch builds, demo channel

A Python driver starts `shot build --watch`, waits for the first build and its check, then deletes one word in `cut.kdl` by splitting its kept range and timestamps the write and the `built` line. The new binary's first build with nothing changed: 7.7 to 9.9 s (before: 31.7 s).

| Binary | Word | Build | Write to `built` |
| --- | --- | --- | --- |
| before | "video," at 269.66 s | 2 of 1885 chunks, Short 0 of 59, in 29.5 s | 29.81 s |
| before | "review." at 649.98 s, landed during the 43 s check | 32.1 s | 83.86 s |
| before | "Export" at 775.06 s, landed during the check, disk filling | 46.7 s | 101.55 s |
| after | "easy" at 274.46 s | 2 of 1885, 0 of 59, in 7.6 s | **7.84 s** |
| after | "editor" at 650.80 s | 8.7 s | **9.11 s** |
| after | "you." at 813.22 s | 13.1 s | **13.46 s** |
| after | "record." at 355.48 s, during a check: `check stopped` 0.1 s after the write | 8.9 s | **9.12 s** |
| after | "video." at 683.83 s | 9.7 s | 65.07 s, the outlier |
| after | "whole" at 925.34 s | 12.2 s | **12.54 s** |
| after | "where" at 505.21 s, 0.5 s after `built`: `check stopped` 0.47 s after the write | 10.8 s | **11.46 s** |

No save built twice. One more save during a check stopped the check at once and then failed with "No space left on device" while the disk was full; it is left out.

#### The outlier

The 65.07 s save printed no `check stopped`, and its build took 9.7 s, so the watcher saw the change about 55 s after the write: either the file event came late or the check had ended without seeing the flag. It happened while the disk was nearly full and the check itself was slow (below). I did not find the cause and did not reproduce it.

### Loudness and true peak, ffmpeg `ebur128=peak=true`

| Export | Before | After |
| --- | --- | --- |
| Demo, main | -14.0 LUFS, -1.9 dBTP | -14.1 LUFS, -2.3 dBTP |
| Dense, main | -14.1 LUFS, -1.3 dBTP | -14.1 LUFS, -1.5 dBTP |
| Dense, the Short | -14.1 LUFS, -1.3 dBTP | -14.1 LUFS, -1.9 dBTP |

The speech fixtures through `voice::finish` and AAC (`true_peak`, release): -13.91 to -14.24 LUFS, -2.05 to -2.43 dBTP.

### Did the sound change

Both exports decoded by ffmpeg and compared sample by sample (a scratch tool):

- **New code at the old 1.0 dB headroom, demo**: every trial gain, loudness and the encoded peak match the old code to 15 digits, and the decoded audio has the same MD5. The speedup itself changes nothing.
- **Dense channel at 1.0 dB**: correlation 0.99942, level -0.04 dB, every second within -0.11 to 0.00 dB. Here both encode twice, and the second search now starts from the found gain, so it lands on a slightly different gain within the 0.1 LU tolerance.
- **With the 1.5 dB headroom**: demo correlation 0.99912, level -0.04 dB, per second -0.26 to +0.45 dB (5th to 95th percentile -0.20 to +0.12); dense correlation 0.99692, +0.06 dB, per second -0.20 to +0.57 dB. The limiter takes about 0.3 to 0.5 dB more off the loudest moments and the gain lifts the rest to the same loudness.

### `shot check`

| Export | Before | After |
| --- | --- | --- |
| Dense, main, just exported | 43.6 s (frames 41.2 s, audio 0.9 s) | **4.6 s** (frames 3.5 s with the audio alongside) |
| Demo, main | 42.7 s | **40 s** |
| The Short | | 0.2 to 0.3 s |

The demo export stayed slow: sampling showed each seek past about 250 s of the file taking about 150 ms, all of it inside VideoToolbox's decode call, against about 2 ms on the dense export. A bounded reader time range did not help. Why the two files differ I could not find; both have one keyframe per 0.5 s chunk (demo: 1885 keyframes of 600 KB on average). Under watch it no longer matters for the next build, since a save stops the check.

## Checks

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace`: 315 passed, 35 ignored. New: `voice::tests::finishing_again_lower_keeps_the_target_and_lowers_the_peaks`.
- Ignored, run by hand: `shot-ship --test frames` (new: keyframe sampling every half second, cancelling), `shot-render --test export` (now also reads the joined audio back at -14 LUFS within 0.5 LU and under -1 dBTP), `shot-render --test true_peak`, `shot-audio` ignored tests: pass.

## Not verified

- **Listening.** The headroom change is measured, not heard.
- **Other players** with the audio room: ffmpeg and AVFoundation only, not QuickTime Player playback or a YouTube upload.
- **The dense channel under watch**: a one-word save at 4:30 was still rendering after 60 s, so it re-rendered far more than 2 chunks. I suspect the beat-synced entrances (`beat-sync=#true`) move every later card when the cut shortens; not confirmed.
- **The outlier** above, and the demo export's slow seeks.
- One scratch build at the old headroom gave audio that differed slightly from the old code (correlation 1.000000, 60 dB under); a rebuild with logging matched bit for bit, and I could not reproduce the first one.

## For other tasks

- **shot-media**: the frames check needs a way to decode keyframes reading forward, without a new `AVAssetReader` per seek, to be fast on every file.
- **Export disk use**: the join writes `<cut>.partial` next to the old export, so an export needs twice its size free (12.8 GB for the main cut). It ran the disk full twice here.
- The AAC encode (2.75 to 3.4 s) is now the largest part of the audio.
