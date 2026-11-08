# 0028: Attempts are derived from the files, ratings and picks are written

Status: Proposed in milestone 13 (stream A, takes view and compose). Waits on Mike.

## Problem

A video recorded in several takes has several attempts at a beat, and until now every recording appended a chapter that kept its whole take, so the cut played the same beat twice and nothing chose between takes. The Takes view needs to know each take's attempt at each beat, remember what a person thought of them, and turn their choices into the main cut. Where each of these lives decides what the CLI and MCP can reuse and what goes stale.

## Choice

- **Attempts are computed, not stored.** `shot_core::Lineup` reads the script, each take's transcript and its `beat` elements and returns a row per beat with every take's attempt. With beat elements an attempt runs from the beat the prompter showed to the next one, trimmed to the words in it. Without them the take's sentences are aligned in order to the beats by shared words (a small dynamic program, monotonic, so a stray common word never jumps ahead). Without beats in the script, the rows are the first take's sentence groups. A new transcript or a re-analysis therefore moves the attempts with it, and nothing goes stale.
- **Ratings and picks are written** in a `takes { attempt ... }` block of `cut.kdl`, the only edit state the view adds. An `attempt` names its take and range and binds to the computed attempt that holds the middle of its range, like an override binds to its element. A pick may name a marked part of an attempt.
- **Composing is two pure `Cut` functions.** `use_attempt` puts an attempt or part in place of every attempt at that beat, where the first of them was, carrying along the removes, reviews, layouts, zooms and overlays the take's blocks have inside it; pauses left between moved beats go with them. `assemble` writes the chapters again from the script: the circled attempt, else the best rated, else the one the cut keeps most of, else the newest, with consecutive beats of one take joined into one block. Using what already holds a beat gives back the same file.
- **Takes that leave the cut** lose their overrides (the grammar requires the take in the cut) and keep their ratings. An edit that would remove a take a Short or X clip uses is refused with the name of the derived cut, instead of dropping its clips.

## Rejected alternatives

- **Storing attempts in `cut.kdl` or the elements**: a second copy of facts the transcript already has, stale after every re-transcription or script edit.
- **Rating by take and beat index**: beats move when the script is edited; take and media range survive that.
- **Silently dropping a derived cut's clips** when its take leaves: loses a person's Short without a word.

## Consequences

The CLI and MCP can offer `shot takes`, `use` and `assemble` on the same core functions later. After a new recording the app shows the Takes view when the new take repeats beats an earlier take has. Alignment quality depends on how close the spoken words are to the beat bullets; recording with the prompter gives exact attempts.

## Affected contracts

[Project files](../contracts/project-files.md#takes), `shot-core`, `shot-app`.
