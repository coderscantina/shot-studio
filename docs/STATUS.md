# Status

Updated 2026-11-07. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

- `main`: the branch of record. It holds the kickoff docs and the first waves:
  - Milestone 01: the workspace, pins, contract, `shot-core`, the fixture generator, the target project and the demo channel. Done except the CI run.
  - Milestone 02: the preview spike, GO by Mike on 2026-11-07. The `shot-media` decoder, player and pacer, the `shot-app` preview and `--bench`.
  - Milestone 08 work items 1 and 2 (`shot-board`) and the start of 3 (the `shot` CLI and MCP server).
  - fmt, clippy and `cargo test --workspace` pass.

No other branches, no worktrees.

## Uncommitted work

`docs/design.md`: the settled rules from the Edit screen design rounds (icons, Edit workspaces, agent proposals, whole-video overview), waiting for Mike to commit. The [mockup canvas](https://claude.ai/artifact/P6kQUAKTd96uCBaHTURQsa) already shows them: three Edit workspace artboards and Hugeicons on every screen.

## Running

None. Generated media lives in `$HOME/.cache/shot-studio-fixtures` (2.5 GB), the shared build cache in `$HOME/.cache/shot-studio-target`. Spike screenshots are in `docs/evidence/m02/` (git-ignored).

## Waiting on Mike

- Create the GitHub repo and push, so CI runs once and milestone 01 closes.
- Install the Xcode Metal Toolchain. Both are in the [milestone index](milestones/README.md#prerequisites-from-mike).
- No open product decisions.

## Next

1. [Milestone 03](milestones/03-opinion-video.md): a usable opinion video. It starts with the 0.5 s GOP switch in capture and fixtures, and the spike's open dropped-frame and color items.
2. The rest of [milestone 08](milestones/08-planning-agents-and-ship.md) work item 3 waits on 03 for transcripts.
