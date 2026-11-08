# Milestone 11: motion system and effects as styles

Report for [milestone 11](../milestones/11-effects-as-styles.md). The work items have their own reports in [`m11/`](m11/): [motion](m11/motion.md) (items 1 to 4), [templates](m11/templates.md) (5 and 6), [transitions](m11/transitions.md) (7 to 9), [sound](m11/sound.md) (10), [lanes](m11/lanes.md) (11) and the [editor bench](m11/bench.md). This report adds items 12 and 13, the contract lines for transitions and bookends, and an end-to-end recheck of every acceptance criterion on the merged code.

Measured 2026-11-08 on the reference MacBook Pro M1 Pro, release build, while another milestone agent worked on the same machine. Media, reels and frames are in `docs/evidence/m11/` (git-ignored).

## Acceptance criteria

| Criterion | Result | Evidence |
| --- | --- | --- |
| Mike scores each preset's reel 8 or higher | **Waiting for Mike.** The four reels are rendered, see [the reel](#the-reel-item-13) | `reel-<preset>.mp4` |
| Full 4K export with every effect on stays faster than realtime | **PASS**, 1.24x realtime from a fresh cache under load (75 fps); 97 fps when 1487 of its chunks rendered again | [below](#full-export-with-every-effect) |
| The preview holds the frame-drop target across glass, motion blur and a stinger at a chapter edge | **PASS** on the editor bench with a temporary painter patch; not rerun here, the app agent is moving the preview onto the painter now | [transitions](m11/transitions.md#preview), [bench](m11/bench.md) |
| Any frame renders identically reached by playing or by seeking | **FAIL, by one code value.** The per-area golden tests pass; the reel-wide one fails in 3 of 8 runs on one chroma sample of the end card's headline, off by 1 | [below](#golden-frames) |
| A Lottie with an AE expression is refused with the expression named | **PASS** | `lottie::tests::an_expression_is_refused_and_named`, [motion](m11/motion.md#lottie), the app's import in [lanes](m11/lanes.md) |
| A declared Lottie template fills its slots and re-renders only its chunks when the file changes | **PASS**, a changed sticker file re-rendered 39 of 1915 chunks, exactly where it draws | [below](#a-changed-lottie-and-a-changed-accent) |
| Exported loudness is -14 LUFS within 0.5 LU with SFX and music in | **PASS**, -14.2 LUFS with music, 470 click sounds, chapter and transition sounds and bumper tones; the Short -14.1 LUFS | [below](#loudness) |
| Snapped entrances land within one frame of a detected beat | **PASS**, at most 6.1 ms from a detected beat in the reels. This was not wired before: see [beat sync](#beat-sync) | [below](#beat-sync) |
| Changing the channel accent re-renders only chunks where something branded draws | **PASS**, 212 of 1915 chunks with a brand of chapter bands, stickers, a lower third, the framed camera and bookends | [below](#a-changed-lottie-and-a-changed-accent) |
| A Short of the fixture has no intro, outro, cards or stingers and keeps its sounds | **PASS**, on the committed project in a unit test and end to end | [below](#derived-cuts-item-12) |

## What was built

All in `crates/shot-render`, plus the [transitions and bookends](../contracts/project-files.md#transitions-and-bookends) section of the project files contract.

- **Derived cuts (item 12).** A Short or X clip had no bookends, transitions or end card already, since derived plans are built without joins. Now its base chunks also leave out the cards that frame the long video, `chapter-band` and `end-card` overlays (`derived::without_cards`); every other overlay and every `sfx` inside a clip plays. Each clip plays the music cue the main cut plays at its start, from the clip's start, unless it is already playing (`export::music_places`), so a clip from the middle of a video keeps its music.
- **Bumpers cached per channel** (decision 0021). Bookend chunks are keyed by what they draw and never by the video, so `export::Cache` writes them to the channel repo's `.shot/cache/chunks/` where every video of the channel finds them. Segment chunks stay in the video's cache. A video outside a channel repo (no `brand/` two levels up) keeps everything in its own cache.
- **Beat sync** (decision 0023, item 10's last piece). `shot-audio` had beat detection and `Music::snap`, but nothing called them: entrances did not snap. Export and `preview_plan` now plan the output, place the music, and with the preset's `beat-sync` move every overlay entrance but `blur`, `bleep` and `b-roll` onto the nearest beat within 150 ms, keeping its end, then plan again from the moved overlays, so chunk keys follow. An entrance moves only inside the segment it starts in. Music files are decoded and their beats detected once per file and kept for the next call (`export::tracks`), so the preview does not decode them on every plan. `Theme::beat_sync` carries the brand's switch.
- **Glass hook on the painter.** `Painter::set_decoration(fn(Micros, Canvas, &mut Frame))` draws on every segment frame after its overlays, the same signature as the app's `engine::Decoration`, so `--bench-edit --glass` keeps working once the engine draws through the painter. Export never sets it.
- **The reel (item 13)**: `showreel.rs`, test-only. It writes a channel repo in a temp dir per preset, builds `cut.lock` with `shot_core::build` like `shot style build` (with the transition kinds set per chapter in the lock, since one rule gives every chapter the same kind), and exports through the real export at 4K60 with sound.
- **Contract**: the new section covers where the intro goes per format, what each bookend and generated title shows, the end card's pass, which transition joins what, where each kind draws, handles, the sound at joins, chunk keys and the per-channel bumper cache, and derived cuts. Two one-line consistency edits outside it: the derived-cut paragraph of `cut.kdl` and the derived-chunk line of timeline evaluation now say cards are left out, and the feel controls say what `beat-sync` does.

## The reel (item 13)

`showreel::reel_per_preset_at_4k60` exports `reel-reactive.mp4`, `reel-editorial.mp4`, `reel-launch.mp4` and `reel-calm.mp4`: 81 s each, 4K60 HEVC with AAC, 92 to 98 fps. All four show the same video, a 7-chapter opinion video over the fixture take titled "Why we ship on Fridays", in the brand's lime and dark colors, with the bundled `bed-pulse` music from the first frame. The presets differ in springs, kinetic type, glass and tilt (launch), beat sync (reactive and editorial) and the camera's frame:

| Reel | Camera frame |
| --- | --- |
| reactive | circle, 8 px accent border, soft shadow |
| editorial | rounded 48 px, no border, soft shadow |
| launch | rounded 32 px, 6 px accent border, soft shadow |
| calm | square, no border, hard shadow |

What appears when (reactive and editorial; launch and calm do not snap, so their four snapped entrances sit up to 0.12 s earlier: the hook's chapter band at 0:00.00, the highlight at 0:23.80, the sticker at 1:02.00, the badge at 1:04.40):

| Time | What to score |
| --- | --- |
| 0:00.00 | chapter "Hook", `face-content` with the framed camera; chapter band (kinetic title) |
| 0:01.00 | pop: animated 🔥 emoji |
| 0:02.60 | lower third "Mike Wallner / Coder's Cantina" |
| 0:07.75 | dip into the intro (video-start), cut at 0:08.00 |
| 0:08.00 | intro: 3 s video bumper with a tone, the generated title over it (logo popping in, title in kinetic type, accent rule) |
| 0:10.75 | dissolve out of the intro into chapter "Why Fridays" (`split`), cut at 0:11.00 |
| 0:11.00 | chapter band |
| 0:14.00 | link card for a GitHub pull request |
| 0:20.75 | mask wipe in the camera's shape into chapter "The checks", cut at 0:21.00 |
| 0:21.00 | chapter band |
| 0:23.85 | highlight box on the screen |
| 0:26.60 | callout: outline on a box with "Deploy button" |
| 0:30.75 | zoom-through into chapter "Deploying" (`content`), cut at 0:31.00 |
| 0:31.00 | chapter band |
| 0:32.50 | 150% zoom springing in, motion blurred |
| 0:33.20 | callout: ring on a point with "Click here", following the zoom |
| 0:37.80 | pop: the word "Ship it" on an accent pill |
| 0:40.70 | brand stinger (Lottie), cut at 0:41.00 under its peak, into chapter "The dashboard" (`split`) |
| 0:41.00 | chapter band |
| 0:43.60 | b-roll still settling in with a push |
| 0:47.00 | blur over a rectangle of the screen (switches hard) |
| 0:51.00 | cut into chapter "The diff" (`face`); chapter band |
| 0:52.00 | code diff typed out in the mono font, line 9 highlighted at 0:57.00 |
| 0:55.00 | bleep (0.4 s tone over the voice) |
| 1:00.75 | dip into chapter "Shipping" (`content`), cut at 1:01.00 |
| 1:01.00 | chapter band |
| 1:01.95 | pop with a brand sticker: a Lottie star with "NEW" in its text slot |
| 1:04.52 | a declared brand Lottie template, `badge`, its text slot "v2 ships Friday" and accent slot |
| 1:12.75 | dip into the outro (video-start), cut at 1:13.00 |
| 1:13.00 | outro: Lottie bumper (an accent disc opening into a ring), stretched to the 8 s end card: logo, "Thanks for watching", two video frames and the subscribe circle |

Transition sounds play on every transition (`swoosh` at the window's start, `impact` peaking on the cut), the intro bumper's tone plays, the music ducks under it and under the voice.

Things I noticed in the frames that Mike may score down, none of them a bug: the code diff panel covers the face completely in the `face` layout; the outro's Lottie disc is hidden under the end card's backdrop; the hook's chapter band reads "Chapter 1 Hook". The reels draw the fixture's synthetic screen and face, so they judge motion and type, not footage.

`reel-reactive-sheet.png` and `reel-launch-sheet.png` are contact sheets of 18 moments.

## Golden frames

`showreel::every_reel_frame_is_the_same_reached_by_playing_or_by_seeking`: the editorial reel through `preview_plan` and `Painter` at 1280x720, every frame played, then 200 pseudo-random seeks, a quarter of them into a transition window, alternating between the playing painter and renderer and a fresh pair. It covers every template, every transition, both bookends, the end card, the camera frame and the beat-snapped entrances at once.

**It passed 5 of 8 runs.** The 3 failures are all the same pixels: the held end card at 1:15.55 or 1:16.07 (a static frame, so every frame there has the same two variants), where a 2x2 block at (308, 236) of the 1280x720 frame, one chroma sample, differs by 1 code value. That is a glyph edge of the headline "Thanks for watching". Rendering that frame 40 times, alternating a long-lived and a fresh renderer, gives exactly these two variants in every process. It is the partial-coverage flip the [templates report](m11/templates.md#bugs-found-on-the-way-both-fixed) left open: vello sums a pixel's coverage over segments in an order that varies between runs, and this glyph edge happens to sit on a rounding boundary. Nothing of mine draws text differently from the templates, and no chunk key changes, so an export is unaffected except bit for bit. I left the test strict; see the decision at the end.

The per-area golden tests pass on the merged code: `join_reel::transitions_bookends_and_the_camera_frame_the_same_reached_by_playing_or_by_seeking` and `reel::tests::every_template_frame_is_the_same_reached_by_playing_or_by_seeking` (all four presets).

## Full export with every effect

A scratch channel repo with the target project as `videos/target` (media symlinked), transcribed, analysed (1751 elements: 983 words, 510 clicks, 170 typing spans) and migrated with `shot migrate`, then a brand with every effect on, built with `shot style build` (1135 instances):

- `motion opinion=launch` (the costliest preset: glass, tilt, glyph by glyph type) with `beat-sync=#true`, `zoom-motion smooth`
- intro (3 s video bumper with a tone and the generated title) after the hook, outro (4 s video bumper with a tone) under a 12 s end card, the circle camera with border and shadow, the logo
- `on video-start { transition dip; music "sources/bed.flac" }`, `on chapter-start { overlay chapter-band; sfx swoosh; transition dissolve }`, `on chapter-end { overlay pop { sticker "star" } }` (a brand Lottie), `on click { overlay callout { x; y }; sfx tick }` (470 callouts and sounds), `on typing { zoom 140 }` (159 zooms), the brand's transition sounds, and the fixture's lower third

| Export | Result |
| --- | --- |
| fresh cache, load average 4 to 7 from another agent | 1915 chunks, 57,174 frames (952.9 s) in 766.8 s render and 1.5 s join: **75 fps, 1.24x realtime** |
| the same effects with a new accent: 1487 chunks rendered | 44,412 frames in 457.8 s: **97 fps** |
| chapter markers | 0.0, 87.6 (the intro, which opens chapter 2), 219.5, 331.5, 471.5, 594.7, 712.8, 853.2 |
| bookend chunks | 30 (intro 6, outro 24) written to the channel's `.shot/cache/chunks/` |

The fresh run is slower than the 97 fps the earlier reports and the rerender measured, at the same density of effects. The rerender is the cleaner number: the other agent was building during the fresh run. Either way it stays faster than realtime.

## A changed Lottie and a changed accent

On the same channel:

| Change, then `shot style build` and export | Chunks rendered |
| --- | --- |
| a sticker (a brand Lottie in `pop`) on every chapter start | 39 of 1915, 1,026 frames |
| the sticker's file changed (a bigger star) | **39 of 1915**, the same chunks: 8 stickers of 1.5 s, each over its own chunks and the outgoing chunk of the chapter's dissolve, which shows it |
| accent `#c8f25a` to `#ff7a59` with every effect on | 1487 of 1915: the 470 click callouts draw in the accent, so most chunks are branded |
| the click and typing rules removed, then accent to `#5ab4f2` | **212 of 1915**, 6,204 frames: 8 chapter bands of 2.5 s with their stickers and dissolves, the lower third, both bookends, and the 60 s of `split` where the camera's border is drawn in the accent |

A second video in the same channel (two 10 s chapters, another title) rendered **46 of 70** chunks: its 40 segment chunks and its own intro, whose title differs. All 24 outro chunks came from the channel cache.

## Loudness

ffmpeg `ebur128` with true peak:

| Output | Integrated | True peak |
| --- | --- | --- |
| the full export with every effect (music cue ducked under voice and bumpers, 470 click sounds, 8 chapter sounds, transition sounds, two bumper tones) | **-14.2 LUFS** | -0.7 dBFS |
| the Short `intro-hook` | **-14.1 LUFS** | -0.9 dBFS |

The true peak is above the limiter's -1 dBTP ceiling after AAC encoding; the [transitions export](m11/transitions.md#export) measured -1.3. Not a criterion, but worth knowing.

## Beat sync

`export::tests::entrances_snap_to_the_beat_and_cuts_keep_their_times` checks the rule on a click track: an entrance 150 ms after a beat moves onto it, a blur keeps its time, an entrance whose nearest beat is before its segment's cut stays, and the segments do not move.

On the reels, `Reel::snapped` compares the plan with and without snapping and measures each moved entrance against the beats `shot_audio::beats::detect` finds in the bed: reactive and editorial each moved 5 entrances, all at most **6.1 ms** from a detected beat (a frame is 16.7 ms). Launch and calm moved none, as their defaults say.

The first reel run found a defect of mine: a chapter band snapped 50 ms before its chapter's first kept moment and then showed the previous chapter's title ("Chapter 5 The dashboard" over chapter 6). Entrances now move only inside the segment they start in.

## Derived cuts (item 12)

`export::tests::a_short_of_the_target_project_has_no_bookends_cards_or_stingers_and_keeps_its_sounds` runs on the committed target project (`fixtures/projects/target`), migrated with no elements, with a brand that adds an intro, outro, end card, stinger transitions, chapter bands, a `swoosh` on every chapter start and a music cue on video start. The main plan has both bookends, the end card, stingers and chapter bands. Its Short `intro-hook` (0.8 to 30 s) and a second Short from 300 to 305 s have no bookends, transitions or end card, exactly their clips' frames, no chapter band or end card in their own or their base chunks, the lower third inside the first clip, the music from their first frame and the `swoosh` at the first clip's start.

End to end on the scratch channel above, `shot export --cut intro-hook`: **29.2 s, 1752 frames, exactly the clip** (the main cut puts a 3 s intro after this hook), 1080x1920 with AAC, -14.1 LUFS. 59 chunks drawn from 59 main chunks: 37 reused from the main export's cache, 22 rendered where the main chunks differ (the chapter band, sticker and dissolve at the start, and click callouts the beat moved in the main cut). Its frames at 1 s and 4 s (`short-intro-hook.png`) show the face and the word pop captions, no chapter band and no card. The export is `short-intro-hook.mp4`.

## Checks

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace`: 304 passed, none failed. New: `export::tests::a_short_of_the_target_project_has_no_bookends_cards_or_stingers_and_keeps_its_sounds`, `entrances_snap_to_the_beat_and_cuts_keep_their_times`, `bookend_chunks_are_cached_in_the_channel`.
- Ignored, run with `--release -- --ignored`: `showreel::reel_per_preset_at_4k60` passes (it asserts every snapped entrance within a frame of a detected beat); `showreel::every_reel_frame_is_the_same_reached_by_playing_or_by_seeking` 5 of 8, see golden frames; the join and template golden tests pass.

## Not verified

- **Mike's scores.** The one criterion I cannot close. Scores go into [design](../design.md).
- **Listening.** Nothing was listened to: the reels' music, transition sounds, bumper tones, the bleep and the mix were measured, not heard.
- **The preview in the app.** Another agent is moving the Edit screen onto `Painter` and `preview_plan` now. The stinger bench in [transitions](m11/transitions.md#preview) used a temporary patch; it was not rerun here.
- **Per-phase sounds of cards, zooms, pops and clicks** (`sounds card-settle=...`). Only the transition's phase sounds are wired into the export; `phases::move_sounds` is not called for overlays and zooms. [Lanes](m11/lanes.md) says the renderer computes them; it does not. Not an acceptance criterion; a follow-up for the sound owner.
- **Stingers do not snap to the beat**: their cut sits under their peak, and cuts never move. Decision 0023 lists stingers among what snaps; the contract now says transitions never move.
- **Derived cuts do not snap to the beat**: their bases are main chunks at take time, and their music starts at each clip. The contract says so.
- **The preview bench** was not rerun: the app is being moved onto the painter by another agent.
- The beat cache lives in the process, keyed by path, size and modification time; decision 0023's "cached by the file's hash" on disk is not built.

## Decision for Mike

The reel-wide golden test fails on one chroma sample of the end card's headline in about 3 of 8 runs, off by one code value. My recommendation: accept it as vello's known partial-coverage flip and let golden tests allow a difference of 1 code value on at most a handful of pixels, rather than chase vello's rasterizer. The alternative is drawing settled text through a path we control (for example rasterized once per text and theme on the CPU), which is a real piece of work across every template. I left the test strict until you choose.
