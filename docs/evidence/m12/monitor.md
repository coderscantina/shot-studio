# Milestone 12 items 1 and 2: streaming elements and the styled monitor

Branch `task/m12-monitor`. Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while two other milestone agents built and measured on the same machine (load average 5 to 16, the machine swapping), so CPU numbers may read high. Screenshots are in this folder (git-ignored).

## What changed

- **Streaming elements** (`shot_analyze::elements::Stream`, replaces the word-only `Log`). While a take records, the stream takes the words the transcriber settles, the transcriber's tentative words, and follows `media/<take>/events.jsonl` as capture appends to it (complete lines only, a torn last line waits). It keeps `word`, `url`, `click`, `typing` and `marker` elements, made by the same functions as the quick pass, so the same facts give the same elements (a test holds this). The settled elements are written atomically to `elements/<take>.jsonl` on every change, so a crash keeps them; `Stream::live` adds the URLs of the tentative words for the monitor only. The quick pass at stop replaces the file as before.
- **Tentative words** (`Transcriber::with_tentative`). After every decode the transcriber hands the words after the stable ones to a callback, empty once they are all stable. Nothing extra is decoded; it reports what the decode already had. `Transcriber::new` is unchanged.
- **The live take** (`crates/shot-app/src/live.rs`). A thread per take, started with the recorder: while the models load and capture starts it builds the style and the GPU monitor (78 to 454 ms), then every 16.7 ms it feeds the stream, and when the elements changed rebuilds the take's style: the take appended to `cut.kdl` as `folder::appended` writes it at stop (its layout and zoom markers as nodes), and on a migrated video, or a channel video's first take, the lock built with `shot_core::build` from the files and the take's live elements, its cuts left out. It then paints capture's newest preview frame.
- **The monitor** (`shot_render::monitor::Monitor`). A frame composes as the painter composes a segment frame: `compose` with the take's layout, zooms, cursor and keystrokes from the last 3 s of events, and the style's overlays, drawn by the same `Renderer` into `420f` BT.601 buffers at 1920x1080. No cuts, bookends or transitions; a b-roll clip leaves the screen showing; muted. Captions follow the voice as heard: the newest cue shows from when its words arrive until 2.5 s after, the newest word popped in Word pop, drawn as derived cuts burn captions.
- **Record screen.** A Styled and Raw switch sits where the mockup has the layout switcher. Styled shows the monitor's frame at 80% of the stage; Raw shows the screen and camera square as before. If the style cannot be built the screen says why and shows the raw feed, and the elements stream on.
- Small shared edits: `shot_capture::events::marker` parses one log line (`read_markers` uses it), `folder::appended` is the append path the stop already used, `captions::group` and `derived::caption_shape` are `pub(crate)`.

## How it was measured

Capture records a display, not a file, so the real main display (2560x1440) was recorded while ffplay played the 30 s screen fixture (transcoded to 1440p H.264 to keep ffplay's decode light) full screen and looping. The mic was off; a `say` clip, "Today we look at a new code editor. You can find it at github dot com slash zed industries slash zed.", was fed through the mic tap path (transcriber, prompter listener, speech detector) in real time, 10 ms chunks stamped with media time, starting 4 s into the take. Two mouse moves and clicks were posted with CGEvent 3 s and 12 s in. A temporary harness (removed before the commit) started and stopped the take and logged the first monitor frame with the link card. The channel was a scratch repo: `on chapter-start { overlay chapter-band }`, `on url host=github { overlay link-card }`, `on click { zoom 160 hold=1_500_000 }`, format `layouts face-content split`, `captions word-pop`.

## Dropped frames

The take's `screen.mp4` packet times with ffprobe, as in [m03 recordtap](../m03/recordtap.md):

| Run | Monitor | Screen frames | Steps other than 1/60 s | Camera gaps |
| --- | --- | --- | --- | --- |
| 5 min, fixture full screen | styled | 17,938 in 298.95 s | 0 | 0 |
| 11 takes of 25 to 34 s, fixture full screen or in a window | styled | 21,690 (6.0 min) | 0 | 0 to 2 per take |
| 3 takes of 34 s | raw | 6,073 | 0 | 0 to 1 per take |

**Screen: 0 drops in 39,628 frames (11.0 min) of styled recording.** The camera (a USB capture device in a dark room) delivers about 20 fps on its own; a missing camera frame appears in raw takes too, so it is the device, not the monitor.

## URL card latency

From the end of "com" in the clip (3.69 s into it, read off its level envelope, the end of the word "github.com" as the transcriber hears it) to the first monitor frame whose style holds the link card, in media time, 11 takes: **0.98 to 1.84 s, median 1.31 s**. The card shows on the refresh the URL element arrives (the lock rebuild and paint are within one 16.7 ms tick). From the transcriber's own end of the word: 1.25 to 1.54 s.

**The 500 ms target is not met.** The card waits for whisper: it decodes once at least 1 s of new audio is in, and a decode of the padded 30 s window takes about 0.3 to 0.8 s here, so a URL is first seen 0.3 to 1.8 s after it is said. The tentative words already save the second decode a stable word needs (about 1 s more). Getting under 500 ms needs a different hearing path, for example a short-window decode with whisper's `audio_ctx` every 250 ms for the monitor alone, at extra GPU cost while recording. Not built: that is a scope call.

The clip's URL came out as "github.com slash zedindustry slash zed.", which `elements::urls` reads as `https://github.com`, then `github.com/zindustry/z` while the tentative words changed; the card follows. Reading a URL that whisper writes half joined is a gap of the URL reader, not of streaming.

## CPU, app process, share of one core

`top` every 2 s from 4 s into the take. The first samples carry the transcriber decoding the clip (100 to 190%), so the steady state is the mean of the last three samples, 16 to 20 s in:

| Fixture | Raw | Styled | Monitor adds |
| --- | --- | --- | --- |
| full screen (app window covered, gpui does not redraw) | 10.6% | 15.3% | 4.7 points |
| in a window beside the app (app redraws its stage) | 21.3% | 24.4% | 3.1 points |

A monitor frame takes 1.2 to 2.2 ms on the live thread at p50 and 2.8 to 14 ms at p95 (the 5 min run: 17,930 frames, p50 1.22 ms, p95 2.78 ms, max 33.9 ms).

## Screenshots

1600x1000 in dark, `screencapture -l` of the app window while recording, the fixture playing in a window on the recorded display:

- `monitor-band.png`: 0:00, the chapter band "Take t01" from `on chapter-start`.
- `monitor-click-zoom.png`: 0:03, the 160% zoom on the click with the drawn cursor.
- `monitor-captions.png`: 0:06, the newest cue with its newest word popped.
- `monitor-card.png`: 0:11, the link card for github.com with the caption.
- `monitor-zoom-card.png`: 0:12, the second click zoom while the card holds.
- `monitor-raw.png`: the Raw switch, the screen and camera square as before.

The recorded display shows the app itself, so the monitor shows the app inside the fixture's window.

Against the mockup: the switch takes the place of the layout switcher (1 to 4), which is not built; the mockup has no styled monitor or switch, so its look is the existing segmented control. The monitor frame draws the brand background around the layout, so the stage shows a second dark box inside the stage; the mockup's stage shows the screen only. Captions burn over the link card at the bottom left; the main cut ships captions as a sidecar, so export never shows that overlap, but derived cuts would. No "Inline retake caught" note.

## Tests

- `elements::tests::the_stream_writes_what_the_quick_pass_writes_for_the_same_facts_as_they_arrive`: a growing event log with a line written in two parts, words and tentative words; the file never holds the tentative URL; the result equals the quick pass for those kinds.
- `elements::tests::a_crash_keeps_the_streamed_words_and_analysis_writes_the_same_bytes_twice`.
- `transcribe::tests::commits_only_what_two_decodes_agree_on` checks the tentative words.
- `monitor::tests::the_newest_cue_shows_from_its_arrival_until_the_voice_is_quiet`.
- `monitor::tests::a_live_frame_draws_as_the_painter_draws_the_same_moment_of_the_cut` (ignored, GPU and fixtures): the 30 s fixtures in split layout with a zoom and a link card, three moments drawn by the monitor from decoded frames and by the painter from the files: under 0.5 mean difference per channel. Passes.

## Not verified

- A real mic and a real voice: the clip went through the mic tap path, not a device.
- Layout hotkeys (1 to 4) and Z markers during a take: the code path is the stop path's `apply_markers`, not run live.
- The first take of a channel video that is not migrated yet, styled live: built from the inputs like a migrated video, not run.
- A take id that differs from the expected one: the monitor makes its style again, not run.
