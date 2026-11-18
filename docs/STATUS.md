# Status

Updated 2026-11-18. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

`main` is the branch of record, public at https://github.com/coderscantina/shot-studio. Local `main` holds milestones 01 to 16, the record rework and the Cut and Edit pass, and is ahead of `origin/main`, not pushed.

`m17/integration` holds [milestone 17](milestones/17-the-cut-proposes-itself.md), 82 commits ahead of `main`, not merged. Worktree: `.claude/worktrees/m17-integration`. [Evidence](evidence/m17-the-cut-proposes-itself.md), decision [0040](decisions/0040-the-cut-proposes-itself.md).
- The best scored attempt plays every beat, sentence by sentence. The Cut step is gone: Record, Edit, Ship. Text counts what is worth a look (N, K, C) and switches a beat's attempt in place (A).
- Council review of the editing flow, 13 rounds on 2026-11-18. Round 13 at `53e757a`: Claude Opus 9.5, GPT Sol 9.5, no must-fixes. Grok 4.7 gave 9.5 in round 5 at `98232bb` and could not review after that, its usage balance is exhausted.
- fmt and clippy pass. `cargo test --workspace`: 680 passed, 1 failed. The failure is the `metrics` watchdog timing test, which this branch does not touch and which passed three of three runs alone. The `shot-editor` timing test also fails under load.
- Never run on this branch: a live recording with the prompter, a real voice, mouse-driven Text and Timeline, a real Ship upload, a release build or bench.

`m16/integration` is merged into `main`. Its branch and worktree `.claude/worktrees/m16-integration` can go.

A local bundle in `dist/` was built on 2026-11-17 from the tree that is now `235f136`, with Mike's uncommitted app icon. It holds none of milestone 17.

## Uncommitted work

On `main`: Mike's app icon work in `README.md`, `packaging/macos/AppIcon.icns`, `packaging/macos/AppIcon.svg` and `scripts/app-icon.sh`. Not touched by agents. `m17/integration` is clean.

## Running

None. Generated media is in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target`. Free disk is about 8 GB, so clear the cache's `debug/` before the next long build. Its `release/` was cleared on 2026-11-18, so the next release build starts cold.

## Waiting on Mike

- Merge `m17/integration` into `main`. Recommended after one real recording, since the council could only test generated speech.
- Confirm the new `takes` nodes in `cut.kdl`: `remove`, `looked`, `undone`, `take <take> { layout, zoom, overlay }` and `override`. Recommended: keep them, they hold a person's work on attempts that left the cut.
- Confirm the retake rules in the Takes section of [the contract](contracts/project-files.md) and in 0040: the half rule for retakes, the placement tables, the acknowledging words, and that any word with a digit is a number, also with a script.
- Decide whether the main cut may drop a take a Short still uses. Today composing is refused with a visible error. Recommended: keep that, lifting it changes the engine, the lock and three contract rules.
- A pacing switch and rule switches in Text, which would write the video's `format.kdl` layer. The app never had them.
- A slimmer floating word toolbar in Text (Cut, Restore and one effects menu, keys unchanged).
- Adopt or change 0038 and 0039.
- Push `main`.
- The [open decisions](milestones/README.md#open-decisions) and the [prerequisites](milestones/README.md#prerequisites-from-mike).

## Next

1. Minor findings of council round 13, in `crates/shot-core/src/takes.rs`. Fix the first before merging, it came in with the fix for "No.":
   - A whole unscripted retake of "... production? No." plays "No." twice, both marked (`numbered` lacks the one-word rule of `Said`).
   - On a short beat a closing "Oops." still makes the flubbed take win, marked.
   - A retake that adds one content word can drop a one-word answer, unmarked.
   - "Now we wait." can be dropped by a partial retake, unmarked, since "wait" is exempt.
   - "Sorry." is no acknowledging word, so the flubbed take wins, marked.
   - A scripted pickup that corrects "8K" to "4K" does not replace the wrong sentence, marked.
2. Milestone 17 checks only a person can do: a prompter recording with Again, a real voice, Timeline drags of a blur across a splice, a real Ship upload.
3. Milestone 17 follow-ups in the [known gaps](milestones/README.md): the flaky timing tests, a fixture clip that retakes half a beat, and `README.md` with its review queue screenshot.
4. [Milestone 09](milestones/09-release-gate.md) items 4 and 5: measure every target on the idle reference machine, then one full video from board to a private upload.
