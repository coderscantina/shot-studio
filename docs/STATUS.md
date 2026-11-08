# Status

Updated 2026-11-08. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

`main` is the branch of record and is public at https://github.com/coderscantina/shot-studio. Local `main` is ahead of `origin/main` and not pushed: it holds milestones 01 to 08, 10 to 12 built except what waits on Mike, milestone 09's release machinery, and the polish wave of 2026-11-08. `polish/integration` is merged and can be deleted.

Three branches are stacked, each finished, checked and not merged. Merging the top one brings all three:
- `polish/council-round1` (28 commits): the council UX review: a video opens on its next step, Cut review (R), the Record standby preview, the Edit selection toolbar and zoom picking, one primary button in Ship.
- `m13/integration` (on top of it): [milestone 13](milestones/13-takes-scenes-and-studio-tools.md), Mike's 16 change requests: the Takes view and assembly, scene switches, zoom framing and ramps, hand-held camera, text effects, burned-in captions, Library with Openverse, the theme window, input switches, the thumbnail studio. [Evidence](evidence/m13-takes-scenes-and-studio-tools.md).
- `m14/integration` (on top of it): [milestone 14](milestones/14-hardening-from-filmcraft-and-effectcraft.md), ideas taken from FilmCraft and EffectCraft, built by six streams and merged:
  - Project files go through `shot-fs`: written with a flush and an atomic rename, under a per-video lock.
  - `version` nodes refuse files from a newer build.
  - Word ends are re-measured against the audio.
  - MCP tools carry the four hints, take strict arguments, survive a panic and confine paths.
  - One keymap, and disabled buttons say why.
  - A control socket (`SHOT_CONTROL=1`) drives one window.
  - A cut-out face cam from Vision masks.
  - Golden failure images, Lottie import warnings, a parser deadline test, and `cargo xtask bench`.
  - [Evidence](evidence/m14-hardening.md).

fmt, workspace clippy and `cargo test --workspace` pass on `m14/integration` except the flaky watchdog test. No worktrees.

## Uncommitted work

Mike's own app icon work: `README.md`, `packaging/macos/AppIcon.icns`, `AppIcon.svg` and `scripts/app-icon.sh`. Not touched by agents.

## Running

No app is running; the PIDs in `.demo/pids` are stale. Generated media is in `$HOME/.cache/shot-studio-fixtures`. The shared build cache is in `$HOME/.cache/shot-studio-target`; clear its `debug/` when disk runs low (32 GB free now). Screenshots in `docs/evidence/` are git-ignored.

## Waiting on Mike

See the [open decisions](milestones/README.md#open-decisions) (11 lines) and [prerequisites](milestones/README.md#prerequisites-from-mike):
- Push `main`, and merge `m14/integration` into it (it carries `m13/integration` and `polish/council-round1`).
- Decisions 0026 to 0030, 0032, 0033 (project file versions) and 0034 (cut-out camera, best judged on a real face first), and the zoom ramp ease.
- How studio edits reach a video's branch so the board shows them.
- Score the four reels and listen to the bundled sounds and beds.
- The design-variants round for milestone 10 item 7. The website domain (postponed).
- The prompter beam splitter check, the YouTube OAuth client, and the org signing secrets.

## Next

1. Milestone 14 follow-ups (in the known gaps):
   - Zoom-through and morph should start exactly on the outgoing frame.
   - The app needs a changed-on-disk check, so it doesn't overwrite a change `shot` made while the video was open.
   - The cut-out needs a look on a real face and an automatic `shot analyze` when it is switched on for old takes.
   - Autocut with the new word bounds needs a run on the full target project.
2. Milestone 13 and council follow-ups:
   - Ship's description lacks the Library's credits, and the bundled Noto emoji aren't credited.
   - The CLI and MCP have no takes commands.
   - `shot record` has no track switch.
   - Cut points at take rates other than 60 fps.
   - Text and Picture empty states.
   - The 1280x800 inspector hints.
3. [Milestone 09](milestones/09-release-gate.md) items 4 and 5: measure every target on the idle reference machine, then one full video from board to a private upload.
