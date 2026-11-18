# Milestone 17: The cut proposes itself

## Outcome

Mike's request of 2026-11-18, after the [review of cutting and takes](../STATUS.md): one take, or a scripted video in several takes, becomes a proposed final cut without assembling it by hand. The automation picks the best attempt at each beat, takes single sentences from another attempt where that helps, cuts restarts, and shows in the transcript what is worth a look ([0040](../decisions/0040-the-cut-proposes-itself.md)).

## Dependencies and ownership

Builds on `main` at `ce65f7b`. Three slices in order on `m17/integration`, each usable on its own. Slice 1 runs as two streams.

| Stream | Items | Owns |
| --- | --- | --- |
| 1A detector | 1 to 4 | `crates/shot-analyze/src/elements/findings.rs`, `mentions.rs`, the `Voice` argument in `elements/mod.rs`, a `takes` group in `scripts/fixtures.sh` and `fixtures/spec/`, `fixtures/README.md` |
| 1B pick | 5 to 9 | `crates/shot-core/src/takes.rs`, `edit.rs`, `cut.rs`, `engine.rs`, `crates/shot-app/src/folder.rs` and `takes/`, `crates/shot-cli/src/ingest.rs`, the contract's Takes section |
| 2 transcript | 10 to 13 | `crates/shot-app/src/edit/`, `model.rs`, `keymap.rs`, `docs/design.md` |
| 3 nav | 14 to 16 | `crates/shot-app/src/studio.rs`, `cut/`, `takes/`, `AGENTS.md` |

## Work

Slice 1, the engine:

1. **Restarts.** A sentence said again is found when its words line up with a later passage after aligning them, not by position, so an added, dropped or changed word does not hide it. A restart needs no full stop from Whisper and has no six word cap. A short sentence in between ("It is, hmm, wait.") does not end the search. "Let me say that again" and its kin are cues: the cue and the passage it repeats go.
2. **Deliberate repetition.** A doubled single word asks (below the presets' 85) unless it is a function word ("the the"). A repeat across a sentence end is no stumble.
3. **Numbers are no links.** "13.53" is not a URL.
4. **Kept fillers teach.** `analyze` passes the fillers a person kept in this video's overrides as the `Voice`.
5. **Alignment.** A take's sentences may go back to an earlier beat, which makes a second attempt there. Words before a retake's first matched beat belong to no beat. Without beats, a later take's passages compete only where they repeat an earlier take's wording, and the rest becomes new rows after the rows there are, in recording order, so a take that carries on is appended. Only a sentence added or reworded between two repeated ones goes right after the first of them, in that row's chapter. The rows a take skips there and the ones it added are worth a look, as is one row it leaves out with nothing new. A take that says again half of its sentences or more is a retake: what it says before the first row it says again goes before that row and into its chapter, what it says between two rows it says again goes after the first, and those and what it says after the last are worth a look, with the rows it leaves unsaid there when they are at most one for each new sentence.
6. **Score and proposal.** `Lineup` scores each attempt and proposes the pieces that play a row, as [0040](../decisions/0040-the-cut-proposes-itself.md) says. Pieces reach into the gaps around their words.
7. **Compose.** One `Cut` function builds the cut from the proposals, a person's picks and the beats a person edited. The app runs it after a take and after its analysis, `shot ingest` after analysis. A take whose recording was undone is marked in `cut.kdl` and stays out until redo. Stars leave the model.
8. **Findings at a block's start.** A finding that overlaps kept material is cut or queued, also when it starts just before the block.
9. **Fixtures.** The four spoken clips of the review (`one-take`, `script-full`, `script-chapter`, `script-beats`) are generated, with the cuts and picks expected of them as ground truth in `#[ignore]`d tests.

Slice 2, the transcript:

10. **Marks.** Text marks restarts that were cut, unsure cuts, splices, close beats and rows a later take skipped or reworded, with "N to look at" and one key to the next. Keep, Cut and switching answer an item.
11. **Attempts in place.** A beat with other attempts shows "2 of 3". One key steps to the next attempt, plays it and writes the pick.
12. **Text cutting faults** from the review: Backspace takes over a rule's cut, restore leaves the pause out, a stale selection or picked chip, Backspace moves forward, the first arrow press ignores its direction, cutting a chapter's last word jumps on, "0:00" for short cuts.
13. **The Takes detail view** opens from a beat, keeps compare, loses stars and Assemble.

Slice 3, the nav:

14. **No Cut step.** Record, Edit, Ship. A Recorded card opens Edit.
15. **What the rules cut** by kind in Text. A pacing switch and rule switches are left open: the Cut screen had none, they are the format's style rules.
16. **Docs.** AGENTS.md, the briefing and the design document say what the app does now.

## Acceptance criteria

- On the `one-take` fixture the lock cuts all three sentence restarts and the spoken cue, asks about "very, very" and places no link card.
- On the three script fixtures, ingested in order, `cut.kdl` plays every beat once, under the script's chapters, with the clean retake of "Add the clap crate" and of "Clap gives us help text", and no attempt that holds a restart.
- A retake of the second sentence of a beat replaces that sentence and keeps the first.
- A pick survives a later, better scored take. One undo takes back a proposal.
- In the app: a beat is switched from the transcript by key, the count goes down on an answer, and the nav has no Cut step. Screenshots at 1600x1000.
- fmt, clippy and the workspace tests pass.

## Evidence

`docs/evidence/m17-the-cut-proposes-itself.md`, screenshots in `docs/evidence/m17/`.
