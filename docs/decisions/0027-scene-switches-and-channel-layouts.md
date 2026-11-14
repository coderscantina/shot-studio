# 0027: Scene switches on layout edges, channel layouts by name in the brand

Status: Adopted by Mike on 2026-11-14. Amended by [0036](0036-one-transition-type.md): a switch is the shared transition type.

## Problem

A layout change inside a take switched hard, and the seven layouts were a closed enum. Mike asked for scene switches "with proper transitions/animations/SFX" and for a layout builder. Both reach `shot-core` (the grammar, the timeline and chunk keys), `shot-render` (geometry and drawing) and the app (the Layout lane, the hotkeys, the builder), and other streams read `Layout` (the Text workspace, the prompter's hotkey marks, thumbnails).

## Choice

- **A switch lives on the edge it moves.** A `layout` range in `cut.kdl` takes optional `in` and `out` children, `<kind> length=<time> sound=<sound>`, for the switch into it at `from` and out of it at `to`; where one range ends and the next starts, the next one's `in` wins. Everything else switches the way the format's new `layout-switch` node says, `morph length=500_000` without one. So most videos write nothing, and a range moved by an edit carries its switches along (`set_layout` keeps a switch on an edge it shares with the change it replaces, a split keeps the outer edges' switches).
- **The window starts on the edge** and runs into the new layout, cut short by the next edge. Each segment then overlaps at most one window, the old layout's frames never change, and a switch keys only the chunks of its window. Chapter-edge transitions stay centered on their edge; they join two pieces, a switch changes one take's picture.
- **Kinds**: `morph` (the default; every slot springs from its place in the old layout to the new one, on a spring that settles in the window), `dissolve`, `zoom-through` and `cut`. A morph is one frame of an interpolated layout ([`LayoutSpec::morph`]), drawn by `compose`; dissolve and zoom-through draw the same moment twice through the painter's two-sided path that chapter transitions use. Preview and export both go through the painter, so they draw the same.
- **Sound**: `sound=brand` plays the brand's `transition` keys by phase over the window, any other name plays that sound at the window's start. No sound by default: the brand's transition sounds were made for chapter edges.
- **`Layout` stays `Copy`.** A channel layout is `Layout::Custom(LayoutName)`, a name stored inline (31 bytes at most). Existing code that copies, matches or hashes a layout keeps working, and every existing `cut.kdl` reads the same; a layout keyword the grammar does not know is now read as a channel layout's name and refused at export if the brand does not define it.
- **Channel layouts are brand nodes**, `layout <name> { screen ...; camera ... }`, slots in pixels of the 3840x2160 design frame with corners and a crop focus, drawn in order. They ride the existing cascade (a format layer's `brand` block adds its own), reach the renderer through the lock's resolved brand, and draw through the same `LayoutSpec` as the built-ins, so zooms, the brand's camera frame and morphs work between any two layouts. A chunk drawing a channel layout adds its slots to its key, and a switch adds the slots of the layout it leaves.
- **The builder is its own window.** It saves into `brand/brand.kdl` as one change with undo (the folder's snapshot holds the brand text), and can offer the layout on the video's 1 to 4 keys through the format's `layouts`. The Edit screen's keys read the format's `layouts` first, then the built-ins, so Edit and Record agree.

## Rejected alternatives

- **A separate `switch at=<time>` list per take**: an edit that moves a layout range would leave switches on edges that no longer exist.
- **A centered window like chapter transitions**: a segment could overlap two windows, and the old layout's frames would change too, re-keying more chunks for the same switch.
- **`Layout` as a non-`Copy` enum with a `String`**: a ripple through every crate that copies layouts, for no gain at the handful of names a channel has.
- **Channel layouts in `formats/<format>.kdl`**: a layout is the channel's look, shared by its formats, like the camera frame.
- **The builder as a sheet over the Edit screen**: the Edit screen's single-key shortcuts (1 to 4, Space, Backspace) would fire behind it.

## Consequences

Videos outside a channel, and channel videos not yet migrated, draw only the built-in layouts: their brand does not reach the picture. Every existing video with layout ranges now morphs on its switches; `layout-switch cut` in a format restores the hard switch. Renaming a channel layout would orphan the ranges that name it, so the builder keeps an existing layout's name.

## Affected contracts

[Project files](../contracts/project-files.md#scene-switches-and-channel-layouts), `shot-core`, `shot-render`, `shot-app`.
