# 0024: `keep` ranges stay in `cut.kdl`, rule-made removals move to the lock

Status: Proposed. Settles an open point of [0019](0019-semantic-video-elements-and-style.md) in milestone 10.

## Problem

Once pacing is a style rule, a take block in `cut.kdl` can no longer tile its material with `keep` and `remove by=<rule>` nodes: the rule's cuts come from the lock. The draft left open whether `keep` ranges stay, or whether a take block lists only manual cuts and the material is implied.

## Choice

- A take block states the material a chapter uses of a take as `keep` ranges, and a person's cuts as `remove ... by=manual`. Both are content: which part of which recording the chapter shows, and what a person took out.
- Rule-made removals (`remove by=silence` and the rest), the review queue (`review`, `ignore`) and rule-made zooms and overlays leave `cut.kdl`. Their results are `cut`, `review`, `zoom` and `overlay` instances in `cut.lock`, each with the element and rule that made it.
- The timeline evaluates a migrated video from both files: the kept ranges less the union of the lock's `cut` instances, with the lock's zooms and overlays next to a person's.
- Overrides are top-level `override <take> <kind> at=<time>` nodes, not children of a take block: an element belongs to a take, a take may appear in several chapters, and the override's identity is the element's.
- An element in material the main cut does not keep gets no instances, so a manual cut silences every rule inside it.

## Rejected alternatives

- **Only manual cuts in the take block, material implied by the take's length**: the chapter boundaries and the choice of material are content a person makes, and reading them needs the media.
- **Rule cuts written back into `cut.kdl` as `remove` nodes**: that is 0014, which 0019 replaced.
- **Overrides inside take blocks**: natural to read, but the same element would be addressable from two chapters, and every crate that builds take blocks would change for a field that is not about the block.

## Consequences

`shot migrate` merges kept ranges that a rule's removal split and turns `ignore` into `{ keep }` overrides, see the [migration mapping](../contracts/project-files.md#migration-of-todays-nodes). Timeline evaluation gains a lock input in milestone 10 item 6.

## Affected contracts

[Project files](../contracts/project-files.md), `shot-core`, `shot-analyze`, `shot-render`, `shot-app`.
