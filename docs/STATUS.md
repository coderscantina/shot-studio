# Status

Updated 2026-11-07. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

- `main`: kickoff docs, milestones 01 and 02, the board, CLI and MCP server.
- `milestone/03`: milestone 03 complete (capture, streaming transcription, transcript editing, render graph with chunked export, preview with sound, Record and Edit screens), on top of `main`. fmt, clippy and `cargo test --workspace` pass. Waiting on Mike's go to merge into `main`.
- `milestone/04-06`: integration branch for milestones 04, 05 and 06, at the tip of `milestone/03`.
- `task/m46-contract` in `.claude/worktrees/m46-contract`: a running agent designs the project file additions for 04 to 06 (review queue, caption fixes, markers, product video effects). It merges into `milestone/04-06`.

## Uncommitted work

`docs/design.md`, `docs/technical-briefing.md`, `docs/milestones/03-opinion-video.md` and the new `docs/mockup/`: the design thread's settled Edit rules and the mockup exported into the repo, waiting for that thread or Mike to commit.

## Running

The contract design agent above. Generated media lives in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target`. Screenshots are in `docs/evidence/m02/` and `docs/evidence/m03/` (git-ignored).

## Waiting on Mike

- Merge `milestone/03` into `main`.
- [Open decisions](milestones/README.md#open-decisions): 1, the cache as plain files ([0011](decisions/0011-cache-as-plain-files.md)).
- Prerequisites: create the GitHub repo so CI runs, install the Xcode Metal Toolchain.
- Milestone 05's prompter test needs Mike at the Elgato Prompter.

## Next

1. Merge the contract design into `milestone/04-06`, then run [04](milestones/04-auto-cut-and-finish.md), [05](milestones/05-record-comfort.md) and [06](milestones/06-product-videos.md) in parallel.
