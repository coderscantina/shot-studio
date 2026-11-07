# Implementation milestones

Mark a milestone complete only after its acceptance criteria pass and an evidence report is linked here.

## Sequence and gates

| ID | Milestone | Dependencies | Owner | Status |
| --- | --- | --- | --- | --- |
| 01 | [Contracts, fixtures and workspace](01-contracts-and-fixtures.md) | None | Core | Done locally, CI run waits for the GitHub repo. [Evidence](../evidence/m01-contracts-and-fixtures.md) |
| 02 | [Preview spike](02-preview-spike.md) | 01 | Media, App | Not started |
| 03 | [A usable opinion video](03-opinion-video.md) | 01, 02 | All roles | Not started |
| 04 | [Auto cut and finish](04-auto-cut-and-finish.md) | 03 | Analysis, Media | Not started |
| 05 | [Record comfort](05-record-comfort.md) | 03 | Analysis, App | Not started |
| 06 | [Product videos](06-product-videos.md) | 03 | Media, App | Not started |
| 07 | [Overlays and derived cuts](07-overlays-and-derived-cuts.md) | 04, 05 | Media, Core | Not started |
| 08 | [Planning, agents and ship](08-planning-agents-and-ship.md) | 03, 04, 07 | Core, Agents | Work items 1 and 2 (board, proposals) merged, 3 started: `shot` CLI and MCP server over the board |
| 09 | [Release gate](09-release-gate.md) | 01 to 08 | Core | Not started |

Milestone 02 is a go/no-go gate. If gpui can't show a 4K60 VideoToolbox decode without a CPU copy, stop and bring the measurements to Mike before 03 starts, because the UI choice changes.

Once 03 lands, 04, 05 and 06 run in parallel. The board and proposal tasks of 08 only need 01 and can start any time after it. The script agent, `shot check` and `shot ship` tasks of 08 wait for their dependencies.

## Open decisions

Waiting for Mike. One line each with a recommendation and a link to its record. Remove the line once the record says Adopted.

- Release tag for a second video shipped in the same week: `v2026.46` already exists, so shipping fails today. Recommend `v2026.46.2` for the second, matching the app's CalVer rule for a second release in a day.
- Thresholds beyond the targets: 100 ms seek, 300 ms voice-follow lag, 0.5 LU loudness tolerance. Recommend adopting as written, they are measurable and match what a viewer notices. [0010](../decisions/0010-acceptance-thresholds.md)

## Prerequisites from Mike

- Before 01 can close: create `github.com/coderscantina/shot-studio` and push, so CI runs once.

- Before 08 ships to YouTube: a Google Cloud project with the YouTube Data API enabled and an OAuth client for a desktop app.
- Before 09: an Apple Developer ID certificate and a notarization profile for the release workflow, and a `coderscantina/homebrew-tap` repo.

## Known gaps

Found by agents and waiting for a task. Remove a line when it's fixed.

- Capture-time markers (A, N, M, Z, B and layout hotkeys) have no place in the contract yet. Milestone 05 adds them, most likely as a `marker` kind in `events.jsonl`.
- Unsure auto-cut results waiting in the review queue are not stored in any file, which breaks the no-hidden-state rule. Milestone 04 adds them to the contract.
- Caption text fixes (misheard names) are not stored in any file, and burned-in segment hashes will need them. Milestone 04.
- Segment hashes use the take id as the source media id, not a hash of the media. A re-recorded take with the same id would hit a stale cache. Milestone 03 decides before the render cache lands.
- `script.md` rejects text between a chapter heading and its first beat. Revisit when milestone 08 builds the script editor.
