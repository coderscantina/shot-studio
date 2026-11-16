# 0039: An ingested clip is a take

Status: Proposed. Mike asked on 2026-11-16 to "ingest external media e.g. from a phone into the video", with transcription; the choices below are the agent's and wait for him.

## Problem

Footage recorded outside Shot Studio, a phone clip or a second camera, can only go into `sources/` as b-roll. B-roll has no words: it is not transcribed, captioned or cut by the style rules, and the Takes view cannot line it up with the script's beats.

## Constraints

- One kind of video ([0035](0035-every-video-on-style-rules.md)): every take is cut and styled by the same rules from its elements.
- A take folder has the contract's shape: fragmented MP4, keyframe every 0.5 s, no B-frames, AAC 48 kHz mono for the mic. Export joins chunks on that grid by stream copy.
- No FFmpeg at run time; the app is one signed bundle. Decode and encode go through AVFoundation and VideoToolbox, as everywhere else.
- Phone clips are portrait or landscape by a track transform, often HDR (HLG, 10-bit), at 24 to 60 fps with a variable rate, with stereo AAC at 44.1 or 48 kHz.

## Choice

- `shot ingest <file> --video <dir>`, the app's Import clip on the Record screen and a file dropped on the Record screen or the Takes view make the clip the next take. Its picture becomes `camera.mp4` and its sound `mic.mp4`; it gets an empty `events.jsonl` and no `screen.mp4` or `system.mp4`.
- The picture is read through an AVFoundation video composition that applies the track transform, tone-maps HDR to SDR BT.709 8-bit and gives a constant rate: 30 fps for clips up to 30, 60 above, so frames sit on the 60 fps grid the editor counts. It is encoded with the hardware HEVC encoder at the upright size, keyframe every 0.5 s, and muxed fragmented like a recording. The sound is mixed to mono, resampled to 48 kHz and encoded as AAC.
- The take is appended to `cut.kdl` as its own chapter, as a recording is, then analysed: transcribed, its elements written, the lock rebuilt. Its rows in the Takes view come from aligning its words with the beats, as for any take recorded without the prompter.
- The source file is read, never moved or changed. A clip without a video track is refused, a clip without audio gets no `mic.mp4`.

## Rejected alternatives

- **B-roll with a transcript**: a second kind of material with words, outside the rules.
- **Copying the phone file as is**: variable rate, B-frames, HDR and rotation reach the renderer, which assumes the take shape.
- **FFmpeg**: not in the bundle, and a second decode path.
- **Ingesting as `screen.mp4`** (a phone screen recording): the screen track promises 60 fps and a cursor log. Left for later.

## Consequences

An ingested 4K HDR clip takes about its length again to transcode on the reference machine (measured in the milestone evidence), and its take folder holds a second copy of the footage. The camera of an ingested take is the only picture, so layouts that need a screen draw the camera alone, as for a recording without a screen.

## Affected contracts

[Project files](../contracts/project-files.md#take-folder) (a take may come from ingest), `shot-media`, `shot-capture`, `shot-cli`, `shot-app`, `scripts/fixtures.sh`.
