# Semantic video contract

Merged into [project files](project-files.md) by milestone 10: the elements, style rules, cascade, `brand.kdl`, `cut.lock`, overrides and the migration mapping are settled there, with tested fixtures in `fixtures/examples/channel/`. This file stays so links from the decisions and the briefing keep working. The draft's open points were settled as follows:

- The selector and effect grammar and the cascade merge: [style rules](project-files.md#style-rules).
- `keep` ranges stay in `cut.kdl` as content; rule-made removals move to the lock ([0024](../decisions/0024-content-stays-in-cut-kdl-style-moves-to-the-lock.md)).
- The lock carries the resolved brand and asset hashes ([0025](../decisions/0025-lock-carries-the-resolved-style.md)).
- Overrides are top-level `override <take> <kind> at=` nodes: an element belongs to a take, not a chapter.
- Style evaluation of a 17 minute take takes milliseconds, see [the milestone 10 evidence](../evidence/m10-semantic-core.md).
- The feel controls' mapping to spring values is milestone 11's.
