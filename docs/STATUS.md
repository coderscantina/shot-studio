# Status

Updated 2026-11-08. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

`main` is the branch of record and is public at https://github.com/coderscantina/shot-studio. It holds milestones 01 to 08, milestones 10 to 12 built except what waits on Mike, and milestone 09's release machinery. fmt, clippy and `cargo test --workspace` pass. No other branches, no worktrees.

## Uncommitted work

On `main`: the welcome window (`crates/shot-app/src/welcome.rs`) that the app opens without a folder: open a video folder (⌘O), create one from a format preset, reopen a recent one. Checked with clippy, `cargo test -p shot-app` and clicks in the real app. Waits for Mike's review and a commit.

Also on `main`, uncommitted on top of the welcome window: an editable Timeline and undo.
- Undo and redo (⌘Z, ⇧⌘Z and an Edit menu) for every write of `cut.kdl`, on every screen (`VideoFolder::undo`, `redo`).
- `shot-core` gains `Cut::add_zoom`, `set_range` and `remove_placed` over a `Placed` zoom or overlay.
- The Timeline lanes edit: click or drag the ruler or a lane to move the playhead, drag across the lanes to select words, Z adds a zoom over the selection, click a zoom or overlay to pick it, drag its edges to trim and its middle to move (snapping to words and the playhead), Backspace removes it, click a cut seam to pick it and R restores it. `=`, `-` and ⇧Z zoom the lanes, scrolling pans, ⌘-scroll and pinch zoom, playback pages the view. A toolbar under the lanes shows what the selection or the pick can do.
- The Text toolbar's Cut, Restore and a new Zoom are clickable. The Timeline's top row shrinks on short windows, and the workspace switcher shows icons only below 1500 px so the title bar fits at 1280.

Checked with fmt, workspace clippy, `cargo test --workspace` (333 passed) and real clicks on the target project at 1600x1000 and 1280x800. Waits for Mike's review and a commit.

## Running

The demo app on `/tmp/shot-demo` (a copy of the target project), PID in `.demo/pids`. Generated media in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target` (clear its `debug/` when disk runs low). Reels and screenshots in `docs/evidence/` (git-ignored); the README's screenshots are in `docs/assets/`.

## Waiting on Mike

See the [open decisions](milestones/README.md#open-decisions) and [prerequisites](milestones/README.md#prerequisites-from-mike):
- Score the four reels and listen to the bundled sounds and beds.
- The design-variants round for milestone 10 item 7.
- The website domain (postponed).
- The prompter beam splitter check, the YouTube OAuth client, and the org signing secrets shared with `shot-studio` if they are limited to selected repos.

## Next

1. Milestone 10 item 7 after the design round.
2. [Milestone 09](milestones/09-release-gate.md) items 4 and 5: measure every target on the idle reference machine, then one full video from board to a private upload.
