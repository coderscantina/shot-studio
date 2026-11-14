# 0035: Every video is on style rules, migrated when it opens

Status: Adopted by Mike on 2026-11-14.

## Problem

Since milestone 10 a video is either migrated (`cut.lock`, the brand, style rules) or written before the lock (rule removals, review items and rule zooms in `cut.kdl`, `auto-cut` and `auto-zoom` in `format.kdl`). Both kinds go through the app, `shot` and the renderer. The split costs code in every crate and leaves gaps: an unmigrated video draws no brand, no channel layouts and no bookends, sounds can only be placed in a migrated video, and the app folds the lock back into the old take-block shape so its screens can show one cut. Nothing is released, so no one outside Mike has a video of the old kind.

## Choice

- There is one kind of video: on style rules, with a lock. The app, `shot` and the MCP server migrate a video written before the lock when they open it, through `shot_core::migrate`, before anything else reads it. A migration that would change the frames refuses as today and the video does not open, with the reason.
- In a channel the migration lands as its own commit on the video's branch ([0037](0037-a-worktree-per-video-branch.md)), so it reads apart from a person's edits.
- A video folder outside a channel repo is a channel of one: the shipped format preset is its channel layer, the app's default brand its brand. It gets a lock like any other video and its edits rebuild it.
- The old grammar (`remove by=<rule>`, `review`, `ignore`, rule zooms, `auto-cut`, `auto-zoom`, `pacing`) is read only inside `shot_core::migrate`. Everywhere else it is refused with a message that says to open or migrate the video.
- Commands that only worked on unmigrated videos go: what they did is a style rule now. `shot migrate` stays for a folder of videos at once.
- The app's screens read the lock directly instead of a cut folded back into the old shape.

## Rejected alternatives

- **Refuse old videos and ask for `shot migrate`**: the same deletion, and one more step for a person who opens an old video.
- **Keep both paths**: every new feature pays for the split again.

## Consequences

Opening an old video writes files once. Old fixtures and test files in the old grammar move to migration tests or are regenerated. The contract's migration section stays as the mapping; its line on rule commands refusing a migrated video goes.

## Affected contracts

[Project files](../contracts/project-files.md#migration-of-todays-nodes), `shot-core`, `shot-render`, `shot-cli`, `shot-mcp`, `shot-app`.
