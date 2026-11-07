# Implementation milestones

Mark a milestone complete only after its acceptance criteria pass and an evidence report is linked here.

## Sequence and gates

| ID | Milestone | Dependencies | Owner | Status |
| --- | --- | --- | --- | --- |
| 01 | [Contracts, fixtures and workspace](01-contracts-and-fixtures.md) | None | Core | Done locally, CI run waits for the GitHub repo. [Evidence](../evidence/m01-contracts-and-fixtures.md) |
| 02 | [Preview spike](02-preview-spike.md) | 01 | Media, App | Complete, GO by Mike on 2026-11-07. [Evidence](../evidence/m02-preview-spike.md) |
| 03 | [A usable opinion video](03-opinion-video.md) | 01, 02 | All roles | Complete. [Evidence](../evidence/m03-opinion-video.md) |
| 04 | [Auto cut and finish](04-auto-cut-and-finish.md) | 03 | Analysis, Media | Complete. Burned-in caption drawing moved to 07. [Evidence](../evidence/m04-auto-cut-and-finish.md) |
| 05 | [Record comfort](05-record-comfort.md) | 03 | Analysis, App | Built, the beam splitter check waits for Mike. [Evidence](../evidence/m05-record-comfort.md) |
| 06 | [Product videos](06-product-videos.md) | 03 | Media, App | Complete. [Evidence](../evidence/m06-product-videos.md) |
| 07 | [Overlays and derived cuts](07-overlays-and-derived-cuts.md) | 04, 05 | Media, Core | Complete, face reframing proven on a stand-in face only. [Evidence](../evidence/m07-overlays-and-derived-cuts.md) |
| 08 | [Planning, agents and ship](08-planning-agents-and-ship.md) | 03, 04, 07 | Core, Agents | Complete, the real YouTube upload waits for Mike's OAuth client. [Evidence](../evidence/m08-planning-agents-and-ship.md) |
| 10 | [Semantic core](10-semantic-core.md) | 07, 08 | Core, Analysis, Media, App | Not started |
| 11 | [Motion system and effects as styles](11-effects-as-styles.md) | 10 (items 1 to 4 can start earlier) | Media, App | Not started |
| 12 | [Styled monitor and watch builds](12-styled-monitor-and-watch-builds.md) | 10, 11 | App, Media, Agents | Not started |
| 09 | [Release gate](09-release-gate.md) | 01 to 08, 10 to 12 | Core | Not started |

Milestone 02 is a go/no-go gate. If gpui can't show a 4K60 VideoToolbox decode without a CPU copy, stop and bring the measurements to Mike before 03 starts, because the UI choice changes.

Once 03 lands, 04, 05 and 06 run in parallel. The board and proposal tasks of 08 only need 01 and can start any time after it. The script agent, `shot check` and `shot ship` tasks of 08 wait for their dependencies.

Milestones 10 to 12 make Shot Studio a semantic video editor ([0019](../decisions/0019-semantic-video-elements-and-style.md)) and run after 07 and 08 merge. The first release waits for them, so 09 comes last. 11 and 12 can overlap once 10's style engine and lock land.

## Open decisions

Waiting for Mike. One line each with a recommendation and a link to its record. Remove the line once the record says Adopted.

- Cache as plain files named by key under `.shot/cache/` instead of SQLite. Recommend adopting: lookups are by one key and segments are files anyway. Milestone 03 already builds on it. [0011](../decisions/0011-cache-as-plain-files.md)
- Review queue as `review` and `ignore` nodes in `cut.kdl`. Superseded by 0019 once adopted; 04 to 08 build on it until milestone 10 migrates. [0013](../decisions/0013-review-queue-in-cut-kdl.md)
- Auto edits written as ordinary nodes with provenance, markers as `events.jsonl` events. Superseded by 0019 once adopted, markers stay. [0014](../decisions/0014-auto-edits-as-written-nodes.md)
- Caption fixes as a "heard, said" dictionary in `cut.kdl`. Recommend adopting: survives re-transcription. [0015](../decisions/0015-caption-fixes-as-a-dictionary.md)
- Product video effects as optional `format.kdl` settings, code from git as an overlay template. Recommend adopting: existing files stay valid. [0016](../decisions/0016-product-effects-as-format-settings.md)
- Derived cuts as windows of main's frames that reuse its cached chunks. Recommend adopting: a Short costs seconds once main is exported. [0017](../decisions/0017-derived-cuts-from-main-chunks.md)
- Overlay templates as typed specs checked at export, unknown templates refused. Recommend adopting: agents fill parameters instead of writing code. [0018](../decisions/0018-overlay-templates-as-typed-specs.md)
- Ship artifacts (thumbnail, upload record with the YouTube video ids) stay in git-ignored `.shot/`; git gets the card move and the tag. Recommend adopting now and adding a committed `ship.kdl` once the board should link to YouTube. [0022](../decisions/0022-ship-artifacts-outside-git.md)
- Semantic video: committed elements per take, cascading style rules, a generated `cut.lock`, pacing as style, review decisions as overrides, shipped videos pinned at their tag. Settled in the effects grill on 2026-11-07; adopt once milestone 10 settles the grammar. [0019](../decisions/0019-semantic-video-elements-and-style.md)
- A channel `brand/brand.kdl` with partial overrides per video, bundled and repo fonts, bumpers git-ignored, styles imported from git. Settled in the effects grill. [0020](../decisions/0020-channel-brand-and-shared-styles.md)
- Chapter-edge transitions, automatic and placed SFX, music cues, intro, outro, chapter bands and the end card. Settled in the effects grill. [0021](../decisions/0021-sound-transitions-and-bookends.md)
- A motion system of springs, glass, depth, motion blur and kinetic type in four presets, Lottie templates declared with typed slots, sound per motion phase, beat sync, stinger, wipe and zoom-through transitions, a scored reel as the gate. Settled in the production grill on 2026-11-07. Amends 0018 and 0021. [0023](../decisions/0023-motion-system-and-lottie-templates.md)

## Prerequisites from Mike

- Install the Xcode Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`) so gpui's shaders compile at build time instead of at app start.
- Before 01 can close: create `github.com/coderscantina/shot-studio` and push, so CI runs once.

- Before 08 ships to YouTube: a Google Cloud project with the YouTube Data API enabled and an OAuth client for a desktop app.
- Before 09: an Apple Developer ID certificate and a notarization profile for the release workflow, and a `coderscantina/homebrew-tap` repo.

## Known gaps

Found by agents and waiting for a task. Remove a line when it's fixed.

- Record does not warn when the camera delivers black frames.
- The Edit screen writes `cut.kdl` on the checked-out branch while proposals merge into `video/<slug>`, so the two can drift.
- The music bed detects the end of its file by 10 s of digital silence; `PcmReader::ended_before` now gives the real end.
- The thumbnail is written to `.shot/export/main.png`; the contract does not name where thumbnails live.
- Agents sharing the target dir overwrite each other's `release/shot`; copy the binary before measuring or running it from a worktree.
- Linear ideas are not wired: no Linear MCP server or CLI was available.
- Face reframing is untested on a real face; Vision does not detect the fixture's pixel face.
- Overlays laid out for 16:9 get cut off inside a Short's 9:16 window.
- The export job queue starts with the shortest jobs although its comment says longest first.
- The b-roll lane color and the Overlays pane icons are local stand-ins, not theme tokens and Hugeicons.
- The app cannot preview a derived cut, and the preview plays no bleep.
- The inline retake toast exists in the prompter but nothing calls it: the live retake rule should call `PrompterPane::live().caught(...)`.
- `PcmReader` in `shot-media` does not expose its length, so the music bed detects the end by looking for 10 s of digital silence.
- Filler learning only uses Keeps from the current video, not the channel. N markers (next beat) are not used by any rule yet.
- A `code-diff` chunk's hash covers the snapshot's path, not its content, so rewriting a snapshot in place reuses stale chunks.
- Capture records no window bounds, so the `window` layout styles the whole screen.
- Auto zoom may end a zoom shorter than the format's `hold` to keep the cursor in view. The contract calls `hold` a minimum; settle which wins.
- Pausing the preview stops the sound without a fade, which may click.
- Old cache chunks and the pre-chunk `.shot/cache/segments/` folder are never cleaned up.
- Recording fragments are not fsynced, so power loss can lose more than the last fragment.
- `script.md` rejects text between a chapter heading and its first beat. Revisit when milestone 08 builds the script editor.
