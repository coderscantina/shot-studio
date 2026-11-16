# 0038: A script editor in the app, ported from Druck

Status: Proposed. Mike asked for the editor on 2026-11-16 ("a proper script writing simple markdown editor based on the work of the druck project"); the choices below are the agent's and wait for him.

## Problem

The Script screen shows `script.md` read-only. Edit script opens the file in the default editor, and the screen picks up what was saved when the window comes back. Writing a script means leaving the app, and the outline, checks and spoken length only update after a round trip.

## Constraints

- `script.md` stays plain Markdown with the grammar of the [contract](../contracts/project-files.md#scriptmd), so the prompter, `shot-core`, Claude and any other editor read it unchanged.
- Edits stay uncommitted in the video's worktree until someone commits them ([0037](0037-a-worktree-per-video-branch.md)). Claude still changes the script only by proposing ([0004](0004-proposals-as-branches.md)).
- Same gpui snapshot as Druck (`gpui-pre =0.3.11`), so its code compiles here.

## Choice

- A new crate `shot-editor` holds Druck's gpui-free text core (`source/`: the `String` buffer with grouped undo, incremental blocks, inline spans, grapheme and word motion, Enter's list continuation) and a fresh editor view and element modelled on Druck's `Galley`. Druck's directives, citations, footnotes, tables, figures, floats, margin notes, review, slash menu and multi-chapter session are left out.
- The view styles the source live: `#` title and `##` chapters as headings, bullets as beats, `[label](target)` showing the label with the target hidden unless the caret is inside it, paragraphs under a beat as its script. Undo and redo, IME, soft wrap, mouse and keyboard selection, list continuation, Cmd+K for a link, and macOS spell checking through `NSSpellChecker` with suggestions.
- The Script screen's document pane is the editor. It saves on a pause in typing (atomic write into the worktree), re-parses on every change so the outline, checks and spoken length follow as you type, and marks the line `Script::parse` refuses. A change on disk from Claude, `shot` or a merged proposal replaces the text as one undo step when the buffer has no unsaved edit.
- Open proposals show as cards above the editor with struck and inserted words, as today, and the selected one marks the lines it changes. Accept first saves the buffer, so 0037's "commit a person's edits first" covers what was just typed.
- Edit script and its default-editor round trip go.

## Rejected alternatives

- **gpui-component's code editor input**: a code editor, not prose. No hidden link syntax, no heading sizes, no spell checking.
- **Keep the external editor**: the round trip this record removes.
- **Porting Druck's whole editor**: about 28k lines tied to Druck's book model; most of it has no use in a script.

## Consequences

Shot Studio carries a copy of Druck's text core. Fixes made in one project do not reach the other on their own; the crate's module docs name the Druck revision it came from. Both are MIT and by the same author, so no notice is added.

## Affected contracts

[Project files](../contracts/project-files.md#scriptmd) (the app writes `script.md` as typed), `shot-app`, workspace `Cargo.toml`.
