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
| 10 | [Semantic core](10-semantic-core.md) | 07, 08 | Core, Analysis, Media, App | Items 1 to 6 and 8 on `milestone/10-12`, item 7 waits for a design round with Mike. [Evidence](../evidence/m10-semantic-core.md) |
| 11 | [Motion system and effects as styles](11-effects-as-styles.md) | 10 (items 1 to 4 can start earlier) | Media, App | Built on `milestone/10-12`; Mike's reel scores are the open criterion. [Evidence](../evidence/m11-effects-as-styles.md) |
| 12 | [Styled monitor and watch builds](12-styled-monitor-and-watch-builds.md) | 10, 11 | App, Media, Agents | Built on `milestone/10-12`; watch rebuild 7.8 to 13.5 s, the URL card latency waits for Mike. [Evidence](../evidence/m12-styled-monitor-and-watch-builds.md) |
| 09 | [Release gate](09-release-gate.md) | 01 to 08, 10 to 12 | Core | Items 1 to 3 and 6 built (release.sh, workflow, bundle, cask, credits); signing waits for Mike's Apple secrets. [Evidence](../evidence/m09/release-machinery.md) |

Milestone 02 is a go/no-go gate. If gpui can't show a 4K60 VideoToolbox decode without a CPU copy, stop and bring the measurements to Mike before 03 starts, because the UI choice changes.

Once 03 lands, 04, 05 and 06 run in parallel. The board and proposal tasks of 08 only need 01 and can start any time after it. The script agent, `shot check` and `shot ship` tasks of 08 wait for their dependencies.

Milestones 10 to 12 make Shot Studio a semantic video editor ([0019](../decisions/0019-semantic-video-elements-and-style.md)) and run after 07 and 08 merge. The first release waits for them, so 09 comes last. 11 and 12 can overlap once 10's style engine and lock land.

## Open decisions

Waiting for Mike. One line each with a recommendation and a link to its record. Remove the line once the record says Adopted.

- Score the four reels (`docs/evidence/m11/reel-*.mp4`, 81 s each, timecoded list in the report); milestone 11 needs 8 or higher per preset.
- Website and domain: `shot-studio.app` or `shot.coderscantina.com`, postponed by Mike on 2026-11-08. No website is linked until then.
- Milestone 10 item 7: a design-variants round for provenance, the scope choice and the rule editor, plus the transition mark style and Picture's placement buttons.

Settled on 2026-11-08: every Proposed record (0011, 0013 to 0025) adopted, the live URL card target relaxed to 1.5 s, golden frames allow 1 code value on up to 16 pixels, codec headroom stays 1.5 dB, presets get jump-cut punch-ins back through an `every=<n>` selector filter, the release workflow reuses Kessel's coderscantina signing secrets, the repo goes public now, SECURITY.md promises best effort, Discussions stay off, the Homebrew tap waits for the first release.

## Prerequisites from Mike

- Install the Xcode Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`) so gpui's shaders compile at build time instead of at app start.
- Before 01 can close: create `github.com/coderscantina/shot-studio` and push, so CI runs once.

- Before 08 ships to YouTube: a Google Cloud project with the YouTube Data API enabled and an OAuth client for a desktop app.
- Before 09: the coderscantina org secrets Kessel signs with (`MACOS_CERTIFICATE`, `MACOS_CERTIFICATE_PASSWORD`, `MACOS_SIGN_IDENTITY`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`) shared with `shot-studio`, and at the first release a `coderscantina/homebrew-tap` repo with `packaging/homebrew/shot-studio.rb` copied to `Casks/`.

## Known gaps

Found by agents and waiting for a task. Remove a line when it's fixed.

- Double-clicking the app opens only About: there is no way to open a video folder from Finder yet. The app has a placeholder icon, unstripped binaries (105 MB), and CI does not catch a stale `crates.txt`.
- One watch save of six took 65 s: its build took 9.7 s but the change was noticed about 55 s late, cause not found (the disk was nearly full). `shot check` still takes about 40 s on one export because seeks past 250 s cost 150 ms in VideoToolbox; a forward-reading keyframe decoder in shot-media would fix it. An export needs about 12.8 GB free for the main cut (`<cut>.partial` beside the old file). On the sound-effect-heavy channel one save re-rendered far more than 2 chunks, likely `beat-sync` moving later cards when the cut shortens.
- Captions draw over the link card on the monitor (export keeps main-cut captions in the sidecar). The Record layout switcher from the mockup is not built; the Styled/Raw switch takes its place.
- The painter has no prefetch for the second side of a two-sided transition (the engine queues 18 frames instead), and `paint` does not return the screen layer it drew, so the engine lays out paused frames twice to map callout clicks. `--bench-edit --glass` draws its own pass although `Painter::set_decoration` now exists.
- Stingers and derived cuts do not snap to the beat. The bookend title and word-pop captions have no phase sounds. The contract's `sounds` row does not yet say which templates count as cards or pops, that clicks need a ripple, or that a phase plays on its first frame.
- The shot-app watchdog test (`the_watchdog_names_where_a_stalled_thread_waits_or_runs`) fails about one run in three at load average 15.
- The contract does not yet describe which transition joins a bookend, where the intro goes, what each title shows, or take handles. Bumpers are cached per video, not per channel as decision 0021 says.
- Placed sounds and lock overlays cannot be removed in the app yet; that waits for the provenance and scope design round. The transition mark style and Picture's placement buttons are not in the mockup and need Mike's design call.
- The app's sound, callout, pop and Lottie placement were driven by an in-app script calling the button handlers, not by real clicks; the Lottie import is covered only by its unit test.
- The end card does not draw the brand logo yet, and a brand font file has only gone through a unit test, not a real export.
- vello's coverage order varies between runs; rectangle shadows are snapped to whole pixels, but a rare one-bit flip in text or other shapes is still possible.
- The first seek after playback in the editor engine takes about 0.5 s.
- The app's preview plays the voice and the lock's music and sound effects, but not export's bumper sound, the brand's per-phase transition sounds or the music ducking under bumpers.
- `shot check` (`check_cut`) and the Ship screen's cut length read the timeline without bookends, so with an intro the chapter rules and the length are off by the bookends.
- `shot-render`'s painter returns no screen layer, so the editor engine lays out a paused frame a second time for callout clicks; and it has no decoration hook, so `--bench-edit --glass` draws its reel in a pass of its own.
- On a migrated video the Cut screen's rule switches and pacing only show; changing the rules waits for the rule editor (milestone 10 item 7). A lock cut on a kind outside the six pacing kinds shows as a person's cut in the app.
- The preview's loudness gain comes from one measuring pass, without export's limiter trials, and a new gain plays from the next seek.
- `shot-render` keeps export's score (bed, lock music and sfx at their output samples) private and `shot-core` its settings diff, so the app has its own copies of both (`score.rs`, `style::layer`).
- A migrated video folder outside a channel repo keeps its lock as it is: edits there write `cut.kdl` but do not rebuild `cut.lock`, as `shot export` does not.
- The bundled sounds and music beds are unheard: Mike should check by ear that each sound fits its name and that the bed loop seams (bed-lofi most of all) are clean.
- Beats are detected per music file on every run; decision 0023 asks for a cache keyed by the file's hash.
- `music "<file>"` only takes a path; bundled beds need a bare name looked up like `sfx`. Music ducks to the cue's duck level under bumpers rather than going silent (an assumption).
- `shot-app --bench` measures the milestone 02 player, not the editor engine, so preview frame drops with motion, glass and motion blur on are unmeasured.
- Element gaps: two `{name}` tags on one beat collide on (kind, t), cursor dwell has no element kind, M markers carry no label, `code` and `expression` have no detector, takes recorded without the prompter get no beat or chapter elements.
- Brand imports are read from `styles/<id>.kdl` in the repo; nothing fetches or pins them from git yet (milestone 10 item 4).
- The Edit screen writes `cut.kdl` on the checked-out branch while proposals merge into `video/<slug>`, so the two can drift.
- The music bed detects the end of its file by 10 s of digital silence; `PcmReader::ended_before` and `PcmReader::len` now give the real end.
- Agents sharing the target dir overwrite each other's `release/shot`; copy the binary before measuring or running it from a worktree.
- Linear ideas are not wired: no Linear MCP server or CLI was available.
- Face reframing is untested on a real face; Vision does not detect the fixture's pixel face.
- Overlays laid out for 16:9 get cut off inside a Short's 9:16 window.
- The export job queue starts with the shortest jobs although its comment says longest first.
- The b-roll lane color and the Overlays pane icons are local stand-ins, not theme tokens and Hugeicons.
- The app cannot preview a derived cut.
- `PcmReader` in `shot-media` does not expose its length, so the music bed detects the end by looking for 10 s of digital silence.
- Filler learning only uses Keeps from the current video, not the channel. N markers (next beat) are not used by any rule yet.
- A `code-diff` chunk's hash covers the snapshot's path, not its content, so rewriting a snapshot in place reuses stale chunks.
- Capture records no window bounds, so the `window` layout styles the whole screen.
- Auto zoom may end a zoom shorter than the format's `hold` to keep the cursor in view. The contract calls `hold` a minimum; settle which wins.
- Pausing the preview stops the sound without a fade, which may click.
- Old cache chunks and the pre-chunk `.shot/cache/segments/` folder are never cleaned up.
- Recording fragments are not fsynced, so power loss can lose more than the last fragment.
- `script.md` rejects text between a chapter heading and its first beat. Revisit when milestone 08 builds the script editor.
