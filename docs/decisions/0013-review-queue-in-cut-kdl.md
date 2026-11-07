# 0013: The review queue lives in cut.kdl

Status: Proposed, superseded by [0019](0019-semantic-video-elements-and-style.md) once that is adopted

## Problem

Auto cut applies confident findings and queues unsure ones for Keep (K) or Cut (C). Until milestone 04 the queue existed only in memory, which breaks the rule that there is no hidden edit state. A decision also has to stick: re-running the rules after a threshold change must not ask the same question twice.

## Constraints

- Every UI action that changes the edit is a change to a project file.
- `cut.kdl` diffs stay readable, and a decision is reversible by editing the file.
- `RangeEdit` keeps its two variants `Keep` and `Remove`, which `shot-app` matches exhaustively.

## Choice

Two nodes in the take block, next to `keep` and `remove` and lying inside kept material: `review from= to= by=<rule>` for a finding waiting in the queue, and `ignore from= to= by=<rule>` for one a person kept. Cut turns a `review` into `remove ... by=<rule>`, Keep into `ignore`. Restoring a rule's removal also writes `ignore`. A manual cut that touches either drops it. A rule that runs again skips findings overlapping a `remove`, `review` or `ignore` of its kind.

Neither node changes pixels, so neither is part of a chunk hash.

## Rejected alternatives

- A separate `review.kdl`: a Keep would live in one file and a Cut in another, and a proposal diff would need both.
- A `kept` flag or provenance on `keep`: breaks every `RangeEdit::Keep(range)` match in the app, and a kept-despite-rule range would stop adjacent keeps from merging.
- Keeping the queue in the cache: deleting `.shot/cache` would lose decisions.

## Consequences

`shot-core` adds `Review`, `Decision`, `Cut::decide` and `Cut::review_queue`. `shot-analyze` writes `review` nodes and reads `ignore` nodes when a rule runs. The Cut screen reads the queue from the cut and writes decisions through `decide`.

## Affected contracts

[Project files](../contracts/project-files.md#cutkdl), `shot-analyze`, the Cut screen in `shot-app`.
