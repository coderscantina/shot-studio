# 0025: The lock carries the resolved brand and the hashes of the files it draws from

Status: Adopted by Mike on 2026-11-08. Settles the shape of `cut.lock` from [0019](0019-semantic-video-elements-and-style.md) and [0020](0020-channel-brand-and-shared-styles.md) in milestone 10.

## Problem

The draft lock listed instances and the import's commit. A shipped video must "freeze its lock and resolved style at the release tag", the renderer needs fonts, colors and camera styling, and the chunk hash needs font and Lottie files by content. Reading those from `brand.kdl` at render time would let a brand edit change a shipped video, and the renderer would resolve the cascade a second time.

## Choice

- The lock holds the brand after the cascade (imports, `brand.kdl`, the format's and the video's `brand` blocks merged), with imports and rules removed. The renderer reads `cut.kdl` for content and `cut.lock` for everything styled, and never resolves the cascade itself.
- The lock lists every file the resolved brand names (font files, logo, stinger, Lottie templates) with its BLAKE3 hash. The build hashes the files; the engine stays pure and refuses a brand that names a file it was not given a hash for. Bumpers are media and not hashed; a missing one stays an export error.
- The lock starts with `lock <version>`. A build refuses another version instead of guessing.
- Provenance on every instance is the layer and the rule's canonical selector, or the override's `at`. It is not hashed, like zoom provenance today, so a renamed layer never re-renders a chunk.
- The rules themselves are not in the lock: the instances are their result, and the layers are the source.

## Rejected alternatives

- **Instances only, brand read at render time**: a rebrand would change shipped videos, against 0019.
- **Rules copied into the lock**: duplicates the layers and makes the lock a second place to read style from.
- **File sizes and modification times for brand assets**: fine for a take's media, which is one machine's, wrong for files in git that every machine has.

## Consequences

`shot restyle` is a rebuild of the lock on purpose. The chunk hash (milestone 10 item 6) reads brand fields and asset hashes from the lock where something branded draws. A shipped video's lock is self-contained at its tag.

## Affected contracts

[Project files](../contracts/project-files.md#cutlock), `shot-core`, `shot-render`, `shot-ship`.
