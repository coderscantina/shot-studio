# Status

Updated 2026-11-08. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

- `main`: milestones 01 to 08, plus the plan for 10 to 12 and the mockup.
- `milestone/10-12`: milestone 10 items 1 to 6 and 8, milestone 11 items 1 to 4 and 10, the editor bench. fmt, clippy and `cargo test --workspace` pass. Not yet merged into `main`.
- `task/m10-applock` in `.claude/worktrees/m10-applock`: running agent putting the app, `shot describe`, thumbnails and the ship plan on the lock, the fixture generator on migrated videos, and preview audio on the export mix.
- `task/m11-templates` in `.claude/worktrees/m11-templates`: running agent rebuilding the 07 templates on the motion system and adding chapter-band, end-card, callout and pop.

## Uncommitted work

None besides this file.

## Running

The two agents above. Generated media in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target` (debug cleared before each wave). Screenshots in `docs/evidence/` (git-ignored).

## Waiting on Mike

- Merge `milestone/10-12` into `main` when this wave lands.
- Milestone 10 item 7 starts with a design-variants round: provenance, the scope choice and the rule editor.
- Listen to the bundled sounds and music beds (loop seams, bed-lofi most of all).
- The prompter beam splitter check, the [open decisions](milestones/README.md#open-decisions) (0011, 0013 to 0025, the pacing and punch-in note), the YouTube OAuth client, and the prerequisites for 09.

## Next

1. Milestone 11 items 7 to 9 (transitions, intro and outro, camera styling) in `shot-render`, then 11 and 12 (Sound and Overlays lanes, derived cuts, the reel for Mike to score).
2. [Milestone 12](milestones/12-styled-monitor-and-watch-builds.md), then [09](milestones/09-release-gate.md).
