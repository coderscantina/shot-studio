# Milestone 02: Preview spike

## Outcome

A gpui window plays and scrubs a 4K60 HEVC fixture through VideoToolbox into IOSurface, drawn as a Metal texture with no CPU copy. This decides whether gpui stays the UI, see [0002](../decisions/0002-gpui-and-zero-copy-preview.md).

## Dependencies and ownership

Requires 01. Media owns the decode path in `crates/shot-media`. App owns the gpui surface in `crates/shot-app`.

## Work

1. Decode a 4K60 HEVC fixture with VideoToolbox into IOSurface-backed `CVPixelBuffer`s.
2. Wrap the IOSurface as a Metal texture and draw it inside a gpui element, without reading pixels back to the CPU.
3. Play at 60 fps and scrub with the mouse: seek to the nearest keyframe and decode forward.
4. Draw one wgpu pass over the frame (a punch-in zoom) to prove the render graph can share the texture.
5. Measure dropped frames, decode latency per seek, CPU and memory, in a release build on the reference machine.

## Acceptance criteria

- Playback of the 4K60 fixture for 60 s drops no frames.
- Random seeks show a frame in under 100 ms.
- A profile shows no CPU copy of frame pixels between decode and display.
- App memory stays under 2 GB while playing.

## Evidence and handoff

Report in `docs/evidence/m02-preview-spike.md` with the measurements, Instruments traces summary and a screen recording. If any criterion fails, write the result and the alternatives that keep the targets, mark 03 gated, and bring it to Mike. On pass, set [0002](../decisions/0002-gpui-and-zero-copy-preview.md) to confirmed by measurement. Consumed by 03.
