# Milestone 03 evidence: a usable opinion video

Recorded 2026-11-07 on `milestone/03`, closed after the closing wave. Machine: MacBook Pro, Apple M1 Pro, 32 GB, macOS 27.0.1, rustc 1.99.1. Fixtures regenerated with a 0.5 s GOP. Task reports are in [m03/](m03/).

## Result

| Criterion | Evidence | Result |
| --- | --- | --- |
| Transcript of the last take ready within 5 s of stop | Fed at recording speed, the 17 min take kept up and flushed in 0.03 s. Clips that end mid-speech flushed in 1.25 to 1.96 s. A real 42 s take had its chapter in `cut.kdl` 2.3 s after stop. Word error rate 1.2 to 1.6 % on the 17 min take. [transcript](m03/transcript.md), [ui](m03/ui.md) | Pass |
| A deleted word is gone from preview and export, `cut.kdl` shows a small diff | Deleting words in the app gave a 3-line diff and restore put the file back byte for byte. The export after a one-word delete maps frames exactly to the plan. [edit](m03/edit.md), [ui](m03/ui.md), [export-incremental](m03/export-incremental.md) | Pass |
| Full export faster than realtime, one chapter re-export under 20 s ([0012](../decisions/0012-export-and-preview-targets-from-measurement.md)) | Full export 588.5 s for 1020 s of source at about 97 fps, 362 MB peak. A one-word delete re-exports in 6.2 to 7.2 s and renders 2 chunks. Clean decode, continuous timestamps, audio sync unchanged by the join. [export](m03/export.md), [export-incremental](m03/export-incremental.md) | Pass |
| Five runs of 60 s drop at most one frame in total, seek p95 under 100 ms | `shot-app --bench $SHOT_FIXTURES/screen-30s.mp4` after all agents finished: 1 drop in 300 s (limit 1), seek p95 66.7 ms over 300 seeks, 69 MB peak footprint. Colors within 2 values per channel of BT.709. [preview](m03/preview.md) | Pass |
| Killing the app mid-recording loses at most the last fragment | `kill -9` at 6.25 s kept 6.0 s of playable screen and system audio. [capture](m03/capture.md) | Pass |
| Record and Edit screens match the mockup at 1600x1000 in dark | Screenshots of Record and the three Edit workspaces in `m03/` (git-ignored). Differences are listed in [ui](m03/ui.md): Record has no live screen preview yet, the selection toolbar sits under the paragraph, the Words lane packs into two rows | Pass. The closing wave added the live screen and camera preview ([recordtap](m03/recordtap.md)). Remaining differences from the mockup are listed there and in [ui](m03/ui.md) |

## Closing wave

- Record shows the screen and camera live while recording, for about 4 points of CPU. The take's `screen.mp4` keeps a gapless 60 fps grid. [recordtap](m03/recordtap.md)
- Preview colors equal export: p95 within 2 values per channel over all four layouts. [audio](m03/audio.md)
- The preview plays the edited audio from the same reader and 5 ms fades as export. Sound lands 10 to 15 ms into the shown frame at the median. [audio](m03/audio.md)
- New takes go into `cut.kdl` through `Cut::append_take`.
- The black preview of new takes on this machine comes from the camera ("USB3 Video" with the lid closed), not from the decoder.

## Not verified

- A long real take that ends mid-sentence, for the flush time.
- Camera and mic of a real recording through the keyframe index fast path. Capture files fall back to a full scan if they differ.
- Power loss mid-recording. Fragments are not fsynced.
- 4K and 120 Hz displays.
- Model download in the app, and real permission errors (everything was already granted).
- Anything by ear: clicks at cuts and at pause, and the acoustic A/V offset end to end. System audio capture recorded silence at the machine's muted volume.
- A lit camera image. The only camera here delivered near-black frames.
