# 0017: Derived cuts are drawn from the main cut's chunks

Status: Adopted by Mike on 2026-11-08.

## Problem

A Short (9:16) and an X clip (1:1) are a few lines in `cut.kdl` on top of the main cut, and milestone 07 asks them to reuse the main video's rendered segments where the pixels match. A derived frame at another aspect never equals a main frame, so a derived chunk can never be a main chunk. Something has to decide what "where the pixels match" reuses, where reframing on the face happens, and what a derived chunk's cache key covers.

## Constraints

- The contract says a derived cut uses the main cut's layouts, zooms and overlays.
- The cache is plain files keyed by content ([0011](0011-cache-as-plain-files.md)); deleting it loses nothing.
- A derived cut burns its captions in, and the caption text depends on the transcript, which is cached analysis, not a project file.
- Rendering the 17 min main cut takes minutes; a Short of it should cost seconds when the main cut is already exported.

## Choice

- A derived frame is a window of the main frame at the same moment: as tall as the main frame (2160 px), as wide as the aspect allows (1215 px for 9:16, 2160 for 1:1), scaled to 1080x1920 or 1080x1080, with the captions drawn over it.
- Export renders a derived cut in two passes. The first pass renders the main chunks the derived cut needs that are not cached yet, keyed and drawn exactly as the main export does, so they are the main export's chunks. The second pass decodes each base chunk, places the window and draws the captions. "Reuse" is the base chunks found in the cache; the CLI prints the counts.
- A derived chunk's key is its own contract hash (aspect, caption style and fixes, take, frames, layout, zooms, overlays) extended with the base chunk's key, the frames it uses, the window position per frame and the caption shown per frame with its highlighted word. That covers the transcript and the face track without hashing either file.
- A Short follows the face. Vision finds the largest face in the camera track every 0.5 s, on the keyframe grid, cached in `.shot/cache/faces/<take>.json` with the track's size and modification time. The layout of the moment (with its zoom) places the face on the main canvas; the window centers on the face position averaged over ±1 s and is moved only as far as needed to keep the whole face inside. Without a face or without the camera in the layout, the window stays centered. An X clip stays centered.
- Captions are the main cut's cues (04's timing) placed on the derived output's timeline and drawn by the vector pass: a rounded box with the spoken word on a lime pill for Word pop.

## Rejected alternatives

- **A vertical layout drawn from the sources** (camera on top, screen below): looks more like a native Short, but shares no pixels with the main cut, so nothing could be reused, and every layout, zoom and overlay would need a second design for 9:16.
- **Cropping the encoded main export**: needs the whole main export first, and changing one word would re-crop everything.
- **Face track in `cut.kdl`**: it is analysis of media, so it belongs in the cache; a person who wants another framing can still add a `layout` change.

## Consequences

- A main export after a Short reuses the base chunks the Short rendered, and the other way round.
- Overlays placed for 16:9 can be cut off at the window's edges. Overlay templates that should show in a Short need to sit inside the middle 1215 px, or a derived cut needs its own overlays later.
- Each derived frame is a second encode of a main frame. At VideoToolbox quality 0.75 and a 2:1 downscale the loss is not visible on the fixtures.
- A derived chunk ends where its clip ends, so a clip edge costs one short chunk, as a word deletion does in the main cut.
- `shot-analyze` depends on `shot-media` (decoding for Vision) and `objc2-vision`.

## Affected contracts

[Project files](../contracts/project-files.md#cutkdl) (derived cuts and timeline evaluation), `shot-core` (`ContentHash::extend`), `shot-render` (`derived`, `export`), `shot-analyze` (`vision`), `shot-cli` (`shot derived`, `shot export --cut`).
