# Implementation milestones

Mark a milestone complete only after its acceptance criteria pass and an evidence report is linked here.

## Sequence and gates

| ID | Milestone | Dependencies | Owner | Status |
| --- | --- | --- | --- | --- |
| 01 | [Contracts, fixtures and workspace](01-contracts-and-fixtures.md) | None | Core | Done locally, CI run waits for the GitHub repo. [Evidence](../evidence/m01-contracts-and-fixtures.md) |
| 02 | [Preview spike](02-preview-spike.md) | 01 | Media, App | GO by Mike on 2026-11-07. The dropped-frame and seek misses move into 03. [Evidence](../evidence/m02-preview-spike.md) |
| 03 | [A usable opinion video](03-opinion-video.md) | 01, 02 | All roles | Timing, editing and recording criteria pass, four screen gaps open. [Evidence](../evidence/m03-opinion-video.md) |
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

- Cache as plain files named by key under `.shot/cache/` instead of SQLite. Recommend adopting: lookups are by one key and segments are files anyway. Milestone 03 already builds on it. [0011](../decisions/0011-cache-as-plain-files.md)

## Prerequisites from Mike

- Install the Xcode Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`) so gpui's shaders compile at build time instead of at app start.
- Before 01 can close: create `github.com/coderscantina/shot-studio` and push, so CI runs once.

- Before 08 ships to YouTube: a Google Cloud project with the YouTube Data API enabled and an OAuth client for a desktop app.
- Before 09: an Apple Developer ID certificate and a notarization profile for the release workflow, and a `coderscantina/homebrew-tap` repo.

## Known gaps

Found by agents and waiting for a task. Remove a line when it's fixed.

- Capture-time markers (A, N, M, Z, B and layout hotkeys) have no place in the contract yet. Milestone 05 adds them, most likely as a `marker` kind in `events.jsonl`.
- Unsure auto-cut results waiting in the review queue are not stored in any file, which breaks the no-hidden-state rule. Milestone 04 adds them to the contract.
- Caption text fixes (misheard names) are not stored in any file, and burned-in segment hashes will need them. Milestone 04.
- `script.md` rejects text between a chapter heading and its first beat. Revisit when milestone 08 builds the script editor.
