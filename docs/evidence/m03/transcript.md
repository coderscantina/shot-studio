# M03 evidence: models and streaming transcription

Measured on 2026-11-07 on the reference MacBook Pro (M1 Pro, 32 GB, macOS 27, rustc 1.99.1), release build. Other agents were building and measuring on the same machine at the same time, so the timings are upper bounds of a busy machine, not an idle one.

## What it does

- `shot_analyze::models`: downloads the models of `models/manifest.json` into `~/Library/Application Support/Shot Studio/models` (`$SHOT_MODELS` overrides it), hashing while streaming into `<file>.part`. A partial download resumes with an HTTP range request, or starts over if the server ignores the range. A size or SHA-256 mismatch deletes the download and fails. Progress goes to a callback. `shot models fetch` and `shot models list` sit on top.
- `shot_analyze::Transcriber`: `feed(t, samples, rate)` from the capture thread, never blocking. A worker thread resamples to 16 kHz, runs Silero VAD on the window, decodes windows of up to 28 s with whisper large-v3-turbo on Metal and commits a word once two consecutive decodes agree on it. Stable words go to the `on_word` callback and into a `shot_core::Transcript`. `finish()` decodes the tail and returns the transcript.
- `shot_analyze::cache`: `.shot/cache/transcripts/<take>.json` under the video folder, format in the module doc.
- `shot transcribe --video <dir> --take <id> [--realtime]` decodes `media/<take>/mic.mp4` with symphonia, feeds it through the same streaming path (as fast as possible, or at recording speed with `--realtime`), writes the cache file and prints the words.

## Choices made on evidence

- **VAD is in, using whisper.cpp's own Silero (ggml) model.** Without it the 17 min take came out as 2,347 words for 987 spoken ones: whisper filled the silence between chapters with "Thank you." and a "The app uses the app" loop that the prompt then fed back. With VAD the same take gives 983 words at 1.6 % WER, and silent windows are skipped undecoded: the offline run took 85 s with VAD and DTW against 138 s without VAD (and with the faster flash attention). whisper.cpp's VAD needs no new dependency, so `ort` is not used. The manifest gained a `silero-vad-ggml` entry (885 KB, pinned revision and SHA-256, verified by `shot models fetch`). The ONNX `silero-vad` entry is untouched for milestone 04.
- **Word times come from DTW** on the large-v3-turbo alignment heads, not whisper's token timestamps. The token timestamps put "Now" in `long-silence` at 2.0 s when it is spoken at 9.14 s. DTW needs flash attention off.
- A DTW time lands near the middle of its token, so a token spans halfway to its neighbours, at most 200 ms to each side. Using the DTW time as the start made words start 90 to 320 ms late.

## Word error rate on the speech clips

`cargo test --release -p shot-analyze --test speech -- --ignored --nocapture`, each clip streamed in 10 ms chunks. Normalization: lowercase, hyphens become spaces, then everything but letters, digits and apostrophes is dropped from each word. Numbers are not normalized.

| Clip | WER | Note |
| --- | --- | --- |
| clean | 0.0 % | |
| filler | 0.0 % | "um" and "uh" are kept |
| long-silence | 0.0 % | |
| mid-pause | 0.0 % | |
| retake | 3.7 % | "The the first" comes out as "The first" |
| self-correction | 35.3 % | "thirteen fifty-three" comes out as "13.53": formatting, the words are right |

The test asserts a mean WER of at most 10 % (6.5 % measured). On the 17 min take against the eight narration texts: 1.6 % WER (987 reference words, 986 recognized). The errors are "simple" heard as "for example", three inserted words, "auto cut" as "autocut" twice, "keep" as "key", "grouped" as "grooved", one dropped word, and "ten" and "four k sixty" written as digits.

## Word timing

Checked against the speech onsets in the clips: the first sample above -60 dBFS after at least 300 ms below it, in 10 ms frames. `say` gives no word positions, so only phrase starts after a pause are checked, 8 onsets in 6 clips.

- Word start minus onset: 0, 0, 0, 0, 40, -110, 120, 120 ms. The test asserts at most 200 ms.
- Example, `long-silence`: `That@0.00 was@0.22 the@0.38 setup.@0.66-1.70 Now@9.14-9.45 let's@9.45 ...`, onsets at 0.00 and 9.14 s.

Word ends and starts inside continuous speech are not verified.

## 17 min take

Track `target-project/media/t01/mic.mp4`, 1020 s AAC 48 kHz mono, about 6.5 min of speech.

| Run | Wall time | Real-time factor | Flush (last sample fed to `finish()` returned) | Peak memory |
| --- | --- | --- | --- | --- |
| As fast as possible | 85.2 s | 0.083 | 84.0 s (the whole backlog, by design) | 2.13 GB |
| `--realtime` | 1021.1 s | kept up, 141 s CPU user time over 1020 s | 0.03 s | 1.99 GB |

Peak memory is the `/usr/bin/time -l` peak footprint and includes the 1.6 GB model. The fast run also holds the whole decoded track (about 200 MB of 48 kHz f32) in the queue, since `feed` never blocks.

The realtime run kept up the whole way (980 words, 1.2 % WER) and the transcript was ready 0.03 s after the last sample. That number flatters: the take ends in silence, so the last window had no speech to decode. When speech runs right up to stop, the flush is one or two decodes. Measured with `--realtime` on the clips that end in speech, two runs each:

| Clip | Flush |
| --- | --- |
| clean (5.0 s) | 1.71 s, 1.67 s |
| retake (7.8 s) | 1.96 s, 1.25 s |
| filler (8.9 s) | 1.41 s, 1.34 s |

These are short windows. A full 28 s window costs about the same encoder time (whisper pads every window to 30 s) plus more tokens to decode, so a flush of 2 to 3 s is expected in the worst case. That is inside the 5 s target, but not measured on a long take that ends mid-sentence.

## Not verified

- Word boundaries inside continuous speech, see above.
- Real microphone audio: every clip is `say` output with digital silence. VAD thresholds are whisper.cpp's defaults (0.5).
- Resume against Hugging Face itself: the resume path is tested against a local server that honors `Range`. The real downloads ran uninterrupted.
