# Milestone 08 evidence: cut proposals in the Edit screen, and audio true peak

Recorded 2026-11-07 on `task/m08-proposals`. Machine: MacBook Pro, Apple M1 Pro, 32 GB, macOS 27, rustc 1.99.1. Other agents built and measured in the shared target dir at the same time. Screenshots are next to this file (git-ignored), 1600x1000 in dark.

## Cut proposals inline in the Edit screen

Built to the design rule "Agent proposals in the editor" and `docs/mockup/Edit.dc.html` and `EditTimeline.dc.html`.

- An open proposal whose `cut.kdl` changes recorded words shows in the Text workspace in place of the paragraphs it changes: struck words it removes, tinted words it brings back, the time saved muted ("saves 1.9 s"), Accept (Tab), Reject (Esc) and Another take. A proposal with several changed stretches shows a card per stretch, marked "2 of 3, accepted together".
- What a proposal changes is read word by word from its `cut.kdl` where it branched and at its head (`Board::proposal_file`, new), so it shows on the cut on disk even after other edits.
- Accept merges the branch into `video/<slug>` (shot-board) and writes the cut to `cut.kdl`: the proposal's head when the cut on disk is still the one it branched from, else its word changes applied on top through `Cut::delete_words` and `Cut::restore`, the same edits MCP made.
- Reject deletes the branch. Another take rejects it, then asks the Claude bridge for a different take with the change in words (`removes t01 w72-w78 "..."`); a quiet line in the transcript shows Claude's latest step while it runs.
- Tab and Esc act on the selected proposal; a click or G selects one. Esc with no proposal selected still clears the word selection. G shows the next proposal: Text workspace, its chapter, playhead at its first change, scrolled to it.
- Away from the text, the Timeline's Changes pane ends with the card: where it applies ("Intro, 00:07"), the words around the change cut to eight on each side with the text at 16 px and a 64ch measure, and "Show at 00:07" (G). The change lines scroll above it.
- A lime spark marks each card on the overview bar in Text and Picture; clicking one shows it.
- The branch never shows. Script's card and Edit's share the action buttons (`ui::proposal_action`) and the struck and inserted pieces (`script::proposal::Piece`, `push`).

### Run in the app

On a copy of the demo channel from milestone 08 (`videos/opinion-target` is the target project, transcribed). Proposals made through `shot mcp` (`propose_cut`, driven over stdio):

```
created proposal opinion-target/tighter-plan at ff2179e
  removes t01 w64-w70 "Before we start, here is the plan." (2.3 s)
  restores t01 w101-w106 "cloud service. Everything runs on this" (2.2 s)
created proposal opinion-target/shorter-setup at e23f966
  removes t01 w72-w78 "we set up a project. Then we" (1.9 s)
```

plus `outside-tighten` from the milestone 08 outside session. Keys and clicks were posted to the running app.

| Step | Result | Screenshot |
| --- | --- | --- |
| Timeline workspace | Card with "Intro, 00:07", Show at 00:07 (G) | `edit-timeline.png` |
| G | Text workspace, Intro, playhead 00:07.26, `outside-tighten` selected | `edit-text-show.png` |
| Tab | `e4528c3 🔀 accept outside-tighten into opinion-target`, branch gone, `cut.kdl` gains `remove from=8_070_000 to=8_600_000 by=manual`, the transcript shows a 0.5 s cut | `edit-text-accepted.png` |
| G, Esc | `tighter-plan` branch deleted, `cut.kdl` unchanged | `edit-text-rejected.png` |
| New proposal | `shorter-setup` inline, "saves 1.9 s" | `edit-text-proposal.png` |
| Another take | `shorter-setup` deleted, "Claude is writing another take", then `proposal/opinion-target/shorter-setup-2` (`248f191 Drop the setup step from the intro plan...`) shown inline, "saves 1.7 s" | `edit-text-another-running.png`, `edit-text-another-done.png` |

### Differences from the mockup

- The card replaces a whole transcript paragraph (up to about 30 words), so it is longer than the mockup's two sentences. Two proposals on the same paragraph each repeat it.
- The title is the proposal's commit subject, so its wording is the agent's ("Cut the stray "For example," from the intro").
- Struck and inserted words are tinted word by word, with small gaps, like removed words in the transcript; the mockup tints the phrase as one span.
- Buttons use the Script screen's 14 px text, the mockup's card 13 px.
- The selected card has a lime border; the mockup has no selected state.
- In Timeline the card shares the 420 px Changes pane with eight change lines, so the lines scroll and Show at wraps to its own row.
- Sparks of nearby proposals overlap on the overview bar.

## Audio true peak

The finish was a sample-peak limiter at -1.5 dBFS with instant attack and no lookahead. AAC overshot it, and its hard knee made the overshoot worse.

Now `Finish` (shot-audio) applies the gain, then a lookahead true-peak limiter (`limiter.rs`): peaks are found 4x oversampled (Kaiser-windowed sinc, 32 taps a phase), the gain reaches what a peak needs over a 5 ms linear attack and recovers over 50 ms, at the -1 dBTP ceiling less 1.0 dB of codec headroom. It adds 256 samples of latency, taken out in `voice.rs`. The limiter takes loudness off the peaks it holds, so `voice::finish` runs up to three trial passes that correct the gain until the result is within 0.1 LU of -14 LUFS, then the final pass. On the 17 min export the audio passes took about 20 s, on their own thread while video renders.

Integrated loudness and true peak of the encoded audio, ffmpeg `ebur128=peak=true` and the in-repo meter (`shot_audio::Meter`):

| File | Before, ffmpeg | Before, in-repo | After, ffmpeg | After, in-repo |
| --- | --- | --- | --- | --- |
| 17 min target project export (15:38 cut) | -14.3 LUFS, -0.2 dBTP | -14.31 LUFS, -0.15 dBTP | -14.1 LUFS, -1.8 dBTP | -14.10 LUFS, -1.84 dBTP |
| speech `clean` | -14.0, -1.4 | -14.03, -1.41 | -14.0, -1.9 | -14.00, -1.86 |
| speech `filler` | -14.7, -0.9 | -14.66, -0.90 | -14.2, -1.7 | -14.22, -1.72 |
| speech `long-silence` | -14.5, -1.3 | -14.52, -1.34 | -14.1, -1.9 | -14.08, -1.92 |
| speech `mid-pause` | -14.1, -1.4 | -14.10, -1.39 | -13.9, -1.9 | -13.90, -1.93 |
| speech `retake` | -14.2, -1.3 | -14.19, -1.33 | -14.1, -1.9 | -14.07, -1.90 |
| speech `self-correction` | -14.4, -1.2 | -14.48, -1.23 | -14.0, -1.9 | -14.10, -1.91 |

The speech clips run through `voice::finish` and the export's AAC encoder (128 kb/s) and are read back from ADTS: `cargo test --release -p shot-render --test true_peak -- --ignored --nocapture` asserts at most -1.0 dBTP and -14 LUFS within 0.5 LU. Before the fix `filler` and `long-silence` were off target by more than 0.5 LU and `filler` peaked at -0.9 dBTP. The export was rendered once with the old CLI and once with the new one reusing the cached video chunks.

The 1.0 dB headroom is more than these files needed (the encode added 0.1 to 0.3 dB over the limiter's -2.0 dBTP). It is kept for louder, brighter material than the fixtures.

### A meter finding outside this task

`shot_ship::media::loudness` (used by `shot check`) stops reading at the first second of digital silence after the start. The target export has one at 0:34, so `shot check` measured 34 s of a 15:38 file: -14.55 LUFS and -1.12 dBTP before the fix, against -14.31 and -0.15 for the whole file. The numbers above read the whole file. That is also why `shot check` reported -0.9 dBTP where ffmpeg read -0.4 in milestone 08.

## Not verified

- A real voice through the new limiter: only the generated fixtures.
- Proposals whose head changes the cut's chapter or take structure: such blocks are skipped, so a proposal that only reorders shows no card.
