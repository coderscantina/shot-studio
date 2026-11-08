# Status

Updated 2026-11-08. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

`main` is the branch of record and is public at https://github.com/coderscantina/shot-studio. Local `main` is ahead of `origin/main` and not pushed: it holds milestones 01 to 08, milestones 10 to 12 built except what waits on Mike, milestone 09's release machinery, the welcome window, timeline editing with undo, and the polish wave of 2026-11-08 merged on the same day. fmt, workspace clippy and `cargo test --workspace` pass except the flaky watchdog test (see known gaps). `polish/integration` is merged and can be deleted.

The script agent can search the web and read pages (WebSearch and WebFetch, script sessions only, recorded in decision 0005), and `propose_script` can add text files to `sources/`. Checked with `shot agent --video` on a copy of the demo channel against a real article. WebFetch will not return a page's full text word for word, so snapshots hold section headings, one exact sentence per section and a paraphrase.

The polish wave holds:
- Timeline: context-aware right-click menus on every lane element, layout from the Layout lane, adding, replacing and removing a music bed with its gain, a zoom slider, a draggable overview frame, zoom to selection or chapter (⌥Z), and a resizable split between the top row and the lanes.
- Sound: the picker auditions sounds and explains when placing needs a migrated channel video, the preview's loudness gain fades in without a seek, a broken music file no longer silences the voice, pausing fades out instead of clicking.
- Text and Picture: panes resize by their edges and keep their widths in `layout.json`, a useful inspector without a selection, working layout buttons, an outline with paragraphs, a Picture stage that fills the space.
- Cut: each review item plays in context (Space) or as cut (⇧Space), with K, C, arrows, ⌘Z and ⌘↩, progress, clear empty states and a note on migrated videos.
- Prompter: the output only shows on Record, never covers the app's own display unless asked, closes with Esc, its close control or ⌘⇧P, and follows displays coming and going. New controls: text size, line height, margins, width, reading line, speed, countdown, mirror, colors.
- Board: its own window per channel, reachable from the welcome window, ⌘O, "New Channel…" and `shot-app <channel-repo>`; new videos, keyboard moves, opening a card into its studio, moving a loose video into a channel.
- Ship: a setup card when there is no OAuth client, progress with Stop for export, upload and thumbnails, disk space and stale-export warnings, all seven checks; export stops between chunks.
- Undo covers `format.kdl`, so a pacing or rule change undoes together with the cut it made.

Checked with real clicks on the target project and a copy of the demo channel: timeline menus, layout submenu, music bed, split and overview drags, Cut, Record to Edit with the prompter output, board drag and drop, pane resizing.

`polish/council-round1` is finished, checked and not merged: 27 commits on top of `main` from a council UX review (Claude Opus and GPT Sol, three rounds, scores 7.5 and 8 to 8.5 and 9). fmt, workspace clippy and `cargo test --workspace` pass except the flaky watchdog test. It holds:
- Flow: a video opens on its next step (Cut while a take has no transcript); the nav ticks finished steps and dots the next; Script has Continue to record (⌘↩), Edit script in the default editor, and findings that name and scroll to their chapter; the welcome window offers Create video and Start a channel side by side; no file names in UI text; the board title shows only the channel name.
- Cut: pacing moves only edges where the old pacing put them, so a change and its way back give the same cut, and it leaves zooms alone; Review the applied cuts (R) with K restore and C leave cut; a neutral empty state with Record a take; Transcribe with progress and Stop; "Speech marked as a pause" when a gap cut holds words; steady review layout at 1280x800.
- Record: a standby preview of camera, screen and mic before a take, access state per track with a button to the Privacy pane, "Starting the preview.", plain failure alerts with Try again, and a stuck screen stream stopped on timeout.
- Edit: a floating selection toolbar above the selection; zoomed words tinted, Z leaves the zoom picked with scale, seek and Remove in the inspector and a minimum hold; paragraphs that end on sentences and stay put under cuts; keyboard-reachable segmented controls with a focus ring; Tab accepts only a focused proposal; an empty state without takes; scrolling Changes, Overlays and inspector at 1280x800; a quiet Saved.
- Ship: one primary button, cards that scroll below the description.

Not verified on that branch: a real take starting from the standby preview, real denied permissions, a real ScreenCaptureKit timeout, and Edit script with a save in an external editor. No worktrees.

`m13/integration` is finished, checked and not merged: it branches from `polish/council-round1` (so merging it brings that branch too) and holds [milestone 13](milestones/13-takes-scenes-and-studio-tools.md), Mike's 16 change requests of 2026-11-08, built by seven streams and merged, with their worktrees and task branches removed. fmt, workspace clippy and `cargo test --workspace` pass except the flaky watchdog test. [Evidence](evidence/m13-takes-scenes-and-studio-tools.md) lists what each item shows and what was not verified. It holds:
- Cut: a Takes view (⌥2) lines up each take's attempt at every script beat, rates (1 to 5) and picks (P) them, compares two back to back (A) with the differing words lit, and composes the cut with "Use this" (↩) or "Assemble picks" (⇧↩). Ratings live in a `takes` block in `cut.kdl`.
- Scenes: a layout switch inside a take morphs (default 500 ms), dissolves, zooms through or cuts, with a sound, set from its mark on the Layout lane (T). A layout builder (⇧L in Edit) saves channel layouts to the brand.
- Zoom framing (auto, center, four quarters) with ramp in and out per zoom; a hand-held camera drift, 0 to 100, on the brand's camera node.
- Text: effects marked on their words in the lane hues, chips that pick and remove them, Z H S P L from a selection, layout chips inline. Burned-in captions: a Captions panel to edit, split and join lines, a style editor with five presets saved per video or for the channel. Delete works wherever Backspace removes something.
- Library (⌘L): sounds, music, memes and images, overlays (with the channel's presets) and effects, Openverse search with download, credit files and Copy credits, placing at the selection or playhead.
- Theme window (⌘⇧B): every brand field with a live preview, a generator from one color or the logo, several named themes, an overlay preset editor that also opens placed overlays.
- Record: quiet prompter settings behind "More settings", per-track input switches in `format.kdl` with every downstream path coping without a track. Ship: a thumbnail studio with variants and YouTube-size previews that writes `main.png`.

## Uncommitted work

None.

## Running

No app is running; the PIDs in `.demo/pids` are stale. Generated media in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target` (clear its `debug/` when disk runs low). Reels and screenshots in `docs/evidence/` (git-ignored); the README's screenshots are in `docs/assets/`.

## Waiting on Mike

See the [open decisions](milestones/README.md#open-decisions) and [prerequisites](milestones/README.md#prerequisites-from-mike):
- Push `main`, and merge `m13/integration` into it (it carries `polish/council-round1`).
- Decisions 0026 to 0030 and 0032 and the zoom ramp ease, each with a recommendation in the [open decisions](milestones/README.md#open-decisions).
- How studio edits reach a video's branch so the board shows them (known gaps).
- Score the four reels and listen to the bundled sounds and beds.
- The design-variants round for milestone 10 item 7.
- The website domain (postponed).
- The prompter beam splitter check, the YouTube OAuth client, and the org signing secrets shared with `shot-studio` if they are limited to selected repos.

## Next

1. Milestone 13 follow-ups: Ship's description does not include the Library's credits yet, and the bundled Noto emoji (CC BY 4.0) are not credited; CLI and MCP have no takes, use or assemble commands; `shot record` has no switch for a track; Text shows an empty block for a take without a mic; the theme window's Captions section only points to the video's Captions panel; at 1600x1000 the Text inspector's "At" card cuts off the last hints; agents driving several app windows at once sent clicks and keys into each other's windows, so one app per agent or a harness that targets one window is needed next time.
2. The council's follow-ups: the target fixture's hand-placed `by=pause` cuts sit on speech and its cut is not what the rules write, so pacing changes nothing on the demo; durations in the Cut review footer and list rows still show a wide mono space before "s"; at 1280x800 the zoom inspector's "At" card covers its last hint; Picture and Timeline have no empty state without takes; Cut's private `button` duplicates `ui::big_button`.
3. Milestone 10 item 7 after the design round.
4. [Milestone 09](milestones/09-release-gate.md) items 4 and 5: measure every target on the idle reference machine, then one full video from board to a private upload.
