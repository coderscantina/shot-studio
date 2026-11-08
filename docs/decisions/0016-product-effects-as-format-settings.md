# 0016: Product video effects are format settings

Status: Adopted by Mike on 2026-11-08. [0019](0019-semantic-video-elements-and-style.md) moves these settings into style rules; the effects stay.

## Problem

Milestone 06 adds auto zoom, click ripples, cursor size and smoothing, a keystroke overlay, window styling, device frames and code from git. Each needs a place: a per-video setting in `format.kdl`, a per-range node in `cut.kdl`, or an overlay template.

## Constraints

- Cursor effects change after recording without re-recording, so they are render settings over the event log.
- A product tour looks finished without keyframing.
- A setting enters a chunk's hash only where it changes pixels.

## Choice

- `format.kdl` gets five optional nodes: `auto-zoom` (triggers, scale, hold), `zoom-motion`, `cursor` (size, smoothing, ripple), `keystrokes` and `window` (padding, corners, shadow, background, frame). A missing node means its default, so existing files read as before. They are per video because `format.kdl` already is: copied from a preset, edited per video.
- `cut.kdl` gets what varies by time: `zoom` nodes with an optional center `x= y=` and provenance. Auto zoom writes them from the event log; "follows the cursor" is a run of centered zooms.
- Code from git is an overlay template. The app snapshots the diff into `sources/` and the template names the file, the line and the moment of the highlight. The template owns its parameters.
- In the chunk hash: cursor and keystrokes count where the layout shows the screen, the window style in the `window` layout, zoom motion and centers with the zooms. The take's media identity covers `events.jsonl`.

## Rejected alternatives

- Per-range cursor and window nodes in `cut.kdl`: nothing in the first release varies them within a video.
- Burning the cursor at capture: the briefing rules it out, and every setting would need a re-record.
- A `follow=cursor` zoom: a renderer that reads events to pan is harder to make preview-equal than explicit centers.
- Commit ranges in the overlay: tags move and repos are machine-specific, so the hash could not cover the pixels.

## Consequences

`shot-render` draws the cursor from events with the format's cursor style, adds the window pass and the keystroke overlay, and includes `events.jsonl` in the media identity. Cursor and keystroke settings re-render every screen chunk when changed; window settings only window chunks. The Product demo screen edits `format.kdl` and the zoom nodes.

## Affected contracts

[Project files](../contracts/project-files.md#product-video-settings), `shot-render`, the Product demo screen in `shot-app`.
