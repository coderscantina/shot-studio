# Milestone 04: Auto cut and finish

## Outcome

When recording stops, a rough cut is already applied: silences, retakes, misspoken words, mid-sentence pauses, fillers and idle screen are removed or queued for review. The video sounds finished at -14 LUFS with captions, jump cuts and punch-in zooms.

## Dependencies and ownership

Requires 03. Analysis owns the rules in `shot-analyze`. Media owns `shot-audio` and the zoom and caption passes in `shot-render`. App owns the Cut screen. Core adds the rule provenance nodes to `cut.kdl` through the contract.

## Work

1. VAD with Silero via `ort`, running during recording.
2. Auto-cut rules, each with a switch, threshold from `format.kdl` and a count: silences, inline retakes (transcript similarity), misspoken words and self-corrections, mid-sentence pauses, fillers (learned per voice), idle screen (frame diffs).
3. A confidence per decision. Confident cuts apply to `cut.kdl` with their rule as provenance. Unsure ones go to the review queue with Keep (K) and Cut (C), and an Edited/Untrimmed toggle.
4. Pacing presets Loose, Natural (0.25 s breath), Tight. Jump cuts and 112% punch-ins on emphasis and every other jump cut.
5. Voice chain: DeepFilterNet denoise, EQ, compressor, `ebur128` loudness to -14 LUFS, music bed with sidechain ducking.
6. Auto captions from word timestamps in Word pop, Clean and Off styles, burned in or as a sidecar file. Misheard name fixes go through Claude as text ([0003](../decisions/0003-claude-through-local-cli.md)).

## Acceptance criteria

- On the `say` fixtures, the known retake and self-correction are cut and the clean version is kept.
- Rough cut and transcript are ready within 5 s of stop on the 17 min project with all rules on.
- Exported audio measures -14 LUFS integrated within 0.5 LU.
- Every applied cut in `cut.kdl` names its rule, and every queue decision writes to `cut.kdl`.

## Evidence and handoff

Report in `docs/evidence/m04-auto-cut-and-finish.md` with rule counts per fixture, loudness measurements, timings and Cut screen screenshots in `docs/evidence/m04/`. Consumed by 07 and 08.
