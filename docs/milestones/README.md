# Implementation milestones

Mark a milestone complete only after its acceptance criteria pass and an evidence report is linked here.

## Sequence and gates

| ID | Milestone | Dependencies | Owner | Status |
| --- | --- | --- | --- | --- |
| 01 | [Contracts, fixtures and workspace](01-contracts-and-fixtures.md) | None | Core | Done locally, CI run waits for the GitHub repo. [Evidence](../evidence/m01-contracts-and-fixtures.md) |
| 02 | [Preview spike](02-preview-spike.md) | 01 | Media, App | Complete, GO by Mike on 2026-11-07. [Evidence](../evidence/m02-preview-spike.md) |
| 03 | [A usable opinion video](03-opinion-video.md) | 01, 02 | All roles | Complete on `milestone/03`. [Evidence](../evidence/m03-opinion-video.md) |
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
- Review queue as `review` and `ignore` nodes in `cut.kdl`. Recommend adopting: no hidden state, one-line diffs, re-runs never ask twice. Milestone 04 builds on it. [0013](../decisions/0013-review-queue-in-cut-kdl.md)
- Auto edits (jump-cut and auto zooms) written as ordinary nodes with provenance, markers as `events.jsonl` events. Recommend adopting: the file shows what the export shows. [0014](../decisions/0014-auto-edits-as-written-nodes.md)
- Caption fixes as a "heard, said" dictionary in `cut.kdl`. Recommend adopting: survives re-transcription. [0015](../decisions/0015-caption-fixes-as-a-dictionary.md)
- Product video effects as optional `format.kdl` settings, code from git as an overlay template. Recommend adopting: existing files stay valid. [0016](../decisions/0016-product-effects-as-format-settings.md)

## Prerequisites from Mike

- Install the Xcode Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`) so gpui's shaders compile at build time instead of at app start.
- Before 01 can close: create `github.com/coderscantina/shot-studio` and push, so CI runs once.

- Before 08 ships to YouTube: a Google Cloud project with the YouTube Data API enabled and an OAuth client for a desktop app.
- Before 09: an Apple Developer ID certificate and a notarization profile for the release workflow, and a `coderscantina/homebrew-tap` repo.

## Known gaps

Found by agents and waiting for a task. Remove a line when it's fixed.

- Record does not warn when the camera delivers black frames.
- Pausing the preview stops the sound without a fade, which may click.
- Old cache chunks and the pre-chunk `.shot/cache/segments/` folder are never cleaned up.
- Recording fragments are not fsynced, so power loss can lose more than the last fragment.
- `script.md` rejects text between a chapter heading and its first beat. Revisit when milestone 08 builds the script editor.
