# Status

Updated 2026-11-08. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

- `main`: milestones 01 to 08, plus the plan for 10 to 12 and the mockup.
- `milestone/10-12`: milestones 10 to 12 built except what waits on Mike, and milestone 09's release machinery (items 1 to 3 and 6). fmt, clippy and `cargo test --workspace --no-fail-fast` pass on an idle machine. Not merged into `main`; waiting on Mike's go.

No task branches, no worktrees.

## Uncommitted work

None.

## Running

None. Generated media in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target` (clear its `debug/` when disk runs low). Reels and screenshots in `docs/evidence/` (git-ignored).

## Waiting on Mike

See the [open decisions](milestones/README.md#open-decisions) and [prerequisites](milestones/README.md#prerequisites-from-mike). In short:
- Merge `milestone/10-12` into `main`.
- Milestone 10 item 7: a design-variants round for provenance, the scope choice and the rule editor; also the transition mark style and Picture's placement buttons.
- Score the four reels (milestone 11) and listen to the bundled sounds and beds.
- Decide: URL card latency target, golden-frame ±1 tolerance, codec headroom 1.5 dB, the pacing and punch-in note, and the Proposed records 0011, 0013 to 0018, 0022, 0024, 0025 (0019 to 0021, 0023 from the grills).
- The prompter beam splitter check. Click Don't Allow on the QuickTime automation prompt a test left on screen.
- Credentials: YouTube OAuth client, the GitHub repo, the Apple secrets, the Homebrew tap.

## Next

1. Milestone 10 item 7 after the design round.
2. [Milestone 09](milestones/09-release-gate.md) items 4 and 5: measure every target on the idle reference machine, then one full video from board to a private upload.
