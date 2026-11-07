# Status

Updated 2026-11-07. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

Each branch builds on the one before it. All pass fmt, clippy and `cargo test --workspace`.

- `main`: kickoff docs, milestones 01 and 02, the board, CLI and MCP server.
- `milestone/03`: milestone 03 complete. Waiting on Mike's go to merge into `main`.
- `milestone/04-06`: milestones 04 and 06 complete, 05 built with the beam splitter check open. The contract additions for 04 to 06 (decisions 0013 to 0016, Proposed).
- `milestone/07-08`: milestone 07 complete, milestone 08 complete except the real YouTube upload (verified against a mock only). Inline proposals in Edit, the true-peak limiter (-1.8 dBTP at -14 LUFS) and the `shot check` loudness fix are in. Decisions 0017, 0018 and 0022 Proposed. This is the tip; merging it carries every branch above.

No task branches, no worktrees.

## Uncommitted work

`docs/design.md`, `docs/milestones/03-opinion-video.md` and the new `docs/mockup/`: the design thread's work, waiting for that thread or Mike to commit. `docs/technical-briefing.md` on `milestone/04-06` already links `docs/mockup/`.

On `milestone/07-08`, from the effects grill on 2026-11-07, waiting for Mike to commit: briefing revision 2 (semantic video), decisions 0019 to 0021, the draft contract `docs/contracts/semantic-video.md`, milestones 10 to 12 and the index, 09's dependencies, status lines on 0013, 0014 and 0016, the lane rule in `docs/design.md` (on top of the design thread's edits) and a link in `docs/README.md`.

Index lines held back until that work is committed, to keep the commits apart. Add them to `docs/milestones/README.md` then:
- Status of 08: complete on `milestone/07-08`, real upload waits for the OAuth client. [Evidence](evidence/m08-planning-agents-and-ship.md).
- Open decisions: 0022, ship artifacts stay in `.shot/`, video ids are local to the machine that shipped.
- Known gap: the Edit screen writes `cut.kdl` on the checked-out branch while proposals merge into `video/<slug>`, so the two can drift.
- Known gap: the music bed still detects the end of its file by 10 s of digital silence; `PcmReader::ended_before` now gives the real end.

## Running

None. Generated media in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target`. Screenshots in `docs/evidence/m0N/` (git-ignored).

## Waiting on Mike

- Merge into `main`: `milestone/07-08` carries 03 to 08.
- The prompter beam splitter check: does the text read correctly through the glass with Mirror off?
- Open decisions: 0011, 0013 to 0018 and 0022 Proposed, all already built on; 0019 to 0021 from the effects grill.
- A YouTube "Desktop app" OAuth client saved as `~/Library/Application Support/Shot Studio/youtube-client.json`, then `shot ship --video <dir>` on a private test video.
- Prerequisites for 09: the GitHub repo so CI runs, the Xcode Metal Toolchain, an Apple Developer ID, the Homebrew tap.

## Next

1. [Milestone 10](milestones/10-semantic-core.md), the semantic core, starting with a design-variants round with Mike for provenance, the scope choice and the rule editor. Then 11 and 12.
2. [Milestone 09](milestones/09-release-gate.md), the release gate, after 10 to 12.
