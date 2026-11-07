# Status

Updated 2026-11-07. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

- `main`: milestones 01 to 08 merged (Mike's go on 2026-11-07), plus the plan for 10 to 12 and the mockup. fmt, clippy and `cargo test --workspace` pass.
- `milestone/10-12`: integration branch for the semantic video milestones, at `main`.
- `task/m10-core` in `.claude/worktrees/m10-core`: running agent for milestone 10 items 1, 3 and 4 (grammar, style engine, cascade, lock, brand) in `shot-core`.
- `task/m11-motion` in `.claude/worktrees/m11-motion`: running agent for milestone 11 items 1 to 4 (motion system, materials, kinetic type, velato) in `shot-render`.

## Uncommitted work

None besides this file.

## Running

The two agents above. Generated media in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target` (debug cleared again, 25 GB freed). Screenshots in `docs/evidence/m0N/` (git-ignored).

## Waiting on Mike

- The prompter beam splitter check: does the text read correctly through the glass with Mirror off?
- [Open decisions](milestones/README.md#open-decisions): 0011, 0013 to 0018 and 0022 Proposed and built on; 0019 to 0021 and 0023 from the grills.
- Milestone 10 item 7 starts with a design-variants round with Mike: provenance, the scope choice and the rule editor.
- A YouTube "Desktop app" OAuth client saved as `~/Library/Application Support/Shot Studio/youtube-client.json`, then `shot ship --video <dir>` on a private test video.
- Prerequisites for 09: the GitHub repo so CI runs, the Xcode Metal Toolchain, an Apple Developer ID, the Homebrew tap.

## Next

1. Merge the two tasks into `milestone/10-12`, then milestone 10 items 2, 5, 6 and 8 (elements, migration, renderer on the lock, MCP) and 11 items 5 on.
2. [Milestone 12](milestones/12-styled-monitor-and-watch-builds.md) once 10's style engine and lock land, then [09](milestones/09-release-gate.md).
