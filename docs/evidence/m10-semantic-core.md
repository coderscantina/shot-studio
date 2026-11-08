# Milestone 10 evidence: semantic core

Running report. Each task of the milestone adds its section; the foundation (items 1, 3 and 4) is below. Measured on the reference machine while two other agents built and measured in the same target dir, so timings may read a little high.

## Foundation: grammar, style engine, brand (items 1, 3, 4)

Branch `task/m10-core`, `crates/shot-core` only, plus the contract merge and fixtures.

### What was settled

- The [project files contract](../contracts/project-files.md) now owns elements, style rules, the cascade, `brand.kdl`, format layers, overrides, `cut.lock` and the migration mapping. The draft [semantic video](../contracts/semantic-video.md) is a pointer to it. Decisions [0024](../decisions/0024-content-stays-in-cut-kdl-style-moves-to-the-lock.md) (keep ranges stay, rule cuts move to the lock, overrides are top-level) and [0025](../decisions/0025-lock-carries-the-resolved-style.md) (the lock carries the resolved brand and asset hashes) record the two cross-module choices the draft left open.
- `fixtures/examples/channel/` is a channel repo in miniature: `brand/brand.kdl`, `formats/opinion.kdl`, and `videos/fridays/` with `format.kdl`, `cut.kdl`, `elements/t01.jsonl`, `elements/t02.jsonl` and `cut.lock`. Every file is shown in the contract, parses, writes back byte-identical, and the lock is what `shot_core::build` produces from the others.

### Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass. `crates/shot-core/tests/semantic_video.rs` holds one test per stated behavior: round trips and contract containment, lock equals build, determinism (two builds, element files in another order), cascade replace and `off`, specificity then layer precedence, review queue and K/C overrides, override binding within the tolerance and orphan detection, format layer resolution, grammar errors with line numbers, and the evaluation timing.

### Style evaluation time

`evaluating_seventeen_minutes_of_elements_is_fast` generates 17 minutes of a dense take: a word every 400 ms, a face box every 500 ms, OCR every 2 s, a click every 3 s, a silence every 5 s, sentences, fillers and URLs. That is 5,882 elements against the example channel's 11 rules.

| Step | Time |
| --- | --- |
| Build the lock (cascade, match, apply, sort) | 3.7 ms |
| Build and write the lock (633 instances, 68 KB) | 5.3 ms |

Against the 5 s rough-cut budget, style evaluation is noise. The test asserts under 500 ms so a regression shows without flaking on a loaded machine.

### Not verified here

- Rendering from the lock, the chunk hash and the golden-frame comparison before and after migration (items 5 and 6).
- Real element files from `shot-analyze` (item 2); the fixture elements are hand-written in the contract's shape.
- Import fetching and asset hashing are the build's IO, outside the pure engine; the tests pass an imported brand and hashes in.

## Migration and the renderer on the lock (items 5, 6)

Branch `task/m10-migrate`, details in [m10/migrate.md](m10/migrate.md).

| Acceptance criterion | Result |
| --- | --- |
| The 17 min fixture migrates and its export matches with every chunk reused | rough cut with `shot autocut`, exported (593 chunks), migrated (27 nodes reproduced by rules, 1 override, 9 manual cuts), exported again: 0 of 593 chunks rendered, MP4 and caption sidecar byte-identical |
| Two builds of the same files write byte-identical locks with no analysis | `shot style build` twice and after deleting `cut.lock`: `cmp` identical, 10 ms each |
| Changing the pacing re-cuts the video; a channel rule reaches unshipped videos only | breath 250 ms in the video's layer moves 10 silence cuts, 14 of 597 chunks render; a channel `on chapter-start` rule changed both unshipped demo locks and left the shipped one byte-identical until `shot restyle` |
| An override survives a re-cut, `shot check` reports one that lost its element | a `keep` on a queued filler stayed bound through a manual cut elsewhere; with the filler gone from the elements, `shot check` reports it under `overrides` |
| Rough cut within 5 s of stop with style evaluation | lock build of the 17 min take 10 ms; with the quick elements pass and the transcriber flush under 0.4 s. Measured on the CLI path: the app's stop path still writes nodes |

Breath is kept only on sides that border kept material, which is what the old pacing did at the take's start and end, and chapter edges sit after the pacing's cuts. Both are in the contract's migration section.
