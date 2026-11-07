# 0021: Sound effects, music cues, chapter transitions and bookends

Status: Proposed. Settled with Mike in the effects grill on 2026-11-07. Amended by [0023](0023-motion-system-and-lottie-templates.md): sounds per motion phase instead of one per effect kind, and the stinger, mask wipe and zoom-through join the chapter-edge transitions.

## Problem

The briefing kept transitions out ("a library of transitions") and had no sound effects, intro, outro or end card, and only one music bed per video. Videos on the channel need structure at chapter edges, a sonic identity, YouTube's end screen and music that changes with the video.

## Constraints

- Jump cuts stay hard.
- Audio is not cached, so sound never forces a video re-render.
- Times are take media time, so an edit elsewhere never shifts anything.
- No private assets in the app repo.

## Choice

- **Transitions** only where a chapter starts: `cut`, `dissolve` or `dip`, 0.5 s by default, audio crossfading or dipping with the picture. Joins to bumpers use the same transition. Clip joins in Shorts and X clips stay hard.
- **SFX**, automatic and placed. Automatic sounds play on effect kinds (zoom in, card entering, click with a ripple, transition, pop): the brand picks the sound, the format turns each kind on. Placed sounds are `sfx` nodes at a take time, added from the transcript toolbar, the Sound lane or a Claude proposal. Sounds come from a bundled CC0 set, the channel's `brand/sounds/` and the video's `sounds/`, looked up in reverse order. SFX are not ducked; loudness is measured after the full mix.
- **Music cues** replace the single bed: a cue starts a track at a take time and plays until the next cue or a stop, crossing chapters, with a 1 s crossfade by default. Music drops under the bumpers.
- **Intro and outro**: a bumper file, a generated title (its own segment of about 3 s on the brand background, drawn over the bumper when both are on), both or none. The format places the intro before the video or after the hook, the cold open being the default for opinion and reaction.
- **Chapter title band** over the first 2 to 3 s of each chapter, adding no time. **End card** with safe areas for YouTube's end screen over the outro, or over the last seconds when there is none. The outro is at least as long as the end card.
- Derived cuts keep SFX, music and the overlays inside their clips, and skip bookends and cards.

## Rejected alternatives

- **Transitions at every join, or a library**: touches the "a segment is one take" model at every join for a look jump cuts reject anyway.
- **Full-frame chapter cards in a dip**: slower pacing, about 1.5 s per chapter.
- **Music per chapter or in output time**: too coarse, or shifted by every cut before it.
- **Synthesized sounds only, or channel files only**: Mike chose a real CC0 set plus his own files.

## Consequences

A chunk at a chapter edge hashes both sides of the transition, the only place two takes share a frame. The preview decodes both sides at a chapter edge. Bumpers render once per file and are reused by every video.

## Affected contracts

[Semantic video draft](../contracts/semantic-video.md), `shot-render`, `shot-audio`, `shot-media`, `shot-app`.
