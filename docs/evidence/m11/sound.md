# Milestone 11, work item 10: sound

Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while other milestone agents were building on the same machine.

## What was built

All in `crates/shot-audio`, as a library with typed inputs in output samples at 48 kHz. Nothing calls it from export or preview yet: shot-render is being moved onto `cut.lock` by another task.

- **Bundled CC0 set** (`sound.rs`, `assets/sounds/`): 11 effects (`click`, `tick`, `pop`, `pluck`, `glitch`, `impact`, `thud`, `swish`, `swoosh`, `whoosh`, `riser`) and 3 music beds (`bed-calm`, `bed-pulse`, `bed-lofi`), 2.7 MB in all. 48 kHz mono 16-bit FLAC, embedded with `include_bytes!` and decoded by symphonia's FLAC decoder, which is lossless, so decoding is bit-exact. Effects are peak normalized to -3 dBFS, beds to -19 LUFS. Every source, license and edit is in `sources/manifest.json`: Kenney's Interface and Impact Sounds, and six OpenGameArt entries, all CC0.
- **Lookup** (`sound::Library`): `<name>.<ext>` in the video's `sounds/`, then `brand/sounds/`, extensions tried in the order wav, flac, aif, aiff, caf, m4a, mp3, then the bundled set. A file is returned as a path for the caller to decode with `PcmReader`, like the music bed today.
- **Sounds per phase** (`phases::move_sounds`): from the motion system's phase times, a bare key starts with the move, `-travel` and `-impact` land their peak on the impact frame, `-settle` on the settle frame, `-exit` starts with the exit. A sound's peak is its loudest sample inside its loudest 10 ms (`Sound::peak`).
- **Music cues** (`Music`, `MusicCue`, `Play`): a cue plays its track looped until the next cue. Cues cross on equal-power curves over 1 s (`CROSSFADE`) from the next cue's start. `play: None` stops the music with the same fade.
- **Mix** (`Mix`): music ducks under the voice with the existing sidechain (extracted from `MusicBed` into a shared `Ducker`, `MusicBed` behaves as before) and under bumper ranges. Sound effects are added after the ducking and never duck. It streams like the other stages: pieces give the same samples as one call.
- **Beats** (`beats::detect`): spectral flux onset strength, tempo from the autocorrelation at one, two and four beats under a log-normal prior around 120 BPM, Ellis dynamic programming beat tracking, each beat then placed on the sharpest 64-sample energy rise in its window. `Music::snap` moves a time to the nearest beat of the cue playing there within 150 ms, rounded to a frame. `beats::beat_sync` is the per-preset switch: the brand's `beat-sync`, else reactive and editorial snap, launch and calm do not.

## Measurements

| Check | Result |
| --- | --- |
| Click tracks at 90, 120, 128 and 150 BPM, 30 s, offsets 0 to 30 000 samples | every click found, each within one 60 fps frame, tempo within 0.2 BPM |
| Crossfade | first and last sample of the fade exact, curve values within 1e-6 |
| Speech fixtures (all six, 43 s) + two music cues crossing + a sound every 1.5 s at 0 dB + a 2 s bumper | mix at -21.00 LUFS, finished at **-14.22 LUFS** and **-1.86 dBTP** with the existing limiter |
| 17 min mix, 8 cues and a sound every 2 s | 0.32 s per pass |
| Beat detection of an 18 s bed | 13 ms |

Bundled beds: `bed-calm` 100.0 BPM, `bed-pulse` 93.0 BPM (32 beats, exactly its 20.6 s loop), `bed-lofi` 105.4 BPM.

## Not verified

- **Nothing was listened to.** I can't hear. The choice of each effect for its name (is `drop_002` a pop, does the 1.3 s excerpt of the air whoosh read as a whoosh, is the power-up a usable riser), the loop seams of the three beds and whether the beds suit developer videos all need Mike's ears. The `bed-lofi` seam is folded over 0.1 s by hand and is the most likely to be audible.
- **Beat detection on swung music is ambiguous.** The lofi source reads as 106 BPM over its full 97 s but as 158 BPM (a 3:2 grid) in its first two 8-bar windows. The bundled excerpt is bars 17 to 24, which read as 105 BPM. Expect the same on other swung or half-time tracks.
- Export and preview do not call any of this yet, so the exported -14 LUFS with SFX and music is shown on the library path only.

## For the integration

`shot-render`'s audio path needs, in `voice.rs`'s `pass`, a `Mix` in place of the `MusicBed`, built from the lock: `sfx` instances and phase sounds as `SfxCue`s, `music` instances as `MusicCue`s, bumper ranges, all placed in output samples. Beats are detected once per music file and should be cached by the file's hash (decision 0023). Contract question: `music "<file>"` names a path, so a bundled bed has no spelling yet; a bare name looked up like `sfx` would fit.
