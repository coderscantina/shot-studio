# 0029: Several themes per channel, each in its own file

Status: Adopted by Mike on 2026-11-14, with the theme as a pointer instead of the swap proposed on 2026-11-08 by stream F of milestone 13.

## Problem

Mike wants to keep more than one look for a channel (a dark and a light one, a look for a series) and switch between them in the theme editor. The build reads one brand, `brand/brand.kdl`, merges it with the imports and the format layers and writes the result into each lock ([0025](0025-lock-carries-the-resolved-style.md)). The theme editor also needs a place for overlay presets: a template with filled parameters and timing that the Edit screen places as a normal `overlay` node.

## Choice

- Every look is a file `brand/themes/<id>.kdl` with the grammar of a format layer's `brand` block: fonts, colors, logo, camera, motion and the other look fields, without imports, rules, `theme`, `overlay-preset` or channel `layout`. `Brand::parse_theme` reads it.
- `brand/brand.kdl` holds what belongs to the channel: imports, rules, overlay presets, channel layouts, sounds and bookends, and `theme <id>` naming the active look. It holds no look fields of its own.
- Switching the active theme changes the one `theme` line. No look moves between files, so a switch is a one-line diff and an edit to a look touches one file.
- Any format layer may name a theme: the channel's `formats/<format>.kdl` or a video's `format.kdl`. The nearest layer that names one wins and replaces the active look, then that layer's `brand` block lays over it. A tutorial format can default to the light theme, and one video can use it without changing the channel.
- A `brand.kdl` written before this has its look inline and no `theme`. Migration ([0035](0035-every-video-on-style-rules.md)) moves that look to `brand/themes/default.kdl` and writes `theme default`.
- Overlay presets are `overlay-preset <id> template=<template> length=<time> { <param> <value> }` in `brand.kdl`. Like the theme name they are not part of the resolved brand: `Brand::merge` leaves them out, so a lock never carries them and a preset change re-renders no placed overlay. A style rule may name one (`overlay preset=<id>`); the build resolves it into the rule's overlay effect, so the lock carries the instance and a shipped lock stays frozen.
- Themes, presets and channel layouts are managed by `shot-core` functions that the app, `shot theme` and the MCP server call. Writes to `brand/` take a channel-level lock through `shot-fs`.

## Rejected alternatives

- **The active look inside `brand.kdl`, swapped with a theme file on a switch** (the 2026-11-08 proposal): a switch moves a whole look between two files, the editor's undo spans both, and the first theme action has to write a name into a brand without one.
- **All themes in `brand/themes/`, `brand.kdl` a copy of the active one**: two copies of the active theme that a hand edit can split.
- **Themes as blocks inside `brand.kdl`, the build picking the active one**: one long file for every look.
- **A theme only in the video's `format.kdl`**: a format, such as tutorial, could not default to its own look.
- **Overlay presets in a file of their own**: one more file for a short list that belongs with the brand's templates.

## Consequences

The build reads two brand files instead of one: `brand.kdl` and the theme the cascade picks. A shipped video keeps its look, since its lock is frozen. Switching themes rebuilds the lock of every video that is built afterwards, like any brand edit. The Library can list overlay presets from the brand and open the overlay editor on one.

## Affected contracts

[Project files](../contracts/project-files.md#themes), `shot-core`, `shot-app`, `shot-cli`, `shot-mcp`.
