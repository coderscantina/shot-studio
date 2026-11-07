# M03 evidence: transcript and layout edits in the project model

Every edit is a pure function on `Cut` in `crates/shot-core/src/edit.rs` that returns the new `Cut`. The app, CLI and MCP call the same functions.

## API

- `Cut::delete_words(at, &Transcript, selection) -> Result<Cut, EditError>`: removes the media of the words overlapping `selection` as `remove ... by=manual`. The cut runs from the first word's start to the last word's end. Existing removals keep their `by`, `keep` ranges are split, adjacent removals of the same `by` merge.
- `Cut::restore(at, range)`: turns removed parts of `range` (manual or rule-made) back into kept media and merges keeps.
- `Cut::set_layout(at, range, layout)` and `Cut::clear_layout(at, range)`: replace and split overlapping layout changes, merge equal neighbours.
- `Cut::transcript_view(at, &Transcript) -> Vec<WordView>`: the words inside the take block with `WordState::Kept` or `Removed(RemovedBy)`. A word is judged by its midpoint.
- `TakeAt { chapter, take }` addresses a take block, because a take can appear more than once in a chapter.

## Checks

- Unit tests in `edit.rs`: middle and edge deletes, word snapping, merging, restore, layout set and clear, transcript view, errors.
- `fixture_projects.rs`: delete, restore and layout set on the committed target project change only the lines of the touched take block in `to_kdl`.
- `project_files.rs`: deleting words in one chapter changes only that chapter's segment hashes.

## Not covered

Selecting words across take blocks or chapters is one call per take block. Deleting a range spanning several chapters is left to the caller.
