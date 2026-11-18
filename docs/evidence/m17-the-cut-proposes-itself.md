# Milestone 17: the cut proposes itself

Source: `m17/integration` at the merge of `m17/text-faults` (`e5b7622`), built 2026-11-18 on an Apple M1 Pro, macOS 27.0.1, debug profile. Decision: [0040](../decisions/0040-the-cut-proposes-itself.md).

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass. `bash -n scripts/fixtures.sh` passes, `shellcheck` and the `takes` group were run by the detector stream. The ignored fixture tests `one_take_finds_its_restarts_and_cue_and_asks_about_emphasis` and `three_takes_of_a_script_play_every_beat_once_with_the_clean_retakes` pass with `SHOT_FIXTURES` set.

## Media

Four clips spoken by `say` (Samantha) over a test pattern, the texts now in `fixtures/spec/` as the `takes` group: `one-take` (54 s, unscripted, three restarts, a spoken cue, fillers, "very, very", a corrected number) and `script-full`, `script-chapter`, `script-beats` (34 s, 16 s, 9 s) for the seven beat script in `fixtures/spec/takes-script.md`. Each went through `shot ingest <clip>.mp4 --video <dir>`.

## One take

| | Before (`ce65f7b`) | After |
| --- | --- | --- |
| "This makes the project a lot easier to. This makes ..." (8 words) | missed | cut, 92 |
| "Now the second change ... Sorry, let me say that again. The second big change ..." | missed, the cue stays | cut with the cue, 90 |
| "The the", "the the" | cut | cut |
| "very, very good" | cut at 85 without asking | asks, 60 |
| "13.53, sorry, 13.35" | cut, and two link cards to `https://13.53` | cut, no link card |
| Length | 39 s of 54 s | 30 s of 54 s |

The transcript after the cuts reads: "So today we look at the new router in version 12. the first thing you notice is, that the config file is gone. the roots now live next to the pages. this makes the project a lot easier to read, because every page owns its root. The second big change is the loader. It runs on the server before the page renders. I think, you know, this is basically the best release in years. Performance is very, very good. The build ran in 13.35 milliseconds. That is the whole update. See you next week."

## Three takes of a script

Before, `shot ingest` stacked the takes as three chapters, 45 s that said most beats two or three times, and "Assemble picks" took the flubbed attempt of both retaken beats. After the third ingest `cut.kdl` is:

```kdl
chapter "Intro" {
    take t01 {
        keep from=0 to=9_460_000
    }
}

chapter "Arguments" {
    take t02 {
        keep from=0 to=2_820_000
    }
    take t03 {
        keep from=0 to=9_090_000
    }
}

chapter "Colors" {
    take t01 {
        keep from=27_240_000 to=34_270_000
    }
}
```

Every beat plays once under the script's chapters, 24 s. "Add the clap crate" and "Clap gives us help text" play the clean third take. The attempts at the last of these score 97 (take 1, a filler), 70 (take 2, a restart) and 100 (take 3).

## In the app

Driven through the control socket, screenshots at 1600x1000 in `m17/`:

- `one-take-open.png`, `one-take-restart.png`, `one-take-next.png`: the video opens on Edit with "8 to look at"; N goes to the first cut restart with its chip picked and plays around it; C confirms it and N moves on. Earlier in the day K on a cut restart wrote `override t01 retake at=11_140_000 { keep }` and the words played again.
- `scripted-open.png`, `scripted-beat.png`: the nav is Record, Edit, Ship; beats with other attempts start with "2 of 2" and "3 of 3", and the inspector lists the attempts with their scores.
- `scripted-switched.png`: A on "Clap gives us help text" played take 1 in place and wrote `attempt t01 from=20_400_000 to=25_530_000 pick=#true`; the marker reads "1 of 3" and the count dropped.
- `scripted-takes.png`: shift-A opens the takes side by side on that beat, with Back to Edit.

## Fixes before the review

Six faults found while building were fixed on `m17/integration` after the report, each with a test:

- A word cut stops at a rule's cut in the pause it takes (`Cut::delete_words` reads the lock).
- A person's cuts in an attempt that leaves the cut wait as `remove <take>` nodes of the `takes` block and come back with the attempt ([contract](../contracts/project-files.md#takes)). This adds a node to `cut.kdl`.
- Without beats, a renamed chapter stays whole when a later take's passage plays in it.
- The app reads a take from disk once for each time its transcript or elements file is written, not on every rebuild of Text.
- `shot check` prints `info: N to look at in Edit (N), which does not stop a ship` for the main cut. Its exit code and problem count are unchanged. On the three ingested takes it printed 1 beside 6 problems of the missing export.
- A chapter's "Cut out" line counts the rules' cuts with a person's. On the three ingested takes "Intro" read "1.4 s in 2 cuts" where it read "Nothing yet", seen in the debug app by screenshot.

The picks of the three script takes are now the ignored test `crates/shot-cli/tests/takes.rs`: it muxes each clip over a test pattern with ffmpeg, runs `shot ingest` three times and expects the chapters and takes of the `cut.kdl` above, one playing attempt per beat and no restart in kept media.

## Fixes after the review

Three reviews of `1f3cb45` found 19 faults. All are fixed on `m17/integration`, each testable one with a test, checked 2026-11-18 with fmt, workspace clippy, `cargo test --workspace` (630 passed) and the three ignored fixture tests.

- The pick: the frame of a proposal is the best scored attempt among those that say the beat whole, so a retake of one sentence is spliced in and never replaces the beat. The score counts the beat's script. A sentence replaces another only when it says all its content words and is as clean, fillers counted. A flubbed sentence can be replaced. Without a script, only a sentence that repeats a sentence competes (`takes::tests`).
- Switching: a person's layouts, zooms, overlays and override answers in material that leaves the cut wait in the `takes` block as `take` blocks and `override` nodes and come back with it ([contract](../contracts/project-files.md#takes)).
- Composing: the app composes after a take was transcribed in Edit, after a failed analysis, and on open when a take the cut plays was analyzed after `cut.kdl` was written. The analysis of a take whose recording was undone no longer brings it back. A cut a Short keeps from composing says why, in Edit, Record and `shot ingest` (`folder::tests`).
- Review keys: N, K, C and A act only in Text, K and C leave an item the playhead moved off, A steps from a splice to the first attempt whole, through the attempts and back to the splice, K and C on a beat write `looked` nodes and no pick, an unsure finding that crosses a block edge is queued with what plays of it, the keys stand above the attempts, and a Recorded card says "N to look at" from the same count as Edit.
- The detector keeps the word that opens the second attempt of a restart: "This parser reads ... today. The parser reads ..." cuts the first sentence and keeps "The".
- Ship plans again when the folder changes and when a run starts, so the description and chapter times are those of the cut it exports.

Seen in the debug app on the three ingested takes, at 1600x1000: `m17/review-beat-keys.png` shows "Clap gives us help text" with three attempts and K, A and ⇧A above them, and `m17/review-beat-answered.png` the same video after K, which wrote `looked t03 from=3_690_000 to=9_090_000`, no pick, and took the count away.

A Short still keeps the main cut from dropping a take it uses, and since the second review also from dropping everything one of its clips plays. A Short is cut from the main cut's segments and the lock, which the rules build only for material the main cut keeps, so letting the take go needs the engine and three contract rules to change. That is left to Mike.

## Fixes after the second review

Three reviews of `5a3ec49` scored it 9.0, 8.5 and 8.5 and found 13 faults. All are fixed on `m17/integration`, checked 2026-11-18 with fmt, workspace clippy, `cargo test --workspace --no-fail-fast` (640 passed) and the three ignored fixture tests of `shot-cli --test takes` and `shot-analyze --test findings`.

- Privacy: an overlay keeps its range and plays in every block of its take whose media it overlaps, so a blur from 2 to 8 s still covers 5 to 8 s when a retake plays up to 5 s. The part that left the cut waits in the `takes` block, and changing or removing the overlay changes every copy (`edit::tests::a_blur_that_starts_before_a_splice_covers_what_plays_after_it`).
- A retake that would leave a Short's clip with nothing is refused like one that drops its take (`a_retake_that_would_empty_a_short_is_refused`).
- Undo of a recording writes `undone <take>` in the `takes` block, redo removes it, and a marked take is in no lineup: not in the app after a relaunch, not in `shot ingest`. Redo composes as a change of its own (`folder::tests::an_undone_take_stays_out_until_redo_which_composes_it`). This adds a node to `cut.kdl`.
- Removing a Short composes: in the app in the change that removes its clips, in `shot derived remove` after the write (`folder::tests::a_cut_a_short_keeps_from_composing_says_why`, `shot-cli --test derived`).
- A ship run plans, renders and checks with the one lock it read with the project (`shot_ship::lock::tests::a_plan_follows_the_lock_read_with_the_project`).
- The pick: a sentence replaces another only when it says the needed words outside its own stumbles, a sentence as much like two of the frame's says the cleaner one again, and without a script a sentence added between two repeated ones is a row of its own right after theirs (`takes::tests`). It plays after that row's sentences, not between them: a row is not split.
- A after the last attempt plays the proposal also when a word was cut in the attempt that played, and the cut stays with that attempt (`going_back_to_the_proposal_wins_over_a_cut_made_in_the_attempt_that_played`).
- The detector keeps the word that opens a restart after "say that again," with a comma (`a_comma_after_a_cue_keeps_the_word_that_opens_the_restart`).
- Edit shows why a cut was not composed for as long as the folder says so. The "At 00:13" card is hidden while a beat's attempts show.
- The demo's last block ends at 34.27 s, not 34.36 s: the last word ends at 34.19 s and the edge pad is 80 ms.

Seen in the debug app on the three ingested takes, at 1600x1000: `m17/review2-beat-attempts.png` shows "Clap gives us help text" after N with its three attempts, the third lit, and no card under them, and `m17/review2-no-beat.png` the card back on the intro.

## Fixes after the third review

Three reviews of `610d33c` scored it 9.0, 8.8 and 9.0 and found nine faults. All are fixed on `m17/integration`, checked 2026-11-18 with fmt, workspace clippy, `cargo test --workspace --no-fail-fast` (647 passed) and the three ignored fixture tests of `shot-cli --test takes` and `shot-analyze --test findings`.

- Privacy: placing, moving or lengthening an overlay goes through one core function, `Cut::spread_overlay`, which the app, the overlay editor, `shot overlays`, `shot code-diff` and the MCP server all reach. With `t01` playing 0 to 4 s and 8 to 12 s around another take, a blur lengthened from 3 s to 11 s is in both blocks and waits in the `takes` block for the media between them; moved to 9 s the first block loses its copy (`edit::tests::a_blur_placed_or_lengthened_after_a_splice_covers_every_block_it_overlaps`). `shot overlays bleep|blur` and the b-roll file of a slot went past the core before and go through it now.
- Redo of a recording composes as a step of its own and keeps the steps still to redo, less the one that composed the same cut, so a word cut made after the recording comes back with the next redo (`folder::tests::redo_past_a_recording_keeps_the_edits_made_after_it`).
- Removing a take on Record composes a video that has not shipped from the takes that stay, so the beat only the removed retake played goes back to the earlier attempt (`folder::tests::removing_a_retake_gives_its_beat_back_to_the_earlier_attempt`). In tests `trash` deletes instead of moving to the Trash.
- The inspector, K and C share one target (`Looks::target`): the item N went to, then the beat that plays at the playhead while it is one to look at, then the first finding within a second. A finding near the playhead shows above the beat it is in, and the beat's K hint shows only while K answers the beat (`edit::looks::tests::the_inspector_and_k_answer_the_beat_before_a_restart_near_the_playhead`).
- A composed cut is checked against the timeline before it is written: each clip of a Short that played with the old lock must play something in the composed cut with the lock that cut builds (`Cut::check_derived`, `shot_mcp::style::lock_with`, `shot_mcp::takes::keeps_shorts`). The app's compose, A, the takes view's pick, `shot ingest` and `shot derived remove` all check (`edit::tests::a_splice_that_leaves_a_short_only_a_restart_the_rules_cut_is_refused`, `folder::tests::a_composed_cut_that_leaves_a_short_only_what_the_rules_cut_is_refused`).
- "The cut was not composed" lasts as long as the fault: only a successful compose clears it, a change of the cut while it shows composes again in the same step, undo and redo carry it with the files, and Record reads it from the folder as Edit does (`folder::tests::a_cut_a_short_keeps_from_composing_says_why`).
- Without a script, a sentence a retake adds is a row right after the row said just before it, also between two paragraphs the retake says again, where it was the last row before (`takes::tests::without_beats_a_sentence_added_between_two_said_again_is_a_row_after_theirs`).
- Ship stamps an export with the cut and the lock its run rendered and compares that with the video's now, so an export is stale when a rule change rebuilt `cut.lock` while it rendered, although the export file is the newer one (`ship::tests::an_export_rendered_from_an_older_lock_is_stale_though_the_file_is_newer`). The stamp lives in the Ship screen: after a relaunch the file times decide again.
- The contract says what undo of a recording does once analysis has composed: the first undo reverts that compose, the second takes the take out.

## Not verified

- Recording through the app with the prompter: the takes came in through `shot ingest`. The app's append and its second compose after analysis are covered by `folder::tests::a_retake_of_a_beat_plays_in_place_of_the_earlier_attempt` and by reading, not by a recording.
- The sentence splice on real speech: `takes::tests::a_retake_of_one_sentence_replaces_that_sentence_only` covers it, no fixture clip retakes half a beat.
- The text cutting fixes that are wiring only: the caret scrolling into view, the "R restores this cut" note, the two step click on a cut chip and the Timeline's seam highlight. The control socket cannot click.
- A real voice. `say` speaks more evenly than a person.
- Release build and bench: not run.
- Of the fixes after the review, by reading only: K and C leaving an item after a seek, the keys doing nothing outside Text, A's steps on a real splice (its order has a unit test), the error line for a cut that was not composed, composing after Transcribe in Edit, and Ship planning again. The control socket cannot seek or click, and no fixture retakes half a beat.
- Of the fixes after the second review, by reading and unit tests only: A going back to the proposal after a word cut in the running app, the error line going away once the cut composes, the overlay editor changing an overlay's copies, and a ship run that has `cut.lock` written under it. The control socket cannot click or seek.
- Of the fixes after the third review, by unit tests and reading only, since the control socket cannot click, seek or record: the inspector and K on a click near a restart in the running app, Record's live "not composed" line, redo past a real recording, Remove on Record with the real Trash, and Ship's stale notice after a rule change during a real export.
- `metrics::tests::the_watchdog_names_where_a_stalled_thread_waits_or_runs` failed in one of five runs, also alone. It is a timing test in a file this work did not touch.

## Left open

- A pacing switch and rule switches in Text. The Cut screen had none either: pacing and rules are the format's style rules. Text lists what the rules cut by kind and says where they are set.
- `README.md` still describes the review queue and shows its screenshot. It has uncommitted changes of Mike's on `main`, so it was not touched.
