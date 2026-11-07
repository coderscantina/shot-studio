# Status

Updated 2026-11-07. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

Each branch builds on the one before it. All pass fmt, clippy and `cargo test --workspace`.

- `main`: kickoff docs, milestones 01 and 02, the board, CLI and MCP server.
- `milestone/03`: milestone 03 complete. Waiting on Mike's go to merge into `main`.
- `milestone/04-06`: milestones 04 and 06 complete, 05 built with the beam splitter check open. The contract additions for 04 to 06 (decisions 0013 to 0016, Proposed).
- `milestone/07-08`: integration branch for the running wave, branched from `milestone/04-06`.
- `task/m07-overlays` and `task/m08-agents` in `.claude/worktrees/`: running agents for milestone 07 and milestone 08 items 3 to 7. `shot ship` (08 item 8) waits for 07.

## Uncommitted work

`docs/design.md`, `docs/milestones/03-opinion-video.md` and the new `docs/mockup/`: the design thread's work, waiting for that thread or Mike to commit. `docs/technical-briefing.md` on `milestone/04-06` already links `docs/mockup/`.

## Running

The two agents above. Generated media in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target` (debug cleared on 2026-11-07, 44 GB freed). Screenshots in `docs/evidence/m0N/` (git-ignored).

## Waiting on Mike

- Merge `milestone/03` into `main` (and the later milestone branches as they close).
- The prompter beam splitter check: does the text read correctly through the glass with Mirror off?
- [Open decisions](milestones/README.md#open-decisions): 5 Proposed records (0011, 0013 to 0016), all already built on.
- Prerequisites: create the GitHub repo so CI runs, install the Xcode Metal Toolchain, a YouTube OAuth client before `shot ship`.

## Next

1. Merge 07 and 08 into `milestone/07-08`, then `shot ship` ([08 item 8](milestones/08-planning-agents-and-ship.md)).
2. [Milestone 09](milestones/09-release-gate.md), the release gate.
