# 0007: Generated fixtures only

Status: Adopted by Mike on 2026-11-07

## Problem

Tests, demos and performance targets need media, which is too large for git and personal when it is real.

## Constraints

- Any agent must be able to recreate the data.
- The targets need a realistic 17 min 4K60 workload.

## Choice

All test and demo media comes from a deterministic generator added in milestone 01: speech clips from macOS `say` with known text, synthetic screen and camera clips with cursor events, the 17 min 4K60 target project, and a demo channel repo. Output goes to an ignored directory.

## Rejected alternatives

- A pinned set of real recordings outside git: not reproducible for other contributors.
- Git LFS media: heavy, and still personal.

## Consequences

Transcription and auto-cut quality are measured against known `say` text. Synthetic speech is cleaner than a real voice, so reports state that real-voice quality is not covered by fixtures.

## Affected contracts

The fixture generator in `scripts/`, every milestone's evidence.
