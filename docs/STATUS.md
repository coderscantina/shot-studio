# Status

Updated 2026-11-19. Read by agents at the start of a thread; rewrite it before handing back.

## Branches

`main` is the branch of record, public at https://github.com/coderscantina/shot-studio. Local `main` holds milestones 01 to 17, the record rework and the Cut and Edit pass, and is ahead of `origin/main`, not pushed. It is the only local branch and has no other worktrees.

`main` holds [milestone 17](milestones/17-the-cut-proposes-itself.md), merged as `5811a7f`. [Evidence](evidence/m17-the-cut-proposes-itself.md), decision [0040](decisions/0040-the-cut-proposes-itself.md).
- The best scored attempt plays every beat, sentence by sentence. The Cut step is gone: Record, Edit, Ship. Text counts what is worth a look (N, K, C) and switches a beat's attempt in place (A).
- Council review of the editing flow, 13 rounds on 2026-11-18. Round 13 at `53e757a`: Claude Opus 9.5, GPT Sol 9.5, no must-fixes. Grok 4.7 gave 9.5 in round 5 at `98232bb` and could not review after that, its usage balance is exhausted.
- fmt and clippy pass. `cargo test --workspace` at `b9011cb`: 683 passed, none failed. The `metrics` watchdog and `shot-editor` timing tests can fail under load.
- Never run on milestone 17: a live recording with the prompter, a real voice, mouse-driven Text and Timeline, a real Ship upload, a bench.

A local bundle is in `dist/`: `Shot Studio.app` and `Shot-Studio-26.11.7.dmg`, built on 2026-11-19 from `b9011cb` with Mike's uncommitted app icon, signed with his Developer ID (hardened runtime), not notarized. `dist/` is not in git.

Mike confirmed on 2026-11-19: the new `takes` nodes in `cut.kdl`, the retake rules in the contract and 0040, and that composing stays refused while a Short uses a take the main cut would drop.

## Uncommitted work

On `main`: Mike's app icon work in `README.md`, `packaging/macos/AppIcon.icns`, `packaging/macos/AppIcon.svg` and `scripts/app-icon.sh`. Not touched by agents.

## Running

None. Generated media is in `$HOME/.cache/shot-studio-fixtures`, the shared build cache in `$HOME/.cache/shot-studio-target`. Free disk is about 51 GB.

## Waiting on Mike

- A pacing switch and rule switches in Text, which would write the video's `format.kdl` layer. The app never had them.
- A slimmer floating word toolbar in Text (Cut, Restore and one effects menu, keys unchanged).
- Adopt or change 0038 and 0039.
- Push `main`.
- The [open decisions](milestones/README.md#open-decisions) and the [prerequisites](milestones/README.md#prerequisites-from-mike).

## Next

1. Minor findings of council round 13, in `crates/shot-core/src/takes.rs`:
   - On a short beat a closing "Oops." still makes the flubbed take win, marked.
   - A retake that adds one content word can drop a one-word answer, unmarked.
   - "Now we wait." can be dropped by a partial retake, unmarked, since "wait" is exempt.
   - "Sorry." is no acknowledging word, so the flubbed take wins, marked.
   - A scripted pickup that corrects "8K" to "4K" does not replace the wrong sentence, marked.
2. Milestone 17 checks only a person can do: a prompter recording with Again, a real voice, Timeline drags of a blur across a splice, a real Ship upload.
3. Milestone 17 follow-ups in the [known gaps](milestones/README.md): the flaky timing tests, a fixture clip that retakes half a beat, and `README.md` with its review queue screenshot.
4. [Milestone 09](milestones/09-release-gate.md) items 4 and 5: measure every target on the idle reference machine, then one full video from board to a private upload.
