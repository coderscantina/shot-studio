# 0036: One transition type for chapter edges and layout switches

Status: Adopted by Mike on 2026-11-14. Amends [0021](0021-sound-transitions-and-bookends.md), [0023](0023-motion-system-and-lottie-templates.md) and [0027](0027-scene-switches-and-channel-layouts.md).

## Problem

Two systems join pictures. Chapter edges take a `TransitionKind` (cut, dissolve, dip, stinger, mask-wipe, zoom-through) from a style rule, centered on the edge, and play the brand's `transition` sounds. Layout switches take a `SwitchKind` (morph, dissolve, zoom-through, cut) from `cut.kdl` `in` and `out` or the format's `layout-switch`, starting on the edge, with `sound=brand|<name>`. Dissolve and zoom-through exist twice, the painter has two-sided drawing for both, and two known gaps sit in that drawing: zoom-through and morph do not start exactly on the outgoing frame, and the second side of a two-sided transition is not prefetched.

## Choice

- One value in `shot-core`: `Transition { kind, length, sound }`, written `<kind> length=<time> sound=<sound>` wherever a transition is written: a chapter rule's effect, a `layout` range's `in` and `out`, the format's `layout-switch`.
- One kind list: `cut`, `dissolve`, `dip`, `stinger`, `mask-wipe`, `zoom-through` and `morph`. Every kind works on both edges except `morph`, which needs the same take on both sides and is refused at export on a chapter edge.
- One sound syntax on both edges: `sound=brand` plays the brand's `transition` keys by phase, any other name plays that sound at the window's start. A chapter edge without `sound` plays the brand's keys, as today; a layout switch without `sound` plays nothing, as today.
- Where the window sits is a property of the edge, not of the transition: centered on a chapter edge, starting on a layout edge ([0027](0027-scene-switches-and-channel-layouts.md) gives the reason).
- One two-sided path in the painter draws every kind on either edge. At progress 0 it shows exactly the outgoing frame and at 1 exactly the incoming one, and it prefetches the second side.

## Rejected alternatives

- **A shared type, the two kind lists kept apart**: the grammar is shared but a person still learns which kinds work where, and the painter keeps both paths.
- **Centered windows on both edges**: 0027's reasons for starting a switch on its edge hold.

## Consequences

Every existing file reads the same: both old spellings are spellings of the shared type. A layout switch can now dip or play a stinger, and a chapter edge can mask-wipe as before. Chunk keys stay as they are for the kinds that existed on each edge.

## Affected contracts

[Project files](../contracts/project-files.md#transitions-and-bookends), [scene switches](../contracts/project-files.md#scene-switches-and-channel-layouts), `shot-core`, `shot-render`, `shot-app`.
