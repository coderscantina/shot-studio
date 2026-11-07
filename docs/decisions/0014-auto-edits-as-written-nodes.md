# 0014: Auto edits are written nodes with provenance

Status: Proposed

## Problem

The pacing adds jump cuts and 112% punch-ins, auto zoom adds zooms on clicks and typing, and record hotkeys drop markers. Each could be written into `cut.kdl` or derived at evaluation time from `format.kdl` and the event log.

## Constraints

- The edit is honest: what you see in the export is in the file.
- A person can delete or move any single zoom.
- Re-running a rule must not duplicate its own work or touch a person's.
- `cut.kdl` diffs stay small after the rough cut.

## Choice

- Jump cuts are not nodes. The pacing places the edges of each `remove`, and the jump is what is left.
- Every zoom is a `zoom` node. Automatic ones carry `by=`: `jump-cut` and `emphasis` from the pacing, `click`, `typing` and `cursor` from auto zoom, `marker` from the Z hotkey. A person's zoom has no `by`. A rule that runs again replaces the zooms with its own `by` and leaves the rest alone. Provenance is not part of the chunk hash.
- Markers are a `marker` kind in `events.jsonl`, with the cursor position at the press. They are facts about the recording, consumed once when the take is appended to the cut: layouts become `layout` nodes, Z a `zoom ... by=marker`, B a b-roll overlay, A and N feed the retake rule and the chapter boundaries.

## Rejected alternatives

- Deriving zooms at evaluation time: "every other jump cut" flips every zoom whenever one cut changes, the file hides what the export shows, and a single zoom cannot be deleted.
- Markers written straight into `cut.kdl` while recording: the take is not in the cut yet, and a crash mid-take would leave edit state for media that may be deleted.

## Consequences

The rough cut is one larger diff when the take is appended; every later diff is the change itself. `shot-capture` writes markers, `shot-analyze` writes the jump-cut and emphasis zooms, `shot-render` writes the click, typing and cursor zooms (it already reads the event log, and the renderer must not depend on analysis), and the app shows provenance per zoom.

## Affected contracts

[Project files](../contracts/project-files.md#cutkdl), `shot-capture`, `shot-analyze`, `shot-render`, `shot-prompter`.
