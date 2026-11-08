# 0029: Several themes per channel, the active one in brand.kdl

Status: Proposed on 2026-11-08 by stream F of milestone 13 (channel theme editor and overlay editor).

## Problem

Mike wants to keep more than one look for a channel (a dark and a light one, a look for a series) and switch between them in the theme editor. The build reads one brand, `brand/brand.kdl`, merges it with the imports and the format layers and writes the result into each lock ([0025](0025-lock-carries-the-resolved-style.md)). The theme editor also needs a place for overlay presets: a template with filled parameters and timing that the Edit screen places as a normal `overlay` node.

## Choice

- `brand/brand.kdl` holds the active theme, as today, and names it with a new `theme <id>` node. The build keeps reading this one file; nothing in `shot-mcp`, the cascade or the lock changes.
- Every other theme is a file `brand/themes/<id>.kdl` with the grammar of a format layer's `brand` block: the look, without imports, rules, `theme` or `overlay-preset`. `Brand::parse_theme` reads it.
- Switching to a theme writes the active look to `brand/themes/<active>.kdl`, puts the chosen file's look into `brand.kdl` with its name, and removes the chosen file. Each theme lives in exactly one file, so there is no copy to drift.
- Imports, rules, overlay presets and channel layouts belong to the channel, not to a theme, and stay in `brand.kdl` through a switch: a video's `cut.kdl` names a channel layout, so it must not vanish with a look.
- A brand without a `theme` node is one theme called `default`; the first management action writes the name.
- Overlay presets are `overlay-preset <id> template=<template> length=<time> { <param> <value> }` in `brand.kdl`. Like the theme name they are not part of the resolved brand: `Brand::merge` leaves them out, so a lock never carries them and a preset change re-renders nothing.
- The theme editor writes the files through `shot-core`'s canonical writer and keeps its own undo history of the brand and the theme files.

## Rejected alternatives

- **All themes in `brand/themes/`, `brand.kdl` a copy of the active one**: two copies of the active theme that a hand edit can split.
- **Themes as blocks inside `brand.kdl`, the build picking the active one**: one long file, and the cascade would gain a selection step in `shot-core` and `shot-mcp`.
- **A video pinning a theme in its `format.kdl` by name**: the build would read a second brand file for that video. A video that needs a different look already has its `brand` block; pinning by name can come later if Mike asks for it.
- **Overlay presets in a file of their own**: one more file for a short list that belongs with the brand's templates.

## Consequences

A theme switch is one commit that shows the look moving between two files. A shipped video keeps its look, since its lock is frozen. Switching themes rebuilds the lock of every video that is built afterwards, like any brand edit. The Library (stream E) can list overlay presets from the brand and open the overlay editor on one.

## Affected contracts

[Project files](../contracts/project-files.md#themes), `shot-core`, `shot-app`.
