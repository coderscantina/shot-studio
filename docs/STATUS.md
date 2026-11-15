# Status

Updated 2026-11-15. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

`main` is the branch of record and is public at https://github.com/coderscantina/shot-studio. Local `main` holds milestones 01 to 14 (the council polish, milestone 13 and 14 were fast-forwarded in on 2026-11-14) and is ahead of `origin/main`, not pushed.

`m15/integration` is on top of `main`, finished, checked and not merged: [milestone 15](milestones/15-one-video-model-transitions-and-themes.md), built by seven streams and merged. [Evidence](evidence/m15-one-video-model.md).
- One kind of video: every video opens on style rules, old ones migrate on open, a lone folder is a channel of one ([0035](decisions/0035-every-video-on-style-rules.md)).
- One transition type for chapter edges and layout switches, one painter path, exact first and last frames ([0036](decisions/0036-one-transition-type.md)).
- Themes as files picked by `theme <id>` in `brand.kdl` or any format layer, `shot theme`, MCP theme and preset tools, presets in rules, a channel lock ([0029](decisions/0029-several-themes-per-channel.md)).
- A worktree per video branch at `.shot/worktrees/<slug>`, migration as its own commit, accept commits a person's edits first, read-only MCP tools write nothing ([0037](decisions/0037-a-worktree-per-video-branch.md)).
- Removing lock overlays and placed sounds, cards inside a Short's safe area, the app's copies of render and core code gone.

fmt, workspace clippy and `cargo test --workspace` pass on `m15/integration`; the GPU tests pass except the known `frame_grid` gap. No worktrees, no other local branches.

## Uncommitted work

Mike's own app icon work: `README.md`, `packaging/macos/AppIcon.icns`, `AppIcon.svg` and `scripts/app-icon.sh`. Not touched by agents.

## Running

No app is running. Generated media is in `$HOME/.cache/shot-studio-fixtures`. The shared build cache is in `$HOME/.cache/shot-studio-target`; clear its `debug/` when disk runs low (about 70 GB free now). Screenshots in `docs/evidence/` are git-ignored.

## Waiting on Mike

- Merge `m15/integration` into `main`, and push `main`.
- The [open decisions](milestones/README.md#open-decisions): resting-cursor zooms (gone with `shot autozoom`), the card size in Shorts, 0034 (cut-out camera, best judged on a real face), the zoom ramp ease, the four reels, the milestone 10 item 7 design round, the website domain (postponed).
- The [prerequisites](milestones/README.md#prerequisites-from-mike): the prompter beam splitter check, the YouTube OAuth client, the org signing secrets.

## Next

1. Milestone 15 follow-ups (in the known gaps): `shot style scope` and the MCP propose and reach tools on worktrees, callout labels and the code-diff panel in Shorts, Remove for a rule's sound, a decode thread for the transition prefetch, the app's changed-on-disk check.
2. Milestone 13 and council follow-ups: credits for the Library and Noto emoji in Ship's description, takes commands in the CLI and MCP, a track switch for `shot record`, cut points at take rates other than 60 fps, Text and Picture empty states, the 1280x800 inspector hints.
3. [Milestone 09](milestones/09-release-gate.md) items 4 and 5: measure every target on the idle reference machine, then one full video from board to a private upload.
