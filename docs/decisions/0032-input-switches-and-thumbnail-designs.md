# 0032: Input switches live in the format, thumbnail designs in their own file

Status: Proposed in milestone 13, stream G.

## Problem

Mike wants to switch the screen, camera, mic and system audio on or off before a take, and to design thumbnails in the app. Both need a place in the video folder. The switches must be remembered, and a talking-head format records no screen on every video, so a video-only switch would be set again each time. A thumbnail design has variants, a frame, text and stickers, and the app never stores edit state outside the project files.

## Choice

- **`record` in `format.kdl`**: `record screen=<bool> camera=<bool> mic=<bool> system-audio=<bool>`, an optional node like the product video settings. It cascades: a channel format can turn the screen off for every video, and a video's `format.kdl` overrides it. The Record screen writes it through the folder like the prompter mode, so it undoes with ⌘Z. It changes only what the next take captures.
- **Readers assume no track**: without `camera.mp4` a layout draws content only, without `screen.mp4` the face only (`Layout::for_tracks`), without `mic.mp4` the voice is silence and the take has no words. A take folder that exists without `mic.mp4` loads as an empty transcript, so no screen asks to transcribe it and the gap rules never read it as one long silence. A missing take folder says nothing: media stays out of git.
- **`thumbnail.kdl` next to `cut.kdl`**: variants of a 1280x720 design, each a preset, a frame (take and media time), backdrop, face style, headline and stickers, and `upload` naming the one Ship uploads. It is committed like the rest of the content; the rendered `main.png` stays in `.shot/export/`.
- **One drawing for canvas and upload**: the studio draws with `shot-render`'s vector pass, the same code and fonts as every frame, and exports the upload variant's 1280x720 render as `main.png`.
- **The face is cut out with Vision person segmentation** at the accurate quality, once per frame picked, on a worker thread so the canvas never waits. A frame without a person shows the face framed instead.

## Rejected alternatives

- **Switches per video in app state outside the project files**: against "no hidden edit state", and every talking-head video would switch the screen off again.
- **Switches on the machine, like the prompter settings**: what a video records is a choice about the video, not the desk.
- **The design in `cut.kdl`**: the cut is the edit's content; thumbnails change on their own schedule, often after shipping.
- **A separate image pipeline (Core Graphics or an image crate) for thumbnails**: the canvas would not match the export, and brand fonts would draw twice.

## Consequences

The format's `record` node joins the cascade's settings, so `shot migrate` and the style layer diff carry it. `shot transcribe` and `shot autocut` on a take recorded without a mic say so and stop. Ship uploads whichever `main.png` was written last: a picked candidate or the design's upload variant.

## Affected contracts

[Project files](../contracts/project-files.md#formatkdl) (`record`, the take folder, [`thumbnail.kdl`](../contracts/project-files.md#thumbnailkdl), ship artifacts), `shot-core`, `shot-capture`, `shot-render`, `shot-ship`, `shot-app`.
