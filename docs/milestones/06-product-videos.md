# Milestone 06: Product videos

## Outcome

A product tour looks finished without manual keyframing: zooms follow clicks, typing and the cursor, the window sits padded on a background with corners and shadow, keystrokes show as an overlay, and code from a git commit range types itself out with the discussed line highlighted.

## Dependencies and ownership

Requires 03. Media owns the effects in `shot-render`. App owns the Product demo screen. Core adds the effect nodes to the contract.

## Work

1. Auto zoom from the cursor and key event log, with easing and a minimum hold time.
2. Click ripples, cursor size and cursor smoothing, drawn from events instead of burned-in pixels.
3. Window styling: padding, corners, shadow, background, device frames.
4. Keystroke overlay.
5. Code from git: point at a commit range, render the diff typing out in the timeline, highlight the line at a transcript phrase.
6. The Product demo screen with click and zoom lanes.

## Acceptance criteria

- On the 30 s screen fixture, zooms land on every click and never cut off the cursor.
- Cursor effects change after recording without re-recording.
- Code from git renders the real diff of a fixture repo commit range.
- Preview and export show the same frames within the golden-frame tolerance.

## Evidence and handoff

Report in `docs/evidence/m06-product-videos.md` with golden frames and screenshots in `docs/evidence/m06/`. Consumed by 09.
