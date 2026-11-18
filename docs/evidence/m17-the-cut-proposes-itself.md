# Milestone 17: the cut proposes itself

Source: `m17/integration` at the merge of `m17/text-faults` (`e5b7622`), built 2026-11-18 on an Apple M1 Pro, macOS 27.0.1, debug profile. Decision: [0040](../decisions/0040-the-cut-proposes-itself.md).

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass. `bash -n scripts/fixtures.sh` passes, `shellcheck` and the `takes` group were run by the detector stream. The ignored fixture test `one_take_finds_its_restarts_and_cue_and_asks_about_emphasis` passes with `SHOT_FIXTURES` set.

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
        keep from=27_240_000 to=34_360_000
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

## Not verified

- Recording through the app with the prompter: the takes came in through `shot ingest`. The app's append and its second compose after analysis are covered by `folder::tests::a_retake_of_a_beat_plays_in_place_of_the_earlier_attempt` and by reading, not by a recording.
- The sentence splice on real speech: `takes::tests::a_retake_of_one_sentence_replaces_that_sentence_only` covers it, no fixture clip retakes half a beat.
- The text cutting fixes that are wiring only: the caret scrolling into view, the "R restores this cut" note, the two step click on a cut chip and the Timeline's seam highlight. The control socket cannot click.
- A real voice. `say` speaks more evenly than a person.
- Release build and bench: not run.

## Left open

- A pacing switch and rule switches in Text. The Cut screen had none either: pacing and rules are the format's style rules. Text lists what the rules cut by kind and says where they are set.
- The script fixtures have no `#[ignore]`d test for their picks; the table above is from a hand run.
- `README.md` still describes the review queue and shows its screenshot. It has uncommitted changes of Mike's on `main`, so it was not touched.
