# Milestone 12: Styled monitor and watch builds

## Outcome

While you record, the monitor shows the styled video: layouts, zooms on clicks, cards when you say a URL, captions and bands, so you see the video you are making. Outside the app, `shot build --watch` rebuilds every cut on each change and runs `shot check`, and agent proposals build a preview for review.

## Dependencies and ownership

Requires 10, and 11 for the full look. App owns the Record screen, Media the live render path, Analysis the streaming elements, Agents `shot build` and proposal previews in `shot-cli`.

## Work

1. Streaming analysis writes elements during the take, so live rules see clicks, URLs and words as they happen.
2. The Record screen's monitor renders through the same render graph with the style applied: looks, no cuts, sounds muted. A switch shows the raw feed.
3. `shot build --video <dir> [--watch]`: rebuild the lock and the cached chunks of every cut when `cut.kdl`, a style layer, an element file or a take changes, then run `shot check`.
4. Proposal previews: a proposal branch builds its lock and changed chunks in its own cache key space, without touching the video branch, and the app plays it from the proposal card.

## Acceptance criteria

- Recording the screen fixture with the styled monitor on drops at most one frame per 5 minutes in the recorded tracks, and a URL said shows its card within 500 ms on the monitor.
- `shot build --watch` re-exports one changed chapter of the 17 min fixture in under 20 s after a save.
- A proposal preview plays without changing the video branch's lock or cache.

## Evidence and handoff

Report in `docs/evidence/m12-styled-monitor-and-watch-builds.md` with timings, drop counts and screenshots in `docs/evidence/m12/`. Consumed by 09.
