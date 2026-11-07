# 0012: Export and preview targets set by measurement

Status: Adopted by Mike on 2026-11-07

## Problem

Milestone 03 measured two targets that the reference machine can't meet for reasons outside our code.

- **Full export.** The M1 Pro's single hardware HEVC encoder caps at about 95 fps at 4K, and more parallel sessions add nothing. The 17 min 4K60 project is 61,200 frames, so about 10 min is the floor. The first export took 591 s ([evidence](../evidence/m03/export.md)).
- **Preview drops.** With our main thread idle, macOS occasionally calls gpui's display link late, about once per 5 minutes of playback. On a mostly idle machine, 4 of 5 runs of 60 s were perfect and one dropped a single frame ([evidence](../evidence/m03/preview.md)).

## Constraints

- The reference machine stays the M1 Pro.
- Re-export after an edit is what Mike waits on most, and its target stays under 20 s.

## Choice

- Full export of the 17 min 4K60 project is **faster than realtime** on the M1 Pro: shorter than the video itself. This was the original bar in the first briefing, and it still beats the 14 min of Christoph Rumpel's editor.
- Preview playback drops **at most one frame per 5 minutes**.

## Rejected alternatives

- 4K30 export by default to keep 5 min: halves the frame rate for every viewer to save waiting time once per video.
- Measuring the 5 min target on Max chips with two encode engines: the reference machine would no longer prove the target.
- Our own Metal layer for the video, presented from the decode thread: more code and an unproven gpui integration for a drop that only shows in preview, never in the export.

## Consequences

The targets table in the [briefing](../technical-briefing.md#targets) and milestone 03's criteria use these numbers. The bench passes when five runs of 60 s drop at most one frame in total. Re-export speed comes from chunked caching, not from encoder throughput.

## Affected contracts

Milestones 03 and 09, the `shot-app --bench` pass rule.
