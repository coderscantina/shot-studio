# 0033: Project files carry a version, written only once it is above 1

Status: Proposed in milestone 14, stream A.

## Problem

A newer Shot Studio may add nodes to `cut.kdl`, `format.kdl`, `video.kdl` or `brand.kdl`. An older build on another Mac, or the CLI from an older checkout, opens such a file. Strict parsing already refuses unknown nodes, but only by naming the first unknown node, which reads like a broken file. A change that keeps every node but changes what one means would not be refused at all, and the older build would write the file back with its own reading.

## Constraints

- Every existing file must open and write back byte-identical: the files are committed, and a version line appearing on the first save would show up in every video's next diff.
- `shot-core` stays pure: the check is part of parsing.
- One rule for the four files, so the app, `shot` and the MCP server refuse the same files.

## Choice

- An optional top-level `version <n>` node, `n` a positive integer. Missing means 1.
- `shot_core`'s `FILE_VERSION` is the version this build reads and writes, 1 today.
- The parser looks for `version` before reading any other node. Above `FILE_VERSION`, the file is refused with `this file is version <n>, and this build reads up to version <FILE_VERSION>`, located at the node. `version 0` and a duplicate `version` are errors like any bad value.
- The writer writes `version <FILE_VERSION>` as the first line only when `FILE_VERSION` is above 1. Today nothing changes on disk; a file that says `version 1` comes back canonical without it.
- `FILE_VERSION` goes up only for a change an older build would get wrong. Adding an optional node does not need it, strict parsing already refuses that file.

## Rejected alternatives

- **Always write `version 1`**: every committed file would change on its next save for no reader's benefit.
- **A version per file kind**: four numbers to keep in step for one rule. One number covers the set a video is.
- **Keep unknown nodes and write them back**: against the strict contract, and an older build still could not tell what a known node now means.
- **Refuse only on write**: the app would show a half-understood edit first.

## Consequences

An older build opening a newer video says to update instead of reporting an unknown node. The first change that bumps `FILE_VERSION` rewrites each file it saves with `version 2` on top, once. The theme files in `brand/themes/` and the other files (`cut.lock`, `thumbnail.kdl`, credits) do not carry a version yet.

## Affected contracts

[Project files](../contracts/project-files.md#common-rules-for-the-kdl-files), `shot-core`.
