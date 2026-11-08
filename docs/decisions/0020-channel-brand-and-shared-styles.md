# 0020: A channel brand, overridable per video, importable from git

Status: Adopted by Mike on 2026-11-08. Settled with Mike in the effects grill on 2026-11-07.

## Problem

Cards, captions, title bands, the end card, camera styling and sounds need one look per channel: fonts, colors, a logo, sounds, intro and outro. `format.kdl` is copied into each video, so a look stored there needs editing in every video to change.

## Constraints

- A rebrand reaches every unshipped video by editing one file.
- A single video can differ, for example a sponsor accent.
- Fonts and sounds enter the chunk hash, so they must be the same on every machine.
- The app repo is MIT and holds no private assets ([0006](0006-mit-open-source-and-distribution.md)).
- Channel media is git-ignored. Bumper videos are tens of MB at 4K.

## Choice

- `brand/brand.kdl` in the channel repo holds the channel's style: fonts, colors, logo, the sound for each effect kind, the intro and outro, and channel-level style rules ([0019](0019-semantic-video-elements-and-style.md)).
- A video overrides with an optional `brand` node in its `format.kdl` naming only the fields that differ.
- Fonts: the app bundles a few OFL fonts as defaults, and the brand can name a font file in `brand/`, hashed by content.
- Files: logo, fonts and sounds commit with `brand.kdl`. Bumper videos live in `brand/media/`, git-ignored like takes. A missing bumper is an export error naming the file.
- `brand.kdl` can import a style from a git repo at a commit, recorded in the lock. The four presets ship this way.
- Brand fields enter a chunk's hash only where something branded draws.

## Rejected alternatives

- **Brand in each `format.kdl`**: older videos keep the old look, a rebrand edits every file.
- **Full `brand.kdl` per video, or named variants**: more files and more ways to drift, for a need a partial override covers.
- **Installed system fonts**: an export on another Mac can differ silently.
- **Bumpers in git or Git LFS**: the repo grows by every bumper revision, or every machine needs one more tool and `shot-board` must handle LFS pointers.
- **Copy-only styles**: no updates, no shared starting point for other channels.

## Consequences

`shot-core` resolves the cascade. `shot-board` reads `brand/` from the channel repo next to `formats/`. Styles are data, not code, so the out-of-scope line on plugins holds.

## Affected contracts

[Semantic video draft](../contracts/semantic-video.md), `shot-core`, `shot-board`, `shot-render`, `shot-app`.
