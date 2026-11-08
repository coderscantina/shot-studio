# 0030: Caption presets, a look in the brand, and line edits as fixes

Status: Proposed, milestone 13 stream D, 2026-11-08.

## Problem

Mike asked to style burned-in captions (font, size, weight, colors, highlight, position, words per line, case) and to edit their text in place. Until now `captions word-pop|clean|off` picked a hard-coded look, and the only text edit was a hand-written `fix` in `cut.kdl`.

## Constraints

- The existing keyword keeps working everywhere it is written: `format.kdl`, `cut.kdl`, a derived cut's `captions=`, the style rules and the lock.
- A channel sets its look once; a video changes single fields of it.
- Every UI action is a file change with undo, and preview equals export.
- The transcript stays cache; caption text lives in git.

## Choice

- **The keyword is the preset.** `word-pop`, `clean`, `karaoke` and `bold` (plus `off`) each set every field and how the word being said stands out (a pill, a color fill up to it, its own color, or nothing).
- **The look is a set of properties.** The same ten properties sit on the brand's `captions` node and on a format's `captions <style>` node; a later layer changes fields of the earlier one. The video's look lives on its `format.kdl` node, so a loose video and a migrated one store it the same way, and it undoes with the format. Use for every video moves it into `brand/brand.kdl`.
- **One design draws both.** `shot_render::captions::Design` resolves the preset, the brand's look and the video's look with the brand's fonts and accent. Export and the Edit preview call the same `shape`, and the design is part of the derived chunk key.
- **A line edit is a fix with context.** Editing a caption line writes one `fix` for the changed words plus a word on each side, so it rarely matches elsewhere. The longest matching fix wins, and a multi-word `said` spreads the matched timing by length, so word pop still lands on each word.
- **Line breaks are nodes.** `split <take> at=<time>` and `join <take> at=<time>` in `caption-fixes` override the automatic grouping at one word.

## Rejected alternatives

- A `caption-look` node in `cut.kdl`: the brief puts styling in the brand and the format layer, and `cut.kdl` is content.
- New keywords for every combination (`word-pop-top`, ...): grows without end and cannot express a size.
- Per-occurrence text edits addressed by time: a re-transcription shifts timings and loses them, as 0015 found for fixes.

## Consequences

- A fix with context still applies wherever the same three words are said again in the video. That is the dictionary rule of 0015; Mike may want per-occurrence edits later.
- The main video still ships captions as a sidecar. The preview shows burned captions only while the Captions panel is open; burning them into the main video is a follow-up.
- `Shape::Caption` became a struct with font, back and emphasis; the record monitor draws the style's preset.

## Affected contracts

[Project files](../contracts/project-files.md#caption-look) (`captions` in the brand and the format, `split` and `join`), `shot-render` captions and derived chunk keys, the Edit screen.
