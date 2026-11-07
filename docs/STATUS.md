# Status

Updated 2026-11-07. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

- `main`: the kickoff docs only (`42ae892`).
- `integration`: all milestone work so far, not merged into `main`. fmt, clippy and `cargo test --workspace` pass on it. It holds:
  - Milestone 01: the workspace, pins, contract, `shot-core`, the fixture generator, the target project and the demo channel. Done except the CI run.
  - Milestone 02: the preview spike (`shot-media` decoder, player and pacer, `shot-app` preview and `--bench`). Measured, and the gate waits for Mike.
  - Milestone 08 work items 1 and 2 (`shot-board`) and the start of 3 (the `shot` CLI and MCP server).
  - Waiting on Mike's go to merge into `main`.

No task branches or worktrees.

## Uncommitted work

`docs/design.md`: a settled icons rule (Hugeicons) added outside this run, left uncommitted for its author.

## Running

None. Generated media lives in `$HOME/.cache/shot-studio-fixtures` (2.5 GB), the shared build cache in `$HOME/.cache/shot-studio-target`. Spike screenshots are in `docs/evidence/m02/` (git-ignored).

## Waiting on Mike

- The milestone 02 gate, plus 2 more open decisions, in the [milestone index](milestones/README.md#open-decisions).
- Prerequisites: create the GitHub repo so CI runs, and install the Xcode Metal Toolchain. See the [milestone index](milestones/README.md#prerequisites-from-mike).

## Next

1. On GO: [milestone 03](milestones/03-opinion-video.md), reusing the spike's decoder, player, pacer and zoom pass.
2. The fixture GOP change to 0.5 s if adopted ([milestone 01](milestones/01-contracts-and-fixtures.md) generator).
3. The rest of [milestone 08](milestones/08-planning-agents-and-ship.md) work item 3 waits on 03 for transcripts.
