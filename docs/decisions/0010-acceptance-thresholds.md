# 0010: Acceptance thresholds beyond the targets

Status: Adopted by Mike on 2026-11-07

## Problem

Some milestone briefs need numbers the agreed [targets](../technical-briefing.md#targets) don't set.

## Constraints

- Each threshold must be measurable on the reference machine with generated fixtures.

## Choice

- A seek in the 4K60 preview shows a frame in under 100 ms ([milestone 02](../milestones/02-preview-spike.md)).
- Voice-follow lag stays under 300 ms at the median ([milestone 05](../milestones/05-record-comfort.md)).
- Exported loudness is within 0.5 LU of -14 LUFS integrated ([milestone 04](../milestones/04-auto-cut-and-finish.md)).

## Rejected alternatives

- No numbers: the briefs would be untestable.

## Consequences

They are part of the [targets table](../technical-briefing.md#targets).

## Affected contracts

Milestones 02, 04 and 05.
