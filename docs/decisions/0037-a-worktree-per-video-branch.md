# 0037: A worktree per video branch

Status: Adopted by Mike on 2026-11-14. Settles the open question of how studio edits reach a video's branch.

## Problem

The board reads each video from its `video/<slug>` branch and merges proposals into it ([0004](0004-proposals-as-branches.md)), but the Edit screen writes `cut.kdl` in whatever branch the channel repo has checked out. Studio edits never reach a card, the two can drift, and files fetched for an opened card land untracked in the main checkout.

## Choice

- Opening a card checks out `video/<slug>` in a worktree at `.shot/worktrees/<slug>` of the channel repo, created on first open and reused after. The app, `shot` and the MCP server work on the video in that worktree.
- A person's edits stay uncommitted in the worktree until they commit. A migration on open ([0035](0035-every-video-on-style-rules.md)) is committed on its own.
- The board shows a card's uncommitted edits from its worktree. Proposals merge into the branch as today; a merge into a branch whose worktree has uncommitted changes to the same files is refused with the files named.
- `brand/` belongs to the channel and is edited in the main checkout, uncommitted. A video worktree reads `brand/` from the main checkout at build time, so a theme edit reaches every unshipped video on its next build and a shipped one keeps its frozen lock.
- A worktree whose branch is merged or deleted is removed when the board opens, unless it has uncommitted changes.
- `.shot/worktrees/` is git-ignored. Media in a video's ignored folders stays in the main checkout's video folder, linked from the worktree, so no take is copied or lost when a worktree is removed.

## Rejected alternatives

- **Commit on save**: the branch gets a commit for every keystroke-sized edit.
- **Edits on the checked-out branch**: today's drift.

## Consequences

The board needs git worktree support in `shot-board`. Opening a video by folder path outside a channel works as before.

## Affected contracts

[Project files](../contracts/project-files.md#channel-repo-layout), `shot-board`, `shot-app`, `shot-cli`, `shot-mcp`.
