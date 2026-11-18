# Status

Updated 2026-11-18. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

`main` is the branch of record and is public at https://github.com/coderscantina/shot-studio. Local `main` holds milestones 01 to 16 and the record rework, and is ahead of `origin/main`, not pushed.

`main` holds precise cuts by text, transcript menus and record changes (2026-11-17), commits from `4ca5097` up to the take Remove fix, not pushed:
- Word edges grow into the gaps where the audio still sounds (`word_bounds`), since Whisper starts a word after a pause up to 200 ms late and a cut there left its first syllable. `shot analyze` measures cached transcripts again against `mic.mp4` (stable, about 1 s a take).
- One cut rule in `shot_core::word_cut`: Delete takes the words and the longer pause around them, edges 80 ms into the gaps. Retake findings cut just before the kept word the same way.
- The restart detector matches words that sound alike ("failing", "falling") and treats one changed word after a shared opening of three as sure (85).
- Removing a rule's zoom: `zoom off` on its element, from the Timeline lane menu and from Remove or Delete on a style chip in Text (overlays and sounds there too).
- Record: the camera preview is 1920x1080 instead of 640x360 (`shot-capture` tap), the prompter can show the camera behind its text (Camera switch, with its visibility in the countdown and while reading under More settings, `prompter.json`), scrolling over the prompter output or its preview moves it by lines (`Prompter::step`), and the outline nests each take under the whole script or its chapter, says which take plays a chapter, and names the next take a retake. Checked in the bundled app by screenshot; trackpad scrolling only by its unit test.
- Right-click menus in Text, shared with the Timeline's (`lane_menu`): words, effect chips, cut chips and seams, layout switch chips and empty space. Copy text (⌘C) is new. Not checked in the running app yet: the control socket cannot right-click.

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

A local bundle from `scripts/bundle.sh` is in `dist/`: `Shot Studio.app` and `Shot-Studio-26.11.7.dmg`, built from `main` after the milestone 16 merge with Mike's uncommitted app icon, signed with his Developer ID (hardened runtime), not notarized. `dist/` is not in git. Rebuilt 2026-11-17 from the tree that is now `235f136`, with Mike's uncommitted app icon.

## Uncommitted work

Mike's own app icon work: `README.md`, `packaging/macos/AppIcon.icns`, `AppIcon.svg` and `scripts/app-icon.sh`. Not touched by agents.

The Cut and Edit council pass (word cursor in Text, picked cut chips, the loose video's next step) is on `main` as `235f136` since 2026-11-18, not pushed.

## Review of cutting and takes, 2026-11-18

Two reviewers read the core and the app, and two spoken test videos went through `shot ingest` and the bundled app (one unscripted take with restarts, three takes of a seven beat script). Nothing is fixed yet. What it found, most important first:
- Assemble picks without ratings takes the longest attempt per beat (`Cut::choice` falls back to `kept_of`), which was the flubbed one in both beats that had a clean retake. Its banner says "from your picks" with none made.
- The restart detector missed three of three sentence restarts: an 8 word restart without a full stop (`repeats` stops at 6 words, `restarts` needs sentence ends from Whisper), a restart after a short sentence in between (`break` at `findings.rs:369`), and "Sorry, let me say that again" with a reworded sentence (words compared by position). "very, very good" is cut at 85 without asking.
- A number said as "13.53" becomes a link card to `https://13.53` (`mentions.rs`).
- `shot ingest` stacks every take as its own chapter, the app's Import and Record replace repeated beats (`VideoFolder::replace_retaken`). A retake that opens with unscripted words can replace beat 1, and a retake of part of a beat drops the rest of it (`takes::align`, `Cut::use_attempt`).
- Joins between takes sit on bare word edges with no pause; a finding that starts just before a block's first word is neither cut nor queued (`engine.rs` `is_kept(e.t)`).
- Restore from a word selection leaves the pause removed; Backspace next to a mixed chip turns a rule's cut into a person's (`model::cut_words_before`); a stale selection or picked chip survives Use this and undo.
- Kept fillers never lower a filler's confidence: `analyze` passes `Voice::default()`.

## Running

No app is running. Generated media is in `$HOME/.cache/shot-studio-fixtures`. The shared build cache is in `$HOME/.cache/shot-studio-target`; clear its `debug/` when disk runs low (cleared 2026-11-17, 52 GB freed). Screenshots in `docs/evidence/` are git-ignored.

## Waiting on Mike

- Decide what to build from the review above: a proposal that picks attempts by quality, and the restart detector fixes come first.
- Decide on a slimmer floating word toolbar in Text (Sol: Cut, Restore and one effects menu, keys unchanged).
- Adopt or change 0038 and 0039, and the retake rule above.
- Push `main`.
- The [open decisions](milestones/README.md#open-decisions): resting-cursor zooms (gone with `shot autozoom`), the card size in Shorts, 0034 (cut-out camera, best judged on a real face), the zoom ramp ease, the four reels, the milestone 10 item 7 design round, the website domain (postponed).
- The [prerequisites](milestones/README.md#prerequisites-from-mike): the prompter beam splitter check, the YouTube OAuth client, the org signing secrets.

## Next

1. Milestone 16 checks only a person can do: Import clip and drop in the app, HDR tone mapping on a real iPhone clip, IME in the editor.
2. Milestone 15 follow-ups (in the known gaps): `shot style scope` and the MCP propose and reach tools on worktrees, callout labels and the code-diff panel in Shorts, Remove for a rule's sound, a decode thread for the transition prefetch, the app's changed-on-disk check.
3. Milestone 13 and council follow-ups: credits for the Library and Noto emoji in Ship's description, takes commands in the CLI and MCP, a track switch for `shot record`, cut points at take rates other than 60 fps, Text and Picture empty states, the 1280x800 inspector hints.
4. Council follow-ups for Cut and Edit: one continuous transcript across chapters (the members' next pick), a selection across take blocks, reopening an answered Cut row (needs `Cut::decide` to take a decided item), rule findings inline in Text, recheck a picked cut after undo.
5. [Milestone 09](milestones/09-release-gate.md) items 4 and 5: measure every target on the idle reference machine, then one full video from board to a private upload.
