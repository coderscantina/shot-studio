# Status

Updated 2026-11-08. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

`main` is the branch of record and is public at https://github.com/coderscantina/shot-studio. It holds milestones 01 to 08, milestones 10 to 12 built except what waits on Mike, and milestone 09's release machinery. fmt, clippy and `cargo test --workspace` pass. No other branches, no worktrees.

## Uncommitted work

None.

## Running

None. Generated media in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target` (clear its `debug/` when disk runs low). Reels and screenshots in `docs/evidence/` (git-ignored); the README's screenshots are in `docs/assets/`.

## Waiting on Mike

See the [open decisions](milestones/README.md#open-decisions) and [prerequisites](milestones/README.md#prerequisites-from-mike):
- Score the four reels and listen to the bundled sounds and beds.
- The design-variants round for milestone 10 item 7.
- The website domain (postponed).
- The prompter beam splitter check, the YouTube OAuth client, and the org signing secrets shared with `shot-studio` if they are limited to selected repos.

## Next

1. Milestone 10 item 7 after the design round.
2. [Milestone 09](milestones/09-release-gate.md) items 4 and 5: measure every target on the idle reference machine, then one full video from board to a private upload.
