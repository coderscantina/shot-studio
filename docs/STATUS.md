# Status

Updated 2026-11-16. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

`main` is the branch of record and is public at https://github.com/coderscantina/shot-studio. Local `main` holds milestones 01 to 16 and the record rework, and is ahead of `origin/main`, not pushed.

`main` holds the record experience rework (2026-11-16):
- Record screen: an outline pane (whole script, chapters with take counts and coverage, the picked chapter's beats to start from, every take with Review and Remove, Import clip), input chips over the stage, a scene strip (1 to 4) that restyles the monitor and switches scenes while recording. A finished take stays on Record and the part moves to the next unrecorded chapter. `crates/shot-app/src/record/`.
- A retake plays in place of earlier attempts at the same beats (`VideoFolder::append_take`), and removing a take sends its media to the Trash. Contract text in `project-files.md`. Waits for Mike's call: it replaces milestone 13's "the cut plays both until you pick".
- Prompter: one text size with opacity for read and upcoming lines, chapter labels, a column that glides so the line read sits on the reading line; voice follow matches the spoken script in beats mode, writes `next` and `again` markers, and jumps back on a restart heard twice; the script reloads whenever Record shows. Display and mirror under More settings.

`main` holds [milestone 16](milestones/16-script-editor-and-clip-ingest.md), merged from `m16/integration`. The branch and its worktree `.claude/worktrees/m16-integration` are still there, merged. [Evidence](evidence/m16-script-editor-and-clip-ingest.md).
- The Script screen edits `script.md` in the app: Druck's text core in a new `shot-editor` crate, live styling, spell checking, autosave into the worktree, Edit script gone ([0038](decisions/0038-script-editor-from-druck.md), Proposed).
- Script writing held to [the bar](technical-briefing.md#the-bar-for-writing-a-script): suggestions always apply (the proposal wins on conflicting script lines, one Undo back), a run commits the person's script edits first and can be stopped, hooks to pick side by side, a chat that keeps every run with selectable Markdown answers and Copy, chapter prose counted in the outline.
- `shot ingest`, Import clip and file drop make a phone clip the next take, upright, SDR, constant rate, then transcribed and cut like a recording ([0039](decisions/0039-ingested-clips-are-takes.md), Proposed).

`main` holds [milestone 15](milestones/15-one-video-model-transitions-and-themes.md), built by seven streams and merged. [Evidence](evidence/m15-one-video-model.md).
- One kind of video: every video opens on style rules, old ones migrate on open, a lone folder is a channel of one ([0035](decisions/0035-every-video-on-style-rules.md)).
- One transition type for chapter edges and layout switches, one painter path, exact first and last frames ([0036](decisions/0036-one-transition-type.md)).
- Themes as files picked by `theme <id>` in `brand.kdl` or any format layer, `shot theme`, MCP theme and preset tools, presets in rules, a channel lock ([0029](decisions/0029-several-themes-per-channel.md)).
- A worktree per video branch at `.shot/worktrees/<slug>`, migration as its own commit, accept commits a person's edits first, read-only MCP tools write nothing ([0037](decisions/0037-a-worktree-per-video-branch.md)).
- Removing lock overlays and placed sounds, cards inside a Short's safe area, the app's copies of render and core code gone.

fmt, workspace clippy and `cargo test --workspace` pass on `main` after the merge; the voice-follow lag fixture test passes (median -19 ms).

A local bundle from `scripts/bundle.sh` is in `dist/`: `Shot Studio.app` and `Shot-Studio-26.11.7.dmg`, built from `main` after the milestone 16 merge with Mike's uncommitted app icon, signed with his Developer ID (hardened runtime), not notarized. `dist/` is not in git.

## Uncommitted work

Mike's own app icon work: `README.md`, `packaging/macos/AppIcon.icns`, `AppIcon.svg` and `scripts/app-icon.sh`. Not touched by agents.

## Running

No app is running. Generated media is in `$HOME/.cache/shot-studio-fixtures`. The shared build cache is in `$HOME/.cache/shot-studio-target`; clear its `debug/` (29 GB) when disk runs low (about 24 GB free now). Screenshots in `docs/evidence/` are git-ignored.

## Waiting on Mike

- Adopt or change 0038 and 0039, and the retake rule above.
- Push `main`.
- The [open decisions](milestones/README.md#open-decisions): resting-cursor zooms (gone with `shot autozoom`), the card size in Shorts, 0034 (cut-out camera, best judged on a real face), the zoom ramp ease, the four reels, the milestone 10 item 7 design round, the website domain (postponed).
- The [prerequisites](milestones/README.md#prerequisites-from-mike): the prompter beam splitter check, the YouTube OAuth client, the org signing secrets.

## Next

1. Milestone 16 checks only a person can do: Import clip and drop in the app, HDR tone mapping on a real iPhone clip, IME in the editor.
2. Milestone 15 follow-ups (in the known gaps): `shot style scope` and the MCP propose and reach tools on worktrees, callout labels and the code-diff panel in Shorts, Remove for a rule's sound, a decode thread for the transition prefetch, the app's changed-on-disk check.
3. Milestone 13 and council follow-ups: credits for the Library and Noto emoji in Ship's description, takes commands in the CLI and MCP, a track switch for `shot record`, cut points at take rates other than 60 fps, Text and Picture empty states, the 1280x800 inspector hints.
4. [Milestone 09](milestones/09-release-gate.md) items 4 and 5: measure every target on the idle reference machine, then one full video from board to a private upload.
