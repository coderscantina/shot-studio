# Implementation milestones

Mark a milestone complete only after its acceptance criteria pass and an evidence report is linked here.

## Sequence and gates

| ID | Milestone | Dependencies | Owner | Status |
| --- | --- | --- | --- | --- |
| 01 | [Contracts, fixtures and workspace](01-contracts-and-fixtures.md) | None | Core | Complete, first CI run green on 2026-11-08. [Evidence](../evidence/m01-contracts-and-fixtures.md) |
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
| 13 | [Takes, scenes and studio tools](13-takes-scenes-and-studio-tools.md) | 10, 11, 12 | All roles | Built on `m13/integration`; decisions 0026 to 0030 and 0032 wait for Mike. [Evidence](../evidence/m13-takes-scenes-and-studio-tools.md) |
| 14 | [Hardening from FilmCraft and EffectCraft](14-hardening-from-filmcraft-and-effectcraft.md) | 13 | All roles | Built on `m14/integration`; decisions 0033 and 0034 wait for Mike. [Evidence](../evidence/m14-hardening.md) |
| 15 | [One video model, one transition type, themes as files](15-one-video-model-transitions-and-themes.md) | 14 | All roles | Built on `m15/integration`; the app check and the quiet bench are in the report. [Evidence](../evidence/m15-one-video-model.md) |
| 16 | [Script editor and clip ingest](16-script-editor-and-clip-ingest.md) | 15 | App, Media, Agents | Built on `m16/integration`; decisions 0038 and 0039 wait for Mike, app import and drop and HDR tone mapping unverified. [Evidence](../evidence/m16-script-editor-and-clip-ingest.md) |
| 09 | [Release gate](09-release-gate.md) | 01 to 08, 10 to 12 | Core | Items 1 to 3 and 6 built (release.sh, workflow, bundle, cask, credits); signing waits for Mike's Apple secrets. [Evidence](../evidence/m09/release-machinery.md) |

Milestone 02 is a go/no-go gate. If gpui can't show a 4K60 VideoToolbox decode without a CPU copy, stop and bring the measurements to Mike before 03 starts, because the UI choice changes.

Once 03 lands, 04, 05 and 06 run in parallel. The board and proposal tasks of 08 only need 01 and can start any time after it. The script agent, `shot check` and `shot ship` tasks of 08 wait for their dependencies.

Milestones 10 to 12 make Shot Studio a semantic video editor ([0019](../decisions/0019-semantic-video-elements-and-style.md)) and run after 07 and 08 merge. The first release waits for them, so 09 comes last. 11 and 12 can overlap once 10's style engine and lock land.

## Open decisions

Waiting for Mike. One line each with a recommendation and a link to its record. Remove the line once the record says Adopted.

- Score the four reels (`docs/evidence/m11/reel-*.mp4`, 81 s each, timecoded list in the report); milestone 11 needs 8 or higher per preset.
- Website and domain: `shot-studio.app` or `shot.coderscantina.com`, postponed by Mike on 2026-11-08. No website is linked until then.
- Cut-out camera ([0034](../decisions/0034-cut-out-camera-from-person-masks.md)): `camera cutout=<bool>` on the brand, masks from Vision's balanced person segmentation cached by `shot analyze`, plain camera with a 200 ms crossfade where no person is found. Recommended: adopt, after one look at a real face.
- Zoom ramps use the smooth zoom's spring, which reads shorter than the ramp's number (a 1.5 s ramp looks done in about 0.5 s). An ease over the whole ramp would also change today's smooth zooms. Recommended: watch one first, see `docs/evidence/m13/c-ramp-in-sequence.png`.
- Resting-cursor zooms went with `shot autozoom` in milestone 15: no style rule zooms where the cursor rests or fits a zoom to keep the cursor in view. Recommended: a `cursor-rest` element from `shot analyze` and a zoom rule in the product-tour preset.
- Cards in a Short fill the safe area's width, a link card title about 37 px tall on a 1080 px frame. Recommended: look at `derived::tests::a_link_card_in_a_short_is_whole`'s frame before tuning.
- Script editor ([0038](../decisions/0038-script-editor-from-druck.md)): Druck's text core in a new `shot-editor` crate, the Script screen's document becomes the editor with autosave into the worktree, Edit script goes. Recommended: adopt.
- Ingested clips ([0039](../decisions/0039-ingested-clips-are-takes.md)): a phone clip becomes the next take as `camera.mp4` and `mic.mp4`, upright, SDR, 30 or 60 fps constant, then transcribed and cut like a recording. Recommended: adopt.
- Milestone 10 item 7: a design-variants round for provenance, the scope choice and the rule editor, plus the transition mark style and Picture's placement buttons.

Settled on 2026-11-14: 0026 to 0028, 0030, 0032 and 0033 adopted, 0029 adopted with the theme as a pointer, and 0035 to 0037 adopted for milestone 15.

Settled on 2026-11-08: every Proposed record (0011, 0013 to 0025) adopted, the live URL card target relaxed to 1.5 s, golden frames allow 1 code value on up to 16 pixels, codec headroom stays 1.5 dB, presets get jump-cut punch-ins back through an `every=<n>` selector filter, the release workflow reuses Kessel's coderscantina signing secrets, the repo goes public now, SECURITY.md promises best effort, Discussions stay off, the Homebrew tap waits for the first release.

## Prerequisites from Mike

- Install the Xcode Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`) so gpui's shaders compile at build time instead of at app start.

- Before 08 ships to YouTube: a Google Cloud project with the YouTube Data API enabled and an OAuth client for a desktop app.
- Before 09: the coderscantina org secrets Kessel signs with (`MACOS_CERTIFICATE`, `MACOS_CERTIFICATE_PASSWORD`, `MACOS_SIGN_IDENTITY`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`) shared with `shot-studio`, and at the first release a `coderscantina/homebrew-tap` repo with `packaging/homebrew/shot-studio.rb` copied to `Casks/`.

## Known gaps

Found by agents and waiting for a task. Remove a line when it's fixed.

- After milestone 16: a change on disk while the script editor holds an unsaved edit is overwritten by the next save; Tab with no proposal selected moves focus out of the editor; a script that never parsed makes the agent draft from scratch. IME, outside-change reload, save on quit and mouse-drag selection in the editor are untested.
- Ingest: HDR to SDR tone mapping is unproven, Import clip and file drop were never driven in the app (the control socket cannot switch screens or drop files), a 4K ingest peaks at about 2 GB, there is no MCP ingest tool, and a phone screen recording cannot be ingested as `screen.mp4`.
- After milestone 15: `shot style scope` and the MCP propose and reach tools still read branch tips, not worktrees; an idea's default `script.md`, `format.kdl` and `cut.lock` stay uncommitted in its worktree; `shot migrate` and `style build --all` create a worktree for every video, shipped ones too; `move_into` leaves the moved text files untracked in the main checkout. A worktree whose branch was deleted and whose only changes are staged can be removed.
- `shot caption-fix` refuses an old video instead of migrating it, and `shot overlays b-roll` cannot fill a slot a rule placed. `README.md` and the technical briefing still name `shot autocut`.
- The painter opens the incoming side of a transition half a second early on the render thread; hiding that pause fully needs a decode thread in `shot-media`.
- The app has a placeholder icon, unstripped binaries (105 MB), and CI does not catch a stale `crates.txt`.
- One watch save of six took 65 s: its build took 9.7 s but the change was noticed about 55 s late, cause not found (the disk was nearly full). `shot check` still takes about 40 s on one export because seeks past 250 s cost 150 ms in VideoToolbox; a forward-reading keyframe decoder in shot-media would fix it. An export needs about 12.8 GB free for the main cut (`<cut>.partial` beside the old file). On the sound-effect-heavy channel one save re-rendered far more than 2 chunks, likely `beat-sync` moving later cards when the cut shortens.
- Captions draw over the link card on the monitor (export keeps main-cut captions in the sidecar). The Record layout switcher from the mockup is not built; the Styled/Raw switch takes its place.
- The editor counts frames at 60 fps, so cut points miss the frame grid of other take rates by up to 20 ms. The test is `#[ignore]`d with numbers.
- The app keeps a video in memory and can overwrite a change `shot` made meanwhile; it needs a changed-on-disk check.
- The control socket stays behind when the app is killed, and `commands` lists about 225 actions per window.
- Stingers and derived cuts do not snap to the beat. The bookend title and word-pop captions have no phase sounds. The contract's `sounds` row does not yet say which templates count as cards or pops, that clicks need a ripple, or that a phase plays on its first frame.
- The shot-app watchdog test (`the_watchdog_names_where_a_stalled_thread_waits_or_runs`) fails about four runs in five at load average 10 to 20: a sample can land before the thread starts sleeping.
- The contract does not yet describe which transition joins a bookend, where the intro goes, what each title shows, or take handles. Bumpers are cached per video, not per channel as decision 0021 says.
- How a removed lock overlay or sound shows its provenance waits for the design round, and a rule's sound cannot be removed in the app yet (only a placed one). The transition mark style and Picture's placement buttons are not in the mockup and need Mike's design call.
- The app's sound, callout, pop and Lottie placement were driven by an in-app script calling the button handlers, not by real clicks; the Lottie import is covered only by its unit test.
- The end card does not draw the brand logo yet, and a brand font file has only gone through a unit test, not a real export.
- vello's coverage order varies between runs; rectangle shadows are snapped to whole pixels, but a rare one-bit flip in text or other shapes is still possible.
- The first seek after playback in the editor engine takes about 0.5 s.
- `shot check` (`check_cut`) and the Ship screen's cut length read the timeline without bookends, so with an intro the chapter rules and the length are off by the bookends.
- On a migrated video the Cut screen's rule switches and pacing only show; changing the rules waits for the rule editor (milestone 10 item 7). A lock cut on a kind outside the six pacing kinds shows as a person's cut in the app.
- The preview's loudness gain comes from one measuring pass, without export's limiter trials; a new gain fades in when it lands.
- A migrated video folder outside a channel repo keeps its lock as it is: edits there write `cut.kdl` but do not rebuild `cut.lock`, as `shot export` does not.
- The bundled sounds and music beds are unheard: Mike should check by ear that each sound fits its name and that the bed loop seams (bed-lofi most of all) are clean.
- Beats are detected per music file on every run; decision 0023 asks for a cache keyed by the file's hash.
- `music "<file>"` only takes a path, so the app copies a bundled bed into `sources/` (about 3 MB); a bare name looked up like `sfx` would avoid the copy. Music ducks to the cue's duck level under bumpers rather than going silent (an assumption).
- `shot-app --bench` measures the milestone 02 player, not the editor engine, so preview frame drops with motion, glass and motion blur on are unmeasured.
- Element gaps: two `{name}` tags on one beat collide on (kind, t), cursor dwell has no element kind, M markers carry no label, `code` and `expression` have no detector, takes recorded without the prompter get no beat or chapter elements.
- Brand imports are read from `styles/<id>.kdl` in the repo; nothing fetches or pins them from git yet (milestone 10 item 4).
- The Edit screen writes `cut.kdl` on the checked-out branch while proposals merge into `video/<slug>`, so the two can drift. The board reads the branches, so studio edits never reach a card and files fetched for an opened card land untracked in the main checkout; a worktree per video branch or a commit on save would fix it (Mike's call).
- Undo and redo errors always show on the Edit screen, whatever screen is open. After a recording the Cut screen keeps its old place in the queue instead of jumping to the new take's first item.
- Shorts upload with the main video's title, the title can only be edited in `video.kdl`, and Stop cannot interrupt the browser sign-in (up to 5 min).
- When the Timeline's error strip shows, it pushes the lanes past the bottom of the window.
- The silence rule ignores the screen, so a silent screen demo is cut like a pause (the target project's intro loses 60 s on any rerun of the rules).
- Placing sounds needs a migrated video in a channel repo; the target-project demo can only audition them.
- The music bed detects the end of its file by 10 s of digital silence; `PcmReader::ended_before` and `PcmReader::len` now give the real end.
- Agents sharing the target dir overwrite each other's `release/shot`; copy the binary before measuring or running it from a worktree.
- Linear ideas are not wired: no Linear MCP server or CLI was available.
- Face reframing is untested on a real face; Vision does not detect the fixture's pixel face.
- In a Short, a callout's label is placed towards the middle of the 16:9 frame and can still fall outside the window, and a code diff's panel is wider than the window. Both are drawn in the base with the screen.
- The export job queue starts with the shortest jobs although its comment says longest first.
- The b-roll lane color and the Overlays pane icons are local stand-ins, not theme tokens and Hugeicons.
- The app cannot preview a derived cut.
- `PcmReader` in `shot-media` does not expose its length, so the music bed detects the end by looking for 10 s of digital silence.
- Filler learning only uses Keeps from the current video, not the channel. N markers (next beat) are not used by any rule yet.
- A `code-diff` chunk's hash covers the snapshot's path, not its content, so rewriting a snapshot in place reuses stale chunks.
- Capture records no window bounds, so the `window` layout styles the whole screen.
- Auto zoom may end a zoom shorter than the format's `hold` to keep the cursor in view. The contract calls `hold` a minimum; settle which wins.
- Old cache chunks and the pre-chunk `.shot/cache/segments/` folder are never cleaned up.
- Recording fragments are not fsynced, so power loss can lose more than the last fragment.
- `script.md` rejects text between a chapter heading and its first beat. Revisit when milestone 08 builds the script editor.
