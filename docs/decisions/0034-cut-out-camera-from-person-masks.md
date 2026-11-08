# 0034: The camera cuts the person out with cached Vision masks

Status: Proposed in milestone 14, stream E.

## Problem

FilmCraft and EffectCraft draw a talking head without its room: the person stands over the screen or the brand's background. Shot Studio's camera is always a framed rectangle. Mike wants the cut-out as a brand setting a video can override, drawn the same in preview and export, and never a broken frame when the mask fails.

## Choice

- **`cutout=<bool>` on the brand's `camera` node**, off by default, like `handheld` in milestone 13: a format layer can write `camera cutout=#true` alone, and the value goes into `cut.lock` with the rest of the resolved brand. The theme editor's Camera section and the Picture inspector's Camera section each have a switch.
- **Masks are analysis, cached beside the take**: `shot analyze` (which the app runs after each take, and `shot migrate` runs too) computes them when the resolved brand has `cutout` on and the take has a camera. They go to `.shot/cache/masks/<take>.bin`, keyed to the size and modification time of `camera.mp4` like the face and text caches, so a new recording is read again. Rendering never runs Vision.
- **Vision's person segmentation at the balanced level, about 30 masks a second, stored 256 pixels wide**, in a sequence request handler so the mask stays steady between frames. Measured on the 12 s, 1080p60 stand-in camera on an M1 Pro: about 38 s of work per minute of camera. The accurate level returned 2016x1512 masks and took 4.5 times as long as balanced on the same frames, detail that 256 pixels throw away. The fast level was a third quicker but returns 256x192, about the stored size, with no margin for the resample. A 60 fps camera shows the mask of the frame before, at most 17 ms old.
- **A confidence per frame**: Vision's own, which is 1 in practice, or 0 when the person covers less than 2 percent of the frame. A frame below half confidence shows the plain camera; within 200 ms of one the cut-out crossfades to it, so a lost mask fades instead of popping.
- **One draw in the shared render graph**: the mask is a one-channel texture that the camera layer samples at its own texture coordinates, so the hand-held drift and zooms move it with the picture. Bilinear scaling to the slot is the feather. The frame's corners, border and shadow fade out as far as the cut-out fades in. Preview, the record monitor and export all draw through the same `compose` and `Renderer`, and a chunk's key includes the mask file's size and modification time.

## Rejected alternatives

- **Running Vision while rendering**: the preview would stutter, and the per-chunk parallel export would compute the same masks several times.
- **Masks as PNG or video files**: a lossy codec blurs the edge, and thousands of small files are slow to open. Run-length bytes keep a 30 s take near 300 KB with empty masks; a real person's masks are estimated at a few KB a frame.
- **A per-video setting outside the brand**: the cascade already carries per-video brand overrides, and `handheld` set the precedent.
- **Matting with an outline or blur of the background**: more look options than asked for. The cut-out is the feature; a styled outline can come later as a separate brand field.

## Consequences

Turning `cutout` on for a video whose takes were analyzed before shows the plain camera until `shot analyze` runs again; the inspector says so. Masks cost disk and time only for brands that use them. Vision is tuned on real people: the fixtures' blocky stand-in face is never found, so the golden tests write the masks the stand-in would have.

## Affected contracts

[Project files](../contracts/project-files.md#brandkdl) (`camera cutout=`, a format layer example, the masks cache), `shot-core`, `shot-analyze`, `shot-render`, `shot-cli`, `shot-ship` (the thumbnail studio uses the same segmenter), `shot-app`.
