# Milestone 11: audio follow-ups

The three audio gaps the [milestone 11 report](../m11-effects-as-styles.md) left open: the true peak over -1 dBTP after AAC, phase sounds on cards, zooms, pops and clicks, and the beat cache on disk. Plus one the painter task found: the preview had no bumper, transition or brand sounds.

Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while two other milestone agents built and tested on the same machine (load average 4 to 6).

## Setup

A scratch channel repo with the target project as `videos/target` (media symlinked), its transcript and 1751 elements copied from the painter task's analysed copy (983 words, 510 clicks, 170 typing spans), migrated, then this brand and `shot style build` (1133 instances):

- `motion opinion=launch` with `beat-sync=#true`, `zoom-motion smooth`, `cursor ripple=#true`
- intro (3 s video bumper with a tone, plus the generated title) after the hook, outro (4 s bumper with a tone) under a 12 s end card, circle camera with border and shadow, the logo
- `on video-start { transition dip; music "sources/bed.flac" }` (the bundled `bed-pulse`), `on chapter-start { overlay chapter-band; sfx swoosh; transition dissolve }`, `on chapter-end { overlay pop }`, `on click { overlay callout; sfx tick }`, `on typing { zoom 140 }`, the fixture's url, OCR, b-roll and lower-third rules
- `sounds zoom=whoosh card-travel=whoosh card-settle=tick click=click transition=swoosh transition-impact=impact pop-settle=pop`

It differs from the m11 recheck in two ways: the ripple is on (so clicks can sound) and the chapter-end pop is the 🔥 emoji instead of a brand Lottie sticker.

## True peak

**Nothing gets past the limiter.** Bumper audio, transition and lock sounds are all `SfxCue`s in the `Mix`, which runs before `Finish`, and the finished samples measured -2.0 dBTP before encoding, the limiter's -1 dBTP ceiling less its 1 dB codec headroom. The overshoot is the AAC encoder's. Decoding the encoded stream and comparing it with the encoder's input found about 115 moments of 100 ms where the input sat at -2.0 dBFS (the limiter holding a loud sound effect) and the decoded output reached -1.6 to -1.2 dBFS. Hard-limited transients, many of them, are what AAC at 128 kb/s overshoots most: up to 1.3 dB, more than the fixed headroom. The m11 recheck's -0.7 dBTP was the same thing with a sticker and denser effects.

**Fix**: export now measures what it encoded. `encode_audio` keeps the AAC stream in memory, decodes it (an ADTS scratch file in the video's `.shot/cache/`, read by the system decoder, then removed) and meters its true peak. Over -1.2 dBTP by the in-repo meter (the ceiling less 0.2 dB for other meters and decoders), it finishes the audio again with the limiter lower by the overshoot plus 0.1 dB, at most 3 encodes, then sends the stream to the join. Loudness is corrected on every attempt as before. A retry costs one more set of audio passes, about 25 s on the 17 min cut, on the audio thread while video renders. With every chunk cached the export took 62 s, 55 s of it waiting for the audio.

| Export | ffmpeg `ebur128` | in-repo meter |
| --- | --- | --- |
| before (main, no phase sounds) | -14.3 LUFS, -1.2 dBTP | -14.26 LUFS, -1.19 dBTP |
| after, the audio alone with phase sounds and no check | -14.1 LUFS, -1.1 dBTP | -14.06 LUFS, -1.06 dBTP |
| after, the export | **-14.1 LUFS, -1.4 dBTP** | -14.07 LUFS, -1.37 dBTP |
| after, the export with a custom settle sound | **-14.1 LUFS, -1.3 dBTP** | -14.07 LUFS, -1.28 dBTP |

The second row is the audio path run on its own (an uncommitted harness), the case the check exists for: adding the phase sounds put 1638 sound effects in the mix and the encode at the ceiling. The two meters agree within 0.05 dB on every file. `true_peak` (the speech fixtures through AAC) still passes.

## Phase sounds

`sounds::automatic` (new, pure) places the brand's `sounds` keys on every move of the plan: transitions as before, then each overlay as a card or a pop (blur, bleep and b-roll have no sound), each zoom, and each click where a ripple draws (`cursor ripple=#true` and a layout showing the screen). Phase times come from the motion system: `Motion::times` gives an element's start, impact, settle and exit on its springs (first part), `zoom::phases` a zoom's start, rest and way out as `punch_at` moves it. Each phase lands on the first frame drawn at or after it, in take time, so it plays wherever the cut shows that frame and nowhere the cut leaves it out; beat-snapped entrances carry their sounds with them. Export and the preview build the score with the same function.

On the scratch channel the score has 1638 sound effects. To find the settle sounds in the export, `card-settle` was pointed at a 2 ms 9 kHz burst in `brand/sounds/settle.wav` and each burst located by a matched filter within 50 ms of where the plan says the card settles: **all 475 (466 callouts, 8 chapter bands, 1 lower third) on their exact sample**, so within one frame. The same search 250 ms off the settle frames found 1 of 475 within a millisecond, so the match is not the filter finding anything anywhere.

## Beat cache

Beats are cached as `.shot/cache/beats/<content hash>-<detector version>.txt` in the channel repo (in the video folder outside a channel), text: `bpm <tempo>` and one beat sample per line. A file with the same content anywhere in the channel reuses them; `beats::DETECTOR` goes into the name so a detector change cannot read old results. The in-process cache by path, size and modification time stays in front, so a preview replanning does not hash or decode again.

- The first export wrote `8105ef5b…-1.txt` at 06:44:04; the second export, a new process 20 minutes later after a brand change, read it and left it untouched.
- Timed through `preview_plan` in a fresh process: 0.50 s with detection, 0.47 s reading the cache. For the 20.6 s bundled bed detection is about 30 ms; it grows with the track, about 0.2 s for a 3 min song. The file still has to be decoded for the mix, which the cache does not save.

## Preview audio

The painter task moved the preview onto the plan's voice (`TimelineAudio::of_plan`), but it still placed its own score from the `EditMap`: no bumper sound, no transition or phase sounds, no music ducking under the bumpers. Now both use shot-render's builders: `export::plan_audio(dir, plan)` for the voice with its bleeps, `export::preview_score(dir, project, plan)` for the `voice::Score` export mixes, and `Score::mix_from(at)` to start that mix anywhere (`Mix::starting_at` in shot-audio). The app's `Cues`, `Sounds` and own `Score` are gone; its chain, preroll, limiter and gain pass are unchanged. The app's mix test now covers a bumper range too.

## Checks

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace`: 310 passed. New: `sounds::tests::each_phase_plays_on_the_first_frame_that_shows_it_where_the_cut_keeps_it`, `export::tests::beats_are_detected_once_per_file_content`, `beats::tests::beats_read_back_exactly_from_their_text_and_nothing_else_reads`, `mix::tests::a_mix_started_late_plays_what_the_whole_mix_plays_there`; the app's score test rewritten on `voice::Score`.
- `true_peak` (ignored, release): passes.

## Not verified

- **Nothing was listened to.** Whether a whoosh on every callout and a click on every ripple is too much, and whether the settle sounds land musically, need Mike's ears. The phase sounds are measured, not heard.
- **The preview's sound in the running app.** The app compiles and its tests pass; I did not run the Edit screen or its bench, so the bumper, transition and phase sounds in the preview are unheard and unmeasured there.
- **A retry beyond the second encode.** On this channel one lower pass was enough; three is a cap, not a measured need.
- Phase sounds on bookends (the generated title's logo pop) and on word-pop captions: not wired; no key names them.
- A video stinger or Lottie bumper's own sound: Lottie has none, and video stingers are not supported.

## For other tasks

- **Contract** (`docs/contracts/project-files.md`, the `sounds` row of `brand.kdl`): say which templates are cards and pops, that a click sounds only where a ripple draws, and that each phase plays on the first frame showing it.
- **Lanes**: [lanes](lanes.md) says the renderer computes the automatic sounds; it now does. The Sound lane could show them from `preview_score`.
