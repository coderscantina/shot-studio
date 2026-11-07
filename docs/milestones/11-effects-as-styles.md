# Milestone 11: Motion system and effects as styles

## Outcome

Overlays move like a launch film, not like fades: springs with choreography, frosted glass, depth, motion blur and kinetic type, in four presets matched to the formats. A video then looks and sounds branded without placing anything: chapter edges transition with the brand's stinger, wipe or dissolve and a title band, the intro and outro play with an end card for YouTube, the camera is styled, links land with layered sound, punchlines pop, motion lands on the music's beat, and one-off sounds and callouts are a click away.

## Dependencies and ownership

Requires 10. Decisions [0021](../decisions/0021-sound-transitions-and-bookends.md) and [0023](../decisions/0023-motion-system-and-lottie-templates.md). Media owns the motion system, materials, templates, transitions, bumpers, velato and camera styling in `shot-render` and `shot-media`. Audio work (SFX, music cues, beat detection, mix) is in `shot-audio` and `shot-render`'s audio path. App owns the Sound and Overlays lanes, the placement tools and the Lottie import check. Core adds presets, feel controls, Lottie template declarations and the effects to the style language.

Work items 1 to 4 touch only `shot-render` and `sources/manifest.json` and can start before 10 merges.

## Work

1. Motion system: closed-form springs over media time, stagger and phase choreography, the four presets (reactive, editorial, launch, calm) with feel controls (tempo, bounce, density, stagger). Zoom moves on springs.
2. Materials: a gaussian blur pass for frosted glass and soft shadows of any shape, perspective tilt and parallax with one light, adaptive motion blur (1 to 8 samples by on-screen speed) on overlays and zoom moves.
3. Kinetic type: per-word and per-glyph springs from parley's glyph positions, mask reveals, word pop captions that scale and settle.
4. velato patched to vello 0.11 and pinned, the Lottie import check that lists unsupported features, Lottie templates declared in `brand.kdl` with typed slots and hashed by content, text slots drawn by parley.
5. Rebuild the 07 templates (`link-card`, `lower-third`, `highlight`, `b-roll`, `code-diff`) on the motion system, with `enter` and `exit` as duration hints. `blur` and `bleep` keep switching hard.
6. New templates `chapter-band`, `end-card` (YouTube end screen safe areas), `callout` (snaps to clicks and OCR boxes) and `pop` (emoji and text, animated Noto emoji subset, channel stickers). All templates take colors, fonts and re-skins from the brand.
7. Chapter-edge transitions `cut`, `dissolve`, `dip`, brand `stinger`, `mask-wipe` and `zoom-through`, with audio to match, in preview and export. The preview decodes both sides of a chapter edge.
8. Intro and outro: bumper, generated title (with a Lottie logo reveal), both or none, placed before the video or after the hook. The end card draws over the outro or the last seconds.
9. `camera` styling: shape, corners, border, shadow, colors from the brand.
10. Sound: a bundled CC0 set, `brand/sounds/` and the video's `sounds/`. Sounds per motion phase timed to the settle frame, placed `sfx` nodes, music cues with crossfades ducked under speech and bumpers, beat detection with snapping per preset. Mixed without ducking SFX, before the loudness pass.
11. App: the B-roll lane becomes Overlays, a Sound lane joins Music, transitions show as marks on chapter edges. Sounds placed from the transcript toolbar and the Sound lane, callouts drawn in the Picture workspace, Lottie files added in the brand with the import check's findings.
12. Derived cuts keep sound and overlays inside their clips, skip bookends, cards and stingers, and keep hard joins.
13. The reel: per preset, every template, transition and kinetic type moment at 4K60 over the fixture project, for Mike to score.

## Acceptance criteria

- Mike scores each preset's reel 8 or higher, recorded in [design](../design.md).
- Full 4K export of the 17 min fixture with every effect on stays faster than realtime, and the preview holds the frame-drop target across glass, motion blur and a stinger at a chapter edge.
- Any frame renders identically whether reached by playing or by seeking, checked with golden frames at random seeks.
- A Lottie using an AE expression is refused with the expression named; a declared Lottie template fills its slots and re-renders only its chunks when the file changes.
- Exported loudness is -14 LUFS within 0.5 LU with SFX and music in, and snapped entrances land within one frame of a detected beat.
- Changing the channel accent re-renders only chunks where something branded draws.
- A Short of the fixture has no intro, outro, cards or stingers and keeps its sounds.

## Evidence and handoff

Report in `docs/evidence/m11-effects-as-styles.md` with the reels, golden frames, export and preview timings, loudness, cache hit counts and screenshots in `docs/evidence/m11/`. Consumed by 12 and 09.
