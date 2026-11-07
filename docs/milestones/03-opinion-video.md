# Milestone 03: A usable opinion video

## Outcome

Mike can record an opinion video with separate screen, camera, mic and system audio tracks, see the transcript appear while he talks, edit by deleting words, switch layouts and export a 4K video. This alone replaces the ScreenFlow plus DaVinci workflow for opinion videos.

## Dependencies and ownership

Requires 01 and 02. Media owns `shot-capture`, `shot-media`, `shot-render`. Analysis owns the streaming transcript in `shot-analyze`. Core owns timeline evaluation in `shot-core`. App owns the Record and Edit screens. Agents owns `shot record` and `shot export` in `shot-cli`.

## Work

1. Capture a session with ScreenCaptureKit and AVFoundation into separate fragmented MP4 tracks plus a cursor and key event log, with HEVC at a 0.5 s GOP. A killed process keeps everything up to the last fragment. Switch the fixture generator to the same 0.5 s GOP and regenerate.
2. Download the whisper model on first use with progress and checksum check, and `shot models fetch` for the CLI.
3. Stream the mic track into whisper during recording and store word timestamps per take in the cache.
4. Evaluate `cut.kdl` into a timeline of segments. Deleting words in the transcript writes removed ranges to `cut.kdl`.
5. Layouts face, face + content, split and content, switched by range and drawn by the render graph in preview and export.
6. Export: render chapters in parallel with hardware encode, cache segments by content hash, join by stream copy. Re-export renders only segments whose hash changed.
7. Reuse the spike's `shot-media` decoder, player and frame pacer and the IOSurface zoom pass. Find the cause of the missed refreshes from the spike on an idle machine, and fix the BT.601 color conversion for BT.709 sources.
8. Build the Record and Edit screens to the mockup and [design](../design.md), with the timeline, waveform and transcript elements.

## Acceptance criteria

- On the 17 min fixture project, the transcript of the last take is ready within 5 s of stop.
- A word deleted in the transcript is gone from the preview and the export, and `cut.kdl` shows the change as a small diff.
- Full 4K60 export of the 17 min project is faster than realtime. Changing one chapter and exporting again takes under 20 s.
- Five runs of 60 s of 4K60 preview playback drop at most one frame in total, and seek p95 stays under 100 ms.
- Killing the app mid-recording loses at most the last fragment.
- Record and Edit screens match the mockup at 1600x1000 in dark.

## Evidence and handoff

Report in `docs/evidence/m03-opinion-video.md` with the timings, a `cut.kdl` diff, screenshots in `docs/evidence/m03/` and an exported clip checksum. Consumed by 04, 05, 06.
