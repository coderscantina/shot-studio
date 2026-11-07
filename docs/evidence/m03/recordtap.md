# M03 evidence: live screen preview on Record, `Cut::append_take`

Checked on 2026-11-07 on the reference MacBook Pro (M1 Pro, 32 GB, macOS 27), release build, other agents building at the same time.

## What it does

- `shot-capture`: `Session::preview()` returns a cloneable `Preview` with `screen()` and `camera()`, each the newest `PreviewFrame { buffer, sequence }`. The capture queues only store the raw `420v` frame in a slot. A converter thread per `Preview` wakes at 60 Hz, takes the newest frame of each source and runs a VideoToolbox pixel transfer into pooled IOSurface `420f` buffers with the BT.601 matrix gpui's surface shader assumes, scaled to at most 1920x1080 (screen) or 640x360 (camera). A slow preview skips frames. The recording path has no extra encode and no copy.
- Record screen: while recording, the stage shows the screen at 80% of its width with the camera as a rounded square in the bottom right corner (26% wide, 3% from the right, 5% from the bottom), as in the Record mockup. The screen redraws on `request_animation_frame`, so it follows the display's refresh. Before and after recording the stage keeps its text.
- `shot-core`: `Cut::append_take(take, title, range)` appends a chapter with one take block that keeps `range`. `VideoFolder::append_take` uses it instead of building the chapter itself.

## Recording cost (20 s takes, app process, share of one core, 7 samples of 2 s each)

| Run | Without preview | With preview |
| --- | --- | --- |
| 1 | 28.2% | 31.9% |
| 2 | 27.4% | 32.3% |

The preview adds about 4 points, including the 60 fps redraw of the app. The take's `screen.mp4` (2560x1440) stays at 60 fps with and without it: 1139, 1136 and 1129 frames in 18.8 to 19.0 s, and every packet step is exactly 1/60 s (ffprobe), so there are no gaps in the grid. The screen was mostly still, so ScreenCaptureKit delivered few new frames; the camera (30 fps) was dark.

## Screenshot

`record-recording.png` in this folder (git-ignored), 1600x1000 in dark, while recording.

## Differences from the mockup

- gpui's surface element has no corner radius yet (a TODO in gpui), so the screen and the camera square have square corners.
- No "Inline retake caught" note, layout switcher or hotkey row (later milestones).

## Not verified

- A 4K or 120 Hz display, or a camera with light on it.
- Colors against export numerically; the preview looked right by eye.
