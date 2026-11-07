# Milestone 10: Semantic core

## Outcome

A recording becomes a committed document of elements, cascading style rules turn it into a generated `cut.lock`, and every existing automatic edit (pacing, auto zoom, link cards, highlights, b-roll slots) is a style rule instead of written nodes. The fixture project renders the same frames before and after the migration, and the app shows where every instance comes from and asks the scope of every change.

## Dependencies and ownership

Requires 07 and 08 merged. Decisions [0019](../decisions/0019-semantic-video-elements-and-style.md) and [0020](../decisions/0020-channel-brand-and-shared-styles.md), draft contract [semantic video](../contracts/semantic-video.md). Core owns the element model, style language, cascade, lock and the contract merge in `shot-core`. Analysis owns writing elements in `shot-analyze`, Claude annotations go through Agents in `shot-cli` and `shot-mcp`. Media moves the auto-zoom and overlay rules out of `shot-render`. App owns provenance, scope and the review queue in `shot-app`.

## Work

1. Settle the grammar in the draft contract: element kinds, selectors, effects, cascade merge, override syntax, `cut.lock`. Make the examples tested fixtures and merge the draft into [project files](../contracts/project-files.md).
2. Elements: analysis writes `elements/<take>.jsonl` while recording and after stop, `shot analyze` re-runs it explicitly. Script tags `{name}` in `script.md` and labeled record markers become elements. Claude annotations run after stop on the transcript only and never block the rough cut.
3. Style engine in `shot-core`: resolve brand, format and video layers, apply rules and overrides to elements, write `cut.lock`. Pure and deterministic.
4. `brand/brand.kdl` with partial overrides, bundled OFL fonts and repo fonts, imports from git pinned in the lock. `format.kdl` holds only differences from its format.
5. Migrate: auto-cut rules, the review queue, pacing, auto zoom, link cards, highlights and b-roll slots become rules. `shot migrate` turns an existing video's nodes into overrides and rules once.
6. Renderer and chunk hash read the lock. Shipping freezes the lock at the tag, `shot restyle` rebuilds a shipped video. `shot check` lists overrides that lost their element.
7. App: provenance on every instance (the rule and layer that made it), a scope choice on every change of a look (only here, this video, this format, the channel) with the number of instances and videos it reaches, K and C as overrides, and a rule editor. Design them in a [design-variants](../design.md) round first.
8. MCP and CLI: agents read elements and the lock, and propose rule or override changes as proposal branches.

## Acceptance criteria

- The 17 min fixture migrates, and its export matches the pre-migration export within the golden-frame tolerance, with every chunk reused.
- Two builds of the same files write byte-identical locks, with no analysis run.
- Changing the pacing re-cuts the video; changing a channel rule changes every unshipped video's lock and leaves a shipped one alone.
- An override survives a re-cut elsewhere, and `shot check` reports one whose element vanished after re-analysis.
- The rough cut is still ready within 5 s of stop, measured with style evaluation included.

## Evidence and handoff

Report in `docs/evidence/m10-semantic-core.md` with lock diffs, timings, golden frames and screenshots in `docs/evidence/m10/`. Consumed by 11 and 12.
